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

use ikeloud_safety::RelPath;
use ikeloud_sync::BaseEntry;

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
    /// Insert or replace the entry for this file (matched by normalized name).
    Set(BaseEntry),
    /// Remove the entry for this path (no-op if there is none).
    Forget(RelPath),
}

/// Kind of a journal entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    /// A file was uploaded.
    Uploaded,
    /// A file was downloaded.
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
}

impl JournalKind {
    /// All kinds (for tests and for the GUI filter).
    pub const ALL: [Self; 8] = [
        Self::Uploaded,
        Self::Downloaded,
        Self::TrashedRemote,
        Self::TrashedLocal,
        Self::MovedRemote,
        Self::MovedLocal,
        Self::ConflictCopy,
        Self::FolderCreated,
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
        }
    }

    /// Kind from its stored name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
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
