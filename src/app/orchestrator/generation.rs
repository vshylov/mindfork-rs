//! Assistant reply generation: send/regenerate/delete-exchange commands,
//! starting a turn, and the background task for the client-side agentic loop (spec §6.3).

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, ToolDecision};
use crate::entities::chat::DeletedCause;
use crate::entities::message::{
    Message, MessageFinish, MessageMetadata, MessageRole, ToolCallRecord,
};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::subagent::{RunKind, RunOutcome, SubagentRun};
use crate::features::tools::subagent::{
    CALL_SUBAGENT_ID, START_SUBAGENT_ID, SubagentArgs, withheld_from_subagent,
};
use crate::features::tools::{
    ChatEffect, ToolContext, ToolParams, ToolRegistry, TurnInfo, control, effective_tool_ids,
};
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, FinishReason,
    ThinkingAccumulator, ThinkingBlock, ThinkingRef, ToolCallAccumulator,
};
use crate::shared::config::ServerMode;
use crate::shared::session_budget::SessionBudget;
use crate::shared::tokens::estimate_prompt;

use super::Orchestrator;
use super::request::{
    PromptContext, RequestEnv, build_request, build_request_in, last_user_message_at,
};

/// Result of a completed generation task (internal channel).
/// What the generation task sends the orchestrator on its one channel: the
/// turn's progress while it runs, then its result. One channel, so every
/// progress message of a turn is delivered **before** its result — which is
/// what lets the orchestrator's in-flight table (docs/subagent-live.md §3.2)
/// be dropped at landing without a race.
pub(super) enum GenMessage {
    Progress { id: Uuid, progress: TurnProgress },
    Done(GenResult),
}

/// One step of a running turn, as the orchestrator's in-flight table needs
/// it (docs/subagent-live.md §3.1): the parent's rounds as they file, and a
/// sub-agent run's life — start, rounds, end — so the transcript is a row of
/// the list and openable while it runs (spec §9.3.2).
pub(super) enum TurnProgress {
    /// The parent's loop filed a round: its assistant message, then the tool
    /// messages — exactly what `file_round` pushed.
    RoundFiled(Vec<Message>),
    /// A sub-agent is about to run: the run as it will land — id, persona,
    /// title, `name`, `created_at`, `User(message)` — with no rounds and no
    /// outcome yet.
    ChildStarted(Box<SubagentRun>),
    /// The sub-agent's loop filed a round. Every `Child*` step names its
    /// run: several runs can be in flight at once (spec §9.3.2, stage 2 of
    /// docs/research/parallel-subagents.md), and the mirror keys on the id.
    ChildRoundFiled { run: Uuid, messages: Vec<Message> },
    /// One step of the turn's own stream (docs/history/subagent-live.md §8,
    /// last bullet): the orchestrator mirrors the round in progress, so the
    /// chat's feed can be rebuilt whole when the user comes back to it
    /// mid-turn. The screen gets the same step directly, as it always did.
    OwnStep(StreamStep),
    /// One step of the sub-agent's stream (§8): the same events its loop
    /// would send a feed, carried as progress so they stay in order with
    /// `ChildRoundFiled` on the one channel. The orchestrator keeps the
    /// round's partial and forwards the step to the screen while the
    /// transcript is the open conversation.
    ChildStep { run: Uuid, step: StreamStep },
    /// The sub-agent run's own token count so far (completion, reasoning).
    ChildTokens {
        run: Uuid,
        completion: u64,
        reasoning: Option<u32>,
    },
    /// Where a run stands (spec §11.10, docs/research/tasks-screen.md §4.3):
    /// the same report the status-bar chip gets straight from the task
    /// ([`AppEvent::SubagentProgress`]), carried to the orchestrator too and
    /// stored on the run's mirror — so a background run's position outlives
    /// the screen that happens to be open, and the tasks screen reads it
    /// off the seat.
    ChildProgress {
        run: Uuid,
        progress: crate::app::events::SubagentProgress,
    },
    /// A dialogue's next line begins (spec §9.13): which side of the
    /// transcript the coming stream belongs to, so the open transcript draws
    /// it in the right bubble. Also resets the round-in-progress partial.
    ChildLineStarted { run: Uuid, role: MessageRole },
    /// A dialogue **edited** its transcript — the director discarded or
    /// rewrote a line — so appending cannot express it: the full replacement.
    ChildTranscript { run: Uuid, messages: Vec<Message> },
    /// The run returned; the landed run carries the same fields.
    ChildEnded {
        run: Uuid,
        outcome: RunOutcome,
        finished_at: chrono::DateTime<chrono::Utc>,
        tokens: u64,
    },
    /// A `start_subagent` call (spec §9.3.2,
    /// docs/research/background-subagents.md §4.2): everything the run
    /// needs, built by the parent as for a group child, handed to the
    /// orchestrator to spawn **outside** the turn — its own task, its own
    /// token, the app's budget. The parent's record lands with the turn
    /// carrying a placeholder; [`super::Orchestrator::spawn_background_run`]
    /// fills it in when the run ends.
    BackgroundStart(Box<BackgroundStart>),
}

/// One step of a loop's stream, as the orchestrator mirrors it (see
/// [`TurnProgress::OwnStep`] / [`TurnProgress::ChildStep`]).
pub(super) enum StreamStep {
    Chunk(String),
    Thoughts(String),
    ToolStarted {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
        result: String,
        images: usize,
    },
    Continue,
    Rewrite,
}

pub(super) struct GenResult {
    pub(super) id: Uuid,
    pub(super) chat_id: Uuid,
    /// New domain messages (assistant with tool_calls, tool results, the final one) —
    /// in order of appearance; the orchestrator appends them to `Chat`.
    pub(super) messages: Vec<Message>,
    /// Tool effects (applied by the orchestrator — the owner of `Chat`).
    pub(super) effects: Vec<ChatEffect>,
    /// Messages discarded by the "rewrite" tool (`rewrite_current_message`):
    /// the previous (incorrect) version + its tool message. Kept in `Chat.deleted`
    /// for manual recovery; not part of inference/the feed. See spec §9.3.
    pub(super) deleted: Vec<Message>,
    /// What the **last** round of this turn actually cost, as the server counted
    /// it. The auto-compaction trigger reads it (spec §6.7); `None` when the
    /// provider reported no `usage`, and then the trigger stays quiet rather than
    /// guessing (sub-decision S2).
    pub(super) usage: Option<TurnUsage>,
    /// The message this turn **continues** (`/continue`, spec §6.4): the turn's
    /// first assistant message is folded into it in place rather than pushed.
    pub(super) continuation: Option<Uuid>,
}

/// The exact size of one round, as reported by the server's `usage`.
///
/// The last round of a turn is the largest — within a turn the history only
/// grows — and the *next* turn's prompt is close to `prompt + completion` plus
/// whatever the user types, which is what makes this a usable trigger input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TurnUsage {
    pub(super) prompt_tokens: u32,
    pub(super) completion_tokens: u64,
    /// The turn's largest prefill sample as the engine measured it (llama.cpp
    /// only; `None` elsewhere) — what the slow-prefill note is computed
    /// from (docs/research/slow-prefill-detection.md §3).
    pub(super) prefill: Option<crate::shared::api::contract::Prefill>,
}

impl TurnUsage {
    /// A lower bound on the next turn's prompt: this turn's prompt plus what was
    /// generated on top of it. The user's next message and any injection deltas
    /// come on top — which the threshold's headroom is there to absorb.
    pub(super) fn next_prompt_estimate(self) -> u64 {
        self.prompt_tokens as u64 + self.completion_tokens
    }
}

/// What `/continue` resumes (spec §6.4): the trailing partial assistant
/// message. `text` rides along for the echo filter — llama.cpp returns
/// prefill + continuation, and the stream must not re-deliver what is already
/// on screen (docs/research/continue-generation.md §4d, §7.1).
#[derive(Debug, Clone)]
pub(super) struct ContinuationSeed {
    pub(super) message_id: Uuid,
    pub(super) text: std::sync::Arc<str>,
}

/// Strips a server echo of the continuation seed from the front of a round's
/// text stream. Bytes are withheld while they keep matching the seed; on a
/// full match the echo is dropped and everything after it flows; on the first
/// mismatch the withheld bytes plus the current delta flow as real content —
/// a non-echoing server (a future llama.cpp, vLLM) loses nothing. A stream
/// that dies while still matching was echoing (the only servers this filter
/// is enabled for echo, and real content diverges at the first new byte), so
/// the withheld bytes are dropped rather than appended twice.
pub(super) struct EchoFilter {
    seed: std::sync::Arc<str>,
    matched: usize,
    decided: bool,
}

impl EchoFilter {
    pub(super) fn new(seed: std::sync::Arc<str>) -> Self {
        Self {
            seed,
            matched: 0,
            decided: false,
        }
    }

    /// The visible part of `delta` — empty while the echo is being consumed,
    /// possibly prefixed with previously withheld bytes on a mismatch.
    pub(super) fn push(&mut self, delta: &str) -> String {
        if self.decided {
            return delta.to_string();
        }
        let remaining = &self.seed.as_bytes()[self.matched..];
        let n = delta.len().min(remaining.len());
        if delta.as_bytes()[..n] == remaining[..n] {
            self.matched += n;
            if self.matched == self.seed.len() {
                self.decided = true;
                // `n` ends exactly where the seed does — a char boundary of
                // the seed, and the bytes match, so of `delta` too.
                return delta[n..].to_string();
            }
            String::new()
        } else {
            self.decided = true;
            // The withheld bytes are byte-identical to the seed's prefix, and
            // `matched` only ever advanced by whole deltas — a char boundary.
            format!("{}{delta}", &self.seed[..self.matched])
        }
    }
}

