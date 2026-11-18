//! `wsb-wezterm upgrade` against release lines the tests publish and sign themselves.

use std::path::{Path, PathBuf};

use crate::testing::{
    manifest, program, release_archive, tar_gz, Publisher, Served, NOT_AFTER, NOW, REGISTRY,
};
use crate::{
    archive_root, fetch_latest, latest, later, misplaced, parse_baked, platform_of,
    program_in_root, unpack, upgrade_with, Line, Outcome, Problem,
};

const LINUX: &str = "linux-x86_64";
const MACOS: &str = "darwin-arm64";

/// The reason a refusal names; anything else fails the test.
fn reason<T: std::fmt::Debug>(outcome: Result<T, Problem>) -> &'static str {
    match outcome {
        Err(Problem::Refused { reason, .. }) => reason,
        other => panic!("expected a refusal, got {:?}", other),
    }
}

/// A line publishing wsb-wezterm `version` for `platform` on the release channel, signed under
/// `label`.
fn published_for(publisher: &Publisher, label: &str, version: &str, platform: &str) -> Served {
    let served = Served::default();
    served.publish(
        publisher,
        label,
        "release",
        version,
        platform,
        release_archive(version, platform),
    );
    served
}

fn published(publisher: &Publisher, label: &str) -> Served {
    published_for(publisher, label, "1.60.4000", LINUX)
}

/// An installed wsb-wezterm `version` for `platform`, its set unpacked in a folder of its own,
/// as a release archive unpacks.
struct Installed {
    _dir: tempfile::TempDir,
    folder: PathBuf,
    set: PathBuf,
    program: PathBuf,
}

impl Installed {
    fn new(version: &str, platform: &str) -> Installed {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().canonicalize().unwrap();
        let archive = folder.join("installed.tar.gz");
        std::fs::write(&archive, release_archive(version, platform)).unwrap();
        let set = unpack(&archive, platform, &folder).unwrap();
        std::fs::remove_file(&archive).unwrap();
        let program = set.join(program_in_root(platform));
        Installed {
            _dir: dir,
            folder,
            set,
            program,
        }
    }

    fn says(&self) -> Vec<u8> {
        std::fs::read(&self.program).unwrap()
    }

    /// Everything beside the set: what an upgrade left behind.
    fn left_beside(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| Path::new(name) != Path::new(self.set.file_name().unwrap()))
            .collect();
        names.sort();
        names
    }
}

fn upgrade(served: &Served, line: &Line, installed: &Installed, platform: &str, version: &str) -> Result<Outcome, String> {
    upgrade_with(served, line, platform, NOW, &installed.program, version)
}

#[test]
fn the_baked_release_line_is_read_back_as_it_was_given() {
    let publisher = Publisher::new();
    let keys = publisher.root_keys().join(",");
    let line = parse_baked(&format!(
        "https://releases.example nightly staging {} {}",
        keys, keys
    ));
    let mut expected = publisher.line("nightly", "staging");
    expected.registry = "https://releases.example".to_string();
    assert_eq!(line, expected);
}

/// wsb-wezterm is published for Linux on x86_64 and arm64 and for macOS on Apple silicon: a
/// folder on Linux, the application bundle on macOS.
#[test]
fn wsb_wezterm_is_published_for_three_platforms() {
    assert_eq!(platform_of("linux", "x86_64"), Some("linux-x86_64"));
    assert_eq!(platform_of("linux", "aarch64"), Some("linux-arm64"));
    assert_eq!(platform_of("macos", "aarch64"), Some("darwin-arm64"));
    assert_eq!(platform_of("windows", "x86_64"), None);
    assert_eq!(platform_of("macos", "x86_64"), None);
    assert_eq!(archive_root(LINUX), "wsb-wezterm");
    assert_eq!(archive_root(MACOS), "wsb-wezterm.app");
    assert_eq!(program_in_root(MACOS), "Contents/MacOS/wsb-wezterm");
    assert_eq!(
        crate::archive_name("1.2.3", MACOS),
        "wsb-wezterm-1.2.3-darwin-arm64.tar.gz"
    );
}

