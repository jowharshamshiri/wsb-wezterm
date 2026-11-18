//! The release line a build bakes into wsb-wezterm (`release_line.rs`).

// The build script's module, of which these tests read the variables and not the name it bakes
// the line into.
#[allow(dead_code)]
#[path = "../release_line.rs"]
mod release_line;

use base64::Engine as _;
use std::collections::BTreeMap;

/// A minisign public key made from `seed`, in minisign's base64 form.
fn key(seed: u8) -> String {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let mut bytes = b"Ed".to_vec();
    bytes.extend_from_slice(&[seed, 1, 2, 3, 4, 5, 6, 7]);
    bytes.extend_from_slice(signing.verifying_key().as_bytes());
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn roots() -> String {
    [key(1), key(2), key(3)].join(",")
}

/// A whole release line's variables, with `changes` applied (an empty value removes one).
fn given(changes: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> = [
        (
            release_line::REGISTRY,
            "https://releases.example".to_string(),
        ),
        (release_line::CHANNEL, "release".to_string()),
        (release_line::ENVIRONMENT, "prod".to_string()),
        (release_line::ROOTS, roots()),
        (release_line::CHARTER_ROOTS, roots()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    for (name, value) in changes {
        if value.is_empty() {
            vars.remove(*name);
        } else {
            vars.insert(name.to_string(), value.to_string());
        }
    }
    vars
}

fn read(vars: &BTreeMap<String, String>) -> Result<Option<String>, String> {
    release_line::read(&|name| vars.get(name).cloned())
}

#[test]
fn a_release_build_bakes_its_whole_line_as_one_value() {
    assert_eq!(
        read(&given(&[])),
        Ok(Some(format!(
            "https://releases.example release prod {} {}",
            roots(),
            roots()
        )))
    );
}

#[test]
fn a_build_given_no_channel_is_a_build_from_source() {
    assert_eq!(read(&given(&[(release_line::CHANNEL, "")])), Ok(None));
    assert_eq!(read(&BTreeMap::new()), Ok(None));
}

#[test]
fn a_channel_without_the_rest_of_its_line_is_refused_naming_what_is_missing() {
    let e = read(&given(&[(release_line::ROOTS, "")])).unwrap_err();
    assert!(
        e.ends_with(&format!("this one is not given {}", release_line::ROOTS)),
        "{}",
        e
    );
}

#[test]
fn each_part_of_a_line_is_held_to_its_shape() {
    let refused = |changes: &[(&str, &str)], says: &str| {
        let e = read(&given(changes)).unwrap_err();
        assert!(e.contains(says), "{} does not say {:?}", e, says);
    };
    refused(
        &[(release_line::REGISTRY, "http://releases.example")],
        "must be an https URL",
    );
    refused(
        &[(release_line::CHANNEL, "beta")],
        "must be `release` or `nightly`",
    );
    refused(
        &[(release_line::ENVIRONMENT, "dev")],
        "must be `prod` or `staging`",
    );
    let two = [key(1), key(2)].join(",");
    refused(&[(release_line::ROOTS, &two)], "has 2 root(s)");
    let twice = [key(1), key(2), key(1)].join(",");
    refused(&[(release_line::CHARTER_ROOTS, &twice)], "twice");
}
