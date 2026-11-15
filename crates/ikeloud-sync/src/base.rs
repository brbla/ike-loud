//! The next common state after a sync run.
//!
//! Follows `NewBase` of the TLA+ models: whatever matches on both sides is
//! stored, component by component (content, path, identity), and nothing
//! else. "Matches" is decided only from what is known for sure:
//!
//! - a [`Transfer`] the executor did in this run: after it, this server
//!   document (with this etag) and this local file (with this hash) hold the
//!   same content;
//! - or both sides are unchanged since the last common state.
//!
//! A local file and its server document are paired with the same rules as
//! the planner uses (identity, then the old path for re-saved files), so the
//! two can never disagree about which file is which.

use std::collections::{BTreeMap, BTreeSet};

use ikeloud_safety::{ContentHash, Etag, LocalId, RelPath};

use crate::names::{key, same_place};
use crate::pairing::LocalIndex;
use crate::state::{BaseEntry, DocId, LocalFile, RemoteFile, States};

/// What the executor learned from one transfer (upload or download): the
/// local file `local` with content `hash` and the server document `doc_id`
/// at version `etag` now hold the same content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// The server path the transfer wrote or read. For a new document it is
    /// the path both sides agreed on at that moment: should one side move
    /// the file right after, this tells which one did.
    pub path: RelPath,
    /// Server document.
    pub doc_id: DocId,
    /// Its version after the transfer.
    pub etag: Etag,
    /// Local file.
    pub local: LocalId,
    /// Its content hash after the transfer.
    pub hash: ContentHash,
}

/// Computes the common state after a run.
///
/// `old` is the common state the plan was built from, `after` a fresh scan of
/// both sides after the run, `transfers` what the executor did. Entries whose
/// file is gone on both sides are dropped; everything else is kept, updated
/// only where both sides demonstrably agree.
pub fn next_base(old: &[BaseEntry], after: &States, transfers: &[Transfer]) -> Vec<BaseEntry> {
    let local_by_id: BTreeMap<LocalId, &LocalFile> =
        after.local.iter().map(|f| (f.id, f)).collect();
    let remote_by_doc: BTreeMap<&DocId, &RemoteFile> =
        after.remote.iter().map(|f| (&f.doc_id, f)).collect();
    let old_by_doc: BTreeMap<&DocId, &BaseEntry> = old.iter().map(|b| (&b.doc_id, b)).collect();
    let index = LocalIndex::new(&after.local, old);

    let mut next: Vec<BaseEntry> = Vec::new();
    let mut used_docs: BTreeSet<DocId> = BTreeSet::new();
    let mut used_local: BTreeSet<LocalId> = BTreeSet::new();

    // 1. Transfers: proof that both sides hold the same content (the last
    //    transfer of a document wins). Only if neither side changed since.
    let mut last: BTreeMap<&DocId, &Transfer> = BTreeMap::new();
    for t in transfers {
        last.insert(&t.doc_id, t);
    }
    for (doc, t) in last {
        let (Some(r), Some(l)) = (remote_by_doc.get(doc), local_by_id.get(&t.local)) else {
            continue;
        };
        if r.etag != t.etag || l.hash != t.hash || used_local.contains(&l.id) {
            continue;
        }
        next.push(BaseEntry {
            path: common_path(Some(old_by_doc.get(doc).map_or(&t.path, |b| &b.path)), l, r),
            local_path: l.path.clone(),
            local: Some(l.id),
            doc_id: doc.clone(),
            hash: t.hash.clone(),
            etag: t.etag.clone(),
        });
        used_docs.insert(doc.clone());
        used_local.insert(l.id);
    }

    // 2. Every other known file, paired like the planner pairs it.
    for b in old {
        if used_docs.contains(&b.doc_id) {
            continue;
        }
        let r = remote_by_doc.get(&b.doc_id).copied();
        let l = index.find(b, |l| !used_local.contains(&l.id));
        if l.is_none() && r.is_none() {
            continue;
        }
        let mut entry = b.clone();
        // A local file that is gone loses its identity: its inode may soon
        // belong to another file.
        entry.local = l.map(|l| l.id);
        if let Some(l) = l {
            entry.local_path = l.path.clone();
        }
        // The stored path must be one a side of this file still holds,
        // whatever happened to the content (the path is a component of its
        // own, as in NewBase of the TLA+ models): a path nobody holds may be
        // taken by another document, which would then collide with this
        // entry (found by the daemon simulation).
        entry.path = match (l, r) {
            (Some(l), Some(r)) => common_path(Some(&b.path), l, r),
            (Some(l), None) => l.path.clone(),
            (None, Some(r)) => r.path.clone(),
            (None, None) => b.path.clone(),
        };
        used_local.extend(entry.local);
        used_docs.insert(b.doc_id.clone());
        next.push(entry);
    }

    // 3. Names and local files stay unique; entries from step 1 (proven) win.
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut slots: BTreeSet<(u64, u64)> = BTreeSet::new();
    next.retain(|b| names.insert(key(b.path.as_str())));
    for b in &mut next {
        if let Some(id) = b.local
            && !slots.insert((id.device, id.inode))
        {
            b.local = None;
        }
    }
    next.sort_by_key(|b| key(b.path.as_str()));
    next
}

/// The path to store for a file whose local and server sides are `l` and
/// `r`, given the path both last agreed on.
///
/// When the two are in different places (moved on one side, the move not
/// carried over yet), the stored path must keep telling the planner which
/// side moved, and it must be a place this file still holds: a path nobody
/// holds any more may be taken by another file, which would then be taken
/// for this one (found by the simulation). So: the old path while the server
/// side is still there (the local side moved), otherwise the local path (the
/// server side moved; the planner then follows it, iCloud wins).
fn common_path(old: Option<&RelPath>, l: &LocalFile, r: &RemoteFile) -> RelPath {
    match old {
        _ if same_place(l.path.as_str(), r.path.as_str()) => r.path.clone(),
        Some(old) if same_place(r.path.as_str(), old.as_str()) => old.clone(),
        Some(_) => l.path.clone(),
        None => r.path.clone(),
    }
}
