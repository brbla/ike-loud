//! [`Remote`] over the real iCloud Drive client (`ikeloud-api`). Read-only.
//!
//! Listing walks the whole tree folder by folder. Any failure fails the
//! whole listing: a partial listing would look like deleted files (spec:
//! an empty or odd listing is an API error, not deleted content). The same
//! holds for a name that is not a usable path component: one odd name stops
//! the run rather than hiding a file.

use std::collections::{BTreeMap, VecDeque};

use ikeloud_api::http::Transport;
use ikeloud_api::{ApiError, Client, Item, Kind, ROOT, SavedSession};
use ikeloud_safety::{ApprovedAction, Etag, RelPath};
use ikeloud_sync::{DocId, RemoteFile, StopReason};

use crate::remote::{Download, Listing, Remote, RemoteError, Written};

/// The real server.
pub struct ICloudRemote<T: Transport> {
    client: Client<T>,
    /// Items of the last listing, for downloads by path.
    items: BTreeMap<RelPath, Item>,
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
            on_session,
        }
    }

    fn saved(&mut self) {
        (self.on_session)(self.client.saved());
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
        let mut queue: VecDeque<(String, Option<RelPath>)> = VecDeque::new();
        queue.push_back((ROOT.to_string(), None));
        while let Some((id, path)) = queue.pop_front() {
            let folder = self.client.list_folder(&id);
            self.saved();
            let folder = folder.map_err(|e| stop_reason(&e))?;
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
        self.items = items;
        Ok(listing)
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        let item = self.items.get(path).cloned().ok_or(RemoteError::Changed)?;
        let content = self.client.open_download(&item);
        self.saved();
        let content = content.map_err(|e| match e {
            // The listing is older than the file: the next run sees why.
            ApiError::Status { status: 404, .. } => RemoteError::Changed,
            e => RemoteError::Stop(stop_reason(&e)),
        })?;
        Ok(Download {
            doc_id: DocId::new(item.docwsid),
            etag: Etag::new(item.etag),
            content,
        })
    }

    fn write(
        &mut self,
        _action: ApprovedAction,
        _content: Option<&mut dyn std::io::Read>,
    ) -> Result<Option<Written>, RemoteError> {
        // Milestone 1 is read-only: the daemon runs in Mode::ReadOnly and the
        // safety layer approves no server write. Should one come anyway,
        // stop and write nothing.
        Err(RemoteError::Stop(StopReason::UnknownResponse))
    }
}
