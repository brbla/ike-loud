//! SQLite storage. The only module that touches `rusqlite`.

use std::path::Path;

use ikeloud_safety::{ContentHash, Etag, LocalId, RelPath};
use ikeloud_sync::names::key;
use ikeloud_sync::{BaseEntry, DocId, Transfer};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{BaseChange, JournalEntry, JournalKind, StateError, UnconfirmedUpload};

/// Schema version this build writes and understands.
pub const SCHEMA_VERSION: i64 = 3;

/// Migrations: `MIGRATIONS[v]` brings version `v` to `v + 1`.
const MIGRATIONS: [&str; 3] = [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3];

const SCHEMA_V1: &str = "
CREATE TABLE base (
    doc_id     TEXT PRIMARY KEY NOT NULL CHECK (doc_id <> ''),
    device     INTEGER,
    inode      INTEGER,
    birth_ns   INTEGER,
    key        TEXT NOT NULL UNIQUE,
    path       TEXT NOT NULL CHECK (path <> ''),
    local_path TEXT NOT NULL CHECK (local_path <> ''),
    hash       TEXT NOT NULL CHECK (hash <> ''),
    etag       TEXT NOT NULL CHECK (etag <> ''),
    UNIQUE (device, inode),
    CHECK ((device IS NULL) = (inode IS NULL)),
    CHECK (inode IS NOT NULL OR birth_ns IS NULL)
) STRICT;
CREATE TABLE journal (
    id     INTEGER PRIMARY KEY,
    time   INTEGER NOT NULL CHECK (time >= 0),
    kind   TEXT NOT NULL,
    path   TEXT NOT NULL CHECK (path <> ''),
    target TEXT CHECK (target IS NULL OR target <> '')
) STRICT;
";

/// Version 2: transfers of actions done but not yet in the common state.
/// Each action records its transfer at once, so a crash before the end of a
/// run does not forget what the action proved (docs/rozhodnuti/0017).
const SCHEMA_V2: &str = "
CREATE TABLE pending (
    seq      INTEGER PRIMARY KEY,
    path     TEXT NOT NULL CHECK (path <> ''),
    doc_id   TEXT NOT NULL CHECK (doc_id <> ''),
    etag     TEXT NOT NULL CHECK (etag <> ''),
    device   INTEGER NOT NULL,
    inode    INTEGER NOT NULL,
    birth_ns INTEGER,
    hash     TEXT NOT NULL CHECK (hash <> '')
) STRICT;
";

/// Version 3: uploads whose last step got no answer. The document may be
/// on iCloud; iCloud's checksum of the content tells a later listing
/// whether it is (decision 0026).
const SCHEMA_V3: &str = "
CREATE TABLE unconfirmed (
    seq      INTEGER PRIMARY KEY,
    path     TEXT NOT NULL CHECK (path <> ''),
    doc_id   TEXT NOT NULL CHECK (doc_id <> ''),
    device   INTEGER NOT NULL,
    inode    INTEGER NOT NULL,
    birth_ns INTEGER,
    hash     TEXT NOT NULL CHECK (hash <> ''),
    checksum TEXT NOT NULL CHECK (checksum <> ''),
    time     INTEGER NOT NULL
) STRICT;
";

/// The state database.
#[derive(Debug)]
pub struct StateDb {
    conn: Connection,
}

fn sql(e: rusqlite::Error) -> StateError {
    StateError::Sql(e.to_string())
}

fn open_err(e: rusqlite::Error) -> StateError {
    StateError::Open(e.to_string())
}

