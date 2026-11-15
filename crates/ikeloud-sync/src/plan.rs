//! Building a plan of actions from three states.
//!
//! Decision table (spec, chapter Basic principles; TLA+ `Plan`):
//!
//! | Change since the last sync | Action                                          |
//! |----------------------------|-------------------------------------------------|
//! | only on Linux              | upload (deletion → iCloud trash)                |
//! | only on iCloud             | download (deletion → system trash)              |
//! | on both                    | iCloud wins, local version → conflict copy      |
//! | nowhere                    | nothing                                         |
//!
//! A change on Linux is detected by content hash, on iCloud by etag, never by
//! time. Files are paired by the key from [`crate::names::key`].

use std::collections::{BTreeMap, BTreeSet};

use ikeloud_safety::{Action, RelPath, names as safety_names};

use crate::names::{key, nfc};
use crate::state::{BaseEntry, LocalFile, RemoteFile, States};

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
    /// Common-state entries that no longer exist on either side.
    pub forget: Vec<RelPath>,
    /// Name collisions the sync skipped.
    pub collisions: Vec<Collision>,
}

/// Builds a plan from the three states.
///
/// `conflict_name(path, n)` returns the name of a conflict copy of `path`;
/// `n` is the attempt number from 1 and increases until the name is free.
/// The caller supplies the name because the user sees it and it is
/// translated.
pub fn build_plan(states: &States, conflict_name: &dyn Fn(&RelPath, u32) -> RelPath) -> Plan {
    let mut plan = Plan::default();
    let local = index(
        &states.local,
        |f| &f.path,
        Side::Local,
        &mut plan.collisions,
    );
    let remote = index(
        &states.remote,
        |f| &f.path,
        Side::Remote,
        &mut plan.collisions,
    );
    let base: BTreeMap<String, &BaseEntry> = states
        .base
        .iter()
        .map(|b| (key(b.path.as_str()), b))
        .collect();
    let blocked: BTreeSet<String> = plan
        .collisions
        .iter()
        .flat_map(|c| c.paths.iter().map(|p| key(p.as_str())))
        .collect();

    let mut taken: BTreeSet<String> = local.keys().chain(remote.keys()).cloned().collect();
    taken.extend(blocked.iter().cloned());

    let keys: BTreeSet<&String> = local
        .keys()
        .chain(remote.keys())
        .chain(base.keys())
        .collect();
    let mut actions = Vec::new();
    for k in keys {
        if blocked.contains(k) {
            continue;
        }
        let l = local.get(k).copied();
        let r = remote.get(k).copied();
        let b = base.get(k).copied();
        decide(l, r, b, &mut actions, &mut plan.forget, &mut |path| {
            free_conflict_name(path, conflict_name, &mut taken)
        });
    }

    plan.actions = missing_folders(&actions, states);
    plan.actions.extend(actions);
    plan
}

/// The decision table for one file.
fn decide(
    l: Option<&LocalFile>,
    r: Option<&RemoteFile>,
    b: Option<&BaseEntry>,
    actions: &mut Vec<Action>,
    forget: &mut Vec<RelPath>,
    copy_name: &mut dyn FnMut(&RelPath) -> RelPath,
) {
    let changed_locally = match (l, b) {
        (None, None) => false,
        (Some(l), Some(b)) => l.hash != b.hash,
        _ => true,
    };
    let changed_remotely = match (r, b) {
        (None, None) => false,
        (Some(r), Some(b)) => r.etag != b.etag,
        _ => true,
    };

    match (changed_locally, changed_remotely, l, r) {
        (false, false, _, _) => {}
        // Deleted on both sides: only forget the common state.
        (_, _, None, None) => {
            if let Some(b) = b {
                forget.push(b.path.clone());
            }
        }
        // Only on Linux.
        (true, false, Some(l), r) => actions.push(Action::Upload {
            path: r.map_or_else(|| RelPath::new(nfc(l.path.as_str())), |r| r.path.clone()),
            expected_etag: r.map(|r| r.etag.clone()),
            size: l.size,
            stable_for: l.stable_for,
        }),
        (true, false, None, Some(r)) => actions.push(Action::TrashRemote {
            path: r.path.clone(),
            expected_etag: r.etag.clone(),
        }),
        // Only on iCloud.
        (false, true, l, Some(r)) => actions.push(Action::Download {
            path: l.map_or_else(|| r.path.clone(), |l| l.path.clone()),
            size: r.size,
            overwrites_local: l.is_some(),
        }),
        (false, true, Some(l), None) => {
            actions.push(Action::TrashLocal {
                path: l.path.clone(),
            });
        }
        // On both: iCloud wins.
        (true, true, None, Some(r)) => actions.push(Action::Download {
            path: r.path.clone(),
            size: r.size,
            overwrites_local: false,
        }),
        (true, true, Some(l), r) => {
            let name = copy_name(&l.path);
            actions.push(Action::MoveLocal {
                from: l.path.clone(),
                to: name.clone(),
            });
            actions.push(Action::UploadConflictCopy {
                path: name,
                size: l.size,
            });
            if let Some(r) = r {
                actions.push(Action::Download {
                    path: r.path.clone(),
                    size: r.size,
                    overwrites_local: false,
                });
            }
        }
    }
}

/// Indexes files by key; files with the same key are reported as a collision.
fn index<'a, T>(
    files: &'a [T],
    path: impl Fn(&T) -> &RelPath,
    side: Side,
    collisions: &mut Vec<Collision>,
) -> BTreeMap<String, &'a T> {
    let mut groups: BTreeMap<String, Vec<&'a T>> = BTreeMap::new();
    for f in files {
        if safety_names::is_ignored(path(f).name()) {
            continue;
        }
        groups.entry(key(path(f).as_str())).or_default().push(f);
    }
    let mut index = BTreeMap::new();
    for (k, group) in groups {
        match group.as_slice() {
            [single] => {
                index.insert(k, *single);
            }
            many => {
                let mut paths: Vec<RelPath> = many.iter().map(|f| path(f).clone()).collect();
                paths.sort();
                collisions.push(Collision { side, paths });
            }
        }
    }
    index
}

fn free_conflict_name(
    path: &RelPath,
    conflict_name: &dyn Fn(&RelPath, u32) -> RelPath,
    taken: &mut BTreeSet<String>,
) -> RelPath {
    let mut n = 1;
    loop {
        let candidate = conflict_name(path, n);
        if taken.insert(key(candidate.as_str())) || n == u32::MAX {
            return candidate;
        }
        n += 1;
    }
}

/// Server folders that must be created before uploading new files,
/// shallower first.
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
