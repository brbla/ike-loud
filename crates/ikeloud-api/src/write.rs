//! Writes to iCloud Drive. Each one is conditional, as decision 0023 found
//! the server allows:
//!
//! - a new file never replaces anything (`allow_conflict: false`; a taken
//!   name is [`ApiError::NameTaken`]);
//! - a replacement names the version it replaces (`modify_file` with the
//!   etag; another version is [`ApiError::VersionChanged`]); there is no
//!   way to replace without one;
//! - trash and move carry the etag (`ETAG_CONFLICT` is
//!   [`ApiError::VersionChanged`]);
//! - rename: the server does not check the etag, and on a taken name it
//!   picks another one itself. [`Client::rename`] reports that as
//!   [`ApiError::NameTaken`]; the caller checks the version just before.
//!
//! Nothing here decides *whether* to write: that is `ikeloud-safety`'s job,
//! and the daemon calls these only with an approved action.

use std::io::Read;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{ApiError, Client, parse_json, web_headers};
use crate::http::{Body, Method, Transport};

/// Where an upload goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadTarget {
    /// A new file in the folder; fails if the name is taken.
    New {
        /// `docwsid` of the folder (`root` for the root).
        folder_docwsid: String,
    },
    /// A new version of an existing document; fails unless the server still
    /// holds version `etag`.
    Replace {
        /// `docwsid` of the folder the document is in.
        folder_docwsid: String,
        /// The document.
        docwsid: String,
        /// The version it replaces.
        etag: String,
    },
}

/// What an upload produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uploaded {
    /// The document.
    pub docwsid: String,
    /// Its new version.
    pub etag: String,
}

/// A folder created on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedFolder {
    /// Drive id (`FOLDER::…`).
    pub drivewsid: String,
    /// Document id.
    pub docwsid: String,
    /// Version.
    pub etag: String,
}

