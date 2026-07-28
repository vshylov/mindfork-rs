//! Per-model input prefixes for embeddings (research
//! docs/research/embedding-input-prefixes.md).
//!
//! Some embedding families expect their input to be marked with its role. The e5
//! family is the common one, and it has **two** conventions, not one: base e5
//! wants `query: ` / `passage: `, while the `-instruct` variant wants an
//! instruction-shaped query and a **bare** passage. bge-m3 — the model this
//! project is calibrated against — wants no marker at all.
//!
//! Measured on the live stand (2026-07-27), 40 documents / 14 queries:
//!
//! | model | convention | top-1 | MRR | mean margin | min margin |
//! |---|---|---|---|---|---|
//! | bge-m3 | none | 11/14 | 0.881 | 0.1480 | 0.0072 |
//! | bge-m3 | e5-instruct | **10/14** | 0.814 | 0.1027 | 0.0156 |
//! | e5-instruct | none | 12/14 | 0.898 | 0.0400 | 0.0002 |
//! | e5-instruct | e5-instruct | 12/14 | 0.899 | **0.0458** | **0.0056** |
//!
//! Two things follow, and they shape the whole design. The gain is **real but
//! narrow** — on e5 the prefixes changed no ranking at all, only the separation
//! (the smallest margin improves 25×, from an arbitrary 0.0002 tie to 0.0056).
//! And the *wrong* convention is actively harmful — it costs bge-m3 a rank and
//! 31% of its margin. Hence [`EmbedConvention::None`] is the default and the
//! choice is never made silently.
//!
//! ## Why a decorator, and why it sits inside the guard
//!
//! Applying prefixes at the call sites would mean 20 chances to forget, silently.
//! Applying them inside `OpenAiClient` would mean tests and mocks never exercise
//! the real path. So it is a decorator, installed in `EngineManager::apply_embed`
//! — and **wrapped by `EmbedGuard`**, not the other way round:
//!
//! ```text
//! EmbedGuard { inner: PrefixedEmbedder { inner: OpenAiClient } }
//! ```
//!
//! That order is load-bearing, not stylistic:
//!
//! - the guard's **calibration** probes then go through the prefixer, so a
//!   model's similarity range is always measured in the same dressing its real
//!   text gets. Without this the reference constants would silently go wrong: on
//!   bge-m3 a `passage: ` prefix moves the unrelated mean +0.097 and narrows the
//!   span 16% (research §3);
//! - the guard's **canary** likewise, so changing the convention reads as a
//!   change of vector space — which it is — and bumps the embedding generation,
//!   exactly like swapping the model (research §4). Measured: a prefixed canary
//!   scores 0.78–0.9965 against a bare one, all below the 0.999 detector.
//!
//! The canary carries [`EmbedRole::Passage`] deliberately. Stored vectors are all
//! passage-role, so the *passage* prefix alone defines the space the database is
//! in: changing the **query** prefix alters retrieval but leaves every stored
//! vector valid, and must therefore not force a reindex. Tracking the passage
//! role gets that granularity right for free.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::shared::api::{EmbedRole, Embedder};

/// The query marker for `-instruct` models. The task description is free text by
/// design; this one is deliberately generic, since the same embedder serves
/// knowledge-base search, note recall and attachment search.
///
/// It is part of the embedded text, so editing it changes the vector space for
/// queries — a constant so that shows up in review.
const INSTRUCT_QUERY_PREFIX: &str =
    "Instruct: Given a query, retrieve passages that answer it\nQuery: ";

/// How the active embedding model expects its input to be marked.
///
/// Serialized in `settings.json` under `embed.convention`; adding a family is a
/// new variant plus a row in [`EmbedConvention::prefix`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbedConvention {
    /// No marker (bge-m3, and every model that does not document one).
    ///
    /// The default, and the only value for which this whole module is a no-op —
    /// which is what keeps existing installations bit-identical (research §7 R6).
    #[default]
    None,
    /// Base e5 (`multilingual-e5-large`, `e5-base`, …): `query: ` / `passage: `.
    E5,
    /// The `-instruct` variants: an instruction-shaped query, and a **bare**
    /// passage. Not the same as [`E5`] — using base e5's markers on an
    /// `-instruct` model measures worse than its own convention.
    ///
    /// [`E5`]: EmbedConvention::E5
    E5Instruct,
}

