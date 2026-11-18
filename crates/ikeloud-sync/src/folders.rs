//! Folders (decision 0027): created and deleted like files, three-way.
//!
//! | local | iCloud | common state | action                                  |
//! |-------|--------|--------------|-----------------------------------------|
//! | yes   | yes    | –            | none (recorded)                         |
//! | yes   | no     | no           | create on iCloud                        |
//! | no    | yes    | no           | create locally                          |
//! | yes   | no     | yes          | deleted on iCloud: remove locally, empty only |
//! | no    | yes    | yes          | deleted locally: trash on iCloud, empty only  |
//!
//! Content is decided by the files: a deleted folder with files goes in
//! two steps, its files first, the folder in a later run when it is empty.
//! A file restored by the file rules keeps its folder (iCloud wins).
//!
//! "Empty" locally: no file of the scan in it, nothing the plan puts in it,
//! and every subfolder removed too; the removal itself is `rmdir`, which
//! fails on a folder with anything at all in it. On iCloud: no file of the
//! listing in it, nothing the plan puts there, every subfolder deleted
//! locally too; trashed deepest first, each checked empty again right
//! before (by the remote).

use std::collections::{BTreeMap, BTreeSet};

use ikeloud_safety::{Action, Etag, RelPath, names as safety_names};

use crate::names::{key, nfc};
use crate::state::States;

/// Folders on both sides and in the last common state.
#[derive(Debug, Clone, Default)]
pub struct FolderStates {
    /// Folders on Linux.
    pub local: Vec<RelPath>,
    /// Folders on iCloud with their versions.
    pub remote: BTreeMap<RelPath, Etag>,
    /// Folders both sides had at the last sync.
    pub base: Vec<RelPath>,
}

/// Whether a folder takes part: every component a synced name.
fn synced(path: &RelPath) -> bool {
    safety_names::validate_path(path).is_ok()
        && path
            .as_str()
            .split('/')
            .all(|part| !safety_names::is_ignored(part))
}

/// Is `inner` (a key) `outer` (a key) or under it?
fn within(inner: &str, outer: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn depth(path: &RelPath) -> usize {
    path.as_str().matches('/').count()
}

/// Folder actions of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderPlan {
    /// To carry out before the file actions, shallower first.
    pub create: Vec<Action>,
    /// To carry out after the file actions, deeper first.
    pub remove: Vec<Action>,
}

