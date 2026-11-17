//! Changes of the iCloud Drive zone from CloudKit (decision 0025).
//!
//! `changes/zone` of the private `com.apple.clouddocs` database returns the
//! zone's records and a sync token; with the token, the next call returns
//! only what changed since (an unchanged zone: one request, no records).
//! The records describe the tree:
//!
//! - `directory/<id>` (type `structure`): a folder; `directory/root` is the
//!   root;
//! - `documentStructure/<id>`: a file's name and place;
//! - `documentContent/<id>`: a file's size and content hash.
//!
//! Names are in `encryptedBasename`: base64 of the name without the
//! extension (really encrypted only with Advanced Data Protection; then
//! they do not decode to text and the caller falls back to the listing).
//! The `recordChangeTag` of the records make the etags of the Drive web
//! service: a folder's is its record's, a file's is `<structure>::<content>`.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::b64;
use crate::client::{ApiError, Client};
use crate::http::{Method, Transport};

/// Records per request (CloudKit's usual page size).
const PAGE: u64 = 200;

/// What a record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// A folder (`directory/<id>`).
    Folder,
    /// A file's name and place (`documentStructure/<id>`).
    FileStructure,
    /// A file's content (`documentContent/<id>`).
    FileContent,
    /// Anything else in the zone; ignored.
    Other,
}

/// One changed record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneRecord {
    /// Kind, from the record name.
    pub kind: RecordKind,
    /// The id after the slash of the record name (`root` for the root).
    pub id: String,
    /// The record was deleted.
    pub deleted: bool,
    /// Version of the record.
    pub change_tag: String,
    /// Id of the parent folder (structure records).
    pub parent: Option<String>,
    /// Name without the extension; `None` when it is not text (encrypted).
    pub basename: Option<String>,
    /// Extension, without the dot.
    pub extension: Option<String>,
    /// Size in bytes (content records).
    pub size: Option<u64>,
    /// Content hash as the server keeps it.
    pub basehash: Option<String>,
}

/// One page of changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneChanges {
    /// The changed records.
    pub records: Vec<ZoneRecord>,
    /// Token for the next call.
    pub sync_token: String,
    /// More changes wait: call again with `sync_token`.
    pub more_coming: bool,
}

#[derive(Deserialize)]
struct RawZone {
    #[serde(rename = "syncToken")]
    sync_token: Option<String>,
    #[serde(rename = "moreComing", default)]
    more_coming: bool,
    #[serde(default)]
    records: Vec<Value>,
    #[serde(rename = "serverErrorCode")]
    server_error_code: Option<String>,
}

#[derive(Deserialize)]
struct RawChanges {
    #[serde(default)]
    zones: Vec<RawZone>,
}

fn kind_and_id(record_name: &str) -> (RecordKind, String) {
    let (prefix, id) = record_name.split_once('/').unwrap_or(("", record_name));
    let kind = match prefix {
        "directory" => RecordKind::Folder,
        "documentStructure" => RecordKind::FileStructure,
        "documentContent" => RecordKind::FileContent,
        _ => RecordKind::Other,
    };
    (kind, id.to_string())
}

fn field<'a>(record: &'a Value, name: &str) -> Option<&'a Value> {
    record.get("fields")?.get(name)?.get("value")
}

