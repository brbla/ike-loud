//! Whole sync runs: the real scanner, planner, safety layer, executor,
//! local actions and state database against a fake iCloud.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)] // test code

mod common;

use common::Fixture;
use ikeloud_daemon::RunOutcome;
use ikeloud_safety::Rejection;
use ikeloud_state::JournalKind;
use ikeloud_sync::StopReason;

#[test]
fn first_sync_brings_both_sides_together() {
    let mut f = Fixture::new("first");
    f.write("Linux.txt", b"from linux");
    f.write("Documents/Smlouva.odt", b"contract");
    f.icloud.put("iPhone.txt", b"from iphone");
    f.icloud.put("Photos/a.jpg", b"jpeg");
    f.settle();
    f.assert_in_sync();
    assert_eq!(
        f.files("root"),
        [
            "Documents/Smlouva.odt",
            "Linux.txt",
            "Photos/a.jpg",
            "iPhone.txt"
        ]
    );
    let base = f.db.base().expect("base");
    assert_eq!(base.len(), 4, "every file is in the common state");
    let again = f.run();
    assert!(
        again.executed.done.is_empty(),
        "nothing left to do: {again:?}"
    );
}

#[test]
fn edits_flow_both_ways_and_replaced_versions_are_kept() {
    let mut f = Fixture::new("edits");
    f.write("a.txt", b"one");
    f.icloud.put("b.txt", b"one");
    f.settle();

    f.write("a.txt", b"two (linux)");
    f.icloud.edit("b.txt", b"two (iphone)");
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.icloud.content("a.txt"), Some(&b"two (linux)"[..]));
    assert_eq!(f.read("b.txt").as_deref(), Some(&b"two (iphone)"[..]));
    // Both replaced versions are kept: the old local b.txt (replaced by the
    // download), and the server's a.txt (read before the upload replaced it
    // on iCloud; spec: before every overwrite, locally and on the server).
    let history = f.files("history");
    assert_eq!(history.len(), 2, "{history:?}");
    let kept = |name: &str| {
        history
            .iter()
            .find(|p| p.ends_with(&format!("/{name}")))
            .map(|p| std::fs::read(f.dir.join("history").join(p)).expect("readable"))
    };
    assert_eq!(
        kept("b.txt").as_deref(),
        Some(&b"one"[..]),
        "old local b.txt"
    );
    assert_eq!(
        kept("a.txt").as_deref(),
        Some(&b"one"[..]),
        "old server a.txt"
    );
    let kinds: Vec<JournalKind> =
        f.db.journal(10)
            .expect("journal")
            .iter()
            .map(|e| e.kind)
            .collect();
    assert!(kinds.contains(&JournalKind::OverwrittenRemote), "{kinds:?}");
    assert!(kinds.contains(&JournalKind::OverwrittenLocal), "{kinds:?}");
}

#[test]
fn deletions_go_to_a_trash_on_the_other_side() {
    let mut f = Fixture::new("delete");
    f.write("linux.txt", b"l");
    f.write("keep.txt", b"k");
    f.icloud.put("iphone.txt", b"i");
    f.settle();

    f.remove("linux.txt");
    f.icloud.delete("iphone.txt");
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.files("root"), ["keep.txt"]);
    assert!(
        f.icloud
            .trash
            .contains(&("linux.txt".into(), b"l".to_vec())),
        "deleted on Linux: in Recently Deleted on iCloud"
    );
    assert_eq!(
        f.files("trash"),
        ["files/iphone.txt", "info/iphone.txt.trashinfo"],
        "deleted on the iPhone: in the system trash"
    );
}

#[test]
fn a_conflict_keeps_both_versions() {
    let mut f = Fixture::new("conflict");
    f.write("report.odt", b"v1");
    f.settle();

    f.write("report.odt", b"v2 linux");
    let path = "report.odt";
    f.icloud.edit(path, b"v2 iphone");
    f.settle();
    f.assert_in_sync();
    assert_eq!(
        f.read("report.odt").as_deref(),
        Some(&b"v2 iphone"[..]),
        "iCloud wins"
    );
    assert_eq!(
        f.read("report.odt (conflict 1)").as_deref(),
        Some(&b"v2 linux"[..]),
        "the local version is a conflict copy, on both sides"
    );
}

