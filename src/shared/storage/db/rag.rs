//! Storage (SQLite) — RAG: documents/search/sources/dimensionality + delete by
//! path. Part of the [`super`] module; split out of the db.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 5).

use super::*;
use crate::shared::embed_identity::EmbedFingerprint;

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
        // The table (not the recorded dimensionality): `meta.rag_dim` is shared
        // with the attachment index, so it may already be set while nothing has
        // been indexed into RAG yet.
        if !table_exists(&conn, "rag_vectors")? {
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

    // ---------- embedding-model identity ----------
    //
    // Dimensionality alone cannot tell two embedding models apart (`bge-m3` and
    // `multilingual-e5-large-instruct` are both 1024-d yet embed the same text to
    // a cosine of ~0.37), so identity is established behaviourally: the canary
    // vector recorded here is compared against a fresh one on startup. See
    // [`crate::shared::embed_identity`] and
    // docs/research/embedding-model-change-reindex.md.

    /// The embedding-model fingerprint the stored vectors were produced under.
    /// `None` — nothing recorded yet (a fresh DB, or after [`Self::reset_vectors`]),
    /// which the caller reads as "no basis to compare" rather than "the model
    /// changed": there is nothing stale to invalidate either way.
    ///
    /// A stored value that does not parse also reads as `None` — deliberately, so
    /// a hand-edited `data.db` cannot brick startup; recording the current model
    /// repairs it.
    pub fn embed_fingerprint(&self) -> Result<Option<EmbedFingerprint>> {
        let conn = self.conn.lock().unwrap();
        let Some(raw) = meta_get(&conn, KEY_EMBED_CANARY)? else {
            return Ok(None);
        };
        // An empty vector is treated as absent too: it can never match anything
        // (cosine is 0 for an empty vector), so keeping it would permanently
        // report "the model changed" on every launch.
        match serde_json::from_str::<Vec<f32>>(&raw) {
            Ok(canary) if !canary.is_empty() => Ok(Some(EmbedFingerprint::new(
                canary,
                meta_get(&conn, KEY_EMBED_MODEL_ID)?,
            ))),
            _ => Ok(None),
        }
    }

    /// Records the fingerprint of the model now in use, replacing any previous
    /// one. Called after the stored vectors have been brought in line with it
    /// (invalidated or reindexed) — writing it earlier would lose the fact that
    /// the change ever happened.
    pub fn set_embed_fingerprint(&self, fp: &EmbedFingerprint) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        meta_set(&conn, KEY_EMBED_CANARY, &serde_json::to_string(&fp.canary)?)?;
        match &fp.model_id {
            Some(id) => meta_set(&conn, KEY_EMBED_MODEL_ID, id),
            // Removed rather than blanked: a stale name from the previous model
            // would otherwise be shown as if it belonged to this one.
            None => meta_del(&conn, KEY_EMBED_MODEL_ID),
        }
    }

    // ---------- RAG staleness after a model change ----------
    //
    // Note vectors and the attachment index can simply be dropped (both re-embed
    // themselves lazily), but RAG chunks cannot: re-embedding them needs the full
    // ingest pipeline, which the user drives with `/rag rebuild`. So instead of
    // silently deleting a knowledge base — the user's own data — a model change
    // **records** which profiles it invalidated, and `rag_search` refuses over
    // those until they are rebuilt. Refusing rather than warning: the vectors are
    // in a different space, so the results would be noise dressed up as answers.

    /// Profiles that currently have RAG documents — the set a model change
    /// invalidates, captured before the fingerprint is overwritten.
    pub fn profiles_with_rag_docs(&self) -> Result<Vec<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT profile_id FROM rag_documents")?;
        let ids = stmt
            .query_map([], |r| Ok(parse_uuid(r.get::<_, String>(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Profiles whose RAG documents predate the current embedding model.
    // The whole-list read: the hot path uses `rag_is_stale`, and the set is
    // written wholesale by the guard — this is for a UI consumer ("these
    // knowledge bases need rebuilding") that does not exist yet.
    #[allow(dead_code)]
    pub fn rag_stale_profiles(&self) -> Result<Vec<Uuid>> {
        let conn = self.conn.lock().unwrap();
        read_stale_profiles(&conn)
    }

    /// Replaces the stale set. An empty slice clears the record entirely.
    pub fn set_rag_stale_profiles(&self, profiles: &[Uuid]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        write_stale_profiles(&conn, profiles)
    }

    /// Marks one profile's knowledge base as reindexed (a successful `/rag
    /// rebuild`) — its documents now match the current model again. A no-op when
    /// the profile was not stale.
    pub fn clear_rag_stale_profile(&self, profile_id: Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stale = read_stale_profiles(&conn)?;
        stale.retain(|p| *p != profile_id);
        write_stale_profiles(&conn, &stale)
    }

    /// Whether this profile's knowledge base is stale. Sits on the search path,
    /// so it is a single `meta` read rather than a scan.
    pub fn rag_is_stale(&self, profile_id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(read_stale_profiles(&conn)?.contains(&profile_id))
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

    /// Drops **both** vector tables (RAG and the chat attachment index) and
    /// forgets everything recorded about the model that produced them: the shared
    /// dimensionality, the fingerprint, and the stale-profile list. Needed when
    /// switching to an embedding model with a different dimensionality. Does
    /// **not** touch `rag_documents` (the caller deletes/reindexes them itself),
    /// but **does** delete `attachment_documents`: their vectors are gone, and
    /// leaving the rows would dangle on rowids sqlite reuses. Returns how many
    /// attachment chunks were dropped, so the caller can report the loss instead
    /// of hiding it. Safe to call even if nothing has been indexed yet.
    ///
    /// This is the "start completely fresh" primitive, hence clearing the
    /// fingerprint too: afterwards there are no vectors to be stale *relative to*,
    /// so the next launch has nothing to compare and records the current model as
    /// the new baseline. Leaving a fingerprint behind would report a model change
    /// against vectors that no longer exist.
    ///
    /// The attachment index is derived data: re-attaching the file rebuilds it,
    /// and `attachment_read` (the guaranteed path) is unaffected.
    pub fn reset_vectors(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let dropped: i64 =
            conn.query_row("SELECT COUNT(*) FROM attachment_documents", [], |r| {
                r.get(0)
            })?;
        conn.execute("DROP TABLE IF EXISTS rag_vectors", [])?;
        conn.execute("DROP TABLE IF EXISTS attachment_vectors", [])?;
        conn.execute("DELETE FROM attachment_documents", [])?;
        meta_del(&conn, KEY_RAG_DIM)?;
        meta_del(&conn, KEY_EMBED_CANARY)?;
        meta_del(&conn, KEY_EMBED_MODEL_ID)?;
        meta_del(&conn, KEY_RAG_STALE_PROFILES)?;
        Ok(dropped as usize)
    }

    /// Deletes all of a profile's chunks (and vectors); keeps sources
    /// (`rag_sources`) — needed for subsequent reindexing. Returns the number of
    /// chunks deleted.
    pub fn rag_delete_all_for_profile(&self, profile_id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_matching(&conn, profile_id, |_| true)
    }
}

/// Reads the stale-profile list. Free functions (rather than calling the public
/// methods) because `self.conn` is a non-reentrant [`std::sync::Mutex`]: a
/// read-modify-write like [`Db::clear_rag_stale_profile`] must do both halves
/// under one acquisition.
///
/// A value that does not parse reads as an empty list — same reasoning as the
/// fingerprint: garbage in `meta` must degrade to "nothing recorded", never to a
/// startup failure. Individual unparseable ids are skipped rather than mapped to
/// a nil uuid, which would be a real profile id nothing matches.
fn read_stale_profiles(conn: &Connection) -> Result<Vec<Uuid>> {
    let Some(raw) = meta_get(conn, KEY_RAG_STALE_PROFILES)? else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
}

/// Writes the stale-profile list, removing the key when the list is empty (see
/// [`meta_del`] — "nothing is stale" is the absence of the key).
fn write_stale_profiles(conn: &Connection, profiles: &[Uuid]) -> Result<()> {
    if profiles.is_empty() {
        return meta_del(conn, KEY_RAG_STALE_PROFILES);
    }
    let ids: Vec<String> = profiles.iter().map(|p| p.to_string()).collect();
    meta_set(conn, KEY_RAG_STALE_PROFILES, &serde_json::to_string(&ids)?)
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
    let has_vectors = table_exists(conn, "rag_vectors")?;
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
        db.reset_vectors().unwrap();
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

    // ---------- embedding-model identity ----------

    #[test]
    fn embed_fingerprint_round_trips() {
        let db = db();
        assert!(db.embed_fingerprint().unwrap().is_none(), "fresh DB");

        let fp = EmbedFingerprint::new(vec![0.1, -0.2, 0.3], Some("bge-m3".into()));
        db.set_embed_fingerprint(&fp).unwrap();
        assert_eq!(db.embed_fingerprint().unwrap(), Some(fp));

        // A fingerprint without a model name round-trips too — an external server
        // that reports nothing useful still gets a usable canary.
        let anon = EmbedFingerprint::new(vec![1.0, 0.0], None);
        db.set_embed_fingerprint(&anon).unwrap();
        assert_eq!(db.embed_fingerprint().unwrap(), Some(anon));
    }

    #[test]
    fn embed_fingerprint_replaces_rather_than_duplicating() {
        let db = db();
        db.set_embed_fingerprint(&EmbedFingerprint::new(vec![1.0, 0.0], Some("old".into())))
            .unwrap();
        let fresh = EmbedFingerprint::new(vec![0.0, 1.0], Some("new".into()));
        db.set_embed_fingerprint(&fresh).unwrap();
        assert_eq!(db.embed_fingerprint().unwrap(), Some(fresh));

        let conn = db.conn.lock().unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key = ?1",
                [KEY_EMBED_CANARY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "one canary row, not an append-only log");
    }

    #[test]
    fn embed_fingerprint_clears_stale_model_name() {
        // Overwriting a named model with an anonymous one must not leave the old
        // name behind — it would be shown as if it belonged to the new model.
        let db = db();
        db.set_embed_fingerprint(&EmbedFingerprint::new(vec![1.0], Some("bge-m3".into())))
            .unwrap();
        db.set_embed_fingerprint(&EmbedFingerprint::new(vec![1.0], None))
            .unwrap();
        assert_eq!(db.embed_fingerprint().unwrap().unwrap().model_id, None);
    }

    #[test]
    fn corrupt_fingerprint_reads_as_absent() {
        // A hand-edited data.db must not brick startup: garbage degrades to
        // "nothing recorded", and the next launch records the current model.
        for bad in ["not json", "{}", "[\"a\"]", "[]"] {
            let db = db();
            {
                let conn = db.conn.lock().unwrap();
                meta_set(&conn, KEY_EMBED_CANARY, bad).unwrap();
                meta_set(&conn, KEY_EMBED_MODEL_ID, "some-model").unwrap();
            }
            assert!(
                db.embed_fingerprint().unwrap().is_none(),
                "canary {bad:?} must read as absent"
            );
        }
    }

    // ---------- RAG staleness ----------

    #[test]
    fn profiles_with_rag_docs_lists_only_those_holding_documents() {
        let db = db();
        let (a, b, empty) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(a, "s", "t", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(a, "s2", "t", vec![0.0, 1.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(b, "s", "t", vec![1.0, 1.0]))
            .unwrap();
        // `empty` has a stored source but no chunks — nothing to invalidate.
        db.rag_source_upsert(empty, "s", "text", Utc::now())
            .unwrap();

        let mut found = db.profiles_with_rag_docs().unwrap();
        found.sort();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(
            found, expected,
            "deduplicated, and no profile without chunks"
        );
    }

    #[test]
    fn stale_profiles_round_trip_and_clear_one() {
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(db.rag_stale_profiles().unwrap().is_empty(), "fresh DB");
        assert!(!db.rag_is_stale(a).unwrap());

        db.set_rag_stale_profiles(&[a, b]).unwrap();
        assert_eq!(db.rag_stale_profiles().unwrap(), vec![a, b]);
        assert!(db.rag_is_stale(a).unwrap());
        assert!(db.rag_is_stale(b).unwrap());

        // A successful /rag rebuild clears exactly one profile.
        db.clear_rag_stale_profile(a).unwrap();
        assert_eq!(db.rag_stale_profiles().unwrap(), vec![b]);
        assert!(!db.rag_is_stale(a).unwrap());
        assert!(db.rag_is_stale(b).unwrap());

        // Clearing a profile that was never stale is a no-op.
        db.clear_rag_stale_profile(Uuid::new_v4()).unwrap();
        assert_eq!(db.rag_stale_profiles().unwrap(), vec![b]);
    }

    #[test]
    fn empty_stale_list_removes_the_key() {
        // Absence is the "nothing is stale" state — a stored "[]" would be a
        // second way to say the same thing.
        let db = db();
        db.set_rag_stale_profiles(&[Uuid::new_v4()]).unwrap();
        db.set_rag_stale_profiles(&[]).unwrap();
        assert!(db.rag_stale_profiles().unwrap().is_empty());

        let conn = db.conn.lock().unwrap();
        assert_eq!(meta_get(&conn, KEY_RAG_STALE_PROFILES).unwrap(), None);
    }

    #[test]
    fn clearing_the_last_stale_profile_removes_the_key() {
        let db = db();
        let a = Uuid::new_v4();
        db.set_rag_stale_profiles(&[a]).unwrap();
        db.clear_rag_stale_profile(a).unwrap();

        let conn = db.conn.lock().unwrap();
        assert_eq!(meta_get(&conn, KEY_RAG_STALE_PROFILES).unwrap(), None);
    }

    #[test]
    fn corrupt_stale_list_reads_as_empty() {
        for bad in ["not json", "{}", "[\"not-a-uuid\"]"] {
            let db = db();
            {
                let conn = db.conn.lock().unwrap();
                meta_set(&conn, KEY_RAG_STALE_PROFILES, bad).unwrap();
            }
            assert!(
                db.rag_stale_profiles().unwrap().is_empty(),
                "stale list {bad:?} must read as empty"
            );
        }
    }

    #[test]
    fn reset_vectors_forgets_the_model_it_indexed_under() {
        // "Start completely fresh": with no vectors left there is nothing to be
        // stale relative to, so a leftover fingerprint would report a model change
        // against data that no longer exists.
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "s", "t", vec![1.0, 0.0]))
            .unwrap();
        db.set_embed_fingerprint(&EmbedFingerprint::new(
            vec![1.0, 0.0],
            Some("bge-m3".into()),
        ))
        .unwrap();
        db.set_rag_stale_profiles(&[p]).unwrap();

        db.reset_vectors().unwrap();

        assert_eq!(db.rag_dimension().unwrap(), None);
        assert!(db.embed_fingerprint().unwrap().is_none());
        assert!(db.rag_stale_profiles().unwrap().is_empty());
        let conn = db.conn.lock().unwrap();
        assert_eq!(meta_get(&conn, KEY_EMBED_MODEL_ID).unwrap(), None);
    }
}
