//! Table tests of plan assessment: input → expected output.
//!
//! The first five rows are the example from the spec (Testing chapter).

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::time::Duration;

use ikeloud_safety::names::NameError;
use ikeloud_safety::{
    Action, Change, ChangeKind, ContentHash, Context, Decision, DeferReason, Etag,
    LocalFolderState, LocalId, LocalVersion, Mode, ReadOnlyReason, RecentChanges, Rejection,
    RelPath, assess,
};

// --- helpers -----------------------------------------------------------------

fn p(s: &str) -> RelPath {
    RelPath::parse(s).expect("well-formed path")
}

fn e(s: &str) -> Etag {
    Etag::new(s)
}

const ID: LocalId = LocalId {
    device: 1,
    inode: 1,
    birth_ns: None,
};

fn version(hash: &str) -> LocalVersion {
    LocalVersion {
        id: ID,
        hash: ContentHash::new(hash),
    }
}

fn upload(path: &str, etag: Option<&str>) -> Action {
    upload_full(path, etag, 10, Duration::from_secs(60))
}

fn upload_full(path: &str, etag: Option<&str>, size: u64, stable_for: Duration) -> Action {
    Action::Upload {
        local: p(path),
        remote: p(path),
        local_id: ID,
        expected_etag: etag.map(e),
        size,
        stable_for,
    }
}

fn upload_stable(path: &str, stable_for: Duration) -> Action {
    upload_full(path, None, 10, stable_for)
}

fn conflict_copy(path: &str, stable_for: Duration) -> Action {
    Action::UploadConflictCopy {
        path: p(path),
        local_id: ID,
        size: 1,
        stable_for,
    }
}

fn trash_local(path: &str) -> Action {
    Action::TrashLocal {
        path: p(path),
        expected: version("h"),
    }
}

fn move_local(from: &str, to: &str) -> Action {
    Action::MoveLocal {
        from: p(from),
        to: p(to),
        local_id: ID,
    }
}

fn trash(path: &str, etag: &str) -> Action {
    Action::TrashRemote {
        path: p(path),
        expected_etag: e(etag),
    }
}

fn download(path: &str, overwrites_local: bool) -> Action {
    Action::Download {
        remote: p(path),
        expected_etag: e("e1"),
        local: p(path),
        size: 10,
        replaces: overwrites_local.then(|| version("h")),
    }
}

/// The engine's name key, for these tests without the Unicode tables:
/// lower case, and the one decomposed letter the tests use (`Ř` as R + ˇ).
fn lowercase_and_nfc(name: &str) -> String {
    name.replace("R\u{30c}", "Ř")
        .replace("r\u{30c}", "ř")
        .to_lowercase()
}

/// Context with `n` synced files `f0`…`f{n-1}` with etag `v1`.
fn context(n: u64) -> Context {
    let mut c = Context::new(n, lowercase_and_nfc);
    for i in 0..n {
        c.remote_etags.insert(p(&format!("f{i}")), e("v1"));
    }
    c
}

/// Delete the first `count` files of the context.
fn delete(count: u64) -> Vec<Action> {
    (0..count).map(|i| trash(&format!("f{i}"), "v1")).collect()
}

/// Simplified result so the tables stay readable.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    NothingToDo,
    Approved { run: usize, defer: Vec<DeferReason> },
    Rejected(Rejection),
}

fn run(plan: Vec<Action>, c: &Context) -> Outcome {
    match assess(plan, c) {
        Ok(Decision::NothingToDo) => Outcome::NothingToDo,
        Ok(Decision::Approved(p)) => Outcome::Approved {
            run: p.actions.len(),
            defer: p.deferred.into_iter().map(|d| d.reason).collect(),
        },
        Err(r) => Outcome::Rejected(r),
    }
}

fn ok(run: usize) -> Outcome {
    Outcome::Approved { run, defer: vec![] }
}

fn deferred(run: usize, reasons: Vec<DeferReason>) -> Outcome {
    Outcome::Approved {
        run,
        defer: reasons,
    }
}

