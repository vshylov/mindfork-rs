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
    /// Системная роль. Системное сообщение передаётся через [`ChatRequest::system`],
    /// поэтому как роль сообщения не конструируется — оставлена для полноты enum.
    #[allow(dead_code)]
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

/// Вызов инструмента ассистентом (в истории assistant-сообщения). См. spec §9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToolCall {
    pub id: String,
    pub name: String,
    /// Аргументы как JSON-строка (так их отдаёт/принимает сервер).
    pub arguments: String,
}

/// Сообщение диалога, передаваемое модели.
#[derive(Debug, Clone)]
pub struct ApiMessage {
    pub role: ApiRole,
    pub content: String,
    /// Идентификатор tool-call (для роли `Tool`).
    pub tool_call_id: Option<String>,
    /// Вызовы инструментов (для роли `Assistant`, инициировавшей tool-call).
    pub tool_calls: Vec<ApiToolCall>,
}

impl ApiMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    /// Assistant-ход с вызовами инструментов (контент может быть пустым).
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ApiToolCall>) -> Self {
        Self {
            role: ApiRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ApiRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
        }
    }
}

/// OpenAI-схема инструмента, передаваемая серверу (он возвращает валидные
/// `tool_calls`). Реализации инструментов (`features/tools`) её формируют.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema объекта параметров.
    pub parameters: serde_json::Value,
}

/// Запрос одного хода генерации.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Системное сообщение (подставляется первым). См. spec §6.2.
    pub system: Option<String>,
    /// Диалог (user/assistant/tool).
    pub messages: Vec<ApiMessage>,
    pub sampling: SamplingConfig,
    /// Схемы доступных инструментов (пусто — без tool-calling). `tool_choice=auto`.
    pub tools: Vec<ToolSchema>,
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

/// Дельта вызова инструмента из стрима (накапливается по `index`). См. spec §6.3.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolCallDelta {
    pub index: usize,
    /// Идентификатор вызова (обычно в первой дельте).
    pub id: Option<String>,
    /// Имя функции (обычно в первой дельте).
    pub name: Option<String>,
    /// Дельта строки аргументов (склеивается).
    pub arguments: String,
}

/// Инкрементальный фрагмент ответа модели.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatChunk {
    /// Дельта основного текста.
    Text(String),
    /// Дельта «мыслей» (reasoning).
    Thoughts(String),
    /// Дельта вызова инструмента (сервер парсит `<tool_call>` сам).
    ToolCall(ToolCallDelta),
    /// Завершение генерации.
    Finished(FinishReason),
}

/// Накопитель вызовов инструментов из потоковых дельт (по `index`).
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<ApiToolCall>,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, delta: ToolCallDelta) {
        while self.calls.len() <= delta.index {
            self.calls.push(ApiToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }
        let call = &mut self.calls[delta.index];
        if let Some(id) = delta.id
            && !id.is_empty()
        {
            call.id = id;
        }
        if let Some(name) = delta.name
            && !name.is_empty()
        {
            call.name = name;
        }
        call.arguments.push_str(&delta.arguments);
    }

    /// Возвращает собранные вызовы (отбрасывая безымянные «дыры»).
    pub fn finish(self) -> Vec<ApiToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .collect()
    }
}

/// Поток фрагментов ответа.
pub type ChatStream = Pin<Box<dyn Stream<Item = ChatChunk> + Send>>;

/// Движок инференса (chat). Реализации: HTTP-клиент к xinfer
/// ([`super::client::OpenAiClient`]) и mock для тестов.
#[async_trait::async_trait]
pub trait EngineBackend: Send + Sync {
    /// Стриминговый одноходовый запрос. Отмена — через `cancel`.
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream>;
}

/// Источник эмбеддингов для RAG. По решению M5 — **выделенный** embedding-сервер
/// (отдельный процесс/порт), поэтому он отделён от [`EngineBackend`] (chat).
/// См. docs/decisions/0002-embeddings-dedicated-server.md.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Возвращает эмбеддинги для каждого входного текста (в том же порядке).
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;
}

/// Эмбеддер-заглушка: возвращает ошибку (embedding-сервер не настроен). RAG в
/// этом режиме недоступен, но инструмент не «падает» — ошибка уходит модели.
pub struct UnavailableEmbedder;

#[async_trait::async_trait]
impl Embedder for UnavailableEmbedder {
    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        anyhow::bail!("embedding-сервер не настроен — RAG недоступен")
    }
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

    #[test]
    fn accumulator_assembles_split_tool_call() {
        let mut acc = ToolCallAccumulator::default();
        acc.push(ToolCallDelta {
            index: 0,
            id: Some("call_1".into()),
            name: Some("note_save".into()),
            arguments: "{\"con".into(),
        });
        acc.push(ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: "tent\":\"hi\"}".into(),
        });
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "note_save");
        assert_eq!(calls[0].arguments, "{\"content\":\"hi\"}");
    }

    #[test]
    fn accumulator_handles_two_parallel_calls() {
        let mut acc = ToolCallAccumulator::default();
        acc.push(ToolCallDelta {
            index: 0,
            id: Some("a".into()),
            name: Some("f".into()),
            arguments: "{}".into(),
        });
        acc.push(ToolCallDelta {
            index: 1,
            id: Some("b".into()),
            name: Some("g".into()),
            arguments: "{}".into(),
        });
        assert_eq!(acc.finish().len(), 2);
    }
}
