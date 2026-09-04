//! "Self-model" (SelfModel) tools — the MVP probe: reading, reflecting on, and
//! minimally updating the agent's representation of itself, its goals, and the
//! interlocutor. Data is per-profile, in SQLite (like notes) — mutator tools write
//! **directly** through `ctx.storage` (not via `ChatEffect`). Isolation by
//! `ctx.profile_id`. See [docs/history/self-model-mvp.md](../../../docs/history/self-model-mvp.md).

use anyhow::Result;
use chrono::Utc;

use crate::entities::profile::ToolId;
use crate::entities::self_model::{
    GoalMatch, GoalStatus, NarrativeSegment, SelfModel, SelfModelParams,
};
use crate::shared::api::EmbedRole;
use crate::shared::i18n::Locale;

use super::{Tool, ToolContext, ToolOutcome, notes};

pub const GET_SELF_MODEL_ID: &str = "get_self_model";
pub const REFLECT_ID: &str = "reflect";
pub const UPDATE_SELF_MODEL_ID: &str = "update_self_model";
pub const UPDATE_USER_MODEL_ID: &str = "update_user_model";
pub const ADD_INSIGHT_ID: &str = "add_insight";

/// All ids of the "self-model" tool group (for detecting edits within a turn).
/// `consolidate_narrative` was removed — observations moved into notes (Tier 1
/// "narrative as notes"), consolidated by note tools (note_revise/note_supersede/
/// note_merge). See docs/history/narrative-as-notes.md.
pub const ALL_IDS: &[&str] = &[
    GET_SELF_MODEL_ID,
    REFLECT_ID,
    UPDATE_SELF_MODEL_ID,
    UPDATE_USER_MODEL_ID,
    ADD_INSIGHT_ID,
];

/// Is the tool part of the "self-model" group (for the `SelfModelChanged` signal
/// after a turn where the model edited something through its tools).
pub fn is_self_model_tool(name: &str) -> bool {
    ALL_IDS.contains(&name)
}

/// Canonical "self-model" maintenance rules in the agent-scaffold language (`loc`)
/// — key `selfmodel.policy_core`. **The single source** of the wording, from which
/// the maintenance protocol ([`maintenance_protocol`]) and the auto-reflection
/// system message (`orchestrator::reflection`) are assembled. The interactive
/// `reflect` rubric — deliberately not from here (a different genre). Localization
/// — axis A, see docs/history/i18n.md.
pub fn policy_core(loc: &Locale) -> &str {
    loc.t("selfmodel.policy_core")
}

/// A persona-neutral "model maintenance protocol" — [`policy_core`] framed as "you
/// manage this model yourself". Mixed into the turn's system prompt on top of any
/// profile persona (see `orchestrator::generation::inject_self_model`), making use
/// of SelfModel tools predictable regardless of persona.
pub fn maintenance_protocol(loc: &Locale) -> String {
    loc.tf(
        "selfmodel.maintenance_wrapper",
        &[("core", policy_core(loc))],
    )
}

/// Loads the profile's model from storage (or an empty one, if it hasn't been
/// created yet). Read from the DB, not from the `ctx.self_model` snapshot, to see
/// edits made by other SelfModel tools within the same turn.
fn load(ctx: &ToolContext) -> Result<SelfModel> {
    Ok(ctx
        .storage
        .db()
        .self_model_get(ctx.profile_id)?
        .unwrap_or_else(|| SelfModel::new(ctx.profile_id)))
}

/// The profile's fresh observations (self-notes) as narrative segments — for a
/// full read (`render_full`). Observations moved into notes (`@self`), so the
/// "self-model" render gets them as a parameter. Newest first, up to
/// `max_narrative`.
fn recent_segments(ctx: &ToolContext) -> Vec<NarrativeSegment> {
    notes::self_notes_recent(
        &ctx.storage,
        ctx.profile_id,
        ctx.self_model_params.max_narrative,
    )
    .into_iter()
    .map(|n| NarrativeSegment {
        id: n.id,
        text: n.content,
        created_at: n.created_at,
    })
    .collect()
}

/// Full "self-model" read for tools (`get_self_model`/`reflect`/echo):
/// `render_full` + a "Related observations" block (the graph over observations,
/// Tier 2) + a "Source citations" block (an observation citing a RAG source,
/// Tier 3, Path 3). Observations, their links, and citations — from notes.
fn render_self_read(ctx: &ToolContext, m: &SelfModel) -> String {
    let recent = recent_segments(ctx);
    let ids: Vec<uuid::Uuid> = recent.iter().map(|s| s.id).collect();
    let mut out = m.render_full(Utc::now(), &recent, ctx.loc);
    if let Some(block) = notes::self_related_block(ctx, &ids) {
        out.push_str(&block);
    }
    if let Some(block) = notes::cited_sources_block(ctx, &ids) {
        out.push_str(&block);
    }
    // Soft gate on description size (stage 2): if the summary has grown — a hint
    // to shrink it. Visible in get_self_model/reflect and auto-reflection (which
    // starts with get_self_model). See docs/summary-as-snapshot.md.
    if let Some(hint) = m.summary_fill_hint(ctx.self_model_params.summary_target_chars, ctx.loc) {
        out.push_str("\n\n");
        out.push_str(&hint);
    }
    out
}