fn mass_change(outcome: &Outcome) -> Option<(u64, u64)> {
    match outcome {
        Outcome::Rejected(Rejection::MassChangeNeedsConfirmation(s)) => {
            Some((s.destructive(), s.total))
        }
        _ => None,
    }
}

// --- example from the spec ------------------------------------------------------

#[test]
fn spec_example() {
    let mut missing = context(200);
    missing.local_folder = LocalFolderState::Missing;

    let cases: Vec<(&str, Vec<Action>, Context, Outcome)> = vec![
        (
            "upload 1 file, server etag matches",
            vec![upload("f0", Some("v1"))],
            context(1),
            ok(1),
        ),
        (
            "upload 1 file, server etag changed",
            vec![upload("f0", Some("v0"))],
            context(1),
            Outcome::Rejected(Rejection::Conflict(vec![p("f0")])),
        ),
        (
            "delete everything, local folder missing",
            delete(200),
            missing,
            Outcome::Rejected(Rejection::LocalFolderUnavailable),
        ),
        ("empty plan", vec![], context(5), Outcome::NothingToDo),
    ];
    for (name, plan, c, expected) in cases {
        assert_eq!(run(plan, &c), expected, "{name}");
    }

    // "delete 120 of 200": the summary lists the files, compared separately.
    let o = run(delete(120), &context(200));
    assert_eq!(mass_change(&o), Some((120, 200)), "delete 120 of 200");
}

// --- surroundings ---------------------------------------------------------------

#[test]
fn surroundings() {
    let empty_folder = |synced: u64| {
        let mut c = context(synced);
        c.local_folder = LocalFolderState::Available { file_count: 0 };
        c
    };
    let empty_listing = |synced: u64| {
        let mut c = context(synced);
        c.remote_item_count = 0;
        c
    };
    let mut missing_empty_plan = context(3);
    missing_empty_plan.local_folder = LocalFolderState::Missing;

    let cases: Vec<(&str, Vec<Action>, Context, Outcome)> = vec![
        (
            "local folder suddenly empty",
            delete(1),
            empty_folder(3),
            Outcome::Rejected(Rejection::LocalFolderUnavailable),
        ),
        (
            "empty folder on first setup is fine",
            vec![download("a", false)],
            empty_folder(0),
            ok(1),
        ),
        (
            "missing folder stops even an empty plan",
            vec![],
            missing_empty_plan,
            Outcome::Rejected(Rejection::LocalFolderUnavailable),
        ),
        (
            "server returned an empty listing",
            vec![trash_local("f0")],
            empty_listing(3),
            Outcome::Rejected(Rejection::SuspiciousRemoteListing),
        ),
        (
            "empty listing of an empty account is fine",
            vec![upload("a", None)],
            empty_listing(0),
            ok(1),
        ),
    ];
    for (name, plan, c, expected) in cases {
        assert_eq!(run(plan, &c), expected, "{name}");
    }
}

// --- version conflicts ------------------------------------------------------------

#[test]
fn conflicts() {
    let mv = |from: &str, to: &str, etag: &str| Action::MoveRemote {
        from: p(from),
        to: p(to),
        expected_etag: e(etag),
    };
    let conflict = |paths: &[&str]| {
        Outcome::Rejected(Rejection::Conflict(paths.iter().map(|s| p(s)).collect()))
    };

    let cases: Vec<(&str, Vec<Action>, Outcome)> = vec![
        ("new file on a free path", vec![upload("new", None)], ok(1)),
        (
            "new file, server already has it",
            vec![upload("f0", None)],
            conflict(&["f0"]),
        ),
        (
            "overwrite, deleted on server meanwhile",
            vec![upload("gone", Some("v1"))],
            conflict(&["gone"]),
        ),
        ("trash, etag matches", vec![trash("f0", "v1")], ok(1)),
        (
            "trash, etag changed",
            vec![trash("f0", "v2")],
            conflict(&["f0"]),
        ),
        (
            "trash, item no longer on server",
            vec![trash("gone", "v1")],
            conflict(&["gone"]),
        ),
        (
            "move, etag matches and target free",
            vec![mv("f0", "g", "v1")],
            ok(1),
        ),
        (
            "move onto an occupied target",
            vec![mv("f0", "f1", "v1")],
            conflict(&["f1"]),
        ),
        (
            "move, etag changed",
            vec![mv("f0", "g", "v9")],
            conflict(&["g"]),
        ),
        (
            "conflict copy onto an occupied name",
            vec![conflict_copy("f0", Duration::from_secs(60))],
            conflict(&["f0"]),
        ),
        (
            "folder that already exists",
            vec![Action::CreateRemoteFolder { path: p("f0") }],
            conflict(&["f0"]),
        ),
        (
            "several conflicts at once, sorted",
            vec![
                trash("f2", "x"),
                upload("f1", Some("x")),
                upload("ok", None),
            ],
            conflict(&["f1", "f2"]),
        ),
        (
            "downloads are not checked for conflicts",
            vec![download("f0", true)],
            ok(1),
        ),
    ];
    for (name, plan, expected) in cases {
        assert_eq!(run(plan, &context(3)), expected, "{name}");
    }
}

