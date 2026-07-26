//! Assistant reply generation: send/regenerate/delete-exchange commands,
//! starting a turn, and the background task for the client-side agentic loop (spec §6.3).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::DeletedCause;
use crate::entities::message::{Message, MessageMetadata, MessageRole, ToolCallRecord};
use crate::entities::profile::ToolId;
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
use super::request::{build_request, last_user_message_at};

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
}

impl Orchestrator {
    pub(super) fn handle_send(&mut self, text: String) {
        if !self.gen_state.is_idle() {
            return;
        }
        let text = text.trim().to_string();
        if text.is_empty() {
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

        // Add the user's message to the history and echo it in the feed. The UI
        // cleared the input box on send — also clear the chat's saved draft.
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.push_message(Message::user(&text));
            chat.draft.clear();
        }
        self.mark_dirty(active_id);
        let _ = self.evt_tx.send(AppEvent::UserMessage(text));

        self.start_generation(active_id, backend);
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
        // The effective set = profile ∩ global switches (spec §9.4).
        let allowed = effective_tool_ids(
            &enabled,
            self.config.tools.web_enabled,
            self.config.tools.python_enabled,
            self.config.tools.fs_enabled,
            self.config.mcp.enabled,
            self.config.engine.mode.cloud_provider(),
        );
        let profile_loc = crate::shared::i18n::locale(profile_lang);
        let schemas = self.registry.schemas_for(&allowed, profile_loc);
        // Copied out before the `chat_mut` borrow below (config can't be read
        // while `Chat` is mutably borrowed). `AttachmentSettings` is `Copy`.
        let attach_cfg = self.config.attachments;

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
            request = build_request(chat, sampling.clone(), schemas, &attach_cfg, profile_loc);
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
        let _ = self
            .evt_tx
            .send(AppEvent::GenerationStarted { generation_id: id });
        self.gen_state.begin(id, cancel.clone());
        spawn_generation(GenSpawn {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            cancel,
            id,
            chat_id: active_id,
            max_rounds: self.config.max_tool_rounds,
            allowed,
            self_model,
            self_model_params,
            inject_enabled,
            maintenance_protocol: self.config.self_model.maintenance_protocol,
            last_user,
            engine_mode: self.config.engine.mode,
            model_name: self.config.engine.active_model_name(),
            ui_loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
            done_tx: self.done_tx.clone(),
        });
    }