/// A line the build's roots signed is read, and its newest release for the platform found,
/// reading the charter first.
#[test]
fn a_line_signed_by_the_builds_roots_is_read() {
    let publisher = Publisher::new();
    let served = published(&publisher, "prod");
    let found = fetch_latest(&served, &publisher.line("release", "prod"), LINUX, NOW)
        .unwrap()
        .expect("a release is published");
    assert_eq!(found.version, "1.60.4000");
    assert_eq!(found.url, Served::archive_url("release", "1.60.4000", LINUX));
    assert_eq!(
        *served.asked.borrow(),
        [
            format!("{}/charter.json", REGISTRY),
            format!("{}/charter.json.sig", REGISTRY),
            Served::manifest_url(),
            format!("{}.sig", Served::manifest_url()),
        ]
    );
    assert_eq!(
        fetch_latest(&served, &publisher.line("nightly", "prod"), LINUX, NOW).unwrap(),
        None
    );
}

/// A manifest changed after it was signed, published without a signature, or signed by
/// another publisher, is refused by its reason.
#[test]
fn a_tampered_unsigned_or_foreign_manifest_is_refused() {
    let publisher = Publisher::new();
    let line = publisher.line("release", "prod");
    let served = published(&publisher, "prod");
    let mut changed = served.files.borrow()[&Served::manifest_url()].clone();
    let at = changed.len() - 2;
    changed[at] ^= 1;
    served.put(&Served::manifest_url(), changed);
    assert_eq!(reason(fetch_latest(&served, &line, LINUX, NOW)), "manifest_signature_invalid");
    served.remove(&format!("{}.sig", Served::manifest_url()));
    assert_eq!(reason(fetch_latest(&served, &line, LINUX, NOW)), "manifest_unsigned");

    let served = published(&publisher, "prod");
    let manifest = served.files.borrow()[&Served::manifest_url()].clone();
    served.put(
        &format!("{}.sig", Served::manifest_url()),
        Publisher::foreign().sign(&manifest, "prod"),
    );
    assert_eq!(reason(fetch_latest(&served, &line, LINUX, NOW)), "insufficient_root_signatures");
}

/// A certificate for another environment, or past its validity, is refused.
#[test]
fn a_certificate_for_another_environment_or_out_of_date_is_refused() {
    let publisher = Publisher::new();
    let served = published(&publisher, "prod");
    let charter = Publisher::charter(REGISTRY, "staging");
    served.put(&format!("{}/charter.json.sig", REGISTRY), publisher.sign_charter(&charter));
    served.put(&format!("{}/charter.json", REGISTRY), charter);
    let refused = fetch_latest(&served, &publisher.line("release", "staging"), LINUX, NOW);
    assert_eq!(reason(refused), "environment_mismatch");
    let served = published(&publisher, "prod");
    let late = fetch_latest(&served, &publisher.line("release", "prod"), LINUX, NOT_AFTER + 1);
    assert_eq!(reason(late), "expired_certificate");
}

/// A foreign or unsigned charter is refused before any manifest is read, and a registry with
/// no charter is not read at all.
#[test]
fn a_foreign_charter_is_refused_before_the_manifest_is_read() {
    let publisher = Publisher::new();
    let line = publisher.line("release", "prod");
    let refuses = |served: &Served, expected: &str| {
        served.asked.borrow_mut().clear();
        assert_eq!(reason(fetch_latest(served, &line, LINUX, NOW)), expected);
        assert!(!served.asked.borrow().contains(&Served::manifest_url()));
    };
    let served = published(&publisher, "prod");
    let charter = Publisher::charter(REGISTRY, "prod");
    served.put(
        &format!("{}/charter.json.sig", REGISTRY),
        Publisher::foreign().sign_charter(&charter),
    );
    refuses(&served, "insufficient_root_signatures");
    let elsewhere = Publisher::charter("https://elsewhere.example", "prod");
    served.put(&format!("{}/charter.json", REGISTRY), elsewhere.clone());
    served.put(&format!("{}/charter.json.sig", REGISTRY), publisher.sign_charter(&elsewhere));
    refuses(&served, "charter_anchor_mismatch");
    let certificate = Publisher::certificate(&publisher.release, "prod");
    let roots = [&publisher.roots[0], &publisher.roots[1]];
    let theirs = Publisher::envelope(
        "other-charter-sig/1",
        "charter_signature",
        &certificate,
        &roots,
        &publisher.release,
        &charter,
    );
    served.put(&format!("{}/charter.json", REGISTRY), charter.clone());
    served.put(&format!("{}/charter.json.sig", REGISTRY), theirs);
    refuses(&served, "unsupported_envelope_format");
    served.remove(&format!("{}/charter.json.sig", REGISTRY));
    refuses(&served, "charter_unsigned");
    served.remove(&format!("{}/charter.json", REGISTRY));
    served.asked.borrow_mut().clear();
    let failed = fetch_latest(&served, &line, LINUX, NOW).expect_err("no charter");
    assert!(matches!(&failed, Problem::Failed(why) if why.contains("has no charter")), "{:?}", failed);
    assert_eq!(*served.asked.borrow(), [format!("{}/charter.json", REGISTRY)]);
}

