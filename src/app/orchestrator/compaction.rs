//! Conversation history compression: folding the older part of a chat into a
//! rolling summary so it keeps fitting the model's context window (spec §6.7,
//! docs/research/history-compression.md).
//!
//! The shape is `title.rs`'s, not `tool_loop.rs`'s: one independent single-turn
//! request with no history and no tools, whose **text** comes back through a
//! typed channel. A `SilentLoop` cannot be used here — its done channel carries
//! only `Result<(), String>`, and a summary is precisely the text.
//!
//! What compression never does is edit `chat.messages`. Only what a *request*
//! carries changes ([`Chat::compaction_view`](crate::entities::chat::Chat::compaction_view)),
//! so the feed, search, export, TTS and the reflection watermark all keep seeing
//! the whole conversation.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, BackgroundKind};
use crate::entities::chat::Compaction;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::features::compaction::{
    build_compaction_digest, plan_cut, roll_user_message, summary_system_message,
};
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend};

use super::Orchestrator;
use super::title::salvage_title_source;

/// A safety net **well above** the stated word limit, not the budget itself.
/// Measured (§9a of the research): a bare `max_tokens` cap does not shorten a
/// summary, it truncates one mid-sentence — the real limit is the word count
/// stated inside the prompt. This only stops a runaway.
const COMPACT_MAX_TOKENS: usize = 2048;

/// The time limit for one roll. Generous for the same reason as auto-title:
/// a model with thinking baked into its template may reason for a while first.
const COMPACT_TIMEOUT: Duration = Duration::from_secs(180);

/// The result of one summarization roll (an internal channel).
pub(super) struct CompactResult {
    pub(super) chat_id: Uuid,
    /// The cut this roll was planned against. Re-validated on arrival: the user
    /// can edit history (`Ctrl+E`/`Ctrl+R`) while the roll is in flight.
    pub(super) boundary_id: Uuid,
    /// 1 for the first compaction of a chat, +1 per roll after that.
    pub(super) rolls: u32,
    /// The summary text, or a message to show the user.
    pub(super) text: Result<String, String>,
}

impl Orchestrator {
    /// `/compact`: fold everything older than the verbatim tail into the rolling
    /// summary. Runs in the background; the result lands in
    /// [`Orchestrator::handle_compact_result`].
    ///
    /// Every refusal is answered, never silent — the user typed a command and is
    /// owed an outcome.
    pub(super) fn handle_compact(&mut self) {
        let ui = self.ui_locale();
        // F10: off means inert. Not even a stored summary is consulted, and the
        // command says where the switch is rather than just declining.
        if !self.config.compaction.enabled {
            let _ = self
                .evt_tx
                .send(AppEvent::Notice(ui.t("ui.compact.disabled").into()));
            return;
        }
        if self.bg_running(BackgroundKind::Compaction) {
            let _ = self
                .evt_tx
                .send(AppEvent::Notice(ui.t("ui.compact.busy").into()));
            return;
        }
        let Some(chat) = self.chats.iter().find(|c| Some(c.id) == self.active_id) else {
            return;
        };
        let chat_id = chat.id;
        // The scaffold language is the profile's (axis A): the model reads the
        // digest, the instructions and the block header. Refusals are for the
        // human, so they stay in the interface language (axis B).
        let loc = self.profile_locale(chat.profile_id);
        let cfg = &self.config.compaction;

        let previous = chat.compaction_view(true);
        let prev_upto = previous.map_or(0, |(_, i)| i);
        // A cut must move the boundary forward, or the roll would re-summarize
        // what the summary already covers and cost a generation for nothing.
        let cut = match plan_cut(&chat.messages, cfg.tail_tokens) {
            Some(cut) if cut > prev_upto => cut,
            _ => {
                let _ = self
                    .evt_tx
                    .send(AppEvent::Notice(ui.t("ui.compact.nothing").into()));
                return;
            }
        };
        let Some(digest) = build_compaction_digest(&chat.messages[prev_upto..cut], loc) else {
            let _ = self
                .evt_tx
                .send(AppEvent::Notice(ui.t("ui.compact.nothing").into()));
            return;
        };
        let boundary_id = chat.messages[cut].id;
        let rolls = chat.compaction.as_ref().map_or(0, |c| c.rolls) + 1;

        let backend = match self.engines.backend_if_ready(ui) {
            Ok(backend) => backend,
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                return;
            }
        };
        // Reasoning is muted the way `title.rs` mutes it: `reasoning_budget = 0`
        // is the only field that reaches a model with thinking baked into its
        // template, and a summarizer that spends its budget reasoning returns an
        // empty `content`.
        let sampling = SamplingConfig {
            max_tokens: Some(COMPACT_MAX_TOKENS),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        };
        let user = match previous {
            Some((summary, _)) => roll_user_message(summary, &digest, loc, cfg.summary_words),
            None => digest,
        };
        let request = ChatRequest {
            system: Some(summary_system_message(loc, cfg.summary_words)),
            messages: vec![ApiMessage::user(user)],
            sampling,
            tools: Vec::new(),
        };
        let cancel = CancellationToken::new();
        spawn_compact(
            backend,
            request,
            chat_id,
            boundary_id,
            rolls,
            cancel.clone(),
            ui,
            self.compact_tx.clone(),
        );
        self.begin_bg(BackgroundKind::Compaction, cancel);
    }

    /// Applies a finished roll.
    pub(super) fn handle_compact_result(&mut self, res: CompactResult) {
        // Stage 1 has only the manual command, so a failure is reported straight
        // to the user and the slot is closed as a success: the failure streak
        // exists for *silent* runs, and alerting twice for a command the user
        // just typed would be noise. When stage 2 adds the automatic trigger it
        // should pass the real result through for the auto path.
        let CompactResult {
            chat_id,
            boundary_id,
            rolls,
            text,
        } = res;
        let outcome = match text {
            Ok(summary) => self.apply_compaction(chat_id, boundary_id, rolls, summary),
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                Ok(())
            }
        };
        self.handle_bg_done(BackgroundKind::Compaction, outcome);
    }

    /// Stores a completed summary, if the boundary it was written against still
    /// exists. Returns `Err` only for an outcome worth counting as a failure.
    fn apply_compaction(
        &mut self,
        chat_id: Uuid,
        boundary_id: Uuid,
        rolls: u32,
        summary: String,
    ) -> Result<(), String> {
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            let msg = self.ui_locale().t("ui.err.compact_timeout").to_string();
            let _ = self.evt_tx.send(AppEvent::Error(msg.clone()));
            return Err(msg);
        }
        let Some(chat) = self.chat_mut(chat_id) else {
            return Ok(());
        };
        // The history may have been edited while the roll was in flight. The
        // boundary is re-found by id: if it is gone, the summary can no longer be
        // placed and is dropped rather than pinned to whatever now sits at that
        // index. The next `/compact` rebuilds it.
        let Some(upto) = chat.messages.iter().position(|m| m.id == boundary_id) else {
            tracing::warn!(chat = %chat_id, "compaction boundary vanished mid-roll, discarding");
            return Ok(());
        };
        chat.compaction = Some(Compaction {
            summary: summary.clone(),
            upto,
            boundary_id,
            compacted_at: chrono::Utc::now(),
            rolls,
        });
        // `modified_at` is deliberately not touched: compressing is housekeeping,
        // it should not bump the chat up the list (the reflection precedent).
        self.mark_dirty(chat_id);
        let _ = self.evt_tx.send(AppEvent::Compacted {
            chat_id,
            boundary: boundary_id,
            summary,
            folded: upto,
        });
        Ok(())
    }
}

