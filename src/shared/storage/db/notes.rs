//! Хранилище (SQLite) — заметки: вставка/список/правка/удаление + эмбеддинги/семантика. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/history/refactoring-god-objects.md, этап 5).

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
    fn note_delete_removes_and_is_profile_isolated() {
        let db = db();
        let p = Uuid::new_v4();
        let other = Uuid::new_v4();
        let n = Note::new(p, "наблюдение", vec![]);
        let id = n.id;
        db.note_insert(&n).unwrap();
        db.note_vector_upsert(id, p, &[1.0, 0.0]).unwrap();
        // Чужой профиль не удаляет.
        assert!(!db.note_delete(other, id).unwrap());
        assert_eq!(db.note_list(p, None, &[], None).unwrap().len(), 1);
        // Свой — удаляет заметку (и её вектор).
        assert!(db.note_delete(p, id).unwrap());
        assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
        // Заметки без вектора нет (обе таблицы пусты) — вектор снят вместе с заметкой.
        assert!(db.notes_missing_vectors(p).unwrap().is_empty());
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
        assert!(db.note_delete(p, note.id).unwrap());
        assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
    }
}
