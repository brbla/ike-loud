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
use std::sync::{Arc, Mutex, mpsc};
use std::time::SystemTime;

use ikeloud_api::{ApiError, Client, HttpsTransport, SavedSession};
use ikeloud_config::{
    Paths, PendingChange, PendingChanges, Request, Settings, State, Status as FileStatus,
    write_private,
};
use ikeloud_daemon::{
    Daemon, Download, Event, ICloudRemote, Listing, Remote, RemoteError, RunOutcome, Status,
    Timing, Written, spawn_watcher,
};
use ikeloud_i18n::{Translations, languages_from_env};
use ikeloud_local::{LocalDisk, trash_date};
use ikeloud_safety::{
    ApprovedAction, ChangeKind, DEFAULT_SETTLE_TIME, MassChangeSummary, Mode, ReadOnlyReason,
    Rejection, RelPath,
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
    // Writes only after the first download is complete, and only when the
    // user turned uploads on (decision 0023). Decided again after every run.
    let first_done = paths.state_db.with_file_name("first-download-done");
    let mode = Cell::new(sync_mode(
        Settings::load(&paths.settings)
            .map(|s| s.upload)
            .unwrap_or(false),
        first_done.exists(),
    ));
    let setup = ikeloud_daemon::SyncSetup {
        disk: LocalDisk::new(folder.clone(), paths.trash.clone(), paths.history.clone()),
        settle_time: DEFAULT_SETTLE_TIME,
        conflict_name: &conflict_name,
        mode: &mode,
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
    // Local changes the last run deferred (read-only: never uploaded).
    let pending = Cell::new(0u64);
    let mut remote = Reporting {
        inner: remote,
        status: &paths.status,
        counts: &counts,
        pending: &pending,
    };
    let mut daemon = Daemon::new(setup, db, watches, Timing::DEFAULT);
    let status_path = paths.status.clone();
    // A mass change waiting for the user, and the GUI's requests.
    let waiting: Arc<Mutex<Option<(String, MassChangeSummary)>>> = Arc::new(Mutex::new(None));
    spawn_requests(paths.request.clone(), events.clone(), Arc::clone(&waiting));
    let pending_path = paths.pending.clone();
    let settings_path = paths.settings.clone();
    let mut on_status = |status: Status| {
        let (state, detail) = describe(&status);
        match &status.outcome {
            Ok(RunOutcome::Rejected(Rejection::MassChangeNeedsConfirmation(summary))) => {
                let pending = pending_changes(summary);
                let _ = pending.save(&pending_path);
                if let Ok(mut w) = waiting.lock() {
                    *w = Some((pending.id, summary.clone()));
                }
            }
            _ => {
                if let Ok(mut w) = waiting.lock() {
                    *w = None;
                }
                let _ = std::fs::remove_file(&pending_path);
            }
        }
        if let Ok(RunOutcome::Ran(report)) = &status.outcome {
            pending.set(u64::try_from(report.deferred.len()).unwrap_or(u64::MAX));
            if first_download_complete(report) && !first_done.exists() {
                let _ = write_private(&first_done, b"1\n");
            }
        }
        mode.set(sync_mode(
            Settings::load(&settings_path)
                .map(|s| s.upload)
                .unwrap_or(false),
            first_done.exists(),
        ));
        let (files, downloaded) = counts.get();
        let mut file = FileStatus::now(state, detail, files, downloaded);
        file.local_pending = pending.get();
        file.writes = mode.get() == Mode::Full;
        let _ = file.save(&status_path);
    };
    // The loop ends when every event sender is gone; the watcher keeps one.
    let _keep = events;
    daemon.run(&mut remote, &receive, &mut on_status);
    ExitCode::SUCCESS
}

/// Takes the GUI's requests (a file, once a second) and turns them into
/// events. A confirmation counts only for the mass change it names: the one
/// the user saw.
fn spawn_requests(
    path: std::path::PathBuf,
    events: mpsc::Sender<Event>,
    waiting: Arc<Mutex<Option<(String, MassChangeSummary)>>>,
) {
    let _ = std::thread::Builder::new()
        .name("ikeloud-requests".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let event = match Request::take(&path) {
                    Some(Request::SyncNow) => Event::SyncNow,
                    Some(Request::Confirm(id)) => {
                        let confirmed = waiting.lock().ok().and_then(|w| {
                            w.as_ref()
                                .filter(|(current, _)| *current == id)
                                .map(|(_, summary)| summary.confirm())
                        });
                        match confirmed {
                            Some(c) => Event::Confirm(c),
                            None => continue,
                        }
                    }
                    None => continue,
                };
                if events.send(event).is_err() {
                    return;
                }
            }
        });
}

