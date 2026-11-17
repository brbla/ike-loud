//! Table tests of the planner: three states → actions.
//!
//! Every plan is also replayed on a simulated local disk (`replay_local`),
//! which fails if a local move takes a missing file or overwrites another
//! one, or a download lands on a taken path without being an overwrite.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::collections::BTreeMap;

use ikeloud_safety::{Action, DEFAULT_SETTLE_TIME, Etag, LocalId, RelPath};
use ikeloud_sync::names::key;
use ikeloud_sync::{BaseEntry, Collision, LocalFile, Plan, Side, States, build_plan};
use ikeloud_testkit::{LOCAL_SIZE, REMOTE_SIZE, STABLE, base, id, local, p, remote, version};

// --- helpers -----------------------------------------------------------------

/// Copy name following the spec's pattern, with a fixed date.
fn copy_name(path: &RelPath, n: u32) -> Option<RelPath> {
    let (stem, ext) = match path.as_str().rsplit_once('.') {
        Some((s, e)) if !s.ends_with('/') && !s.is_empty() => (s, format!(".{e}")),
        _ => (path.as_str(), String::new()),
    };
    let number = if n == 1 {
        String::new()
    } else {
        format!(" {n}")
    };
    RelPath::parse(format!("{stem} (Linux conflict 2026-11-15{number}){ext}")).ok()
}

fn copy(path: &str) -> String {
    copy_name(&p(path), 1)
        .expect("a copy name")
        .as_str()
        .to_string()
}

/// Applies the local actions of a plan to the scanned local files and checks
/// that no local action loses or overwrites a file.
fn replay_local(states: &States, plan: &Plan) {
    let mut disk: BTreeMap<String, String> = states
        .local
        .iter()
        .map(|f| (key(f.path.as_str()), f.path.as_str().to_string()))
        .collect();
    for action in &plan.actions {
        match action {
            Action::MoveLocal { from, to, .. } => {
                assert!(
                    disk.remove(&key(from.as_str())).is_some(),
                    "move of a missing local file {from} in {plan:#?}"
                );
                let to_key = key(to.as_str());
                assert!(
                    !disk.contains_key(&to_key),
                    "move {from} → {to} overwrites a local file in {plan:#?}"
                );
                disk.insert(to_key, to.as_str().to_string());
            }
            Action::Download {
                local: path,
                replaces,
                ..
            } => {
                let taken = disk.contains_key(&key(path.as_str()));
                assert_eq!(
                    taken,
                    replaces.is_some(),
                    "download to {path}: taken={taken} but replaces={replaces:?} in {plan:#?}"
                );
                disk.insert(key(path.as_str()), path.as_str().to_string());
            }
            Action::TrashLocal { path, .. } => {
                assert!(
                    disk.remove(&key(path.as_str())).is_some(),
                    "trash of a missing local file {path}"
                );
            }
            _ => {}
        }
    }
}

fn plan(states: &States) -> Plan {
    plan_with(states, &copy_name)
}

fn plan_with(states: &States, name: &dyn Fn(&RelPath, u32) -> Option<RelPath>) -> Plan {
    let result = build_plan(states, DEFAULT_SETTLE_TIME, name);
    replay_local(states, &result);
    result
}

/// Upload of local file number `n`.
fn upload(path: &str, n: u64, etag: Option<&str>) -> Action {
    upload_from(path, path, n, etag)
}

fn upload_from(local: &str, remote: &str, n: u64, etag: Option<&str>) -> Action {
    Action::Upload {
        local: p(local),
        remote: p(remote),
        local_id: id(n),
        expected_etag: etag.map(Etag::new),
        size: LOCAL_SIZE,
        stable_for: STABLE,
    }
}

/// Download to a free local path.
fn download(path: &str, etag: &str) -> Action {
    Action::Download {
        remote: p(path),
        expected_etag: Etag::new(etag),
        local: p(path),
        size: REMOTE_SIZE,
        replaces: None,
    }
}

/// Download over local file number `n` with content `hash`.
fn download_over(path: &str, n: u64, hash: &str, etag: &str) -> Action {
    Action::Download {
        remote: p(path),
        expected_etag: Etag::new(etag),
        local: p(path),
        size: REMOTE_SIZE,
        replaces: Some(version(n, hash)),
    }
}

