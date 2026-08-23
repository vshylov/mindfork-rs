//! `/reindex` — re-embeds every stored vector with the current embedding model
//! (stage 2 of docs/research/embedding-model-change-reindex.md).
//!
//! **Re-embedding is not re-chunking** (research §3). A model change invalidates
//! vectors, not text, and every store already keeps the text its vectors were
//! made from — `notes.content`, `rag_documents.chunk_text`,
//! `attachment_documents.chunk_text`. So this job needs no source files and no
//! chunker, which is what lets it do three things `/rag rebuild` cannot:
//!
//! - repair legacy knowledge-base rows whose stored text is absent and whose
//!   file is gone (a rebuild counts those as errors and drops them);
//! - cover **every** profile in one run, instead of one per switch;
//! - bring attachment indexes back without the user re-attaching each file.
//!
//! **And what has no vectors at all** (the backfill stage). Re-embedding assumes
//! rows to re-embed; a chat attachment can be missing them entirely — a data
//! directory carried to another machine without `data.db` brings the chats, and
//! with them every attachment's text, but no index over it (spec §5.2). The text
//! is right there in the chat file, so the repair needs no re-attaching either:
//! the stage walks the chat files, finds by-reference attachments the index holds
//! **no** rows for, and rebuilds them through the very function `/file attach`
//! uses. It runs first, and it is disjoint from the queues below by construction
//! — see [`Db::attachment_known_ids`](crate::shared::storage::db::Db::attachment_known_ids).
//!
//! It is driven by the generation marker: a row whose `embed_gen` is not current
//! is work, and stamping it removes it from the queue. That makes the job
//! **resumable** — interrupting it leaves a consistent partial state, and a rerun
//! picks up exactly where it stopped. Knowledge-base search stays refused
//! meanwhile through the per-profile stale marks from stage 1, so a half-finished
//! index never answers a query.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, RagProgress};
use crate::entities::attachment::AttachMode;
use crate::features::tools::rag::ChunkParams;
use crate::shared::api::{EmbedRole, Embedder};
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;
use crate::shared::storage::db::{Db, ReembedPending, ReembedRow};

use super::Orchestrator;
use super::attachments::{AttachIndex, index_attachment};
use super::rag::EMBED_BATCH_CHUNKS;

impl Orchestrator {
    /// Starts the background re-embed of every stored vector (`/reindex`).
    /// Shares the single background-indexing slot with the `/rag` commands (one
    /// at a time, same progress banner), so it cancels whichever was running.
    pub(super) fn handle_reindex(&mut self) {
        let cancel = self.reset_rag_cancel();
        spawn_reembed(Reembed {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            // Only the backfill stage chunks anything, and it chunks exactly as
            // `/file attach` would today — a file restored from a chat file must
            // land in the same shape as one attached by hand.
            params: ChunkParams::from_settings(&self.config.rag),
            cancel,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
        });
    }
}

/// Parameters of the background re-embed task.
struct Reembed {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    /// Chunking for the backfill stage (the re-embed stages reuse stored text
    /// and never chunk).
    params: ChunkParams,
    cancel: CancellationToken,
    /// The interface language (axis B) — progress and errors are for the user.
    loc: &'static Locale,
    evt_tx: UnboundedSender<AppEvent>,
}

/// The three vector stores. They differ only in how a batch is fetched and
/// written back, so the job runs one loop over this rather than three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Store {
    Notes,
    Attachments,
    Rag,
}

impl Store {
    /// Cheapest first: notes are few and restore memory almost immediately,
    /// while the knowledge base is the largest and is the one held back by a
    /// stale mark until the very end anyway.
    const ALL: [Store; 3] = [Store::Notes, Store::Attachments, Store::Rag];

