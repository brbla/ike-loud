//! Tests of the scanner on real files in a temporary folder.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)] // test code

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ikeloud_local::{Scan, ScanError, SkipReason, scan};
use ikeloud_sync::LocalFile;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ikeloud-local-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn write(&self, rel: &str, content: &[u8]) {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("folders");
        }
        fs::write(path, content).expect("write");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scan_later(dir: &TempDir) -> Scan {
    // A minute after now: every file written by the test counts as settled.
    scan(dir.path(), SystemTime::now() + Duration::from_secs(60)).expect("scan")
}

fn file<'a>(s: &'a Scan, path: &str) -> &'a LocalFile {
    s.files
        .iter()
        .find(|f| f.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} not in {s:?}"))
}

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn files_in_folders() {
    let d = TempDir::new("basic");
    d.write("abc.txt", b"abc");
    d.write("Dokumenty/Příloha.pdf", b"abc");
    d.write("a/b/c/deep", b"");
    fs::create_dir_all(d.path().join("empty")).expect("folder");
    let s = scan_later(&d);
    let mut paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["Dokumenty/Příloha.pdf", "a/b/c/deep", "abc.txt"]);
    assert_eq!(s.skipped, vec![], "empty folders are not files");
    let abc = file(&s, "abc.txt");
    assert_eq!(abc.hash.as_str(), ABC_SHA256);
    assert_eq!(abc.size, 3);
    assert!(abc.stable_for >= Duration::from_secs(59), "{abc:?}");
    assert_eq!(
        file(&s, "Dokumenty/Příloha.pdf").hash,
        abc.hash,
        "same content, same hash"
    );
}

#[test]
fn identity_survives_a_rename_but_not_a_save_as_new_file() {
    let d = TempDir::new("identity");
    d.write("a.txt", b"one");
    let first = file(&scan_later(&d), "a.txt").clone();

    fs::rename(d.path().join("a.txt"), d.path().join("b.txt")).expect("rename");
    let renamed = file(&scan_later(&d), "b.txt").clone();
    assert_eq!(renamed.id, first.id, "a rename keeps the identity");

    // An editor: write a new file and rename it over the old one.
    d.write("b.txt.tmp", b"two");
    fs::rename(d.path().join("b.txt.tmp"), d.path().join("b.txt")).expect("rename over");
    let resaved = file(&scan_later(&d), "b.txt").clone();
    assert_ne!(
        resaved.id.same_file(&first.id),
        Some(true),
        "a new file is never certainly the old one"
    );
    assert_ne!(
        (resaved.id.device, resaved.id.inode),
        (first.id.device, first.id.inode),
        "while the old file is still open, its inode is not reused"
    );
}

#[test]
fn birth_time_when_the_filesystem_has_one() {
    let d = TempDir::new("birth");
    d.write("a", b"x");
    let a = file(&scan_later(&d), "a").clone();
    let created = fs::metadata(d.path().join("a")).expect("meta").created();
    assert_eq!(
        a.id.birth_ns.is_some(),
        created.is_ok(),
        "birth time exactly when statx reports one"
    );
}

#[test]
fn a_file_just_written_is_not_settled() {
    let d = TempDir::new("settle");
    d.write("a", b"x");
    let s = scan(d.path(), SystemTime::now()).expect("scan");
    assert!(file(&s, "a").stable_for < Duration::from_secs(5));
    // A clock that is behind: the change looks like the future; just now.
    let s = scan(d.path(), SystemTime::now() - Duration::from_secs(3600)).expect("scan");
    assert_eq!(file(&s, "a").stable_for, Duration::ZERO);
}

#[test]
fn what_is_not_synced_is_reported() {
    let d = TempDir::new("skip");
    d.write("real.txt", b"x");
    std::os::unix::fs::symlink("/etc/passwd", d.path().join("link")).expect("symlink");
    std::os::unix::fs::symlink(d.path(), d.path().join("loop")).expect("symlink to folder");
    let bad = std::ffi::OsStr::from_bytes(b"bad-\xff-name");
    fs::write(d.path().join(bad), b"x").expect("non-UTF-8 name");
    let s = scan_later(&d);
    assert_eq!(s.files.len(), 1, "{s:?}");
    let mut skipped: Vec<(String, SkipReason)> =
        s.skipped.into_iter().map(|k| (k.path, k.reason)).collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        skipped,
        vec![
            ("bad-\u{fffd}-name".to_owned(), SkipReason::NotUtf8),
            ("link".to_owned(), SkipReason::Symlink),
            ("loop".to_owned(), SkipReason::Symlink),
        ]
    );
}

#[test]
fn a_missing_folder_is_an_error_not_an_empty_folder() {
    let d = TempDir::new("missing");
    assert_eq!(
        scan(&d.path().join("unplugged"), SystemTime::now()),
        Err(ScanError::Missing)
    );
    d.write("file", b"x");
    assert_eq!(
        scan(&d.path().join("file"), SystemTime::now()),
        Err(ScanError::NotADirectory)
    );
}

