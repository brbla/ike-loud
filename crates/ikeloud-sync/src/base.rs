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

use ikeloud_safety::Etag;

use crate::names::{key, same_place};
use crate::state::{BaseEntry, ContentHash, DocId, LocalFile, RemoteFile, States};

/// What the executor learned from one transfer (upload or download): the
/// local file `inode` with content `hash` and the server document `doc_id`
/// at version `etag` now hold the same content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// Server document.
    pub doc_id: DocId,
    /// Its version after the transfer.
    pub etag: Etag,
    /// Local file.
    pub inode: u64,
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
    let local_by_inode: BTreeMap<u64, &LocalFile> =
        after.local.iter().map(|f| (f.inode, f)).collect();
    let remote_by_doc: BTreeMap<&DocId, &RemoteFile> =
        after.remote.iter().map(|f| (&f.doc_id, f)).collect();
    let old_by_doc: BTreeMap<&DocId, &BaseEntry> = old.iter().map(|b| (&b.doc_id, b)).collect();
    let known_inodes: BTreeSet<u64> = old.iter().filter_map(|b| b.inode).collect();

    let mut next: Vec<BaseEntry> = Vec::new();
    let mut used_docs: BTreeSet<DocId> = BTreeSet::new();
    let mut used_inodes: BTreeSet<u64> = BTreeSet::new();

    // 1. Transfers: proof that both sides hold the same content (the last
    //    transfer of a document wins). Only if neither side changed since.
    let mut last: BTreeMap<&DocId, &Transfer> = BTreeMap::new();
    for t in transfers {
        last.insert(&t.doc_id, t);
    }
    for (doc, t) in last {
        let (Some(r), Some(l)) = (remote_by_doc.get(doc), local_by_inode.get(&t.inode)) else {
            continue;
        };
        if r.etag != t.etag || l.hash != t.hash || used_inodes.contains(&l.inode) {
            continue;
        }
        let path = if same_place(l.path.as_str(), r.path.as_str()) {
            r.path.clone()
        } else {
            // Still in different places (e.g. moved on one side): the path
            // stays what both sides last agreed on.
            old_by_doc
                .get(doc)
                .map_or_else(|| r.path.clone(), |b| b.path.clone())
        };
        next.push(BaseEntry {
            path,
            inode: Some(l.inode),
            doc_id: doc.clone(),
            hash: t.hash.clone(),
            etag: t.etag.clone(),
        });
        used_docs.insert(doc.clone());
        used_inodes.insert(l.inode);
    }

    // 2. Every other known file, paired like the planner pairs it.
    for b in old {
        if used_docs.contains(&b.doc_id) {
            continue;
        }
        let r = remote_by_doc.get(&b.doc_id).copied();
        let l = b
            .inode
            .and_then(|i| local_by_inode.get(&i).copied())
            .filter(|l| !used_inodes.contains(&l.inode))
            .or_else(|| {
                // Re-saved by "write a new file and rename it over".
                after.local.iter().find(|l| {
                    key(l.path.as_str()) == key(b.path.as_str())
                        && !known_inodes.contains(&l.inode)
                        && !used_inodes.contains(&l.inode)
                })
            });
        let mut entry = b.clone();
        match (l, r) {
            (None, None) => continue,
            (Some(l), Some(r)) => {
                entry.inode = Some(l.inode);
                let unchanged = l.hash == b.hash && r.etag == b.etag;
                if unchanged && same_place(l.path.as_str(), r.path.as_str()) {
                    entry.path = r.path.clone();
                }
                used_inodes.insert(l.inode);
            }
            (Some(l), None) => {
                entry.inode = Some(l.inode);
                used_inodes.insert(l.inode);
            }
            (None, Some(_)) => {
                // The local file is gone (or its inode now belongs to another
                // file): forget the local identity, keep the server one.
                if entry.inode.is_some_and(|i| used_inodes.contains(&i)) {
                    entry.inode = None;
                }
            }
        }
        used_docs.insert(b.doc_id.clone());
        next.push(entry);
    }

    // 3. Names and inodes stay unique; entries from step 1 (proven) win.
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut inodes: BTreeSet<u64> = BTreeSet::new();
    next.retain(|b| names.insert(key(b.path.as_str())));
    for b in &mut next {
        if let Some(i) = b.inode
            && !inodes.insert(i)
        {
            b.inode = None;
        }
    }
    next.sort_by_key(|b| key(b.path.as_str()));
    next
}
