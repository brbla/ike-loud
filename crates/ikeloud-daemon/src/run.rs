//! One sync run.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ikeloud_local::{LocalDisk, ScanError, Skipped, scan};
use ikeloud_safety::{
    Action, BRAKE_WINDOW, Context, Decision, DeferredAction, LocalFolderState,
    MassChangeConfirmation, RecentChanges, Rejection, RelPath, assess,
};
use ikeloud_state::{BaseChange, JournalEntry, JournalKind, StateDb, StateError};
use ikeloud_sync::names::key;
use ikeloud_sync::{
    BaseEntry, Collision, RemoteFile, Report, States, StopReason, build_plan, execute, next_base,
};

use crate::effects::{RunEffects, unix_secs};
use crate::remote::Remote;

/// How a run ended.
#[derive(Debug)]
pub enum RunOutcome {
    /// The plan was assessed and the approved actions carried out (maybe
    /// none, maybe not all: see the report).
    Ran(RunReport),
    /// The safety layer stopped the whole plan; nothing was changed. A mass
    /// change waits for the user's confirmation (pass it to the next run).
    Rejected(Rejection),
    /// The local folder could not be read completely; nothing was changed.
    ScanFailed(ScanError),
    /// The server could not be listed; nothing was changed.
    RemoteUnavailable(StopReason),
}

/// What a run did and what it left alone.
#[derive(Debug)]
pub struct RunReport {
    /// The executor's report.
    pub executed: Report,
    /// Actions the safety layer deferred, with the reasons.
    pub deferred: Vec<DeferredAction>,
    /// Names in a collision, left alone.
    pub collisions: Vec<Collision>,
    /// Local files still being written, left alone this run.
    pub busy: Vec<RelPath>,
    /// Local entries that are not synced (links, special files, …).
    pub skipped: Vec<Skipped>,
    /// Whether the new common state was stored. Not when the folder could
    /// not be read again after the run: the old common state is safe, a
    /// common state from a partial scan is not. (A server that cannot be
    /// listed after the run is predicted from what the run did.)
    pub committed: bool,
}

/// What stays the same between runs.
pub struct SyncSetup<'a> {
    /// The synced folder, its trash and local history.
    pub disk: LocalDisk,
    /// How long a local file must be unchanged before it is synced.
    pub settle_time: Duration,
    /// Names conflict copies: `(path, n)` → the name (translated, see
    /// `ikeloud_i18n`), `None` if the translation gives no usable name.
    pub conflict_name: &'a dyn Fn(&RelPath, u32) -> Option<RelPath>,
}

/// Runs one sync against `remote`.
///
/// `confirmation` is the user's answer to an earlier
/// [`Rejection::MassChangeNeedsConfirmation`]. Errors are errors of the state
/// database only: nothing was committed.
pub fn sync_once(
    setup: &SyncSetup<'_>,
    remote: &mut dyn Remote,
    db: &mut StateDb,
    confirmation: Option<MassChangeConfirmation>,
    now: SystemTime,
) -> Result<RunOutcome, StateError> {
    let (disk, settle_time) = (&setup.disk, setup.settle_time);
    let root = disk.root();
    let local = match scan(root, now) {
        Ok(s) => s,
        Err(ScanError::Missing | ScanError::NotADirectory) => {
            return Ok(RunOutcome::Rejected(Rejection::LocalFolderUnavailable));
        }
        Err(e) => return Ok(RunOutcome::ScanFailed(e)),
    };
    let listing = match remote.list() {
        Ok(l) => l,
        Err(reason) => return Ok(RunOutcome::RemoteUnavailable(reason)),
    };
    let base = db.base()?;
    let states = States {
        local: local.files,
        remote: listing.files,
        remote_folders: listing.folders,
        base,
    };
    let plan = build_plan(&states, settle_time, setup.conflict_name);

    let mut context = Context::new(count(states.base.len()), key);
    context.local_folder = LocalFolderState::Available {
        file_count: count(states.local.len()),
    };
    context.remote_item_count = count(states.remote.len());
    context.remote_etags = states
        .remote
        .iter()
        .map(|f| (f.path.clone(), f.etag.clone()))
        .collect();
    context.settle_time = settle_time;
    context.recent = recent_changes(db, now)?;
    let confirmed = confirmation.is_some();
    context.confirmation = confirmation;

    let (approved, deferred) = match assess(plan.actions, &context) {
        Ok(Decision::NothingToDo) => (vec![], vec![]),
        Ok(Decision::Approved(p)) => (p.actions, p.deferred),
        Err(rejection) => return Ok(RunOutcome::Rejected(rejection)),
    };

    let mut effects = RunEffects {
        disk,
        remote,
        now,
        journal: vec![],
    };
    let executed = execute(approved, &mut effects);
    let mut journal = effects.journal;
    if confirmed && let Some(first) = journal.first() {
        journal.insert(
            0,
            JournalEntry {
                time: first.time,
                kind: JournalKind::MassChangeConfirmed,
                path: first.path.clone(),
                target: None,
            },
        );
    }

    // The new common state from a fresh look at both sides. If the server
    // cannot be listed now (it went offline during the run), its last
    // listing with what this run did to it stands in: what the transfers
    // prove must not be forgotten, or the next run would take every file
    // uploaded before the outage for a new file on both sides and make
    // conflict copies of them. Should the prediction be off, the next real
    // listing shows a change on iCloud, which is handled like any other.
    let after_remote = remote
        .list()
        .map(|r| r.files)
        .unwrap_or_else(|_| predicted_remote(&states.remote, &executed));
    let committed = match scan(root, now) {
        Ok(l) => {
            let after = States {
                local: l.files,
                remote: after_remote,
                remote_folders: vec![],
                base: vec![],
            };
            let new_base = next_base(&states.base, &after, &executed.transfers);
            db.commit(&changes(&states.base, &new_base), &journal)?;
            true
        }
        // The folder cannot be read again: only the journal is stored. The
        // old common state stays and the next run plans from it.
        Err(_) => {
            db.commit(&[], &journal)?;
            false
        }
    };
    Ok(RunOutcome::Ran(RunReport {
        executed,
        deferred,
        collisions: plan.collisions,
        busy: plan.busy,
        skipped: local.skipped,
        committed,
    }))
}

