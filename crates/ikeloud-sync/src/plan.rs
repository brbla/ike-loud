//! Building a plan of actions from three states.
//!
//! The algorithm follows the TLA+ models `tla/Sync.tla` (content) and
//! `tla/SyncMoves.tla` (moves and path collisions).
//!
//! **Pairing.** A local file and a server document are the same file when
//! the last common state says so: by identity locally (device, inode and
//! birth time, see `pairing.rs`) and by document ID on the server. Editors
//! that save by writing a new file and renaming it over the old one give the
//! file a new inode; so when the identity of a common-state entry is gone,
//! an unpaired local file standing on the entry's path is taken as the same
//! file. Files not in the common state are paired by name (key from
//! [`crate::names::key`]).
//!
//! **Files left alone** this run, on both sides: names in a collision,
//! files still being written (unchanged for less than the settle time: an
//! editor may still rename or replace them) and known files renamed to a
//! name that is never synced (`report.odt` → `report.odt.part`). Their
//! server counterparts stay as they are; nothing is uploaded, downloaded or
//! deleted for them.
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
use std::time::Duration;

use ikeloud_safety::{Action, LocalId, LocalVersion, RelPath, names as safety_names};

use crate::names::{key, nfc, same_place};
use crate::pairing::LocalIndex;
use crate::state::{BaseEntry, DocId, LocalFile, RemoteFile, States};

/// How many conflict copy names are tried before giving up on a file.
pub const MAX_COPY_NAME_ATTEMPTS: u32 = 1000;

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
    /// Actions in the order they should run. The common state after the run
    /// (including files that were deleted on both sides or moved the same way
    /// on both) is computed by [`crate::base::next_base`].
    pub actions: Vec<Action>,
    /// Name collisions the sync skipped.
    pub collisions: Vec<Collision>,
    /// Local files still being written; synced once they settle.
    pub busy: Vec<RelPath>,
    /// Files left alone because no free conflict copy name was found (the
    /// translation of the name does not use the number).
    pub no_copy_name: Vec<RelPath>,
}

/// Builds a plan from the three states.
///
/// Local files unchanged for less than `settle_time` are left alone (see
/// the module documentation). `conflict_name(path, n)` returns the name of a
/// conflict copy of `path`; `n` is the attempt number from 1 and increases
/// until the name is free (at most [`MAX_COPY_NAME_ATTEMPTS`]). The caller
/// supplies the name because the user sees it and it is translated.
///
/// The plan may leave work for the next run (e.g. a file that was renamed to
/// make room is moved to its final place later); the executor checks every
/// action's preconditions and the next scan picks up the rest.
pub fn build_plan(
    states: &States,
    settle_time: Duration,
    conflict_name: &dyn Fn(&RelPath, u32) -> RelPath,
) -> Plan {
    let mut planner = Planner::new(states, conflict_name);
    planner.run(states, settle_time);
    let mut plan = planner.plan;
    let folders = missing_folders(&plan.actions, states);
    plan.actions.splice(0..0, folders);
    plan
}

