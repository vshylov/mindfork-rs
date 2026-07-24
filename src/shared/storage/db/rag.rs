//! Storage (SQLite) — RAG: documents/search/sources/dimensionality + delete by
//! path. Part of the [`super`] module; split out of the db.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 5).

use super::*;

impl Db {
    // ---------- linking notes to RAG sources (Tier 3, Path 3) ----------

    /// Whether a profile has a RAG source with this name (in chunks or stored
    /// sources). For validating `note_cite_source` — citing is only allowed for a
    /// source that actually exists. Isolation by `profile_id`.
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

    /// kNN search over a profile's knowledge base (isolation by `profile_id` via
    /// a partition key).
    pub fn rag_search(&self, profile_id: Uuid, query: &[f32], k: usize) -> Result<Vec<RagHit>> {
        let conn = self.conn.lock().unwrap();
        if vec_dim(&conn)?.is_none() {
            return Ok(Vec::new()); // nothing indexed yet
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

    /// Number of a profile's RAG documents (a repository operation; a UI
    /// consumer — later).
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

    /// Deletes all documents (chunks) with an exact `source` match for a profile.
    /// Returns the number deleted. Used by idempotent file reindexing (`/rag
    /// add` — replaces the source's previous chunks instead of duplicating them).
    pub fn rag_delete_by_source(&self, profile_id: Uuid, source: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_matching(&conn, profile_id, |s| s == source)
    }

    /// Deletes documents by path: the path itself (a file) and everything under
    /// it (a directory). Comparison is robust to separators (`/` ↔ `\`) and case
    /// (Windows) and **does not require the file to exist on disk** (`/rag
    /// remove`). Returns the number deleted.
    pub fn rag_delete_under(&self, profile_id: Uuid, path: &str) -> Result<usize> {
        let needle = norm_path(path);
        let prefix = format!("{needle}/");
        let pred = |s: &str| {
            let s = norm_path(s);
            s == needle || s.starts_with(&prefix)
        };
        let conn = self.conn.lock().unwrap();
        let removed = delete_matching(&conn, profile_id, pred)?;
        // Also drop stored sources (for `/rag rebuild`) via the same predicate.
        delete_sources_matching(&conn, profile_id, pred)?;
        Ok(removed)
    }

    /// Saves (or replaces) an indexed source's raw text — needed for reindexing
    /// (`/rag rebuild`) without reading the file from disk. Isolation by
    /// `profile_id`.
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

    /// Appends text to a stored source (or creates it). Used by the `rag_add`
    /// tool, which **accumulates** chunks of a single source (unlike file
    /// indexing, which replaces the source) — so that reindexing gets all the
    /// added text, not just the last fragment.
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

    /// A profile's stored sources (for reindexing). Isolation by `profile_id`.
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

    /// Summary of a profile's sources (`/rag list`): for each source, the chunk
    /// count and the date of the earliest chunk. Sorted by source. Isolation by
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

    /// Current RAG vector dimensionality (shared across the whole DB; `None` —
    /// nothing indexed yet). Used by reindexing to detect a model change.
    pub fn rag_dimension(&self) -> Result<Option<usize>> {
        let conn = self.conn.lock().unwrap();
        vec_dim(&conn)
    }

    /// Whether **other** profiles (besides `profile_id`) have indexed documents.
    /// The vector dimensionality in sqlite-vec is one for the whole DB, so
    /// changing the embedding model (a different dimensionality) affects
    /// everyone — this flag lets reindexing refuse rather than overwrite someone
    /// else's data.
    pub fn rag_other_profiles_have_docs(&self, profile_id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rag_documents WHERE profile_id <> ?1",
            params![profile_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Drops the vector table entirely (drop + forget the dimensionality): needed
    /// when switching to an embedding model with a different dimensionality.
    /// Does **not** touch documents (`rag_documents`) — the caller deletes/
    /// reindexes them itself. Safe to call even if the table doesn't exist yet.
    pub fn rag_reset_vectors(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DROP TABLE IF EXISTS rag_vectors", [])?;
        conn.execute("DELETE FROM meta WHERE key = 'rag_dim'", [])?;
        Ok(())
    }

    /// Deletes all of a profile's chunks (and vectors); keeps sources
    /// (`rag_sources`) — needed for subsequent reindexing. Returns the number of
    /// chunks deleted.
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

/// Deletes a profile's documents whose `source` passes the predicate (along with
/// their vectors in `vec0`). Returns the number of documents deleted.
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
    // The vector table only exists after the first insert; there's nothing to
    // delete without it.
    let has_vectors = vec_dim(conn)?.is_some();
    for rowid in &victims {
        if has_vectors {
            conn.execute("DELETE FROM rag_vectors WHERE rowid = ?1", params![rowid])?;
        }
        conn.execute("DELETE FROM rag_documents WHERE rowid = ?1", params![rowid])?;
    }
    Ok(victims.len())
}

/// Normalizes a path for robust comparison: separators to `/`, no trailing
/// slash, lowercase on Windows (NTFS is case-insensitive).
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
        // Profile B has a vector identical to the query — it must not "leak"
        // into A's search.
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

        // Both of a.txt's chunks are deleted, b.txt remains.
        assert_eq!(db.rag_delete_by_source(p, "/data/a.txt").unwrap(), 2);
        assert_eq!(db.rag_count(p).unwrap(), 1);
        // Search no longer finds them either (vectors deleted).
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
        // A foreign profile with the same path must not be affected (isolation).
        db.rag_insert(&RagDocument::new(other, "/data/a.txt", "x", vec![1.0, 0.0]))
            .unwrap();

        // Deleting the directory removes the file and nested ones, but not
        // "/other" and not the foreign profile.
        assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 2);
        assert_eq!(db.rag_count(p).unwrap(), 1);
        assert_eq!(db.rag_count(other).unwrap(), 1);

        // The prefix does not catch a neighboring directory sharing the name's start.
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
        // Backslashes and a trailing slash in the query match the stored
        // "/"-source.
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
        // A foreign profile does not appear in the output.
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
        // A repeat upsert replaces the content instead of creating a duplicate.
        db.rag_source_upsert(p, "/data/a.txt", "новый текст", Utc::now())
            .unwrap();
        let stored = db.rag_stored_sources(p).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, "новый текст");

        // Deleting under the path removes both the chunks and the stored source.
        assert_eq!(db.rag_delete_under(p, "/data").unwrap(), 1);
        assert!(db.rag_stored_sources(p).unwrap().is_empty());
    }

    #[test]
    fn rag_rebuild_dimension_change_flow() {
        let db = db();
        let p = Uuid::new_v4();
        // Indexed at dimensionality 2.
        db.rag_insert(&RagDocument::new(p, "a", "c", vec![1.0, 0.0]))
            .unwrap();
        assert_eq!(db.rag_dimension().unwrap(), Some(2));
        assert!(!db.rag_other_profiles_have_docs(p).unwrap());

        // Reindexing to dimensionality 3 is impossible without a reset (mismatch).
        assert!(
            db.rag_insert(&RagDocument::new(p, "a", "c", vec![0.0, 1.0, 0.0]))
                .is_err()
        );
        // Reset the vectors and clear the profile's documents, then index at the
        // new dimensionality.
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