impl StateDb {
    /// Opens (or creates) the database file and brings the schema up to date.
    pub fn open(path: &Path) -> Result<Self, StateError> {
        let conn = Connection::open(path).map_err(open_err)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(open_err)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StateError::Open(format!("journal mode is {mode}, not WAL")));
        }
        Self::configure(conn)
    }

    /// An in-memory database (tests, dry runs).
    pub fn open_in_memory() -> Result<Self, StateError> {
        Self::configure(Connection::open_in_memory().map_err(open_err)?)
    }

    fn configure(conn: Connection) -> Result<Self, StateError> {
        conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")
            .map_err(open_err)?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), StateError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(open_err)?;
        if version > SCHEMA_VERSION {
            return Err(StateError::NewerSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        let from = usize::try_from(version)
            .map_err(|_| StateError::Open(format!("unknown schema version {version}")))?;
        let steps = MIGRATIONS.get(from..).unwrap_or_default();
        if steps.is_empty() {
            return Ok(());
        }
        // All steps in one transaction: a crash leaves the old version.
        let tx = self.conn.transaction().map_err(sql)?;
        for step in steps {
            tx.execute_batch(step).map_err(sql)?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// The last common state of all files, sorted by normalized name.
    pub fn base(&self) -> Result<Vec<BaseEntry>, StateError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {BASE_COLUMNS} FROM base ORDER BY key"))
            .map_err(sql)?;
        let rows = stmt.query_map([], base_row).map_err(sql)?;
        rows.map(|row| row.map_err(sql)?).collect()
    }

    /// The common-state entry of one file, matched by normalized name.
    pub fn base_entry(&self, path: &RelPath) -> Result<Option<BaseEntry>, StateError> {
        self.conn
            .query_row(
                &format!("SELECT {BASE_COLUMNS} FROM base WHERE key = ?1"),
                params![key(path.as_str())],
                base_row,
            )
            .optional()
            .map_err(sql)?
            .transpose()
    }

    /// Applies changes of the common state and appends journal entries in
    /// one transaction: either everything is stored or nothing.
    pub fn commit(
        &mut self,
        changes: &[BaseChange],
        journal: &[JournalEntry],
    ) -> Result<(), StateError> {
        let tx = self.conn.transaction().map_err(sql)?;
        // First remove every entry the commit touches, then insert the new
        // ones: uniqueness of names and inodes is checked on the final state,
        // so e.g. two files swapping names in one commit is fine, while two
        // documents ending up with one name still fails the whole commit.
        for change in changes {
            let doc = match change {
                BaseChange::Set(e) => &e.doc_id,
                BaseChange::Forget(doc) => doc,
            };
            tx.execute("DELETE FROM base WHERE doc_id = ?1", params![doc.as_str()])
                .map_err(sql)?;
        }
        for change in changes {
            if let BaseChange::Set(e) = change {
                check_entry(&tx, e)?;
                tx.execute(
                    &format!(
                        "INSERT INTO base ({BASE_COLUMNS}, key)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                    ),
                    params![
                        e.path.as_str(),
                        e.local_path.as_str(),
                        e.local.map(|id| u64_to_sql(id.device)),
                        e.local.map(|id| u64_to_sql(id.inode)),
                        e.local.and_then(|id| id.birth_ns),
                        e.doc_id.as_str(),
                        e.hash.as_str(),
                        e.etag.as_str(),
                        key(e.path.as_str()),
                    ],
                )
                .map_err(sql)?;
            }
        }
        for entry in journal {
            insert_journal(&tx, entry)?;
        }
        // The common state now holds what the pending transfers proved.
        tx.execute("DELETE FROM pending", []).map_err(sql)?;
        // Dropping `tx` without commit (any `?` above) rolls everything back.
        tx.commit().map_err(sql)
    }

    /// Records one carried-out action at once, in its own transaction: its
    /// journal entry and, for a transfer, what it proved. Survives a crash
    /// before the end of the run; [`StateDb::commit`] clears the transfers.
    pub fn record(
        &mut self,
        entry: &JournalEntry,
        transfer: Option<&Transfer>,
    ) -> Result<(), StateError> {
        let tx = self.conn.transaction().map_err(sql)?;
        insert_journal(&tx, entry)?;
        if let Some(t) = transfer {
            tx.execute(
                "INSERT INTO pending (doc_id, etag, device, inode, birth_ns, hash, path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    t.doc_id.as_str(),
                    t.etag.as_str(),
                    u64_to_sql(t.local.device),
                    u64_to_sql(t.local.inode),
                    t.local.birth_ns,
                    t.hash.as_str(),
                    t.path.as_str()
                ],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    /// Transfers recorded since the last commit, in the order they happened.
    pub fn pending(&self) -> Result<Vec<Transfer>, StateError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT doc_id, etag, device, inode, birth_ns, hash, path FROM pending ORDER BY seq",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(sql)?;
        let mut transfers = Vec::new();
        for row in rows {
            let (doc, etag, device, inode, birth_ns, hash, path) = row.map_err(sql)?;
            transfers.push(Transfer {
                path: stored_path("pending", path)?,
                doc_id: DocId::new(doc),
                etag: Etag::new(etag),
                local: LocalId {
                    device: u64_from_sql(device),
                    inode: u64_from_sql(inode),
                    birth_ns,
                },
                hash: ContentHash::new(hash),
            });
        }
        Ok(transfers)
    }

    /// Remembers an upload whose last step got no answer.
    pub fn add_unconfirmed(&mut self, upload: &UnconfirmedUpload) -> Result<(), StateError> {
        self.conn
            .execute(
                "INSERT INTO unconfirmed (path, device, inode, birth_ns, hash, checksum, time, doc_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    upload.path.as_str(),
                    u64_to_sql(upload.local.device),
                    u64_to_sql(upload.local.inode),
                    upload.local.birth_ns,
                    upload.hash.as_str(),
                    upload.checksum,
                    i64::try_from(upload.time).unwrap_or(i64::MAX),
                    upload.doc_id.as_str()
                ],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Uploads whose last step got no answer, oldest first, with their keys
    /// for [`StateDb::settle_unconfirmed`].
    pub fn unconfirmed(&self) -> Result<Vec<(i64, UnconfirmedUpload)>, StateError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, path, device, inode, birth_ns, hash, checksum, time, doc_id
                 FROM unconfirmed ORDER BY seq",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })
            .map_err(sql)?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, path, device, inode, birth_ns, hash, checksum, time, doc) =
                row.map_err(sql)?;
            out.push((
                seq,
                UnconfirmedUpload {
                    path: stored_path("unconfirmed", path)?,
                    doc_id: DocId::new(doc),
                    local: LocalId {
                        device: u64_from_sql(device),
                        inode: u64_from_sql(inode),
                        birth_ns,
                    },
                    hash: ContentHash::new(hash),
                    checksum,
                    time: u64::try_from(time).unwrap_or(0),
                },
            ));
        }
        Ok(out)
    }

    /// Settles an unconfirmed upload in one transaction: forgets it and,
    /// when a listing showed the document with its content, records what
    /// the upload proved (its journal entry and transfer, as
    /// [`StateDb::record`] does).
    pub fn settle_unconfirmed(
        &mut self,
        seq: i64,
        confirmed: Option<(&JournalEntry, &Transfer)>,
    ) -> Result<(), StateError> {
        let tx = self.conn.transaction().map_err(sql)?;
        tx.execute("DELETE FROM unconfirmed WHERE seq = ?1", params![seq])
            .map_err(sql)?;
        if let Some((entry, t)) = confirmed {
            insert_journal(&tx, entry)?;
            tx.execute(
                "INSERT INTO pending (doc_id, etag, device, inode, birth_ns, hash, path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    t.doc_id.as_str(),
                    t.etag.as_str(),
                    u64_to_sql(t.local.device),
                    u64_to_sql(t.local.inode),
                    t.local.birth_ns,
                    t.hash.as_str(),
                    t.path.as_str()
                ],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    /// The newest `limit` journal entries, newest first.
    pub fn journal(&self, limit: usize) -> Result<Vec<JournalEntry>, StateError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.query_journal(
            "SELECT time, kind, path, target FROM journal ORDER BY id DESC LIMIT ?1",
            limit,
        )
    }

    /// Journal entries from `since` (Unix seconds) on, newest first: what
    /// happened in the mass-change brake window.
    pub fn journal_since(&self, since: u64) -> Result<Vec<JournalEntry>, StateError> {
        let since = i64::try_from(since).map_err(|_| StateError::InvalidValue { field: "time" })?;
        self.query_journal(
            "SELECT time, kind, path, target FROM journal WHERE time >= ?1 ORDER BY id DESC",
            since,
        )
    }

    fn query_journal(&self, query: &str, value: i64) -> Result<Vec<JournalEntry>, StateError> {
        let mut stmt = self.conn.prepare(query).map_err(sql)?;
        let rows = stmt
            .query_map(params![value], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sql)?;
        let mut entries = Vec::new();
        for row in rows {
            let (time, kind, path, target) = row.map_err(sql)?;
            let kind = JournalKind::from_name(&kind).ok_or_else(|| StateError::CorruptRow {
                table: "journal",
                detail: format!("unknown kind {kind:?}"),
            })?;
            let time = u64::try_from(time).map_err(|_| StateError::CorruptRow {
                table: "journal",
                detail: format!("negative time {time}"),
            })?;
            entries.push(JournalEntry {
                time,
                kind,
                path: stored_path("journal", path)?,
                target: target.map(|t| stored_path("journal", t)).transpose()?,
            });
        }
        Ok(entries)
    }
}

fn insert_journal(tx: &rusqlite::Transaction<'_>, entry: &JournalEntry) -> Result<(), StateError> {
    let time = i64::try_from(entry.time).map_err(|_| StateError::InvalidValue { field: "time" })?;
    tx.execute(
        "INSERT INTO journal (time, kind, path, target) VALUES (?1, ?2, ?3, ?4)",
        params![
            time,
            entry.kind.name(),
            entry.path.as_str(),
            entry.target.as_ref().map(RelPath::as_str)
        ],
    )
    .map_err(sql)?;
    Ok(())
}

/// Checks an entry against the state stored so far in this transaction, so
/// that a broken invariant is reported as what it is. The schema enforces
/// the same rules as a backstop.
fn check_entry(tx: &rusqlite::Transaction<'_>, e: &BaseEntry) -> Result<(), StateError> {
    let empty = [
        ("doc_id", e.doc_id.as_str()),
        ("path", e.path.as_str()),
        ("local_path", e.local_path.as_str()),
        ("hash", e.hash.as_str()),
        ("etag", e.etag.as_str()),
    ]
    .into_iter()
    .find(|(_, value)| value.is_empty());
    if let Some((field, _)) = empty {
        return Err(StateError::InvalidValue { field });
    }
    let name_taken = tx
        .query_row(
            "SELECT 1 FROM base WHERE key = ?1",
            params![key(e.path.as_str())],
            |_| Ok(()),
        )
        .optional()
        .map_err(sql)?
        .is_some();
    if name_taken {
        return Err(StateError::NameTaken {
            path: e.path.clone(),
        });
    }
    if let Some(id) = e.local {
        let file_taken = tx
            .query_row(
                "SELECT 1 FROM base WHERE device = ?1 AND inode = ?2",
                params![u64_to_sql(id.device), u64_to_sql(id.inode)],
                |_| Ok(()),
            )
            .optional()
            .map_err(sql)?
            .is_some();
        if file_taken {
            return Err(StateError::LocalFileTaken {
                path: e.path.clone(),
            });
        }
    }
    Ok(())
}

/// Columns of a common-state entry, in the order `base_row` reads them.
const BASE_COLUMNS: &str = "path, local_path, device, inode, birth_ns, doc_id, hash, etag";

/// A path read back from the database; malformed text is a corrupt row.
fn stored_path(table: &'static str, text: String) -> Result<RelPath, StateError> {
    RelPath::parse(text.as_str()).map_err(|e| StateError::CorruptRow {
        table,
        detail: format!("malformed path {text:?}: {e:?}"),
    })
}

/// One row of `base`: an SQL error, or the entry (itself an error when the
/// stored values are not valid).
fn base_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<BaseEntry, StateError>> {
    let device: Option<i64> = r.get(2)?;
    let inode: Option<i64> = r.get(3)?;
    let birth_ns: Option<i64> = r.get(4)?;
    // The schema guarantees device and inode are both set or both NULL.
    let local = device.zip(inode).map(|(device, inode)| LocalId {
        device: u64_from_sql(device),
        inode: u64_from_sql(inode),
        birth_ns,
    });
    let (path, local_path): (String, String) = (r.get(0)?, r.get(1)?);
    let (doc_id, hash, etag): (String, String, String) = (r.get(5)?, r.get(6)?, r.get(7)?);
    Ok(stored_path("base", path).and_then(|path| {
        Ok(BaseEntry {
            path,
            local_path: stored_path("base", local_path)?,
            local,
            doc_id: DocId::new(doc_id),
            hash: ContentHash::new(hash),
            etag: Etag::new(etag),
        })
    }))
}

/// SQLite integers are signed; device and inode numbers are stored bit for bit.
fn u64_to_sql(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn u64_from_sql(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

#[cfg(test)]
mod tests {
    use super::{SCHEMA_VERSION, StateDb};
    use crate::StateError;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ikeloud-state-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("state.db")
    }

    #[test]
    fn newer_schema_is_refused_and_left_untouched() {
        let path = temp_file("newer");
        drop(StateDb::open(&path).expect("create"));
        {
            let conn = rusqlite::Connection::open(&path).expect("raw open");
            conn.pragma_update(None, "user_version", 99)
                .expect("set version");
        }
        assert_eq!(
            StateDb::open(&path).map(|_| ()),
            Err(StateError::NewerSchema {
                found: 99,
                supported: SCHEMA_VERSION
            })
        );
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read");
        assert_eq!(v, 99, "the file was not migrated");
    }

    #[test]
    fn malformed_stored_paths_are_reported() {
        let db = StateDb::open_in_memory().expect("open");
        db.conn
            .execute(
                "INSERT INTO base (doc_id, key, path, local_path, hash, etag)
                 VALUES ('d', '../x', '../x', '../x', 'h', 'e')",
                [],
            )
            .expect("insert");
        db.conn
            .execute(
                "INSERT INTO journal (time, kind, path) VALUES (1, 'uploaded', 'a//b')",
                [],
            )
            .expect("insert");
        assert!(matches!(
            db.base(),
            Err(StateError::CorruptRow { table: "base", .. })
        ));
        assert!(matches!(
            db.journal(10),
            Err(StateError::CorruptRow {
                table: "journal",
                ..
            })
        ));
    }

    #[test]
    fn a_version_1_database_is_migrated_keeping_its_data() {
        let path = temp_file("migrate");
        {
            // A database as version 1 wrote it.
            let conn = rusqlite::Connection::open(&path).expect("raw open");
            conn.execute_batch(super::SCHEMA_V1).expect("v1 schema");
            conn.execute(
                "INSERT INTO base (doc_id, device, inode, key, path, local_path, hash, etag)
                 VALUES ('d', 1, 1, 'a', 'a', 'a', 'h', 'e')",
                [],
            )
            .expect("row");
            conn.pragma_update(None, "user_version", 1)
                .expect("version");
        }
        let db = StateDb::open(&path).expect("migrate");
        assert_eq!(db.base().expect("base").len(), 1, "data kept");
        assert_eq!(
            db.pending().expect("pending"),
            vec![],
            "the new table exists"
        );
        let v: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read");
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn unknown_journal_kind_is_reported() {
        let db = StateDb::open_in_memory().expect("open");
        db.conn
            .execute(
                "INSERT INTO journal (time, kind, path) VALUES (1, 'teleported', 'a')",
                [],
            )
            .expect("insert");
        assert!(matches!(
            db.journal(10),
            Err(StateError::CorruptRow {
                table: "journal",
                ..
            })
        ));
    }

    #[test]
    fn uncommitted_transaction_is_lost_after_a_crash() {
        let path = temp_file("crash");
        drop(StateDb::open(&path).expect("create"));
        {
            // Simulated crash: a transaction is started and the process dies.
            let mut conn = rusqlite::Connection::open(&path).expect("raw open");
            let tx = conn.transaction().expect("begin");
            tx.execute(
                "INSERT INTO base (doc_id, device, inode, key, path, local_path, hash, etag)
                 VALUES ('d', 1, 1, 'a', 'a', 'a', 'h', 'e')",
                [],
            )
            .expect("insert");
            std::mem::forget(tx);
            drop(conn);
        }
        let db = StateDb::open(&path).expect("reopen");
        assert_eq!(db.base().expect("read"), vec![]);
    }
}