fn trash_local(path: &str, n: u64, hash: &str) -> Action {
    Action::TrashLocal {
        path: p(path),
        expected: version(n, hash),
    }
}

fn trash_remote(path: &str, etag: &str) -> Action {
    Action::TrashRemote {
        path: p(path),
        expected_etag: Etag::new(etag),
    }
}

fn move_local(from: &str, to: &str, n: u64) -> Action {
    Action::MoveLocal {
        from: p(from),
        to: p(to),
        local_id: id(n),
    }
}

fn conflict_copy(path: &str, n: u64) -> Action {
    Action::UploadConflictCopy {
        path: p(path),
        local_id: id(n),
        size: LOCAL_SIZE,
        stable_for: STABLE,
    }
}

fn move_remote(from: &str, to: &str, etag: &str) -> Action {
    Action::MoveRemote {
        from: p(from),
        to: p(to),
        expected_etag: Etag::new(etag),
    }
}

/// Local file number `n` kept as a conflict copy (and the server version
/// downloaded in its place).
/// `download_too`: the server version (its etag) downloaded in its place.
fn conflict(path: &str, n: u64, download_too: Option<&str>) -> Vec<Action> {
    let c = copy(path);
    let mut a = vec![move_local(path, &c, n), conflict_copy(&c, n)];
    if let Some(etag) = download_too {
        a.push(download(path, etag));
    }
    a
}

// --- decision table (content) -----------------------------------------------------

#[test]
fn decision_table() {
    // Common state: file "a", inode 1, document d1, content h1, etag e1.
    // l/r: None = no file on that side; h1/e1 = unchanged; h2/e2 = changed.
    type Case = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        Vec<Action>,
    );
    let cases: Vec<Case> = vec![
        ("nothing changed", Some("h1"), Some("e1"), true, vec![]),
        (
            "changed only on Linux",
            Some("h2"),
            Some("e1"),
            true,
            vec![upload("a", 1, Some("e1"))],
        ),
        (
            "changed only on iCloud",
            Some("h1"),
            Some("e2"),
            true,
            vec![download_over("a", 1, "h1", "e2")],
        ),
        (
            "changed on both",
            Some("h2"),
            Some("e2"),
            true,
            conflict("a", 1, Some("e2")),
        ),
        (
            "deleted on Linux",
            None,
            Some("e1"),
            true,
            vec![trash_remote("a", "e1")],
        ),
        (
            "deleted on iCloud",
            Some("h1"),
            None,
            true,
            vec![trash_local("a", 1, "h1")],
        ),
        ("deleted on both", None, None, true, vec![]),
        (
            "deleted on Linux, changed on iCloud",
            None,
            Some("e2"),
            true,
            vec![download("a", "e2")],
        ),
        (
            "changed on Linux, deleted on iCloud",
            Some("h2"),
            None,
            true,
            conflict("a", 1, None),
        ),
        // No common state (the file is new).
        (
            "new only on Linux",
            Some("h1"),
            None,
            false,
            vec![upload("a", 1, None)],
        ),
        (
            "new only on iCloud",
            None,
            Some("e1"),
            false,
            vec![download("a", "e1")],
        ),
        (
            "new on both",
            Some("h1"),
            Some("e1"),
            false,
            conflict("a", 1, Some("e1")),
        ),
    ];
    for (name, l, r, with_base, expected) in cases {
        let states = States {
            local: l.map(|h| local("a", 1, h)).into_iter().collect(),
            remote: r.map(|e| remote("a", "d1", e)).into_iter().collect(),
            base: if with_base {
                vec![base("a", 1, "d1", "h1", "e1")]
            } else {
                vec![]
            },
            ..States::default()
        };
        assert_eq!(plan(&states).actions, expected, "{name}");
    }
}

