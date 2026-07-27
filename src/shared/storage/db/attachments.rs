//! Storage (SQLite) — the chat-scoped semantic index over file attachments
//! (`/file attach`, stage 3 of docs/file-attachments.md). Part of the [`super`]
//! module.
//!
//! **Why a separate index rather than a `chat_id` column on `rag_documents`**
//! (fork F11, confirmed by the user 2026-07-27): `rag_vectors` is a vec0 table
//! partitioned by `profile_id`, and the `k` constraint applies **inside** the
//! partition — a `WHERE chat_id = …` in the join would filter *after* kNN and
//! silently return fewer than `k` hits. The column doesn't exist either, and
//! `CREATE TABLE IF NOT EXISTS` cannot add one, so it would take a guarded
//! `ALTER` or the first real `DB_STEPS` bump. On top of that, the user's profile
//! knowledge base is curated: one chat's attachments would pollute
//! `/rag list|rebuild|remove` and cross-source dedup.
//!
//! **Isolation**: scoping is by `chat_id` (the vec0 partition key), which is
//! strictly narrower than the `profile_id` isolation invariant (spec §9.5) — a
//! chat belongs to exactly one profile, so the invariant holds a fortiori.
//!
//! **Dimensionality is shared with RAG** (`meta.rag_dim`, one per DB): switching
//! the embedding model resets both indexes at once (see [`Db::reset_vectors`]).

use super::*;