    pub(super) fn handle_done(&mut self, res: GenResult) {
        // Apply only the result of the current generation (protection against
        // stale ones): finish() transitions to Idle only on a matching id.
        if !self.gen_state.finish(res.id) {
            return;
        }

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
            for effect in res.effects {
                match effect {
                    ChatEffect::SetSystemMessage(s) => chat.system_message = s,
                    ChatEffect::SetSamplingOverride(s) => chat.sampling_override = Some(*s),
                }
            }
            self.mark_dirty(res.chat_id);
            self.emit_chat_list();
        }
        if self_model_touched {
            let _ = self.evt_tx.send(AppEvent::SelfModelChanged);
        }
        // After a successful reply — maybe it's time for background auto-reflection
        // (Tier 3), notes auto-consolidation ("sleep", Tier 3), and/or self-model
        // auto-consolidation ("sleep" for the self-model, stage A1 —
        // docs/history/self-model-consolidation.md).
        self.maybe_auto_reflect(res.chat_id);
        self.maybe_auto_consolidate(res.chat_id);
        self.maybe_auto_self_consolidate(res.chat_id);
    }
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
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenResult>,
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
        id,
        chat_id,
        max_rounds,
        allowed,
        self_model,
        self_model_params,
        inject_enabled,
        maintenance_protocol,
        last_user,
        engine_mode,
        model_name,
        ui_loc,
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

        let mut messages: Vec<Message> = Vec::new();
        let mut effects: Vec<ChatEffect> = Vec::new();
        // Discarded by the "rewrite" tool (for the deleted archive).
        let mut deleted: Vec<Message> = Vec::new();
        let mut round: u32 = 0;
        // Cumulative reply-token counter across all agentic-loop rounds — the
        // live indicator keeps growing from round to round.
        let mut total_tokens: u64 = 0;
        // Cumulative reasoning tokens ("thoughts") across rounds.
        let mut total_reasoning: u32 = 0;
        // The next domain assistant message starts a new bubble (after
        // `send_followup_message`). See spec §9.3.
        let mut pending_new_bubble = false;
        let reason;

        let allowed_has = |name: &str| allowed.iter().any(|t| t == name);

        loop {
            let out = stream_round(
                &backend,
                request.clone(),
                &cancel,
                id,
                &evt_tx,
                total_tokens,
                total_reasoning,
                ui_loc,
            )
            .await;
            total_tokens += out.tokens;
            total_reasoning += out.reasoning_tokens;

            // A round with tool calls — execute and continue the loop.
            if out.reason == FinishReason::ToolCalls && !out.calls.is_empty() {
                if round >= max_rounds {
                    // Limit reached: DON'T execute new calls, ask the model instead
                    // to sum up what's already been gathered — a final round WITHOUT
                    // tools. Otherwise (the previous behavior) `out` would only
                    // contain an intent to call more tools with empty text →
                    // `finalize_message` returned `None`, and the user got no reply
                    // at all, even though enough data had accumulated over the
                    // previous rounds. Tools are removed from the request, so the
                    // model must answer with text (the stream goes into the feed).
                    let _ = evt_tx.send(AppEvent::Error(ctx.loc.tf(
                        "loop.round_limit_reached",
                        &[("max_rounds", &max_rounds.to_string())],
                    )));
                    request.tools.clear();
                    // The final round's token counter is emitted by `stream_round` itself
                    // (from `base = total_*`); after that we `break`, no need to accumulate.
                    let final_out = stream_round(
                        &backend,
                        request.clone(),
                        &cancel,
                        id,
                        &evt_tx,
                        total_tokens,
                        total_reasoning,
                        ui_loc,
                    )
                    .await;
                    if let Some(mut m) =
                        finalize_message(&final_out, &ctx, engine_mode, &model_name)
                    {
                        m.new_bubble = pending_new_bubble;
                        messages.push(m);
                    }
                    // The finish reason comes from the final round (usually Stop; on
                    // user cancellation/a stream error — Cancelled/Error), not an
                    // artificial Stop.
                    reason = final_out.reason;
                    break;
                }
                round += 1;

                // Conversation control tools (spec §9.3) are recognized only if
                // they're actually enabled in the profile — otherwise a plain
                // refusal below. `rewrite` discards the current round; `followup`
                // starts a new bubble.
                let rewrite = out
                    .calls
                    .iter()
                    .any(|c| c.name == control::REWRITE_CURRENT_ID && allowed_has(&c.name));
                let followup = out
                    .calls
                    .iter()
                    .any(|c| c.name == control::SEND_FOLLOWUP_ID && allowed_has(&c.name));

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
                request.messages.push(
                    ApiMessage::assistant_tool_calls(out.text.clone(), out.calls.clone())
                        .with_thinking(thinking),
                );
                let mut records: Vec<ToolCallRecord> = Vec::new();
                let mut tool_msgs: Vec<Message> = Vec::new();
                for call in &out.calls {
                    // A no-argument call gives an empty argument string — we store it
                    // as an empty OBJECT, not `Null`: otherwise serializing the history
                    // entry gives `"null"`, and strict providers (Anthropic) expect an
                    // object in `input` (see shared/api/anthropic/wire.rs). An object is
                    // also safer for invoke (deserializing a struct from `null` panics).
                    let args: serde_json::Value = serde_json::from_str(&call.arguments)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    let is_control = control::is_control_tool(&call.name);
                    let result = if !allowed_has(&call.name) {
                        // Protection: the tool is disabled globally/in the profile.
                        ctx.loc.tf("loop.tool_disabled", &[("name", &call.name)])
                    } else if is_control {
                        // A control tool: the result is "permission" (the model will
                        // see it in the next round). Executed by the loop, not
                        // through the registry.
                        control::control_permission_text(&call.name, ctx.loc)
                    } else if rewrite {
                        // This round is being discarded — side-effect tools aren't executed.
                        ctx.loc.t("loop.rewrite_skipped").to_string()
                    } else {
                        // Execution under a `select!` with the cancellation token: Esc
                        // doesn't wait for a long-running tool (MCP/network) to finish.
                        // Tools that read `ctx.cancel` terminate themselves (MCP sends
                        // the server notifications/cancelled); this is a safety net
                        // for the rest.
                        let invoked = tokio::select! {
                            _ = cancel.cancelled() => None,
                            res = registry.invoke(&call.name, &ctx, args.clone()) => Some(res),
                        };
                        match invoked {
                            None => ctx.loc.t("loop.tool_cancelled").to_string(),
                            Some(Ok(outcome)) => {
                                effects.extend(outcome.effects);
                                outcome.result
                            }
                            Some(Err(err)) => ctx.loc.tf(
                                "loop.tool_error",
                                &[("name", &call.name), ("err", &err.to_string())],
                            ),
                        }
                    };
                    // A UI tool block — only for regular executed calls (the internal
                    // followup/rewrite ones, and ones skipped during a rewrite, don't get one).
                    if !is_control && !rewrite {
                        let _ = evt_tx.send(AppEvent::ToolCall {
                            generation_id: id,
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            result: result.clone(),
                        });
                    }
                    request.messages.push(ApiMessage::tool(&call.id, &result));
                    records.push(ToolCallRecord {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: args,
                        result: Some(result.clone()),
                        // The thought signature (Gemini 3) is persisted — needed on history replay.
                        thought_signature: call.thought_signature.clone(),
                    });
                    tool_msgs.push(tool_message(call, result));
                }

                // The round's domain assistant message (text + thoughts + tool blocks).
                let mut am = Message::assistant(out.text.clone());
                if !out.thoughts.is_empty() {
                    am.thoughts = Some(out.thoughts.clone());
                }
                am.tool_calls = records;

                if rewrite {
                    // Discard the round: assistant + tool messages → the deleted archive.
                    // The live feed clears the current bubble for the rewritten reply.
                    // `pending_new_bubble` is deliberately left alone (the final round absorbs it).
                    deleted.push(am);
                    deleted.extend(tool_msgs);
                    let _ = evt_tx.send(AppEvent::AssistantRewrite { generation_id: id });
                } else {
                    // assistant BEFORE this round's tool messages.
                    am.new_bubble = std::mem::take(&mut pending_new_bubble);
                    messages.push(am);
                    messages.extend(tool_msgs);
                    if followup {
                        // The next assistant message — as a separate bubble.
                        pending_new_bubble = true;
                        let _ = evt_tx.send(AppEvent::AssistantContinue { generation_id: id });
                    }
                }
                // The turn was cancelled while tools were executing — what's
                // accumulated is already saved above, don't start the next round.
                if cancel.is_cancelled() {
                    reason = FinishReason::Cancelled;
                    break;
                }
                continue;
            }

            // The final round (Stop/Length/Cancelled/Error, or no calls).
            if let Some(mut m) = finalize_message(&out, &ctx, engine_mode, &model_name) {
                m.new_bubble = pending_new_bubble;
                messages.push(m);
            }
            reason = out.reason;
            break;
        }

        let _ = evt_tx.send(AppEvent::Finished {
            generation_id: id,
            reason,
        });
        let _ = done_tx.send(GenResult {
            id,
            chat_id,
            messages,
            effects,
            deleted,
        });
    });
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
    evt_tx: &UnboundedSender<AppEvent>,
    base_tokens: u64,
    base_reasoning: u32,
    ui_loc: &'static crate::shared::i18n::Locale,
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
    // The round's reasoning tokens (from `usage`; `0` — the provider doesn't separate them).
    let mut round_reasoning: u32 = 0;

    // The reply counter: `context: None` leaves the prior conversation estimate
    // untouched (emitted by start_generation); the exact `context` only comes from the server's usage.
    let emit_completion = |completion: u64| {
        let _ = evt_tx.send(AppEvent::TokenUsage {
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
                        let _ = evt_tx.send(AppEvent::Chunk {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(base_tokens + streamed);
                    }
                    ChatChunk::Thoughts(t) => {
                        thoughts.push_str(&t);
                        streamed += 1;
                        let _ = evt_tx.send(AppEvent::Thoughts {
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
                        round_reasoning = u.reasoning_tokens;
                        let _ = evt_tx.send(AppEvent::TokenUsage {
                            generation_id: id,
                            completion: base_tokens + u.completion_tokens as u64,
                            context: Some(u.prompt_tokens as u64),
                            context_exact: true,
                            reasoning: Some(base_reasoning + u.reasoning_tokens),
                        });
                    }
                    ChatChunk::Finished(r) => {
                        reason = r;
                        break;
                    }
                }
            }
        }
        Err(err) => {
            let _ = evt_tx.send(AppEvent::Error(
                ui_loc.tf("ui.err.generation_failed", &[("err", &err.to_string())]),
            ));
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
