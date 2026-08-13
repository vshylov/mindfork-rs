//! Self-model auto-consolidation (the self-model's "sleep", stage A1): every N
//! assistant replies in a chat, a background task asks the model to review its own
//! "self-model" and consolidate it **itself** — merge duplicate observations
//! (`@self` notes), compress a bloated description (`summary`), link contradictions. Like
//! auto-reflection and notes auto-consolidation, this is a **mini agentic loop**: the
//! model calls self-model/note tools, the loop executes them (they write directly into
//! `Storage`). The chat isn't mutated, nothing streams to the UI — the "sleep" is silent
//! and opt-in (`config.self_model.auto_consolidate_every`, off by default). A separate
//! toggle from auto-reflection: the self-model's and notes' gates/data are already kept
//! apart, its own counter is more precise. See docs/history/self-model-consolidation.md (stage A1).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::events::BackgroundKind;
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::self_model::SelfModelParams;
use crate::features::tools::{ToolContext, ToolParams, TurnInfo, notes, self_model};
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

/// The token ceiling for a self-model "sleep" round's reply (with margin for "thoughts" before the call).
const SELF_CONSOLIDATE_MAX_TOKENS: usize = 2048;
/// The round limit for the self-model "sleep" mini agentic loop (a backstop against looping).
const SELF_CONSOLIDATE_MAX_ROUNDS: u32 = 8;
/// The time limit for the whole self-model consolidation.
const SELF_CONSOLIDATE_TIMEOUT: Duration = Duration::from_secs(180);

/// Tools available to the self-model "sleep" (intersected with the profile's set). Over
/// observation-notes (`@self`): rewrite a near-duplicate (`note_revise`), replace
/// with a "scar" (`note_supersede`), merge (`note_merge`); the graph — link
/// contradicting/refining ones (`note_link`/`note_neighbors`). Plus `update_self_model`
/// (compress a bloated `summary`) and `update_user_model` (reconcile the interlocutor).
/// `get_self_model` gives the full observation ids and the current `summary`. `note_recall`
/// is **deliberately withheld** — it hides `@self`; the model takes full ids from `get_self_model`
/// (like reflection). See docs/history/self-model-consolidation.md (stage A1).
const SELF_CONSOLIDATE_TOOL_IDS: &[&str] = &[
    self_model::GET_SELF_MODEL_ID,
    self_model::UPDATE_SELF_MODEL_ID,
    self_model::UPDATE_USER_MODEL_ID,
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

/// The system message for background self-model consolidation: framing + the shared
/// `POLICY_CORE` (the same maintenance rules as the protocol/reflection). Built at
/// runtime, since it splices a `const` fragment together with the rules constant.
fn self_consolidate_system_message(loc: &crate::shared::i18n::Locale) -> String {
    loc.tf(
        "prompt.self_consolidate.system",
        &[("core", self_model::policy_core(loc))],
    )
}

impl Orchestrator {
    /// Called after a successful generation (`handle_done`): counts assistant replies
    /// and, once the threshold is reached, starts background "self-model" consolidation. Silently
    /// does nothing if the feature is disabled, the profile hasn't enabled self-model
    /// tools, "sleep" is already running, there's nothing to consolidate (observations < 2 and
    /// `summary` isn't bloated), or the server isn't ready. The cadence counter is reset
    /// **only on an actual spawn** — a gate skip doesn't lose the accumulated cycle.
    pub(super) fn maybe_auto_self_consolidate(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.self_model.auto_consolidate_every;
        if every == 0 {
            return;
        }

        let profile_id;
        let lang; // the profile's agent-scaffold language (axis A)
        let system_message;
        let last_user;
        let allowed: Vec<ToolId>;
        {
            let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
                return;
            };
            profile_id = chat.profile_id;
            let Some(profile) = self.profiles.iter().find(|p| p.id == profile_id) else {
                return;
            };
            lang = profile.language;
            // Gate: the profile enables self-model tools (like injection/reflection).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == self_model::GET_SELF_MODEL_ID)
            {
                return;
            }
            allowed = SELF_CONSOLIDATE_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
        }

        // The reply counter since the last "sleep": increment; if the threshold isn't reached — exit
        // (the counter keeps accumulating). Reset — only on an actual spawn (below), so
        // a gate skip doesn't lose the accumulated cycle.
        {
            let count = self.self_consolidate_counts.entry(chat_id).or_insert(0);
            *count += 1;
            if !tool_loop::due(*count, every) {
                return;
            }
        }
        if self.bg_running(BackgroundKind::SelfConsolidation) {
            return; // already running — skip without a reset (we'll retry next turn)
        }

        // Nothing to consolidate? There's a signal if observations (`@self`) ≥ 2 (the
        // self-consolidation overview is non-empty) OR the self-description is bloated past the target. Otherwise —
        // exit without resetting the counter (retry later).
        let loc = crate::shared::i18n::locale(lang);
        let params = SelfModelParams::from_settings(&self.config.self_model);
        let obs_count = self
            .storage
            .db()
            .note_list(profile_id, None, &[notes::SELF_NOTE_TAG.to_string()], None)
            .unwrap_or_default()
            .len();
        let summary_hint = self
            .storage
            .db()
            .self_model_get(profile_id)
            .ok()
            .flatten()
            .and_then(|m| m.summary_fill_hint(params.summary_target_chars, loc));
        if obs_count < 2 && summary_hint.is_none() {
            return;
        }

        // Is the server ready? Otherwise silently skip (the counter isn't reset).
        let Ok(backend) = self.engines.backend_if_ready(self.ui_locale()) else {
            return;
        };
        // All gates passed — reset the counter and spawn.
        self.self_consolidate_counts.insert(chat_id, 0);

        // The digest: the self-consolidation overview (similar observation pairs / contradicts / with no
        // links; `None` when observations < 2) + a hint about a bloated description (if any).
        let overview = notes::build_self_consolidation_overview(&self.storage, profile_id, loc);
        let digest = match (overview, summary_hint) {
            (Some(o), Some(h)) => format!("{o}\n\n{h}"),
            (Some(o), None) => o,
            (None, Some(h)) => h,
            // The gate above rules this out; a defensive branch — nothing to consolidate.
            (None, None) => return,
        };

        // The cancellation token — before the context: its clone goes into `ToolContext.cancel`.
        let cancel = CancellationToken::new();
        let ctx = ToolContext::new(
            self.tool_deps(backend.clone()),
            ToolParams::from_config(&self.config),
            TurnInfo {
                profile_id,
                chat_id,
                system_message,
                effective_sampling: SamplingConfig::default(),
                last_user_message_at: last_user,
                // A background task runs outside a chat turn — no attachments.
                attachments: std::sync::Arc::from(Vec::new()),
                history: None,
                other_chats: std::sync::Arc::from(Vec::new()),
                lang,
                cancel: cancel.clone(),
            },
        );
        let sampling = SamplingConfig {
            max_tokens: Some(SELF_CONSOLIDATE_MAX_TOKENS),
            temperature: Some(0.3),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(self_consolidate_system_message(loc)),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: self.registry.schemas_for(&allowed, loc),
        };

        // Spawn the task and set the slot (the "running" flag + a quiet status-bar indicator).
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel: cancel.clone(),
            max_rounds: SELF_CONSOLIDATE_MAX_ROUNDS,
            timeout: SELF_CONSOLIDATE_TIMEOUT,
            label: "auto self-consolidation",
            profile_id,
            kind: BackgroundKind::SelfConsolidation,
            done_tx: self.bg_done_tx.clone(),
            // A2: summary↔observation semantics (embedding summary paragraphs on the fly in
            // the task). See docs/history/self-model-consolidation.md §A2.
            summary_semantics: Some(tool_loop::SummarySemantics {
                embedder: self.engines.embedder(),
                storage: self.storage.clone(),
                profile_id,
                loc,
            }),
        });
        self.begin_bg(BackgroundKind::SelfConsolidation, cancel);
    }
}