/// Runs one summarization roll: a single independent request, text collected,
/// result posted to `compact_tx`.
#[allow(clippy::too_many_arguments)] // cohesive: one call's parameters, the `spawn_title` shape
fn spawn_compact(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    chat_id: Uuid,
    boundary_id: Uuid,
    rolls: u32,
    cancel: CancellationToken,
    loc: &'static crate::shared::i18n::Locale,
    compact_tx: UnboundedSender<CompactResult>,
) {
    tokio::spawn(async move {
        let collect = async {
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            let mut text = String::new();
            let mut thoughts = String::new();
            let mut truncated = false;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    // Kept as a salvage source for the same reason as auto-title:
                    // a model that never "finished thinking" leaves `content` empty.
                    ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                    ChatChunk::Finished(reason) => {
                        truncated = matches!(reason, crate::shared::api::FinishReason::Length);
                        break;
                    }
                    ChatChunk::ThoughtsSignature(_)
                    | ChatChunk::ToolCall(_)
                    | ChatChunk::Usage(_) => {}
                }
            }
            Ok::<(String, String, bool), anyhow::Error>((text, thoughts, truncated))
        };
        let text = match tokio::time::timeout(COMPACT_TIMEOUT, collect).await {
            Ok(Ok((text, thoughts, truncated))) => {
                if truncated {
                    // Don't hide a truncation — the same rule the conversation's
                    // own overflow follows (§1.2). The summary is still usable,
                    // so this is a log line, not a refusal.
                    tracing::warn!(
                        chat = %chat_id,
                        "the summary hit the token ceiling and was cut; raise the ceiling or lower the word limit"
                    );
                }
                Ok(salvage_title_source(text, thoughts))
            }
            Ok(Err(err)) => Err(loc.tf("ui.err.title_gen_failed", &[("err", &err.to_string())])),
            Err(_) => {
                cancel.cancel();
                Err(loc.t("ui.err.compact_timeout").to_string())
            }
        };
        let _ = compact_tx.send(CompactResult {
            chat_id,
            boundary_id,
            rolls,
            text,
        });
    });
}
