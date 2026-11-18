//! Folders (decision 0027): the three-way table and what "empty" means.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::collections::BTreeMap;

use ikeloud_safety::{Action, Etag};
use ikeloud_sync::{FolderPlan, FolderStates, States, next_base_folders, plan_folders};
use ikeloud_testkit::{local, p, remote};

fn folders(local: &[&str], remote: &[&str], base: &[&str]) -> FolderStates {
    FolderStates {
        local: local.iter().map(|x| p(x)).collect(),
        remote: remote
            .iter()
            .map(|x| (p(x), Etag::new(format!("f-{x}"))))
            .collect::<BTreeMap<_, _>>(),
        base: base.iter().map(|x| p(x)).collect(),
    }
}

fn no_files() -> States {
    States::default()
}

fn create_remote(x: &str) -> Action {
    Action::CreateRemoteFolder { path: p(x) }
}
fn create_local(x: &str) -> Action {
    Action::CreateLocalFolder { path: p(x) }
}
fn remove_local(x: &str) -> Action {
    Action::RemoveLocalFolder { path: p(x) }
}
fn trash_remote(x: &str) -> Action {
    Action::TrashRemoteFolder {
        path: p(x),
        expected_etag: Etag::new(format!("f-{x}")),
    }
}

#[test]
fn new_folders_are_created_on_the_other_side_parents_first() {
    let plan = plan_folders(
        &folders(&["a", "a/b", "a/b/c"], &["x/y", "x"], &[]),
        &no_files(),
        &[],
    );
    assert_eq!(
        plan,
        FolderPlan {
            create: vec![
                create_remote("a"),
                create_remote("a/b"),
                create_remote("a/b/c"),
                create_local("x"),
                create_local("x/y"),
            ],
            remove: vec![],
        }
    );
}

#[test]
fn folders_on_both_sides_need_nothing_letter_case_aside() {
    let plan = plan_folders(&folders(&["Fotky"], &["fotky"], &[]), &no_files(), &[]);
    assert_eq!(plan, FolderPlan::default());
}

#[test]
fn a_folder_deleted_on_icloud_is_removed_locally_deepest_first() {
    let plan = plan_folders(
        &folders(&["a", "a/b", "keep"], &["keep"], &["a", "a/b", "keep"]),
        &no_files(),
        &[],
    );
    assert_eq!(plan.remove, vec![remove_local("a/b"), remove_local("a")]);
    assert!(plan.create.is_empty());
}

#[test]
fn a_folder_deleted_on_icloud_with_local_files_stays_for_now() {
    let mut states = no_files();
    states.local = vec![local("a/b/new.txt", 1, "h")];
    let plan = plan_folders(&folders(&["a", "a/b"], &[], &["a", "a/b"]), &states, &[]);
    assert_eq!(plan, FolderPlan::default(), "files decide first");
}

#[test]
fn a_folder_deleted_locally_goes_to_the_icloud_trash_deepest_first() {
    let plan = plan_folders(
        &folders(&["keep"], &["a", "a/b", "keep"], &["a", "a/b", "keep"]),
        &no_files(),
        &[],
    );
    assert_eq!(plan.remove, vec![trash_remote("a/b"), trash_remote("a")]);
}

#[test]
fn a_folder_deleted_locally_with_files_on_icloud_stays_for_now() {
    let mut states = no_files();
    states.remote = vec![remote("a/b/photo.jpg", "d1", "e1")];
    let plan = plan_folders(&folders(&[], &["a", "a/b"], &["a", "a/b"]), &states, &[]);
    assert_eq!(plan, FolderPlan::default());
}

#[test]
fn a_subfolder_still_on_linux_keeps_its_parent_on_icloud() {
    // "a" deleted locally but "a/b" exists locally: nothing to trash.
    let plan = plan_folders(
        &folders(&["a/b"], &["a", "a/b"], &["a", "a/b"]),
        &no_files(),
        &[],
    );
    assert!(plan.remove.is_empty(), "{plan:?}");
}

#[test]
fn nothing_is_removed_where_the_plan_puts_something() {
    let download = Action::Download {
        remote: p("a/f.txt"),
        expected_etag: Etag::new("e"),
        local: p("a/f.txt"),
        size: 1,
        replaces: None,
    };
    let plan = plan_folders(&folders(&["a"], &[], &["a"]), &no_files(), &[download]);
    assert!(plan.remove.is_empty());
}

#[test]
fn folders_the_file_plan_creates_are_not_created_twice() {
    let plan = plan_folders(
        &folders(&["a"], &[], &[]),
        &no_files(),
        &[create_remote("a")],
    );
    assert!(plan.create.is_empty());
}

#[test]
fn ignored_names_are_not_synced_as_folders() {
    // The same names as files (safety::names::is_ignored).
    let plan = plan_folders(
        &folders(&["download.part", "a.swp/b"], &[], &[]),
        &no_files(),
        &[],
    );
    assert_eq!(plan, FolderPlan::default());
}

#[test]
fn the_common_state_keeps_pending_deletions() {
    let next = next_base_folders(
        &[p("gone"), p("pending"), p("both")],
        &[p("both"), p("pending"), p("new-local")],
        &[p("both"), p("new-both")],
    );
    // both: on both sides; pending: deleted on iCloud, not removed yet;
    // gone: on neither side; new-local: not on iCloud yet.
    assert_eq!(next, vec![p("both"), p("pending")]);
}

#[test]
fn a_new_subfolder_keeps_its_parent_deleted_on_icloud() {
    // Found by the daemon simulation: "x" deleted on iCloud, "x/y" new on
    // Linux. The new folder wins, as a new file would.
    let plan = plan_folders(&folders(&["x", "x/y"], &[], &["x"]), &no_files(), &[]);
    assert_eq!(
        plan,
        FolderPlan {
            create: vec![create_remote("x"), create_remote("x/y")],
            remove: vec![],
        }
    );
}
