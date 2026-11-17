//! [`Remote`] over the real iCloud Drive client (`ikeloud-api`).
//!
//! Writes are the approved actions of the safety layer, each one checked
//! by version the way decision 0023 found the server allows: a refused
//! write ("name taken", "another version") changes nothing on the server
//! and is [`RemoteError::Changed`]; the next run plans again.
//!
//! Listing walks the whole tree folder by folder. Any failure fails the
//! whole listing: a partial listing would look like deleted files (spec:
//! an empty or odd listing is an API error, not deleted content). The same
//! holds for a name that is not a usable path component: one odd name stops
//! the run rather than hiding a file.

use std::collections::{BTreeMap, VecDeque};

use ikeloud_api::http::Transport;
use ikeloud_api::{ApiError, Client, Item, Kind, ROOT, SavedSession, UploadTarget};
use ikeloud_safety::{Action, ApprovedAction, Etag, RelPath};
use ikeloud_sync::{DocId, RemoteFile, StopReason};

use crate::remote::{Download, Listing, Remote, RemoteError, Written};

/// The real server.
pub struct ICloudRemote<T: Transport> {
    client: Client<T>,
    /// Files of the last listing (and written since), by path.
    items: BTreeMap<RelPath, Item>,
    /// Folders of the last listing (and created since), by path; "" is the
    /// root.
    folders: BTreeMap<String, Folder>,
    /// Called whenever the session may have changed (new cookies), to save it.
    on_session: Box<dyn FnMut(&SavedSession)>,
}

impl<T: Transport> std::fmt::Debug for ICloudRemote<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ICloudRemote")
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl<T: Transport> ICloudRemote<T> {
    /// Remote over a signed-in client.
    pub fn new(client: Client<T>, on_session: Box<dyn FnMut(&SavedSession)>) -> Self {
        Self {
            client,
            items: BTreeMap::new(),
            folders: root_only(),
            on_session,
        }
    }

    fn saved(&mut self) {
        (self.on_session)(self.client.saved());
    }
}

/// Folders listed per request.
const LIST_BATCH: usize = 25;

/// A folder's ids and version.
#[derive(Debug, Clone)]
struct Folder {
    drivewsid: String,
    docwsid: String,
    etag: String,
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

impl<T: Transport> ICloudRemote<T> {
    fn folder(&self, path: &str) -> Result<Folder, RemoteError> {
        self.folders.get(path).cloned().ok_or(RemoteError::Changed)
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
                let item = self.items.get(path).ok_or(RemoteError::Changed)?;
                UploadTarget::Replace {
                    folder_docwsid: parent.docwsid.clone(),
                    docwsid: item.docwsid.clone(),
                    etag: expected.as_str().to_string(),
                }
            }
        };
        let uploaded = self
            .client
            .upload(&target, path.name(), size, content, now_ms())
            .map_err(|e| write_error(&e))?;
        self.items.insert(
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
        if let Some(item) = self.items.get(path) {
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
        if let Some(mut item) = self.items.remove(from) {
            item.name = to.name().to_string();
            self.items.insert(to.clone(), item);
        }
        Ok(())
    }
}

/// What a failed call means for the run.
pub fn stop_reason(error: &ApiError) -> StopReason {
    match error {
        ApiError::Transport(_) => StopReason::Offline,
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

impl<T: Transport> Remote for ICloudRemote<T> {
    fn list(&mut self) -> Result<Listing, StopReason> {
        let mut listing = Listing::default();
        let mut items = BTreeMap::new();
        let mut folders = root_only();
        let mut queue: VecDeque<(String, Option<RelPath>)> = VecDeque::new();
        queue.push_back((ROOT.to_string(), None));
        while !queue.is_empty() {
            // Up to LIST_BATCH folders per request: one request per folder
            // made a tree of hundreds of folders take many minutes to list.
            let batch: Vec<(String, Option<RelPath>)> =
                queue.drain(..queue.len().min(LIST_BATCH)).collect();
            let ids: Vec<String> = batch.iter().map(|(id, _)| id.clone()).collect();
            let listed = self.client.list_folders(&ids);
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
                            folders.insert(
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
                            items.insert(item_path, item);
                        }
                        Kind::Other => {}
                    }
                }
            }
        }
        self.items = items;
        self.folders = folders;
        Ok(listing)
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let item = self.items.get(path).cloned().ok_or(RemoteError::Changed)?;
        let opened = self.client.open_download(&item);
        self.saved();
        let opened = opened.map_err(|e| match e {
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
        approved: ApprovedAction,
        content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        let result = match approved.action() {
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
            Action::CreateRemoteFolder { path } => match self.folder(path.parent()) {
                Ok(parent) => self
                    .client
                    .create_folder(&parent.drivewsid, path.name())
                    .map(|created| {
                        self.folders.insert(
                            path.as_str().to_string(),
                            Folder {
                                drivewsid: created.drivewsid,
                                docwsid: created.docwsid,
                                etag: created.etag,
                            },
                        );
                        None
                    })
                    .map_err(|e| write_error(&e)),
                Err(e) => Err(e),
            },
            Action::TrashRemote {
                path,
                expected_etag,
            } => self.item_ids(path).and_then(|(drivewsid, _)| {
                self.client
                    .trash(&drivewsid, expected_etag.as_str())
                    .map(|()| {
                        self.items.remove(path);
                        None
                    })
                    .map_err(|e| write_error(&e))
            }),
            Action::MoveRemote {
                from,
                to,
                expected_etag,
            } => self.move_remote(from, to, expected_etag).map(|()| None),
            // Local actions never reach the server.
            Action::Download { .. } | Action::TrashLocal { .. } | Action::MoveLocal { .. } => {
                Err(RemoteError::Stop(StopReason::UnknownResponse))
            }
        };
        self.saved();
        result
    }
}