impl Orchestrator {
    pub(super) fn handle_send(&mut self, text: String) {
        if !self.gen_state.is_idle() {
            return;
        }
        let text = text.trim().to_string();
        // An empty message is normally nothing to send — unless images are staged, in
        // which case "look at this" with no words is a complete request (spec §9.10).
        let has_staged_images = self
            .active_id
            .and_then(|id| self.staged_images.get(&id))
            .is_some_and(|staged| !staged.is_empty());
        if text.is_empty() && !has_staged_images {
            return;
        }
        let Some(active_id) = self.active_id else {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.no_active_chat").into(),
            ));
            return;
        };
        // A sub-agent transcript is read-only (spec §11.2): the screen refuses
        // first; this is the route-independent answer, with the text returned.
        if self.parent_of(active_id).is_some() {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.read_only_chat").into(),
            ));
            let _ = self.evt_tx.send(AppEvent::RestoreInput(text));
            return;
        }
        let Some(backend) = self.ready_backend() else {
            // Server not ready: the UI already cleared the input box — return the
            // text so the user doesn't lose the message (the error is shown separately).
            let _ = self.evt_tx.send(AppEvent::RestoreInput(text));
            return;
        };

        // Staging is consumed here and nowhere else: the images become part of the
        // message, and from this point `/image remove` can no longer reach them. Taken
        // only after every early return above, so a failed send leaves them staged.
        let images = self.take_staged_images(active_id);

        // Add the user's message to the history and echo it in the feed. The UI
        // cleared the input box on send — also clear the chat's saved draft.
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.push_message(Message::user(&text).with_images(images));
            chat.draft.clear();
        }
        self.mark_dirty(active_id);
        let _ = self.evt_tx.send(AppEvent::UserMessage(text));

        self.start_generation(active_id, backend, None);
        // Automatic titling at the `AfterUserMessage` point (spec §11.2), fired
        // for the conversation's first user message — **after** the reply's own
        // request, so on a single-slot server the title never queues ahead of
        // the answer (docs/history/auto-chat-title.md D3).
        if self
            .chats
            .iter()
            .find(|c| c.id == active_id)
            .is_some_and(|c| crate::features::rename_chat::is_first_user_message(&c.messages))
        {
            self.maybe_auto_title(
                active_id,
                crate::shared::config::AutoTitleMode::AfterUserMessage,
            );
        }
    }

    /// Regenerates the last assistant reply (spec §11.7): deletes everything after
    /// the last user message (the old reply + tool messages) and starts generation
    /// again from the same request. The feed is rebuilt via a re-emit of
    /// `ChatActivated`. Ignored during generation.
    pub(super) fn handle_regenerate(&mut self) {
        if !self.gen_state.is_idle() {
            return;
        }
        // Unconditional (not a setting): the reply being spoken is about to vanish.
        self.stop_tts();
        let Some(active_id) = self.active_id else {
            return;
        };
        // Check server readiness BEFORE truncating the history: otherwise, on a
        // not-yet-ready server (model loading), the old reply would be wiped out
        // and the new one wouldn't arrive.
        let Some(backend) = self.ready_backend() else {
            return;
        };
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            // A task notification is a user-side row (spec §9.3.2): the
            // reply it woke is redone, the notification kept.
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User || m.is_notification())
            else {
                return; // no user message — nothing to regenerate
            };
            // Save what's deleted (the assistant's reply + the round's tool messages)
            // and the input draft, for manual recovery (spec §11.7).
            let draft = chat.draft.clone();
            let removed = chat.messages.split_off(idx + 1);
            chat.record_deleted(removed, draft, DeletedCause::Regenerate);
            chat.modified_at = chrono::Utc::now();
        }
        self.mark_dirty(active_id);
        self.activate(active_id); // rebuild the feed without the old reply
        self.emit_chat_list();
        // A background run whose exchange just left the live messages ends
        // with it (research fork F8).
        self.cancel_orphaned_background_runs(active_id);
        self.start_generation(active_id, backend, None);
    }

    /// Resumes the last interrupted assistant reply in place (`/continue`,
    /// spec §6.4): the history goes out with the partial as its trailing
    /// assistant message and the engine continues it (assistant prefill),
    /// everything that arrives appending into the same `Message`. A turn
    /// interrupted **between** tool rounds — the chat ends with tool results —
    /// resumes the agentic loop instead, with nothing to prefill. Every
    /// refusal answers with the route that works (docs/lessons.md §4); the
    /// cheap state gates (`generating`, no chat, a read-only transcript) were
    /// already answered by the typed route on the screen.
    pub(super) fn handle_continue(&mut self) {
        if !self.gen_state.is_idle() {
            return;
        }
        let Some(active_id) = self.active_id else {
            return;
        };
        if self.parent_of(active_id).is_some() {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.read_only_chat").into(),
            ));
            return;
        }
        // The capability gate first — its answer does not depend on the server
        // being up, and a cloud user should hear "cannot" rather than wait out
        // a readiness check to hear it (single source of truth: research §2).
        let model = self.effective_model_name();
        if !self
            .config
            .engine
            .mode
            .supports_continuation(model.as_deref())
        {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.cmd.continue_unsupported").into(),
            ));
            return;
        }
        let seed = {
            let Some(chat) = self.chats.iter().find(|c| c.id == active_id) else {
                return;
            };
            match chat.messages.last() {
                // Interrupted between rounds: the loop resumes on the recorded
                // tool results — the ordinary agentic request shape.
                Some(m) if m.role == MessageRole::Tool => None,
                Some(m) if m.role == MessageRole::Assistant => {
                    if m.text.is_empty() {
                        // Cut inside the reasoning, before any visible text —
                        // no provider can resume a thought over a chat API (F4).
                        let _ = self.evt_tx.send(AppEvent::Error(
                            self.ui_locale().t("ui.cmd.continue_thoughts").into(),
                        ));
                        return;
                    }
                    let finish = m.metadata.as_ref().and_then(|md| md.finish);
                    if finish == Some(crate::entities::message::MessageFinish::Stop) {
                        let _ = self.evt_tx.send(AppEvent::Error(
                            self.ui_locale().t("ui.cmd.continue_complete").into(),
                        ));
                        return;
                    }
                    // `Cancelled`/`Error`/`Length`, and `None` for messages
                    // stored before the bookkeeping existed (fork F1).
                    Some(ContinuationSeed {
                        message_id: m.id,
                        text: std::sync::Arc::from(m.text.as_str()),
                    })
                }
                _ => {
                    let _ = self.evt_tx.send(AppEvent::Error(
                        self.ui_locale().t("ui.cmd.continue_nothing").into(),
                    ));
                    return;
                }
            }
        };
        let Some(backend) = self.ready_backend() else {
            return;
        };
        // A tool-result tail resumes as an ordinary next round (`seed` is
        // `None`); a text tail rides the prefill.
        self.start_generation(active_id, backend, seed);
    }

    /// Deletes the last exchange: the assistant's reply together with the user
    /// message that triggered it (spec §11.7). The user's text is returned to the
    /// input box (`RestoreInput`) so it can be edited and resent.
    /// Ignored during generation.
    pub(super) fn handle_delete_last(&mut self) {
        if !self.gen_state.is_idle() {
            return;
        }
        // Unconditional (not a setting): the exchange being spoken is about to vanish.
        self.stop_tts();
        let Some(active_id) = self.active_id else {
            return;
        };
        let user_text;
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            // A task notification is a user-side row (spec §9.3.2): the
            // exchange it woke goes with it, and nothing returns to the box.
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User || m.is_notification())
            else {
                return; // no user message — nothing to delete
            };
            user_text = if chat.messages[idx].is_notification() {
                String::new()
            } else {
                chat.messages[idx].text.clone()
            };
            // Save what's deleted (the user message + the assistant's reply) and the
            // input draft BEFORE returning the user's text to the field — for manual
            // recovery (spec §11.7).
            let draft = chat.draft.clone();
            let removed = chat.messages.split_off(idx);
            chat.record_deleted(removed, draft, DeletedCause::DeleteExchange);
            chat.modified_at = chrono::Utc::now();
        }
        self.mark_dirty(active_id);
        self.activate(active_id); // rebuild the feed without the deleted exchange
        self.emit_chat_list();
        self.cancel_orphaned_background_runs(active_id);
        let _ = self.evt_tx.send(AppEvent::RestoreInput(user_text));
    }

    /// Returns the engine if the chat server is ready; otherwise emits a clear
    /// error into the chat feed (`AppEvent::Error`) and returns `None`. Gates both
    /// send and regenerate — so the request doesn't go to a still-loading server
    /// (otherwise 503 → "engine returned an error status"). For chat-list
    /// operations (auto-title) the error must go into the overlay — there
    /// [`EngineManager::backend_if_ready`](super::engines::EngineManager) is used directly.
    pub(super) fn ready_backend(&self) -> Option<Arc<dyn EngineBackend>> {
        match self.engines.backend_if_ready(self.ui_locale()) {
            Ok(backend) => Some(backend),
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                None
            }
        }
    }

    /// Starts generation from the chat's current state (the history is already
    /// prepared: either the user's message was appended, or the old reply was
    /// truncated). The shared part for sending a new message, regenerating,
    /// and `/continue` — which passes the `continuation` seed so the turn
    /// prefills the trailing partial and appends into it (spec §6.4).
    pub(super) fn start_generation(
        &mut self,
        active_id: Uuid,
        backend: Arc<dyn EngineBackend>,
        continuation: Option<ContinuationSeed>,
    ) {
        self.start_generation_woken(active_id, backend, continuation, false);
    }

    /// The turn the app starts on a background run's task notification
    /// (spec §9.3.2): nobody typed anything, so an empty first generation is
    /// worth one muted re-ask rather than an empty bubble (fork F11).
    pub(super) fn start_woken_generation(
        &mut self,
        active_id: Uuid,
        backend: Arc<dyn EngineBackend>,
    ) {
        self.start_generation_woken(active_id, backend, None, true);
    }

    /// [`Self::start_generation`] with the one flag its two entry points
    /// differ by.
    fn start_generation_woken(
        &mut self,
        active_id: Uuid,
        backend: Arc<dyn EngineBackend>,
        continuation: Option<ContinuationSeed>,
        woken: bool,
    ) {
        // Speech stops per the setting (off by default: listening to the reply
        // while the next one is being written is legitimate). See spec §11.9.
        if self.config.tts.stop_on_generation_start {
            self.stop_tts();
        }
        // A snapshot at the start of the turn: sampling, available tools, context.
        let sampling = self.effective_sampling(active_id);
        let Some(chat_ref) = self.chats.iter().find(|c| c.id == active_id) else {
            return;
        };
        let profile_id = chat_ref.profile_id;
        let profile_lang = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.language)
            .unwrap_or_default();
        let enabled = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.enabled_tools.clone())
            .unwrap_or_default();
        // Where this chat's verbatim history starts, if compression folded
        // anything away. It decides three things at once, which is the point:
        // what the request carries, whether the summary block is in the prompt,
        // and whether the read-back tools are offered (spec §6.7, S12).
        let history_upto = chat_ref
            .compaction_view(self.config.compaction.enabled)
            .map(|(_, upto)| upto);
        // The chat's attached code project (spec §9.12). Like the folded range
        // above, it decides two things at once — whether the `code_*` tools are
        // offered and whether the prompt carries the workspace block — so the
        // block can never name a tool the turn does not have.
        let workspace = chat_ref.workspace.clone();
        // Where the editing tools record what a file looked like before they
        // changed it. Derived here, with the workspace, so the two cannot
        // disagree about which chat is being edited.
        let workspace_journal = workspace.as_ref().map(|_| {
            self.storage
                .json()
                .workspace_dir()
                .join(active_id.to_string())
        });
        // The effective set = profile ∩ global switches (spec §9.4).
        let allowed = effective_tool_ids(
            &enabled,
            &crate::features::tools::ToolGates {
                web: self.config.tools.web_enabled,
                python: self.config.tools.python_enabled,
                fs: self.config.tools.fs_enabled,
                mcp: self.config.mcp.enabled,
                background: self.config.tools.subagent_background,
                history: history_upto.is_some(),
                workspace: workspace.is_some(),
                // Which slots carry a line, so a `code_test` with nothing to
                // run is never advertised (spec §9.12).
                workspace_commands: workspace
                    .as_ref()
                    .map(crate::features::tools::code::WorkspaceCommands::of)
                    .unwrap_or_default(),
                sampling_provider: self.config.engine.mode.cloud_provider(),
            },
        );
        // Does this turn actually offer the read-back tools? A folded range
        // normally implies them, but a profile can have them switched off — and
        // then the summary block must not name them (spec §6.7).
        let history_tools = allowed.iter().any(|t| {
            t == crate::features::tools::history::HISTORY_READ_ID
                || t == crate::features::tools::history::HISTORY_SEARCH_ID
        });
        let profile_loc = crate::shared::i18n::locale(profile_lang);
        let schemas = self.registry.schemas_for(&allowed, profile_loc);
        // Copied out before the `chat_mut` borrow below (config can't be read
        // while `Chat` is mutably borrowed). `AttachmentSettings` is `Copy`.
        let attach_cfg = self.config.attachments;
        let compact_cfg = self.config.compaction.clone();
        // Which attached files have a semantic index — the pinned block only
        // offers `attachment_search` for those (spec §9.7). One indexed lookup,
        // and only when the chat has attachments at all.
        let indexed: Vec<Uuid> = if chat_ref.attachments.is_empty() {
            Vec::new()
        } else {
            self.storage
                .db()
                .attachment_indexed_ids(active_id)
                .unwrap_or_default()
        };
        // The other chats of this profile, for `chat_search`/`chat_read`
        // (spec §9.11) — built only when the turn actually offers the pair,
        // like the history render below. The snapshot is those tools' whole
        // world, so the scope (this profile, not this chat, nothing hidden) is
        // decided in one place: `snapshot_other_chats`.
        let chat_tools_on = allowed.iter().any(|t| {
            t == crate::features::tools::chats::CHAT_SEARCH_ID
                || t == crate::features::tools::chats::CHAT_READ_ID
        });
        let other_chats: Vec<crate::features::tools::chats::ChatRef> = if chat_tools_on {
            crate::features::tools::chats::snapshot_other_chats(&self.chats, profile_id, active_id)
        } else {
            Vec::new()
        };

        // The profile's "self-model" at the start of the turn. Injection into the
        // system prompt happens only if the profile enabled get_self_model (opt-in);
        // the injection itself (observations by relevance to the last message +
        // recency) happens in the generation task (needs async embedding). See
        // docs/history/narrative-as-notes.md (Tier 2).
        let self_model_params =
            crate::entities::self_model::SelfModelParams::from_settings(&self.config.self_model);
        let inject_enabled = enabled
            .iter()
            .any(|t| t == crate::features::tools::self_model::GET_SELF_MODEL_ID);
        // A one-time idempotent migration of the old "self-model" narrative into
        // self-notes (@self). Best-effort. See docs/history/narrative-as-notes.md, step 6.
        if inject_enabled {
            crate::features::tools::notes::migrate_self_narrative(&self.storage, profile_id);
        }
        let self_model = self.storage.db().self_model_get(profile_id).ok().flatten();

        // The turn's cancellation token is created before the tool context: its
        // clone goes into `ToolContext.cancel` (long-running tools — MCP/network —
        // are interrupted via Esc).
        let cancel = CancellationToken::new();

        // Resolved once and used three times: `ToolContext.model_name` (what
        // `get_llm_name` answers, spec §9.14), the `GenerationStarted` event
        // below (the live bubble's header) and `GenSpawn.model_name` (the
        // finished message's metadata). One read, so no pair of them can
        // disagree. In `external` mode with no model named in settings this is
        // what the engine said it is running (see `model_name::ModelDiscovery`)
        // — the message records the model that actually answered, not a blank.
        let model_name = self.effective_model_name();
        let engine_mode = self.config.engine.mode;

        // Build the request/context + take the last user message (for relevance-
        // based injection of observations in the task).
        // The turn's session budget (spec §11.6): every stream of the turn —
        // the loops' own and a tool's summary request (`ToolContext::sessions`)
        // — takes a permit of this one budget, and under a shared KV pool
        // (`session_pool`, admission-by-budget §4.4) a reservation of it, so
        // it is made before the context and shared with the task.
        // App-wide since background runs (docs/research/background-subagents.md
        // §4.7): the same `Arc` a run out in the background holds, so the
        // run and this turn take turns under one permit count and one pool.
        let sessions = self.session_budget();
        let mut request;
        let ctx;
        let last_user;
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            request = build_request(
                chat,
                sampling.clone(),
                schemas,
                &PromptContext {
                    attachments: &attach_cfg,
                    compaction: &compact_cfg,
                    indexed: &indexed,
                    history_tools,
                    // A project can be attached while the profile has some or
                    // all of the tools switched off; the block describes what
                    // this turn actually has, and nothing else.
                    offered_tools: &allowed,
                    loc: profile_loc,
                },
            );
            last_user = chat
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::User)
                .map(|m| m.text.clone())
                .unwrap_or_default();
            // `turn` is built by the last access to `chat`; after this the `chat`
            // borrow ends and `self` (deps/config) can be read for `new`.
            let turn = TurnInfo {
                profile_id,
                chat_id: active_id,
                system_message: chat.system_message.clone(),
                effective_sampling: sampling,
                last_user_message_at: last_user_message_at(chat),
                // The turn's attachment snapshot — what `attachment_read` sees
                // (spec §9.7). `Arc` — the context is cloned per call and the
                // texts can be large.
                attachments: std::sync::Arc::from(chat.attachments.clone()),
                // The folded-away range, rendered for `history_read`/
                // `history_search` (spec §6.7). Rendered only when the tools are
                // actually in this turn's set: with none of them offered, the
                // work would be pure cost — and a chat with no compaction skips
                // it entirely, which is every chat until the first roll.
                history: history_upto
                    .filter(|_| history_tools)
                    .and_then(|upto| {
                        crate::features::compaction::HistoryView::render(
                            &chat.messages[..upto],
                            profile_loc,
                        )
                    })
                    .map(std::sync::Arc::new),
                other_chats: std::sync::Arc::from(other_chats),
                workspace: workspace.clone(),
                workspace_journal,
                lang: profile_lang,
                cancel: cancel.clone(),
                model_name: model_name.clone(),
                engine_mode,
                sessions: Some(sessions.clone()),
                silent_lane: false,
            };
            ctx = ToolContext::new(
                self.tool_deps(backend.clone()),
                ToolParams::from_config(&self.config),
                turn,
            );
        }

        // The history already ends with the partial being continued —
        // `build_request` sent it as the trailing assistant message; the flag
        // is what makes the wire ask the server to continue it in place.
        request.continue_final = continuation.is_some();

        let id = Uuid::new_v4();
        let _ = self.evt_tx.send(AppEvent::GenerationStarted {
            generation_id: id,
            model: model_name.clone(),
            continuation: continuation.is_some(),
        });
        self.gen_state.begin(id, cancel.clone());
        self.inflight = Some(super::InflightTurn {
            generation: id,
            chat: active_id,
            rounds: Vec::new(),
            partial: Default::default(),
            children: Vec::new(),
            continuation: continuation.is_some(),
        });
        // The confirmation channel for this turn (fork F8). The sender is kept
        // next to the turn id so a reply arriving for an older turn — the user
        // pressed a key just as the turn was cancelled and a new one began — is
        // dropped instead of unblocking the wrong call.
        let (confirm_tx, confirm_rx) = tokio::sync::mpsc::unbounded_channel();
        self.confirm = Some((id, confirm_tx));
        spawn_generation(GenSpawn {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            cancel,
            confirm_dangerous: self.config.tools.confirm_dangerous,
            image_cfg: self.config.images,
            confirm_rx,
            id,
            chat_id: active_id,
            max_rounds: self.config.max_tool_rounds,
            workspace_max_rounds: self.config.workspace.max_rounds,
            subagent: SubagentLimits::from_config(&self.config.tools),
            sessions,
            concurrent_calls: self.config.engine.active_concurrent_calls(),
            compaction_summary: self
                .chats
                .iter()
                .find(|c| c.id == active_id)
                .and_then(|c| c.compaction_view(self.config.compaction.enabled))
                .map(|(s, _)| s.to_string()),
            allowed,
            self_model,
            self_model_params,
            inject_enabled,
            maintenance_protocol: self.config.self_model.maintenance_protocol,
            last_user,
            engine_mode,
            model_name,
            ui_loc: self.ui_locale(),
            compaction_enabled: self.config.compaction.enabled,
            continuation,
            woken,
            evt_tx: self.evt_tx.clone(),
            done_tx: self.done_tx.clone(),
            background: self.background_slots.clone(),
        });
    }

    /// Did this turn deliver the conversation's **first** substantive reply?
    ///
    /// Asked **before** the turn's result is applied: afterwards the reply is
    /// part of the history and the question can no longer be asked.
    /// Regenerating the first reply re-fires by construction — the truncation
    /// removed the only reply, so the next one is again the first
    /// (spec §11.2, D2).
    fn is_first_reply(&self, res: &GenResult) -> bool {
        let substantive = res
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty());
        substantive
            && self
                .chats
                .iter()
                .find(|c| c.id == res.chat_id)
                .is_some_and(|c| {
                    c.messages.iter().any(|m| m.role == MessageRole::User)
                        && !crate::features::rename_chat::has_assistant_reply(&c.messages)
                })
    }

    /// The slow-prefill note (docs/research/slow-prefill-detection.md §3.3):
    /// from the turn's largest prefill sample as the engine measured it, the
    /// seconds a stream cancelled during its prompt would hold its slot at
    /// the batch this server runs — the managed launch line's, or llama.cpp's
    /// default for an external server — and, when that is worth saying, one
    /// feed note per server session naming the figures and the one change:
    /// the *Batch (-b)* field for a managed server, the launch line for an
    /// external one. A cloud, a server without timings, a batch at the knee
    /// or a prompt too short to measure say nothing.
    pub(super) fn note_slow_prefill(
        &mut self,
        prefill: Option<crate::shared::api::contract::Prefill>,
    ) {
        use crate::shared::api::managed::{LLAMA_DEFAULT_BATCH, launched_batch, prefill_hold};
        use crate::shared::config::ServerMode;
        let Some(prefill) = prefill else { return };
        let managed = &self.config.engine.managed;
        let (batch, key) = match self.config.engine.mode {
            ServerMode::Managed => (
                launched_batch(managed.batch_size, managed.gpu_layers),
                "ui.notice.slow_prefill_managed",
            ),
            ServerMode::External => (LLAMA_DEFAULT_BATCH, "ui.notice.slow_prefill_external"),
            _ => return,
        };
        let Some(hold) = prefill_hold(batch, prefill) else {
            return;
        };
        let tps = prefill.tokens_per_second().unwrap_or_default().round() as u32;
        tracing::info!(
            tokens = prefill.tokens,
            ms = prefill.ms,
            tps,
            batch,
            hold,
            "slow prefill: a cancelled stream would hold its slot for a batch"
        );
        if !self.engines.claim_prefill_note() {
            return;
        }
        let loc = self.ui_locale();
        let _ = self.evt_tx.send(AppEvent::Notice(loc.tf(
            key,
            &[
                ("tps", &tps.to_string()),
                ("hold", &hold.to_string()),
                ("batch", &batch.to_string()),
            ],
        )));
    }

    pub(super) fn handle_done(&mut self, res: GenResult) {
        // Apply only the result of the current generation (protection against
        // stale ones): finish() transitions to Idle only on a matching id.
        if !self.gen_state.finish(res.id) {
            return;
        }
        // The turn is over: drop its confirmation sender, so `confirm` really is
        // `None` between turns as its doc says. Nothing depends on this — a reply
        // arriving now is dropped by the `generation_id` guard, and the receiver
        // is gone with the task — but a field that outlives what it describes is
        // an invitation to reason wrongly about it later.
        self.confirm = None;
        let mut res = res;
        self.carry_inflight_rename(&mut res);
        if res.messages.is_empty() && res.effects.is_empty() && res.deleted.is_empty() {
            // A turn that landed nothing still ends the wait of a result
            // that arrived meanwhile (docs/research/background-subagents.md §4.4).
            self.land_pending_runs(res.chat_id);
            return;
        }
        // Did the model edit the "self-model" via its own tools this turn? If so —
        // signal `SelfModelChanged` (an open `F3` screen will re-fetch the snapshot).
        let self_model_touched = res.messages.iter().any(|m| {
            m.tool_calls
                .iter()
                .any(|tc| crate::features::tools::self_model::is_self_model_tool(&tc.name))
        });
        // Read before the apply below — afterwards the reply is part of the
        // history and the question can no longer be asked.
        let first_reply = self.is_first_reply(&res);
        // The exchange's language model, for the profile's history (spec
        // §9.14). Also read before the apply: the messages move into the chat
        // below, and a `/continue` tail is folded away by `land_continuation`.
        // Every round of one turn carries the same frozen name (the single
        // `effective_model_name` read), so the newest metadata suffices; a
        // turn whose engine did not say a name records nothing (`model: None`).
        let turn_llm: Option<(String, crate::shared::config::ServerMode)> = res
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == MessageRole::Assistant)
            .find_map(|m| {
                let md = m.metadata.as_ref()?;
                Some((md.model.clone()?, md.mode))
            });
        // Sub-agent runs that landed with this turn and have a reply to name
        // themselves by — titled below, once they are part of the chat
        // (docs/research/subagent-chats.md §3.10).
        let landed_runs: Vec<Uuid> = res
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter_map(|r| r.subagent.as_deref())
            .filter(|run| run.final_reply().is_some())
            .map(|run| run.id)
            .collect();
        // Attachments a tool produced this turn (spec §9.9) — applied below,
        // outside the `chat` borrow.
        let mut attached: Vec<crate::entities::attachment::Attachment> = Vec::new();
        if let Some(chat) = self.chat_mut(res.chat_id) {
            // Discarded by the "rewrite" tool — into the deleted archive (manual
            // recovery by editing JSON), like Ctrl+E/Ctrl+R. See spec §9.3, §11.7.
            if !res.deleted.is_empty() {
                let deleted = std::mem::take(&mut res.deleted);
                chat.record_deleted(deleted, String::new(), DeletedCause::Rewrite);
            }
            land_continuation(chat, &mut res);
            for msg in res.messages {
                chat.push_message(msg);
            }
            // Tool effects are applied by the orchestrator (the owner of Chat, §4.4.2).
            attached = apply_effects(chat, res.effects);
            self.mark_dirty(res.chat_id);
            self.emit_chat_list();
        }
        // The same path `/file attach` takes — one place decides what attaching
        // entails (spec §9.7). A turn cancelled after the tool ran still gets
        // here: the transcript was already paid for.
        for a in attached {
            self.insert_attachment(res.chat_id, a);
        }
        // A background run that ended while this turn ran: its notification
        // goes after the turn's rows, and the assistant may be woken on it
        // (docs/research/background-subagents.md §4.4). Before the silent
        // follow-ups, which a turn in flight makes wait their turn.
        self.land_pending_runs(res.chat_id);
        // The profile's language-model history (spec §9.14): an exchange just
        // completed, so append a record when the model differs — by name or
        // mode — from the newest one (the store decides, `llm_history_note`).
        if let Some((model, mode)) = turn_llm {
            self.record_llm_history(res.chat_id, model, mode);
        }
        if self_model_touched {
            let _ = self.evt_tx.send(AppEvent::SelfModelChanged);
        }
        // The conversation's first reply just landed — maybe give the chat its
        // name (`interface.auto_title` at `AfterAssistantReply`, spec §11.2).
        // Ahead of the four background follow-ups only because it is the one
        // the user can see happen; none of the five depend on each other.
        if first_reply {
            self.maybe_auto_title(
                res.chat_id,
                crate::shared::config::AutoTitleMode::AfterAssistantReply,
            );
        }
        // A sub-agent transcript's whole life lands at once, so both trigger
        // points are now — one request per landed run (spec §9.3.2, §11.2).
        for run in landed_runs {
            self.maybe_auto_title_run(run);
        }
        // Maybe the conversation is approaching the model's context window
        // (spec §6.7) — it reads what this turn actually cost, the freshest
        // measurement available. **Ahead** of the three loops below: the
        // silent lane runs one request at a time in the order they were
        // asked for, and the roll is the one silent task that protects the
        // *next* turn (docs/research/silent-tasks-budget.md §4.5, fork F6).
        // The slow-prefill note, from the engine's own clock over the prompt
        // (docs/research/slow-prefill-detection.md §3.3): once per server
        // session, before the roll it may be about to advise on.
        self.note_slow_prefill(res.usage.as_ref().and_then(|u| u.prefill));
        self.maybe_auto_compact(res.chat_id, res.usage);
        // Then background auto-reflection (Tier 3), notes auto-consolidation
        // ("sleep", Tier 3), and/or self-model auto-consolidation ("sleep"
        // for the self-model, stage A1 — docs/history/self-model-consolidation.md).
        self.maybe_auto_reflect(res.chat_id);
        self.maybe_auto_consolidate(res.chat_id);
        self.maybe_auto_self_consolidate(res.chat_id);
    }

    /// Appends a record to the chat's profile's language-model history when
    /// the exchange's model differs from the newest record (spec §9.14; the
    /// dedup lives in [`crate::shared::storage::Db::llm_history_note`]).
    /// Best-effort, like the reflection follow-ups around its call site: a
    /// failed write is logged and never fails the turn (docs/lessons.md §8 —
    /// prefer best-effort on paths that protect data).
    fn record_llm_history(
        &self,
        chat_id: Uuid,
        model: String,
        mode: crate::shared::config::ServerMode,
    ) {
        let Some(profile_id) = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .map(|c| c.profile_id)
        else {
            return;
        };
        let rec = crate::entities::profile::LlmChange {
            changed_at: chrono::Utc::now(),
            model,
            mode,
        };
        if let Err(err) = self.storage.db().llm_history_note(profile_id, &rec) {
            tracing::warn!(error = %err, profile = %profile_id,
                "failed to record the language-model history");
        }
    }

    /// Retires the in-flight mirror (docs/history/subagent-live.md §3.6): every
    /// progress message preceded the turn's result on the one channel. What it
    /// still knows — a title the user gave the running transcript — is carried
    /// onto the landed run's record here, then it goes.
    fn carry_inflight_rename(&mut self, res: &mut GenResult) {
        let Some(inflight) = self.inflight.take().filter(|t| t.generation == res.id) else {
            return;
        };
        // Every running transcript the user named while it ran — there can
        // be several in one turn now (spec §9.3.2).
        for edited in inflight
            .children
            .iter()
            .map(|c| &c.run)
            .filter(|r| r.renamed_manually)
        {
            if let Some(run) = res
                .messages
                .iter_mut()
                .flat_map(|m| m.tool_calls.iter_mut())
                .filter_map(|r| r.subagent.as_deref_mut())
                .find(|r| r.id == edited.id)
            {
                run.title = edited.title.clone();
                run.renamed_manually = true;
            }
        }
    }

    /// Routes the user's answer into the turn that asked (spec §9.8, fork F8).
    ///
    /// A reply for a turn that is no longer in flight is **dropped**: the user
    /// can press a key at the exact moment a turn is cancelled and the next one
    /// starts, and unblocking the new turn's call with the old turn's answer
    /// would run a tool nobody looked at. Same guard as `AppEvent::TokenUsage`'s
    /// `generation_id`.
    pub(super) fn handle_confirm_tool(
        &mut self,
        generation_id: Uuid,
        call_id: String,
        decision: ToolDecision,
    ) {
        let Some((id, tx)) = &self.confirm else {
            return;
        };
        if *id != generation_id {
            tracing::debug!(reply_for = %generation_id, in_flight = %id,
                "dropped a tool confirmation from a finished turn");
            return;
        }
        let _ = tx.send((call_id, decision));
    }
}

/// `/continue`: the turn's first assistant message finishes the seed in place
/// (fork F8) — same id, same bubble, no seam — instead of opening a message of
/// its own. A turn that continues nothing passes through untouched.
fn land_continuation(chat: &mut crate::entities::chat::Chat, res: &mut GenResult) {
    let Some(seed_id) = res.continuation else {
        return;
    };
    let Some(pos) = res
        .messages
        .iter()
        .position(|m| m.role == MessageRole::Assistant)
    else {
        return;
    };
    let round = res.messages.remove(pos);
    match chat.messages.iter_mut().rfind(|m| m.id == seed_id) {
        Some(seed) => merge_continuation(seed, round),
        // The seed vanished mid-turn (edited away by hand): keep the round as
        // its own message rather than lose the text.
        None => res.messages.insert(pos, round),
    }
}

/// Applies one turn's tool effects to `chat` and returns the attachments among
/// them.
///
/// An attachment needs the whole orchestrator (index prune, background
/// indexing, the feed note), which cannot be had while `chat` is borrowed — so
/// it is collected here and applied by the caller once the borrow ends.
fn apply_effects(
    chat: &mut crate::entities::chat::Chat,
    effects: Vec<ChatEffect>,
) -> Vec<crate::entities::attachment::Attachment> {
    let mut attached = Vec::new();
    for effect in effects {
        match effect {
            ChatEffect::SetSystemMessage(s) => chat.system_message = s,
            ChatEffect::SetSamplingOverride(s) => chat.sampling_override = Some(*s),
            ChatEffect::AddAttachment(a) => attached.push(*a),
        }
    }
    attached
}

/// Which limit ended a turn — the two are enforced together and the message has
/// to name the right one, or it sends the user to a setting that was not the
/// problem (docs/lessons.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundLimit {
    /// `max_tool_rounds` (spec §6.3) — the budget for *external* work.
    Tools,
    /// `workspace.max_rounds` (spec §9.12) — the budget for work inside the
    /// attached project, which is exempt from the one above.
    Workspace,
}

/// Parameters for launching the generation task (the agentic loop).
struct GenSpawn {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    ctx: ToolContext,
    request: ChatRequest,
    cancel: CancellationToken,
    id: Uuid,
    chat_id: Uuid,
    max_rounds: u32,
    /// How many rounds this turn may spend entirely inside the attached project
    /// (`config.workspace.max_rounds`; 0 — no limit). Separate from
    /// `max_rounds` because the `code_*` family is exempt from that one, and
    /// "exempt" is not "unbounded" (spec §9.12).
    workspace_max_rounds: u32,
    /// A sub-agent run's limits (`config.tools`, spec §9.3.2).
    subagent: SubagentLimits,
    /// The turn's session budget — the permits of the active engine section's
    /// `sessions` (spec §11.6) over the KV pool the streams share when one is
    /// known ([`super::pool`]), shared with the turn's `ToolContext` so a
    /// tool's own engine request counts too. One permit, and the turn's loops
    /// take turns exactly as they did before the setting existed.
    sessions: Arc<SessionBudget>,
    /// Width of a round's concurrent tool group — the active section's
    /// `concurrent_calls` (spec §6.3). One: the sequential round.
    concurrent_calls: u32,
    /// The chat's rolling summary when one is in force (`Chat::compaction_view`)
    /// — the folded half of the dialogue director's conversation brief
    /// (spec §9.13, fork F6); the unfolded half is the request's own tail.
    compaction_summary: Option<String>,
    /// Effectively allowed tools (protection against calling a disabled one).
    allowed: Vec<ToolId>,
    /// The profile's "self-model" (a snapshot at the start of the turn) + injection
    /// parameters/flags. Injection into the system prompt is done in the task (needs
    /// async embedding for relevance-based selection of observations). See
    /// docs/history/narrative-as-notes.md (Tier 2).
    self_model: Option<crate::entities::self_model::SelfModel>,
    self_model_params: crate::entities::self_model::SelfModelParams,
    inject_enabled: bool,
    maintenance_protocol: bool,
    /// The last user message — the query for relevance-based injection of observations.
    last_user: String,
    /// Engine mode and model name — a snapshot for `Message.metadata` (spec §8.3).
    engine_mode: ServerMode,
    model_name: Option<String>,
    /// Interface language (axis B) — for error messages shown to a human.
    ui_loc: &'static crate::shared::i18n::Locale,
    /// `tools.confirm_dangerous` — when off, nothing is asked and no tool is
    /// gated, so the loop behaves exactly as it did before the feature (spec
    /// §9.8). Snapshotted at the start of the turn, like the other config.
    confirm_dangerous: bool,
    /// Limits for an image a tool returned (`config.images`): the same downscale
    /// ceiling and byte cap a user's `/image attach` gets, so third-party pixels
    /// cannot cost more than the user's own (spec §9.10).
    image_cfg: crate::shared::config::ImageSettings,
    /// The user's answers to [`AppEvent::ToolConfirmRequest`], routed in by the
    /// orchestrator. The **only** channel in the codebase that runs orchestrator
    /// → task; everything else (`title_tx`, `imp_done`, the background-task done
    /// channel) runs the other way. See docs/history/tool-confirmation.md §3, fork F8.
    confirm_rx: UnboundedReceiver<(String, ToolDecision)>,
    /// `compaction.enabled` — read only to pick *which* advice a context-overflow
    /// error gives (spec §6.7): with compression on it names `/compact`, with it
    /// off it names the setting. Pointing at a command that would refuse is the
    /// dead end this project has closed three times.
    compaction_enabled: bool,
    /// The message this turn continues (`/continue`, spec §6.4): its text feeds
    /// the echo filter on the first round, its id rides `GenResult` so the
    /// orchestrator folds the round into it in place.
    continuation: Option<ContinuationSeed>,
    /// The app started this turn itself, on a background run's task
    /// notification (spec §9.3.2): nobody typed anything, and the whole new
    /// input is the note. Measured on the gate model, such a turn can spend
    /// its entire reply cap in `reasoning_content` and land empty
    /// (docs/research/background-dialogues.md §3, fork F11), so it gets the
    /// one-shot muted re-ask a dialogue's line gets.
    woken: bool,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenMessage>,
    /// The background-run slots (see [`TurnShared::background`]).
    background: Arc<BackgroundSlots>,
}

