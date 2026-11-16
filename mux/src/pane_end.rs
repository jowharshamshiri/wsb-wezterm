//! How each pane ended: why the multiplexer closed it, when it did, and
//! how the program in it ended.
//!
//! A program in a pane is told its pane is closing only by a hangup, which
//! says nothing of why. The multiplexer keeps the end of each of the last
//! [`KEPT`] panes it ended, or whose program ended, for whoever asks once the
//! pane is gone (`wsb-wezterm cli pane-end`): the program itself, as its
//! hangup arrives, or whoever finds it gone later.

use crate::client::ClientId;
use crate::pane::PaneId;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many pane ends a multiplexer keeps: the oldest is forgotten when
/// another is added past this.
pub const KEPT: usize = 1024;

/// How one pane ended, as far as it has.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PaneEnd {
    pub pane_id: PaneId,
    /// Why the multiplexer closed the pane; none when it did not, and the
    /// program in it ended by itself.
    pub closed: Option<Closed>,
    /// How the program in the pane ended; none while it runs.
    pub program: Option<ProgramEnd>,
}

/// The multiplexer closing a pane.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Closed {
    pub why: ClosedFor,
    /// The client that asked, when one did.
    pub client: Option<ClientSeen>,
    /// The signal that stopped the server, when it is exiting.
    pub signal: Option<SignalSeen>,
    /// When, in milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// A signal the server was sent: its number, and the process that sent it
/// and the processes that started that one, nearest first, where the system
/// names a sender.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SignalSeen {
    pub number: i32,
    pub sender: Option<u32>,
    pub lineage: Vec<ProcessSeen>,
}

/// Why the multiplexer closed a pane.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClosedFor {
    /// A client closed the pane itself (`wsb-wezterm cli kill-pane`, or a
    /// window's close of a pane).
    Pane,
    /// A client closed the pane's tab.
    Tab,
    /// A client closed the pane's window.
    Window,
    /// The server is exiting, and closed every pane it had.
    ServerExiting,
}

impl ClosedFor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Tab => "tab",
            Self::Window => "window",
            Self::ServerExiting => "server_exiting",
        }
    }
}

/// A client of the multiplexer, as it named itself, and, when it runs on
/// the multiplexer's own host, the program it is and the programs that
/// started it, nearest first, as they were when it asked.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientSeen {
    pub hostname: String,
    pub username: String,
    pub pid: u32,
    pub lineage: Vec<ProcessSeen>,
}

/// One process: its id and its command line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProcessSeen {
    pub pid: u32,
    pub argv: Vec<String>,
}

/// How a pane's program ended.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProgramEnd {
    pub pid: Option<u32>,
    /// Its exit code; none when a signal ended it.
    pub code: Option<u32>,
    /// The number of the signal that ended it.
    pub signal: Option<i32>,
    /// When the multiplexer saw it end, in milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// The ends a multiplexer keeps, newest last.
#[derive(Default)]
pub struct PaneEnds {
    ends: VecDeque<PaneEnd>,
}

impl PaneEnds {
    /// The end of pane `pane_id`, when this multiplexer keeps one.
    pub fn get(&self, pane_id: PaneId) -> Option<PaneEnd> {
        self.ends.iter().find(|end| end.pane_id == pane_id).cloned()
    }

    /// Keep why pane `pane_id` is being closed, unless a reason is kept
    /// already: the first reason is the one that closed it (a tab closed
    /// closes its panes, each of which is then removed).
    pub fn closing(&mut self, pane_id: PaneId, closed: Closed) {
        let end = self.entry(pane_id);
        if end.closed.is_none() {
            end.closed = Some(closed);
        }
    }

    /// Keep how pane `pane_id`'s program ended.
    pub fn program_ended(&mut self, pane_id: PaneId, program: ProgramEnd) {
        self.entry(pane_id).program = Some(program);
    }

    fn entry(&mut self, pane_id: PaneId) -> &mut PaneEnd {
        let at = match self.ends.iter().position(|end| end.pane_id == pane_id) {
            Some(at) => at,
            None => {
                if self.ends.len() == KEPT {
                    self.ends.pop_front();
                }
                self.ends.push_back(PaneEnd {
                    pane_id,
                    closed: None,
                    program: None,
                });
                self.ends.len() - 1
            }
        };
        &mut self.ends[at]
    }
}

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// The furthest a lineage goes back from the process it starts at.
const LINEAGE: usize = 6;

/// Process `pid` and the processes that started it, nearest first, up to
/// [`LINEAGE`] of them or the first process of the system, as this host
/// shows them; empty when it shows no process `pid`.
pub fn lineage(pid: u32) -> Vec<ProcessSeen> {
    let mut seen = Vec::new();
    let mut next = Some(pid);
    while let Some(pid) = next {
        if seen.len() == LINEAGE || pid == 0 {
            break;
        }
        let Some((parent, argv)) = procinfo::LocalProcessInfo::parent_and_argv(pid) else {
            break;
        };
        seen.push(ProcessSeen { pid, argv });
        next = (pid != 1).then_some(parent);
    }
    seen
}

impl ClientSeen {
    /// Client `client` as it asked: its lineage when it runs on this host.
    pub fn of(client: &ClientId) -> Self {
        let here = hostname::get()
            .map(|name| name.to_string_lossy().to_string())
            .ok();
        let lineage = if here.as_deref() == Some(client.hostname.as_str()) {
            lineage(client.pid)
        } else {
            Vec::new()
        };
        Self {
            hostname: client.hostname.clone(),
            username: client.username.clone(),
            pid: client.pid,
            lineage,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn closed(why: ClosedFor) -> Closed {
        Closed {
            why,
            client: None,
            signal: None,
            at_ms: 1,
        }
    }

    #[test]
    fn the_first_reason_a_pane_is_closed_for_is_kept() {
        let mut ends = PaneEnds::default();
        ends.closing(3, closed(ClosedFor::Tab));
        ends.closing(3, closed(ClosedFor::Window));
        assert_eq!(ends.get(3).unwrap().closed.unwrap().why, ClosedFor::Tab);
    }

    #[test]
    fn a_program_end_joins_the_reason_its_pane_was_closed_for() {
        let mut ends = PaneEnds::default();
        ends.closing(3, closed(ClosedFor::Pane));
        ends.program_ended(
            3,
            ProgramEnd {
                pid: Some(40),
                code: None,
                signal: Some(1),
                at_ms: 2,
            },
        );
        let end = ends.get(3).unwrap();
        assert_eq!(end.closed.unwrap().why, ClosedFor::Pane);
        assert_eq!(end.program.unwrap().signal, Some(1));
        assert_eq!(ends.get(4), None);
    }

    #[test]
    fn past_the_ends_kept_the_oldest_is_forgotten() {
        let mut ends = PaneEnds::default();
        for pane in 0..=KEPT {
            ends.closing(pane as PaneId, closed(ClosedFor::Pane));
        }
        assert_eq!(ends.get(0), None);
        assert!(ends.get(1).is_some());
        assert!(ends.get(KEPT as PaneId).is_some());
    }

    #[test]
    fn this_process_lineage_starts_at_it_and_names_its_program() {
        let seen = lineage(std::process::id());
        assert_eq!(seen[0].pid, std::process::id());
        let program = std::env::current_exe().unwrap();
        let first = std::path::Path::new(&seen[0].argv[0]);
        assert_eq!(first.file_name(), program.file_name());
        assert!(seen.len() >= 2, "it has a parent: {:?}", seen);
    }
}
