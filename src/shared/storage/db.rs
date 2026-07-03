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
        // Замещённые (superseded) заметки скрыты из активной выдачи (хранятся ради
        // «шрама»/трассировки) — anti-join по note_superseded.
        let mut sql = String::from(
            "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at
             FROM notes n
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE n.profile_id = ?1 AND s.note_id IS NULL",
        );
        if query.is_some() {
            sql.push_str(" AND n.content LIKE ?2");
        }
        sql.push_str(" ORDER BY n.updated_at DESC");

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

    /// Переписывает содержимое заметки на месте (ревизия), обновляя `updated_at`.
    /// Изоляция по `profile_id` в `WHERE`. `false`, если заметка не найдена/чужая.
    pub fn note_update(&self, id: Uuid, profile_id: Uuid, content: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE notes SET content = ?1, updated_at = ?2 WHERE id = ?3 AND profile_id = ?4",
            params![
                content,
                Utc::now().to_rfc3339(),
                id.to_string(),
                profile_id.to_string(),
            ],
        )?;
        Ok(n > 0)
    }

    /// Сохраняет/заменяет эмбеддинг заметки (для семантического поиска). Вектор —
    /// JSON-массив f32 в боковой таблице (намеренно НЕ vec0: заметок немного,
    /// косинус считаем в Rust — см. [`Self::note_search_semantic`]).
    pub fn note_vector_upsert(
        &self,
        note_id: Uuid,
        profile_id: Uuid,
        embedding: &[f32],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO note_vectors(note_id, profile_id, embedding) VALUES (?1, ?2, ?3)
             ON CONFLICT(note_id) DO UPDATE SET
                 profile_id = excluded.profile_id,
                 embedding = excluded.embedding",
            params![
                note_id.to_string(),
                profile_id.to_string(),
                serde_json::to_string(embedding)?,
            ],
        )?;
        Ok(())
    }

    /// Семантический поиск заметок профиля по косинусной близости к `query`.
    /// Brute-force в Rust (заметок десятки–сотни); заметки без эмбеддинга
    /// пропускаются. Возвращает до `k` пар (заметка, близость) по убыванию.
    /// Изоляция — `WHERE n.profile_id = ?`.
    pub fn note_search_semantic(
        &self,
        profile_id: Uuid,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(Note, f32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at, v.embedding
             FROM notes n
             JOIN note_vectors v ON v.note_id = n.id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE n.profile_id = ?1 AND s.note_id IS NULL",
        )?;
        let mut scored: Vec<(Note, f32)> = stmt
            .query_map(params![profile_id.to_string()], |r| {
                let note = row_to_note(r)?;
                let emb: Vec<f32> =
                    serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default();
                Ok((note, emb))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(note, emb)| {
                let score = cosine(query, &emb);
                (note, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        Ok(scored)
    }

    /// Заметки профиля, у которых ещё нет эмбеддинга (для бэкфилла «старых» заметок,
    /// созданных до векторного поиска, импортированных или сохранённых при
    /// недоступном тогда эмбеддере). Возвращает пары (id, содержимое).
    pub fn notes_missing_vectors(&self, profile_id: Uuid) -> Result<Vec<(Uuid, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.content FROM notes n
             LEFT JOIN note_vectors v ON v.note_id = n.id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE n.profile_id = ?1 AND v.note_id IS NULL AND s.note_id IS NULL",
        )?;
        let rows = stmt
            .query_map(params![profile_id.to_string()], |r| {
                Ok((parse_uuid(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Заметка по id в рамках профиля (включая замещённую) — для чтения тегов при
    /// замещении/слиянии: новая версия наследует теги исходной (в т.ч. `@self`, чтобы
    /// self-заметка не «выпала» в пользовательскую выдачу). `None` — не найдена/чужая.
    pub fn note_get(&self, profile_id: Uuid, id: Uuid) -> Result<Option<Note>> {
        let conn = self.conn.lock().unwrap();
        let note = conn
            .query_row(
                "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at
                 FROM notes n WHERE n.id = ?1 AND n.profile_id = ?2",
                params![id.to_string(), profile_id.to_string()],
                row_to_note,
            )
            .optional()?;
        Ok(note)
    }

    // ---------- граф связей и «шрамы» (Ярус 2) ----------

    /// Заметка существует у профиля и не замещена (для проверки концов связи).
    pub fn note_is_active(&self, profile_id: Uuid, id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM notes n
                 LEFT JOIN note_superseded s ON s.note_id = n.id
                 WHERE n.id = ?1 AND n.profile_id = ?2 AND s.note_id IS NULL",
                params![id.to_string(), profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Создаёт направленную связь between двумя заметками (идемпотентно по PK).
    /// Изоляция по `profile_id`. Возвращает `true`, если связь действительно создана
    /// (`false` — такая связь уже была, `INSERT OR IGNORE` ничего не вставил).
    pub fn note_link_insert(
        &self,
        profile_id: Uuid,
        from_id: Uuid,
        to_id: Uuid,
        relation: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT OR IGNORE INTO note_links(profile_id, from_id, to_id, relation, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                profile_id.to_string(),
                from_id.to_string(),
                to_id.to_string(),
                relation,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(n > 0)
    }

    /// Число связей, в которых участвует заметка (в любую сторону). Для предупреждения
    /// при ревизии смыслонесущего узла (его рёбра могут стать неверными).
    pub fn note_link_count(&self, profile_id: Uuid, id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM note_links
             WHERE profile_id = ?1 AND (from_id = ?2 OR to_id = ?2)",
            params![profile_id.to_string(), id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Соседи заметки по графу (в обе стороны), исключая замещённые. Опциональный
    /// фильтр по типу связи. Возвращает (заметка, тип связи, исходящая ли связь).
    pub fn note_neighbors(
        &self,
        profile_id: Uuid,
        id: Uuid,
        relation: Option<&str>,
    ) -> Result<Vec<(Note, String, bool)>> {
        let conn = self.conn.lock().unwrap();
        let rel = if relation.is_some() {
            " AND l.relation = ?3"
        } else {
            ""
        };
        let sql = format!(
            "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at, l.relation, 1
             FROM note_links l
             JOIN notes n ON n.id = l.to_id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE l.profile_id = ?1 AND l.from_id = ?2 AND s.note_id IS NULL{rel}
             UNION ALL
             SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at, l.relation, 0
             FROM note_links l
             JOIN notes n ON n.id = l.from_id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE l.profile_id = ?1 AND l.to_id = ?2 AND s.note_id IS NULL{rel}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row| -> rusqlite::Result<(Note, String, bool)> {
            let note = row_to_note(r)?;
            let relation: String = r.get(6)?;
            let outgoing: i64 = r.get(7)?;
            Ok((note, relation, outgoing != 0))
        };
        let rows = if let Some(rel) = relation {
            stmt.query_map(params![profile_id.to_string(), id.to_string(), rel], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![profile_id.to_string(), id.to_string()], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    /// Помечает заметку замещённой другой (скрывается из активной выдачи, хранится
    /// ради «шрама»/трассировки). Идемпотентно (перезапись записи о замещении).
    pub fn note_supersede_mark(&self, profile_id: Uuid, old_id: Uuid, new_id: Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO note_superseded(note_id, profile_id, superseded_by, superseded_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(note_id) DO UPDATE SET
                 superseded_by = excluded.superseded_by,
                 superseded_at = excluded.superseded_at",
            params![
                old_id.to_string(),
                profile_id.to_string(),
                new_id.to_string(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Активные заметки профиля с их эмбеддингами (для консолидации: поиск дублей
    /// попарным косинусом). Замещённые исключены.
    pub fn notes_with_vectors(&self, profile_id: Uuid) -> Result<Vec<(Note, Vec<f32>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at, v.embedding
             FROM notes n
             JOIN note_vectors v ON v.note_id = n.id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE n.profile_id = ?1 AND s.note_id IS NULL",
        )?;
        let rows = stmt
            .query_map(params![profile_id.to_string()], |r| {
                let note = row_to_note(r)?;
                let emb: Vec<f32> =
                    serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default();
                Ok((note, emb))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Все связи профиля `(from, to, relation)` — для обзора консолидации.
    pub fn note_links_all(&self, profile_id: Uuid) -> Result<Vec<(Uuid, Uuid, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT from_id, to_id, relation FROM note_links WHERE profile_id = ?1")?;
        let rows = stmt
            .query_map(params![profile_id.to_string()], |r| {
                Ok((
                    parse_uuid(r.get::<_, String>(0)?),
                    parse_uuid(r.get::<_, String>(1)?),
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Переносит связи замещённой заметки на новую (при merge): рёбра, где участвует
    /// `old_id`, перенаправляются на `new_id`; самопетли и дубли отбрасываются.
    pub fn note_links_retarget(&self, profile_id: Uuid, old_id: Uuid, new_id: Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let edges: Vec<(String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT from_id, to_id, relation FROM note_links
                 WHERE profile_id = ?1 AND (from_id = ?2 OR to_id = ?2)",
            )?;
            stmt.query_map(params![profile_id.to_string(), old_id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let (old_s, new_s) = (old_id.to_string(), new_id.to_string());
        for (from, to, rel) in edges {
            // Удаляем старое ребро, затем вставляем перенацеленное (OR IGNORE от дублей).
            conn.execute(
                "DELETE FROM note_links WHERE profile_id=?1 AND from_id=?2 AND to_id=?3 AND relation=?4",
                params![profile_id.to_string(), from, to, rel],
            )?;
            let nf = if from == old_s { &new_s } else { &from };
            let nt = if to == old_s { &new_s } else { &to };
            if nf == nt {
                continue; // самопетля после переноса — отбрасываем
            }
            conn.execute(
                "INSERT OR IGNORE INTO note_links(profile_id, from_id, to_id, relation, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![profile_id.to_string(), nf, nt, rel, Utc::now().to_rfc3339()],
            )?;
        }
        Ok(())
    }

    // ---------- модель себя (SelfModel) ----------

    /// Модель себя профиля (`None`, если ещё не создавалась). Изоляция по PK.
    pub fn self_model_get(&self, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_get_conn(&conn, profile_id)
    }

    /// Сохраняет модель себя (INSERT OR REPLACE), повышая `version`. Поле `version`
    /// в переданной модели игнорируется — авторитетный счётчик ведёт хранилище.
    ///
    /// Для правки существующей модели предпочтителен [`Self::self_model_update`]:
    /// он делает чтение-правку-запись **атомарно** (под одним захватом мьютекса),
    /// исключая гонку «прочитал → кто-то записал → записал поверх» между фоновой
    /// авто-рефлексией, ручной правкой `F3` и инструментами хода. Прямой upsert
    /// оставлен как симметричный примитив хранилища (get/upsert/update) и
    /// используется тестами; прикладные писатели идут через `self_model_update`.
    #[allow(dead_code)]
    pub fn self_model_upsert(&self, model: &SelfModel) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::self_model_upsert_conn(&conn, model)?;
        Ok(())
    }

    /// Атомарное чтение-правка-запись модели профиля: SELECT + `mutate` + upsert
    /// под **одним** захватом мьютекса соединения. `mutate` возвращает `true`, если
    /// модель изменилась (иначе запись и рост `version` не делаются). Возвращает
    /// модель после правки (с авторитетными `version`/`updated_at`, если записана)
    /// и признак записи. Так исключается гонка load-modify-save между параллельными
    /// писателями (см. док к [`Self::self_model_upsert`]).
    ///
    /// `mutate` вызывается под захваченным мьютексом БД — внутри него **нельзя**
    /// обращаться к другим методам `Db` того же соединения (реентерабельный `lock()`
    /// нереентерабельного `Mutex` = дедлок); это чистая правка значения `SelfModel`.
    pub fn self_model_update(
        &self,
        profile_id: Uuid,
        mutate: impl FnOnce(&mut SelfModel) -> bool,
    ) -> Result<(SelfModel, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut model = Self::self_model_get_conn(&conn, profile_id)?
            .unwrap_or_else(|| SelfModel::new(profile_id));
        let changed = mutate(&mut model);
        if changed {
            model = Self::self_model_upsert_conn(&conn, &model)?;
        }
        Ok((model, changed))
    }

    /// Чтение модели по соединению (без захвата мьютекса — вызывается под ним).
    fn self_model_get_conn(conn: &Connection, profile_id: Uuid) -> Result<Option<SelfModel>> {
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM self_models WHERE profile_id = ?1",
                params![profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Запись модели по соединению (без захвата мьютекса — вызывается под ним).
    /// Возвращает фактически сохранённую модель (с повышенным `version` и свежим
    /// `updated_at`), чтобы вызывающий не перечитывал БД.
    fn self_model_upsert_conn(conn: &Connection, model: &SelfModel) -> Result<SelfModel> {
        // rusqlite не поддерживает u64 в ToSql/FromSql — версию храним как i64.
        let prev: Option<i64> = conn
            .query_row(
                "SELECT version FROM self_models WHERE profile_id = ?1",
                params![model.profile_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let mut stored = model.clone();
        stored.version = prev.unwrap_or(0).max(0) as u64 + 1;
        stored.updated_at = Utc::now();
        conn.execute(
            "INSERT INTO self_models(profile_id, data, version, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id) DO UPDATE SET
                 data = excluded.data,
                 version = excluded.version,
                 updated_at = excluded.updated_at",
            params![
                stored.profile_id.to_string(),
                serde_json::to_string(&stored)?,
                stored.version as i64,
                stored.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(stored)
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
    /// требует наличия файла на диске** (`/rag remove`). Возвращает число удалённых.
    pub fn rag_delete_under(&self, profile_id: Uuid, path: &str) -> Result<usize> {
        let needle = norm_path(path);
        let prefix = format!("{needle}/");
        let pred = |s: &str| {
            let s = norm_path(s);
            s == needle || s.starts_with(&prefix)
        };
        let conn = self.conn.lock().unwrap();
        let removed = delete_matching(&conn, profile_id, pred)?;
        // Снимаем и сохранённые исходники (для `/rag rebuild`) по тому же предикату.
        delete_sources_matching(&conn, profile_id, pred)?;
        Ok(removed)
    }

    /// Сохраняет (или заменяет) исходный текст индексированного источника — нужен
    /// для реиндексации (`/rag rebuild`) без обращения к файлу на диске. Изоляция
    /// по `profile_id`.
    pub fn rag_source_upsert(
        &self,
        profile_id: Uuid,
        source: &str,
        content: &str,
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO rag_sources(profile_id, source, content, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id, source) DO UPDATE SET
                 content = excluded.content, created_at = excluded.created_at",
            params![
                profile_id.to_string(),
                source,
                content,
                created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Дописывает текст к сохранённому исходнику (или создаёт его). Используется
    /// инструментом `rag_add`, который **накапливает** чанки одного источника (в
    /// отличие от файловой индексации, заменяющей источник) — чтобы реиндексация
    /// получила весь добавленный текст, а не только последний фрагмент.
    pub fn rag_source_append(
        &self,
        profile_id: Uuid,
        source: &str,
        content: &str,
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO rag_sources(profile_id, source, content, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id, source) DO UPDATE SET
                 content = content || ?5 || excluded.content",
            params![
                profile_id.to_string(),
                source,
                content,
                created_at.to_rfc3339(),
                "\n\n",
            ],
        )?;
        Ok(())
    }

    /// Сохранённые исходники профиля (для реиндексации). Изоляция по `profile_id`.
    pub fn rag_stored_sources(&self, profile_id: Uuid) -> Result<Vec<RagStoredSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source, content FROM rag_sources WHERE profile_id = ?1 ORDER BY source",
        )?;
        let rows = stmt
            .query_map(params![profile_id.to_string()], |r| {
                Ok(RagStoredSource {
                    source: r.get(0)?,
                    content: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Сводка по источникам профиля (`/rag list`): для каждого источника число
    /// чанков и дата самого раннего чанка. Сортировка — по источнику. Изоляция по
    /// `profile_id`.
    pub fn rag_list_sources(&self, profile_id: Uuid) -> Result<Vec<RagSourceInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source, COUNT(*), MIN(created_at)
             FROM rag_documents WHERE profile_id = ?1
             GROUP BY source ORDER BY source",
        )?;
        let rows = stmt
            .query_map(params![profile_id.to_string()], |r| {
                Ok(RagSourceInfo {
                    source: r.get(0)?,
                    chunks: r.get::<_, i64>(1)? as usize,
                    created_at: parse_dt(r.get::<_, String>(2)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Текущая размерность векторов RAG (общая на всю БД; `None` — ещё ничего не
    /// индексировали). Используется реиндексацией для распознавания смены модели.
    pub fn rag_dimension(&self) -> Result<Option<usize>> {
        let conn = self.conn.lock().unwrap();
        vec_dim(&conn)
    }

    /// Есть ли у **других** профилей (кроме `profile_id`) проиндексированные
    /// документы. Размерность векторов в sqlite-vec одна на всю БД, поэтому смена
    /// embedding-модели (другая размерность) затрагивает всех — этот признак
    /// позволяет реиндексации отказать, не затирая чужие данные.
    pub fn rag_other_profiles_have_docs(&self, profile_id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rag_documents WHERE profile_id <> ?1",
            params![profile_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Сбрасывает таблицу векторов целиком (drop + забыть размерность): нужно при
    /// смене embedding-модели с другой размерностью. Документы (`rag_documents`)
    /// **не** трогает — вызывающий сам удаляет/переиндексирует. Безопасно вызывать,
    /// даже если таблицы ещё нет.
    pub fn rag_reset_vectors(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DROP TABLE IF EXISTS rag_vectors", [])?;
        conn.execute("DELETE FROM meta WHERE key = 'rag_dim'", [])?;
        Ok(())
    }

    /// Удаляет все чанки (и векторы) профиля; исходники (`rag_sources`) сохраняет —
    /// они нужны для последующей реиндексации. Возвращает число удалённых чанков.
    pub fn rag_delete_all_for_profile(&self, profile_id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_matching(&conn, profile_id, |_| true)
    }
}

/// Удаляет сохранённые исходники профиля, чьи `source` проходят предикат.
fn delete_sources_matching(
    conn: &Connection,
    profile_id: Uuid,
    pred: impl Fn(&str) -> bool,
) -> Result<()> {
    let sources: Vec<String> = {
        let mut stmt = conn.prepare("SELECT source FROM rag_sources WHERE profile_id = ?1")?;
        stmt.query_map(params![profile_id.to_string()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for source in sources.into_iter().filter(|s| pred(s)) {
        conn.execute(
            "DELETE FROM rag_sources WHERE profile_id = ?1 AND source = ?2",
            params![profile_id.to_string(), source],
        )?;
    }
    Ok(())
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
         );",
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
    fn note_update_only_own_profile() {
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let note = Note::new(a, "v1", vec![]);
        let id = note.id;
        db.note_insert(&note).unwrap();
        // Чужой профиль переписать не может.
        assert!(!db.note_update(id, b, "hacked").unwrap());
        // Свой — может.
        assert!(db.note_update(id, a, "v2").unwrap());
        assert_eq!(db.note_list(a, None, &[], None).unwrap()[0].content, "v2");
        // Несуществующая заметка.
        assert!(!db.note_update(Uuid::new_v4(), a, "x").unwrap());
    }

    #[test]
    fn note_semantic_search_ranks_and_isolates() {
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let n1 = Note::new(a, "rust", vec![]);
        let n2 = Note::new(a, "banana", vec![]);
        let (id1, id2) = (n1.id, n2.id);
        db.note_insert(&n1).unwrap();
        db.note_insert(&n2).unwrap();
        db.note_vector_upsert(id1, a, &[1.0, 0.0, 0.0]).unwrap();
        db.note_vector_upsert(id2, a, &[0.0, 1.0, 0.0]).unwrap();
        // Заметка другого профиля с близким вектором — не должна попасть в выдачу a.
        let nb = Note::new(b, "other", vec![]);
        db.note_insert(&nb).unwrap();
        db.note_vector_upsert(nb.id, b, &[1.0, 0.0, 0.0]).unwrap();

        let hits = db.note_search_semantic(a, &[0.9, 0.1, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 2); // только профиль a
        assert_eq!(hits[0].0.id, id1); // ближе к [1,0,0]
        assert!(hits[0].1 > hits[1].1);

        // k ограничивает выдачу.
        let top1 = db.note_search_semantic(a, &[0.9, 0.1, 0.0], 1).unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0.id, id1);
    }

    #[test]
    fn notes_missing_vectors_lists_unembedded() {
        let db = db();
        let a = Uuid::new_v4();
        let n1 = Note::new(a, "with vec", vec![]);
        let n2 = Note::new(a, "no vec", vec![]);
        db.note_insert(&n1).unwrap();
        db.note_insert(&n2).unwrap();
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        let missing = db.notes_missing_vectors(a).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, "no vec");
    }

    #[test]
    fn note_vector_upsert_replaces() {
        let db = db();
        let a = Uuid::new_v4();
        let n = Note::new(a, "x", vec![]);
        let id = n.id;
        db.note_insert(&n).unwrap();
        db.note_vector_upsert(id, a, &[1.0, 0.0]).unwrap();
        db.note_vector_upsert(id, a, &[0.0, 1.0]).unwrap(); // замена
        let hits = db.note_search_semantic(a, &[0.0, 1.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!((hits[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn supersede_hides_note_from_list_and_search() {
        let db = db();
        let a = Uuid::new_v4();
        let old = Note::new(a, "old", vec![]);
        let new = Note::new(a, "new", vec![]);
        db.note_insert(&old).unwrap();
        db.note_insert(&new).unwrap();
        db.note_vector_upsert(old.id, a, &[1.0, 0.0]).unwrap();
        db.note_vector_upsert(new.id, a, &[1.0, 0.0]).unwrap();
        db.note_supersede_mark(a, old.id, new.id).unwrap();

        // Замещённая скрыта и из списка, и из семантики, и из is_active.
        let list = db.note_list(a, None, &[], None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, new.id);
        let hits = db.note_search_semantic(a, &[1.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, new.id);
        assert!(!db.note_is_active(a, old.id).unwrap());
        assert!(db.note_is_active(a, new.id).unwrap());
    }

    #[test]
    fn links_and_neighbors_both_directions_and_isolation() {
        let db = db();
        let a = Uuid::new_v4();
        let n1 = Note::new(a, "n1", vec![]);
        let n2 = Note::new(a, "n2", vec![]);
        let n3 = Note::new(a, "n3", vec![]);
        db.note_insert(&n1).unwrap();
        db.note_insert(&n2).unwrap();
        db.note_insert(&n3).unwrap();
        assert!(db.note_link_insert(a, n1.id, n2.id, "refines").unwrap());
        assert!(db.note_link_insert(a, n3.id, n1.id, "contradicts").unwrap());
        // Повтор той же связи не создаётся (false) — дубля в таблице нет.
        assert!(!db.note_link_insert(a, n1.id, n2.id, "refines").unwrap());

        let nb = db.note_neighbors(a, n1.id, None).unwrap();
        assert_eq!(nb.len(), 2); // исходящая на n2 + входящая от n3 (дубль не учтён)
        assert!(
            nb.iter()
                .any(|(n, r, out)| n.id == n2.id && r == "refines" && *out)
        );
        assert!(
            nb.iter()
                .any(|(n, r, out)| n.id == n3.id && r == "contradicts" && !*out)
        );

        // Фильтр по типу связи.
        let only = db.note_neighbors(a, n1.id, Some("refines")).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].0.id, n2.id);

        // Замещённый сосед исчезает из выдачи.
        let repl = Note::new(a, "n2b", vec![]);
        db.note_insert(&repl).unwrap();
        db.note_supersede_mark(a, n2.id, repl.id).unwrap();
        let nb2 = db.note_neighbors(a, n1.id, None).unwrap();
        assert!(!nb2.iter().any(|(n, _, _)| n.id == n2.id));
    }

    #[test]
    fn notes_with_vectors_active_only() {
        let db = db();
        let a = Uuid::new_v4();
        let n1 = Note::new(a, "n1", vec![]);
        let n2 = Note::new(a, "n2", vec![]);
        db.note_insert(&n1).unwrap();
        db.note_insert(&n2).unwrap();
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        db.note_vector_upsert(n2.id, a, &[0.0, 1.0]).unwrap();
        // Замещённая исключается из выдачи.
        let r = Note::new(a, "r", vec![]);
        db.note_insert(&r).unwrap();
        db.note_supersede_mark(a, n2.id, r.id).unwrap();

        let wv = db.notes_with_vectors(a).unwrap();
        assert_eq!(wv.len(), 1);
        assert_eq!(wv[0].0.id, n1.id);
        assert_eq!(wv[0].1, vec![1.0, 0.0]);
    }

    #[test]
    fn merge_link_retarget_moves_dedups_and_drops_selfloop() {
        let db = db();
        let a = Uuid::new_v4();
        let s1 = Note::new(a, "s1", vec![]);
        let s2 = Note::new(a, "s2", vec![]);
        let x = Note::new(a, "x", vec![]);
        let merged = Note::new(a, "merged", vec![]);
        for n in [&s1, &s2, &x, &merged] {
            db.note_insert(n).unwrap();
        }
        // s1→x и s2→x (после переноса станут дублем); s1→s2 (станет самопетлёй).
        db.note_link_insert(a, s1.id, x.id, "contradicts").unwrap();
        db.note_link_insert(a, s2.id, x.id, "contradicts").unwrap();
        db.note_link_insert(a, s1.id, s2.id, "relates").unwrap();

        db.note_links_retarget(a, s1.id, merged.id).unwrap();
        db.note_links_retarget(a, s2.id, merged.id).unwrap();

        let all = db.note_links_all(a).unwrap();
        assert_eq!(all.len(), 1); // дубль схлопнут, самопетля отброшена
        assert_eq!(all[0].0, merged.id);
        assert_eq!(all[0].1, x.id);
        assert_eq!(all[0].2, "contradicts");
    }

    #[test]
    fn self_model_round_trip_and_isolation() {
        use crate::entities::self_model::SelfModel;
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        // Изначально нет.
        assert!(db.self_model_get(a).unwrap().is_none());

        let mut m = SelfModel::new(a);
        m.summary = "о себе A".into();
        m.add_goal("цель A");
        db.self_model_upsert(&m).unwrap();

        let loaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(loaded.summary, "о себе A");
        assert_eq!(loaded.goals.len(), 1);
        assert_eq!(loaded.version, 1); // версия выставлена хранилищем

        // Профиль B не видит модель A.
        assert!(db.self_model_get(b).unwrap().is_none());

        // Повторный upsert повышает версию и заменяет данные.
        let mut m2 = loaded.clone();
        m2.summary = "обновлено".into();
        db.self_model_upsert(&m2).unwrap();
        let reloaded = db.self_model_get(a).unwrap().unwrap();
        assert_eq!(reloaded.summary, "обновлено");
        assert_eq!(reloaded.version, 2);
    }

    #[test]
    fn self_model_update_is_atomic_under_concurrency() {
        use std::sync::Arc;
        // Два потока параллельно дописывают инсайты в модель одного профиля. При
        // неатомарном read-modify-write часть записей терялась бы (гонка «прочитал →
        // другой записал → записал поверх»). `self_model_update` держит SELECT+upsert
        // под одним захватом мьютекса — ни одна запись не теряется.
        let db = Arc::new(db());
        let pid = Uuid::new_v4();
        let n: usize = 50;
        let handles: Vec<_> = ["A", "B"]
            .iter()
            .map(|prefix| {
                let db = db.clone();
                let prefix = prefix.to_string();
                std::thread::spawn(move || {
                    for i in 0..n {
                        db.self_model_update(pid, |m| {
                            // Накапливаем цели — считаем ровно (add_goal без потолка).
                            m.add_goal(format!("{prefix}{i}"));
                            true
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let m = db.self_model_get(pid).unwrap().unwrap();
        assert_eq!(m.goals.len(), 2 * n);
        assert_eq!(m.version, (2 * n) as u64); // каждая правка = один upsert
    }

    #[test]
    fn self_model_update_skips_write_when_unchanged() {
        let db = db();
        let pid = Uuid::new_v4();
        // mutate вернул false → записи и роста версии нет, строки в БД не появилось.
        let (model, changed) = db.self_model_update(pid, |_m| false).unwrap();
        assert!(!changed);
        assert_eq!(model.version, 0);
        assert!(db.self_model_get(pid).unwrap().is_none());
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
    fn rag_list_sources_aggregates_chunks_per_source() {
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c1", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "c2", vec![0.0, 1.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/b.md", "c3", vec![1.0, 1.0]))
            .unwrap();
        // Чужой профиль не попадает в выдачу.
        db.rag_insert(&RagDocument::new(
            Uuid::new_v4(),
            "/o.txt",
            "x",
            vec![1.0, 0.0],
        ))
        .unwrap();

        let sources = db.rag_list_sources(p).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].source, "/data/a.txt");
        assert_eq!(sources[0].chunks, 2);
        assert_eq!(sources[1].source, "/data/b.md");
        assert_eq!(sources[1].chunks, 1);
    }

    #[test]
    fn rag_sources_store_and_delete_with_chunks() {
        let db = db();
        let p = Uuid::new_v4();
        db.rag_source_upsert(p, "/data/a.txt", "полный текст", Utc::now())
            .unwrap();
        db.rag_insert(&RagDocument::new(p, "/data/a.txt", "чанк", vec![1.0, 0.0]))
            .unwrap();
        // Повторный upsert заменяет содержимое, а не плодит дубликат.
        db.rag_source_upsert(p, "/data/a.txt", "новый текст", Utc::now())
            .unwrap();
        let stored = db.rag_stored_sources(p).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, "новый текст");

        // Удаление под путём снимает и чанки, и сохранённый исходник.
        assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 1);
        assert!(db.rag_stored_sources(p).unwrap().is_empty());
    }

    #[test]
    fn rag_rebuild_dimension_change_flow() {
        let db = db();
        let p = Uuid::new_v4();
        // Индексировано в размерности 2.
        db.rag_insert(&RagDocument::new(p, "a", "c", vec![1.0, 0.0]))
            .unwrap();
        assert_eq!(db.rag_dimension().unwrap(), Some(2));
        assert!(!db.rag_other_profiles_have_docs(p).unwrap());

        // Реиндексация в размерность 3 невозможна без сброса (mismatch).
        assert!(
            db.rag_insert(&RagDocument::new(p, "a", "c", vec![0.0, 1.0, 0.0]))
                .is_err()
        );
        // Сбрасываем векторы и чистим документы профиля, затем индексируем в новой размерности.
        db.rag_delete_all_for_profile(p).unwrap();
        db.rag_reset_vectors().unwrap();
        assert_eq!(db.rag_dimension().unwrap(), None);
        db.rag_insert(&RagDocument::new(p, "a", "c", vec![0.0, 1.0, 0.0]))
            .unwrap();
        assert_eq!(db.rag_dimension().unwrap(), Some(3));
        assert_eq!(db.rag_count(p).unwrap(), 1);
    }

    #[test]
    fn rag_other_profiles_have_docs_detects_neighbors() {
        let db = db();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(a, "a", "c", vec![1.0, 0.0]))
            .unwrap();
        assert!(!db.rag_other_profiles_have_docs(a).unwrap());
        db.rag_insert(&RagDocument::new(b, "b", "c", vec![0.0, 1.0]))
            .unwrap();
        assert!(db.rag_other_profiles_have_docs(a).unwrap());
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
