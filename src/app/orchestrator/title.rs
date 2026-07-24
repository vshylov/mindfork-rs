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

use super::Orchestrator;

/// The reply token ceiling for auto-title generation. We try to turn off "thoughts"
/// (`reasoning_budget=0` + `chat_template_kwargs.enable_thinking=false`), but
/// some models (thinking "baked into" the GGUF, e.g. Gemma `peg-gemma4`) ignore
/// this and still "reason" for hundreds of tokens before the reply — hence a
/// generous budget, so the model has time to finish "thinking" and produce the title.
const TITLE_MAX_TOKENS: usize = 2048;

/// The time limit for generating a chat's auto-title (with margin for "thinking" models).
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The result of the chat auto-title background task (an internal channel).
pub(super) struct TitleResult {
    pub(super) chat_id: Uuid,
    /// The model's raw reply text (or an error message to show in the UI).
    pub(super) text: Result<String, String>,
}

impl Orchestrator {
    /// A chat's auto-title (spec §11.2): the model reads the conversation (or its start
    /// and end, if it's long) and comes up with a short title. The request runs as a
    /// background task; the result arrives at [`Orchestrator::handle_title_result`].
    /// The chat server must be ready (`Ready`) — otherwise a clear error.
    pub(super) fn handle_auto_rename(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        // The agent-scaffold language — from the chat's profile (axis A): the digest and the
        // system message for auto-title are localized with it (the title is still requested "in
        // the conversation's language", see `prompt.title.system`). Errors — for the human (axis B),
        // stay in Russian.
        let loc = self.profile_locale(chat.profile_id);
        let Some(digest) =
            crate::features::rename_chat::build_conversation_digest(&chat.messages, loc)
        else {
            let _ = self.evt_tx.send(AppEvent::ChatListError(
                self.ui_locale().t("ui.err.title_not_enough").into(),
            ));
            return;
        };
        let backend = match self.engines.backend_if_ready() {
            Ok(backend) => backend,
            Err(msg) => {
                // Auto-title is a chat-list operation: we show the server-readiness
                // error in the list overlay, not in the chat feed (where a full-screen overlay
                // would hide it).
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
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
            self.ui_locale(),
            self.title_tx.clone(),
        );
    }

    /// Applies the result of background auto-title generation: cleans up/normalizes
    /// the title and renames the chat (or shows an error).
    pub(super) fn handle_title_result(&mut self, res: TitleResult) {
        match res.text {
            Ok(raw) => {
                let Some(title) = crate::features::rename_chat::clean_generated_title(&raw) else {
                    let _ = self.evt_tx.send(AppEvent::ChatListError(
                        self.ui_locale().t("ui.err.title_empty").into(),
                    ));
                    return;
                };
                if let Some(chat) = self.chat_mut(res.chat_id) {
                    chat.title = title.clone();
                    self.mark_dirty(res.chat_id);
                    self.emit_chat_list();
                    let _ = self.evt_tx.send(AppEvent::ChatRenamed {
                        id: res.chat_id,
                        title,
                    });
                }
            }
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
            }
        }
    }
}

/// Starts the chat auto-title background task: one independent request to the model
/// (with no history/tools), collecting the text, sending the result into `title_tx`.
/// The time limit — [`TITLE_TIMEOUT`].
fn spawn_title(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    chat_id: Uuid,
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
        let _ = title_tx.send(TitleResult { chat_id, text });
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
