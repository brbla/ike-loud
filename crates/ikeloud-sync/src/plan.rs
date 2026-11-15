//! Building a plan of actions from three states.
//!
//! The algorithm follows the TLA+ models `tla/Sync.tla` (content) and
//! `tla/SyncMoves.tla` (moves and path collisions).
//!
//! **Pairing.** A local file and a server document are the same file when
//! the last common state says so: by inode locally and by document ID on the
//! server. Editors that save by writing a new file and renaming it over the
//! old one give the file a new inode; so when the inode of a common-state
//! entry is gone, an unpaired local file standing on the entry's path is
//! taken as the same file. Files not in the common state are paired by name
//! (key from [`crate::names::key`]).
//!
//! **Decision per file** (spec, chapter Basic principles), content first:
//!
//! | Change since the last sync | Action                                          |
//! |----------------------------|-------------------------------------------------|
//! | only on Linux              | upload (deletion → iCloud trash)                |
//! | only on iCloud             | download (deletion → system trash)              |
//! | on both                    | iCloud wins, local version → conflict copy      |
//! | nowhere                    | nothing                                         |
//!
//! When the content is in sync, the path follows: a move on one side is
//! repeated on the other as a move; if both sides moved, iCloud wins.
//!
//! **Path collisions** (iCloud wins, nothing on the server is pushed aside):
//! a target taken locally by another file → that local file is renamed to a
//! conflict copy name first; a target taken on the server → the local file
//! yields and goes to the server under a conflict copy name.
//!
//! A change on Linux is detected by content hash, on iCloud by etag, never by
//! time.

use std::collections::{BTreeMap, BTreeSet};

use ikeloud_safety::{Action, RelPath, names as safety_names};

use crate::names::{key, nfc};
use crate::state::{BaseEntry, DocId, LocalFile, RemoteFile, States};

/// The side a collision is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// On Linux (case-sensitive, iCloud is not).
    Local,
    /// On iCloud (should not happen; treated as a suspicious listing).
    Remote,
}

/// Files with the same name after normalization. Sync skips them until the
/// user renames them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The side the files are on.
    pub side: Side,
    /// The colliding paths, sorted.
    pub paths: Vec<RelPath>,
}

/// Result of the planner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Actions in the order they should run.
    pub actions: Vec<Action>,
    /// Common-state entries (by document ID) that no longer exist on either side.
    pub forget: Vec<DocId>,
    /// Files where both sides already agree but the common state is stale
    /// (moved the same way on both sides, or a new inode after a save); only
    /// the common state changes.
    pub adopt: Vec<BaseEntry>,
    /// Name collisions the sync skipped.
    pub collisions: Vec<Collision>,
}

/// Builds a plan from the three states.
///
/// `conflict_name(path, n)` returns the name of a conflict copy of `path`;
/// `n` is the attempt number from 1 and increases until the name is free.
/// The caller supplies the name because the user sees it and it is
/// translated.
///
/// The plan may leave work for the next run (e.g. a file that was renamed to
/// make room is moved to its final place later); the executor checks every
/// action's preconditions and the next scan picks up the rest.
pub fn build_plan(states: &States, conflict_name: &dyn Fn(&RelPath, u32) -> RelPath) -> Plan {
    let mut planner = Planner::new(states, conflict_name);
    planner.run(states);
    let mut plan = planner.plan;
    let folders = missing_folders(&plan.actions, states);
    plan.actions.splice(0..0, folders);
    plan
}

struct Planner<'a> {
    conflict_name: &'a dyn Fn(&RelPath, u32) -> RelPath,
    plan: Plan,
    /// Normalized names taken on each side, with who holds them.
    local_taken: BTreeMap<String, u64>,
    remote_taken: BTreeMap<String, DocId>,
    /// Every name in use anywhere (for picking free conflict copy names).
    names_in_use: BTreeSet<String>,
    /// Where a local file is after the moves planned so far (by inode). Every
    /// later decision must use this path, never the scanned one: the scanned
    /// path may by then hold a different file.
    current: BTreeMap<u64, RelPath>,
}

