//! Notes auto-consolidation ("sleep", Tier 3): every N assistant replies in a chat,
//! a background task asks the model to review the knowledge base and consolidate
//! it **itself** — merge duplicates, revise/replace stale entries, link
//! related ones. Like auto-reflection, this is a **mini agentic loop**: the model calls
//! note tools, the loop executes them (they write directly into `Storage`). The chat isn't
//! mutated, nothing streams to the UI — consolidation is silent and opt-in
//! (`config.notes.auto_consolidate_every`, off by default).
//! See docs/history/notes-connectivity.md (Tier 3).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::events::BackgroundKind;
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::notes;
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

/// The token ceiling for a consolidation round's reply (with margin for "thoughts" before the call).
const CONSOLIDATE_MAX_TOKENS: usize = 2048;
/// The round limit for the consolidation mini agentic loop (a backstop against looping).
const CONSOLIDATE_MAX_ROUNDS: u32 = 8;
/// The time limit for the whole consolidation.
const CONSOLIDATE_TIMEOUT: Duration = Duration::from_secs(180);

/// Tools available to consolidation (intersected with the profile's set).
const CONSOLIDATE_TOOL_IDS: &[&str] = &[
    "note_recall",
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

impl Orchestrator {
    /// Called after a successful generation (`handle_done`): counts assistant replies
    /// and, once the threshold is reached, starts background consolidation. Silently does nothing
    /// if the feature is disabled, the profile hasn't enabled note tools, consolidation is
    /// already running, there are fewer than two active notes, or the server isn't ready.
    pub(super) fn maybe_auto_consolidate(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.notes.auto_consolidate_every;
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
            // Gate: the profile enables consolidation (note_merge is the core operation).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == notes::NOTE_MERGE_ID)
            {
                return;
            }
            allowed = CONSOLIDATE_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(ToString::to_string)
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
        }

        // The reply counter since the last consolidation: increment; if the threshold isn't reached —
        // exit (the counter keeps accumulating). Reset — only on an actual spawn
        // (below), so a gate skip doesn't lose the accumulated cycle.
        {
            let count = self.consolidate_counts.entry(chat_id).or_insert(0);
            *count += 1;
            if !tool_loop::due(*count, every) {
                return;
            }
        }
        if self.bg_running(BackgroundKind::Consolidation) {
            return; // already running — skip without a reset (we'll retry next turn)
        }
        // Nothing to consolidate if there are fewer than two user notes (self-notes
        // don't count — consolidation doesn't operate on them). The counter isn't reset — retry.
        let active_user = self
            .storage
            .db()
            .note_list(profile_id, None, &[], None)
            .unwrap_or_default()
            .iter()
            .filter(|n| !notes::is_self_note(n))
            .count();
        if active_user < 2 {
            return;
        }
        // Is the server ready? Otherwise silently skip (the counter isn't reset).
        let Ok(backend) = self.engines.backend_if_ready(self.ui_locale()) else {
            return;
        };
        // All gates passed — reset the counter and spawn.
        self.consolidate_counts.insert(chat_id, 0);
        let overview = notes::build_consolidation_overview(
            &self.storage,
            profile_id,
            crate::shared::i18n::locale(lang),
        );

        // The cancellation token — before the context: its clone goes into `ToolContext.cancel`.
        let cancel = CancellationToken::new();
        let sessions = self.session_budget();
        let ctx = self.background_tool_ctx(
            backend.clone(),
            sessions,
            profile_id,
            chat_id,
            system_message,
            last_user,
            lang,
            cancel.clone(),
        );
        let sampling = SamplingConfig {
            max_tokens: Some(CONSOLIDATE_MAX_TOKENS),
            temperature: Some(0.3),
            ..Default::default()
        };
        let request = ChatRequest {
            continue_final: false,
            system: Some(
                crate::shared::i18n::locale(lang)
                    .t("prompt.consolidate.system")
                    .to_string(),
            ),
            messages: vec![ApiMessage::user(overview)],
            sampling,
            tools: self
                .registry
                .schemas_for(&allowed, crate::shared::i18n::locale(lang)),
        };

        // Spawn the task and set the slot (the "running" flag + a quiet status-bar indicator).
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel: cancel.clone(),
            max_rounds: CONSOLIDATE_MAX_ROUNDS,
            timeout: CONSOLIDATE_TIMEOUT,
            label: "auto-consolidation",
            profile_id,
            kind: BackgroundKind::Consolidation,
            done_tx: self.bg_done_tx.clone(),
            // Notes consolidation is about user notes, not the self-model's summary;
            // summary↔observation semantics don't apply to it.
            summary_semantics: None,
        });
        self.begin_bg(BackgroundKind::Consolidation, cancel);
    }
}
