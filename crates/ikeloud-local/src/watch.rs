//! Change hints from the kernel (inotify).
//!
//! A hint only says "something in the folder changed, scan soon". What
//! changed is always decided by a full scan: inotify loses events when its
//! queue overflows, does not watch subfolders on its own, and misses changes
//! made while a folder was not watched yet (spec: inotify is a hint, a full
//! scan gives the truth). So the watcher keeps no paths and the order is:
//! [`Watches::watch_all`], then scan. Anything that changes after the first
//! is reported; anything before it, the scan sees.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchMask};

/// What the kernel reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// Something in a watched folder changed.
    Changed,
    /// Events were lost (the kernel's queue overflowed): something may have
    /// changed anywhere.
    Lost,
}

/// Waits for change hints of one folder tree. Usually lives in a thread of
/// its own; [`Watcher::watches`] adds folders from another one.
pub struct Watcher {
    inotify: Inotify,
    buffer: Vec<u8>,
}

/// Adds watches for a folder tree; can be used while [`Watcher::wait`]
/// blocks in another thread.
#[derive(Clone)]
pub struct Watches {
    watches: inotify::Watches,
    root: PathBuf,
}

/// How much of the folder tree is watched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Folders watched (the root included).
    pub folders: usize,
    /// Folders that could not be watched (usually the system limit of
    /// watches, `fs.inotify.max_user_watches`). Their changes show up at the
    /// next regular scan only.
    pub failed: usize,
}

const EVENTS: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF)
    .union(WatchMask::ONLYDIR)
    .union(WatchMask::DONT_FOLLOW);

impl Watcher {
    /// A watcher for the folder tree at `root`, with nothing watched yet
    /// (see [`Watches::watch_all`]).
    ///
    /// # Errors
    /// The kernel refused an inotify instance (limit of instances reached).
    pub fn new(root: &Path) -> io::Result<(Self, Watches)> {
        let inotify = Inotify::init()?;
        let watches = Watches {
            watches: inotify.watches(),
            root: root.to_path_buf(),
        };
        let watcher = Self {
            inotify,
            buffer: vec![0; 64 * 1024],
        };
        Ok((watcher, watches))
    }

    /// Blocks until the kernel reports a change. A removed watch (its folder
    /// was deleted) is no change by itself: its deletion is reported by the
    /// parent folder.
    ///
    /// # Errors
    /// Reading from the kernel failed; the watcher is then of no use.
    pub fn wait(&mut self) -> io::Result<Hint> {
        loop {
            let mut hint = None;
            for event in self.inotify.read_events_blocking(&mut self.buffer)? {
                if event.mask.contains(EventMask::Q_OVERFLOW) {
                    hint = Some(Hint::Lost);
                } else if event.mask != EventMask::IGNORED && hint.is_none() {
                    hint = Some(Hint::Changed);
                }
            }
            if let Some(hint) = hint {
                return Ok(hint);
            }
        }
    }
}

impl Watches {
    /// Watches the root and every folder below it; symbolic links are not
    /// followed. Watching a folder again is harmless, so this is simply
    /// called before every scan: folders created since are added, and a
    /// root that was gone and is back is watched again.
    pub fn watch_all(&mut self) -> Coverage {
        let mut coverage = Coverage::default();
        let root = self.root.clone();
        self.watch_tree(&root, &mut coverage);
        coverage
    }

    fn watch_tree(&mut self, dir: &Path, coverage: &mut Coverage) {
        if self.watches.add(dir, EVENTS).is_err() {
            // Gone meanwhile, unreadable or over the limit: the scan decides
            // which; for the hints it is a folder not watched.
            coverage.failed += 1;
            return;
        }
        coverage.folders += 1;
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                self.watch_tree(&entry.path(), coverage);
            }
        }
    }
}