impl<'a> Planner<'a> {
    fn new(states: &States, conflict_name: &'a dyn Fn(&RelPath, u32) -> RelPath) -> Self {
        Self {
            conflict_name,
            plan: Plan::default(),
            local_taken: BTreeMap::new(),
            remote_taken: BTreeMap::new(),
            current: BTreeMap::new(),
            names_in_use: states
                .local
                .iter()
                .map(|f| key(f.path.as_str()))
                .chain(states.remote.iter().map(|f| key(f.path.as_str())))
                .collect(),
        }
    }

    fn run(&mut self, states: &States) {
        let local = usable(
            &states.local,
            |f| &f.path,
            Side::Local,
            &mut self.plan.collisions,
        );
        let remote = usable(
            &states.remote,
            |f| &f.path,
            Side::Remote,
            &mut self.plan.collisions,
        );
        self.local_taken = local
            .iter()
            .map(|f| (key(f.path.as_str()), f.inode))
            .collect();
        self.current = local.iter().map(|f| (f.inode, f.path.clone())).collect();
        self.remote_taken = remote
            .iter()
            .map(|f| (key(f.path.as_str()), f.doc_id.clone()))
            .collect();

        let mut local_by_inode: BTreeMap<u64, &LocalFile> =
            local.iter().map(|f| (f.inode, *f)).collect();
        let mut local_by_key: BTreeMap<String, &LocalFile> =
            local.iter().map(|f| (key(f.path.as_str()), *f)).collect();
        let mut remote_by_doc: BTreeMap<&DocId, &RemoteFile> =
            remote.iter().map(|f| (&f.doc_id, *f)).collect();

        let base_inodes: BTreeSet<u64> = states.base.iter().map(|b| b.inode).collect();
        let blocked: BTreeSet<String> = self
            .plan
            .collisions
            .iter()
            .flat_map(|c| c.paths.iter().map(|p| key(p.as_str())))
            .collect();

        // Files known to the common state, paired by identity.
        let mut base: Vec<&BaseEntry> = states.base.iter().collect();
        base.sort_by_key(|b| key(b.path.as_str()));
        for b in base {
            if blocked.contains(&key(b.path.as_str())) {
                // A name collision: leave both sides of this file alone.
                remote_by_doc.remove(&b.doc_id);
                continue;
            }
            let l = local_by_inode.get(&b.inode).copied().or_else(|| {
                // Saved by "write a new file and rename it over": new inode, same path.
                let candidate = local_by_key.get(&key(b.path.as_str())).copied()?;
                (!base_inodes.contains(&candidate.inode)).then_some(candidate)
            });
            if let Some(l) = l {
                local_by_inode.remove(&l.inode);
                local_by_key.remove(&key(l.path.as_str()));
            }
            let r = remote_by_doc.remove(&b.doc_id);
            self.decide_known(l, r, b);
        }

        // New files, paired by name.
        let new_local: BTreeMap<String, &LocalFile> = local_by_inode
            .values()
            .map(|f| (key(f.path.as_str()), *f))
            .collect();
        let new_remote: BTreeMap<String, &RemoteFile> = remote_by_doc
            .values()
            .map(|f| (key(f.path.as_str()), *f))
            .collect();
        let names: BTreeSet<&String> = new_local
            .keys()
            .chain(new_remote.keys())
            .filter(|k| !blocked.contains(*k))
            .collect();
        for k in names {
            self.decide_new(new_local.get(k).copied(), new_remote.get(k).copied());
        }
    }

