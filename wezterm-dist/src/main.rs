//! Builds wsb-wezterm's release archives.
//!
//! An archive is `wsb-wezterm-<version>-<platform>.tar.gz` and unpacks to the platform's
//! installed set: the folder `wsb-wezterm/` on Linux, holding the programs and what a package
//! puts in the system's places, and the bundle `wsb-wezterm.app/` on macOS.
//!
//! `wsb-wezterm-dist release --platform <platform> --out-dir <dir>` makes one archive, on a
//! machine of that platform, and prints its path. The build is given its release line, which
//! wezterm-release bakes into `wsb-wezterm`, and which names the build by version.txt; an
//! archive is never made without one.

// wezterm-release's build script's own reading of the release line: this tool reads the
// variables as it does, and leaves the baking, and the name it bakes into, to it.
#[allow(dead_code)]
#[path = "../../wezterm-release/release_line.rs"]
mod release_line;

mod archive;
mod build;
mod layout;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.first().map(String::as_str) {
        Some("release") => release_args(&args[1..])
            .and_then(|(platform, out_dir)| release(&platform, &out_dir)),
        _ => Err("usage: wsb-wezterm-dist release --platform <platform> --out-dir <dir>".into()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wsb-wezterm-dist: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn release_args(args: &[String]) -> Result<(String, PathBuf)> {
    let (mut platform, mut out_dir) = (None, None);
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.as_str() {
            "--platform" => &mut platform,
            "--out-dir" => &mut out_dir,
            other => return Err(format!("unknown argument {:?}", other)),
        };
        if slot.is_some() {
            return Err(format!("{} is given twice", flag));
        }
        *slot = Some(
            rest.next()
                .ok_or_else(|| format!("{} takes a value", flag))?
                .clone(),
        );
    }
    Ok((
        platform.ok_or("release takes --platform")?,
        PathBuf::from(out_dir.ok_or("release takes --out-dir")?),
    ))
}

/// The repository: the folder above this tool.
fn repository() -> Result<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::fs::canonicalize(&here)
        .map_err(|e| format!("cannot resolve the repository at {}: {}", here.display(), e))
}

/// The release line this build is given, as wezterm-release's build bakes it.
fn release_line(get: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    release_line::read(get)?.ok_or_else(|| {
        format!(
            "a release archive is built with its release line ({}), and {} is not set: \
             a wsb-wezterm without one cannot upgrade itself",
            release_line::INPUTS.join(", "),
            release_line::CHANNEL
        )
    })
}

/// Refuses a platform this machine is not, or one wsb-wezterm is not released for.
fn built_here(platform: &str) -> Result<()> {
    if !["linux-x86_64", "linux-arm64", "darwin-arm64"].contains(&platform) {
        return Err(format!(
            "{} is not a platform wsb-wezterm is released for (platforms: linux-x86_64, linux-arm64, darwin-arm64)",
            platform
        ));
    }
    match wezterm_release::platform() {
        Some(here) if here == platform => Ok(()),
        here => Err(format!(
            "{} is built on a {} machine, and this one is {}",
            platform,
            platform,
            here.unwrap_or("no platform wsb-wezterm is released for")
        )),
    }
}

fn release(platform: &str, out_dir: &Path) -> Result<()> {
    built_here(platform)?;
    let repo = repository()?;
    let version = build::version(&repo)?;
    // Before the build, which takes long: an archive without them is not made.
    let line = release_line(&|name| std::env::var(name).ok())?;
    let macos = platform.starts_with("darwin-");
    // macOS 11 is the first macOS on Apple silicon, the one macOS platform released.
    let built = build::build(&repo, macos.then_some("11.0"))?;
    build::check(&built, &version, &line)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {}", out_dir.display(), e))?;
    let terminfo = out_dir.join(format!(".terminfo-{}", std::process::id()));
    if macos {
        build::compile_terminfo(&repo, &terminfo)?;
    }
    let made = (|| {
        let entries = layout::set(platform, &repo, &built.programs, macos.then_some(terminfo.as_path()))?;
        let out = out_dir.join(archive::name(&version, platform));
        archive::write(&entries, platform, &out)?;
        Ok::<PathBuf, String>(out)
    })();
    if macos {
        std::fs::remove_dir_all(&terminfo)
            .map_err(|e| format!("cannot remove {}: {}", terminfo.display(), e))?;
    }
    println!("{}", made?.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_is_made_only_with_a_whole_release_line() {
        let e = release_line(&|_| None).unwrap_err();
        assert!(e.contains("is not set: a wsb-wezterm without one cannot upgrade itself"), "{}", e);
    }

    #[test]
    fn only_this_machines_released_platform_is_built() {
        let here = wezterm_release::platform().expect("the tests run on a released platform");
        assert!(built_here(here).is_ok());
        let e = built_here("windows-x86_64").unwrap_err();
        assert!(e.contains("not a platform wsb-wezterm is released for"), "{}", e);
        for other in ["linux-x86_64", "linux-arm64", "darwin-arm64"].iter().filter(|p| **p != here) {
            assert!(built_here(other).unwrap_err().contains(here), "{}", other);
        }
    }

    #[test]
    fn release_takes_a_platform_and_an_out_dir_once_each() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            release_args(&args(&["--platform", "linux-x86_64", "--out-dir", "o"])).unwrap(),
            ("linux-x86_64".to_string(), PathBuf::from("o"))
        );
        assert_eq!(release_args(&args(&["--out-dir", "o"])).unwrap_err(), "release takes --platform");
    }
}
