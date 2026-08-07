//! The agent's "self-model" — a per-profile representation of itself, its goals,
//! and the interlocutor. Lives in SQLite (like notes/RAG), isolated by `profile_id`.
//! A minimal MVP probe: free-form text + goals + a user model, no numeric
//! "belief strengths". See [docs/history/self-model-mvp.md](../../docs/history/self-model-mvp.md).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::config::SelfModelSettings;
use crate::shared::i18n::Locale;

/// Render/storage parameters for the "self-model" (from `config.self_model`). Passed
/// into the entity's methods instead of hardcoded constants, so the user can tune
/// the narrative size and the prompt-injection volume. An analogue of
/// [`ChunkParams`](crate::features::tools::rag::ChunkParams) for RAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfModelParams {
    /// How many recent observations (self-notes) to pull in for a full model read
    /// (`get_self_model`/`reflect`). Observations moved into notes — there's no
    /// longer a FIFO storage cap; the parameter only bounds the read size. The
    /// historical field name is kept for `settings.json` compatibility.
    pub max_narrative: usize,
    /// How many fresh observations go into the system prompt (injection).
    pub narrative_in_prompt: usize,
    /// The character ceiling for rendering the model into the system prompt.
    pub prompt_cap: usize,
    /// How many closed goals to keep in the structure (the oldest beyond this — into
    /// a narrative scar).
    pub max_closed_goals: usize,
    /// A size target for the self-description (summary): beyond it,
    /// [`SelfModel::summary_fill_hint`] returns a soft hint to shorten it. A gate,
    /// not a ceiling.
    pub summary_target_chars: usize,
}

impl Default for SelfModelParams {
    fn default() -> Self {
        Self::from_settings(&SelfModelSettings::default())
    }
}

impl SelfModelParams {
    /// Builds parameters from settings, sanitizing values (protection against
    /// zeros and inconsistency: at least 1 insight is stored, no more goes into the
    /// prompt than is stored, a readable minimum prompt character count).
    pub fn from_settings(s: &SelfModelSettings) -> Self {
        let max_narrative = s.max_narrative.max(1);
        Self {
            max_narrative,
            narrative_in_prompt: s.narrative_in_prompt.min(max_narrative),
            prompt_cap: s.prompt_cap.max(100),
            max_closed_goals: s.max_closed_goals.max(1),
            summary_target_chars: s.summary_target_chars.max(200),
        }
    }
}

/// The agent's representation of itself (one instance per profile).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfModel {
    pub profile_id: Uuid,
    /// Grows on every save — a rough indicator of "how much it's changed".
    #[serde(default)]
    pub version: u64,
    /// Free-form "about me" text (who I am, what I value, how I behave).
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub user_model: UserModel,
    /// A "self over time" narrative: short insights/observations (including noticed
    /// contradictions — plain prose, no separate type). Append-only with a cap.
    #[serde(default)]
    pub narrative: Vec<NarrativeSegment>,
    pub updated_at: DateTime<Utc>,
}

/// A narrative fragment: a short observation/insight of the agent, timestamped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NarrativeSegment {
    pub id: Uuid,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

/// A long-term goal/intention of the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub id: Uuid,
    pub description: String,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
    /// When the goal left `Active` (for a closed goal's age and folding old closed
    /// ones). `None` for active goals and old records (no migration —
    /// `#[serde(default)]`). A closed goal's age is counted from it, otherwise from
    /// `created_at`.
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Completed,
    Abandoned,
}

/// How many recent completed/stale goals to show on a full read (`render_full`) —
/// the lifecycle is visible, but the list doesn't grow without bound.
const CLOSED_GOALS_SHOWN: usize = 5;

/// The result of resolving a goal reference by its "handle" (a short `#id` or a
/// full UUID). See [`SelfModel::match_goal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalMatch {
    /// A goal was found unambiguously.
    One(Uuid),
    /// No matches.
    None,
    /// The prefix is ambiguous (several goals matched).
    Ambiguous,
}

/// A short human-readable goal id: the first 6 hex characters of the UUID. Shown in
/// reads and accepted by `complete_goals`/`abandon_goals` (a full UUID too). At a
/// handful of goals, a collision is virtually impossible, and `match_goal` catches
/// ambiguity regardless.
fn short_hex(id: &Uuid) -> String {
    id.simple().to_string()[..6].to_string()
}

/// A public short id for tool echoes (the first 6 hex chars of the UUID, the same
/// format as `#id` in `render_full` reads). A wrapper over [`short_hex`] — so the
/// edit-delta echo (stage 4, docs/summary-as-snapshot.md) refers to goals by the
/// same handles they're closed with.
pub fn short_id(id: &Uuid) -> String {
    short_hex(id)
}