    /// A file the common state knows (`b`), with what is left of it on each side.
    fn decide_known(&mut self, l: Option<&LocalFile>, r: Option<&RemoteFile>, b: &BaseEntry) {
        let local_changed = l.is_none_or(|l| l.hash != b.hash);
        let remote_changed = r.is_none_or(|r| r.etag != b.etag);

        match (l, r) {
            (None, None) => self.plan.forget.push(b.doc_id.clone()),
            (Some(l), None) if !local_changed => {
                let path = self.path_of(l);
                self.push(Action::TrashLocal { path });
            }
            // Deleted on iCloud, changed on Linux: iCloud wins, keep the local version.
            (Some(l), None) => self.keep_as_conflict_copy(l),
            (None, Some(r)) if !remote_changed => self.push(Action::TrashRemote {
                path: r.path.clone(),
                expected_etag: r.etag.clone(),
            }),
            // Deleted on Linux, changed on iCloud: iCloud wins, restore it.
            (None, Some(r)) => self.place_locally(r),
            (Some(l), Some(r)) => match (local_changed, remote_changed) {
                (true, false) => self.push(Action::Upload {
                    path: r.path.clone(),
                    expected_etag: Some(r.etag.clone()),
                    size: l.size,
                    stable_for: l.stable_for,
                }),
                (false, true) => {
                    let path = self.path_of(l);
                    self.push(Action::Download {
                        path,
                        size: r.size,
                        overwrites_local: true,
                    });
                }
                (true, true) => {
                    // Download to where the local file was before it became a copy.
                    let original = self.path_of(l);
                    self.keep_as_conflict_copy(l);
                    self.push(Action::Download {
                        path: original,
                        size: r.size,
                        overwrites_local: false,
                    });
                }
                (false, false) => self.decide_path(l, r, b),
            },
        }
    }

    /// Content is in sync; only the path (or the inode) may differ.
    fn decide_path(&mut self, l: &LocalFile, r: &RemoteFile, b: &BaseEntry) {
        let local_path = self.path_of(l);
        if local_path == r.path {
            if local_path != b.path || l.inode != b.inode {
                self.plan.adopt.push(BaseEntry {
                    path: r.path.clone(),
                    inode: l.inode,
                    doc_id: r.doc_id.clone(),
                    hash: l.hash.clone(),
                    etag: r.etag.clone(),
                });
            }
        } else if r.path != b.path {
            // Moved on iCloud (or on both sides): iCloud wins.
            self.move_locally(l, r.path.clone());
        } else {
            // Moved only on Linux.
            self.move_remotely(l, r);
        }
    }

    /// Files the common state does not know yet.
    fn decide_new(&mut self, l: Option<&LocalFile>, r: Option<&RemoteFile>) {
        match (l, r) {
            (None, None) => {}
            (Some(l), None) => {
                let from = self.path_of(l);
                let target = RelPath::new(nfc(from.as_str()));
                if self.remote_taken_by_other(&target, None) {
                    // A different file already has this name on iCloud: yield.
                    let copy = self.free_copy_name(&from);
                    self.local_move(l.inode, from, copy.clone());
                    self.push(upload_new(l, copy));
                } else {
                    self.push(upload_new(l, target));
                }
            }
            (None, Some(r)) => self.place_locally(r),
            // New on both sides under the same name: without a server checksum
            // the contents cannot be compared, so iCloud wins and the local
            // file becomes a conflict copy.
            (Some(l), Some(r)) => {
                self.keep_as_conflict_copy(l);
                self.push(Action::Download {
                    path: r.path.clone(),
                    size: r.size,
                    overwrites_local: false,
                });
            }
        }
    }

    /// Rename the local file to a conflict copy name and upload it as a new file.
    fn keep_as_conflict_copy(&mut self, l: &LocalFile) {
        let from = self.path_of(l);
        let copy = self.free_copy_name(&from);
        self.local_move(l.inode, from, copy.clone());
        self.push(Action::UploadConflictCopy {
            path: copy,
            size: l.size,
        });
    }

    /// Download a server file to its server path, making room locally if needed.
    fn place_locally(&mut self, r: &RemoteFile) {
        self.evict_local(&r.path, None);
        self.push(Action::Download {
            path: r.path.clone(),
            size: r.size,
            overwrites_local: false,
        });
    }

    /// Move a local file to `target`, making room locally if needed.
    fn move_locally(&mut self, l: &LocalFile, target: RelPath) {
        self.evict_local(&target, Some(l.inode));
        let from = self.path_of(l);
        self.local_move(l.inode, from, target);
    }

    /// Repeat a local move on the server, or yield if the name is taken there.
    fn move_remotely(&mut self, l: &LocalFile, r: &RemoteFile) {
        let from = self.path_of(l);
        let target = RelPath::new(nfc(from.as_str()));
        if self.remote_taken_by_other(&target, Some(&r.doc_id)) {
            let copy = self.free_copy_name(&from);
            self.local_move(l.inode, from, copy.clone());
            self.push(Action::MoveRemote {
                from: r.path.clone(),
                to: copy,
                expected_etag: r.etag.clone(),
            });
        } else {
            self.push(Action::MoveRemote {
                from: r.path.clone(),
                to: target,
                expected_etag: r.etag.clone(),
            });
        }
    }

