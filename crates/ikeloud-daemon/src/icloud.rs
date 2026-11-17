//! [`Remote`] over the real iCloud Drive client (`ikeloud-api`).
//!
//! Writes are the approved actions of the safety layer, each one checked
//! by version the way decision 0023 found the server allows: a refused
//! write ("name taken", "another version") changes nothing on the server
//! and is [`RemoteError::Changed`]; the next run plans again.
//!
//! Listing: from CloudKit's changes of the Drive zone with a sync token
//! (decision 0025: an unchanged zone costs one request, and iCloud slows
//! an account down after some 40 quick requests). When the account has no
//! CloudKit address or the tree from the changes is doubtful
//! ([`crate::tree`]), the run walks the folders instead, 25 per request.
//! Any failure fails the whole listing: a partial listing would look like
//! deleted files (spec: an empty or odd listing is an API error, not
//! deleted content). The same holds for a name that is not a usable path
//! component: one odd name stops the run rather than hiding a file.
//!
//! Independent transfers may run at once ([`Remote::fork`], decision 0025):
//! each fork has its own client with the same session and shares the maps
//! of folders and files; the session a fork ends with comes back to the
//! main remote, which saves it.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use ikeloud_api::http::Transport;
use ikeloud_api::{ApiError, Client, Item, Kind, ROOT, SavedSession, UploadTarget};
use ikeloud_safety::{Action, ApprovedAction, Etag, RelPath};
use ikeloud_sync::{DocId, RemoteFile, StopReason};

use crate::remote::{Download, Listing, Remote, RemoteError, Written};
use crate::tree::{CloudTree, Unusable};

/// Folders listed per request.
const LIST_BATCH: usize = 25;

/// A folder's ids and version.
#[derive(Debug, Clone)]
struct Folder {
    drivewsid: String,
    docwsid: String,
    etag: String,
}

/// What the last listing found, and what was written since.
#[derive(Debug)]
struct Maps {
    /// Files, by path.
    items: BTreeMap<RelPath, Item>,
    /// Folders, by path; "" is the root.
    folders: BTreeMap<String, Folder>,
}

impl Default for Maps {
    fn default() -> Self {
        Self {
            items: BTreeMap::new(),
            folders: root_only(),
        }
    }
}

fn root_only() -> BTreeMap<String, Folder> {
    let mut folders = BTreeMap::new();
    folders.insert(
        String::new(),
        Folder {
            drivewsid: ROOT.to_string(),
            docwsid: "root".to_string(),
            etag: String::new(),
        },
    );
    folders
}

/// A client and the shared maps: what the main remote and its forks have in
/// common.
struct Core<T: Transport> {
    client: Client<T>,
    maps: Arc<Mutex<Maps>>,
}

/// The real server.
pub struct ICloudRemote<T: Transport> {
    core: Core<T>,
    /// The tree from CloudKit's zone changes.
    tree: CloudTree,
    /// The session a fork ended with, newer than the main client's.
    latest: Arc<Mutex<Option<SavedSession>>>,
    /// Called whenever the session may have changed (new cookies), to save it.
    on_session: Box<dyn FnMut(&SavedSession)>,
}

/// A copy of the remote for one worker thread.
struct Forked<T: Transport> {
    core: Core<T>,
    latest: Arc<Mutex<Option<SavedSession>>>,
}

impl<T: Transport> std::fmt::Debug for ICloudRemote<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ICloudRemote").finish_non_exhaustive()
    }
}

fn lock(maps: &Mutex<Maps>) -> MutexGuard<'_, Maps> {
    // A worker that panicked while holding the lock left consistent maps
    // (every change is one insert or remove): keep going.
    maps.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl<T: Transport> ICloudRemote<T> {
    /// Remote over a signed-in client.
    pub fn new(client: Client<T>, on_session: Box<dyn FnMut(&SavedSession)>) -> Self {
        Self {
            core: Core {
                client,
                maps: Arc::new(Mutex::new(Maps::default())),
            },
            latest: Arc::new(Mutex::new(None)),
            on_session,
            tree: CloudTree::default(),
        }
    }

    fn saved(&mut self) {
        (self.on_session)(self.core.client.saved());
    }
}

