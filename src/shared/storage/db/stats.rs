//! Read-only counts over a `data.db`, for `mindfork stats` (docs/data-stats.md,
//! spec §12.4).
//!
//! Deliberately **not** a method of [`super::Db`]: opening a `Db` runs the
//! baseline DDL and the migrations, and the summary must leave what it reads
//! exactly as it found it (F6) — it is run to decide which of several copies to
//! keep, so a copy it had quietly upgraded would be a different copy. Two
//! sources, one set of queries:
//!
//! * a **file** — the live database, opened read-only after the header check
//!   [`super::vacuum_into`] makes for the same reason (SQLite can delete a stale
//!   sidecar next to a file it reads as zero-page, read-only or not);
//! * an **image** — a backup's `data.db`, read from the zip entry straight into
//!   SQLite's memory (F5): nothing of an encrypted archive reaches the disk.
//!
//! Only ordinary tables are queried, never the `vec0` virtual ones, so the
//! counts do not depend on the sqlite-vec extension being loadable for the
//! image at hand. A table this binary knows and the database lacks counts as
//! zero — a database that predates note links has no links.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};

/// How long a read waits for a writer's lock before giving up. The app commits
/// in milliseconds; this only has to outlast one of those, and a summary that
/// hangs on a wedged database would be worse than one that says so.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// What `data.db` holds, across all profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DbStats {
    /// Notes, superseded ones included.
    pub notes: u64,
    /// Notes another note replaced — the notes' form of soft delete
    /// (spec §9.5): kept, and left out of every listing.
    pub notes_superseded: u64,
    pub note_links: u64,
    /// The newest `notes.updated_at`. `None` — no notes, or a value that does
    /// not read as a timestamp.
    pub last_note_change: Option<DateTime<Utc>>,
    pub rag_sources: u64,
    pub rag_chunks: u64,
    pub self_models: u64,
}

/// Counts over the database file at `path`, which is left untouched.
pub fn stats_of_file(path: &Path) -> Result<DbStats> {
    if !super::is_sqlite_file(path) {
        bail!("{} is not a SQLite database", path.display());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    collect(&conn)
}

/// Counts over a database **image** of exactly `size` bytes — a backup's
/// `data.db` as it comes out of the archive.
pub fn stats_of_image(image: impl Read, size: usize) -> Result<DbStats> {
    let mut conn = Connection::open_in_memory()?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, image, size, true)
        .context("loading the database image")?;
    collect(&conn)
}

fn collect(conn: &Connection) -> Result<DbStats> {
    Ok(DbStats {
        notes: count(conn, "notes", "SELECT COUNT(*) FROM notes")?,
        // Joined, not a bare count: a superseded note that was later deleted
        // outright must not be reported as a note that is still kept.
        notes_superseded: if has_table(conn, "notes")? {
            count(
                conn,
                "note_superseded",
                "SELECT COUNT(*) FROM note_superseded s JOIN notes n ON n.id = s.note_id",
            )?
        } else {
            0
        },
        note_links: count(conn, "note_links", "SELECT COUNT(*) FROM note_links")?,
        last_note_change: last_note_change(conn)?,
        rag_sources: count(conn, "rag_sources", "SELECT COUNT(*) FROM rag_sources")?,
        rag_chunks: count(conn, "rag_documents", "SELECT COUNT(*) FROM rag_documents")?,
        self_models: count(conn, "self_models", "SELECT COUNT(*) FROM self_models")?,
    })
}

fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let found: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(found > 0)
}

