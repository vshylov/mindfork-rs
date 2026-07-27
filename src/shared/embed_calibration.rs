//! Per-model calibration of the project's similarity gates.
//!
//! Three thresholds decide when two pieces of memory mean the same thing:
//! `CONSOLIDATE_SIMILARITY` (0.85), `TRAIT_SIMILARITY` (0.72) and
//! `SUMMARY_OBS_SIMILARITY` (0.62). All three are absolute cosines derived from
//! live runs against **bge-m3** — so each is a position inside *that* model's
//! cosine distribution, not a universal constant.
//!
//! Measured on the probe corpus below (2026-07-27):
//!
//! | | bge-m3 | multilingual-e5-large-instruct |
//! |---|---|---|
//! | paraphrase mean | 0.8176 | 0.9456 |
//! | unrelated mean | 0.4128 | 0.7897 |
//! | usable span | 0.4048 | 0.1559 |
//!
//! e5's usable range is **2.6× narrower**, which is the whole problem in one
//! number: on e5 the *unrelated* mean (0.7897) already sits above the 0.72 trait
//! gate, so after a perfectly executed reindex the gates would flip from
//! "silently never fire" to "fire on everything" — 8/8 unrelated probe pairs
//! (research §6).
//!
//! The fix is not a table of per-model constants: that only helps models someone
//! has already measured, and an arbitrary local GGUF would still be handed
//! bge-m3's numbers (§8.2, S7). Instead the corpus is embedded **once**, on the
//! same once-per-model path that records the canary fingerprint
//! ([`crate::shared::embed_identity`]), and the two measured means anchor an
//! affine map (S8):
//!
//! ```text
//! t' = u + (t − u_ref) · (p − u) / (p_ref − u_ref)
//! ```
//!
//! bge-m3 maps to itself, so nothing moves for the model the project is tuned
//! on. The mapped thresholds fire on the same probe pairs as the raw ones do
//! under bge-m3 (§8.2) — the map equalizes *scale*, it cannot equalize
//! semantics, and it is not meant to.
//!
//! **Failure is always downhill.** With no calibration recorded the scale is the
//! identity ([`SimilarityScale::identity`], S9), so an existing installation is
//! untouched until a model actually changes; a calibration that cannot be
//! measured, cannot be read back, or comes out degenerate likewise falls back to
//! the identity. A failed calibration can therefore only leave the gates exactly
//! as they are today — never make them wilder.

use std::sync::LazyLock;

use serde::Deserialize;

/// bge-m3's mean cosine over the corpus's **unrelated** pairs, and the anchor
/// the raw thresholds are expressed relative to.
///
/// Measured on the exact corpus in `embed_probes.json`; changing that file
/// invalidates this number and every threshold derived from it (see the
/// fixture's own header).
pub const REFERENCE_UNRELATED: f32 = 0.4128;

/// bge-m3's mean cosine over the corpus's **paraphrase** pairs. See
/// [`REFERENCE_UNRELATED`].
pub const REFERENCE_PARAPHRASE: f32 = 0.8176;

/// The reference model's usable dynamic range — the denominator of the affine
/// map. Non-zero by construction (the two constants above are measured and
/// distinct).
const REFERENCE_SPAN: f32 = REFERENCE_PARAPHRASE - REFERENCE_UNRELATED;

/// A mapped threshold is still a cosine, so it cannot meaningfully leave this
/// range. Clamping is a backstop against a pathological calibration rather than
/// something the arithmetic reaches on real models: with plausible inputs the
/// three project thresholds map well inside it. Of the two ends, hitting the
/// upper one is the harmless direction (a gate that effectively never fires);
/// the lower one would mean "everything is a duplicate", which is exactly what
/// the degeneracy checks in [`SimilarityScale::from_calibration`] exist to
/// prevent from arising at all.
const MIN_THRESHOLD: f32 = 0.0;
const MAX_THRESHOLD: f32 = 1.0;

/// The probe corpus. A data file rather than a `.rs` literal because it is a
/// measurement fixture that must never be edited casually (S10) — the reasoning
/// is recorded in the file's own header, and it has a deliberate entry in
/// `tools/cyrillic_scan.py` because it is intentionally bilingual.
const PROBES_JSON: &str = include_str!("embed_probes.json");

/// Pairs of short statements of the kind the gates actually judge: two that mean
/// the same thing, and two that have nothing in common.
///
/// The fixture's `_comment` key is ignored — serde skips unknown fields, which
/// is what lets the file document itself.
#[derive(Debug, Default, Deserialize)]
struct ProbeCorpus {
    paraphrase: Vec<(String, String)>,
    unrelated: Vec<(String, String)>,
}

