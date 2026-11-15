//! Which local file is which known file. The one pairing rule, shared by the
//! planner and by [`crate::next_base`] so the two can never disagree.

use std::collections::BTreeMap;

use ikeloud_safety::LocalId;

use crate::names::{nfc, same_place};
use crate::state::{BaseEntry, LocalFile};

/// Is `l` (from a later scan) the local file of common-state entry `b`?
///
/// With a birth time on both sides the answer is certain. Without it the
/// inode may have been reused by a new, unrelated file; then only a file
/// that kept the entry's path or its content counts as the same file. A file
/// that was renamed and edited between two scans on such a filesystem is
/// taken for a new file: its old server copy goes to the iCloud trash and it
/// is uploaded anew. Nothing is lost; only the document history is.
pub(crate) fn is_same(b: &BaseEntry, l: &LocalFile) -> bool {
    let Some(id) = b.local else {
        return false;
    };
    match id.same_file(&l.id) {
        Some(same) => same,
        None => same_place(b.local_path.as_str(), l.path.as_str()) || b.hash == l.hash,
    }
}

fn slot(id: &LocalId) -> (u64, u64) {
    (id.device, id.inode)
}

/// Local files of one scan, indexed for pairing with common-state entries.
pub(crate) struct LocalIndex<'a> {
    by_inode: BTreeMap<(u64, u64), &'a LocalFile>,
    /// By exact place (NFC); only places held by a single file.
    by_place: BTreeMap<String, &'a LocalFile>,
    base_by_inode: BTreeMap<(u64, u64), &'a BaseEntry>,
}

impl<'a> LocalIndex<'a> {
    pub(crate) fn new(local: &'a [LocalFile], base: &'a [BaseEntry]) -> Self {
        let mut by_place: BTreeMap<String, Option<&'a LocalFile>> = BTreeMap::new();
        for l in local {
            by_place
                .entry(nfc(l.path.as_str()))
                .and_modify(|held| *held = None)
                .or_insert(Some(l));
        }
        Self {
            by_inode: local.iter().map(|l| (slot(&l.id), l)).collect(),
            by_place: by_place
                .into_iter()
                .filter_map(|(k, l)| Some((k, l?)))
                .collect(),
            base_by_inode: base
                .iter()
                .filter_map(|b| Some((slot(&b.local?), b)))
                .collect(),
        }
    }

    /// The local file that is entry `b` now, among the files `available`
    /// says are not paired yet: the file with `b`'s identity, else (saved by
    /// "write a new file and rename it over": new inode, same path) a file
    /// no entry knows, standing exactly where `b`'s local file was. Exactly:
    /// an editor keeps the name, while `a.txt` next to a deleted `A.txt` is
    /// another file; and where the local file was, not the common path,
    /// which differs while a move is pending (both found by the simulation).
    pub(crate) fn find(
        &self,
        b: &BaseEntry,
        available: impl Fn(&LocalFile) -> bool,
    ) -> Option<&'a LocalFile> {
        let by_identity = b
            .local
            .and_then(|id| self.by_inode.get(&slot(&id)).copied())
            .filter(|l| is_same(b, l) && available(l));
        by_identity.or_else(|| {
            let l = self.by_place.get(&nfc(b.local_path.as_str())).copied()?;
            (available(l) && !self.is_known(l)).then_some(l)
        })
    }

    /// May this file belong to some common-state entry? Also when that
    /// cannot be ruled out (the inode of an entry, birth times unknown): a
    /// file that may be another entry's is never taken for a re-save, since
    /// an unclear case must not overwrite a document (found by the
    /// simulation).
    fn is_known(&self, l: &LocalFile) -> bool {
        self.base_by_inode
            .get(&slot(&l.id))
            .is_some_and(|b| b.local.is_some_and(|id| id.same_file(&l.id) != Some(false)))
    }
}
