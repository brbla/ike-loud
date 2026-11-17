//! Where ike loud keeps its files, and the two small files the daemon and
//! the GUI share:
//!
//! - the settings (`$XDG_CONFIG_HOME/ikeloud/settings`): the synced folder;
//! - the status (`$XDG_STATE_HOME/ikeloud/status`): what the daemon is doing,
//!   written by the daemon after every change and read by the GUI.
//!
//! Both are plain `key=value` lines, written atomically (temporary file,
//! `sync_all`, rename) with mode 0600. Unknown keys are ignored, so an older
//! GUI can read a newer daemon's status.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Files of one user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Settings.
    pub settings: PathBuf,
    /// The signed-in session (cookies, trust token; never the password).
    pub session: PathBuf,
    /// The sync state database.
    pub state_db: PathBuf,
    /// Daemon status for the GUI.
    pub status: PathBuf,
    /// A mass change waiting for the user's confirmation (daemon → GUI).
    pub pending: PathBuf,
    /// A request of the GUI to the daemon (confirm, sync now).
    pub request: PathBuf,
    /// Local history: replaced versions kept for 30 days.
    pub history: PathBuf,
    /// The user's trash (freedesktop.org), where local deletions go.
    pub trash: PathBuf,
}

impl Paths {
    /// Paths from the XDG variables, falling back to `$HOME`.
    pub fn from_env() -> io::Result<Self> {
        let var = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let home = var("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Self::new(
            &var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            &var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
            &var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
        ))
    }

    /// Paths under the given XDG base folders.
    pub fn new(config: &Path, state: &Path, data: &Path) -> Self {
        let config = config.join("ikeloud");
        let state = state.join("ikeloud");
        Self {
            settings: config.join("settings"),
            session: state.join("session.json"),
            state_db: state.join("state.sqlite"),
            status: state.join("status"),
            pending: state.join("pending-changes"),
            request: state.join("request"),
            history: data.join("ikeloud").join("history"),
            trash: data.join("Trash"),
        }
    }
}

/// Settings the user chose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The synced folder; `None` until the first setup is done.
    pub folder: Option<PathBuf>,
    /// Upload local changes to iCloud. Off by default while writing is in
    /// beta (decision 0023); even on, the daemon only reads until the first
    /// download is complete.
    pub upload: bool,
}

impl Settings {
    /// Reads the settings; a missing file means defaults.
    pub fn load(path: &Path) -> io::Result<Self> {
        let map = match fs::read_to_string(path) {
            Ok(text) => parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            folder: map
                .get("folder")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
            upload: map.get("upload").is_some_and(|v| v.trim() == "1"),
        })
    }

    /// Writes the settings.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        if let Some(folder) = &self.folder {
            let folder = folder.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "the folder name is not UTF-8")
            })?;
            if folder.contains('\n') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the folder name has a line break",
                ));
            }
            text.push_str(&format!("folder={folder}\n"));
        }
        if self.upload {
            text.push_str("upload=1\n");
        }
        write_private(path, text.as_bytes())
    }
}

/// What the daemon is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Starting or not running.
    Starting,
    /// Both sides agree.
    Idle,
    /// Listing or transferring.
    Syncing,
    /// A mass change waits for the user's confirmation.
    NeedsConfirmation,
    /// The sign-in expired; sign in again in the app.
    SignedOut,
    /// No connection; it retries on its own.
    Offline,
    /// Something needs attention (detail in the status).
    Error,
}

impl State {
    /// Name in the status file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Syncing => "syncing",
            Self::NeedsConfirmation => "needs-confirmation",
            Self::SignedOut => "signed-out",
            Self::Offline => "offline",
            Self::Error => "error",
        }
    }

    /// From the name in the status file.
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Starting,
            Self::Idle,
            Self::Syncing,
            Self::NeedsConfirmation,
            Self::SignedOut,
            Self::Offline,
            Self::Error,
        ]
        .into_iter()
        .find(|s| s.name() == name)
    }
}