impl ProbeCorpus {
    /// Every pair, paraphrase first — the order [`probe_texts`] flattens and
    /// [`measure`] reads back.
    fn pairs(&self) -> impl Iterator<Item = &(String, String)> {
        self.paraphrase.iter().chain(self.unrelated.iter())
    }
}

/// A malformed fixture degrades to an empty corpus rather than panicking: this
/// runs inside a TUI, and "no calibration" is a state the whole design already
/// handles (the identity scale). `corpus_is_well_formed` pins that the shipped
/// file parses, so the empty fallback is unreachable in a built binary.
static CORPUS: LazyLock<ProbeCorpus> =
    LazyLock::new(|| serde_json::from_str(PROBES_JSON).unwrap_or_default());

/// The probe corpus flattened for embedding: every text in a stable order, both
/// halves of a pair adjacent.
///
/// The caller embeds these in **one** request and hands the vectors straight to
/// [`measure`], which relies on this exact order.
pub fn probe_texts() -> Vec<String> {
    CORPUS
        .pairs()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .collect()
}

/// Turns the embedded probes back into the two means.
///
/// `None` when the input cannot be trusted to describe the corpus — a vector
/// count that does not match it, an empty vector, or mixed dimensionalities.
/// Refusing is the point: a calibration derived from a mismatched batch would be
/// a *wrong scale*, which is worse than no scale at all (S9).
pub fn measure(vectors: &[Vec<f32>]) -> Option<Calibration> {
    let (paraphrase_pairs, unrelated_pairs) = (CORPUS.paraphrase.len(), CORPUS.unrelated.len());
    if paraphrase_pairs == 0 || unrelated_pairs == 0 {
        return None;
    }
    if vectors.len() != 2 * (paraphrase_pairs + unrelated_pairs) {
        return None;
    }
    // A real embedder returns one width for the whole batch; anything else means
    // the response does not line up with the request we sent.
    let dim = vectors[0].len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return None;
    }

    let mean = |pairs: &[Vec<f32>]| -> Option<f32> {
        let sum: f32 = pairs.chunks_exact(2).map(|p| cosine(&p[0], &p[1])).sum();
        let mean = sum / (pairs.len() / 2) as f32;
        mean.is_finite().then_some(mean)
    };
    let split = 2 * paraphrase_pairs;
    Some(Calibration {
        paraphrase: mean(&vectors[..split])?,
        unrelated: mean(&vectors[split..])?,
    })
}

/// One embedding model's measured similarity range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// Mean cosine over pairs that have nothing in common — the floor of the
    /// model's usable range.
    pub unrelated: f32,
    /// Mean cosine over pairs that mean the same thing — its ceiling.
    pub paraphrase: f32,
}

/// Maps a threshold expressed in the reference (bge-m3) scale into the active
/// model's measured range.
#[derive(Debug, Clone, Copy)]
pub struct SimilarityScale {
    /// `None` — the identity, so a threshold passes through **exactly**
    /// unchanged rather than through arithmetic that merely ought to cancel out.
    /// That matters: this is the state every existing installation is in, and
    /// bit-identical pass-through is what makes "nothing changes until the model
    /// does" a fact instead of a rounding argument.
    affine: Option<Affine>,
}

/// The mapping itself, pre-divided so [`SimilarityScale::map`] is one multiply
/// and one add.
#[derive(Debug, Clone, Copy)]
struct Affine {
    unrelated: f32,
    /// How much narrower (or wider) this model's range is than the reference's.
    factor: f32,
}

impl SimilarityScale {
    /// Thresholds pass through unchanged — the state of a DB that has never been
    /// calibrated, and the fallback for every failure (S9).
    pub fn identity() -> Self {
        Self { affine: None }
    }

    /// The scale for a measured model, or the identity when the measurement
    /// cannot describe a usable range.
    ///
    /// Rejected as degenerate: a non-finite value, a value outside the cosine
    /// range it claims to be one of, and `paraphrase <= unrelated` (an inverted
    /// or zero span). A zero span is the interesting one — it would collapse all
    /// three gates onto the unrelated mean, i.e. make everything a duplicate.
    pub fn from_calibration(c: Calibration) -> Self {
        let plausible = [c.unrelated, c.paraphrase]
            .iter()
            .all(|v| v.is_finite() && (-1.0..=1.0).contains(v));
        let span = c.paraphrase - c.unrelated;
        if !plausible || span <= 0.0 {
            return Self::identity();
        }
        Self {
            affine: Some(Affine {
                unrelated: c.unrelated,
                factor: span / REFERENCE_SPAN,
            }),
        }
    }

