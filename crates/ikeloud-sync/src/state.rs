//! The three states a plan is built from.

use std::time::Duration;

use ikeloud_safety::{Etag, RelPath};

/// Content hash of a local file (checksum).
///
/// A change on Linux is decided by content, never by modification time: the
/// computer's clock can be wrong.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

/// Identity of a document on the server; stays the same across renames.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(String);

impl DocId {
    /// Document ID as the server returned it.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The ID as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A file in the local folder at the last scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Relative path as it is on disk.
    pub path: RelPath,
    /// Inode: identity of the file on Linux; stays the same across renames.
    /// Editors that save by writing a new file and renaming it over the old
    /// one change it, see [`crate::build_plan`].
    pub inode: u64,
    /// Content hash.
    pub hash: ContentHash,
    /// Size in bytes.
    pub size: u64,
    /// How long the file has not changed.
    pub stable_for: Duration,
}

/// A file on the server according to the last listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// Relative path as the server returned it.
    pub path: RelPath,
    /// Document ID.
    pub doc_id: DocId,
    /// Version on the server.
    pub etag: Etag,
    /// Size in bytes.
    pub size: u64,
}

/// Last common state of a file (stored in SQLite): which local file and
/// which server document are the same file, and what both looked like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseEntry {
    /// Relative path that was the same on both sides.
    pub path: RelPath,
    /// Inode of the local file.
    pub inode: u64,
    /// Document ID on the server.
    pub doc_id: DocId,
    /// Content hash that was on both sides.
    pub hash: ContentHash,
    /// Server etag after the last sync.
    pub etag: Etag,
}

/// All three states for one planner run.
#[derive(Debug, Clone, Default)]
pub struct States {
    /// Files on Linux.
    pub local: Vec<LocalFile>,
    /// Files on iCloud.
    pub remote: Vec<RemoteFile>,
    /// Folders on iCloud (for creating missing folders).
    pub remote_folders: Vec<RelPath>,
    /// Last common state.
    pub base: Vec<BaseEntry>,
}
