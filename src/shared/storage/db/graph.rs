//! Хранилище (SQLite) — граф связей заметок + замещение + цитирование источников. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/refactoring-god-objects.md, этап 5).

use super::*;

impl Db {
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

    /// Связывает заметку с RAG-источником (по имени источника — стабильно к
    /// переиндексации, в отличие от id чанков). Идемпотентно по PK. Возвращает
    /// `true`, если связь действительно создана. Изоляция по `profile_id`.
    pub fn note_cite_source_insert(
        &self,
        profile_id: Uuid,
        note_id: Uuid,
        source: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT OR IGNORE INTO note_rag_links(profile_id, note_id, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                profile_id.to_string(),
                note_id.to_string(),
                source,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(n > 0)
    }

    /// RAG-источники, на которые ссылается заметка (для показа при припоминании).
    /// Изоляция по `profile_id`.
    pub fn note_cited_sources(&self, profile_id: Uuid, note_id: Uuid) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source FROM note_rag_links
             WHERE profile_id = ?1 AND note_id = ?2 ORDER BY source",
        )?;
        let rows = stmt.query_map(params![profile_id.to_string(), note_id.to_string()], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Активные (не замещённые) заметки профиля, ссылающиеся на данный RAG-источник —
    /// обратное направление (поиск заметок через RAG, «оба органа»). Изоляция по
    /// `profile_id`.
    pub fn notes_citing_source(&self, profile_id: Uuid, source: &str) -> Result<Vec<Note>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.profile_id, n.content, n.tags, n.created_at, n.updated_at
             FROM note_rag_links l
             JOIN notes n ON n.id = l.note_id
             LEFT JOIN note_superseded s ON s.note_id = n.id
             WHERE l.profile_id = ?1 AND l.source = ?2 AND s.note_id IS NULL
             ORDER BY n.updated_at DESC",
        )?;
        let rows = stmt.query_map(params![profile_id.to_string(), source], row_to_note)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
}
