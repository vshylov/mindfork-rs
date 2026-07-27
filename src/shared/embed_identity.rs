//! Identity of the embedding model that produced the stored vectors.
//!
//! Stored vectors are only comparable to a query embedded by the **same** model.
//! Dimensionality alone cannot establish that: `bge-m3` and
//! `multilingual-e5-large-instruct` are both 1024-d, yet embedding the same text
//! with both gives a cosine of only ~0.37 — different vector spaces that every
//! dimension-based guard waves through (see
//! docs/research/embedding-model-change-reindex.md §1).
//!
//! So identity is established **behaviourally**: we embed a fixed canary string,
//! store the vector, and compare on the next run. Measured on a live pair of
//! `llama-server` instances (2026-07-27):
//!
//! | comparison | cosine |
//! |---|---|
//! | same model, repeat call | 1.000000 |
//! | same model, alone vs. inside a batch of 4 | 1.000000 |
//! | bge-m3 vs. multilingual-e5-large-instruct | 0.368940 |
//!
//! A separation margin of 0.63 makes [`CANARY_MATCH`] uncontroversial. Unlike a
//! config fingerprint, this also catches what config cannot see: the same GGUF
//! path re-pointed at another file, a requantization, or a server restarted with
//! different pooling/normalization flags.

use serde::{Deserialize, Serialize};

use crate::shared::embed_prefix::EmbedConvention;

/// The canary text. **Never change this** without bumping the suffix: a new
/// string produces a different vector, which would read as "the model changed"
/// for every existing installation. Deliberately short (one embed call, cheap)
/// and mixed-script, so a model that treats scripts differently still moves.
pub const CANARY_TEXT: &str = "mindfork embedding canary v1";

/// Cosine at or above which two canary vectors are the same model. The measured
/// gap is enormous (1.000000 vs 0.368940), so this only has to be below the
/// numeric noise of a single provider — cloud providers are not bit-exact the
/// way a local `llama-server` is.
pub const CANARY_MATCH: f32 = 0.999;

/// The embedding model identity recorded alongside the stored vectors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedFingerprint {
    /// Embedding of [`CANARY_TEXT`] — the actual identity signal.
    pub canary: Vec<f32>,
    /// Human-readable model name for the message shown to the user (from the
    /// settings, [`crate::shared::config::EmbedSettings::active_model_name`]).
    /// Display metadata only — never the trigger: a generic id (`"gpt"`) or an
    /// unchanged name after a file swap makes it unreliable on its own.
    pub model_id: Option<String>,
    /// Id of the input-prefix convention the stored vectors were produced under
    /// ([`crate::shared::embed_prefix::EmbedConvention::id`]).
    ///
    /// Unlike `model_id` this **is** a trigger, and it is the one exception to
    /// "identity is behavioural". Changing the convention genuinely changes the
    /// vector space, and the canary does detect it — but by as little as 0.0025
    /// on a model whose passage marker barely moves the vector (research
    /// docs/research/embedding-input-prefixes.md §4). A config component makes
    /// that detection exact. It can only *add* detections, never mask one, which
    /// is what separates it from a config-only fingerprint (rejected as D2 in
    /// docs/research/embedding-model-change-reindex.md §4).
    ///
    /// `None` on a fingerprint written before this existed — treated as the
    /// default convention, which is what those installations were using.
    #[serde(default)]
    pub convention: Option<String>,
}

impl EmbedFingerprint {
    pub fn new(canary: Vec<f32>, model_id: Option<String>, convention: &str) -> Self {
        Self {
            canary,
            model_id,
            convention: Some(convention.to_string()),
        }
    }

    /// Whether `fresh` was produced the same way as this fingerprint: the same
    /// model **and** the same input convention.
    ///
    /// A different dimensionality is decisive on its own; otherwise the canary
    /// cosine decides, with the convention as an exact second trigger. A stored
    /// `None` convention means "recorded before conventions existed", i.e. the
    /// default — so an installation that never changes it sees no difference.
    pub fn matches(&self, fresh: &EmbedFingerprint) -> bool {
        let default = EmbedConvention::default().id();
        let stored = self.convention.as_deref().unwrap_or(default);
        let current = fresh.convention.as_deref().unwrap_or(default);
        stored == current
            && self.canary.len() == fresh.canary.len()
            && cosine(&self.canary, &fresh.canary) >= CANARY_MATCH
    }

    /// Model name for display, or a placeholder when settings carry none (an
    /// external server that reports nothing useful).
    pub fn display_id(&self) -> &str {
        self.model_id.as_deref().unwrap_or("?")
    }
}

