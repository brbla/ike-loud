//! The check run (decision 0028): local files against iCloud's checksums,
//! after sync runs against the fake iCloud.

#![allow(clippy::expect_used, clippy::panic)] // test code

mod common;

use std::time::Duration;

use common::Fixture;
use ikeloud_daemon::{CheckLimits, CheckReport};
use ikeloud_safety::RelPath;
use ikeloud_state::JournalKind;

/// No limit on bytes or time; every file due again after `every`.
fn limits(every: Duration) -> CheckLimits {
    CheckLimits {
        every,
        bytes: u64::MAX,
        time: Duration::from_secs(3600),
    }
}

const WEEK: Duration = Duration::from_secs(7 * 24 * 3600);

fn report(
    checked: u64,
    found: &[&str],
    checkable: u64,
    current: u64,
    differing: u64,
) -> CheckReport {
    CheckReport {
        checked,
        found: found
            .iter()
            .map(|p| RelPath::parse(*p).expect("path"))
            .collect(),
        checkable,
        current,
        differing,
    }
}

fn findings(f: &Fixture) -> Vec<String> {
    f.db.journal(100)
        .expect("journal")
        .into_iter()
        .filter(|e| e.kind == JournalKind::CheckDiffers)
        .map(|e| e.path.as_str().to_string())
        .collect()
}

/// Files on both sides, synced.
fn synced(name: &str) -> Fixture {
    let mut f = Fixture::new(name);
    f.write("a.txt", b"from linux");
    f.write("docs/b.txt", b"also from linux");
    f.icloud.put("c.txt", b"from the iphone");
    f.icloud.put("photos/d.jpg", &[0xff; 70_000]);
    f.settle();
    f.assert_in_sync();
    f
}

#[test]
fn synced_files_have_icloud_s_checksum() {
    let mut f = synced("check-same");
    assert_eq!(f.check(limits(WEEK)), report(4, &[], 4, 4, 0));
    // Checked: not due again within the week.
    assert_eq!(f.check(limits(WEEK)), report(0, &[], 4, 4, 0));
    // Due again after it.
    assert_eq!(f.check(limits(Duration::ZERO)), report(4, &[], 4, 4, 0));
    assert!(findings(&f).is_empty());
}

#[test]
fn other_content_on_icloud_under_the_same_version_is_reported_once() {
    let mut f = synced("check-differs");
    f.check(limits(WEEK));
    f.icloud.corrupt("c.txt", b"not what was synced");
    let run = f.run();
    assert!(
        run.executed.done.is_empty(),
        "the sync sees no change: {run:?}"
    );
    // Not due yet: the result of a week ago stands.
    assert_eq!(f.check(limits(WEEK)), report(0, &[], 4, 4, 0));

    assert_eq!(
        f.check(limits(Duration::ZERO)),
        report(4, &["c.txt"], 4, 4, 1)
    );
    assert_eq!(findings(&f), ["c.txt"]);
    // Checked again: still differs, not reported again.
    assert_eq!(f.check(limits(Duration::ZERO)), report(4, &[], 4, 4, 1));
    assert_eq!(findings(&f), ["c.txt"]);
    // Nothing was changed on either side.
    assert_eq!(f.read("c.txt").as_deref(), Some(&b"from the iphone"[..]));
    assert_eq!(f.icloud.content("c.txt"), Some(&b"not what was synced"[..]));
}

#[test]
fn a_new_version_is_checked_at_once_and_reported_again_if_it_differs() {
    let mut f = synced("check-new-version");
    f.icloud.corrupt("c.txt", b"broken");
    // The check compares with the checksum of the last listing.
    assert_eq!(f.check(limits(Duration::ZERO)).found.len(), 0);
    f.run();
    assert_eq!(f.check(limits(Duration::ZERO)).found.len(), 1);
    // A new version from the iPhone: downloaded, checked first, the same.
    f.icloud.edit("c.txt", b"fixed on the iphone");
    f.settle();
    assert_eq!(f.check(limits(WEEK)), report(1, &[], 4, 4, 0));
    // The next broken version is a new finding.
    f.icloud.corrupt("c.txt", b"broken again");
    f.run();
    assert_eq!(
        f.check(limits(Duration::ZERO)),
        report(4, &["c.txt"], 4, 4, 1)
    );
    assert_eq!(findings(&f), ["c.txt", "c.txt"]);
}

#[test]
fn a_local_change_not_synced_yet_is_left_to_the_sync() {
    let mut f = synced("check-local-change");
    f.write("a.txt", b"edited, not uploaded yet");
    assert_eq!(f.check(limits(WEEK)), report(3, &[], 4, 3, 0));
    assert!(findings(&f).is_empty());
    f.settle();
    assert_eq!(
        f.check(limits(WEEK)),
        report(1, &[], 4, 4, 0),
        "the uploaded version"
    );
}

#[test]
fn a_part_stops_after_its_bytes() {
    let mut f = synced("check-parts");
    let one_byte = CheckLimits {
        bytes: 1,
        ..limits(WEEK)
    };
    // Each part reads one file (it may go over the limit with that file).
    let parts: Vec<u64> = (0..5).map(|_| f.check(one_byte).checked).collect();
    assert_eq!(parts, [1, 1, 1, 1, 0]);
    // The oldest result is checked first.
    let again = CheckLimits {
        every: Duration::ZERO,
        ..one_byte
    };
    let oldest = f.db.verified().expect("read");
    let first = oldest
        .values()
        .min_by_key(|v| v.time)
        .map(|v| v.doc_id.clone())
        .expect("results");
    f.check(again);
    let after = f.db.verified().expect("read");
    let rechecked: Vec<_> = after
        .iter()
        .filter(|(doc, v)| oldest.get(*doc).is_some_and(|o| o.time != v.time) || **doc == first)
        .map(|(doc, _)| doc.clone())
        .collect();
    assert!(rechecked.contains(&first), "{rechecked:?}");
}

#[test]
fn results_of_files_gone_are_forgotten() {
    let mut f = synced("check-gone");
    f.check(limits(WEEK));
    assert_eq!(f.db.verified().expect("read").len(), 4);
    f.icloud.delete("c.txt");
    f.settle();
    assert_eq!(f.check(limits(WEEK)), report(0, &[], 3, 3, 0));
    assert_eq!(f.db.verified().expect("read").len(), 3);
}

#[test]
fn without_icloud_s_checksums_nothing_can_be_checked() {
    let mut f = Fixture::new("check-no-sums");
    f.icloud.no_checksums = true;
    f.icloud.put("c.txt", b"from the iphone");
    f.settle();
    assert_eq!(f.check(limits(WEEK)), report(0, &[], 0, 0, 0));
}

#[test]
fn a_file_replaced_on_the_disk_is_skipped() {
    let mut f = synced("check-replaced");
    // Another file under the same name (another inode): the sync decides.
    f.remove("a.txt");
    f.write("a.txt", b"from linux");
    let r = f.check(limits(WEEK));
    assert_eq!((r.checked, r.checkable), (3, 4), "{r:?}");
}
