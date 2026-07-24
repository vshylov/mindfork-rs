//! Storage (SQLite) — the notes link graph + supersession + source citation. Part of the [`super`] module; split out
//! of the db.rs monolith (see docs/history/refactoring-god-objects.md, stage 5).

use super::*;

impl Db {
    /// Creates a directed link between two notes (idempotent via PK).
    /// Isolated by `profile_id`. Returns `true` if the link was actually
    /// created (`false` — such a link already existed, `INSERT OR IGNORE`
    /// inserted nothing).
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

    /// The number of links the note participates in (in either direction).
    /// For warning when revising a load-bearing node (its edges may become wrong).
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

    /// The note's graph neighbors (in both directions), excluding superseded
    /// ones. An optional filter by link type. Returns (note, link type,
    /// whether the link is outgoing).
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

    /// Marks a note as superseded by another (it's hidden from active output,
    /// kept for the "scar"/trail). Idempotent (overwrites the supersession record).
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

    /// Links a note to a RAG source (by the source's name — stable across
    /// reindexing, unlike chunk ids). Idempotent via PK. Returns `true` if
    /// the link was actually created. Isolated by `profile_id`.
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

    /// The RAG sources a note cites (for display on recall).
    /// Isolated by `profile_id`.
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

    /// The profile's active (not superseded) notes that cite this RAG source —
    /// the reverse direction (finding notes via RAG, "both organs"). Isolated
    /// by `profile_id`.
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

    /// All of the profile's links `(from, to, relation)` — for the consolidation overview.
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

    /// Retargets a superseded note's links onto the new one (on merge): edges
    /// involving `old_id` are redirected to `new_id`; self-loops and duplicates are dropped.
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
            // Delete the old edge, then insert the retargeted one (OR IGNORE against duplicates).
            conn.execute(
                "DELETE FROM note_links WHERE profile_id=?1 AND from_id=?2 AND to_id=?3 AND relation=?4",
                params![profile_id.to_string(), from, to, rel],
            )?;
            let nf = if from == old_s { &new_s } else { &from };
            let nt = if to == old_s { &new_s } else { &to };
            if nf == nt {
                continue; // a self-loop after retargeting — drop it
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
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

        // A superseded note is hidden from the list, from semantics, and from is_active.
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
        // Repeating the same link doesn't create it (false) — no duplicate in the table.
        assert!(!db.note_link_insert(a, n1.id, n2.id, "refines").unwrap());

        let nb = db.note_neighbors(a, n1.id, None).unwrap();
        assert_eq!(nb.len(), 2); // outgoing to n2 + incoming from n3 (the duplicate doesn't count)
        assert!(
            nb.iter()
                .any(|(n, r, out)| n.id == n2.id && r == "refines" && *out)
        );
        assert!(
            nb.iter()
                .any(|(n, r, out)| n.id == n3.id && r == "contradicts" && !*out)
        );

        // A filter by link type.
        let only = db.note_neighbors(a, n1.id, Some("refines")).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].0.id, n2.id);

        // A superseded neighbor disappears from the output.
        let repl = Note::new(a, "n2b", vec![]);
        db.note_insert(&repl).unwrap();
        db.note_supersede_mark(a, n2.id, repl.id).unwrap();
        let nb2 = db.note_neighbors(a, n1.id, None).unwrap();
        assert!(!nb2.iter().any(|(n, _, _)| n.id == n2.id));
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
        // s1→x and s2→x (become duplicates after retargeting); s1→s2 (becomes a self-loop).
        db.note_link_insert(a, s1.id, x.id, "contradicts").unwrap();
        db.note_link_insert(a, s2.id, x.id, "contradicts").unwrap();
        db.note_link_insert(a, s1.id, s2.id, "relates").unwrap();

        db.note_links_retarget(a, s1.id, merged.id).unwrap();
        db.note_links_retarget(a, s2.id, merged.id).unwrap();

        let all = db.note_links_all(a).unwrap();
        assert_eq!(all.len(), 1); // the duplicate collapsed, the self-loop was dropped
        assert_eq!(all[0].0, merged.id);
        assert_eq!(all[0].1, x.id);
        assert_eq!(all[0].2, "contradicts");
    }

    #[test]
    fn note_rag_links_bidirectional_and_isolated() {
        use crate::entities::note::Note;
        let db = db();
        let p = Uuid::new_v4();
        let other = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/kb/spec.md", "chunk", vec![1.0, 0.0]))
            .unwrap();
        let note = Note::new(p, "опирается на спеку", vec![]);
        let nid = note.id;
        db.note_insert(&note).unwrap();

        // The source's existence (isolated by profile).
        assert!(db.rag_source_exists(p, "/kb/spec.md").unwrap());
        assert!(!db.rag_source_exists(p, "/kb/missing.md").unwrap());
        assert!(!db.rag_source_exists(other, "/kb/spec.md").unwrap());

        // The link is idempotent.
        assert!(db.note_cite_source_insert(p, nid, "/kb/spec.md").unwrap());
        assert!(!db.note_cite_source_insert(p, nid, "/kb/spec.md").unwrap());

        // The forward direction: the note's sources.
        assert_eq!(
            db.note_cited_sources(p, nid).unwrap(),
            vec!["/kb/spec.md".to_string()]
        );
        // The reverse direction: the source's notes (+ isolation).
        let citing = db.notes_citing_source(p, "/kb/spec.md").unwrap();
        assert_eq!(citing.len(), 1);
        assert_eq!(citing[0].id, nid);
        assert!(
            db.notes_citing_source(other, "/kb/spec.md")
                .unwrap()
                .is_empty()
        );

        // Deleting a note removes its source citations.
        db.note_delete(p, nid).unwrap();
        assert!(db.notes_citing_source(p, "/kb/spec.md").unwrap().is_empty());
    }

    #[test]
    fn notes_citing_source_hides_superseded() {
        use crate::entities::note::Note;
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "/kb/a.md", "c", vec![1.0, 0.0]))
            .unwrap();
        let n = Note::new(p, "старое", vec![]);
        let nid = n.id;
        db.note_insert(&n).unwrap();
        db.note_cite_source_insert(p, nid, "/kb/a.md").unwrap();
        // A superseded note doesn't surface in the reverse path.
        let new = Note::new(p, "новое", vec![]);
        let new_id = new.id;
        db.note_insert(&new).unwrap();
        db.note_supersede_mark(p, nid, new_id).unwrap();
        assert!(db.notes_citing_source(p, "/kb/a.md").unwrap().is_empty());
    }
}