/// Extracts a string array by key (empty if missing/not an array).
fn str_array(args: &serde_json::Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Cosine-similarity threshold above which an added trait counts as **related** to
/// an existing one (the `add_traits` gate). Calibrated on live bge-m3 (Step C): it
/// compresses short traits into a narrow band, and real paraphrases score
/// ~0.73-0.83 ("values brevity" ↔ "appreciates concise answers" = 0.77,
/// "skeptical" ↔ "critical" = 0.73), while unrelated ones score lower (coffee↔hiking
/// = 0.69, programmer↔cooking = 0.58). The 0.72 threshold separates them.
/// **Important:** bge-m3 groups by *dimension/topic*, not by direction of meaning,
/// so antonyms also fall into the band ("values brevity" ↔ "values long
/// explanations" = 0.71) — but that's a feature of the gate: a related trait is
/// worth surfacing so the model can decide **duplicate (merge) or contradiction
/// (record as an observation)**. See docs/history/narrative-as-notes.md (Tier 2,
/// Step C).
///
/// A position in the **reference (bge-m3) scale**, not an absolute cosine: the
/// numbers above describe *that* model's distribution, and another model's can be
/// far narrower — on `multilingual-e5-large-instruct` the unrelated mean is 0.79,
/// i.e. above this constant, so used raw the gate would fire on everything
/// (research §6). It is therefore read through
/// [`crate::shared::embed_calibration::SimilarityScale`], which maps the same
/// intent to ~0.91 there (research §8.2).
const TRAIT_SIMILARITY: f32 = 0.72;

/// The related-traits gate (Step C): for EVERY actually-added trait, looks for the
/// closest one among the PRIOR ones (existing before this edit) above the
/// [`TRAIT_SIMILARITY`] threshold. Returns pairs (added, close existing one),
/// newest first. Embeds new + prior traits in one request; traits have no stored
/// vectors (a flat `Vec<String>`), so we compute on the fly. Empty when the
/// embedder is unavailable or the vector count doesn't match — **graceful
/// degradation**, a direct mirror of the `add_insight`/`note_save` gates. See
/// docs/history/narrative-as-notes.md (Tier 2, Step C).
async fn near_duplicate_traits(
    ctx: &ToolContext,
    added: &[String],
    existing_before: &[String],
) -> Vec<(String, String)> {
    if added.is_empty() || existing_before.is_empty() {
        return Vec::new();
    }
    // One request: added ones first, then prior ones — to split by the boundary.
    let texts: Vec<String> = added.iter().chain(existing_before).cloned().collect();
    // Passage on both sides: the comparison is symmetric (new traits against
    // prior ones) and feeds TRAIT_SIMILARITY, which is calibrated on
    // passage-role text. A mismatched role here would cost up to 17% of a
    // compressed model's usable range (research §2.3).
    let Ok(vecs) = ctx.embedder.embed(texts, EmbedRole::Passage).await else {
        return Vec::new();
    };
    if vecs.len() != added.len() + existing_before.len() {
        return Vec::new();
    }
    let (added_vecs, existing_vecs) = vecs.split_at(added.len());
    // The constant is a position in the reference (bge-m3) scale, so it has to be
    // read in the active model's range (identity until the model actually
    // changes). Once, not inside the nested loop.
    let threshold = ctx.storage.db().similarity_scale().map(TRAIT_SIMILARITY);
    let mut out = Vec::new();
    for (i, a) in added.iter().enumerate() {
        // Closest prior trait above the threshold (one per added trait — no noise).
        let mut best: Option<(f32, usize)> = None;
        for (j, _) in existing_before.iter().enumerate() {
            let s = notes::cosine(&added_vecs[i], &existing_vecs[j]);
            if s >= threshold && best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, j));
            }
        }
        if let Some((_, j)) = best {
            out.push((a.clone(), existing_before[j].clone()));
        }
    }
    out
}

/// `get_self_model` — current state of the self-model (read).
pub struct GetSelfModel;

#[async_trait::async_trait]
impl Tool for GetSelfModel {
    fn id(&self) -> ToolId {
        GET_SELF_MODEL_ID.into()
    }
    /// Reads the profile's self-model; a read (docs/research/concurrent-tools.md
    /// §2.3). Its writers are not marked.
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "show self-model"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.get_self_model.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        Ok(ToolOutcome::text(render_self_read(ctx, &m)))
    }
}

/// `reflect` — returns the current model and a rubric for reflection. Writes
/// nothing: this is the reflection "entry point", after which the model calls
/// `update_self_model`/`update_user_model` on its own if there's something to record.
pub struct Reflect;

#[async_trait::async_trait]
impl Tool for Reflect {
    fn id(&self) -> ToolId {
        REFLECT_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "self-reflection"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.reflect.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let m = load(ctx)?;
        // Observation overview for consolidation (similar pairs / contradicts /
        // without links) — concrete data under the rubric below. Empty if there
        // are fewer than 2 observations.
        let mut overview =
            notes::build_self_consolidation_overview(&ctx.storage, ctx.profile_id, ctx.loc)
                .unwrap_or_default();
        // A2: semantic overlap between self-description (summary) paragraphs and
        // observations — embedding paragraphs on the fly (summary has no stored
        // vectors). See docs/history/self-model-consolidation.md §A2.
        if let Some(section) = notes::summary_observation_overlaps(
            &ctx.storage,
            ctx.embedder.as_ref(),
            ctx.profile_id,
            ctx.loc,
        )
        .await
        {
            if overview.is_empty() {
                overview = section;
            } else {
                overview.push_str("\n\n");
                overview.push_str(&section);
            }
        }
        let overview = if overview.is_empty() {
            String::new()
        } else {
            format!("\n\n{overview}")
        };
        let out = format!(
            "{}\n{}{overview}\n\n{}",
            ctx.loc.t("tool.reflect.rubric.header"),
            render_self_read(ctx, &m),
            ctx.loc.t("tool.reflect.rubric.questions"),
        );
        Ok(ToolOutcome::text(out))
    }
}

/// `add_insight` — adds a short observation/insight to the narrative (including
/// noticed contradictions — in prose, no separate type). Writes directly to the DB.
pub struct AddInsight;

