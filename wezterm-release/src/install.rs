//! `wsb-wezterm upgrade`: installs the newest release of the channel this wsb-wezterm was
//! published on, for the platform it runs on, in place of the installed one.
//!
//! The release is found and verified as the crate's top says: the registry's charter, then the
//! signed manifest, then the archive's size, SHA-256 and layout. Its `wsb-wezterm` must say it
//! is the version the manifest published before anything is replaced. What it replaces is the
//! whole installed set the running program is part of: the `wsb-wezterm/` folder unpacked from
//! a Linux archive, or `wsb-wezterm.app` on macOS. The set is moved aside and the new one moved
//! into its place, each in one rename, so its folder holds one whole release, the old or the
//! new. A wsb-wezterm a package manager installed is left to that manager ([`crate::package`]),
//! and one built from source has no release line to follow.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Line, Problem, Release, Transport};

/// What `wsb-wezterm upgrade` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The channel has published nothing for this platform yet.
    NothingPublished {
        channel: String,
        platform: String,
        installed: String,
    },
    /// The newest release is no later than the one running: nothing is installed, and never an
    /// older one.
    UpToDate {
        channel: String,
        platform: String,
        installed: String,
        newest: String,
    },
    /// The newest release was installed at `set`, in place of `from`.
    Installed {
        channel: String,
        platform: String,
        from: String,
        to: String,
        set: PathBuf,
    },
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::NothingPublished {
                channel,
                platform,
                installed,
            } => write!(
                f,
                "no wsb-wezterm is published on the {} channel for {} yet; wsb-wezterm {} stays",
                channel, platform, installed
            ),
            Outcome::UpToDate {
                channel,
                platform,
                installed,
                newest,
            } => write!(
                f,
                "wsb-wezterm {} is the newest release on the {} channel for {}; wsb-wezterm {} is up to date",
                newest, channel, platform, installed
            ),
            Outcome::Installed {
                channel,
                platform,
                from,
                to,
                set,
            } => write!(
                f,
                "wsb-wezterm {} is the newest release on the {} channel for {}: installed it at {} in place of \
                 wsb-wezterm {}. Programs of the old one keep running until they are restarted: quit wsb-wezterm \
                 and its servers, and start them again",
                to,
                channel,
                platform,
                set.display(),
                from
            ),
        }
    }
}

