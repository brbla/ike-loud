//! Local actions on the synced folder, with their preconditions checked
//! right before acting (the contract of `ikeloud_sync::Effects`).
//!
//! - Nothing is ever overwritten that was not expected: a move or a new file
//!   is linked to its target, which fails if the target exists (a hard link
//!   never replaces), and only then the old name is removed.
//! - A file is deleted or replaced only if it is still the file the planner
//!   saw: same identity and same content (hashed again).
//! - Replaced content goes to the local history first; deleted files go to
//!   the system trash (freedesktop.org Trash specification).
//!
//! Between the check and the act there is a window of microseconds in which
//! a change can still slip in; the local history is the safety net for it.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ikeloud_safety::{ContentHash, LocalId, LocalVersion, RelPath};

use crate::scan::local_id;

/// Why a local action did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskError {
    /// A precondition no longer holds (the file changed, moved, or the
    /// target is taken). Nothing was changed.
    Changed,
    /// The disk refused (full, read-only, permissions, another filesystem).
    Io(io::ErrorKind),
}

impl From<io::Error> for DiskError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.kind())
    }
}

/// The synced folder and where replaced and deleted files go.
#[derive(Debug, Clone)]
pub struct LocalDisk {
    root: PathBuf,
    trash: PathBuf,
    history: PathBuf,
}

/// What [`LocalDisk::purge_history`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Saved versions removed.
    pub removed: usize,
    /// Saved versions that are due but could not be removed (tried again
    /// next time).
    pub failed: usize,
}

/// How long replaced versions are kept (spec: 30 days).
pub const HISTORY_KEPT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Unique part of temporary and history names within this process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

impl LocalDisk {
    /// `trash` is a freedesktop.org trash folder (with `files/` and `info/`)
    /// on the same filesystem as `root`; `history` keeps replaced versions.
    pub fn new(root: PathBuf, trash: PathBuf, history: PathBuf) -> Self {
        Self {
            root,
            trash,
            history,
        }
    }

