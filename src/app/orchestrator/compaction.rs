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
use crate::shared::config::ServerMode;
use crate::shared::session_budget::SILENT_YIELDS_MAX;

use super::Orchestrator;
use super::generation::TurnUsage;
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
    /// Who asked — a failure is reported differently (see [`CompactOrigin`]).
    pub(super) origin: CompactOrigin,
    /// The summary text, or a message to show the user.
    pub(super) text: Result<String, String>,
}

/// What started a roll. The two differ **only** in how a failure is reported
/// (sub-decision S6): a command the user just typed is owed an answer straight
/// away, while a silent background run belongs in the failure streak the
/// background-slot machinery already provides — alerting once at the third
/// consecutive failure instead of on every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompactOrigin {
    /// `/compact`.
    Manual,
    /// The threshold was crossed at the end of a turn.
    Auto,
}

/// What a roll is about to do, decided **without** touching the engine.
///
/// The seam exists so the two entry points can share the planning while
/// answering differently: `/compact` turns a `None` into "nothing to compress
/// yet", the automatic trigger just stays quiet — and a test can assert on the
/// plan instead of inferring it from what a roll happened to send.
pub(super) struct RollPlan {
    /// Index of the first message that stays verbatim: `messages[..cut]` is what
    /// this roll folds in. Always a `User` message (see `plan_cut`).
    cut: usize,
    boundary_id: Uuid,
    rolls: u32,
    request: ChatRequest,
}

/// Where the context window comes from and what has been learned about it.
///
/// Only the *discovered* half needs state: an explicit setting and a managed
/// server's `-c` are read straight from the config every time. The engine is
/// asked once per applied engine (`invalidate` on a settings change and on a
/// readiness flip, so a server that came up late is re-asked), never per turn.
#[derive(Default)]
pub(super) struct ContextDiscovery {
    /// Bumped by [`Self::invalidate`]. An answer that arrives for an older epoch
    /// is dropped: switching from a local 8k model to a cloud one mid-flight must
    /// not leave the cloud measured against the local window.
    epoch: u64,
    /// A question is in flight — don't ask again.
    pending: bool,
    /// An answer arrived for the current epoch (possibly "cannot say").
    answered: bool,
    /// The window the engine reported, in tokens.
    known: Option<u32>,
}

impl ContextDiscovery {
    /// Forgets what was learned: the engine changed, or its readiness flipped and
    /// a server that could not answer before may answer now.
    pub(super) fn invalidate(&mut self) {
        self.epoch += 1;
        self.pending = false;
        self.answered = false;
        self.known = None;
    }

