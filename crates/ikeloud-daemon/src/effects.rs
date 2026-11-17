//! The effects of a sync run: local actions on the disk, server actions
//! through [`Remote`], each recorded in the journal when done.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use ikeloud_hash::HashingReader;
use ikeloud_local::{DiskError, LocalDisk};
use ikeloud_safety::{Action, ApprovedAction, ContentHash, LocalId, RelPath};
use ikeloud_state::{JournalEntry, JournalKind, StateDb};
use ikeloud_sync::{ApplyResult, EffectError, Effects, StopReason, Transfer};

use crate::remote::{Remote, RemoteError};

pub(crate) struct RunEffects<'a> {
    pub(crate) disk: &'a LocalDisk,
    pub(crate) remote: &'a mut dyn Remote,
    pub(crate) db: &'a mut StateDb,
    pub(crate) now: SystemTime,
    /// An action could not be recorded: no further action is carried out,
    /// so nothing more happens that the state does not know.
    pub(crate) record_failed: bool,
}

impl From<RemoteError> for EffectError {
    fn from(e: RemoteError) -> Self {
        match e {
            RemoteError::Changed => Self::Changed,
            RemoteError::Stop(reason) => Self::Stop(reason),
        }
    }
}

fn disk_error(e: DiskError) -> EffectError {
    match e {
        DiskError::Changed => EffectError::Changed,
        DiskError::Io(std::io::ErrorKind::StorageFull) => EffectError::Stop(StopReason::NoSpace),
        DiskError::Io(_) => EffectError::Stop(StopReason::LocalDisk),
    }
}

/// What carrying out one action needs; the main thread's, or a worker's
/// with its own fork of the remote.
struct Ctx<'a> {
    disk: &'a LocalDisk,
    remote: &'a mut dyn Remote,
    now: SystemTime,
}

/// Uploads the local file `path` (still `id`) with an approved action.
fn upload(
    ctx: &mut Ctx<'_>,
    action: ApprovedAction,
    path: &RelPath,
    remote_path: RelPath,
    id: LocalId,
) -> Result<Option<Transfer>, EffectError> {
    // Replacing a version on the server: keep that version in the local
    // history first, and only if it is still the planned one.
    if let Action::Upload {
        expected_etag: Some(expected),
        remote,
        ..
    } = action.action()
    {
        let mut previous = ctx.remote.download(remote)?;
        if &previous.etag != expected {
            return Err(EffectError::Changed);
        }
        ctx.disk
            .keep_remote_version(remote, &mut previous.content, ctx.now)
            .map_err(disk_error)?;
    }
    let file = ctx.disk.open(path, id).map_err(disk_error)?;
    let mut reader = HashingReader::new(file);
    let written = ctx
        .remote
        .write(action, Some(&mut reader))?
        .ok_or(EffectError::Stop(StopReason::UnknownResponse))?;
    Ok(Some(Transfer {
        path: remote_path,
        doc_id: written.doc_id,
        etag: written.etag,
        local: id,
        hash: ContentHash::new(ikeloud_hash::hex(&reader.finish())),
    }))
}

fn perform(ctx: &mut Ctx<'_>, approved: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
    match approved.action().clone() {
        Action::Upload {
            local,
            remote,
            local_id,
            ..
        } => upload(ctx, approved, &local, remote, local_id),
        Action::UploadConflictCopy { path, local_id, .. } => {
            upload(ctx, approved, &path, path.clone(), local_id)
        }
        Action::CreateRemoteFolder { .. }
        | Action::TrashRemote { .. }
        | Action::MoveRemote { .. } => {
            ctx.remote.write(approved, None)?;
            Ok(None)
        }
        Action::Download {
            remote,
            expected_etag,
            local,
            replaces,
            ..
        } => {
            let mut download = ctx.remote.download(&remote)?;
            // Another version, or another document moved to this path
            // since the plan: write nothing, the next run plans again.
            if download.etag != expected_etag {
                return Err(EffectError::Changed);
            }
            let (id, hash) = ctx
                .disk
                .write(&local, &mut download.content, replaces.as_ref(), ctx.now)
                .map_err(disk_error)?;
            Ok(Some(Transfer {
                path: remote,
                doc_id: download.doc_id,
                etag: download.etag,
                local: id,
                hash,
            }))
        }
        Action::TrashLocal { path, expected } => {
            ctx.disk
                .trash(&path, &expected, ctx.now)
                .map_err(disk_error)?;
            Ok(None)
        }
        Action::MoveLocal { from, to, local_id } => {
            ctx.disk
                .move_file(&from, &to, local_id)
                .map_err(disk_error)?;
            Ok(None)
        }
    }
}

