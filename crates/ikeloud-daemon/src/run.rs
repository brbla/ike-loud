//! One sync run.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ikeloud_local::{HashCache, LocalDisk, ScanError, Skipped, scan_with};
use ikeloud_safety::{
    Action, BRAKE_WINDOW, Context, Decision, DeferredAction, Etag, LocalFolderState,
    MassChangeConfirmation, Mode, RecentChanges, Rejection, RelPath, assess,
};
use ikeloud_state::{BaseChange, JournalEntry, JournalKind, StateDb, StateError};
use ikeloud_sync::names::key;
use ikeloud_sync::{
    BaseEntry, Collision, DocId, FolderStates, RemoteFile, Report, States, StopReason, Transfer,
    build_plan, execute, next_base, next_base_folders, plan_folders,
};

use crate::effects::{RunEffects, unix_secs};
use crate::remote::{Listing, Remote};

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
    /// common state from a partial scan is not; the recorded transfers wait
    /// for the next run. (A server that cannot be listed after the run is
    /// predicted from what the run did.)
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
    /// Two-way, or read-only: the safety layer then defers every server
    /// write (first download, settings). Read at the start of every run, so
    /// it can change while the daemon runs.
    pub mode: &'a std::cell::Cell<Mode>,
}

/// Runs one sync against `remote`: [`start_run`], then
/// [`ExecutedRun::finish`].
///
/// `confirmation` is the user's answer to an earlier
/// [`Rejection::MassChangeNeedsConfirmation`]. `hashes` are the content
/// hashes of earlier scans (an empty cache reads every file). Errors are
/// errors of the state database only.
pub fn sync_once(
    setup: &SyncSetup<'_>,
    remote: &mut dyn Remote,
    db: &mut StateDb,
    hashes: &mut HashCache,
    confirmation: Option<MassChangeConfirmation>,
    now: SystemTime,
) -> Result<RunOutcome, StateError> {
    match start_run(setup, remote, db, hashes, confirmation, now)? {
        Started::Executed(run) => run.finish(setup, remote, db, hashes),
        Started::Ended(outcome) => Ok(outcome),
    }
}

/// The first part of a run: scan, list, plan, assess, carry out. Every
/// carried-out action is recorded at once ([`StateDb::record`]): a crash
/// after this part forgets nothing an action proved.
pub enum Started {
    /// Actions were carried out; [`ExecutedRun::finish`] stores the new
    /// common state.
    Executed(ExecutedRun),
    /// The run ended before carrying out anything.
    Ended(RunOutcome),
}

/// A run whose actions are done and recorded, but whose new common state is
/// not stored yet.
#[derive(Debug)]
pub struct ExecutedRun {
    base: Vec<BaseEntry>,
    remote_before: Vec<RemoteFile>,
    executed: Report,
    deferred: Vec<DeferredAction>,
    collisions: Vec<Collision>,
    busy: Vec<RelPath>,
    skipped: Vec<Skipped>,
    now: SystemTime,
    /// Folders of the common state before the run (`None`: never recorded).
    base_folders: Option<Vec<RelPath>>,
}

/// How long an upload without an answer waits for a listing that shows
/// it; after that it is taken for one that never reached iCloud.
const UNCONFIRMED_KEPT: u64 = 24 * 60 * 60;

