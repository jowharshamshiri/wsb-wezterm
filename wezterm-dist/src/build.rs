//! Building the programs, and checking what was built before it is archived.

use crate::layout::PROGRAMS;
use crate::Result;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The cargo packages whose binaries are [`PROGRAMS`].
const PACKAGES: [&str; 4] = ["wezterm", "wezterm-gui", "wezterm-mux-server", "strip-ansi-escapes"];

/// What a build produced, and the paths of the machine it was built on, which no program may
/// carry.
pub struct Built {
    pub programs: Vec<(&'static str, PathBuf)>,
    pub machine_paths: Vec<PathBuf>,
}

fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    let shown = format!("{:?}", cmd);
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run {}: {}", shown, e))?;
    if !out.status.success() {
        return Err(format!("{} failed ({})", shown, out.status));
    }
    Ok(out.stdout)
}

/// The version being released: the repository's version.txt, which a release build names
/// itself by (wezterm-version's build script).
pub fn version(repo: &Path) -> Result<String> {
    let file = repo.join("version.txt");
    let version = std::fs::read_to_string(&file)
        .map_err(|e| format!("cannot read {}: {}", file.display(), e))?
        .trim()
        .to_owned();
    let numbers: Vec<&str> = version.split('.').collect();
    if numbers.len() != 3 || !numbers.iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
        return Err(format!(
            "{} holds {:?}, not the <major>.<minor>.<patch> a release is published as",
            file.display(),
            version
        ));
    }
    Ok(version)
}

/// Cargo's home, as the dependency graph reads crates from it.
fn cargo_home(meta: &serde_json::Value) -> Option<PathBuf> {
    meta["packages"].as_array()?.iter().find_map(|p| {
        let manifest = p["manifest_path"].as_str()?;
        ["/registry/", "/git/checkouts/"]
            .iter()
            .find_map(|marker| manifest.find(marker).map(|at| PathBuf::from(&manifest[..at])))
    })
}

/// The flags that replace this machine's paths, wherever rustc writes one into a program, with
/// fixed ones; rustc applies the last prefix that matches, so the most specific comes last.
fn remaps(pairs: &[(PathBuf, &str)]) -> String {
    pairs
        .iter()
        .map(|(from, to)| format!("--remap-path-prefix={}={}", from.display(), to))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

/// Builds the programs for this machine's platform, given the release line in the
/// environment, which the build bakes into `wsb-wezterm`.
pub fn build(repo: &Path, macos_deployment_target: Option<&str>) -> Result<Built> {
    let meta: serde_json::Value = serde_json::from_slice(&run(Command::new("cargo")
        .current_dir(repo)
        .args(["metadata", "--format-version", "1", "--locked"]))?)
    .map_err(|e| format!("cargo metadata: {}", e))?;
    let target_dir = PathBuf::from(
        meta["target_directory"]
            .as_str()
            .ok_or("cargo metadata names no target directory")?,
    );
    let sysroot = PathBuf::from(
        String::from_utf8_lossy(&run(Command::new("rustc").args(["--print", "sysroot"]))?).trim(),
    );
    let mut pairs: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.parent().is_some()) {
        pairs.push((home, "/home"));
    }
    if let Some(home) = cargo_home(&meta) {
        pairs.push((home, "/cargo"));
    }
    pairs.extend([
        (repo.to_path_buf(), "/wsb-wezterm"),
        (target_dir, "/build"),
        (sysroot, "/rustc"),
    ]);

    let mut cmd = Command::new("cargo");
    cmd.current_dir(repo)
        .env("CARGO_ENCODED_RUSTFLAGS", remaps(&pairs))
        .args(["build", "--locked", "--release", "--message-format=json-render-diagnostics"]);
    for package in PACKAGES {
        cmd.args(["-p", package]);
    }
    if let Some(floor) = macos_deployment_target {
        cmd.env("MACOSX_DEPLOYMENT_TARGET", floor);
    }
    let shown = format!("{:?}", cmd);
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot run {}: {}", shown, e))?;
    let stdout = child.stdout.take().expect("piped");
    let mut programs: Vec<(&'static str, PathBuf)> = Vec::new();
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.map_err(|e| format!("reading cargo's messages: {}", e))?;
        if let Some((name, path)) = executable(&line)? {
            programs.retain(|(n, _)| *n != name);
            programs.push((name, path));
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("waiting for {}: {}", shown, e))?;
    if !status.success() {
        return Err(format!("{} failed ({})", shown, status));
    }
    for program in PROGRAMS {
        if !programs.iter().any(|(n, _)| *n == program) {
            return Err(format!("cargo built no {}: its messages name no such executable", program));
        }
    }
    Ok(Built {
        programs,
        machine_paths: pairs.into_iter().map(|(from, _)| from).collect(),
    })
}

/// The program a line of cargo's messages says was built, if it is one of [`PROGRAMS`].
fn executable(line: &str) -> Result<Option<(&'static str, PathBuf)>> {
    if !line.starts_with('{') {
        return Ok(None);
    }
    let message: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| format!("cargo wrote a message that is not JSON ({}): {}", e, line))?;
    let is_bin = message["target"]["kind"]
        .as_array()
        .is_some_and(|k| k.iter().any(|k| k == "bin"));
    if message["reason"] != "compiler-artifact" || !is_bin {
        return Ok(None);
    }
    let Some(name) = PROGRAMS.iter().find(|p| message["target"]["name"] == **p) else {
        return Ok(None);
    };
    let path = message["executable"]
        .as_str()
        .ok_or_else(|| format!("cargo built {} and names no executable for it", name))?;
    Ok(Some((name, PathBuf::from(path))))
}

