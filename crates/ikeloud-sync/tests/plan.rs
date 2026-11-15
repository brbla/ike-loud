//! Table tests of the planner: three states → actions.
//!
//! Every plan is also replayed on a simulated local disk (`replay_local`),
//! which fails if a local move takes a missing file or overwrites another
//! one, or a download lands on a taken path without being an overwrite.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::collections::BTreeMap;
use std::time::Duration;

use ikeloud_safety::{Action, Etag, RelPath};
use ikeloud_sync::names::key;
use ikeloud_sync::{
    BaseEntry, Collision, ContentHash, DocId, LocalFile, Plan, RemoteFile, Side, States, build_plan,
};

// --- helpers -----------------------------------------------------------------

fn p(s: &str) -> RelPath {
    RelPath::new(s)
}

fn local(path: &str, inode: u64, hash: &str) -> LocalFile {
    LocalFile {
        path: p(path),
        inode,
        hash: ContentHash::new(hash),
        size: 7,
        stable_for: Duration::from_secs(60),
    }
}

fn remote(path: &str, doc: &str, etag: &str) -> RemoteFile {
    RemoteFile {
        path: p(path),
        doc_id: DocId::new(doc),
        etag: Etag::new(etag),
        size: 9,
    }
}

fn base(path: &str, inode: u64, doc: &str, hash: &str, etag: &str) -> BaseEntry {
    BaseEntry {
        path: p(path),
        inode,
        doc_id: DocId::new(doc),
        hash: ContentHash::new(hash),
        etag: Etag::new(etag),
    }
}

/// Copy name following the spec's pattern, with a fixed date.
fn copy_name(path: &RelPath, n: u32) -> RelPath {
    let (stem, ext) = match path.as_str().rsplit_once('.') {
        Some((s, e)) if !s.ends_with('/') && !s.is_empty() => (s, format!(".{e}")),
        _ => (path.as_str(), String::new()),
    };
    let number = if n == 1 {
        String::new()
    } else {
        format!(" {n}")
    };
    RelPath::new(format!("{stem} (Linux conflict 2026-11-15{number}){ext}"))
}

