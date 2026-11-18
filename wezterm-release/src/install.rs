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
    upgrade_renaming(
        transport,
        line,
        platform,
        now,
        program,
        installed,
        &|from, to| std::fs::rename(from, to),
    )
}

/// [`upgrade_with`], moving the sets with `rename`.
pub(crate) fn upgrade_renaming(
    transport: &dyn Transport,
    line: &Line,
    platform: &str,
    now: u64,
    program: &Path,
    installed: &str,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
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
    let mut staging = Staging::new(directory)?;
    let unpacked = take(transport, &newest, staging.path())?;
    let aside = staging.path().join("previous");
    if let Err(failed) = put_in_place(&unpacked, &set, &aside, rename) {
        if failed.stranded {
            staging.keep();
        }
        return Err(failed.message);
    }
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
/// included, when the upgrade ends, however it ends, unless it is kept because the previous
/// set could not be moved back out of it.
struct Staging {
    path: PathBuf,
    kept: bool,
}

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
        Ok(Staging { path, kept: false })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Leaves the folder where it is when the upgrade ends: it holds the only copy of the
    /// previous set.
    fn keep(&mut self) {
        self.kept = true;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        if let Err(e) = std::fs::remove_dir_all(&self.path) {
            eprintln!(
                "wsb-wezterm: could not remove {}, which the upgrade downloaded into: {}",
                self.path.display(),
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

/// Why the new set is not in place, and whether the old one is still where it was moved aside
/// to (`stranded`), outside the place it is run from.
#[derive(Debug)]
struct NotInPlace {
    message: String,
    stranded: bool,
}

/// Moves the installed `set` aside to `aside` and `new` into its place, with `rename`: two
/// renames, the second undone by moving the old set back when it fails. An old set that cannot
/// be moved back either stays at `aside`, and the failure names it.
fn put_in_place(
    new: &Path,
    set: &Path,
    aside: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), NotInPlace> {
    rename(set, aside).map_err(|e| NotInPlace {
        message: format!(
            "cannot move {} aside to {}: {}",
            set.display(),
            aside.display(),
            e
        ),
        stranded: false,
    })?;
    if let Err(e) = rename(new, set) {
        let failed = format!(
            "cannot put the new wsb-wezterm in place at {}: {}",
            set.display(),
            e
        );
        return Err(match rename(aside, set) {
            Ok(()) => NotInPlace {
                message: format!("{}; the old wsb-wezterm is back in place", failed),
                stranded: false,
            },
            Err(back) => NotInPlace {
                message: format!(
                    "{}; and the old wsb-wezterm could not be moved back from {}: {}. It is kept there: \
                     move it back to {} by hand",
                    failed,
                    aside.display(),
                    back,
                    set.display()
                ),
                stranded: true,
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;

    /// An installed set, a new one beside it, and where the installed one goes aside, each with
    /// a file naming which it is.
    struct Sets {
        _dir: tempfile::TempDir,
        new: PathBuf,
        set: PathBuf,
        aside: PathBuf,
    }

    impl Sets {
        fn new() -> Sets {
            let dir = tempfile::tempdir().unwrap();
            let staging = dir.path().join(".wsb-wezterm-upgrade-1");
            let new = staging.join("unpacked").join("wsb-wezterm");
            let set = dir.path().join("wsb-wezterm");
            std::fs::create_dir_all(&new).unwrap();
            std::fs::create_dir_all(&set).unwrap();
            std::fs::write(new.join("which"), "new").unwrap();
            std::fs::write(set.join("which"), "old").unwrap();
            Sets {
                new,
                set,
                aside: staging.join("previous"),
                _dir: dir,
            }
        }

        fn which(folder: &Path) -> String {
            std::fs::read_to_string(folder.join("which")).unwrap()
        }
    }

    /// A rename that fails for each source in `failing` and renames anything else.
    fn renaming_but<'a>(
        failing: &'a [PathBuf],
        calls: &'a RefCell<Vec<PathBuf>>,
    ) -> impl Fn(&Path, &Path) -> io::Result<()> + 'a {
        move |from, to| {
            calls.borrow_mut().push(from.to_path_buf());
            if failing.iter().any(|failing| failing == from) {
                Err(io::Error::other("the rename was refused"))
            } else {
                std::fs::rename(from, to)
            }
        }
    }

    #[test]
    fn the_new_set_takes_the_installed_ones_place() {
        let sets = Sets::new();
        let calls = RefCell::new(Vec::new());
        put_in_place(
            &sets.new,
            &sets.set,
            &sets.aside,
            &renaming_but(&[], &calls),
        )
        .unwrap();
        assert_eq!(Sets::which(&sets.set), "new");
        assert_eq!(Sets::which(&sets.aside), "old");
    }

    #[test]
    fn a_new_set_that_cannot_be_put_in_place_puts_the_old_one_back() {
        let sets = Sets::new();
        let calls = RefCell::new(Vec::new());
        let failed = put_in_place(
            &sets.new,
            &sets.set,
            &sets.aside,
            &renaming_but(std::slice::from_ref(&sets.new), &calls),
        )
        .unwrap_err();
        assert!(!failed.stranded, "{}", failed.message);
        assert!(
            failed
                .message
                .contains("the old wsb-wezterm is back in place"),
            "{}",
            failed.message
        );
        assert_eq!(Sets::which(&sets.set), "old");
        assert!(!sets.aside.exists());
    }

    #[test]
    fn an_old_set_that_cannot_be_moved_back_is_left_where_it_is_and_named() {
        let sets = Sets::new();
        let calls = RefCell::new(Vec::new());
        let failed = put_in_place(
            &sets.new,
            &sets.set,
            &sets.aside,
            &renaming_but(&[sets.new.clone(), sets.aside.clone()], &calls),
        )
        .unwrap_err();
        assert!(failed.stranded, "{}", failed.message);
        assert!(
            failed.message.contains(&sets.aside.display().to_string()),
            "{}",
            failed.message
        );
        assert!(
            failed
                .message
                .contains(&format!("move it back to {} by hand", sets.set.display())),
            "{}",
            failed.message
        );
        assert_eq!(Sets::which(&sets.aside), "old");
        assert_eq!(
            *calls.borrow(),
            vec![sets.set.clone(), sets.new.clone(), sets.aside.clone()]
        );
    }

    #[test]
    fn a_staging_folder_is_removed_unless_it_holds_the_old_set() {
        let dir = tempfile::tempdir().unwrap();
        let removed = Staging::new(dir.path()).unwrap();
        let removed_path = removed.path().to_path_buf();
        std::fs::write(removed_path.join("download"), "x").unwrap();
        drop(removed);
        assert!(!removed_path.exists());

        let mut kept = Staging::new(dir.path()).unwrap();
        let kept_path = kept.path().to_path_buf();
        std::fs::create_dir(kept_path.join("previous")).unwrap();
        kept.keep();
        drop(kept);
        assert!(kept_path.join("previous").is_dir());
    }
}
