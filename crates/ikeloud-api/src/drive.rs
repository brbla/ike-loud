//! iCloud Drive: listing folders and downloading files. Read-only; writes
//! come later and will need an approval from `ikeloud-safety`.
//!
//! Observed on the real service (decision 0021):
//!
//! - an item has a stable id (`docwsid`), `parentId`, `etag`, the name
//!   without the extension and the extension apart;
//! - `dateModified` is the file's own modification time, not the upload time;
//! - etags have different shapes (`a`, `6`, `9::8`) and are compared only
//!   for equality;
//! - a download asks `docws` for a signed URL, then reads the content from
//!   `cvws.icloud-content.com`.

use std::io::Write;

use serde::Deserialize;
use serde_json::json;

use crate::client::{ApiError, Client};
use crate::http::{Method, Transport};

/// Drive id of the root folder.
pub const ROOT: &str = "FOLDER::com.apple.CloudDocs::root";

/// Kind of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Folder.
    Folder,
    /// File.
    File,
    /// App library or another kind the client does not sync.
    Other,
}

/// One item of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Drive id (`FILE::…` / `FOLDER::…`), for listing.
    pub drivewsid: String,
    /// Document id, stable across renames and moves; for downloading.
    pub docwsid: String,
    /// Kind.
    pub kind: Kind,
    /// Full name including the extension.
    pub name: String,
    /// Size in bytes (files only).
    pub size: Option<u64>,
    /// Version tag; compared for equality only.
    pub etag: String,
    /// Drive id of the parent folder.
    pub parent: Option<String>,
    /// Modification time as the server sends it (RFC 3339), files only.
    pub modified: Option<String>,
}

/// A listed folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Drive id.
    pub drivewsid: String,
    /// Name (empty for the root).
    pub name: String,
    /// Version tag of the folder.
    pub etag: String,
    /// Items.
    pub items: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawItem {
    drivewsid: String,
    docwsid: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    extension: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    etag: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    date_modified: Option<String>,
}

#[derive(Deserialize)]
struct RawFolder {
    drivewsid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    etag: String,
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Deserialize)]
struct DownloadInfo {
    data_token: Option<Token>,
    package_token: Option<Token>,
}

#[derive(Deserialize)]
struct Token {
    url: String,
}

impl From<RawItem> for Item {
    fn from(raw: RawItem) -> Self {
        let kind = match raw.kind.as_str() {
            "FOLDER" => Kind::Folder,
            "FILE" => Kind::File,
            _ => Kind::Other,
        };
        let name = match raw.extension.as_deref() {
            Some(ext) if !ext.is_empty() && kind == Kind::File => format!("{}.{ext}", raw.name),
            _ => raw.name,
        };
        Self {
            drivewsid: raw.drivewsid,
            docwsid: raw.docwsid,
            kind,
            name,
            size: raw.size,
            etag: raw.etag,
            parent: raw.parent_id,
            modified: raw.date_modified,
        }
    }
}

impl<T: Transport> Client<T> {
    /// Lists a folder (one level).
    pub fn list_folder(&mut self, drivewsid: &str) -> Result<Folder, ApiError> {
        let account = self.account().ok_or(ApiError::NotSignedIn)?.clone();
        let url = format!(
            "{}/retrieveItemDetailsInFolders?dsid={}",
            account.drivews, account.dsid
        );
        let body = json!([{"drivewsid": drivewsid, "partialData": false}]);
        let folders: Vec<RawFolder> = self.web_json(
            "retrieveItemDetailsInFolders",
            Method::Post,
            &url,
            Some(body),
        )?;
        let raw = folders
            .into_iter()
            .find(|f| f.drivewsid == drivewsid)
            .ok_or_else(|| ApiError::Malformed {
                call: "retrieveItemDetailsInFolders",
                what: "the folder is missing in the response".to_string(),
            })?;
        Ok(Folder {
            drivewsid: raw.drivewsid,
            name: raw.name,
            etag: raw.etag,
            items: raw.items.into_iter().map(Item::from).collect(),
        })
    }

    /// Downloads a file into `out`; returns the number of bytes written.
    pub fn download(&mut self, item: &Item, out: &mut dyn Write) -> Result<u64, ApiError> {
        let account = self.account().ok_or(ApiError::NotSignedIn)?.clone();
        let url = format!(
            "{}/ws/com.apple.CloudDocs/download/by_id?document_id={}&dsid={}",
            account.docws, item.docwsid, account.dsid
        );
        let info: DownloadInfo = self.web_json("download/by_id", Method::Get, &url, None)?;
        let token = info
            .data_token
            .or(info.package_token)
            .ok_or_else(|| ApiError::Malformed {
                call: "download/by_id",
                what: "no data token".to_string(),
            })?;
        let mut response = self.web_stream(&token.url)?;
        let written = std::io::copy(&mut response.body, out)
            .map_err(|e| ApiError::Transport(crate::http::TransportError::Io(e.to_string())))?;
        if let Some(size) = item.size
            && size != written
        {
            return Err(ApiError::Malformed {
                call: "download",
                what: format!("got {written} bytes, the listing says {size}"),
            });
        }
        Ok(written)
    }
}