impl Effects for RunEffects<'_> {
    fn apply(&mut self, approved: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        if self.record_failed {
            return Err(EffectError::Stop(StopReason::LocalDisk));
        }
        let action = approved.action().clone();
        let mut ctx = Ctx {
            disk: self.disk,
            remote: &mut *self.remote,
            now: self.now,
        };
        let result = perform(&mut ctx, approved);
        if let Ok(transfer) = &result {
            // Recorded at once: a crash from here on forgets nothing. The
            // time of the action itself (a run can take long).
            let entry = journal_entry(&action, unix_secs(SystemTime::now()));
            if self.db.record(&entry, transfer.as_ref()).is_err() {
                self.record_failed = true;
            }
        }
        result
    }

    /// Independent actions on up to [`WORKERS`] threads, each with its own
    /// fork of the remote. Every result is recorded on this thread as soon
    /// as it arrives, so a crash forgets nothing that was done; a stop (or a
    /// failed record) lets the workers finish what they hold and take
    /// nothing new.
    fn apply_group(&mut self, actions: Vec<ApprovedAction>) -> Vec<Option<ApplyResult>> {
        let count = actions.len();
        let mut forks = Vec::new();
        while forks.len() < WORKERS.min(count) {
            match self.remote.fork() {
                Some(fork) => forks.push(fork),
                None => break,
            }
        }
        if forks.len() < 2 || self.record_failed {
            let mut results = Vec::with_capacity(count);
            let mut stopped = false;
            for action in actions {
                if stopped {
                    results.push(None);
                    continue;
                }
                let result = self.apply(action);
                stopped = matches!(result, Err(EffectError::Stop(_)));
                results.push(Some(result));
            }
            return results;
        }
        let queue: Mutex<VecDeque<(usize, ApprovedAction)>> =
            Mutex::new(actions.into_iter().enumerate().collect());
        let stop = AtomicBool::new(false);
        let mut results: Vec<Option<ApplyResult>> = (0..count).map(|_| None).collect();
        let (disk, now) = (self.disk, self.now);
        let (sender, receiver) = mpsc::channel();
        std::thread::scope(|scope| {
            for mut fork in forks {
                let (sender, queue, stop) = (sender.clone(), &queue, &stop);
                scope.spawn(move || {
                    let mut ctx = Ctx {
                        disk,
                        remote: &mut *fork,
                        now,
                    };
                    while !stop.load(Ordering::SeqCst) {
                        let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                        let Some((index, approved)) = next else { break };
                        let action = approved.action().clone();
                        let result = perform(&mut ctx, approved);
                        if matches!(result, Err(EffectError::Stop(_))) {
                            stop.store(true, Ordering::SeqCst);
                        }
                        if sender.send((index, action, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            for (index, action, result) in receiver {
                if let Ok(transfer) = &result {
                    let entry = journal_entry(&action, unix_secs(SystemTime::now()));
                    if self.db.record(&entry, transfer.as_ref()).is_err() {
                        self.record_failed = true;
                        stop.store(true, Ordering::SeqCst);
                    }
                    if let Action::Upload { remote: path, .. }
                    | Action::UploadConflictCopy { path, .. } = &action
                    {
                        self.remote.uploaded_elsewhere(path);
                    }
                }
                if let Some(slot) = results.get_mut(index) {
                    *slot = Some(result);
                }
            }
        });
        results
    }
}

/// Actions carried out at once (decision 0025: four writes at a time are
/// twice as fast; eight make iCloud answer 503 RETRY).
const WORKERS: usize = 4;

pub(crate) fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The journal entry of a carried-out action.
pub(crate) fn journal_entry(action: &Action, time: u64) -> JournalEntry {
    let (kind, path, target) = match action {
        Action::Upload {
            remote,
            expected_etag: Some(_),
            ..
        } => (JournalKind::OverwrittenRemote, remote, None),
        Action::Upload { remote, .. } => (JournalKind::Uploaded, remote, None),
        Action::UploadConflictCopy { path, .. } => (JournalKind::ConflictCopy, path, None),
        Action::CreateRemoteFolder { path } => (JournalKind::FolderCreated, path, None),
        Action::TrashRemote { path, .. } => (JournalKind::TrashedRemote, path, None),
        Action::MoveRemote { from, to, .. } => (JournalKind::MovedRemote, from, Some(to)),
        Action::Download {
            local,
            replaces: Some(_),
            ..
        } => (JournalKind::OverwrittenLocal, local, None),
        Action::Download { local, .. } => (JournalKind::Downloaded, local, None),
        Action::TrashLocal { path, .. } => (JournalKind::TrashedLocal, path, None),
        Action::MoveLocal { from, to, .. } => (JournalKind::MovedLocal, from, Some(to)),
    };
    JournalEntry {
        time,
        kind,
        path: path.clone(),
        target: target.cloned(),
    }
}
