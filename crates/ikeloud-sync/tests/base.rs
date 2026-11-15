//! Table tests of next_base: old common state + scan after the run +
//! transfers → new common state.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::time::Duration;

use ikeloud_safety::{Etag, RelPath};
use ikeloud_sync::{
    BaseEntry, ContentHash, DocId, LocalFile, RemoteFile, States, Transfer, next_base,
};

fn local(path: &str, inode: u64, hash: &str) -> LocalFile {
    LocalFile {
        path: RelPath::new(path),
        inode,
        hash: ContentHash::new(hash),
        size: 1,
        stable_for: Duration::from_secs(60),
    }
}

fn remote(path: &str, doc: &str, etag: &str) -> RemoteFile {
    RemoteFile {
        path: RelPath::new(path),
        doc_id: DocId::new(doc),
        etag: Etag::new(etag),
        size: 1,
    }
}

fn base(path: &str, inode: Option<u64>, doc: &str, hash: &str, etag: &str) -> BaseEntry {
    BaseEntry {
        path: RelPath::new(path),
        inode,
        doc_id: DocId::new(doc),
        hash: ContentHash::new(hash),
        etag: Etag::new(etag),
    }
}

fn transfer(doc: &str, etag: &str, inode: u64, hash: &str) -> Transfer {
    Transfer {
        doc_id: DocId::new(doc),
        etag: Etag::new(etag),
        inode,
        hash: ContentHash::new(hash),
    }
}

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
            transfers: vec![transfer("d1", "e2", 1, "h2")],
            expected: vec![base("a", Some(1), "d1", "h2", "e2")],
        },
        Case {
            name: "new file uploaded: new entry",
            old: vec![],
            local: vec![local("n", 5, "h5")],
            remote: vec![remote("n", "d5", "e5")],
            transfers: vec![transfer("d5", "e5", 5, "h5")],
            expected: vec![base("n", Some(5), "d5", "h5", "e5")],
        },
        Case {
            name: "new NFD file uploaded under its NFC name: one place",
            old: vec![],
            local: vec![local(nfd, 5, "h5")],
            remote: vec![remote("Příloha", "d5", "e5")],
            transfers: vec![transfer("d5", "e5", 5, "h5")],
            expected: vec![base("Příloha", Some(5), "d5", "h5", "e5")],
        },
        Case {
            name: "download to another place (moved on one side): content learned, path kept",
            old: vec![base("a", Some(1), "d1", "h1", "e1")],
            local: vec![local("mine", 9, "h2"), local("a (copy)", 1, "hx")],
            remote: vec![remote("moved", "d1", "e2"), remote("a (copy)", "d7", "e7")],
            transfers: vec![transfer("d7", "e7", 1, "hx"), transfer("d1", "e2", 9, "h2")],
            expected: vec![
                base("a", Some(9), "d1", "h2", "e2"),
                base("a (copy)", Some(1), "d7", "hx", "e7"),
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
            transfers: vec![transfer("d1", "e2", 1, "h2")],
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
            transfers: vec![transfer("d2", "eb", 1, "hb")],
            expected: vec![
                base("a", None, "d1", "h1", "e1"),
                base("b", Some(1), "d2", "hb", "eb"),
            ],
        },
        Case {
            name: "the last transfer of a document wins",
            old: vec![],
            local: vec![local("a", 1, "h2")],
            remote: vec![remote("a", "d1", "e2")],
            transfers: vec![transfer("d1", "e1", 1, "h1"), transfer("d1", "e2", 1, "h2")],
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
