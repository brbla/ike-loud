//! The effects of a sync run: local actions on the disk, server actions
//! through [`Remote`], each recorded in the journal when done.

use std::time::{SystemTime, UNIX_EPOCH};

use ikeloud_hash::HashingReader;
use ikeloud_local::{DiskError, LocalDisk};
use ikeloud_safety::{Action, ApprovedAction, ContentHash, LocalId, RelPath};
use ikeloud_state::{JournalEntry, JournalKind};
use ikeloud_sync::{EffectError, Effects, StopReason, Transfer};

use crate::remote::{Remote, RemoteError};

pub(crate) struct RunEffects<'a> {
    pub(crate) disk: &'a LocalDisk,
    pub(crate) remote: &'a mut dyn Remote,
    pub(crate) now: SystemTime,
    pub(crate) journal: Vec<JournalEntry>,
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

impl RunEffects<'_> {
    /// Uploads the local file `path` (still `id`) with an approved action.
    fn upload(
        &mut self,
        action: ApprovedAction,
        path: &RelPath,
        id: LocalId,
    ) -> Result<Option<Transfer>, EffectError> {
        let file = self.disk.open(path, id).map_err(disk_error)?;
        let mut reader = HashingReader::new(file);
        let written = self
            .remote
            .write(action, Some(&mut reader))?
            .ok_or(EffectError::Stop(StopReason::UnknownResponse))?;
        Ok(Some(Transfer {
            doc_id: written.doc_id,
            etag: written.etag,
            local: id,
            hash: ContentHash::new(ikeloud_hash::hex(&reader.finish())),
        }))
    }

    fn perform(&mut self, approved: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        match approved.action().clone() {
            Action::Upload {
                local, local_id, ..
            } => self.upload(approved, &local, local_id),
            Action::UploadConflictCopy { path, local_id, .. } => {
                self.upload(approved, &path, local_id)
            }
            Action::CreateRemoteFolder { .. }
            | Action::TrashRemote { .. }
            | Action::MoveRemote { .. } => {
                self.remote.write(approved, None)?;
                Ok(None)
            }
            Action::Download {
                remote,
                local,
                replaces,
                ..
            } => {
                let mut download = self.remote.download(&remote)?;
                let (id, hash) = self
                    .disk
                    .write(&local, &mut download.content, replaces.as_ref(), self.now)
                    .map_err(disk_error)?;
                Ok(Some(Transfer {
                    doc_id: download.doc_id,
                    etag: download.etag,
                    local: id,
                    hash,
                }))
            }
            Action::TrashLocal { path, expected } => {
                self.disk
                    .trash(&path, &expected, self.now)
                    .map_err(disk_error)?;
                Ok(None)
            }
            Action::MoveLocal { from, to, local_id } => {
                self.disk
                    .move_file(&from, &to, local_id)
                    .map_err(disk_error)?;
                Ok(None)
            }
        }
    }
}

impl Effects for RunEffects<'_> {
    fn apply(&mut self, approved: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        let action = approved.action().clone();
        let result = self.perform(approved);
        if result.is_ok() {
            self.journal
                .push(journal_entry(&action, unix_secs(self.now)));
        }
        result
    }
}

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