/// What the confirmation round trip owns, behind [`TurnShared::confirm`]'s
/// lock: the reply receiver and the "approved for this turn" set. One lock
/// for both, held for the whole ask-and-wait, is what makes the popup one
/// question at a time when several loops of the turn run at once
/// (docs/research/parallel-subagents.md §4.3).
struct ConfirmState {
    /// The user's answers to [`AppEvent::ToolConfirmRequest`], routed in by
    /// the orchestrator — the one channel that runs orchestrator → task.
    rx: UnboundedReceiver<(String, ToolDecision)>,
    /// Tools the user approved "for the rest of this turn" (fork F4). The turn
    /// is the natural unit — it is the scope of one user request and it ends by
    /// itself, so nothing outlives it and no standing permission accumulates.
    /// Shared by every loop of the turn for the same reason: same turn, same
    /// request.
    allowed_for_turn: HashSet<ToolId>,
}

/// Everything [`confirm_call`] needs that does not change between calls.
struct ConfirmGate<'a> {
    /// `tools.confirm_dangerous`. When `false` the gate is a no-op and never
    /// even asks the registry — the whole feature is switchable off (spec §9.8).
    enabled: bool,
    registry: &'a ToolRegistry,
    evt_tx: &'a UnboundedSender<AppEvent>,
    cancel: &'a CancellationToken,
    id: Uuid,
    /// Agent-scaffold language: the refusal text is read by the **model**
    /// (axis A), unlike the popup, which the user reads.
    loc: &'static crate::shared::i18n::Locale,
}

/// Asks the user before a dangerous tool call, if the feature is on.
///
/// Returns `None` when the call may proceed, or `Some(text)` — the result to
/// hand the model instead of running it. Approving "for the turn" is recorded in
/// `allowed_for_turn`, so the same tool is not asked about again before the turn
/// ends.
///
/// Waiting is bounded only by cancellation (fork F6): `Esc` and `Quit` both fire
/// the turn's token, so a popup left open cannot wedge the task forever. A
/// closed channel — the orchestrator dropped the sender because the turn is over
/// — reads as a refusal rather than as approval.
async fn confirm_call(
    gate: ConfirmGate<'_>,
    call: &ApiToolCall,
    confirm: &tokio::sync::Mutex<ConfirmState>,
) -> Option<String> {
    // Taken before the check and held through the answer: a sibling loop that
    // reaches a dangerous call meanwhile waits here, so at most one popup is
    // ever open and an "allow for this turn" given to the first covers the
    // second before it asks. Cancellation inside the wait releases it.
    let mut state = confirm.lock().await;
    let ConfirmState {
        rx: confirm_rx,
        allowed_for_turn,
    } = &mut *state;
    let needs_ask = gate.enabled
        && !allowed_for_turn.contains(call.name.as_str())
        && gate
            .registry
            .get(&call.name)
            .is_some_and(|tool| tool.danger());
    if !needs_ask {
        return None;
    }
    let _ = gate.evt_tx.send(AppEvent::ToolConfirmRequest {
        generation_id: gate.id,
        call_id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
    });
    let decision = tokio::select! {
        _ = gate.cancel.cancelled() => None,
        reply = wait_for_decision(confirm_rx, &call.id) => reply,
    };
    match decision {
        Some(ToolDecision::AllowForTurn) => {
            allowed_for_turn.insert(call.name.clone());
            None
        }
        Some(ToolDecision::Allow) => None,
        Some(ToolDecision::Deny) => Some(gate.loc.tf("loop.tool_denied", &[("name", &call.name)])),
        // Cancelled, or the channel closed with the question unanswered.
        None => Some(gate.loc.t("loop.tool_cancelled").to_string()),
    }
}

/// The answer to **this** call, skipping any that arrive for another one.
///
/// A mismatch is possible whenever the model made several calls in one round and
/// the user answered them out of order; answering the wrong call would run a tool
/// the user never looked at, so the id is checked rather than assumed.
async fn wait_for_decision(
    confirm_rx: &mut UnboundedReceiver<(String, ToolDecision)>,
    call_id: &str,
) -> Option<ToolDecision> {
    loop {
        let (id, decision) = confirm_rx.recv().await?;
        if id == call_id {
            return Some(decision);
        }
        tracing::debug!(reply_for = %id, waiting_for = %call_id,
            "dropped a tool confirmation meant for another call");
    }
}

/// Accumulator for a single stream round.
struct RoundOutput {
    text: String,
    thoughts: String,
    /// The references to the reasoning (Anthropic's signature / OpenAI's reasoning
    /// items), in order: needed to resend the thinking blocks on an assistant turn
    /// with a tool call in the same turn. Empty for backends with no extended
    /// thinking (llama.cpp) or when there were no "thoughts"; several on Responses
    /// ([`ThinkingAccumulator`]).
    thinking: Vec<ThinkingRef>,
    calls: Vec<ApiToolCall>,
    reason: FinishReason,
    /// Tokens generated in the round: the exact value from the server's `usage`, else
    /// the count of streamed deltas (an approximation — for llama-server one delta ≈
    /// one token).
    tokens: u64,
    /// The prompt's processing as the engine measured it (llama.cpp's `timings`;
    /// `None` elsewhere) — the slow-prefill note's sample
    /// (docs/research/slow-prefill-detection.md §3.1).
    prefill: Option<crate::shared::api::contract::Prefill>,
    /// The **exact** prompt size the server reported for this round, from `usage`.
    /// `None` when the provider reported none — and then it stays `None` rather
    /// than falling back to the byte estimate: auto-compaction reads this, and
    /// the estimate's error changes sign by content type (§9a M9 of the
    /// research), i.e. it is unsafe precisely on the tool-heavy chats that
    /// overflow first. See sub-decision S2.
    prompt_tokens: Option<u32>,
    /// Reasoning tokens ("thoughts") for the round from `usage` (`0` — the provider
    /// doesn't separate them).
    reasoning_tokens: u32,
}

/// Launches the client-side agentic-loop task (spec §6.3): stream → on
/// `finish_reason=ToolCalls` execute tools → a new request, up to
/// `max_rounds`. Effects and new messages are returned to the orchestrator.
fn spawn_generation(spawn: GenSpawn) {
    let GenSpawn {
        backend,
        registry,
        ctx,
        mut request,
        cancel,
        confirm_dangerous,
        image_cfg,
        confirm_rx,
        id,
        chat_id,
        max_rounds,
        workspace_max_rounds,
        subagent,
        sessions,
        concurrent_calls,
        allowed,
        self_model,
        self_model_params,
        inject_enabled,
        maintenance_protocol,
        last_user,
        engine_mode,
        model_name,
        ui_loc,
        compaction_enabled,
        compaction_summary,
        continuation,
        woken,
        evt_tx,
        done_tx,
        background,
    } = spawn;

    tokio::spawn(async move {
        // Injecting the "self-model" into the system prompt (in the task — needs
        // async embedding of the last message for relevance-based selection of
        // observations; Tier 2). With injection disabled, `inject_self_model`
        // returns system as is.
        {
            let recent = injection_recent(
                &ctx.storage,
                ctx.embedder.as_ref(),
                ctx.profile_id,
                inject_enabled,
                &last_user,
                &self_model_params,
            )
            .await;
            request.system = inject_self_model(
                request.system.take(),
                self_model.as_ref(),
                inject_enabled,
                maintenance_protocol,
                &self_model_params,
                chrono::Utc::now(),
                &recent,
                ctx.loc,
            );
        }
        // Prompt-token estimate (after self-model injection) — the exact count will
        // come from the server's `usage` and replace the estimate. See spec §11.1.
        let _ = evt_tx.send(AppEvent::TokenUsage {
            generation_id: id,
            completion: 0,
            context: Some(estimate_prompt_tokens(&request)),
            context_exact: false,
            reasoning: None,
        });

        let shared = TurnShared {
            background,
            backend,
            registry,
            confirm_dangerous,
            image_cfg,
            confirm: tokio::sync::Mutex::new(ConfirmState {
                rx: confirm_rx,
                allowed_for_turn: HashSet::new(),
            }),
            counters: TurnCounters::default(),
            id,
            max_rounds,
            workspace_max_rounds,
            subagent,
            sessions,
            concurrent_calls,
            engine_mode,
            model_name,
            ui_loc,
            evt_tx: evt_tx.clone(),
            done_tx: done_tx.clone(),
            compaction_enabled,
            compaction_summary,
        };
        let mut turn = TurnLoop {
            shared: &shared,
            ctx,
            request,
            cancel,
            allowed,
            messages: Vec::new(),
            effects: Vec::new(),
            deleted: Vec::new(),
            round: 0,
            workspace_rounds: 0,
            total_tokens: 0,
            total_reasoning: 0,
            last_usage: None,
            pending_new_bubble: false,
            woken,
            depth: 0,
            run_id: None,
            ended_by_limit: None,
            persona: None,
            echo_seed: continuation.as_ref().map(|c| c.text.clone()),
        };
        let reason = turn.run().await;

        // Whether `/continue` would resume what this turn leaves behind — the
        // interruption notes name the command only when it will actually work
        // (fork F9; text derived from state, docs/lessons.md §4). A tool-result
        // tail resumes the loop; an assistant tail needs visible text; a turn
        // that filed nothing left nothing new — unless it was itself a
        // continuation, whose seed still stands.
        let tail_continuable = match turn.messages.last() {
            Some(m) if m.role == MessageRole::Tool => true,
            Some(m) if m.role == MessageRole::Assistant => !m.text.is_empty(),
            _ => continuation.is_some(),
        };
        let continuable = engine_mode.supports_continuation(turn.shared.model_name.as_deref())
            && matches!(
                reason,
                FinishReason::Cancelled | FinishReason::Error | FinishReason::Length
            )
            && tail_continuable;
        let _ = evt_tx.send(AppEvent::Finished {
            generation_id: id,
            reason,
            continuable,
        });
        let _ = done_tx.send(GenMessage::Done(GenResult {
            id,
            chat_id,
            messages: turn.messages,
            effects: turn.effects,
            deleted: turn.deleted,
            usage: turn.last_usage,
            continuation: continuation.map(|c| c.message_id),
        }));
    });
}

/// What every loop of one turn shares: the engine, the registry, the UI
/// channel, the progress channel, the confirmation round trip and the limits. Owned by the generation
/// task, one per turn; the turn's own loop borrows it, and a **child** loop — a
/// sub-agent run (docs/research/subagent-chats.md §3.2) — borrows it from its
/// parent for the duration of the call, which is sound because the parent is
/// suspended inside `execute_call` while the child runs. Split out of
/// [`TurnLoop`] so that a nested loop is the same type over the same shared
/// part, not a second loop with its own copy of these behaviours.
struct TurnShared {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    confirm_dangerous: bool,
    image_cfg: crate::shared::config::ImageSettings,
    /// The dangerous-call confirmation round trip (spec §9.8), one question
    /// at a time — see [`ConfirmState`].
    confirm: tokio::sync::Mutex<ConfirmState>,
    /// The turn's running token totals across every loop of it — what the
    /// status bar shows (docs/research/parallel-subagents.md §4.4). Each loop
    /// still keeps its own count for its record; this is the sum, kept where
    /// concurrent loops can all add to it.
    counters: TurnCounters,
    /// The turn's generation id: every streamed event and every confirmation
    /// request carries it, a child's included — the popup and the reply
    /// routing know one turn, not one loop.
    id: Uuid,
    max_rounds: u32,
    workspace_max_rounds: u32,
    /// A sub-agent run's limits (spec §9.3.2).
    subagent: SubagentLimits,
    /// The turn's session budget (docs/research/parallel-subagents.md §4.2):
    /// a permit is held for the duration of one request stream and for nothing
    /// else — a round's tool execution, a popup waiting for the user, a child's
    /// web fetch hold no session. Every loop of the turn, the turn's own
    /// included, streams under it; sized from the active engine section's
    /// `sessions`. With one permit the loops take turns as they always did.
    /// Under a shared KV pool a stream also reserves what it will occupy and
    /// waits for room (docs/research/admission-by-budget.md §4.1). Shared
    /// (`Arc`) with the turn's `ToolContext`: a tool's own engine request —
    /// `fetch_url`'s page summary — takes a permit of the same budget
    /// (docs/research/concurrent-tools.md §4.5).
    sessions: Arc<SessionBudget>,
    /// Width of a round's concurrent tool group (spec §6.3,
    /// docs/research/concurrent-tools.md §4.2–§4.4): how many of a segment's
    /// marked calls are alive at once. One: no segment is formed and every
    /// call takes the sequential path, bit for bit.
    concurrent_calls: u32,
    engine_mode: ServerMode,
    model_name: Option<String>,
    ui_loc: &'static crate::shared::i18n::Locale,
    evt_tx: UnboundedSender<AppEvent>,
    /// The progress channel to the orchestrator — the same one the result
    /// goes on, so progress and result arrive in order ([`GenMessage`]).
    done_tx: UnboundedSender<GenMessage>,
    /// `compaction.enabled` — picks which advice a context-overflow error gives
    /// (see [`GenSpawn::compaction_enabled`]).
    compaction_enabled: bool,
    /// The chat's rolling summary, for the dialogue director's brief
    /// (see [`GenSpawn::compaction_summary`]).
    compaction_summary: Option<String>,
    /// How many background runs are out, against the cap
    /// (`tools.subagent_background_max`) — owned by the orchestrator, taken
    /// by the loop that starts a run, released by the run that ends
    /// (docs/research/background-subagents.md §4.8).
    background: Arc<BackgroundSlots>,
}

/// The turn-wide token totals ([`TurnShared::counters`]): every loop adds its
/// streamed deltas as they arrive and corrects to the server's exact `usage`
/// at the round's end, so the bar's number grows monotonically whichever loop
/// produced the token.
#[derive(Default)]
struct TurnCounters {
    tokens: std::sync::atomic::AtomicU64,
    reasoning: std::sync::atomic::AtomicU32,
}

/// One round's token report from [`stream_round`] to its [`RoundSink`]: the
/// turn's totals for the status bar, the loop's own cumulative count for a
/// transcript's counter, the exact context when the server said.
struct TokenReport {
    turn_completion: u64,
    own_completion: u64,
    turn_reasoning: Option<u32>,
    own_reasoning: Option<u32>,
    context: Option<u64>,
    context_exact: bool,
}

/// One agentic loop's state: the turn's own, or a sub-agent's run inside it.
/// Moved verbatim out of [`spawn_generation`]'s async block (Sonar S3776): the
/// loop itself is [`Self::run`], one tool round is [`Self::tool_round`], one call
/// — [`Self::execute_call`] / [`Self::resolve_call_result`]. The struct follows
/// the module's parameter-struct pattern ([`GenSpawn`], [`ConfirmGate`]); it
/// still never touches `Chat` — results go back through [`GenResult`].
struct TurnLoop<'a> {
    /// Borrowed immutably by every loop of the turn — the parent's and any
    /// number of children running at once (the mutable parts sit behind
    /// their own locks and atomics, see [`TurnShared`]).
    shared: &'a TurnShared,
    ctx: ToolContext,
    request: ChatRequest,
    /// This loop's cancellation: the turn's token for the turn's own loop; a
    /// child token for a sub-agent, so a run timeout ends the child alone while
    /// `Esc` on the turn ends both.
    cancel: CancellationToken,
    allowed: Vec<ToolId>,
    /// New domain messages accumulated across the loop's rounds.
    messages: Vec<Message>,
    /// Tool effects accumulated across the loop's rounds.
    effects: Vec<ChatEffect>,
    /// Discarded by the "rewrite" tool (for the deleted archive).
    deleted: Vec<Message>,
    round: u32,
    /// Rounds spent entirely on the attached project. Exempt from
    /// `max_tool_rounds`, bounded by `workspace.max_rounds`.
    workspace_rounds: u32,
    /// Cumulative reply-token counter across all agentic-loop rounds — the
    /// live indicator keeps growing from round to round.
    total_tokens: u64,
    /// Cumulative reasoning tokens ("thoughts") across rounds.
    total_reasoning: u32,
    /// The exact size of the most recent round, when the server reported one.
    /// Written by [`Self::stream`], so neither call site can forget it.
    last_usage: Option<TurnUsage>,
    /// The next domain assistant message starts a new bubble (after
    /// `send_followup_message`). See spec §9.3.
    pending_new_bubble: bool,
    /// The turn the app started itself on a task notification — the one
    /// turn allowed a muted re-ask when its first round comes back empty
    /// (see [`GenSpawn::woken`]).
    woken: bool,
    /// Nesting level: `0` for the turn's own loop, `1` for a sub-agent's run.
    /// [`Self::run_subagent`] refuses below the top — the belt under the braces
    /// of an allowed set that never offers `call_subagent` there.
    depth: u8,
    /// The run this loop is: `None` for the turn's own loop, whose stream
    /// reaches the feed; the run's id for a sub-agent's, whose stream goes to
    /// the orchestrator as progress under that id — its text would land in
    /// the parent's bubble — while only the turn's token total passes to the
    /// bar (see [`RoundSink`]). What every progress step of the loop is keyed by.
    run_id: Option<Uuid>,
    /// Which budget ended this loop, when one did — the parent reads it to
    /// record a sub-agent's outcome as `RoundLimit` rather than `Completed`.
    ended_by_limit: Option<RoundLimit>,
    /// A sub-agent's display name for the status-bar chip
    /// (`AppEvent::SubagentProgress`); `None` on the turn's own loop, which
    /// reports nothing of the kind.
    persona: Option<String>,
    /// The continuation seed's text, consumed by the **first** round's stream:
    /// llama.cpp echoes the prefill back, and the filter keeps it off the
    /// screen and out of the round (`/continue`, research §4d). `None` on an
    /// ordinary turn, on every later round, and on a sub-agent's loop.
    echo_seed: Option<std::sync::Arc<str>>,
}

/// The limits of one sub-agent run, snapshotted from `config.tools` with the
/// rest of the turn's configuration (spec §9.3.2, docs/research/subagent-chats.md §3.12).
#[derive(Debug, Clone, Copy)]
struct SubagentLimits {
    /// The per-round reply cap, min'ed with the effective `max_tokens` —
    /// a dialogue participant's line rides the same cap (spec §9.13).
    max_tokens: usize,
    /// The whole run — every round and tool call of it.
    run_timeout: std::time::Duration,
    /// The whole dialogue run (`run_dialogue`) — every participant line and
    /// director checkpoint of it. Its own knob: the honest default differs
    /// from the sub-agent's by an order of magnitude (spec §9.13).
    dialogue_run_timeout: std::time::Duration,
    /// How many of one round's sub-agents run at once
    /// (`tools.subagent_parallel`; docs/research/parallel-subagents.md §4.2).
    parallel: u32,
}

impl SubagentLimits {
    fn from_config(tools: &crate::shared::config::ToolSettings) -> Self {
        Self {
            max_tokens: tools.subagent_max_tokens,
            run_timeout: std::time::Duration::from_secs(tools.subagent_run_timeout_secs),
            dialogue_run_timeout: std::time::Duration::from_secs(tools.dialogue_run_timeout_secs),
            parallel: tools.subagent_parallel,
        }
    }
}

/// Where one loop's events go. The turn's own loop sends everything to the
/// screen; a sub-agent's loop sends its stream to the **orchestrator** as
/// progress (docs/history/subagent-live.md §8) — it is the transcript's
/// stream, not the parent's — except the token counter, which also goes on
/// to the status bar re-based on the parent's, with its `context` half
/// dropped: the child's prompt size is not the conversation's, and the bar
/// shows one number (research §3.5).
struct RoundSink<'a> {
    evt_tx: &'a UnboundedSender<AppEvent>,
    done_tx: &'a UnboundedSender<GenMessage>,
    turn: Uuid,
    /// `None` on the turn's own loop, whose stream goes to the screen
    /// directly and to the orchestrator as a mirror; `Some(run id)` on a
    /// sub-agent's, whose stream goes to the orchestrator only, keyed by the
    /// run — several can be in flight at once.
    child: Option<Uuid>,
    /// A dialogue's streams grow the open transcript **per message**, not per
    /// token (research §3.7): the token-level partial has no speaker side yet,
    /// and half the lines land on the `User` side — streaming them into the
    /// assistant-side partial would draw every other line in the wrong bubble.
    /// `true` drops a child's stream steps and keeps only the token counter;
    /// the filed messages (`ChildRoundFiled`) carry the transcript's growth.
    mute_steps: bool,
}

impl RoundSink<'_> {
    fn send(&self, event: AppEvent) {
        let progress = match self.child {
            None => {
                // The turn's own loop: the screen gets every event as it
                // always did; the orchestrator mirrors the round's steps.
                let step = stream_step(&event);
                let _ = self.evt_tx.send(event);
                match step {
                    Some(step) => TurnProgress::OwnStep(step),
                    None => return,
                }
            }
            // A retry or an error inside the run: the parent's result text
            // says how the run ended; nothing to draw meanwhile.
            Some(run) => {
                if self.mute_steps {
                    return;
                }
                match stream_step(&event) {
                    Some(step) => TurnProgress::ChildStep { run, step },
                    None => return,
                }
            }
        };
        let _ = self.done_tx.send(GenMessage::Progress {
            id: self.turn,
            progress,
        });
    }

    /// The token counter: the turn's totals go to the status bar from every
    /// loop (one number, whichever loop produced the token); a child's own
    /// count goes to the orchestrator for its transcript's counter.
    fn tokens(&self, r: TokenReport) {
        match self.child {
            None => {
                let _ = self.evt_tx.send(AppEvent::TokenUsage {
                    generation_id: self.turn,
                    completion: r.turn_completion,
                    context: r.context,
                    context_exact: r.context_exact,
                    reasoning: r.turn_reasoning,
                });
            }
            Some(run) => {
                // The child's prompt size is not the conversation's: the
                // bar shows one number, so the `context` half is dropped.
                let _ = self.evt_tx.send(AppEvent::TokenUsage {
                    generation_id: self.turn,
                    completion: r.turn_completion,
                    context: None,
                    context_exact: false,
                    reasoning: r.turn_reasoning,
                });
                let _ = self.done_tx.send(GenMessage::Progress {
                    id: self.turn,
                    progress: TurnProgress::ChildTokens {
                        run,
                        completion: r.own_completion,
                        reasoning: r.own_reasoning,
                    },
                });
            }
        }
    }
}