    /// `t' = u + (t − u_ref) · (p − u) / (p_ref − u_ref)`, clamped to the cosine
    /// range (see [`MIN_THRESHOLD`]).
    pub fn map(&self, threshold: f32) -> f32 {
        match self.affine {
            None => threshold,
            Some(Affine { unrelated, factor }) => (unrelated
                + (threshold - REFERENCE_UNRELATED) * factor)
                .clamp(MIN_THRESHOLD, MAX_THRESHOLD),
        }
    }
}

/// Cosine similarity (0.0 on a length mismatch or a zero norm). A local copy
/// rather than a shared helper, for the same reason
/// [`crate::shared::embed_identity`] keeps one: the existing implementations are
/// private to their modules (`db`, `features::tools::notes`,
/// `features::tools::web`), and `shared` cannot import from `features` (FSD).
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

    /// The three gates this whole mechanism exists for.
    const GATES: [f32; 3] = [0.85, 0.72, 0.62];

    /// `multilingual-e5-large-instruct`'s measured means — the worked example of
    /// research §8.2, and the model whose 2.6× narrower range motivates all of
    /// this.
    const E5_UNRELATED: f32 = 0.7897;
    const E5_PARAPHRASE: f32 = 0.9456;

    fn scale(unrelated: f32, paraphrase: f32) -> SimilarityScale {
        SimilarityScale::from_calibration(Calibration {
            unrelated,
            paraphrase,
        })
    }

    // ---------- the fixture ----------

    #[test]
    fn corpus_is_well_formed() {
        // Pins that the shipped file parses (the empty fallback in `CORPUS` is a
        // safety net, not a state a release may ship in) and that it still has
        // the shape the reference constants were measured on.
        assert_eq!(CORPUS.paraphrase.len(), 8, "8 paraphrase pairs (§8.2)");
        assert_eq!(CORPUS.unrelated.len(), 8, "8 unrelated pairs (§8.2)");
        assert!(
            CORPUS.pairs().all(|(a, b)| !a.is_empty() && !b.is_empty()),
            "an empty probe would embed to nothing and skew a mean"
        );
    }

    #[test]
    fn probe_texts_are_flattened_in_a_stable_order() {
        let first = probe_texts();
        assert_eq!(first.len(), 32, "16 pairs, both halves");
        assert_eq!(
            first,
            probe_texts(),
            "the order must not vary between calls"
        );
        // Paraphrase first, both halves of a pair adjacent — the layout
        // `measure` reads back.
        assert_eq!(first[0], CORPUS.paraphrase[0].0);
        assert_eq!(first[1], CORPUS.paraphrase[0].1);
        assert_eq!(first[16], CORPUS.unrelated[0].0);
    }

    // ---------- measuring ----------

    /// 32 vectors in `probe_texts` order: paraphrase halves 45° apart
    /// (cos = 1/√2), unrelated halves orthogonal (cos = 0).
    fn probe_vectors() -> Vec<Vec<f32>> {
        let mut v = Vec::new();
        for _ in 0..CORPUS.paraphrase.len() {
            v.push(vec![1.0, 0.0]);
            v.push(vec![1.0, 1.0]);
        }
        for _ in 0..CORPUS.unrelated.len() {
            v.push(vec![1.0, 0.0]);
            v.push(vec![0.0, 1.0]);
        }
        v
    }

    #[test]
    fn measure_returns_the_two_means() {
        let c = measure(&probe_vectors()).expect("a well-formed batch");
        assert!(
            (c.paraphrase - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "{c:?}"
        );
        assert!(c.unrelated.abs() < 1e-5, "{c:?}");
    }

    #[test]
    fn measure_refuses_a_batch_that_does_not_describe_the_corpus() {
        // A wrong count, an empty vector or mixed widths all mean the response
        // does not line up with the request — guessing a scale from it would be
        // worse than having none.
        assert_eq!(measure(&[]), None, "empty");
        assert_eq!(measure(&vec![vec![1.0, 0.0]; 31]), None, "one short");
        assert_eq!(measure(&vec![vec![1.0, 0.0]; 33]), None, "one long");

        let mut empty_vector = probe_vectors();
        empty_vector[3] = Vec::new();
        assert_eq!(measure(&empty_vector), None, "an empty vector");

        let mut mixed = probe_vectors();
        mixed[3] = vec![1.0, 0.0, 0.0];
        assert_eq!(measure(&mixed), None, "mixed dimensionalities");
    }

    // ---------- the scale ----------

    #[test]
    fn identity_passes_thresholds_through_untouched() {
        // Exact equality, not approximate: this is the state of every
        // uncalibrated installation, so "nothing changes" must be literal.
        for t in GATES.iter().chain(&[0.0, 0.5, 1.0]) {
            assert_eq!(SimilarityScale::identity().map(*t), *t);
        }
    }

    #[test]
    fn the_reference_calibration_is_the_identity() {
        // The property that guarantees bge-m3 users see no change at all: the
        // constants were measured on this model, so mapping into its own range
        // must return them.
        let s = scale(REFERENCE_UNRELATED, REFERENCE_PARAPHRASE);
        for t in GATES {
            assert!((s.map(t) - t).abs() < 1e-5, "{t} -> {}", s.map(t));
        }
    }

    #[test]
    fn e5_reproduces_the_measured_thresholds() {
        // The worked example from research §8.2: e5-large-instruct's measured
        // means, and the mapped gates validated there against the probe corpus.
        // These numbers are load-bearing — they are what makes the trait gate
        // reject the unrelated pairs it would otherwise accept.
        let s = scale(E5_UNRELATED, E5_PARAPHRASE);
        for (raw, expected) in [(0.85, 0.958), (0.72, 0.908), (0.62, 0.869)] {
            assert!(
                (s.map(raw) - expected).abs() < 0.002,
                "{raw} -> {} (§8.2 says {expected})",
                s.map(raw)
            );
        }
        // And the point of the whole exercise: e5's unrelated mean sits *above*
        // the raw 0.72 trait gate (which is why the raw constant fires on 8/8
        // unrelated pairs there), but below the mapped one.
        let trait_gate = s.map(0.72);
        assert!(
            E5_UNRELATED > 0.72 && trait_gate > E5_UNRELATED,
            "unrelated {E5_UNRELATED} must sit above the raw gate and below the mapped one ({trait_gate})"
        );
    }

    #[test]
    fn a_narrower_range_moves_every_gate_up_and_keeps_their_order() {
        let s = scale(E5_UNRELATED, E5_PARAPHRASE);
        let mapped: Vec<f32> = GATES.iter().map(|t| s.map(*t)).collect();
        assert!(
            mapped.windows(2).all(|w| w[0] > w[1]),
            "the gates keep their relative order: {mapped:?}"
        );
        assert!(
            GATES.iter().zip(&mapped).all(|(raw, m)| m > raw),
            "a compressed range pushes every gate up: {mapped:?}"
        );
    }

    #[test]
    fn a_degenerate_calibration_falls_back_to_the_identity() {
        // Never a wrong scale — the fallback can only leave the gates as they
        // already are (S9).
        let degenerate = [
            (0.9, 0.4, "inverted: paraphrase below unrelated"),
            (
                0.5,
                0.5,
                "zero span: every gate would collapse onto one value",
            ),
            (f32::NAN, 0.8, "not a number"),
            (0.4, f32::NAN, "not a number"),
            (0.4, f32::INFINITY, "not finite"),
            (-2.0, 0.8, "outside the cosine range"),
            (0.4, 1.5, "outside the cosine range"),
        ];
        for (unrelated, paraphrase, why) in degenerate {
            for t in GATES {
                assert_eq!(scale(unrelated, paraphrase).map(t), t, "{why}");
            }
        }
    }

    #[test]
    fn a_mapped_threshold_never_leaves_the_cosine_range() {
        // A plausible-but-extreme calibration (a model spanning the full cosine
        // range) would overshoot without the clamp.
        let s = scale(-1.0, 1.0);
        for t in GATES.iter().chain(&[0.0, 0.1, 0.99, 1.0]) {
            let mapped = s.map(*t);
            assert!(
                (MIN_THRESHOLD..=MAX_THRESHOLD).contains(&mapped),
                "{t} -> {mapped}"
            );
        }
    }

    // ---------- cosine ----------

    #[test]
    fn cosine_is_scale_invariant_and_safe_on_bad_input() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0, "length mismatch");
        assert_eq!(cosine(&[], &[]), 0.0, "empty");
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0, "zero norm");
    }
}
