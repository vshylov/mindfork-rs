//! Auto-title for a chat (spec §11.2): a background task asks the model to come up with
//! a short title from the conversation; the result is applied to the chat in the loop.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend};
use crate::shared::config::AutoTitleMode;

use super::Orchestrator;

/// The reply token ceiling for auto-title generation. We try to turn off "thoughts"
/// (`reasoning_budget=0` + `chat_template_kwargs.enable_thinking=false`), but
/// some models (thinking "baked into" the GGUF, e.g. Gemma `peg-gemma4`) ignore
/// this and still "reason" for hundreds of tokens before the reply — hence a
/// generous budget, so the model has time to finish "thinking" and produce the title.
const TITLE_MAX_TOKENS: usize = 2048;

/// The time limit for generating a chat's auto-title (with margin for "thinking" models).
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Who asked for the title — decides error visibility and the apply-time guard.
///
/// A **requested** run (the chat-list action) reports its failures to the list
/// overlay and applies last-write-wins: the user asked for this title moments
/// ago. An **automatic** run (the trigger of spec §11.2) is a background
/// nicety: failures go to the log (the spec §6.8 rule for background turns),
/// and a result loses to a manual rename made while the task ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TitleOrigin {
    Requested,
    Auto,
}

/// The result of the chat auto-title background task (an internal channel).
pub(super) struct TitleResult {
    pub(super) chat_id: Uuid,
    /// The model's raw reply text (or an error message to show in the UI).
    pub(super) text: Result<String, String>,
    pub(super) origin: TitleOrigin,
}

impl Orchestrator {
    /// The chat-list auto-title action (spec §11.2): the user asked, so
    /// failures are shown in the list overlay.
    pub(super) fn handle_auto_rename(&mut self, id: Uuid) {
        self.start_title_task(id, TitleOrigin::Requested);
    }

    /// The automatic titling trigger (spec §11.2): fires when the conversation
    /// reaches the configured point (`interface.auto_title`) — the call site
    /// says which point it stands at — and never for a chat the user renamed.
    /// Quiet by construction: every skip and failure is a log line, not a
    /// popup, because nobody asked for this run.
    pub(super) fn maybe_auto_title(&mut self, id: Uuid, point: AutoTitleMode) {
        if self.config.interface.auto_title != point {
            return;
        }
        if self
            .chats
            .iter()
            .find(|c| c.id == id)
            .is_none_or(|c| c.renamed_manually)
        {
            return;
        }
        self.start_title_task(id, TitleOrigin::Auto);
    }

    /// The automatic titling of a sub-agent transcript that has just landed
    /// (spec §9.3.2, docs/research/subagent-chats.md §3.10). A transcript's
    /// user message and reply arrive together, so **both** trigger points of
    /// `interface.auto_title` fire here — only `Off` fires nothing — and the
    /// hand-named one (`renamed_manually`, which a migrated or re-opened run
    /// can carry) is left alone, as for a chat.
    pub(super) fn maybe_auto_title_run(&mut self, id: Uuid) {
        if self.config.interface.auto_title == AutoTitleMode::Off {
            return;
        }
        match self.view(id) {
            Some(super::ChatView::Child { run, .. }) if !run.renamed_manually => {}
            _ => return,
        }
        self.start_title_task(id, TitleOrigin::Auto);
    }

