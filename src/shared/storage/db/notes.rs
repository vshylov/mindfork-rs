//! Хранилище (SQLite) — заметки: вставка/список/правка/удаление + эмбеддинги/семантика. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/refactoring-god-objects.md, этап 5).

use super::*;

impl Db {
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

    /// Жёсткое удаление заметки профиля по id (вместе с её вектором). Используется
    /// удалением наблюдения из экрана `F3` (наблюдения — self-заметки). Изоляция по
    /// `profile_id` в `WHERE`. Возвращает, была ли удалена заметка.
    pub fn note_delete(&self, profile_id: Uuid, id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        // Вектор удаляем безусловно (боковая таблица; чужой профиль сюда не попадёт,
        // т.к. note_id уникален и проверка профиля — на самой заметке ниже).
        conn.execute(
            "DELETE FROM note_vectors WHERE note_id = ?1",
            params![id.to_string()],
        )?;
        // Ссылки на RAG-источники этой заметки тоже снимаем (Ярус 3, Путь 3).
        conn.execute(
            "DELETE FROM note_rag_links WHERE profile_id = ?1 AND note_id = ?2",
            params![profile_id.to_string(), id.to_string()],
        )?;
        let n = conn.execute(
            "DELETE FROM notes WHERE id = ?1 AND profile_id = ?2",
            params![id.to_string(), profile_id.to_string()],
        )?;
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
}