    fn label(self, loc: &'static Locale) -> &'static str {
        match self {
            Store::Notes => loc.t("ui.reindex.store.notes"),
            Store::Attachments => loc.t("ui.reindex.store.attachments"),
            Store::Rag => loc.t("ui.reindex.store.knowledge_base"),
        }
    }

    /// The next batch of rows whose vectors predate the current generation.
    fn fetch(self, db: &Db, limit: usize) -> anyhow::Result<Vec<ReembedRow>> {
        match self {
            Store::Notes => db.notes_to_reembed(limit),
            Store::Attachments => db.attachment_rows_to_reembed(limit),
            Store::Rag => db.rag_rows_to_reembed(limit),
        }
    }

    /// Writes one row's new vector back, stamping the current generation.
    fn write(self, db: &Db, row: &ReembedRow, embedding: &[f32]) -> anyhow::Result<()> {
        match self {
            // Notes are keyed by their domain id, not by rowid — the same upsert
            // the lazy backfill uses, so both paths stay identical.
            Store::Notes => db.note_vector_upsert(row.id, row.partition, embedding),
            Store::Attachments => db.attachment_set_vector(row.rowid, row.partition, embedding),
            Store::Rag => db.rag_set_vector(row.rowid, row.partition, embedding),
        }
    }

    fn pending(self, p: &ReembedPending) -> usize {
        match self {
            Store::Notes => p.notes,
            Store::Attachments => p.attachments,
            Store::Rag => p.rag,
        }
    }
}

/// Outcome of draining one store (or of the backfill stage). The counts written
/// are not carried here — they are threaded through the job-wide [`Counters`],
/// so the banner shows one continuous figure across every stage.
struct Drained {
    errors: usize,
    /// The embedder itself failed — the whole job must stop, not just this store.
    fatal: Option<String>,
}

/// The job's two running totals, which are **not** the same number.
///
/// A re-embed stage writes one vector per unit of work, so for it they move
/// together. The backfill's unit is a *file*: one attachment is one step of the
/// banner and many vectors in the database. Reporting the banner's figure at the
/// end would then undercount the work, and reporting the vector count in the
/// banner would need a total nobody can know before chunking. So the banner
/// counts what was planned (`done` against a total fixed up front) and the
/// closing note counts what was written (`rows`).
#[derive(Default)]
struct Counters {
    /// Units of work finished — the banner's `index/total`.
    done: usize,
    /// Vectors actually written — the closing note's figure.
    rows: usize,
}

/// A by-reference attachment the index holds no rows for at all: the chat file
/// has its text, the database has nothing. Found by [`scan_missing_attachments`]
/// and rebuilt by [`backfill_attachments`].
///
/// Only the ids are kept. The text is what makes an attachment big — up to
/// `MAX_ATTACH_BYTES` each — so holding every missing file's text to plan the
/// work could cost more memory than the work itself; the second pass loads one
/// chat at a time instead, which also re-checks that the attachment is still
/// there.
struct MissingIndex {
    chat_id: Uuid,
    attachment_id: Uuid,
}

fn spawn_reembed(task: Reembed) {
    let Reembed {
        embedder,
        storage,
        params,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Embedder precheck — also tells us the dimensionality we are moving to.
        let dim = match embedder
            .embed(vec!["ping".into()], EmbedRole::Passage)
            .await
        {
            Ok(v) => v.first().map(|e| e.len()).unwrap_or(0),
            Err(err) => {
                send(RagProgress::Failed(loc.tf(
                    "ui.err.rag_embedder_unavailable",
                    &[("err", &err.to_string())],
                )));
                return;
            }
        };
        if dim == 0 {
            send(RagProgress::Failed(loc.t("ui.err.rag_empty_vector").into()));
            return;
        }

        // 2. A `vec0` table is fixed-width, so a different dimensionality cannot
        //    reuse it. Dropping both leaves every row without a vector — exactly
        //    the state a foreign generation already describes, so the loop below
        //    needs no special case (research §8.1, S5). Document rows, the
        //    fingerprint and the stale marks are kept.
        let current_dim = storage.db().rag_dimension().unwrap_or(None);
        if matches!(current_dim, Some(d) if d != dim)
            && let Err(err) = storage.db().drop_vector_tables()
        {
            send(RagProgress::Failed(loc.tf(
                "ui.err.rag_reset_vectors",
                &[("err", &err.to_string())],
            )));
            return;
        }

        // 3. How much there is to do.
        let pending = match storage.db().count_rows_to_reembed() {
            Ok(p) => p,
            Err(err) => {
                send(RagProgress::Failed(
                    loc.tf("ui.err.rag_read_kb", &[("err", &err.to_string())]),
                ));
                return;
            }
        };
        // Attachments with no rows at all — disjoint from `pending` above, which
        // only ever counts rows that exist. A blocking walk of the chat files:
        // it parses JSON off the async runtime, exactly as the search index's
        // reconciliation pass does.
        let missing = {
            let storage = storage.clone();
            match tokio::task::spawn_blocking(move || scan_missing_attachments(&storage)).await {
                Ok(found) => found,
                Err(err) => {
                    tracing::warn!(error = %err, "reindex: the attachment scan panicked");
                    Vec::new()
                }
            }
        };
        let total = pending.notes + pending.attachments + pending.rag + missing.len();
        if total == 0 {
            // Everything already matches the current model — say so plainly
            // rather than pretending work happened.
            send(RagProgress::Reembedded {
                rows: 0,
                errors: 0,
                cancelled: false,
            });
            return;
        }
        send(RagProgress::Started { total });

        let mut counters = Counters::default();
        let mut errors = 0usize;

        // First: the files that have no index at all. Cheapest to reason about
        // (nothing downstream depends on it) and, after a data move, the only
        // work there is.
        if !missing.is_empty() {
            let drained = backfill_attachments(
                &missing,
                &embedder,
                &storage,
                params,
                &cancel,
                loc,
                total,
                &mut counters,
                &evt_tx,
            )
            .await;
            errors += drained.errors;
            if let Some(err) = drained.fatal {
                send(RagProgress::Failed(err));
                return;
            }
        }

        for store in Store::ALL {
            if cancel.is_cancelled() || store.pending(&pending) == 0 {
                continue;
            }
            let drained = drain_store(
                store,
                &embedder,
                &storage,
                &cancel,
                loc,
                total,
                &mut counters,
                &evt_tx,
            )
            .await;
            errors += drained.errors;
            if let Some(err) = drained.fatal {
                send(RagProgress::Failed(err));
                return;
            }
        }

        // 4. Lift the stale marks once the knowledge base holds no old-model
        //    vectors at all. Derived from the queue rather than from "the loop
        //    ran", so a cancelled or partly failed run correctly leaves them.
        if storage
            .db()
            .count_rows_to_reembed()
            .map(|p| p.rag == 0)
            .unwrap_or(false)
            && let Err(err) = storage.db().set_rag_stale_profiles(&[])
        {
            tracing::warn!(error = %err, "failed to clear the stale knowledge-base marks");
        }

        send(RagProgress::Reembedded {
            rows: counters.rows,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Re-embeds one store batch by batch until its queue is empty, cancelled, or
/// stuck. [`Counters`] is job-wide, so the banner shows one continuous progress
/// figure across every stage.
#[allow(clippy::too_many_arguments)] // cohesive job state, threaded from one call site
async fn drain_store(
    store: Store,
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    cancel: &CancellationToken,
    loc: &'static Locale,
    total: usize,
    counters: &mut Counters,
    evt_tx: &UnboundedSender<AppEvent>,
) -> Drained {
    let mut errors = 0usize;

    loop {
        if cancel.is_cancelled() {
            break;
        }
        let rows = match store.fetch(storage.db(), EMBED_BATCH_CHUNKS) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(?store, error = %err, "re-embed: failed to read the work queue");
                errors += 1;
                break;
            }
        };
        if rows.is_empty() {
            break;
        }

        let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
        // Passage, identical to what the original writers used (rag.rs,
        // attachments.rs, notes/save.rs). Re-embedding under a different role
        // would quietly re-create the mixed-space problem this job exists to fix.
        let embeddings = match embedder.embed(texts, EmbedRole::Passage).await {
            Ok(v) if v.len() == rows.len() => v,
            // The embedding server died mid-job, or answered incoherently.
            // Retrying would spin on the same batch forever, so stop the job and
            // report — what is already stamped stays valid, and a rerun resumes.
            Ok(_) => {
                return Drained {
                    errors,
                    fatal: Some(loc.t("ui.err.rag_wrong_vector_count").into()),
                };
            }
            Err(err) => {
                return Drained {
                    errors,
                    fatal: Some(loc.tf(
                        "ui.err.rag_embedder_unavailable",
                        &[("err", &err.to_string())],
                    )),
                };
            }
        };

        let (written, write_errors) = write_batch(store, storage, &rows, embeddings, counters);
        errors += write_errors;
        // A batch that wrote nothing leaves the queue unchanged, so the next
        // fetch returns the same rows — the loop would never end. Stop this
        // store instead; the rows stay foreign and a later run can retry them.
        if written == 0 {
            break;
        }

        let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
            index: counters.done,
            total,
            name: store.label(loc).to_string(),
            dir: String::new(),
            chunks_done: 0,
            chunks_total: 0,
        }));
    }