/// Upgrades the running wsb-wezterm, `installed` (its version), along the release line it was
/// built with, over HTTPS.
pub fn upgrade(installed: &str) -> Result<Outcome, String> {
    let line = crate::baked().ok_or(crate::NO_LINE)?;
    let platform = crate::platform().ok_or_else(|| {
        format!(
            "wsb-wezterm is published for no {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let running = std::env::current_exe()
        .map_err(|e| format!("cannot tell where this wsb-wezterm is: {}", e))?;
    let program = running
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {}", running.display(), e))?;
    upgrade_with(
        &crate::Https::new(),
        &line,
        platform,
        mg_srel::unix_now(),
        &program,
        installed,
    )
}

/// Upgrades the wsb-wezterm `installed` whose `wsb-wezterm` program is at `program` (the file
/// itself, resolved) along `line`, reached through `transport`, to the newest release for
/// `platform` at the time `now`.
pub fn upgrade_with(
    transport: &dyn Transport,
    line: &Line,
    platform: &str,
    now: u64,
    program: &Path,
    installed: &str,
) -> Result<Outcome, String> {
    let Some(newest) = crate::fetch_latest(transport, line, platform, now)? else {
        return Ok(Outcome::NothingPublished {
            channel: line.channel.clone(),
            platform: platform.to_string(),
            installed: installed.to_string(),
        });
    };
    if !crate::later(&newest.version, installed)? {
        return Ok(Outcome::UpToDate {
            channel: line.channel.clone(),
            platform: platform.to_string(),
            installed: installed.to_string(),
            newest: newest.version,
        });
    }
    if let Some(package) = crate::package::of(program)? {
        return Err(format!(
            "wsb-wezterm {} is the newest release on the {} channel for {}, and {}",
            newest.version,
            line.channel,
            platform,
            package.upgrade_with()
        ));
    }
    let set = installed_set(program, platform)?;
    let directory = set
        .parent()
        .ok_or_else(|| format!("{} is in no directory", set.display()))?;
    let staging = Staging::new(directory)?;
    let unpacked = take(transport, &newest, staging.path())?;
    put_in_place(&unpacked, &set, &staging.path().join("previous"))?;
    Ok(Outcome::Installed {
        channel: line.channel.clone(),
        platform: platform.to_string(),
        from: installed.to_string(),
        to: newest.version,
        set,
    })
}

/// The installed set `program` is part of: the folder or bundle a release archive unpacked to,
/// by the name it unpacked as. A program anywhere else was not installed from a release.
fn installed_set(program: &Path, platform: &str) -> Result<PathBuf, String> {
    let root = crate::archive_root(platform);
    let depth = crate::program_in_root(platform).split('/').count();
    let set = program.ancestors().nth(depth);
    match set {
        Some(set)
            if set.file_name().is_some_and(|name| name == root)
                && set.join(crate::program_in_root(platform)) == program =>
        {
            Ok(set.to_path_buf())
        }
        _ => Err(format!(
            "{} is not the wsb-wezterm of a release unpacked from its archive, which is {}/{}: install a \
             release by unpacking its archive",
            program.display(),
            root,
            crate::program_in_root(platform)
        )),
    }
}

/// A folder beside the installed set for one upgrade's download, on the same file system so
/// the sets move in one rename each; removed with everything in it, the previous set
/// included, when the upgrade ends, however it ends.
struct Staging(PathBuf);

impl Staging {
    fn new(directory: &Path) -> Result<Staging, String> {
        let path = directory.join(format!(".wsb-wezterm-upgrade-{}", std::process::id()));
        std::fs::create_dir(&path).map_err(|e| {
            format!(
                "cannot write in {}, where wsb-wezterm is installed: {}",
                directory.display(),
                e
            )
        })?;
        Ok(Staging(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "wsb-wezterm: could not remove {}, which the upgrade downloaded into: {}",
                self.0.display(),
                e
            );
        }
    }
}

/// Downloads and verifies `release`'s archive into `staging`, unpacks it, and answers the set
/// it holds, once its `wsb-wezterm` says it is the release's version.
fn take(transport: &dyn Transport, release: &Release, staging: &Path) -> Result<PathBuf, Problem> {
    let archive = crate::download(transport, release, staging)?;
    let root = crate::unpack(&archive, &release.platform, &staging.join("unpacked"))?;
    let inside = crate::program_in_root(&release.platform);
    let program = root.join(inside);
    if !program.is_file() {
        return Err(Problem::Refused {
            reason: "archive_layout",
            message: format!(
                "{} holds no {}/{}",
                release.url,
                crate::archive_root(&release.platform),
                inside
            ),
        });
    }
    let says = says_version(&program)?;
    let expected = format!("wsb-wezterm {}", release.version);
    if says != expected {
        return Err(Problem::Refused {
            reason: "archive_version",
            message: format!(
                "the wsb-wezterm in {} says {:?}, and the signed manifest publishes it as {}",
                release.url, says, expected
            ),
        });
    }
    Ok(root)
}

/// The first line `<program> --version` prints.
fn says_version(program: &Path) -> Result<String, Problem> {
    let output = Command::new(program)
        .arg("--version")
        .output()
        .map_err(|e| Problem::Failed(format!("cannot run {}: {}", program.display(), e)))?;
    if !output.status.success() {
        return Err(Problem::Failed(format!(
            "{} --version failed ({}): {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string())
}

/// Moves the installed `set` aside to `aside` and `new` into its place: two renames, the second
/// undone by moving the old set back when it fails.
fn put_in_place(new: &Path, set: &Path, aside: &Path) -> Result<(), String> {
    std::fs::rename(set, aside).map_err(|e| {
        format!(
            "cannot move {} aside to {}: {}",
            set.display(),
            aside.display(),
            e
        )
    })?;
    if let Err(e) = std::fs::rename(new, set) {
        let back = std::fs::rename(aside, set)
            .map(|()| "the old wsb-wezterm is back in place".to_string())
            .unwrap_or_else(|back| {
                format!(
                    "and the old wsb-wezterm could not be moved back from {}: {}",
                    aside.display(),
                    back
                )
            });
        return Err(format!(
            "cannot put the new wsb-wezterm in place at {}: {}; {}",
            set.display(),
            e,
            back
        ));
    }
    Ok(())
}