/// The daemon status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// State.
    pub state: State,
    /// Short technical detail for the window (one line, may be empty).
    pub detail: String,
    /// Files on iCloud at the last listing.
    pub remote_files: u64,
    /// Files downloaded since the daemon started.
    pub downloaded: u64,
    /// Local changes that are not uploaded (read-only mode defers them).
    pub local_pending: u64,
    /// The daemon uploads (two-way sync); `false`: it only reads.
    pub writes: bool,
    /// Seconds since 1970 of the last write of the status.
    pub updated: u64,
    /// Process id of the daemon that wrote it.
    pub pid: u32,
}

impl Status {
    /// A status written now by this process.
    pub fn now(
        state: State,
        detail: impl Into<String>,
        remote_files: u64,
        downloaded: u64,
    ) -> Self {
        Self {
            state,
            detail: detail.into().replace(['\n', '\r'], " "),
            remote_files,
            downloaded,
            local_pending: 0,
            writes: false,
            updated: unix_now(),
            pid: std::process::id(),
        }
    }

    /// Writes the status file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = format!(
            "state={}\ndetail={}\nremote_files={}\ndownloaded={}\nlocal_pending={}\nwrites={}\nupdated={}\npid={}\n",
            self.state.name(),
            self.detail.replace(['\n', '\r'], " "),
            self.remote_files,
            self.downloaded,
            self.local_pending,
            u8::from(self.writes),
            self.updated,
            self.pid
        );
        write_private(path, text.as_bytes())
    }

    /// Reads the status file; `None` when there is none or it is unreadable.
    pub fn load(path: &Path) -> Option<Self> {
        let map = parse(&fs::read_to_string(path).ok()?);
        let number = |key: &str| {
            map.get(key)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        Some(Self {
            state: State::from_name(map.get("state")?)?,
            detail: map.get("detail").cloned().unwrap_or_default(),
            remote_files: number("remote_files"),
            downloaded: number("downloaded"),
            local_pending: number("local_pending"),
            writes: number("writes") == 1,
            updated: number("updated"),
            pid: map.get("pid").and_then(|v| v.parse().ok()).unwrap_or(0),
        })
    }
}

/// One change of a mass change, as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    /// What happens: `trash-remote`, `overwrite-remote`, `trash-local`,
    /// `overwrite-local`, `move-remote`, `move-local`.
    pub kind: String,
    /// The file.
    pub path: String,
    /// Where a move takes it.
    pub to: Option<String>,
}

/// A mass change the safety layer stopped, waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChanges {
    /// Identifies exactly this set of changes; a confirmation names it, so
    /// it never covers a set the user has not seen.
    pub id: String,
    /// Files deleted or overwritten in the brake window, this plan included.
    pub destructive: u64,
    /// Files moved in the brake window, this plan included.
    pub moves: u64,
    /// Files synced.
    pub total: u64,
    /// The changes of this plan.
    pub files: Vec<PendingChange>,
}

impl PendingChanges {
    /// Writes the file: a header of `key=value` lines, an empty line, then
    /// one change per line (`kind<TAB>path[<TAB>to]`).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut text = format!(
            "id={}\ndestructive={}\nmoves={}\ntotal={}\n\n",
            self.id, self.destructive, self.moves, self.total
        );
        for change in &self.files {
            let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
            text.push_str(&clean(&change.kind));
            text.push('\t');
            text.push_str(&clean(&change.path));
            if let Some(to) = &change.to {
                text.push('\t');
                text.push_str(&clean(to));
            }
            text.push('\n');
        }
        write_private(path, text.as_bytes())
    }

    /// Reads the file; `None` when there is none or it is unreadable.
    pub fn load(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let (head, body) = text.split_once("\n\n")?;
        let map = parse(head);
        let number = |key: &str| {
            map.get(key)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let files = body
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\t');
                Some(PendingChange {
                    kind: parts.next()?.to_string(),
                    path: parts.next()?.to_string(),
                    to: parts.next().map(str::to_string),
                })
            })
            .collect();
        Some(Self {
            id: map.get("id")?.clone(),
            destructive: number("destructive"),
            moves: number("moves"),
            total: number("total"),
            files,
        })
    }
}

/// What the GUI asks the daemon to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Carry out the mass change with this id.
    Confirm(String),
    /// Run now.
    SyncNow,
}