    Drained {
        errors,
        fatal: None,
    }
}

/// Walks the chat files and lists by-reference attachments the index holds no
/// rows for. Blocking (JSON parsing + SQLite), so it runs on the blocking pool.
///
/// **Scope.** By-reference only, because inline attachments are deliberately not
/// indexed at all (their whole text is in every request already, so search would
/// return duplicates of what the model can see). Hidden chats are skipped: a
/// soft-deleted conversation must not have work done for it, let alone become
/// searchable again. And `attachment_known_ids` — not `attachment_indexed_ids` —
/// keeps this disjoint from the re-embed queues: a file whose rows are merely
/// from an older model already has a home there.
///
/// One chat's failure is logged and skipped, never aborting the walk: a single
/// unreadable file must not cost the repair of all the others (the rule the
/// search index's reconciliation already follows).
fn scan_missing_attachments(storage: &Storage) -> Vec<MissingIndex> {
    let files = match storage.json().chat_files() {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "reindex: cannot list the chat files");
            return Vec::new();
        }
    };
    let mut missing = Vec::new();
    for file in files {
        let chat = match storage.json().load_chat(file.id) {
            Ok(Some(chat)) => chat,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(chat = %file.id, error = %format!("{err:#}"),
                    "reindex: skipped a chat whose file could not be read");
                continue;
            }
        };
        if chat.is_hidden || !chat.attachments.iter().any(is_by_reference) {
            continue;
        }
        let known = match storage.db().attachment_known_ids(chat.id) {
            Ok(ids) => ids,
            Err(err) => {
                tracing::warn!(chat = %chat.id, error = %err,
                    "reindex: cannot read which attachments are indexed");
                continue;
            }
        };
        missing.extend(
            chat.attachments
                .iter()
                .filter(|a| is_by_reference(a) && !known.contains(&a.id))
                .map(|a| MissingIndex {
                    chat_id: chat.id,
                    attachment_id: a.id,
                }),
        );
    }
    missing
}

