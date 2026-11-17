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

use std::io::{ErrorKind, Read, Write};

use serde::Deserialize;
use serde_json::json;

use crate::client::{ApiError, Client};
use crate::http::{Method, Transport, TransportError};

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

    /// Opens a file for reading. The stream fails with
    /// [`std::io::ErrorKind::UnexpectedEof`] or [`std::io::ErrorKind::InvalidData`]
    /// when the content is shorter or longer than the listing says, so a
    /// broken transfer never looks like a complete file.
    pub fn open_download(&mut self, item: &Item) -> Result<Box<dyn Read + Send>, ApiError> {
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
        let response = self.web_stream(&token.url)?;
        Ok(match item.size {
            Some(size) => Box::new(Exact {
                inner: response.body,
                left: size,
            }),
            None => response.body,
        })
    }

    /// Downloads a file into `out`; returns the number of bytes written.
    pub fn download(&mut self, item: &Item, out: &mut dyn Write) -> Result<u64, ApiError> {
        let mut content = self.open_download(item)?;
        std::io::copy(&mut content, out).map_err(|e| match e.kind() {
            ErrorKind::UnexpectedEof | ErrorKind::InvalidData => ApiError::Malformed {
                call: "download",
                what: e.to_string(),
            },
            _ => ApiError::Transport(TransportError::Io(e.to_string())),
        })
    }
}

/// A reader that must yield exactly `left` more bytes.
struct Exact {
    inner: Box<dyn Read + Send>,
    left: u64,
}

impl Read for Exact {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        let got = u64::try_from(n).unwrap_or(u64::MAX);
        if n == 0 && self.left > 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                format!("the content ended {} bytes early", self.left),
            ));
        }
        if got > self.left {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "the content is longer than the listing says",
            ));
        }
        self.left -= got;
        Ok(n)
    }
}