// --- mass-change brake -------------------------------------------------------------

#[test]
fn brake_thresholds() {
    // (name, deletions, synced, should brake)
    let cases: Vec<(&str, u64, u64, bool)> = vec![
        ("50 of 1000: exactly at the absolute limit", 50, 1000, false),
        ("51 of 1000: above the absolute limit", 51, 1000, true),
        ("10 of 20: 50 %, but not more than 10 files", 10, 20, false),
        ("11 of 100: 11 %", 11, 100, true),
        ("11 of 110: exactly 10 %", 11, 110, false),
        ("11 of 109: just above 10 %", 11, 109, true),
        ("11 of 111: below 10 %", 11, 111, false),
        ("1 of 1: a small folder does not brake", 1, 1, false),
    ];
    for (name, count, total, should_brake) in cases {
        let o = run(delete(count), &context(total));
        assert_eq!(mass_change(&o).is_some(), should_brake, "{name}: {o:?}");
    }
}

#[test]
fn brake_what_counts() {
    let moves: Vec<Action> = (0..100)
        .map(|i| move_local(&format!("f{i}"), &format!("g{i}")))
        .collect();
    let overwrites: Vec<Action> = (0..60)
        .map(|i| upload(&format!("f{i}"), Some("v1")))
        .collect();
    let new_files: Vec<Action> = (0..60).map(|i| upload(&format!("n{i}"), None)).collect();
    let download_over: Vec<Action> = (0..60).map(|i| download(&format!("f{i}"), true)).collect();
    let download_new: Vec<Action> = (0..60).map(|i| download(&format!("n{i}"), false)).collect();
    let local_trash: Vec<Action> = (0..60).map(|i| trash_local(&format!("f{i}"))).collect();

    let cases: Vec<(&str, Vec<Action>, bool)> = vec![
        ("100 moves do not count as deletions", moves, false),
        ("overwrites on the server count", overwrites, true),
        ("new files do not count", new_files, false),
        ("downloads over local files count", download_over, true),
        ("downloads of new files do not count", download_new, false),
        ("local trash counts", local_trash, true),
    ];
    for (name, plan, should_brake) in cases {
        let o = run(plan, &context(200));
        assert_eq!(mass_change(&o).is_some(), should_brake, "{name}: {o:?}");
    }
}

