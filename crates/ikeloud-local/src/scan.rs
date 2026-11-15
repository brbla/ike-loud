//! Scanning the synced folder.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ikeloud_safety::{ContentHash, LocalId, PathError, RelPath};
use ikeloud_sync::LocalFile;

/// What a scan found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Regular files, in no particular order.
    pub files: Vec<LocalFile>,
    /// Entries that are not synced, with the reason; shown to the user.
    pub skipped: Vec<Skipped>,
    /// How many files were read to hash them (the others were unchanged
    /// since the scan that hashed them, see [`HashCache`]).
    pub hashed: usize,
}

/// Content hashes from earlier scans, so a file that has not changed is not
/// read again: without it every scan reads the whole folder, and the daemon
/// scans after every change.
///
/// A hash is reused only for the same file (device, inode, birth time) with
/// the same size, modification time and status change time (ctime), all to
/// the nanosecond. No program can set ctime; any write or rename moves it to
/// the current time. One hole stays: a write in the same clock tick as the
/// hashing leaves ctime as it was (filesystems keep coarse time stamps, FAT
/// even two-second ones). So a hash is trusted only if the file had not
/// changed for [`HashCache::margin`] when its hashing started; until then the
/// file is read again at every scan, as git does with "racily clean" files.
/// What is left is a clock set back by hand to the same nanosecond; the
/// daemon starts from an empty cache once a day.
#[derive(Debug)]
pub struct HashCache {
    margin: Duration,
    entries: BTreeMap<(u64, u64), Cached>,
}

#[derive(Debug, Clone)]
struct Cached {
    stamp: Stamp,
    hash: ContentHash,
    /// The file had been unchanged for the margin when hashing started.
    trusted: bool,
}

/// What must stay the same for a hash to be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    id: LocalId,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        Self {
            id: local_id(meta),
            size: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

impl Default for HashCache {
    fn default() -> Self {
        Self::new(Self::MARGIN)
    }
}

impl HashCache {
    /// The default margin: above the two-second time stamps of FAT.
    pub const MARGIN: Duration = Duration::from_secs(3);

    /// An empty cache that trusts a hash once the file had been unchanged
    /// for `margin` when it was hashed.
    #[must_use]
    pub fn new(margin: Duration) -> Self {
        Self {
            margin,
            entries: BTreeMap::new(),
        }
    }

    /// The margin, see the type.
    #[must_use]
    pub fn margin(&self) -> Duration {
        self.margin
    }

    /// Forgets every hash: the next scan reads every file.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of remembered hashes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no hash is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn reusable(&self, stamp: &Stamp) -> Option<&ContentHash> {
        self.entries
            .get(&(stamp.id.device, stamp.id.inode))
            .filter(|c| c.trusted && c.stamp == *stamp)
            .map(|c| &c.hash)
    }
}

/// An entry the scan did not take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Where it is, relative to the synced folder (lossy if the name is not
    /// UTF-8).
    pub path: String,
    /// Why.
    pub reason: SkipReason,
}

/// Why an entry is not synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// A symbolic link: never followed (it may point outside the folder).
    Symlink,
    /// A device, pipe or socket.
    Special,
    /// The name is not valid UTF-8; iCloud names are text.
    NotUtf8,
    /// The name cannot be a path (e.g. contains NUL; cannot happen on Linux).
    Malformed(PathError),
    /// The entry vanished or could not be read during the scan; the next
    /// scan tries again.
    Unreadable(io::ErrorKind),
}

/// Why the scan as a whole failed. Sync must stop: an unreadable folder must
/// never look like an empty one (spec: a missing folder deletes nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    /// The synced folder does not exist (an unplugged drive, a deleted folder).
    Missing,
    /// The synced folder is not a directory.
    NotADirectory,
    /// The synced folder itself or one of its folders could not be read.
    Unreadable {
        /// Relative path of the folder ("" for the root).
        folder: String,
        /// The error.
        kind: io::ErrorKind,
    },
}

/// Scans `root`, reading every file. `now` is the time the scan counts
/// stability against.
///
/// A file whose size, modification time or identity changes while it is
/// being hashed is reported as just changed (`stable_for` zero): its hash
/// may be of a half-written file, and the planner leaves such files alone.
pub fn scan(root: &Path, now: SystemTime) -> Result<Scan, ScanError> {
    scan_with(root, now, &mut HashCache::default())
}

/// Scans `root` like [`scan`], reading only files that changed since `cache`
/// learnt their hashes. After a complete scan the cache holds the hashes of
/// this scan; after a failed one it is left as it was.
pub fn scan_with(root: &Path, now: SystemTime, cache: &mut HashCache) -> Result<Scan, ScanError> {
    let meta = fs::symlink_metadata(root).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ScanError::Missing,
        kind => ScanError::Unreadable {
            folder: String::new(),
            kind,
        },
    })?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory);
    }
    let mut walker = Walker {
        now,
        old: cache,
        new: BTreeMap::new(),
        out: Scan::default(),
    };
    walker.walk(root, "")?;
    let Walker { new, out, .. } = walker;
    cache.entries = new;
    Ok(out)
}

struct Walker<'a> {
    now: SystemTime,
    old: &'a HashCache,
    new: BTreeMap<(u64, u64), Cached>,
    out: Scan,
}