#[async_trait::async_trait]
impl Tool for AddInsight {
    fn id(&self) -> ToolId {
        ADD_INSIGHT_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "add observation"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.add_insight.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": loc.t("tool.add_insight.param.text")}
            },
            "required": ["text"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            anyhow::bail!(ctx.loc.t("tool.add_insight.err.text_empty"));
        }
        // An observation is a self-note (@self): gets an embedding, semantic
        // search, a graph, and consolidation on par with regular notes, but is
        // hidden from user-facing recall. See docs/history/narrative-as-notes.md.
        let id =
            notes::create_note(ctx, text.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await?;
        let mut msg = ctx.loc.tf(
            "tool.add_insight.result.recorded",
            &[("id", &id.to_string())],
        );
        // The gate (Tier 1's core hypothesis): similar existing observations — to
        // rewrite a near-duplicate via note_revise/note_supersede rather than
        // spawning a copy.
        let similar = notes::self_note_similar(ctx, &text, id).await;
        if !similar.is_empty() {
            msg.push('\n');
            msg.push_str(ctx.loc.t("tool.add_insight.gate.similar"));
            for n in similar {
                msg.push_str(&format!("\n- (id={}) {}", n.id, n.content));
            }
        }
        Ok(ToolOutcome::text(msg))
    }
}

/// `update_self_model` — edits the self-description and/or goals.
pub struct UpdateSelfModel;

#[async_trait::async_trait]
impl Tool for UpdateSelfModel {
    fn id(&self) -> ToolId {
        UPDATE_SELF_MODEL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "update self-model"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.update_self_model.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": loc.t("tool.update_self_model.param.summary")},
                "add_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.add_goals")},
                "complete_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.complete_goals")},
                "abandon_goals": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_self_model.param.abandon_goals")}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Unresolved goal handles and scars from folded closed goals are collected
        // inside the atomic edit (captured by `&mut`); writing the model happens
        // under one mutex acquisition, while scars → self-notes happen afterward
        // (the closure has no access to storage/async).
        let mut deltas = SelfModelEditDeltas::default();
        let params = ctx.self_model_params;
        let loc = ctx.loc; // &'static — copy so the closure doesn't borrow ctx
        let (model, changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            apply_self_model_edit(m, &args, params, loc, &mut deltas)
        })?;

        // Scars from folded closed goals → self-notes (observations). Best-effort.
        for scar in &deltas.scars {
            let _ =
                notes::create_note(ctx, scar.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await;
        }

        if !changed {
            let mut msg = ctx.loc.t("selfmodel.result.nothing").to_string();
            if !deltas.unresolved.is_empty() {
                msg.push('\n');
                msg.push_str(&ctx.loc.tf(
                    "tool.update_self_model.goals_not_found",
                    &[("list", &deltas.unresolved.join(", "))],
                ));
            }
            return Ok(ToolOutcome::text(msg));
        }
        Ok(ToolOutcome::text(update_self_model_echo(
            ctx.loc, &model, params, &deltas,
        )))
    }
}

/// Side data of `update_self_model`'s atomic edit, collected inside the
/// `self_model_update` closure (captured by `&mut`) and consumed afterward.
#[derive(Default)]
struct SelfModelEditDeltas {
    /// Unresolved goal handles (misses and, localized, ambiguities).
    unresolved: Vec<String>,
    /// Scars from folded closed goals — written as self-notes after the edit.
    scars: Vec<String>,
    /// Whether the self-description changed — for the size line in the echo
    /// (the size gate, stage 2).
    summary_changed: bool,
    /// Edit deltas for a compact echo (stage 4): what was actually added/closed
    /// — instead of a full render_full (that stays with get_self_model). Goals
    /// are named by #id — the same handle used later to close them.
    added_goals: Vec<(String, uuid::Uuid)>,
    completed: Vec<uuid::Uuid>,
    abandoned: Vec<uuid::Uuid>,
}

/// The body of `update_self_model`'s atomic edit. Runs INSIDE the
/// `self_model_update` closure — one mutex acquisition, no storage/async access
/// here (the atomicity boundary is load-bearing). Returns whether the model
/// changed.
fn apply_self_model_edit(
    m: &mut SelfModel,
    args: &serde_json::Value,
    params: SelfModelParams,
    loc: &Locale,
    d: &mut SelfModelEditDeltas,
) -> bool {
    let mut changed = false;
    if let Some(s) = args.get("summary").and_then(|v| v.as_str()) {
        let s = s.trim().to_string();
        if m.summary != s {
            m.summary = s;
            changed = true;
            d.summary_changed = true;
        }
    }
    for g in str_array(args, "add_goals") {
        let before = m.goals.len();
        m.add_goal(g);
        // add_goal ignores empty ones — record only what was actually added.
        if m.goals.len() != before {
            let goal = m.goals.last().expect("just added");
            d.added_goals.push((goal.description.clone(), goal.id));
            changed = true;
        }
    }
    // Goals are closed by #id/a full id — resolve the handle among the
    // model's goals.
    changed |= close_goals(
        m,
        args,
        "complete_goals",
        GoalStatus::Completed,
        loc,
        &mut d.completed,
        &mut d.unresolved,
    );
    changed |= close_goals(
        m,
        args,
        "abandon_goals",
        GoalStatus::Abandoned,
        loc,
        &mut d.abandoned,
        &mut d.unresolved,
    );
    // Folding old closed goals: fold returns scars (texts) — we'll
    // write them as self-notes after the atomic edit (a cap on closed
    // goals: the structure doesn't grow, the "biography" is preserved
    // as an observation).
    d.scars = m.fold_closed_goals(params.max_closed_goals, loc);
    if !d.scars.is_empty() {
        changed = true;
    }
    changed
}

/// Closes goals from the `key` string array by #id/full-id handle (see
/// `SelfModel::match_goal`): closed ids go into `closed`, misses/ambiguities
/// into `unresolved`. Returns whether anything actually changed.
fn close_goals(
    m: &mut SelfModel,
    args: &serde_json::Value,
    key: &str,
    status: GoalStatus,
    loc: &Locale,
    closed: &mut Vec<uuid::Uuid>,
    unresolved: &mut Vec<String>,
) -> bool {
    let mut changed = false;
    for h in str_array(args, key) {
        match m.match_goal(&h) {
            GoalMatch::One(id) => {
                if m.set_goal_status(id, status) {
                    closed.push(id);
                    changed = true;
                }
            }
            GoalMatch::None => unresolved.push(h),
            GoalMatch::Ambiguous => {
                unresolved.push(loc.tf("tool.update_self_model.ambiguous", &[("h", &h)]))
            }
        }
    }
    changed
}

