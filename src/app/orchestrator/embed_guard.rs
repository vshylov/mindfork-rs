//! Detection of an **embedding-model change** and invalidation of the vectors it
//! orphans (stage 1 of docs/research/embedding-model-change-reindex.md).
//!
//! Stored vectors are only comparable to a query embedded by the same model.
//! Dimensionality cannot establish that — `bge-m3` and
//! `multilingual-e5-large-instruct` are both 1024-d, so a swap between them
//! passes every existing guard while turning retrieval into noise (measured:
//! the same text embedded by both scores ~0.37). The guard closes that hole
//! behaviourally, via a canary vector ([`crate::shared::embed_identity`]).
//!
//! ## Why a decorator
//!
//! Embeddings are deliberately lazy (ADR 0002): the embedder itself is untouched
//! until a real call. `apply_embed` does spawn a `/health` probe, but that only
//! reports the *server* is up — it says nothing about which model answers, which
//! is exactly what the canary establishes; and the cloud has no probe at all.
//! Wrapping [`Embedder`] instead makes the check run at the first *real* use,
//! when the server has demonstrably answered, and makes it impossible to forget
//! at a call site. A failed check never blocks the actual work — it simply
//! retries on the next call.
//!
//! ## What happens on a detected change
//!
//! **Nothing is deleted.** The guard bumps the embedding *generation*
//! (`db::embed_gen`), after which every older vector reads as foreign. One
//! counter increment retires the whole database, and each store then follows a
//! route that already exists:
//!
//! - **notes** (`note_vectors`) — foreign vectors are invisible to semantic
//!   search, and `notes_missing_vectors` lists their notes, so the existing
//!   `ensure_note_vectors` backfill re-embeds them on the next semantic path.
//!   Self-healing within one `note_recall`.
//! - **chat attachments** — `attachment_indexed_ids` ignores foreign rows, so
//!   `attachment_search` degrades to its `not_indexed` answer pointing at
//!   `attachment_read` (the guaranteed path, spec §9.7), and the rows stay
//!   available for `/reindex` to rebuild without the user re-attaching anything.
//! - **the knowledge base** — too large to heal on a read path, and it is the
//!   user's own data, so the affected profiles are additionally marked stale and
//!   `rag_search` refuses over them until `/reindex` (or `/rag rebuild`) rewrites
//!   their vectors.
//!
//! Keeping the rows is what makes the re-embed job possible at all — it works
//! from the text they already hold — and it makes switching *back* to the
//! previous model free.
//!
//! The fingerprint is recorded **after** invalidation, so an interrupted run just
//! redoes it on the next launch, and a healthy launch never re-invalidates.
//!
//! ## Calibration
//!
//! Recording a fingerprint also measures the model's **similarity range**
//! ([`crate::shared::embed_calibration`], research §8.2), on the same
//! once-per-model path. Cosine distributions differ sharply between models —
//! e5-large-instruct's usable range is 2.6× narrower than bge-m3's — so the
//! project's similarity gates are read as positions in a reference scale rather
//! than as absolute numbers. Without this, a correct re-embedding would still
//! leave every gate mistuned.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::OnceCell;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::events::AppEvent;
use crate::shared::api::{EmbedRole, Embedder};
use crate::shared::embed_calibration;
use crate::shared::embed_identity::{CANARY_TEXT, EmbedFingerprint};
use crate::shared::embed_prefix::EmbedConvention;
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;
use crate::shared::storage::db::ReembedPending;

/// Wraps the real embedder and verifies, once per instance, that the stored
/// vectors were produced by the same model. Rebuilt whenever the embedding
/// settings change (see `Orchestrator::apply_embed_settings`), so switching the
/// model in settings re-arms the check.
pub(super) struct EmbedGuard {
    inner: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    /// Display name of the model now in use (settings-derived, may be absent).
    model_id: Option<String>,
    /// The input-prefix convention in force. Recorded in the fingerprint as an
    /// exact second trigger (research docs/research/embedding-input-prefixes.md
    /// §4) and used to decide whether to hint at a better one.
    convention: EmbedConvention,
    /// Interface language (axis B) — the notice goes to the user, not the model.
    loc: &'static Locale,
    evt_tx: UnboundedSender<AppEvent>,
    /// Holds the check's outcome; only set once it has actually succeeded, so a
    /// still-loading server is retried rather than being taken for a verdict.
    checked: OnceCell<()>,
}