/// The mirrored shape of a feed event, when it is one of the round's steps.
fn stream_step(event: &AppEvent) -> Option<StreamStep> {
    Some(match event {
        AppEvent::Chunk { text, .. } => StreamStep::Chunk(text.clone()),
        AppEvent::Thoughts { text, .. } => StreamStep::Thoughts(text.clone()),
        AppEvent::ToolCallStarted {
            call_id,
            name,
            arguments,
            ..
        } => StreamStep::ToolStarted {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        },
        AppEvent::ToolCall {
            call_id,
            name,
            arguments,
            result,
            images,
            ..
        } => StreamStep::ToolCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            result: result.clone(),
            images: *images,
        },
        AppEvent::AssistantContinue { .. } => StreamStep::Continue,
        AppEvent::AssistantRewrite { .. } => StreamStep::Rewrite,
        _ => return None,
    })
}

impl TurnLoop<'_> {
    /// The kind of request this loop's rounds are, for the budget's ratio
    /// (docs/research/title-impersonation-usage.md §3.1): the turn's own,
    /// or a child run's — a persona's prompt and the turn's tools, a
    /// population of its own.
    fn shape(&self) -> crate::shared::session_budget::Shape {
        if self.depth == 0 {
            crate::shared::session_budget::Shape::Turn
        } else {
            crate::shared::session_budget::Shape::Run
        }
    }

    /// Is the tool in the turn's effectively allowed set (profile ∩ global
    /// switches)?
    fn allowed_has(&self, name: &str) -> bool {
        self.allowed.iter().any(|t| t == name)
    }

    /// Tells the status bar where a sub-agent run stands (spec §9.3.2): the
    /// round about to start, or the tool it is entering. Sent **around** the
    /// muted sink — this is the one event of a child's that is meant for the
    /// parent's screen. A no-op on the turn's own loop.
    fn report_progress(&self, tool: Option<&str>) {
        let (Some(name), Some(run)) = (&self.persona, self.run_id) else {
            return;
        };
        // `tool_round` counts the round before it executes the calls, so a
        // tool belongs to the round already counted; a stream opens the next.
        let counted = self.round + self.workspace_rounds;
        let round = if tool.is_some() { counted } else { counted + 1 };
        let progress = crate::app::events::SubagentProgress {
            name: name.clone(),
            round,
            tool: tool.map(str::to_string),
            kind: crate::app::events::RunProgressKind::Subagent,
        };
        // The same position to the orchestrator, for the run's mirror and
        // the tasks screen (spec §11.10) — one value, two readers.
        self.progress(TurnProgress::ChildProgress {
            run,
            progress: progress.clone(),
        });
        let _ = self.shared.evt_tx.send(AppEvent::SubagentProgress {
            generation_id: self.shared.id,
            run,
            progress: Some(progress),
        });
    }

    /// Sends one step of the turn to the orchestrator (see [`TurnProgress`]).
    fn progress(&self, progress: TurnProgress) {
        let _ = self.shared.done_tx.send(GenMessage::Progress {
            id: self.shared.id,
            progress,
        });
    }

    /// A filed round, reported as the parent's or the sub-agent's by depth.
    fn report_progress_filed(&self, messages: Vec<Message>) {
        self.progress(match self.run_id {
            None => TurnProgress::RoundFiled(messages),
            Some(run) => TurnProgress::ChildRoundFiled { run, messages },
        });
    }

    /// This loop's event sink (see [`RoundSink`]).
    fn sink(&self) -> RoundSink<'_> {
        RoundSink {
            evt_tx: &self.shared.evt_tx,
            done_tx: &self.shared.done_tx,
            turn: self.shared.id,
            child: self.run_id,
            mute_steps: false,
        }
    }

    /// The agentic loop itself: stream → on `finish_reason=ToolCalls` execute
    /// tools → a new request, up to `max_rounds`. Returns the turn's finish
    /// reason.
    /// One round through [`stream_round`], recording what it cost.
    ///
    /// Both call sites go through here so the usage cannot be recorded at one of
    /// them and forgotten at the other — the round-limit branch runs its own
    /// final round, and it is the one whose size the next turn actually starts
    /// from.
    async fn stream(&mut self) -> RoundOutput {
        // The continuation seed is the first round's alone: it filters the
        // server's echo of the prefill (research §4d, §7.1).
        let echo = self.echo_seed.take().map(EchoFilter::new);
        // What this stream will occupy of a shared KV pool
        // (docs/research/admission-by-budget.md §4.2): the calibrated estimate
        // of the request, floored by the last round's exact size plus what it
        // generated (the history only grows), plus the reply cap — which a
        // child and a summary always carry, and the turn's own stream may not
        // (then it reserves the pool: it never overlaps another stream anyway).
        let estimate = estimate_prompt_tokens(&self.request);
        let floor = self.last_usage.map_or(0, TurnUsage::next_prompt_estimate);
        let need = self.shared.sessions.price(
            self.shape(),
            estimate,
            floor,
            self.request.sampling.max_tokens.map(|m| m as u64),
        );
        // A session for the stream, and only for the stream: the permit and
        // the reservation are dropped with this block, before the round's
        // tools run.
        let out = {
            let Some(_session) = self.shared.sessions.acquire(need, &self.cancel).await else {
                return cancelled_round();
            };
            stream_round(
                &self.shared.backend,
                self.request.clone(),
                &self.cancel,
                self.shared.id,
                &self.sink(),
                &self.shared.counters,
                self.total_tokens,
                self.total_reasoning,
                self.shared.ui_loc,
                self.shared.compaction_enabled,
                self.shared
                    .engine_mode
                    .supports_continuation(self.shared.model_name.as_deref()),
                echo,
            )
            .await
        };
        // The prefill was consumed by the round that carried it: later rounds
        // end with tool results (nothing to continue), and re-suppressing
        // thinking there would change rounds that continue nothing.
        self.request.continue_final = false;
        if let Some(prompt_tokens) = out.prompt_tokens {
            // The exact size next to the estimate made for the same request:
            // the estimator's correction for every later reservation of the
            // turn (admission-by-budget §4.3).
            self.shared
                .sessions
                .record_usage(self.shape(), estimate, prompt_tokens as u64);
            // The turn keeps its largest prefill sample: a session's first
            // round processes the prompt cold, later rounds ride the cache
            // (docs/research/slow-prefill-detection.md §3.1).
            let mut prefill = self.last_usage.as_ref().and_then(|u| u.prefill);
            crate::shared::api::contract::Prefill::keep_larger(&mut prefill, out.prefill);
            self.last_usage = Some(TurnUsage {
                prompt_tokens,
                completion_tokens: out.tokens,
                prefill,
            });
        }
        out
    }

    async fn run(&mut self) -> FinishReason {
        let mut muted_retry_left = self.woken;
        loop {
            self.report_progress(None);
            let mut out = self.stream().await;
            self.total_tokens += out.tokens;
            self.total_reasoning += out.reasoning_tokens;

            // A turn the app started on a task notification can spend its
            // whole cap thinking and say nothing — measured 2 in 5 on the
            // gate model (docs/research/background-dialogues.md §3). The
            // recovery is the dialogue's (spec §9.13): one re-ask with
            // thinking muted, whose tokens count like any other round's. A
            // second empty reply is reported honestly.
            if muted_retry_left
                && out.calls.is_empty()
                && out.text.trim().is_empty()
                && out.reason != FinishReason::Cancelled
            {
                muted_retry_left = false;
                self.request.sampling.reasoning_budget = Some(0);
                self.report_progress(None);
                out = self.stream().await;
                self.total_tokens += out.tokens;
                self.total_reasoning += out.reasoning_tokens;
            }

            // A round with tool calls — execute and continue the loop.
            if out.reason == FinishReason::ToolCalls && !out.calls.is_empty() {
                if let Some(reason) = self.tool_round(out).await {
                    return reason;
                }
                continue;
            }

            // The final round (Stop/Length/Cancelled/Error, or no calls).
            if let Some(mut m) = finalize_message(
                &out,
                &self.ctx.effective_sampling,
                self.shared.engine_mode,
                &self.shared.model_name,
            ) {
                m.new_bubble = self.pending_new_bubble;
                self.messages.push(m);
            }
            return out.reason;
        }
    }

    /// Which budget, if either, the turn has run out of — the ordinary one, or
    /// the workspace ceiling that keeps an exempt loop from running forever.
    ///
    /// `workspace.max_rounds == 0` means the user switched the second one off.
    /// That is a supported choice rather than an oversight, and what remains
    /// underneath it is `Esc`, the per-command timeout and the one-at-a-time
    /// gate (spec §9.12).
    fn budget_exhausted(&self) -> Option<RoundLimit> {
        if self.round >= self.shared.max_rounds {
            return Some(RoundLimit::Tools);
        }
        if self.shared.workspace_max_rounds > 0
            && self.workspace_rounds >= self.shared.workspace_max_rounds
        {
            return Some(RoundLimit::Workspace);
        }
        None
    }

    /// Whether a call by this name spends a round of the `max_tool_rounds`
    /// budget — the tool's own answer (`Tool::counts_toward_round_limit`).
    ///
    /// An unknown name counts: it is about to become a "no such tool" result,
    /// and a model inventing tool names is exactly the loop the limit is for.
    fn counts_toward_round_limit(&self, name: &str) -> bool {
        self.shared
            .registry
            .get(name)
            .is_none_or(|tool| tool.counts_toward_round_limit())
    }

    /// The turn's round budget is spent: one final round **without tools**.
    ///
    /// DON'T execute new calls — ask the model instead to sum up what's already
    /// been gathered. Otherwise (the previous behavior) the round would only
    /// contain an intent to call more tools with empty text →
    /// `finalize_message` returned `None`, and the user got no reply at all,
    /// even though enough data had accumulated over the previous rounds. Tools
    /// are removed from the request, so the model must answer with text (the
    /// stream goes into the feed).
    ///
    /// Returns the finish reason of that final round (usually `Stop`; on user
    /// cancellation/a stream error — `Cancelled`/`Error`), not an artificial
    /// `Stop`.
    async fn final_round(&mut self, limit: RoundLimit) -> FinishReason {
        // Name the limit that actually fired: quoting `max_tool_rounds` at
        // someone whose turn was ended by the *project* budget points them
        // at the wrong setting (docs/lessons.md §4).
        let (key, n) = match limit {
            RoundLimit::Tools => ("loop.round_limit_reached", self.shared.max_rounds),
            RoundLimit::Workspace => (
                "loop.workspace_round_limit_reached",
                self.shared.workspace_max_rounds,
            ),
        };
        self.sink().send(AppEvent::Error(
            self.ctx.loc.tf(key, &[("max_rounds", &n.to_string())]),
        ));
        self.ended_by_limit = Some(limit);
        self.request.tools.clear();
        // The final round's token counter is emitted by `stream_round` itself
        // (from `base = total_*`); after that the turn ends, no need to accumulate.
        let final_out = self.stream().await;
        if let Some(mut m) = finalize_message(
            &final_out,
            &self.ctx.effective_sampling,
            self.shared.engine_mode,
            &self.shared.model_name,
        ) {
            m.new_bubble = self.pending_new_bubble;
            self.messages.push(m);
        }
        final_out.reason
    }

    /// Files the round's messages: on `rewrite` the round is discarded into the
    /// deleted archive, otherwise it is appended, with `followup` opening the
    /// next assistant message as its own bubble.
    fn file_round(
        &mut self,
        mut am: Message,
        tool_msgs: Vec<Message>,
        rewrite: bool,
        followup: bool,
    ) {
        if rewrite {
            // Discard the round: assistant + tool messages → the deleted archive.
            // The live feed clears the current bubble for the rewritten reply.
            // `pending_new_bubble` is deliberately left alone (the final round absorbs it).
            self.deleted.push(am);
            self.deleted.extend(tool_msgs);
            self.sink().send(AppEvent::AssistantRewrite {
                generation_id: self.shared.id,
            });
            return;
        }
        // assistant BEFORE this round's tool messages.
        am.new_bubble = std::mem::take(&mut self.pending_new_bubble);
        // The orchestrator's in-flight mirror of this round
        // (docs/subagent-live.md §3.1): the parent's rounds rebuild its feed
        // after a switch back; a sub-agent's grow its open transcript.
        let filed: Vec<Message> = std::iter::once(am.clone())
            .chain(tool_msgs.iter().cloned())
            .collect();
        self.report_progress_filed(filed);
        self.messages.push(am);
        self.messages.extend(tool_msgs);
        if followup {
            // The next assistant message — as a separate bubble.
            self.pending_new_bubble = true;
            self.sink().send(AppEvent::AssistantContinue {
                generation_id: self.shared.id,
            });
        }
    }

    /// One round that ended in tool calls: the round-limit final round, the
    /// control-tool recognition, executing every call, and assembling the
    /// round's domain messages. `Some(reason)` ends the turn; `None` — run the
    /// next round.
    async fn tool_round(&mut self, out: RoundOutput) -> Option<FinishReason> {
        if let Some(limit) = self.budget_exhausted() {
            return Some(self.final_round(limit).await);
        }
        // A round spent entirely inside the attached project does not cost the
        // budget (spec §9.12). The limit exists to stop a model looping on
        // *external* work, where every round is a request and possibly money;
        // a code fix is read → change → check, and eight rounds end it halfway.
        // A round is counted when **any** call in it counts, so mixing a
        // `web_search` into a round of reads still spends one — the exemption
        // cannot be used as a way round the limit.
        if out
            .calls
            .iter()
            .any(|c| self.counts_toward_round_limit(&c.name))
        {
            self.round += 1;
        } else {
            // Exempt, but still counted: see `workspace.max_rounds`.
            self.workspace_rounds += 1;
        }

        // Conversation control tools (spec §9.3) are recognized only if
        // they're actually enabled in the profile — otherwise a plain
        // refusal below. `rewrite` discards the current round; `followup`
        // starts a new bubble.
        let rewrite = out
            .calls
            .iter()
            .any(|c| c.name == control::REWRITE_CURRENT_ID && self.allowed_has(&c.name));
        let followup = out
            .calls
            .iter()
            .any(|c| c.name == control::SEND_FOLLOWUP_ID && self.allowed_has(&c.name));

        // The assistant turn with calls — into the request history (also
        // needed for inference in the next continuation/rewrite round). With
        // extended thinking (Anthropic) or reasoning items (OpenAI Responses)
        // we attach the thinking blocks, in order: an assistant turn with
        // tool_use in the same turn is required to carry them, otherwise the
        // next request → 400. They exist only if the model actually returned
        // "thoughts"; other backends ignore the field. The round's thoughts
        // text rides on the first block — Anthropic's one block resends its
        // text with the signature, Responses sends only id + ciphertext.
        let thinking: Vec<ThinkingBlock> = out
            .thinking
            .iter()
            .enumerate()
            .map(|(i, r)| ThinkingBlock {
                text: if i == 0 {
                    out.thoughts.clone()
                } else {
                    String::new()
                },
                signature: r.signature.clone(),
                id: r.id.clone(),
            })
            .collect();
        self.request.messages.push(
            ApiMessage::assistant_tool_calls(out.text.clone(), out.calls.clone())
                .with_thinking_blocks(thinking),
        );
        let (records, tool_msgs) = self.execute_round(&out.calls, rewrite).await;

        // The round's domain assistant message (text + thoughts + tool blocks).
        let mut am = Message::assistant(out.text.clone());
        if !out.thoughts.is_empty() {
            am.thoughts = Some(out.thoughts.clone());
        }
        am.tool_calls = records;

        self.file_round(am, tool_msgs, rewrite, followup);
        // An attachment a tool produced this round (a video transcript,
        // spec §9.9) is mirrored into the turn's snapshot, so
        // `attachment_read`/`attachment_search` find it in the **next
        // round** — which is when the model, having just been told it
        // exists, will ask for it. Without this the tool result would be
        // an instruction the turn cannot carry out: the effect itself is
        // applied to `Chat` by the orchestrator only when the turn ends
        // (docs/history/youtube-transcript.md §3 F1).
        //
        // Once per round, not per call: within a round the model has
        // already issued its calls, so finer granularity would buy
        // nothing. The loop still never touches `Chat` — this is its own
        // snapshot.
        sync_attachments(&mut self.ctx, &self.effects);

        // The turn was cancelled while tools were executing — what's
        // accumulated is already saved above, don't start the next round.
        if self.cancel.is_cancelled() {
            return Some(FinishReason::Cancelled);
        }
        None
    }

    /// Executes a round's calls and returns its records and tool messages, in
    /// the model's order. Three phases (docs/research/parallel-subagents.md
    /// §4.1): the ordinary calls resolve in the model's order, as they always
    /// did, while the round's `call_subagent` calls — its parallel group — are
    /// only announced and prepared ([`Self::resolve_round`]); the group runs,
    /// at most `tools.subagent_parallel` children at once, each card closing
    /// as its run lands ([`Self::run_group`]); the request history and the
    /// round's records are written in the model's order, so what the model
    /// and the chat see is exactly what a sequential round would have left.
    async fn execute_round(
        &mut self,
        calls: &[ApiToolCall],
        rewrite: bool,
    ) -> (Vec<ToolCallRecord>, Vec<Message>) {
        let (mut results, group, mut announced) = self.resolve_round(calls, rewrite).await;
        if !group.is_empty() {
            for (i, done) in self.run_group(group).await {
                self.effects.extend(done.effects);
                self.keep_tool_sample(done.prefill);
                // The card closes as its run lands, whatever the order.
                self.announce_result(&calls[i], &done.result.text, 0);
                announced[i] = true;
                results[i] = Some(done.result);
            }
        }
        let mut records: Vec<ToolCallRecord> = Vec::new();
        let mut tool_msgs: Vec<Message> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            let result = results[i]
                .take()
                .expect("every call of the round resolves in one of the phases");
            self.record_call(
                call,
                result,
                rewrite,
                announced[i],
                &mut records,
                &mut tool_msgs,
            )
            .await;
        }
        (records, tool_msgs)
    }

    /// Phase one: every ordinary call resolved in the model's order — a
    /// **segment** of consecutive concurrent-marked calls as one group at its
    /// position ([`Self::run_segment`], docs/research/concurrent-tools.md
    /// §4.2), everything else one at a time; every sub-agent call announced
    /// (its card opens) and prepared as a [`ChildSpec`], or refused on the
    /// spot when the call is malformed. The third value says which cards a
    /// segment already closed as its results landed.
    ///
    /// The loop itself answers two questions and nothing else: which of the
    /// three kinds the call at `i` is, and where the next one starts. What
    /// each kind *does* is a method of its own — the shape docs/lessons.md
    /// §10 prescribes for a dispatch whose arms carry preconditions.
    async fn resolve_round(
        &mut self,
        calls: &[ApiToolCall],
        rewrite: bool,
    ) -> (Vec<Option<CallResult>>, Vec<(usize, ChildSpec)>, Vec<bool>) {
        let mut results: Vec<Option<CallResult>> = calls.iter().map(|_| None).collect();
        let mut group: Vec<(usize, ChildSpec)> = Vec::new();
        let mut announced = vec![false; calls.len()];
        let mut i = 0;
        while i < calls.len() {
            let call = &calls[i];
            if self.is_group_call(call, rewrite) {
                self.queue_group_call(call, i, &mut group, &mut results);
                i += 1;
            } else if self.is_background_call(call, rewrite) {
                results[i] = Some(self.resolve_background_call(call));
                i += 1;
            } else {
                let end = self.segment_end(calls, i, rewrite);
                self.resolve_ordinary(&calls[i..end], i, rewrite, &mut results, &mut announced)
                    .await;
                i = end;
            }
        }
        (results, group, announced)
    }

    /// A member of the round's parallel group: the card opens and the
    /// [`ChildSpec`] joins the group under the call's own index, or a
    /// malformed call is refused on the spot and never reaches the group
    /// (docs/research/parallel-subagents.md §4.1).
    fn queue_group_call(
        &mut self,
        call: &ApiToolCall,
        at: usize,
        group: &mut Vec<(usize, ChildSpec)>,
        results: &mut [Option<CallResult>],
    ) {
        self.announce_call(call);
        match self.child_spec(&Self::call_args(call)) {
            Ok(spec) => group.push((at, spec)),
            Err(refusal) => results[at] = Some(refusal),
        }
    }

    /// A background run starts now and answers at once — its card opens and
    /// closes within the round, like any call's
    /// (docs/research/background-subagents.md §4.2, and
    /// docs/research/background-dialogues.md §4.2 for the scene's twin).
    fn resolve_background_call(&mut self, call: &ApiToolCall) -> CallResult {
        self.announce_call(call);
        self.report_progress(Some(&call.name));
        let args = Self::call_args(call);
        if call.name == crate::features::tools::dialogue::START_DIALOGUE_ID {
            self.start_background_dialogue(&args)
        } else {
            self.start_background(&args)
        }
    }

    /// Everything that is not a sub-agent call, at index `at` of the round:
    /// `span` is either the one call, resolved where it stands, or a segment
    /// of two or more run at once ([`Self::run_segment`]). The segment's
    /// results arrive in the model's order, so its effects land in that order
    /// too (fork F6): a sequential round and a concurrent one leave the same
    /// `Chat`. A segment closes its cards as the results land, which is what
    /// it marks in `announced`.
    async fn resolve_ordinary(
        &mut self,
        span: &[ApiToolCall],
        at: usize,
        rewrite: bool,
        results: &mut [Option<CallResult>],
        announced: &mut [bool],
    ) {
        if span.len() == 1 {
            results[at] = Some(self.resolve_call(&span[0], rewrite).await);
            return;
        }
        for (j, done) in self.run_segment(span).await {
            self.effects.extend(done.effects);
            self.keep_tool_sample(done.prefill);
            results[at + j] = Some(done.result);
            announced[at + j] = true;
        }
    }

    /// Whether `call` may be a member of a concurrent segment: a round not
    /// being discarded, a name the profile offers, and a tool whose author
    /// marked it (`Tool::concurrent`, docs/research/concurrent-tools.md §4.1).
    /// A control call, a sub-agent, a disabled name or a writer is none of
    /// these and resolves at its own position, as it always did.
    fn is_concurrent_call(&self, call: &ApiToolCall, rewrite: bool) -> bool {
        !rewrite
            && !control::is_control_tool(&call.name)
            && self.allowed_has(&call.name)
            && self.shared.registry.is_concurrent(&call.name)
    }

    /// The end (exclusive) of the segment that starts at `start`: the first
    /// later index whose call is not a concurrent member, or the round's end.
    /// At a width of one no segment is ever formed — `start + 1` — so the
    /// default of a local engine takes the sequential path bit for bit
    /// (docs/research/concurrent-tools.md §4.4).
    fn segment_end(&self, calls: &[ApiToolCall], start: usize, rewrite: bool) -> usize {
        if self.shared.concurrent_calls <= 1 {
            return start + 1;
        }
        let members = calls[start..]
            .iter()
            .take_while(|c| self.is_concurrent_call(c, rewrite))
            .count();
        start + members.max(1)
    }

    /// Runs a segment of concurrent calls (docs/research/concurrent-tools.md
    /// §4.3): every member's card opens first, the invocations run as futures
    /// inside this task — at most `concurrent_calls` of them polled at once —
    /// each card closing as its result lands, and the results come back in
    /// the model's order, with the index each had in the segment. The
    /// confirmation gate is not consulted: a marked tool is never dangerous,
    /// which a registry test pins.
    async fn run_segment(&self, calls: &[ApiToolCall]) -> Vec<(usize, CallDone)> {
        let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
        self.report_progress(Some(&names.join(", ")));
        for call in calls {
            self.announce_call(call);
        }
        let width = self.shared.concurrent_calls.max(1) as usize;
        // The futures are made by calling the `async fn` directly rather than
        // inside an `async move` closure: the closure form captures `&self`
        // under a higher-ranked lifetime the spawned task cannot name
        // ("implementation of `FnOnce` is not general enough").
        let members: Vec<_> = calls
            .iter()
            .enumerate()
            .map(|(j, call)| self.invoke_member(j, call))
            .collect();
        let mut done: Vec<(usize, CallDone)> = futures_util::stream::iter(members)
            .buffer_unordered(width)
            .collect()
            .await;
        done.sort_by_key(|(j, _)| *j);
        done
    }

    /// A tool's own request as part of the turn's largest prefill sample
    /// (docs/research/page-summary-usage.md §3.2): the page summary's stream
    /// is a stream of the turn (spec §9.3.1), so its timing competes with the
    /// rounds' for the one note. Folded into the round that carried it — a
    /// round always precedes its tools — and dropped where no round reported
    /// a usage, since a provider without one sends no timing either.
    fn keep_tool_sample(&mut self, sample: Option<crate::shared::api::contract::Prefill>) {
        if let Some(usage) = &mut self.last_usage {
            crate::shared::api::contract::Prefill::keep_larger(&mut usage.prefill, sample);
        }
    }

    /// One member of a segment: the invocation under the same `select!` with
    /// the turn's cancellation token the sequential path uses, the outcome
    /// mapped the same way, and the card closed as the result lands. The card
    /// carries the tool's own image count — none of the marked tools returns
    /// images, and the record keeps the prepared count as always. Returns the
    /// member's index in the segment with its result.
    async fn invoke_member(&self, j: usize, call: &ApiToolCall) -> (usize, CallDone) {
        let args = Self::call_args(call);
        let invoked = tokio::select! {
            _ = self.cancel.cancelled() => None,
            res = self.shared.registry.invoke(&call.name, &self.ctx, args) => Some(res),
        };
        let done = match invoked {
            None => CallDone {
                result: self.ctx.loc.t("loop.tool_cancelled").to_string().into(),
                effects: Vec::new(),
                prefill: None,
            },
            Some(Ok(outcome)) => CallDone {
                result: CallResult {
                    text: outcome.result,
                    images: outcome.images,
                    subagent: None,
                },
                effects: outcome.effects,
                prefill: outcome.prefill,
            },
            Some(Err(err)) => CallDone {
                result: self
                    .ctx
                    .loc
                    .tf(
                        "loop.tool_error",
                        &[("name", &call.name), ("err", &err.to_string())],
                    )
                    .into(),
                effects: Vec::new(),
                prefill: None,
            },
        };
        self.announce_result(call, &done.result.text, done.result.images.len());
        (j, done)
    }

    /// Phase two: the group's children as futures inside this task, at most
    /// `tools.subagent_parallel` polled at once, yielded in completion order
    /// with the index each had in the round.
    async fn run_group(&self, group: Vec<(usize, ChildSpec)>) -> Vec<(usize, CallDone)> {
        let width = self.shared.subagent.parallel.max(1) as usize;
        let shared = self.shared;
        let loc = self.ctx.loc;
        futures_util::stream::iter(
            group
                .into_iter()
                .map(|(i, spec)| async move { (i, run_child(shared, loc, spec).await) }),
        )
        .buffer_unordered(width)
        .collect()
        .await
    }

    /// One call's arguments as the tools take them. A no-argument call gives an
    /// empty argument string — stored as an empty OBJECT, not `Null`: otherwise
    /// serializing the history entry gives `"null"`, and strict providers
    /// (Anthropic) expect an object in `input` (see shared/api/anthropic/wire.rs).
    /// An object is also safer for invoke (deserializing a struct from `null`
    /// panics).
    fn call_args(call: &ApiToolCall) -> serde_json::Value {
        serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}))
    }

    /// Opens a call's card before it runs (spec §11.3).
    fn announce_call(&self, call: &ApiToolCall) {
        self.sink().send(AppEvent::ToolCallStarted {
            generation_id: self.shared.id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
    }

    /// Closes a call's card with its result.
    fn announce_result(&self, call: &ApiToolCall, result: &str, images: usize) {
        self.sink().send(AppEvent::ToolCall {
            generation_id: self.shared.id,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            result: result.to_string(),
            images,
        });
    }

    /// Resolves one ordinary call of the round: the card opens, the result
    /// comes from [`Self::resolve_call_result`] (gates, confirmation, the
    /// invocation). A control call and a call skipped by a rewrite never get
    /// a card.
    async fn resolve_call(&mut self, call: &ApiToolCall, rewrite: bool) -> CallResult {
        let args = Self::call_args(call);
        let is_control = control::is_control_tool(&call.name);
        self.report_progress(Some(&call.name));
        if !is_control && !rewrite {
            self.announce_call(call);
        }
        self.resolve_call_result(call, &args, is_control, rewrite)
            .await
    }

    /// Records one resolved call: the tool message into the request history,
    /// the record and the domain tool message for the round — and closes the
    /// card, unless the round already did as the result landed (`announced`,
    /// the parallel group's case).
    async fn record_call(
        &mut self,
        call: &ApiToolCall,
        result: CallResult,
        rewrite: bool,
        announced: bool,
        records: &mut Vec<ToolCallRecord>,
        tool_msgs: &mut Vec<Message>,
    ) {
        let CallResult {
            text: result,
            images,
            subagent,
        } = result;
        let is_control = control::is_control_tool(&call.name);
        // Decoded and downscaled here, once, so the same prepared bytes go into the
        // request and into the stored message — the object the model sees and the object
        // the chat keeps must be one (spec §9.10).
        let images = prepare_tool_images(images, self.shared.image_cfg).await;
        // A UI tool block — only for regular executed calls (the internal
        // followup/rewrite ones, and ones skipped during a rewrite, don't get one).
        if !is_control && !rewrite && !announced {
            self.announce_result(call, &result, images.len());
        }
        self.request.messages.push(
            ApiMessage::tool(&call.id, &result).with_images(
                images
                    .iter()
                    .enumerate()
                    .map(|(i, image)| {
                        crate::shared::api::ApiImage::new(
                            image.mime.clone(),
                            &image.data,
                            Some(self.ctx.loc.tf(
                                "prompt.images.label",
                                &[("n", &(i + 1).to_string()), ("name", &image.name)],
                            )),
                        )
                    })
                    .collect(),
            ),
        );
        records.push(ToolCallRecord {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: Self::call_args(call),
            result: Some(result.clone()),
            // The thought signature (Gemini 3) is persisted — needed on history replay.
            thought_signature: call.thought_signature.clone(),
            images: images.len(),
            subagent,
        });
        tool_msgs.push(tool_message(call, result).with_images(images));
    }

    /// One call's result text: the disabled/control/rewrite gates, the
    /// confirmation round-trip (spec §9.8), and the invocation under a
    /// `select!` with the turn's cancellation token — moved verbatim from the
    /// loop body.
    async fn resolve_call_result(
        &mut self,
        call: &ApiToolCall,
        args: &serde_json::Value,
        is_control: bool,
        rewrite: bool,
    ) -> CallResult {
        if !self.allowed_has(&call.name) {
            // Protection: the tool is disabled globally/in the profile.
            self.ctx
                .loc
                .tf("loop.tool_disabled", &[("name", &call.name)])
                .into()
        } else if is_control {
            // A control tool: the result is "permission" (the model will
            // see it in the next round). Executed by the loop, not
            // through the registry.
            control::control_permission_text(&call.name, self.ctx.loc).into()
        } else if rewrite {
            // This round is being discarded — side-effect tools aren't executed.
            self.ctx.loc.t("loop.rewrite_skipped").to_string().into()
        } else if let Some(refusal) = confirm_call(
            ConfirmGate {
                enabled: self.shared.confirm_dangerous,
                registry: &self.shared.registry,
                evt_tx: &self.shared.evt_tx,
                cancel: &self.cancel,
                id: self.shared.id,
                loc: self.ctx.loc,
            },
            call,
            &self.shared.confirm,
        )
        .await
        {
            // Declined, or the turn was cancelled while the popup was
            // open. Either way the loop carries on and the model is
            // told (fork F5) — ending the turn here would throw away
            // the text already streamed.
            refusal.into()
        } else if call.name == CALL_SUBAGENT_ID {
            // A loop-executed tool (spec §9.3.2): the sub-agent is a nested
            // loop over this turn's shared part, not a registry call.
            self.run_subagent(args).await
        } else if call.name == START_SUBAGENT_ID {
            // Reached only below the top of the turn (the round's own calls
            // resolve in `resolve_round`): refused there, no nesting.
            self.start_background(args)
        } else if call.name == crate::features::tools::dialogue::START_DIALOGUE_ID {
            // Reached only below the top of the turn, like its sibling above.
            self.start_background_dialogue(args)
        } else if call.name == crate::features::tools::dialogue::RUN_DIALOGUE_ID {
            // The second loop-executed tool (spec §9.13): a directed dialogue
            // of two personas, driven by this loop over the same shared part.
            self.run_dialogue(args).await
        } else {
            // Execution under a `select!` with the cancellation token: Esc
            // doesn't wait for a long-running tool (MCP/network) to finish.
            // Tools that read `ctx.cancel` terminate themselves (MCP sends
            // the server notifications/cancelled); this is a safety net
            // for the rest.
            let invoked = tokio::select! {
                _ = self.cancel.cancelled() => None,
                res = self.shared.registry.invoke(&call.name, &self.ctx, args.clone()) => Some(res),
            };
            match invoked {
                None => self.ctx.loc.t("loop.tool_cancelled").to_string().into(),
                Some(Ok(outcome)) => {
                    self.effects.extend(outcome.effects);
                    self.keep_tool_sample(outcome.prefill);
                    CallResult {
                        text: outcome.result,
                        images: outcome.images,
                        subagent: None,
                    }
                }
                Some(Err(err)) => self
                    .ctx
                    .loc
                    .tf(
                        "loop.tool_error",
                        &[("name", &call.name), ("err", &err.to_string())],
                    )
                    .into(),
            }
        }
    }
}

impl TurnLoop<'_> {
    /// Runs a sub-agent (spec §9.3.2, docs/research/subagent-chats.md
    /// §3.2–§3.4) — the single-call path, reached only where a
    /// `call_subagent` meets the loop outside a round's parallel group: a loop
    /// below the top, which refuses the name whatever the allowed set says.
    /// A round's own calls go through [`Self::child_spec`] and [`run_child`]
    /// as a group in [`Self::tool_round`].
    async fn run_subagent(&mut self, args: &serde_json::Value) -> CallResult {
        match self.child_spec(args) {
            Err(refusal) => refusal,
            Ok(spec) => {
                let done = run_child(self.shared, self.ctx.loc, spec).await;
                self.effects.extend(done.effects);
                done.result
            }
        }
    }

    /// Whether a call of this round belongs to its parallel group
    /// (docs/research/parallel-subagents.md §4.1): a `call_subagent` the
    /// profile offers, at the top of the turn, in a round that is not being
    /// discarded. Everything else — every other tool, a nested loop's
    /// refusal, a switched-off tool's refusal — takes the ordinary path.
    fn is_group_call(&self, call: &ApiToolCall, rewrite: bool) -> bool {
        call.name == CALL_SUBAGENT_ID && self.depth == 0 && !rewrite && self.allowed_has(&call.name)
    }

    /// Whether a call of this round starts a **background** run —
    /// `start_subagent` (docs/research/background-subagents.md §4.1) or
    /// `start_dialogue` (background-dialogues.md §4.2): the same three
    /// conditions as the group's, for the twins the profile offers only
    /// when `tools.subagent_background` is on.
    fn is_background_call(&self, call: &ApiToolCall, rewrite: bool) -> bool {
        (call.name == START_SUBAGENT_ID
            || call.name == crate::features::tools::dialogue::START_DIALOGUE_ID)
            && self.depth == 0
            && !rewrite
            && self.allowed_has(&call.name)
    }

    /// Starts a background run (docs/research/background-subagents.md
    /// §4.2): the spec a group child would get, over a token of its own —
    /// not the turn's, so `Esc` ends the turn and not the run — handed to
    /// the orchestrator as progress to spawn outside this task. The call's
    /// result is the *started* line with the transcript's address, and the
    /// record lands with the turn carrying a placeholder run the landing
    /// fills in. Refused past the cap (`tools.subagent_background_max`),
    /// with the number, and below the top of the turn.
    fn start_background(&mut self, args: &serde_json::Value) -> CallResult {
        let loc = self.ctx.loc;
        let spec = match self.child_spec_with(args, START_SUBAGENT_ID, CancellationToken::new()) {
            Ok(spec) => spec,
            Err(refusal) => return refusal,
        };
        if let Err(out) = self.shared.background.take() {
            return loc
                .tf(
                    "tool.start_subagent.result.too_many",
                    &[("n", &out.to_string())],
                )
                .into();
        }
        let placeholder = SubagentRun {
            id: spec.run_id,
            kind: RunKind::Subagent,
            title: spec.parsed.initial_title(),
            renamed_manually: false,
            name: spec.parsed.name.clone(),
            created_at: chrono::Utc::now(),
            finished_at: None,
            system_message: spec.parsed.system_message.clone(),
            sampling_override: None,
            messages: vec![spec.user.clone()],
            outcome: None,
            tokens: 0,
            participants: Vec::new(),
            background: true,
        };
        let address = crate::features::chat_links::uri(spec.run_id);
        let text = loc.tf(
            "tool.start_subagent.result.started",
            &[
                ("name", &spec.parsed.initial_title()),
                ("address", &address),
            ],
        );
        self.progress(TurnProgress::BackgroundStart(Box::new(BackgroundStart {
            spec: RunSpec::Subagent(Box::new(spec)),
            parts: SharedParts::of(self.shared),
        })));
        CallResult {
            text,
            images: Vec::new(),
            subagent: Some(Box::new(placeholder)),
        }
    }

    /// Starts a **background dialogue** (spec §9.13,
    /// docs/research/background-dialogues.md §4.2): the scene is parsed here
    /// so the model gets a straight refusal for a malformed call and the
    /// *started* line can name the transcript, the director's inputs are
    /// snapshotted (fork F3 — the persona and the conversation brief as they
    /// are at the call), and the run leaves the turn as progress. The record
    /// lands with the turn as a `kind: Dialogue` placeholder the landing
    /// fills in. Refused past the shared cap (`tools.subagent_background_max`,
    /// fork F7) and below the top of the turn.
    fn start_background_dialogue(&mut self, args: &serde_json::Value) -> CallResult {
        use crate::features::tools::dialogue::{self, START_DIALOGUE_ID};
        let loc = self.ctx.loc;
        let parsed = match dialogue::DialogueArgs::parse(args, loc) {
            Ok(parsed) => parsed,
            Err(err) => {
                return loc
                    .tf(
                        "loop.tool_error",
                        &[("name", START_DIALOGUE_ID), ("err", &err.to_string())],
                    )
                    .into();
            }
        };
        if self.depth > 0 {
            return loc
                .tf("loop.tool_disabled", &[("name", START_DIALOGUE_ID)])
                .into();
        }
        if let Err(out) = self.shared.background.take() {
            return loc
                .tf(
                    "tool.start_subagent.result.too_many",
                    &[("n", &out.to_string())],
                )
                .into();
        }
        // The participants' lines run under the chat's sampling; the scene
        // caps each of them itself, exactly as the foreground driver does.
        let spec = DialogueSpec {
            run_id: Uuid::new_v4(),
            persona: self.ctx.system_message.clone(),
            brief: conversation_brief(
                self.shared.compaction_summary.as_deref(),
                &self.request.messages,
                loc,
            ),
            sampling: self.ctx.effective_sampling.clone(),
            cancel: CancellationToken::new(),
            loc,
            args: parsed,
        };
        let placeholder = spec.placeholder();
        let text = loc.tf(
            "tool.start_dialogue.result.started",
            &[
                ("name", &placeholder.title),
                ("address", &crate::features::chat_links::uri(spec.run_id)),
            ],
        );
        self.progress(TurnProgress::BackgroundStart(Box::new(BackgroundStart {
            spec: RunSpec::Dialogue(Box::new(spec)),
            parts: SharedParts::of(self.shared),
        })));
        CallResult {
            text,
            images: Vec::new(),
            subagent: Some(Box::new(placeholder)),
        }
    }

    /// Everything a child needs, built by the parent before the child starts
    /// (research §3.3): the parsed call; the turn's tools minus the withheld
    /// ones, in the turn's order, with their schemas; the request over the
    /// parent's environment under the child's persona and reply cap; a context
    /// of its own; a cancellation token under the parent's, so `Esc` on the
    /// turn ends the child while a timeout ends the child alone. `Err` is the
    /// result to hand the model instead of a run: a malformed call, or a loop
    /// below the top — no nesting, twice over (research §3.2).
    fn child_spec(&self, args: &serde_json::Value) -> Result<ChildSpec, CallResult> {
        self.child_spec_with(args, CALL_SUBAGENT_ID, self.cancel.child_token())
    }

    /// [`Self::child_spec`] for either delegation tool: `tool` names the
    /// caller in a refusal, `cancel` is the run's token — a child of the
    /// turn's for a group child, a fresh one for a background run.
    fn child_spec_with(
        &self,
        args: &serde_json::Value,
        tool: &str,
        cancel: CancellationToken,
    ) -> Result<ChildSpec, CallResult> {
        let loc = self.ctx.loc;
        let parsed = SubagentArgs::parse(args, loc).map_err(|err| {
            CallResult::from(loc.tf(
                "loop.tool_error",
                &[("name", tool), ("err", &err.to_string())],
            ))
        })?;
        if self.depth > 0 {
            return Err(loc.tf("loop.tool_disabled", &[("name", tool)]).into());
        }
        let limits = self.shared.subagent;
        let allowed: Vec<ToolId> = self
            .allowed
            .iter()
            .filter(|t| !withheld_from_subagent(t))
            .cloned()
            .collect();
        let schemas = self.shared.registry.schemas_for(&allowed, loc);
        // The reply cap on top of the effective sampling, as the tool-less
        // version applied it.
        let mut sampling = self.ctx.effective_sampling.clone();
        sampling.max_tokens = Some(
            sampling
                .max_tokens
                .map_or(limits.max_tokens, |m| m.min(limits.max_tokens)),
        );
        // The child's context is the parent's — environment, scope, journal —
        // under its own persona and knobs, with no folded history to read back
        // and the token the caller chose (see `child_spec_with`).
        let mut ctx = self.ctx.clone();
        ctx.system_message = parsed.system_message.clone();
        ctx.effective_sampling = sampling.clone();
        ctx.last_user_message_at = Some(chrono::Utc::now());
        ctx.history = None;
        ctx.cancel = cancel.clone();
        // Which attached files have an index — the attachment block names
        // `attachment_search` only for those (spec §9.7), same as the parent.
        let indexed: Vec<Uuid> = if ctx.attachments.is_empty() {
            Vec::new()
        } else {
            ctx.storage
                .db()
                .attachment_indexed_ids(ctx.chat_id)
                .unwrap_or_default()
        };
        let user = Message::user(parsed.message.clone());
        let request = build_request_in(
            &parsed.system_message,
            std::slice::from_ref(&user),
            &RequestEnv {
                attachments: &ctx.attachments,
                workspace: ctx.workspace.as_ref(),
                compaction: None,
            },
            sampling,
            schemas,
            &PromptContext {
                attachments: &ctx.attachment_cfg,
                // Only `enabled` is read, and only by the `&Chat` wrapper a
                // sub-agent does not go through: its compaction is `None` above.
                compaction: &crate::shared::config::CompactionSettings::default(),
                indexed: &indexed,
                history_tools: false,
                offered_tools: &allowed,
                loc,
            },
        );
        Ok(ChildSpec {
            parsed,
            // The run's identity, minted before it runs: the list shows the
            // transcript under this id from the first round on, and the
            // landed record keeps it (docs/subagent-live.md §3.1).
            run_id: Uuid::new_v4(),
            allowed,
            request,
            ctx,
            cancel,
            user,
            limits,
            max_rounds: self.shared.max_rounds,
            depth: self.depth + 1,
        })
    }
}

