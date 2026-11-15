//! Tests of the plain types the safety layer defines. They are used by the
//! other crates, but must be correct on their own: `cargo-mutants` on this
//! crate runs only these tests.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use ikeloud_safety::{
    Action, Change, ChangeKind, ContentHash, Etag, LocalId, LocalVersion, RelPath,
};

#[test]
fn relative_paths() {
    // (path, parent, name)
    let cases: &[(&str, &str, &str)] = &[
        ("a.txt", "", "a.txt"),
        ("dir/a.txt", "dir", "a.txt"),
        ("a/b/c", "a/b", "c"),
        ("Příloha/smlouva.pdf", "Příloha", "smlouva.pdf"),
    ];
    for (path, parent, name) in cases {
        let p = RelPath::new(*path);
        assert_eq!(p.parent(), *parent, "{path}: parent");
        assert_eq!(p.name(), *name, "{path}: name");
        assert_eq!(p.as_str(), *path, "{path}: text");
        assert_eq!(p.to_string(), *path, "{path}: display");
    }
}

#[test]
fn content_hash_keeps_its_text() {
    assert_eq!(ContentHash::new("abc123").as_str(), "abc123");
}

#[test]
fn local_identity() {
    let id = |inode: u64, birth_ns: Option<i64>| LocalId {
        device: 1,
        inode,
        birth_ns,
    };
    // (name, earlier, later, expected)
    let cases: &[(&str, LocalId, LocalId, Option<bool>)] = &[
        (
            "same inode and birth",
            id(1, Some(5)),
            id(1, Some(5)),
            Some(true),
        ),
        (
            "inode reused: another birth",
            id(1, Some(5)),
            id(1, Some(6)),
            Some(false),
        ),
        ("another inode", id(1, Some(5)), id(2, Some(5)), Some(false)),
        (
            "another device",
            id(1, Some(5)),
            LocalId {
                device: 2,
                ..id(1, Some(5))
            },
            Some(false),
        ),
        ("birth unknown later", id(1, Some(5)), id(1, None), None),
        ("birth unknown before", id(1, None), id(1, Some(5)), None),
        ("birth unknown on both", id(1, None), id(1, None), None),
        (
            "another inode, birth unknown: certainly not",
            id(1, None),
            id(2, None),
            Some(false),
        ),
    ];
    for (name, earlier, later, expected) in cases {
        assert_eq!(earlier.same_file(later), *expected, "{name}");
    }
}

#[test]
fn what_an_action_changes() {
    let p = |s: &str| RelPath::new(s);
    let version = LocalVersion {
        id: LocalId {
            device: 1,
            inode: 1,
            birth_ns: None,
        },
        hash: ContentHash::new("h1"),
    };
    let change = |kind, path: &str, to: Option<&str>, version: &str| {
        Some(Change {
            kind,
            path: p(path),
            to: to.map(p),
            version: version.into(),
        })
    };
    let cases: Vec<(&str, Action, Option<Change>)> = vec![
        (
            "local trash: the content hash the plan saw",
            Action::TrashLocal {
                path: p("a"),
                expected: version.clone(),
            },
            change(ChangeKind::TrashLocal, "a", None, "h1"),
        ),
        (
            "download over a local file",
            Action::Download {
                remote: p("r"),
                local: p("l"),
                size: 1,
                replaces: Some(version.clone()),
            },
            change(ChangeKind::OverwriteLocal, "l", None, "h1"),
        ),
        (
            "download to a free place",
            Action::Download {
                remote: p("r"),
                local: p("l"),
                size: 1,
                replaces: None,
            },
            None,
        ),
        (
            "a local move: the identity it was planned against",
            Action::MoveLocal {
                from: p("a"),
                to: p("b"),
                local_id: LocalId {
                    device: 3,
                    inode: 4,
                    birth_ns: Some(5),
                },
            },
            change(ChangeKind::MoveLocal, "a", Some("b"), "3:4:5"),
        ),
        (
            "a local move without a birth time",
            Action::MoveLocal {
                from: p("a"),
                to: p("b"),
                local_id: version.id,
            },
            change(ChangeKind::MoveLocal, "a", Some("b"), "1:1:-"),
        ),
        (
            "a server move: the etag",
            Action::MoveRemote {
                from: p("a"),
                to: p("b"),
                expected_etag: Etag::new("e1"),
            },
            change(ChangeKind::MoveRemote, "a", Some("b"), "e1"),
        ),
        (
            "an upload of a new file changes no existing file",
            Action::UploadConflictCopy {
                path: p("c"),
                local_id: version.id,
                size: 1,
                stable_for: std::time::Duration::ZERO,
            },
            None,
        ),
    ];
    for (name, action, expected) in cases {
        assert_eq!(action.change(), expected, "{name}");
    }
    assert!(ChangeKind::MoveLocal.is_move() && ChangeKind::MoveRemote.is_move());
    assert!(!ChangeKind::TrashLocal.is_move() && !ChangeKind::OverwriteRemote.is_move());
}