impl<T: Transport + Clone> ICloudRemote<T> {
    /// Takes over the session a fork ended with (cookies may have rotated).
    fn catch_up(&mut self) {
        let newer = self.latest.lock().ok().and_then(|mut l| l.take());
        if let Some(session) = newer {
            let transport = self.core.client.transport().clone();
            self.core.client = Client::restore(transport, session);
            self.saved();
        }
    }
}

/// A refused write wrote nothing: the plan no longer fits.
fn write_error(error: &ApiError) -> RemoteError {
    match error {
        ApiError::NameTaken | ApiError::VersionChanged | ApiError::Rejected { .. } => {
            RemoteError::Changed
        }
        e => RemoteError::Stop(stop_reason(e)),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl<T: Transport> Core<T> {
    fn folder(&self, path: &str) -> Result<Folder, RemoteError> {
        lock(&self.maps)
            .folders
            .get(path)
            .cloned()
            .ok_or(RemoteError::Changed)
    }

    fn upload(
        &mut self,
        path: &RelPath,
        expected_etag: Option<&Etag>,
        size: u64,
        content: &mut dyn std::io::Read,
    ) -> Result<Option<Written>, RemoteError> {
        let parent = self.folder(path.parent())?;
        let target = match expected_etag {
            None => UploadTarget::New {
                folder_docwsid: parent.docwsid.clone(),
            },
            Some(expected) => {
                let docwsid = lock(&self.maps)
                    .items
                    .get(path)
                    .map(|i| i.docwsid.clone())
                    .ok_or(RemoteError::Changed)?;
                UploadTarget::Replace {
                    folder_docwsid: parent.docwsid.clone(),
                    docwsid,
                    etag: expected.as_str().to_string(),
                }
            }
        };
        let uploaded = self
            .client
            .upload(&target, path.name(), size, content, now_ms())
            .map_err(|e| write_error(&e))?;
        lock(&self.maps).items.insert(
            path.clone(),
            Item {
                drivewsid: format!("FILE::com.apple.CloudDocs::{}", uploaded.docwsid),
                docwsid: uploaded.docwsid.clone(),
                kind: Kind::File,
                name: path.name().to_string(),
                size: Some(size),
                etag: uploaded.etag.clone(),
                parent: Some(parent.drivewsid),
                modified: None,
            },
        );
        Ok(Some(Written {
            doc_id: DocId::new(uploaded.docwsid),
            etag: Etag::new(uploaded.etag),
        }))
    }

    /// Drive id and version of a file or folder at `path`.
    fn item_ids(&self, path: &RelPath) -> Result<(String, String), RemoteError> {
        if let Some(item) = lock(&self.maps).items.get(path) {
            return Ok((item.drivewsid.clone(), item.etag.clone()));
        }
        let folder = self.folder(path.as_str())?;
        Ok((folder.drivewsid, folder.etag))
    }

    fn move_remote(
        &mut self,
        from: &RelPath,
        to: &RelPath,
        expected: &Etag,
    ) -> Result<(), RemoteError> {
        let (drivewsid, _) = self.item_ids(from)?;
        let mut etag = expected.as_str().to_string();
        if from.parent() != to.parent() {
            let destination = self.folder(to.parent())?;
            etag = self
                .client
                .move_item(&drivewsid, &etag, &destination.drivewsid)
                .map_err(|e| write_error(&e))?;
        }
        if from.name() != to.name() {
            // The server does not check the version of a rename (0023):
            // check it here, from a fresh listing, just before.
            let parent = self.folder(to.parent())?;
            let listing = self
                .client
                .list_folder(&parent.drivewsid)
                .map_err(|e| write_error(&e))?;
            let current = listing
                .items
                .iter()
                .find(|i| i.drivewsid == drivewsid)
                .ok_or(RemoteError::Changed)?;
            if current.etag != etag {
                return Err(RemoteError::Changed);
            }
            self.client
                .rename(&drivewsid, &etag, to.name())
                .map_err(|e| write_error(&e))?;
        }
        let mut maps = lock(&self.maps);
        if let Some(mut item) = maps.items.remove(from) {
            item.name = to.name().to_string();
            maps.items.insert(to.clone(), item);
        }
        Ok(())
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let item = lock(&self.maps)
            .items
            .get(path)
            .cloned()
            .ok_or(RemoteError::Changed)?;
        let opened = self.client.open_download(&item).map_err(|e| match e {
            // The listing is older than the file: the next run sees why.
            ApiError::Status { status: 404, .. } => RemoteError::Changed,
            e => RemoteError::Stop(stop_reason(&e)),
        })?;
        // The version being read is the one the server reports now; when it
        // differs from the planned one, the effects write nothing.
        let etag = opened.etag.unwrap_or(item.etag);
        Ok(Download {
            doc_id: DocId::new(item.docwsid),
            etag: Etag::new(etag),
            content: opened.content,
        })
    }

    fn write(
        &mut self,
        approved: &ApprovedAction,
        content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        match approved.action() {
            Action::Upload {
                remote,
                expected_etag,
                size,
                ..
            } => match content {
                Some(content) => self.upload(remote, expected_etag.as_ref(), *size, content),
                None => Err(RemoteError::Stop(StopReason::LocalDisk)),
            },
            Action::UploadConflictCopy { path, size, .. } => match content {
                Some(content) => self.upload(path, None, *size, content),
                None => Err(RemoteError::Stop(StopReason::LocalDisk)),
            },
            Action::CreateRemoteFolder { path } => {
                let parent = self.folder(path.parent())?;
                let created = self
                    .client
                    .create_folder(&parent.drivewsid, path.name())
                    .map_err(|e| write_error(&e))?;
                lock(&self.maps).folders.insert(
                    path.as_str().to_string(),
                    Folder {
                        drivewsid: created.drivewsid,
                        docwsid: created.docwsid,
                        etag: created.etag,
                    },
                );
                Ok(None)
            }
            Action::TrashRemote {
                path,
                expected_etag,
            } => {
                let (drivewsid, _) = self.item_ids(path)?;
                self.client
                    .trash(&drivewsid, expected_etag.as_str())
                    .map_err(|e| write_error(&e))?;
                lock(&self.maps).items.remove(path);
                Ok(None)
            }
            Action::MoveRemote {
                from,
                to,
                expected_etag,
            } => self.move_remote(from, to, expected_etag).map(|()| None),
            // Local actions never reach the server.
            Action::Download { .. } | Action::TrashLocal { .. } | Action::MoveLocal { .. } => {
                Err(RemoteError::Stop(StopReason::UnknownResponse))
            }
        }
    }
}

/// What a failed call means for the run.
pub fn stop_reason(error: &ApiError) -> StopReason {
    match error {
        // "Try again later" (503 RETRY when too many writes at once, 429,
        // a gateway in between): like an outage, the run retries later with
        // a growing wait. Never "unknown response" (that would stop sync).
        ApiError::Transport(_)
        | ApiError::Status {
            status: 429 | 502 | 503 | 504,
            ..
        } => StopReason::Offline,
        ApiError::SessionExpired
        | ApiError::NotSignedIn
        | ApiError::BadCredentials
        | ApiError::BadCode
        | ApiError::TooManyCodes
        | ApiError::NoTrustedPhone => StopReason::SignedOut,
        ApiError::Srp(_)
        | ApiError::ServerProofMismatch
        | ApiError::DriveNotSetUp
        | ApiError::DriveUnavailable
        | ApiError::Status { .. }
        | ApiError::Malformed { .. } => StopReason::UnknownResponse,
        // Nothing was written; the item is as the next listing shows it.
        ApiError::NameTaken | ApiError::VersionChanged | ApiError::Rejected { .. } => {
            StopReason::UnknownResponse
        }
    }
}

fn child(parent: Option<&RelPath>, name: &str) -> Option<RelPath> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    match parent {
        None => RelPath::parse(name).ok(),
        Some(p) => RelPath::parse(format!("{}/{name}", p.as_str())).ok(),
    }
}

/// Why the changes could not give the listing.
enum NoChanges {
    /// Stop the run (offline, signed out, …).
    Stop(StopReason),
    /// List the folders instead (no CloudKit, a doubtful tree, an error).
    Walk(Unusable),
}

impl<T: Transport + Clone + Send + 'static> ICloudRemote<T> {
    /// The listing from the zone's changes since the last sync token.
    fn list_from_changes(&mut self) -> Result<Listing, NoChanges> {
        let has_cloudkit = self
            .core
            .client
            .account()
            .is_some_and(|a| a.ckdatabasews.is_some());
        if !has_cloudkit {
            return Err(NoChanges::Walk(Unusable("no CloudKit address".into())));
        }
        if self.tree.wants_full() {
            self.tree.begin_full();
        }
        loop {
            let page = self.core.client.zone_changes(self.tree.token.as_deref());
            self.saved();
            match page {
                Ok(page) => {
                    self.tree.apply(page.records);
                    self.tree.token = Some(page.sync_token);
                    if !page.more_coming {
                        break;
                    }
                }
                Err(e) => {
                    // Busy, offline, signed out: as for any listing. Anything
                    // else (an expired token, an odd answer): start over.
                    let reason = stop_reason(&e);
                    self.tree.reset();
                    return Err(match reason {
                        StopReason::Offline | StopReason::SignedOut => NoChanges::Stop(reason),
                        _ => NoChanges::Walk(Unusable(e.to_string())),
                    });
                }
            }
        }
        let built = self.tree.build().map_err(|e| {
            self.tree.reset();
            NoChanges::Walk(e)
        })?;
        let mut maps = Maps::default();
        maps.items.extend(built.items);
        for (path, ids) in built.folders {
            maps.folders.insert(
                path,
                Folder {
                    drivewsid: ids.drivewsid,
                    docwsid: ids.docwsid,
                    etag: ids.etag,
                },
            );
        }
        *lock(&self.core.maps) = maps;
        Ok(built.listing)
    }
}