/// What a detected model change retired.
#[derive(Debug, Default, PartialEq)]
struct Invalidated {
    /// Vectors now awaiting re-embedding, per store. Notes rebuild themselves on
    /// the next semantic path; attachments and the knowledge base wait for
    /// `/reindex`.
    pending: ReembedPending,
    /// Profiles whose knowledge-base search is off until it is rebuilt.
    stale_profiles: usize,
}

impl Invalidated {
    /// Total vectors awaiting re-embedding — what `/reindex` would do.
    fn total(&self) -> usize {
        self.pending.notes + self.pending.attachments + self.pending.rag
    }

    /// Whether anything was actually affected. A DB with no vectors at all
    /// (a fresh install, or a user who never used RAG or memory) needs no notice
    /// — the fingerprint is simply recorded.
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl EmbedGuard {
    pub(super) fn new(
        inner: Arc<dyn Embedder>,
        storage: Arc<Storage>,
        model_id: Option<String>,
        convention: EmbedConvention,
        loc: &'static Locale,
        evt_tx: UnboundedSender<AppEvent>,
    ) -> Self {
        Self {
            inner,
            storage,
            model_id,
            convention,
            loc,
            evt_tx,
            checked: OnceCell::new(),
        }
    }

    /// Embeds the canary and compares it with what the DB recorded, acting on a
    /// mismatch. Returns `Err` when the embedder is unavailable, so the check is
    /// retried on the next call instead of being cached as a verdict.
    async fn run_check(&self) -> Result<()> {
        // Passage, deliberately: stored vectors are all passage-role, so the
        // passage marker alone defines the space the database is in. A change to
        // the *query* marker alters retrieval but leaves every stored vector
        // valid, and must not force a reindex (research
        // docs/research/embedding-input-prefixes.md §4).
        let fresh = self
            .inner
            .embed(vec![CANARY_TEXT.to_string()], EmbedRole::Passage)
            .await?
            .into_iter()
            .next()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow!("embedder returned no canary vector"))?;

        let stored = self.storage.db().embed_fingerprint().unwrap_or(None);
        let current = EmbedFingerprint::new(fresh, self.model_id.clone(), self.convention.id());

        match stored {
            // Same model and same input convention — the common path.
            Some(prev) if prev.matches(&current) => return Ok(()),
            Some(prev) => {
                let hit = self.invalidate();
                tracing::warn!(
                    previous = prev.display_id(),
                    current = current.display_id(),
                    notes = hit.pending.notes,
                    attachments = hit.pending.attachments,
                    rag = hit.pending.rag,
                    stale_profiles = hit.stale_profiles,
                    "embedding model changed: vectors from the previous model were invalidated"
                );
                self.notify(&prev, &current, &hit);
            }
            // Nothing recorded yet: a fresh DB, an installation predating this
            // check, or a `/rag rebuild` that reset the vectors. Adopt the
            // current model silently — with no prior fingerprint there is no
            // evidence anything is stale, and claiming otherwise would cry wolf
            // on every first launch.
            None => tracing::info!(
                model = current.display_id(),
                "recorded the embedding model fingerprint"
            ),
        }

        if let Err(err) = self.storage.db().set_embed_fingerprint(&current) {
            // Non-fatal: the check simply repeats on the next launch.
            tracing::warn!(error = %err, "failed to record the embedding fingerprint");
        }
        self.calibrate().await;
        Ok(())
    }