/// Assigns a trimmed string field; `false` (no change) when the trimmed value
/// is already there.
fn assign_trimmed(slot: &mut String, value: &str) -> bool {
    let value = value.trim().to_string();
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// Replaces a list wholesale; `false` when the new list is identical.
fn assign_list(slot: &mut Vec<String>, value: Vec<String>) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// Sets/clears a goal's `closed_at` based on its current status: on leaving
/// `Active` — stamps the moment (if not already stamped), on returning to `Active`
/// — clears it.
fn stamp_closed(g: &mut Goal) {
    match g.status {
        GoalStatus::Active => g.closed_at = None,
        _ => {
            if g.closed_at.is_none() {
                g.closed_at = Some(Utc::now());
            }
        }
    }
}

/// A coarse human-readable age label for a record (day granularity). Within one
/// day the text is stable — so the "self-model" injection into the system prompt
/// doesn't change turn to turn (the local model's prefix cache suffers no more
/// than once a day, beyond actual model edits). A negative difference (hours ahead
/// due to clock skew) is treated as "today".
fn age_label(at: DateTime<Utc>, now: DateTime<Utc>, loc: &Locale) -> String {
    let days = (now - at).num_days();
    match days {
        d if d <= 0 => loc.t("selfmodel.age.today").to_string(),
        1 => loc.t("selfmodel.age.yesterday").to_string(),
        2..=6 => loc.tf("selfmodel.age.days", &[("n", &days.to_string())]),
        7..=30 => loc.tf("selfmodel.age.weeks", &[("n", &(days / 7).to_string())]),
        31..=364 => loc.tf("selfmodel.age.months", &[("n", &(days / 30).to_string())]),
        _ => loc.tf("selfmodel.age.years", &[("n", &(days / 365).to_string())]),
    }
}

/// Resolves a "handle" (a full UUID or a short hex prefix, with or without a
/// leading `#`; case-insensitive) among a set of ids. Shared logic for goals and
/// observations.
fn resolve_handle(handle: &str, ids: &[Uuid]) -> GoalMatch {
    let h = handle.trim().trim_start_matches('#').to_lowercase();
    if h.is_empty() {
        return GoalMatch::None;
    }
    // A full UUID (with or without dashes).
    if let Ok(u) = Uuid::parse_str(&h) {
        return if ids.contains(&u) {
            GoalMatch::One(u)
        } else {
            GoalMatch::None
        };
    }
    // Otherwise — a prefix of the id's hex representation (the first `simple()` characters).
    let mut found: Option<Uuid> = None;
    for id in ids {
        if id.simple().to_string().starts_with(&h) {
            if found.is_some() {
                return GoalMatch::Ambiguous;
            }
            found = Some(*id);
        }
    }
    found.map_or(GoalMatch::None, GoalMatch::One)
}

/// A manual edit to the "self-model" from the UI editor (`F3`). Applied by the
/// entity ([`SelfModel::apply_edit`]); the orchestrator saves it. UI↔orchestrator contract.
#[derive(Debug, Clone, PartialEq)]
pub enum SelfModelEdit {
    /// Replace the short self-description.
    SetSummary(String),
    /// Add a new active goal.
    AddGoal(String),
    /// Change a goal's text by id.
    SetGoalText { id: Uuid, text: String },
    /// Cycle a goal's status (Active→Completed→Abandoned→Active).
    CycleGoalStatus(Uuid),
    /// Delete a goal by id.
    DeleteGoal(Uuid),
    /// Replace the list of perceived interlocutor traits.
    SetTraits(Vec<String>),
    /// Replace the list of the interlocutor's current interests.
    SetInterests(Vec<String>),
    /// Replace the relationship-dynamic description.
    SetRelationship(String),
    /// Delete a narrative insight by id.
    DeleteInsight(Uuid),
    /// Clear the whole model (description/goals/interlocutor/narrative).
    Clear,
}

/// The agent's representation of the interlocutor (free-form lists/text, no id).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserModel {
    #[serde(default)]
    pub perceived_traits: Vec<String>,
    #[serde(default)]
    pub current_interests: Vec<String>,
    #[serde(default)]
    pub relationship_dynamic: String,
}

impl SelfModel {
    /// An empty model for a profile.
    pub fn new(profile_id: Uuid) -> Self {
        Self {
            profile_id,
            version: 0,
            summary: String::new(),
            goals: Vec::new(),
            user_model: UserModel::default(),
            narrative: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    /// Whether the **structural** part of the model is empty: an empty description,
    /// no active goals (only those are rendered), and an empty interlocutor model.
    /// The narrative is **not accounted for** here — it moved into notes (`@self`,
    /// see docs/history/narrative-as-notes.md) and is passed into the render via
    /// the `recent` parameter; the caller checks for observations
    /// (`is_empty() && recent.is_empty()`). Completed goals alone don't make an
    /// otherwise "empty" model informative.
    pub fn is_empty(&self) -> bool {
        self.summary.trim().is_empty()
            && self.active_goals().next().is_none()
            && self.user_model.is_empty()
    }

    /// Active goals (for rendering/reading).
    pub fn active_goals(&self) -> impl Iterator<Item = &Goal> {
        self.goals.iter().filter(|g| g.status == GoalStatus::Active)
    }

    /// Adds a new active goal.
    pub fn add_goal(&mut self, description: impl Into<String>) {
        let description = description.into().trim().to_string();
        if description.is_empty() {
            return;
        }
        self.goals.push(Goal {
            id: Uuid::new_v4(),
            description,
            status: GoalStatus::Active,
            created_at: Utc::now(),
            closed_at: None,
        });
    }

    /// Sets a goal's status (by id). Returns `true` if the goal was found.
    /// Sets/clears `closed_at` on leaving `Active`/reactivation.
    pub fn set_goal_status(&mut self, id: Uuid, status: GoalStatus) -> bool {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.status = status;
            stamp_closed(g);
            true
        } else {
            false
        }
    }

    /// Cycles a goal's status Active→Completed→Abandoned→Active (by id).
    /// Returns `true` if the goal was found.
    pub fn cycle_goal_status(&mut self, id: Uuid) -> bool {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.status = match g.status {
                GoalStatus::Active => GoalStatus::Completed,
                GoalStatus::Completed => GoalStatus::Abandoned,
                GoalStatus::Abandoned => GoalStatus::Active,
            };
            stamp_closed(g);
            true
        } else {
            false
        }
    }

