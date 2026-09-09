//! Impersonation (`Ctrl+U`, spec §11.8): the model writes the next message "on behalf
//! of the user". The system message is replaced with the impersonation one, the
//! user/assistant roles in history are swapped. A background task streams the reply.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::features::tools::self_model;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason};

use super::Orchestrator;

/// The time limit for one impersonation (a safety net against a stuck task). Generous:
/// on a slow local `llama-server`, prompt processing alone can take ~a minute, plus
/// generation on the CPU runs at ~5 tok/s — at 120s the reply would get cut off mid-word
/// (see the log `srv stop: cancel task` at exactly 120s). On a timeout, the accumulated text
/// **isn't lost** (handed to the field as if the limit were reached) — see `spawn_impersonation`.
const IMPERSONATION_TIMEOUT: Duration = Duration::from_secs(600);

/// What the impersonation task leaves for the orchestrator's landing: the
/// generation it was, how its stream ended, and the engine's timing of its
/// prompt — the whole conversation with the roles swapped under its own
/// system, processed cold, the largest prompt a session makes — kept only on
/// the **shared** engine, the one server the slow-prefill rule knows
/// (docs/research/oneshot-samples.md §3.1). `None` from a stream that ended
/// before its usage chunk and from a separate impersonation server.
pub(super) struct ImpDone {
    pub(super) id: Uuid,
    pub(super) reason: FinishReason,
    pub(super) prefill: Option<crate::shared::api::contract::Prefill>,
}

impl Orchestrator {
    /// The impersonation system message for a chat's profile: the user persona from the
    /// impersonation profile the assistant profile points at (spec §11.8). No reference,
    /// a dangling id (the persona was deleted), or an empty message → the shared default
    /// text, so impersonation always has something to work from.
    pub(super) fn impersonation_system(
        &self,
        profile: Option<&crate::entities::profile::Profile>,
        loc: &'static crate::shared::i18n::Locale,
    ) -> String {
        profile
            .and_then(|p| p.impersonation_profile_id)
            .and_then(|id| {
                self.config
                    .impersonation_profiles
                    .iter()
                    .find(|ip| ip.id == id)
            })
            .map(|ip| ip.system_message.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| loc.t("prompt.impersonation.default").to_string())
    }

    /// Writes a message on behalf of the user (impersonation, `Ctrl+U`, spec §11.8):
    /// the assistant's system message is replaced with the profile's impersonation one, and
    /// the user/assistant roles in history are swapped — the model continues the
    /// conversation "on behalf of the user". The text streams into the input-box preview. Ignored
    /// during generation/another impersonation.
    pub(super) fn handle_impersonate(&mut self, seed: String) {
        if !self.gen_state.is_idle() || self.imp_gen.is_some() {
            return;
        }
        let Some(active_id) = self.active_id else {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale().t("ui.err.no_active_chat").into(),
            ));
            return;
        };
        let backend = match self
            .engines
            .impersonation_backend_if_ready(self.config.impersonation_engine.mode, self.ui_locale())
        {
            Ok(backend) => backend,
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                return;
            }
        };
        let Some(chat) = self.chats.iter().find(|c| c.id == active_id) else {
            return;
        };
        let loc = self.profile_locale(chat.profile_id);
        let profile = self.profiles.iter().find(|p| p.id == chat.profile_id);
        let imp_system = self.impersonation_system(profile, loc);
        // The interlocutor model is mixed into the impersonation prompt (the agent writes ON BEHALF
        // OF the human) — but only if the profile enabled the self-model (the same opt-in gate
        // as passive injection into a regular turn).
        let user_hint = profile
            .filter(|p| {
                p.enabled_tools
                    .iter()
                    .any(|t| t == self_model::GET_SELF_MODEL_ID)
            })
            .and_then(|_| {
                self.storage
                    .db()
                    .self_model_get(chat.profile_id)
                    .ok()
                    .flatten()
            })
            .and_then(|m| {
                let cap = crate::entities::self_model::SelfModelParams::from_settings(
                    &self.config.self_model,
                )
                .prompt_cap;
                m.user_model.render_for_impersonation(cap, loc)
            });
        let request = build_impersonation_request(
            chat,
            chat.compaction_view(self.config.compaction.enabled),
            imp_system,
            &seed,
            self.config.impersonation_sampling.clone(),
            user_hint.as_deref(),
            loc,
        );

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        self.imp_gen = Some(id);
        self.imp_cancel = Some(cancel.clone());
        let _ = self
            .evt_tx
            .send(AppEvent::ImpersonationStarted { generation_id: id });
        // On the shared engine the request is one more stream on the chat
        // engine's pool, so it takes the budget's silent lane and waits for
        // room like the app's other background requests
        // (docs/research/silent-tasks-budget.md §4.2, fork F1); a separate
        // impersonation server has a pool of its own and one stream at a
        // time by this method's gate — nothing to guard.
        let sessions = (self.config.impersonation_engine.mode
            == crate::shared::config::ImpersonationMode::Shared)
            .then(|| self.session_budget());
        spawn_impersonation(
            backend,
            request,
            id,
            cancel,
            self.ui_locale(),
            self.evt_tx.clone(),
            self.imp_done_tx.clone(),
            sessions,
        );
    }

    /// Cancels the current impersonation (`Esc` in the preview). Completion arrives
    /// through `imp_done` and emits `ImpersonationFinished{Cancelled}`.
    pub(super) fn handle_cancel_impersonation(&mut self) {
        if let Some(token) = &self.imp_cancel {
            token.cancel();
        }
    }

    /// Completion of the background impersonation task: clears the state and
    /// emits the final result, then offers the engine's timing of the prompt
    /// to the slow-prefill rule — after the request's own landing, the roll's
    /// shape (docs/research/oneshot-samples.md §3.1). A superseded generation
    /// lands nothing, but its prompt was processed on this server all the
    /// same, so its sample is offered too.
    pub(super) fn handle_imp_done(&mut self, done: ImpDone) {
        let ImpDone {
            id,
            reason,
            prefill,
        } = done;
        if self.imp_gen == Some(id) {
            self.imp_gen = None;
            self.imp_cancel = None;
            let _ = self.evt_tx.send(AppEvent::ImpersonationFinished {
                generation_id: id,
                reason,
            });
        }
        self.note_slow_prefill(prefill);
    }
}

