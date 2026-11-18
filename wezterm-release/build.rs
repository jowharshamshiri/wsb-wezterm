//! Bakes in the release line a release build is given (`WSB_WEZTERM_BAKED_RELEASE_LINE`), which
//! `wsb-wezterm upgrade` follows. A build given none is a build from source.

#[path = "release_line.rs"]
mod release_line;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=release_line.rs");
    for name in release_line::INPUTS {
        println!("cargo:rerun-if-env-changed={}", name);
    }
    let line =
        release_line::read(&|name| std::env::var(name).ok()).unwrap_or_else(|e| panic!("{}", e));
    if let Some(line) = line {
        println!("cargo:rustc-env={}={}", release_line::BAKED, line);
    }
}