#[test]
fn brake_confirmation_covers_exactly_the_listed_files() {
    let c = context(200);
    let overwrite_all = |stable_secs: u64, size: u64| -> Vec<Action> {
        (0..120)
            .map(|i| {
                upload_full(
                    &format!("f{i}"),
                    Some("v1"),
                    size,
                    Duration::from_secs(stable_secs),
                )
            })
            .collect()
    };
    let Err(Rejection::MassChangeNeedsConfirmation(summary)) = assess(overwrite_all(60, 10), &c)
    else {
        panic!("expected the brake");
    };
    assert_eq!(summary.destructive(), 120);
    assert_eq!(
        summary.files.first(),
        Some(&Change {
            kind: ChangeKind::OverwriteRemote,
            path: p("f0"),
            to: None,
            version: "v1".into(),
        }),
        "the user sees which files and what happens to them"
    );

    let mut confirmed = c.clone();
    confirmed.confirmation = Some(summary.confirm());
    let cases: Vec<(&str, Vec<Action>, bool)> = vec![
        ("the same plan", overwrite_all(60, 10), false),
        (
            "after a rescan: the files are older and bigger, nothing else changed",
            overwrite_all(120, 99),
            false,
        ),
        ("only some of the confirmed files", delete(0), false),
        (
            "a file the user did not see",
            {
                let mut plan = overwrite_all(60, 10);
                plan.push(trash("f150", "v1"));
                plan
            },
            true,
        ),
        (
            "another action on a confirmed file (trash instead of overwrite)",
            {
                let mut plan = overwrite_all(60, 10);
                plan.push(trash("f0", "v1"));
                plan
            },
            true,
        ),
    ];
    for (name, plan, should_brake) in cases {
        let o = run(plan, &confirmed);
        assert_eq!(mass_change(&o).is_some(), should_brake, "{name}: {o:?}");
    }

    let mut changed_version = c.clone();
    changed_version.confirmation = Some(summary.confirm());
    for i in 0..120 {
        changed_version
            .remote_etags
            .insert(p(&format!("f{i}")), e("v2"));
    }
    let plan: Vec<Action> = (0..120)
        .map(|i| upload(&format!("f{i}"), Some("v2")))
        .collect();
    assert!(
        mass_change(&run(plan, &changed_version)).is_some(),
        "the files changed on the server since the user confirmed"
    );
}

#[test]
fn brake_counts_over_the_window() {
    // (name, deletions earlier in the window, the plan, should brake)
    let new_files: Vec<Action> = (0..5).map(|i| upload(&format!("n{i}"), None)).collect();
    let cases: Vec<(&str, u64, Vec<Action>, bool)> = vec![
        ("45 earlier + 5 now: at the limit", 45, delete(5), false),
        (
            "45 earlier + 6 now: a slow deletion is caught",
            45,
            delete(6),
            true,
        ),
        (
            "60 earlier, this plan deletes nothing: no question",
            60,
            new_files,
            false,
        ),
        (
            "60 earlier, one more: asks (the user confirms, and the daemon \
             counts the window from the confirmation)",
            60,
            delete(1),
            true,
        ),
    ];
    for (name, earlier, plan, should_brake) in cases {
        let mut c = context(1000);
        c.recent = RecentChanges {
            destructive: earlier,
            moves: 0,
        };
        let o = run(plan, &c);
        assert_eq!(mass_change(&o).is_some(), should_brake, "{name}: {o:?}");
    }
}