/// Uploads whose last step got no answer (decision 0026): a listing that
/// shows their document with the uploaded content (iCloud's checksum)
/// proves the upload, as its answer would have; it becomes a recorded
/// transfer, which goes into the common state below like the transfers of
/// a run that crashed. Without that, the plan would take the file for one
/// new on both sides and make a conflict copy of it. The transfer keeps the
/// path the upload went to, so a move on iCloud since then is seen as one.
/// Listings without checksums (the folders walked) settle nothing.
fn settle_unconfirmed(
    db: &mut StateDb,
    listing: &Listing,
    now: SystemTime,
) -> Result<(), StateError> {
    for (seq, upload) in db.unconfirmed()? {
        let file = listing.files.iter().find(|f| f.doc_id == upload.doc_id);
        let checksum = file.and_then(|f| listing.checksums.get(&f.path));
        match (file, checksum) {
            (Some(file), Some(checksum)) if *checksum == upload.checksum => {
                let transfer = Transfer {
                    path: upload.path.clone(),
                    doc_id: upload.doc_id,
                    etag: file.etag.clone(),
                    local: upload.local,
                    hash: upload.hash,
                };
                let entry = JournalEntry {
                    time: upload.time,
                    kind: JournalKind::Uploaded,
                    path: upload.path,
                    target: None,
                };
                db.settle_unconfirmed(seq, Some((&entry, &transfer)))?;
            }
            // Another content: the upload did not land (or was replaced).
            (Some(_), Some(_)) => db.settle_unconfirmed(seq, None)?,
            // Not there, and another document holds its path (the file was
            // uploaded again): it did not land.
            (None, _) if listing.files.iter().any(|f| f.path == upload.path) => {
                db.settle_unconfirmed(seq, None)?;
            }
            _ if unix_secs(now).saturating_sub(upload.time) > UNCONFIRMED_KEPT => {
                db.settle_unconfirmed(seq, None)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether `r`, listed under another version than `known`, has the same
/// content iCloud had at `known` (its checksum): a rename or move on
/// iCloud gives a document a new version (seen live), the content stays
/// (decision 0025 step 3).
fn same_content(
    known: &Etag,
    r: &RemoteFile,
    checksums: &BTreeMap<RelPath, String>,
    stored: &BTreeMap<DocId, (Etag, String)>,
) -> bool {
    r.etag != *known
        && stored
            .get(&r.doc_id)
            .is_some_and(|(etag, sum)| etag == known && checksums.get(&r.path) == Some(sum))
}

/// Common-state entries whose document is listed under a new version with
/// the same content, with that version.
fn same_content_new_versions(
    base: &[BaseEntry],
    remote: &[RemoteFile],
    checksums: &BTreeMap<RelPath, String>,
    stored: &BTreeMap<DocId, (Etag, String)>,
) -> Vec<BaseEntry> {
    let by_doc: BTreeMap<&DocId, &RemoteFile> = remote.iter().map(|f| (&f.doc_id, f)).collect();
    base.iter()
        .filter_map(|b| {
            let r = by_doc.get(&b.doc_id)?;
            same_content(&b.etag, r, checksums, stored).then(|| BaseEntry {
                etag: r.etag.clone(),
                ..b.clone()
            })
        })
        .collect()
}

/// Transfers whose document is listed under a new version with the same
/// content take that version: what the transfer proved still holds (a
/// file moved on iCloud right after its upload is not new on both sides).
fn refresh_transfers(
    transfers: &mut [Transfer],
    remote: &[RemoteFile],
    checksums: &BTreeMap<RelPath, String>,
    stored: &BTreeMap<DocId, (Etag, String)>,
) {
    let by_doc: BTreeMap<&DocId, &RemoteFile> = remote.iter().map(|f| (&f.doc_id, f)).collect();
    for t in transfers {
        if let Some(r) = by_doc.get(&t.doc_id)
            && same_content(&t.etag, r, checksums, stored)
        {
            t.etag = r.etag.clone();
        }
    }
}

/// The checksum of every listed document at its listed version.
fn listed_checksums(
    remote: &[RemoteFile],
    checksums: &BTreeMap<RelPath, String>,
) -> BTreeMap<DocId, (Etag, String)> {
    remote
        .iter()
        .filter_map(|f| {
            Some((
                f.doc_id.clone(),
                (f.etag.clone(), checksums.get(&f.path)?.clone()),
            ))
        })
        .collect()
}

/// The first part of a run, see [`Started`].
pub fn start_run(
    setup: &SyncSetup<'_>,
    remote: &mut dyn Remote,
    db: &mut StateDb,
    hashes: &mut HashCache,
    confirmation: Option<MassChangeConfirmation>,
    now: SystemTime,
) -> Result<Started, StateError> {
    let (disk, settle_time) = (&setup.disk, setup.settle_time);
    let local = match scan_with(disk.root(), now, hashes) {
        Ok(s) => s,
        Err(ScanError::Missing | ScanError::NotADirectory) => {
            return Ok(Started::Ended(RunOutcome::Rejected(
                Rejection::LocalFolderUnavailable,
            )));
        }
        Err(e) => return Ok(Started::Ended(RunOutcome::ScanFailed(e))),
    };
    let listing = match remote.list() {
        Ok(l) => l,
        Err(reason) => return Ok(Started::Ended(RunOutcome::RemoteUnavailable(reason))),
    };
    settle_unconfirmed(db, &listing, now)?;
    let remote_checksums = listing.checksums.clone();
    let base_folders = db.base_folders()?;
    let folder_states = FolderStates {
        local: local.folders,
        remote: listing.folder_etags.clone(),
        base: base_folders.clone().unwrap_or_default(),
    };
    let mut states = States {
        local: local.files,
        remote: listing.files,
        remote_folders: listing.folders,
        base: db.base()?,
    };
    // Transfers recorded by a run that crashed before storing its common
    // state: they go into the common state first, or the plan would take
    // the files they synced for new files on both sides.
    let stored = db.remote_checksums()?;
    let mut pending = db.pending()?;
    refresh_transfers(&mut pending, &states.remote, &remote_checksums, &stored);
    if !pending.is_empty() {
        let recovered = next_base(&states.base, &states, &pending);
        db.commit(&changes(&states.base, &recovered), &[])?;
        states.base = recovered;
    }
    // A new version with the content the common state knows (a rename or
    // move on iCloud changes the version, decision 0025 step 3): the common
    // state takes the new version, so nothing is downloaded again.
    let same = same_content_new_versions(&states.base, &states.remote, &remote_checksums, &stored);
    if !same.is_empty() {
        let changes: Vec<BaseChange> = same.iter().cloned().map(BaseChange::Set).collect();
        db.commit(&changes, &[])?;
        for entry in same {
            if let Some(b) = states.base.iter_mut().find(|b| b.doc_id == entry.doc_id) {
                *b = entry;
            }
        }
    }
    // What iCloud says each document's content is now: known for the next
    // listing (and for this run's own downloads). Not from a listing
    // without checksums (the folders walked).
    if !remote_checksums.is_empty() {
        db.set_remote_checksums(&listed_checksums(&states.remote, &remote_checksums))?;
    }
    let mut plan = build_plan(&states, settle_time, setup.conflict_name);
    // Folders (decision 0027); in a database from before, not until they
    // were recorded once: a folder on one side may be one deleted on the
    // other. (A new database knows folders from the start.)
    if base_folders.is_some() {
        let folders = plan_folders(&folder_states, &states, &plan.actions);
        plan.actions.splice(0..0, folders.create);
        plan.actions.extend(folders.remove);
    }

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
    context.remote_folder_etags = listing.folder_etags;
    context.settle_time = settle_time;
    context.mode = setup.mode.get();
    context.recent = recent_changes(db, now)?;
    context.confirmation = confirmation;

    let (approved, deferred, confirmed) = match assess(plan.actions, &context) {
        Ok(Decision::NothingToDo) => (vec![], vec![], false),
        Ok(Decision::Approved(p)) => (p.actions, p.deferred, p.confirmed),
        Err(rejection) => return Ok(Started::Ended(RunOutcome::Rejected(rejection))),
    };
    let first_path = approved.first().map(|a| a.action().path().clone());

    let mut effects = RunEffects {
        disk,
        remote,
        db,
        now,
        record_failed: false,
        unconfirmed: std::sync::Mutex::new(Vec::new()),
        checksums: std::sync::Mutex::new(Vec::new()),
    };
    let executed = execute(approved, &mut effects);
    // The brake window starts again after what the user confirmed, and only
    // if the confirmation covered the plan (decision 0020, found by the TLA+
    // model SyncBrake): recorded before the actions, the confirmed changes
    // would count against the next plan; recorded for a plan it does not
    // cover, deletions the user never saw would not count. A crash before
    // this line only makes the brake count the confirmed changes once more.
    if confirmed && let Some(path) = first_path {
        db.record(
            &JournalEntry {
                time: unix_secs(now),
                kind: JournalKind::MassChangeConfirmed,
                path,
                target: None,
            },
            None,
        )?;
    }
    Ok(Started::Executed(ExecutedRun {
        base: states.base,
        remote_before: states.remote,
        executed,
        deferred,
        collisions: plan.collisions,
        busy: plan.busy,
        skipped: local.skipped,
        now,
        base_folders,
    }))
}

impl ExecutedRun {
    /// The second part of a run: the new common state from a fresh look at
    /// both sides and every transfer recorded since the last commit (also of
    /// an earlier run that crashed before this point), stored in one
    /// transaction.
    pub fn finish(
        self,
        setup: &SyncSetup<'_>,
        remote: &mut dyn Remote,
        db: &mut StateDb,
        hashes: &mut HashCache,
    ) -> Result<RunOutcome, StateError> {
        // If the server cannot be listed now (it went offline during the
        // run), its last listing with what this run did to it stands in:
        // what the transfers prove must not be forgotten, or the next run
        // would take every file uploaded before the outage for a new file
        // on both sides and make conflict copies of them. Should the
        // prediction be off, the next real listing shows a change on
        // iCloud, which is handled like any other.
        let listed = remote.list().ok();
        let after_folders = listed.as_ref().map(|r| r.folders.clone());
        let after_checksums = listed.as_ref().map(|r| r.checksums.clone());
        let after_remote = listed
            .map(|r| r.files)
            .unwrap_or_else(|| predicted_remote(&self.remote_before, &self.executed));
        let committed = match scan_with(setup.disk.root(), self.now, hashes) {
            Ok(l) => {
                // Without a fresh listing the folders stay as they were;
                // the next run sees the difference and records it.
                if let Some(remote_folders) = &after_folders {
                    let old = self.base_folders.clone().unwrap_or_default();
                    db.set_base_folders(&next_base_folders(&old, &l.folders, remote_folders))?;
                }
                let after = States {
                    local: l.files,
                    remote: after_remote,
                    remote_folders: vec![],
                    base: vec![],
                };
                let mut transfers = db.pending()?;
                if let Some(checksums) = &after_checksums {
                    let stored = db.remote_checksums()?;
                    refresh_transfers(&mut transfers, &after.remote, checksums, &stored);
                }
                let new_base = next_base(&self.base, &after, &transfers);
                db.commit(&changes(&self.base, &new_base), &[])?;
                // What iCloud says each document's content is now (from a
                // fresh listing with checksums only).
                if let Some(checksums) = after_checksums.as_ref().filter(|c| !c.is_empty()) {
                    db.set_remote_checksums(&listed_checksums(&after.remote, checksums))?;
                }
                true
            }
            // The folder cannot be read again: the old common state stays
            // and the recorded transfers wait for the next run.
            Err(_) => false,
        };
        Ok(RunOutcome::Ran(RunReport {
            executed: self.executed,
            deferred: self.deferred,
            collisions: self.collisions,
            busy: self.busy,
            skipped: self.skipped,
            committed,
        }))
    }
}

/// The server as it must be after the actions this run did: the listing
/// from before, with each done server action applied and every version a
/// transfer saw. Transfers come in the order of the done actions that make
/// them (uploads and downloads).
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
            // A download read the document at the version it reports (the
            // user may have changed it since the listing).
            Action::Download { .. } => {
                if let Some(t) = transfers.next() {
                    for f in files.iter_mut().filter(|f| f.doc_id == t.doc_id) {
                        f.etag = t.etag.clone();
                    }
                }
            }
            Action::TrashRemote { path, .. } => files.retain(|f| &f.path != path),
            Action::MoveRemote { from, to, .. } => {
                for f in files.iter_mut().filter(|f| &f.path == from) {
                    f.path = to.clone();
                }
            }
            // Empty when trashed; whatever is listed under it went with it.
            Action::TrashRemoteFolder { path, .. } => {
                let prefix = format!("{}/", path.as_str());
                files.retain(|f| !f.path.as_str().starts_with(&prefix));
            }
            Action::CreateRemoteFolder { .. }
            | Action::TrashLocal { .. }
            | Action::MoveLocal { .. }
            | Action::CreateLocalFolder { .. }
            | Action::RemoveLocalFolder { .. } => {}
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
            | JournalKind::OverwrittenLocal
            | JournalKind::FolderRemovedLocal
            | JournalKind::FolderTrashedRemote => recent.destructive += 1,
            JournalKind::MovedRemote | JournalKind::MovedLocal => recent.moves += 1,
            JournalKind::Uploaded
            | JournalKind::Downloaded
            | JournalKind::ConflictCopy
            | JournalKind::FolderCreated
            | JournalKind::FolderCreatedLocal
            | JournalKind::CheckDiffers => {}
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
