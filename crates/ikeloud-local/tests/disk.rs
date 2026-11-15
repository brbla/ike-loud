//! Tests of the local actions on real files: they act only when the file is
//! still what the planner saw, never overwrite anything unexpected, and keep
//! what they replace or delete.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)] // test code

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ikeloud_local::{DiskError, LocalDisk, scan};
use ikeloud_safety::{LocalVersion, RelPath};
use ikeloud_sync::LocalFile;

struct World {
    dir: PathBuf,
    disk: LocalDisk,
}

impl World {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ikeloud-disk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["root", "trash", "history"] {
            fs::create_dir_all(dir.join(sub)).expect("folders");
        }
        let disk = LocalDisk::new(dir.join("root"), dir.join("trash"), dir.join("history"));
        Self { dir, disk }
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    fn write(&self, rel: &str, content: &[u8]) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("folders");
        fs::write(path, content).expect("write");
    }

    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        fs::read(self.root().join(rel)).ok()
    }

    fn scanned(&self, rel: &str) -> LocalFile {
        scan(&self.root(), SystemTime::now() + Duration::from_secs(60))
            .expect("scan")
            .files
            .into_iter()
            .find(|f| f.path.as_str() == rel)
            .unwrap_or_else(|| panic!("{rel} not found"))
    }

    fn version(&self, rel: &str) -> LocalVersion {
        let f = self.scanned(rel);
        LocalVersion {
            id: f.id,
            hash: f.hash,
        }
    }

    fn files_in(&self, sub: &str) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for e in fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push(p.strip_prefix(base).expect("inside").display().to_string());
                }
            }
        }
        let base = self.dir.join(sub);
        let mut out = Vec::new();
        walk(&base, &base, &mut out);
        out.sort();
        out
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn p(s: &str) -> RelPath {
    RelPath::parse(s).expect("well-formed path")
}

const NOW: SystemTime = SystemTime::UNIX_EPOCH;

#[test]
fn move_keeps_the_file_and_never_overwrites() {
    let w = World::new("move");
    w.write("a.txt", b"a");
    w.write("taken.txt", b"other");
    let id = w.scanned("a.txt").id;

    assert_eq!(
        w.disk.move_file(&p("a.txt"), &p("taken.txt"), id),
        Err(DiskError::Changed),
        "never over another file"
    );
    assert_eq!(w.read("taken.txt").as_deref(), Some(&b"other"[..]));
    assert_eq!(w.read("a.txt").as_deref(), Some(&b"a"[..]));

    w.disk
        .move_file(&p("a.txt"), &p("new/folder/b.txt"), id)
        .expect("move into a new folder");
    assert_eq!(w.read("a.txt"), None);
    assert_eq!(w.read("new/folder/b.txt").as_deref(), Some(&b"a"[..]));
    assert_eq!(w.scanned("new/folder/b.txt").id, id, "the same file");

    assert_eq!(
        w.disk.move_file(&p("new/folder/b.txt"), &p("c.txt"), {
            let mut other = id;
            other.inode += 1;
            other
        }),
        Err(DiskError::Changed),
        "another file than the one planned"
    );
    assert_eq!(
        w.disk.move_file(&p("gone.txt"), &p("c.txt"), id),
        Err(DiskError::Changed)
    );
}

#[test]
fn trash_only_the_version_the_planner_saw() {
    let w = World::new("trash");
    w.write("Příloha 1.pdf", b"one");
    let seen = w.version("Příloha 1.pdf");

    w.write("Příloha 1.pdf", b"edited meanwhile");
    assert_eq!(
        w.disk.trash(&p("Příloha 1.pdf"), &seen, NOW),
        Err(DiskError::Changed),
        "an edit made after the plan is never thrown away"
    );
    assert_eq!(w.files_in("trash"), Vec::<String>::new());

    let seen = w.version("Příloha 1.pdf");
    w.disk
        .trash(&p("Příloha 1.pdf"), &seen, NOW)
        .expect("trash");
    assert_eq!(w.read("Příloha 1.pdf"), None);
    assert_eq!(
        w.files_in("trash"),
        vec!["files/Příloha 1.pdf", "info/Příloha 1.pdf.trashinfo"]
    );
    let info = fs::read_to_string(w.dir.join("trash/info/Příloha 1.pdf.trashinfo")).expect("info");
    assert!(info.starts_with("[Trash Info]\nPath=/"), "{info}");
    assert!(info.contains("P%C5%99%C3%ADloha%201.pdf\n"), "{info}");
    assert!(
        info.ends_with("DeletionDate=1970-01-01T00:00:00\n"),
        "{info}"
    );

    // A second file of the same name: the trash keeps both.
    w.write("Příloha 1.pdf", b"two");
    let seen = w.version("Příloha 1.pdf");
    w.disk
        .trash(&p("Příloha 1.pdf"), &seen, NOW)
        .expect("trash");
    assert_eq!(
        fs::read(w.dir.join("trash/files/Příloha 1.pdf (1)")).expect("second"),
        b"two"
    );
}