/// Delta echo (stage 4): only what changed, without a full render_full (a
/// full read — get_self_model's job). Saves tokens and doesn't "anchor" the
/// model on the essay genre.
fn update_self_model_echo(
    loc: &Locale,
    model: &SelfModel,
    params: SelfModelParams,
    d: &SelfModelEditDeltas,
) -> String {
    use crate::entities::self_model::short_id;
    let mut msg = loc.t("tool.update_self_model.result.updated").to_string();
    // Feedback about description size (stage 2): always on a summary edit, so
    // the model sees growth even before exceeding the target. See
    // docs/summary-as-snapshot.md.
    if d.summary_changed {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.size",
            &[
                ("n", &model.summary.chars().count().to_string()),
                ("target", &params.summary_target_chars.to_string()),
            ],
        ));
    }
    if !d.added_goals.is_empty() {
        let list: Vec<String> = d
            .added_goals
            .iter()
            .map(|(desc, id)| format!("#{} {desc}", short_id(id)))
            .collect();
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.added_goals",
            &[("list", &list.join("; "))],
        ));
    }
    if !d.completed.is_empty() {
        let ids: Vec<String> = d
            .completed
            .iter()
            .map(|id| format!("#{}", short_id(id)))
            .collect();
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.completed",
            &[("list", &ids.join(", "))],
        ));
    }
    if !d.abandoned.is_empty() {
        let ids: Vec<String> = d
            .abandoned
            .iter()
            .map(|id| format!("#{}", short_id(id)))
            .collect();
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.abandoned",
            &[("list", &ids.join(", "))],
        ));
    }
    if !d.scars.is_empty() {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.folded",
            &[("n", &d.scars.len().to_string())],
        ));
    }
    if !d.unresolved.is_empty() {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_self_model.goals_not_found_paren",
            &[("list", &d.unresolved.join(", "))],
        ));
    }
    msg
}

/// `update_user_model` — edits the representation of the interlocutor.
pub struct UpdateUserModel;

#[async_trait::async_trait]
impl Tool for UpdateUserModel {
    fn id(&self) -> ToolId {
        UPDATE_USER_MODEL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::SelfModel
    }
    fn ui_label(&self) -> &'static str {
        "update user model"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.update_user_model.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "add_traits": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.add_traits")},
                "remove_traits": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.remove_traits")},
                "add_interests": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.add_interests")},
                "remove_interests": {"type": "array", "items": {"type": "string"}, "description": loc.t("tool.update_user_model.param.remove_interests")},
                "relationship_dynamic": {"type": "string", "description": loc.t("tool.update_user_model.param.relationship_dynamic")},
                "note": {"type": "string", "description": loc.t("tool.update_user_model.param.note")}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        // Removal/note-presence flags are collected inside the atomic edit
        // (captured by `&mut`); the write happens under one mutex acquisition
        // (protection against a race).
        //
        // The near-duplicate-traits gate (Step C): needs a snapshot of the traits
        // BEFORE adding — collected inside the atomic edit (captured by `&mut`),
        // the embedding happens after.
        let requested_traits = str_array(&args, "add_traits");
        let mut deltas = UserModelEditDeltas::default();
        let (model, changed) = ctx.storage.db().self_model_update(ctx.profile_id, |m| {
            apply_user_model_edit(m, &args, &requested_traits, &mut deltas)
        })?;

        // Scar → self-note (observation). Best-effort. `note` by itself doesn't
        // count as a model change (it's a separate note), but makes the call
        // meaningful.
        if let Some(scar) = &deltas.note_scar {
            let _ =
                notes::create_note(ctx, scar.clone(), vec![notes::SELF_NOTE_TAG.to_string()]).await;
        }

        if !changed && !deltas.has_note {
            return Ok(ToolOutcome::text(ctx.loc.t("selfmodel.result.nothing")));
        }
        // Actually-added traits (new after dedup, no within-batch repeats) —
        // compare them against the prior ones via the near-duplicate gate
        // (embedding only if there's something to compare; softly empty if the
        // embedder is unavailable).
        let added_traits = newly_added_traits(&requested_traits, &deltas.existing_before_traits);
        let dup_pairs =
            near_duplicate_traits(ctx, &added_traits, &deltas.existing_before_traits).await;

        Ok(ToolOutcome::text(update_user_model_echo(
            ctx.loc, &model, &deltas, &dup_pairs,
        )))
    }
}

/// Side data of `update_user_model`'s atomic edit, collected inside the
/// `self_model_update` closure (captured by `&mut`) and consumed afterward.
#[derive(Default)]
struct UserModelEditDeltas {
    removed_traits: bool,
    removed_interests: bool,
    replaced_dynamic: bool,
    has_note: bool,
    /// The revision scar (`note`) is collected inside the closure, but written
    /// **after** — as a self-note (observation), not into the model blob
    /// (async/storage are outside the closure).
    note_scar: Option<String>,
    /// A snapshot of the traits BEFORE adding — for the near-duplicate gate.
    existing_before_traits: Vec<String>,
}

