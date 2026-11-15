//! Tests of the state database through its public API.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::path::PathBuf;

use ikeloud_safety::RelPath;
use ikeloud_state::{BaseChange, JournalEntry, JournalKind, StateDb, StateError};
use ikeloud_sync::{BaseEntry, DocId};
use ikeloud_testkit::base as entry;

fn set(path: &str, n: u64, doc: &str, hash: &str, etag: &str) -> BaseChange {
    BaseChange::Set(entry(path, n, doc, hash, etag))
}

fn forget(doc: &str) -> BaseChange {
    BaseChange::Forget(DocId::new(doc))
}

fn log(time: u64, kind: JournalKind, path: &str, target: Option<&str>) -> JournalEntry {
    JournalEntry {
        time,
        kind,
        path: RelPath::new(path),
        target: target.map(RelPath::new),
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ikeloud-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("state.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn common_state_changes() {
    // (name, commits applied in order, expected state)
    type Case = (&'static str, Vec<Vec<BaseChange>>, Vec<BaseEntry>);
    let cases: Vec<Case> = vec![
        ("empty database", vec![], vec![]),
        (
            "entries come back sorted by normalized name",
            vec![vec![
                set("b.txt", 1, "d1", "h1", "e1"),
                set("A.txt", 2, "d2", "h2", "e2"),
            ]],
            vec![
                entry("A.txt", 2, "d2", "h2", "e2"),
                entry("b.txt", 1, "d1", "h1", "e1"),
            ],
        ),
        (
            "set replaces the entry of the same document",
            vec![
                vec![set("a", 1, "d1", "h1", "e1")],
                vec![set("a", 1, "d1", "h2", "e2")],
            ],
            vec![entry("a", 1, "d1", "h2", "e2")],
        ),
        (
            "a move keeps the document and changes the path",
            vec![
                vec![set("a", 1, "d1", "h", "e")],
                vec![set("dir/b", 1, "d1", "h", "e")],
            ],
            vec![entry("dir/b", 1, "d1", "h", "e")],
        ),
        (
            "a new inode after a save replaces the old one",
            vec![
                vec![set("a", 1, "d1", "h", "e")],
                vec![set("a", 77, "d1", "h", "e")],
            ],
            vec![entry("a", 77, "d1", "h", "e")],
        ),
        (
            "two files swap names in one commit",
            vec![
                vec![set("a", 1, "d1", "h1", "e1"), set("b", 2, "d2", "h2", "e2")],
                vec![set("b", 1, "d1", "h1", "e1"), set("a", 2, "d2", "h2", "e2")],
            ],
            vec![
                entry("a", 2, "d2", "h2", "e2"),
                entry("b", 1, "d1", "h1", "e1"),
            ],
        ),
        (
            "forget removes",
            vec![
                vec![set("a", 1, "d1", "h", "e"), set("b", 2, "d2", "h", "e")],
                vec![forget("d1")],
            ],
            vec![entry("b", 2, "d2", "h", "e")],
        ),
        (
            "forget of a missing entry is a no-op",
            vec![vec![forget("nothing")]],
            vec![],
        ),
        (
            "the largest inode survives",
            vec![vec![set("a", u64::MAX, "d1", "h", "e")]],
            vec![entry("a", u64::MAX, "d1", "h", "e")],
        ),
        (
            "a local move pending: local path and identity round-trip",
            vec![vec![BaseChange::Set(BaseEntry {
                local_path: RelPath::new("moved"),
                local: Some(ikeloud_safety::LocalId {
                    device: u64::MAX,
                    inode: 3,
                    birth_ns: Some(-5),
                }),
                ..entry("a", 1, "d1", "h", "e")
            })]],
            vec![BaseEntry {
                local_path: RelPath::new("moved"),
                local: Some(ikeloud_safety::LocalId {
                    device: u64::MAX,
                    inode: 3,
                    birth_ns: Some(-5),
                }),
                ..entry("a", 1, "d1", "h", "e")
            }],
        ),
        (
            "same inode on another device is another file",
            vec![vec![
                set("a", 1, "d1", "h", "e"),
                BaseChange::Set(BaseEntry {
                    local: Some(ikeloud_safety::LocalId {
                        device: 2,
                        ..ikeloud_testkit::id(1)
                    }),
                    ..entry("b", 1, "d2", "h", "e")
                }),
            ]],
            vec![
                entry("a", 1, "d1", "h", "e"),
                BaseEntry {
                    local: Some(ikeloud_safety::LocalId {
                        device: 2,
                        ..ikeloud_testkit::id(1)
                    }),
                    ..entry("b", 1, "d2", "h", "e")
                },
            ],
        ),
    ];
    for (name, commits, expected) in cases {
        let mut db = StateDb::open_in_memory().expect("open");
        for changes in &commits {
            db.commit(changes, &[]).expect("commit");
        }
        assert_eq!(db.base().expect("read"), expected, "{name}");
    }
}

#[test]
fn inconsistent_commits_fail_whole() {
    // (name, change that must fail on top of entries a/d1/1 and b/d2/2, error)
    let taken = |s: &str| StateError::NameTaken {
        path: RelPath::new(s),
    };
    let invalid = |field| StateError::InvalidValue { field };
    let cases: Vec<(&str, Vec<BaseChange>, StateError)> = vec![
        (
            "another document takes a taken name",
            vec![set("a", 3, "d3", "h", "e")],
            taken("a"),
        ),
        (
            "same name in another letter case",
            vec![set("A", 3, "d3", "h", "e")],
            taken("A"),
        ),
        (
            "NFD form of a taken NFC name",
            vec![set("Pr\u{30c}i\u{301}loha", 3, "d3", "h", "e")],
            taken("Pr\u{30c}i\u{301}loha"),
        ),
        (
            "two new documents with one name in one commit",
            vec![set("n", 3, "d3", "h", "e"), set("N", 4, "d4", "h", "e")],
            taken("N"),
        ),
        (
            "another document takes a taken local file",
            vec![set("c", 1, "d3", "h", "e")],
            StateError::LocalFileTaken {
                path: RelPath::new("c"),
            },
        ),
        (
            "empty hash",
            vec![set("c", 3, "d3", "", "e")],
            invalid("hash"),
        ),
        (
            "empty document ID",
            vec![set("c", 3, "", "h", "e")],
            invalid("doc_id"),
        ),
    ];
    for (name, changes, error) in cases {
        let mut db = StateDb::open_in_memory().expect("open");
        let start = vec![
            set("a", 1, "d1", "h", "e"),
            set("b", 2, "d2", "h", "e"),
            set("Příloha", 9, "d9", "h", "e"),
        ];
        db.commit(&start, &[]).expect("commit");
        let before = db.base().expect("read");
        let mut all = vec![forget("d2")];
        all.extend(changes);
        let result = db.commit(&all, &[log(1, JournalKind::Uploaded, "x", None)]);
        assert_eq!(result, Err(error), "{name}");
        assert_eq!(
            db.base().expect("read"),
            before,
            "{name}: nothing of the commit is stored"
        );
        assert_eq!(db.journal(10).expect("read"), vec![], "{name}");
    }
}

#[test]
fn lookup_of_one_entry() {
    let mut db = StateDb::open_in_memory().expect("open");
    db.commit(&[set("Docs/Report.odt", 1, "d1", "h", "e")], &[])
        .expect("commit");
    assert_eq!(
        db.base_entry(&RelPath::new("docs/report.odt"))
            .expect("read"),
        Some(entry("Docs/Report.odt", 1, "d1", "h", "e"))
    );
    assert_eq!(db.base_entry(&RelPath::new("missing")).expect("read"), None);
}

#[test]
fn journal_order_limit_and_kinds() {
    let mut db = StateDb::open_in_memory().expect("open");
    let all: Vec<JournalEntry> = JournalKind::ALL
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let target = matches!(
                k,
                JournalKind::MovedLocal | JournalKind::MovedRemote | JournalKind::ConflictCopy
            )
            .then_some("target");
            log(
                u64::try_from(i).expect("small"),
                *k,
                &format!("f{i}"),
                target,
            )
        })
        .collect();
    db.commit(&[], &all).expect("commit");

    let newest_first: Vec<JournalEntry> = all.iter().rev().cloned().collect();
    assert_eq!(
        db.journal(100).expect("read"),
        newest_first,
        "every kind round-trips"
    );
    assert_eq!(
        db.journal(3).expect("read"),
        newest_first[..3].to_vec(),
        "limit"
    );
    assert_eq!(db.journal(0).expect("read"), vec![], "zero limit");
}

#[test]
fn journal_kind_names_are_stable() {
    // Stored in user databases: renaming a kind would break old journals.
    let names: Vec<&str> = JournalKind::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(
        names,
        [
            "uploaded",
            "downloaded",
            "trashed-remote",
            "trashed-local",
            "moved-remote",
            "moved-local",
            "conflict-copy",
            "folder-created"
        ]
    );
    for k in JournalKind::ALL {
        assert_eq!(JournalKind::from_name(k.name()), Some(k));
    }
    assert_eq!(JournalKind::from_name("teleported"), None);
}

#[test]
fn time_out_of_range_is_rejected() {
    let mut db = StateDb::open_in_memory().expect("open");
    let result = db.commit(&[], &[log(u64::MAX, JournalKind::Uploaded, "a", None)]);
    assert_eq!(result, Err(StateError::InvalidValue { field: "time" }));
    assert_eq!(db.journal(10).expect("read"), vec![]);
}

#[test]
fn state_survives_reopening() {
    let dir = TempDir::new("reopen");
    {
        let mut db = StateDb::open(&dir.db()).expect("create");
        db.commit(
            &[set("a", 1, "d1", "h", "e")],
            &[log(7, JournalKind::Downloaded, "a", None)],
        )
        .expect("commit");
    }
    let db = StateDb::open(&dir.db()).expect("reopen");
    assert_eq!(
        db.base().expect("read"),
        vec![entry("a", 1, "d1", "h", "e")]
    );
    assert_eq!(
        db.journal(10).expect("read"),
        vec![log(7, JournalKind::Downloaded, "a", None)]
    );
    assert!(
        dir.0.join("state.db-wal").exists(),
        "the database runs in WAL mode"
    );
}

#[test]
fn a_second_handle_sees_committed_state() {
    let dir = TempDir::new("two");
    let mut writer = StateDb::open(&dir.db()).expect("writer");
    let reader = StateDb::open(&dir.db()).expect("reader");
    writer
        .commit(&[set("a", 1, "d1", "h", "e")], &[])
        .expect("commit");
    assert_eq!(
        reader.base().expect("read"),
        vec![entry("a", 1, "d1", "h", "e")]
    );
}

#[test]
fn unopenable_path_is_an_open_error() {
    let dir = TempDir::new("bad");
    let result = StateDb::open(&dir.0.join("missing-folder").join("state.db"));
    assert!(matches!(result, Err(StateError::Open(_))), "{result:?}");
}

#[test]
fn entries_without_a_known_inode() {
    // Several files may have lost their local identity at once.
    let mut db = StateDb::open_in_memory().expect("open");
    let a = entry("a", None, "d1", "h", "e");
    let b = entry("b", None, "d2", "h", "e");
    db.commit(
        &[BaseChange::Set(a.clone()), BaseChange::Set(b.clone())],
        &[],
    )
    .expect("commit");
    assert_eq!(db.base().expect("read"), vec![a, b]);
}
