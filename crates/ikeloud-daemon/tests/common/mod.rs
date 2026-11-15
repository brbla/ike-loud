//! A fake iCloud and a test folder for sync runs.

#![allow(dead_code, clippy::expect_used, clippy::panic)] // shared test code

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ikeloud_daemon::{
    Download, Listing, Remote, RemoteError, RunOutcome, RunReport, SyncSetup, Written, sync_once,
};
use ikeloud_local::LocalDisk;
use ikeloud_safety::{
    Action, ApprovedAction, DEFAULT_SETTLE_TIME, Etag, MassChangeConfirmation, RelPath,
};
use ikeloud_state::StateDb;
use ikeloud_sync::names::key;
use ikeloud_sync::{DocId, RemoteFile, StopReason};

#[derive(Debug, Clone)]
pub struct FakeFile {
    pub doc: u64,
    pub etag: u64,
    pub content: Vec<u8>,
}

/// A fake iCloud: conditional writes (etag, free name), names that differ
/// only in letter case are the same name, uploads need an existing folder,
/// deletions go to "Recently Deleted". `offline_after` makes it go offline
/// after that many writes.
#[derive(Debug, Default)]
pub struct FakeICloud {
    pub files: BTreeMap<String, FakeFile>,
    pub folders: BTreeSet<String>,
    pub trash: Vec<(String, Vec<u8>)>,
    pub offline_after: Option<usize>,
    pub writes: usize,
    next: u64,
}

impl FakeICloud {
    pub fn put(&mut self, path: &str, content: &[u8]) {
        let (doc, etag) = (self.fresh(), self.fresh());
        self.ensure_folders(path);
        self.files.insert(
            path.into(),
            FakeFile {
                doc,
                etag,
                content: content.to_vec(),
            },
        );
    }

    pub fn edit(&mut self, path: &str, content: &[u8]) {
        let etag = self.fresh();
        let f = self.files.get_mut(path).expect("exists");
        f.content = content.to_vec();
        f.etag = etag;
    }

    pub fn delete(&mut self, path: &str) {
        let f = self.files.remove(path).expect("exists");
        self.trash.push((path.into(), f.content));
    }

    pub fn content(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(|f| f.content.as_slice())
    }

    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn ensure_folders(&mut self, path: &str) {
        let mut prefix = String::new();
        for part in path
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .expect("path")
            .1
        {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.folders.insert(prefix.clone());
        }
    }

    /// The file holding this name (letter case and normalization ignored).
    fn holder(&self, path: &str) -> Option<String> {
        self.files
            .keys()
            .chain(self.folders.iter())
            .find(|p| key(p) == key(path))
            .cloned()
    }

    fn parent_exists(&self, path: &RelPath) -> bool {
        let parent = path.parent();
        parent.is_empty() || self.folders.iter().any(|f| key(f) == key(parent))
    }

    fn create(&mut self, path: &RelPath, content: Vec<u8>) -> Result<Written, RemoteError> {
        if self.holder(path.as_str()).is_some() || !self.parent_exists(path) {
            return Err(RemoteError::Changed);
        }
        let (doc, etag) = (self.fresh(), self.fresh());
        self.files
            .insert(path.as_str().into(), FakeFile { doc, etag, content });
        Ok(written(doc, etag))
    }

    fn matches(&self, path: &RelPath, etag: &Etag) -> bool {
        self.files
            .get(path.as_str())
            .is_some_and(|f| f.etag.to_string() == etag.as_str())
    }
}

fn written(doc: u64, etag: u64) -> Written {
    Written {
        doc_id: DocId::new(doc.to_string()),
        etag: Etag::new(etag.to_string()),
    }
}

impl Remote for FakeICloud {
    fn list(&mut self) -> Result<Listing, StopReason> {
        if self.offline_after == Some(0) {
            return Err(StopReason::Offline);
        }
        Ok(Listing {
            files: self
                .files
                .iter()
                .map(|(p, f)| RemoteFile {
                    path: RelPath::parse(p.as_str()).expect("path"),
                    doc_id: DocId::new(f.doc.to_string()),
                    etag: Etag::new(f.etag.to_string()),
                    size: u64::try_from(f.content.len()).expect("size"),
                })
                .collect(),
            folders: self
                .folders
                .iter()
                .map(|p| RelPath::parse(p.as_str()).expect("path"))
                .collect(),
        })
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let f = self.files.get(path.as_str()).ok_or(RemoteError::Changed)?;
        Ok(Download {
            doc_id: DocId::new(f.doc.to_string()),
            etag: Etag::new(f.etag.to_string()),
            content: Box::new(Cursor::new(f.content.clone())),
        })
    }