impl Request {
    /// Writes the request (replacing an earlier one not yet taken).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = match self {
            Self::Confirm(id) => format!("confirm={}\n", id.replace(['\n', '\r'], "")),
            Self::SyncNow => "sync-now\n".to_string(),
        };
        write_private(path, text.as_bytes())
    }

    /// Takes the request: reads and removes it. `None` when there is none.
    pub fn take(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let _ = fs::remove_file(path);
        let line = text.lines().next()?.trim();
        match line.split_once('=') {
            Some(("confirm", id)) if !id.is_empty() => Some(Self::Confirm(id.to_string())),
            _ if line == "sync-now" => Some(Self::SyncNow),
            _ => None,
        }
    }
}

/// Seconds since 1970.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .collect()
}

/// Writes a file only the user can read, atomically: a reader sees the old
/// or the new content, never a part.
pub fn write_private(path: &Path, content: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent folder"))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Why a folder cannot become the synced folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderProblem {
    /// Not an absolute path.
    NotAbsolute,
    /// It exists and is not empty (spec: never merge silently).
    NotEmpty,
    /// It exists and is not a folder.
    NotAFolder,
    /// It is inside the app's own data, or holds it.
    Overlaps(PathBuf),
    /// It cannot be read or created.
    Io(io::ErrorKind),
}

/// Checks a folder chosen as the synced folder: new or empty, absolute,
/// not overlapping the app's own files. Does not create it.
pub fn check_folder(folder: &Path, paths: &Paths) -> Result<(), FolderProblem> {
    if !folder.is_absolute() {
        return Err(FolderProblem::NotAbsolute);
    }
    let own = [
        paths.history.as_path(),
        paths.trash.as_path(),
        paths.state_db.parent().unwrap_or(Path::new("/")),
        paths.settings.parent().unwrap_or(Path::new("/")),
    ];
    for dir in own {
        if folder.starts_with(dir) || dir.starts_with(folder) {
            return Err(FolderProblem::Overlaps(dir.to_path_buf()));
        }
    }
    match fs::symlink_metadata(folder) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(FolderProblem::Io(e.kind())),
        Ok(meta) if !meta.is_dir() => Err(FolderProblem::NotAFolder),
        Ok(_) => {
            let mut entries = fs::read_dir(folder).map_err(|e| FolderProblem::Io(e.kind()))?;
            if entries.next().is_some() {
                Err(FolderProblem::NotEmpty)
            } else {
                Ok(())
            }
        }
    }
}