/// Folder actions for a run, given the file plan `files` already made from
/// `states`. Folders the file plan creates on iCloud are not created twice.
pub fn plan_folders(folders: &FolderStates, states: &States, files: &[Action]) -> FolderPlan {
    let by_key = |paths: &mut dyn Iterator<Item = &RelPath>| -> BTreeMap<String, RelPath> {
        paths
            .filter(|p| synced(p))
            .map(|p| (key(p.as_str()), p.clone()))
            .collect()
    };
    let local = by_key(&mut folders.local.iter());
    let remote = by_key(&mut folders.remote.keys());
    let base: BTreeSet<String> = folders.base.iter().map(|p| key(p.as_str())).collect();

    // Names files hold (a folder never replaces a file) and every path the
    // file plan touches (nothing is removed where something goes).
    let local_files: Vec<String> = states.local.iter().map(|f| key(f.path.as_str())).collect();
    let remote_files: Vec<String> = states.remote.iter().map(|f| key(f.path.as_str())).collect();
    let planned: Vec<String> = files
        .iter()
        .flat_map(|a| {
            let mut paths = vec![key(a.path().as_str())];
            if let Action::Upload { local, .. } = a {
                paths.push(key(local.as_str()));
            }
            paths
        })
        .collect();
    let created_remote: BTreeSet<String> = files
        .iter()
        .filter_map(|a| match a {
            Action::CreateRemoteFolder { path } => Some(key(path.as_str())),
            _ => None,
        })
        .collect();

    let mut create_remote: Vec<RelPath> = Vec::new();
    let mut create_local: Vec<RelPath> = Vec::new();
    // Deleted on one side, candidates for removal on the other.
    let mut gone_remote: BTreeMap<String, RelPath> = BTreeMap::new();
    let mut gone_local: BTreeMap<String, RelPath> = BTreeMap::new();
    for (k, path) in &local {
        if remote.contains_key(k) {
            continue;
        }
        if base.contains(k) {
            gone_remote.insert(k.clone(), path.clone());
        } else if !created_remote.contains(k) && !remote_files.contains(k) {
            create_remote.push(path.map(nfc).unwrap_or_else(|| path.clone()));
        }
    }
    for (k, path) in &remote {
        if local.contains_key(k) {
            continue;
        }
        if base.contains(k) {
            gone_local.insert(k.clone(), path.clone());
        } else if !local_files.contains(k) {
            create_local.push(path.clone());
        }
    }

    // A new folder keeps its parents, also one deleted on iCloud (as a new
    // file keeps its folder): they are created again first.
    let mut wanted: BTreeSet<String> = create_remote.iter().map(|p| key(p.as_str())).collect();
    for path in create_remote.clone() {
        for ancestor in path.ancestors() {
            let k = key(ancestor.as_str());
            if !remote.contains_key(&k) && !created_remote.contains(&k) && wanted.insert(k) {
                create_remote.push(ancestor);
            }
        }
    }
    let mut plan = FolderPlan::default();
    create_remote.sort_by_key(depth);
    plan.create.extend(
        create_remote
            .into_iter()
            .map(|path| Action::CreateRemoteFolder { path }),
    );
    create_local.sort_by_key(depth);
    plan.create.extend(
        create_local
            .into_iter()
            .map(|path| Action::CreateLocalFolder { path }),
    );

    // Deleted on iCloud: remove locally what is empty, deepest first.
    let mut remove: Vec<RelPath> = gone_remote
        .iter()
        .filter(|(k, _)| {
            !local_files.iter().any(|f| within(f, k))
                && !planned.iter().any(|p| within(p, k))
                && local
                    .keys()
                    .filter(|sub| within(sub, k))
                    .all(|sub| gone_remote.contains_key(sub))
        })
        .map(|(_, p)| p.clone())
        .collect();
    remove.sort_by_key(|p| std::cmp::Reverse(depth(p)));
    plan.remove.extend(
        remove
            .into_iter()
            .map(|path| Action::RemoveLocalFolder { path }),
    );

    // Deleted locally: trash on iCloud what is empty, deepest first (the
    // remote checks each one is empty right before, see TrashRemoteFolder).
    let empty_remote = |k: &String| {
        !remote_files.iter().any(|f| within(f, k))
            && !planned.iter().any(|p| within(p, k))
            && remote
                .keys()
                .filter(|sub| within(sub, k))
                .all(|sub| gone_local.contains_key(sub))
    };
    let mut trash: Vec<(RelPath, Etag)> = gone_local
        .iter()
        .filter(|(k, _)| empty_remote(k))
        .filter_map(|(_, path)| Some((path.clone(), folders.remote.get(path)?.clone())))
        .collect();
    trash.sort_by_key(|(p, _)| std::cmp::Reverse(depth(p)));
    plan.remove.extend(
        trash
            .into_iter()
            .map(|(path, expected_etag)| Action::TrashRemoteFolder {
                path,
                expected_etag,
            }),
    );
    plan
}

/// The folders both sides have after a run, plus those deleted on one side
/// whose deletion could not be carried over yet (not empty): the decision
/// stands until it can.
pub fn next_base_folders(old: &[RelPath], local: &[RelPath], remote: &[RelPath]) -> Vec<RelPath> {
    let local: BTreeSet<String> = local.iter().map(|p| key(p.as_str())).collect();
    let remote_keys: BTreeSet<String> = remote.iter().map(|p| key(p.as_str())).collect();
    let mut next: BTreeMap<String, RelPath> = BTreeMap::new();
    for path in remote.iter().filter(|p| synced(p)) {
        let k = key(path.as_str());
        if local.contains(&k) {
            next.insert(k, path.clone());
        }
    }
    for path in old {
        let k = key(path.as_str());
        if local.contains(&k) || remote_keys.contains(&k) {
            next.entry(k).or_insert_with(|| path.clone());
        }
    }
    next.into_values().collect()
}
