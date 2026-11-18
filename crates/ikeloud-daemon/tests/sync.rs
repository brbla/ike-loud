//! Whole sync runs: the real scanner, planner, safety layer, executor,
//! local actions and state database against a fake iCloud.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)] // test code

use std::fs;

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

#[test]
fn uploads_run_in_parallel_and_all_are_recorded() {
    let mut f = Fixture::new("parallel");
    for i in 0..20 {
        f.write(&format!("d{}/f{i}.txt", i % 3), format!("{i}").as_bytes());
    }
    match f.run_parallel() {
        RunOutcome::Ran(report) => {
            assert_eq!(report.executed.stopped, None);
            let uploads = report
                .executed
                .done
                .iter()
                .filter(|a| matches!(a, ikeloud_safety::Action::Upload { .. }))
                .count();
            assert_eq!(uploads, 20);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.icloud.files.len(), 20);
    assert_eq!(
        f.db.base().expect("base").len(),
        20,
        "every upload recorded"
    );
    assert!(f.run().executed.done.is_empty(), "nothing left to do");
}

#[test]
fn an_outage_in_a_parallel_group_loses_no_record() {
    let mut f = Fixture::new("parallel-outage");
    for i in 0..12 {
        f.write(&format!("f{i:02}.txt"), format!("{i}").as_bytes());
    }
    // Offline after five writes: some uploads are done, the rest stop.
    f.icloud.offline_after = Some(5);
    let first = f.run_parallel();
    let RunOutcome::Ran(report) = first else {
        panic!("{first:?}")
    };
    assert_eq!(report.executed.stopped, Some(StopReason::Offline));
    let on_icloud = f.icloud.files.len();
    assert!(on_icloud >= 5, "{on_icloud}");
    // Whatever reached iCloud is in the common state, so the next run does
    // not take it for a new file on both sides (no conflict copies).
    assert_eq!(f.db.base().expect("base").len(), on_icloud);
    f.icloud.offline_after = None;
    let second = f.run_parallel();
    assert!(matches!(second, RunOutcome::Ran(_)), "{second:?}");
    assert_eq!(f.icloud.files.len(), 12);
    assert!(
        f.icloud.files.keys().all(|p| !p.contains("conflict")),
        "{:?}",
        f.icloud.files.keys()
    );
    assert!(f.run().executed.done.is_empty());
}

/// Files and conflict copies on both sides after the fixture settled.
fn conflict_copies(f: &Fixture) -> Vec<String> {
    let mut all = f.files("root");
    all.extend(f.icloud.files.keys().cloned());
    all.retain(|p| p.contains("(conflict"));
    all
}

#[test]
fn an_upload_whose_answer_is_lost_is_recognized_by_its_checksum() {
    let mut f = Fixture::new("lost-new");
    f.write("a.txt", b"new on linux");
    f.write("b.txt", b"another file");
    f.icloud.lose_upload_answer = true;
    let r = f.run();
    assert_eq!(r.executed.stopped, None, "not an outage: {r:?}");
    assert_eq!(r.executed.skipped.len(), 1, "only this file waits");
    assert_eq!(
        f.icloud.content("b.txt"),
        Some(&b"another file"[..]),
        "the run went on"
    );
    assert_eq!(
        f.icloud.content("a.txt"),
        Some(&b"new on linux"[..]),
        "it did land"
    );
    assert_eq!(f.db.unconfirmed().expect("read").len(), 1);

    f.settle();
    f.assert_in_sync();
    assert_eq!(
        conflict_copies(&f),
        Vec::<String>::new(),
        "no conflict copy"
    );
    assert!(f.db.unconfirmed().expect("read").is_empty(), "settled");
    assert_eq!(f.db.base().expect("base").len(), 2);
    let uploaded =
        f.db.journal(10)
            .expect("journal")
            .iter()
            .filter(|e| e.kind == JournalKind::Uploaded && e.path.as_str() == "a.txt")
            .count();
    assert_eq!(uploaded, 1, "in the journal once");
}

#[test]
fn a_lost_answer_for_a_new_version_needs_no_conflict_copy_either() {
    let mut f = Fixture::new("lost-edit");
    f.write("a.txt", b"v1");
    f.settle();
    f.write("a.txt", b"v2 from linux");
    f.icloud.lose_upload_answer = true;
    f.run();
    f.settle();
    f.assert_in_sync();
    assert_eq!(conflict_copies(&f), Vec::<String>::new());
    assert_eq!(f.icloud.content("a.txt"), Some(&b"v2 from linux"[..]));
}

#[test]
fn a_file_edited_after_a_lost_answer_keeps_both_versions() {
    // A transfer proves both sides equal only while neither changed (the
    // common state's rule, also for runs that crashed): the edited file is
    // new on both sides, so iCloud's copy wins and the edit is kept as a
    // conflict copy. Rare (an edit within a minute of a lost answer);
    // nothing is lost.
    let mut f = Fixture::new("lost-then-edit");
    f.write("a.txt", b"first");
    f.icloud.lose_upload_answer = true;
    f.run();
    f.write("a.txt", b"second, longer");
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.icloud.content("a.txt"), Some(&b"first"[..]));
    assert_eq!(
        f.icloud.content("a.txt (conflict 1)"),
        Some(&b"second, longer"[..])
    );
    assert!(f.db.unconfirmed().expect("read").is_empty(), "settled");
}