/// A signed manifest that is not wsb-wezterm's, or leaves the platform out, is refused for
/// its content.
#[test]
fn a_manifest_that_is_not_wsb_weztermss_or_not_whole_is_refused() {
    let url = "https://release.example/wsb-wezterm/release/1.0.0/a";
    let sha = "a".repeat(64);
    let good = manifest("release", "1.0.0", LINUX, url, &sha, 10);
    let message = |bytes: &[u8], platform: &str| match latest(bytes, "release", platform) {
        Err(Problem::Refused {
            reason: "manifest_content",
            message,
        }) => message,
        other => panic!("expected a refusal of the content, got {:?}", other),
    };
    assert!(message(&good, MACOS).contains("not published for darwin-arm64"));
    let other = String::from_utf8(good.clone())
        .unwrap()
        .replace("\"appId\":\"wsb-wezterm\"", "\"appId\":\"boxmux\"");
    assert!(message(other.as_bytes(), LINUX).contains("boxmux's, not wsb-wezterm's"));
    let short = manifest("release", "1.0.0", LINUX, url, "abc", 10);
    assert!(message(&short, LINUX).contains("as its SHA-256"));
}

/// The newest release replaces the whole installed set, on Linux its folder and on macOS its
/// application bundle, once its archive matches the signed manifest and its program says it is
/// the published version; nothing is left beside it.
#[cfg(unix)]
#[test]
fn the_newest_release_replaces_the_whole_installed_set() {
    for platform in [LINUX, MACOS] {
        let publisher = Publisher::new();
        let served = published_for(&publisher, "prod", "1.60.4000", platform);
        let installed = Installed::new("1.51.3451", platform);
        let stale = installed.set.join("left-by-the-old-release");
        std::fs::write(&stale, "old").unwrap();
        let outcome = upgrade(&served, &publisher.line("release", "prod"), &installed, platform, "1.51.3451").unwrap();
        assert_eq!(
            outcome,
            Outcome::Installed {
                channel: "release".to_string(),
                platform: platform.to_string(),
                from: "1.51.3451".to_string(),
                to: "1.60.4000".to_string(),
                set: installed.set.clone(),
            }
        );
        assert_eq!(installed.says(), program("1.60.4000"), "{}", platform);
        assert!(!stale.exists(), "the old set is gone whole: {}", platform);
        assert_eq!(installed.left_beside(), Vec::<String>::new(), "{}", platform);
    }
}

/// The same release, or an older one, installs nothing: never a downgrade.
#[test]
fn a_release_no_later_than_the_running_one_installs_nothing() {
    let publisher = Publisher::new();
    let line = publisher.line("release", "prod");
    for running in ["1.60.4000", "1.60.4001", "2.0.0"] {
        let served = published(&publisher, "prod");
        let installed = Installed::new(running, LINUX);
        assert_eq!(
            upgrade(&served, &line, &installed, LINUX, running).unwrap(),
            Outcome::UpToDate {
                channel: "release".to_string(),
                platform: LINUX.to_string(),
                installed: running.to_string(),
                newest: "1.60.4000".to_string(),
            }
        );
        assert_eq!(installed.says(), program(running));
    }
}

/// An archive other than the signed one, by its SHA-256 or size, one holding an AppleDouble
/// file, and one whose program is another version, are each refused, and the installed set
/// stays as it was, with nothing left beside it.
#[test]
fn an_archive_other_than_the_signed_release_is_refused_and_nothing_changes() {
    let publisher = Publisher::new();
    let line = publisher.line("release", "prod");
    let url = Served::archive_url("release", "1.60.4000", LINUX);
    let archive = release_archive("1.60.4000", LINUX);
    let mut changed = archive.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 1;
    let cases: Vec<(&str, Option<Vec<u8>>, Vec<u8>)> = vec![
        ("archive_sha256_mismatch", Some(changed), archive.clone()),
        ("archive_size_mismatch", Some([archive.clone(), b"x".to_vec()].concat()), archive.clone()),
        (
            "archive_layout",
            None,
            tar_gz(&[("._wsb-wezterm", b"attributes"), ("wsb-wezterm/wsb-wezterm", &program("1.60.4000"))]),
        ),
        (
            "archive_layout",
            None,
            tar_gz(&[("wsb-wezterm/wsb-wezterm", &program("1.60.4000")), ("wsb-wezterm/._wsb-wezterm", b"a")]),
        ),
        ("archive_version", None, release_archive("1.59.0", LINUX)),
    ];
    for (expected, served_bytes, signed) in cases {
        let served = Served::default();
        served.publish(&publisher, "prod", "release", "1.60.4000", LINUX, signed);
        if let Some(bytes) = served_bytes {
            served.put(&url, bytes);
        }
        let installed = Installed::new("1.51.3451", LINUX);
        let refused = upgrade(&served, &line, &installed, LINUX, "1.51.3451").expect_err("refused");
        assert!(refused.contains(&format!("(refused: {})", expected)), "{}", refused);
        assert_eq!(installed.says(), program("1.51.3451"));
        assert_eq!(installed.left_beside(), Vec::<String>::new());
    }
}