/// Whether a folder must never be emptied to make room for iCloud: the
/// root, the home folder, the user's standard folders (Documents, Desktop,
/// …) and everything that contains one of them. One wrong click in the folder
/// dialog must not send the user's documents to the trash.
pub fn protected_folder(folder: &Path, home: &Path, user_dirs: &[PathBuf]) -> bool {
    let folder = folder.components().collect::<PathBuf>();
    folder.parent().is_none()
        || home.starts_with(&folder)
        || user_dirs.iter().any(|dir| dir.starts_with(&folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ikeloud-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_round_trip() {
        let dir = temp("settings");
        let path = dir.join("a/settings");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let settings = Settings {
            folder: Some(PathBuf::from("/home/u/iCloud Drive")),
            upload: true,
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let mode = fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        // Relative paths are not accepted from the file.
        fs::write(&path, "folder=relative\nunknown=1\n").unwrap();
        assert_eq!(Settings::load(&path).unwrap().folder, None);
        let bad = Settings {
            folder: Some(PathBuf::from("/a\nb")),
            upload: false,
        };
        assert!(bad.save(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_round_trip() {
        let dir = temp("status");
        let path = dir.join("status");
        assert_eq!(Status::load(&path), None);
        let mut status = Status::now(State::Syncing, "two\nlines", 12, 3);
        status.local_pending = 4;
        status.writes = true;
        status.save(&path).unwrap();
        let read = Status::load(&path).unwrap();
        assert_eq!(read, status);
        assert_eq!(read.detail, "two lines");
        fs::write(&path, "state=dancing\n").unwrap();
        assert_eq!(Status::load(&path), None);
        fs::write(&path, "state=idle\nfuture=1\n").unwrap();
        assert_eq!(Status::load(&path).unwrap().state, State::Idle);
        for state in [
            State::Starting,
            State::Idle,
            State::Syncing,
            State::NeedsConfirmation,
            State::SignedOut,
            State::Offline,
            State::Error,
        ] {
            assert_eq!(State::from_name(state.name()), Some(state));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn folder_checks() {
        let dir = temp("folder");
        let paths = Paths::new(&dir.join("config"), &dir.join("state"), &dir.join("data"));
        assert_eq!(check_folder(&dir.join("new"), &paths), Ok(()));
        fs::create_dir(dir.join("empty")).unwrap();
        assert_eq!(check_folder(&dir.join("empty"), &paths), Ok(()));
        fs::create_dir(dir.join("full")).unwrap();
        fs::write(dir.join("full/x"), "x").unwrap();
        assert_eq!(
            check_folder(&dir.join("full"), &paths),
            Err(FolderProblem::NotEmpty)
        );
        assert_eq!(
            check_folder(&dir.join("full/x"), &paths),
            Err(FolderProblem::NotAFolder)
        );
        assert_eq!(
            check_folder(Path::new("rel"), &paths),
            Err(FolderProblem::NotAbsolute)
        );
        assert!(matches!(
            check_folder(&dir.join("data/Trash/files"), &paths),
            Err(FolderProblem::Overlaps(_))
        ));
        assert!(matches!(
            check_folder(&dir, &paths),
            Err(FolderProblem::Overlaps(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pending_and_requests_round_trip() {
        let dir = temp("pending");
        let path = dir.join("pending-changes");
        let pending = PendingChanges {
            id: "abc".to_string(),
            destructive: 51,
            moves: 2,
            total: 300,
            files: vec![
                PendingChange {
                    kind: "trash-remote".to_string(),
                    path: "a/b.txt".to_string(),
                    to: None,
                },
                PendingChange {
                    kind: "move-local".to_string(),
                    path: "x\ty".to_string(),
                    to: Some("z".to_string()),
                },
            ],
        };
        pending.save(&path).unwrap();
        let read = PendingChanges::load(&path).unwrap();
        assert_eq!(read.id, "abc");
        assert_eq!(read.destructive, 51);
        assert_eq!(read.files.len(), 2);
        assert_eq!(read.files[1].path, "x y", "a tab cannot split the line");
        assert_eq!(read.files[1].to.as_deref(), Some("z"));

        let request = dir.join("request");
        assert_eq!(Request::take(&request), None);
        Request::Confirm("abc".to_string()).save(&request).unwrap();
        assert_eq!(
            Request::take(&request),
            Some(Request::Confirm("abc".to_string()))
        );
        assert_eq!(Request::take(&request), None, "taken once");
        Request::SyncNow.save(&request).unwrap();
        assert_eq!(Request::take(&request), Some(Request::SyncNow));
        fs::write(&request, "confirm=\n").unwrap();
        assert_eq!(Request::take(&request), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn paths_layout() {
        let p = Paths::new(Path::new("/c"), Path::new("/s"), Path::new("/d"));
        assert_eq!(p.settings, Path::new("/c/ikeloud/settings"));
        assert_eq!(p.session, Path::new("/s/ikeloud/session.json"));
        assert_eq!(p.status, Path::new("/s/ikeloud/status"));
        assert_eq!(p.trash, Path::new("/d/Trash"));
    }

    #[test]
    fn protected_folders() {
        let home = Path::new("/home/u");
        let dirs = [
            PathBuf::from("/home/u/Documents"),
            PathBuf::from("/home/u/Desktop"),
        ];
        for p in [
            "/",
            "/home",
            "/home/u",
            "/home/u/",
            "/home/u/Documents",
            "/home/u/Desktop",
        ] {
            assert!(protected_folder(Path::new(p), home, &dirs), "{p}");
        }
        for p in [
            "/home/u/iCloud",
            "/home/u/Documents/iCloud",
            "/mnt/disk/icloud",
        ] {
            assert!(!protected_folder(Path::new(p), home, &dirs), "{p}");
        }
    }
}
