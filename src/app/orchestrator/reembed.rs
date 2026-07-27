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
//! It is driven by the generation marker: a row whose `embed_gen` is not current
//! is work, and stamping it removes it from the queue. That makes the job
//! **resumable** — interrupting it leaves a consistent partial state, and a rerun
//! picks up exactly where it stopped. Knowledge-base search stays refused
//! meanwhile through the per-profile stale marks from stage 1, so a half-finished
//! index never answers a query.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::events::{AppEvent, RagProgress};
use crate::shared::api::Embedder;
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;
use crate::shared::storage::db::{Db, ReembedPending, ReembedRow};

use super::Orchestrator;
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

/// Outcome of draining one store. The count of rows written is not carried here
/// — it is threaded through the job-wide `done` counter, so the banner shows one
/// continuous figure across all three stores.
struct Drained {
    errors: usize,
    /// The embedder itself failed — the whole job must stop, not just this store.
    fatal: Option<String>,
}

fn spawn_reembed(task: Reembed) {
    let Reembed {
        embedder,
        storage,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Embedder precheck — also tells us the dimensionality we are moving to.
        let dim = match embedder.embed(vec!["ping".into()]).await {
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
        let total = pending.notes + pending.attachments + pending.rag;
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

        let mut done = 0usize;
        let mut errors = 0usize;
        for store in Store::ALL {
            if cancel.is_cancelled() || store.pending(&pending) == 0 {
                continue;
            }
            let drained = drain_store(
                store, &embedder, &storage, &cancel, loc, total, &mut done, &evt_tx,
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
            rows: done,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Re-embeds one store batch by batch until its queue is empty, cancelled, or
/// stuck. `done` is the job-wide counter so the banner shows one continuous
/// progress figure across all three stores.
#[allow(clippy::too_many_arguments)] // cohesive job state, threaded from one call site
async fn drain_store(
    store: Store,
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    cancel: &CancellationToken,
    loc: &'static Locale,
    total: usize,
    done: &mut usize,
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
        let embeddings = match embedder.embed(texts).await {
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

        let mut written = 0usize;
        for (row, embedding) in rows.iter().zip(embeddings) {
            match store.write(storage.db(), row, &embedding) {
                Ok(()) => {
                    written += 1;
                    *done += 1;
                }
                Err(err) => {
                    errors += 1;
                    tracing::warn!(?store, rowid = row.rowid, error = %err, "re-embed: failed to write a vector");
                }
            }
        }
        // A batch that wrote nothing leaves the queue unchanged, so the next
        // fetch returns the same rows — the loop would never end. Stop this
        // store instead; the rows stay foreign and a later run can retry them.
        if written == 0 {
            break;
        }

        let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
            index: *done,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::note::Note;
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
            cancel,
            loc: locale(Lang::Ru),
            evt_tx: tx,
        });
        rx
    }

    /// Drains the channel and returns the terminal event.
    async fn finish(mut rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> RagProgress {
        let mut last = None;
        while let Some(AppEvent::RagProgress(p)) = rx.recv().await {
            let terminal = matches!(p, RagProgress::Reembedded { .. } | RagProgress::Failed(_));
            last = Some(p);
            if terminal {
                break;
            }
        }
        last.expect("the job must report an outcome")
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
        let (Ok(url_a), Ok(url_b)) = (
            std::env::var("MINDFORK_EMBED_URL"),
            std::env::var("MINDFORK_EMBED_URL_ALT"),
        ) else {
            eprintln!("skip: MINDFORK_EMBED_URL / MINDFORK_EMBED_URL_ALT not set");
            return;
        };
        let model_a: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_a));
        let model_b: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_b));

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
            .embed(corpus.iter().map(|s| s.to_string()).collect())
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
            .embed(vec![note.content.clone()])
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
                locale(Lang::Ru),
                tx.clone(),
            )
        };
        guard(model_a, "bge-m3")
            .embed(vec!["warm up".into()])
            .await
            .unwrap();
        guard(model_b.clone(), "e5-large-instruct")
            .embed(vec!["warm up".into()])
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
        let query_vec = model_b.embed(vec![QUERY.into()]).await.unwrap().remove(0);
        let hits = storage.db().rag_search(profile, &query_vec, 3).unwrap();
        assert_eq!(
            hits.first().map(|h| h.chunk_text.as_str()),
            Some(PARIS),
            "after re-embedding, the correct chunk ranks first again: {hits:?}"
        );
        // And the note is searchable under the new model as well.
        let note_query = model_b
            .embed(vec!["how should I answer?".into()])
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
