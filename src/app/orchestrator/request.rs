//! Маппинг доменных сообщений ([`Message`]) в формат запроса движка ([`ChatRequest`]).

use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ApiToolCall, ChatRequest};

/// Конвертирует доменное сообщение в сообщение для модели. Системные сообщения
/// передаются через [`ChatRequest::system`] (здесь — `None`). Assistant с
/// tool-вызовами и tool-результаты восстанавливаются для корректной истории
/// (строгая валидация порядка сервером, contract §3.2).
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

/// Доменная запись tool-вызова → форма для запроса (аргументы как JSON-строка).
fn record_to_api(rec: &ToolCallRecord) -> ApiToolCall {
    ApiToolCall {
        id: rec.id.clone(),
        name: rec.name.clone(),
        arguments: rec.arguments.to_string(),
        // Подпись мысли (Gemini 3) сохранена в записи — переотправляем на реплее
        // истории, иначе Gemini 3 вернёт 400 на исторический functionCall.
        thought_signature: rec.thought_signature.clone(),
    }
}

/// Время последнего user-сообщения чата (для `ToolContext`).
pub(super) fn last_user_message_at(chat: &Chat) -> Option<chrono::DateTime<chrono::Utc>> {
    chat.messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.timestamp)
}

/// Строит запрос генерации из текущего состояния чата с набором схем инструментов.
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
