//! Notes — consolidate_notes + consolidation overviews (user-facing / @self). Part
//! of the [`super`] module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;

/// A similarity threshold as the *model* is told it, at the same 2 decimals the
/// pair lines below use (`{s:.2}`).
///
/// Two properties earn the fixed precision. The reference constants print exactly
/// as they did before calibration existed (`0.85`), so an uncalibrated
/// installation's overview is byte-identical; and because rounding is monotonic,
/// a listed pair (`s >= threshold`) can never *display* below the displayed
/// threshold — the model is never shown a number that contradicts the selection
/// it is looking at. A raw `f32::to_string` of a mapped threshold would print
/// like `0.95810324`.
fn format_threshold(t: f32) -> String {
    format!("{t:.2}")
}

/// Builds a knowledge-base overview for consolidation: similar pairs (possible
/// duplicates by cosine), `contradicts` links, notes with no links. Data only (no
/// rubric) — used by both the `consolidate_notes` tool and background auto-
/// consolidation. Isolation by `profile_id`. A pure DB read (no embedder needed —
/// vectors are already in the DB).
pub(crate) fn build_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
    loc: &crate::shared::i18n::Locale,
) -> String {
    // Consolidation — only over user-facing notes: self-notes (@self) are
    // excluded, so "sleep" doesn't mix memory about oneself with memory about the
    // interlocutor.
    let mut active = storage
        .db()
        .note_list(profile_id, None, &[], None)
        .unwrap_or_default();
    active.retain(|n| !is_self_note(n));
    if active.is_empty() {
        return loc.t("notes.overview.empty").to_string();
    }
    let mut with_vec = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    with_vec.retain(|(n, _)| !is_self_note(n));
    let links = storage.db().note_links_all(profile_id).unwrap_or_default();

    // The constant is a position in the reference (bge-m3) scale, so it has to be
    // read in the active model's range (identity until the model actually
    // changes). Once, not inside the O(n²) loop.
    let threshold = storage.db().similarity_scale().map(CONSOLIDATE_SIMILARITY);

    // Similar pairs (possible duplicates) by cosine, descending by similarity.
    let mut pairs: Vec<(f32, &Note, &Note)> = Vec::new();
    for i in 0..with_vec.len() {
        for j in (i + 1)..with_vec.len() {
            let s = cosine(&with_vec[i].1, &with_vec[j].1);
            if s >= threshold {
                pairs.push((s, &with_vec[i].0, &with_vec[j].0));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    let contradicts: Vec<&(Uuid, Uuid, String)> = links
        .iter()
        .filter(|(_, _, r)| r == "contradicts")
        .collect();
    let linked: std::collections::HashSet<Uuid> =
        links.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    let dangling: Vec<&Note> = active.iter().filter(|n| !linked.contains(&n.id)).collect();

    let mut out = format!(
        "{}\n{}\n",
        loc.t("notes.overview.header"),
        loc.tf(
            "notes.overview.active",
            &[
                ("n", &active.len().to_string()),
                ("d", &dangling.len().to_string())
            ]
        )
    );

    out.push('\n');
    // The threshold that actually selected the pairs, not the reference constant.
    out.push_str(&loc.tf(
        "notes.overview.pairs",
        &[
            ("sim", &format_threshold(threshold)),
            ("n", &pairs.len().to_string()),
        ],
    ));
    out.push('\n');
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }

    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.contradicts",
        &[("n", &contradicts.len().to_string())],
    ));
    out.push('\n');
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }

    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.dangling",
        &[("n", &dangling.len().to_string())],
    ));
    out.push('\n');
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }

    out.trim_end().to_string()
}

/// Overview of **"about self" observations** (`@self` self-notes) for
/// consolidation: similar pairs (possible duplicates), `contradicts` links among
/// observations, observations with no links. An analogue of
/// [`build_consolidation_overview`], but over "about self" memory — for auto-
/// reflection and the `reflect` tool. The self-consolidation overview was deferred
/// in Tier 2 "until the value of linking is confirmed"; linking was confirmed
/// (Tier 3, GO) — enabling it. `None` if there are fewer than 2 observations
/// (nothing to consolidate). A pure DB read (vectors already in the DB). Isolation
/// by `profile_id`. See docs/history/narrative-as-notes.md.
pub(crate) fn build_self_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
    // Only "about self" observations (@self) — mirroring the exclusion of self
    // from the user-facing notes overview: observation "sleep" doesn't touch
    // memory about the interlocutor.
    let active = storage
        .db()
        .note_list(profile_id, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap_or_default();
    if active.len() < 2 {
        return None;
    }
    let self_ids: std::collections::HashSet<Uuid> = active.iter().map(|n| n.id).collect();
    let mut with_vec = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    with_vec.retain(|(n, _)| is_self_note(n));
    let links = storage.db().note_links_all(profile_id).unwrap_or_default();

    // Read in the active model's range — see the sibling overview above.
    let threshold = storage.db().similarity_scale().map(CONSOLIDATE_SIMILARITY);

    // Similar pairs (possible duplicate observations) by cosine, descending by similarity.
    let mut pairs: Vec<(f32, &Note, &Note)> = Vec::new();
    for i in 0..with_vec.len() {
        for j in (i + 1)..with_vec.len() {
            let s = cosine(&with_vec[i].1, &with_vec[j].1);
            if s >= threshold {
                pairs.push((s, &with_vec[i].0, &with_vec[j].0));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    // contradicts among observations — both ends @self (the observation graph).
    let contradicts: Vec<&(Uuid, Uuid, String)> = links
        .iter()
        .filter(|(f, t, r)| r == "contradicts" && self_ids.contains(f) && self_ids.contains(t))
        .collect();
    let linked: std::collections::HashSet<Uuid> =
        links.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    let dangling: Vec<&Note> = active.iter().filter(|n| !linked.contains(&n.id)).collect();

    let mut out = format!(
        "{}\n{}\n",
        loc.t("notes.self_overview.header"),
        loc.tf(
            "notes.self_overview.count",
            &[
                ("n", &active.len().to_string()),
                ("d", &dangling.len().to_string())
            ]
        )
    );
    out.push('\n');
    // The threshold that actually selected the pairs, not the reference constant.
    out.push_str(&loc.tf(
        "notes.overview.pairs",
        &[
            ("sim", &format_threshold(threshold)),
            ("n", &pairs.len().to_string()),
        ],
    ));
    out.push('\n');
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }
    out.push('\n');
    out.push_str(&loc.tf(
        "notes.self_overview.contradicts",
        &[("n", &contradicts.len().to_string())],
    ));
    out.push('\n');
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }
    out.push('\n');
    out.push_str(&loc.tf(
        "notes.self_overview.dangling",
        &[("n", &dangling.len().to_string())],
    ));
    out.push('\n');
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }
    Some(out.trim_end().to_string())
}

