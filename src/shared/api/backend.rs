//! Контракт движка инференса (`EngineBackend`) и его типы. Скрывает транспорт
//! (HTTP к xinfer) за трейтом — тестируемость (mock/replay) и возможность
//! сменить транспорт. См. spec §6.1 и docs/xinfer-contract.md §8.

use std::pin::Pin;

use anyhow::Result;
use futures_util::Stream;
use tokio_util::sync::CancellationToken;

use crate::entities::sampling::SamplingConfig;

/// Роль сообщения в запросе к модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiRole {
    System,
    User,
    Assistant,
    Tool,
}

impl ApiRole {
    pub fn as_wire(self) -> &'static str {
        match self {
            ApiRole::System => "system",
            ApiRole::User => "user",
            ApiRole::Assistant => "assistant",
            ApiRole::Tool => "tool",
        }
    }
}

/// Сообщение диалога, передаваемое модели.
#[derive(Debug, Clone)]
pub struct ApiMessage {
    pub role: ApiRole,
    pub content: String,
    /// Идентификатор tool-call (для роли `Tool`).
    pub tool_call_id: Option<String>,
}

impl ApiMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::User,
            content: content.into(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// Запрос одного хода генерации.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Системное сообщение (подставляется первым). См. spec §6.2.
    pub system: Option<String>,
    /// Диалог (user/assistant/tool).
    pub messages: Vec<ApiMessage>,
    pub sampling: SamplingConfig,
    // tools / tool_choice добавляются на M5.
}

/// Причина завершения генерации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    Cancelled,
    Error,
}

impl FinishReason {
    /// Маппинг строкового `finish_reason` из ответа сервера.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "tool_calls" => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        }
    }
}

/// Инкрементальный фрагмент ответа модели.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatChunk {
    /// Дельта основного текста.
    Text(String),
    /// Дельта «мыслей» (reasoning).
    Thoughts(String),
    // ToolCallDelta(...) — на M5.
    /// Завершение генерации.
    Finished(FinishReason),
}

/// Поток фрагментов ответа.
pub type ChatStream = Pin<Box<dyn Stream<Item = ChatChunk> + Send>>;

/// Движок инференса. Реализации: HTTP-клиент к xinfer ([`super::client::XinferClient`])
/// и mock для тестов.
#[async_trait::async_trait]
pub trait EngineBackend: Send + Sync {
    /// Стриминговый одноходовый запрос. Отмена — через `cancel`.
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream>;

    /// Эмбеддинги (RAG). На M1 может быть не реализован.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(FinishReason::from_wire("stop"), FinishReason::Stop);
        assert_eq!(FinishReason::from_wire("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_wire("tool_calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_wire("weird"), FinishReason::Stop);
    }

    #[test]
    fn role_wire_strings() {
        assert_eq!(ApiRole::System.as_wire(), "system");
        assert_eq!(ApiRole::Tool.as_wire(), "tool");
    }
}