    /// Measures the model's similarity range so the project's thresholds can be
    /// read in *its* scale rather than bge-m3's (research §8.2). Runs on the same
    /// once-per-model path as the fingerprint, costing one extra request of 32
    /// short strings.
    ///
    /// Entirely best-effort: any failure leaves the previous calibration (or
    /// none) in place, and the absence of one means the thresholds are used
    /// exactly as they are today. A failed calibration can therefore only leave
    /// the gates as they were — never make them wilder.
    async fn calibrate(&self) {
        let probes = embed_calibration::probe_texts();
        // Passage: every gate this calibrates is a passage-to-passage comparison
        // (research §5.3), and going through the prefixer means a model's range
        // is always measured in the same dressing its real text gets — which is
        // what keeps the reference constants valid (§3).
        let vectors = match self.inner.embed(probes, EmbedRole::Passage).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(error = %err, "similarity calibration skipped");
                return;
            }
        };
        let Some(calibration) = embed_calibration::measure(&vectors) else {
            tracing::warn!("similarity calibration probe returned an unusable result");
            return;
        };
        match self.storage.db().set_embed_calibration(&calibration) {
            Ok(()) => tracing::info!(
                unrelated = calibration.unrelated,
                paraphrase = calibration.paraphrase,
                "calibrated the similarity scale"
            ),
            Err(err) => tracing::warn!(error = %err, "failed to record the similarity calibration"),
        }
    }

    /// Retires everything the previous model produced and records which knowledge
    /// bases are left stale. Every step is best-effort: a failure is logged and
    /// the rest still runs — a partial invalidation is strictly better than none,
    /// and the next launch retries whatever was missed.
    ///
    /// Nothing is deleted. Starting a new **generation** makes every older vector
    /// read as foreign — invisible to search and listed as work for `/reindex`
    /// (see the research doc §8.1, S3). Keeping the rows means the re-embed job
    /// has their text to work from, attachment indexes come back without the user
    /// re-attaching each file, and switching *back* to the previous model costs
    /// nothing.
    fn invalidate(&self) -> Invalidated {
        let db = self.storage.db();

        if let Err(err) = db.bump_embed_generation() {
            // Without a bump the old vectors would stay visible and be silently
            // mixed with the new model's queries — the exact failure this guard
            // exists to prevent. Say so loudly.
            tracing::error!(error = %err, "failed to retire the previous model's vectors");
        }
        let pending = db.count_rows_to_reembed().unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to count vectors awaiting re-embedding");
            Default::default()
        });
        let stale = db.profiles_with_rag_docs().unwrap_or_else(|err| {
            tracing::warn!(error = %err, "failed to list profiles with RAG documents");
            Vec::new()
        });
        if !stale.is_empty()
            && let Err(err) = db.set_rag_stale_profiles(&stale)
        {
            tracing::warn!(error = %err, "failed to record stale knowledge bases");
        }

        Invalidated {
            pending,
            stale_profiles: stale.len(),
        }
    }

    /// Tells the user what happened, in the interface language: what was retired,
    /// how to rebuild it all at once, and — only when it applies — that
    /// knowledge-base search is off until they do. Nothing is said when nothing
    /// was affected.
    fn notify(&self, prev: &EmbedFingerprint, current: &EmbedFingerprint, hit: &Invalidated) {
        if hit.is_empty() {
            return;
        }
        let mut msg = self.loc.tf(
            "ui.embed.model_changed",
            &[("old", prev.display_id()), ("new", current.display_id())],
        );
        msg.push(' ');
        msg.push_str(
            &self
                .loc
                .tf("ui.embed.reindex_hint", &[("n", &hit.total().to_string())]),
        );
        if hit.stale_profiles > 0 {
            msg.push(' ');
            msg.push_str(&self.loc.tf(
                "ui.embed.rag_stale",
                &[("n", &hit.stale_profiles.to_string())],
            ));
        }
        if let Some(suggested) = self.suggested_convention() {
            msg.push(' ');
            msg.push_str(
                &self
                    .loc
                    .tf("ui.embed.convention_hint", &[("name", suggested.id())]),
            );
        }
        let _ = self.evt_tx.send(AppEvent::Error(msg));
    }

    /// A convention the new model's name suggests but the settings do not use.
    ///
    /// Only ever a *hint* — never applied. A wrong convention is measurably
    /// harmful (it costs bge-m3 a rank and 31% of its margin, research §2.1), so
    /// the choice stays the user's; this just makes it discoverable at the one
    /// moment it is relevant. `None` when the name suggests nothing, or suggests
    /// what is already set.
    fn suggested_convention(&self) -> Option<EmbedConvention> {
        let suggested = EmbedConvention::suggested_for(self.model_id.as_deref()?)?;
        (suggested != self.convention).then_some(suggested)
    }
}

