//! Storage (SQLite) — **embedding generations** and the re-embed work queue
//! (stage 2 of docs/research/embedding-model-change-reindex.md §8.1). Part of
//! the [`super`] module.
//!
//! ## What a generation is
//!
//! Stored vectors are only comparable to a query embedded by the **same** model,
//! and dimensionality cannot establish that (`bge-m3` and
//! `multilingual-e5-large-instruct` are both 1024-d yet embed the same text to a
//! cosine of ~0.37). Identity itself lives in the canary fingerprint
//! ([`crate::shared::embed_identity`]); a *row* only needs to say **which
//! generation produced it**, so the marker is a small integer counter in `meta`
//! against an `embed_gen` column on the three plain tables that hold vectors
//! (S1).
//!
//! This replaces stage 1's blunt deletion (S3): on a model change the counter is
//! bumped and every existing row simply reads as **foreign** — nothing is thrown
//! away, the healing paths pick the rows up, and switching *back* to the previous
//! model costs nothing at all.
//!
//! ## The two halves
//!
//! - **Passive healing.** Foreign vectors are invisible to search
//!   ([`Db::note_search_semantic`], [`Db::attachment_indexed_ids`]) and notes
//!   with one are listed by [`Db::notes_missing_vectors`], so the existing
//!   `ensure_note_vectors` backfill re-embeds them on the next semantic path.
//! - **The queue below.** RAG chunks and attachment fragments are too many to
//!   heal on a read path, so a job walks them in batches: *for each row whose
//!   generation is not current, embed its stored text, replace the vector, stamp
//!   the generation*. Interrupting it leaves a consistent partial state, and the
//!   next run resumes exactly where it stopped (S5).
//!
//! ## The similarity calibration
//!
//! The other per-model fact recorded here: the active model's measured
//! similarity range ([`crate::shared::embed_calibration`]). It belongs beside
//! the generation counter because it is produced on the same once-per-model
//! path and answers the same question from the other side — the counter says
//! *which* model the stored vectors came from, the calibration says what its
//! cosines *mean*, so the project's bge-m3-tuned thresholds can be read in its
//! scale. Absent means "never calibrated", and the thresholds are then used
//! exactly as written.
//!
//! ## Scope
//!
//! The queue readers are deliberately **global** — across every profile and
//! every chat. The embedder is a single global server, so a model change
//! invalidates all of them at once; a per-profile operation would be the wrong
//! shape (research §7, R1). This is not a breach of the `profile_id` isolation
//! invariant (spec §9.5): the invariant governs what one profile's *queries*
//! may see, and the re-embed job serves no query — it rewrites a row's vector in
//! place, under the very partition key the row already carries.

use super::*;
use crate::shared::embed_calibration::{Calibration, SimilarityScale};

// The queue half of this module (everything from [`ReembedRow`] down, minus the
// pieces the model-change guard already calls) precedes its consumer: the job
// that drains it arrives with the `/reindex` command (research §8.1, S4). Hence
// the pointwise `#[allow(dead_code)]` the project uses for deliberately
// ahead-of-consumer API (`Db::rag_count`, `Db::rag_stale_profiles`) — drop each
// one as the job starts calling it. The tests below exercise all of it.

/// One row awaiting re-embedding.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ReembedRow {
    /// Rowid of the row holding the text, and the key its vector is joined by.
    pub rowid: i64,
    /// Domain id: `rag_documents.id` / `attachment_documents.id` / `notes.id`.
    /// Notes are re-embedded through [`Db::note_vector_upsert`], which is keyed
    /// by this rather than by rowid.
    pub id: Uuid,
    /// The row's partition key: the profile for RAG and notes, the chat for
    /// attachments.
    pub partition: Uuid,
    /// The text to embed. Already stored, so re-embedding needs neither the
    /// source file nor the chunker (research §3).
    pub text: String,
}

/// How much work the re-embed job still has, per store (for progress
/// reporting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReembedPending {
    pub rag: usize,
    pub attachments: usize,
    pub notes: usize,
}

