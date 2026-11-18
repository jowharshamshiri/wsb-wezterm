/// The variable that makes a build a release (see wezterm-release's `release_line.rs`).
const RELEASE_CHANNEL: &str = "BAHRAMJ_RELEASE_CHANNEL";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={}", RELEASE_CHANNEL);

    let mut ci_tag = String::new();
    // A release names itself by the version it is published as, the repository's
    // version.txt, which `wsb-wezterm upgrade` compares with the newest published one.
    let release = std::env::var(RELEASE_CHANNEL).is_ok_and(|channel| !channel.trim().is_empty());
    if release {
        println!("cargo:rerun-if-changed=../version.txt");
        let version = std::fs::read_to_string("../version.txt").unwrap_or_else(|e| {
            panic!("a release build of wsb-wezterm is named by version.txt, which cannot be read: {e}")
        });
        ci_tag = version.trim().to_string();
        let numbers: Vec<&str> = ci_tag.split('.').collect();
        if numbers.len() != 3
            || !numbers
                .iter()
                .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            panic!(
                "version.txt holds {ci_tag:?}, not the <major>.<minor>.<patch> a release is published as"
            );
        }
    } else if let Ok(tag) = std::fs::read("../.tag") {
        // If a file named `.tag` is present, we'll take its contents for the
        // version number that we report in wsb-wezterm -h.
        if let Ok(s) = String::from_utf8(tag) {
            ci_tag = s.trim().to_string();
            println!("cargo:rerun-if-changed=../.tag");
        }
    } else {
        // Otherwise we'll derive it from the git information

        if let Ok(repo) = git2::Repository::discover(".") {
            if let Ok(ref_head) = repo.find_reference("HEAD") {
                let repo_path = repo.path().to_path_buf();

                if let Ok(resolved) = ref_head.resolve() {
                    if let Some(name) = resolved.name() {
                        let path = repo_path.join(name);
                        if path.exists() {
                            println!(
                                "cargo:rerun-if-changed={}",
                                path.canonicalize().unwrap().display()
                            );
                        }
                    }
                }
            }

            if let Ok(output) = std::process::Command::new("git")
                .args(&[
                    "-c",
                    "core.abbrev=8",
                    "show",
                    "-s",
                    "--format=%cd-%h",
                    "--date=format:%Y%m%d-%H%M%S",
                ])
                .output()
            {
                if output.status.success() {
                    let info = String::from_utf8_lossy(&output.stdout);
                    ci_tag = info.trim().to_string();
                }
            }
        }
    }

    // A build names itself by its tag; one that cannot is refused rather
    // than built without a name.
    if ci_tag.is_empty() {
        eprintln!(
            "wsb-wezterm cannot name this build: the source has no .tag at its top, \
             and git cannot describe its HEAD. Build from a git checkout, or write \
             the build's name to .tag (as `git -c core.abbrev=8 show -s \
             --format=%cd-%h --date=format:%Y%m%d-%H%M%S` prints it)."
        );
        std::process::exit(1);
    }

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=WSB_WEZTERM_TARGET_TRIPLE={}", target);
    println!("cargo:rustc-env=WSB_WEZTERM_CI_TAG={}", ci_tag);
}