    /// Reports a titling failure where its origin belongs: the chat-list
    /// overlay for a requested run, the log for an automatic one.
    fn report_title_error(&self, origin: TitleOrigin, msg: String) {
        match origin {
            TitleOrigin::Requested => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
            }
            TitleOrigin::Auto => tracing::warn!(error = %msg, "automatic chat titling skipped"),
        }
    }

    /// A chat's auto-title (spec §11.2): the model reads the conversation (or its start
    /// and end, if it's long) and comes up with a short title. The request runs as a
    /// background task; the result arrives at [`Orchestrator::handle_title_result`].
    /// The chat server must be ready (`Ready`) — otherwise a clear error.
    fn start_title_task(&mut self, id: Uuid, origin: TitleOrigin) {
        // A chat, or a sub-agent transcript — the digest is the transcript's
        // own exchange, the language its parent's profile's.
        let (profile_id, messages) = match self.view(id) {
            Some(super::ChatView::Top(chat)) => (chat.profile_id, chat.messages.clone()),
            Some(super::ChatView::Child { parent, run }) => {
                (parent.profile_id, run.messages.clone())
            }
            None => return,
        };
        // The agent-scaffold language — from the chat's profile (axis A): the digest and the
        // system message for auto-title are localized with it (the title is still requested "in
        // the conversation's language", see `prompt.title.system`). Errors — for the human,
        // in the interface language (axis B, `self.ui_locale()`).
        let loc = self.profile_locale(profile_id);
        let Some(digest) = crate::features::rename_chat::build_conversation_digest(&messages, loc)
        else {
            self.report_title_error(origin, self.ui_locale().t("ui.err.title_not_enough").into());
            return;
        };
        let backend = match self.engines.backend_if_ready(self.ui_locale()) {
            Ok(backend) => backend,
            Err(msg) => {
                // A requested title is a chat-list operation: the server-readiness
                // error goes into the list overlay, not the chat feed (where a
                // full-screen overlay would hide it). An automatic one logs.
                self.report_title_error(origin, msg);
                return;
            }
        };
        // A fresh, compact sampling config (not inheriting the chat's override): a short reply,
        // a moderate temperature, reasoning disabled (a title doesn't need "thoughts" and
        // they just eat the token budget), no tools. What actually matters — `reasoning_
        // budget=0`: for models with thinking "baked into" the template (Gemma `peg-gemma4`,
        // Qwen) only this field actually suppresses "thoughts"; the server ignores the
        // `thinking`/`reasoning_effort` fields for such templates (otherwise the model would spend its
        // whole budget on "thoughts" and the reply text would come back empty).
        let sampling = SamplingConfig {
            max_tokens: Some(TITLE_MAX_TOKENS),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(crate::features::rename_chat::title_system_message(loc)),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: Vec::new(),
        };
        spawn_title(
            backend,
            request,
            id,
            origin,
            self.ui_locale(),
            self.title_tx.clone(),
        );
    }

    /// Applies the result of background auto-title generation: cleans up/normalizes
    /// the title and renames the chat (or reports the failure per its origin).
    pub(super) fn handle_title_result(&mut self, res: TitleResult) {
        let raw = match res.text {
            Ok(raw) => raw,
            Err(msg) => {
                self.report_title_error(res.origin, msg);
                return;
            }
        };
        let Some(title) = crate::features::rename_chat::clean_generated_title(&raw) else {
            self.report_title_error(res.origin, self.ui_locale().t("ui.err.title_empty").into());
            return;
        };
        if !self.apply_title(res.chat_id, res.origin, &title) {
            return;
        }
        self.emit_chat_list();
        let _ = self.evt_tx.send(AppEvent::ChatRenamed {
            id: res.chat_id,
            title,
        });
    }

    /// Writes a generated title onto the chat — or onto the sub-agent
    /// transcript, under the same rule — and says whether it landed. `false`
    /// when the conversation is gone, or when an **automatic** result lost to
    /// a rename the user made while the task ran: their choice wins
    /// (docs/history/auto-chat-title.md D1). A requested result keeps
    /// last-write-wins — the user asked for this title moments ago.
    fn apply_title(&mut self, chat_id: Uuid, origin: TitleOrigin, title: &str) -> bool {
        if let Some(chat) = self.chat_mut(chat_id) {
            if origin == TitleOrigin::Auto && chat.renamed_manually {
                tracing::debug!(chat = %chat_id,
                    "automatic title dropped: the chat was renamed manually meanwhile");
                return false;
            }
            chat.title = title.to_string();
            self.mark_dirty(chat_id);
            return true;
        }
        let mut dropped = false;
        let found = self.with_child_mut(chat_id, |run| {
            if origin == TitleOrigin::Auto && run.renamed_manually {
                dropped = true;
            } else {
                run.title = title.to_string();
            }
        });
        found && !dropped
    }
}

/// Starts the chat auto-title background task: one independent request to the model
/// (with no history/tools), collecting the text, sending the result into `title_tx`.
/// The time limit — [`TITLE_TIMEOUT`].
fn spawn_title(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    chat_id: Uuid,
    origin: TitleOrigin,
    loc: &'static crate::shared::i18n::Locale,
    title_tx: UnboundedSender<TitleResult>,
) {
    tokio::spawn(async move {
        let cancel = CancellationToken::new();
        let collect = async {
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            let mut text = String::new();
            let mut thoughts = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    // Accumulate "thoughts" as a fallback source: if the model never
                    // "finished thinking" (produced only reasoning), we'll pull the title
                    // out of the last substantive line of the reasoning.
                    ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                    ChatChunk::Finished(_) => break,
                    // A background turn: the streak counter reports the failure to the
                    // user (three in a row), and the log is where the reason belongs.
                    // A background turn: the retry is worth a log line (a flaky provider is
                    // otherwise invisible here) but has nothing to show — these turns have no
                    // chip of their own.
                    ChatChunk::Retry {
                        attempt,
                        max,
                        delay,
                    } => {
                        tracing::info!(attempt, max, ?delay, "retrying a a title turn");
                    }
                    ChatChunk::Error { message, .. } => {
                        tracing::warn!(error = %message, "engine error while generating a title");
                    }
                    ChatChunk::ThoughtsSignature(_)
                    | ChatChunk::ToolCall(_)
                    | ChatChunk::Usage(_) => {}
                }
            }
            Ok::<(String, String), anyhow::Error>((text, thoughts))
        };
        let text = match tokio::time::timeout(TITLE_TIMEOUT, collect).await {
            Ok(Ok((text, thoughts))) => Ok(salvage_title_source(text, thoughts)),
            Ok(Err(err)) => Err(loc.tf("ui.err.title_gen_failed", &[("err", &err.to_string())])),
            Err(_) => {
                cancel.cancel();
                Err(loc.t("ui.err.title_timeout").to_string())
            }
        };
        let _ = title_tx.send(TitleResult {
            chat_id,
            text,
            origin,
        });
    });
}

/// Picks the raw title source: the model's main reply, and if it's empty
/// (the model didn't "finish thinking" within the budget) — the last substantive
/// line of the reasoning. `clean_generated_title` does the final normalization.
pub(super) fn salvage_title_source(text: String, thoughts: String) -> String {
    if !text.trim().is_empty() {
        return text;
    }
    thoughts
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}