#[cfg(test)]
mod tests {
    use super::self_consolidate_system_message;

    /// The reference locale (ru) for assertions on Russian substrings (pins the ru bundle).
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn self_consolidate_system_message_composes_from_policy_core() {
        let msg = self_consolidate_system_message(ru());
        // Composed from the shared POLICY_CORE (the same rules as the maintenance protocol).
        assert!(msg.contains(crate::features::tools::self_model::policy_core(ru())));
        // Plus the "sleep"-specific framing.
        assert!(msg.contains("get_self_model"));
        assert!(msg.contains("note_merge"));
        assert!(msg.contains("update_self_model"));
    }

    /// Per-language (§3.5 docs/history/i18n.md): the system message is composed for EVERY
    /// built-in language, embeds `policy_core` of the same language, the `{core}`
    /// placeholder is substituted (with nothing left over), carries tool names (stable, not translated).
    #[test]
    fn self_consolidate_system_message_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let msg = self_consolidate_system_message(l);
            assert!(
                msg.contains(crate::features::tools::self_model::policy_core(l)),
                "{lang:?}: policy_core is not embedded"
            );
            assert!(
                !msg.contains("{core}"),
                "{lang:?}: the placeholder wasn't substituted"
            );
            assert!(
                msg.contains("get_self_model") && msg.contains("note_merge"),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn self_consolidate_tools_cover_observations_and_summary() {
        use super::SELF_CONSOLIDATE_TOOL_IDS;
        use crate::features::tools::{notes, self_model};
        // Observations (graph/merge) + summary compression — present; note_recall isn't (it hides @self).
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_MERGE_ID));
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_LINK_ID));
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&self_model::UPDATE_SELF_MODEL_ID));
        assert!(!SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_RECALL_ID));
    }
}