fn copy(path: &str) -> String {
    copy_name(&p(path), 1).as_str().to_string()
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
            Action::MoveLocal { from, to } => {
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
                overwrites_local,
                ..
            } => {
                let taken = disk.contains_key(&key(path.as_str()));
                assert_eq!(
                    taken, *overwrites_local,
                    "download to {path}: taken={taken} but overwrites_local={overwrites_local} in {plan:#?}"
                );
                disk.insert(key(path.as_str()), path.as_str().to_string());
            }
            Action::TrashLocal { path } => {
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
    let result = build_plan(states, &copy_name);
    replay_local(states, &result);
    result
}

fn upload(path: &str, etag: Option<&str>) -> Action {
    upload_from(path, path, etag)
}

fn upload_from(local: &str, remote: &str, etag: Option<&str>) -> Action {
    Action::Upload {
        local: p(local),
        remote: p(remote),
        expected_etag: etag.map(Etag::new),
        size: 7,
        stable_for: Duration::from_secs(60),
    }
}

fn download(path: &str, overwrites: bool) -> Action {
    Action::Download {
        remote: p(path),
        local: p(path),
        size: 9,
        overwrites_local: overwrites,
    }
}

fn move_local(from: &str, to: &str) -> Action {
    Action::MoveLocal {
        from: p(from),
        to: p(to),
    }
}

fn move_remote(from: &str, to: &str, etag: &str) -> Action {
    Action::MoveRemote {
        from: p(from),
        to: p(to),
        expected_etag: Etag::new(etag),
    }
}

fn conflict(path: &str, download_too: bool) -> Vec<Action> {
    let c = copy(path);
    let mut a = vec![
        move_local(path, &c),
        Action::UploadConflictCopy {
            path: p(&c),
            size: 7,
        },
    ];
    if download_too {
        a.push(download(path, false));
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
            vec![upload("a", Some("e1"))],
        ),
        (
            "changed only on iCloud",
            Some("h1"),
            Some("e2"),
            true,
            vec![download("a", true)],
        ),
        (
            "changed on both",
            Some("h2"),
            Some("e2"),
            true,
            conflict("a", true),
        ),
        (
            "deleted on Linux",
            None,
            Some("e1"),
            true,
            vec![Action::TrashRemote {
                path: p("a"),
                expected_etag: Etag::new("e1"),
            }],
        ),
        (
            "deleted on iCloud",
            Some("h1"),
            None,
            true,
            vec![Action::TrashLocal { path: p("a") }],
        ),
        ("deleted on both", None, None, true, vec![]),
        (
            "deleted on Linux, changed on iCloud",
            None,
            Some("e2"),
            true,
            vec![download("a", false)],
        ),
        (
            "changed on Linux, deleted on iCloud",
            Some("h2"),
            None,
            true,
            conflict("a", false),
        ),
        // No common state (the file is new).
        (
            "new only on Linux",
            Some("h1"),
            None,
            false,
            vec![upload("a", None)],
        ),
        (
            "new only on iCloud",
            None,
            Some("e1"),
            false,
            vec![download("a", false)],
        ),
        (
            "new on both",
            Some("h1"),
            Some("e1"),
            false,
            conflict("a", true),
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
fn deleted_on_both_is_forgotten() {
    let states = States {
        base: vec![base("a", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    let result = plan(&states);
    assert_eq!(result.actions, vec![]);
    assert_eq!(result.forget, vec![DocId::new("d1")]);
}

#[test]
fn modification_time_does_not_decide() {
    let mut l = local("a", 1, "h1");
    l.stable_for = Duration::ZERO;
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
    let cases: Vec<(&str, &str, Vec<Action>, Vec<BaseEntry>)> = vec![
        (
            "new inode and new content: an ordinary upload",
            "h2",
            vec![upload("report.odt", Some("e1"))],
            vec![],
        ),
        (
            "new inode, same content: only the common state learns the inode",
            "h1",
            vec![],
            vec![base("report.odt", 77, "d1", "h1", "e1")],
        ),
    ];
    for (name, hash, actions, adopt) in cases {
        let states = States {
            local: vec![local("report.odt", 77, hash)],
            remote: vec![remote("report.odt", "d1", "e1")],
            base: vec![base("report.odt", 1, "d1", "h1", "e1")],
            ..States::default()
        };
        let result = plan(&states);
        assert_eq!(result.actions, actions, "{name}");
        assert_eq!(result.adopt, adopt, "{name}");
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
        result.actions.contains(&Action::TrashRemote {
            path: p("a"),
            expected_etag: Etag::new("e1")
        }),
        "a was deleted locally: {:#?}",
        result.actions
    );
}

// --- moves ---------------------------------------------------------------------------

#[test]
fn moves() {
    // (name, local path, remote path, expected actions, expected adopt path)
    type Case = (
        &'static str,
        &'static str,
        &'static str,
        Vec<Action>,
        Option<&'static str>,
    );
    let cases: Vec<Case> = vec![
        ("not moved", "a", "a", vec![], None),
        (
            "moved only on Linux",
            "dir/b",
            "a",
            vec![move_remote("a", "dir/b", "e1")],
            None,
        ),
        (
            "moved only on iCloud",
            "a",
            "dir/b",
            vec![move_local("a", "dir/b")],
            None,
        ),
        (
            "moved on both, differently: iCloud wins",
            "c",
            "b",
            vec![move_local("c", "b")],
            None,
        ),
        ("moved on both the same way", "b", "b", vec![], Some("b")),
        (
            "renamed locally to another letter case",
            "A",
            "a",
            vec![move_remote("a", "A", "e1")],
            None,
        ),
        (
            "renamed on iCloud to another letter case",
            "a",
            "A",
            vec![move_local("a", "A")],
            None,
        ),
    ];
    for (name, l, r, expected, adopt) in cases {
        let states = States {
            local: vec![local(l, 1, "h1")],
            remote: vec![remote(r, "d1", "e1")],
            remote_folders: vec![p("dir")],
            base: vec![base("a", 1, "d1", "h1", "e1")],
        };
        let result = plan(&states);
        assert_eq!(result.actions, expected, "{name}");
        assert_eq!(
            result.adopt,
            adopt
                .map(|a| base(a, 1, "d1", "h1", "e1"))
                .into_iter()
                .collect::<Vec<_>>(),
            "{name}"
        );
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
        vec![upload_from("b", "a", Some("e1"))]
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
            move_local("b", &copy("b")),
            move_local("a", "b"),
            upload(&copy("b"), None),
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
        actions.contains(&move_local("b", &copy("b"))),
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
        actions.contains(&move_local("B", &copy("B"))),
        "{actions:#?}"
    );
    assert!(actions.contains(&move_local("a", "b")), "{actions:#?}");
    assert!(actions.contains(&upload(&copy("B"), None)), "{actions:#?}");
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
            vec![upload_from(nfd, "Příloha.pdf", None)],
        ),
        (
            "NFD locally and NFC on iCloud, both new: the same name",
            States {
                local: vec![local(nfd, 1, "h1")],
                remote: vec![remote("Příloha.pdf", "d1", "e1")],
                ..States::default()
            },
            vec![
                move_local(nfd, &copy(nfd)),
                Action::UploadConflictCopy {
                    path: p(&copy(nfd)),
                    size: 7,
                },
                download("Příloha.pdf", false),
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
        vec![upload("b.txt", None)],
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
            move_local("report.odt", second),
            Action::UploadConflictCopy {
                path: p(second),
                size: 7
            },
            download("report.odt", false),
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
            vec![folder("a"), folder("a/b"), upload("a/b/c.txt", None)],
        ),
        (
            "folder already on the server (as a folder)",
            States {
                local: vec![local("a/b/c.txt", 1, "h")],
                remote_folders: vec![p("a"), p("A/B")],
                ..States::default()
            },
            vec![upload("a/b/c.txt", None)],
        ),
        (
            "server folder seen from a file inside it",
            States {
                local: vec![local("a/new.txt", 1, "h"), local("a/old.txt", 2, "x")],
                remote: vec![remote("a/old.txt", "d2", "e")],
                base: vec![base("a/old.txt", 2, "d2", "x", "e")],
                ..States::default()
            },
            vec![upload("a/new.txt", None)],
        ),
        (
            "two files in the same new folder, folder once",
            States {
                local: vec![local("s/1", 1, "h"), local("s/2", 2, "h")],
                ..States::default()
            },
            vec![folder("s"), upload("s/1", None), upload("s/2", None)],
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
        let mut c = Context::new(states.base.len() as u64);
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