/// Similarity threshold above which a self-description (`summary`) paragraph
/// counts as semantically matching an observation (`@self` note) — the A2 section
/// of the self-consolidation overview. **Calibrated on live bge-m3**
/// (like `TRAIT_SIMILARITY` in `self_model.rs`, the `summary_obs_calibration_e2e_live`
/// smoke): "paragraph ↔ observation" paraphrases scored 0.69-0.80, unrelated pairs
/// — 0.48-0.51; a clean gap 0.51→0.69. The 0.62 threshold (inside the gap, with
/// margin on both sides) catches all paraphrases and filters out unrelated ones.
/// Paragraphs are longer than short traits, so paraphrases score a bit lower than
/// at the trait gate (0.73-0.83). See docs/history/self-model-consolidation.md §A2.
///
/// A position in the **reference (bge-m3) scale**, not an absolute cosine: what
/// "0.62" means depends on the model's dynamic range, so it is read through
/// [`crate::shared::embed_calibration::SimilarityScale`] before use. On a model
/// whose range is 2.6× narrower the same intent sits at ~0.87 (research §8.2).
const SUMMARY_OBS_SIMILARITY: f32 = 0.62;

/// Minimum length of a `summary` paragraph (in characters) to participate in the
/// comparison: a shorter fragment is too small for a meaningful match.
const SUMMARY_PARAGRAPH_MIN_CHARS: usize = 40;

/// Semantic overlap between self-description (`summary`) paragraphs and
/// observations (`@self` notes) — the A2 section of the self-consolidation
/// overview. Observations already have vectors in the DB, but `summary` doesn't
/// (free-form text) — so paragraphs are embedded **on the fly** in one request (a
/// direct mirror of the trait gate `self_model::near_duplicate_traits`). Returns a
/// section with pairs "paragraph ≈ observation X → extract/stitch", or `None` if
/// there are no matches / no observations / an empty summary. **Graceful
/// degradation**: the embedder is unavailable, returned empty, or gave a mismatched
/// vector count → `None` (like RAG/web reranking). Isolation by `profile_id`. See
/// docs/history/self-model-consolidation.md §A2.
pub(crate) async fn summary_observation_overlaps(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
    // The profile's self-description (summary).
    let model = storage.db().self_model_get(profile_id).ok().flatten()?;
    let summary = model.summary.trim();
    if summary.is_empty() {
        return None;
    }
    // Summary paragraphs (by blank lines), dropping ones that are too short.
    let paragraphs: Vec<String> = summary
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| p.chars().count() >= SUMMARY_PARAGRAPH_MIN_CHARS)
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    // Observations (@self) with stored vectors — we compare against those.
    let mut obs = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    obs.retain(|(n, _)| is_self_note(n));
    if obs.is_empty() {
        return None;
    }
    // Embed the paragraphs in one request (summary has no stored vectors — on the fly).
    // Graceful degradation: an error/mismatched vector count → no section.
    let Ok(vecs) = embedder.embed(paragraphs.clone()).await else {
        return None;
    };
    if vecs.len() != paragraphs.len() {
        return None;
    }
    // Read in the active model's range, once — not inside the nested loop.
    let threshold = storage.db().similarity_scale().map(SUMMARY_OBS_SIMILARITY);
    let mut lines: Vec<String> = Vec::new();
    for (p, pv) in paragraphs.iter().zip(&vecs) {
        // The closest observation above the threshold (one per paragraph — no noise).
        let mut best: Option<(f32, &Note)> = None;
        for (n, nv) in &obs {
            let s = cosine(pv, nv);
            if s >= threshold && best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, n));
            }
        }
        if let Some((_, n)) = best {
            lines.push(format!(
                "- {} ≈ (id={}) {}",
                clip(p, 60),
                n.id,
                clip(&n.content, 60)
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n{}",
        loc.t("notes.self_overview.summary_obs"),
        lines.join("\n")
    ))
}

/// `consolidate_notes` — a knowledge-base overview + a rubric for consolidation
/// (an entry point, like SelfModel's `reflect`). Changes nothing: the model then
/// calls merge/supersede/revise/link on its own.
pub struct ConsolidateNotes;

#[async_trait::async_trait]
impl Tool for ConsolidateNotes {
    fn id(&self) -> ToolId {
        CONSOLIDATE_NOTES_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "notes consolidation"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.consolidate_notes.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let overview = build_consolidation_overview(&ctx.storage, ctx.profile_id, ctx.loc);
        let out = format!(
            "{overview}\n\n{}",
            ctx.loc.t("tool.consolidate_notes.rubric")
        );
        Ok(ToolOutcome::text(out))
    }
}