/// Whether this attachment is one the index is supposed to hold (fork F13 —
/// see [`scan_missing_attachments`]).
fn is_by_reference(attachment: &crate::entities::attachment::Attachment) -> bool {
    attachment.mode == AttachMode::ByReference
}

/// Rebuilds the listed attachments from the text in their chat files, through
/// the same [`index_attachment`] `/file attach` uses.
///
/// `missing` comes out of the scan in chat order, so one chat is loaded once and
/// reused for its whole run of attachments. Loading here rather than carrying
/// the text from the scan bounds the memory to a single chat — and re-reads the
/// file, so an attachment removed since the scan is simply not found and not
/// rebuilt.
///
/// A failure is fatal for the job, matching [`drain_store`]: both realistic
/// causes — the embedder gone, or a vector dimensionality the database cannot
/// take — will fail the next file identically, and grinding through fifty of
/// them to say so fifty times helps nobody. What was written stays valid, and a
/// rerun resumes from it.
#[allow(clippy::too_many_arguments)] // cohesive job state, threaded from one call site
async fn backfill_attachments(
    missing: &[MissingIndex],
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    params: ChunkParams,
    cancel: &CancellationToken,
    loc: &'static Locale,
    total: usize,
    counters: &mut Counters,
    evt_tx: &UnboundedSender<AppEvent>,
) -> Drained {
    let mut loaded: Option<crate::entities::chat::Chat> = None;

    for item in missing {
        if cancel.is_cancelled() {
            break;
        }
        if loaded.as_ref().is_none_or(|c| c.id != item.chat_id) {
            loaded = storage
                .json()
                .load_chat(item.chat_id)
                .unwrap_or_else(|err| {
                    tracing::warn!(chat = %item.chat_id, error = %format!("{err:#}"),
                    "reindex: cannot reread a chat to rebuild its attachment index");
                    None
                });
        }
        // The chat or the attachment is gone since the scan (removed, or the
        // chat deleted). Not an error — there is simply nothing to rebuild, and
        // the unit is still spent so the banner reaches its total.
        let Some((chat, attachment)) = loaded.as_ref().and_then(|chat| {
            chat.attachments
                .iter()
                .find(|a| a.id == item.attachment_id)
                .map(|a| (chat, a))
        }) else {
            counters.done += 1;
            continue;
        };

        let index = AttachIndex {
            chat_id: item.chat_id,
            attachment_id: attachment.id,
            name: attachment.name.clone(),
            source: attachment.source.clone(),
            text: attachment.text.clone(),
        };
        // The banner names the file and the conversation it belongs to: after a
        // move there may be many, and "which chat" is what tells them apart.
        let title = chat.title.clone();
        let name = index.name.clone();
        let at = counters.done;
        let outcome = index_attachment(
            embedder,
            storage,
            params,
            &index,
            loc,
            cancel,
            |chunks_done, chunks_total| {
                let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
                    index: at,
                    total,
                    name: name.clone(),
                    dir: title.clone(),
                    chunks_done,
                    chunks_total,
                }));
            },
        )
        .await;

        counters.done += 1;
        match outcome {
            Ok(rows) => counters.rows += rows,
            Err(reason) => {
                tracing::warn!(chat = %item.chat_id, attachment = %index.name, reason = %reason,
                    "reindex: failed to rebuild an attachment index");
                return Drained {
                    errors: 1,
                    fatal: Some(reason),
                };
            }
        }
    }

    Drained {
        errors: 0,
        fatal: None,
    }
}