/// Builds the impersonation request (spec §11.8): the system message is the
/// impersonation one (the user's persona), the user/assistant roles in history are swapped (the
/// model continues the conversation "on behalf of the user"). No tools. If `seed` isn't empty,
/// the model is asked to continue text that's already started.
///
/// `compaction` — the chat's compaction view
/// ([`Chat::compaction_view`](crate::entities::chat::Chat::compaction_view)), i.e.
/// the rolling summary and the index its folded prefix ends at (spec §6.7).
/// Impersonation sends the conversation too, so without this it would keep
/// hitting the very context ceiling the compression track exists to remove —
/// only from a different key. Passed as the pair the view already returns, so a
/// summary can never arrive without the cut it describes, or the other way
/// round. `None` (nothing folded, or the master switch is off) leaves the
/// request byte-for-byte what it was before compression existed.
///
/// The block is injected with `tools = false`: impersonation has **no** tools,
/// so the model is told to work from the summary rather than pointed at
/// `history_read`/`history_search` it cannot call — the same rule a regular turn
/// follows (sub-decision S12, see [`inject_compaction`]).
pub(super) fn build_impersonation_request(
    chat: &Chat,
    compaction: Option<(&str, usize)>,
    mut system: String,
    seed: &str,
    mut sampling: SamplingConfig,
    user_hint: Option<&str>,
    loc: &crate::shared::i18n::Locale,
) -> ChatRequest {
    // Impersonation writes the reply into the input box and **discards** "thoughts" (Thoughts
    // are ignored in `spawn_impersonation`), so it doesn't need reasoning. What actually
    // matters — `reasoning_budget=0`: for models with thinking "baked into" the template (Gemma
    // `peg-gemma4`, Qwen) only this field actually suppresses "thoughts" (+ `enable_thinking=false`
    // in `wire.rs`); the server ignores the `thinking`/`reasoning_effort` fields for them.
    // Without this the model would spend its whole token budget on reasoning_content, and the reply's
    // `content` would come back empty — the preview stayed empty (the same bug class
    // as auto-title, see `title.rs`).
    sampling.thinking = Some(false);
    sampling.reasoning_effort = Some(ReasoningEffort::None);
    sampling.reasoning_budget = Some(0);
    let (summary, upto) = match compaction {
        Some((s, i)) => (Some(s), i),
        None => (None, 0),
    };
    // Ordered by volatility, as in `build_request`: the persona never changes,
    // the summary only on a compaction, the interlocutor model most often — and
    // the seed continuation stays last, being the immediate instruction.
    // `inject_compaction` returns `Some` for a `Some` input whatever the summary
    // is, so the fallback is unreachable.
    system =
        super::request::inject_compaction(Some(system), summary, false, loc).unwrap_or_default();
    // A hint about the interlocutor (a model of who we're writing on behalf of) — before the seed continuation.
    if let Some(hint) = user_hint.map(str::trim).filter(|s| !s.is_empty()) {
        system.push_str("\n\n");
        system.push_str(hint);
    }
    // The folded prefix is replaced by the summary block above; `chat.messages`
    // is untouched, exactly as on a regular turn.
    let messages = chat.messages[upto..]
        .iter()
        .filter_map(swap_role_message)
        .collect();
    let seed = seed.trim();
    if !seed.is_empty() {
        system.push_str("\n\n");
        system.push_str(&loc.tf("prompt.impersonation.continue", &[("seed", seed)]));
    }
    ChatRequest {
        continue_final: false,
        system: Some(system),
        messages,
        sampling,
        tools: Vec::new(),
    }
}

