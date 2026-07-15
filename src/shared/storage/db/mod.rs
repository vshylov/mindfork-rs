//! SQLite-хранилище заметок и RAG (с sqlite-vec). Изоляция по `profile_id`
//! обязательна во всех запросах (инвариант, spec §10.3). См. spec §5.2.
//!
//! Вектор RAG хранится в виртуальной таблице `vec0` с **partition key**
//! `profile_id` — это обеспечивает корректный per-profile kNN (а не «top-k по
//! всем профилям с последующей фильтрацией»). Размерность вектора фиксируется
//! при первой вставке (lazy).

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

/// Регистрирует расширение sqlite-vec (один раз на процесс).
fn register_sqlite_vec() {
    // Тип функции-точки входа задаётся выводом из сигнатуры sqlite3_auto_extension;
    // аннотации transmute здесь только зашумят (это канонический паттерн sqlite-vec).
    #[allow(clippy::missing_transmute_annotations)]
    REGISTER_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// SQLite-хранилище заметок и RAG.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Открывает БД по пути (создаёт при отсутствии) и применяет миграции.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        register_sqlite_vec();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::from_conn(conn)
    }

    /// Открывает БД в памяти (для тестов).
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

/// Приводит БД к текущей схеме (ADR 0006): additive-DDL (идемпотентный, каждый раз) +
/// baseline-штамп `user_version` + breaking-шаги в транзакциях. Downgrade (БД новее
/// приложения) — защитный `bail`; пользовательский локализованный отказ ставит
/// [`crate::features::data_migration`] (peek `user_version` до открытия хранилища).
fn migrate(conn: &mut Connection) -> Result<()> {
    // CREATE ... IF NOT EXISTS выполняется КАЖДЫЙ раз — это механизм добавления новых
    // таблиц/индексов существующим БД без bump версии (additive-политика, Ф12).
    baseline_ddl(conn)?;

    let from = read_user_version(conn)?;
    if from > DB_SCHEMA {
        bail!(
            "data.db из более новой версии приложения (схема {from}, поддерживается {DB_SCHEMA})"
        );
    }
    // Существующую/свежую БД (user_version = 0) штампуем baseline-версией. Это не
    // миграция данных (DDL идемпотентен) — pre-migration бэкап не нужен.
    if from == 0 {
        set_user_version(conn, DB_SCHEMA)?;
    }
    apply_db_steps(conn, DB_STEPS, from)?;
    Ok(())
}

/// Шаг миграции SQLite (breaking): трансформация схемы/данных в транзакции. Реестр
/// `DB_STEPS` пока пуст (все схемы = 1); первый реальный breaking добавит шаг + фикстуру.
#[allow(dead_code)] // конструируется первой реальной миграцией (и в тестах)
struct DbStep {
    to: u32,
    summary: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const DB_STEPS: &[DbStep] = &[];

/// Прогоняет breaking-шаги `> from`: каждый в своей транзакции **вместе** с обновлением
/// `user_version` — на ошибке откат целиком (ни схема, ни версия не меняются).
fn apply_db_steps(conn: &mut Connection, steps: &[DbStep], from: u32) -> Result<()> {
    // `from.max(1)`: baseline = 1, реальные шаги начинаются со 2 (штамп 0→1 — не шаг).
    for step in steps.iter().filter(|s| s.to > from.max(1)) {
        let tx = conn.transaction()?;
        (step.apply)(&tx).with_context(|| format!("миграция data.db → v{}", step.to))?;
        set_user_version(&tx, step.to)?;
        tx.commit()?;
    }
    Ok(())
}

fn read_user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? as u32)
}

fn set_user_version(conn: &Connection, v: u32) -> Result<()> {
    // `PRAGMA user_version = N` не принимает связанный параметр — форматируем (v: u32,
    // инъекция невозможна). Внутри транзакции изменение атомарно с ней.
    conn.execute_batch(&format!("PRAGMA user_version = {v};"))?;
    Ok(())
}

/// Читает `PRAGMA user_version` файла БД (0 — файла нет / свежая). Для координации
/// общего pre-migrate момента в [`crate::features::data_migration`]: downgrade-guard и
/// решение о бэкапе принимаются до открытия хранилища.
pub fn peek_user_version(path: &std::path::Path) -> Result<u32> {
    if !path.exists() {
        return Ok(0);
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    read_user_version(&conn)
}

/// Есть ли ожидающие **реальные** breaking-миграции БД. Baseline (0→1) не в счёт —
/// он идемпотентен и бэкапа не требует. Определяет, включать ли БД в pre-migration бэкап.
pub fn needs_step_migration(user_version: u32) -> bool {
    DB_STEPS.iter().any(|s| s.to > user_version.max(1))
}

/// Идемпотентная схема БД (additive; см. [`migrate`]).
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

/// Текущая размерность векторов RAG (если таблица векторов уже создана).
fn vec_dim(conn: &Connection) -> Result<Option<usize>> {
    let dim: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'rag_dim'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(dim.map(|d| d.parse().unwrap_or(0)))
}

/// Создаёт виртуальную таблицу векторов под нужную размерность (один раз).
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

/// Косинусная близость двух векторов (0, если длины разнятся или нулевая норма).
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

// ---------- подмодули по доменам (разбор god-object: docs/history/refactoring-god-objects.md, этап 5) ----------

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
        // Схема применена (одна из таблиц baseline есть).
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
        // Реестр DB_STEPS пуст — реальных миграций (кроме baseline) нет.
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
            bail!("умышленный сбой шага");
        }

        let mut conn = Connection::open_in_memory().unwrap();
        set_user_version(&conn, 1).unwrap();

        // Успешный шаг: таблица создана, версия = 2.
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

        // Падающий шаг: полный откат — таблицы нет, версия прежняя.
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
