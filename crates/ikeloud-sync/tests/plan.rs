//! Table tests of the planner: three states → actions.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::time::Duration;

use ikeloud_safety::{Action, Etag, RelPath};
use ikeloud_sync::{
    BaseEntry, Collision, ContentHash, LocalFile, Plan, RemoteFile, Side, States, build_plan,
};

// --- helpers -----------------------------------------------------------------

fn p(s: &str) -> RelPath {
    RelPath::new(s)
}

fn local(path: &str, hash: &str) -> LocalFile {
    LocalFile {
        path: p(path),
        hash: ContentHash::new(hash),
        size: 7,
        stable_for: Duration::from_secs(60),
    }
}

fn remote(path: &str, etag: &str) -> RemoteFile {
    RemoteFile {
        path: p(path),
        etag: Etag::new(etag),
        size: 9,
    }
}

fn base(path: &str, hash: &str, etag: &str) -> BaseEntry {
    BaseEntry {
        path: p(path),
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

fn plan(states: &States) -> Plan {
    build_plan(states, &copy_name)
}

fn upload(path: &str, etag: Option<&str>) -> Action {
    Action::Upload {
        path: p(path),
        expected_etag: etag.map(Etag::new),
        size: 7,
        stable_for: Duration::from_secs(60),
    }
}

fn download(path: &str, overwrites: bool) -> Action {
    Action::Download {
        path: p(path),
        size: 9,
        overwrites_local: overwrites,
    }
}

fn conflict(path: &str, copy: &str, download_too: bool) -> Vec<Action> {
    let mut a = vec![
        Action::MoveLocal {
            from: p(path),
            to: p(copy),
        },
        Action::UploadConflictCopy {
            path: p(copy),
            size: 7,
        },
    ];
    if download_too {
        a.push(download(path, false));
    }
    a
}

// --- decision table ---------------------------------------------------------------

#[test]
fn decision_table() {
    // Common state: file "a" with content h1 and etag e1.
    // l/r: None = no file on that side; h1/e1 = unchanged; h2/e2 = changed.
    type Case = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        Vec<Action>,
    );
    let copy_a = "a (Linux conflict 2026-11-15)";
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
            conflict("a", copy_a, true),
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
            conflict("a", copy_a, false),
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
            conflict("a", copy_a, true),
        ),
    ];
    for (name, l, r, with_base, expected) in cases {
        let states = States {
            local: l.map(|h| local("a", h)).into_iter().collect(),
            remote: r.map(|e| remote("a", e)).into_iter().collect(),
            base: if with_base {
                vec![base("a", "h1", "e1")]
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
        base: vec![base("a", "h1", "e1")],
        ..States::default()
    };
    let p2 = plan(&states);
    assert_eq!(p2.actions, vec![]);
    assert_eq!(p2.forget, vec![p("a")]);
}

#[test]
fn modification_time_does_not_decide() {
    // A different "stability" (time) with the same hash is not a change.
    let mut l = local("a", "h1");
    l.stable_for = Duration::ZERO;
    let states = States {
        local: vec![l],
        remote: vec![remote("a", "e1")],
        base: vec![base("a", "h1", "e1")],
        ..States::default()
    };
    assert_eq!(plan(&states).actions, vec![]);
}

// --- names --------------------------------------------------------------------------

#[test]
fn pairing_after_normalization() {
    let nfd = "Pr\u{30c}i\u{301}loha.pdf";
    let cases: Vec<(&str, States, Vec<Action>)> = vec![
        (
            "NFD on Linux and NFC on iCloud is the same file",
            States {
                local: vec![local(nfd, "h1")],
                remote: vec![remote("Příloha.pdf", "e1")],
                base: vec![base("Příloha.pdf", "h1", "e1")],
                ..States::default()
            },
            vec![],
        ),
        (
            "different case is the same file; uploads under the server name",
            States {
                local: vec![local("photo.JPG", "h2")],
                remote: vec![remote("Photo.jpg", "e1")],
                base: vec![base("Photo.jpg", "h1", "e1")],
                ..States::default()
            },
            vec![upload("Photo.jpg", Some("e1"))],
        ),
        (
            "a new NFD file is uploaded as NFC",
            States {
                local: vec![local(nfd, "h1")],
                ..States::default()
            },
            vec![upload("Příloha.pdf", None)],
        ),
        (
            "download over a local file goes to the local name",
            States {
                local: vec![local("photo.jpg", "h1")],
                remote: vec![remote("Photo.jpg", "e2")],
                base: vec![base("Photo.jpg", "h1", "e1")],
                ..States::default()
            },
            vec![download("photo.jpg", true)],
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
            local("a.txt", "h1"),
            local("A.txt", "h2"),
            local("b.txt", "h3"),
        ],
        remote: vec![remote("a.txt", "e1")],
        base: vec![base("a.txt", "h1", "e1")],
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
        remote: vec![remote("x", "e1"), remote("X", "e2")],
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
            local(".~lock.report.odt#", "h"),
            local("a.swp", "h"),
            local("x.part", "h"),
        ],
        remote: vec![remote("photos/.DS_Store", "e")],
        ..States::default()
    };
    assert_eq!(plan(&states), Plan::default());
}

