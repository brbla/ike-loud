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

/// One action of a plan.
///
/// There is deliberately no permanent delete on the server: deleting always
/// moves to the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Upload a local file to the server.
    Upload {
        /// Where on the server.
        path: RelPath,
        /// Server version the engine knows; `None` = the file is not there.
        expected_etag: Option<Etag>,
        /// Size in bytes.
        size: u64,
        /// How long the local file has not changed (size or mtime).
        stable_for: Duration,
    },
    /// Keep the local version as a conflict copy and upload it as a new file.
    UploadConflictCopy {
        /// Name of the conflict copy on the server.
        path: RelPath,
        /// Size in bytes.
        size: u64,
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
        /// What to download.
        path: RelPath,
        /// Size in bytes.
        size: u64,
        /// Overwrites an existing local file (the previous version goes to history).
        overwrites_local: bool,
    },
    /// Move a local file to the system trash.
    TrashLocal {
        /// What to delete.
        path: RelPath,
    },
    /// Rename or move a local file.
    MoveLocal {
        /// From.
        from: RelPath,
        /// To.
        to: RelPath,
    },
}

impl Action {
    /// The path the action is about (the target for moves).
    pub fn path(&self) -> &RelPath {
        match self {
            Self::Upload { path, .. }
            | Self::UploadConflictCopy { path, .. }
            | Self::CreateRemoteFolder { path }
            | Self::TrashRemote { path, .. }
            | Self::Download { path, .. }
            | Self::TrashLocal { path } => path,
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

    /// Does it delete or overwrite existing content? Counted by the mass-change brake.
    pub fn deletes_or_overwrites(&self) -> bool {
        match self {
            Self::Upload { expected_etag, .. } => expected_etag.is_some(),
            Self::Download {
                overwrites_local, ..
            } => *overwrites_local,
            Self::TrashRemote { .. } | Self::TrashLocal { .. } => true,
            Self::UploadConflictCopy { .. }
            | Self::CreateRemoteFolder { .. }
            | Self::MoveRemote { .. }
            | Self::MoveLocal { .. } => false,
        }
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