/// The mass change as the GUI shows it, with an id bound to exactly these
/// changes (the files, their versions and destinations).
fn pending_changes(summary: &MassChangeSummary) -> PendingChanges {
    let mut seed = String::new();
    let files = summary
        .files
        .iter()
        .map(|change| {
            seed.push_str(&format!(
                "{:?}\u{0}{}\u{0}{:?}\u{0}{}\n",
                change.kind,
                change.path.as_str(),
                change.to.as_ref().map(RelPath::as_str),
                change.version
            ));
            PendingChange {
                kind: match change.kind {
                    ChangeKind::TrashRemote => "trash-remote",
                    ChangeKind::OverwriteRemote => "overwrite-remote",
                    ChangeKind::TrashLocal => "trash-local",
                    ChangeKind::OverwriteLocal => "overwrite-local",
                    ChangeKind::MoveRemote => "move-remote",
                    ChangeKind::MoveLocal => "move-local",
                }
                .to_string(),
                path: change.path.as_str().to_string(),
                to: change.to.as_ref().map(|t| t.as_str().to_string()),
            }
        })
        .collect();
    PendingChanges {
        id: ikeloud_hash::hex(&ikeloud_hash::sha256(&[seed.as_bytes()])),
        destructive: summary.destructive(),
        moves: summary.moves(),
        total: summary.total,
        files,
    }
}

/// Two-way only after the first download and with uploads turned on.
fn sync_mode(upload: bool, first_download_done: bool) -> Mode {
    if !first_download_done {
        Mode::ReadOnly(ReadOnlyReason::FirstDownload)
    } else if !upload {
        Mode::ReadOnly(ReadOnlyReason::Settings)
    } else {
        Mode::Full
    }
}

/// The first download is complete when a run had nothing left to do from
/// the server's side: it stopped for nothing, carried out nothing (the
/// earlier runs did), and skipped no download. Deferred actions are the
/// uploads read-only mode holds back.
fn first_download_complete(report: &ikeloud_daemon::RunReport) -> bool {
    report.executed.stopped.is_none()
        && report.executed.done.is_empty()
        && report.executed.skipped.is_empty()
        && report.executed.not_attempted.is_empty()
        && report.busy.is_empty()
}

/// What a finished run means for the user.
fn describe(status: &Status) -> (State, String) {
    match &status.outcome {
        Err(e) => (State::Error, format!("state database: {e:?}")),
        Ok(RunOutcome::Ran(report)) => {
            if let Some(stop) = report.executed.stopped {
                return stopped(stop);
            }
            // Deferred local changes go to the status as a number.
            if report.executed.done.is_empty() {
                (State::Idle, String::new())
            } else {
                (State::Syncing, String::new())
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
    /// Local changes not uploaded, from the last run.
    pending: &'a Cell<u64>,
}

impl<R: Remote> Reporting<'_, R> {
    fn save(&self, detail: &str) {
        let (files, downloaded) = self.counts.get();
        let mut file = FileStatus::now(State::Syncing, detail, files, downloaded);
        file.local_pending = self.pending.get();
        let _ = file.save(self.status);
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