#[test]
fn a_rename_is_a_move_on_icloud() {
    let mut f = Fixture::new("rename");
    f.write("a.txt", b"x");
    f.settle();
    let doc = f.icloud.files["a.txt"].doc;
    std::fs::create_dir(f.root().join("Archive")).expect("folder");
    std::fs::rename(f.root().join("a.txt"), f.root().join("Archive/a.txt")).expect("mv");
    f.settle();
    f.assert_in_sync();
    assert_eq!(
        f.icloud.files["Archive/a.txt"].doc, doc,
        "the same document, moved"
    );
    assert!(f.icloud.trash.is_empty(), "not deleted and uploaded again");
}

#[test]
fn going_offline_in_the_middle_is_safe() {
    let mut f = Fixture::new("offline");
    for n in 0..5 {
        f.write(&format!("f{n}.txt"), b"x");
    }
    f.icloud.offline_after = Some(2);
    let r = f.run();
    assert_eq!(r.executed.stopped, Some(StopReason::Offline));
    assert_eq!(r.executed.done.len(), 2, "{r:?}");
    assert!(
        r.committed,
        "what the transfers prove is stored even offline"
    );
    assert_eq!(
        f.db.base().expect("base").len(),
        2,
        "the two uploaded files"
    );

    // Back online: the rest follows; no conflict copies of the two files
    // uploaded before the outage, nothing uploaded twice.
    f.icloud.offline_after = None;
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.icloud.files.len(), 5);
}

#[test]
fn a_mass_deletion_waits_for_the_user() {
    let mut f = Fixture::new("mass");
    for n in 0..60 {
        f.write(&format!("f{n:02}.txt"), b"x");
    }
    f.settle();
    for n in 0..55 {
        f.remove(&format!("f{n:02}.txt"));
    }
    let RunOutcome::Rejected(Rejection::MassChangeNeedsConfirmation(summary)) = f.run_with(None)
    else {
        panic!("expected the brake");
    };
    assert_eq!(summary.destructive(), 55);
    assert_eq!(
        f.icloud.files.len(),
        60,
        "nothing deleted before the user confirms"
    );

    let RunOutcome::Ran(r) = f.run_with(Some(summary.confirm())) else {
        panic!("confirmed");
    };
    assert_eq!(r.executed.done.len(), 55);
    assert_eq!(f.icloud.files.len(), 5);
    assert_eq!(f.icloud.trash.len(), 55, "all in Recently Deleted");
    let journal = f.db.journal(100).expect("journal");
    assert!(
        journal
            .iter()
            .any(|e| e.kind == JournalKind::MassChangeConfirmed),
        "the confirmation is in the journal and restarts the brake window"
    );
    f.settle();

    // The confirmed deletions do not count against the next one (they did
    // while the confirmation was recorded before them: TLA+ SyncBrake).
    f.remove("f55.txt");
    let RunOutcome::Ran(r) = f.run_with(None) else {
        panic!("one more deletion after a confirmed mass deletion needs no question");
    };
    assert_eq!(r.executed.done.len(), 1);
}

#[test]
fn a_missing_folder_deletes_nothing() {
    let mut f = Fixture::new("missing");
    f.write("a.txt", b"x");
    f.settle();
    std::fs::remove_dir_all(f.root()).expect("unplug");
    assert!(matches!(
        f.run_with(None),
        RunOutcome::Rejected(Rejection::LocalFolderUnavailable)
    ));
    assert_eq!(f.icloud.files.len(), 1, "iCloud untouched");
    assert!(f.icloud.trash.is_empty());
}

#[test]
fn icloud_unreachable_changes_nothing() {
    let mut f = Fixture::new("unreachable");
    f.write("a.txt", b"x");
    f.icloud.offline_after = Some(0);
    assert!(matches!(
        f.run_with(None),
        RunOutcome::RemoteUnavailable(StopReason::Offline)
    ));
    assert_eq!(f.db.base().expect("base"), vec![]);
}