#[async_trait::async_trait]
impl Embedder for EmbedGuard {
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        // Best-effort: a failed check (server still loading, embeddings not
        // configured) must never block real work — the call below reports the
        // real error itself, and the check retries next time.
        let _ = self.checked.get_or_try_init(|| self.run_check()).await;
        self.inner.embed(texts, role).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::note::Note;
    use crate::shared::api::mock::MockEmbedder;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::paths::Paths;
    use uuid::Uuid;

    /// An embedder whose output depends on `salt`, so two instances stand for two
    /// different models at the **same** dimensionality — the case dimensionality
    /// checks cannot see, and the whole reason this guard exists.
    struct SaltedEmbedder {
        salt: usize,
        dim: usize,
    }

    #[async_trait::async_trait]
    impl Embedder for SaltedEmbedder {
        async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0; self.dim];
                    for (i, b) in t.bytes().enumerate() {
                        v[(i + b as usize + self.salt * 7) % self.dim] += 1.0;
                    }
                    v[self.salt % self.dim] += 5.0; // pull the spaces apart
                    v
                })
                .collect())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        storage: Arc<Storage>,
        rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
        tx: UnboundedSender<AppEvent>,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Fixture {
            _dir: dir,
            storage,
            rx,
            tx,
        }
    }

    fn guard(f: &Fixture, inner: Arc<dyn Embedder>, model: &str) -> EmbedGuard {
        guard_with(f, inner, model, EmbedConvention::None)
    }

    /// The guard as production builds it: wrapping the prefixer, so the canary
    /// and the calibration probes go through the convention (research §3–§4).
    fn guard_with(
        f: &Fixture,
        inner: Arc<dyn Embedder>,
        model: &str,
        convention: EmbedConvention,
    ) -> EmbedGuard {
        EmbedGuard::new(
            Arc::new(crate::shared::embed_prefix::PrefixedEmbedder::new(
                inner, convention,
            )),
            f.storage.clone(),
            Some(model.to_string()),
            convention,
            locale(Lang::Ru),
            f.tx.clone(),
        )
    }

    // ---------- the prefixer sits inside the guard (research §3–§4) ----------

    /// Records what actually reached the real embedder.
    #[derive(Default)]
    struct Recorder {
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Embedder for Recorder {
        async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
            self.seen.lock().unwrap().extend(texts.iter().cloned());
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[tokio::test]
    async fn the_canary_and_the_calibration_go_through_the_prefixer() {
        // The ordering property both traps rest on. If the guard wrapped the raw
        // embedder instead, the canary would be blind to a convention switch and
        // the calibration would be measured in a dressing the real text never
        // wears — silently invalidating the reference constants (research §3).
        let f = fixture();
        let rec = Arc::new(Recorder::default());
        guard_with(&f, rec.clone(), "e5", EmbedConvention::E5)
            .embed(vec!["настоящий текст".into()], EmbedRole::Passage)
            .await
            .unwrap();

        let seen = rec.seen.lock().unwrap();
        assert!(
            seen.iter().any(|t| t == &format!("passage: {CANARY_TEXT}")),
            "the canary must carry the passage marker: {seen:?}"
        );
        assert!(
            seen.iter().filter(|t| t.starts_with("passage: ")).count() > 30,
            "the 32 calibration probes are prefixed too: {}",
            seen.len()
        );
        assert!(
            seen.iter().any(|t| t == "passage: настоящий текст"),
            "and so is the real call: {seen:?}"
        );
    }

    #[tokio::test]
    async fn turning_a_convention_on_reads_as_a_changed_vector_space() {
        // Enabling prefixes genuinely re-embeds everything into a different
        // space, so it must invalidate exactly like a model swap — otherwise the
        // user would keep querying old vectors with newly-dressed queries.
        let mut f = fixture();
        let profile = Uuid::new_v4();
        let note = Note::new(profile, "заметка", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0, 0.0])
            .unwrap();
        let embedder = Arc::new(MockEmbedder::new(16));

        guard_with(&f, embedder.clone(), "e5", EmbedConvention::None)
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let gen_before = f.storage.db().embed_generation().unwrap();
        while f.rx.try_recv().is_ok() {}

        // Same model, same everything — only the convention changes.
        guard_with(&f, embedder, "e5", EmbedConvention::E5)
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert!(
            f.storage.db().embed_generation().unwrap() > gen_before,
            "the generation must be bumped, retiring the old vectors"
        );
        assert!(
            f.rx.try_recv().is_ok(),
            "and the user must be told, as for any model change"
        );
        // The note itself survives — nothing is ever deleted (research §8.1 S3).
        assert!(f.storage.db().note_get(profile, note.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn the_model_change_notice_hints_at_a_matching_convention() {
        // A hint, never an action: a wrong convention is measurably harmful, so
        // the choice stays the user's (research §7 R3).
        let mut f = fixture();
        let profile = Uuid::new_v4();
        let note = Note::new(profile, "заметка", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0, 0.0])
            .unwrap();

        guard(&f, Arc::new(SaltedEmbedder { salt: 0, dim: 16 }), "bge-m3")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();
        while f.rx.try_recv().is_ok() {}

        guard(
            &f,
            Arc::new(SaltedEmbedder { salt: 3, dim: 16 }),
            "multilingual-e5-large-instruct-q8_0.gguf",
        )
        .embed(vec!["x".into()], EmbedRole::Passage)
        .await
        .unwrap();

        let AppEvent::Error(msg) = f.rx.try_recv().expect("a notice") else {
            panic!("expected an error event");
        };
        assert!(msg.contains("e5-instruct"), "{msg}");
    }

    #[tokio::test]
    async fn no_hint_when_the_convention_already_matches() {
        let mut f = fixture();
        let profile = Uuid::new_v4();
        let note = Note::new(profile, "заметка", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0, 0.0])
            .unwrap();

        guard_with(
            &f,
            Arc::new(SaltedEmbedder { salt: 0, dim: 16 }),
            "bge-m3",
            EmbedConvention::E5Instruct,
        )
        .embed(vec!["x".into()], EmbedRole::Passage)
        .await
        .unwrap();
        while f.rx.try_recv().is_ok() {}

        guard_with(
            &f,
            Arc::new(SaltedEmbedder { salt: 3, dim: 16 }),
            "multilingual-e5-large-instruct-q8_0.gguf",
            EmbedConvention::E5Instruct,
        )
        .embed(vec!["x".into()], EmbedRole::Passage)
        .await
        .unwrap();

        let AppEvent::Error(msg) = f.rx.try_recv().expect("a notice") else {
            panic!("expected an error event");
        };
        assert!(
            !msg.contains("e5-instruct"),
            "nothing to suggest when it is already set: {msg}"
        );
    }

    #[tokio::test]
    async fn first_run_records_fingerprint_without_notifying() {
        let mut f = fixture();
        let g = guard(&f, Arc::new(MockEmbedder::new(16)), "model-a");
        g.embed(vec!["hello".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert!(
            f.storage.db().embed_fingerprint().unwrap().is_some(),
            "the fingerprint is recorded on the first use"
        );
        assert!(
            f.rx.try_recv().is_err(),
            "a first launch must not claim anything changed"
        );
    }

    #[tokio::test]
    async fn same_model_is_silent_and_keeps_vectors() {
        let mut f = fixture();
        let profile = Uuid::new_v4();
        let note = Note::new(profile, "a note", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0, 0.0])
            .unwrap();

        // Two separate guards over the same model — as if the app were restarted.
        guard(&f, Arc::new(MockEmbedder::new(16)), "model-a")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let _ = f.rx.try_recv();
        guard(&f, Arc::new(MockEmbedder::new(16)), "model-a")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert!(f.rx.try_recv().is_err(), "no notice for an unchanged model");
        assert!(
            f.storage
                .db()
                .notes_missing_vectors(profile)
                .unwrap()
                .is_empty(),
            "an unchanged model must not invalidate anything"
        );
    }

    #[tokio::test]
    async fn same_dimension_model_swap_is_detected_and_invalidates() {
        let mut f = fixture();
        let profile = Uuid::new_v4();
        let note = Note::new(profile, "a note", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &[1.0, 0.0])
            .unwrap();

        // Model A records the fingerprint.
        guard(&f, Arc::new(SaltedEmbedder { salt: 0, dim: 32 }), "model-a")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let _ = f.rx.try_recv();

        // Model B — same dimensionality, different vector space.
        guard(&f, Arc::new(SaltedEmbedder { salt: 3, dim: 32 }), "model-b")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();

        // The note survives, its vector does not → the backfill will re-embed it.
        assert_eq!(
            f.storage.db().notes_missing_vectors(profile).unwrap().len(),
            1,
            "note vectors read as foreign, so the existing backfill re-embeds them"
        );
        assert_eq!(
            f.storage
                .db()
                .note_list(profile, None, &[], None)
                .unwrap()
                .len(),
            1,
            "the note content itself is untouched"
        );
        match f.rx.try_recv() {
            Ok(AppEvent::Error(msg)) => {
                assert!(msg.contains("model-a") && msg.contains("model-b"), "{msg}");
            }
            other => panic!("expected a model-change notice, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rag_documents_are_marked_stale_not_deleted() {
        let f = fixture();
        let profile = Uuid::new_v4();
        f.storage
            .db()
            .rag_insert(&crate::entities::rag::RagDocument::new(
                profile,
                "kb.txt",
                "chunk",
                vec![1.0; 32],
            ))
            .unwrap();

        guard(&f, Arc::new(SaltedEmbedder { salt: 0, dim: 32 }), "model-a")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();
        guard(&f, Arc::new(SaltedEmbedder { salt: 3, dim: 32 }), "model-b")
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert!(
            f.storage.db().rag_is_stale(profile).unwrap(),
            "the knowledge base is marked stale for the affected profile"
        );
        assert_eq!(
            f.storage.db().rag_count(profile).unwrap(),
            1,
            "the user's knowledge base is never deleted — only marked"
        );
    }

    #[tokio::test]
    async fn detection_happens_once_per_instance() {
        let f = fixture();
        let g = guard(&f, Arc::new(MockEmbedder::new(16)), "model-a");
        g.embed(vec!["a".into()], EmbedRole::Passage).await.unwrap();
        let first = f.storage.db().embed_fingerprint().unwrap();
        // Corrupt the record; a second call on the same instance must not re-check
        // (and therefore must not rewrite it).
        f.storage
            .db()
            .set_embed_fingerprint(&EmbedFingerprint::new(
                vec![9.0; 4],
                Some("junk".into()),
                EmbedConvention::None.id(),
            ))
            .unwrap();
        g.embed(vec!["b".into()], EmbedRole::Passage).await.unwrap();
        assert_ne!(
            f.storage.db().embed_fingerprint().unwrap(),
            first,
            "the check is cached per instance, so the record stays as we left it"
        );
    }

    /// The case the whole feature exists for, against real models: `bge-m3` and
    /// `multilingual-e5-large-instruct` are **both 1024-d**, so no dimensionality
    /// check can tell them apart, yet the same text embedded by both scores only
    /// ~0.37 (docs/research/embedding-model-change-reindex.md §1). Needs two
    /// embedding servers:
    ///
    /// ```text
    /// llama-server -m bge-m3-Q8_0.gguf --port 8001 --embeddings
    /// llama-server -m multilingual-e5-large-instruct-q8_0.gguf --port 8002 --embeddings
    /// MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1
    /// MINDFORK_EMBED_URL_ALT=http://127.0.0.1:8002/v1
    /// ```
    #[tokio::test]
    #[ignore = "requires two live embedding servers (MINDFORK_EMBED_URL, MINDFORK_EMBED_URL_ALT)"]
    async fn same_dimension_model_swap_detected_live() {
        let (Ok(url_a), Ok(url_b)) = (
            std::env::var("MINDFORK_EMBED_URL"),
            std::env::var("MINDFORK_EMBED_URL_ALT"),
        ) else {
            eprintln!("skip: MINDFORK_EMBED_URL / MINDFORK_EMBED_URL_ALT not set");
            return;
        };
        let model_a: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_a));
        let model_b: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_b));

        let mut f = fixture();
        let profile = Uuid::new_v4();

        // Index a note and a knowledge-base chunk under model A.
        let note = Note::new(profile, "the user prefers concise answers", vec![]);
        f.storage.db().note_insert(&note).unwrap();
        let vec_a = model_a
            .embed(vec![note.content.clone()], EmbedRole::Passage)
            .await
            .unwrap()
            .remove(0);
        let dim_a = vec_a.len();
        f.storage
            .db()
            .note_vector_upsert(note.id, profile, &vec_a)
            .unwrap();
        f.storage
            .db()
            .rag_insert(&crate::entities::rag::RagDocument::new(
                profile, "kb.txt", "a chunk", vec_a,
            ))
            .unwrap();

        guard(&f, model_a.clone(), "bge-m3")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();
        assert!(f.storage.db().embed_fingerprint().unwrap().is_some());
        assert!(f.rx.try_recv().is_err(), "a first launch claims nothing");

        // A second launch on the SAME server must stay silent. This is the more
        // important half: a false positive here would wipe the note vectors and
        // nag the user on every single launch. Real servers are not obliged to be
        // bit-exact, so this has to be checked against one rather than a mock.
        guard(&f, model_a, "bge-m3")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();
        assert!(
            f.rx.try_recv().is_err(),
            "the same live model must not read as a change"
        );
        assert!(
            f.storage
                .db()
                .notes_missing_vectors(profile)
                .unwrap()
                .is_empty(),
            "the same live model must not invalidate anything"
        );

        // Swap the model. Same dimensionality — the point of the test.
        let dim_b = model_b
            .embed(vec!["x".into()], EmbedRole::Passage)
            .await
            .unwrap()[0]
            .len();
        assert_eq!(
            dim_a, dim_b,
            "this smoke is only meaningful for two models of the SAME dimensionality \
             (that is the case no existing guard can catch)"
        );
        guard(&f, model_b, "multilingual-e5-large-instruct")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();

        assert_eq!(
            f.storage.db().notes_missing_vectors(profile).unwrap().len(),
            1,
            "the note's old-model vector must read as foreign so the backfill re-embeds it"
        );
        assert!(
            f.storage.db().rag_is_stale(profile).unwrap(),
            "the knowledge base must be marked stale"
        );
        assert_eq!(
            f.storage.db().rag_count(profile).unwrap(),
            1,
            "the knowledge base itself must be left intact"
        );
        assert!(
            matches!(f.rx.try_recv(), Ok(AppEvent::Error(_))),
            "the user must be told"
        );
    }

    /// The point of stage 3, on real models: the project's gates are tuned to
    /// bge-m3, and e5-large-instruct's cosine range is 2.6× narrower, so the raw
    /// constants land in the wrong place inside it (research §8.2). Asserts both
    /// that bge-m3 keeps its numbers and that e5 gets corrected ones — and, the
    /// part that actually matters, that an **unrelated** pair which the raw
    /// constant would have accepted is correctly rejected by the calibrated one.
    #[tokio::test]
    #[ignore = "requires two live embedding servers (MINDFORK_EMBED_URL, MINDFORK_EMBED_URL_ALT)"]
    async fn similarity_scale_follows_the_model_live() {
        use crate::shared::embed_calibration::{REFERENCE_PARAPHRASE, REFERENCE_UNRELATED};

        let (Ok(url_a), Ok(url_b)) = (
            std::env::var("MINDFORK_EMBED_URL"),
            std::env::var("MINDFORK_EMBED_URL_ALT"),
        ) else {
            eprintln!("skip: MINDFORK_EMBED_URL / MINDFORK_EMBED_URL_ALT not set");
            return;
        };
        let bge: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_a));
        let e5: Arc<dyn Embedder> = Arc::new(crate::shared::api::OpenAiClient::new(url_b));

        // bge-m3 is the model the reference constants were measured on, so its
        // calibration must come out as (near) identity — existing installations
        // must not silently shift.
        let f_bge = fixture();
        guard(&f_bge, bge.clone(), "bge-m3")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let c = f_bge
            .storage
            .db()
            .embed_calibration()
            .unwrap()
            .expect("bge-m3 must calibrate");
        eprintln!("bge-m3 calibration: {c:?}");
        assert!(
            (c.unrelated - REFERENCE_UNRELATED).abs() < 0.05
                && (c.paraphrase - REFERENCE_PARAPHRASE).abs() < 0.05,
            "the reference constants must still describe bge-m3: {c:?}"
        );
        let scale_bge = f_bge.storage.db().similarity_scale();
        for t in [0.85, 0.72, 0.62] {
            assert!(
                (scale_bge.map(t) - t).abs() < 0.05,
                "bge-m3 must keep its thresholds: {t} -> {}",
                scale_bge.map(t)
            );
        }

        // e5 has a much narrower range, so its thresholds must move up.
        let f_e5 = fixture();
        guard(&f_e5, e5.clone(), "e5-large-instruct")
            .embed(vec!["warm up".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let scale_e5 = f_e5.storage.db().similarity_scale();
        eprintln!(
            "e5 calibration: {:?}; 0.72 -> {:.4}, 0.85 -> {:.4}",
            f_e5.storage.db().embed_calibration().unwrap(),
            scale_e5.map(0.72),
            scale_e5.map(0.85)
        );
        assert!(
            scale_e5.map(0.72) > 0.85,
            "e5's trait gate must move well above the raw constant: {}",
            scale_e5.map(0.72)
        );

        // The behavioural payoff: a pair with nothing in common. Under e5 it
        // scores above the raw 0.72 — which is exactly how a correct re-embedding
        // would still have left the trait gate firing on everything — and below
        // the calibrated threshold.
        let unrelated = e5
            .embed(
                vec![
                    "the user values brevity in answers".into(),
                    "the train leaves from platform nine".into(),
                ],
                EmbedRole::Passage,
            )
            .await
            .unwrap();
        let s = cosine_for_test(&unrelated[0], &unrelated[1]);
        eprintln!("e5 unrelated pair scores {s:.4}");
        assert!(
            s >= 0.72,
            "this smoke is only meaningful while the raw constant misfires on e5 \
             (measured 0.75); got {s:.4}"
        );
        assert!(
            s < scale_e5.map(0.72),
            "the calibrated trait gate must reject an unrelated pair: {s:.4} vs {:.4}",
            scale_e5.map(0.72)
        );
    }

    /// Cosine for the smoke above (the production ones are private to their own
    /// modules, and this file is not one of them).
    fn cosine_for_test(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    #[tokio::test]
    async fn unavailable_embedder_does_not_record_anything() {
        let f = fixture();
        let g = guard(
            &f,
            Arc::new(crate::shared::api::UnavailableEmbedder),
            "model-a",
        );
        assert!(
            g.embed(vec!["x".into()], EmbedRole::Passage).await.is_err(),
            "the real error surfaces"
        );
        assert!(
            f.storage.db().embed_fingerprint().unwrap().is_none(),
            "a failed check must not be cached as a verdict"
        );
    }
}
