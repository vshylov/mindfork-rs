//! Self-model auto-reflection (Tier 3): every N assistant replies in a chat a
//! background task asks the model to review the recent conversation and
//! **itself** update its self-model (via the SelfModel tools). Unlike
//! auto-titling (a single-turn request with no tools) this is a **mini
//! agentic loop**: the model calls `update_self_model`/`update_user_model`/
//! `add_insight`, and the loop executes them (the tools write directly into
//! `Storage`). The chat isn't mutated, nothing is streamed to the UI —
//! reflection is silent and opt-in (`config.self_model.auto_reflect_every`,
//! off by default). See docs/history/self-model-mvp.md.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use chrono::{DateTime, Utc};

use crate::app::events::BackgroundKind;
use crate::entities::chat::{Chat, DeletedCause};
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::{ToolContext, ToolParams, TurnInfo, notes, self_model};
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

/// Reply-token ceiling per reflection round (with headroom for "thoughts" before the call).
const REFLECT_MAX_TOKENS: usize = 2048;
/// Round limit for reflection's mini agentic loop (a backstop against looping).
const REFLECT_MAX_ROUNDS: u32 = 6;
/// Time limit for the whole reflection run.
const REFLECT_TIMEOUT: Duration = Duration::from_secs(120);

/// Tools available to reflection (intersected with the profile's set). `reflect`
/// (the rubric) isn't needed — the auto mode is already "reflecting". Observations
/// moved into notes (Tier 1), so instead of the removed `consolidate_narrative`,
/// reflection is given note tools for consolidating observation-notes: rewrite a
/// near-duplicate (`note_revise`), replace it with a "scar" (`note_supersede`), or
/// merge (`note_merge`). A graph over observations (Tier 2): `note_link`/
/// `note_neighbors` — link contradicting/refining observations (an id from
/// `get_self_model`). **Cross-organ links (Tier 3):** `note_recall` is given — it
/// returns ids of user-facing notes "about the interlocutor" (still hiding self-notes),
/// so reflection can link an observation "about self" with a fact "about the
/// interlocutor" (`note_link` self↔user). See docs/history/narrative-as-notes.md.
const REFLECT_TOOL_IDS: &[&str] = &[
    self_model::GET_SELF_MODEL_ID,
    self_model::UPDATE_SELF_MODEL_ID,
    self_model::UPDATE_USER_MODEL_ID,
    self_model::ADD_INSIGHT_ID,
    notes::NOTE_RECALL_ID,
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

/// System message for background self-reflection: framing + the shared `POLICY_CORE`
/// (stage 6 — the same rules as the maintenance protocol). Built at runtime because it
/// splices a `const` fragment together with the rules constant.
fn reflect_system_message(loc: &crate::shared::i18n::Locale) -> String {
    loc.tf(
        "prompt.reflect.system",
        &[("core", self_model::policy_core(loc))],
    )
}

/// The start of the reflection window (clamping the watermark to the history length —
/// resilient to `Ctrl+R`/`Ctrl+E` truncation) and the number of assistant replies in
/// that window. Cadence is counted over the window `messages[wm..]`, not the whole
/// history — so each cycle doesn't re-read material already reflected on. A pure
/// function — testable.
pub(super) fn reflect_window(messages: &[Message], reflected_upto: Option<usize>) -> (usize, u32) {
    let wm = reflected_upto.unwrap_or(0).min(messages.len());
    let count = messages[wm..]
        .iter()
        .filter(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty())
        .count() as u32;
    (wm, count)
}

/// A summary of behavioral signals over the reflection window (deletions with
/// `deleted_at > since`): how many times the interlocutor regenerated/deleted a reply
/// (indirect evidence the "reply didn't land") and how many times the assistant itself
/// rewrote a reply. `None` if there are no signals. Gives reflection real behavior
/// instead of just self-descriptions. Entries with no cause (old ones) aren't counted.
/// A pure function — testable.
pub(super) fn behavior_markers(
    chat: &Chat,
    since: Option<DateTime<Utc>>,
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
    let (mut regen, mut del, mut rewrite) = (0u32, 0u32, 0u32);
    for d in &chat.deleted {
        if let Some(s) = since
            && d.deleted_at <= s
        {
            continue; // before the last reflection — already accounted for
        }
        match d.cause {
            Some(DeletedCause::Regenerate) => regen += 1,
            Some(DeletedCause::DeleteExchange) => del += 1,
            Some(DeletedCause::Rewrite) => rewrite += 1,
            None => {}
        }
    }
    if regen == 0 && del == 0 && rewrite == 0 {
        return None;
    }
    // Interlocutor signals (regeneration/deletion) are kept separate from the agent's
    // own behavior (rewriting) — the latter can't be attributed to the interlocutor.
    let mut about_user: Vec<String> = Vec::new();
    if regen > 0 {
        about_user.push(loc.tf("reflect.behavior.regen", &[("n", &regen.to_string())]));
    }
    if del > 0 {
        about_user.push(loc.tf("reflect.behavior.deleted", &[("n", &del.to_string())]));
    }
    let mut out = String::new();
    if !about_user.is_empty() {
        out.push_str(loc.t("reflect.behavior.header"));
        out.push_str(&about_user.join("; "));
        out.push('.');
    }
    if rewrite > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&loc.tf("reflect.behavior.rewrite", &[("n", &rewrite.to_string())]));
    }
    Some(out)
}