/// A program outside a set unpacked from a release, such as one built from source and put
/// on PATH, is not replaced; a deb's is left to apt, naming its command.
#[test]
fn only_an_unpacked_release_is_replaced_and_a_debs_is_left_to_apt() {
    let publisher = Publisher::new();
    let line = publisher.line("release", "prod");
    let served = published(&publisher, "prod");
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().canonicalize().unwrap().join("bin").join("wsb-wezterm");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, program("1.51.3451")).unwrap();
    let refused = upgrade_with(&served, &line, LINUX, NOW, &elsewhere, "1.51.3451").expect_err("not a release");
    assert!(refused.contains("is not the wsb-wezterm of a release unpacked from its archive"), "{}", refused);

    let installed = Installed::new("1.51.3451", LINUX);
    std::fs::write(
        installed.program.with_file_name(crate::package::MARKER),
        serde_json::json!({
            "format": 1,
            "manager": "apt",
            "program": installed.program,
            "update": "sudo apt install --only-upgrade wsb-wezterm",
        })
        .to_string(),
    )
    .unwrap();
    let refused = upgrade(&served, &line, &installed, LINUX, "1.51.3451").expect_err("left to apt");
    assert_eq!(
        refused,
        "wsb-wezterm 1.60.4000 is the newest release on the release channel for linux-x86_64, and \
         wsb-wezterm was installed by apt, which upgrades it: run `sudo apt install --only-upgrade wsb-wezterm`"
    );
    assert!(!served.asked.borrow().contains(&Served::archive_url("release", "1.60.4000", LINUX)));
}

/// A registry that cannot be reached fails naming what was asked.
#[test]
fn an_unreachable_registry_fails_naming_it() {
    struct Down;
    impl crate::Transport for Down {
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
            Err(format!("could not reach {}: connection refused", url))
        }
        fn download(&self, url: &str, _: &Path, _: u64) -> Result<u64, String> {
            Err(format!("could not reach {}: connection refused", url))
        }
    }
    let publisher = Publisher::new();
    let installed = Installed::new("1.51.3451", LINUX);
    let failed = upgrade_with(&Down, &publisher.line("release", "prod"), LINUX, NOW, &installed.program, "1.51.3451")
        .expect_err("down");
    assert_eq!(failed, format!("could not reach {}/charter.json: connection refused", REGISTRY));
}

#[test]
fn an_archive_unpacks_only_into_its_one_directory() {
    let unpack_bytes = |platform: &str, bytes: Vec<u8>| {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(&archive, bytes).unwrap();
        (unpack(&archive, platform, &dir.path().join("into")).map(|_| ()), dir)
    };
    assert_eq!(unpack_bytes(MACOS, release_archive("1.0.0", MACOS)).0, Ok(()));
    assert_eq!(reason(unpack_bytes(MACOS, release_archive("1.0.0", LINUX)).0), "archive_layout");
    assert_eq!(reason(unpack_bytes(LINUX, tar_gz(&[("other/wsb-wezterm", b"x")])).0), "archive_layout");
    assert_eq!(reason(unpack_bytes(LINUX, b"not gzip".to_vec()).0), "archive_layout");
    assert!(misplaced(Path::new("wsb-wezterm.app/Contents/._Info.plist"), "wsb-wezterm.app")
        .unwrap()
        .contains("AppleDouble"));
}

#[test]
fn a_release_is_later_by_its_numbers() {
    assert_eq!(later("1.60.4000", "1.51.3451"), Ok(true));
    assert_eq!(later("1.51.3451", "1.51.3451"), Ok(false));
    assert_eq!(later("1.9.0", "1.10.0"), Ok(false));
    assert!(later("1.51.3451", "20261117-120000-dc780c05")
        .unwrap_err()
        .contains("the installed version"));
}
