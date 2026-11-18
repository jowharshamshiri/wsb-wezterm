//! The archive: one `.tar.gz` per platform, unpacking to the platform's installed set,
//! `wsb-wezterm/` or `wsb-wezterm.app/`.
//!
//! The archive is written entry by entry from the files the set names, never by archiving a
//! folder: it holds exactly the set's folders and files, with fixed modes and times and no
//! owner, so no file a machine keeps beside them (macOS's `._` AppleDouble files and
//! `.DS_Store`) and none of their extended attributes can reach a release.

use crate::layout::Entry;
use crate::Result;
use std::path::Path;

pub fn name(version: &str, platform: &str) -> String {
    wezterm_release::archive_name(version, platform)
}

/// Writes the archive of `entries` for `platform` to `out`: the set's one directory, each
/// folder an entry names, and each entry, in name order. The same files make the same bytes:
/// no times, no owner; a folder and a program are 0755 and any other file 0644.
pub fn write(entries: &[Entry], platform: &str, out: &Path) -> Result<()> {
    let root = wezterm_release::archive_root(platform);
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(twice) = sorted.windows(2).find(|w| w[0].name == w[1].name) {
        return Err(format!("{} is in the archive twice", twice[0].name));
    }
    if let Some(entry) = sorted
        .iter()
        .find(|e| wezterm_release::misplaced(&Path::new(root).join(&e.name), root).is_some())
    {
        return Err(format!("{} is no name a release archive holds", entry.name));
    }
    let mut members: Vec<(String, Option<(bool, Vec<u8>)>)> = vec![(format!("{}/", root), None)];
    let mut folders = std::collections::BTreeSet::new();
    for entry in sorted {
        let path = format!("{}/{}", root, entry.name);
        let mut parent = Path::new(&path).parent();
        while let Some(folder) = parent.filter(|p| *p != Path::new(root)) {
            folders.insert(format!("{}/", folder.to_string_lossy()));
            parent = folder.parent();
        }
        let bytes = std::fs::read(&entry.from)
            .map_err(|e| format!("cannot read {}: {}", entry.from.display(), e))?;
        members.push((path, Some((entry.program, bytes))));
    }
    members.extend(folders.into_iter().map(|f| (f, None)));
    members.sort_by(|a, b| a.0.cmp(&b.0));

    let file = std::fs::File::create(out)
        .map_err(|e| format!("cannot create {}: {}", out.display(), e))?;
    let fail = |e: std::io::Error| format!("cannot write {}: {}", out.display(), e);
    let gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(file, flate2::Compression::best());
    let mut b = tar::Builder::new(gz);
    for (path, file) in &members {
        let mut h = tar::Header::new_gnu();
        h.set_mtime(0);
        h.set_uid(0);
        h.set_gid(0);
        match file {
            None => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
                b.append_data(&mut h, path, std::io::empty())
                    .map_err(fail)?;
            }
            Some((program, bytes)) => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(if *program { 0o755 } else { 0o644 });
                h.set_size(bytes.len() as u64);
                b.append_data(&mut h, path, bytes.as_slice())
                    .map_err(fail)?;
            }
        }
    }
    b.into_inner()
        .map_err(fail)?
        .finish()
        .map(|_| ())
        .map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let at = dir.join(name.replace('/', "_"));
        std::fs::write(&at, body).unwrap();
        at
    }

    fn entries(dir: &Path, names: &[(&str, bool)]) -> Vec<Entry> {
        names
            .iter()
            .map(|(name, program)| Entry {
                name: name.to_string(),
                from: file(dir, name, name),
                program: *program,
            })
            .collect()
    }

    fn members(archive: &Path) -> Vec<(String, u32)> {
        let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive).unwrap());
        tar::Archive::new(gz)
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().to_string_lossy().into_owned(),
                    e.header().mode().unwrap(),
                )
            })
            .collect()
    }

    /// The macOS archive is the bundle, its folders and files in order with their modes, and
    /// nothing else; the same files make the same bytes.
    #[test]
    fn an_archive_holds_the_set_alone_and_is_the_same_bytes_every_time() {
        let dir = tempfile::tempdir().unwrap();
        // Beside the files the set names, what a Mac leaves, which no archive takes.
        std::fs::write(dir.path().join("._Info.plist"), "attributes").unwrap();
        std::fs::write(dir.path().join(".DS_Store"), "finder").unwrap();
        let set = entries(
            dir.path(),
            &[
                ("Contents/MacOS/wsb-wezterm", true),
                ("Contents/Info.plist", false),
            ],
        );
        let (first, second) = (dir.path().join("a.tar.gz"), dir.path().join("b.tar.gz"));
        write(&set, "darwin-arm64", &first).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(
            &set.iter().rev().cloned().collect::<Vec<_>>(),
            "darwin-arm64",
            &second,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
        assert_eq!(
            members(&first),
            [
                ("wsb-wezterm.app/".to_string(), 0o755),
                ("wsb-wezterm.app/Contents/".to_string(), 0o755),
                ("wsb-wezterm.app/Contents/Info.plist".to_string(), 0o644),
                ("wsb-wezterm.app/Contents/MacOS/".to_string(), 0o755),
                (
                    "wsb-wezterm.app/Contents/MacOS/wsb-wezterm".to_string(),
                    0o755
                ),
            ]
        );
    }

    /// `wsb-wezterm upgrade` unpacks what this writes into the set it installs.
    #[test]
    fn the_updater_unpacks_the_archive_into_the_set() {
        let dir = tempfile::tempdir().unwrap();
        let set = entries(
            dir.path(),
            &[
                ("wsb-wezterm", true),
                ("share/terminfo/wsb-wezterm.terminfo", false),
            ],
        );
        let archive = dir.path().join(name("1.2.3", "linux-arm64"));
        write(&set, "linux-arm64", &archive).unwrap();
        let root = wezterm_release::unpack(&archive, "linux-arm64", &dir.path().join("into"))
            .map_err(|e| e.to_string())
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("wsb-wezterm")).unwrap(),
            "wsb-wezterm"
        );
        assert!(root.join("share/terminfo/wsb-wezterm.terminfo").is_file());
    }

    /// A name twice, or one the updater would refuse, is never written.
    #[test]
    fn an_entry_the_updater_would_refuse_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a.tar.gz");
        let twice = entries(dir.path(), &[("wsb-wezterm", true), ("wsb-wezterm", false)]);
        assert_eq!(
            write(&twice, "linux-x86_64", &out).unwrap_err(),
            "wsb-wezterm is in the archive twice"
        );
        let apple = entries(dir.path(), &[("share/._wsb-wezterm.desktop", false)]);
        assert_eq!(
            write(&apple, "linux-x86_64", &out).unwrap_err(),
            "share/._wsb-wezterm.desktop is no name a release archive holds"
        );
        assert!(!out.exists());
    }
}
