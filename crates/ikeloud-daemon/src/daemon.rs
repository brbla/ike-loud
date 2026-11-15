//! The daemon's loop: wait for the next run (see [`Schedule`]), renew the
//! watches, run, report; in between, take events: change hints from the
//! watcher thread and requests from the user.

use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Instant, SystemTime};

use ikeloud_local::{Coverage, HashCache, Hint, Watcher, Watches};
use ikeloud_safety::{DeferReason, MassChangeConfirmation, Rejection};
use ikeloud_state::{StateDb, StateError};

use crate::remote::Remote;
use crate::run::{RunOutcome, SyncSetup, sync_once};
use crate::schedule::{RunResult, Schedule, Timing};

/// What the loop reacts to besides time.
#[derive(Debug)]
pub enum Event {
    /// The local folder changed (a hint).
    LocalChange,
    /// Change hints were lost: run soon, the scan decides.
    HintsLost,
    /// The user asked to sync now.
    SyncNow,
    /// The user confirmed a mass change; the next run carries it out.
    Confirm(MassChangeConfirmation),
    /// Stop after the current run.
    Stop,
}

/// What one run ended with, for the user interface.
#[derive(Debug)]
pub struct Status {
    /// The run.
    pub outcome: Result<RunOutcome, StateError>,
    /// How much of the folder the change hints cover (`None`: no watcher).
    pub coverage: Option<Coverage>,
    /// Failed runs in a row, this one included.
    pub failures: u32,
}

/// Starts a thread that turns inotify events of the synced folder `root` into
/// [`Event::LocalChange`] and [`Event::HintsLost`]. Returns the watches to
/// hand to [`Daemon::new`]. The thread ends when the receiver is gone (at
/// its next event) or when the kernel stops answering (after sending
/// [`Event::HintsLost`]: the regular runs still see every change).
///
/// # Errors
/// The kernel refused an inotify instance.
pub fn spawn_watcher(root: &Path, events: Sender<Event>) -> std::io::Result<Watches> {
    let (mut watcher, watches) = Watcher::new(root)?;
    std::thread::Builder::new()
        .name("ikeloud-watch".into())
        .spawn(move || {
            loop {
                let event = match watcher.wait() {
                    Ok(Hint::Changed) => Event::LocalChange,
                    Ok(Hint::Lost) => Event::HintsLost,
                    Err(_) => {
                        let _ = events.send(Event::HintsLost);
                        return;
                    }
                };
                if events.send(event).is_err() {
                    return;
                }
            }
        })?;
    Ok(watches)
}

/// The daemon: the state that lives across runs.
pub struct Daemon<'a> {
    setup: SyncSetup<'a>,
    db: StateDb,
    hashes: HashCache,
    watches: Option<Watches>,
    schedule: Schedule,
    rehashed: Instant,
    confirmation: Option<MassChangeConfirmation>,
}

impl<'a> Daemon<'a> {
    /// A daemon whose first run is due at once. `watches` come from
    /// [`spawn_watcher`]; without them only the regular runs see local
    /// changes.
    #[must_use]
    pub fn new(
        setup: SyncSetup<'a>,
        db: StateDb,
        watches: Option<Watches>,
        timing: Timing,
    ) -> Self {
        let now = Instant::now();
        Self {
            setup,
            db,
            hashes: HashCache::default(),
            watches,
            schedule: Schedule::new(timing, now),
            rehashed: now,
            confirmation: None,
        }
    }

    /// Runs until [`Event::Stop`] or until every sender of `events` is gone.
    /// `status` gets the result of every run.
    pub fn run(
        &mut self,
        remote: &mut dyn Remote,
        events: &Receiver<Event>,
        status: &mut dyn FnMut(Status),
    ) {
        loop {
            let now = Instant::now();
            let next = self.schedule.next_run();
            if next > now {
                match events.recv_timeout(next.saturating_duration_since(now)) {
                    Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(event) => {
                        self.take(event);
                        continue;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            // Due: still take what came meanwhile (above all a stop).
            while let Ok(event) = events.try_recv() {
                if matches!(event, Event::Stop) {
                    return;
                }
                self.take(event);
            }
            status(self.run_once(remote));
        }
    }

    fn take(&mut self, event: Event) {
        let now = Instant::now();
        match event {
            Event::LocalChange => self.schedule.local_change(now),
            Event::HintsLost | Event::SyncNow => self.schedule.run_soon(now),
            Event::Confirm(c) => {
                self.confirmation = Some(c);
                self.schedule.run_soon(now);
            }
            Event::Stop => {}
        }
    }

    fn run_once(&mut self, remote: &mut dyn Remote) -> Status {
        let started = Instant::now();
        if started.saturating_duration_since(self.rehashed) >= self.schedule.timing().rehash {
            self.hashes.clear();
            self.rehashed = started;
        }
        // Watches first, then the scan: what changes after this is hinted,
        // what changed before it the scan sees.
        let coverage = self.watches.as_mut().map(Watches::watch_all);
        let outcome = sync_once(
            &self.setup,
            remote,
            &mut self.db,
            &mut self.hashes,
            self.confirmation.clone(),
            SystemTime::now(),
        );
        // A confirmation is used up once a run got past the safety layer;
        // until then (offline, …) it waits for the next run.
        if matches!(outcome, Ok(RunOutcome::Ran(_))) {
            self.confirmation = None;
        }
        self.schedule
            .ran(started, Instant::now(), classify(&outcome));
        Status {
            outcome,
            coverage,
            failures: self.schedule.failures(),
        }
    }
}

/// What a run means for the schedule.
fn classify(outcome: &Result<RunOutcome, StateError>) -> RunResult {
    match outcome {
        Ok(RunOutcome::Ran(r)) if r.executed.stopped.is_some() || !r.committed => RunResult::Failed,
        Ok(RunOutcome::Ran(r)) if !r.executed.done.is_empty() || !r.executed.skipped.is_empty() => {
            RunResult::Progress
        }
        Ok(RunOutcome::Ran(r))
            if !r.busy.is_empty()
                || r.deferred
                    .iter()
                    .any(|d| d.reason == DeferReason::StillBeingWritten) =>
        {
            RunResult::Busy
        }
        Ok(RunOutcome::Ran(_)) => RunResult::Idle,
        // Changed on the server since the listing: plan again.
        Ok(RunOutcome::Rejected(Rejection::Conflict(_))) => RunResult::Progress,
        // Waits for the user; the regular runs check whether it still holds.
        Ok(RunOutcome::Rejected(Rejection::MassChangeNeedsConfirmation(_))) => RunResult::Idle,
        Ok(
            RunOutcome::Rejected(
                Rejection::LocalFolderUnavailable
                | Rejection::SuspiciousRemoteListing
                | Rejection::NotEnoughICloudSpace { .. }
                | Rejection::NotEnoughDiskSpace { .. },
            )
            | RunOutcome::ScanFailed(_)
            | RunOutcome::RemoteUnavailable(_),
        )
        | Err(_) => RunResult::Failed,
    }
}
