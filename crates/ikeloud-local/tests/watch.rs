//! The watcher on a real folder: hints arrive for changes in watched
//! folders, new folders are watched from the next `watch_all`.

#![allow(clippy::expect_used, clippy::panic)] // test code

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use ikeloud_local::{Coverage, Hint, Watcher};

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(200);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ikeloud-watch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub/deeper")).expect("temp dir");
    dir
}

#[test]
fn changes_anywhere_in_the_tree_are_hinted() {
    let root = temp("tree");
    let (mut watcher, mut watches) = Watcher::new(&root).expect("inotify");
    assert_eq!(
        watches.watch_all(),
        Coverage {
            folders: 3,
            failed: 0
        }
    );
    let (tx, rx) = mpsc::channel();
    // The watcher thread blocks in the kernel; it ends with the test process.
    std::thread::spawn(move || {
        while let Ok(hint) = watcher.wait() {
            if tx.send(hint).is_err() {
                break;
            }
        }
    });
    let drain = || while rx.recv_timeout(QUIET).is_ok() {};

    fs::write(root.join("sub/deeper/a.txt"), b"x").expect("write");
    assert_eq!(rx.recv_timeout(WAIT), Ok(Hint::Changed), "deep file");
    drain();

    fs::rename(root.join("sub/deeper/a.txt"), root.join("b.txt")).expect("mv");
    assert_eq!(rx.recv_timeout(WAIT), Ok(Hint::Changed), "move");
    drain();

    // A new folder: its creation is hinted (by its parent); what is written
    // into it before the next watch_all only the scan sees.
    fs::create_dir(root.join("new")).expect("mkdir");
    assert_eq!(rx.recv_timeout(WAIT), Ok(Hint::Changed), "new folder");
    drain();
    fs::write(root.join("new/early.txt"), b"x").expect("write");
    assert!(rx.recv_timeout(QUIET).is_err(), "not watched yet");
    assert_eq!(watches.watch_all().folders, 4);
    fs::write(root.join("new/late.txt"), b"x").expect("write");
    assert_eq!(rx.recv_timeout(WAIT), Ok(Hint::Changed), "watched now");
    drain();

    fs::remove_dir_all(root.join("sub")).expect("rm");
    assert_eq!(rx.recv_timeout(WAIT), Ok(Hint::Changed), "deleted tree");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_missing_root_is_not_watched() {
    let root = temp("missing").join("gone");
    let (_watcher, mut watches) = Watcher::new(&root).expect("inotify");
    assert_eq!(
        watches.watch_all(),
        Coverage {
            folders: 0,
            failed: 1
        }
    );
}
