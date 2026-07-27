//! Storage (SQLite) — notes: insert/list/edit/delete + embeddings/semantics. Part
//! of the [`super`] module; split out of the db.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 5).

use super::*;

impl Db {
    // ---------- notes ----------

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

    /// A profile's notes: an optional content-substring filter and tag filter,
    /// sorted by `updated_at` desc, an optional limit. Isolation by profile.
    pub fn note_list(
        &self,
        profile_id: Uuid,
        query: Option<&str>,
        tags: &[String],
        limit: Option<usize>,
    ) -> Result<Vec<Note>> {
        let conn = self.conn.lock().unwrap();
        // Superseded notes are hidden from active output (kept for the "scar"/
        // trace) — anti-join against note_superseded.
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

    /// Hard-deletes a profile's note by id (along with its vector). Used by
    /// deleting an observation from the `F3` screen (observations are self-notes).
    /// Isolation by `profile_id` in `WHERE`. Returns whether the note was deleted.
    pub fn note_delete(&self, profile_id: Uuid, id: Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        // The vector is deleted unconditionally (a side table; a foreign profile
        // cannot land here, since note_id is unique and the profile check is on
        // the note itself below).
        conn.execute(
            "DELETE FROM note_vectors WHERE note_id = ?1",
            params![id.to_string()],
        )?;
        // Also drop this note's links to RAG sources (Tier 3, Path 3).
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

    /// Rewrites a note's content in place (a revision), bumping `updated_at`.
    /// Isolation by `profile_id` in `WHERE`. `false` if the note is not found/foreign.
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

    /// Saves/replaces a note's embedding (for semantic search). The vector is a
    /// JSON array of f32 in a side table (deliberately NOT vec0: there are only a
    /// few notes, cosine is computed in Rust — see [`Self::note_search_semantic`]).
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

    /// Semantic search of a profile's notes by cosine similarity to `query`.
    /// Brute-force in Rust (notes number in the tens–hundreds); notes without an
    /// embedding are skipped. Returns up to `k` pairs (note, similarity) in
    /// descending order. Isolation — `WHERE n.profile_id = ?`.
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

    /// Drops every stored note embedding, across **all profiles**. The note
    /// content is untouched, so [`Self::notes_missing_vectors`] will list them
    /// again and `ensure_note_vectors` re-embeds them lazily on the next semantic
    /// path — the invalidation is self-healing and costs the user nothing.
    /// Returns how many vectors were dropped.
    ///
    /// Global on purpose, and not a breach of the `profile_id` isolation
    /// invariant (spec §10.3): the embedder is a single global server, so a model
    /// change invalidates every profile's vectors at once. Scoping this per
    /// profile would leave the others silently comparing a fresh query against
    /// vectors from a different vector space.
    pub fn note_vectors_clear_all(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM note_vectors", [])?)
    }

    /// A profile's notes that still have no embedding (for backfilling "old"
    /// notes created before vector search, imported, or saved while the embedder
    /// was unavailable at the time). Returns pairs (id, content).
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

    /// A note by id within a profile (including a superseded one) — for reading
    /// tags on supersession/merging: the new version inherits the source's tags
    /// (including `@self`, so a self-note doesn't "fall out" into user-facing
    /// output). `None` — not found/foreign.
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

    // ---------- link graph and "scars" (Tier 2) ----------

    /// A profile's note exists and is not superseded (for checking a link's ends).
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

    /// A profile's active notes with their embeddings (for consolidation: finding
    /// duplicates via pairwise cosine). Superseded ones are excluded.
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
        // Profile B is not visible from A.
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
        // A foreign profile cannot overwrite it.
        assert!(!db.note_update(id, b, "hacked").unwrap());
        // Its own profile can.
        assert!(db.note_update(id, a, "v2").unwrap());
        assert_eq!(db.note_list(a, None, &[], None).unwrap()[0].content, "v2");
        // A nonexistent note.
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
        // Another profile's note with a close vector must not leak into a's output.
        let nb = Note::new(b, "other", vec![]);
        db.note_insert(&nb).unwrap();
        db.note_vector_upsert(nb.id, b, &[1.0, 0.0, 0.0]).unwrap();

        let hits = db.note_search_semantic(a, &[0.9, 0.1, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 2); // profile a only
        assert_eq!(hits[0].0.id, id1); // closer to [1,0,0]
        assert!(hits[0].1 > hits[1].1);

        // k limits the output.
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
        db.note_vector_upsert(id, a, &[0.0, 1.0]).unwrap(); // replacement
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
        // A superseded one is excluded from the output.
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
        // A foreign profile does not delete it.
        assert!(!db.note_delete(other, id).unwrap());
        assert_eq!(db.note_list(p, None, &[], None).unwrap().len(), 1);
        // Its own profile deletes the note (and its vector).
        assert!(db.note_delete(p, id).unwrap());
        assert!(db.note_list(p, None, &[], None).unwrap().is_empty());
        // No note lacking a vector (both tables are empty) — the vector was
        // removed along with the note.
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
    fn note_vectors_clear_all_keeps_notes_and_self_heals() {
        // The invalidation used when the embedding model changes: the vectors are
        // worthless in the new space, but the notes themselves are intact — so
        // they must come back through the ordinary lazy-backfill path.
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let n1 = Note::new(a, "n1", vec![]);
        let n2 = Note::new(a, "n2", vec![]);
        let nb = Note::new(b, "other profile", vec![]);
        db.note_insert(&n1).unwrap();
        db.note_insert(&n2).unwrap();
        db.note_insert(&nb).unwrap();
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        db.note_vector_upsert(n2.id, a, &[0.0, 1.0]).unwrap();
        db.note_vector_upsert(nb.id, b, &[1.0, 1.0]).unwrap();

        // Global: the embedder is one server, so every profile is invalidated.
        assert_eq!(db.note_vectors_clear_all().unwrap(), 3);

        // The notes survive...
        assert_eq!(db.note_list(a, None, &[], None).unwrap().len(), 2);
        assert_eq!(db.note_list(b, None, &[], None).unwrap().len(), 1);
        // ...semantic search finds nothing until they are re-embedded...
        assert!(
            db.note_search_semantic(a, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        // ...and the backfill path lists them, which is what re-embeds them.
        assert_eq!(db.notes_missing_vectors(a).unwrap().len(), 2);
        assert_eq!(db.notes_missing_vectors(b).unwrap().len(), 1);

        // Re-embedding restores search — the self-healing property.
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        assert_eq!(db.note_search_semantic(a, &[1.0, 0.0], 5).unwrap().len(), 1);
    }

    #[test]
    fn note_vectors_clear_all_is_safe_on_an_empty_db() {
        let db = db();
        assert_eq!(db.note_vectors_clear_all().unwrap(), 0);
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
