//! SQLite storage of notes and RAG (with sqlite-vec). Isolation by `profile_id`
//! is mandatory in every query (invariant, spec §10.3). See spec §5.2.
//!
//! The RAG vector is stored in a `vec0` virtual table with a **partition key**
//! of `profile_id` — this guarantees correct per-profile kNN (rather than "top-k
//! across all profiles, then filtering"). The vector dimensionality is fixed on
//! the first insert (lazy).

use std::sync::{Mutex, Once};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::entities::note::Note;
use crate::entities::rag::{RagDocument, RagHit, RagSourceInfo, RagStoredSource};
use crate::entities::self_model::SelfModel;
use crate::shared::storage::schema::DB_SCHEMA;

static REGISTER_VEC: Once = Once::new();

/// Registers the sqlite-vec extension (once per process).
fn register_sqlite_vec() {
    // The entry-point function's type is inferred from sqlite3_auto_extension's
    // signature; transmute annotations here would only add noise (this is the
    // canonical sqlite-vec pattern).
    #[allow(clippy::missing_transmute_annotations)]
    REGISTER_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// SQLite storage of notes and RAG.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Opens the DB at a path (creating it if absent) and applies migrations.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        register_sqlite_vec();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::from_conn(conn)
    }

    /// Opens an in-memory DB (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        register_sqlite_vec();
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        let mut conn = conn;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

/// Brings the DB to the current schema (ADR 0006): additive DDL (idempotent, every
/// time) + a baseline `user_version` stamp + breaking steps in transactions. Downgrade
/// (a DB newer than the app) — a protective `bail`; the user-facing localized refusal
/// is placed by [`crate::features::data_migration`] (peeking `user_version` before
/// opening storage).
fn migrate(conn: &mut Connection) -> Result<()> {
    // CREATE ... IF NOT EXISTS runs EVERY time — this is the mechanism for adding new
    // tables/indexes to an existing DB without a version bump (the additive policy, F12).
    baseline_ddl(conn)?;

    let from = read_user_version(conn)?;
    if from > DB_SCHEMA {
        bail!("data.db is from a newer app version (schema {from}, supported is {DB_SCHEMA})");
    }
    // An existing/fresh DB (user_version = 0) gets stamped with the baseline version.
    // This is not a data migration (the DDL is idempotent) — no pre-migration backup needed.
    if from == 0 {
        set_user_version(conn, DB_SCHEMA)?;
    }
    apply_db_steps(conn, DB_STEPS, from)?;
    Ok(())
}

/// A breaking SQLite migration step: a schema/data transformation in a transaction. The
/// `DB_STEPS` registry is empty for now (all schemas = 1); the first real breaking
/// change will add a step + a fixture.
#[allow(dead_code)] // constructed by the first real migration (and in tests)
struct DbStep {
    to: u32,
    summary: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const DB_STEPS: &[DbStep] = &[];

/// Runs breaking steps `> from`: each in its own transaction **together** with the
/// `user_version` update — on error, a full rollback (neither the schema nor the
/// version changes).
fn apply_db_steps(conn: &mut Connection, steps: &[DbStep], from: u32) -> Result<()> {
    // `from.max(1)`: baseline = 1, real steps start at 2 (the 0→1 stamp is not a step).
    for step in steps.iter().filter(|s| s.to > from.max(1)) {
        let tx = conn.transaction()?;
        (step.apply)(&tx).with_context(|| format!("data.db migration → v{}", step.to))?;
        set_user_version(&tx, step.to)?;
        tx.commit()?;
    }
    Ok(())
}

fn read_user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as u32)
}

fn set_user_version(conn: &Connection, v: u32) -> Result<()> {
    // `PRAGMA user_version = N` doesn't accept a bound parameter — we format
    // it in (v: u32, injection is impossible). Inside a transaction the
    // change is atomic with it.
    conn.execute_batch(&format!("PRAGMA user_version = {v};"))?;
    Ok(())
}