/// Rows whose generation is not the current one, oldest rowid first. `ORDER BY
/// rowid` is what makes a batched job resumable: each pass takes the oldest
/// outstanding rows, and stamping them removes them from the next pass, so
/// progress is monotonic even if the job is interrupted between batches.
#[allow(dead_code)]
fn rows_to_reembed(
    conn: &Connection,
    table: &str,
    partition_col: &str,
    text_col: &str,
    limit: usize,
) -> Result<Vec<ReembedRow>> {
    let generation = current_embed_gen(conn)?;
    // `IFNULL(..., NULL_EMBED_GEN)`: rows predating the marker carry NULL and
    // must read as foreign — see [`NULL_EMBED_GEN`]. Column and table names are
    // hardcoded by the callers below, so formatting them in is injection-safe.
    let sql = format!(
        "SELECT rowid, id, {partition_col}, {text_col} FROM {table}
         WHERE IFNULL(embed_gen, ?1) <> ?2
         ORDER BY rowid LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![NULL_EMBED_GEN, generation, limit as i64], |r| {
            Ok(ReembedRow {
                rowid: r.get(0)?,
                id: parse_uuid(r.get::<_, String>(1)?),
                partition: parse_uuid(r.get::<_, String>(2)?),
                text: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Counts what [`rows_to_reembed`] would eventually return (the same predicate,
/// without the batch limit).
fn count_to_reembed(conn: &Connection, table: &str) -> Result<usize> {
    let generation = current_embed_gen(conn)?;
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE IFNULL(embed_gen, ?1) <> ?2");
    let n: i64 = conn.query_row(&sql, params![NULL_EMBED_GEN, generation], |r| r.get(0))?;
    Ok(n as usize)
}

/// The notes half of the queue, shared by the list and the count. A note needs
/// re-embedding when it has **no** vector or a foreign-generation one;
/// superseded notes are excluded, exactly as [`Db::notes_missing_vectors`] does
/// — they are hidden from every semantic path, so embedding them is wasted work.
const NOTES_TO_REEMBED_PREDICATE: &str = "FROM notes n
     LEFT JOIN note_vectors v ON v.note_id = n.id
     LEFT JOIN note_superseded s ON s.note_id = n.id
     WHERE s.note_id IS NULL
       AND (v.note_id IS NULL OR IFNULL(v.embed_gen, ?1) <> ?2)";

impl Db {
    // ---------- the generation counter ----------

    /// The embedding generation currently in force. Every vector written from
    /// now on is stamped with it, and anything stamped otherwise (including the
    /// `NULL` of a row that predates the marker) is foreign.
    #[allow(dead_code)] // the writers stamp themselves; a reader arrives with `/reindex`
    pub fn embed_generation(&self) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        current_embed_gen(&conn)
    }

    /// Starts a new generation, returning it. Called when a model change is
    /// detected: from that moment every stored vector reads as foreign, without
    /// a single row being touched.
    ///
    /// Monotonic on purpose — the counter is never reset, not even by
    /// [`Db::reset_vectors`], because reusing a number would make surviving rows
    /// of an old generation read as current.
    pub fn bump_embed_generation(&self) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        // Saturating rather than wrapping: overflow needs four billion model
        // changes, and wrapping is the one outcome that would silently validate
        // stale vectors.
        let next = current_embed_gen(&conn)?.saturating_add(1);
        meta_set(&conn, KEY_EMBED_GEN, &next.to_string())?;
        Ok(next)
    }

    // ---------- the similarity calibration ----------

    /// The active model's measured similarity range. `None` — never calibrated,
    /// so callers use [`SimilarityScale::identity`] and the thresholds keep the
    /// values they are written with.
    ///
    /// A half-written or unparseable pair also reads as `None`, deliberately —
    /// the same rule the canary follows: garbage in `meta` degrades to the
    /// natural empty state, and here that state is "no correction", which is
    /// always safe.
    pub fn embed_calibration(&self) -> Result<Option<Calibration>> {
        let conn = self.conn.lock().unwrap();
        read_calibration(&conn)
    }

    /// Records the calibration of the model now in use, replacing any previous
    /// one. Written on the same once-per-model path as the fingerprint.
    pub fn set_embed_calibration(&self, c: &Calibration) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        meta_set(&conn, KEY_EMBED_CAL_UNRELATED, &c.unrelated.to_string())?;
        meta_set(&conn, KEY_EMBED_CAL_PARAPHRASE, &c.paraphrase.to_string())
    }

    /// The scale the project's reference thresholds should be read in.
    ///
    /// Infallible on purpose: a threshold is needed on paths that have no way to
    /// report a storage error, and every failure has the same right answer —
    /// the identity, i.e. today's behaviour.
    pub fn similarity_scale(&self) -> SimilarityScale {
        self.embed_calibration()
            .ok()
            .flatten()
            .map(SimilarityScale::from_calibration)
            .unwrap_or_else(SimilarityScale::identity)
    }
}

/// Reads the recorded calibration; both halves must be present and parse.
///
/// A free function for the same reason as [`current_embed_gen`]: `self.conn` is
/// a non-reentrant [`std::sync::Mutex`], and the callers below run under a lock
/// they already hold.
fn read_calibration(conn: &Connection) -> Result<Option<Calibration>> {
    let read = |key: &str| -> Result<Option<f32>> {
        Ok(meta_get(conn, key)?.and_then(|v| v.parse::<f32>().ok()))
    };
    Ok(
        match (
            read(KEY_EMBED_CAL_UNRELATED)?,
            read(KEY_EMBED_CAL_PARAPHRASE)?,
        ) {
            (Some(unrelated), Some(paraphrase)) => Some(Calibration {
                unrelated,
                paraphrase,
            }),
            _ => None,
        },
    )
}

/// Forgets the calibration, so the thresholds fall back to the values they are
/// written with until the model is measured again.
///
/// Called by [`Db::reset_vectors`] — the "start over" primitive — which also
/// clears the fingerprint: the two describe the same model, and leaving one
/// behind would claim knowledge about vectors that no longer exist. The next
/// launch records both together.
///
/// Deliberately **not** called by [`Db::drop_vector_tables`]: that one keeps the
/// fingerprint precisely because the model is unchanged (or has already been
/// recorded and calibrated by the guard that detected the change), so discarding
/// its calibration would leave the gates uncorrected for the very model the job
/// is re-embedding into.
pub(super) fn clear_calibration(conn: &Connection) -> Result<()> {
    meta_del(conn, KEY_EMBED_CAL_UNRELATED)?;
    meta_del(conn, KEY_EMBED_CAL_PARAPHRASE)
}

// ---------- the re-embed work queue ----------
//
// Ahead of its consumer (see the note above [`ReembedRow`]) — hence the
// block-level allow. `count_rows_to_reembed` is already called by the
// model-change guard; the attribute is a no-op for it.
#[allow(dead_code)]
impl Db {
    /// RAG chunks whose vectors predate the current generation, across **all
    /// profiles** (see the module doc on scope). At most `limit` — the job
    /// embeds in batches and must be resumable.
    pub fn rag_rows_to_reembed(&self, limit: usize) -> Result<Vec<ReembedRow>> {
        let conn = self.conn.lock().unwrap();
        rows_to_reembed(&conn, "rag_documents", "profile_id", "chunk_text", limit)
    }

    /// Attachment fragments whose vectors predate the current generation, across
    /// **all chats**. The partition is the chat id.
    pub fn attachment_rows_to_reembed(&self, limit: usize) -> Result<Vec<ReembedRow>> {
        let conn = self.conn.lock().unwrap();
        rows_to_reembed(
            &conn,
            "attachment_documents",
            "chat_id",
            "chunk_text",
            limit,
        )
    }

    /// Notes (across **all profiles**) whose vector is missing or
    /// foreign-generation. Re-embed them with [`Db::note_vector_upsert`], keyed
    /// by [`ReembedRow::id`].
    ///
    /// Overlaps [`Db::notes_missing_vectors`] by design: that one heals a single
    /// profile lazily on its next semantic path, this one lets the job finish
    /// the whole DB without waiting for the user to visit each profile.
    pub fn notes_to_reembed(&self, limit: usize) -> Result<Vec<ReembedRow>> {
        let conn = self.conn.lock().unwrap();
        let generation = current_embed_gen(&conn)?;
        let sql = format!(
            "SELECT n.rowid, n.id, n.profile_id, n.content
             {NOTES_TO_REEMBED_PREDICATE}
             ORDER BY n.rowid LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![NULL_EMBED_GEN, generation, limit as i64], |r| {
                Ok(ReembedRow {
                    rowid: r.get(0)?,
                    id: parse_uuid(r.get::<_, String>(1)?),
                    partition: parse_uuid(r.get::<_, String>(2)?),
                    text: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// How many rows still need re-embedding, per store. Agrees with the list
    /// readers above (same predicates, no batch limit), so the job can report
    /// "N of M" without a second source of truth.
    pub fn count_rows_to_reembed(&self) -> Result<ReembedPending> {
        let conn = self.conn.lock().unwrap();
        let generation = current_embed_gen(&conn)?;
        let notes: i64 = conn.query_row(
            &format!("SELECT COUNT(*) {NOTES_TO_REEMBED_PREDICATE}"),
            params![NULL_EMBED_GEN, generation],
            |r| r.get(0),
        )?;
        Ok(ReembedPending {
            rag: count_to_reembed(&conn, "rag_documents")?,
            attachments: count_to_reembed(&conn, "attachment_documents")?,
            notes: notes as usize,
        })
    }

    // ---------- replacing one row's vector ----------

    /// Replaces one RAG chunk's vector and stamps it with the current
    /// generation. The chunk's `rowid`, text and id are untouched, so nothing
    /// downstream is invalidated — re-embedding is not re-chunking (research
    /// §3).
    pub fn rag_set_vector(&self, rowid: i64, profile_id: Uuid, embedding: &[f32]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        set_vector(
            &conn,
            VectorTarget {
                docs: "rag_documents",
                vectors: "rag_vectors",
                partition_col: "profile_id",
            },
            rowid,
            profile_id,
            embedding,
            ensure_vec_table,
        )
    }

    /// Replaces one attachment fragment's vector and stamps it with the current
    /// generation. The partition is the chat id.
    pub fn attachment_set_vector(
        &self,
        rowid: i64,
        chat_id: Uuid,
        embedding: &[f32],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        set_vector(
            &conn,
            VectorTarget {
                docs: "attachment_documents",
                vectors: "attachment_vectors",
                partition_col: "chat_id",
            },
            rowid,
            chat_id,
            embedding,
            ensure_attachment_vec_table,
        )
    }

    /// Drops both `vec0` tables and forgets the recorded dimensionality, keeping
    /// every document row, the fingerprint and the stale marks.
    ///
    /// The re-embed job calls this up front when the new model's dimensionality
    /// differs (research §8.1, S5): a `vec0` table is fixed-width, so it cannot
    /// hold both sizes, and [`Db::rag_set_vector`] would rightly refuse. Dropping
    /// them leaves every row without a vector — which is precisely the state a
    /// foreign generation already describes, so the job's single loop covers a
    /// dimensionality change with no special case. The tables are recreated at
    /// the new width on the first write.
    ///
    /// Distinct from [`Db::reset_vectors`], which additionally deletes the
    /// attachment rows and forgets which model produced everything: that is the
    /// "start over" primitive, this one is "keep the texts, re-embed them".
    pub fn drop_vector_tables(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DROP TABLE IF EXISTS rag_vectors", [])?;
        conn.execute("DROP TABLE IF EXISTS attachment_vectors", [])?;
        meta_del(&conn, KEY_RAG_DIM)?;
        Ok(())
    }
}

/// Which pair of tables a vector replacement addresses.
#[allow(dead_code)]
struct VectorTarget {
    docs: &'static str,
    vectors: &'static str,
    partition_col: &'static str,
}

/// Replaces one row's vector and stamps its generation.
///
/// The step order is load-bearing:
///
/// 1. `ensure_table` first — a dimensionality mismatch must fail **before**
///    anything is written, or the row would end up stamped as current while
///    still holding the old model's vector.
/// 2. Skip a row that is gone (or belongs to another partition). A job embeds a
///    batch and writes it back, so the user may have deleted the source or
///    removed the attachment in between; failing the batch over an ordinary race
///    would be wrong, and inserting the vector anyway would orphan it on a rowid
///    sqlite later reuses.
/// 3. Vector, then stamp — never the other way round. An interruption between
///    the two leaves the row looking foreign, so the next pass simply redoes it;
///    the reverse order would mark work done that was never performed.
#[allow(dead_code)]
fn set_vector(
    conn: &Connection,
    target: VectorTarget,
    rowid: i64,
    partition: Uuid,
    embedding: &[f32],
    ensure_table: fn(&Connection, usize) -> Result<()>,
) -> Result<()> {
    ensure_table(conn, embedding.len())?;

    let VectorTarget {
        docs,
        vectors,
        partition_col,
    } = target;
    let exists: Option<i64> = conn
        .query_row(
            &format!("SELECT 1 FROM {docs} WHERE rowid = ?1 AND {partition_col} = ?2"),
            params![rowid, partition.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Ok(());
    }

    conn.execute(
        &format!("DELETE FROM {vectors} WHERE rowid = ?1"),
        params![rowid],
    )?;
    conn.execute(
        &format!("INSERT INTO {vectors}(rowid, {partition_col}, embedding) VALUES (?1, ?2, ?3)"),
        params![
            rowid,
            partition.to_string(),
            bytemuck::cast_slice::<f32, u8>(embedding),
        ],
    )?;
    conn.execute(
        &format!("UPDATE {docs} SET embed_gen = ?1 WHERE rowid = ?2"),
        params![current_embed_gen(conn)?, rowid],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn chunk(chat: Uuid, att: Uuid, text: &str, v: Vec<f32>) -> AttachmentChunk {
        AttachmentChunk::new(chat, att, "a.txt", text, v)
    }

    /// Forces a row's generation to `NULL`, standing in for every row in a real
    /// user's DB — they all predate the marker.
    fn null_out(db: &Db, table: &str) {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(&format!("UPDATE {table} SET embed_gen = NULL"))
            .unwrap();
    }

    // ---------- the counter ----------

    #[test]
    fn generation_starts_at_one_and_increments() {
        let db = db();
        assert_eq!(db.embed_generation().unwrap(), 1, "fresh DB");
        assert_eq!(db.bump_embed_generation().unwrap(), 2);
        assert_eq!(db.embed_generation().unwrap(), 2);
        assert_eq!(db.bump_embed_generation().unwrap(), 3);
        assert_eq!(db.embed_generation().unwrap(), 3);
    }

    #[test]
    fn generation_survives_reopening_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        Db::open(&path).unwrap().bump_embed_generation().unwrap();
        assert_eq!(Db::open(&path).unwrap().embed_generation().unwrap(), 2);
    }

    #[test]
    fn corrupt_generation_reads_as_the_first() {
        // Garbage in `meta` degrades to the natural empty state — and reading
        // low is the safe direction: rows stamped higher then read as foreign.
        let db = db();
        {
            let conn = db.conn.lock().unwrap();
            meta_set(&conn, KEY_EMBED_GEN, "not a number").unwrap();
        }
        assert_eq!(db.embed_generation().unwrap(), 1);
    }

    #[test]
    fn a_written_vector_is_stamped_and_turns_foreign_after_a_bump() {
        let db = db();
        let (p, chat) = (Uuid::new_v4(), Uuid::new_v4());
        let note = Note::new(p, "n", vec![]);
        db.note_insert(&note).unwrap();
        db.note_vector_upsert(note.id, p, &[1.0, 0.0]).unwrap();
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(chat, Uuid::new_v4(), "frag", vec![1.0, 0.0]))
            .unwrap();

        // Written under the current generation: nothing is outstanding.
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );

        db.bump_embed_generation().unwrap();
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending {
                rag: 1,
                attachments: 1,
                notes: 1
            },
            "every store reads as foreign after a bump"
        );
    }

    // ---------- the similarity calibration ----------

    fn cal(unrelated: f32, paraphrase: f32) -> Calibration {
        Calibration {
            unrelated,
            paraphrase,
        }
    }

    #[test]
    fn calibration_round_trips_and_replaces() {
        let db = db();
        assert_eq!(db.embed_calibration().unwrap(), None, "fresh DB");

        let c = cal(0.7897, 0.9456);
        db.set_embed_calibration(&c).unwrap();
        assert_eq!(db.embed_calibration().unwrap(), Some(c));

        // A second model overwrites rather than accumulating.
        let other = cal(0.4128, 0.8176);
        db.set_embed_calibration(&other).unwrap();
        assert_eq!(db.embed_calibration().unwrap(), Some(other));
    }

    #[test]
    fn calibration_survives_reopening_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let c = cal(0.7897, 0.9456);
        Db::open(&path).unwrap().set_embed_calibration(&c).unwrap();
        assert_eq!(
            Db::open(&path).unwrap().embed_calibration().unwrap(),
            Some(c)
        );
    }

    #[test]
    fn an_unreadable_calibration_reads_as_absent() {
        // Garbage in `meta` degrades to the natural empty state — and here that
        // state is "no correction", so a hand-edited DB can only lose the
        // calibration, never gain a wrong one.
        let db = db();
        db.set_embed_calibration(&cal(0.4, 0.8)).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            meta_set(&conn, KEY_EMBED_CAL_UNRELATED, "not a number").unwrap();
        }
        assert_eq!(db.embed_calibration().unwrap(), None);

        // Half a pair is unusable too: the map needs both anchors.
        let half = Db::open_in_memory().unwrap();
        {
            let conn = half.conn.lock().unwrap();
            meta_set(&conn, KEY_EMBED_CAL_PARAPHRASE, "0.9456").unwrap();
        }
        assert_eq!(db.embed_calibration().unwrap(), None);
    }

    #[test]
    fn similarity_scale_is_the_identity_until_something_is_recorded() {
        // The state of every existing installation: thresholds untouched.
        let db = db();
        for t in [0.85, 0.72, 0.62] {
            assert_eq!(db.similarity_scale().map(t), t);
        }
        // A measured model moves them (the e5 figures of research §8.2).
        db.set_embed_calibration(&cal(0.7897, 0.9456)).unwrap();
        assert!((db.similarity_scale().map(0.72) - 0.908).abs() < 0.002);
    }

    #[test]
    fn reset_vectors_forgets_the_calibration_but_dropping_the_tables_keeps_it() {
        // `reset_vectors` is "start over": it already discards the fingerprint,
        // and the calibration describes the same model, so it goes with it.
        let db = db();
        db.set_embed_calibration(&cal(0.7897, 0.9456)).unwrap();
        db.reset_vectors().unwrap();
        assert_eq!(db.embed_calibration().unwrap(), None);
        assert_eq!(db.similarity_scale().map(0.72), 0.72, "back to identity");

        // `drop_vector_tables` is "keep the texts, re-embed them" — the re-embed
        // job's own step, run *after* the guard has recorded and calibrated the
        // new model. Clearing here would throw away a fresh, correct calibration
        // and leave the gates uncorrected for the model being re-embedded into.
        let kept = Db::open_in_memory().unwrap();
        let c = cal(0.7897, 0.9456);
        kept.set_embed_calibration(&c).unwrap();
        kept.drop_vector_tables().unwrap();
        assert_eq!(kept.embed_calibration().unwrap(), Some(c));
    }

    // ---------- NULL (pre-marker) rows count as foreign ----------

    #[test]
    fn null_generation_reads_as_foreign_everywhere() {
        // Every row in an existing user's DB carries NULL. A plain `= ?`/`<> ?`
        // would evaluate to NULL and silently skip exactly these rows.
        let db = db();
        let (p, chat, att) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let note = Note::new(p, "n", vec![]);
        db.note_insert(&note).unwrap();
        db.note_vector_upsert(note.id, p, &[1.0, 0.0]).unwrap();
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(chat, att, "frag", vec![1.0, 0.0]))
            .unwrap();

        null_out(&db, "note_vectors");
        null_out(&db, "rag_documents");
        null_out(&db, "attachment_documents");

        // The queue readers list them...
        assert_eq!(db.notes_to_reembed(10).unwrap().len(), 1);
        assert_eq!(db.rag_rows_to_reembed(10).unwrap().len(), 1);
        assert_eq!(db.attachment_rows_to_reembed(10).unwrap().len(), 1);
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending {
                rag: 1,
                attachments: 1,
                notes: 1
            }
        );
        // ...the lazy note backfill lists it...
        assert_eq!(db.notes_missing_vectors(p).unwrap().len(), 1);
        // ...and no semantic path serves it.
        assert!(
            db.note_search_semantic(p, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        assert!(db.notes_with_vectors(p).unwrap().is_empty());
        assert!(db.attachment_indexed_ids(chat).unwrap().is_empty());
        assert!(
            db.attachment_search(chat, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
    }

    // ---------- the queue ----------

    #[test]
    fn queue_carries_the_partition_and_the_text() {
        let db = db();
        let (p, chat, att) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let doc = RagDocument::new(p, "s", "the chunk text", vec![1.0, 0.0]);
        let doc_id = doc.id;
        db.rag_insert(&doc).unwrap();
        db.attachment_insert(&chunk(chat, att, "the fragment", vec![1.0, 0.0]))
            .unwrap();
        let note = Note::new(p, "the note body", vec![]);
        db.note_insert(&note).unwrap();
        db.bump_embed_generation().unwrap();

        let rag = db.rag_rows_to_reembed(10).unwrap();
        assert_eq!(rag.len(), 1);
        assert_eq!(rag[0].partition, p, "RAG is partitioned by profile");
        assert_eq!(rag[0].id, doc_id);
        assert_eq!(rag[0].text, "the chunk text");

        let atts = db.attachment_rows_to_reembed(10).unwrap();
        assert_eq!(atts[0].partition, chat, "attachments by chat");
        assert_eq!(atts[0].text, "the fragment");

        let notes = db.notes_to_reembed(10).unwrap();
        assert_eq!(notes[0].partition, p);
        assert_eq!(notes[0].id, note.id, "keyed by note id, not rowid");
        assert_eq!(notes[0].text, "the note body");
    }

    #[test]
    fn queue_spans_every_profile_and_chat() {
        // A model change invalidates all of them at once — the job must not have
        // to be run once per profile.
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(a, "s", "x", vec![1.0, 0.0]))
            .unwrap();
        db.rag_insert(&RagDocument::new(b, "s", "y", vec![0.0, 1.0]))
            .unwrap();
        db.attachment_insert(&chunk(a, Uuid::new_v4(), "x", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(b, Uuid::new_v4(), "y", vec![0.0, 1.0]))
            .unwrap();
        for p in [a, b] {
            let n = Note::new(p, "n", vec![]);
            db.note_insert(&n).unwrap();
        }
        db.bump_embed_generation().unwrap();

        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending {
                rag: 2,
                attachments: 2,
                notes: 2
            }
        );
        assert_eq!(db.rag_rows_to_reembed(10).unwrap().len(), 2);
        assert_eq!(db.attachment_rows_to_reembed(10).unwrap().len(), 2);
        assert_eq!(db.notes_to_reembed(10).unwrap().len(), 2);
    }

    #[test]
    fn batches_are_stable_and_a_resumed_job_makes_progress() {
        let db = db();
        let p = Uuid::new_v4();
        for i in 0..5 {
            db.rag_insert(&RagDocument::new(
                p,
                "s",
                format!("chunk {i}"),
                vec![1.0, 0.0],
            ))
            .unwrap();
        }
        db.bump_embed_generation().unwrap();

        // The limit is honoured, and the order is stable across calls.
        let first = db.rag_rows_to_reembed(2).unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first, db.rag_rows_to_reembed(2).unwrap(), "stable order");
        assert!(first[0].rowid < first[1].rowid, "oldest rowid first");

        // Processing a batch removes it from the next one — progress, not a loop.
        for row in &first {
            db.rag_set_vector(row.rowid, row.partition, &[0.0, 1.0])
                .unwrap();
        }
        assert_eq!(db.count_rows_to_reembed().unwrap().rag, 3);
        let second = db.rag_rows_to_reembed(2).unwrap();
        assert!(second.iter().all(|r| !first.contains(r)), "no repeats");

        // Draining the rest empties the queue.
        for row in db.rag_rows_to_reembed(100).unwrap() {
            db.rag_set_vector(row.rowid, row.partition, &[0.0, 1.0])
                .unwrap();
        }
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );
    }

    #[test]
    fn superseded_notes_are_not_queued() {
        // They are hidden from every semantic path, so embedding them is waste.
        let db = db();
        let p = Uuid::new_v4();
        let old = Note::new(p, "old", vec![]);
        let new = Note::new(p, "new", vec![]);
        db.note_insert(&old).unwrap();
        db.note_insert(&new).unwrap();
        db.note_supersede_mark(p, old.id, new.id).unwrap();
        db.bump_embed_generation().unwrap();

        let queued = db.notes_to_reembed(10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, new.id);
        assert_eq!(
            db.count_rows_to_reembed().unwrap().notes,
            queued.len(),
            "the count uses the same predicate as the list"
        );
    }

    // ---------- replacing a vector ----------

    #[test]
    fn set_vector_replaces_stamps_and_keeps_the_rowid() {
        let db = db();
        let (p, chat, att) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(chat, att, "frag", vec![1.0, 0.0]))
            .unwrap();
        db.bump_embed_generation().unwrap();

        let rag = db.rag_rows_to_reembed(10).unwrap().remove(0);
        db.rag_set_vector(rag.rowid, rag.partition, &[0.0, 1.0])
            .unwrap();
        let atts = db.attachment_rows_to_reembed(10).unwrap().remove(0);
        db.attachment_set_vector(atts.rowid, atts.partition, &[0.0, 1.0])
            .unwrap();

        // Stamped current — out of the queue.
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );
        // The rowid (and therefore the chunk id) survived...
        let same_rowid: i64 = {
            let conn = db.conn.lock().unwrap();
            conn.query_row("SELECT rowid FROM rag_documents", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(same_rowid, rag.rowid);
        // ...and both searches find the row under its NEW vector only.
        let hits = db.rag_search(p, &[0.0, 1.0], 5).unwrap();
        assert_eq!(hits.len(), 1, "replaced, not duplicated");
        assert_eq!(hits[0].id, rag.id);
        assert!(hits[0].distance < 0.001, "matches the new vector");

        assert_eq!(db.attachment_indexed_ids(chat).unwrap(), vec![att]);
        let hits = db.attachment_search(chat, &[0.0, 1.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].distance < 0.001);
    }

    #[test]
    fn set_vector_skips_a_row_that_is_gone_or_foreign() {
        // The user may delete a source between the job reading a batch and
        // writing it back — an ordinary race, not a reason to fail the batch.
        // Writing anyway would orphan a vector on a rowid sqlite later reuses.
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(a, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.bump_embed_generation().unwrap();
        let row = db.rag_rows_to_reembed(10).unwrap().remove(0);

        // A foreign partition is refused silently...
        db.rag_set_vector(row.rowid, b, &[0.0, 1.0]).unwrap();
        assert!(db.rag_search(b, &[0.0, 1.0], 5).unwrap().is_empty());
        // ...and so is a rowid that no longer exists.
        db.rag_delete_by_source(a, "s").unwrap();
        db.rag_set_vector(row.rowid, a, &[0.0, 1.0]).unwrap();
        assert!(db.rag_search(a, &[0.0, 1.0], 5).unwrap().is_empty());
        assert_eq!(db.rag_count(a).unwrap(), 0);
    }

    #[test]
    fn set_vector_refuses_a_dimension_mismatch_without_stamping() {
        // Failing loudly is the point: a stamped row holding the old model's
        // vector would look healed while being noise.
        let db = db();
        let p = Uuid::new_v4();
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.bump_embed_generation().unwrap();
        let row = db.rag_rows_to_reembed(10).unwrap().remove(0);

        assert!(db.rag_set_vector(row.rowid, p, &[1.0, 0.0, 0.0]).is_err());
        assert_eq!(
            db.count_rows_to_reembed().unwrap().rag,
            1,
            "still outstanding — the failure left no false progress"
        );
    }

    #[test]
    fn set_vector_works_at_a_new_dimensionality_after_the_tables_are_dropped() {
        // A dimensionality change is still incremental (S5): the job drops the
        // vec0 tables up front, then every row simply reads as foreign and takes
        // the ordinary path — one algorithm for both cases, no special-casing.
        let db = db();
        let (p, chat, att) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(chat, att, "frag", vec![1.0, 0.0]))
            .unwrap();
        db.bump_embed_generation().unwrap();

        db.drop_vector_tables().unwrap();
        assert_eq!(db.rag_dimension().unwrap(), None);

        // The texts survived the drop — which is what makes re-embedding
        // possible at all (a `vec0` table cannot hold two widths, the rows can).
        let rag = db.rag_rows_to_reembed(10).unwrap().remove(0);
        let atts = db.attachment_rows_to_reembed(10).unwrap().remove(0);
        assert_eq!(rag.text, "chunk");
        assert_eq!(atts.text, "frag");

        db.rag_set_vector(rag.rowid, rag.partition, &[0.0, 1.0, 0.0])
            .unwrap();
        db.attachment_set_vector(atts.rowid, atts.partition, &[0.0, 1.0, 0.0])
            .unwrap();

        assert_eq!(
            db.rag_dimension().unwrap(),
            Some(3),
            "both tables recreated at the new size"
        );
        assert_eq!(db.rag_search(p, &[0.0, 1.0, 0.0], 5).unwrap().len(), 1);
        assert_eq!(
            db.attachment_search(chat, &[0.0, 1.0, 0.0], 5)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );
    }

    #[test]
    fn drop_vector_tables_keeps_everything_reset_vectors_would_discard() {
        // The two primitives must stay distinguishable: this one is "keep the
        // texts, re-embed them", `reset_vectors` is "start over". Losing the
        // attachment rows here would mean a dimensionality change silently
        // discarded every chat's index instead of rebuilding it.
        let db = db();
        let (p, chat, att) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
            .unwrap();
        db.attachment_insert(&chunk(chat, att, "frag", vec![1.0, 0.0]))
            .unwrap();
        let fp = crate::shared::embed_identity::EmbedFingerprint::new(
            vec![1.0, 0.0],
            Some("bge-m3".into()),
            "none",
        );
        db.set_embed_fingerprint(&fp).unwrap();
        db.set_rag_stale_profiles(&[p]).unwrap();

        // The production sequence: a model change bumps the generation, then the
        // job drops the tables because the new model is a different width.
        db.bump_embed_generation().unwrap();
        db.drop_vector_tables().unwrap();

        assert_eq!(db.rag_count(p).unwrap(), 1, "document rows kept");
        assert_eq!(
            db.attachment_rows_to_reembed(10).unwrap()[0].text,
            "frag",
            "attachment rows kept — unlike reset_vectors, which deletes them"
        );
        assert_eq!(
            db.embed_fingerprint().unwrap(),
            Some(fp),
            "fingerprint kept"
        );
        assert!(db.rag_is_stale(p).unwrap(), "stale marks kept");
        // Only the vectors are gone, so the searches degrade to empty until the
        // job refills them.
        assert!(db.rag_search(p, &[1.0, 0.0], 5).unwrap().is_empty());
        assert!(
            db.attachment_search(chat, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
    }

    // ---------- self-healing (carried over from the invalidation this replaces) ----------

    #[test]
    fn a_bump_hides_note_vectors_and_the_backfill_restores_search() {
        // What stage 1 achieved by deleting every note vector, now achieved by
        // leaving them in place: the notes are intact, semantic search serves
        // nothing until they are re-embedded, and the ordinary lazy backfill is
        // what re-embeds them.
        let db = db();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let n1 = Note::new(a, "n1", vec![]);
        let n2 = Note::new(a, "n2", vec![]);
        let nb = Note::new(b, "other profile", vec![]);
        for n in [&n1, &n2, &nb] {
            db.note_insert(n).unwrap();
        }
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        db.note_vector_upsert(n2.id, a, &[0.0, 1.0]).unwrap();
        db.note_vector_upsert(nb.id, b, &[1.0, 1.0]).unwrap();

        db.bump_embed_generation().unwrap();

        // The notes survive — and so do the vector rows, unlike stage 1.
        assert_eq!(db.note_list(a, None, &[], None).unwrap().len(), 2);
        assert_eq!(db.note_list(b, None, &[], None).unwrap().len(), 1);
        // Semantic search finds nothing until they are re-embedded...
        assert!(
            db.note_search_semantic(a, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        // ...every profile is affected at once (one global embedder)...
        assert_eq!(db.notes_missing_vectors(a).unwrap().len(), 2);
        assert_eq!(db.notes_missing_vectors(b).unwrap().len(), 1);

        // ...and re-embedding restores search — the self-healing property.
        db.note_vector_upsert(n1.id, a, &[1.0, 0.0]).unwrap();
        assert_eq!(db.note_search_semantic(a, &[1.0, 0.0], 5).unwrap().len(), 1);
        assert_eq!(db.notes_missing_vectors(a).unwrap().len(), 1);
    }

    #[test]
    fn a_bump_empties_the_attachment_index_until_it_is_re_embedded() {
        // The block only points the model at `attachment_search` for files it can
        // really search, so a foreign-generation index must read as absent.
        let db = db();
        let (chat, att) = (Uuid::new_v4(), Uuid::new_v4());
        db.attachment_insert(&chunk(chat, att, "frag", vec![1.0, 0.0]))
            .unwrap();
        assert_eq!(db.attachment_indexed_ids(chat).unwrap(), vec![att]);

        db.bump_embed_generation().unwrap();
        assert!(db.attachment_indexed_ids(chat).unwrap().is_empty());
        assert!(
            db.attachment_search(chat, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty(),
            "the vectors are still there, but they are from another model"
        );

        // Nothing was thrown away: the text is intact, so re-embedding restores it.
        let row = db.attachment_rows_to_reembed(10).unwrap().remove(0);
        assert_eq!(row.text, "frag");
        db.attachment_set_vector(row.rowid, row.partition, &[0.0, 1.0])
            .unwrap();
        assert_eq!(db.attachment_indexed_ids(chat).unwrap(), vec![att]);
        assert_eq!(db.attachment_search(chat, &[0.0, 1.0], 5).unwrap().len(), 1);
    }

    #[test]
    fn switching_back_to_the_previous_model_needs_no_work() {
        // The payoff of keeping the rows: a generation the DB has already seen
        // makes its vectors current again, with nothing re-embedded.
        let db = db();
        let p = Uuid::new_v4();
        let note = Note::new(p, "n", vec![]);
        db.note_insert(&note).unwrap();
        db.note_vector_upsert(note.id, p, &[1.0, 0.0]).unwrap();
        let first = db.embed_generation().unwrap();

        db.bump_embed_generation().unwrap();
        assert!(
            db.note_search_semantic(p, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );

        {
            let conn = db.conn.lock().unwrap();
            meta_set(&conn, KEY_EMBED_GEN, &first.to_string()).unwrap();
        }
        assert_eq!(db.note_search_semantic(p, &[1.0, 0.0], 5).unwrap().len(), 1);
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );
    }

    // ---------- the schema ----------

    #[test]
    fn the_added_column_survives_reopening_and_keeps_the_data() {
        // `baseline_ddl` runs on every open, so the guarded ALTER must be
        // idempotent — and must not disturb rows written by the previous run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let (p, chat) = (Uuid::new_v4(), Uuid::new_v4());
        {
            let db = Db::open(&path).unwrap();
            let note = Note::new(p, "n", vec![]);
            db.note_insert(&note).unwrap();
            db.note_vector_upsert(note.id, p, &[1.0, 0.0]).unwrap();
            db.rag_insert(&RagDocument::new(p, "s", "chunk", vec![1.0, 0.0]))
                .unwrap();
            db.attachment_insert(&chunk(chat, Uuid::new_v4(), "frag", vec![1.0, 0.0]))
                .unwrap();
        }
        // Reopening applies the DDL a second time.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.note_list(p, None, &[], None).unwrap().len(), 1);
        assert_eq!(db.rag_count(p).unwrap(), 1);
        assert_eq!(db.attachment_indexed_ids(chat).unwrap().len(), 1);
        assert_eq!(
            db.count_rows_to_reembed().unwrap(),
            ReembedPending::default(),
            "the stamps written by the first run survived"
        );
        assert_eq!(db.note_search_semantic(p, &[1.0, 0.0], 5).unwrap().len(), 1);
    }

    #[test]
    fn add_column_if_missing_is_idempotent() {
        let db = db();
        let conn = db.conn.lock().unwrap();
        assert!(column_exists(&conn, "rag_documents", "embed_gen").unwrap());
        assert!(!column_exists(&conn, "rag_documents", "nope").unwrap());
        // A second call on a column that already exists is a no-op, not an error.
        add_column_if_missing(&conn, "rag_documents", "embed_gen", "INTEGER").unwrap();
    }
}
