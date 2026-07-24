//! Maps domain messages ([`Message`]) into the engine request format ([`ChatRequest`]).

use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ApiToolCall, ChatRequest};

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

/// Builds a generation request from the chat's current state with a set of tool schemas.
pub(super) fn build_request(
    chat: &Chat,
    sampling: SamplingConfig,
    tools: Vec<crate::shared::api::ToolSchema>,
) -> ChatRequest {
    let system = if chat.system_message.trim().is_empty() {
        None
    } else {
        Some(chat.system_message.clone())
    };
    ChatRequest {
        system,
        messages: chat.messages.iter().filter_map(message_to_api).collect(),
        sampling,
        tools,
    }
}
