//! Actions proposed by the sync engine and assessed by the safety layer.

use std::fmt;
use std::time::Duration;

/// Relative path inside the synced folder, components separated by `/`.
///
/// Name validity is checked by [`crate::names`]; this type only carries it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Creates a path from the text the engine produced.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// The path as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parent folder (empty for the root).
    pub fn parent(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(parent, _)| parent)
    }

    /// Last path component.
    pub fn name(&self) -> &str {
        self.0
            .rsplit_once('/')
            .map_or(self.0.as_str(), |(_, name)| name)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Version of an item on the server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Etag(String);

impl Etag {
    /// Creates an etag from the value the server returned.
    pub fn new(etag: impl Into<String>) -> Self {
        Self(etag.into())
    }

    /// The etag as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Content hash of a local file (checksum).
///
/// A change on Linux is decided by content, never by modification time: the
/// computer's clock can be wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    /// Hash from the text form of a checksum.
    pub fn new(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    /// The hash as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of a local file: stays the same across renames and edits.
///
/// The inode alone is not enough: Linux gives the inode of a deleted file to
/// the next new file, and pairing by inode alone would then treat an
/// unrelated new file as an edit of the deleted one. The birth time (from
/// `statx`) tells the two apart; on filesystems that do not record it,
/// [`LocalId::same_file`] cannot decide and the caller must be careful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId {
    /// Device the file is on.
    pub device: u64,
    /// Inode number on that device.
    pub inode: u64,
    /// Birth time in nanoseconds since the Unix epoch, if the filesystem
    /// records it.
    pub birth_ns: Option<i64>,
}

impl LocalId {
    /// Is `later` (from a later scan) the same file as `self`?
    ///
    /// `Some(false)`: certainly not (another inode, or the inode was reused).
    /// `Some(true)`: certainly yes. `None`: the inode matches but the birth
    /// time is unknown, so a reused inode cannot be ruled out.
    pub fn same_file(&self, later: &Self) -> Option<bool> {
        if (self.device, self.inode) != (later.device, later.inode) {
            return Some(false);
        }
        match (self.birth_ns, later.birth_ns) {
            (Some(a), Some(b)) => Some(a == b),
            _ => None,
        }
    }
}

/// A local file as the engine saw it: which file and what content. Local
/// actions that destroy or replace content carry it, and the effects check
/// it right before acting, so a change made in between is never lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVersion {
    /// Which file.
    pub id: LocalId,
    /// Its content.
    pub hash: ContentHash,
}

/// What a destructive action does to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DestructionKind {
    /// Moves a server file to "Recently Deleted".
    TrashRemote,
    /// Replaces the content of a server file.
    OverwriteRemote,
    /// Moves a local file to the system trash.
    TrashLocal,
    /// Replaces the content of a local file (the old one goes to history).
    OverwriteLocal,
}

/// One file a destructive action deletes or overwrites, with the version it
/// was planned against. The user confirms a mass change as a set of these;
/// the confirmation then covers exactly these versions of these files.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Destruction {
    /// What happens to the file.
    pub kind: DestructionKind,
    /// The file.
    pub path: RelPath,
    /// Server etag or local content hash the action was planned against.
    pub version: String,
}

