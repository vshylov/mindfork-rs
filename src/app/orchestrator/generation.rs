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
use crate::entities::message::{Message, MessageMetadata, MessageRole, ToolCallRecord};
use crate::entities::profile::ToolId;
use crate::entities::subagent::{RunKind, RunOutcome, SubagentRun};
use crate::features::tools::subagent::{CALL_SUBAGENT_ID, SubagentArgs, withheld_from_subagent};
use crate::features::tools::{
    ChatEffect, ToolContext, ToolParams, ToolRegistry, TurnInfo, control, effective_tool_ids,
};
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, FinishReason, ThinkingBlock,
    ThinkingRef, ToolCallAccumulator,
};
use crate::shared::config::ServerMode;
use crate::shared::tokens::estimate_prompt;

use super::Orchestrator;
use super::request::{
    PromptContext, RequestEnv, build_request, build_request_in, last_user_message_at,
};

/// Result of a completed generation task (internal channel).
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
}

impl TurnUsage {
    /// A lower bound on the next turn's prompt: this turn's prompt plus what was
    /// generated on top of it. The user's next message and any injection deltas
    /// come on top — which the threshold's headroom is there to absorb.
    pub(super) fn next_prompt_estimate(self) -> u64 {
        self.prompt_tokens as u64 + self.completion_tokens
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

        self.start_generation(active_id, backend);
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
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User)
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
        self.start_generation(active_id, backend);
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
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User)
            else {
                return; // no user message — nothing to delete
            };
            user_text = chat.messages[idx].text.clone();
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
    /// truncated). The shared part for sending a new message and regenerating.
    fn start_generation(&mut self, active_id: Uuid, backend: Arc<dyn EngineBackend>) {
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

        // Build the request/context + take the last user message (for relevance-
        // based injection of observations in the task).
        let request;
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
            };
            ctx = ToolContext::new(
                self.tool_deps(backend.clone()),
                ToolParams::from_config(&self.config),
                turn,
            );
        }

        let id = Uuid::new_v4();
        // Resolved once and used twice: the event below (the live bubble's
        // header) and `GenSpawn.model_name` (the finished message's metadata).
        // One read, so the header cannot name a different model than the one
        // the stored message will claim.
        let model_name = self.config.engine.active_model_name();
        let _ = self.evt_tx.send(AppEvent::GenerationStarted {
            generation_id: id,
            model: model_name.clone(),
        });
        self.gen_state.begin(id, cancel.clone());
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
            allowed,
            self_model,
            self_model_params,
            inject_enabled,
            maintenance_protocol: self.config.self_model.maintenance_protocol,
            last_user,
            engine_mode: self.config.engine.mode,
            model_name,
            ui_loc: self.ui_locale(),
            compaction_enabled: self.config.compaction.enabled,
            evt_tx: self.evt_tx.clone(),
            done_tx: self.done_tx.clone(),
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

        if res.messages.is_empty() && res.effects.is_empty() && res.deleted.is_empty() {
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
        // Attachments a tool produced this turn (spec §9.9) — applied below,
        // outside the `chat` borrow.
        let mut attached: Vec<crate::entities::attachment::Attachment> = Vec::new();
        if let Some(chat) = self.chat_mut(res.chat_id) {
            // Discarded by the "rewrite" tool — into the deleted archive (manual
            // recovery by editing JSON), like Ctrl+E/Ctrl+R. See spec §9.3, §11.7.
            if !res.deleted.is_empty() {
                chat.record_deleted(res.deleted, String::new(), DeletedCause::Rewrite);
            }
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
        // After a successful reply — maybe it's time for background auto-reflection
        // (Tier 3), notes auto-consolidation ("sleep", Tier 3), and/or self-model
        // auto-consolidation ("sleep" for the self-model, stage A1 —
        // docs/history/self-model-consolidation.md).
        self.maybe_auto_reflect(res.chat_id);
        self.maybe_auto_consolidate(res.chat_id);
        self.maybe_auto_self_consolidate(res.chat_id);
        // …and maybe the conversation is approaching the model's context window
        // (spec §6.7). Last of the four deliberately: it reads what this turn
        // actually cost, which is the freshest measurement available.
        self.maybe_auto_compact(res.chat_id, res.usage);
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
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenResult>,
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
    allowed_for_turn: &mut HashSet<ToolId>,
    confirm_rx: &mut UnboundedReceiver<(String, ToolDecision)>,
) -> Option<String> {
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
    /// A reference to the reasoning (Anthropic signature / OpenAI reasoning item):
    /// needed to resend the thinking block on an assistant turn with a tool call in
    /// the same turn. `None` for backends with no extended thinking (llama.cpp) or
    /// when there were no "thoughts".
    thinking_ref: Option<ThinkingRef>,
    calls: Vec<ApiToolCall>,
    reason: FinishReason,
    /// Tokens generated in the round: the exact value from the server's `usage`, else
    /// the count of streamed deltas (an approximation — for llama-server one delta ≈
    /// one token).
    tokens: u64,
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
        evt_tx,
        done_tx,
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

        let mut shared = TurnShared {
            backend,
            registry,
            confirm_dangerous,
            image_cfg,
            confirm_rx,
            id,
            max_rounds,
            workspace_max_rounds,
            subagent,
            engine_mode,
            model_name,
            ui_loc,
            evt_tx: evt_tx.clone(),
            allowed_for_turn: HashSet::new(),
            compaction_enabled,
        };
        let mut turn = TurnLoop {
            shared: &mut shared,
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
            depth: 0,
            live: true,
            token_base: 0,
            ended_by_limit: None,
        };
        let reason = turn.run().await;

        let _ = evt_tx.send(AppEvent::Finished {
            generation_id: id,
            reason,
        });
        let _ = done_tx.send(GenResult {
            id,
            chat_id,
            messages: turn.messages,
            effects: turn.effects,
            deleted: turn.deleted,
            usage: turn.last_usage,
        });
    });
}