    /// Applies a manual edit from the UI editor (`F3`). Returns `true` if the model
    /// changed (whether the orchestrator should save it). Pure logic — testable
    /// without an orchestrator. Trait/interest lists are replaced wholesale.
    pub fn apply_edit(&mut self, edit: SelfModelEdit) -> bool {
        match edit {
            SelfModelEdit::SetSummary(s) => assign_trimmed(&mut self.summary, &s),
            SelfModelEdit::AddGoal(desc) => {
                let before = self.goals.len();
                self.add_goal(desc);
                self.goals.len() != before
            }
            SelfModelEdit::SetGoalText { id, text } => self.set_goal_text(id, &text),
            SelfModelEdit::CycleGoalStatus(id) => self.cycle_goal_status(id),
            SelfModelEdit::DeleteGoal(id) => {
                let before = self.goals.len();
                self.goals.retain(|g| g.id != id);
                self.goals.len() != before
            }
            SelfModelEdit::SetTraits(v) => assign_list(&mut self.user_model.perceived_traits, v),
            SelfModelEdit::SetInterests(v) => {
                assign_list(&mut self.user_model.current_interests, v)
            }
            SelfModelEdit::SetRelationship(s) => {
                assign_trimmed(&mut self.user_model.relationship_dynamic, &s)
            }
            SelfModelEdit::DeleteInsight(id) => {
                let before = self.narrative.len();
                self.narrative.retain(|n| n.id != id);
                self.narrative.len() != before
            }
            SelfModelEdit::Clear => self.clear_all(),
        }
    }

    /// [`SelfModelEdit::SetGoalText`]: renames a goal by id. An empty or
    /// unchanged text, or an unknown id, is a no-op (`false`).
    fn set_goal_text(&mut self, id: Uuid, text: &str) -> bool {
        let text = text.trim().to_string();
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            if text.is_empty() || g.description == text {
                return false;
            }
            g.description = text;
            true
        } else {
            false
        }
    }

    /// [`SelfModelEdit::Clear`]: wipes every part of the model; `false` when
    /// there was nothing to wipe.
    fn clear_all(&mut self) -> bool {
        if self.is_empty() && self.summary.is_empty() && self.goals.is_empty() {
            return false;
        }
        self.summary.clear();
        self.goals.clear();
        self.user_model = UserModel::default();
        self.narrative.clear();
        true
    }

    /// Folds old closed goals into "scar" observations and removes them from
    /// `goals`, keeping no more than `keep` of the most recently closed (by
    /// `closed_at`/`created_at`). **Returns** the scar texts ("[goal archive] …") —
    /// the caller records them as self-notes (observations moved into notes, see
    /// docs/history/narrative-as-notes.md). Doesn't touch active goals. This is
    /// integration, not loss: a closed goal leaves as a scar observation rather
    /// than being silently deleted.
    pub fn fold_closed_goals(&mut self, keep: usize, loc: &Locale) -> Vec<String> {
        let freshness = |g: &Goal| g.closed_at.unwrap_or(g.created_at);
        let mut closed: Vec<(Uuid, DateTime<Utc>)> = self
            .goals
            .iter()
            .filter(|g| g.status != GoalStatus::Active)
            .map(|g| (g.id, freshness(g)))
            .collect();
        if closed.len() <= keep {
            return Vec::new();
        }
        closed.sort_by_key(|(_, at)| std::cmp::Reverse(*at)); // newest first
        let fold_ids: std::collections::HashSet<Uuid> =
            closed[keep..].iter().map(|(id, _)| *id).collect();

        let scars: Vec<String> = self
            .goals
            .iter()
            .filter(|g| fold_ids.contains(&g.id))
            .map(|g| {
                let verb = loc.t(match g.status {
                    GoalStatus::Completed => "selfmodel.status.completed",
                    GoalStatus::Abandoned => "selfmodel.status.abandoned",
                    GoalStatus::Active => "selfmodel.status.active",
                });
                loc.tf(
                    "selfmodel.goal_archive",
                    &[("verb", verb), ("text", g.description.trim())],
                )
            })
            .collect();
        self.goals.retain(|g| !fold_ids.contains(&g.id));
        scars
    }

    /// A soft hint about a bloated self-description: `None` while `summary` stays
    /// within the target `target`; otherwise text with the current size and the
    /// target, directing event-like content into observations. A direct analogue
    /// of the former `narrative_fill_hint`, but for `summary` — the one organ that
    /// had no size feedback. A gate, not a ceiling: doesn't truncate or block
    /// anything. See docs/summary-as-snapshot.md (stage 2).
    pub fn summary_fill_hint(&self, target: usize, loc: &Locale) -> Option<String> {
        let n = self.summary.chars().count();
        (n > target).then(|| {
            loc.tf(
                "selfmodel.summary_hint",
                &[("n", &n.to_string()), ("target", &target.to_string())],
            )
        })
    }

    /// A compact human-readable block for system-prompt injection.
    /// `None` if the structural part is empty **and** there are no observations.
    /// Observations (`recent` — self-notes, newest first, prepared by the caller)
    /// go into the block, `narrative_in_prompt` freshest ones; the result is
    /// truncated to `max_chars` characters. `now` — the reference point for age
    /// labels (day granularity, stable within a day — see [`age_label`]).
    pub fn render_for_prompt(
        &self,
        max_chars: usize,
        narrative_in_prompt: usize,
        now: DateTime<Utc>,
        recent: &[NarrativeSegment],
        loc: &Locale,
    ) -> Option<String> {
        if self.is_empty() && recent.is_empty() {
            return None;
        }
        let mut out = format!("{}\n", loc.t("selfmodel.render.header"));
        if !self.summary.trim().is_empty() {
            out.push_str(loc.t("selfmodel.render.about"));
            // A per-section budget (stage 3): the description gets no more than half
            // the limit, so a bloated summary doesn't crowd goals/interlocutor/
            // observations out of the injection. The final truncation of the whole
            // block below remains a safety net. See docs/summary-as-snapshot.md.
            out.push_str(&truncate_chars_word(self.summary.trim(), max_chars / 2));
            out.push('\n');
        }
        let active: Vec<&Goal> = self.active_goals().collect();
        if !active.is_empty() {
            out.push_str(loc.t("selfmodel.render.goals_active"));
            out.push('\n');
            for g in active {
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_prompt",
                    &[
                        ("desc", g.description.trim()),
                        ("age", &age_label(g.created_at, now, loc)),
                    ],
                ));
                out.push('\n');
            }
        }
        render_user_model(&mut out, &self.user_model, loc);
        if !recent.is_empty() && narrative_in_prompt > 0 {
            out.push_str(loc.t("selfmodel.render.observations_recent"));
            out.push('\n');
            for seg in recent.iter().take(narrative_in_prompt) {
                out.push_str(&loc.tf(
                    "selfmodel.item.obs_prompt",
                    &[
                        ("age", &age_label(seg.created_at, now, loc)),
                        ("text", seg.text.trim()),
                    ],
                ));
                out.push('\n');
            }
        }
        Some(truncate_chars(out.trim_end(), max_chars))
    }

    /// Resolves a goal reference by "handle": a full UUID or a short hex prefix
    /// (with or without a leading `#`). Case-insensitive. Searches only among the
    /// model's goals.
    pub fn match_goal(&self, handle: &str) -> GoalMatch {
        let ids: Vec<Uuid> = self.goals.iter().map(|g| g.id).collect();
        resolve_handle(handle, &ids)
    }

    /// A full human-readable read of the model — for tools (`get_self_model`,
    /// `reflect`, an echo after edits). Unlike [`Self::render_for_prompt`] (a
    /// compact injection), **truncates nothing**, shows all observations
    /// (`recent` — self-notes, newest first, prepared by the caller) and goals with
    /// their status. Goals — with a short `#id` (resolved by `update_self_model`'s
    /// resolver); observations — with the **full** id (they're notes, rewritten/
    /// replaced by note_revise/note_supersede using the full id). Marks an empty
    /// model explicitly. See docs/history/self-model-mvp.md, docs/history/narrative-as-notes.md.
    pub fn render_full(
        &self,
        now: DateTime<Utc>,
        recent: &[NarrativeSegment],
        loc: &Locale,
    ) -> String {
        let header = loc.t("selfmodel.render.header");
        let mut out = format!("{header}\n");
        if !self.summary.trim().is_empty() {
            out.push_str(loc.t("selfmodel.render.about"));
            out.push_str(self.summary.trim());
            out.push('\n');
        }
        let active: Vec<&Goal> = self.active_goals().collect();
        let closed: Vec<&Goal> = self
            .goals
            .iter()
            .filter(|g| g.status != GoalStatus::Active)
            .collect();
        if !active.is_empty() || !closed.is_empty() {
            out.push_str(loc.t("selfmodel.render.goals_ref"));
            out.push('\n');
            let active_st = loc.t("selfmodel.status.active");
            for g in &active {
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_full",
                    &[
                        ("id", &short_hex(&g.id)),
                        ("status", active_st),
                        ("age", &age_label(g.created_at, now, loc)),
                        ("text", g.description.trim()),
                    ],
                ));
                out.push('\n');
            }
            // Recent closed ones — compact, newest first. Age — from the moment of
            // closing (`closed_at`), otherwise from creation.
            for g in closed.iter().rev().take(CLOSED_GOALS_SHOWN) {
                let st = loc.t(match g.status {
                    GoalStatus::Completed => "selfmodel.status.completed",
                    GoalStatus::Abandoned => "selfmodel.status.stale",
                    GoalStatus::Active => "selfmodel.status.active",
                });
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_full",
                    &[
                        ("id", &short_hex(&g.id)),
                        ("status", st),
                        (
                            "age",
                            &age_label(g.closed_at.unwrap_or(g.created_at), now, loc),
                        ),
                        ("text", g.description.trim()),
                    ],
                ));
                out.push('\n');
            }
        }
        render_user_model(&mut out, &self.user_model, loc);
        // Observations (self-notes) in full, newest first — no truncation. Full id:
        // an observation is rewritten/replaced by note tools using the full id.
        if !recent.is_empty() {
            out.push_str(&loc.tf(
                "selfmodel.render.observations",
                &[("n", &recent.len().to_string())],
            ));
            out.push('\n');
            for seg in recent {
                out.push_str(&loc.tf(
                    "selfmodel.item.obs_full",
                    &[
                        ("id", &seg.id.to_string()),
                        ("age", &age_label(seg.created_at, now, loc)),
                        ("text", seg.text.trim()),
                    ],
                ));
                out.push('\n');
            }
        }
        let body = out.trim_end();
        if body == header {
            return loc.t("selfmodel.render.empty").to_string();
        }
        body.to_string()
    }
}