/// The body of `update_user_model`'s atomic edit. Runs INSIDE the
/// `self_model_update` closure — one mutex acquisition, no storage/async access
/// here (the atomicity boundary is load-bearing). Returns whether the model
/// changed.
fn apply_user_model_edit(
    m: &mut SelfModel,
    args: &serde_json::Value,
    requested_traits: &[String],
    d: &mut UserModelEditDeltas,
) -> bool {
    let mut changed = false;

    // Lists — merge (add/remove with dedup), not replacement: an edit
    // doesn't zero out the accumulated representation (a common
    // "overwritten by mood" bug).
    d.existing_before_traits = m.user_model.perceived_traits.clone();
    changed |= m.user_model.add_traits(requested_traits.to_vec());
    d.removed_traits = m
        .user_model
        .remove_traits(&str_array(args, "remove_traits"));
    changed |= d.removed_traits;
    changed |= m.user_model.add_interests(str_array(args, "add_interests"));
    d.removed_interests = m
        .user_model
        .remove_interests(&str_array(args, "remove_interests"));
    changed |= d.removed_interests;
    if let Some(s) = args.get("relationship_dynamic").and_then(|v| v.as_str()) {
        let s = s.trim().to_string();
        if m.user_model.relationship_dynamic != s {
            // Replacing a NON-EMPTY dynamic — a substantial revision
            // (unlike initial population); ask to leave a trace (a scar),
            // like traits.
            d.replaced_dynamic = !m.user_model.relationship_dynamic.trim().is_empty();
            m.user_model.relationship_dynamic = s;
            changed = true;
        }
    }
    // Revision scar: save `note` (what changed and why) — it goes out as
    // an "about self" observation note, so a change of opinion about the
    // interlocutor leaves a trace (traits are flat — the "biography" of
    // their changes lives in observations).
    d.note_scar = args
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    d.has_note = d.note_scar.is_some();
    changed
}

/// Requested traits that are new against the prior list (case-insensitive
/// dedup, no within-batch repeats).
fn newly_added_traits(requested: &[String], existing: &[String]) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    for t in requested {
        let lc = t.to_lowercase();
        let known = existing.iter().any(|x| x.to_lowercase() == lc)
            || added.iter().any(|x| x.to_lowercase() == lc);
        if !known {
            added.push(t.clone());
        }
    }
    added
}

/// Delta echo (stage 4): compact final lists of the interlocutor model
/// instead of a full render_full (a full read — get_self_model's job). The
/// lists are short by construction (merge with dedup), so we show them in full.
fn update_user_model_echo(
    loc: &Locale,
    model: &SelfModel,
    d: &UserModelEditDeltas,
    dup_pairs: &[(String, String)],
) -> String {
    let mut msg = loc.t("tool.update_user_model.result.updated").to_string();
    let u = &model.user_model;
    if !u.perceived_traits.is_empty() {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_user_model.traits",
            &[("list", &u.perceived_traits.join(", "))],
        ));
    }
    if !u.current_interests.is_empty() {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_user_model.interests",
            &[("list", &u.current_interests.join(", "))],
        ));
    }
    if !u.relationship_dynamic.trim().is_empty() {
        msg.push('\n');
        msg.push_str(&loc.tf(
            "tool.update_user_model.relationship",
            &[("dyn", u.relationship_dynamic.trim())],
        ));
    }
    // Confirmation of the revision scar (if `note` was passed) — its text is visible.
    if let Some(scar) = &d.note_scar {
        msg.push('\n');
        msg.push_str(&loc.tf("tool.update_user_model.scar_saved", &[("scar", scar)]));
    }
    // The related-traits gate (Step C): a topically close trait already
    // exists. bge-m3 groups traits by dimension (paraphrases AND antonyms), so
    // we ask the model to DECIDE: is this a duplicate (merge via
    // remove_traits) or a contradiction (record via add_insight) — a mirror
    // of the add_insight gate, but over the flat trait list (integration
    // instead of accumulation).
    if !dup_pairs.is_empty() {
        msg.push('\n');
        msg.push_str(loc.t("tool.update_user_model.gate.related"));
        for (added, existing) in dup_pairs {
            msg.push_str(&format!("\n- «{added}» ≈ «{existing}»"));
        }
    }
    // Removing a trait/interest or replacing a non-empty dynamic — a revision
    // of judgment. No reason recorded → remind to leave a trace as an
    // observation (a scar) rather than changing it silently (a shift in
    // relationship dynamic is the most significant revision of the
    // interlocutor model).
    if (d.removed_traits || d.removed_interests || d.replaced_dynamic) && !d.has_note {
        msg.push('\n');
        msg.push_str(loc.t("tool.update_user_model.nudge_note"));
    }
    msg
}