    /// If another local file stands on `target`, rename it to a conflict copy name.
    fn evict_local(&mut self, target: &RelPath, owner: Option<u64>) {
        let k = key(target.as_str());
        let Some(&occupant) = self.local_taken.get(&k) else {
            return;
        };
        if Some(occupant) == owner {
            return;
        }
        let from = self
            .current
            .get(&occupant)
            .cloned()
            .unwrap_or_else(|| target.clone());
        let copy = self.free_copy_name(&from);
        self.local_move(occupant, from, copy);
    }

    /// Current local path of a file (after the moves planned so far).
    fn path_of(&self, l: &LocalFile) -> RelPath {
        self.current
            .get(&l.inode)
            .cloned()
            .unwrap_or_else(|| l.path.clone())
    }

    /// The only way the planner moves a local file: records the new path.
    fn local_move(&mut self, inode: u64, from: RelPath, to: RelPath) {
        self.local_taken.remove(&key(from.as_str()));
        self.local_taken.insert(key(to.as_str()), inode);
        self.current.insert(inode, to.clone());
        self.push(Action::MoveLocal { from, to });
    }

    fn remote_taken_by_other(&self, target: &RelPath, owner: Option<&DocId>) -> bool {
        self.remote_taken
            .get(&key(target.as_str()))
            .is_some_and(|holder| Some(holder) != owner)
    }

    fn free_copy_name(&mut self, path: &RelPath) -> RelPath {
        let mut n = 1;
        loop {
            let candidate = (self.conflict_name)(path, n);
            if self.names_in_use.insert(key(candidate.as_str())) || n == u32::MAX {
                return candidate;
            }
            n += 1;
        }
    }

    fn push(&mut self, action: Action) {
        self.plan.actions.push(action);
    }
}

fn upload_new(l: &LocalFile, path: RelPath) -> Action {
    Action::Upload {
        path,
        expected_etag: None,
        size: l.size,
        stable_for: l.stable_for,
    }
}

/// Files that take part in sync: not ignored and not in a name collision
/// (collisions are reported).
fn usable<'a, T>(
    files: &'a [T],
    path: impl Fn(&T) -> &RelPath,
    side: Side,
    collisions: &mut Vec<Collision>,
) -> Vec<&'a T> {
    let mut groups: BTreeMap<String, Vec<&'a T>> = BTreeMap::new();
    for f in files {
        if safety_names::is_ignored(path(f).name()) {
            continue;
        }
        groups.entry(key(path(f).as_str())).or_default().push(f);
    }
    let mut result = Vec::new();
    for group in groups.into_values() {
        match group.as_slice() {
            [single] => result.push(*single),
            many => {
                let mut paths: Vec<RelPath> = many.iter().map(|f| path(f).clone()).collect();
                paths.sort();
                collisions.push(Collision { side, paths });
            }
        }
    }
    result
}

/// Server folders that must be created before uploading or moving files
/// into them, shallower first.
fn missing_folders(actions: &[Action], states: &States) -> Vec<Action> {
    let mut existing: BTreeSet<String> = states
        .remote_folders
        .iter()
        .map(|p| key(p.as_str()))
        .collect();
    for f in &states.remote {
        existing.extend(ancestors(f.path.as_str()).map(key));
    }
    let mut new: Vec<(usize, RelPath)> = Vec::new();
    for a in actions {
        let path = match a {
            Action::Upload {
                path,
                expected_etag: None,
                ..
            }
            | Action::UploadConflictCopy { path, .. } => path,
            Action::MoveRemote { to, .. } => to,
            _ => continue,
        };
        for ancestor in ancestors(path.as_str()) {
            if existing.insert(key(ancestor)) {
                new.push((ancestor.matches('/').count(), RelPath::new(nfc(ancestor))));
            }
        }
    }
    new.sort();
    new.into_iter()
        .map(|(_, path)| Action::CreateRemoteFolder { path })
        .collect()
}

/// Parent folders of a path, from the root: `a/b/c.txt` → `a`, `a/b`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .filter_map(move |(i, _)| path.get(..i))
}
