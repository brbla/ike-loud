//! ike loud daemon: background sync.
//!
//! Milestone 1: read-only. Downloads iCloud Drive into the synced folder and
//! never writes to the server (`Mode::ReadOnly`, the safety layer defers
//! every server write).
//!
//! Started by the GUI (later a systemd user service with core dumps off, or
//! the Background portal in Flatpak). Reads the folder from the settings and
//! the session saved by the GUI's sign-in (`ikeloud_config::Paths`), and
//! writes what it does to the status file the GUI shows. Exit codes:
//! 0 stopped, 2 not set up, 3 signed out, 1 other error.

use std::cell::Cell;
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::SystemTime;

use ikeloud_api::{ApiError, Client, HttpsTransport, SavedSession};
use ikeloud_config::{Paths, Settings, State, Status as FileStatus, write_private};
use ikeloud_daemon::{
    Daemon, Download, ICloudRemote, Listing, Remote, RemoteError, RunOutcome, Status, Timing,
    Written, spawn_watcher,
};
use ikeloud_i18n::{Translations, languages_from_env};
use ikeloud_local::{LocalDisk, trash_date};
use ikeloud_safety::{
    ApprovedAction, DEFAULT_SETTLE_TIME, Mode, ReadOnlyReason, Rejection, RelPath,
};
use ikeloud_state::StateDb;
use ikeloud_sync::StopReason;

fn main() -> ExitCode {
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(_) => return ExitCode::from(2),
    };
    let report = |state: State, detail: &str| {
        let _ = FileStatus::now(state, detail, 0, 0).save(&paths.status);
    };
    report(State::Starting, "");

    let Some(folder) = Settings::load(&paths.settings).ok().and_then(|s| s.folder) else {
        report(State::Error, "not set up: choose a folder in the app");
        return ExitCode::from(2);
    };
    let Some(saved) = std::fs::read_to_string(&paths.session)
        .ok()
        .and_then(|t| SavedSession::from_json(&t))
    else {
        report(State::SignedOut, "");
        return ExitCode::from(3);
    };
    let mut client = Client::restore(HttpsTransport::new(), saved);
    match client.validate() {
        // Offline at start: the saved addresses are enough, the loop retries.
        Ok(()) | Err(ApiError::Transport(_)) => {}
        Err(ApiError::SessionExpired) => {
            report(State::SignedOut, "");
            return ExitCode::from(3);
        }
        Err(e) => {
            report(State::Error, &e.to_string());
            return ExitCode::FAILURE;
        }
    }
    if client.account().is_none() {
        report(State::SignedOut, "");
        return ExitCode::from(3);
    }
    let _ = write_private(&paths.session, client.saved().to_json().as_bytes());

    if let Err(e) = std::fs::create_dir_all(&folder) {
        report(State::Error, &format!("cannot create the folder: {e}"));
        return ExitCode::FAILURE;
    }
    let db = match StateDb::open(&paths.state_db) {
        Ok(db) => db,
        Err(e) => {
            report(State::Error, &format!("state database: {e:?}"));
            return ExitCode::FAILURE;
        }
    };
    let translations = Translations::new(&languages_from_env()).ok();
    let conflict_name = |path: &RelPath, n: u32| {
        let today: String = trash_date(SystemTime::now()).chars().take(10).collect();
        match &translations {
            Some(t) => t.conflict_copy_name(path, &today, n),
            None => path.sibling(&format!("{} (conflict {today} {n})", path.name())),
        }
    };
    let setup = ikeloud_daemon::SyncSetup {
        disk: LocalDisk::new(folder.clone(), paths.trash.clone(), paths.history.clone()),
        settle_time: DEFAULT_SETTLE_TIME,
        conflict_name: &conflict_name,
        mode: Mode::ReadOnly(ReadOnlyReason::FirstDownload),
    };

    let (events, receive) = mpsc::channel();
    let watches = spawn_watcher(&folder, events.clone()).ok();
    let session_file = paths.session.clone();
    let remote = ICloudRemote::new(
        client,
        Box::new(move |saved: &SavedSession| {
            let _ = write_private(&session_file, saved.to_json().as_bytes());
        }),
    );
    let counts = Cell::new((0u64, 0u64));
    let mut remote = Reporting {
        inner: remote,
        status: &paths.status,
        counts: &counts,
    };
    let mut daemon = Daemon::new(setup, db, watches, Timing::DEFAULT);
    let status_path = paths.status.clone();
    let mut on_status = |status: Status| {
        let (state, detail) = describe(&status);
        let (files, downloaded) = counts.get();
        let _ = FileStatus::now(state, detail, files, downloaded).save(&status_path);
    };
    // The loop ends when every event sender is gone; the watcher keeps one.
    let _keep = events;
    daemon.run(&mut remote, &receive, &mut on_status);
    ExitCode::SUCCESS
}

