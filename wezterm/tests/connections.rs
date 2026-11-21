//! A client's connection to a real `wsb-wezterm-mux-server`, on a socket of
//! its own, as the multiplexer protocol has it: a client may send while
//! replies are on their way to it, and one that stops reading stalls no
//! other client. The server is the one built beside this crate's
//! `wsb-wezterm`: `cargo build -p wezterm-mux-server` first.
#![cfg(unix)]

mod common;

use codec::{ClientKind, DecodedPdu, Pdu, Ping, SetClientId};
use common::{Server, WAIT};
use mux::client::{ClientId, ClientRole};
use serde_json::Value;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// How long a server waits on a client that accepts nothing it sends before
/// it drops the client (the server's own limit).
const STALL_LIMIT: Duration = Duration::from_secs(30);

fn encode(pdu: Pdu, serial: u64) -> Vec<u8> {
    let mut bytes = vec![];
    pdu.encode(&mut bytes, serial).unwrap();
    bytes
}

/// The clients the server lists with this test process's pid: the raw
/// connections the test made itself.
fn own_clients(server: &Server) -> Vec<Value> {
    let clients: Vec<Value> =
        serde_json::from_str(&server.cli(&["list-clients", "--format", "json"])).unwrap();
    clients
        .into_iter()
        .filter(|c| c["pid"].as_u64() == Some(std::process::id() as u64))
        .collect()
}

#[test]
fn a_client_sending_while_its_replies_wait_is_read_and_answered() {
    let server = Server::start();
    let stream = UnixStream::connect(server.socket()).unwrap();

    // A client that writes its requests one after another and reads the
    // replies after: far more of both than the socket holds either way, so
    // the server must read while its replies wait to be written.
    const PINGS: u64 = 300_000;
    let mut requests = vec![];
    for serial in 1..=PINGS {
        requests.extend(encode(Pdu::Ping(Ping {}), serial));
    }
    let mut writer = stream.try_clone().unwrap();
    let (sent, written) = channel();
    std::thread::spawn(move || sent.send(writer.write_all(&requests)));
    written
        .recv_timeout(WAIT)
        .unwrap_or_else(|_| {
            panic!(
                "the server stopped reading the client's requests while its replies waited: \
                 {} bytes of pings were not taken in {:?}",
                PINGS * 3,
                WAIT
            )
        })
        .unwrap();

    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut replies = BufReader::new(stream);
    for serial in 1..=PINGS {
        let DecodedPdu { serial: got, pdu } = Pdu::decode(&mut replies)
            .unwrap_or_else(|err| panic!("reply {} of {}: {:#}", serial, PINGS, err));
        assert_eq!((got, pdu.pdu_name()), (serial, "Pong"));
    }
    // The server still answers others.
    assert_eq!(server.panes().len(), 1);
}

#[test]
fn a_client_that_stops_reading_stalls_no_other_and_is_dropped() {
    // A pane that prints without pause, as a busy session does: every
    // client is sent its changes.
    let server = Server::sized(
        80,
        24,
        &[
            "sh",
            "-c",
            "while :; do echo \"$RANDOM $RANDOM $RANDOM\"; done",
        ],
    );
    let pane = server.panes()[0]["pane_id"].as_u64().unwrap();
    let mut relay = server.relay(pane, &[]);
    relay.expect("hello");

    // A client that says who it is, and then reads nothing more, as one
    // stopped mid-read does.
    let mut stalled = UnixStream::connect(server.socket()).unwrap();
    stalled
        .write_all(&encode(
            Pdu::SetClientId(SetClientId {
                client_id: ClientId::new(),
                kind: ClientKind::Client(ClientRole::Command),
            }),
            1,
        ))
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while own_clients(&server).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the server never listed the client"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let stalled_at = Instant::now();

    // While it holds what the server sent it unread, the others are
    // answered and kept up to date.
    std::thread::sleep(Duration::from_secs(3));
    for _ in 0..3 {
        let started = Instant::now();
        assert_eq!(server.panes().len(), 1);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the server took {:?} to list its panes",
            started.elapsed()
        );
        relay.expect("screen");
    }

    // The server drops it once it has taken nothing for the server's limit,
    // and closes its connection.
    let deadline = stalled_at + STALL_LIMIT + WAIT;
    while !own_clients(&server).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the server still lists a client that has read nothing for {:?}",
            stalled_at.elapsed()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        stalled_at.elapsed() >= STALL_LIMIT - Duration::from_secs(5),
        "the server dropped a client after only {:?}",
        stalled_at.elapsed()
    );
    stalled.set_read_timeout(Some(WAIT)).unwrap();
    let mut rest = vec![];
    match std::io::Read::read_to_end(&mut stalled, &mut rest) {
        Ok(_) => {}
        Err(err) => assert_eq!(
            err.kind(),
            std::io::ErrorKind::ConnectionReset,
            "the dropped client's connection was not closed: {}",
            err
        ),
    }
    relay.expect("screen");
}