// `consolidate_narrative` was removed: observations moved into notes (Tier 1),
// consolidated by note tools (note_revise/note_supersede/note_merge) — they're
// stronger (replacement with a "scar" rather than deletion by id). See
// docs/history/narrative-as-notes.md.

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    /// The profile's self-notes (observations) — newest first. Observations moved
    /// into notes (@self), so we check them there, not in the model blob.
    fn self_notes(
        storage: &crate::shared::storage::Storage,
        profile: Uuid,
    ) -> Vec<crate::entities::note::Note> {
        storage
            .db()
            .note_list(profile, None, &[notes::SELF_NOTE_TAG.to_string()], None)
            .unwrap()
    }

    #[test]
    fn self_model_tool_descriptions_are_localized() {
        // Every "self-model" tool returns DIFFERENT text on ru/en (catches a
        // forgotten `_loc`), en — no Cyrillic. §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let (r, e) = (locale(Lang::Ru), locale(Lang::En));
        let no_cyr = |s: &str| {
            !s.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
        };
        let pairs: Vec<(String, String)> = vec![
            (GetSelfModel.description(r), GetSelfModel.description(e)),
            (Reflect.description(r), Reflect.description(e)),
            (AddInsight.description(r), AddInsight.description(e)),
            (
                UpdateSelfModel.description(r),
                UpdateSelfModel.description(e),
            ),
            (
                UpdateUserModel.description(r),
                UpdateUserModel.description(e),
            ),
        ];
        for (ru_d, en_d) in pairs {
            assert_ne!(ru_d, en_d, "description not localized: {ru_d}");
            assert!(no_cyr(&en_d), "Cyrillic in en description: {en_d}");
        }
    }

    #[tokio::test]
    async fn add_insight_result_localized_for_all_langs() {
        // Confirmation of recording an observation renders in every built-in language.
        use crate::shared::i18n::{Lang, locale};
        for &lang in Lang::ALL {
            let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
            ctx.loc = locale(lang);
            let out = AddInsight
                .invoke(&ctx, serde_json::json!({"text": "hello observation"}))
                .await
                .unwrap();
            // The result starts with the localized "recorded (id=…)" template.
            let prefix = locale(lang)
                .t("tool.add_insight.result.recorded")
                .split("{id}")
                .next()
                .unwrap();
            assert!(out.result.starts_with(prefix), "{lang:?}: {}", out.result);
        }
    }

    #[test]
    fn is_self_model_tool_recognizes_group() {
        assert!(is_self_model_tool(UPDATE_SELF_MODEL_ID));
        assert!(is_self_model_tool(ADD_INSIGHT_ID));
        assert!(is_self_model_tool(GET_SELF_MODEL_ID));
        assert!(!is_self_model_tool("note_save"));
        assert!(!is_self_model_tool("web_search"));
    }

    /// The reference locale (ru) — asserts on Russian substrings pin the ru bundle.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn maintenance_protocol_wraps_policy_core() {
        let p = maintenance_protocol(ru());
        // The "you manage this yourself" wrapper + the whole POLICY_CORE (with the
        // key anti-flattery phrase).
        assert!(p.contains("Ты сам ведёшь эту «модель себя»"));
        assert!(p.contains(policy_core(ru())));
        assert!(p.contains("угодливости"));
    }

    /// Per-language (§3.5): the maintenance protocol wraps `policy_core` of the
    /// same language on every built-in language; the `{core}` placeholder is substituted.
    #[test]
    fn maintenance_protocol_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let p = maintenance_protocol(l);
            assert!(
                p.contains(policy_core(l)),
                "{lang:?}: policy_core not embedded"
            );
            assert!(
                !p.contains("{core}"),
                "{lang:?}: placeholder not substituted"
            );
        }
    }

    #[test]
    fn policy_core_routes_events_to_insights() {
        // Stage 1 (summary — a snapshot, not a chronicle): the genre boundary runs
        // along the axis "state → summary, event/conclusion → add_insight (even durable)".
        let core = policy_core(ru());
        assert!(core.contains("снимок"));
        assert!(core.contains("СОКРАЩАЙ"));
        assert!(core.contains("ДАЖЕ ЕСЛИ"));
        // The explicit "read it in full before editing" step (protection against
        // editing from a truncated view).
        assert!(core.contains("прочти его целиком через get_self_model"));
    }

    #[test]
    fn policy_core_nudges_interest_aging() {
        // A3-light: a nudge on interest aging via the existing remove_interests.
        let core = policy_core(ru());
        assert!(core.contains("remove_interests"));
        assert!(core.contains("устаревал"));
        assert!(core.contains("давно не"));
    }

    #[test]
    fn update_self_model_description_routes_events_to_insights() {
        // The tool description routes event-like conclusions into add_insight,
        // while summary stays a compact snapshot.
        let d =
            UpdateSelfModel.description(crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru));
        assert!(d.contains("снимок"));
        assert!(d.contains("add_insight"));
    }

    #[tokio::test]
    async fn get_on_empty_reports_empty() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("пуста"));
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn update_summary_and_goal_persists() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);

        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"summary": "ценю ясность", "add_goals": ["помочь с проектом"]}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("обновлена"));

        // Recorded in storage under this profile.
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.summary, "ценю ясность");
        assert_eq!(stored.active_goals().count(), 1);

        // get_self_model reflects the record.
        let got = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(got.result.contains("ценю ясность"));
        assert!(got.result.contains("помочь с проектом"));
    }

    #[tokio::test]
    async fn update_summary_echo_shows_size() {
        // Stage 2: the summary-edit echo always carries a size line (feedback on growth).
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"summary": "ценю ясность"}))
            .await
            .unwrap();
        assert!(out.result.contains("Описание:"));
        assert!(out.result.contains("симв."));
        // An edit with no summary (goal only) — no size line.
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Описание:"));
    }

    #[tokio::test]
    async fn update_self_model_echo_is_delta_not_full() {
        // Stage 4: the edit echo carries deltas (#id of the added goal), but NOT
        // the full summary text (a full read — only get_self_model's job).
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "summary": "УНИКАЛЬНЫЙ_МАРКЕР_ОПИСАНИЯ",
                    "add_goals": ["новая цель"]
                }),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Добавлены цели:"));
        assert!(out.result.contains("новая цель"));
        assert!(out.result.contains('#'));
        assert!(out.result.contains("Описание:")); // the size line (stage 2)
        // The summary text doesn't make it into the echo (no "anchoring" on the
        // essay genre).
        assert!(!out.result.contains("УНИКАЛЬНЫЙ_МАРКЕР_ОПИСАНИЯ"));
    }

    #[tokio::test]
    async fn update_self_model_echo_shows_closed_goal_id() {
        // Stage 4: closing a goal is reflected in the echo by its #id (the same
        // handle used to close it).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;
        let short = id.simple().to_string()[..6].to_string();
        let out = UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [format!("#{short}")]}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Закрыты выполненными:"));
        assert!(out.result.contains(&format!("#{short}")));
    }

    #[tokio::test]
    async fn update_user_model_echo_shows_final_lists() {
        // Stage 4: the echo shows compact final lists, not a full render_full.
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "add_traits": ["скептик"],
                    "add_interests": ["Rust"],
                    "relationship_dynamic": "рабочие"
                }),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Черты теперь: скептик"));
        assert!(out.result.contains("Интересы теперь: Rust"));
        assert!(out.result.contains("Отношения: рабочие"));
    }

    #[tokio::test]
    async fn get_self_model_surfaces_summary_fill_hint_over_target() {
        // Stage 2: a bloated description (over the target) raises a hint in the read.
        use crate::entities::self_model::SelfModelParams;
        use crate::shared::config::SelfModelSettings;
        let profile = Uuid::new_v4();
        let (_d, _s, mut ctx) = ctx_with_storage(profile);
        // The target 5 is sanitized to the 200 floor — take a description longer
        // than 200 characters.
        ctx.self_model_params = SelfModelParams::from_settings(&SelfModelSettings {
            summary_target_chars: 5,
            ..SelfModelSettings::default()
        });
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"summary": "я".repeat(250)}))
            .await
            .unwrap();
        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Описание себя разрослось"));
    }

    #[tokio::test]
    async fn complete_goal_by_id() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["старая цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;

        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [id.to_string()]}),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.active_goals().count(), 0);
    }

    #[tokio::test]
    async fn update_user_model_persists() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "add_traits": ["скептичный", "глубокий"],
                    "relationship_dynamic": "рабочие"
                }),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
        assert_eq!(stored.user_model.relationship_dynamic, "рабочие");
    }

    #[tokio::test]
    async fn update_user_model_merges_not_overwrites() {
        // The key fix: an edit doesn't overwrite the prior data ("overwritten by mood" bug).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["добрый"]}))
            .await
            .unwrap();
        // A second edit in a different "mood" — adds, doesn't replace.
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["прямолинейный"]}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
        assert!(
            stored
                .user_model
                .perceived_traits
                .contains(&"добрый".to_string())
        );
        // remove_traits removes precisely.
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"remove_traits": ["добрый"]}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(
            stored.user_model.perceived_traits,
            vec!["прямолинейный".to_string()]
        );
    }

    #[tokio::test]
    async fn complete_goal_by_short_id() {
        // The model references a goal by the short #id from get_self_model.
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"add_goals": ["цель"]}))
            .await
            .unwrap();
        let id = storage.db().self_model_get(profile).unwrap().unwrap().goals[0].id;
        let short = id.simple().to_string()[..6].to_string();

        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"complete_goals": [format!("#{short}")]}),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.active_goals().count(), 0);

        // A nonexistent #id — a clear report, not a panic.
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"complete_goals": ["#zzzzzz"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Не найдены цели"));
    }

    #[tokio::test]
    async fn update_with_nothing_is_noop() {
        let (_d, storage, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = UpdateSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Нечего обновлять"));
        // Nothing was recorded.
        assert!(
            storage
                .db()
                .self_model_get(ctx.profile_id)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn reflect_returns_current_and_rubric() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        let out = Reflect.invoke(&ctx, serde_json::json!({})).await.unwrap();
        assert!(out.result.contains("Вопросы для размышления"));
        // Stage 1: the rubric asks about the self-description's growth.
        assert!(out.result.contains("Не разрослось ли описание себя"));
        assert!(out.effects.is_empty());
        // With no observations (< 2) the self-consolidation overview isn't mixed in.
        assert!(!out.result.contains("Обзор наблюдений"));
    }

    #[tokio::test]
    async fn reflect_includes_self_consolidation_overview() {
        // Tier 3: with ≥2 observations, reflect mixes in the self-consolidation
        // overview (concrete similar pairs / links / no-links under the rubric).
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaaa bbbb"}))
            .await
            .unwrap();
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaab"}))
            .await
            .unwrap();
        let out = Reflect.invoke(&ctx, serde_json::json!({})).await.unwrap();
        assert!(out.result.contains("Обзор наблюдений"));
        assert!(out.result.contains("Похожие пары"));
    }

    #[tokio::test]
    async fn add_insight_creates_self_note_and_shows() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        let out = AddInsight
            .invoke(
                &ctx,
                serde_json::json!({"text": "напряжение между краткостью и полнотой"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("Наблюдение записано"));
        // Recorded as a self-note (@self), not in the model blob (there is none).
        assert_eq!(self_notes(&storage, profile).len(), 1);
        assert!(storage.db().self_model_get(profile).unwrap().is_none());

        // get_self_model shows the observation (assembled from notes).
        let got = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(got.result.contains("Наблюдения"));
        assert!(got.result.contains("напряжение"));

        // Empty text — an error, nothing created.
        assert!(
            AddInsight
                .invoke(&ctx, serde_json::json!({"text": "  "}))
                .await
                .is_err()
        );
        assert_eq!(self_notes(&storage, profile).len(), 1);
    }

    #[tokio::test]
    async fn add_insight_gate_surfaces_similar_observation() {
        // Tier 1's core hypothesis: a near-duplicate observation surfaces the gate
        // (a similar existing self-note) with a hint to rewrite via note_revise.
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaaa bbbb"}))
            .await
            .unwrap();
        let out = AddInsight
            .invoke(&ctx, serde_json::json!({"text": "aaab"}))
            .await
            .unwrap();
        assert!(out.result.contains("Похожие наблюдения"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("note_revise"));
    }

    #[tokio::test]
    async fn get_self_model_surfaces_linked_observations() {
        // Tier 2: get_self_model shows links between observations (the graph).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        AddInsight
            .invoke(&ctx, serde_json::json!({"text": "ценю краткость"}))
            .await
            .unwrap();
        AddInsight
            .invoke(
                &ctx,
                serde_json::json!({"text": "иногда бываю многословен"}),
            )
            .await
            .unwrap();
        let ns = self_notes(&storage, profile);
        storage
            .db()
            .note_link_insert(profile, ns[0].id, ns[1].id, "contradicts")
            .unwrap();

        let out = GetSelfModel
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("Связи наблюдений"));
        assert!(out.result.contains("contradicts"));
    }

    #[tokio::test]
    async fn add_trait_gate_surfaces_near_duplicate() {
        // Step C: adding a trait related to an existing one raises the gate
        // (a mirror of the add_insight gate). MockEmbedder(16) — a bag of
        // characters: "aaaa bbbb" ↔ "aaab" are close (cosine ≈ 0.89 > the 0.72 threshold).
        let profile = Uuid::new_v4();
        let (_d, _s, ctx) = ctx_with_storage(profile);
        // The first trait — no prior ones, the gate stays silent.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Родственные черты"));
        // The second trait is close to the first → the gate shows the related trait.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaab"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Родственные черты"));
        assert!(out.result.contains("aaab"));
        assert!(out.result.contains("aaaa bbbb"));
        assert!(out.result.contains("remove_traits"));
    }

    #[tokio::test]
    async fn add_trait_gate_follows_the_calibrated_scale() {
        // TRAIT_SIMILARITY is a position in bge-m3's scale, not an absolute
        // cosine. On `multilingual-e5-large-instruct` (range 2.6× narrower,
        // research §8.2) the same intent sits at ~0.908 — so a pair the raw
        // constant calls related no longer is, while a closer one still is.
        // MockEmbedder(16), a bag of characters: "aaaa bbbb" ↔ "aaab" = 0.894,
        // "aaab" ↔ "aaaab" = 0.997.
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        storage
            .db()
            .set_embed_calibration(&crate::shared::embed_calibration::Calibration {
                unrelated: 0.7897,
                paraphrase: 0.9456,
            })
            .unwrap();

        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        // 0.894 clears the raw 0.72 gate (that is what
        // `add_trait_gate_surfaces_near_duplicate` pins, uncalibrated) but not
        // the mapped 0.908 one.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaab"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Родственные черты"), "{}", out.result);

        // 0.997 clears it — the gate moved, it did not switch off.
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaab"]}))
            .await
            .unwrap();
        assert!(out.result.contains("Родственные черты"), "{}", out.result);
        assert!(out.result.contains("aaab"), "{}", out.result);
    }

    #[tokio::test]
    async fn add_trait_gate_silent_for_dissimilar() {
        // An unrelated trait doesn't raise the gate (no false positives).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["aaaa bbbb"]}))
            .await
            .unwrap();
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["wwww"]}))
            .await
            .unwrap();
        assert!(!out.result.contains("Родственные черты"));
        // Both traits are kept (the gate blocks nothing — only warns).
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.user_model.perceived_traits.len(), 2);
    }

    #[tokio::test]
    async fn removing_trait_with_note_leaves_scar_as_self_note() {
        // Trait revision with a note leaves a trace — now as a self-note (observation).
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"add_traits": ["компетентный одиночка"]}),
            )
            .await
            .unwrap();
        UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "remove_traits": ["компетентный одиночка"],
                    "add_traits": ["в команде раскрывается при доверии"],
                    "note": "Пересмотрел: раньше видел одиночкой, но в команде при доверии он силён"
                }),
            )
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        // The trait is replaced, and the reason is preserved as a self-note (not
        // in the blob, not erased).
        assert_eq!(
            stored.user_model.perceived_traits,
            vec!["в команде раскрывается при доверии".to_string()]
        );
        assert!(stored.narrative.is_empty());
        let n = self_notes(&storage, profile);
        assert_eq!(n.len(), 1);
        assert!(n[0].content.contains("Пересмотрел"));
    }

    #[tokio::test]
    async fn removing_trait_without_note_nudges_for_scar() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        UpdateUserModel
            .invoke(&ctx, serde_json::json!({"add_traits": ["скептик"]}))
            .await
            .unwrap();
        let out = UpdateUserModel
            .invoke(&ctx, serde_json::json!({"remove_traits": ["скептик"]}))
            .await
            .unwrap();
        assert!(out.result.contains("без пояснения"));
        assert!(out.result.contains("note"));
    }

    #[tokio::test]
    async fn replacing_nonempty_dynamic_without_note_nudges() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        // Initial population of the dynamic — WITHOUT a reminder (not a revision).
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"relationship_dynamic": "доверительные"}),
            )
            .await
            .unwrap();
        assert!(!out.result.contains("без пояснения"));
        // Replacing a non-empty dynamic without a note — a reminder about the scar.
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({"relationship_dynamic": "натянутые"}),
            )
            .await
            .unwrap();
        assert!(out.result.contains("без пояснения"));
        // With a note — no reminder, and the reason goes out as an observation
        // (visible in the echo).
        let out = UpdateUserModel
            .invoke(
                &ctx,
                serde_json::json!({
                    "relationship_dynamic": "снова тёплые",
                    "note": "помирились после спора"
                }),
            )
            .await
            .unwrap();
        assert!(!out.result.contains("без пояснения"));
        assert!(out.result.contains("помирились после спора"));
    }

    #[tokio::test]
    async fn update_self_model_folds_old_closed_goals_into_notes() {
        use crate::entities::self_model::SelfModelParams;
        use crate::shared::config::SelfModelSettings;
        let profile = Uuid::new_v4();
        let (_d, storage, mut ctx) = ctx_with_storage(profile);
        // Keep no more than 2 closed goals.
        ctx.self_model_params = SelfModelParams::from_settings(&SelfModelSettings {
            max_closed_goals: 2,
            ..SelfModelSettings::default()
        });
        UpdateSelfModel
            .invoke(
                &ctx,
                serde_json::json!({"add_goals": ["ц0", "ц1", "ц2", "ц3", "ц4"]}),
            )
            .await
            .unwrap();
        let ids: Vec<String> = storage
            .db()
            .self_model_get(profile)
            .unwrap()
            .unwrap()
            .goals
            .iter()
            .map(|g| g.id.to_string())
            .collect();
        // Close all five — folding will keep the 2 most-recently-closed, 3 → to
        // the archive (now as self-notes, not in the blob).
        UpdateSelfModel
            .invoke(&ctx, serde_json::json!({"complete_goals": ids}))
            .await
            .unwrap();
        let stored = storage.db().self_model_get(profile).unwrap().unwrap();
        assert_eq!(stored.goals.len(), 2); // the closed-goal cap is respected
        assert!(stored.narrative.is_empty());
        // Three folded goals — self-notes "[goal archive]".
        let archived = self_notes(&storage, profile)
            .into_iter()
            .filter(|n| n.content.starts_with("[архив цели]"))
            .count();
        assert_eq!(archived, 3);
    }
}