/// Swaps a message's role for impersonation (user↔assistant). System/Tool and
/// empty messages are dropped (there are no tools in impersonation mode).
pub(super) fn swap_role_message(message: &Message) -> Option<ApiMessage> {
    if message.text.trim().is_empty() {
        return None;
    }
    match message.role {
        MessageRole::User => Some(ApiMessage::assistant(&message.text)),
        MessageRole::Assistant => Some(ApiMessage::user(&message.text)),
        MessageRole::System | MessageRole::Tool => None,
    }
}

/// Starts the background impersonation task: streams the reply text into the preview
/// (`ImpersonationChunk`), and on completion/timeout/cancellation sends `(id, reason)` into
/// `done_tx`. "Thoughts" and tool calls are ignored (only text goes into the input box).
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_impersonation(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    id: Uuid,
    cancel: CancellationToken,
    loc: &'static crate::shared::i18n::Locale,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<ImpDone>,
    sessions: Option<Arc<crate::shared::session_budget::SessionBudget>>,
) {
    tokio::spawn(async move {
        let run = async {
            let mut reason = FinishReason::Stop;
            // The engine's timing of the prompt, kept under the record's own
            // condition — a budget, i.e. the shared engine (oneshot-samples
            // §3.1, fork F3).
            let mut prefill = None;
            let estimate = super::generation::estimate_prompt_tokens(&request);
            let _lane = match sessions.as_deref() {
                Some(budget) => {
                    let need = budget.price(
                        crate::shared::session_budget::Shape::Impersonation,
                        estimate,
                        0,
                        request.sampling.max_tokens.map(|m| m as u64),
                    );
                    // The user's own request, never displaced for a background
                    // run's round (silent-preemption §4.3, R4).
                    match budget
                        .acquire_silent(need, &cancel, "impersonation", false)
                        .await
                    {
                        Some(reservation) => Some(reservation),
                        None => return Ok((FinishReason::Cancelled, None)),
                    }
                }
                None => None,
            };
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        let _ = evt_tx.send(AppEvent::ImpersonationChunk {
                            generation_id: id,
                            text: t,
                        });
                    }
                    // A background turn: the retry is worth a log line (a flaky provider is
                    // otherwise invisible here) but has nothing to show — these turns have no
                    // chip of their own.
                    ChatChunk::Retry {
                        attempt,
                        max,
                        delay,
                    } => {
                        tracing::info!(attempt, max, ?delay, "retrying a an impersonation turn");
                    }
                    ChatChunk::Error { message, .. } => {
                        tracing::warn!(error = %message, "engine error while impersonating");
                    }
                    // The exact size next to the estimate the reservation was
                    // priced from, under impersonation's own kind
                    // (docs/research/title-impersonation-usage.md §3.2); a
                    // separate impersonation server has no budget to record into.
                    ChatChunk::Usage(u) => {
                        if let Some(budget) = sessions.as_deref() {
                            budget.record_usage(
                                crate::shared::session_budget::Shape::Impersonation,
                                estimate,
                                u.prompt_tokens as u64,
                            );
                            prefill = u.prefill;
                        }
                    }
                    ChatChunk::Thoughts(_)
                    | ChatChunk::ThoughtsSignature(_)
                    | ChatChunk::ToolCall(_) => {}
                    ChatChunk::Finished(r) => {
                        reason = r;
                        break;
                    }
                }
            }
            Ok::<(FinishReason, Option<crate::shared::api::contract::Prefill>), anyhow::Error>((
                reason, prefill,
            ))
        };
        // Distinguish a user cancellation from a timeout: on cancellation (`Esc`) the stream inside
        // `run` catches `cancel.cancelled()` and itself returns `Finished(Cancelled)` — it
        // arrives here as `Ok(Ok(Cancelled))` and leads to discarding the text
        // (the user changed their mind). A timeout is `Err(_)`: we abort the server task
        // (`cancel.cancel()`), but **keep** the accumulated text, returning it as
        // `Length` (the model was writing a valid reply, just slowly). The old
        // unconditional `if cancel.is_cancelled() { Cancelled }` collapsed both cases into
        // discarding — which is why a timeout-truncated reply used to disappear.
        let (reason, prefill) = match tokio::time::timeout(IMPERSONATION_TIMEOUT, run).await {
            Ok(Ok(landed)) => landed,
            Ok(Err(err)) => {
                let _ = evt_tx.send(AppEvent::Error(
                    loc.tf("ui.err.impersonation_failed", &[("err", &err.to_string())]),
                ));
                (FinishReason::Error, None)
            }
            Err(_) => {
                cancel.cancel();
                (FinishReason::Length, None)
            }
        };
        let _ = done_tx.send(ImpDone {
            id,
            reason,
            prefill,
        });
    });
}
