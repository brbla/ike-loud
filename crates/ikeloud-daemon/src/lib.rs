//! ike loud daemon: background sync.
//!
//! [`sync_once`] is one sync run: scan the local folder, list the server,
//! plan, let the safety layer assess the plan, carry out the approved
//! actions, and store the new common state and the journal in one
//! transaction. The server is behind [`Remote`]: the real iCloud client, or
//! a fake one in tests.
//!
//! [`Daemon`] repeats runs: after local changes reported by inotify
//! ([`spawn_watcher`]), regularly, and with growing waits after failures, as
//! [`Schedule`] decides with the [`Timing`] of decision 0018.

mod daemon;
mod effects;
mod icloud;
mod remote;
mod run;
mod schedule;
mod tree;

use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Asks the daemon to stop: the actions under way finish and are recorded,
/// no new one starts, the run ends.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Whether a stop was asked for.
pub fn shutting_down() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

pub use daemon::{Daemon, Event, Status, spawn_watcher};
pub use icloud::{ICloudRemote, stop_reason};
pub use remote::{Download, Listing, Remote, RemoteError, Written};
pub use run::{ExecutedRun, RunOutcome, RunReport, Started, SyncSetup, start_run, sync_once};
pub use schedule::{RunResult, Schedule, Timing};