#[test]
fn brake_counts_moves_apart() {
    let moves = |n: u64| -> Vec<Action> {
        (0..n)
            .map(|i| move_local(&format!("f{i}"), &format!("g{i}")))
            .collect()
    };
    let summary = |o: &Outcome| match o {
        Outcome::Rejected(Rejection::MassChangeNeedsConfirmation(s)) => {
            Some((s.destructive(), s.moves()))
        }
        _ => None,
    };
    // (name, synced files, moves earlier, plan, expected (deletions, moves))
    type Case = (&'static str, u64, u64, Vec<Action>, Option<(u64, u64)>);
    let cases: Vec<Case> = vec![
        ("renaming a folder of 500 files", 2000, 0, moves(500), None),
        ("501 moves at once", 2000, 0, moves(501), Some((0, 501))),
        (
            "more than half of the files moved within a day",
            200,
            90,
            moves(11),
            Some((0, 101)),
        ),
        (
            "moves do not add up with deletions",
            2000,
            0,
            {
                let mut plan = moves(400);
                plan.extend(delete(40));
                plan
            },
            None,
        ),
    ];
    for (name, synced, earlier, plan, expected) in cases {
        let mut c = context(synced);
        c.recent.moves = earlier;
        assert_eq!(summary(&run(plan, &c)), expected, "{name}");
    }
}

#[test]
fn brake_ignores_deferred_actions() {
    let mut c = context(200);
    c.mode = Mode::ReadOnly(ReadOnlyReason::Settings);
    assert_eq!(
        run(delete(120), &c),
        deferred(
            0,
            vec![DeferReason::ReadOnly(ReadOnlyReason::Settings); 120]
        )
    );
}

// --- deferred actions -----------------------------------------------------------

#[test]
fn read_only_mode() {
    for reason in [
        ReadOnlyReason::Settings,
        ReadOnlyReason::FirstDownload,
        ReadOnlyReason::UnknownApiResponse,
        ReadOnlyReason::RemoteKillSwitch,
    ] {
        let mut c = context(3);
        c.mode = Mode::ReadOnly(reason);
        let plan = vec![
            upload("f0", Some("v1")),
            trash("f1", "v1"),
            Action::CreateRemoteFolder { path: p("s") },
            download("f2", true),
            move_local("f2", "x"),
        ];
        assert_eq!(
            run(plan, &c),
            deferred(2, vec![DeferReason::ReadOnly(reason); 3]),
            "{reason:?}: server writes wait, local actions run"
        );
    }
}

#[test]
fn read_only_mode_reports_no_conflict() {
    let mut c = context(1);
    c.mode = Mode::ReadOnly(ReadOnlyReason::Settings);
    assert_eq!(
        run(vec![upload("f0", Some("old"))], &c),
        deferred(0, vec![DeferReason::ReadOnly(ReadOnlyReason::Settings)])
    );
}

#[test]
fn settle_time() {
    let cases: Vec<(&str, Duration, Outcome)> = vec![
        (
            "being written right now",
            Duration::ZERO,
            deferred(0, vec![DeferReason::StillBeingWritten]),
        ),
        (
            "4.999 s",
            Duration::from_millis(4999),
            deferred(0, vec![DeferReason::StillBeingWritten]),
        ),
        ("exactly 5 s", Duration::from_secs(5), ok(1)),
        ("unchanged for an hour", Duration::from_secs(3600), ok(1)),
    ];
    for (name, stable_for, expected) in cases {
        assert_eq!(
            run(vec![upload_stable("a", stable_for)], &context(0)),
            expected,
            "upload, {name}"
        );
        assert_eq!(
            run(vec![conflict_copy("a", stable_for)], &context(0)),
            expected,
            "conflict copy, {name}"
        );
    }
}

#[test]
fn ignored_files() {
    let ign = || deferred(0, vec![DeferReason::Ignored]);
    let cases: Vec<(&str, Action, Outcome)> = vec![
        (
            "LibreOffice lock",
            upload(".~lock.report.odt#", None),
            ign(),
        ),
        (
            "lock inside a folder",
            upload("doc/.~lock.a.ods#", None),
            ign(),
        ),
        ("vim swap", upload(".report.txt.swp", None), ign()),
        ("unfinished download", upload("film.mkv.part", None), ign()),
        ("Mac metadata file", upload(".DS_Store", None), ign()),
        (
            "Mac metadata file from the server",
            download("photos/.DS_Store", false),
            ign(),
        ),
        (
            "similar ordinary name",
            upload(".~lock.no-hash", None),
            ok(1),
        ),
        (
            "swp in the middle of the name",
            upload("a.swp.txt", None),
            ok(1),
        ),
        ("DS_Store without the dot", upload("DS_Store", None), ok(1)),
    ];
    for (name, action, expected) in cases {
        assert_eq!(run(vec![action], &context(0)), expected, "{name}");
    }
}

#[test]
fn invalid_names() {
    let err = |n| deferred(0, vec![DeferReason::InvalidName(n)]);
    let too_long = "a".repeat(256);
    let exactly_255 = "a".repeat(255);
    // 128 two-byte characters = 256 bytes
    let too_long_czech = "č".repeat(128);
    let cases: Vec<(&str, String, Outcome)> = vec![
        (
            "ordinary name with diacritics",
            "Příloha – smlouva.pdf".into(),
            ok(1),
        ),
        (
            "newline",
            "a\nb".into(),
            err(NameError::ForbiddenChar('\n')),
        ),
        ("tab", "a\tb".into(), err(NameError::ForbiddenChar('\t'))),
        ("255 bytes", exactly_255, ok(1)),
        ("256 bytes", too_long, err(NameError::TooLong)),
        (
            "256 bytes in Czech",
            too_long_czech,
            err(NameError::TooLong),
        ),
    ];
    for (name, path, expected) in cases {
        assert_eq!(
            run(vec![upload(&path, None)], &context(0)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn case_collisions() {
    let mut c = context(0);
    c.remote_etags.insert(p("Photo.jpg"), e("v1"));
    c.remote_etags.insert(p("Documents/Příloha.pdf"), e("v1"));
    // Written by a Mac: decomposed (NFD).
    c.remote_etags.insert(p("Mac/PR\u{30c}ÍLOHA.txt"), e("v1"));
    c.synced_count = 3;
    c.remote_item_count = 3;
    c.local_folder = LocalFolderState::Available { file_count: 3 };

    let collision = |s: &str| deferred(0, vec![DeferReason::CaseCollision { with: p(s) }]);
    let cases: Vec<(&str, Vec<Action>, Outcome)> = vec![
        (
            "different case than on the server",
            vec![upload("photo.jpg", None)],
            collision("Photo.jpg"),
        ),
        (
            "Czech letters",
            vec![upload("Documents/PŘÍLOHA.pdf", None)],
            collision("Documents/Příloha.pdf"),
        ),
        (
            "NFD on the server, NFC and another case in the plan",
            vec![upload("Mac/příloha.txt", None)],
            collision("Mac/PR\u{30c}ÍLOHA.txt"),
        ),
        (
            "two new names in the plan",
            vec![upload("a.txt", None), upload("A.txt", None)],
            deferred(1, vec![DeferReason::CaseCollision { with: p("a.txt") }]),
        ),
        (
            "moving another file onto a colliding name",
            vec![Action::MoveRemote {
                from: p("Documents/Příloha.pdf"),
                to: p("PHOTO.JPG"),
                expected_etag: e("v1"),
            }],
            collision("Photo.jpg"),
        ),
        (
            "renaming a file to another letter case of its own name",
            vec![Action::MoveRemote {
                from: p("Photo.jpg"),
                to: p("PHOTO.jpg"),
                expected_etag: e("v1"),
            }],
            ok(1),
        ),
        (
            "same name in another folder",
            vec![upload("other/photo.jpg", None)],
            ok(1),
        ),
        (
            "overwriting the existing file",
            vec![upload("Photo.jpg", Some("v1"))],
            ok(1),
        ),
    ];
    for (name, plan, expected) in cases {
        assert_eq!(run(plan, &c), expected, "{name}");
    }
}

// --- free space ---------------------------------------------------------------------

#[test]
fn free_space() {
    let with_space = |icloud: Option<u64>, disk: Option<u64>| {
        let mut c = context(3);
        c.icloud_free = icloud;
        c.disk_free = disk;
        c
    };
    // every helper action has 10 bytes
    let plan = || vec![upload("a", None), upload("b", None), download("f0", true)];
    let cases: Vec<(&str, Context, Outcome)> = vec![
        ("space unknown", with_space(None, None), ok(3)),
        ("exactly enough", with_space(Some(20), Some(10)), ok(3)),
        (
            "too little on iCloud",
            with_space(Some(19), Some(100)),
            Outcome::Rejected(Rejection::NotEnoughICloudSpace {
                needed: 20,
                free: 19,
            }),
        ),
        (
            "too little on disk",
            with_space(Some(100), Some(9)),
            Outcome::Rejected(Rejection::NotEnoughDiskSpace {
                needed: 10,
                free: 9,
            }),
        ),
        (
            "iCloud full",
            with_space(Some(0), None),
            Outcome::Rejected(Rejection::NotEnoughICloudSpace {
                needed: 20,
                free: 0,
            }),
        ),
    ];
    for (name, c, expected) in cases {
        assert_eq!(run(plan(), &c), expected, "{name}");
    }
}

#[test]
fn huge_files_do_not_overflow() {
    let mut c = context(0);
    c.icloud_free = Some(u64::MAX - 1);
    let huge = |path: &str| upload_full(path, None, u64::MAX, Duration::from_secs(60));
    assert_eq!(
        run(vec![huge("a"), huge("b")], &c),
        Outcome::Rejected(Rejection::NotEnoughICloudSpace {
            needed: u64::MAX,
            free: u64::MAX - 1
        })
    );
}

// --- approved action ------------------------------------------------------------------

#[test]
fn approved_action_carries_the_original_action() {
    let action = upload("f0", Some("v1"));
    let Ok(Decision::Approved(plan)) = assess(vec![action.clone()], &context(1)) else {
        panic!("expected approval");
    };
    let [approved] = plan.actions.as_slice() else {
        panic!("expected one action");
    };
    assert_eq!(approved.action(), &action);
}

#[test]
fn a_plan_is_confirmed_only_if_the_confirmation_covers_it() {
    let c = context(200);
    let Err(Rejection::MassChangeNeedsConfirmation(summary)) = assess(delete(60), &c) else {
        panic!("expected the brake");
    };
    let mut with = c.clone();
    with.confirmation = Some(summary.confirm());
    let confirmed = |plan: Vec<Action>, c: &Context| match assess(plan, c) {
        Ok(Decision::Approved(p)) => p.confirmed,
        other => panic!("approved: {other:?}"),
    };
    // (name, plan, context, confirmed)
    let cases: Vec<(&str, Vec<Action>, &Context, bool)> = vec![
        ("the confirmed plan", delete(60), &with, true),
        (
            "a part of it (the rest after an outage)",
            delete(3),
            &with,
            true,
        ),
        (
            "a small plan the confirmation does not cover",
            vec![trash("f150", "v1")],
            &with,
            false,
        ),
        (
            "a covered deletion and a file the user did not see",
            vec![trash("f0", "v1"), trash("f150", "v1")],
            &with,
            false,
        ),
        (
            "no change to confirm (uploads only)",
            vec![upload("n", None)],
            &with,
            false,
        ),
        ("no confirmation", delete(3), &c, false),
    ];
    for (name, plan, context, expected) in cases {
        assert_eq!(confirmed(plan, context), expected, "{name}");
    }
}

// --- folders (decision 0027) ----------------------------------------------------

fn trash_folder(path: &str, etag: &str) -> Action {
    Action::TrashRemoteFolder {
        path: p(path),
        expected_etag: e(etag),
    }
}

#[test]
fn folders_count_for_the_brake_and_their_versions_are_checked() {
    let mut c = context(200);
    for i in 0..60 {
        c.remote_folder_etags.insert(p(&format!("d{i}")), e("f1"));
    }
    let removals: Vec<Action> = (0..60)
        .map(|i| Action::RemoveLocalFolder {
            path: p(&format!("d{i}")),
        })
        .collect();
    let trashes: Vec<Action> = (0..60)
        .map(|i| trash_folder(&format!("d{i}"), "f1"))
        .collect();
    let creations: Vec<Action> = (0..60)
        .map(|i| Action::CreateLocalFolder {
            path: p(&format!("n{i}")),
        })
        .collect();
    assert!(
        mass_change(&run(removals, &c)).is_some(),
        "local removals count"
    );
    assert!(
        mass_change(&run(trashes, &c)).is_some(),
        "iCloud trash counts"
    );
    assert_eq!(run(creations, &c), ok(60), "creations do not");

    assert_eq!(run(vec![trash_folder("d1", "f1")], &c), ok(1));
    assert_eq!(
        run(vec![trash_folder("d1", "changed")], &c),
        Outcome::Rejected(Rejection::Conflict(vec![p("d1")])),
        "another version on iCloud"
    );
    assert_eq!(
        run(vec![trash_folder("gone", "f1")], &c),
        Outcome::Rejected(Rejection::Conflict(vec![p("gone")])),
        "not on iCloud"
    );
}

#[test]
fn read_only_defers_the_icloud_trash_but_not_local_folders() {
    let mut c = context(10);
    c.remote_folder_etags.insert(p("d"), e("f1"));
    c.mode = Mode::ReadOnly(ReadOnlyReason::Settings);
    let plan = vec![
        trash_folder("d", "f1"),
        Action::CreateLocalFolder { path: p("n") },
        Action::RemoveLocalFolder { path: p("old") },
    ];
    assert_eq!(
        run(plan, &c),
        deferred(2, vec![DeferReason::ReadOnly(ReadOnlyReason::Settings)])
    );
}
