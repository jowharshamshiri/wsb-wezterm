//! How a pane ended, as `wsb-wezterm cli pane-end` prints it from a real
//! `wsb-wezterm-mux-server`: why the server closed it, the client that
//! asked, and how its program ended. `cargo build -p wezterm-mux-server`
//! first; the test of a window closing a tab also needs an X display,
//! `WSB_WEZTERM_TEST_DISPLAY`, xdotool and `cargo build -p wezterm-gui`.
#![cfg(unix)]

mod common;

use common::{display, Server, WAIT};
use serde_json::Value;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a window may take to start and attach.
const ATTACH: Duration = Duration::from_secs(90);

/// The end `pane-end` prints for `pane`, once `done` holds of it.
fn end_once(server: &Server, pane: u64, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let output = server
            .wezterm()
            .args(["cli", "--no-auto-start", "pane-end", "--pane-id"])
            .arg(pane.to_string())
            .output()
            .unwrap();
        if output.status.success() {
            let end: Value = serde_json::from_slice(&output.stdout).unwrap();
            if done(&end) {
                return end;
            }
        }
        assert!(
            Instant::now() < deadline,
            "pane {pane} never showed {what}; pane-end says {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn(server: &Server, program: &[&str]) -> u64 {
    server
        .cli(&[&["spawn", "--new-window", "--"], program].concat())
        .trim()
        .parse()
        .unwrap()
}

/// Whether any process of `lineage` ran a command line holding `words`, in
/// order.
fn ran(lineage: &Value, words: &[&str]) -> bool {
    lineage.as_array().unwrap().iter().any(|process| {
        let argv: Vec<&str> = process["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect();
        argv.windows(words.len()).any(|run| run == words)
    })
}

#[test]
fn a_pane_a_client_kills_names_the_client_and_its_program_hung_up() {
    let server = Server::start();
    let pane = spawn(&server, &["cat"]);
    server.cli(&["kill-pane", "--pane-id", &pane.to_string()]);

    let end = end_once(&server, pane, "its program's end", |end| {
        !end["program"].is_null()
    });
    let closed = &end["closed"];
    assert_eq!(closed["why"], "pane", "{end}");
    let client = &closed["client"];
    assert_eq!(
        client["hostname"],
        hostname::get().unwrap().to_string_lossy().as_ref(),
        "{end}"
    );
    assert!(
        ran(&client["lineage"], &["kill-pane", "--pane-id"]),
        "the client's lineage names the command that asked: {}",
        end
    );
    assert_eq!(
        client["lineage"][1]["pid"],
        std::process::id(),
        "the command's parent is this test: {end}"
    );
    assert!(closed["signal"].is_null(), "{}", end);
    assert_eq!(end["program"]["signal"], libc::SIGHUP, "{end}");
    assert!(end["program"]["code"].is_null(), "{}", end);
}

#[test]
fn a_program_that_ends_by_itself_leaves_an_end_with_no_close() {
    let server = Server::start();
    let pane = spawn(&server, &["sh", "-c", "sleep 0.5; exit 0"]);
    let end = end_once(&server, pane, "its program's end", |end| {
        !end["program"].is_null()
    });
    assert!(end["closed"].is_null(), "{}", end);
    assert_eq!(end["program"]["code"], 0, "{end}");
    assert!(end["program"]["signal"].is_null(), "{}", end);
}

#[test]
fn a_running_pane_has_no_end_and_an_unknown_one_is_refused() {
    let server = Server::start();
    for (pane, said) in [
        ("0", "keeps no end of pane 0: it runs and is not closing"),
        ("4711", "keeps no end of pane 4711"),
    ] {
        let output = server
            .wezterm()
            .args(["cli", "--no-auto-start", "pane-end", "--pane-id", pane])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(3), "{stderr}");
        assert!(stderr.contains(said), "{}", stderr);
        assert!(
            output.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn a_server_stopped_by_a_signal_closes_its_panes_naming_it_and_waits_for_them() {
    // A program that takes a while to end on its hangup, as one that records
    // how it ended does.
    let mut server = Server::running(&[
        "sh",
        "-c",
        "trap 'sleep 2; exit 0' HUP; while :; do sleep 0.1; done",
    ]);
    // SAFETY: kill(2) on the server this test started.
    assert_eq!(
        unsafe { libc::kill(server.child.id() as i32, libc::SIGTERM) },
        0
    );

    let end = end_once(&server, 0, "its close", |end| !end["closed"].is_null());
    let closed = &end["closed"];
    assert_eq!(closed["why"], "server_exiting", "{end}");
    assert_eq!(closed["signal"]["number"], libc::SIGTERM, "{end}");
    assert_eq!(closed["signal"]["sender"], std::process::id(), "{end}");
    assert_eq!(
        closed["signal"]["lineage"][0]["pid"],
        std::process::id(),
        "{end}"
    );
    assert!(
        end["program"].is_null(),
        "the server answers while its program is still ending: {}",
        end
    );

    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = server.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the server never exited once its pane's program ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{:?}", status);
}

#[test]
fn a_tab_a_window_closes_names_the_tab_and_the_window_as_client() {
    let display = display();
    let server = Server::sized(80, 24, &["cat"]);
    let _window = server.window(&display, 80, 24);
    let deadline = Instant::now() + ATTACH;
    let gui_pid = loop {
        let clients: Vec<Value> =
            serde_json::from_str(&server.cli(&["list-clients", "--format", "json"])).unwrap();
        if let Some(gui) = clients.iter().find(|client| client["role"] == "gui") {
            break gui["pid"].as_u64().unwrap();
        }
        assert!(Instant::now() < deadline, "no window attached");
        std::thread::sleep(Duration::from_millis(100));
    };
    let window_id = xdotool(
        &display,
        &["search", "--sync", "--pid", &gui_pid.to_string()],
    );
    let window_id = window_id.lines().next().unwrap().to_string();
    // Close the tab, and answer the window's question whether to.
    xdotool(
        &display,
        &[
            "key",
            "--window",
            &window_id,
            "--clearmodifiers",
            "ctrl+shift+w",
        ],
    );
    std::thread::sleep(Duration::from_secs(1));
    xdotool(&display, &["key", "--window", &window_id, "y"]);

    let end = end_once(&server, 0, "its program's end", |end| {
        !end["program"].is_null()
    });
    let closed = &end["closed"];
    assert_eq!(closed["why"], "tab", "{end}");
    assert_eq!(closed["client"]["pid"], gui_pid, "{end}");
    assert!(
        ran(&closed["client"]["lineage"], &["connect", "t"]),
        "the client is the window: {}",
        end
    );
}

fn xdotool(display: &str, args: &[&str]) -> String {
    let output = Command::new("xdotool")
        .args(args)
        .env("DISPLAY", display)
        .output()
        .expect("xdotool runs");
    assert!(
        output.status.success(),
        "xdotool {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