#[test]
fn without_checksums_a_lost_answer_still_ends_safely_in_a_conflict_copy() {
    let mut f = Fixture::new("lost-walk");
    f.write("a.txt", b"x");
    f.icloud.no_checksums = true;
    f.icloud.lose_upload_answer = true;
    f.run();
    f.settle();
    f.assert_in_sync();
    // Nothing lost: both copies hold the content.
    assert_eq!(conflict_copies(&f).len(), 2, "the copy, on both sides");
    assert_eq!(f.icloud.content("a.txt"), Some(&b"x"[..]));
}

#[test]
fn a_file_moved_on_icloud_after_a_lost_answer_is_moved_locally() {
    // Found by the daemon simulation: matched by document, not by path.
    let mut f = Fixture::new("lost-moved");
    f.write("a.txt", b"content");
    f.icloud.lose_upload_answer = true;
    f.run();
    let moved = f.icloud.files.remove("a.txt").expect("landed");
    f.icloud.folders.insert("d".into());
    f.icloud.files.insert("d/e.txt".into(), moved);
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.files("root"), ["d/e.txt"]);
    assert_eq!(conflict_copies(&f), Vec::<String>::new());
}

#[test]
fn a_lost_answer_of_an_upload_that_did_not_land_is_forgotten_once_uploaded_again() {
    let mut f = Fixture::new("lost-not-landed");
    f.write("a.txt", b"x");
    f.icloud.lose_upload_answer = true;
    f.run();
    // As if iCloud never created it.
    f.icloud.files.clear();
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.icloud.content("a.txt"), Some(&b"x"[..]));
    assert!(f.db.unconfirmed().expect("read").is_empty(), "forgotten");
    assert_eq!(conflict_copies(&f), Vec::<String>::new());
}

#[test]
fn empty_folders_are_created_on_the_other_side() {
    let mut f = Fixture::new("folders-new");
    f.settle(); // the first run records the folders
    fs::create_dir_all(f.root().join("Linux/empty")).expect("mkdir");
    f.icloud.folders.insert("iPhone".into());
    f.settle();
    f.assert_folders_in_sync();
    assert_eq!(f.local_folders(), ["Linux", "Linux/empty", "iPhone"]);
}

#[test]
fn a_folder_deleted_on_linux_goes_to_the_icloud_trash_after_its_files() {
    let mut f = Fixture::new("folders-del-local");
    f.write("keep.txt", b"an empty side stops sync: one file stays");
    f.write("Old/a/x.txt", b"x");
    fs::create_dir_all(f.root().join("Old/b")).expect("mkdir");
    f.settle();
    f.assert_folders_in_sync();
    fs::remove_dir_all(f.root().join("Old")).expect("rm");
    f.settle();
    f.assert_folders_in_sync();
    assert!(f.icloud.folders.is_empty(), "{:?}", f.icloud.folders);
    assert_eq!(f.icloud.trash.len(), 1, "the file is in Recently Deleted");
}

#[test]
fn a_folder_deleted_on_icloud_is_removed_locally_after_its_files() {
    let mut f = Fixture::new("folders-del-remote");
    f.icloud
        .put("keep.txt", b"an empty side stops sync: one file stays");
    f.icloud.put("Old/a/x.txt", b"x");
    f.icloud.folders.insert("Old/b".into());
    f.settle();
    f.assert_folders_in_sync();
    f.icloud.delete("Old/a/x.txt");
    f.icloud.folders.clear();
    f.settle();
    f.assert_folders_in_sync();
    assert_eq!(f.local_folders(), Vec::<String>::new());
    assert_eq!(
        f.files("trash/files"),
        ["x.txt"],
        "the file is in the system trash"
    );
}