#[test]
fn a_download_to_a_free_place() {
    let w = World::new("download-new");
    let (id, hash) = w
        .disk
        .write(&p("d/new.txt"), &mut &b"abc"[..], None, NOW)
        .expect("write");
    assert_eq!(w.read("d/new.txt").as_deref(), Some(&b"abc"[..]));
    let scanned = w.scanned("d/new.txt");
    assert_eq!(
        (scanned.id, scanned.hash),
        (id, hash),
        "what the transfer reports"
    );
    assert_eq!(
        w.files_in("root"),
        vec!["d/new.txt"],
        "no temporary file left"
    );

    w.write("taken.txt", b"mine");
    assert_eq!(
        w.disk
            .write(&p("taken.txt"), &mut &b"theirs"[..], None, NOW),
        Err(DiskError::Changed),
        "a file appeared there meanwhile: never overwritten"
    );
    assert_eq!(w.read("taken.txt").as_deref(), Some(&b"mine"[..]));
    assert_eq!(w.files_in("root"), vec!["d/new.txt", "taken.txt"]);
}

#[test]
fn a_download_over_a_file_keeps_the_old_version() {
    let w = World::new("download-over");
    w.write("a.txt", b"old");
    let seen = w.version("a.txt");

    w.write("a.txt", b"edited meanwhile");
    assert_eq!(
        w.disk
            .write(&p("a.txt"), &mut &b"new"[..], Some(&seen), NOW),
        Err(DiskError::Changed),
        "an edit made after the plan is never overwritten"
    );
    assert_eq!(w.read("a.txt").as_deref(), Some(&b"edited meanwhile"[..]));
    assert_eq!(w.files_in("root"), vec!["a.txt"], "no temporary file left");

    let seen = w.version("a.txt");
    w.disk
        .write(&p("a.txt"), &mut &b"new"[..], Some(&seen), NOW)
        .expect("write");
    assert_eq!(w.read("a.txt").as_deref(), Some(&b"new"[..]));
    let kept = w.files_in("history");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].ends_with("/a.txt"), "{kept:?}");
    assert_eq!(
        fs::read(w.dir.join("history").join(&kept[0])).expect("kept"),
        b"edited meanwhile",
        "the replaced version is in the local history"
    );
}

#[test]
fn old_versions_leave_the_history_after_30_days() {
    use ikeloud_local::{HISTORY_KEPT, Purged};
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);
    let w = World::new("purge");
    let at = |days: u32| SystemTime::UNIX_EPOCH + DAY * days;
    let replace = |days, content: &[u8]| {
        let seen = w.version("a.txt");
        w.disk
            .write(&p("a.txt"), &mut &*content, Some(&seen), at(days))
            .expect("write");
    };
    w.write("a.txt", b"v1");
    replace(5, b"v2"); // v1 saved on day 5
    replace(20, b"v3"); // v2 saved on day 20
    replace(60, b"v4"); // v3 saved on day 60: "in the future" below
    // Not ours: left alone whatever their age.
    let history = w.dir.join("history");
    fs::create_dir_all(history.join("notes")).expect("mkdir");
    fs::write(history.join("1-2-3"), b"a file").expect("write");
    let outside = w.dir.join("outside");
    fs::create_dir_all(&outside).expect("mkdir");
    fs::write(outside.join("keep.txt"), b"keep").expect("write");
    std::os::unix::fs::symlink(&outside, history.join("1-2-4")).expect("symlink");

    assert_eq!(
        w.disk.purge_history(at(40), HISTORY_KEPT).expect("purge"),
        Purged {
            removed: 1,
            failed: 0
        }
    );
    let mut kept: Vec<Vec<u8>> = w
        .files_in("history")
        .iter()
        .filter(|f| f.ends_with("/a.txt"))
        .map(|f| fs::read(history.join(f)).expect("read"))
        .collect();
    kept.sort();
    assert_eq!(kept, [b"v2".to_vec(), b"v3".to_vec()], "v1 was 35 days old");
    assert!(history.join("notes").is_dir());
    assert!(history.join("1-2-3").is_file());
    assert_eq!(fs::read(outside.join("keep.txt")).expect("read"), b"keep");
    assert!(fs::symlink_metadata(history.join("1-2-4")).is_ok());
    assert_eq!(w.read("a.txt").as_deref(), Some(&b"v4"[..]));

    // Nothing is due twice; a clock before the epoch + 30 days removes nothing.
    assert_eq!(
        w.disk.purge_history(at(40), HISTORY_KEPT).expect("purge"),
        Purged::default()
    );
    assert_eq!(
        w.disk.purge_history(at(10), HISTORY_KEPT).expect("purge"),
        Purged::default()
    );
    let gone = LocalDisk::new(w.root(), w.dir.join("trash"), w.dir.join("no-history"));
    assert_eq!(
        gone.purge_history(at(40), HISTORY_KEPT).expect("purge"),
        Purged::default()
    );
}