impl Db {
    /// Writes one indexed fragment of an attachment (vector + text). Stamped
    /// with the current embedding generation (see the [`super::embed_gen`]
    /// module), read under the lock we already hold — so an unstamped vector
    /// cannot be written.
    pub fn attachment_insert(&self, chunk: &AttachmentChunk) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        ensure_attachment_vec_table(&conn, chunk.embedding.len())?;
        conn.execute(
            "INSERT INTO attachment_documents(id, chat_id, attachment_id, name, chunk_text, created_at, embed_gen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                chunk.id.to_string(),
                chunk.chat_id.to_string(),
                chunk.attachment_id.to_string(),
                chunk.name,
                chunk.text,
                chunk.created_at.to_rfc3339(),
                current_embed_gen(&conn)?,
            ],
        )?;
        let rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO attachment_vectors(rowid, chat_id, embedding) VALUES (?1, ?2, ?3)",
            params![
                rowid,
                chunk.chat_id.to_string(),
                bytemuck::cast_slice::<f32, u8>(&chunk.embedding),
            ],
        )?;
        Ok(())
    }

    /// kNN search over one chat's attachments (isolation by `chat_id` via the
    /// partition key — the search never reaches another conversation's files).
    ///
    /// Fragments from a previous embedding generation are filtered out as belt
    /// and braces: [`Self::attachment_indexed_ids`] is the real gate (the tool
    /// answers "nothing indexed" and the pinned block stops advertising search),
    /// but a vector from another model must never reach a result list even if
    /// some future caller reaches this directly.
    pub fn attachment_search(
        &self,
        chat_id: Uuid,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<AttachmentHit>> {
        let conn = self.conn.lock().unwrap();
        if !table_exists(&conn, "attachment_vectors")? {
            return Ok(Vec::new()); // nothing indexed yet
        }
        let mut stmt = conn.prepare(
            "SELECT d.attachment_id, d.name, d.chunk_text, v.distance
             FROM attachment_vectors v
             JOIN attachment_documents d ON d.rowid = v.rowid
             WHERE v.chat_id = ?1 AND v.embedding MATCH ?2 AND k = ?3
               AND IFNULL(d.embed_gen, ?4) = ?5
             ORDER BY v.distance",
        )?;
        let hits = stmt
            .query_map(
                params![
                    chat_id.to_string(),
                    bytemuck::cast_slice::<f32, u8>(query),
                    k as i64,
                    NULL_EMBED_GEN,
                    current_embed_gen(&conn)?
                ],
                |r| {
                    Ok(AttachmentHit {
                        attachment_id: parse_uuid(r.get::<_, String>(0)?),
                        name: r.get(1)?,
                        text: r.get(2)?,
                        distance: r.get(3)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hits)
    }

    /// Which of a chat's attachments actually have a **usable** index. Drives two
    /// things: the pinned block only points the model at `attachment_search` for
    /// files it can really search, and the tool can tell "nothing indexed here"
    /// from "no hits".
    ///
    /// Chunks from a previous embedding generation therefore do not count — they
    /// are in another model's vector space, so searching them would return noise.
    /// Reporting them as absent makes a model change degrade into exactly the
    /// state the feature already handles (`attachment_read`, the guaranteed
    /// page-by-page path, is unaffected), with no caller change at all — and the
    /// chunks come back the moment the re-embed job stamps them.
    pub fn attachment_indexed_ids(&self, chat_id: Uuid) -> Result<Vec<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT attachment_id FROM attachment_documents
             WHERE chat_id = ?1 AND IFNULL(embed_gen, ?2) = ?3",
        )?;
        let ids = stmt
            .query_map(
                params![
                    chat_id.to_string(),
                    NULL_EMBED_GEN,
                    current_embed_gen(&conn)?
                ],
                |r| Ok(parse_uuid(r.get::<_, String>(0)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Drops one attachment's chunks (re-indexing replaces them instead of
    /// duplicating — the same idempotency `/rag add` has). Returns the number
    /// deleted.
    pub fn attachment_delete(&self, chat_id: Uuid, attachment_id: Uuid) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_attachment_rows(&conn, chat_id, |id| id == attachment_id)
    }

    /// Drops chunks of a chat's attachments that are no longer attached (hygiene
    /// after `/file remove` and after re-attaching). Also collects rows written by
    /// an indexing task that finished **after** its attachment was removed — the
    /// race is otherwise invisible (the search itself filters by the turn's
    /// snapshot, so such rows are never shown). Returns the number deleted.
    pub fn attachment_prune(&self, chat_id: Uuid, keep: &[Uuid]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        delete_attachment_rows(&conn, chat_id, |id| !keep.contains(&id))
    }
}

/// Deletes a chat's attachment chunks matching the predicate (along with their
/// vectors). Returns the number of documents deleted.
fn delete_attachment_rows(
    conn: &Connection,
    chat_id: Uuid,
    pred: impl Fn(Uuid) -> bool,
) -> Result<usize> {
    let rows: Vec<(i64, Uuid)> = {
        let mut stmt = conn
            .prepare("SELECT rowid, attachment_id FROM attachment_documents WHERE chat_id = ?1")?;
        stmt.query_map(params![chat_id.to_string()], |r| {
            Ok((r.get::<_, i64>(0)?, parse_uuid(r.get::<_, String>(1)?)))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let victims: Vec<i64> = rows
        .into_iter()
        .filter(|(_, id)| pred(*id))
        .map(|(rowid, _)| rowid)
        .collect();
    if victims.is_empty() {
        return Ok(0);
    }
    // The vector table only exists after the first insert.
    let has_vectors = table_exists(conn, "attachment_vectors")?;
    for rowid in &victims {
        if has_vectors {
            conn.execute(
                "DELETE FROM attachment_vectors WHERE rowid = ?1",
                params![rowid],
            )?;
        }
        conn.execute(
            "DELETE FROM attachment_documents WHERE rowid = ?1",
            params![rowid],
        )?;
    }
    Ok(victims.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn chunk(chat: Uuid, att: Uuid, name: &str, text: &str, v: Vec<f32>) -> AttachmentChunk {
        AttachmentChunk::new(chat, att, name, text, v)
    }

    #[test]
    fn attachment_knn_respects_chat_isolation() {
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (att_a, att_b) = (Uuid::new_v4(), Uuid::new_v4());
        // Chat B holds a vector identical to the query — it must not leak into A.
        db.attachment_insert(&chunk(b, att_b, "b.txt", "B", vec![1.0, 0.0, 0.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(
            a,
            att_a,
            "a.txt",
            "A near",
            vec![0.9, 0.1, 0.0, 0.0],
        ))
        .unwrap();
        db.attachment_insert(&chunk(a, att_a, "a.txt", "A far", vec![0.0, 0.0, 1.0, 0.0]))
            .unwrap();

        let hits = db.attachment_search(a, &[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 2, "only chat A's fragments");
        assert_eq!(hits[0].text, "A near");
        assert_eq!(hits[0].name, "a.txt");
        assert_eq!(hits[0].attachment_id, att_a);
        assert_eq!(hits[1].text, "A far");
    }

    #[test]
    fn search_is_empty_before_any_insert() {
        let db = db();
        assert!(
            db.attachment_search(Uuid::new_v4(), &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        assert!(
            db.attachment_indexed_ids(Uuid::new_v4())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn delete_and_prune_scope_to_one_chat() {
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
        db.attachment_insert(&chunk(a, one, "1.txt", "x", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(a, two, "2.txt", "y", vec![0.0, 1.0]))
            .unwrap();
        // The same attachment id in another chat must survive (isolation).
        db.attachment_insert(&chunk(b, one, "1.txt", "z", vec![1.0, 1.0]))
            .unwrap();

        assert_eq!(db.attachment_delete(a, one).unwrap(), 1);
        assert_eq!(db.attachment_indexed_ids(a).unwrap(), vec![two]);
        assert_eq!(db.attachment_indexed_ids(b).unwrap(), vec![one]);
        // And the vector is gone too — search no longer returns it.
        let hits = db.attachment_search(a, &[1.0, 0.0], 5).unwrap();
        assert!(hits.iter().all(|h| h.attachment_id == two));

        // Pruning keeps only what is still attached.
        assert_eq!(
            db.attachment_prune(a, &[two]).unwrap(),
            0,
            "nothing to drop"
        );
        assert_eq!(db.attachment_prune(a, &[]).unwrap(), 1);
        assert!(db.attachment_indexed_ids(a).unwrap().is_empty());
        assert_eq!(db.attachment_indexed_ids(b).unwrap(), vec![one]);
    }

    #[test]
    fn reindexing_an_attachment_replaces_its_chunks() {
        let db = db();
        let (chat, att) = (Uuid::new_v4(), Uuid::new_v4());
        db.attachment_insert(&chunk(chat, att, "a.txt", "старый текст", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_delete(chat, att).unwrap();
        db.attachment_insert(&chunk(chat, att, "a.txt", "новый текст", vec![1.0, 0.0]))
            .unwrap();
        let hits = db.attachment_search(chat, &[1.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1, "no duplicate from the previous run");
        assert_eq!(hits[0].text, "новый текст");
    }

    #[test]
    fn dimension_is_shared_with_rag_and_reset_clears_both() {
        let db = db();
        let chat = Uuid::new_v4();
        let profile = Uuid::new_v4();
        // Indexing an attachment registers the shared dimension...
        db.attachment_insert(&chunk(chat, Uuid::new_v4(), "a.txt", "x", vec![1.0, 0.0]))
            .unwrap();
        assert_eq!(db.rag_dimension().unwrap(), Some(2));
        // ...so a RAG document of a different dimensionality is refused (the same
        // guard as the other way round) — a model change goes through /rag rebuild.
        assert!(
            db.rag_insert(&crate::entities::rag::RagDocument::new(
                profile,
                "s",
                "t",
                vec![0.0, 1.0, 0.0]
            ))
            .is_err()
        );
        // A document of the matching dimensionality is fine — both indexes coexist.
        db.rag_insert(&crate::entities::rag::RagDocument::new(
            profile,
            "s",
            "t",
            vec![0.0, 1.0],
        ))
        .unwrap();

        // The reset drops both vector tables and the attachment chunks (their
        // rowids would otherwise dangle and get reused).
        assert_eq!(db.reset_vectors().unwrap(), 1, "attachment chunks dropped");
        assert_eq!(db.rag_dimension().unwrap(), None);
        assert!(db.attachment_indexed_ids(chat).unwrap().is_empty());
        assert!(
            db.attachment_search(chat, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        // The new dimensionality is now free to be registered by either index.
        db.attachment_insert(&chunk(
            chat,
            Uuid::new_v4(),
            "a.txt",
            "x",
            vec![1.0, 0.0, 0.0],
        ))
        .unwrap();
        assert_eq!(db.rag_dimension().unwrap(), Some(3));
    }

    #[test]
    fn rag_search_survives_a_dimension_registered_by_attachments_only() {
        // Regression: the RAG reader used to bail out on "no recorded dimension".
        // With a shared `meta.rag_dim` the dimension can be registered while
        // `rag_vectors` does not exist yet — the reader must check the table.
        let db = db();
        db.attachment_insert(&chunk(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "a.txt",
            "x",
            vec![1.0, 0.0],
        ))
        .unwrap();
        assert!(
            db.rag_search(Uuid::new_v4(), &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
    }
}