/// The "About the interlocutor" block — shared by [`SelfModel::render_for_prompt`]
/// and [`SelfModel::render_full`] (byte-for-byte in both). The `, ` and `;`
/// separators are punctuation, language-neutral and stay in the code; only the
/// label captions are localized.
fn render_user_model(out: &mut String, u: &UserModel, loc: &Locale) {
    if u.is_empty() {
        return;
    }
    out.push_str(loc.t("selfmodel.render.user"));
    if !u.perceived_traits.is_empty() {
        out.push_str(loc.t("selfmodel.render.user.traits"));
        out.push_str(&u.perceived_traits.join(", "));
        out.push(';');
    }
    if !u.current_interests.is_empty() {
        out.push_str(loc.t("selfmodel.render.user.interests"));
        out.push_str(&u.current_interests.join(", "));
        out.push(';');
    }
    if !u.relationship_dynamic.trim().is_empty() {
        out.push_str(loc.t("selfmodel.render.user.relationship"));
        out.push_str(u.relationship_dynamic.trim());
    }
    out.push('\n');
}

impl UserModel {
    pub fn is_empty(&self) -> bool {
        self.perceived_traits.is_empty()
            && self.current_interests.is_empty()
            && self.relationship_dynamic.trim().is_empty()
    }

    /// A compact hint for impersonation (`Ctrl+U`): the agent writes a reply **on
    /// behalf of** the person, and `UserModel` is a model of that person, so mixing
    /// it into the impersonation system prompt makes the voice more accurate. This
    /// is a prompt fragment (text the *model* reads) — localized via `loc` in the
    /// agent-scaffold language (axis A), like [`render_user_model`] above.
    /// `None` if the model is empty; the result is truncated to `max_chars` characters.
    pub fn render_for_impersonation(&self, max_chars: usize, loc: &Locale) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = loc.t("selfmodel.render.impersonation.intro").to_string();
        if !self.perceived_traits.is_empty() {
            out.push_str(loc.t("selfmodel.render.impersonation.traits"));
            out.push_str(&self.perceived_traits.join(", "));
            out.push(';');
        }
        if !self.current_interests.is_empty() {
            out.push_str(loc.t("selfmodel.render.impersonation.interests"));
            out.push_str(&self.current_interests.join(", "));
            out.push(';');
        }
        if !self.relationship_dynamic.trim().is_empty() {
            out.push_str(loc.t("selfmodel.render.impersonation.relationship"));
            out.push_str(self.relationship_dynamic.trim());
        }
        Some(truncate_chars(out.trim_end_matches([';', ' ']), max_chars))
    }

    /// Adds traits (case-insensitive dedup, empty ones dropped). Returns whether
    /// the list changed. **Merge, not replace** — the edit doesn't overwrite prior data.
    pub fn add_traits(&mut self, items: Vec<String>) -> bool {
        merge_into(&mut self.perceived_traits, items)
    }
    /// Removes traits by match (case-insensitive). Returns whether it changed.
    pub fn remove_traits(&mut self, items: &[String]) -> bool {
        remove_from(&mut self.perceived_traits, items)
    }
    /// Adds interests (case-insensitive dedup). Returns whether it changed.
    pub fn add_interests(&mut self, items: Vec<String>) -> bool {
        merge_into(&mut self.current_interests, items)
    }
    /// Removes interests by match (case-insensitive). Returns whether it changed.
    pub fn remove_interests(&mut self, items: &[String]) -> bool {
        remove_from(&mut self.current_interests, items)
    }
}

