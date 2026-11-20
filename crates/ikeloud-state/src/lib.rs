//! Persistent state of ike loud: the last common state of every file and
//! the journal of actions, stored in SQLite.
//!
//! Everything a sync run changes is written in one transaction, so after a
//! crash the daemon continues from the last committed state (spec: "state is
//! written transactionally"). The database runs in WAL mode with
//! `synchronous=FULL`.
//!
//! This crate is the single wrapper of `rusqlite`; its types never leave it.

mod db;

use ikeloud_safety::{ContentHash, Etag, LocalId, RelPath};
use ikeloud_sync::{BaseEntry, DocId};

pub use db::{SCHEMA_VERSION, StateDb};

/// Error of the state database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The database could not be opened or configured.
    Open(String),
    /// An SQL statement failed (the transaction was rolled back).
    Sql(String),
    /// The database was written by a newer version of the app. It is left
    /// untouched.
    NewerSchema {
        /// Version found in the file.
        found: i64,
        /// Newest version this build understands.
        supported: i64,
    },
    /// Two documents would have the same name (after normalization). The
    /// commit was rolled back.
    NameTaken {
        /// The name.
        path: RelPath,
    },
    /// Two documents would have the same local file. The commit was rolled
    /// back.
    LocalFileTaken {
        /// The name of the second document.
        path: RelPath,
    },
    /// An entry or journal entry with an empty or out-of-range value. The
    /// commit was rolled back.
    InvalidValue {
        /// Which value.
        field: &'static str,
    },
    /// A stored row has a value this build does not understand.
    CorruptRow {
        /// Table name.
        table: &'static str,
        /// What was wrong.
        detail: String,
    },
}

/// One change of the last common state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseChange {
    /// Insert or replace the entry of this document.
    Set(BaseEntry),
    /// Remove the entry of this document (no-op if there is none).
    Forget(DocId),
}

/// Kind of a journal entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    /// A new file was uploaded.
    Uploaded,
    /// A new file was downloaded.
    Downloaded,
    /// A file was moved to the iCloud trash.
    TrashedRemote,
    /// A file was moved to the system trash.
    TrashedLocal,
    /// A file was moved or renamed on iCloud.
    MovedRemote,
    /// A file was moved or renamed locally.
    MovedLocal,
    /// A conflict copy was created.
    ConflictCopy,
    /// A folder was created on iCloud.
    FolderCreated,
    /// A file on iCloud was overwritten by the local version.
    OverwrittenRemote,
    /// A local file was overwritten by the iCloud version (the old one is in
    /// the local history).
    OverwrittenLocal,
    /// The user confirmed a mass change; the brake window starts again.
    MassChangeConfirmed,
    /// A folder was created locally.
    FolderCreatedLocal,
    /// An empty local folder was removed (deleted on iCloud).
    FolderRemovedLocal,
    /// An empty folder was moved to the iCloud trash (deleted locally).
    FolderTrashedRemote,
    /// The check run found a file whose content differs between the disk
    /// and iCloud, though both are at the version of the common state
    /// (decision 0028). Nothing was changed.
    CheckDiffers,
}

impl JournalKind {
    /// All kinds (for tests and for the GUI filter).
    pub const ALL: [Self; 15] = [
        Self::Uploaded,
        Self::Downloaded,
        Self::TrashedRemote,
        Self::TrashedLocal,
        Self::MovedRemote,
        Self::MovedLocal,
        Self::ConflictCopy,
        Self::FolderCreated,
        Self::OverwrittenRemote,
        Self::OverwrittenLocal,
        Self::MassChangeConfirmed,
        Self::FolderCreatedLocal,
        Self::FolderRemovedLocal,
        Self::FolderTrashedRemote,
        Self::CheckDiffers,
    ];

    /// Stable name stored in the database. Never change an existing name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Uploaded => "uploaded",
            Self::Downloaded => "downloaded",
            Self::TrashedRemote => "trashed-remote",
            Self::TrashedLocal => "trashed-local",
            Self::MovedRemote => "moved-remote",
            Self::MovedLocal => "moved-local",
            Self::ConflictCopy => "conflict-copy",
            Self::FolderCreated => "folder-created",
            Self::OverwrittenRemote => "overwritten-remote",
            Self::OverwrittenLocal => "overwritten-local",
            Self::MassChangeConfirmed => "mass-change-confirmed",
            Self::FolderCreatedLocal => "folder-created-local",
            Self::FolderRemovedLocal => "folder-removed-local",
            Self::FolderTrashedRemote => "folder-trashed-remote",
            Self::CheckDiffers => "check-differs",
        }
    }

    /// Kind from its stored name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// An upload whose last step got no answer: the document may be on iCloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnconfirmedUpload {
    /// Where on iCloud (the path the upload went to).
    pub path: RelPath,
    /// The document written.
    pub doc_id: DocId,
    /// The local file uploaded.
    pub local: LocalId,
    /// Hash of the content uploaded.
    pub hash: ContentHash,
    /// iCloud's checksum of that content.
    pub checksum: String,
    /// When (Unix seconds).
    pub time: u64,
}

/// What the check run found for one document (decision 0028): at the
/// version `etag` on iCloud and the content `hash` on the disk, iCloud's
/// checksum and the local content were the same, or not. Holds only while
/// the common state still has that version and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// The document.
    pub doc_id: DocId,
    /// Its version on iCloud when checked.
    pub etag: Etag,
    /// The content hash of the local file when checked.
    pub hash: ContentHash,
    /// When (Unix seconds).
    pub time: u64,
    /// Whether both sides had the same content.
    pub same: bool,
}

/// One entry of the journal: what happened, when and to which file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    /// Unix time in seconds (given by the caller; the clock is not read here).
    pub time: u64,
    /// What happened.
    pub kind: JournalKind,
    /// The file it happened to.
    pub path: RelPath,
    /// Target path of a move or name of a conflict copy.
    pub target: Option<RelPath>,
}