/// One action of a plan.
///
/// There is deliberately no permanent delete on the server: deleting always
/// moves to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Upload a local file to the server.
    Upload {
        /// Which local file (may differ from `remote` when the file was
        /// also moved; the move follows in a later run).
        local: RelPath,
        /// Where on the server.
        remote: RelPath,
        /// The local file the engine saw there (the effects check it).
        local_id: LocalId,
        /// Server version the engine knows; `None` = the file is not there.
        expected_etag: Option<Etag>,
        /// Size in bytes.
        size: u64,
        /// How long the local file has not changed (size or mtime).
        stable_for: Duration,
    },
    /// Keep the local version as a conflict copy and upload it as a new file.
    UploadConflictCopy {
        /// Name of the conflict copy, locally and on the server.
        path: RelPath,
        /// The local file the engine saw (the effects check it).
        local_id: LocalId,
        /// Size in bytes.
        size: u64,
        /// How long the local file has not changed (size or mtime).
        stable_for: Duration,
    },
    /// Create a folder on the server.
    CreateRemoteFolder {
        /// Path of the new folder.
        path: RelPath,
    },
    /// Move an item on the server to "Recently Deleted".
    TrashRemote {
        /// What to delete.
        path: RelPath,
        /// Version the engine knows.
        expected_etag: Etag,
    },
    /// Rename or move an item on the server (by document ID).
    MoveRemote {
        /// From.
        from: RelPath,
        /// To.
        to: RelPath,
        /// Version the engine knows.
        expected_etag: Etag,
    },
    /// Download a file from the server.
    Download {
        /// Which server file.
        remote: RelPath,
        /// Where locally (may differ from `remote` when the file was also
        /// moved; the move follows in a later run).
        local: RelPath,
        /// Size in bytes.
        size: u64,
        /// The local file it replaces (its version goes to local history);
        /// `None` = the local path must be free.
        replaces: Option<LocalVersion>,
    },
    /// Move a local file to the system trash.
    TrashLocal {
        /// What to delete.
        path: RelPath,
        /// The file and content the engine saw there.
        expected: LocalVersion,
    },
    /// Rename or move a local file.
    MoveLocal {
        /// From.
        from: RelPath,
        /// To.
        to: RelPath,
        /// The file the engine saw at `from`.
        local_id: LocalId,
    },
}

impl Action {
    /// The path the action writes to: the server path for uploads, the
    /// local path for downloads, the target for moves.
    pub fn path(&self) -> &RelPath {
        match self {
            Self::Upload { remote: path, .. }
            | Self::Download { local: path, .. }
            | Self::UploadConflictCopy { path, .. }
            | Self::CreateRemoteFolder { path }
            | Self::TrashRemote { path, .. }
            | Self::TrashLocal { path, .. } => path,
            Self::MoveRemote { to, .. } | Self::MoveLocal { to, .. } => to,
        }
    }

    /// Does the action change anything on the server?
    pub fn writes_remote(&self) -> bool {
        matches!(
            self,
            Self::Upload { .. }
                | Self::UploadConflictCopy { .. }
                | Self::CreateRemoteFolder { .. }
                | Self::TrashRemote { .. }
                | Self::MoveRemote { .. }
        )
    }

    /// What the action deletes or overwrites, if anything. Counted by the
    /// mass-change brake and listed to the user.
    pub fn destruction(&self) -> Option<Destruction> {
        let (kind, path, version) = match self {
            Self::Upload {
                remote,
                expected_etag: Some(etag),
                ..
            } => (DestructionKind::OverwriteRemote, remote, etag.as_str()),
            Self::Download {
                local,
                replaces: Some(old),
                ..
            } => (DestructionKind::OverwriteLocal, local, old.hash.as_str()),
            Self::TrashRemote {
                path,
                expected_etag,
            } => (DestructionKind::TrashRemote, path, expected_etag.as_str()),
            Self::TrashLocal { path, expected } => {
                (DestructionKind::TrashLocal, path, expected.hash.as_str())
            }
            Self::Upload { .. }
            | Self::Download { .. }
            | Self::UploadConflictCopy { .. }
            | Self::CreateRemoteFolder { .. }
            | Self::MoveRemote { .. }
            | Self::MoveLocal { .. } => return None,
        };
        Some(Destruction {
            kind,
            path: path.clone(),
            version: version.to_owned(),
        })
    }

    /// Bytes the action adds on the server.
    pub fn bytes_to_remote(&self) -> u64 {
        match self {
            Self::Upload { size, .. } | Self::UploadConflictCopy { size, .. } => *size,
            _ => 0,
        }
    }

    /// Bytes the action adds on the local disk.
    pub fn bytes_to_disk(&self) -> u64 {
        match self {
            Self::Download { size, .. } => *size,
            _ => 0,
        }
    }
}