impl Walker<'_> {
    fn walk(&mut self, dir: &Path, rel: &str) -> Result<(), ScanError> {
        let unreadable = |e: io::Error| ScanError::Unreadable {
            folder: rel.to_owned(),
            kind: e.kind(),
        };
        let mut entries: Vec<(PathBuf, std::ffi::OsString)> = Vec::new();
        for entry in fs::read_dir(dir).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            entries.push((entry.path(), entry.file_name()));
        }
        for (path, name) in entries {
            let skip = |reason| Skipped {
                path: join(rel, &name.to_string_lossy()),
                reason,
            };
            let Some(name) = name.to_str() else {
                self.out.skipped.push(skip(SkipReason::NotUtf8));
                continue;
            };
            let rel_path = join(rel, name);
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    self.out
                        .skipped
                        .push(skip(SkipReason::Unreadable(e.kind())));
                    continue;
                }
            };
            let kind = meta.file_type();
            if kind.is_symlink() {
                self.out.skipped.push(skip(SkipReason::Symlink));
            } else if kind.is_dir() {
                self.walk(&path, &rel_path)?;
            } else if !kind.is_file() {
                self.out.skipped.push(skip(SkipReason::Special));
            } else {
                match RelPath::parse(rel_path) {
                    Err(e) => self.out.skipped.push(skip(SkipReason::Malformed(e))),
                    Ok(rel) => match self.describe(&path, rel, &meta) {
                        Ok(file) => self.out.files.push(file),
                        Err(e) => self
                            .out
                            .skipped
                            .push(skip(SkipReason::Unreadable(e.kind()))),
                    },
                }
            }
        }
        Ok(())
    }

    /// One regular file, with its hash from the cache if it is unchanged.
    fn describe(&mut self, path: &Path, rel: RelPath, meta: &Metadata) -> io::Result<LocalFile> {
        let stamp = Stamp::of(meta);
        if let Some(hash) = self.old.reusable(&stamp) {
            let hash = hash.clone();
            self.new.insert(
                (stamp.id.device, stamp.id.inode),
                Cached {
                    stamp,
                    hash: hash.clone(),
                    trusted: true,
                },
            );
            return Ok(LocalFile {
                path: rel,
                id: stamp.id,
                hash,
                size: stamp.size,
                stable_for: stable_for(meta, self.now),
            });
        }
        let started = SystemTime::now();
        let (file, after) = describe(path, rel, meta, self.now)?;
        self.out.hashed += 1;
        // Only a file that did not change while it was read may be remembered,
        // and only trusted once it had been unchanged for the margin.
        if file.stable_for > Duration::ZERO {
            let stamp = Stamp::of(&after);
            let trusted = changed_at(&after)
                .and_then(|t| t.checked_add(self.old.margin))
                .is_some_and(|t| t < started);
            self.new.insert(
                (stamp.id.device, stamp.id.inode),
                Cached {
                    stamp,
                    hash: file.hash.clone(),
                    trusted,
                },
            );
        }
        Ok(file)
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// One regular file: hashed, then checked that it did not change meanwhile.
/// Also returns the metadata after hashing.
fn describe(
    path: &Path,
    rel: RelPath,
    before: &Metadata,
    now: SystemTime,
) -> io::Result<(LocalFile, Metadata)> {
    let mut file = File::open(path)?;
    let hash = ikeloud_hash::sha256_reader(&mut file)?;
    let after = file.metadata()?;
    let unchanged = (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
    ) == (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
    );
    let file = LocalFile {
        path: rel,
        id: local_id(&after),
        hash: ContentHash::new(ikeloud_hash::hex(&hash)),
        size: after.len(),
        stable_for: if unchanged {
            stable_for(&after, now)
        } else {
            Duration::ZERO
        },
    };
    Ok((file, after))
}

/// How long the file has not changed: since its last content change
/// (mtime) or metadata change (ctime, e.g. a rename or a restored mtime),
/// whichever is later. A time in the future counts as just now.
fn stable_for(meta: &Metadata, now: SystemTime) -> Duration {
    changed_at(meta)
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or(Duration::ZERO)
}

/// The last content or metadata change (the later of mtime and ctime).
fn changed_at(meta: &Metadata) -> Option<SystemTime> {
    [
        (meta.mtime(), meta.mtime_nsec()),
        (meta.ctime(), meta.ctime_nsec()),
    ]
    .into_iter()
    .filter_map(|(secs, nanos)| time_from_unix(secs, nanos))
    .max()
}

fn time_from_unix(secs: i64, nanos: i64) -> Option<SystemTime> {
    let nanos = u32::try_from(nanos).ok()?;
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(u64::try_from(secs).ok()?, nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))?
            .checked_add(Duration::from_nanos(u64::from(nanos)))
    }
}

/// The identity of a file from its metadata (shared with `disk.rs`, so the
/// scanner and the checks before acting can never disagree).
pub(crate) fn local_id(meta: &Metadata) -> LocalId {
    LocalId {
        device: meta.dev(),
        inode: meta.ino(),
        birth_ns: meta.created().ok().and_then(nanos_since_epoch),
    }
}

/// Birth time as nanoseconds since the Unix epoch (before it: negative).
fn nanos_since_epoch(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok(),
        Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
    }
}