    /// The synced folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn abs(&self, path: &RelPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    /// The file at `path`, if it is still the regular file `id`.
    fn check(&self, path: &RelPath, id: LocalId) -> Result<Metadata, DiskError> {
        match fs::symlink_metadata(self.abs(path)) {
            Ok(m) if m.file_type().is_file() && local_id(&m) == id => Ok(m),
            Ok(_) => Err(DiskError::Changed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(DiskError::Changed),
            Err(e) => Err(e.into()),
        }
    }

    /// The file at `path`, if it is still `expected`: same file, same content.
    fn check_version(&self, path: &RelPath, expected: &LocalVersion) -> Result<(), DiskError> {
        self.check(path, expected.id)?;
        let hash = ikeloud_hash::sha256_reader(&mut File::open(self.abs(path))?)?;
        if ContentHash::new(ikeloud_hash::hex(&hash)) == expected.hash {
            Ok(())
        } else {
            Err(DiskError::Changed)
        }
    }

    /// Opens the file at `path` for reading (an upload), if it is still the
    /// file `id`. The identity is checked on the opened file itself, so a
    /// file swapped in between the check and the open cannot be read.
    pub fn open(&self, path: &RelPath, id: LocalId) -> Result<File, DiskError> {
        let file = match File::open(self.abs(path)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(DiskError::Changed),
            Err(e) => return Err(e.into()),
        };
        let meta = file.metadata()?;
        if meta.file_type().is_file() && local_id(&meta) == id {
            Ok(file)
        } else {
            Err(DiskError::Changed)
        }
    }

    /// Creates the folder `path` (and its parents). A file in its place:
    /// [`DiskError::Changed`].
    pub fn create_folder(&self, path: &RelPath) -> Result<(), DiskError> {
        match fs::create_dir_all(self.abs(path)) {
            Ok(()) => match fs::symlink_metadata(self.abs(path)) {
                Ok(m) if m.file_type().is_dir() => Ok(()),
                _ => Err(DiskError::Changed),
            },
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(DiskError::Changed),
            Err(e) => Err(e.into()),
        }
    }

    /// Removes the folder `path` if it is empty (`rmdir`: anything in it,
    /// hidden or not synced, keeps it: [`DiskError::Changed`]). Already
    /// gone is fine; a symlink or file there is not touched.
    pub fn remove_empty_folder(&self, path: &RelPath) -> Result<(), DiskError> {
        match fs::symlink_metadata(self.abs(path)) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => return Err(DiskError::Changed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        match fs::remove_dir(self.abs(path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Err(DiskError::Changed),
            Err(e) => Err(e.into()),
        }
    }

    /// Renames or moves the file `id` from `from` to `to`; never over another
    /// file.
    pub fn move_file(&self, from: &RelPath, to: &RelPath, id: LocalId) -> Result<(), DiskError> {
        self.check(from, id)?;
        let target = self.abs(to);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        move_no_replace(&self.abs(from), &target)
    }

    /// Moves `path` to the system trash, if it is still `expected`.
    pub fn trash(
        &self,
        path: &RelPath,
        expected: &LocalVersion,
        now: SystemTime,
    ) -> Result<(), DiskError> {
        self.check_version(path, expected)?;
        let files = self.trash.join("files");
        let info = self.trash.join("info");
        fs::create_dir_all(&files)?;
        fs::create_dir_all(&info)?;
        // Reserve a free name by creating its info file exclusively (spec).
        let (name, mut info_file) = (0..1000)
            .find_map(|n| {
                let name = match n {
                    0 => path.name().to_owned(),
                    n => format!("{} ({n})", path.name()),
                };
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(info.join(format!("{name}.trashinfo")))
                    .ok()
                    .map(|f| (name, f))
            })
            .ok_or(DiskError::Io(io::ErrorKind::AlreadyExists))?;
        let original = self.abs(path);
        let written = writeln!(
            info_file,
            "[Trash Info]\nPath={}\nDeletionDate={}",
            percent_encode(&original),
            trash_date(now)
        );
        let moved = written
            .map_err(DiskError::from)
            .and_then(|()| move_no_replace(&original, &files.join(&name)));
        if moved.is_err() {
            let _ = fs::remove_file(info.join(format!("{name}.trashinfo")));
        }
        moved
    }

    /// Writes `content` to `path` (a download). With `replaces`, the file
    /// there must still be that version, which is first kept in the local
    /// history; without it, `path` must be free. A download with the very
    /// content of the file it replaces leaves that file alone. Returns the
    /// identity and content hash of the file.
    pub fn write(
        &self,
        path: &RelPath,
        content: &mut dyn Read,
        replaces: Option<&LocalVersion>,
        now: SystemTime,
    ) -> Result<(LocalId, ContentHash), DiskError> {
        let target = self.abs(path);
        let dir = target.parent().unwrap_or(&self.root).to_path_buf();
        fs::create_dir_all(&dir)?;
        // A temporary name the scanner and the planner ignore (*.part).
        let temp = dir.join(format!(".ikeloud-{}.part", unique()));
        let result = self.write_via(&temp, &target, path, content, replaces, now);
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn write_via(
        &self,
        temp: &Path,
        target: &Path,
        path: &RelPath,
        content: &mut dyn Read,
        replaces: Option<&LocalVersion>,
        now: SystemTime,
    ) -> Result<(LocalId, ContentHash), DiskError> {
        {
            let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
            io::copy(content, &mut file)?;
            file.sync_all()?;
        }
        let hash = ikeloud_hash::sha256_reader(&mut File::open(temp)?)?;
        match replaces {
            None => {
                move_no_replace(temp, target)?;
            }
            Some(old) if old.hash.as_str() == ikeloud_hash::hex(&hash) => {
                // The same content (e.g. iCloud changed the version of a
                // file it only renamed): the file stays as it is, with no
                // copy in history and no new inode.
                self.check_version(path, old)?;
                fs::remove_file(temp)?;
                return Ok((old.id, old.hash.clone()));
            }
            Some(old) => {
                self.check_version(path, old)?;
                self.keep_in_history(path, now)?;
                // rename replaces atomically: the target always holds either
                // the old or the new complete file.
                fs::rename(temp, target)?;
            }
        }
        let id = local_id(&fs::symlink_metadata(target)?);
        Ok((id, ContentHash::new(ikeloud_hash::hex(&hash))))
    }

    /// Removes versions from the local history that were saved more than
    /// `kept` before `now`.
    ///
    /// Each saved version is a folder `<seconds>-<unique>` named after the
    /// time it was saved (not the file's own times: a hard link keeps the
    /// modification time of the file it was saved from). Anything else in
    /// the history folder was not put there by ike loud and is left alone,
    /// as is a version saved "in the future" (a clock set back since). A
    /// history folder that does not exist yet has nothing to remove.
    ///
    /// # Errors
    /// The history folder exists but cannot be read.
    pub fn purge_history(&self, now: SystemTime, kept: Duration) -> io::Result<Purged> {
        let mut purged = Purged::default();
        let Some(cutoff) = now
            .checked_sub(kept)
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        else {
            return Ok(purged);
        };
        let entries = match fs::read_dir(&self.history) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(purged),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let Some(saved) = entry.file_name().to_str().and_then(saved_at) else {
                continue;
            };
            // Only our own folders, never through a symbolic link.
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if !is_dir || saved >= cutoff.as_secs() {
                continue;
            }
            match fs::remove_dir_all(entry.path()) {
                Ok(()) => purged.removed += 1,
                Err(_) => purged.failed += 1,
            }
        }
        Ok(purged)
    }

    /// Keeps a version read from the server in the local history, before an
    /// upload replaces it there (spec: before every overwrite, locally and
    /// on the server, the previous version is set aside for 30 days). Written
    /// completely and to disk, or not at all.
    pub fn keep_remote_version(
        &self,
        path: &RelPath,
        content: &mut dyn std::io::Read,
        now: SystemTime,
    ) -> Result<(), DiskError> {
        let secs = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let kept = self
            .history
            .join(format!("{secs}-{}", unique()))
            .join(path.as_str());
        if let Some(parent) = kept.parent() {
            fs::create_dir_all(parent)?;
        }
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&kept)?;
            std::io::copy(content, &mut file)?;
            file.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&kept);
        }
        Ok(result?)
    }

