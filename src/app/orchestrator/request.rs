//! Maps domain messages ([`Message`]) into the engine request format ([`ChatRequest`]).

use crate::entities::attachment::{AttachMode, Attachment, format_bytes};
use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ApiToolCall, ChatRequest};
use crate::shared::config::AttachmentSettings;
use crate::shared::i18n::Locale;

/// Converts a domain message into a message for the model. System messages
/// go through [`ChatRequest::system`] (here — `None`). Assistant messages with
/// tool calls and tool results are rebuilt for a correct history
/// (strict order validation by the server, contract §3.2).
fn message_to_api(message: &Message) -> Option<ApiMessage> {
    match message.role {
        MessageRole::System => None,
        MessageRole::User => Some(ApiMessage::user(&message.text)),
        MessageRole::Assistant => {
            if message.tool_calls.is_empty() {
                Some(ApiMessage::assistant(&message.text))
            } else {
                let calls = message.tool_calls.iter().map(record_to_api).collect();
                Some(ApiMessage::assistant_tool_calls(&message.text, calls))
            }
        }
        MessageRole::Tool => message
            .tool_call_id
            .as_ref()
            .map(|id| ApiMessage::tool(id, &message.text)),
    }
}

/// Domain tool-call record → the request-body form (arguments as a JSON string).
fn record_to_api(rec: &ToolCallRecord) -> ApiToolCall {
    ApiToolCall {
        id: rec.id.clone(),
        name: rec.name.clone(),
        arguments: rec.arguments.to_string(),
        // A thought signature (Gemini 3) is preserved on the record — resent on
        // history replay, otherwise Gemini 3 returns a 400 on a historical functionCall.
        thought_signature: rec.thought_signature.clone(),
    }
}

/// Timestamp of the chat's last user message (for `ToolContext`).
pub(super) fn last_user_message_at(chat: &Chat) -> Option<chrono::DateTime<chrono::Utc>> {
    chat.messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.timestamp)
}

/// Builds a generation request from the chat's current state with a set of tool
/// schemas. Attached files (`/file attach`) are injected into the system prompt
/// — see [`inject_attachments`].
pub(super) fn build_request(
    chat: &Chat,
    sampling: SamplingConfig,
    tools: Vec<crate::shared::api::ToolSchema>,
    attachments: &AttachmentSettings,
    loc: &Locale,
) -> ChatRequest {
    let system = if chat.system_message.trim().is_empty() {
        None
    } else {
        Some(chat.system_message.clone())
    };
    ChatRequest {
        system: inject_attachments(system, &chat.attachments, attachments, loc),
        messages: chat.messages.iter().filter_map(message_to_api).collect(),
        sampling,
        tools,
    }
}

/// Appends the pinned block of attached files to the system prompt
/// (docs/file-attachments.md §4.3). A pure function — testable with no engine.
///
/// Placement: **`system`**, so the block sits at the front of the prefix and the
/// whole conversation after it stays prefix-cached (spec §6.6); it is
/// re-prefilled only when the attachment set changes — the same trade-off
/// already accepted for the self-model injection. The header is in the
/// **profile** language (axis A): the model reads it.
///
/// An inline attachment contributes its full text; a by-reference one
/// contributes metadata plus the head excerpt. Returns `system` unchanged when
/// nothing is attached.
pub(super) fn inject_attachments(
    system: Option<String>,
    attachments: &[Attachment],
    cfg: &AttachmentSettings,
    loc: &Locale,
) -> Option<String> {
    if attachments.is_empty() {
        return system;
    }
    let mut block = String::from(loc.t("prompt.attachments.header"));
    for a in attachments {
        // The fence must not occur inside the content, otherwise a file could
        // "close" its own section and the rest would read as instructions.
        let width = fence_width(&a.text);
        let open = "<".repeat(width);
        let close = ">".repeat(width);
        let size = format_bytes(a.bytes);
        let (head, body) = match a.mode {
            AttachMode::Inline => (
                loc.tf(
                    "prompt.attachments.begin_full",
                    &[
                        ("open", &open),
                        ("name", &a.name),
                        ("size", &size),
                        ("close", &close),
                    ],
                ),
                a.text.as_str(),
            ),
            AttachMode::ByReference => (
                loc.tf(
                    "prompt.attachments.begin_excerpt",
                    &[
                        ("open", &open),
                        ("name", &a.name),
                        ("size", &size),
                        ("tokens", &a.est_tokens.to_string()),
                        ("close", &close),
                    ],
                ),
                a.excerpt(cfg.excerpt_tokens),
            ),
        };
        block.push_str("\n\n");
        block.push_str(&head);
        block.push('\n');
        block.push_str(body);
        block.push('\n');
        block.push_str(&loc.tf(
            "prompt.attachments.end",
            &[("open", &open), ("name", &a.name), ("close", &close)],
        ));
    }
    Some(match system {
        Some(s) if !s.is_empty() => format!("{s}\n\n{block}"),
        _ => block,
    })
}

/// How many angle brackets the section fence needs so that it doesn't occur in
/// the file's own content (a file quoting `>>>` must not be able to close its
/// own section). Starts at 3 and widens until the closing marker is absent.
fn fence_width(text: &str) -> usize {
    let mut width = 3;
    // A pathological file (a long run of '>') can't push this far: each step
    // requires the text to contain a strictly longer run.
    while text.contains(&">".repeat(width)) {
        width += 1;
    }
    width
}