/// What every loop of one turn shares: the engine, the registry, the UI
/// channel, the confirmation round trip and the limits. Owned by the generation
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
    confirm_rx: UnboundedReceiver<(String, ToolDecision)>,
    /// The turn's generation id: every streamed event and every confirmation
    /// request carries it, a child's included — the popup and the reply
    /// routing know one turn, not one loop.
    id: Uuid,
    max_rounds: u32,
    workspace_max_rounds: u32,
    /// A sub-agent run's limits (spec §9.3.2).
    subagent: SubagentLimits,
    engine_mode: ServerMode,
    model_name: Option<String>,
    ui_loc: &'static crate::shared::i18n::Locale,
    evt_tx: UnboundedSender<AppEvent>,
    /// Tools the user approved "for the rest of this turn" (fork F4). The turn
    /// is the natural unit — it is the scope of one user request and it ends by
    /// itself, so nothing outlives it and no standing permission accumulates.
    /// Shared by a child loop for the same reason: same turn, same request.
    allowed_for_turn: HashSet<ToolId>,
    /// `compaction.enabled` — picks which advice a context-overflow error gives
    /// (see [`GenSpawn::compaction_enabled`]).
    compaction_enabled: bool,
}

/// One agentic loop's state: the turn's own, or a sub-agent's run inside it.
/// Moved verbatim out of [`spawn_generation`]'s async block (Sonar S3776): the
/// loop itself is [`Self::run`], one tool round is [`Self::tool_round`], one call
/// — [`Self::execute_call`] / [`Self::resolve_call_result`]. The struct follows
/// the module's parameter-struct pattern ([`GenSpawn`], [`ConfirmGate`]); it
/// still never touches `Chat` — results go back through [`GenResult`].
struct TurnLoop<'a> {
    shared: &'a mut TurnShared,
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
    /// Nesting level: `0` for the turn's own loop, `1` for a sub-agent's run.
    /// [`Self::run_subagent`] refuses below the top — the belt under the braces
    /// of an allowed set that never offers `call_subagent` there.
    depth: u8,
    /// Whether this loop's stream reaches the feed. The turn's own loop is
    /// live; a sub-agent's is **muted** — its text would land in the parent's
    /// bubble — and only its token counter passes, re-based on
    /// [`Self::token_base`] (see [`RoundSink`]).
    live: bool,
    /// What the turn had already cost when this loop started: a sub-agent's
    /// counter continues the parent's rather than restarting at zero, because
    /// the user pays for both.
    token_base: u64,
    /// Which budget ended this loop, when one did — the parent reads it to
    /// record a sub-agent's outcome as `RoundLimit` rather than `Completed`.
    ended_by_limit: Option<RoundLimit>,
}

