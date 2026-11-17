//! When to run: after local changes, regularly, and again after failures.
//!
//! Pure logic over monotonic times, so it is tested without waiting; the
//! loop in `daemon.rs` feeds it events and sleeps until [`Schedule::next_run`].

use std::time::{Duration, Instant};

/// How often the daemon syncs (decision 0018). Every run scans the whole
/// folder and lists the whole server, so runs are spaced out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// After a local change, wait until the folder has been quiet this long
    /// (the settle time: a file changed this long ago is ready to upload).
    pub quiet: Duration,
    /// …but no longer than this after the first change, so a folder that
    /// keeps changing still syncs.
    pub max_delay: Duration,
    /// Never start a run sooner than this after the previous one ended.
    pub min_gap: Duration,
    /// Run at least this often without any local change: changes on iCloud
    /// are seen only by listing it, and the full scan is the truth about the
    /// local folder.
    pub poll: Duration,
    /// Wait after a failed run (offline, folder missing, …); doubles with
    /// every further failure, up to `retry_max`.
    pub retry: Duration,
    /// The longest wait after failures.
    pub retry_max: Duration,
    /// After a run that changed something, run again at once (after
    /// `min_gap`) to check it settled; at most this many times in a row.
    pub follow_ups: u32,
    /// Daily upkeep this often (also at the first run): forget every
    /// remembered hash, so the next scan reads every file
    /// (`ikeloud_local::HashCache`), and purge the local history.
    pub upkeep: Duration,
    /// How long replaced versions stay in the local history (spec: 30 days).
    pub keep_history: Duration,
}

impl Timing {
    /// The defaults (decision 0018). `poll` and the retries are to be tuned
    /// against the real server in milestone 0.
    pub const DEFAULT: Self = Self {
        quiet: Duration::from_secs(5),
        max_delay: Duration::from_secs(30),
        min_gap: Duration::from_secs(10),
        // A minute: with CloudKit's changes an unchanged iCloud costs one
        // request (decision 0025); before, every poll walked all folders.
        poll: Duration::from_secs(60),
        retry: Duration::from_secs(30),
        retry_max: Duration::from_secs(30 * 60),
        follow_ups: 3,
        upkeep: Duration::from_secs(24 * 60 * 60),
        keep_history: ikeloud_local::HISTORY_KEPT,
    };
}

impl Default for Timing {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What a run means for the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    /// Nothing to do: both sides agree.
    Idle,
    /// Actions were carried out (or the server changed under the plan):
    /// check again soon.
    Progress,
    /// Files were still being written: run again when they have settled.
    Busy,
    /// Nothing could be done for now (offline, folder missing, state
    /// database, …): retry with growing waits.
    Failed,
}

/// The schedule of sync runs.
#[derive(Debug, Clone)]
pub struct Schedule {
    timing: Timing,
    /// The next run for its own reasons (start, poll, follow-up, retry).
    due: Instant,
    /// When the last run ended.
    last_end: Option<Instant>,
    /// First and last local change not covered by a run yet.
    changes: Option<(Instant, Instant)>,
    /// Failed runs in a row.
    failures: u32,
    /// Follow-ups left before falling back to the poll interval.
    follow_ups: u32,
}

impl Schedule {
    /// A schedule whose first run is due at `now`.
    #[must_use]
    pub fn new(timing: Timing, now: Instant) -> Self {
        Self {
            timing,
            due: now,
            last_end: None,
            changes: None,
            failures: 0,
            follow_ups: timing.follow_ups,
        }
    }

    /// The timing in use.
    #[must_use]
    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    /// Failed runs in a row.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// The local folder changed at `now` (a hint from inotify).
    pub fn local_change(&mut self, now: Instant) {
        self.changes = Some(match self.changes {
            Some((first, _)) => (first, now),
            None => (now, now),
        });
    }

    /// Run as soon as allowed: the user asked, confirmed a mass change, or
    /// inotify lost events. Also cuts a wait after failures short.
    pub fn run_soon(&mut self, now: Instant) {
        self.due = self.due.min(now);
    }

    /// When the next run should start.
    #[must_use]
    pub fn next_run(&self) -> Instant {
        let mut next = self.due;
        // A local change cannot be synced while runs fail (the server or
        // the folder is unavailable): it waits for the retry.
        if self.failures == 0
            && let Some((first, last)) = self.changes
        {
            let settled = last + self.timing.quiet;
            next = next.min(settled.min(first + self.timing.max_delay));
        }
        match self.last_end {
            Some(end) => next.max(end + self.timing.min_gap),
            None => next,
        }
    }