    /// Keeps the current version of `path` in the local history (a hard
    /// link, or a copy across filesystems).
    fn keep_in_history(&self, path: &RelPath, now: SystemTime) -> Result<(), DiskError> {
        let secs = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let kept = self
            .history
            .join(format!("{secs}-{}", unique()))
            .join(path.as_str());
        if let Some(parent) = kept.parent() {
            fs::create_dir_all(parent)?;
        }
        let source = self.abs(path);
        if fs::hard_link(&source, &kept).is_err() {
            fs::copy(&source, &kept)?;
        }
        Ok(())
    }
}

/// The time a history folder `<seconds>-<pid>-<n>` was saved, in seconds
/// since the Unix epoch; `None` for any other name.
fn saved_at(name: &str) -> Option<u64> {
    let mut parts = name.split('-');
    let secs = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits(secs) || rest.len() != 2 || !rest.iter().all(|p| digits(p)) {
        return None;
    }
    secs.parse().ok()
}

/// Moves `from` to `to` unless `to` exists. A hard link is created first
/// (it fails if the target exists, atomically), then the old name removed.
/// On filesystems without hard links: a rename after checking the target is
/// free.
fn move_no_replace(from: &Path, to: &Path) -> Result<(), DiskError> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(from) {
                let _ = fs::remove_file(to);
                return Err(e.into());
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(DiskError::Changed),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(DiskError::Changed),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => Err(e.into()),
        Err(_) => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(DiskError::Changed);
            }
            fs::rename(from, to).map_err(DiskError::from)
        }
    }
}

/// `Path=` of a .trashinfo file: the absolute path, percent-encoded except
/// for unreserved characters and `/` (freedesktop.org Trash specification).
fn percent_encode(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|&b| {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// `DeletionDate=` (YYYY-MM-DDThh:mm:ss). The specification asks for local
/// time; the standard library knows no time zones, so this is UTC: a file
/// manager may show the deletion a few hours off. Also the date in the name
/// of a conflict copy.
pub fn trash_date(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::{percent_encode, saved_at, trash_date};

    #[test]
    fn history_folder_names() {
        assert_eq!(saved_at("1700000000-42-7"), Some(1_700_000_000));
        for other in [
            "",
            "1700000000",
            "1700000000-42",
            "x-42-7",
            "17-42-7-1",
            "17--7",
            "+17-1-2",
            "notes.txt",
        ] {
            assert_eq!(saved_at(other), None, "{other}");
        }
    }
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn trash_dates() {
        let cases: &[(u64, &str)] = &[
            (0, "1970-01-01T00:00:00"),
            (951_782_400, "2000-02-29T00:00:00"),
            (1_794_700_800 + 3_723, "2026-11-15T01:02:03"),
            (4_107_542_399, "2100-02-28T23:59:59"),
        ];
        for (secs, text) in cases {
            assert_eq!(trash_date(UNIX_EPOCH + Duration::from_secs(*secs)), *text);
        }
    }

    #[test]
    fn trash_paths() {
        assert_eq!(
            percent_encode(Path::new("/home/a/iCloud/Příloha 1.pdf")),
            "/home/a/iCloud/P%C5%99%C3%ADloha%201.pdf"
        );
    }
}
