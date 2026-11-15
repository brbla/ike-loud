//! Scanning the synced folder.

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

/// Scans `root`. `now` is the time the scan counts stability against.
///
/// A file whose size, modification time or identity changes while it is
/// being hashed is reported as just changed (`stable_for` zero): its hash
/// may be of a half-written file, and the planner leaves such files alone.
pub fn scan(root: &Path, now: SystemTime) -> Result<Scan, ScanError> {
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
    let mut result = Scan::default();
    walk(root, "", now, &mut result)?;
    Ok(result)
}

fn walk(dir: &Path, rel: &str, now: SystemTime, out: &mut Scan) -> Result<(), ScanError> {
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
            out.skipped.push(skip(SkipReason::NotUtf8));
            continue;
        };
        let rel_path = join(rel, name);
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                out.skipped.push(skip(SkipReason::Unreadable(e.kind())));
                continue;
            }
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            out.skipped.push(skip(SkipReason::Symlink));
        } else if kind.is_dir() {
            walk(&path, &rel_path, now, out)?;
        } else if !kind.is_file() {
            out.skipped.push(skip(SkipReason::Special));
        } else {
            match RelPath::parse(rel_path) {
                Err(e) => out.skipped.push(skip(SkipReason::Malformed(e))),
                Ok(rel) => match describe(&path, rel, &meta, now) {
                    Ok(file) => out.files.push(file),
                    Err(e) => out.skipped.push(skip(SkipReason::Unreadable(e.kind()))),
                },
            }
        }
    }
    Ok(())
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// One regular file: hashed, then checked that it did not change meanwhile.
fn describe(
    path: &Path,
    rel: RelPath,
    before: &Metadata,
    now: SystemTime,
) -> io::Result<LocalFile> {
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
    Ok(LocalFile {
        path: rel,
        id: local_id(&after),
        hash: ContentHash::new(ikeloud_hash::hex(&hash)),
        size: after.len(),
        stable_for: if unchanged {
            stable_for(&after, now)
        } else {
            Duration::ZERO
        },
    })
}

/// How long the file has not changed: since its last content change
/// (mtime) or metadata change (ctime, e.g. a rename or a restored mtime),
/// whichever is later. A time in the future counts as just now.
fn stable_for(meta: &Metadata, now: SystemTime) -> Duration {
    let changed = [
        (meta.mtime(), meta.mtime_nsec()),
        (meta.ctime(), meta.ctime_nsec()),
    ]
    .into_iter()
    .filter_map(|(secs, nanos)| time_from_unix(secs, nanos))
    .max();
    changed
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or(Duration::ZERO)
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