/// The server as it must be after the actions this run did: the listing
/// from before, with each done server action applied. Transfers come in the
/// order of the done actions that make them (uploads and downloads).
fn predicted_remote(before: &[RemoteFile], report: &Report) -> Vec<RemoteFile> {
    let mut files = before.to_vec();
    let mut transfers = report.transfers.iter();
    for action in &report.done {
        match action {
            Action::Upload { remote, size, .. } => {
                if let Some(t) = transfers.next() {
                    files.retain(|f| f.doc_id != t.doc_id);
                    files.push(RemoteFile {
                        path: remote.clone(),
                        doc_id: t.doc_id.clone(),
                        etag: t.etag.clone(),
                        size: *size,
                    });
                }
            }
            Action::UploadConflictCopy { path, size, .. } => {
                if let Some(t) = transfers.next() {
                    files.push(RemoteFile {
                        path: path.clone(),
                        doc_id: t.doc_id.clone(),
                        etag: t.etag.clone(),
                        size: *size,
                    });
                }
            }
            Action::Download { .. } => {
                transfers.next();
            }
            Action::TrashRemote { path, .. } => files.retain(|f| &f.path != path),
            Action::MoveRemote { from, to, .. } => {
                for f in files.iter_mut().filter(|f| &f.path == from) {
                    f.path = to.clone();
                }
            }
            Action::CreateRemoteFolder { .. }
            | Action::TrashLocal { .. }
            | Action::MoveLocal { .. } => {}
        }
    }
    files
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Changes carried out in the brake window: the last day, but not before the
/// user last confirmed a mass change.
fn recent_changes(db: &StateDb, now: SystemTime) -> Result<RecentChanges, StateError> {
    let since = unix_secs(
        now.checked_sub(BRAKE_WINDOW)
            .unwrap_or(SystemTime::UNIX_EPOCH),
    );
    let mut recent = RecentChanges::default();
    for entry in db.journal_since(since)? {
        match entry.kind {
            JournalKind::MassChangeConfirmed => break,
            JournalKind::TrashedRemote
            | JournalKind::TrashedLocal
            | JournalKind::OverwrittenRemote
            | JournalKind::OverwrittenLocal => recent.destructive += 1,
            JournalKind::MovedRemote | JournalKind::MovedLocal => recent.moves += 1,
            JournalKind::Uploaded
            | JournalKind::Downloaded
            | JournalKind::ConflictCopy
            | JournalKind::FolderCreated => {}
        }
    }
    Ok(recent)
}

/// What changed between two common states, as database changes.
fn changes(old: &[BaseEntry], new: &[BaseEntry]) -> Vec<BaseChange> {
    let old: BTreeMap<_, _> = old.iter().map(|b| (b.doc_id.clone(), b)).collect();
    let new_docs: BTreeMap<_, _> = new.iter().map(|b| (b.doc_id.clone(), b)).collect();
    let mut changes: Vec<BaseChange> = old
        .keys()
        .filter(|doc| !new_docs.contains_key(*doc))
        .map(|doc| BaseChange::Forget(doc.clone()))
        .collect();
    changes.extend(
        new.iter()
            .filter(|b| old.get(&b.doc_id) != Some(b))
            .map(|b| BaseChange::Set(b.clone())),
    );
    changes
}