impl EmbedConvention {
    /// Every variant, in the order the settings screen cycles them.
    pub const ALL: [Self; 3] = [Self::None, Self::E5, Self::E5Instruct];

    /// Stable identifier, used as the settings label and as the fingerprint's
    /// convention component (see `EmbedFingerprint`).
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::E5 => "e5",
            Self::E5Instruct => "e5-instruct",
        }
    }

    /// The marker to put in front of a text in this role. Empty means "bare".
    pub fn prefix(self, role: EmbedRole) -> &'static str {
        match (self, role) {
            (Self::None, _) => "",
            (Self::E5, EmbedRole::Query) => "query: ",
            (Self::E5, EmbedRole::Passage) => "passage: ",
            // The instruct variant marks only the query side.
            (Self::E5Instruct, EmbedRole::Query) => INSTRUCT_QUERY_PREFIX,
            (Self::E5Instruct, EmbedRole::Passage) => "",
        }
    }

    /// Next value in the cycle (`dir` is +1/-1), for the settings screen.
    pub fn cycle(self, dir: i32) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(i + dir).rem_euclid(n) as usize]
    }

    /// The convention a model *name* suggests, if any — used only for a passive
    /// hint when the embedding model changes, never to select one (research §7
    /// R3: a wrong convention is measurably harmful, so it is never applied
    /// without the user asking).
    ///
    /// Matches on the name as reported by settings, which for a managed server is
    /// the GGUF path.
    pub fn suggested_for(model_id: &str) -> Option<Self> {
        let name = model_id.to_ascii_lowercase();
        if !name.contains("e5") {
            return None;
        }
        Some(if name.contains("instruct") {
            Self::E5Instruct
        } else {
            Self::E5
        })
    }
}

/// Applies the active convention's marker to every text before handing it to the
/// real embedder.
///
/// Installed inside [`crate::app::orchestrator`]'s `EmbedGuard`; see the module
/// docs for why that order matters.
pub struct PrefixedEmbedder {
    inner: Arc<dyn Embedder>,
    convention: EmbedConvention,
}

impl PrefixedEmbedder {
    pub fn new(inner: Arc<dyn Embedder>, convention: EmbedConvention) -> Self {
        Self { inner, convention }
    }
}