#[test]
fn a_folder_deleted_on_icloud_keeps_what_linux_added() {
    let mut f = Fixture::new("folders-del-remote-new");
    f.icloud.folders.insert("Shared".into());
    f.settle();
    f.write("Shared/new.txt", b"new on linux");
    f.icloud.folders.clear();
    f.settle();
    f.assert_folders_in_sync();
    assert_eq!(
        f.icloud.content("Shared/new.txt"),
        Some(&b"new on linux"[..])
    );
}

#[test]
fn a_database_from_before_folders_only_records_them_first() {
    // Folders on one side only when folder sync starts may be ones deleted
    // on the other side before: neither created nor deleted, only the ones
    // on both sides recorded.
    let mut f = Fixture::new("folders-upgrade");
    // A database from before: files in the common state, no folders.
    f.write("keep.txt", b"synced before");
    f.icloud.put("keep.txt", b"synced before");
    let doc = f.icloud.files["keep.txt"].clone();
    let entry = ikeloud_sync::BaseEntry {
        path: ikeloud_safety::RelPath::parse("keep.txt").expect("path"),
        local_path: ikeloud_safety::RelPath::parse("keep.txt").expect("path"),
        local: None,
        doc_id: ikeloud_sync::DocId::new(doc.doc.to_string()),
        hash: ikeloud_safety::ContentHash::new(ikeloud_hash::hex(&ikeloud_hash::sha256(&[
            b"synced before",
        ]))),
        etag: ikeloud_safety::Etag::new(doc.etag.to_string()),
    };
    f.db.commit(&[ikeloud_state::BaseChange::Set(entry)], &[])
        .expect("commit");
    f.db.forget_folders_for_tests().expect("as before v4");
    fs::create_dir_all(f.root().join("left-behind")).expect("mkdir");
    f.icloud.folders.insert("deleted-here".into());
    f.icloud.folders.insert("both".into());
    fs::create_dir_all(f.root().join("both")).expect("mkdir");

    f.run();
    assert_eq!(
        f.local_folders(),
        ["both", "left-behind"],
        "nothing created"
    );
    assert!(!f.icloud.folders.contains("left-behind"));
    assert!(f.icloud.folders.contains("deleted-here"), "nothing deleted");
    assert_eq!(
        f.db.base_folders().expect("read").map(|v| v.len()),
        Some(1),
        "only both"
    );
}

/// Renames a file on the fake iCloud as iCloud does: a new version.
fn rename_on_icloud(f: &mut Fixture, from: &str, to: &str) {
    let mut file = f.icloud.files.remove(from).expect("exists");
    file.etag += 1000;
    f.icloud.files.insert(to.into(), file);
}

fn journal_kinds(f: &Fixture) -> Vec<JournalKind> {
    f.db.journal(100)
        .expect("journal")
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

#[test]
fn a_rename_on_icloud_is_not_downloaded_again() {
    // Decision 0025 step 3: iCloud gives a renamed file a new version; its
    // content checksum tells it is the same content.
    let mut f = Fixture::new("rename-icloud");
    f.write("keep.txt", b"k");
    f.write("a.txt", b"big content");
    f.settle();
    let before = journal_kinds(&f).len();
    rename_on_icloud(&mut f, "a.txt", "b.txt");
    f.settle();
    f.assert_in_sync();
    let new: Vec<JournalKind> = journal_kinds(&f)
        .into_iter()
        .take(journal_kinds(&f).len() - before)
        .collect();
    assert_eq!(new, vec![JournalKind::MovedLocal], "moved, not downloaded");
    assert!(
        f.files("history").is_empty(),
        "no old version kept: nothing replaced"
    );
}

#[test]
fn without_checksums_a_rename_on_icloud_is_downloaded_as_before() {
    let mut f = Fixture::new("rename-icloud-walk");
    f.write("keep.txt", b"k");
    f.write("a.txt", b"content");
    f.settle();
    f.icloud.no_checksums = true;
    rename_on_icloud(&mut f, "a.txt", "b.txt");
    f.settle();
    f.assert_in_sync();
    assert!(
        journal_kinds(&f).contains(&JournalKind::OverwrittenLocal)
            || journal_kinds(&f).contains(&JournalKind::Downloaded)
    );
}

#[test]
fn a_new_version_with_other_content_is_downloaded() {
    let mut f = Fixture::new("edit-icloud-checksum");
    f.write("keep.txt", b"k");
    f.write("a.txt", b"v1");
    f.settle();
    f.icloud.edit("a.txt", b"v2 from iphone");
    f.settle();
    f.assert_in_sync();
    assert_eq!(f.read("a.txt").as_deref(), Some(&b"v2 from iphone"[..]));
}
