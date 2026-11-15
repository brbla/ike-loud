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
}

/// A file in the local folder at the last scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Relative path as it is on disk.
    pub path: RelPath,
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
    /// Version on the server.
    pub etag: Etag,
    /// Size in bytes.
    pub size: u64,
}

/// Last common state of a file (stored in SQLite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseEntry {
    /// Relative path.
    pub path: RelPath,
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
