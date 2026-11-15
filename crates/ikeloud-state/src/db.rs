//! SQLite storage. The only module that touches `rusqlite`.

use std::path::Path;

use ikeloud_safety::{Etag, RelPath};
use ikeloud_sync::names::key;
use ikeloud_sync::{BaseEntry, ContentHash};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{BaseChange, JournalEntry, JournalKind, StateError};

/// Schema version this build writes and understands.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE base (
    key  TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL CHECK (path <> ''),
    hash TEXT NOT NULL CHECK (hash <> ''),
    etag TEXT NOT NULL CHECK (etag <> '')
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
            .prepare("SELECT path, hash, etag FROM base ORDER BY key")
            .map_err(sql)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BaseEntry {
                    path: RelPath::new(r.get::<_, String>(0)?),
                    hash: ContentHash::new(r.get::<_, String>(1)?),
                    etag: Etag::new(r.get::<_, String>(2)?),
                })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    /// The common-state entry of one file, matched by normalized name.
    pub fn base_entry(&self, path: &RelPath) -> Result<Option<BaseEntry>, StateError> {
        self.conn
            .query_row(
                "SELECT path, hash, etag FROM base WHERE key = ?1",
                params![key(path.as_str())],
                |r| {
                    Ok(BaseEntry {
                        path: RelPath::new(r.get::<_, String>(0)?),
                        hash: ContentHash::new(r.get::<_, String>(1)?),
                        etag: Etag::new(r.get::<_, String>(2)?),
                    })
                },
            )
            .optional()
            .map_err(sql)
    }

    /// Applies changes of the common state and appends journal entries in
    /// one transaction: either everything is stored or nothing.
    pub fn commit(
        &mut self,
        changes: &[BaseChange],
        journal: &[JournalEntry],
    ) -> Result<(), StateError> {
        let tx = self.conn.transaction().map_err(sql)?;
        for change in changes {
            match change {
                BaseChange::Set(e) => {
                    tx.execute(
                        "INSERT INTO base (key, path, hash, etag) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (key) DO UPDATE SET
                             path = excluded.path, hash = excluded.hash, etag = excluded.etag",
                        params![
                            key(e.path.as_str()),
                            e.path.as_str(),
                            e.hash.as_str(),
                            e.etag.as_str()
                        ],
                    )
                    .map_err(sql)?;
                }
                BaseChange::Forget(path) => {
                    tx.execute(
                        "DELETE FROM base WHERE key = ?1",
                        params![key(path.as_str())],
                    )
                    .map_err(sql)?;
                }
            }
        }
        for entry in journal {
            let time = i64::try_from(entry.time)
                .map_err(|_| StateError::Sql(format!("time {} out of range", entry.time)))?;
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
                path: RelPath::new(path),
                target: target.map(RelPath::new),
            });
        }
        Ok(entries)
    }
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
                "INSERT INTO base (key, path, hash, etag) VALUES ('a', 'a', 'h', 'e')",
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
