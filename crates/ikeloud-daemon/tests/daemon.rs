//! The daemon's loop in real time: the real watcher, schedule and runs
//! against the fake iCloud, with short timings.

#![allow(clippy::expect_used, clippy::panic)] // test code

mod common;

use std::io::Read;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use common::{FakeICloud, Fixture, copy_name};
use ikeloud_daemon::{
    Daemon, Download, Event, Listing, Remote, RemoteError, RunOutcome, Status, SyncSetup, Timing,
    Written, spawn_watcher,
};
use ikeloud_local::LocalDisk;
use ikeloud_safety::{ApprovedAction, RelPath};
use ikeloud_state::StateDb;
use ikeloud_sync::StopReason;

const WAIT: Duration = Duration::from_secs(10);

/// The fake iCloud, shared between the daemon's thread and the test.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<FakeICloud>>);

impl Shared {
    fn with<T>(&self, f: impl FnOnce(&mut FakeICloud) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Remote for Shared {
    fn list(&mut self) -> Result<Listing, StopReason> {
        self.with(Remote::list)
    }
    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        self.with(|i| i.download(path))
    }
    fn write(
        &mut self,
        action: ApprovedAction,
        content: Option<&mut dyn Read>,
    ) -> Result<Option<Written>, RemoteError> {
        self.with(|i| i.write(action, content))
    }
}

/// What the test sees of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    done: usize,
    ok: bool,
    failures: u32,
}

fn seen(s: &Status) -> Seen {
    let (done, ok) = match &s.outcome {
        Ok(RunOutcome::Ran(r)) => (r.executed.done.len(), r.executed.stopped.is_none()),
        _ => (0, false),
    };
    Seen {
        done,
        ok,
        failures: s.failures,
    }
}

struct Running {
    /// Removes the folders when dropped.
    _fixture: Fixture,
    fixture_dir: std::path::PathBuf,
    icloud: Shared,
    events: Sender<Event>,
    runs: Receiver<Seen>,
    thread: JoinHandle<()>,
}

impl Running {
    fn start(name: &str, watch: bool, offline: bool) -> Self {
        let mut fixture = Fixture::new(name);
        let dir = fixture.dir.clone();
        let db = StateDb::open_in_memory().expect("db");
        let icloud = Shared(Arc::new(Mutex::new(std::mem::take(&mut fixture.icloud))));
        if offline {
            icloud.with(|i| i.offline_after = Some(0));
        }
        let (events, receiver) = mpsc::channel();
        let watches =
            watch.then(|| spawn_watcher(&dir.join("root"), events.clone()).expect("inotify"));
        let (seen_tx, runs) = mpsc::channel();
        let (thread_dir, mut remote) = (dir.clone(), icloud.clone());
        let thread = std::thread::spawn(move || {
            let setup = SyncSetup {
                disk: LocalDisk::new(
                    thread_dir.join("root"),
                    thread_dir.join("trash"),
                    thread_dir.join("history"),
                ),
                settle_time: Duration::from_millis(200),
                conflict_name: &copy_name,
            };
            let timing = Timing {
                quiet: Duration::from_millis(300),
                max_delay: Duration::from_secs(2),
                min_gap: Duration::from_millis(100),
                // Never within a test: every run here has another reason.
                poll: Duration::from_secs(3600),
                retry: Duration::from_millis(100),
                retry_max: Duration::from_millis(400),
                follow_ups: 3,
                rehash: Duration::from_secs(3600),
            };
            let mut daemon = Daemon::new(setup, db, watches, timing);
            daemon.run(&mut remote, &receiver, &mut |s| {
                let _ = seen_tx.send(seen(&s));
            });
        });
        Self {
            _fixture: fixture,
            fixture_dir: dir,
            icloud,
            events,
            runs,
            thread,
        }
    }

    fn write(&self, rel: &str, content: &[u8]) {
        std::fs::write(self.fixture_dir.join("root").join(rel), content).expect("write");
    }

    fn next_run(&self) -> Seen {
        self.runs.recv_timeout(WAIT).expect("a run")
    }

    /// Runs until one that did nothing.
    fn until_idle(&self) -> Vec<Seen> {
        let mut runs = vec![];
        loop {
            let s = self.next_run();
            runs.push(s);
            if s.done == 0 && s.ok {
                return runs;
            }
        }
    }

    fn stop(self) -> FakeICloud {
        self.events.send(Event::Stop).expect("daemon alive");
        self.thread.join().expect("daemon thread");
        std::mem::take(&mut *self.icloud.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[test]
fn a_local_change_is_synced_without_waiting_for_the_poll() {
    let d = Running::start("loop-watch", true, false);
    d.icloud.with(|i| i.put("from-iphone.txt", b"i"));
    let first = d.until_idle();
    assert_eq!(first[0].done, 1, "the first run starts at once: {first:?}");

    d.write("from-linux.txt", b"l");
    let runs = d.until_idle();
    assert!(runs.iter().any(|s| s.done == 1), "uploaded: {runs:?}");
    assert!(
        d.runs.recv_timeout(Duration::from_secs(1)).is_err(),
        "then quiet until the poll"
    );
    let icloud = d.stop();
    assert_eq!(icloud.content("from-linux.txt"), Some(&b"l"[..]));
}

#[test]
fn without_hints_only_the_user_or_the_poll_starts_a_run() {
    let d = Running::start("loop-nowatch", false, false);
    d.until_idle();
    d.write("a.txt", b"x");
    assert!(
        d.runs.recv_timeout(Duration::from_secs(1)).is_err(),
        "no watcher: nothing notices"
    );
    d.events.send(Event::SyncNow).expect("send");
    let runs = d.until_idle();
    assert_eq!(runs[0].done, 1, "{runs:?}");
    assert!(d.stop().content("a.txt").is_some());
}

#[test]
fn an_unreachable_server_is_retried_with_growing_waits() {
    let d = Running::start("loop-offline", true, true);
    let failures: Vec<u32> = (0..4).map(|_| d.next_run().failures).collect();
    assert_eq!(failures, [1, 2, 3, 4]);
    d.write("a.txt", b"x");
    d.icloud.with(|i| i.offline_after = None);
    let runs = d.until_idle();
    assert_eq!(runs.last().map(|s| s.failures), Some(0), "{runs:?}");
    assert!(d.stop().content("a.txt").is_some());
}