/// What a finished run means for the user.
fn describe(status: &Status) -> (State, String) {
    match &status.outcome {
        Err(e) => (State::Error, format!("state database: {e:?}")),
        Ok(RunOutcome::Ran(report)) => {
            if let Some(stop) = report.executed.stopped {
                return stopped(stop);
            }
            let waiting = report.deferred.len();
            if !report.executed.done.is_empty() {
                (State::Syncing, String::new())
            } else if waiting > 0 {
                (
                    State::Idle,
                    format!("{waiting} local changes are not uploaded (read-only)"),
                )
            } else {
                (State::Idle, String::new())
            }
        }
        Ok(RunOutcome::Rejected(Rejection::MassChangeNeedsConfirmation(_))) => {
            (State::NeedsConfirmation, String::new())
        }
        Ok(RunOutcome::Rejected(r)) => {
            (State::Error, format!("stopped by the safety layer: {r:?}"))
        }
        Ok(RunOutcome::ScanFailed(e)) => {
            (State::Error, format!("the folder cannot be read: {e:?}"))
        }
        Ok(RunOutcome::RemoteUnavailable(stop)) => stopped(*stop),
    }
}

fn stopped(stop: StopReason) -> (State, String) {
    match stop {
        StopReason::Offline => (State::Offline, String::new()),
        StopReason::SignedOut => (State::SignedOut, String::new()),
        StopReason::NoSpace => (State::Error, "no space left".to_string()),
        StopReason::UnknownResponse => (
            State::Error,
            "iCloud answered something unexpected".to_string(),
        ),
        StopReason::LocalDisk => (State::Error, "the disk refused a write".to_string()),
    }
}

/// Writes "syncing" when a run starts talking to the server, and counts
/// files listed and downloaded for the status written after the run.
struct Reporting<'a, R: Remote> {
    inner: R,
    status: &'a Path,
    /// (files on iCloud, files downloaded since start)
    counts: &'a Cell<(u64, u64)>,
}

impl<R: Remote> Reporting<'_, R> {
    fn save(&self, detail: &str) {
        let (files, downloaded) = self.counts.get();
        let _ = FileStatus::now(State::Syncing, detail, files, downloaded).save(self.status);
    }
}

impl<R: Remote> Remote for Reporting<'_, R> {
    fn list(&mut self) -> Result<Listing, StopReason> {
        self.save("");
        let listing = self.inner.list()?;
        let (_, downloaded) = self.counts.get();
        self.counts.set((
            u64::try_from(listing.files.len()).unwrap_or(u64::MAX),
            downloaded,
        ));
        Ok(listing)
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let download = self.inner.download(path)?;
        let (files, downloaded) = self.counts.get();
        self.counts.set((files, downloaded.saturating_add(1)));
        self.save(path.as_str());
        Ok(download)
    }

    fn write(
        &mut self,
        action: ApprovedAction,
        content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        self.inner.write(action, content)
    }
}
