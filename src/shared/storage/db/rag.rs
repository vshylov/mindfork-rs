//! Хранилище (SQLite) — RAG: документы/поиск/источники/размерность + удаление по пути. Часть модуля [`super`]; разбито из
//! монолита db.rs (см. docs/refactoring-god-objects.md, этап 5).

use super::*;

impl Db {
    // ---------- связи заметок с RAG-источниками (Ярус 3, Путь 3) ----------

    /// Есть ли у профиля RAG-источник с таким именем (в чанках или сохранённых
    /// исходниках). Для валидации `note_cite_source` — ссылаться можно лишь на
    /// реально существующий источник. Изоляция по `profile_id`.
    pub fn rag_source_exists(&self, profile_id: Uuid, source: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 WHERE EXISTS(
                     SELECT 1 FROM rag_documents WHERE profile_id = ?1 AND source = ?2)
                   OR EXISTS(
                     SELECT 1 FROM rag_sources WHERE profile_id = ?1 AND source = ?2)",
                params![profile_id.to_string(), source],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
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