impl Orchestrator {
    /// Called after successful generation (`handle_done`): counts assistant replies
    /// **in the window since the last reflection** and, once the threshold is reached,
    /// starts a background reflection run. A silent no-op if the feature is off, the
    /// profile hasn't enabled the self-model tools, reflection is already running, the
    /// server isn't ready, or there isn't enough conversation in the window. The
    /// watermark (`Chat.reflected_upto`) shifts **only on an actual spawn** — a gate
    /// skip doesn't lose the accumulated cycle.
    pub(super) fn maybe_auto_reflect(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.self_model.auto_reflect_every;
        if every == 0 {
            return;
        }

        // Snapshot of chat/profile data (the borrow is released before editing self's fields).
        let profile_id;
        let lang; // the profile's agent-scaffold language (axis A)
        let system_message;
        let last_user;
        let mut digest;
        let watermark; // history length at the moment of coverage — fixed at spawn time
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
            // Gate: the profile has the self-model tools enabled (same gate as prompt injection).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == self_model::GET_SELF_MODEL_ID)
            {
                return;
            }
            // Window-based cadence: assistant replies since the last reflection. Not
            // enough accumulated yet — bail out, watermark untouched.
            let (wm, count) = reflect_window(&chat.messages, chat.reflected_upto);
            if !tool_loop::due(count, every) {
                return;
            }
            allowed = REFLECT_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
            let loc = crate::shared::i18n::locale(lang);
            // The digest is over the window only (not the whole history): otherwise
            // every cycle would re-read what's already been reflected on and produce
            // duplicate insights.
            let Some(d) =
                crate::features::rename_chat::build_conversation_digest(&chat.messages[wm..], loc)
            else {
                return; // not enough conversation in the window — watermark untouched
            };
            // Behavioral signals over the window (regenerations/deletions since the last
            // reflection) — food for the interlocutor model. `since` = the previous
            // `reflected_at` (not yet overwritten by the spawn below).
            digest = match behavior_markers(chat, chat.reflected_at, loc) {
                Some(markers) => format!("{d}\n\n{markers}"),
                None => d,
            };
            watermark = chat.messages.len();
        }

        // An overview of observations for consolidation (similar pairs / contradicts /
        // unlinked) — concrete data for reflecting on memory "about self" (the
        // self-consolidation overview, deferred in Tier 2; enabled after confirming the
        // value of linking in Tier 3). Empty if observations < 2.
        if let Some(overview) = notes::build_self_consolidation_overview(
            &self.storage,
            profile_id,
            crate::shared::i18n::locale(lang),
        ) {
            digest = format!("{digest}\n\n{overview}");
        }

        // Is reflection already running? Skip without shifting the watermark (retry next turn).
        if self.bg_running(BackgroundKind::Reflection) {
            return;
        }
        // Is the server ready? Otherwise skip without shifting the watermark (retry later).
        let Ok(backend) = self.engines.backend_if_ready(self.ui_locale()) else {
            return;
        };

