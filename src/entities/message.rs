//! Сообщение чата и связанные типы. См. spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::sampling::SamplingConfig;

/// Роль сообщения в чате.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Запись о вызове инструмента (для сворачиваемых tool-блоков в UI). См. spec §5.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Снимок параметров генерации, фактически применённых к сообщению.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub sampling: SamplingConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Сообщение чата.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub role: MessageRole,
    pub text: String,
    /// Блок рассуждений (CoT).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thoughts: Option<String>,
    /// Вызовы инструментов в этом сообщении.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
    pub timestamp: DateTime<Utc>,
    #[serde(default = "default_true")]
    pub is_markdown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
    /// Для роли `Tool`: идентификатор tool-call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Для роли `Tool`: имя инструмента.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Message {
    /// Создаёт сообщение с новым `id` и текущим временем.
    pub fn new(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role,
            text: text.into(),
            thoughts: None,
            tool_calls: Vec::new(),
            timestamp: Utc::now(),
            is_markdown: true,
            metadata: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(MessageRole::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(MessageRole::Assistant, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_minimal_message() {
        let m = Message::user("привет");
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn role_serializes_lowercase() {
        let json = serde_json::to_string(&MessageRole::Assistant).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn defaults_applied_on_minimal_json() {
        // Без is_markdown/tool_calls — должны примениться дефолты.
        let raw = format!(
            r#"{{"id":"{}","role":"user","text":"hi","timestamp":"2026-06-14T00:00:00Z"}}"#,
            Uuid::nil()
        );
        let m: Message = serde_json::from_str(&raw).unwrap();
        assert!(m.is_markdown);
        assert!(m.tool_calls.is_empty());
        assert!(m.thoughts.is_none());
    }
}