// --- conflict copies -----------------------------------------------------------------

#[test]
fn conflict_copy_gets_a_free_name() {
    let taken = "report (Linux conflict 2026-11-15).odt";
    let states = States {
        local: vec![local("report.odt", "h2"), local(taken, "x")],
        remote: vec![remote("report.odt", "e2"), remote(taken, "y")],
        base: vec![base("report.odt", "h1", "e1"), base(taken, "x", "y")],
        ..States::default()
    };
    assert_eq!(
        plan(&states).actions,
        conflict(
            "report.odt",
            "report (Linux conflict 2026-11-15 2).odt",
            true
        )
    );
}

#[test]
fn two_copies_in_one_run_do_not_clash() {
    let states = States {
        local: vec![local("d/a.txt", "h2"), local("e/a.txt", "h2")],
        remote: vec![remote("d/a.txt", "e2"), remote("e/a.txt", "e2")],
        base: vec![base("d/a.txt", "h1", "e1"), base("e/a.txt", "h1", "e1")],
        ..States::default()
    };
    let result = plan(&states);
    let copies: Vec<&RelPath> = result
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::UploadConflictCopy { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(
        copies,
        vec![
            &p("d/a (Linux conflict 2026-11-15).txt"),
            &p("e/a (Linux conflict 2026-11-15).txt")
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
                local: vec![local("a/b/c.txt", "h")],
                ..States::default()
            },
            vec![folder("a"), folder("a/b"), upload("a/b/c.txt", None)],
        ),
        (
            "folder already on the server (as a folder)",
            States {
                local: vec![local("a/b/c.txt", "h")],
                remote_folders: vec![p("a"), p("A/B")],
                ..States::default()
            },
            vec![upload("a/b/c.txt", None)],
        ),
        (
            "server folder seen from a file inside it",
            States {
                local: vec![local("a/new.txt", "h"), local("a/old.txt", "x")],
                remote: vec![remote("a/old.txt", "e")],
                base: vec![base("a/old.txt", "x", "e")],
                ..States::default()
            },
            vec![upload("a/new.txt", None)],
        ),
        (
            "two files in the same new folder, folder once",
            States {
                local: vec![local("s/1", "h"), local("s/2", "h")],
                ..States::default()
            },
            vec![folder("s"), upload("s/1", None), upload("s/2", None)],
        ),
        (
            "overwriting an existing file creates no folder",
            States {
                local: vec![local("a/b.txt", "h2")],
                remote: vec![remote("a/b.txt", "e1")],
                base: vec![base("a/b.txt", "h1", "e1")],
                ..States::default()
            },
            vec![upload("a/b.txt", Some("e1"))],
        ),
    ];
    for (name, states, expected) in cases {
        assert_eq!(plan(&states).actions, expected, "{name}");
    }
}

// --- the whole plan passes the safety layer --------------------------------------------

#[test]
fn plan_after_a_linux_change_passes_safety() {
    use ikeloud_safety::{Context, Decision, assess};

    let states = States {
        local: vec![local("a", "h2"), local("b", "h1")],
        remote: vec![remote("a", "e1"), remote("b", "e1")],
        base: vec![base("a", "h1", "e1"), base("b", "h1", "e1")],
        ..States::default()
    };
    let result = plan(&states);
    let mut c = Context::new(2);
    for r in &states.remote {
        c.remote_etags.insert(r.path.clone(), r.etag.clone());
    }
    let Ok(Decision::Approved(approved)) = assess(result.actions, &c) else {
        panic!("the plan should pass");
    };
    assert_eq!(approved.actions.len(), 1);
    assert!(approved.deferred.is_empty());
}