/// What a child needs to run — see [`TurnLoop::child_spec`]. Owns everything
/// of its own, so several can be built by one parent and run at once — or
/// be carried out of the turn altogether (a background run,
/// [`BackgroundStart`]): opaque to the orchestrator, which only hands it
/// to [`spawn_background_run`].
pub(super) struct ChildSpec {
    parsed: SubagentArgs,
    run_id: Uuid,
    allowed: Vec<ToolId>,
    request: ChatRequest,
    ctx: ToolContext,
    cancel: CancellationToken,
    user: Message,
    limits: SubagentLimits,
    max_rounds: u32,
    depth: u8,
}

/// What a child leaves for its parent to record: the call's result (the
/// reply text with its trailer, and the run for the record) and the effects
/// addressed to the parent's chat (research §3.4).
struct CallDone {
    result: CallResult,
    effects: Vec<ChatEffect>,
    /// The engine's timing of a request the call made on its own — a page
    /// summary's — for the turn's largest sample (page-summary-usage §3.2).
    prefill: Option<crate::shared::api::contract::Prefill>,
}

/// A background run about to start (docs/research/background-subagents.md
/// §4.2): the child's spec over a token of its own, and the parts of the
/// turn's shared state a run needs — the engine, the registry, the limits —
/// as `Arc`s and copies, so the run owes the turn nothing once spawned.
pub(super) struct BackgroundStart {
    spec: RunSpec,
    parts: SharedParts,
}

/// What a background run *is* (docs/research/background-dialogues.md F5): a
/// sub-agent over its `ChildSpec`, or a directed scene over its own. The two
/// share every later step — the seat, the landing by id, the notification,
/// the stop — so only the start and the spawn branch.
pub(super) enum RunSpec {
    Subagent(Box<ChildSpec>),
    Dialogue(Box<DialogueSpec>),
}

/// A background dialogue's start (F3): the parsed scene plus the director's
/// inputs **snapshotted at the call** — the parent's persona and the folded
/// conversation brief — because a scene ending twenty minutes later has no
/// turn left to read them from.
pub(super) struct DialogueSpec {
    run_id: Uuid,
    args: crate::features::tools::dialogue::DialogueArgs,
    persona: String,
    brief: String,
    sampling: SamplingConfig,
    cancel: CancellationToken,
    loc: &'static crate::shared::i18n::Locale,
}

