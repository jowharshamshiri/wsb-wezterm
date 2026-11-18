//! What a release's installed set holds, platform by platform: each file's place in the set and
//! where it comes from, the repository or the build.

use crate::Result;
use std::path::{Path, PathBuf};

/// The programs a release carries, each the binary of the same name.
pub const PROGRAMS: [&str; 4] = [
    "wsb-wezterm",
    "wsb-wezterm-gui",
    "wsb-wezterm-mux-server",
    "wsb-wezterm-strip-ansi-escapes",
];

/// One file of the set: its path in the set, where it is taken from, and whether it is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub from: PathBuf,
    pub program: bool,
}

fn entry(name: &str, from: PathBuf, program: bool) -> Entry {
    Entry {
        name: name.to_string(),
        from,
        program,
    }
}

/// The repository's files the Linux set carries, by their place in it: what the system's own
/// places take from it when a package installs it (the desktop entry, the icon, the completions,
/// the shell integration, the terminfo source), all beside the programs.
pub const LINUX_FILES: &[(&str, &str, bool)] = &[
    ("open-wsb-wezterm-here", "assets/open-wsb-wezterm-here", true),
    ("LICENSE.md", "LICENSE.md", false),
    (
        "share/applications/com.bahramj.wsb-wezterm.desktop",
        "assets/wsb-wezterm.desktop",
        false,
    ),
    (
        "share/icons/hicolor/128x128/apps/com.bahramj.wsb-wezterm.png",
        "assets/icon/terminal.png",
        false,
    ),
    (
        "share/metainfo/com.bahramj.wsb-wezterm.appdata.xml",
        "assets/wsb-wezterm.appdata.xml",
        false,
    ),
    (
        "share/nautilus-python/extensions/wsb-wezterm-nautilus.py",
        "assets/wsb-wezterm-nautilus.py",
        false,
    ),
    (
        "share/bash-completion/completions/wsb-wezterm",
        "assets/shell-completion/bash",
        false,
    ),
    (
        "share/zsh/site-functions/_wsb-wezterm",
        "assets/shell-completion/zsh",
        false,
    ),
    (
        "share/fish/vendor_completions.d/wsb-wezterm.fish",
        "assets/shell-completion/fish",
        false,
    ),
    (
        "etc/profile.d/wsb-wezterm.sh",
        "assets/shell-integration/wsb-wezterm.sh",
        false,
    ),
    (
        "share/terminfo/wsb-wezterm.terminfo",
        "termwiz/data/wsb-wezterm.terminfo",
        false,
    ),
];

/// The repository's files the macOS bundle carries, by their place in `wsb-wezterm.app`.
pub const MACOS_FILES: &[(&str, &str, bool)] = &[
    ("Contents/Info.plist", "assets/macos/wsb-wezterm.app/Contents/Info.plist", false),
    (
        "Contents/Resources/terminal.icns",
        "assets/macos/wsb-wezterm.app/Contents/Resources/terminal.icns",
        false,
    ),
    (
        "Contents/Resources/wsb-wezterm.sh",
        "assets/shell-integration/wsb-wezterm.sh",
        false,
    ),
    ("Contents/Resources/shell-completion/bash", "assets/shell-completion/bash", false),
    ("Contents/Resources/shell-completion/zsh", "assets/shell-completion/zsh", false),
    ("Contents/Resources/shell-completion/fish", "assets/shell-completion/fish", false),
    ("Contents/Resources/LICENSE.md", "LICENSE.md", false),
];

/// Where the set holds `program`, for `platform`.
pub fn program_place(platform: &str, program: &str) -> String {
    match wezterm_release::program_in_root(platform).rsplit_once('/') {
        Some((programs, _)) => format!("{}/{}", programs, program),
        None => program.to_string(),
    }
}