/// The limits of one sub-agent run, snapshotted from `config.tools` with the
/// rest of the turn's configuration (spec §9.3.2, docs/research/subagent-chats.md §3.12).
#[derive(Debug, Clone, Copy)]
struct SubagentLimits {
    /// The per-round reply cap, min'ed with the effective `max_tokens`.
    max_tokens: usize,
    /// The whole run — every round and tool call of it.
    run_timeout: std::time::Duration,
}

impl SubagentLimits {
    fn from_config(tools: &crate::shared::config::ToolSettings) -> Self {
        Self {
            max_tokens: tools.subagent_max_tokens,
            run_timeout: std::time::Duration::from_secs(tools.subagent_run_timeout_secs),
        }
    }
}

/// Where one loop's events go. The turn's own loop sends everything; a
/// sub-agent's loop is muted except for the token counter, whose `context`
/// half is dropped too — the child's prompt size is not the conversation's,
/// and the status bar shows one number (research §3.5).
struct RoundSink<'a> {
    evt_tx: &'a UnboundedSender<AppEvent>,
    live: bool,
}

impl RoundSink<'_> {
    fn send(&self, event: AppEvent) {
        if self.live {
            let _ = self.evt_tx.send(event);
            return;
        }
        if let AppEvent::TokenUsage {
            generation_id,
            completion,
            reasoning,
            ..
        } = event
        {
            let _ = self.evt_tx.send(AppEvent::TokenUsage {
                generation_id,
                completion,
                context: None,
                context_exact: false,
                reasoning,
            });
        }
    }
}