fn parse_record(record: &Value) -> Option<ZoneRecord> {
    let (kind, id) = kind_and_id(record.get("recordName")?.as_str()?);
    let text = |name: &str| {
        field(record, name)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Some(ZoneRecord {
        kind,
        id,
        deleted: record
            .get("deleted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        change_tag: record
            .get("recordChangeTag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        parent: record
            .get("parent")
            .and_then(|p| p.get("recordName"))
            .and_then(Value::as_str)
            .map(|name| kind_and_id(name).1),
        basename: text("encryptedBasename")
            .and_then(|b| b64::decode(&b))
            .and_then(|bytes| String::from_utf8(bytes).ok()),
        extension: text("extension").filter(|e| !e.is_empty()),
        size: field(record, "size").and_then(Value::as_u64),
        basehash: text("basehash"),
    })
}

impl<T: Transport> Client<T> {
    /// The changes of the Drive zone since `sync_token` (all records when
    /// `None`), one page. A zone-level error (e.g. an expired token) is
    /// [`ApiError::Rejected`]: start again without a token.
    pub fn zone_changes(&mut self, sync_token: Option<&str>) -> Result<ZoneChanges, ApiError> {
        let account = self.account().ok_or(ApiError::NotSignedIn)?.clone();
        let base = account.ckdatabasews.ok_or(ApiError::DriveUnavailable)?;
        let url = format!(
            "{base}/database/1/com.apple.clouddocs/production/private/changes/zone?remapEnums=true&getCurrentSyncToken=true&dsid={}",
            account.dsid
        );
        let mut zone = json!({"zoneID": {"zoneName": "com.apple.CloudDocs"}, "resultsLimit": PAGE});
        if let (Some(token), Some(map)) = (sync_token, zone.as_object_mut()) {
            map.insert("syncToken".to_string(), json!(token));
        }
        let raw: RawChanges = self.web_json(
            "changes/zone",
            Method::Post,
            &url,
            Some(json!({"zones": [zone]})),
        )?;
        let zone = raw
            .zones
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::Malformed {
                call: "changes/zone",
                what: "no zone in the answer".to_string(),
            })?;
        if let Some(code) = zone.server_error_code {
            return Err(ApiError::Rejected {
                call: "changes/zone",
                status: code,
            });
        }
        let sync_token = zone.sync_token.ok_or_else(|| ApiError::Malformed {
            call: "changes/zone",
            what: "no sync token".to_string(),
        })?;
        let records = zone
            .records
            .iter()
            .map(|r| {
                parse_record(r).ok_or_else(|| ApiError::Malformed {
                    call: "changes/zone",
                    what: "a record without a name".to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ZoneChanges {
            records,
            sync_token,
            more_coming: zone.more_coming,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_as_the_server_sends_them() {
        // Shapes from the test account (decision 0025).
        let file = json!({
            "recordName": "documentStructure/6B9F",
            "recordType": "structure",
            "recordChangeTag": "c",
            "parent": {"recordName": "directory/7A6D", "action": "NONE"},
            "fields": {
                "encryptedBasename": {"value": "cGhvdG8tMTAwMDU3MzA1", "type": "BYTES"},
                "extension": {"value": "jpg", "type": "STRING"},
                "basehash": {"value": "rOM7", "type": "BYTES"}
            }
        });
        let r = parse_record(&file).unwrap();
        assert_eq!(r.kind, RecordKind::FileStructure);
        assert_eq!(r.id, "6B9F");
        assert_eq!(r.parent.as_deref(), Some("7A6D"));
        assert_eq!(r.basename.as_deref(), Some("photo-100057305"));
        assert_eq!(r.extension.as_deref(), Some("jpg"));
        assert_eq!(r.change_tag, "c");
        assert!(!r.deleted);

        let content = json!({
            "recordName": "documentContent/6B9F", "recordType": "content", "recordChangeTag": "b",
            "fields": {"size": {"value": 390363, "type": "INT64"}}
        });
        let r = parse_record(&content).unwrap();
        assert_eq!((r.kind, r.size), (RecordKind::FileContent, Some(390363)));

        let folder = json!({"recordName": "directory/root", "recordType": "structure", "recordChangeTag": "4st"});
        let r = parse_record(&folder).unwrap();
        assert_eq!(
            (r.kind, r.id.as_str(), r.parent),
            (RecordKind::Folder, "root", None)
        );

        let gone = json!({"recordName": "documentStructure/X", "deleted": true});
        assert!(parse_record(&gone).unwrap().deleted);

        // Encrypted (ADP): not text, so no name.
        let secret = json!({"recordName": "directory/Y", "fields": {"encryptedBasename": {"value": "/w=="}}});
        assert_eq!(parse_record(&secret).unwrap().basename, None);
    }
}
