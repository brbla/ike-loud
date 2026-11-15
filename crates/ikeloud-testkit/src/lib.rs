//! Builders for test data shared by the tests of several crates, so every
//! test describes files the same way: short paths, small numbers for
//! identities, short strings for hashes and etags.
//!
//! Only a dev-dependency (checked by `scripts/check-deps.py`).

use std::time::Duration;

use ikeloud_safety::{ContentHash, Etag, LocalId, LocalVersion, RelPath};
use ikeloud_sync::{BaseEntry, DocId, LocalFile, RemoteFile, Transfer};

/// Size of every local file built here.
pub const LOCAL_SIZE: u64 = 7;
/// Size of every server file built here.
pub const REMOTE_SIZE: u64 = 9;
/// How long every local file built here has been unchanged (well past the
/// settle time).
pub const STABLE: Duration = Duration::from_secs(60);

/// A relative path.
pub fn p(path: &str) -> RelPath {
    RelPath::new(path)
}

/// Identity of local file number `n`: device 1, inode `n`, born at `n`.
pub fn id(n: u64) -> LocalId {
    LocalId {
        device: 1,
        inode: n,
        birth_ns: Some(i64::try_from(n).unwrap_or(i64::MAX)),
    }
}

/// A local file with identity [`id`]`(n)`.
pub fn local(path: &str, n: u64, hash: &str) -> LocalFile {
    LocalFile {
        path: p(path),
        id: id(n),
        hash: ContentHash::new(hash),
        size: LOCAL_SIZE,
        stable_for: STABLE,
    }
}

/// A server file.
pub fn remote(path: &str, doc: &str, etag: &str) -> RemoteFile {
    RemoteFile {
        path: p(path),
        doc_id: DocId::new(doc),
        etag: Etag::new(etag),
        size: REMOTE_SIZE,
    }
}

/// A common-state entry; `n` is the local file number, `None` when the
/// local identity is unknown.
pub fn base(path: &str, n: impl Into<Option<u64>>, doc: &str, hash: &str, etag: &str) -> BaseEntry {
    BaseEntry {
        path: p(path),
        local_path: p(path),
        local: n.into().map(id),
        doc_id: DocId::new(doc),
        hash: ContentHash::new(hash),
        etag: Etag::new(etag),
    }
}

/// What a transfer of document `doc` to local file number `n` produced.
pub fn transfer(doc: &str, etag: &str, n: u64, hash: &str) -> Transfer {
    Transfer {
        doc_id: DocId::new(doc),
        etag: Etag::new(etag),
        local: id(n),
        hash: ContentHash::new(hash),
    }
}

/// Local file number `n` with content `hash`, as a precondition.
pub fn version(n: u64, hash: &str) -> LocalVersion {
    LocalVersion {
        id: id(n),
        hash: ContentHash::new(hash),
    }
}