    /// The engine generation a pending answer would have to match.
    #[cfg(test)]
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Applies an answer if it belongs to the current engine.
    fn apply(&mut self, epoch: u64, budget: Option<u32>) {
        if epoch != self.epoch {
            return;
        }
        self.pending = false;
        self.answered = true;
        self.known = budget;
    }
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
        let Some(plan) = self.plan_roll(chat) else {
            let _ = self
                .evt_tx
                .send(AppEvent::Notice(ui.t("ui.compact.nothing").into()));
            return;
        };
        if let Err(msg) = self.spawn_roll(chat_id, plan, CompactOrigin::Manual) {
            let _ = self.evt_tx.send(AppEvent::Error(msg));
        }
    }

    /// Compacts on its own once the conversation approaches the model's context
    /// window (spec §6.7, fork F4a). Called at the end of a turn, next to the
    /// other background triggers.
    ///
    /// Runs on **the chat whose turn just finished**, not "the active one": they
    /// are the same today (one generation at a time), and reading the turn's own
    /// id is what stays correct if that ever stops being true (S7).
    pub(super) fn maybe_auto_compact(&mut self, chat_id: Uuid, usage: Option<TurnUsage>) {
        let (enabled, threshold_pct) = {
            let cfg = &self.config.compaction;
            (cfg.enabled, cfg.threshold_pct)
        };
        if !enabled || threshold_pct == 0 {
            return;
        }
        // S2: the exact figure or nothing. The byte estimate's error changes sign
        // by content type (§9a M9), so it would fire late on exactly the
        // tool-heavy chats that overflow first — a fallback worse than silence.
        let Some(usage) = usage else { return };
        let Some(budget) = self.context_budget() else {
            return;
        };
        // The reply reserve lives in the headroom the percentage leaves, not in a
        // knob of its own (S5).
        let threshold = budget.saturating_mul(threshold_pct as u64) / 100;
        if usage.next_prompt_estimate() < threshold {
            return;
        }
        // One at a time — the same gate as the other background tasks. A roll
        // already running will move the boundary anyway.
        if self.bg_running(BackgroundKind::Compaction) {
            return;
        }
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            return;
        };
        let Some(plan) = self.plan_roll(chat) else {
            // Nothing left to fold: the tail alone already fills the window. Only
            // the read-back tools (stage 3) or a bigger `-c` help here, and both
            // are the user's move — saying it every turn would be nagging.
            tracing::debug!(chat = %chat_id, "over the compaction threshold with nothing left to fold");
            return;
        };
        let folded = plan.cut;
        if let Err(reason) = self.spawn_roll(chat_id, plan, CompactOrigin::Auto) {
            // The server not being ready is not a compaction failure — the turn
            // that just ended used it, so this is a transient state and the next
            // turn will try again. It must not spend a strike.
            tracing::debug!(chat = %chat_id, %reason, "auto-compaction deferred");
            return;
        }
        tracing::info!(
            chat = %chat_id,
            prompt = usage.prompt_tokens,
            budget,
            folded,
            "auto-compaction started"
        );
    }

    /// The context window to measure against, in tokens, or `None` when nothing
    /// can say what it is (then the automatic trigger stays inactive — spec §6.7).
    ///
    /// Order: an explicit setting → a managed server's `-c` (that number *is*
    /// what the child was launched with, and needs no network) → what the engine
    /// itself reports. The last one is asked in the background, so the first turn
    /// after a (re)connect kicks the question off and returns `None`; by the time
    /// a real conversation approaches its window the answer is long since in.
    pub(super) fn context_budget(&mut self) -> Option<u64> {
        if let Some(explicit) = self.config.compaction.context_tokens.filter(|&n| n > 0) {
            return Some(explicit as u64);
        }
        if self.config.engine.mode == ServerMode::Managed {
            let ctx = self.config.engine.managed.context_size;
            return (ctx > 0).then_some(ctx as u64);
        }
        if let Some(known) = self.context.known {
            return Some(known as u64);
        }
        if !self.context.answered && !self.context.pending {
            self.ask_engine_for_budget();
        }
        None
    }

    /// Asks the engine what its window is, in the background (S1).
    fn ask_engine_for_budget(&mut self) {
        // Deliberately `backend()` rather than `backend_if_ready`: a server that
        // is still loading answers `/props` perfectly well, and gating on
        // readiness would only postpone the question for no gain.
        let Some(backend) = self.engines.backend.clone() else {
            return;
        };
        self.context.pending = true;
        let epoch = self.context.epoch;
        let tx = self.budget_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send((epoch, backend.context_budget().await));
        });
    }

    /// Records what the engine answered about its context window.
    pub(super) fn handle_budget_result(&mut self, epoch: u64, budget: Option<u32>) {
        self.context.apply(epoch, budget);
        if let Some(n) = budget {
            tracing::info!(context_budget = n, "engine reported its context window");
        }
    }

    /// Decides what a roll would fold and builds its request — no engine, no
    /// side effects. `None`: there is nothing worth folding.
    fn plan_roll(&self, chat: &crate::entities::chat::Chat) -> Option<RollPlan> {
        // The scaffold language is the profile's (axis A): the model reads the
        // digest, the instructions and the block header. Refusals are for the
        // human, so they stay in the interface language (axis B).
        let loc = self.profile_locale(chat.profile_id);
        let cfg = &self.config.compaction;

        let previous = chat.compaction_view(true);
        let prev_upto = previous.map_or(0, |(_, i)| i);
        // A cut must move the boundary forward, or the roll would re-summarize
        // what the summary already covers and cost a generation for nothing.
        let cut = plan_cut(&chat.messages, cfg.tail_tokens).filter(|&cut| cut > prev_upto)?;
        let digest = build_compaction_digest(&chat.messages[prev_upto..cut], loc)?;
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
        Some(RollPlan {
            cut,
            boundary_id: chat.messages[cut].id,
            rolls: chat.compaction.as_ref().map_or(0, |c| c.rolls) + 1,
            request: ChatRequest {
                continue_final: false,
                system: Some(summary_system_message(loc, cfg.summary_words)),
                messages: vec![ApiMessage::user(user)],
                sampling,
                tools: Vec::new(),
            },
        })
    }

    /// Launches a planned roll. `Err` — the engine is not ready, with a message
    /// for the human.
    fn spawn_roll(
        &mut self,
        chat_id: Uuid,
        plan: RollPlan,
        origin: CompactOrigin,
    ) -> Result<(), String> {
        let backend = self.engines.backend_if_ready(self.ui_locale())?;
        let cancel = CancellationToken::new();
        let sessions = self.session_budget();
        spawn_compact(
            backend,
            plan,
            chat_id,
            origin,
            cancel.clone(),
            self.ui_locale(),
            self.compact_tx.clone(),
            sessions,
        );
        self.begin_bg(BackgroundKind::Compaction, cancel);
        Ok(())
    }

    /// Applies a finished roll.
    pub(super) fn handle_compact_result(&mut self, res: CompactResult) {
        let CompactResult {
            chat_id,
            boundary_id,
            rolls,
            origin,
            text,
        } = res;
        // S6: a command the user just typed reports its failure at once and the
        // slot closes as a success — the failure streak exists for *silent* runs,
        // and alerting twice would be noise. A background roll does the opposite:
        // it stays quiet and lets the streak alert once at the third consecutive
        // failure.
        let outcome = match (text, origin) {
            (Ok(summary), _) => self.apply_compaction(chat_id, boundary_id, rolls, summary, origin),
            (Err(msg), CompactOrigin::Manual) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                Ok(())
            }
            (Err(msg), CompactOrigin::Auto) => Err(msg),
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
        origin: CompactOrigin,
    ) -> Result<(), String> {
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            let msg = self.ui_locale().t("ui.err.compact_timeout").to_string();
            if origin == CompactOrigin::Manual {
                let _ = self.evt_tx.send(AppEvent::Error(msg.clone()));
            }
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
/// result posted to `compact_tx`. The request streams on the **silent lane**
/// of the app's budget (docs/research/silent-tasks-budget.md §4.2), priced
/// from the request it was handed — the digest is sized by the conversation,
/// and it is the one silent request that fires when the conversation is at
/// its largest.
#[allow(clippy::too_many_arguments)]
fn spawn_compact(
    backend: Arc<dyn EngineBackend>,
    plan: RollPlan,
    chat_id: Uuid,
    origin: CompactOrigin,
    cancel: CancellationToken,
    loc: &'static crate::shared::i18n::Locale,
    compact_tx: UnboundedSender<CompactResult>,
    sessions: Arc<crate::shared::session_budget::SessionBudget>,
) {
    let RollPlan {
        boundary_id,
        rolls,
        request,
        cut: _,
    } = plan;
    tokio::spawn(async move {
        let need = sessions.price(
            super::generation::estimate_prompt_tokens(&request),
            0,
            request.sampling.max_tokens.map(|m| m as u64),
        );
        // Taken before the timeout starts: waiting behind an open stream is
        // not this roll's slowness. A wait cancelled (the app is quitting)
        // reports as the timeout would — nothing was summarized. A turn that
        // does not fit beside the roll's stream displaces it
        // (docs/research/silent-preemption.md §4.4): the same request is
        // made again, up to `SILENT_YIELDS_MAX` times; then the stream holds.
        let mut yields: u32 = 0;
        let text = loop {
            let Some(lane) = sessions
                .acquire_silent(need, &cancel, "compaction", yields < SILENT_YIELDS_MAX)
                .await
            else {
                let _ = compact_tx.send(CompactResult {
                    chat_id,
                    boundary_id,
                    rolls,
                    origin,
                    text: Err(loc.t("ui.err.compact_timeout").to_string()),
                });
                return;
            };
            let token = lane.stream_token();
            let request = request.clone();
            let collect = async {
                let mut stream = backend.chat_stream(request, token.clone()).await?;
                let mut text = String::new();
                let mut thoughts = String::new();
                let mut truncated = false;
                let mut cancelled = false;
                let mut failure: Option<String> = None;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        ChatChunk::Text(t) => text.push_str(&t),
                        // Kept as a salvage source for the same reason as auto-title:
                        // a model that never "finished thinking" leaves `content` empty.
                        ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                        ChatChunk::Finished(reason) => {
                            truncated = matches!(reason, crate::shared::api::FinishReason::Length);
                            cancelled =
                                matches!(reason, crate::shared::api::FinishReason::Cancelled);
                            break;
                        }
                        // Unlike the other background turns this one is reported to the
                        // user when they asked for it (`/compact` is owed an answer), so
                        // the reason is carried out instead of only logged — otherwise a
                        // roll killed mid-stream would report whatever fragment arrived
                        // as if it were a summary.
                        // A background turn: the retry is worth a log line (a flaky provider is
                        // otherwise invisible here) but has nothing to show — these turns have no
                        // chip of their own.
                        ChatChunk::Retry {
                            attempt,
                            max,
                            delay,
                        } => {
                            tracing::info!(attempt, max, ?delay, "retrying a a compaction turn");
                        }
                        ChatChunk::Error { message, .. } => failure = Some(message),
                        ChatChunk::ThoughtsSignature(_)
                        | ChatChunk::ToolCall(_)
                        | ChatChunk::Usage(_) => {}
                    }
                }
                if let Some(err) = failure {
                    anyhow::bail!("{err}");
                }
                Ok::<(String, String, bool, bool), anyhow::Error>((
                    text, thoughts, truncated, cancelled,
                ))
            };
            match tokio::time::timeout(COMPACT_TIMEOUT, collect).await {
                Ok(Ok((_, _, _, true))) if lane.displaced() => {
                    yields += 1;
                    tracing::info!(
                        chat = %chat_id,
                        yields,
                        "the roll's stream was displaced by an interactive one; made again"
                    );
                }
                // Cancelled by the app's own quit: a fragment is not a summary,
                // and the cancelled wait's wording says nothing was summarized.
                Ok(Ok((_, _, _, true))) => break Err(loc.t("ui.err.compact_timeout").to_string()),
                Ok(Ok((text, thoughts, truncated, false))) => {
                    if truncated {
                        // Don't hide a truncation — the same rule the conversation's
                        // own overflow follows (§1.2). The summary is still usable,
                        // so this is a log line, not a refusal.
                        tracing::warn!(
                            chat = %chat_id,
                            "the summary hit the token ceiling and was cut; raise the ceiling or lower the word limit"
                        );
                    }
                    break Ok(salvage_title_source(text, thoughts));
                }
                Ok(Err(err)) => {
                    break Err(loc.tf("ui.err.compact_failed", &[("err", &err.to_string())]));
                }
                Err(_) => {
                    cancel.cancel();
                    break Err(loc.t("ui.err.compact_timeout").to_string());
                }
            }
        };
        let _ = compact_tx.send(CompactResult {
            chat_id,
            boundary_id,
            rolls,
            origin,
            text,
        });
    });
}