        // All gates passed — fix the watermark (the window is covered) and save the chat.
        // `modified_at` is untouched: reflection shouldn't bump the chat up the list.
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.reflected_upto = Some(watermark);
            chat.reflected_at = Some(Utc::now());
        }
        self.mark_dirty(chat_id);

        // Cancellation token — before the context: its clone goes into `ToolContext.cancel`.
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
                lang,
                cancel: cancel.clone(),
            },
        );
        let sampling = SamplingConfig {
            max_tokens: Some(REFLECT_MAX_TOKENS),
            temperature: Some(0.4),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(reflect_system_message(crate::shared::i18n::locale(lang))),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: self
                .registry
                .schemas_for(&allowed, crate::shared::i18n::locale(lang)),
        };

        // Spawn the task and take the slot (the "running" flag + a quiet status-bar indicator).
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel: cancel.clone(),
            max_rounds: REFLECT_MAX_ROUNDS,
            timeout: REFLECT_TIMEOUT,
            label: "auto-reflection",
            profile_id,
            kind: BackgroundKind::Reflection,
            done_tx: self.bg_done_tx.clone(),
            // A2: summary↔observation semantics (embedding summary paragraphs on the fly
            // inside the task). See docs/history/self-model-consolidation.md §A2.
            summary_semantics: Some(tool_loop::SummarySemantics {
                embedder: self.engines.embedder(),
                storage: self.storage.clone(),
                profile_id,
                loc: crate::shared::i18n::locale(lang),
            }),
        });
        self.begin_bg(BackgroundKind::Reflection, cancel);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Chat, DeletedCause, Message, behavior_markers, reflect_system_message, reflect_window,
    };

    /// The reference locale (ru) for asserting on Russian substrings (pins the ru bundle).
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn reflect_system_message_composes_from_policy_core() {
        let msg = reflect_system_message(ru());
        // Assembled from the shared POLICY_CORE (the same rules as the maintenance protocol).
        assert!(msg.contains(crate::features::tools::self_model::policy_core(ru())));
        // Plus reflection-specific framing.
        assert!(msg.contains("get_self_model"));
        assert!(msg.contains("Поведенческие сигналы"));
        assert!(msg.contains("только вызывай инструменты"));
        // Tier 2: reflection is nudged to link observations (the graph).
        assert!(msg.contains("note_link"));
    }

    /// Per-language (§3.5 docs/history/i18n.md): the reflection system message is
    /// assembled for EVERY built-in language, embeds `policy_core` of that same
    /// language, the `{core}` placeholder is substituted (no leftover), and it carries
    /// tool names (stable, not translated).
    #[test]
    fn reflect_system_message_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let msg = reflect_system_message(l);
            assert!(
                msg.contains(crate::features::tools::self_model::policy_core(l)),
                "{lang:?}: policy_core not embedded"
            );
            assert!(
                !msg.contains("{core}"),
                "{lang:?}: placeholder not substituted"
            );
            assert!(
                msg.contains("get_self_model") && msg.contains("note_link"),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn reflect_tools_include_graph() {
        // Tier 2: auto-reflection is given note_link/note_neighbors (a graph over observations).
        // Tier 3: + note_recall (ids of user notes for cross-organ links).
        use super::{REFLECT_TOOL_IDS, notes};
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_LINK_ID));
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_NEIGHBORS_ID));
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_RECALL_ID));
    }

    #[test]
    fn reflect_message_nudges_cross_organ_linking() {
        // Tier 3: reflection is nudged to link an observation "about self" with a fact "about
        // the interlocutor" (a cross-organ edge via note_recall + note_link).
        let msg = super::reflect_system_message(ru());
        assert!(msg.contains("note_recall"));
        assert!(msg.contains("о собеседнике"));
        // The self-consolidation overview: reflection is told to use its block.
        assert!(msg.contains("Обзор наблюдений для консолидации"));
    }

    #[test]
    fn behavior_markers_counts_by_cause_and_filters_since() {
        use crate::entities::chat::DeletedExchange;
        use crate::entities::profile::Profile;
        use chrono::{Duration, Utc};

        let p = Profile::new("P", "s");
        let mut chat = Chat::from_profile(&p, "t");
        let base = Utc::now();
        let mk = |at, cause| DeletedExchange {
            deleted_at: at,
            messages: vec![Message::user("x")],
            draft: String::new(),
            cause: Some(cause),
        };
        chat.deleted = vec![
            mk(base - Duration::hours(1), DeletedCause::Regenerate),
            mk(base - Duration::hours(2), DeletedCause::Regenerate),
            mk(base - Duration::hours(3), DeletedCause::DeleteExchange),
            mk(base - Duration::days(5), DeletedCause::Rewrite), // before since
            DeletedExchange {
                deleted_at: base - Duration::hours(1),
                messages: vec![Message::user("x")],
                draft: String::new(),
                cause: None, // an old entry with no cause — not counted
            },
        ];
        // since = 4h ago → the last 3 (2 regen + 1 delete); rewrite (5d) is cut off.
        let out = behavior_markers(&chat, Some(base - Duration::hours(4)), ru()).unwrap();
        assert!(out.contains("перегенерировал твой ответ ×2"));
        assert!(out.contains("удалил обмен ×1"));
        assert!(!out.contains("переписывал"));
        // since=None → count everything, the agent's own behavior (rewrite) — a separate phrase.
        let all = behavior_markers(&chat, None, ru()).unwrap();
        assert!(all.contains("Ты сам переписывал свой ответ ×1"));
        // No signals → None.
        let empty = Chat::from_profile(&p, "t2");
        assert!(behavior_markers(&empty, None, ru()).is_none());
    }

    #[test]
    fn reflect_window_counts_assistant_from_watermark() {
        let msgs = vec![
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::assistant(""), // an empty reply doesn't count
            Message::user("u3"),
            Message::assistant("a3"),
        ];
        // With no watermark — count all non-empty assistant replies (a1,a2,a3).
        assert_eq!(reflect_window(&msgs, None), (0, 3));
        // Watermark after a2 (index 4): only a3 is in the window.
        assert_eq!(reflect_window(&msgs, Some(4)), (4, 1));
        // Watermark at the end — the window is empty.
        assert_eq!(reflect_window(&msgs, Some(msgs.len())), (7, 0));
    }

    #[test]
    fn reflect_window_clamps_past_watermark_after_truncation() {
        // The history is truncated (Ctrl+R/Ctrl+E) — the watermark exceeds the length: clamp to len,
        // the window is empty, no panic.
        let msgs = vec![Message::user("u1"), Message::assistant("a1")];
        assert_eq!(reflect_window(&msgs, Some(99)), (2, 0));
    }
}
