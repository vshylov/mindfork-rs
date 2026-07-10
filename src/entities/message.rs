//! Сообщение чата и связанные типы. См. spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::sampling::SamplingConfig;
use crate::shared::config::ServerMode;

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
    /// Подпись мысли (Gemini 3 `thoughtSignature`), привязанная к этому вызову.
    /// Персистится ради реплея истории: Gemini 3 требует подпись на исторических
    /// `functionCall` (иначе `400`). Прочие провайдеры — `None` (у Anthropic/OpenAI
    /// подпись одна на ход и не персистится). См. docs/research/gemini-native-client.md §2.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Снимок параметров генерации, фактически применённых к сообщению.
///
/// `sampling` содержит **только** поля, доступные в режиме движка на момент
/// генерации (см. [`SamplingConfig::retain_supported`]) — прочие поля движок бы
/// не принял, поэтому в снимок «что применилось» они не попадают. `mode` — режим
/// движка (managed/external/openai/gemini/claude), `model` — имя модели.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub sampling: SamplingConfig,
    /// Режим движка, которым сгенерировано сообщение. `#[serde(default)]` → старые
    /// сообщения без поля читаются как `Managed`.
    #[serde(default)]
    pub mode: ServerMode,
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
    /// Сообщение ассистента начинает **новый пузырь** в ленте (не склеивается с
    /// предыдущим блоком ассистента). Ставится управляющим инструментом
    /// «написать ещё сообщение» (`send_followup_message`) для второго и далее
    /// сообщений. По умолчанию `false` — обычная склейка раундов agentic-loop.
    /// См. spec §9.3.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_bubble: bool,
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
            new_bubble: false,
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

    #[test]
    fn tool_call_thought_signature_defaults_and_roundtrips() {
        // Старая запись без поля читается как None (без миграции).
        let raw = r#"{"id":"c1","name":"calc","arguments":{}}"#;
        let rec: ToolCallRecord = serde_json::from_str(raw).unwrap();
        assert!(rec.thought_signature.is_none());
        // С подписью — round-trip; пустое поле не сериализуется.
        let with_sig = ToolCallRecord {
            id: "c1".into(),
            name: "calc".into(),
            arguments: serde_json::json!({}),
            result: None,
            thought_signature: Some("SIG".into()),
        };
        let json = serde_json::to_string(&with_sig).unwrap();
        assert!(json.contains("thought_signature"));
        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.thought_signature.as_deref(), Some("SIG"));
        // Без подписи — ключ отсутствует (skip_serializing_if).
        let no_sig = ToolCallRecord {
            id: "c1".into(),
            name: "calc".into(),
            arguments: serde_json::json!({}),
            result: None,
            thought_signature: None,
        };
        assert!(
            !serde_json::to_string(&no_sig)
                .unwrap()
                .contains("thought_signature")
        );
    }
}
