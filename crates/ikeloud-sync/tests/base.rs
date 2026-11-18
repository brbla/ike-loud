//! Table tests of next_base: old common state + scan after the run +
//! transfers → new common state.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use ikeloud_safety::LocalId;
use ikeloud_sync::{BaseEntry, LocalFile, RemoteFile, States, Transfer, next_base};
use ikeloud_testkit::{base, local, remote, transfer};

#[test]
fn next_common_state() {
    struct Case {
        name: &'static str,
        old: Vec<BaseEntry>,
        local: Vec<LocalFile>,
        remote: Vec<RemoteFile>,
        transfers: Vec<Transfer>,
        expected: Vec<BaseEntry>,
    }
    let nfd = "Pr\u{30c}i\u{301}loha";
    let cases = vec![
        Case {
            name: "unchanged on both sides: unchanged entry",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 1, "h1")],
            remote: vec![remote("a", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", Some(1), "d1", "h1", "e1")],
        },
        Case {
            name: "upload: new content and etag",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 1, "h2")],
            remote: vec![remote("a", "d1", "e2")],
            transfers: vec![transfer("a", "d1", "e2", 1, "h2")],
            expected: vec![base("a", Some(1), "d1", "h2", "e2")],
        },
        Case {
            name: "new file uploaded: new entry",
            old: vec![],
            local: vec![local("n", 5, "h5")],
            remote: vec![remote("n", "d5", "e5")],
            transfers: vec![transfer("n", "d5", "e5", 5, "h5")],
            expected: vec![base("n", Some(5), "d5", "h5", "e5")],
        },
        Case {
            name: "new NFD file uploaded under its NFC name: one place",
            old: vec![],
            local: vec![local(nfd, 5, "h5")],
            remote: vec![remote("Příloha", "d5", "e5")],
            transfers: vec![transfer("Příloha", "d5", "e5", 5, "h5")],
            // The local name stays as it is on disk (NFD).
            expected: vec![BaseEntry {
                local_path: ikeloud_testkit::p(nfd),
                ..base("Příloha", Some(5), "d5", "h5", "e5")
            }],
        },
        Case {
            name: "transfer while both sides moved apart: the path the local file holds",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("mine", 9, "h2"), local("a (copy)", 1, "hx")],
            remote: vec![remote("moved", "d1", "e2"), remote("a (copy)", "d7", "e7")],
            transfers: vec![
                transfer("a (copy)", "d7", "e7", 1, "hx"),
                transfer("moved", "d1", "e2", 9, "h2"),
            ],
            expected: vec![
                base("a (copy)", Some(1), "d7", "hx", "e7"),
                base("mine", Some(9), "d1", "h2", "e2"),
            ],
        },
        Case {
            // Daemon simulation, seed 611: a crash after both transfers;
            // meanwhile the user swapped the two local files and moved the
            // copy on iCloud. Both want "a"; the moved one takes its old
            // path (both moved: iCloud wins), nobody is dropped.
            name: "two proven files wanting one name: the second path, not a lost entry",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 2, "h5"), local("c", 3, "h4")],
            remote: vec![remote("a", "d1", "e3"), remote("d/e", "d4", "e4")],
            transfers: vec![
                transfer("c (copy)", "d4", "e4", 2, "h5"),
                transfer("a", "d1", "e3", 3, "h4"),
            ],
            expected: vec![
                BaseEntry {
                    local_path: ikeloud_testkit::p("c"),
                    ..base("a", Some(3), "d1", "h4", "e3")
                },
                BaseEntry {
                    local_path: ikeloud_testkit::p("a"),
                    ..base("c (copy)", Some(2), "d4", "h5", "e4")
                },
            ],
        },
        Case {
            name: "transfer while moved only locally: the old path (the server file holds it)",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("mine", 1, "h2")],
            remote: vec![remote("a", "d1", "e2")],
            transfers: vec![transfer("a", "d1", "e2", 1, "h2")],
            expected: vec![BaseEntry {
                local_path: ikeloud_testkit::p("mine"),
                ..base("a", Some(1), "d1", "h2", "e2")
            }],
        },
        Case {
            name: "the old path taken by another document meanwhile: never kept for this one",
            old: vec![base("b", Some(1), "d1", "h1", "e1")],
            local: vec![local("Příloha", 10, "h9"), local("b", 12, "h6")],
            remote: vec![remote("c", "d1", "e9"), remote("b", "d6", "e6")],
            transfers: vec![
                transfer("c", "d1", "e9", 10, "h9"),
                transfer("b", "d6", "e6", 12, "h6"),
            ],
            expected: vec![
                base("b", Some(12), "d6", "h6", "e6"),
                base("Příloha", Some(10), "d1", "h9", "e9"),
            ],
        },
        Case {
            name: "moved the same way on both sides: path updated",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("b", 1, "h1")],
            remote: vec![remote("b", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("b", Some(1), "d1", "h1", "e1")],
        },
        Case {
            name: "moved only on iCloud: path stays until the local move",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 1, "h1")],
            remote: vec![remote("b", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", Some(1), "d1", "h1", "e1")],
        },
        Case {
            name: "re-saved with a new inode, same content: inode learned",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 77, "h1")],
            remote: vec![remote("a", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", Some(77), "d1", "h1", "e1")],
        },
        Case {
            name: "gone on both sides: dropped",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![],
            remote: vec![],
            transfers: vec![],
            expected: vec![],
        },
        Case {
            name: "changed after the transfer: the transfer proves nothing",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 1, "h3")],
            remote: vec![remote("a", "d1", "e2")],
            transfers: vec![transfer("a", "d1", "e2", 1, "h2")],
            expected: vec![base("a", Some(1), "d1", "h1", "e1")],
        },
        Case {
            name: "changed on one side, no transfer: entry kept as it was",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("a", 1, "h2")],
            remote: vec![remote("a", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", Some(1), "d1", "h1", "e1")],
        },
        Case {
            name: "local file gone and its inode now another file's: identity forgotten",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("b", 1, "hb")],
            remote: vec![remote("a", "d1", "e1"), remote("b", "d2", "eb")],
            transfers: vec![transfer("b", "d2", "eb", 1, "hb")],
            expected: vec![
                base("a", None, "d1", "h1", "e1"),
                base("b", Some(1), "d2", "hb", "eb"),
            ],
        },
        Case {
            name: "local file gone: its identity is forgotten (the inode may be reused)",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![],
            remote: vec![remote("a", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", None, "d1", "h1", "e1")],
        },
        Case {
            name: "a reused inode (another birth time) is not the entry's file",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![LocalFile {
                id: LocalId {
                    birth_ns: Some(99),
                    ..ikeloud_testkit::id(1)
                },
                ..local("x", 1, "hx")
            }],
            remote: vec![remote("a", "d1", "e1")],
            transfers: vec![],
            expected: vec![base("a", None, "d1", "h1", "e1")],
        },
        Case {
            name: "the last transfer of a document wins",
            old: vec![],
            local: vec![local("a", 1, "h2")],
            remote: vec![remote("a", "d1", "e2")],
            transfers: vec![
                transfer("a", "d1", "e1", 1, "h1"),
                transfer("a", "d1", "e2", 1, "h2"),
            ],
            expected: vec![base("a", Some(1), "d1", "h2", "e2")],
        },
    ];
    for c in cases {
        let after = States {
            local: c.local,
            remote: c.remote,
            remote_folders: vec![],
            base: vec![],
        };
        assert_eq!(
            next_base(&c.old, &after, &c.transfers),
            c.expected,
            "{}",
            c.name
        );
    }
}
