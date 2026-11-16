//! The server stops on SIGTERM, SIGINT or SIGHUP by closing its panes
//! first. Each pane's end then says the server was exiting, and which signal
//! and which sender stopped it (`wsb-wezterm cli pane-end`), and the server
//! stays to answer that until every pane's program has ended, or for
//! [`WAIT`] at most, before it exits.

use mux::pane_end::{lineage, SignalSeen};
use mux::Mux;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// The longest the server waits for the programs of its panes to end once
/// it is stopping.
pub const WAIT: Duration = Duration::from_secs(10);

/// The first stop signal, its sender, and the pipe the handler wakes the
/// watcher through.
static SIGNAL: AtomicI32 = AtomicI32::new(0);
static SENDER: AtomicI32 = AtomicI32::new(0);
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn stop(signal: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    // Only the first stop signal counts: the server is stopping already.
    if SIGNAL
        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    // SAFETY: the kernel hands a valid siginfo to an SA_SIGINFO handler;
    // reading its sender touches no other memory.
    let sender = if info.is_null() {
        0
    } else {
        unsafe { (*info).si_pid() }
    };
    SENDER.store(sender, Ordering::SeqCst);
    let fd = WAKE.load(Ordering::SeqCst);
    // SAFETY: write(2) is async-signal-safe, and the byte outlives the call.
    unsafe {
        libc::write(fd, b"s".as_ptr() as *const libc::c_void, 1);
    }
}

/// Take over SIGTERM, SIGINT and SIGHUP, and stop the server as this module
/// says when one arrives.
pub fn watch() -> anyhow::Result<()> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe(2) writes two descriptors into `fds`.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        anyhow::bail!(
            "cannot make the pipe the server stops through: {}",
            std::io::Error::last_os_error()
        );
    }
    for fd in fds {
        // SAFETY: fcntl on a descriptor this function owns.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    WAKE.store(fds[1], Ordering::SeqCst);
    let read_end = fds[0];
    std::thread::Builder::new()
        .name("stopping".to_string())
        .spawn(move || {
            let mut byte = 0u8;
            loop {
                // SAFETY: reads one byte into `byte`.
                let read = unsafe { libc::read(read_end, &mut byte as *mut u8 as *mut _, 1) };
                if read == 1 {
                    break;
                }
                if read < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                log::error!(
                    "the server can no longer hear a stop signal: {}",
                    std::io::Error::last_os_error()
                );
                return;
            }
            stop_now();
        })?;
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: installs a handler that touches only atomics and write(2).
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = stop as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                anyhow::bail!(
                    "cannot take over signal {signal}: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }
    Ok(())
}

/// Close every pane for the signal that arrived, wait for their programs,
/// and exit.
fn stop_now() {
    let number = SIGNAL.load(Ordering::SeqCst);
    let sender = u32::try_from(SENDER.load(Ordering::SeqCst))
        .ok()
        .filter(|pid| *pid > 0);
    let seen = SignalSeen {
        number,
        sender,
        lineage: sender.map(lineage).unwrap_or_default(),
    };
    log::info!(
        "stopping on signal {number}{}: closing every pane",
        match sender {
            Some(pid) => format!(" from process {pid}"),
            None => String::new(),
        }
    );
    let mux = Mux::get();
    let running = mux.close_every_pane_for_exit(seen);
    let deadline = Instant::now() + WAIT;
    while !running.iter().all(|pane| mux.pane_program_ended(*pane)) {
        if Instant::now() > deadline {
            let left: Vec<String> = running
                .iter()
                .filter(|pane| !mux.pane_program_ended(**pane))
                .map(|pane| pane.to_string())
                .collect();
            log::warn!(
                "the programs of panes {} did not end within {} s of their hangup; exiting",
                left.join(", "),
                WAIT.as_secs()
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    wezterm_blob_leases::clear_storage();
    std::process::exit(0);
}
