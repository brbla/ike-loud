//! SQLite storage. The only module that touches `rusqlite`.

use std::path::Path;

use ikeloud_safety::{ContentHash, Etag, LocalId, RelPath};
use ikeloud_sync::names::key;
use ikeloud_sync::{BaseEntry, DocId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{BaseChange, JournalEntry, JournalKind, StateError};

/// Schema version this build writes and understands.
pub const SCHEMA_VERSION: i64 = 1;

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
        match version {
            0 => {
                let tx = self.conn.transaction().map_err(sql)?;
                tx.execute_batch(SCHEMA_V1).map_err(sql)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(sql)?;
                tx.commit().map_err(sql)
            }
            SCHEMA_VERSION => Ok(()),
            found if found > SCHEMA_VERSION => Err(StateError::NewerSchema {
                found,
                supported: SCHEMA_VERSION,
            }),
            found => Err(StateError::Open(format!("unknown schema version {found}"))),
        }
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
            let time = i64::try_from(entry.time)
                .map_err(|_| StateError::InvalidValue { field: "time" })?;
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
        }
        // Dropping `tx` without commit (any `?` above) rolls everything back.
        tx.commit().map_err(sql)
    }

    /// The newest `limit` journal entries, newest first.
    pub fn journal(&self, limit: usize) -> Result<Vec<JournalEntry>, StateError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare("SELECT time, kind, path, target FROM journal ORDER BY id DESC LIMIT ?1")
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![limit], |r| {
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