#[derive(Deserialize)]
struct UploadSlot {
    document_id: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SingleFile {
    receipt: Option<String>,
    reference_checksum: Option<String>,
    file_checksum: Option<String>,
    wrapping_key: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadReceipt {
    single_file: SingleFile,
}

#[derive(Deserialize)]
struct ItemResult {
    #[serde(default)]
    status: String,
    #[serde(default)]
    drivewsid: String,
    #[serde(default)]
    docwsid: String,
    #[serde(default)]
    etag: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    extension: Option<String>,
}

/// Per-item status of the `drivews` calls.
fn item_status(item: &ItemResult, call: &'static str) -> Result<(), ApiError> {
    match item.status.as_str() {
        "OK" => Ok(()),
        "ETAG_CONFLICT" => Err(ApiError::VersionChanged),
        other => Err(ApiError::Rejected {
            call,
            status: other.to_string(),
        }),
    }
}

fn first_item(value: &Value, key: &str, call: &'static str) -> Result<ItemResult, ApiError> {
    value
        .get(key)
        .and_then(|items| items.get(0))
        .cloned()
        .and_then(|item| serde_json::from_value(item).ok())
        .ok_or_else(|| ApiError::Malformed {
            call,
            what: format!("no {key}[0]"),
        })
}

fn full_name(item: &ItemResult) -> String {
    match item.extension.as_deref() {
        Some(ext) if !ext.is_empty() => format!("{}.{ext}", item.name),
        _ => item.name.clone(),
    }
}

/// The 409 of `update/documents` names the reason in `error_code`.
fn update_conflict(body: &Value) -> ApiError {
    match body.get("error_code").and_then(Value::as_str) {
        Some("UNIQUE_FIELD_FAILURE") => ApiError::NameTaken,
        Some("OP_LOCK_FAILURE") => ApiError::VersionChanged,
        other => ApiError::Rejected {
            call: "update/documents",
            status: other.unwrap_or("409").to_string(),
        },
    }
}

impl<T: Transport> Client<T> {
    fn drivews_url(&self, path: &str) -> Result<String, ApiError> {
        let account = self.account().ok_or(ApiError::NotSignedIn)?;
        Ok(format!("{}/{path}?dsid={}", account.drivews, account.dsid))
    }

    fn docws_url(&self, path: &str) -> Result<String, ApiError> {
        let account = self.account().ok_or(ApiError::NotSignedIn)?;
        Ok(format!(
            "{}/ws/com.apple.CloudDocs/{path}?dsid={}",
            account.docws, account.dsid
        ))
    }

    /// Creates folder `name` in the folder `parent` (its drive id).
    pub fn create_folder(&mut self, parent: &str, name: &str) -> Result<CreatedFolder, ApiError> {
        let url = self.drivews_url("createFolders")?;
        let body = json!({
            "destinationDrivewsId": parent,
            "folders": [{"clientId": format!("FOLDER::UNKNOWN_ZONE::TempId-{}", crate::client::random_hex(16)), "name": name}],
        });
        let value: Value = self.web_json("createFolders", Method::Post, &url, Some(body))?;
        let item = first_item(&value, "folders", "createFolders")?;
        item_status(&item, "createFolders")?;
        Ok(CreatedFolder {
            drivewsid: item.drivewsid,
            docwsid: item.docwsid,
            etag: item.etag,
        })
    }

    /// Moves an item (file or folder) to "Recently Deleted", if it is still
    /// at version `etag`.
    pub fn trash(&mut self, drivewsid: &str, etag: &str) -> Result<(), ApiError> {
        let url = self.drivews_url("moveItemsToTrash")?;
        let body =
            json!({"items": [{"drivewsid": drivewsid, "etag": etag, "clientId": drivewsid}]});
        let value: Value = self.web_json("moveItemsToTrash", Method::Post, &url, Some(body))?;
        let item = first_item(&value, "items", "moveItemsToTrash")?;
        item_status(&item, "moveItemsToTrash")
    }

    /// Moves an item into folder `destination` (drive id), if it is still at
    /// version `etag`. Returns the new version.
    pub fn move_item(
        &mut self,
        drivewsid: &str,
        etag: &str,
        destination: &str,
    ) -> Result<String, ApiError> {
        let url = self.drivews_url("moveItems")?;
        let body = json!({
            "destinationDrivewsId": destination,
            "items": [{"drivewsid": drivewsid, "etag": etag, "clientId": drivewsid}],
        });
        let value: Value = self.web_json("moveItems", Method::Post, &url, Some(body))?;
        let item = first_item(&value, "items", "moveItems")?;
        item_status(&item, "moveItems")?;
        Ok(item.etag)
    }

    /// Renames an item in its folder. The server does not check `etag`
    /// (decision 0023): the caller checks the version just before. When the
    /// server picked another name (the asked one was taken), the result is
    /// [`ApiError::NameTaken`] and the item has that other name.
    pub fn rename(&mut self, drivewsid: &str, etag: &str, name: &str) -> Result<String, ApiError> {
        let url = self.drivews_url("renameItems")?;
        let body = json!({"items": [{"drivewsid": drivewsid, "etag": etag, "name": name}]});
        let value: Value = self.web_json("renameItems", Method::Post, &url, Some(body))?;
        let item = first_item(&value, "items", "renameItems")?;
        item_status(&item, "renameItems")?;
        if full_name(&item) != name {
            return Err(ApiError::NameTaken);
        }
        Ok(item.etag)
    }

    /// Uploads `len` bytes from `content` as file `name`. `mtime_ms` is the
    /// file's modification time (milliseconds since 1970).
    pub fn upload(
        &mut self,
        target: &UploadTarget,
        name: &str,
        len: u64,
        content: &mut dyn Read,
        mtime_ms: u64,
    ) -> Result<Uploaded, ApiError> {
        // 1. A slot for the content.
        let url = self.docws_url("upload/web")?;
        let request = json!({
            "filename": name,
            "type": "FILE",
            "size": len.to_string(),
            "content_type": "application/octet-stream",
        });
        let slots: Vec<UploadSlot> =
            self.web_json("upload/web", Method::Post, &url, Some(request))?;
        let slot = slots
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::Malformed {
                call: "upload/web",
                what: "no upload slot".to_string(),
            })?;

        // 2. The content, streamed as multipart/form-data.
        let boundary = format!("----ikeloud{}", crate::client::random_hex(12));
        let safe_name = name.replace(['"', '\r', '\n'], "_");
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{safe_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let total = u64::try_from(head.len())
            .ok()
            .and_then(|h| h.checked_add(len))
            .and_then(|t| t.checked_add(u64::try_from(tail.len()).ok()?))
            .ok_or_else(|| ApiError::Malformed {
                call: "upload",
                what: "file too large".to_string(),
            })?;
        let mut body = std::io::Cursor::new(head.into_bytes())
            .chain(Exactly::new(content, len))
            .chain(std::io::Cursor::new(tail.into_bytes()));
        let mut headers = web_headers();
        headers.retain(|(k, _)| k != "Content-Type");
        headers.push((
            "Content-Type".to_string(),
            format!("multipart/form-data; boundary={boundary}"),
        ));
        let mut response = self.send_body(
            Method::Post,
            &slot.url,
            headers,
            Body::Stream {
                reader: &mut body,
                len: total,
            },
        )?;
        match response.status {
            200 => {}
            401 | 421 | 450 => return Err(ApiError::SessionExpired),
            status => {
                return Err(ApiError::Status {
                    call: "upload",
                    status,
                });
            }
        }
        let receipt: UploadReceipt = parse_json(&mut response, "upload")?;
        let file = receipt.single_file;
        if file.size.is_some_and(|size| size != len) {
            return Err(ApiError::Malformed {
                call: "upload",
                what: "the server got another size".to_string(),
            });
        }

        // 3. The document: a new one, or a new version of the given one.
        let (command, document_id, folder, etag) = match target {
            UploadTarget::New { folder_docwsid } => {
                ("add_file", slot.document_id, folder_docwsid, None)
            }
            UploadTarget::Replace {
                folder_docwsid,
                docwsid,
                etag,
            } => ("modify_file", docwsid.clone(), folder_docwsid, Some(etag)),
        };
        let mut update = json!({
            "allow_conflict": false,
            "btime": mtime_ms,
            "mtime": mtime_ms,
            "command": command,
            "create_short_guid": true,
            "data": {
                "receipt": file.receipt,
                "reference_signature": file.reference_checksum,
                "signature": file.file_checksum,
                "size": len,
                "wrapping_key": file.wrapping_key,
            },
            "document_id": document_id,
            "file_flags": {"is_executable": false, "is_hidden": false, "is_writable": true},
            "path": {"path": name, "starting_document_id": folder},
        });
        if let (Some(etag), Some(map)) = (etag, update.as_object_mut()) {
            map.insert("etag".to_string(), json!(etag));
        }
        let url = self.docws_url("update/documents")?;
        let mut response = self.send_body(
            Method::Post,
            &url,
            web_headers(),
            Body::Bytes(update.to_string().into_bytes()),
        )?;
        match response.status {
            200 => {}
            409 => {
                let body: Value = parse_json(&mut response, "update/documents")?;
                return Err(update_conflict(&body));
            }
            401 | 421 | 450 => return Err(ApiError::SessionExpired),
            status => {
                return Err(ApiError::Status {
                    call: "update/documents",
                    status,
                });
            }
        }
        let value: Value = parse_json(&mut response, "update/documents")?;
        let document = value
            .get("results")
            .and_then(|r| r.get(0))
            .and_then(|r| r.get("document"))
            .ok_or_else(|| ApiError::Malformed {
                call: "update/documents",
                what: "no results[0].document".to_string(),
            })?;
        let text = |key: &str| {
            document
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match (text("document_id"), text("etag")) {
            (Some(docwsid), Some(etag)) => Ok(Uploaded { docwsid, etag }),
            _ => Err(ApiError::Malformed {
                call: "update/documents",
                what: "document without id or etag".to_string(),
            }),
        }
    }
}

/// Reads exactly `left` bytes: a file that grew or shrank while being sent
/// fails the upload instead of sending something else than declared.
struct Exactly<'a> {
    inner: &'a mut dyn Read,
    left: u64,
}

impl<'a> Exactly<'a> {
    fn new(inner: &'a mut dyn Read, len: u64) -> Self {
        Self { inner, left: len }
    }
}

impl Read for Exactly<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            // Nothing more may come: a longer file is an error.
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the file grew while being uploaded",
                )),
            };
        }
        let max = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.inner.read(buf.get_mut(..max).unwrap_or_default())?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the file shrank while being uploaded",
            ));
        }
        self.left = self
            .left
            .saturating_sub(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }
}