    fn write(
        &mut self,
        action: ApprovedAction,
        content: Option<&mut dyn Read>,
    ) -> Result<Option<Written>, RemoteError> {
        match self.offline_after {
            Some(0) => return Err(RemoteError::Stop(StopReason::Offline)),
            Some(n) => self.offline_after = Some(n - 1),
            None => {}
        }
        self.writes += 1;
        let mut bytes = Vec::new();
        if let Some(c) = content {
            c.read_to_end(&mut bytes).expect("read local content");
        }
        match action.action() {
            Action::Upload {
                remote,
                expected_etag: Some(e),
                ..
            } => {
                if !self.matches(remote, e) {
                    return Err(RemoteError::Changed);
                }
                let etag = self.fresh();
                let f = self.files.get_mut(remote.as_str()).expect("matched");
                f.content = bytes;
                f.etag = etag;
                Ok(Some(written(f.doc, etag)))
            }
            Action::Upload { remote, .. } => self.create(remote, bytes).map(Some),
            Action::UploadConflictCopy { path, .. } => self.create(path, bytes).map(Some),
            Action::CreateRemoteFolder { path } => {
                if self.holder(path.as_str()).is_some() || !self.parent_exists(path) {
                    return Err(RemoteError::Changed);
                }
                self.folders.insert(path.as_str().into());
                Ok(None)
            }
            Action::TrashRemote {
                path,
                expected_etag,
            } => {
                if !self.matches(path, expected_etag) {
                    return Err(RemoteError::Changed);
                }
                self.delete(path.as_str());
                Ok(None)
            }
            Action::MoveRemote {
                from,
                to,
                expected_etag,
            } => {
                let holder = self.holder(to.as_str());
                if !self.matches(from, expected_etag)
                    || holder.is_some_and(|h| h != from.as_str())
                    || !self.parent_exists(to)
                {
                    return Err(RemoteError::Changed);
                }
                let f = self.files.remove(from.as_str()).expect("matched");
                self.files.insert(to.as_str().into(), f);
                Ok(None)
            }
            other => panic!("not a server write: {other:?}"),
        }
    }
}

/// A synced folder, its trash and history, a state database and a fake
/// iCloud, in a temporary directory.
pub struct Fixture {
    pub dir: PathBuf,
    pub db: StateDb,
    pub icloud: FakeICloud,
}

fn copy_name(path: &RelPath, n: u32) -> Option<RelPath> {
    path.sibling(&format!("{} (conflict {n})", path.name()))
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ikeloud-daemon-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["root", "trash", "history"] {
            fs::create_dir_all(dir.join(sub)).expect("folders");
        }
        Self {
            dir,
            db: StateDb::open_in_memory().expect("db"),
            icloud: FakeICloud::default(),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    pub fn write(&self, rel: &str, content: &[u8]) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("folders");
        fs::write(path, content).expect("write");
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        fs::read(self.root().join(rel)).ok()
    }

    pub fn remove(&self, rel: &str) {
        fs::remove_file(self.root().join(rel)).expect("remove");
    }

    /// Files under `sub` (root, trash, history), relative, sorted.
    pub fn files(&self, sub: &str) -> Vec<String> {
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

    pub fn run_with(&mut self, confirmation: Option<MassChangeConfirmation>) -> RunOutcome {
        let setup = SyncSetup {
            disk: LocalDisk::new(
                self.root(),
                self.dir.join("trash"),
                self.dir.join("history"),
            ),
            settle_time: DEFAULT_SETTLE_TIME,
            conflict_name: &copy_name,
        };
        // A minute from now: every file the test wrote counts as settled.
        let now = SystemTime::now() + Duration::from_secs(60);
        sync_once(&setup, &mut self.icloud, &mut self.db, confirmation, now).expect("state db")
    }

    pub fn run(&mut self) -> RunReport {
        match self.run_with(None) {
            RunOutcome::Ran(r) => r,
            other => panic!("the run did not run: {other:?}"),
        }
    }

    /// Runs until a run does nothing (at most 5 runs).
    pub fn settle(&mut self) {
        for _ in 0..5 {
            let r = self.run();
            if r.executed.done.is_empty() && r.executed.skipped.is_empty() {
                return;
            }
        }
        panic!("did not settle in 5 runs");
    }

    /// Local files and server files with their contents, compared.
    pub fn assert_in_sync(&self) {
        let local: BTreeMap<String, Vec<u8>> = self
            .files("root")
            .into_iter()
            .map(|p| {
                let c = self.read(&p).expect("read");
                (p, c)
            })
            .collect();
        let remote: BTreeMap<String, Vec<u8>> = self
            .icloud
            .files
            .iter()
            .map(|(p, f)| (p.clone(), f.content.clone()))
            .collect();
        assert_eq!(local, remote, "local and iCloud differ");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