/// Reads the `PRAGMA user_version` of a DB file (0 — file missing / fresh). For
/// coordinating the shared pre-migrate moment in [`crate::features::data_migration`]:
/// the downgrade guard and the backup decision are made before opening storage.
pub fn peek_user_version(path: &std::path::Path) -> Result<u32> {
    if !path.exists() {
        return Ok(0);
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    read_user_version(&conn)
}

/// Whether there are pending **real** breaking DB migrations. Baseline (0→1) doesn't
/// count — it's idempotent and needs no backup. Determines whether the DB is included
/// in the pre-migration backup.
pub fn needs_step_migration(user_version: u32) -> bool {
    DB_STEPS.iter().any(|s| s.to > user_version.max(1))
}

/// The idempotent DB schema (additive; see [`migrate`]).
fn baseline_ddl(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

         CREATE TABLE IF NOT EXISTS notes (
             id          TEXT PRIMARY KEY,
             profile_id  TEXT NOT NULL,
             content     TEXT NOT NULL,
             tags        TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             updated_at  TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_notes_profile ON notes(profile_id);

         CREATE TABLE IF NOT EXISTS note_vectors (
             note_id     TEXT PRIMARY KEY,
             profile_id  TEXT NOT NULL,
             embedding   TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_note_vectors_profile ON note_vectors(profile_id);

         CREATE TABLE IF NOT EXISTS rag_documents (
             rowid       INTEGER PRIMARY KEY,
             id          TEXT NOT NULL UNIQUE,
             profile_id  TEXT NOT NULL,
             source      TEXT NOT NULL,
             chunk_text  TEXT NOT NULL,
             created_at  TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_rag_profile ON rag_documents(profile_id);

         CREATE TABLE IF NOT EXISTS rag_sources (
             profile_id  TEXT NOT NULL,
             source      TEXT NOT NULL,
             content     TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             PRIMARY KEY (profile_id, source)
         );

         CREATE TABLE IF NOT EXISTS self_models (
             profile_id  TEXT PRIMARY KEY,
             data        TEXT NOT NULL,
             version     INTEGER NOT NULL,
             updated_at  TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS note_links (
             profile_id  TEXT NOT NULL,
             from_id     TEXT NOT NULL,
             to_id       TEXT NOT NULL,
             relation    TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             PRIMARY KEY (profile_id, from_id, to_id, relation)
         );
         CREATE INDEX IF NOT EXISTS idx_note_links_from ON note_links(profile_id, from_id);
         CREATE INDEX IF NOT EXISTS idx_note_links_to ON note_links(profile_id, to_id);

         CREATE TABLE IF NOT EXISTS note_superseded (
             note_id        TEXT PRIMARY KEY,
             profile_id     TEXT NOT NULL,
             superseded_by  TEXT NOT NULL,
             superseded_at  TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS note_rag_links (
             profile_id  TEXT NOT NULL,
             note_id     TEXT NOT NULL,
             source      TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             PRIMARY KEY (profile_id, note_id, source)
         );
         CREATE INDEX IF NOT EXISTS idx_note_rag_note ON note_rag_links(profile_id, note_id);
         CREATE INDEX IF NOT EXISTS idx_note_rag_source ON note_rag_links(profile_id, source);",
    )?;
    Ok(())
}

/// The current RAG vector dimensionality (if the vector table already exists).
fn vec_dim(conn: &Connection) -> Result<Option<usize>> {
    let dim: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'rag_dim'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(dim.map(|d| d.parse().unwrap_or(0)))
}

/// Creates the virtual vector table for the needed dimensionality (once).
fn ensure_vec_table(conn: &Connection, dim: usize) -> Result<()> {
    if dim == 0 {
        bail!("refusing to index an empty embedding");
    }
    match vec_dim(conn)? {
        Some(existing) if existing == dim => Ok(()),
        Some(existing) => bail!("embedding dim mismatch: table is {existing}, got {dim}"),
        None => {
            conn.execute(
                &format!(
                    "CREATE VIRTUAL TABLE rag_vectors USING vec0(
                         profile_id TEXT partition key,
                         embedding float[{dim}]
                     )"
                ),
                [],
            )?;
            conn.execute(
                "INSERT INTO meta(key, value) VALUES ('rag_dim', ?1)",
                params![dim.to_string()],
            )?;
            Ok(())
        }
    }
}

fn row_to_note(r: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: parse_uuid(r.get::<_, String>(0)?),
        profile_id: parse_uuid(r.get::<_, String>(1)?),
        content: r.get(2)?,
        tags: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
        created_at: parse_dt(r.get::<_, String>(4)?),
        updated_at: parse_dt(r.get::<_, String>(5)?),
    })
}

/// Cosine similarity of two vectors (0 if lengths differ or a norm is zero).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum();
    let nb: f32 = b.iter().map(|x| x * x).sum();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn parse_uuid(s: String) -> Uuid {
    Uuid::parse_str(&s).unwrap_or(Uuid::nil())
}

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ---------- domain submodules (god-object breakup: docs/history/refactoring-god-objects.md, stage 5) ----------

mod graph;
mod notes;
mod rag;
mod self_model;

#[cfg(test)]
mod migrate_tests {
    use super::*;

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_some()
    }

    #[test]
    fn baseline_stamps_fresh_db_to_v1() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(read_user_version(&conn).unwrap(), DB_SCHEMA);
        // The schema was applied (one of the baseline tables exists).
        assert!(table_exists(&conn, "notes"));
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(read_user_version(&conn).unwrap(), DB_SCHEMA);
    }

    #[test]
    fn migrate_refuses_downgrade() {
        let mut conn = Connection::open_in_memory().unwrap();
        set_user_version(&conn, DB_SCHEMA + 5).unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    #[test]
    fn peek_user_version_zero_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(peek_user_version(&dir.path().join("nope.db")).unwrap(), 0);
    }

    #[test]
    fn no_pending_step_migration_at_v1() {
        // The DB_STEPS registry is empty — there are no real migrations (besides baseline).
        assert!(!needs_step_migration(0));
        assert!(!needs_step_migration(1));
    }

    #[test]
    fn apply_db_steps_commits_and_rolls_back_transactionally() {
        fn good(c: &Connection) -> Result<()> {
            c.execute_batch("CREATE TABLE t_ok(x)")?;
            Ok(())
        }
        fn bad(c: &Connection) -> Result<()> {
            c.execute_batch("CREATE TABLE t_bad(x)")?;
            bail!("deliberate step failure");
        }

        let mut conn = Connection::open_in_memory().unwrap();
        set_user_version(&conn, 1).unwrap();

        // A successful step: the table is created, version = 2.
        apply_db_steps(
            &mut conn,
            &[DbStep {
                to: 2,
                summary: "ok",
                apply: good,
            }],
            1,
        )
        .unwrap();
        assert_eq!(read_user_version(&conn).unwrap(), 2);
        assert!(table_exists(&conn, "t_ok"));

        // A failing step: a full rollback — no table, the version is unchanged.
        let res = apply_db_steps(
            &mut conn,
            &[DbStep {
                to: 3,
                summary: "bad",
                apply: bad,
            }],
            2,
        );
        assert!(res.is_err());
        assert_eq!(read_user_version(&conn).unwrap(), 2);
        assert!(!table_exists(&conn, "t_bad"));
    }
}
