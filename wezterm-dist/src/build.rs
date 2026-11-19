//! Building the programs, and checking what was built before it is archived.

use crate::layout::PROGRAMS;
use crate::Result;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The cargo packages whose binaries are [`PROGRAMS`].
const PACKAGES: [&str; 4] = [
    "wezterm",
    "wezterm-gui",
    "wezterm-mux-server",
    "strip-ansi-escapes",
];

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
    if numbers.len() != 3
        || !numbers
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    {
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
        ["/registry/", "/git/checkouts/"].iter().find_map(|marker| {
            manifest
                .find(marker)
                .map(|at| PathBuf::from(&manifest[..at]))
        })
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

/// The target rustc builds for when given none: this machine's, which the programs are built for.
fn rustc_host() -> Result<String> {
    let out = run(Command::new("rustc").arg("-vV"))?;
    String::from_utf8_lossy(&out)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(|h| h.trim().to_owned())
        .ok_or_else(|| "rustc -vV names no host".to_owned())
}

/// The flags that link the programs for `target`, with the toolchain at `sysroot`. A Linux build
/// links with the rust-lld that toolchain ships, through cc as rustc links when it picks rust-lld
/// itself. rustc picks it for x86_64 Linux and not for arm64 Linux, where it links with GNU ld,
/// and the two linkers differ in what the programs then need of glibc: std references
/// `pidfd_spawnp` and `pidfd_getpid` (glibc 2.39) weakly, and runs without them where glibc has
/// none. rust-lld makes a version needed only by weak references a weak need, which an older
/// glibc's loader passes over; GNU ld makes it a hard need, and that loader refuses to start the
/// program.
fn link_flags(target: &str, sysroot: &Path) -> Result<Vec<String>> {
    if !target.ends_with("-linux-gnu") {
        return Ok(Vec::new());
    }
    // Where rustc looks for its own linker: among the host's tools, and `target` is the host.
    let gcc_ld = sysroot
        .join("lib")
        .join("rustlib")
        .join(target)
        .join("bin")
        .join("gcc-ld");
    let lld = gcc_ld.join("ld.lld");
    if !lld.is_file() {
        return Err(format!(
            "{} is not there: a Linux release links with the rust-lld its toolchain ships, and the toolchain at {} ships none",
            lld.display(),
            sysroot.display()
        ));
    }
    Ok(vec![
        "-Clink-arg=-fuse-ld=lld".to_owned(),
        format!("-Clink-arg=-B{}", gcc_ld.display()),
    ])
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
    let link = link_flags(&rustc_host()?, &sysroot)?;
    let mut pairs: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.parent().is_some())
    {
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

    let rustflags: Vec<String> = std::iter::once(remaps(&pairs)).chain(link).collect();

    let mut cmd = Command::new("cargo");
    cmd.current_dir(repo)
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags.join("\u{1f}"))
        .args([
            "build",
            "--locked",
            "--release",
            "--message-format=json-render-diagnostics",
        ]);
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
            return Err(format!(
                "cargo built no {}: its messages name no such executable",
                program
            ));
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
    std::fs::create_dir_all(into)
        .map_err(|e| format!("cannot create {}: {}", into.display(), e))?;
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
        assert_eq!(
            executable(&artifact("wezterm", "lib", "/b/libwezterm.rlib")).unwrap(),
            None
        );
        assert_eq!(
            executable(&artifact("sync-color-schemes", "bin", "/b/x")).unwrap(),
            None
        );
        assert_eq!(executable("   Compiling wezterm v0.1.0").unwrap(), None);
    }

    #[test]
    fn a_program_carrying_a_build_machine_path_is_caught() {
        let paths = [PathBuf::from("/home/someone")];
        assert_eq!(
            carried(b"x /home/someone/src/lib.rs", &paths),
            Some(&paths[0])
        );
        assert_eq!(carried(b"x /wsb-wezterm/src/lib.rs", &paths), None);
    }

    #[test]
    fn a_release_is_named_by_version_txt_in_the_published_form() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("version.txt"), "1.51.3451\n").unwrap();
        assert_eq!(version(dir.path()).unwrap(), "1.51.3451");
        std::fs::write(dir.path().join("version.txt"), "20261117-120000-dc780c05\n").unwrap();
        assert!(version(dir.path())
            .unwrap_err()
            .contains("not the <major>.<minor>.<patch>"));
    }

    /// The program is held to its line and its version.
    #[cfg(unix)]
    #[test]
    fn wsb_wezterm_is_held_to_its_line_and_its_version() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("wsb-wezterm");
        let line = "https://r.example release prod a,b,c d,e,f";
        std::fs::write(
            &program,
            format!("#!/bin/sh\n# {}\necho 'wsb-wezterm 1.2.3'\n", line),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let built = Built {
            programs: vec![("wsb-wezterm", program.clone())],
            machine_paths: vec![],
        };
        assert!(check(&built, "1.2.3", line).is_ok());
        assert!(check(&built, "1.2.4", line)
            .unwrap_err()
            .contains("says \"wsb-wezterm 1.2.3\""));
        assert!(check(
            &built,
            "1.2.3",
            "https://r.example nightly prod a,b,c d,e,f"
        )
        .unwrap_err()
        .contains("without the release line"));
    }

    /// The version needs of the ELF program at `path`, as `readelf` reads them: each version of a
    /// library the program needs, and whether that need is weak.
    #[cfg(target_os = "linux")]
    fn version_needs(path: &Path) -> Vec<(String, String, bool)> {
        let out = run(Command::new("readelf")
            .args(["--version-info", "--wide"])
            .arg(path))
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let section = text
            .split("Version needs section")
            .nth(1)
            .unwrap_or_else(|| panic!("{} has no version needs:\n{}", path.display(), text));
        let mut file = String::new();
        let mut needs = Vec::new();
        for line in section.lines().skip(1) {
            if line.trim().is_empty() {
                break;
            }
            let field = |name: &str| {
                line.split(name)
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(str::to_owned)
            };
            if let Some(f) = field("File: ") {
                file = f;
            } else if let (Some(name), Some(flags)) = (field("Name: "), field("Flags: ")) {
                needs.push((
                    file.clone(),
                    name,
                    flags.split('|').any(|f| f.trim() == "WEAK"),
                ));
            }
        }
        needs
    }

    /// A Linux release's programs need glibc's newer versions weakly where they reference them
    /// weakly, linked as rustc links on an arm64 build machine.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_release_needs_weakly_referenced_glibc_versions_weakly() {
        let host = rustc_host().unwrap();
        let sysroot = PathBuf::from(
            String::from_utf8_lossy(
                &run(Command::new("rustc").args(["--print", "sysroot"])).unwrap(),
            )
            .trim(),
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("probe.rs");
        // Starting a program takes std through pidfd_spawnp and pidfd_getpid, which it references
        // weakly and which came in glibc 2.39, after the floor a release keeps to.
        std::fs::write(
            &source,
            "fn main() { std::process::Command::new(\"true\").status().unwrap(); }\n",
        )
        .unwrap();
        let probe = dir.path().join("probe");
        let mut rustc = Command::new("rustc");
        rustc
            .args(["--edition", "2021", "-O"])
            .arg(&source)
            .arg("-o")
            .arg(&probe);
        // How rustc links an arm64 Linux program unless told otherwise, on any machine: through cc
        // and GNU ld. rustc for x86_64 Linux links with rust-lld by default.
        rustc.args(["-C", "linker-features=-lld"]);
        rustc.args(link_flags(&host, &sysroot).unwrap());
        run(&mut rustc).unwrap();

        let needs = version_needs(&probe);
        let pidfd = needs
            .iter()
            .find(|(file, name, _)| file == "libc.so.6" && name == "GLIBC_2.39")
            .unwrap_or_else(|| {
                panic!(
                    "the probe needs no GLIBC_2.39 of libc.so.6, so this machine's glibc is older than 2.39 and the test shows nothing on it: {:?}",
                    needs
                )
            });
        assert!(
            pidfd.2,
            "the probe needs libc.so.6's GLIBC_2.39 outright, which a glibc older than 2.39 refuses to start, though it references that version weakly alone: {:?}",
            needs
        );

        let bare = dir.path().join("sysroot");
        let lld = bare
            .join("lib")
            .join("rustlib")
            .join(&host)
            .join("bin")
            .join("gcc-ld")
            .join("ld.lld");
        let e = link_flags(&host, &bare).unwrap_err();
        assert!(e.contains(&lld.display().to_string()), "{}", e);
    }
}