impl TurnLoop<'_> {
    /// Is the tool in the turn's effectively allowed set (profile ∩ global
    /// switches)?
    fn allowed_has(&self, name: &str) -> bool {
        self.allowed.iter().any(|t| t == name)
    }

    /// This loop's event sink (see [`RoundSink`]).
    fn sink(&self) -> RoundSink<'_> {
        RoundSink {
            evt_tx: &self.shared.evt_tx,
            live: self.live,
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
        let out = stream_round(
            &self.shared.backend,
            self.request.clone(),
            &self.cancel,
            self.shared.id,
            &self.sink(),
            self.token_base + self.total_tokens,
            self.total_reasoning,
            self.shared.ui_loc,
            self.shared.compaction_enabled,
        )
        .await;
        if let Some(prompt_tokens) = out.prompt_tokens {
            self.last_usage = Some(TurnUsage {
                prompt_tokens,
                completion_tokens: out.tokens,
            });
        }
        out
    }

    async fn run(&mut self) -> FinishReason {
        loop {
            let out = self.stream().await;
            self.total_tokens += out.tokens;
            self.total_reasoning += out.reasoning_tokens;

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
                &self.ctx,
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
            &self.ctx,
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
        // extended thinking (Anthropic) we attach a thinking block with a
        // signature: an assistant turn with tool_use in the same turn is
        // required to carry it, otherwise the next request → 400. The
        // signature exists only if the model actually returned "thoughts";
        // other backends ignore the field.
        let thinking = out.thinking_ref.clone().map(|r| ThinkingBlock {
            text: out.thoughts.clone(),
            signature: r.signature,
            id: r.id,
        });
        self.request.messages.push(
            ApiMessage::assistant_tool_calls(out.text.clone(), out.calls.clone())
                .with_thinking(thinking),
        );
        let mut records: Vec<ToolCallRecord> = Vec::new();
        let mut tool_msgs: Vec<Message> = Vec::new();
        for call in &out.calls {
            self.execute_call(call, rewrite, &mut records, &mut tool_msgs)
                .await;
        }

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

    /// Executes one tool call: resolves its result (gates/confirmation/the
    /// actual invocation), emits the UI tool block, and records the call into
    /// the request history + the round's domain records.
    async fn execute_call(
        &mut self,
        call: &ApiToolCall,
        rewrite: bool,
        records: &mut Vec<ToolCallRecord>,
        tool_msgs: &mut Vec<Message>,
    ) {
        // A no-argument call gives an empty argument string — we store it
        // as an empty OBJECT, not `Null`: otherwise serializing the history
        // entry gives `"null"`, and strict providers (Anthropic) expect an
        // object in `input` (see shared/api/anthropic/wire.rs). An object is
        // also safer for invoke (deserializing a struct from `null` panics).
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
        let is_control = control::is_control_tool(&call.name);
        let CallResult {
            text: result,
            images,
            subagent,
        } = self
            .resolve_call_result(call, &args, is_control, rewrite)
            .await;
        // Decoded and downscaled here, once, so the same prepared bytes go into the
        // request and into the stored message — the object the model sees and the object
        // the chat keeps must be one (spec §9.10).
        let images = prepare_tool_images(images, self.shared.image_cfg).await;
        // A UI tool block — only for regular executed calls (the internal
        // followup/rewrite ones, and ones skipped during a rewrite, don't get one).
        if !is_control && !rewrite {
            self.sink().send(AppEvent::ToolCall {
                generation_id: self.shared.id,
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                result: result.clone(),
                images: images.len(),
            });
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
            arguments: args,
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
            &mut self.shared.allowed_for_turn,
            &mut self.shared.confirm_rx,
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
    /// Runs a sub-agent (spec §9.3.2, docs/research/subagent-chats.md §3.2–§3.4):
    /// a child loop of the same type over this turn's shared part, with this
    /// turn's tools minus the withheld ones, this turn's environment, and a
    /// persona and a single message of its own. Returns the model's result text
    /// and the run for the record.
    ///
    /// The child borrows `self.shared` for the duration of the call — sound,
    /// because this loop is suspended here until it returns — so everything the
    /// parent needs afterwards is read into locals first.
    async fn run_subagent(&mut self, args: &serde_json::Value) -> CallResult {
        let loc = self.ctx.loc;
        let parsed = match SubagentArgs::parse(args, loc) {
            Ok(a) => a,
            Err(err) => {
                return loc
                    .tf(
                        "loop.tool_error",
                        &[("name", CALL_SUBAGENT_ID), ("err", &err.to_string())],
                    )
                    .into();
            }
        };
        // No nesting, whatever the allowed set says (it never offers the tool
        // below the top, so this is the second lock on the same door).
        if self.depth > 0 {
            return loc
                .tf("loop.tool_disabled", &[("name", CALL_SUBAGENT_ID)])
                .into();
        }
        let started = chrono::Utc::now();
        let limits = self.shared.subagent;
        let max_rounds = self.shared.max_rounds;

        // The turn's tools minus the withheld (research §3.3), in the turn's
        // order; the schemas follow, so the model never sees what it may not call.
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
        // and a token of its own that the parent's cancels with it.
        let cancel = self.cancel.child_token();
        let mut ctx = self.ctx.clone();
        ctx.system_message = parsed.system_message.clone();
        ctx.effective_sampling = sampling.clone();
        ctx.last_user_message_at = Some(started);
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

        let token_base = self.token_base + self.total_tokens;
        let mut child = TurnLoop {
            shared: &mut *self.shared,
            ctx,
            request,
            cancel: cancel.clone(),
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
            depth: self.depth + 1,
            live: false,
            token_base,
            ended_by_limit: None,
        };
        // Boxed: `run` → `tool_round` → `execute_call` → here → `run` is a
        // recursive async chain, and the compiler needs one indirection in it.
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
        // Everything the parent keeps, out of the child before its borrow ends.
        // Its own discarded drafts (`rewrite_current_message`) are dropped: the
        // archive's promise is recovering what the *user* lost (research §3.6).
        let child_messages = std::mem::take(&mut child.messages);
        let child_effects = std::mem::take(&mut child.effects);
        let child_tokens = child.total_tokens;
        let child_reasoning = child.total_reasoning;
        drop(child);

        // Effects go to the chat they describe (research §3.4): identity to the
        // run, environment to the parent — which also mirrors an attachment into
        // this loop's snapshot at the round's end, as for any tool.
        let mut run = SubagentRun {
            id: Uuid::new_v4(),
            kind: RunKind::Subagent,
            title: parsed.initial_title(),
            renamed_manually: false,
            name: parsed.name.clone(),
            created_at: started,
            finished_at: Some(chrono::Utc::now()),
            system_message: parsed.system_message.clone(),
            sampling_override: None,
            messages: std::iter::once(user).chain(child_messages).collect(),
            outcome: Some(outcome),
            tokens: child_tokens,
        };
        for effect in child_effects {
            match effect {
                ChatEffect::SetSystemMessage(s) => run.system_message = s,
                ChatEffect::SetSamplingOverride(s) => run.sampling_override = Some(*s),
                a @ ChatEffect::AddAttachment(_) => self.effects.push(a),
            }
        }
        self.total_tokens += child_tokens;
        self.total_reasoning += child_reasoning;

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
            RunOutcome::Failed => {
                loc.tf("tool.call_subagent.result.failed", &[("address", &address)])
            }
            RunOutcome::RoundLimit => loc.tf(
                "tool.call_subagent.result.round_limit",
                &[
                    ("address", &address),
                    ("max_rounds", &max_rounds.to_string()),
                ],
            ),
        };
        CallResult {
            text: format!("{body}\n\n{status}"),
            images: Vec::new(),
            subagent: Some(Box::new(run)),
        }
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
    ui_loc: &'static crate::shared::i18n::Locale,
) -> String {
    ui_loc.tf(
        engine_error_key(err, compaction_enabled, partial),
        &[("err", err)],
    )
}

/// Which of the four things to tell the user — split out from
/// [`engine_error_note`] so the decision can be tested as a decision, without
/// asserting on localized prose.
pub(super) fn engine_error_key(err: &str, compaction_enabled: bool, partial: bool) -> &'static str {
    match (
        crate::features::compaction::is_context_overflow(err),
        compaction_enabled,
        partial,
    ) {
        (true, true, _) => "ui.err.context_overflow",
        (true, false, _) => "ui.err.context_overflow_off",
        (false, _, true) => "ui.err.generation_interrupted",
        (false, _, false) => "ui.err.generation_failed",
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
    base_tokens: u64,
    base_reasoning: u32,
    ui_loc: &'static crate::shared::i18n::Locale,
    compaction_enabled: bool,
) -> RoundOutput {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut thoughts_signature: Option<String> = None;
    let mut thoughts_id: Option<String> = None;
    let mut acc = ToolCallAccumulator::default();
    let mut reason = FinishReason::Stop;
    // Live count: the number of reply deltas (≈ tokens). If it arrives, the exact
    // value from the server's `usage` replaces the approximation.
    let mut streamed: u64 = 0;
    let mut usage_tokens: Option<u64> = None;
    // The exact prompt size, when the server reports one. Deliberately without an
    // estimate fallback — see `RoundOutput::prompt_tokens`.
    let mut usage_prompt: Option<u32> = None;
    // The round's reasoning tokens (from `usage`; `0` — the provider doesn't separate them).
    let mut round_reasoning: u32 = 0;

    // The reply counter: `context: None` leaves the prior conversation estimate
    // untouched (emitted by start_generation); the exact `context` only comes from the server's usage.
    let emit_completion = |completion: u64| {
        sink.send(AppEvent::TokenUsage {
            generation_id: id,
            completion,
            context: None,
            context_exact: false,
            reasoning: None,
        });
    };

    match backend.chat_stream(request, cancel.clone()).await {
        Ok(mut stream) => {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        text.push_str(&t);
                        streamed += 1;
                        sink.send(AppEvent::Chunk {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(base_tokens + streamed);
                    }
                    ChatChunk::Thoughts(t) => {
                        thoughts.push_str(&t);
                        streamed += 1;
                        sink.send(AppEvent::Thoughts {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(base_tokens + streamed);
                    }
                    // A reference to the reasoning (Anthropic signature / OpenAI
                    // reasoning item) — not shown in the UI, accumulated for resending
                    // on tool use. `id` is carried only by OpenAI Responses (reasoning
                    // `rs_…`); the signature/encrypted content — by both.
                    ChatChunk::ThoughtsSignature(r) => {
                        thoughts_signature
                            .get_or_insert_with(String::new)
                            .push_str(&r.signature);
                        if r.id.is_some() {
                            thoughts_id = r.id;
                        }
                    }
                    ChatChunk::ToolCall(delta) => acc.push(delta),
                    ChatChunk::Usage(u) => {
                        // The exact count from the server: both the reply and the
                        // conversation (prompt) — replaces the delta-based approximation
                        // and the conversation estimate. Reasoning tokens ("thoughts") —
                        // cumulative across rounds (base + current).
                        usage_tokens = Some(u.completion_tokens as u64);
                        usage_prompt = Some(u.prompt_tokens);
                        round_reasoning = u.reasoning_tokens;
                        sink.send(AppEvent::TokenUsage {
                            generation_id: id,
                            completion: base_tokens + u.completion_tokens as u64,
                            context: Some(u.prompt_tokens as u64),
                            context_exact: true,
                            reasoning: Some(base_reasoning + u.reasoning_tokens),
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
                        sink.send(AppEvent::Error(engine_error_note(
                            &message,
                            compaction_enabled,
                            partial,
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
                ui_loc,
            )));
            reason = FinishReason::Error;
        }
    }

    let thinking_ref =
        (thoughts_signature.is_some() || thoughts_id.is_some()).then(|| ThinkingRef {
            id: thoughts_id,
            signature: thoughts_signature.unwrap_or_default(),
        });

    RoundOutput {
        text,
        thoughts,
        thinking_ref,
        calls: acc.finish(),
        reason,
        tokens: usage_tokens.unwrap_or(streamed),
        prompt_tokens: usage_prompt,
        reasoning_tokens: round_reasoning,
    }
}

/// Client-side estimate of the prompt's token count (the whole conversation) for
/// the live indicator before the server's exact `usage.prompt_tokens` arrives.
/// Accounts for the system message, message texts, and tool-call arguments in
/// the history.
fn estimate_prompt_tokens(req: &ChatRequest) -> u64 {
    let mut parts: Vec<&str> = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        parts.push(m.content.as_str());
        for tc in &m.tool_calls {
            parts.push(tc.arguments.as_str());
        }
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
fn finalize_message(
    out: &RoundOutput,
    ctx: &ToolContext,
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
        sampling: ctx
            .effective_sampling
            .retain_supported(mode.cloud_provider()),
        mode,
        model: model.clone(),
    });
    Some(m)
}