/// Runs the counting `sql`, or answers zero when `table` does not exist.
fn count(conn: &Connection, table: &str, sql: &str) -> Result<u64> {
    if !has_table(conn, table)? {
        return Ok(0);
    }
    let n: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .with_context(|| format!("counting {table}"))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// The newest note change. Every `updated_at` is written by the app as UTC
/// RFC 3339, where text order is time order — so `MAX` over the column is the
/// newest, and only that one value is parsed.
fn last_note_change(conn: &Connection) -> Result<Option<DateTime<Utc>>> {
    if !has_table(conn, "notes")? {
        return Ok(None);
    }
    let newest: Option<String> =
        conn.query_row("SELECT MAX(updated_at) FROM notes", [], |row| row.get(0))?;
    Ok(newest
        .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
        .map(|at| at.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A database the way the app leaves it: the real schema, then rows.
    /// Returns the directory guard with the path.
    fn seeded() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        {
            let conn = Connection::open(&path).unwrap();
            super::super::baseline_ddl(&conn).unwrap();
            conn.execute_batch(
                "INSERT INTO notes VALUES
                   ('n1','p','old','[]','2026-01-01T00:00:00+00:00','2026-01-02T00:00:00+00:00'),
                   ('n2','p','new','[]','2026-01-01T00:00:00+00:00','2026-03-05T10:20:30.5+00:00'),
                   ('n3','q','other','[]','2026-01-01T00:00:00+00:00','2026-02-01T00:00:00+00:00');
                 INSERT INTO note_superseded VALUES
                   ('n1','p','n2','2026-03-05T10:20:30+00:00'),
                   ('gone','p','n2','2026-03-05T10:20:30+00:00');
                 INSERT INTO note_links VALUES ('p','n2','n3','relates','2026-03-05T10:20:30+00:00');
                 INSERT INTO rag_sources VALUES ('p','a.txt','text','2026-01-01T00:00:00+00:00');
                 INSERT INTO rag_documents (id, profile_id, source, chunk_text, created_at) VALUES
                   ('d1','p','a.txt','one','2026-01-01T00:00:00+00:00'),
                   ('d2','p','a.txt','two','2026-01-01T00:00:00+00:00');
                 INSERT INTO self_models VALUES ('p','{}',1,'2026-01-01T00:00:00+00:00');",
            )
            .unwrap();
        }
        (dir, path)
    }

    fn expected() -> DbStats {
        DbStats {
            notes: 3,
            // `gone` is superseded on paper and deleted in fact — not counted.
            notes_superseded: 1,
            note_links: 1,
            last_note_change: Some("2026-03-05T10:20:30.5Z".parse().unwrap()),
            rag_sources: 1,
            rag_chunks: 2,
            self_models: 1,
        }
    }

    fn listing(dir: &Path) -> Vec<(String, u64)> {
        let mut out: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_file_is_counted_and_left_as_it_was() {
        let (dir, path) = seeded();
        let before = (listing(dir.path()), fs::read(&path).unwrap());
        assert_eq!(stats_of_file(&path).unwrap(), expected());
        assert_eq!((listing(dir.path()), fs::read(&path).unwrap()), before);
    }

    #[test]
    fn an_image_counts_the_same_as_the_file_it_was_read_from() {
        let (_dir, path) = seeded();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            stats_of_image(bytes.as_slice(), bytes.len()).unwrap(),
            expected()
        );
    }

    /// The hazard the header check exists for (docs/lessons.md §8): SQLite
    /// deletes a stale `-wal` next to a file it reads as zero-page, and a
    /// read-only open does not stop it. The sidecar is the witness.
    #[test]
    fn a_non_database_is_refused_before_sqlite_can_touch_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        fs::write(&path, b"").unwrap();
        fs::write(dir.path().join("data.db-wal"), b"stale").unwrap();
        let before = listing(dir.path());
        assert!(stats_of_file(&path).is_err());
        assert_eq!(listing(dir.path()), before);
    }

    #[test]
    fn a_garbage_image_is_an_error_not_a_row_of_zeros() {
        let junk = vec![0x5a_u8; 4096];
        assert!(stats_of_image(junk.as_slice(), junk.len()).is_err());
    }

    /// A database older than the newer tables: what it has is counted, what it
    /// predates is zero — and the missing `notes` side of the join is not an error.
    #[test]
    fn tables_the_database_predates_count_as_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE rag_sources (profile_id TEXT, source TEXT, content TEXT, created_at TEXT);
                 INSERT INTO rag_sources VALUES ('p','a','b','c');
                 CREATE TABLE note_superseded (note_id TEXT, profile_id TEXT,
                                               superseded_by TEXT, superseded_at TEXT);
                 INSERT INTO note_superseded VALUES ('x','p','y','z');",
            )
            .unwrap();
        }
        assert_eq!(
            stats_of_file(&path).unwrap(),
            DbStats {
                rag_sources: 1,
                ..DbStats::default()
            }
        );
    }
}
