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