/// Cosine similarity (0.0 on a length mismatch or a zero norm). A local copy
/// rather than a shared helper: the three existing ones are private to their
/// modules (`db`, `features::tools::notes`, `features::tools::web`), and
/// `shared` cannot import from `features` (FSD).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fingerprint under the default convention — the state of every
    /// installation that never touches the setting.
    fn fp(v: &[f32]) -> EmbedFingerprint {
        EmbedFingerprint::new(
            v.to_vec(),
            Some("test-model".into()),
            EmbedConvention::None.id(),
        )
    }

    /// A fresh reading to compare against `fp`, same convention.
    fn fresh(v: &[f32]) -> EmbedFingerprint {
        fp(v)
    }

    #[test]
    fn identical_vector_matches() {
        let f = fp(&[1.0, 0.0, 0.0]);
        assert!(f.matches(&fresh(&[1.0, 0.0, 0.0])));
    }

    #[test]
    fn scaled_vector_still_matches() {
        // Cosine ignores magnitude: a provider that returns unnormalized vectors
        // (or normalizes differently between versions of the same model) must not
        // read as a model change.
        let f = fp(&[1.0, 2.0, 3.0]);
        assert!(f.matches(&fresh(&[2.0, 4.0, 6.0])));
    }

    #[test]
    fn tiny_numeric_noise_still_matches() {
        // Cloud providers are not bit-exact; the threshold must tolerate that.
        let f = fp(&[1.0, 0.0, 0.0]);
        assert!(f.matches(&fresh(&[0.9999, 0.001, 0.0])));
    }

    #[test]
    fn different_model_does_not_match() {
        // The real measured cross-model figure is ~0.369; anything near it must
        // read as a different model.
        let f = fp(&[1.0, 0.0, 0.0]);
        assert!(!f.matches(&fresh(&[0.369, 0.929, 0.0])));
    }

    #[test]
    fn different_dimension_never_matches() {
        let f = fp(&[1.0, 0.0, 0.0]);
        assert!(!f.matches(&fresh(&[1.0, 0.0])));
        assert!(!f.matches(&fresh(&[])));
    }

    #[test]
    fn display_id_falls_back_to_placeholder() {
        assert_eq!(fp(&[1.0]).display_id(), "test-model");
        assert_eq!(
            EmbedFingerprint::new(vec![1.0], None, "none").display_id(),
            "?"
        );
    }

    // ---------- the convention as a second, exact trigger (research §4) ----------

    #[test]
    fn a_changed_convention_is_a_changed_space_even_with_an_identical_canary() {
        // The case the config component exists for: a model whose passage marker
        // barely moves the vector (measured: 0.9965 on e5, only 0.0025 clear of
        // the detector). The canary alone could wave that through; the id cannot.
        let stored = EmbedFingerprint::new(vec![1.0, 0.0], Some("e5".into()), "none");
        let current = EmbedFingerprint::new(vec![1.0, 0.0], Some("e5".into()), "e5-instruct");
        assert!(!stored.matches(&current));
        // ...and switching back is recognised just as exactly.
        assert!(current.matches(&EmbedFingerprint::new(
            vec![1.0, 0.0],
            Some("e5".into()),
            "e5-instruct"
        )));
    }

    #[test]
    fn a_fingerprint_predating_conventions_reads_as_the_default() {
        // Every fingerprint already on disk has no convention field. Those
        // installations were using the default, so reading `None` as anything
        // else would report a spurious model change on the next launch.
        let legacy = EmbedFingerprint {
            canary: vec![1.0, 0.0],
            model_id: Some("bge-m3".into()),
            convention: None,
        };
        let current = EmbedFingerprint::new(vec![1.0, 0.0], Some("bge-m3".into()), "none");
        assert!(legacy.matches(&current), "no spurious change on upgrade");
        assert!(
            !legacy.matches(&EmbedFingerprint::new(
                vec![1.0, 0.0],
                Some("bge-m3".into()),
                "e5"
            )),
            "but turning a convention on is still a change"
        );
    }

    #[test]
    fn legacy_json_without_a_convention_deserializes() {
        // `#[serde(default)]` — the same additive rule the rest of the project
        // follows (ADR 0006 F12), so no migration is needed.
        let json = r#"{"canary":[1.0,0.0],"model_id":"bge-m3"}"#;
        let f: EmbedFingerprint = serde_json::from_str(json).unwrap();
        assert_eq!(f.convention, None);
    }

    #[test]
    fn canary_text_is_stable() {
        // Guards against a careless edit: changing the string would make every
        // existing installation report a model change on the next launch.
        assert_eq!(CANARY_TEXT, "mindfork embedding canary v1");
    }
}