/// Adds items to a list with case-insensitive dedup (Unicode). Empty-after-trim
/// items are dropped. Returns `true` if anything was added.
fn merge_into(list: &mut Vec<String>, items: Vec<String>) -> bool {
    let mut changed = false;
    for it in items {
        let it = it.trim().to_string();
        if it.is_empty() {
            continue;
        }
        let lc = it.to_lowercase();
        if !list.iter().any(|x| x.to_lowercase() == lc) {
            list.push(it);
            changed = true;
        }
    }
    changed
}

/// Removes items from the list matching (case-insensitive) any of `items`.
/// Returns `true` if the list changed.
fn remove_from(list: &mut Vec<String>, items: &[String]) -> bool {
    let targets: Vec<String> = items
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return false;
    }
    let before = list.len();
    list.retain(|x| !targets.contains(&x.to_lowercase()));
    list.len() != before
}

/// Truncation by characters (not bytes — Cyrillic) with an ellipsis.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

/// Truncation at a word boundary: like [`truncate_chars`], but rolls back to the
/// last space within the limit, so as not to tear a word mid-way ("…" inside a word
/// reads as corrupted memory). If there's no space (one long word) — cuts by
/// character. The result, like [`truncate_chars`]'s, is no longer than `max_chars`
/// characters.
fn truncate_chars_word(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    let head: String = s.chars().take(take).collect();
    // rfind gives the byte index of the space (valid for slicing at a char boundary).
    let base = match head.rfind(char::is_whitespace) {
        Some(idx) => head[..idx].trim_end(),
        None => head.as_str(),
    };
    // The rollback ate everything (a leading space) — fall back to the char-wise head.
    let base = if base.is_empty() { head.as_str() } else { base };
    format!("{base}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default render/storage parameters for tests.
    fn p() -> SelfModelParams {
        SelfModelParams::default()
    }

    /// A time reference point for render tests (matches the record-creation moment
    /// → fresh ones show as "today").
    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    /// A reference locale (ru) for render tests: asserting Russian substrings
    /// simultaneously pins the ru bundle's content. Per-language checks — below
    /// (`render_localized_for_all_langs`, `age_label_localized_for_all_langs`).
    fn loc() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// An observation (self-note) for render tests: id + text + "now".
    /// Observations moved into notes and are passed into the render via the `recent` parameter.
    fn seg(text: &str) -> NarrativeSegment {
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: text.into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn empty_model_renders_none() {
        let m = SelfModel::new(Uuid::new_v4());
        assert!(m.is_empty());
        // Empty both structurally and by observations → None.
        assert!(
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
                .is_none()
        );
    }

    #[test]
    fn render_includes_recent_observations() {
        // Only observations (recent), the structural part is empty → the model is informative.
        let m = SelfModel::new(Uuid::new_v4());
        let recent = [seg("заметил напряжение между «кратко» и «полно»")];
        let r = m
            .render_for_prompt(
                p().prompt_cap,
                p().narrative_in_prompt,
                now(),
                &recent,
                loc(),
            )
            .unwrap();
        assert!(r.contains("Недавние наблюдения:"));
        assert!(r.contains("напряжение"));
    }

    #[test]
    fn render_for_prompt_takes_freshest_n() {
        // recent — newest first; only narrative_in_prompt freshest go into the prompt.
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            narrative_in_prompt: 2,
            prompt_cap: 1000,
            ..SelfModelSettings::default()
        });
        let m = SelfModel::new(Uuid::new_v4());
        let recent: Vec<NarrativeSegment> =
            (0..10).rev().map(|i| seg(&format!("инсайт {i}"))).collect();
        let r = m
            .render_for_prompt(
                params.prompt_cap,
                params.narrative_in_prompt,
                now(),
                &recent,
                loc(),
            )
            .unwrap();
        assert_eq!(r.matches("инсайт ").count(), 2);
        // The first two (newest) — indices 9 and 8.
        assert!(r.contains("инсайт 9"));
        assert!(r.contains("инсайт 8"));
    }

    #[test]
    fn add_and_complete_goals() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("помочь с рефакторингом");
        m.add_goal("  "); // empty one is ignored
        assert_eq!(m.goals.len(), 1);
        assert_eq!(m.active_goals().count(), 1);

        let id = m.goals[0].id;
        assert!(m.set_goal_status(id, GoalStatus::Completed));
        assert_eq!(m.active_goals().count(), 0);
        // a nonexistent goal
        assert!(!m.set_goal_status(Uuid::new_v4(), GoalStatus::Abandoned));
    }

    #[test]
    fn render_includes_sections() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю честность".into();
        m.add_goal("разобраться в коде");
        m.user_model.perceived_traits = vec!["любопытный".into()];
        m.user_model.current_interests = vec!["Rust".into()];
        m.user_model.relationship_dynamic = "доверительные".into();

        let r = m
            .render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert!(r.contains("О себе: ценю честность"));
        assert!(r.contains("разобраться в коде"));
        assert!(r.contains("черты: любопытный"));
        assert!(r.contains("интересы: Rust"));
        assert!(r.contains("отношения: доверительные"));
    }

    #[test]
    fn params_sanitize_inconsistent_settings() {
        // Zero max → a minimum of 1; in_prompt no more than max; a tiny cap → the floor.
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            max_narrative: 0,
            narrative_in_prompt: 99,
            prompt_cap: 1,
            ..SelfModelSettings::default()
        });
        assert_eq!(params.max_narrative, 1);
        assert_eq!(params.narrative_in_prompt, 1);
        assert_eq!(params.prompt_cap, 100);
    }

    #[test]
    fn summary_target_sanitized_to_floor() {
        // A tiny target → a floor of 200 (protection against a meaninglessly small value).
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            summary_target_chars: 10,
            ..SelfModelSettings::default()
        });
        assert_eq!(params.summary_target_chars, 200);
    }

    #[test]
    fn summary_fill_hint_only_over_target() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "к".repeat(50);
        // Within the target — no hint.
        assert!(m.summary_fill_hint(100, loc()).is_none());
        // Beyond the target — a hint with the numbers.
        m.summary = "к".repeat(150);
        let hint = m.summary_fill_hint(100, loc()).unwrap();
        assert!(hint.contains("150"));
        assert!(hint.contains("100"));
        assert!(hint.contains("add_insight"));
    }

    #[test]
    fn completed_goals_not_rendered() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("старая цель");
        let id = m.goals[0].id;
        m.set_goal_status(id, GoalStatus::Completed);
        // only a completed goal → the model is "empty" for rendering
        assert!(m.is_empty());
        assert!(
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
                .is_none()
        );
    }

    #[test]
    fn apply_edit_covers_operations() {
        let mut m = SelfModel::new(Uuid::new_v4());
        assert!(m.apply_edit(SelfModelEdit::SetSummary("я краток".into())));
        assert_eq!(m.summary, "я краток");
        // repeating the same — no change
        assert!(!m.apply_edit(SelfModelEdit::SetSummary("я краток".into())));

        assert!(m.apply_edit(SelfModelEdit::AddGoal("помочь".into())));
        let gid = m.goals[0].id;
        assert!(m.apply_edit(SelfModelEdit::SetGoalText {
            id: gid,
            text: "помочь лучше".into()
        }));
        assert_eq!(m.goals[0].description, "помочь лучше");
        // status cycle: Active → Completed
        assert!(m.apply_edit(SelfModelEdit::CycleGoalStatus(gid)));
        assert_eq!(m.goals[0].status, GoalStatus::Completed);
        assert!(m.apply_edit(SelfModelEdit::DeleteGoal(gid)));
        assert!(m.goals.is_empty());

        assert!(m.apply_edit(SelfModelEdit::SetTraits(vec!["скептик".into()])));
        assert!(m.apply_edit(SelfModelEdit::SetInterests(vec!["Rust".into()])));
        assert!(m.apply_edit(SelfModelEdit::SetRelationship("рабочие".into())));
        assert_eq!(m.user_model.perceived_traits, vec!["скептик".to_string()]);

        // DeleteInsight in apply_edit operates on the `narrative` field (in
        // production the orchestrator intercepts it and deletes the self-note; the
        // field is kept for reconstructing the `F3` snapshot and compatibility).
        // Populate the field directly.
        m.narrative.push(seg("наблюдение"));
        let iid = m.narrative[0].id;
        assert!(m.apply_edit(SelfModelEdit::DeleteInsight(iid)));
        assert!(m.narrative.is_empty());

        // Clear resets everything; a repeat Clear on an empty model — a no-op.
        assert!(m.apply_edit(SelfModelEdit::Clear));
        assert!(m.is_empty());
        assert!(!m.apply_edit(SelfModelEdit::Clear));
        // nonexistent ids — a no-op
        assert!(!m.apply_edit(SelfModelEdit::DeleteGoal(Uuid::new_v4())));
    }

    #[test]
    fn bloated_summary_does_not_starve_sections() {
        // Stage 3: a bloated description doesn't crowd goals/interlocutor/
        // observations out of the injection (a per-section budget: summary ≤ half the limit).
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "слово ".repeat(400); // ~2400 chars, many words
        m.add_goal("активная цель");
        m.user_model.perceived_traits = vec!["внимательный".into()];
        let recent = [seg("свежее наблюдение о стиле")];

        let r = m.render_for_prompt(1200, 3, now(), &recent, loc()).unwrap();
        // All sections are present despite the bloated description.
        assert!(r.contains("Активные цели:"), "goals crowded out: {r}");
        assert!(
            r.contains("О собеседнике:"),
            "interlocutor crowded out: {r}"
        );
        assert!(
            r.contains("Недавние наблюдения:"),
            "observations crowded out: {r}"
        );
        // The block is within the limit; the description is truncated (beyond half the budget).
        assert!(r.chars().count() <= 1200);
        assert!(r.contains("О себе: "));
    }

    #[test]
    fn small_summary_not_truncated() {
        // A small description passes through with no "…" (unchanged behavior for
        // non-bloated models).
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю ясность и краткость".into();
        let r = m
            .render_for_prompt(1200, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert!(r.contains("О себе: ценю ясность и краткость"));
        assert!(!r.contains('…'));
    }

    #[test]
    fn truncate_word_does_not_split_word() {
        // Truncation at a word boundary doesn't tear a word mid-way.
        let s = "первое второе третье четвёртое пятое";
        let out = truncate_chars_word(s, 20);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 20);
        // Trimming at a word boundary: without "…" the result is a prefix of whole words.
        let body = out.trim_end_matches('…');
        assert!(s.starts_with(body.trim_end()));
        assert!(!body.trim_end().is_empty());
        // One long word with no spaces — falls back to char-wise truncation.
        let long = "я".repeat(50);
        let out = truncate_chars_word(&long, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn render_truncates_to_cap() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        let r = m
            .render_for_prompt(50, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert_eq!(r.chars().count(), 50);
        assert!(r.ends_with('…'));
    }

    #[test]
    fn render_full_shows_goal_ids_and_full_observation_ids() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        m.add_goal("активная цель");
        m.add_goal("завершённая цель");
        let done = m.goals[1].id;
        m.set_goal_status(done, GoalStatus::Completed);
        // Many observations (recent, newest first) — a full read shows all of them.
        let recent: Vec<NarrativeSegment> =
            (0..12).rev().map(|i| seg(&format!("инсайт {i}"))).collect();

        let full = m.render_full(now(), &recent, loc());
        assert!(!full.ends_with('…'), "a full read isn't truncated");
        // The active goal — with a short #id, status, and age label ("today").
        let short = short_hex(&m.goals[0].id);
        assert!(full.contains(&format!("#{short} (активна · сегодня) активная цель")));
        // The completed one is also visible (lifecycle), with age from closing.
        assert!(full.contains("(выполнена · сегодня) завершённая цель"));
        // All observations (not just narrative_in_prompt=3), with the FULL id (for
        // note_revise/note_supersede).
        assert_eq!(full.matches("инсайт ").count(), 12);
        assert!(full.contains("Наблюдения (12"));
        assert!(full.contains(&format!("(id={})", recent[0].id)));
        // The full self-description in its entirety (not truncated to prompt_cap).
        assert!(full.contains(&"я".repeat(500)));
    }

    #[test]
    fn render_full_on_empty_marks_empty() {
        let m = SelfModel::new(Uuid::new_v4());
        assert_eq!(m.render_full(now(), &[], loc()), "(модель себя пока пуста)");
    }

    #[test]
    fn match_goal_by_prefix_full_and_ambiguous() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("первая");
        let id = m.goals[0].id;
        // A full UUID.
        assert_eq!(m.match_goal(&id.to_string()), GoalMatch::One(id));
        // A short hex prefix, with a leading '#'.
        let short = short_hex(&id);
        assert_eq!(m.match_goal(&format!("#{short}")), GoalMatch::One(id));
        // Case-insensitive.
        assert_eq!(m.match_goal(&short.to_uppercase()), GoalMatch::One(id));
        // A nonexistent one.
        assert_eq!(m.match_goal("zzzzzz"), GoalMatch::None);
        assert_eq!(m.match_goal(""), GoalMatch::None);
        // An empty prefix (after stripping '#') would match everything → ambiguous.
        m.add_goal("вторая");
        assert_eq!(m.match_goal("#"), GoalMatch::None); // empty → None, not Ambiguous
    }

    #[test]
    fn user_model_merge_add_remove() {
        let mut u = UserModel::default();
        assert!(u.add_traits(vec!["добрый".into(), "Добрый".into(), "  ".into()]));
        // Case-insensitive dedup + dropping the empty one.
        assert_eq!(u.perceived_traits, vec!["добрый".to_string()]);
        // A new edit doesn't overwrite — merge.
        assert!(u.add_traits(vec!["прямолинейный".into()]));
        assert_eq!(u.perceived_traits.len(), 2);
        // Repeating an already-known one — no change.
        assert!(!u.add_traits(vec!["добрый".into()]));
        // Removal by case-insensitive match.
        assert!(u.remove_traits(&["ДОБРЫЙ".into()]));
        assert_eq!(u.perceived_traits, vec!["прямолинейный".to_string()]);
        // Removing a nonexistent one — a no-op.
        assert!(!u.remove_traits(&["нет такого".into()]));
    }

    #[test]
    fn user_model_render_for_impersonation() {
        // Empty → None.
        assert!(
            UserModel::default()
                .render_for_impersonation(500, loc())
                .is_none()
        );
        let u = UserModel {
            perceived_traits: vec!["скептик".into(), "любопытный".into()],
            current_interests: vec!["Rust".into()],
            relationship_dynamic: "доверительные, на равных".into(),
        };
        let r = u.render_for_impersonation(500, loc()).unwrap();
        assert!(r.contains("за которого ты пишешь"));
        assert!(r.contains("черты — скептик, любопытный"));
        assert!(r.contains("интересы — Rust"));
        assert!(r.contains("отношения с собеседником — доверительные, на равных"));
    }

    /// Per-locale coverage (docs/history/i18n.md §3.5): rendering under EVERY built-in
    /// language, no unsubstituted `{…}`, and no Cyrillic leaking into `en`.
    #[test]
    fn render_for_impersonation_localized_for_all_langs() {
        let u = UserModel {
            perceived_traits: vec!["skeptic".into()],
            current_interests: vec!["Rust".into()],
            relationship_dynamic: "trusting, as equals".into(),
        };
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let r = u.render_for_impersonation(500, l).unwrap();
            assert!(!r.contains('{') && !r.contains('}'), "{lang:?}: {r}");
            if lang == crate::shared::i18n::Lang::En {
                assert!(
                    !r.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                    "Cyrillic leaked into en: {r}"
                );
            }
        }
    }

    /// Per-language coverage (§3.5 docs/history/i18n.md): rendering under EVERY
    /// built-in language — sections/observations are tagged with headers from that
    /// language's bundle, placeholders are substituted (catches a broken/incomplete
    /// translation and `{…}` gaps in a specific language).
    #[test]
    fn render_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let mut m = SelfModel::new(Uuid::new_v4());
            m.summary = "s".into();
            m.add_goal("g");
            m.user_model.perceived_traits = vec!["t".into()];
            let recent = [seg("obs")];
            let r = m
                .render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &recent, l)
                .unwrap();
            assert!(r.contains(l.t("selfmodel.render.header")), "{lang:?}: {r}");
            assert!(r.contains(l.t("selfmodel.render.goals_active")), "{lang:?}");
            assert!(r.contains(l.t("selfmodel.render.user")), "{lang:?}");
            assert!(
                r.contains(l.t("selfmodel.render.observations_recent")),
                "{lang:?}"
            );
            // The "today" age is substituted with no leftover `{…}`.
            assert!(!r.contains('{'), "{lang:?}: leftover placeholder: {r}");
            // A full read in the same language — its own header, no truncation.
            let full = m.render_full(now(), &recent, l);
            assert!(full.contains(l.t("selfmodel.render.goals_ref")), "{lang:?}");
            assert!(!full.contains('{'), "{lang:?}: placeholder in full: {full}");
        }
    }

    /// Per-language age labels: every bucket variant substitutes `{n}` and leaves
    /// no placeholder — across all built-in languages.
    #[test]
    fn age_label_localized_for_all_langs() {
        use chrono::Duration;
        let base = Utc::now();
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            for d in [0i64, 1, 3, 10, 40, 400] {
                let s = age_label(base - Duration::days(d), base, l);
                assert!(!s.contains('{'), "{lang:?} d={d}: {s}");
                assert!(!s.is_empty());
            }
        }
    }

    #[test]
    fn age_label_buckets() {
        use chrono::Duration;
        let base = Utc::now();
        let ago = |d: i64| base - Duration::days(d);
        assert_eq!(age_label(base, base, loc()), "сегодня");
        assert_eq!(age_label(ago(1), base, loc()), "вчера");
        assert_eq!(age_label(ago(3), base, loc()), "3 дн.");
        assert_eq!(age_label(ago(6), base, loc()), "6 дн.");
        assert_eq!(age_label(ago(7), base, loc()), "1 нед.");
        assert_eq!(age_label(ago(20), base, loc()), "2 нед.");
        assert_eq!(age_label(ago(31), base, loc()), "1 мес.");
        assert_eq!(age_label(ago(200), base, loc()), "6 мес.");
        assert_eq!(age_label(ago(365), base, loc()), "1 г.");
        assert_eq!(age_label(ago(800), base, loc()), "2 г.");
        // A time "from the future" (clock skew) → falls back to the "today" bucket, not a panic.
        assert_eq!(age_label(base + Duration::hours(5), base, loc()), "сегодня");
    }

    #[test]
    fn set_goal_status_stamps_and_clears_closed_at() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("цель");
        let id = m.goals[0].id;
        assert!(m.goals[0].closed_at.is_none()); // active — no stamp
        m.set_goal_status(id, GoalStatus::Completed);
        let closed = m.goals[0].closed_at;
        assert!(closed.is_some()); // closing stamped the moment
        // A repeat closing (to a different status) doesn't shift the moment.
        m.set_goal_status(id, GoalStatus::Abandoned);
        assert_eq!(m.goals[0].closed_at, closed);
        // Reactivation clears the stamp.
        m.set_goal_status(id, GoalStatus::Active);
        assert!(m.goals[0].closed_at.is_none());
    }

    #[test]
    fn fold_closed_goals_returns_scars_beyond_keep() {
        use chrono::Duration;
        let mut m = SelfModel::new(Uuid::new_v4());
        // Five closed goals with different closing times + one active.
        for i in 0..5 {
            m.add_goal(format!("закрытая {i}"));
        }
        m.add_goal("активная");
        // Close the first five, stamping different closed_at (older ones — earlier).
        for i in 0..5 {
            let id = m.goals[i].id;
            m.set_goal_status(id, GoalStatus::Completed);
            m.goals[i].closed_at = Some(Utc::now() - Duration::days((5 - i) as i64));
        }
        // Keep the 2 freshest closed, the other 3 → come back as scars (the caller
        // records them as self-notes).
        let scars = m.fold_closed_goals(2, loc());
        assert_eq!(scars.len(), 3);
        // The active one is untouched; total goals: 2 closed + 1 active.
        assert_eq!(m.goals.len(), 3);
        assert_eq!(m.active_goals().count(), 1);
        // Scars — "goal archive" entries; the oldest closed one is among them.
        assert!(scars.iter().all(|s| s.starts_with("[архив цели]")));
        assert!(scars.iter().any(|s| s.contains("закрытая 0")));
        // Fewer than keep closed → empty.
        assert!(m.fold_closed_goals(2, loc()).is_empty());
    }
}