    /// A run that started at `started` ended at `ended` with `result`.
    pub fn ran(&mut self, started: Instant, ended: Instant, result: RunResult) {
        self.last_end = Some(ended);
        // Changes before the run started are in its scan (watches are
        // renewed before the scan); later ones still need a run.
        if self.changes.is_some_and(|(_, last)| last < started) {
            self.changes = None;
        }
        let t = self.timing;
        self.due = match result {
            RunResult::Idle => {
                self.failures = 0;
                self.follow_ups = t.follow_ups;
                ended + t.poll
            }
            RunResult::Progress => {
                self.failures = 0;
                if self.follow_ups > 0 {
                    self.follow_ups -= 1;
                    ended
                } else {
                    // Not settled after several runs: something keeps
                    // changing; back to the regular interval.
                    ended + t.poll
                }
            }
            RunResult::Busy => {
                self.failures = 0;
                ended + t.quiet
            }
            RunResult::Failed => {
                self.failures = self.failures.saturating_add(1);
                ended + backoff(t, self.failures)
            }
        };
    }
}

/// The wait after `failures` failed runs in a row: `retry`, doubling, at
/// most `retry_max`.
fn backoff(t: Timing, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(31);
    t.retry
        .checked_mul(1 << doublings)
        .unwrap_or(t.retry_max)
        .min(t.retry_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A poll interval longer than the scenarios below, so they test the
    // local-change rules alone.
    const T: Timing = Timing {
        poll: Duration::from_secs(300),
        ..Timing::DEFAULT
    };

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn first_run_at_once_then_every_poll_interval() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        assert_eq!(s.next_run(), t0);
        s.ran(t0, t0 + secs(1), RunResult::Idle);
        assert_eq!(s.next_run(), t0 + secs(1) + T.poll);
    }

    #[test]
    fn a_local_change_runs_when_the_folder_is_quiet() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        s.ran(t0, t0, RunResult::Idle);
        s.local_change(t0 + secs(60));
        assert_eq!(s.next_run(), t0 + secs(65));
        s.local_change(t0 + secs(63));
        assert_eq!(s.next_run(), t0 + secs(68), "quiet again from the last");
    }

    #[test]
    fn a_folder_that_keeps_changing_still_syncs() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        s.ran(t0, t0, RunResult::Idle);
        for n in 0..100 {
            s.local_change(t0 + secs(60 + n));
        }
        assert_eq!(s.next_run(), t0 + secs(60) + T.max_delay);
    }

    #[test]
    fn runs_keep_a_gap() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        s.ran(t0, t0 + secs(1), RunResult::Idle);
        s.local_change(t0 + secs(1));
        s.run_soon(t0 + secs(2));
        assert_eq!(s.next_run(), t0 + secs(1) + T.min_gap);
    }

    #[test]
    fn a_change_during_a_run_needs_another_run() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        s.local_change(t0 + secs(1));
        s.local_change(t0 + secs(3));
        s.ran(t0 + secs(2), t0 + secs(4), RunResult::Idle);
        assert_eq!(s.next_run(), t0 + secs(4) + T.min_gap, "the change at 3 s");
        s.ran(t0 + secs(14), t0 + secs(15), RunResult::Idle);
        assert_eq!(s.next_run(), t0 + secs(15) + T.poll, "covered now");
    }

    #[test]
    fn progress_is_followed_up_a_few_times() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        let mut end = t0;
        for _ in 0..T.follow_ups {
            s.ran(end, end, RunResult::Progress);
            assert_eq!(s.next_run(), end + T.min_gap);
            end += T.min_gap;
        }
        s.ran(end, end, RunResult::Progress);
        assert_eq!(s.next_run(), end + T.poll, "not settling: back to polling");
        s.ran(end, end, RunResult::Idle);
        s.ran(end, end, RunResult::Progress);
        assert_eq!(s.next_run(), end + T.min_gap, "follow-ups are back");
    }

    #[test]
    fn busy_files_are_retried_once_settled() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        s.ran(t0, t0, RunResult::Busy);
        assert_eq!(s.next_run(), t0 + T.min_gap.max(T.quiet));
    }

    #[test]
    fn failures_back_off_and_ignore_local_changes() {
        let t0 = Instant::now();
        let mut s = Schedule::new(T, t0);
        let mut waits = vec![];
        let mut end = t0;
        for _ in 0..8 {
            s.ran(end, end, RunResult::Failed);
            s.local_change(end + secs(1));
            let next = s.next_run();
            waits.push((next - end).as_secs());
            end = next;
        }
        assert_eq!(waits, [30, 60, 120, 240, 480, 960, 1800, 1800]);
        assert_eq!(s.failures(), 8);
        s.ran(end, end, RunResult::Failed);
        assert_eq!(s.next_run(), end + T.retry_max);
        s.run_soon(end + secs(5));
        assert_eq!(s.next_run(), end + T.min_gap, "the user can cut it short");
        s.ran(end, end, RunResult::Idle);
        assert_eq!(s.failures(), 0);
    }

    #[test]
    fn backoff_never_overflows() {
        let t = Timing {
            retry: Duration::MAX,
            ..T
        };
        assert_eq!(backoff(t, u32::MAX), t.retry_max);
        assert_eq!(backoff(T, 0), T.retry);
        assert_eq!(Timing::default(), Timing::DEFAULT);
    }
}