impl BackgroundStart {
    /// The run's id — minted with the spec, the placeholder's and the
    /// landed record's.
    pub(super) fn run_id(&self) -> Uuid {
        match &self.spec {
            RunSpec::Subagent(spec) => spec.run_id,
            RunSpec::Dialogue(spec) => spec.run_id,
        }
    }

    /// The run as it will land, before it runs: what the orchestrator's
    /// mirror starts from (the same shape `ChildStarted` carries).
    pub(super) fn placeholder(&self) -> SubagentRun {
        match &self.spec {
            RunSpec::Subagent(spec) => SubagentRun {
                id: spec.run_id,
                kind: RunKind::Subagent,
                title: spec.parsed.initial_title(),
                renamed_manually: false,
                name: spec.parsed.name.clone(),
                created_at: chrono::Utc::now(),
                finished_at: None,
                system_message: spec.parsed.system_message.clone(),
                sampling_override: None,
                messages: vec![spec.user.clone()],
                outcome: None,
                tokens: 0,
                participants: Vec::new(),
                background: true,
            },
            RunSpec::Dialogue(spec) => spec.placeholder(),
        }
    }
}

impl DialogueSpec {
    /// The scene as it will land before its first line: the opening the
    /// caller authored, the two personas, `kind: Dialogue` — the shape
    /// `DialogueCtx::run_parsed` reports through `ChildStarted`.
    fn placeholder(&self) -> SubagentRun {
        SubagentRun {
            id: self.run_id,
            kind: RunKind::Dialogue,
            title: self.args.initial_title(self.loc),
            renamed_manually: false,
            name: None,
            created_at: chrono::Utc::now(),
            finished_at: None,
            system_message: String::new(),
            sampling_override: None,
            messages: vec![if self.args.opening_by_a {
                Message::assistant(self.args.opening.clone())
            } else {
                Message::user(self.args.opening.clone())
            }],
            outcome: None,
            tokens: 0,
            participants: vec![
                crate::entities::subagent::Participant {
                    name: self.args.a.name.clone(),
                    system_message: self.args.a.system_message.clone(),
                },
                crate::entities::subagent::Participant {
                    name: self.args.b.name.clone(),
                    system_message: self.args.b.system_message.clone(),
                },
            ],
            background: true,
        }
    }
}

/// The cloneable half of [`TurnShared`] — what a background run takes with
/// it out of the turn.
struct SharedParts {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    image_cfg: crate::shared::config::ImageSettings,
    max_rounds: u32,
    workspace_max_rounds: u32,
    subagent: SubagentLimits,
    concurrent_calls: u32,
    engine_mode: ServerMode,
    model_name: Option<String>,
    ui_loc: &'static crate::shared::i18n::Locale,
    compaction_enabled: bool,
    compaction_summary: Option<String>,
    background: Arc<BackgroundSlots>,
}

impl SharedParts {
    fn of(shared: &TurnShared) -> Self {
        Self {
            backend: shared.backend.clone(),
            registry: shared.registry.clone(),
            image_cfg: shared.image_cfg,
            max_rounds: shared.max_rounds,
            workspace_max_rounds: shared.workspace_max_rounds,
            subagent: shared.subagent,
            concurrent_calls: shared.concurrent_calls,
            engine_mode: shared.engine_mode,
            model_name: shared.model_name.clone(),
            ui_loc: shared.ui_loc,
            compaction_enabled: shared.compaction_enabled,
            compaction_summary: shared.compaction_summary.clone(),
            background: shared.background.clone(),
        }
    }
}

/// How many background runs are out, against the cap
/// (`tools.subagent_background_max`, docs/research/background-subagents.md
/// §4.8). Owned by the orchestrator, shared with every turn: a loop takes a
/// slot when it starts a run, the run gives it back when it ends — so two
/// siblings of one round cannot both pass a cap of one.
#[derive(Debug)]
pub(super) struct BackgroundSlots {
    out: std::sync::atomic::AtomicU32,
    max: std::sync::atomic::AtomicU32,
}

impl BackgroundSlots {
    pub(super) fn new(max: u32) -> Self {
        Self {
            out: std::sync::atomic::AtomicU32::new(0),
            max: std::sync::atomic::AtomicU32::new(max.max(1)),
        }
    }

    /// The cap, as the settings say now (a settings edit lowers or raises
    /// it for the *next* start; runs already out are not ended).
    pub(super) fn set_max(&self, max: u32) {
        self.max
            .store(max.max(1), std::sync::atomic::Ordering::SeqCst);
    }

    /// Takes a slot, or says how many are out when none is free.
    fn take(&self) -> Result<(), u32> {
        use std::sync::atomic::Ordering::SeqCst;
        let max = self.max.load(SeqCst);
        let mut out = self.out.load(SeqCst);
        loop {
            if out >= max {
                return Err(out);
            }
            match self.out.compare_exchange(out, out + 1, SeqCst, SeqCst) {
                Ok(_) => return Ok(()),
                Err(seen) => out = seen,
            }
        }
    }

    fn release(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        let _ = self
            .out
            .fetch_update(SeqCst, SeqCst, |n| Some(n.saturating_sub(1)));
    }
}

/// What a background run says to the orchestrator: its stream and rounds
/// while it runs (the same steps a turn's child sends, keyed by the run's
/// own generation id), and its end — the landed run, the result text the
/// notification quotes, and the effects for the parent chat.
pub(super) enum BackgroundMessage {
    Progress {
        generation: Uuid,
        progress: TurnProgress,
    },
    Done {
        generation: Uuid,
        run: Box<SubagentRun>,
        result: String,
        effects: Vec<ChatEffect>,
    },
}

/// What the orchestrator adds to a [`BackgroundStart`] to spawn it
/// (docs/research/background-subagents.md §4.2): the run's own generation
/// id, the app-wide session budget, and the channels.
pub(super) struct BackgroundSpawn {
    pub(super) generation: Uuid,
    pub(super) sessions: Arc<SessionBudget>,
    pub(super) evt_tx: UnboundedSender<AppEvent>,
    pub(super) bg_tx: UnboundedSender<BackgroundMessage>,
}

/// Spawns a background run as a task of its own: a [`TurnShared`] built
/// from the parts the turn handed over — no confirmation round trip (its
/// calls run as with `confirm_dangerous` off, research fork F3, the user's
/// decision), fresh counters, the app's budget — and the same [`run_child`]
/// a group child runs under, so everything a sub-agent is, a background
/// one is. Returns the run's cancellation token, which the orchestrator
/// keeps for `/subagents stop`, the deletion of the spawning exchange and
/// `Quit`.
pub(super) fn spawn_background_run(
    start: BackgroundStart,
    spawn: BackgroundSpawn,
) -> CancellationToken {
    let BackgroundStart { mut spec, parts } = start;
    let BackgroundSpawn {
        generation,
        sessions,
        evt_tx,
        bg_tx,
    } = spawn;
    let cancel = match &mut spec {
        RunSpec::Subagent(spec) => {
            // The run streams under the app's budget, like the turn it left.
            spec.ctx.sessions = Some(sessions.clone());
            spec.cancel.clone()
        }
        // A scene's own streams are priced by `DialogueCtx` against the same
        // budget (F4); its participants have no tools, so there is no tool
        // context to hand one to.
        RunSpec::Dialogue(spec) => spec.cancel.clone(),
    };
    // The run's progress goes on a channel of its own and is forwarded under
    // its generation id, so the orchestrator can tell it from the turn's;
    // its end follows on the same path, after the channel has closed, so
    // the landing never overtakes the last filed round.
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<GenMessage>();
    let (end_tx, end_rx) = tokio::sync::oneshot::channel::<BackgroundMessage>();
    let forward = bg_tx;
    tokio::spawn(async move {
        while let Some(message) = done_rx.recv().await {
            if let GenMessage::Progress { progress, .. } = message {
                let _ = forward.send(BackgroundMessage::Progress {
                    generation,
                    progress,
                });
            }
        }
        if let Ok(end) = end_rx.await {
            let _ = forward.send(end);
        }
    });
    // Nothing ever asks: the gate is off, and the receiver is never read.
    let (_confirm_tx, confirm_rx) = tokio::sync::mpsc::unbounded_channel();
    let slots = parts.background.clone();
    let shared = TurnShared {
        backend: parts.backend,
        registry: parts.registry,
        confirm_dangerous: false,
        image_cfg: parts.image_cfg,
        confirm: tokio::sync::Mutex::new(ConfirmState {
            rx: confirm_rx,
            allowed_for_turn: HashSet::new(),
        }),
        counters: TurnCounters::default(),
        id: generation,
        max_rounds: parts.max_rounds,
        workspace_max_rounds: parts.workspace_max_rounds,
        subagent: parts.subagent,
        sessions,
        concurrent_calls: parts.concurrent_calls,
        engine_mode: parts.engine_mode,
        model_name: parts.model_name,
        ui_loc: parts.ui_loc,
        evt_tx,
        done_tx,
        compaction_enabled: parts.compaction_enabled,
        compaction_summary: parts.compaction_summary,
        background: parts.background,
    };
    tokio::spawn(async move {
        let (result, effects) = match spec {
            RunSpec::Subagent(spec) => {
                let loc = spec.ctx.loc;
                // A background run's landing offers no sample: the child's
                // stays with its loop (page-summary-usage §7).
                let CallDone {
                    result, effects, ..
                } = run_child(&shared, loc, *spec).await;
                (result, effects)
            }
            // The scene runs through the very same driver the turn's own
            // `run_dialogue` enters, over a context built from the snapshot
            // (docs/research/background-dialogues.md §4.2) — so a background
            // scene and a foreground one cannot drift apart.
            RunSpec::Dialogue(spec) => {
                let DialogueSpec {
                    run_id,
                    args,
                    persona,
                    brief,
                    sampling,
                    cancel,
                    loc,
                } = *spec;
                let result = DialogueCtx {
                    shared: &shared,
                    loc,
                    sampling,
                    persona,
                    brief,
                    cancel,
                    depth: 0,
                    run_id,
                }
                .run_parsed(args)
                .await;
                (result, Vec::new())
            }
        };
        slots.release();
        let run = result
            .subagent
            .expect("a background run always lands a run on its result");
        let _ = end_tx.send(BackgroundMessage::Done {
            generation,
            run,
            result: result.text,
            effects,
        });
        // Dropping the shared part closes the run's progress channel; the
        // forwarder then delivers the end above, last.
        drop(shared);
    });
    cancel
}

/// Runs one sub-agent over the turn's shared part: a child loop of the same
/// type as the turn's own, borrowing `shared` immutably and owning nothing of
/// its parent — which is what lets a round's group run several at once
/// (docs/research/parallel-subagents.md §4.1, §4.3). Reports the run's start
/// and end to the orchestrator, assembles the run and the result text.
async fn run_child(
    shared: &TurnShared,
    loc: &'static crate::shared::i18n::Locale,
    spec: ChildSpec,
) -> CallDone {
    let ChildSpec {
        parsed,
        run_id,
        allowed,
        request,
        ctx,
        cancel,
        user,
        limits,
        max_rounds,
        depth,
    } = spec;
    let started = chrono::Utc::now();
    let progress = |progress: TurnProgress| {
        let _ = shared.done_tx.send(GenMessage::Progress {
            id: shared.id,
            progress,
        });
    };
    progress(TurnProgress::ChildStarted(Box::new(SubagentRun {
        id: run_id,
        kind: RunKind::Subagent,
        title: parsed.initial_title(),
        renamed_manually: false,
        name: parsed.name.clone(),
        created_at: started,
        finished_at: None,
        system_message: parsed.system_message.clone(),
        sampling_override: None,
        messages: vec![user.clone()],
        outcome: None,
        tokens: 0,
        participants: Vec::new(),
        background: false,
    })));

    let mut child = TurnLoop {
        shared,
        ctx,
        request,
        cancel: cancel.clone(),
        allowed,
        // A child is never a woken turn: the notification reaches the parent.
        woken: false,
        messages: Vec::new(),
        effects: Vec::new(),
        deleted: Vec::new(),
        round: 0,
        workspace_rounds: 0,
        total_tokens: 0,
        total_reasoning: 0,
        last_usage: None,
        pending_new_bubble: false,
        depth,
        run_id: Some(run_id),
        ended_by_limit: None,
        persona: Some(parsed.initial_title()),
        // A sub-agent's run continues nothing — the seed is the turn's.
        echo_seed: None,
    };
    // Boxed: `run` → `tool_round` → here → `run` is a recursive async chain,
    // and the compiler needs one indirection in it.
    let finished = tokio::time::timeout(limits.run_timeout, Box::pin(child.run())).await;
    let outcome = match finished {
        Err(_) => {
            // The run's own token, so the parent's turn goes on.
            cancel.cancel();
            RunOutcome::TimedOut
        }
        Ok(FinishReason::Cancelled) => RunOutcome::Cancelled,
        Ok(FinishReason::Error) => RunOutcome::Failed,
        Ok(_) if child.ended_by_limit.is_some() => RunOutcome::RoundLimit,
        Ok(_) => RunOutcome::Completed,
    };
    // Everything the parent keeps, out of the child. Its own discarded drafts
    // (`rewrite_current_message`) are dropped: the archive's promise is
    // recovering what the *user* lost (research §3.6).
    let child_messages = std::mem::take(&mut child.messages);
    let child_effects = std::mem::take(&mut child.effects);
    let child_tokens = child.total_tokens;
    drop(child);
    // The chip goes with the run; the parent's turn is still generating.
    let _ = shared.evt_tx.send(AppEvent::SubagentProgress {
        generation_id: shared.id,
        run: run_id,
        progress: None,
    });
    let finished_at = chrono::Utc::now();
    progress(TurnProgress::ChildEnded {
        run: run_id,
        outcome,
        finished_at,
        tokens: child_tokens,
    });
    let mut run = SubagentRun {
        id: run_id,
        kind: RunKind::Subagent,
        title: parsed.initial_title(),
        renamed_manually: false,
        name: parsed.name.clone(),
        created_at: started,
        finished_at: Some(finished_at),
        system_message: parsed.system_message.clone(),
        sampling_override: None,
        messages: std::iter::once(user).chain(child_messages).collect(),
        outcome: Some(outcome),
        tokens: child_tokens,
        participants: Vec::new(),
        background: false,
    };
    // Effects go to the chat they describe (research §3.4): identity to the
    // run, environment to the parent — which mirrors an attachment into its
    // own snapshot at the round's end, as for any tool.
    let mut effects = Vec::new();
    for effect in child_effects {
        match effect {
            ChatEffect::SetSystemMessage(s) => run.system_message = s,
            ChatEffect::SetSamplingOverride(s) => run.sampling_override = Some(*s),
            a @ ChatEffect::AddAttachment(_) => effects.push(a),
        }
    }

    // The model's result: the final reply and one line naming the transcript —
    // and, when the run did not complete, why (docs/lessons.md §4: a result
    // that says only "cannot" costs the next three turns).
    let address = crate::features::chat_links::uri(run.id);
    let body = run
        .final_reply()
        .map(str::to_string)
        .unwrap_or_else(|| loc.t("tool.call_subagent.result.empty").to_string());
    let status = match outcome {
        RunOutcome::Completed => loc.tf(
            "tool.call_subagent.result.transcript",
            &[("address", &address)],
        ),
        RunOutcome::Cancelled => loc.tf(
            "tool.call_subagent.result.cancelled",
            &[("address", &address)],
        ),
        RunOutcome::TimedOut => loc.tf(
            "tool.call_subagent.result.timeout",
            &[
                ("address", &address),
                ("secs", &limits.run_timeout.as_secs().to_string()),
            ],
        ),
        RunOutcome::Failed => loc.tf("tool.call_subagent.result.failed", &[("address", &address)]),
        RunOutcome::RoundLimit => loc.tf(
            "tool.call_subagent.result.round_limit",
            &[
                ("address", &address),
                ("max_rounds", &max_rounds.to_string()),
            ],
        ),
    };
    CallDone {
        result: CallResult {
            text: format!("{body}\n\n{status}"),
            images: Vec::new(),
            subagent: Some(Box::new(run)),
        },
        effects,
        // The child's own sample stays with its loop (page-summary-usage §7).
        prefill: None,
    }
}

/// How a dialogue loop ended, before it maps onto [`RunOutcome`]
/// (spec §9.13). `Cancelled` is the parent's `Esc` through the child token;
/// `Failed` names whose generation the engine gave nothing usable for — a
/// participant's line (empty even after the muted re-ask, research §5.1) or
/// a checkpoint the director's engine failed on.
enum DialogueEnd {
    Stopped {
        reason: String,
        summary: Option<String>,
    },
    Cap,
    Cancelled,
    Failed {
        who: String,
    },
}

/// One dialogue run's mutable state, owned **outside** the timed loop so a
/// timeout keeps the partial transcript (the future is dropped, the state
/// survives — the same shape `run_subagent` gets from its child loop).
struct DialogueState {
    /// The run's id — what its progress steps and chip are keyed by.
    run_id: Uuid,
    /// The run's title — the status-bar chip names the scene by it.
    title: String,
    /// The role-encoded transcript (research §3.5): participant `a` is
    /// `Assistant`, `b` is `User`, director interventions are `System`.
    transcript: Vec<Message>,
    /// Standing director notes per participant — each participant's system
    /// appendix from the moment it was issued (identity stays with the run,
    /// research §3.4).
    notes_a: Vec<String>,
    notes_b: Vec<String>,
    /// Generated lines, retried ones included — the `max_messages` meter.
    generated: usize,
    next_checkpoint: usize,
    /// How much of the transcript the director has been shown.
    rendered: usize,
    /// The director's persistent conversation: script increments as `user`
    /// turns, its verdicts as its own tool-call turns (research §3.2) — which
    /// keeps its context append-only, the cache-friendly shape §5.1 measured.
    director_msgs: Vec<ApiMessage>,
    tokens: u64,
    reasoning: u32,
}

/// The director's conversation brief (fork F6): the chat's rolling summary
/// when one is in force, then the tail of the parent request's own
/// conversation — most recent turns within a fixed budget. Empty on a chat
/// with no history yet.
fn conversation_brief(
    summary: Option<&str>,
    messages: &[ApiMessage],
    loc: &'static crate::shared::i18n::Locale,
) -> String {
    const BRIEF_BUDGET: usize = 4000;
    let mut tail: Vec<String> = Vec::new();
    let mut spent = 0usize;
    for m in messages.iter().rev() {
        let label = match m.role {
            crate::shared::api::contract::ApiRole::User => loc.t("prompt.dialogue.role_user"),
            crate::shared::api::contract::ApiRole::Assistant => {
                loc.t("prompt.dialogue.role_assistant")
            }
            _ => continue,
        };
        let text = m.content.trim();
        if text.is_empty() {
            continue;
        }
        let line = format!("{label}: {text}");
        if spent + line.len() > BRIEF_BUDGET && !tail.is_empty() {
            break;
        }
        spent += line.len();
        tail.push(line);
        if spent > BRIEF_BUDGET {
            break;
        }
    }
    tail.reverse();
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(s.to_string());
    }
    if !tail.is_empty() {
        parts.push(tail.join("\n"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n{}", loc.t("prompt.dialogue.brief"), parts.join("\n\n"))
}

/// Everything a directed dialogue needs from the turn that staged it
/// (spec §9.13, docs/research/background-dialogues.md F6 — the user's
/// decision, 2026-09-05). The driver used to be a `TurnLoop` method and
/// reached into the loop for five unrelated things: the locale and sampling
/// of `ToolContext`, the parent chat's persona, the turn's live request tail
/// (from which the director's brief is folded) and the turn's cancellation
/// token. Naming them makes the scene runnable from anywhere that can
/// produce them — the live turn below, and a background task from a
/// snapshot — without the loop's other twenty fields coming along.
struct DialogueCtx<'a> {
    /// The turn's shared parts: the backend, the counters, the limits, the
    /// event and progress senders. Borrowed immutably, like every loop's.
    shared: &'a TurnShared,
    /// The profile's language — every prompt and result text of the scene.
    loc: &'static crate::shared::i18n::Locale,
    /// The sampling the participants' lines run under (the director's is
    /// derived from it: muted, with a small verdict cap).
    sampling: SamplingConfig,
    /// The parent chat's persona — the head of the director's system prompt
    /// (fork F6 of the dialogue track: the main agent directs).
    persona: String,
    /// The director's conversation brief, already folded
    /// ([`conversation_brief`]): the rolling summary plus the tail of the
    /// parent's own conversation.
    brief: String,
    /// The scene's own token — a child of the turn's for a foreground
    /// dialogue, so `Esc` ends both.
    cancel: CancellationToken,
    /// The nesting guard's input: `0` for a scene staged by the turn's own
    /// loop, which is the only depth that may stage one.
    depth: u8,
    /// The run's id, minted by whoever starts the scene: a foreground call
    /// mints it here, a background one mints it at the call so its *started*
    /// line can name the transcript's address before the scene begins
    /// (docs/research/background-dialogues.md §4.2).
    run_id: Uuid,
}

impl TurnLoop<'_> {
    /// Runs a directed dialogue (spec §9.13, docs/research/two-agent-dialogue.md)
    /// on behalf of the turn: names what the scene needs from this loop and
    /// hands it to [`DialogueCtx::run`], which is the driver.
    async fn run_dialogue(&mut self, args: &serde_json::Value) -> CallResult {
        DialogueCtx {
            shared: self.shared,
            loc: self.ctx.loc,
            sampling: self.ctx.effective_sampling.clone(),
            persona: self.ctx.system_message.clone(),
            brief: conversation_brief(
                self.shared.compaction_summary.as_deref(),
                &self.request.messages,
                self.ctx.loc,
            ),
            cancel: self.cancel.child_token(),
            depth: self.depth,
            run_id: Uuid::new_v4(),
        }
        .run(args)
        .await
    }
}

