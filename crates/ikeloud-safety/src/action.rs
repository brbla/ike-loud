//! Actions proposed by the sync engine and assessed by the safety layer.

use std::fmt;
use std::time::Duration;

/// Relative path inside the synced folder, components separated by `/`.
///
/// Always well-formed: not empty, no empty component (`a//b`, `/a`, `a/`),
/// no `.` or `..` (so it can never point outside the synced folder), no NUL.
/// Whether the server accepts the name (length, characters) is a separate
/// question, answered by [`crate::names::validate_path`]: a local file with
/// such a name exists and must still be reported.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

/// Why text is not a well-formed relative path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// Empty text, or an empty component (`a//b`, `/a`, `a/`).
    EmptyComponent,
    /// A component is `.` or `..`.
    DotOrDotDot,
    /// The text contains a NUL character.
    Nul,
}

impl RelPath {
    /// Checks and creates a path. The only way to make one.
    pub fn parse(path: impl Into<String>) -> Result<Self, PathError> {
        let path = path.into();
        if path.contains('\0') {
            return Err(PathError::Nul);
        }
        for component in path.split('/') {
            match component {
                "" => return Err(PathError::EmptyComponent),
                "." | ".." => return Err(PathError::DotOrDotDot),
                _ => {}
            }
        }
        Ok(Self(path))
    }

    /// The same file name in another form (e.g. Unicode normalized).
    /// `None` if the new text is not a well-formed path.
    pub fn map(&self, f: impl FnOnce(&str) -> String) -> Option<Self> {
        Self::parse(f(&self.0)).ok()
    }

    /// A file named `name` in the same folder. `None` if `name` is not a
    /// single well-formed component (contains `/`, is empty, `.` or `..`).
    pub fn sibling(&self, name: &str) -> Option<Self> {
        if name.contains('/') {
            return None;
        }
        match self.parent() {
            "" => Self::parse(name),
            parent => Self::parse(format!("{parent}/{name}")),
        }
        .ok()
    }

    /// Parent folders, from the root: `a/b/c.txt` → `a`, `a/b`.
    pub fn ancestors(&self) -> impl Iterator<Item = RelPath> + '_ {
        self.0
            .match_indices('/')
            .filter_map(|(i, _)| self.0.get(..i))
            .map(|prefix| Self(prefix.to_owned()))
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

/// What an action the mass-change brake counts does to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKind {
    /// Moves a server file to "Recently Deleted".
    TrashRemote,
    /// Replaces the content of a server file.
    OverwriteRemote,
    /// Moves a local file to the system trash.
    TrashLocal,
    /// Replaces the content of a local file (the old one goes to history).
    OverwriteLocal,
    /// Moves or renames a server file.
    MoveRemote,
    /// Moves or renames a local file.
    MoveLocal,
}

impl ChangeKind {
    /// A move (counted apart from deletions and overwrites).
    pub fn is_move(self) -> bool {
        matches!(self, Self::MoveRemote | Self::MoveLocal)
    }
}

/// One file an action deletes, overwrites or moves, with the version it was
/// planned against. The user confirms a mass change as a set of these; the
/// confirmation then covers exactly these versions of these files.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Change {
    /// What happens to the file.
    pub kind: ChangeKind,
    /// The file.
    pub path: RelPath,
    /// Where a move takes it.
    pub to: Option<RelPath>,
    /// What the action was planned against: the server etag, the local
    /// content hash, or (a local move) the local identity.
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

    /// The name the action creates on the server, if it creates one: a new
    /// file, a conflict copy, a folder, or the target of a move. The planner
    /// creates missing folders for these and the safety layer checks them
    /// for case collisions.
    pub fn new_remote_name(&self) -> Option<&RelPath> {
        match self {
            Self::Upload {
                remote: path,
                expected_etag: None,
                ..
            }
            | Self::UploadConflictCopy { path, .. }
            | Self::CreateRemoteFolder { path } => Some(path),
            Self::MoveRemote { to, .. } => Some(to),
            Self::Upload { .. }
            | Self::TrashRemote { .. }
            | Self::Download { .. }
            | Self::TrashLocal { .. }
            | Self::MoveLocal { .. } => None,
        }
    }

    /// What the action does to an existing file, if the mass-change brake
    /// counts it (deleting, overwriting, moving). Listed to the user.
    pub fn change(&self) -> Option<Change> {
        let (kind, path, to, version) = match self {
            Self::Upload {
                remote,
                expected_etag: Some(etag),
                ..
            } => (
                ChangeKind::OverwriteRemote,
                remote,
                None,
                etag.as_str().to_owned(),
            ),
            Self::Download {
                local,
                replaces: Some(old),
                ..
            } => (
                ChangeKind::OverwriteLocal,
                local,
                None,
                old.hash.as_str().to_owned(),
            ),
            Self::TrashRemote {
                path,
                expected_etag,
            } => (
                ChangeKind::TrashRemote,
                path,
                None,
                expected_etag.as_str().to_owned(),
            ),
            Self::TrashLocal { path, expected } => (
                ChangeKind::TrashLocal,
                path,
                None,
                expected.hash.as_str().to_owned(),
            ),
            Self::MoveRemote {
                from,
                to,
                expected_etag,
            } => (
                ChangeKind::MoveRemote,
                from,
                Some(to),
                expected_etag.as_str().to_owned(),
            ),
            Self::MoveLocal { from, to, local_id } => (
                ChangeKind::MoveLocal,
                from,
                Some(to),
                format!(
                    "{}:{}:{}",
                    local_id.device,
                    local_id.inode,
                    local_id
                        .birth_ns
                        .map_or_else(|| "-".to_owned(), |b| b.to_string())
                ),
            ),
            Self::Upload { .. }
            | Self::Download { .. }
            | Self::UploadConflictCopy { .. }
            | Self::CreateRemoteFolder { .. } => return None,
        };
        Some(Change {
            kind,
            path: path.clone(),
            to: to.cloned(),
            version,
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