/// The set of `platform`: the repository's files at `repo`, the built programs, and, on macOS,
/// the compiled terminfo (`terminfo`, a folder `tic` wrote, every file of it under
/// `Contents/Resources/terminfo/`). A file the repository lacks is refused, naming it.
pub fn set(
    platform: &str,
    repo: &Path,
    programs: &[(&'static str, PathBuf)],
    terminfo: Option<&Path>,
) -> Result<Vec<Entry>> {
    let files = if platform.starts_with("darwin-") {
        MACOS_FILES
    } else {
        LINUX_FILES
    };
    let mut entries = Vec::new();
    for (name, from, program) in files {
        let from = repo.join(from);
        if !from.is_file() {
            return Err(format!(
                "a release of wsb-wezterm for {} carries {}, and {} is not there",
                platform,
                name,
                from.display()
            ));
        }
        entries.push(entry(name, from, *program));
    }
    for (name, from) in programs {
        entries.push(entry(&program_place(platform, name), from.clone(), true));
    }
    if platform.starts_with("darwin-") {
        let terminfo = terminfo.ok_or("a macOS release carries the compiled terminfo, and none was made")?;
        for (relative, from) in files_under(terminfo)? {
            entries.push(entry(
                &format!("Contents/Resources/terminfo/{}", relative),
                from,
                false,
            ));
        }
    }
    Ok(entries)
}

/// Every file under `dir`, by its path relative to it, in name order.
fn files_under(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let listed = std::fs::read_dir(&folder)
            .map_err(|e| format!("cannot read {}: {}", folder.display(), e))?;
        for item in listed {
            let path = item
                .map_err(|e| format!("cannot read {}: {}", folder.display(), e))?
                .path();
            if path.is_dir() {
                folders.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(dir)
                    .expect("found under it")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, path));
            } else {
                return Err(format!(
                    "{} is neither a file nor a folder: a release carries none",
                    path.display()
                ));
            }
        }
    }
    found.sort();
    if found.is_empty() {
        return Err(format!("{} holds no compiled terminfo", dir.display()));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> PathBuf {
        crate::repository().unwrap()
    }

    fn built(dir: &Path) -> Vec<(&'static str, PathBuf)> {
        PROGRAMS
            .iter()
            .map(|p| {
                let at = dir.join(p);
                std::fs::write(&at, p).unwrap();
                (*p, at)
            })
            .collect()
    }

    /// The Linux set holds the programs, the launcher, the licence and what a package puts in
    /// the system's places, every one of them a file of the repository.
    #[test]
    fn the_linux_set_is_the_programs_and_the_repositorys_files() {
        let dir = tempfile::tempdir().unwrap();
        let entries = set("linux-x86_64", &repository(), &built(dir.path()), None).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        for program in PROGRAMS {
            assert!(names.contains(&program), "{}", program);
        }
        assert!(names.contains(&"share/applications/com.bahramj.wsb-wezterm.desktop"));
        assert!(names.contains(&"share/terminfo/wsb-wezterm.terminfo"));
        let programs: Vec<&str> = entries.iter().filter(|e| e.program).map(|e| e.name.as_str()).collect();
        assert_eq!(programs.len(), PROGRAMS.len() + 1, "{:?}", programs);
    }

    /// The macOS bundle holds the programs in Contents/MacOS, the bundle's own files and the
    /// compiled terminfo under Contents/Resources.
    #[test]
    fn the_macos_bundle_holds_its_programs_resources_and_terminfo() {
        let dir = tempfile::tempdir().unwrap();
        let terminfo = dir.path().join("terminfo");
        std::fs::create_dir_all(terminfo.join("77")).unwrap();
        std::fs::write(terminfo.join("77/wsb-wezterm"), "compiled").unwrap();
        let entries = set("darwin-arm64", &repository(), &built(dir.path()), Some(&terminfo)).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Contents/MacOS/wsb-wezterm"));
        assert!(names.contains(&"Contents/MacOS/wsb-wezterm-mux-server"));
        assert!(names.contains(&"Contents/Info.plist"));
        assert!(names.contains(&"Contents/Resources/terminfo/77/wsb-wezterm"));
        let e = set("darwin-arm64", &repository(), &built(dir.path()), None).unwrap_err();
        assert!(e.contains("compiled terminfo"), "{}", e);
    }

    /// A file of the repository that is missing stops the release, naming it.
    #[test]
    fn a_missing_file_of_the_repository_is_refused_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let e = set("linux-arm64", dir.path(), &[], None).unwrap_err();
        assert!(e.contains("carries open-wsb-wezterm-here"), "{}", e);
    }
}