impl DialogueCtx<'_> {
    /// The dialogue's own status-bar chip (spec §9.13): which line is being
    /// written, or that the director is judging the scene — so a parent turn
    /// parked inside a long dialogue never reads as a stuck "generating"
    /// (docs/lessons.md §4). Worded by the screen; cleared with the run.
    fn dialogue_chip(
        &self,
        run: Uuid,
        title: &str,
        round: u32,
        kind: crate::app::events::RunProgressKind,
    ) {
        let progress = crate::app::events::SubagentProgress {
            name: title.to_string(),
            round,
            tool: None,
            kind,
        };
        // The scene's position to the orchestrator too — the tasks screen
        // reads it off the run's mirror (spec §11.10).
        self.progress(TurnProgress::ChildProgress {
            run,
            progress: progress.clone(),
        });
        let _ = self.shared.evt_tx.send(AppEvent::SubagentProgress {
            generation_id: self.shared.id,
            run,
            progress: Some(progress),
        });
    }

    /// Sends one step of the scene to the orchestrator, keyed by the turn
    /// that owns it — [`TurnLoop::progress`] for a scene.
    fn progress(&self, progress: TurnProgress) {
        let _ = self.shared.done_tx.send(GenMessage::Progress {
            id: self.shared.id,
            progress,
        });
    }

    /// The scene itself (spec §9.13, research §3.3): two persona contexts
    /// and a director context taking strictly sequential turns on one
    /// backend — at most one request of the scene in flight, the feature's
    /// VRAM contract (research §3.9). Returns the result text and the run
    /// for the record, exactly as `run_subagent` does.
    async fn run(&self, args: &serde_json::Value) -> CallResult {
        use crate::features::tools::dialogue::{self, RUN_DIALOGUE_ID};
        let loc = self.loc;
        let parsed = match dialogue::DialogueArgs::parse(args, loc) {
            Ok(a) => a,
            Err(err) => {
                return loc
                    .tf(
                        "loop.tool_error",
                        &[("name", RUN_DIALOGUE_ID), ("err", &err.to_string())],
                    )
                    .into();
            }
        };
        // No nesting, whatever the allowed set says — the same second lock
        // `run_subagent` keeps on its own door.
        if self.depth > 0 {
            return loc
                .tf("loop.tool_disabled", &[("name", RUN_DIALOGUE_ID)])
                .into();
        }
        self.run_parsed(parsed).await
    }

    /// The scene over already-parsed arguments — the seam a background start
    /// enters through, having parsed at the call to answer the model at once
    /// (docs/research/background-dialogues.md §4.2).
    async fn run_parsed(
        &self,
        parsed: crate::features::tools::dialogue::DialogueArgs,
    ) -> CallResult {
        use crate::features::tools::dialogue;
        let loc = self.loc;
        let started = chrono::Utc::now();
        let limits = self.shared.subagent;
        let a_label = parsed.label(true, loc);
        let b_label = parsed.label(false, loc);

        // Participants ride the chat's sampling under the shared per-line cap;
        // the director's checkpoints run with thinking muted (the probe's
        // empty-turn rule, research §5.1) and a small verdict cap.
        let mut sampling = self.sampling.clone();
        sampling.max_tokens = Some(
            sampling
                .max_tokens
                .map_or(limits.max_tokens, |m| m.min(limits.max_tokens)),
        );
        let mut muted = sampling.clone();
        muted.reasoning_budget = Some(0);
        let mut director_sampling = muted.clone();
        director_sampling.max_tokens = Some(512.min(limits.max_tokens));

        let direction = parsed
            .direction
            .clone()
            .unwrap_or_else(|| loc.t("prompt.dialogue.direction_default").to_string());
        let appendix = loc.tf(
            "prompt.dialogue.director",
            &[("a", &a_label), ("b", &b_label), ("direction", &direction)],
        );
        // The director is the main agent directing (fork F6): the parent
        // turn's own persona, the conversation brief, then the appendix. The
        // self-model injection stays top-turn-only (ADR 0010 F3).
        let director_system = [self.persona.as_str(), &self.brief, &appendix]
            .iter()
            .filter(|s| !s.trim().is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n\n");
        let verdict_schemas = dialogue::verdict_tools(loc, &a_label, &b_label);

        let participants = vec![
            crate::entities::subagent::Participant {
                name: parsed.a.name.clone(),
                system_message: parsed.a.system_message.clone(),
            },
            crate::entities::subagent::Participant {
                name: parsed.b.name.clone(),
                system_message: parsed.b.system_message.clone(),
            },
        ];
        let opening = if parsed.opening_by_a {
            Message::assistant(parsed.opening.clone())
        } else {
            Message::user(parsed.opening.clone())
        };
        let run_id = self.run_id;
        let cancel = self.cancel.clone();
        self.progress(TurnProgress::ChildStarted(Box::new(SubagentRun {
            id: run_id,
            kind: RunKind::Dialogue,
            title: parsed.initial_title(loc),
            renamed_manually: false,
            name: None,
            created_at: started,
            finished_at: None,
            system_message: appendix.clone(),
            sampling_override: None,
            messages: vec![opening.clone()],
            outcome: None,
            tokens: 0,
            participants: participants.clone(),
            background: false,
        })));

        let run = run_id;
        let mut st = DialogueState {
            run_id,
            title: parsed.initial_title(loc),
            transcript: vec![opening],
            notes_a: Vec::new(),
            notes_b: Vec::new(),
            generated: 0,
            next_checkpoint: parsed.moderate_every,
            rendered: 0,
            director_msgs: Vec::new(),
            tokens: 0,
            reasoning: 0,
        };
        let finished = tokio::time::timeout(
            limits.dialogue_run_timeout,
            Box::pin(self.dialogue_loop(
                &mut st,
                &parsed,
                (&a_label, &b_label),
                &director_system,
                &director_sampling,
                &verdict_schemas,
                &sampling,
                &muted,
                &cancel,
                run,
            )),
        )
        .await;
        let end = match finished {
            Err(_) => {
                // The run's own token, so the parent's turn goes on.
                cancel.cancel();
                None
            }
            Ok(end) => Some(end),
        };
        let outcome = match &end {
            None => RunOutcome::TimedOut,
            Some(DialogueEnd::Cancelled) => RunOutcome::Cancelled,
            Some(DialogueEnd::Failed { .. }) => RunOutcome::Failed,
            Some(DialogueEnd::Cap) => RunOutcome::RoundLimit,
            Some(DialogueEnd::Stopped { .. }) => RunOutcome::Completed,
        };
        // The chip goes with the run; the parent's turn is still generating.
        let _ = self.shared.evt_tx.send(AppEvent::SubagentProgress {
            generation_id: self.shared.id,
            run: run_id,
            progress: None,
        });
        let finished_at = chrono::Utc::now();
        self.progress(TurnProgress::ChildEnded {
            run: run_id,
            outcome,
            finished_at,
            tokens: st.tokens,
        });
        let run = SubagentRun {
            id: run_id,
            kind: RunKind::Dialogue,
            title: parsed.initial_title(loc),
            renamed_manually: false,
            name: None,
            created_at: started,
            finished_at: Some(finished_at),
            system_message: appendix,
            sampling_override: None,
            messages: st.transcript,
            outcome: Some(outcome),
            tokens: st.tokens,
            participants,
            background: false,
        };

        // The result closes the door (docs/lessons.md §4): how it ended, and
        // the one route to the words — the transcript's address.
        let generated = st.generated.to_string();
        let mut status = match &end {
            Some(DialogueEnd::Stopped { reason, summary }) => {
                let mut s = loc.tf(
                    "tool.run_dialogue.result.completed",
                    &[
                        ("a", &a_label),
                        ("b", &b_label),
                        ("messages", &generated),
                        ("reason", reason),
                    ],
                );
                if let Some(summary) = summary {
                    s.push('\n');
                    s.push_str(
                        &loc.tf("tool.run_dialogue.result.summary", &[("summary", summary)]),
                    );
                }
                s
            }
            Some(DialogueEnd::Cap) => loc.tf(
                "tool.run_dialogue.result.cap",
                &[
                    ("a", &a_label),
                    ("b", &b_label),
                    ("max_messages", &parsed.max_messages.to_string()),
                ],
            ),
            Some(DialogueEnd::Cancelled) => loc.t("tool.run_dialogue.result.cancelled").to_string(),
            Some(DialogueEnd::Failed { who }) => {
                loc.tf("tool.run_dialogue.result.failed", &[("who", who)])
            }
            None => loc.tf(
                "tool.run_dialogue.result.timeout",
                &[("secs", &limits.dialogue_run_timeout.as_secs().to_string())],
            ),
        };
        status.push_str("\n\n");
        status.push_str(&loc.tf(
            "tool.run_dialogue.result.transcript",
            &[("address", &crate::features::chat_links::uri(run.id))],
        ));
        CallResult {
            text: status,
            images: Vec::new(),
            subagent: Some(Box::new(run)),
        }
    }

    /// The dialogue's main loop (research §3.3): participant turns in strict
    /// alternation, a director checkpoint every `moderate_every` generated
    /// lines, until the director stops it or the cap fires.
    #[allow(clippy::too_many_arguments)] // one internal seam; a struct would re-group what DialogueState already holds
    async fn dialogue_loop(
        &self,
        st: &mut DialogueState,
        parsed: &crate::features::tools::dialogue::DialogueArgs,
        labels: (&str, &str),
        director_system: &str,
        director_sampling: &SamplingConfig,
        verdict_schemas: &[crate::shared::api::contract::ToolSchema],
        sampling: &SamplingConfig,
        muted: &SamplingConfig,
        cancel: &CancellationToken,
        run: Uuid,
    ) -> DialogueEnd {
        use crate::features::tools::dialogue;
        loop {
            if st.generated >= parsed.max_messages {
                return DialogueEnd::Cap;
            }
            if st.generated >= st.next_checkpoint {
                st.next_checkpoint += parsed.moderate_every;
                match self
                    .dialogue_checkpoint(
                        st,
                        parsed,
                        labels,
                        director_system,
                        director_sampling,
                        verdict_schemas,
                        sampling,
                        muted,
                        cancel,
                        run,
                    )
                    .await
                {
                    Ok(Some((reason, summary))) => {
                        return DialogueEnd::Stopped { reason, summary };
                    }
                    Ok(None) => continue,
                    Err(end) => return end,
                }
            }
            let speaker_a =
                dialogue::next_speaker_a(&st.transcript).unwrap_or(!parsed.opening_by_a);
            let line = match self
                .dialogue_line(
                    st, parsed, labels, speaker_a, None, sampling, muted, cancel, run,
                )
                .await
            {
                Ok(line) => line,
                Err(end) => return end,
            };
            self.progress(TurnProgress::ChildRoundFiled {
                run: st.run_id,
                messages: vec![line.clone()],
            });
            st.transcript.push(line);
            st.generated += 1;
        }
    }

    /// One participant's line: the derived view (research §3.2), one streamed
    /// generation, and the muted re-ask when the reply came back empty — the
    /// all-thinking spiral the probe measured (research §5.1).
    #[allow(clippy::too_many_arguments)]
    async fn dialogue_line(
        &self,
        st: &mut DialogueState,
        parsed: &crate::features::tools::dialogue::DialogueArgs,
        labels: (&str, &str),
        speaker_a: bool,
        one_shot: Option<&str>,
        sampling: &SamplingConfig,
        muted: &SamplingConfig,
        cancel: &CancellationToken,
        run: Uuid,
    ) -> Result<Message, DialogueEnd> {
        use crate::features::tools::dialogue;
        let loc = self.loc;
        let persona = if speaker_a { &parsed.a } else { &parsed.b };
        let notes = if speaker_a { &st.notes_a } else { &st.notes_b };
        let who = if speaker_a { labels.0 } else { labels.1 };
        let (system, messages) = dialogue::participant_view(
            &st.transcript,
            speaker_a,
            &persona.system_message,
            notes,
            one_shot,
            &dialogue::ViewText {
                scene: parsed.scene.as_deref(),
                begins: loc.t("prompt.dialogue.begins"),
                note_prefix: loc.t("prompt.dialogue.note_prefix"),
            },
        );
        let request = |s: SamplingConfig| ChatRequest {
            system: Some(system.clone()),
            messages: messages.clone(),
            sampling: s,
            tools: Vec::new(),
            ..Default::default()
        };
        // The coming stream's side, for the open transcript (stage 2): the
        // line's tokens draw in the speaker's own bubble.
        let role = if speaker_a {
            MessageRole::Assistant
        } else {
            MessageRole::User
        };
        self.progress(TurnProgress::ChildLineStarted {
            run: st.run_id,
            role,
        });
        self.dialogue_chip(
            st.run_id,
            &st.title,
            (st.generated + 1) as u32,
            crate::app::events::RunProgressKind::DialogueLine,
        );
        let mut out = self
            .dialogue_stream(request(sampling.clone()), cancel, st, run, true)
            .await;
        match out.reason {
            FinishReason::Cancelled => return Err(DialogueEnd::Cancelled),
            FinishReason::Error => {
                return Err(DialogueEnd::Failed {
                    who: who.to_string(),
                });
            }
            _ => {}
        }
        if out.text.trim().is_empty() {
            // The whole cap went into reasoning — re-ask once with thinking
            // muted; a second empty reply fails the run honestly. The re-ask
            // is the same line starting over: the open transcript's partial
            // resets with it.
            self.progress(TurnProgress::ChildLineStarted {
                run: st.run_id,
                role,
            });
            out = self
                .dialogue_stream(request(muted.clone()), cancel, st, run, true)
                .await;
            match out.reason {
                FinishReason::Cancelled => return Err(DialogueEnd::Cancelled),
                FinishReason::Error => {
                    return Err(DialogueEnd::Failed {
                        who: who.to_string(),
                    });
                }
                _ => {}
            }
            if out.text.trim().is_empty() {
                return Err(DialogueEnd::Failed {
                    who: who.to_string(),
                });
            }
        }
        let Some(mut m) = finalize_message(
            &out,
            &self.sampling,
            self.shared.engine_mode,
            &self.shared.model_name,
        ) else {
            return Err(DialogueEnd::Failed {
                who: who.to_string(),
            });
        };
        if !speaker_a {
            // The role-encoded transcript (research §3.5): b's side is `User`.
            m.role = MessageRole::User;
        }
        Ok(m)
    }

    /// One director checkpoint: the incremental script, the verdict request
    /// (thinking muted), and the verdicts applied in call order
    /// (research §3.3–§3.4). `Ok(Some(..))` — the director stopped the
    /// dialogue; `Ok(None)` — it goes on. A reply with no tool call counts as
    /// `continue` — the dialogue proceeds toward its cap rather than stalling.
    #[allow(clippy::too_many_arguments)]
    async fn dialogue_checkpoint(
        &self,
        st: &mut DialogueState,
        parsed: &crate::features::tools::dialogue::DialogueArgs,
        labels: (&str, &str),
        director_system: &str,
        director_sampling: &SamplingConfig,
        verdict_schemas: &[crate::shared::api::contract::ToolSchema],
        sampling: &SamplingConfig,
        muted: &SamplingConfig,
        cancel: &CancellationToken,
        run: Uuid,
    ) -> Result<Option<(String, Option<String>)>, DialogueEnd> {
        use crate::features::tools::dialogue::{self, Verdict};
        let loc = self.loc;
        let user = self.dialogue_script(st, labels);
        st.director_msgs.push(ApiMessage::user(user));
        let request = ChatRequest {
            system: Some(director_system.to_string()),
            messages: st.director_msgs.clone(),
            sampling: director_sampling.clone(),
            tools: verdict_schemas.to_vec(),
            ..Default::default()
        };
        self.dialogue_chip(
            st.run_id,
            &st.title,
            st.generated as u32,
            crate::app::events::RunProgressKind::DialogueDirector,
        );
        let out = self.dialogue_stream(request, cancel, st, run, false).await;
        match out.reason {
            FinishReason::Cancelled => return Err(DialogueEnd::Cancelled),
            FinishReason::Error => {
                return Err(DialogueEnd::Failed {
                    who: loc.t("tool.run_dialogue.director_label").to_string(),
                });
            }
            _ => {}
        }
        // The verdicts stay in the director's own conversation, so it
        // remembers what it already directed (research §3.2).
        st.director_msgs.push(
            crate::shared::api::contract::ApiMessage::assistant_tool_calls(
                out.text.clone(),
                out.calls.clone(),
            ),
        );
        for call in &out.calls {
            st.director_msgs.push(ApiMessage::tool(
                call.id.clone(),
                loc.t("prompt.dialogue.noted"),
            ));
        }
        let (verdicts, _unknown) = dialogue::parse_verdicts(&out.calls);
        for verdict in verdicts {
            match verdict {
                Verdict::Continue => {}
                Verdict::Stop { reason, summary } => {
                    self.dialogue_stop(st, &reason, summary.as_deref());
                    return Ok(Some((reason, summary)));
                }
                Verdict::Note { to_a, to_b, text } => {
                    self.dialogue_note(st, labels, (to_a, to_b), text);
                }
                Verdict::Retry { note } => {
                    self.dialogue_retry(
                        st,
                        parsed,
                        labels,
                        note.as_deref(),
                        sampling,
                        muted,
                        cancel,
                        run,
                    )
                    .await?;
                }
                Verdict::Rewrite { text } => self.dialogue_rewrite(st, labels, text),
            }
        }
        Ok(None)
    }

    /// The incremental script one checkpoint shows the director
    /// (research §3.3): the transcript lines it has not seen yet, each
    /// labelled by its speaker, under the opening or the continuation header.
    /// Advances `rendered` — interventions are excluded, since the director's
    /// own tool-call turns already carry them.
    fn dialogue_script(&self, st: &mut DialogueState, labels: (&str, &str)) -> String {
        let loc = self.loc;
        let new_lines: Vec<String> = st.transcript[st.rendered..]
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .map(|m| {
                let who = if m.role == MessageRole::Assistant {
                    labels.0
                } else {
                    labels.1
                };
                format!("{who}: {}", m.text)
            })
            .collect();
        st.rendered = st.transcript.len();
        let header = if st.director_msgs.is_empty() {
            loc.t("prompt.dialogue.script_opening")
        } else {
            loc.t("prompt.dialogue.script_more")
        };
        format!(
            "{header}\n\n{}\n\n{}",
            new_lines.join("\n\n"),
            loc.t("prompt.dialogue.ask")
        )
    }

    /// The `Stop` verdict's intervention row — the reason, and the director's
    /// closing summary when it wrote one.
    fn dialogue_stop(&self, st: &mut DialogueState, reason: &str, summary: Option<&str>) {
        let loc = self.loc;
        let line = match summary {
            Some(s) => loc.tf(
                "tool.run_dialogue.stop_line_summary",
                &[("reason", reason), ("summary", s)],
            ),
            None => loc.tf("tool.run_dialogue.stop_line", &[("reason", reason)]),
        };
        self.dialogue_intervention(st, line);
    }

    /// The `Note` verdict: a standing direction filed as an intervention row
    /// and appended to each addressed participant's notes — identity stays
    /// with the run from the moment it was issued (research §3.4).
    fn dialogue_note(
        &self,
        st: &mut DialogueState,
        labels: (&str, &str),
        to: (bool, bool),
        text: String,
    ) {
        let loc = self.loc;
        let (to_a, to_b) = to;
        let whom = match to {
            (true, false) => labels.0.to_string(),
            (false, true) => labels.1.to_string(),
            _ => format!("{}, {}", labels.0, labels.1),
        };
        self.dialogue_intervention(
            st,
            loc.tf(
                "tool.run_dialogue.note_line",
                &[("to", &whom), ("text", &text)],
            ),
        );
        if to_a {
            st.notes_a.push(text.clone());
        }
        if to_b {
            st.notes_b.push(text);
        }
    }

    /// The `Retry` verdict: discard the last line and generate it again, with
    /// the director's note as a one-shot instruction. A no-op when there is
    /// nothing generated to retry or the `max_messages` cap leaves no slot for
    /// the regeneration.
    #[allow(clippy::too_many_arguments)]
    async fn dialogue_retry(
        &self,
        st: &mut DialogueState,
        parsed: &crate::features::tools::dialogue::DialogueArgs,
        labels: (&str, &str),
        note: Option<&str>,
        sampling: &SamplingConfig,
        muted: &SamplingConfig,
        cancel: &CancellationToken,
        run: Uuid,
    ) -> Result<(), DialogueEnd> {
        let loc = self.loc;
        // Only a generated line can be retried, and the retry's regeneration
        // spends a `max_messages` slot of its own.
        if st.generated == 0 || st.generated >= parsed.max_messages {
            return Ok(());
        }
        let Some(last) = st
            .transcript
            .iter()
            .rposition(|m| m.role != MessageRole::System)
        else {
            return Ok(());
        };
        let speaker_a = st.transcript[last].role == MessageRole::Assistant;
        let who = if speaker_a { labels.0 } else { labels.1 };
        // A discard cannot be expressed by appending: the open transcript
        // gets the full replacement (stage 2), with the intervention row
        // saying what happened.
        st.transcript.remove(last);
        let line = match note {
            Some(n) => loc.tf(
                "tool.run_dialogue.retry_line_note",
                &[("who", who), ("note", n)],
            ),
            None => loc.tf("tool.run_dialogue.retry_line", &[("who", who)]),
        };
        st.transcript.push(Message::new(MessageRole::System, line));
        st.rendered = st.transcript.len();
        self.progress(TurnProgress::ChildTranscript {
            run: st.run_id,
            messages: st.transcript.clone(),
        });
        let line = self
            .dialogue_line(
                st, parsed, labels, speaker_a, note, sampling, muted, cancel, run,
            )
            .await?;
        self.progress(TurnProgress::ChildRoundFiled {
            run: st.run_id,
            messages: vec![line.clone()],
        });
        st.transcript.push(line);
        st.generated += 1;
        Ok(())
    }

    /// The `Rewrite` verdict: the director's final cut replaces the last
    /// line's words in place. A no-op when there is no line to rewrite.
    fn dialogue_rewrite(&self, st: &mut DialogueState, labels: (&str, &str), text: String) {
        let loc = self.loc;
        let Some(last) = st
            .transcript
            .iter()
            .rposition(|m| m.role != MessageRole::System)
        else {
            return;
        };
        let who = if st.transcript[last].role == MessageRole::Assistant {
            labels.0
        } else {
            labels.1
        };
        let line = loc.tf("tool.run_dialogue.rewrite_line", &[("who", who)]);
        // The final cut replaces the words; the original's thoughts described
        // a line that no longer exists. An in-place edit cannot be expressed
        // by appending: the open transcript gets the full replacement
        // (stage 2).
        st.transcript[last].text = text;
        st.transcript[last].thoughts = None;
        st.transcript.push(Message::new(MessageRole::System, line));
        st.rendered = st.transcript.len();
        self.progress(TurnProgress::ChildTranscript {
            run: st.run_id,
            messages: st.transcript.clone(),
        });
    }

    /// Files one director intervention as a `System` entry of the transcript
    /// (rendered as a note row, research §3.4) and mirrors it live.
    fn dialogue_intervention(&self, st: &mut DialogueState, text: String) {
        let m = Message::new(MessageRole::System, text);
        self.progress(TurnProgress::ChildRoundFiled {
            run: st.run_id,
            messages: vec![m.clone()],
        });
        st.transcript.push(m);
        // The director already knows what it did — its own tool-call turn
        // carries it — so interventions are not re-rendered into the script.
        st.rendered = st.transcript.len();
    }

    /// One streamed dialogue generation through the child sink, with the
    /// run's tokens accounted. `steps` — whether the stream's tokens reach
    /// the open transcript (a participant's line does, stage 2 of the track;
    /// a director checkpoint stays muted — its deliberation is not a line).
    ///
    /// The stream takes a session permit and a pool reservation like every
    /// other (`TurnLoop::stream`, spec §6.3): the scene's "one request in
    /// flight" was an invariant of running *inside* a turn, and a background
    /// scene has no turn to be inside — so the admission guard is what keeps
    /// it (docs/research/background-dialogues.md R3, fork F4). Nothing is
    /// priced at one session with no pool, which is the behaviour every
    /// foreground scene had before.
    async fn dialogue_stream(
        &self,
        request: ChatRequest,
        cancel: &CancellationToken,
        st: &mut DialogueState,
        run: Uuid,
        steps: bool,
    ) -> RoundOutput {
        let sink = RoundSink {
            evt_tx: &self.shared.evt_tx,
            done_tx: &self.shared.done_tx,
            turn: self.shared.id,
            child: Some(run),
            mute_steps: !steps,
        };
        // Each context is its own conversation, so there is no last-round
        // floor to raise the estimate with — the request is priced as it is.
        let need = self.shared.sessions.price(
            crate::shared::session_budget::Shape::Run,
            estimate_prompt_tokens(&request),
            0,
            request.sampling.max_tokens.map(|m| m as u64),
        );
        let Some(_session) = self.shared.sessions.acquire(need, cancel).await else {
            return cancelled_round();
        };
        let out = stream_round(
            &self.shared.backend,
            request,
            cancel,
            self.shared.id,
            &sink,
            &self.shared.counters,
            st.tokens,
            st.reasoning,
            self.shared.ui_loc,
            self.shared.compaction_enabled,
            false,
            None,
        )
        .await;
        st.tokens += out.tokens;
        st.reasoning += out.reasoning_tokens;
        out
    }
}