#[test]
fn deleted_on_both_needs_no_action() {
    let states = States {
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    assert_eq!(plan(&states).actions, vec![]);
}

#[test]
fn modification_time_does_not_decide() {
    // Unchanged content counts as unchanged however recently it was touched
    // (the file is left alone only while it is being written).
    let mut l = local("a", 1, "h1");
    l.stable_for = DEFAULT_SETTLE_TIME;
    let states = States {
        local: vec![l],
        remote: vec![remote("a", "d1", "e1")],
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    assert_eq!(plan(&states).actions, vec![]);
}

// --- editors that save by "write new file + rename" ------------------------------

#[test]
fn saving_with_a_new_inode_is_not_a_new_file() {
    let cases: Vec<(&str, &str, Vec<Action>)> = vec![
        (
            "new inode and new content: an ordinary upload",
            "h2",
            vec![upload("report.odt", 77, Some("e1"))],
        ),
        ("new inode, same content: nothing to do", "h1", vec![]),
    ];
    for (name, hash, actions) in cases {
        let states = States {
            local: vec![local("report.odt", 77, hash)],
            remote: vec![remote("report.odt", "d1", "e1")],
            base: vec![base("report.odt", 1, "d1", "h1", "e1")],
            ..States::default()
        };
        let result = plan(&states);
        assert_eq!(result.actions, actions, "{name}");
    }
}

#[test]
fn a_known_inode_on_another_path_is_not_taken_for_a_resave() {
    // Inode 2 belongs to "b" (moved onto a's path after a was deleted); it must
    // stay paired with "b", not with "a".
    let states = States {
        local: vec![local("a", 2, "hb")],
        remote: vec![remote("a", "d1", "e1"), remote("b", "d2", "e2")],
        base: vec![
            base("a", 1, "d1", "ha", "e1"),
            base("b", 2, "d2", "hb", "e2"),
        ],
        ..States::default()
    };
    let result = plan(&states);
    assert!(
        result.actions.contains(&trash_remote("a", "e1")),
        "a was deleted locally: {:#?}",
        result.actions
    );
}

// --- identity: reused inodes --------------------------------------------------------

/// Local file `inode` on device 1 with the given birth time.
fn reborn(path: &str, inode: u64, birth_ns: Option<i64>, hash: &str) -> LocalFile {
    LocalFile {
        id: LocalId {
            device: 1,
            inode,
            birth_ns,
        },
        ..local(path, inode, hash)
    }
}

fn entry_born(path: &str, birth_ns: Option<i64>, hash: &str) -> BaseEntry {
    BaseEntry {
        local: Some(LocalId {
            device: 1,
            inode: 1,
            birth_ns,
        }),
        ..base(path, 1, "d1", hash, "e1")
    }
}

#[test]
fn a_reused_inode_is_not_the_old_file() {
    // "a" (inode 1) was deleted; a new, unrelated file got inode 1. Pairing by
    // inode alone would upload its content over document d1. Found by the
    // independent review.
    let upload_of = |l: &LocalFile, etag: Option<&str>| Action::Upload {
        local: l.path.clone(),
        remote: l.path.clone(),
        local_id: l.id,
        expected_etag: etag.map(Etag::new),
        size: LOCAL_SIZE,
        stable_for: STABLE,
    };
    let gone_and_new = |x: &LocalFile| vec![trash_remote("a", "e1"), upload_of(x, None)];
    let new_x = reborn("x", 1, Some(2), "hx");
    let new_x_no_birth = reborn("x", 1, None, "hx");
    let edited_a = reborn("a", 1, None, "hx");
    // (name, base entry birth, scanned file, expected)
    let cases: Vec<(&str, Option<i64>, LocalFile, Vec<Action>)> = vec![
        (
            "birth times differ: a new file",
            Some(1),
            new_x.clone(),
            gone_and_new(&new_x),
        ),
        (
            "birth times equal: the same file, moved and edited (content first)",
            Some(1),
            reborn("x", 1, Some(1), "hx"),
            vec![upload_from("x", "a", 1, Some("e1"))],
        ),
        (
            "no birth time, other path and content: taken for a new file",
            None,
            new_x_no_birth.clone(),
            gone_and_new(&new_x_no_birth),
        ),
        (
            "no birth time, other path, same content: a move",
            None,
            reborn("x", 1, None, "h1"),
            vec![move_remote("a", "x", "e1")],
        ),
        (
            "no birth time, same path, other content: an edit",
            None,
            edited_a.clone(),
            vec![upload_of(&edited_a, Some("e1"))],
        ),
    ];
    for (name, birth, scanned, expected) in cases {
        let states = States {
            local: vec![scanned],
            remote: vec![remote("a", "d1", "e1")],
            base: vec![entry_born("a", birth, "h1")],
            ..States::default()
        };
        assert_eq!(plan(&states).actions, expected, "{name}");
    }
}

// --- files left alone -----------------------------------------------------------------

#[test]
fn files_being_written_are_left_alone_on_both_sides() {
    let busy = |path: &str, n: u64, hash: &str| LocalFile {
        stable_for: DEFAULT_SETTLE_TIME / 2,
        ..local(path, n, hash)
    };
    let cases: Vec<(&str, States)> = vec![
        (
            "a known file edited on both sides: no conflict copy yet",
            States {
                local: vec![busy("a", 1, "h2")],
                remote: vec![remote("a", "d1", "e2")],
                base: vec![base("a", 1, "d1", "h1", "e1")],
                ..States::default()
            },
        ),
        (
            "changed only on iCloud: not downloaded over the file being written",
            States {
                local: vec![busy("a", 1, "h1")],
                remote: vec![remote("a", "d1", "e2")],
                base: vec![base("a", 1, "d1", "h1", "e1")],
                ..States::default()
            },
        ),
        (
            "a new file with a new server file of the same name: no eviction",
            States {
                local: vec![busy("a", 1, "h1")],
                remote: vec![remote("a", "d1", "e1")],
                ..States::default()
            },
        ),
        (
            "a new file: not uploaded yet",
            States {
                local: vec![busy("a", 1, "h1")],
                ..States::default()
            },
        ),
    ];
    for (name, states) in cases {
        let result = plan(&states);
        assert_eq!(result.actions, vec![], "{name}");
        assert_eq!(result.busy, vec![p("a")], "{name}");
    }
}

#[test]
fn renaming_a_known_file_to_an_ignored_name_does_not_delete_it() {
    // "report.odt" renamed locally to "report.odt.part" (never synced). The
    // server copy must stay; found by the independent review.
    for ignored in ["report.odt.part", ".report.odt.swp", ".~lock.report.odt#"] {
        let states = States {
            local: vec![local(ignored, 1, "h1")],
            remote: vec![remote("report.odt", "d1", "e1")],
            base: vec![base("report.odt", 1, "d1", "h1", "e1")],
            ..States::default()
        };
        assert_eq!(plan(&states).actions, vec![], "{ignored}");
    }
}

// --- conflict copy names from a translation ----------------------------------------------

#[test]
fn a_translation_without_the_number_cannot_hang_the_planner() {
    // Two conflicts, but the translated name ignores the attempt number:
    // the second file gets no free name and is left alone (reported).
    let states = States {
        local: vec![local("a", 1, "h2"), local("b", 2, "h2")],
        remote: vec![remote("a", "d1", "e2"), remote("b", "d2", "e2")],
        base: vec![
            base("a", 1, "d1", "h1", "e1"),
            base("b", 2, "d2", "h1", "e1"),
        ],
        ..States::default()
    };
    let constant = |_: &RelPath, _: u32| Some(p("copy"));
    let result = plan_with(&states, &constant);
    assert_eq!(
        result.actions,
        vec![
            move_local("a", "copy", 1),
            conflict_copy("copy", 1),
            download("a", "e2")
        ]
    );
    assert_eq!(result.no_copy_name, vec![p("b")]);
}

#[test]
fn copy_names_that_would_not_sync_are_refused() {
    let states = States {
        local: vec![local("d/a", 1, "h2")],
        remote: vec![remote("d/a", "d1", "e2")],
        base: vec![base("d/a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    type Naming = dyn Fn(&RelPath, u32) -> Option<RelPath>;
    let cases: Vec<(&str, &Naming)> = vec![
        ("an ignored name", &|_, n| Some(p(&format!("d/a {n}.part")))),
        ("another folder", &|_, n| Some(p(&format!("e/a {n}")))),
        ("no usable name at all", &|_, _| None),
    ];
    for (name, naming) in cases {
        let result = plan_with(&states, naming);
        assert_eq!(result.actions, vec![], "{name}");
        assert_eq!(result.no_copy_name, vec![p("d/a")], "{name}");
    }
}

// --- moves ---------------------------------------------------------------------------

#[test]
fn moves() {
    // (name, local path, remote path, expected actions)
    type Case = (&'static str, &'static str, &'static str, Vec<Action>);
    let cases: Vec<Case> = vec![
        ("not moved", "a", "a", vec![]),
        (
            "moved only on Linux",
            "dir/b",
            "a",
            vec![move_remote("a", "dir/b", "e1")],
        ),
        (
            "moved only on iCloud",
            "a",
            "dir/b",
            vec![move_local("a", "dir/b", 1)],
        ),
        (
            "moved on both, differently: iCloud wins",
            "c",
            "b",
            vec![move_local("c", "b", 1)],
        ),
        ("moved on both the same way", "b", "b", vec![]),
        (
            "renamed locally to another letter case",
            "A",
            "a",
            vec![move_remote("a", "A", "e1")],
        ),
        (
            "renamed on iCloud to another letter case",
            "a",
            "A",
            vec![move_local("a", "A", 1)],
        ),
    ];
    for (name, l, r, expected) in cases {
        let states = States {
            local: vec![local(l, 1, "h1")],
            remote: vec![remote(r, "d1", "e1")],
            remote_folders: vec![p("dir")],
            base: vec![base("a", 1, "d1", "h1", "e1")],
        };
        let result = plan(&states);
        assert_eq!(result.actions, expected, "{name}");
    }
}

#[test]
fn a_move_is_never_a_delete_and_upload() {
    let states = States {
        local: vec![local("new/name.txt", 1, "h1")],
        remote: vec![remote("old.txt", "d1", "e1")],
        base: vec![base("old.txt", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    let actions = plan(&states).actions;
    assert!(
        actions.iter().all(|a| !matches!(
            a,
            Action::TrashRemote { .. } | Action::TrashLocal { .. } | Action::Upload { .. }
        )),
        "{actions:#?}"
    );
    assert!(actions.contains(&move_remote("old.txt", "new/name.txt", "e1")));
    assert!(actions.contains(&Action::CreateRemoteFolder { path: p("new") }));
}

#[test]
fn content_first_then_path() {
    // Moved and edited on Linux: this run uploads, the next one moves.
    let states = States {
        local: vec![local("b", 1, "h2")],
        remote: vec![remote("a", "d1", "e1")],
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    assert_eq!(
        plan(&states).actions,
        vec![upload_from("b", "a", 1, Some("e1"))]
    );
}

// --- path collisions ---------------------------------------------------------------------

#[test]
fn icloud_move_onto_a_local_new_file() {
    // d1 moved to "b" on iCloud; locally a new file (inode 9) sits on "b".
    let states = States {
        local: vec![local("a", 1, "h1"), local("b", 9, "x")],
        remote: vec![remote("b", "d1", "e1")],
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    assert_eq!(
        plan(&states).actions,
        vec![
            move_local("b", &copy("b"), 9),
            move_local("a", "b", 1),
            upload(&copy("b"), 9, None),
        ],
        "the local file makes room, is uploaded under its copy name, and is never moved twice"
    );
}

#[test]
fn local_move_onto_a_name_taken_on_icloud() {
    // a moved to "b" locally; on iCloud another document already is "b".
    let states = States {
        local: vec![local("b", 1, "h1"), local("c", 2, "h2")],
        remote: vec![remote("a", "d1", "e1"), remote("b", "d2", "e2")],
        base: vec![
            base("a", 1, "d1", "h1", "e1"),
            base("c", 2, "d2", "h2", "e2"),
        ],
        ..States::default()
    };
    let actions = plan(&states).actions;
    assert!(
        actions.contains(&move_local("b", &copy("b"), 1)),
        "{actions:#?}"
    );
    assert!(
        actions.contains(&move_remote("a", &copy("b"), "e1")),
        "{actions:#?}"
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::MoveRemote { to, .. } if to.as_str() == "b")),
        "nothing is pushed aside on iCloud: {actions:#?}"
    );
}

#[test]
fn new_local_file_onto_a_name_taken_on_icloud() {
    // A new local "B" while iCloud has a known file moved to "b".
    let states = States {
        local: vec![local("a", 1, "h1"), local("B", 9, "x")],
        remote: vec![remote("b", "d1", "e1")],
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    let actions = plan(&states).actions;
    assert!(
        actions.contains(&move_local("B", &copy("B"), 9)),
        "{actions:#?}"
    );
    assert!(actions.contains(&move_local("a", "b", 1)), "{actions:#?}");
    assert!(
        actions.contains(&upload(&copy("B"), 9, None)),
        "{actions:#?}"
    );
}

#[test]
fn swap_on_icloud() {
    // a ↔ b swapped on iCloud. One run makes room and moves; the replay checks
    // nothing is overwritten. The next run finishes.
    let states = States {
        local: vec![local("a", 1, "h1"), local("b", 2, "h2")],
        remote: vec![remote("b", "d1", "e1"), remote("a", "d2", "e2")],
        base: vec![
            base("a", 1, "d1", "h1", "e1"),
            base("b", 2, "d2", "h2", "e2"),
        ],
        ..States::default()
    };
    let actions = plan(&states).actions;
    assert!(!actions.is_empty());
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::TrashLocal { .. } | Action::TrashRemote { .. })),
        "a swap deletes nothing: {actions:#?}"
    );
}

// --- names ---------------------------------------------------------------------------

#[test]
fn pairing_new_files_after_normalization() {
    let nfd = "Pr\u{30c}i\u{301}loha.pdf";
    let cases: Vec<(&str, States, Vec<Action>)> = vec![
        (
            "a new NFD file is uploaded as NFC",
            States {
                local: vec![local(nfd, 1, "h1")],
                ..States::default()
            },
            vec![upload_from(nfd, "Příloha.pdf", 1, None)],
        ),
        (
            "NFD locally and NFC on iCloud, both new: the same name",
            States {
                local: vec![local(nfd, 1, "h1")],
                remote: vec![remote("Příloha.pdf", "d1", "e1")],
                ..States::default()
            },
            vec![
                move_local(nfd, &copy(nfd), 1),
                conflict_copy(&copy(nfd), 1),
                download("Příloha.pdf", "e1"),
            ],
        ),
    ];
    for (name, states, expected) in cases {
        assert_eq!(plan(&states).actions, expected, "{name}");
    }
}

#[test]
fn local_collisions_are_skipped() {
    let states = States {
        local: vec![
            local("a.txt", 1, "h1"),
            local("A.txt", 2, "h2"),
            local("b.txt", 3, "h3"),
        ],
        remote: vec![remote("a.txt", "d1", "e1")],
        base: vec![base("a.txt", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    let result = plan(&states);
    assert_eq!(
        result.actions,
        vec![upload("b.txt", 3, None)],
        "colliding files are not synced"
    );
    assert_eq!(
        result.collisions,
        vec![Collision {
            side: Side::Local,
            paths: vec![p("A.txt"), p("a.txt")]
        }]
    );
}

#[test]
fn renaming_a_known_file_into_a_collision_does_not_delete_it() {
    // "d/e" (known, inode 5) renamed locally to "A" while "a" exists: a name
    // collision. Found by the simulation: the file looked deleted and went
    // to the iCloud trash.
    let states = States {
        local: vec![local("A", 5, "h5"), local("a", 6, "h6")],
        remote: vec![remote("d/e", "d5", "e5"), remote("a", "d6", "e6")],
        base: vec![
            base("d/e", 5, "d5", "h5", "e5"),
            base("a", 6, "d6", "h6", "e6"),
        ],
        ..States::default()
    };
    let result = plan(&states);
    assert_eq!(result.actions, vec![], "both files are left alone");
    assert_eq!(result.collisions.len(), 1);
}

#[test]
fn remote_collisions_are_skipped() {
    let states = States {
        remote: vec![remote("x", "d1", "e1"), remote("X", "d2", "e2")],
        ..States::default()
    };
    let result = plan(&states);
    assert_eq!(result.actions, vec![]);
    assert_eq!(
        result.collisions,
        vec![Collision {
            side: Side::Remote,
            paths: vec![p("X"), p("x")]
        }]
    );
}

#[test]
fn ignored_files_are_not_planned() {
    let states = States {
        local: vec![
            local(".~lock.report.odt#", 1, "h"),
            local("a.swp", 2, "h"),
            local("x.part", 3, "h"),
        ],
        remote: vec![remote("photos/.DS_Store", "d1", "e")],
        ..States::default()
    };
    assert_eq!(plan(&states), Plan::default());
}

// --- conflict copies -----------------------------------------------------------------

#[test]
fn conflict_copy_gets_a_free_name() {
    let taken = "report (Linux conflict 2026-11-15).odt";
    let states = States {
        local: vec![local("report.odt", 1, "h2"), local(taken, 2, "x")],
        remote: vec![remote("report.odt", "d1", "e2"), remote(taken, "d2", "y")],
        base: vec![
            base("report.odt", 1, "d1", "h1", "e1"),
            base(taken, 2, "d2", "x", "y"),
        ],
        ..States::default()
    };
    let second = "report (Linux conflict 2026-11-15 2).odt";
    assert_eq!(
        plan(&states).actions,
        vec![
            move_local("report.odt", second, 1),
            conflict_copy(second, 1),
            download("report.odt", "e2"),
        ]
    );
}

// --- folders --------------------------------------------------------------------------

#[test]
fn missing_folders_are_created_first() {
    let folder = |s: &str| Action::CreateRemoteFolder { path: p(s) };
    let cases: Vec<(&str, States, Vec<Action>)> = vec![
        (
            "new file in a new deep folder",
            States {
                local: vec![local("a/b/c.txt", 1, "h")],
                ..States::default()
            },
            vec![folder("a"), folder("a/b"), upload("a/b/c.txt", 1, None)],
        ),
        (
            "folder already on the server (as a folder)",
            States {
                local: vec![local("a/b/c.txt", 1, "h")],
                remote_folders: vec![p("a"), p("A/B")],
                ..States::default()
            },
            vec![upload("a/b/c.txt", 1, None)],
        ),
        (
            "server folder seen from a file inside it",
            States {
                local: vec![local("a/new.txt", 1, "h"), local("a/old.txt", 2, "x")],
                remote: vec![remote("a/old.txt", "d2", "e")],
                base: vec![base("a/old.txt", 2, "d2", "x", "e")],
                ..States::default()
            },
            vec![upload("a/new.txt", 1, None)],
        ),
        (
            "two files in the same new folder, folder once",
            States {
                local: vec![local("s/1", 1, "h"), local("s/2", 2, "h")],
                ..States::default()
            },
            vec![folder("s"), upload("s/1", 1, None), upload("s/2", 2, None)],
        ),
        (
            "a move into a new folder creates it",
            States {
                local: vec![local("n/a", 1, "h")],
                remote: vec![remote("a", "d1", "e")],
                base: vec![base("a", 1, "d1", "h", "e")],
                ..States::default()
            },
            vec![folder("n"), move_remote("a", "n/a", "e")],
        ),
    ];
    for (name, states, expected) in cases {
        assert_eq!(plan(&states).actions, expected, "{name}");
    }
}

// --- the whole plan passes the safety layer --------------------------------------------

#[test]
fn plans_pass_the_safety_layer() {
    use ikeloud_safety::{Context, Decision, assess};

    let cases: Vec<(&str, States)> = vec![
        (
            "a change on Linux",
            States {
                local: vec![local("a", 1, "h2"), local("b", 2, "h1")],
                remote: vec![remote("a", "d1", "e1"), remote("b", "d2", "e1")],
                base: vec![
                    base("a", 1, "d1", "h1", "e1"),
                    base("b", 2, "d2", "h1", "e1"),
                ],
                ..States::default()
            },
        ),
        (
            "a case-only rename on Linux",
            States {
                local: vec![local("Photo.jpg", 1, "h1")],
                remote: vec![remote("photo.jpg", "d1", "e1")],
                base: vec![base("photo.jpg", 1, "d1", "h1", "e1")],
                ..States::default()
            },
        ),
    ];
    for (name, states) in cases {
        let result = plan(&states);
        let mut c = Context::new(u64::try_from(states.base.len()).expect("small"), key);
        for r in &states.remote {
            c.remote_etags.insert(r.path.clone(), r.etag.clone());
        }
        let Ok(Decision::Approved(approved)) = assess(result.actions, &c) else {
            panic!("{name}: the plan should pass");
        };
        assert_eq!(approved.actions.len(), 1, "{name}");
        assert!(
            approved.deferred.is_empty(),
            "{name}: {:?}",
            approved.deferred
        );
    }
}