struct Planner<'a> {
    conflict_name: &'a dyn Fn(&RelPath, u32) -> RelPath,
    plan: Plan,
    /// Normalized names taken on each side, with who holds them.
    local_taken: BTreeMap<String, LocalId>,
    remote_taken: BTreeMap<String, DocId>,
    /// Every name in use anywhere (for picking free conflict copy names).
    names_in_use: BTreeSet<String>,
    /// Where a local file is after the moves planned so far. Every later
    /// decision must use this path, never the scanned one: the scanned path
    /// may by then hold a different file.
    current: BTreeMap<LocalId, RelPath>,
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

    fn run(&mut self, states: &States, settle_time: Duration) {
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
        self.local_taken = local.iter().map(|f| (key(f.path.as_str()), f.id)).collect();
        self.current = local.iter().map(|f| (f.id, f.path.clone())).collect();
        self.remote_taken = remote
            .iter()
            .map(|f| (key(f.path.as_str()), f.doc_id.clone()))
            .collect();

        let held = self.held_names(states, settle_time);
        let is_held = |path: &RelPath| {
            held.contains(&key(path.as_str())) || safety_names::is_ignored(path.name())
        };
        let index = LocalIndex::new(&states.local, &states.base);
        let remote_by_doc: BTreeMap<&DocId, &RemoteFile> =
            remote.iter().map(|f| (&f.doc_id, *f)).collect();
        let mut paired_local: BTreeSet<LocalId> = BTreeSet::new();
        let mut paired_docs: BTreeSet<&DocId> = BTreeSet::new();

        // Files known to the common state, paired by identity.
        let mut base: Vec<&BaseEntry> = states.base.iter().collect();
        base.sort_by_key(|b| key(b.path.as_str()));
        for b in base {
            let l = index.find(b, |l| !paired_local.contains(&l.id));
            let r = remote_by_doc.get(&b.doc_id).copied();
            paired_local.extend(l.map(|l| l.id));
            paired_docs.insert(&b.doc_id);
            let left_alone = is_held(&b.path)
                || l.is_some_and(|l| is_held(&l.path))
                || states
                    .remote
                    .iter()
                    .any(|f| f.doc_id == b.doc_id && is_held(&f.path));
            if !left_alone {
                self.decide_known(l, r, b);
            }
        }

        // New files, paired by name.
        let new_local: BTreeMap<String, &LocalFile> = local
            .iter()
            .filter(|f| !paired_local.contains(&f.id) && !is_held(&f.path))
            .map(|f| (key(f.path.as_str()), *f))
            .collect();
        let new_remote: BTreeMap<String, &RemoteFile> = remote
            .iter()
            .filter(|f| !paired_docs.contains(&f.doc_id) && !is_held(&f.path))
            .map(|f| (key(f.path.as_str()), *f))
            .collect();
        let names: BTreeSet<&String> = new_local.keys().chain(new_remote.keys()).collect();
        for k in names {
            self.decide_new(new_local.get(k).copied(), new_remote.get(k).copied());
        }
    }

    /// Names sync leaves alone this run, on both sides: names in a
    /// collision and names of local files still being written.
    fn held_names(&mut self, states: &States, settle_time: Duration) -> BTreeSet<String> {
        let mut busy: Vec<RelPath> = states
            .local
            .iter()
            .filter(|f| f.stable_for < settle_time && !safety_names::is_ignored(f.path.name()))
            .map(|f| f.path.clone())
            .collect();
        busy.sort();
        let held = self
            .plan
            .collisions
            .iter()
            .flat_map(|c| c.paths.iter())
            .chain(&busy)
            .map(|p| key(p.as_str()))
            .collect();
        self.plan.busy = busy;
        held
    }

    /// A file the common state knows (`b`), with what is left of it on each side.
    fn decide_known(&mut self, l: Option<&LocalFile>, r: Option<&RemoteFile>, b: &BaseEntry) {
        let local_changed = l.is_none_or(|l| l.hash != b.hash);
        let remote_changed = r.is_none_or(|r| r.etag != b.etag);

        match (l, r) {
            // Gone on both sides: nothing to do (next_base drops the entry).
            (None, None) => {}
            (Some(l), None) if !local_changed => {
                let path = self.path_of(l);
                self.push(Action::TrashLocal {
                    path,
                    expected: version(l),
                });
            }
            // Deleted on iCloud, changed on Linux: iCloud wins, keep the local version.
            (Some(l), None) => {
                // No free name: reported in `no_copy_name`, left alone.
                let _ = self.keep_as_conflict_copy(l);
            }
            (None, Some(r)) if !remote_changed => self.push(Action::TrashRemote {
                path: r.path.clone(),
                expected_etag: r.etag.clone(),
            }),
            // Deleted on Linux, changed on iCloud: iCloud wins, restore it.
            (None, Some(r)) => self.place_locally(r),
            (Some(l), Some(r)) => match (local_changed, remote_changed) {
                (true, false) => {
                    let local = self.path_of(l);
                    self.push(Action::Upload {
                        local,
                        remote: r.path.clone(),
                        local_id: l.id,
                        expected_etag: Some(r.etag.clone()),
                        size: l.size,
                        stable_for: l.stable_for,
                    });
                }
                (false, true) => {
                    let local = self.path_of(l);
                    self.push(Action::Download {
                        remote: r.path.clone(),
                        local,
                        size: r.size,
                        replaces: Some(version(l)),
                    });
                }
                (true, true) => {
                    // Download to where the local file was before it became a copy.
                    let original = self.path_of(l);
                    if self.keep_as_conflict_copy(l).is_some() {
                        self.push(download_new(r, original));
                    }
                }
                (false, false) => self.decide_path(l, r, b),
            },
        }
    }

    /// Content is in sync; only the path may differ. (If both sides already
    /// agree, next_base records it; no action is needed.)
    fn decide_path(&mut self, l: &LocalFile, r: &RemoteFile, b: &BaseEntry) {
        let local_path = self.path_of(l);
        if same_place(local_path.as_str(), r.path.as_str()) {
            // Nothing to do.
        } else if !same_place(r.path.as_str(), b.path.as_str()) {
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
                    if let Some(copy) = self.free_copy_name(&from) {
                        self.local_move(l.id, from, copy.clone());
                        self.push(upload_new(l, copy.clone(), copy));
                    }
                } else {
                    self.push(upload_new(l, from, target));
                }
            }
            (None, Some(r)) => self.place_locally(r),
            // New on both sides under the same name: without a server checksum
            // the contents cannot be compared, so iCloud wins and the local
            // file becomes a conflict copy.
            (Some(l), Some(r)) => {
                if self.keep_as_conflict_copy(l).is_some() {
                    self.push(download_new(r, r.path.clone()));
                }
            }
        }
    }

    /// Rename the local file to a conflict copy name and upload it as a new
    /// file. `None` (nothing planned) if no free name was found.
    fn keep_as_conflict_copy(&mut self, l: &LocalFile) -> Option<()> {
        let from = self.path_of(l);
        let copy = self.free_copy_name(&from)?;
        self.local_move(l.id, from, copy.clone());
        self.push(Action::UploadConflictCopy {
            path: copy,
            local_id: l.id,
            size: l.size,
            stable_for: l.stable_for,
        });
        Some(())
    }

    /// Download a server file to its server path, making room locally if needed.
    fn place_locally(&mut self, r: &RemoteFile) {
        if self.evict_local(&r.path, None).is_some() {
            self.push(download_new(r, r.path.clone()));
        }
    }

    /// Move a local file to `target`, making room locally if needed.
    fn move_locally(&mut self, l: &LocalFile, target: RelPath) {
        if self.evict_local(&target, Some(l.id)).is_some() {
            let from = self.path_of(l);
            self.local_move(l.id, from, target);
        }
    }

    /// Repeat a local move on the server, or yield if the name is taken there.
    fn move_remotely(&mut self, l: &LocalFile, r: &RemoteFile) {
        let from = self.path_of(l);
        let target = RelPath::new(nfc(from.as_str()));
        let to = if self.remote_taken_by_other(&target, Some(&r.doc_id)) {
            let Some(copy) = self.free_copy_name(&from) else {
                return;
            };
            self.local_move(l.id, from, copy.clone());
            copy
        } else {
            target
        };
        self.push(Action::MoveRemote {
            from: r.path.clone(),
            to,
            expected_etag: r.etag.clone(),
        });
    }

    /// If another local file stands on `target`, rename it to a conflict
    /// copy name. `None` if it is in the way and no free name was found.
    fn evict_local(&mut self, target: &RelPath, owner: Option<LocalId>) -> Option<()> {
        let k = key(target.as_str());
        let Some(&occupant) = self.local_taken.get(&k) else {
            return Some(());
        };
        if Some(occupant) == owner {
            return Some(());
        }
        let from = self
            .current
            .get(&occupant)
            .cloned()
            .unwrap_or_else(|| target.clone());
        let copy = self.free_copy_name(&from)?;
        self.local_move(occupant, from, copy);
        Some(())
    }

    /// Current local path of a file (after the moves planned so far).
    fn path_of(&self, l: &LocalFile) -> RelPath {
        self.current
            .get(&l.id)
            .cloned()
            .unwrap_or_else(|| l.path.clone())
    }

    /// The only way the planner moves a local file: records the new path.
    fn local_move(&mut self, id: LocalId, from: RelPath, to: RelPath) {
        self.local_taken.remove(&key(from.as_str()));
        self.local_taken.insert(key(to.as_str()), id);
        self.current.insert(id, to.clone());
        self.push(Action::MoveLocal {
            from,
            to,
            local_id: id,
        });
    }

    fn remote_taken_by_other(&self, target: &RelPath, owner: Option<&DocId>) -> bool {
        self.remote_taken
            .get(&key(target.as_str()))
            .is_some_and(|holder| Some(holder) != owner)
    }

    /// A conflict copy name no file uses, in the same folder, that is synced
    /// (not an ignored name). `None` after [`MAX_COPY_NAME_ATTEMPTS`]; the
    /// file is then reported and left alone.
    fn free_copy_name(&mut self, path: &RelPath) -> Option<RelPath> {
        for n in 1..=MAX_COPY_NAME_ATTEMPTS {
            let candidate = (self.conflict_name)(path, n);
            let usable =
                candidate.parent() == path.parent() && !safety_names::is_ignored(candidate.name());
            if usable && self.names_in_use.insert(key(candidate.as_str())) {
                return Some(candidate);
            }
        }
        self.plan.no_copy_name.push(path.clone());
        None
    }

    fn push(&mut self, action: Action) {
        self.plan.actions.push(action);
    }
}

fn upload_new(l: &LocalFile, local: RelPath, remote: RelPath) -> Action {
    Action::Upload {
        local,
        remote,
        local_id: l.id,
        expected_etag: None,
        size: l.size,
        stable_for: l.stable_for,
    }
}

/// Download to a local path that must be free.
fn download_new(r: &RemoteFile, local: RelPath) -> Action {
    Action::Download {
        remote: r.path.clone(),
        local,
        size: r.size,
        replaces: None,
    }
}

/// The local file as the planner saw it (a precondition of local actions).
fn version(l: &LocalFile) -> LocalVersion {
    LocalVersion {
        id: l.id,
        hash: l.hash.clone(),
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
    for path in actions.iter().filter_map(Action::new_remote_name) {
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