/// What one tool call produced for the model: its result text, any images it
/// returned (spec §9.10), and — for `call_subagent` — the run for the record.
/// Every gate and refusal path yields text alone — only a real invocation can
/// produce pixels or a transcript, which is what `From<String>` keeps cheap to express.
struct CallResult {
    text: String,
    images: Vec<crate::features::tools::ToolImage>,
    subagent: Option<Box<SubagentRun>>,
}

impl From<String> for CallResult {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
            subagent: None,
        }
    }
}

/// Decodes, downscales and re-encodes the images a tool returned, on the blocking pool.
///
/// The same preparation a user's `/image attach` gets, for the same reasons (spec §9.10):
/// third-party pixels must not cost more than the user's own, and a provider that takes
/// only png/jpeg must not be handed a webp. An image that fails to decode is **dropped**
/// rather than reported: the tool's own text already said what it returned, and a
/// half-broken picture is not something the model can act on.
async fn prepare_tool_images(
    images: Vec<crate::features::tools::ToolImage>,
    cfg: crate::shared::config::ImageSettings,
) -> Vec<crate::entities::message_image::MessageImage> {
    if images.is_empty() {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        images
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                let raw = b64.decode(&image.data).ok()?;
                if raw.len() as u64 > cfg.max_bytes {
                    tracing::warn!(bytes = raw.len(), "tool image over the size cap, dropped");
                    return None;
                }
                let prepared =
                    crate::features::image_prepare::prepare(&raw, cfg.downscale_px).ok()?;
                Some(crate::entities::message_image::MessageImage::new(
                    // Numbered from 1, like everything the user sees: the name is what
                    // the model's label cites, and "the second image" has to mean the
                    // same thing on both sides.
                    format!("tool-image-{}.{}", i + 1, ext_of(prepared.mime)),
                    format!("tool:{}", Uuid::new_v4()),
                    prepared.mime,
                    prepared.width,
                    prepared.height,
                    b64.encode(&prepared.bytes),
                ))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// The file extension matching a prepared image's MIME type.
fn ext_of(mime: &str) -> &'static str {
    if mime == "image/jpeg" { "jpg" } else { "png" }
}

/// Rebuilds the turn's attachment snapshot from the `AddAttachment` effects the
/// round produced, applying the same dedupe-by-source rule the orchestrator will
/// apply when it persists them — so what the model can read now and what ends up
/// in the chat file are the same set.
///
/// A no-op in the overwhelming majority of rounds (no such effect), so it checks
/// before rebuilding rather than cloning the list every round.
pub(super) fn sync_attachments(ctx: &mut ToolContext, effects: &[ChatEffect]) {
    let added: Vec<&crate::entities::attachment::Attachment> = effects
        .iter()
        .filter_map(|e| match e {
            ChatEffect::AddAttachment(a) => Some(a.as_ref()),
            _ => None,
        })
        .collect();
    if added.is_empty() {
        return;
    }
    let mut list: Vec<crate::entities::attachment::Attachment> = ctx.attachments.to_vec();
    for a in added {
        if list.iter().any(|x| x.id == a.id) {
            continue; // already mirrored by an earlier round
        }
        list.retain(|x| x.source != a.source);
        list.push(a.clone());
    }
    ctx.attachments = list.into();
}

/// The feed note for a failed turn.
///
/// One helper for **both** failure paths — the pre-stream `Err` and the in-stream
/// [`ChatChunk::Error`] — so the two can never drift into describing the same
/// condition differently.
///
/// - The conversation outgrew the window: say what to do about it rather than
///   handing back raw provider JSON in a generic wrapper. Which advice depends on
///   the switch — naming `/compact` while compression is off would send the user to
///   a command that refuses (spec §6.7, sub-decision S4).
/// - `partial` — text or "thoughts" already reached the screen, so the reply is a
///   fragment rather than a failure to answer. Saying only "generation error"
///   there leaves the user guessing whether what they can see is the whole
///   answer, which is the defect class this project keeps re-learning
///   (docs/lessons.md §4): name what happened *and* the way to a whole reply.
fn engine_error_note(
    err: &str,
    compaction_enabled: bool,
    partial: bool,
    continuable: bool,
    ui_loc: &'static crate::shared::i18n::Locale,
) -> String {
    ui_loc.tf(
        engine_error_key(err, compaction_enabled, partial, continuable),
        &[("err", err)],
    )
}

/// Which of the five things to tell the user — split out from
/// [`engine_error_note`] so the decision can be tested as a decision, without
/// asserting on localized prose. `continuable` — the mode can resume the kept
/// partial (`/continue`), so the note names the route that picks up where the
/// cut happened rather than only the one that starts over (fork F9).
pub(super) fn engine_error_key(
    err: &str,
    compaction_enabled: bool,
    partial: bool,
    continuable: bool,
) -> &'static str {
    match (
        crate::features::compaction::is_context_overflow(err),
        compaction_enabled,
        partial,
        continuable,
    ) {
        (true, true, _, _) => "ui.err.context_overflow",
        (true, false, _, _) => "ui.err.context_overflow_off",
        (false, _, true, true) => "ui.err.generation_interrupted_continuable",
        (false, _, true, false) => "ui.err.generation_interrupted",
        (false, _, false, _) => "ui.err.generation_failed",
    }
}

/// Streams a single request, relaying `Text`/`Thoughts` to the UI, accumulating
/// tool calls and the token counter. `base_tokens`/`base_reasoning` — tokens/
/// reasoning tokens accumulated by previous rounds; the UI counter grows
/// cumulatively. Returns the accumulated round.
#[allow(clippy::too_many_arguments)]
async fn stream_round(
    backend: &Arc<dyn EngineBackend>,
    request: ChatRequest,
    cancel: &CancellationToken,
    id: Uuid,
    sink: &RoundSink<'_>,
    counters: &TurnCounters,
    own_base_tokens: u64,
    own_base_reasoning: u32,
    ui_loc: &'static crate::shared::i18n::Locale,
    compaction_enabled: bool,
    continuation_supported: bool,
    mut echo: Option<EchoFilter>,
) -> RoundOutput {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut thinking = ThinkingAccumulator::default();
    let mut acc = ToolCallAccumulator::default();
    let mut reason = FinishReason::Stop;
    // Live count: the number of reply deltas (≈ tokens). If it arrives, the exact
    // value from the server's `usage` replaces the approximation.
    let mut streamed: u64 = 0;
    let mut usage_tokens: Option<u64> = None;
    // The exact prompt size, when the server reports one. Deliberately without an
    // estimate fallback — see `RoundOutput::prompt_tokens`.
    let mut usage_prompt: Option<u32> = None;
    // The engine's own clock over the prompt (llama.cpp's `timings`), for
    // the slow-prefill note (docs/research/slow-prefill-detection.md §3.1).
    let mut usage_prefill: Option<crate::shared::api::contract::Prefill> = None;
    // The round's reasoning tokens (from `usage`; `0` — the provider doesn't separate them).
    let mut round_reasoning: u32 = 0;

    // The reply counter: `context: None` leaves the prior conversation estimate
    // untouched (emitted by start_generation); the exact `context` only comes from the server's usage.
    // Each delta is counted into the turn's total as it arrives; the exact
    // `usage` corrects the total at the end (see the `Usage` arm).
    let emit_completion = |streamed: u64| {
        use std::sync::atomic::Ordering::Relaxed;
        counters.tokens.fetch_add(1, Relaxed);
        sink.tokens(TokenReport {
            turn_completion: counters.tokens.load(Relaxed),
            own_completion: own_base_tokens + streamed,
            turn_reasoning: None,
            own_reasoning: None,
            context: None,
            context_exact: false,
        });
    };

    match backend.chat_stream(request, cancel.clone()).await {
        Ok(mut stream) => {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        // A continuation round strips the server's echo of the
                        // prefill: the withheld bytes are already on screen and
                        // in the seed message (research §4d).
                        let visible = strip_echo(echo.as_mut(), t);
                        streamed += 1;
                        emit_completion(streamed);
                        relay_text(visible, &mut text, sink, id);
                    }
                    ChatChunk::Thoughts(t) => {
                        thoughts.push_str(&t);
                        streamed += 1;
                        sink.send(AppEvent::Thoughts {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(streamed);
                    }
                    // A reference to the reasoning (Anthropic signature / OpenAI
                    // reasoning item) — not shown in the UI, accumulated for resending
                    // on tool use: one entry per reasoning item on Responses (a reply
                    // may carry several), one block on Anthropic (`ThinkingAccumulator`).
                    ChatChunk::ThoughtsSignature(r) => thinking.push(r),
                    ChatChunk::ToolCall(delta) => acc.push(delta),
                    ChatChunk::Usage(u) => {
                        // The exact count from the server: both the reply and the
                        // conversation (prompt) — replaces the delta-based approximation
                        // and the conversation estimate. Reasoning tokens ("thoughts") —
                        // cumulative across rounds (base + current).
                        use std::sync::atomic::Ordering::Relaxed;
                        let exact = u.completion_tokens as u64;
                        usage_tokens = Some(exact);
                        usage_prompt = Some(u.prompt_tokens);
                        usage_prefill = u.prefill.or(usage_prefill);
                        round_reasoning = u.reasoning_tokens;
                        // Correct the turn's total from the delta count to the
                        // server's figure — the two differ by whatever a delta
                        // carried that was not exactly one token.
                        if exact >= streamed {
                            counters.tokens.fetch_add(exact - streamed, Relaxed);
                        } else {
                            counters.tokens.fetch_sub(streamed - exact, Relaxed);
                        }
                        counters.reasoning.fetch_add(u.reasoning_tokens, Relaxed);
                        sink.tokens(TokenReport {
                            turn_completion: counters.tokens.load(Relaxed),
                            own_completion: own_base_tokens + exact,
                            turn_reasoning: Some(counters.reasoning.load(Relaxed)),
                            own_reasoning: Some(own_base_reasoning + u.reasoning_tokens),
                            context: Some(u.prompt_tokens as u64),
                            context_exact: true,
                        });
                    }
                    // The engine is waiting before another attempt (spec §6.8). Not
                    // a failure yet, so nothing is recorded — only shown, and only
                    // while it lasts.
                    ChatChunk::Retry {
                        attempt,
                        max,
                        delay,
                    } => {
                        sink.send(AppEvent::Retrying {
                            generation_id: id,
                            attempt,
                            max,
                            // Rounded up: a chip reading "in 0 s" while it waits
                            // would be its own small lie.
                            delay_secs: delay.as_secs().max(1),
                        });
                    }
                    // A failure that arrived *after* the stream opened. Before this
                    // arm the reply simply stopped — the partial text was kept and
                    // persisted with nothing on screen saying why, so an overloaded
                    // provider looked like a model that had finished talking
                    // (docs/research/cloud-retry-backoff.md §1.2).
                    ChatChunk::Error { message, .. } => {
                        let partial = !text.is_empty() || !thoughts.is_empty();
                        // `/continue` resumes visible text, not a bare thought
                        // (fork F4) — so the note names it only for one.
                        let continuable = continuation_supported && !text.is_empty();
                        sink.send(AppEvent::Error(engine_error_note(
                            &message,
                            compaction_enabled,
                            partial,
                            continuable,
                            ui_loc,
                        )));
                        // The client always yields `Finished(Error)` next; setting the
                        // reason here keeps this correct even if one ever stops.
                        reason = FinishReason::Error;
                    }
                    ChatChunk::Finished(r) => {
                        reason = r;
                        break;
                    }
                }
            }
        }
        Err(err) => {
            let err = err.to_string();
            sink.send(AppEvent::Error(engine_error_note(
                &err,
                compaction_enabled,
                false,
                false,
                ui_loc,
            )));
            reason = FinishReason::Error;
        }
    }

    RoundOutput {
        text,
        thoughts,
        thinking: thinking.finish(),
        calls: acc.finish(),
        reason,
        tokens: usage_tokens.unwrap_or(streamed),
        prompt_tokens: usage_prompt,
        prefill: usage_prefill,
        reasoning_tokens: round_reasoning,
    }
}

/// The continuation echo filter over one text delta ([`stream_round`]'s `Text`
/// arm): with no filter armed the delta passes through whole.
fn strip_echo(echo: Option<&mut EchoFilter>, t: String) -> String {
    match echo {
        Some(f) => f.push(&t),
        None => t,
    }
}

/// Accumulates a visible text delta and relays it to the feed; an empty one (a
/// chunk the echo filter withheld whole) sends nothing.
fn relay_text(visible: String, text: &mut String, sink: &RoundSink<'_>, id: Uuid) {
    if visible.is_empty() {
        return;
    }
    text.push_str(&visible);
    sink.send(AppEvent::Chunk {
        generation_id: id,
        text: visible,
    });
}

/// Client-side estimate of the prompt's token count (the whole conversation) for
/// the live indicator before the server's exact `usage.prompt_tokens` arrives,
/// and for the session budget's reservations (admission-by-budget §4.2).
/// Accounts for the system message, message texts, tool-call arguments in
/// the history, and the tool schemas as the wire sends them — the largest
/// part of a turn's prompt (measured: 24 schemas, 18 116 bytes, about 4270
/// tokens against 85 for the rest of a fresh chat's request —
/// docs/research/roll-usage-calibration.md §2.1), counted as one part at
/// the text's bytes-per-token; the budget's calibration absorbs the
/// difference.
pub(super) fn estimate_prompt_tokens(req: &ChatRequest) -> u64 {
    let tools = (!req.tools.is_empty()).then(|| crate::shared::api::openai::tools_json(&req.tools));
    let mut parts: Vec<&str> = Vec::with_capacity(req.messages.len() + 1);
    for m in &req.messages {
        parts.push(m.content.as_str());
        for tc in &m.tool_calls {
            parts.push(tc.arguments.as_str());
        }
    }
    if let Some(t) = &tools {
        parts.push(t.as_str());
    }
    estimate_prompt(req.system.as_deref(), parts)
}

/// Blends relevant and recent self-notes for relevance-based injection
/// (Tier 2): first the relevant ones (in decreasing order of closeness, up to
/// `n`), then **the freshest observation is guaranteed** (continuity of "what I
/// just noticed") — bumping the last relevant one out if there's no room.
/// Dedup by id. A pure function — testable.
pub(super) fn blend_self_notes(
    relevant: Vec<crate::entities::note::Note>,
    fresh: &[crate::entities::note::Note],
    n: usize,
) -> Vec<crate::entities::note::Note> {
    let mut out: Vec<crate::entities::note::Note> = Vec::new();
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for r in relevant {
        if out.len() >= n {
            break;
        }
        if seen.insert(r.id) {
            out.push(r);
        }
    }
    if let Some(f) = fresh.first()
        && !seen.contains(&f.id)
    {
        if out.len() >= n && !out.is_empty() {
            out.pop();
        }
        out.push(f.clone());
    }
    out
}

/// Gathers observations (self-notes) for injection into the system prompt (Tier 2):
/// **relevant** to the latest reply + a guaranteed freshest observation, with a fallback to
/// plain recency when the embedder is unavailable/the query is empty. Empty if injection is
/// disabled. Async (embedding the query) — that's why it's factored out of the sync handler into
/// the generation task. Tested over a temp store + `MockEmbedder`.
pub(super) async fn injection_recent(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    inject_enabled: bool,
    last_user: &str,
    params: &crate::entities::self_model::SelfModelParams,
) -> Vec<crate::entities::self_model::NarrativeSegment> {
    if !inject_enabled {
        return Vec::new();
    }
    use crate::features::tools::notes;
    let n = params.narrative_in_prompt;
    let fresh = notes::self_notes_recent(storage, profile_id, params.max_narrative);
    // Observations relevant to the latest reply; empty → fall back to recency.
    let relevant = notes::self_notes_relevant(storage, embedder, profile_id, last_user, n).await;
    let picked = if relevant.is_empty() {
        fresh
    } else {
        blend_self_notes(relevant, &fresh, n)
    };
    picked
        .into_iter()
        .map(|nt| crate::entities::self_model::NarrativeSegment {
            id: nt.id,
            text: nt.content,
            created_at: nt.created_at,
        })
        .collect()
}

/// Mixes the "self-model" into the turn's system prompt (SelfModel MVP, see
/// docs/history/self-model-mvp.md): a compact render of the current model (if non-empty) plus,
/// when `maintenance_protocol` is on, a persona-neutral maintenance protocol. Returns
/// the previous `system` unchanged if injection is disabled (the profile hasn't enabled
/// `get_self_model`) or there's nothing to mix in (an empty model and the protocol is off).
/// The protocol is mixed in even for an empty model — to get the model to start maintaining it.
/// A pure function — testable with no engine.
#[allow(clippy::too_many_arguments)]
pub(super) fn inject_self_model(
    system: Option<String>,
    model: Option<&crate::entities::self_model::SelfModel>,
    enabled: bool,
    maintenance_protocol: bool,
    params: &crate::entities::self_model::SelfModelParams,
    now: chrono::DateTime<chrono::Utc>,
    recent: &[crate::entities::self_model::NarrativeSegment],
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
    if !enabled {
        return system;
    }
    // Observations (self-notes) can exist without the model blob — then we render
    // an empty model with observations. `render_for_prompt` returns None only if it's empty
    // both structurally and in observations.
    let empty;
    let m = match model {
        Some(m) => m,
        None => {
            empty = crate::entities::self_model::SelfModel::new(uuid::Uuid::nil());
            &empty
        }
    };
    let block = m.render_for_prompt(
        params.prompt_cap,
        params.narrative_in_prompt,
        now,
        recent,
        loc,
    );
    // Gather the parts to mix in: the model render (if any) + the protocol (if enabled).
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = block {
        parts.push(b);
    }
    if maintenance_protocol {
        // The maintenance protocol is assembled from the shared POLICY_CORE (stage 6) — the same
        // rules as the background auto-reflection.
        parts.push(crate::features::tools::self_model::maintenance_protocol(
            loc,
        ));
        // A data-aware note: if the self-description has grown past its target —
        // a concrete hint to shorten it (the static protocol becomes specific once
        // the summary is actually bloated). See docs/summary-as-snapshot.md (stage 2).
        if let Some(hint) = m.summary_fill_hint(params.summary_target_chars, loc) {
            parts.push(format!("({hint})"));
        }
    }
    if parts.is_empty() {
        return system; // nothing to mix in
    }
    let inject = parts.join("\n\n");
    Some(match system {
        Some(s) => format!("{s}\n\n{inject}"),
        None => inject,
    })
}

/// A domain tool message (role `Tool`) tied to the call.
fn tool_message(call: &ApiToolCall, result: String) -> Message {
    let mut m = Message::new(MessageRole::Tool, result);
    m.tool_call_id = Some(call.id.clone());
    m.tool_name = Some(call.name.clone());
    m
}

/// The turn's final assistant message (if there's text/thoughts) with a metadata
/// snapshot: the engine mode, model name, and sampling, **pared down to the fields
/// available in that mode** (the engine wouldn't accept an unavailable field — spec §8.3).
/// The round's finish reason is recorded too (`MessageFinish`, spec §6.4) — it is
/// what lets `/continue` tell an interrupted reply from a completed one.
fn finalize_message(
    out: &RoundOutput,
    sampling: &SamplingConfig,
    mode: ServerMode,
    model: &Option<String>,
) -> Option<Message> {
    if out.text.is_empty() && out.thoughts.is_empty() {
        return None;
    }
    let mut m = Message::assistant(out.text.clone());
    if !out.thoughts.is_empty() {
        m.thoughts = Some(out.thoughts.clone());
    }
    m.metadata = Some(MessageMetadata {
        sampling: sampling.retain_supported(mode.cloud_provider()),
        mode,
        model: model.clone(),
        finish: Some(match out.reason {
            FinishReason::Length => MessageFinish::Length,
            FinishReason::Cancelled => MessageFinish::Cancelled,
            FinishReason::Error => MessageFinish::Error,
            // `ToolCalls` reaches here only with an empty call list (a claim
            // with nothing behind it) — the round ended like a plain stop.
            FinishReason::Stop | FinishReason::ToolCalls => MessageFinish::Stop,
        }),
    });
    Some(m)
}

/// Folds the continuation round's first assistant message into the seed
/// message it continues, in place (`/continue`, fork F8): the text is appended
/// byte-exactly — the seam is the model's own, already echo-stripped — the
/// thoughts are joined, tool calls extend, and the metadata becomes the
/// round's, except the model name, which stays the seed's unless the
/// continuation outgrew what it continued (fork F7). The message keeps its id
/// and timestamp: it is the same reply, finished later.
pub(super) fn merge_continuation(seed: &mut Message, round: Message) {
    let seed_len = seed.text.len();
    seed.text.push_str(&round.text);
    if let Some(t) = round.thoughts {
        match &mut seed.thoughts {
            Some(existing) => {
                existing.push_str("\n\n");
                existing.push_str(&t);
            }
            None => seed.thoughts = Some(t),
        }
    }
    seed.tool_calls.extend(round.tool_calls);
    let kept_model = seed.metadata.as_ref().and_then(|md| md.model.clone());
    let outgrown = round.text.len() > seed_len;
    if let Some(mut md) = round.metadata {
        if !outgrown && kept_model.is_some() {
            md.model = kept_model;
        }
        seed.metadata = Some(md);
    }
}

/// A round that never streamed: cancelled while waiting for a session, or for
/// room in the pool (`SessionBudget::acquire`). Nothing was produced, nothing
/// was counted, and the loop lands what it already has.
fn cancelled_round() -> RoundOutput {
    RoundOutput {
        text: String::new(),
        thoughts: String::new(),
        thinking: Vec::new(),
        calls: Vec::new(),
        reason: FinishReason::Cancelled,
        tokens: 0,
        prompt_tokens: None,
        prefill: None,
        reasoning_tokens: 0,
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    /// The round a loop lands when cancelled before it could stream: nothing
    /// produced, nothing counted (the budget's own waits are tested with it,
    /// `shared::session_budget`).
    #[test]
    fn a_round_cancelled_while_waiting_is_empty() {
        let round = cancelled_round();
        assert_eq!(round.reason, FinishReason::Cancelled);
        assert_eq!((round.tokens, round.prompt_tokens), (0, None));
        assert!(round.text.is_empty() && round.calls.is_empty());
    }
}
