//! The server side of a sync run.

use std::io::Read;

use ikeloud_safety::{ApprovedAction, Etag, RelPath};
use ikeloud_sync::{DocId, RemoteFile, StopReason};

/// What the server holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Files.
    pub files: Vec<RemoteFile>,
    /// Folders (also empty ones).
    pub folders: Vec<RelPath>,
}

/// Content of a server file being downloaded.
pub struct Download {
    /// The document.
    pub doc_id: DocId,
    /// Its version, the one being read.
    pub etag: Etag,
    /// The content.
    pub content: Box<dyn Read>,
}

/// What a server write produced (uploads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The document written.
    pub doc_id: DocId,
    /// Its new version.
    pub etag: Etag,
}

/// Why a server operation did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteError {
    /// A precondition no longer holds (the etag differs, the name is
    /// taken, the file is gone). Nothing was changed.
    Changed,
    /// Further work makes no sense now.
    Stop(StopReason),
}

/// The server: the iCloud client, or a fake in tests.
///
/// Writes take an [`ApprovedAction`], which only the safety layer creates:
/// nothing reaches the server without its approval. Every write must be
/// conditional on what the action expects (etag, free name) and fail with
/// [`RemoteError::Changed`] otherwise.
pub trait Remote {
    /// Lists everything on the server.
    fn list(&mut self) -> Result<Listing, StopReason>;

    /// Opens a server file for reading.
    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError>;

    /// Carries out an approved server write. `content` is the file content
    /// for uploads; uploads return the document and its new version.
    fn write(
        &mut self,
        action: ApprovedAction,
        content: Option<&mut dyn Read>,
    ) -> Result<Option<Written>, RemoteError>;

    /// A copy for a worker thread, to carry out independent actions at once
    /// (decision 0025); `None`: this remote works one action at a time.
    fn fork(&mut self) -> Option<Box<dyn Remote + Send>> {
        None
    }

    /// A fork uploaded `path` (for counting what the main remote did not
    /// see itself).
    fn uploaded_elsewhere(&mut self, _path: &RelPath) {}
}