impl<T: Transport + Clone + Send + 'static> ICloudRemote<T> {
    /// The listing by walking the folders, 25 per request.
    fn list_by_walking(&mut self) -> Result<Listing, StopReason> {
        let mut listing = Listing::default();
        let mut maps = Maps::default();
        let mut queue: VecDeque<(String, Option<RelPath>)> = VecDeque::new();
        queue.push_back((ROOT.to_string(), None));
        while !queue.is_empty() {
            // Up to LIST_BATCH folders per request: one request per folder
            // made a tree of hundreds of folders take many minutes to list.
            let batch: Vec<(String, Option<RelPath>)> =
                queue.drain(..queue.len().min(LIST_BATCH)).collect();
            let ids: Vec<String> = batch.iter().map(|(id, _)| id.clone()).collect();
            let listed = self.core.client.list_folders(&ids);
            self.saved();
            let listed = listed.map_err(|e| stop_reason(&e))?;
            for (folder, (_, path)) in listed.into_iter().zip(batch) {
                for item in folder.items {
                    if item.kind == Kind::Other {
                        // App libraries (Pages, Numbers…) are not part of the
                        // synced Drive folder.
                        continue;
                    }
                    let item_path =
                        child(path.as_ref(), &item.name).ok_or(StopReason::UnknownResponse)?;
                    match item.kind {
                        Kind::Folder => {
                            maps.folders.insert(
                                item_path.as_str().to_string(),
                                Folder {
                                    drivewsid: item.drivewsid.clone(),
                                    docwsid: item.docwsid.clone(),
                                    etag: item.etag.clone(),
                                },
                            );
                            listing.folders.push(item_path.clone());
                            queue.push_back((item.drivewsid.clone(), Some(item_path)));
                        }
                        Kind::File => {
                            listing.files.push(RemoteFile {
                                path: item_path.clone(),
                                doc_id: DocId::new(item.docwsid.clone()),
                                etag: Etag::new(item.etag.clone()),
                                size: item.size.unwrap_or(0),
                            });
                            maps.items.insert(item_path, item);
                        }
                        Kind::Other => {}
                    }
                }
            }
        }
        *lock(&self.core.maps) = maps;
        Ok(listing)
    }

    /// Diagnostics: the listing from CloudKit and by walking the folders,
    /// compared; the differences, one per line (empty: the same).
    #[doc(hidden)]
    pub fn compare_listings(&mut self) -> Result<Vec<String>, StopReason> {
        self.tree.reset();
        let from_changes = match self.list_from_changes() {
            Ok(listing) => listing,
            Err(NoChanges::Stop(reason)) => return Err(reason),
            Err(NoChanges::Walk(Unusable(why))) => {
                return Ok(vec![format!("CloudKit unusable: {why}")]);
            }
        };
        let walked = self.list_by_walking()?;
        let files = |l: &Listing| -> std::collections::BTreeSet<String> {
            l.files
                .iter()
                .map(|f| {
                    format!(
                        "file {} {} {} {}",
                        f.path.as_str(),
                        f.doc_id.as_str(),
                        f.etag.as_str(),
                        f.size
                    )
                })
                .chain(l.folders.iter().map(|p| format!("folder {}", p.as_str())))
                .collect()
        };
        let (a, b) = (files(&from_changes), files(&walked));
        let mut out: Vec<String> = a
            .difference(&b)
            .map(|x| format!("only CloudKit: {x}"))
            .collect();
        out.extend(b.difference(&a).map(|x| format!("only walk:     {x}")));
        out.push(format!(
            "CloudKit: {} files, {} folders; walk: {} files, {} folders",
            from_changes.files.len(),
            from_changes.folders.len(),
            walked.files.len(),
            walked.folders.len()
        ));
        Ok(out)
    }
}