/// Holds the programs to what a release is: none carries the build machine's paths,
/// `wsb-wezterm`, which upgrades the set, carries the release line it was given, and says it is
/// this version.
pub fn check(built: &Built, version: &str, line: &str) -> Result<()> {
    for (name, path) in &built.programs {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        if let Some(found) = carried(&bytes, &built.machine_paths) {
            return Err(format!(
                "{} ({}) carries the build machine's path {}: it was built without its paths remapped",
                name,
                path.display(),
                found.display()
            ));
        }
        if *name == "wsb-wezterm" {
            if !bytes.windows(line.len()).any(|w| w == line.as_bytes()) {
                return Err(format!(
                    "{} was built without the release line it was given ({})",
                    path.display(),
                    line
                ));
            }
            let says = says_version(path)?;
            if says != format!("wsb-wezterm {}", version) {
                return Err(format!(
                    "{} says {:?}, and the release is wsb-wezterm {}",
                    path.display(),
                    says,
                    version
                ));
            }
        }
    }
    Ok(())
}

/// The first of `paths`, followed by a separator, that `bytes` contains.
fn carried<'a>(bytes: &[u8], paths: &'a [PathBuf]) -> Option<&'a PathBuf> {
    paths.iter().find(|path| {
        let needle = format!("{}{}", path.display(), std::path::MAIN_SEPARATOR);
        bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
    })
}

/// The first line `<program> --version` prints, run with nothing of this machine's
/// environment but its path.
fn says_version(program: &Path) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.arg("--version").env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    let out = run(&mut cmd)?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned())
}

/// The terminfo entry compiled by the system's `tic` into a folder of its own, `into`, as a
/// macOS bundle carries it.
pub fn compile_terminfo(repo: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into).map_err(|e| format!("cannot create {}: {}", into.display(), e))?;
    run(Command::new("tic")
        .arg("-x")
        .arg("-o")
        .arg(into)
        .arg(repo.join("termwiz/data/wsb-wezterm.terminfo")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_programs_executables_are_taken_from_cargos_messages() {
        let artifact = |name: &str, kind: &str, exe: &str| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "target": { "name": name, "kind": [kind] },
                "executable": exe,
            })
            .to_string()
        };
        assert_eq!(
            executable(&artifact("wsb-wezterm-gui", "bin", "/b/wsb-wezterm-gui")).unwrap(),
            Some(("wsb-wezterm-gui", PathBuf::from("/b/wsb-wezterm-gui")))
        );
        assert_eq!(executable(&artifact("wezterm", "lib", "/b/libwezterm.rlib")).unwrap(), None);
        assert_eq!(executable(&artifact("sync-color-schemes", "bin", "/b/x")).unwrap(), None);
        assert_eq!(executable("   Compiling wezterm v0.1.0").unwrap(), None);
    }

    #[test]
    fn a_program_carrying_a_build_machine_path_is_caught() {
        let paths = [PathBuf::from("/home/someone")];
        assert_eq!(carried(b"x /home/someone/src/lib.rs", &paths), Some(&paths[0]));
        assert_eq!(carried(b"x /wsb-wezterm/src/lib.rs", &paths), None);
    }

    #[test]
    fn a_release_is_named_by_version_txt_in_the_published_form() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("version.txt"), "1.51.3451\n").unwrap();
        assert_eq!(version(dir.path()).unwrap(), "1.51.3451");
        std::fs::write(dir.path().join("version.txt"), "20261117-120000-dc780c05\n").unwrap();
        assert!(version(dir.path()).unwrap_err().contains("not the <major>.<minor>.<patch>"));
    }

    /// The program is held to its line and its version.
    #[cfg(unix)]
    #[test]
    fn wsb_wezterm_is_held_to_its_line_and_its_version() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("wsb-wezterm");
        let line = "https://r.example release prod a,b,c d,e,f";
        std::fs::write(&program, format!("#!/bin/sh\n# {}\necho 'wsb-wezterm 1.2.3'\n", line)).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let built = Built {
            programs: vec![("wsb-wezterm", program.clone())],
            machine_paths: vec![],
        };
        assert!(check(&built, "1.2.3", line).is_ok());
        assert!(check(&built, "1.2.4", line).unwrap_err().contains("says \"wsb-wezterm 1.2.3\""));
        assert!(check(&built, "1.2.3", "https://r.example nightly prod a,b,c d,e,f")
            .unwrap_err()
            .contains("without the release line"));
    }
}
