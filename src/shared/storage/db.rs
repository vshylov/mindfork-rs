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
use crate::entities::rag::{RagDocument, RagHit};

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
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ---------- заметки ----------

    pub fn note_insert(&self, note: &Note) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes(id, profile_id, content, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                note.id.to_string(),
                note.profile_id.to_string(),
                note.content,
                serde_json::to_string(&note.tags)?,
                note.created_at.to_rfc3339(),
                note.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Заметки профиля: опционально фильтр по подстроке содержимого и тегам,
    /// сортировка по `updated_at` убыв., опциональный лимит. Изоляция по профилю.
    pub fn note_list(
        &self,
        profile_id: Uuid,
        query: Option<&str>,
        tags: &[String],
        limit: Option<usize>,
    ) -> Result<Vec<Note>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, profile_id, content, tags, created_at, updated_at
             FROM notes WHERE profile_id = ?1",
        );
        if query.is_some() {
            sql.push_str(" AND content LIKE ?2");
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let like = query.map(|q| format!("%{q}%"));
        let rows = if let Some(like) = &like {
            stmt.query_map(params![profile_id.to_string(), like], row_to_note)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![profile_id.to_string()], row_to_note)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut notes: Vec<Note> = rows;
        if !tags.is_empty() {
            notes.retain(|n| tags.iter().all(|t| n.tags.contains(t)));
        }
        if let Some(limit) = limit {
            notes.truncate(limit);
        }
        Ok(notes)
    }

    /// Жёсткое удаление заметки по id (репозиторная операция; UI-потребитель — позже).
    #[allow(dead_code)]
    pub fn note_delete(&self, id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM notes WHERE id = ?1", params![id.to_string()])?;
        Ok(n > 0)
    }

    // ---------- RAG ----------

    pub fn rag_insert(&self, doc: &RagDocument) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        ensure_vec_table(&conn, doc.embedding.len())?;
        conn.execute(
            "INSERT INTO rag_documents(id, profile_id, source, chunk_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                doc.id.to_string(),
                doc.profile_id.to_string(),
                doc.source,
                doc.chunk_text,
                doc.created_at.to_rfc3339(),
            ],
        )?;
        let rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO rag_vectors(rowid, profile_id, embedding) VALUES (?1, ?2, ?3)",
            params![
                rowid,
                doc.profile_id.to_string(),
                bytemuck::cast_slice::<f32, u8>(&doc.embedding),
            ],
        )?;
        Ok(())
    }

    /// kNN-поиск по базе знаний профиля (изоляция по `profile_id` через partition key).
    pub fn rag_search(&self, profile_id: Uuid, query: &[f32], k: usize) -> Result<Vec<RagHit>> {
        let conn = self.conn.lock().unwrap();
        if vec_dim(&conn)?.is_none() {
            return Ok(Vec::new()); // ещё ничего не индексировали
        }
        let mut stmt = conn.prepare(
            "SELECT d.id, d.source, d.chunk_text, v.distance
             FROM rag_vectors v
             JOIN rag_documents d ON d.rowid = v.rowid
             WHERE v.profile_id = ?1 AND v.embedding MATCH ?2 AND k = ?3
             ORDER BY v.distance",
        )?;
        let hits = stmt
            .query_map(
                params![
                    profile_id.to_string(),
                    bytemuck::cast_slice::<f32, u8>(query),
                    k as i64
                ],
                |r| {
                    Ok(RagHit {
                        id: parse_uuid(r.get::<_, String>(0)?),
                        source: r.get(1)?,
                        chunk_text: r.get(2)?,
                        distance: r.get(3)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hits)
    }

    /// Число RAG-документов профиля (репозиторная операция; UI-потребитель — позже).
    #[allow(dead_code)]
    pub fn rag_count(&self, profile_id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rag_documents WHERE profile_id = ?1",
            params![profile_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Удаляет все документы (чанки) с точным совпадением `source` у профиля.
    /// Возвращает число удалённых. Используется идемпотентной переиндексацией файла
    /// (`/rag add` — заменяем прежние чанки источника, а не плодим дубликаты).
    pub fn rag_delete_by_source(&self, profile_id: Uuid, source: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_matching(&conn, profile_id, |s| s == source)
    }

    /// Удаляет документы по пути: сам путь (файл) и всё, что под ним (директория).
    /// Сравнение устойчиво к разделителям (`/` ↔ `\`) и регистру (Windows) и **не
    /// требует наличия файла на диске** (`/rag delete`). Возвращает число удалённых.
    pub fn rag_delete_under(&self, profile_id: Uuid, path: &str) -> Result<usize> {
        let needle = norm_path(path);
        let prefix = format!("{needle}/");
        let conn = self.conn.lock().unwrap();
        delete_matching(&conn, profile_id, |s| {
            let s = norm_path(s);
            s == needle || s.starts_with(&prefix)
        })
    }
}

/// Удаляет документы профиля, чьи `source` проходят предикат (вместе с их
/// векторами в `vec0`). Возвращает число удалённых документов.
fn delete_matching(
    conn: &Connection,
    profile_id: Uuid,
    pred: impl Fn(&str) -> bool,
) -> Result<usize> {
    let rows: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT rowid, source FROM rag_documents WHERE profile_id = ?1")?;
        stmt.query_map(params![profile_id.to_string()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let victims: Vec<i64> = rows
        .into_iter()
        .filter(|(_, s)| pred(s))
        .map(|(rowid, _)| rowid)
        .collect();
    if victims.is_empty() {
        return Ok(0);
    }
    // Таблица векторов есть только после первой вставки; вне неё удалять нечего.
    let has_vectors = vec_dim(conn)?.is_some();
    for rowid in &victims {
        if has_vectors {
            conn.execute("DELETE FROM rag_vectors WHERE rowid = ?1", params![rowid])?;
        }
        conn.execute("DELETE FROM rag_documents WHERE rowid = ?1", params![rowid])?;
    }
    Ok(victims.len())
}

/// Нормализует путь для устойчивого сравнения: разделители к `/`, без хвостового
/// слэша, на Windows — нижний регистр (NTFS регистронезависим).
fn norm_path(p: &str) -> String {
    let unified = p.replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

fn migrate(conn: &Connection) -> Result<()> {
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

         CREATE TABLE IF NOT EXISTS rag_documents (
             rowid       INTEGER PRIMARY KEY,
             id          TEXT NOT NULL UNIQUE,
             profile_id  TEXT NOT NULL,
             source      TEXT NOT NULL,
             chunk_text  TEXT NOT NULL,
             created_at  TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_rag_profile ON rag_documents(profile_id);",
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

fn parse_uuid(s: String) -> Uuid {
    Uuid::parse_str(&s).unwrap_or(Uuid::nil())
}

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn notes_isolated_by_profile() {
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        db.note_insert(&Note::new(a, "secret of A", vec![]))
            .unwrap();
        db.note_insert(&Note::new(b, "secret of B", vec![]))
            .unwrap();

        let a_notes = db.note_list(a, None, &[], None).unwrap();
        assert_eq!(a_notes.len(), 1);
        assert_eq!(a_notes[0].content, "secret of A");
        // Профиль B не виден из A.
        assert!(a_notes.iter().all(|n| n.profile_id == a));
    }

    #[test]
    fn note_query_and_tag_filter() {
        let db = db();
        let p = Uuid::new_v4();
        db.note_insert(&Note::new(p, "likes tea", vec!["pref".into()]))
            .unwrap();
        db.note_insert(&Note::new(
            p,
            "likes coffee",
            vec!["pref".into(), "drink".into()],
        ))
        .unwrap();

        assert_eq!(db.note_list(p, Some("tea"), &[], None).unwrap().len(), 1);
        assert_eq!(
            db.note_list(p, None, &["drink".to_string()], None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(db.note_list(p, None, &[], Some(1)).unwrap().len(), 1);
    }

    #[test]
    fn note_delete_works() {
        let db = db();
        let p = Uuid::new_v4();
        let note = Note::new(p, "x", vec![]);
        db.note_insert(&note).unwrap();
        assert!(db.note_delete(note.id).unwrap());
        assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
    }

    #[test]
    fn rag_knn_respects_profile_isolation() {
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        // Профиль B имеет вектор, идентичный запросу — он не должен «утечь» в поиск A.
        db.rag_insert(&RagDocument::new(b, "b", "B doc", vec![1.0, 0.0, 0.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(
            a,
            "a1",
            "A near",
            vec![0.9, 0.1, 0.0, 0.0],
        ))
        .unwrap();
        db.rag_insert(&RagDocument::new(
            a,
            "a2",
            "A far",
            vec![0.0, 0.0, 1.0, 0.0],
        ))
        .unwrap();

        let hits = db.rag_search(a, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 2, "only profile A docs");
        assert_eq!(hits[0].chunk_text, "A near");
        assert_eq!(hits[1].chunk_text, "A far");
        assert_eq!(db.rag_count(a).unwrap(), 2);
        assert_eq!(db.rag_count(b).unwrap(), 1);
    }

    #[test]
    fn rag_search_empty_before_any_insert() {
        let db = db();
        let hits = db.rag_search(Uuid::new_v4(), &[1.0, 0.0], 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn rag_delete_by_source_removes_exact_only() {
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c1", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c2", vec![0.0, 1.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/b.txt", "c3", vec![1.0, 1.0]))
            .unwrap();

        // Удаляются оба чанка источника a.txt, b.txt остаётся.
        assert_eq!(db.rag_delete_by_source(p, "/data/a.txt").unwrap(), 2);
        assert_eq!(db.rag_count(p).unwrap(), 1);
        // Поиск тоже больше их не находит (векторы удалены).
        let hits = db.rag_search(p, &[1.0, 0.0], 5).unwrap();
        assert!(hits.iter().all(|h| h.source == "/data/b.txt"));
    }

    #[test]
    fn rag_delete_under_removes_path_and_descendants() {
        let db = db();
        let p = Uuid::new_v4();
        let other = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "a", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/sub/b.txt", "b", vec![0.0, 1.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/other/c.txt", "c", vec![1.0, 1.0]))
            .unwrap();
        // Чужой профиль с тем же путём не должен затрагиваться (изоляция).
        db.rag_insert(&RagDocument::new(other, "/data/a.txt", "x", vec![1.0, 0.0]))
            .unwrap();

        // Удаление директории сносит файл и вложенные, но не «/other» и не чужой профиль.
        assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 2);
        assert_eq!(db.rag_count(p).unwrap(), 1);
        assert_eq!(db.rag_count(other).unwrap(), 1);

        // Префикс не цепляет соседнюю директорию с общим началом имени.
        db.rag_insert(&RagDocument::new(p, "/x/file.txt", "f", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(
            p,
            "/x-extra/file.txt",
            "g",
            vec![0.0, 1.0],
        ))
        .unwrap();
        assert_eq!(db.rag_delete_under(p, "/x").unwrap(), 1);
    }

    #[test]
    fn rag_delete_under_tolerates_separators_and_trailing_slash() {
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "a", vec![1.0, 0.0]))
            .unwrap();
        // Обратные слэши и хвостовой слэш в запросе матчат сохранённый «/»-источник.
        assert_eq!(db.rag_delete_under(p, "\\data\\").unwrap(), 1);
    }

    #[test]
    fn rag_dim_mismatch_errors() {
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "s", "t", vec![1.0, 0.0, 0.0, 0.0]))
            .unwrap();
        let err = db.rag_insert(&RagDocument::new(p, "s", "t", vec![1.0, 0.0]));
        assert!(err.is_err());
    }
}