impl<T: Transport + Clone + Send + 'static> Remote for ICloudRemote<T> {
    fn list(&mut self) -> Result<Listing, StopReason> {
        self.catch_up();
        match self.list_from_changes() {
            Ok(listing) => return Ok(listing),
            Err(NoChanges::Stop(reason)) => return Err(reason),
            Err(NoChanges::Walk(Unusable(why))) => {
                // For the service's journal (journalctl --user -u ikeloud).
                #[allow(clippy::print_stderr)]
                {
                    eprintln!("listing the folders instead of CloudKit changes: {why}");
                }
            }
        }
        self.list_by_walking()
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        self.catch_up();
        let result = self.core.download(path);
        self.saved();
        result
    }

    fn write(
        &mut self,
        approved: ApprovedAction,
        content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        self.catch_up();
        let result = self.core.write(&approved, content);
        self.saved();
        result
    }

    fn fork(&mut self) -> Option<Box<dyn Remote + Send>> {
        self.catch_up();
        let client = Client::restore(
            self.core.client.transport().clone(),
            self.core.client.saved().clone(),
        );
        Some(Box::new(Forked {
            core: Core {
                client,
                maps: Arc::clone(&self.core.maps),
            },
            latest: Arc::clone(&self.latest),
        }))
    }
}

impl<T: Transport + Send> Forked<T> {
    fn hand_back(&mut self) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(self.core.client.saved().clone());
        }
    }
}

impl<T: Transport + Send> Remote for Forked<T> {
    fn list(&mut self) -> Result<Listing, StopReason> {
        // Forks only carry out actions; listing is the main remote's.
        Err(StopReason::UnknownResponse)
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let result = self.core.download(path);
        self.hand_back();
        result
    }

    fn write(
        &mut self,
        approved: ApprovedAction,
        content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        let result = self.core.write(&approved, content);
        self.hand_back();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_server_is_retried_not_an_unknown_answer() {
        for status in [429, 502, 503, 504] {
            let e = ApiError::Status {
                call: "createFolders",
                status,
            };
            assert_eq!(stop_reason(&e), StopReason::Offline, "{status}");
            assert!(matches!(
                write_error(&e),
                RemoteError::Stop(StopReason::Offline)
            ));
        }
        let e = ApiError::Status {
            call: "x",
            status: 500,
        };
        assert_eq!(stop_reason(&e), StopReason::UnknownResponse);
    }
}