#[async_trait::async_trait]
impl Embedder for PrefixedEmbedder {
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        let prefix = self.convention.prefix(role);
        // The default convention must cost nothing at all — not a reallocation,
        // and above all not a changed string: this is the state every existing
        // installation is in.
        if prefix.is_empty() {
            return self.inner.embed(texts, role).await;
        }
        let marked = texts.into_iter().map(|t| format!("{prefix}{t}")).collect();
        self.inner.embed(marked, role).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records exactly what reached the real embedder — the only way a silent
    /// prefix/role error is observable.
    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<(String, EmbedRole)>>,
    }

    #[async_trait::async_trait]
    impl Embedder for Recorder {
        async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
            let mut seen = self.seen.lock().unwrap();
            for t in &texts {
                seen.push((t.clone(), role));
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    async fn sent(convention: EmbedConvention, role: EmbedRole) -> Vec<String> {
        let rec = Arc::new(Recorder::default());
        let e = PrefixedEmbedder::new(rec.clone(), convention);
        e.embed(vec!["сколько стоит?".into(), "how much?".into()], role)
            .await
            .unwrap();
        let seen = rec.seen.lock().unwrap();
        seen.iter().map(|(t, _)| t.clone()).collect()
    }

    #[tokio::test]
    async fn none_passes_text_through_byte_for_byte() {
        // The property that makes this feature free for existing installations:
        // the default convention must not alter a single byte.
        for role in [EmbedRole::Query, EmbedRole::Passage] {
            assert_eq!(
                sent(EmbedConvention::None, role).await,
                ["сколько стоит?", "how much?"],
                "{role:?}"
            );
        }
    }

    #[tokio::test]
    async fn e5_marks_both_sides_differently() {
        assert_eq!(
            sent(EmbedConvention::E5, EmbedRole::Query).await[0],
            "query: сколько стоит?"
        );
        assert_eq!(
            sent(EmbedConvention::E5, EmbedRole::Passage).await[0],
            "passage: сколько стоит?"
        );
    }

    #[tokio::test]
    async fn e5_instruct_marks_only_the_query() {
        // The distinction the research turned up: the -instruct variant wants a
        // bare passage, so treating it like base e5 measures worse.
        let q = sent(EmbedConvention::E5Instruct, EmbedRole::Query).await;
        assert!(q[0].starts_with("Instruct: "), "{q:?}");
        assert!(q[0].ends_with("\nQuery: сколько стоит?"), "{q:?}");
        assert_eq!(
            sent(EmbedConvention::E5Instruct, EmbedRole::Passage).await,
            ["сколько стоит?", "how much?"],
            "the instruct variant's passages are bare"
        );
    }

    #[tokio::test]
    async fn the_role_reaches_the_inner_embedder_unchanged() {
        // The decorator marks the text; it must not rewrite the role, or the
        // guard beneath it would fingerprint the wrong space.
        let rec = Arc::new(Recorder::default());
        let e = PrefixedEmbedder::new(rec.clone(), EmbedConvention::E5);
        e.embed(vec!["x".into()], EmbedRole::Query).await.unwrap();
        e.embed(vec!["y".into()], EmbedRole::Passage).await.unwrap();
        let seen = rec.seen.lock().unwrap();
        assert_eq!(seen[0].1, EmbedRole::Query);
        assert_eq!(seen[1].1, EmbedRole::Passage);
    }

    #[test]
    fn every_convention_has_a_stable_id_and_all_are_listed() {
        // `id` is persisted in the fingerprint, so a careless rename would read
        // as a convention change for every installation using it.
        assert_eq!(EmbedConvention::None.id(), "none");
        assert_eq!(EmbedConvention::E5.id(), "e5");
        assert_eq!(EmbedConvention::E5Instruct.id(), "e5-instruct");
        assert_eq!(EmbedConvention::ALL.len(), 3);
        assert_eq!(EmbedConvention::default(), EmbedConvention::None);
    }

    #[test]
    fn only_none_is_a_no_op() {
        // Pins the invariant the fingerprint relies on: any non-default
        // convention changes at least one side's text, so switching to it is
        // detectable as a change of vector space.
        for c in EmbedConvention::ALL {
            let touches = [EmbedRole::Query, EmbedRole::Passage]
                .iter()
                .any(|r| !c.prefix(*r).is_empty());
            assert_eq!(touches, c != EmbedConvention::None, "{c:?}");
        }
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        assert_eq!(EmbedConvention::None.cycle(1), EmbedConvention::E5);
        assert_eq!(EmbedConvention::E5Instruct.cycle(1), EmbedConvention::None);
        assert_eq!(EmbedConvention::None.cycle(-1), EmbedConvention::E5Instruct);
    }

    #[test]
    fn suggestion_recognises_the_e5_family_and_nothing_else() {
        // Feeds a hint only. Matched against the name settings report, which for
        // a managed server is the GGUF path.
        assert_eq!(
            EmbedConvention::suggested_for(r"D:\LLM\GGUF\multilingual-e5-large-instruct-q8_0.gguf"),
            Some(EmbedConvention::E5Instruct)
        );
        assert_eq!(
            EmbedConvention::suggested_for("intfloat/multilingual-e5-large"),
            Some(EmbedConvention::E5)
        );
        assert_eq!(EmbedConvention::suggested_for("bge-m3-Q8_0.gguf"), None);
        assert_eq!(EmbedConvention::suggested_for(""), None);
    }

    #[test]
    fn the_instruct_query_marker_has_the_documented_shape() {
        // `Instruct: <task>\nQuery: ` is the form the -instruct family documents;
        // getting it wrong measures no better than no prefix at all.
        let p = EmbedConvention::E5Instruct.prefix(EmbedRole::Query);
        assert_eq!(p, INSTRUCT_QUERY_PREFIX);
        assert!(
            p.starts_with("Instruct: ") && p.ends_with("\nQuery: "),
            "{p}"
        );
    }

    // ---------- live ----------

    /// The claim the whole feature rests on, against real models: e5's own
    /// convention widens the separation between a relevant and an irrelevant
    /// passage, and bge-m3's does not (it is a model that wants bare text).
    ///
    /// Deliberately asserts on the **margin**, not on top-1: the research
    /// measured that prefixes change no ranking on a 40-document corpus (§2.1),
    /// so a test asserting a recovered rank would be asserting something that
    /// was never true.
    ///
    /// ```text
    /// MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1       # bge-m3
    /// MINDFORK_EMBED_URL_ALT=http://127.0.0.1:8002/v1   # multilingual-e5-large-instruct
    /// ```
    #[tokio::test]
    #[ignore = "requires two live embedding servers (MINDFORK_EMBED_URL, MINDFORK_EMBED_URL_ALT)"]
    async fn conventions_behave_as_measured_live() {
        const BGE: (&str, &str) = ("MINDFORK_EMBED_URL", "MINDFORK_EMBED_KEY");
        const E5: (&str, &str) = ("MINDFORK_EMBED_URL_ALT", "MINDFORK_EMBED_KEY_ALT");
        if crate::shared::api::live_client(BGE.0, BGE.1).is_none()
            || crate::shared::api::live_client(E5.0, E5.1).is_none()
        {
            eprintln!("skip: MINDFORK_EMBED_URL / MINDFORK_EMBED_URL_ALT not set");
            return;
        }

        const QUERY: &str = "какой внутренний код сборки проекта?";
        const RELEVANT: &str =
            "Внутренний код сборки проекта — ZARYA-7719, он указывается в отчётах о релизе.";
        const IRRELEVANT: &str = "Чугунной сковороде нужна прокалка перед первым использованием.";

        /// Cosine gap between the relevant and the irrelevant passage.
        async fn margin(server: (&str, &str), c: EmbedConvention) -> f32 {
            let raw: Arc<dyn Embedder> =
                Arc::new(crate::shared::api::live_client(server.0, server.1).unwrap());
            let e = PrefixedEmbedder::new(raw, c);
            let q = e
                .embed(vec![QUERY.into()], EmbedRole::Query)
                .await
                .unwrap()
                .remove(0);
            let docs = e
                .embed(vec![RELEVANT.into(), IRRELEVANT.into()], EmbedRole::Passage)
                .await
                .unwrap();
            cos(&q, &docs[0]) - cos(&q, &docs[1])
        }

        fn cos(a: &[f32], b: &[f32]) -> f32 {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            dot / (na * nb)
        }

        let e5_none = margin(E5, EmbedConvention::None).await;
        let e5_own = margin(E5, EmbedConvention::E5Instruct).await;
        let bge_none = margin(BGE, EmbedConvention::None).await;
        let bge_wrong = margin(BGE, EmbedConvention::E5Instruct).await;
        eprintln!(
            "e5: none={e5_none:.4} own={e5_own:.4} | bge: none={bge_none:.4} wrong={bge_wrong:.4}"
        );

        assert!(
            e5_own > e5_none,
            "e5's own convention must separate better: {e5_none:.4} -> {e5_own:.4}"
        );
        assert!(
            bge_none > bge_wrong,
            "and the wrong convention must hurt bge-m3, which is why `none` is the \
             default and nothing is auto-applied: {bge_none:.4} -> {bge_wrong:.4}"
        );
    }
}