/// Writes one batch's vectors back (each row: vector, then generation stamp —
/// `Store::write` delegates to the DB primitives that keep that order).
/// Returns `(written, errors)`: a failed row is counted and logged, never
/// fatal — it stays foreign and a later run can retry it.
fn write_batch(
    store: Store,
    storage: &Arc<Storage>,
    rows: &[ReembedRow],
    embeddings: Vec<Vec<f32>>,
    counters: &mut Counters,
) -> (usize, usize) {
    let mut written = 0usize;
    let mut errors = 0usize;
    for (row, embedding) in rows.iter().zip(embeddings) {
        match store.write(storage.db(), row, &embedding) {
            Ok(()) => {
                written += 1;
                // One row re-embedded is one unit of work and one vector.
                counters.done += 1;
                counters.rows += 1;
            }
            Err(err) => {
                errors += 1;
                tracing::warn!(?store, rowid = row.rowid, error = %err, "re-embed: failed to write a vector");
            }
        }
    }
    (written, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::attachment::{AttachMode, Attachment, AttachmentChunk};
    use crate::entities::chat::Chat;
    use crate::entities::note::Note;
    use crate::entities::profile::Profile;
    use crate::entities::rag::RagDocument;
    use crate::shared::api::mock::MockEmbedder;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::paths::Paths;
    use uuid::Uuid;

    fn deps() -> (tempfile::TempDir, Arc<Storage>) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        (dir, storage)
    }

    fn run(
        storage: &Arc<Storage>,
        embedder: Arc<dyn Embedder>,
        cancel: CancellationToken,
    ) -> tokio::sync::mpsc::UnboundedReceiver<AppEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_reembed(Reembed {
            embedder,
            storage: storage.clone(),
            params: ChunkParams::default(),
            cancel,
            loc: locale(Lang::Ru),
            evt_tx: tx,
        });
        rx
    }

    /// Drains the channel and returns the terminal event.
    async fn finish(rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> RagProgress {
        collect(rx)
            .await
            .pop()
            .expect("the job must report an outcome")
    }

    /// Every progress event the job sent, in order, up to and including the
    /// terminal one — for the tests that are about the banner rather than the
    /// outcome.
    async fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> Vec<RagProgress> {
        let mut seen = Vec::new();
        while let Some(AppEvent::RagProgress(p)) = rx.recv().await {
            let terminal = matches!(p, RagProgress::Reembedded { .. } | RagProgress::Failed(_));
            seen.push(p);
            if terminal {
                break;
            }
        }
        seen
    }

    /// Seeds one note and one knowledge-base chunk, then retires them.
    fn seed_and_retire(storage: &Arc<Storage>, profile: Uuid) {
        let note = Note::new(profile, "a note about brevity", vec![]);
        storage.db().note_insert(&note).unwrap();
        storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0; 16])
            .unwrap();
        storage
            .db()
            .rag_insert(&RagDocument::new(
                profile,
                "kb.txt",
                "a chunk",
                vec![1.0; 16],
            ))
            .unwrap();
        storage.db().set_rag_stale_profiles(&[profile]).unwrap();
        storage.db().bump_embed_generation().unwrap();
    }

    #[tokio::test]
    async fn reembeds_every_store_and_lifts_the_stale_mark() {
        let (_d, storage) = deps();
        let profile = Uuid::new_v4();
        seed_and_retire(&storage, profile);
        assert!(storage.db().rag_is_stale(profile).unwrap());

        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(16)),
            CancellationToken::new(),
        );
        match finish(rx).await {
            RagProgress::Reembedded {
                rows,
                errors,
                cancelled,
            } => {
                assert_eq!(rows, 2, "one note + one chunk");
                assert_eq!(errors, 0);
                assert!(!cancelled);
            }
            other => panic!("expected Reembedded, got {other:?}"),
        }

        let pending = storage.db().count_rows_to_reembed().unwrap();
        assert_eq!(pending, ReembedPending::default(), "the queue is drained");
        assert!(
            !storage.db().rag_is_stale(profile).unwrap(),
            "a fully re-embedded base is no longer stale"
        );
    }

    /// Writes a chat file with one attachment in the given mode, as the app
    /// would have saved it — the state a data directory arrives in when it was
    /// copied without `data.db`.
    fn seed_chat_with_attachment(
        storage: &Arc<Storage>,
        text: &str,
        mode: AttachMode,
        hidden: bool,
    ) -> (Chat, Attachment) {
        let profile = Profile::new("A", "sys");
        storage.json().upsert_profile(&profile).unwrap();
        let mut chat = Chat::from_profile(&profile, "a chat with a file");
        let attachment = Attachment::new("report.md", "/old/report.md", text.to_string(), 42, mode);
        chat.attachments.push(attachment.clone());
        chat.is_hidden = hidden;
        storage.json().save_chat(&chat).unwrap();
        (chat, attachment)
    }

    /// The case the whole stage exists for: the chat file holds the text, the
    /// database holds nothing, and `/reindex` rebuilds the index without the
    /// user re-attaching a file that is on the other machine.
    #[tokio::test]
    async fn an_attachment_with_no_rows_is_rebuilt_from_the_chat_file() {
        let (_d, storage) = deps();
        // Long enough to span several chunks (the default target is 800
        // characters): one file is one step of the banner and many vectors, and
        // that difference is what the job's two counters exist for.
        let text = format!(
            "лунная база строится в 2031 году. {}",
            "ещё текст. ".repeat(300)
        );
        let (chat, attachment) =
            seed_chat_with_attachment(&storage, &text, AttachMode::ByReference, false);
        assert!(
            storage
                .db()
                .attachment_indexed_ids(chat.id)
                .unwrap()
                .is_empty()
        );

        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(16)),
            CancellationToken::new(),
        );
        let seen = collect(rx).await;
        assert!(
            matches!(seen.first(), Some(RagProgress::Started { total: 1 })),
            "one file is one unit of work: {seen:?}"
        );
        match seen.last() {
            Some(RagProgress::Reembedded {
                rows,
                errors,
                cancelled,
            }) => {
                assert!(
                    *rows > 1,
                    "and many vectors — the closing note counts those, not the units: {rows}"
                );
                assert_eq!(*errors, 0);
                assert!(!cancelled);
            }
            other => panic!("expected Reembedded, got {other:?}"),
        }

        assert_eq!(
            storage.db().attachment_indexed_ids(chat.id).unwrap(),
            vec![attachment.id],
            "the file is searchable again"
        );
        let query = Arc::new(MockEmbedder::new(16))
            .embed(vec!["лунная база".into()], EmbedRole::Query)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let hits = storage.db().attachment_search(chat.id, &query, 3).unwrap();
        assert!(
            hits.iter().any(|h| h.text.contains("лунная база")),
            "and the text really is in the index: {hits:?}"
        );
    }

    /// Scope. An inline file is deliberately never indexed (its text is in every
    /// request already), and a hidden chat must not have work done for it — the
    /// repair must not resurrect a soft-deleted conversation's index.
    #[tokio::test]
    async fn inline_attachments_and_hidden_chats_are_left_alone() {
        for (mode, hidden) in [(AttachMode::Inline, false), (AttachMode::ByReference, true)] {
            let (_d, storage) = deps();
            let (chat, _) = seed_chat_with_attachment(&storage, "какой-то текст", mode, hidden);

            let rx = run(
                &storage,
                Arc::new(MockEmbedder::new(16)),
                CancellationToken::new(),
            );
            assert_eq!(
                finish(rx).await,
                RagProgress::Reembedded {
                    rows: 0,
                    errors: 0,
                    cancelled: false
                },
                "{mode:?}, hidden={hidden}: nothing to do"
            );
            assert!(
                storage
                    .db()
                    .attachment_known_ids(chat.id)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    /// A chat can hold both kinds, and then "does this chat have anything to do?"
    /// is not the same question as "is this file one to index". Only the
    /// by-reference half is rebuilt; the inline one stays out of the index, as
    /// it would have if it were attached today.
    #[tokio::test]
    async fn only_the_by_reference_half_of_a_mixed_chat_is_rebuilt() {
        let (_d, storage) = deps();
        let profile = Profile::new("A", "sys");
        storage.json().upsert_profile(&profile).unwrap();
        let mut chat = Chat::from_profile(&profile, "a chat with two files");
        let inline = Attachment::new(
            "small.txt",
            "/old/small.txt",
            "короткий текст целиком в запросе".to_string(),
            10,
            AttachMode::Inline,
        );
        let by_ref = Attachment::new(
            "big.md",
            "/old/big.md",
            "длинный текст, который читают по ссылке".to_string(),
            99,
            AttachMode::ByReference,
        );
        chat.attachments.push(inline.clone());
        chat.attachments.push(by_ref.clone());
        storage.json().save_chat(&chat).unwrap();

        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(16)),
            CancellationToken::new(),
        );
        assert!(matches!(
            finish(rx).await,
            RagProgress::Reembedded { errors: 0, .. }
        ));

        assert_eq!(
            storage.db().attachment_known_ids(chat.id).unwrap(),
            vec![by_ref.id],
            "the inline file must not have been indexed alongside it"
        );
    }

    /// The two halves of the job must not overlap. A file whose rows are merely
    /// from an older model is the re-embed queue's work: it keeps its rows (and
    /// their text), gets fresh vectors, and is **not** re-chunked from the chat
    /// file. If the backfill asked "is it searchable?" instead of "does it have
    /// rows?", this file would be done twice and the queue it was counted into
    /// would come up short.
    #[tokio::test]
    async fn a_file_whose_rows_are_only_stale_stays_with_the_re_embed_queue() {
        let (_d, storage) = deps();
        let (chat, attachment) = seed_chat_with_attachment(
            &storage,
            "первый фрагмент. второй фрагмент.",
            AttachMode::ByReference,
            false,
        );
        // One hand-written row, then retire it: rows exist, none is searchable.
        storage
            .db()
            .attachment_insert(&AttachmentChunk::new(
                chat.id,
                attachment.id,
                &attachment.name,
                "первый фрагмент",
                vec![1.0; 16],
            ))
            .unwrap();
        storage.db().bump_embed_generation().unwrap();
        assert!(
            storage
                .db()
                .attachment_indexed_ids(chat.id)
                .unwrap()
                .is_empty(),
            "not searchable"
        );
        assert!(
            scan_missing_attachments(&storage).is_empty(),
            "but not missing either — the re-embed queue owns it"
        );

        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(16)),
            CancellationToken::new(),
        );
        match finish(rx).await {
            RagProgress::Reembedded { rows, errors, .. } => {
                assert_eq!(rows, 1, "the one existing row, re-embedded once");
                assert_eq!(errors, 0);
            }
            other => panic!("expected Reembedded, got {other:?}"),
        }
        // Re-chunking would have replaced the hand-written row with the chunker's
        // own; the text surviving verbatim is what proves it did not happen.
        let query = Arc::new(MockEmbedder::new(16))
            .embed(vec!["первый фрагмент".into()], EmbedRole::Query)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let hits = storage.db().attachment_search(chat.id, &query, 5).unwrap();
        assert_eq!(hits.len(), 1, "still exactly one row: {hits:?}");
        assert_eq!(hits[0].text, "первый фрагмент");
    }

    /// An attachment removed between the scan and the work is not an error and
    /// not a rebuild — the second read of the chat file is what notices.
    #[tokio::test]
    async fn an_attachment_gone_since_the_scan_is_simply_skipped() {
        let (_d, storage) = deps();
        let (mut chat, _) = seed_chat_with_attachment(
            &storage,
            "текст, который сейчас исчезнет",
            AttachMode::ByReference,
            false,
        );
        let missing = scan_missing_attachments(&storage);
        assert_eq!(missing.len(), 1);

        chat.attachments.clear();
        storage.json().save_chat(&chat).unwrap();

        let mut counters = Counters::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let drained = backfill_attachments(
            &missing,
            &(Arc::new(MockEmbedder::new(16)) as Arc<dyn Embedder>),
            &storage,
            ChunkParams::default(),
            &CancellationToken::new(),
            locale(Lang::Ru),
            1,
            &mut counters,
            &tx,
        )
        .await;

        assert_eq!(drained.errors, 0);
        assert!(drained.fatal.is_none());
        assert_eq!(counters.rows, 0, "nothing was rebuilt");
        assert_eq!(
            counters.done, 1,
            "but the unit is spent, so the banner still reaches its total"
        );
    }

    #[tokio::test]
    async fn nothing_to_do_reports_zero_without_touching_anything() {
        let (_d, storage) = deps();
        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(16)),
            CancellationToken::new(),
        );
        assert_eq!(
            finish(rx).await,
            RagProgress::Reembedded {
                rows: 0,
                errors: 0,
                cancelled: false
            }
        );
    }

    #[tokio::test]
    async fn a_dead_embedder_fails_the_job_without_stamping() {
        let (_d, storage) = deps();
        let profile = Uuid::new_v4();
        seed_and_retire(&storage, profile);
        let before = storage.db().count_rows_to_reembed().unwrap();

        let rx = run(
            &storage,
            Arc::new(crate::shared::api::UnavailableEmbedder),
            CancellationToken::new(),
        );
        assert!(matches!(finish(rx).await, RagProgress::Failed(_)));
        assert_eq!(
            storage.db().count_rows_to_reembed().unwrap(),
            before,
            "a failed job must leave the queue untouched, so a rerun redoes it"
        );
        assert!(
            storage.db().rag_is_stale(profile).unwrap(),
            "and must not lift the stale mark"
        );
    }

    #[tokio::test]
    async fn cancelled_before_start_leaves_the_stale_mark() {
        let (_d, storage) = deps();
        let profile = Uuid::new_v4();
        seed_and_retire(&storage, profile);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let rx = run(&storage, Arc::new(MockEmbedder::new(16)), cancel);
        match finish(rx).await {
            RagProgress::Reembedded { cancelled, .. } => assert!(cancelled),
            other => panic!("expected a cancelled Reembedded, got {other:?}"),
        }
        assert!(
            storage.db().rag_is_stale(profile).unwrap(),
            "an interrupted run must keep search refused — the base is still mixed"
        );
    }

    /// The end-to-end case the whole track exists for, against real models:
    /// index under one, swap to another of the **same dimensionality** (which no
    /// dimension check can see), and assert that `/reindex` actually restores
    /// retrieval — not merely that it reports success. Needs two servers:
    ///
    /// ```text
    /// MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1       # bge-m3
    /// MINDFORK_EMBED_URL_ALT=http://127.0.0.1:8002/v1   # multilingual-e5-large-instruct
    /// ```
    #[tokio::test]
    #[ignore = "requires two live embedding servers (MINDFORK_EMBED_URL, MINDFORK_EMBED_URL_ALT)"]
    async fn reindex_restores_retrieval_after_a_model_swap_live() {
        let (Some(a), Some(b)) = (
            crate::shared::api::live_client("MINDFORK_EMBED_URL", "MINDFORK_EMBED_KEY"),
            crate::shared::api::live_client("MINDFORK_EMBED_URL_ALT", "MINDFORK_EMBED_KEY_ALT"),
        ) else {
            eprintln!("skip: MINDFORK_EMBED_URL / MINDFORK_EMBED_URL_ALT not set");
            return;
        };
        let model_a: Arc<dyn Embedder> = Arc::new(a);
        let model_b: Arc<dyn Embedder> = Arc::new(b);

        let (_d, storage) = deps();
        let profile = Uuid::new_v4();
        const PARIS: &str = "The capital of France is Paris, the country's largest city.";
        let corpus = [
            PARIS,
            "The cat sat on the windowsill and watched the rain.",
            "Rust is a systems programming language focused on safety.",
        ];
        const QUERY: &str = "Which city is the capital of France?";

        // 1. Index the corpus and one note under model A.
        let vectors = model_a
            .embed(
                corpus.iter().map(|s| s.to_string()).collect(),
                EmbedRole::Passage,
            )
            .await
            .unwrap();
        for (text, vector) in corpus.iter().zip(vectors) {
            storage
                .db()
                .rag_insert(&RagDocument::new(profile, "kb.txt", *text, vector))
                .unwrap();
        }
        let note = Note::new(profile, "the user prefers concise answers", vec![]);
        storage.db().note_insert(&note).unwrap();
        let note_vec = model_a
            .embed(vec![note.content.clone()], EmbedRole::Passage)
            .await
            .unwrap()
            .remove(0);
        storage
            .db()
            .note_vector_upsert(note.id, profile, &note_vec)
            .unwrap();

        // 2. The guard records model A, then sees model B and retires everything.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let guard = |inner: Arc<dyn Embedder>, name: &str| {
            super::super::embed_guard::EmbedGuard::new(
                inner,
                storage.clone(),
                Some(name.to_string()),
                crate::shared::embed_prefix::EmbedConvention::None,
                locale(Lang::Ru),
                tx.clone(),
            )
        };
        guard(model_a, "bge-m3")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();
        guard(model_b.clone(), "e5-large-instruct")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert!(
            storage.db().rag_is_stale(profile).unwrap(),
            "the swap must mark the knowledge base stale"
        );
        let before = storage.db().count_rows_to_reembed().unwrap();
        assert_eq!(before.rag, corpus.len(), "every chunk is queued");
        assert_eq!(before.notes, 1, "the note is queued too");

        // 3. Re-embed everything with model B.
        let rx = run(&storage, model_b.clone(), CancellationToken::new());
        match finish(rx).await {
            RagProgress::Reembedded {
                rows,
                errors,
                cancelled,
            } => {
                assert_eq!(rows, corpus.len() + 1);
                assert_eq!(errors, 0);
                assert!(!cancelled);
            }
            other => panic!("expected Reembedded, got {other:?}"),
        }
        assert_eq!(
            storage.db().count_rows_to_reembed().unwrap(),
            ReembedPending::default()
        );
        assert!(!storage.db().rag_is_stale(profile).unwrap());

        // 4. The payoff: a model-B query now retrieves the right chunk. Reporting
        //    success is not enough — retrieval itself has to work again.
        let query_vec = model_b
            .embed(vec![QUERY.into()], EmbedRole::Query)
            .await
            .unwrap()
            .remove(0);
        let hits = storage.db().rag_search(profile, &query_vec, 3).unwrap();
        assert_eq!(
            hits.first().map(|h| h.chunk_text.as_str()),
            Some(PARIS),
            "after re-embedding, the correct chunk ranks first again: {hits:?}"
        );
        // And the note is searchable under the new model as well.
        let note_query = model_b
            .embed(vec!["how should I answer?".into()], EmbedRole::Passage)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            storage
                .db()
                .note_search_semantic(profile, &note_query, 5)
                .unwrap()
                .len(),
            1,
            "the note's vector was rewritten, so semantic recall sees it again"
        );
    }

    /// A dimensionality change cannot reuse the fixed-width `vec0` tables; the
    /// job drops them up front and every row then takes the ordinary path.
    #[tokio::test]
    async fn a_dimension_change_is_handled_in_the_same_loop() {
        let (_d, storage) = deps();
        let profile = Uuid::new_v4();
        storage
            .db()
            .rag_insert(&RagDocument::new(
                profile,
                "kb.txt",
                "a chunk",
                vec![1.0; 16],
            ))
            .unwrap();
        storage.db().bump_embed_generation().unwrap();
        assert_eq!(storage.db().rag_dimension().unwrap(), Some(16));

        // The new model is 32-dimensional.
        let rx = run(
            &storage,
            Arc::new(MockEmbedder::new(32)),
            CancellationToken::new(),
        );
        match finish(rx).await {
            RagProgress::Reembedded { rows, errors, .. } => {
                assert_eq!(rows, 1);
                assert_eq!(errors, 0, "a dimension change is not an error");
            }
            other => panic!("expected Reembedded, got {other:?}"),
        }
        assert_eq!(
            storage.db().rag_dimension().unwrap(),
            Some(32),
            "the vector tables were rebuilt at the new width"
        );
        // The chunk is searchable again, under the new model.
        let q = vec![1.0; 32];
        assert_eq!(storage.db().rag_search(profile, &q, 5).unwrap().len(), 1);
    }
}