#[test]
fn an_unreadable_folder_stops_the_scan() {
    use std::os::unix::fs::PermissionsExt;
    let d = TempDir::new("unreadable");
    d.write("locked/secret", b"x");
    let locked = d.path().join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod");
    let result = scan(d.path(), SystemTime::now());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("chmod back");
    if fs::read_dir(&locked).is_ok() && result.is_ok() {
        // Running as root: permissions do not stop us; nothing to check.
        return;
    }
    assert_eq!(
        result,
        Err(ScanError::Unreadable {
            folder: "locked".into(),
            kind: std::io::ErrorKind::PermissionDenied
        }),
        "a folder we cannot read must not look empty (its files would look deleted)"
    );
}

// --- the scanner and the planner on a real filesystem ------------------------------

mod with_the_planner {
    use super::*;
    use ikeloud_safety::{Action, DEFAULT_SETTLE_TIME, Etag, RelPath};
    use ikeloud_sync::{BaseEntry, DocId, RemoteFile, States, build_plan};

    /// The common state and server as if everything scanned now was synced.
    fn synced(s: &Scan) -> (Vec<BaseEntry>, Vec<RemoteFile>) {
        let mut base = Vec::new();
        let mut remote = Vec::new();
        for (n, f) in s.files.iter().enumerate() {
            let doc = DocId::new(format!("d{n}"));
            let etag = Etag::new(format!("e{n}"));
            base.push(BaseEntry {
                path: f.path.clone(),
                local_path: f.path.clone(),
                local: Some(f.id),
                doc_id: doc.clone(),
                hash: f.hash.clone(),
                etag: etag.clone(),
            });
            remote.push(RemoteFile {
                path: f.path.clone(),
                doc_id: doc,
                etag,
                size: f.size,
            });
        }
        (base, remote)
    }

    fn plan_after(d: &TempDir, base: &[BaseEntry], remote: &[RemoteFile]) -> Vec<Action> {
        let states = States {
            local: scan_later(d).files,
            remote: remote.to_vec(),
            remote_folders: vec![],
            base: base.to_vec(),
        };
        build_plan(&states, DEFAULT_SETTLE_TIME, &|p: &RelPath, n| {
            RelPath::parse(format!("{} ({n})", p.as_str())).ok()
        })
        .actions
    }

    #[test]
    fn a_rename_is_a_move() {
        let d = TempDir::new("plan-rename");
        d.write("a.txt", b"one");
        let (base, remote) = synced(&scan_later(&d));
        fs::create_dir(d.path().join("archive")).expect("folder");
        fs::rename(d.path().join("a.txt"), d.path().join("archive/a.txt")).expect("rename");
        let actions = plan_after(&d, &base, &remote);
        assert!(
            actions.iter().any(|a| matches!(a,
                Action::MoveRemote { from, to, .. }
                    if from.as_str() == "a.txt" && to.as_str() == "archive/a.txt")),
            "{actions:#?}"
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::TrashRemote { .. } | Action::Upload { .. })),
            "never delete + upload: {actions:#?}"
        );
    }

    #[test]
    fn an_editor_save_is_an_edit() {
        let d = TempDir::new("plan-save");
        d.write("report.odt", b"one");
        let (base, remote) = synced(&scan_later(&d));
        d.write(".~report.odt.tmp", b"two");
        fs::rename(
            d.path().join(".~report.odt.tmp"),
            d.path().join("report.odt"),
        )
        .expect("rename over");
        let actions = plan_after(&d, &base, &remote);
        assert!(
            matches!(actions.as_slice(),
                [Action::Upload { remote, expected_etag: Some(_), .. }]
                    if remote.as_str() == "report.odt"),
            "{actions:#?}"
        );
    }

    #[test]
    fn a_deleted_file_is_trashed_even_when_new_files_appear() {
        let d = TempDir::new("plan-reuse");
        d.write("old.txt", b"old");
        d.write("keep.txt", b"keep");
        let (base, remote) = synced(&scan_later(&d));
        // Delete, then create files: if the filesystem hands one of them the
        // freed inode, that is the case of the independent review. Whether
        // it does is the filesystem's choice; the TLA+ model (SyncIdentity)
        // and the simulation cover inode reuse exhaustively.
        let old_ino = file(&scan_later(&d), "old.txt").id.inode;
        fs::remove_file(d.path().join("old.txt")).expect("delete");
        for n in 0..50 {
            let name = format!("new{n}.txt");
            d.write(&name, b"new");
            if file(&scan_later(&d), &name).id.inode == old_ino {
                break;
            }
        }
        let actions = plan_after(&d, &base, &remote);
        assert!(
            actions.iter().any(|a| matches!(a,
                Action::TrashRemote { path, .. } if path.as_str() == "old.txt")),
            "the deleted file goes to the iCloud trash: {actions:#?}"
        );
        assert!(
            !actions.iter().any(|a| matches!(a,
                Action::Upload { remote, expected_etag: Some(_), .. }
                    if remote.as_str() == "old.txt")),
            "nothing is uploaded over the deleted file's document: {actions:#?}"
        );
    }
}
