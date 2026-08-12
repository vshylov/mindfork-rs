//! A chat message and related types. See spec §5.1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::message_image::MessageImage;
use crate::entities::sampling::SamplingConfig;
use crate::shared::config::ServerMode;

/// The message's role in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool-call record (for collapsible tool blocks in the UI). See spec §5.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// A thought signature (Gemini 3 `thoughtSignature`) tied to this call.
    /// Persisted for history replay: Gemini 3 requires the signature on historical
    /// `functionCall`s (otherwise `400`). Other providers — `None` (Anthropic/OpenAI
    /// have one signature per turn, not persisted). See docs/research/gemini-native-client.md §2.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    /// How many images this call returned (spec §9.10). A **count**, deliberately: the
    /// pixels live on the `Tool` message the call produced, and the feed only needs to
    /// know a chip is due. Persisting the count is what makes a reloaded conversation
    /// show the same block as a live one, without the feed having to walk to another
    /// message to find out. Additive — old records read as `0`.
    #[serde(default, skip_serializing_if = "crate::entities::message::is_zero")]
    pub images: usize,
}

/// `skip_serializing_if` for a count that is almost always zero.
pub(crate) fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A snapshot of the generation parameters actually applied to the message.
///
/// `sampling` holds **only** the fields available in the engine mode at generation
/// time (see [`SamplingConfig::retain_supported`]) — other fields the engine
/// wouldn't have accepted, so they don't make it into the "what was applied"
/// snapshot. `mode` — the engine mode (managed/external/openai/gemini/claude),
/// `model` — the model name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub sampling: SamplingConfig,
    /// The engine mode the message was generated with. `#[serde(default)]` → old
    /// messages without the field read as `Managed`.
    #[serde(default)]
    pub mode: ServerMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub role: MessageRole,
    pub text: String,
    /// The reasoning (CoT) block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thoughts: Option<String>,
    /// Tool calls in this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
    pub timestamp: DateTime<Utc>,
    #[serde(default = "default_true")]
    pub is_markdown: bool,
    /// The assistant message starts a **new bubble** in the feed (not merged with
    /// the previous assistant block). Set by the control tool "write another
    /// message" (`send_followup_message`) for the second and later messages.
    /// `false` by default — the normal agentic-loop round merging. See spec §9.3.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_bubble: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
    /// For the `Tool` role: the tool-call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For the `Tool` role: the tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Images attached to this message (spec §9.10). Message-scoped, unlike file
    /// attachments: an image belongs to the turn that introduced it and is replayed as
    /// ordinary history afterwards. Additive (ADR 0006 F12) — old chats read unchanged
    /// and a chat without images serializes exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MessageImage>,
}

fn default_true() -> bool {
    true
}

impl Message {
    /// Creates a message with a new `id` and the current time.
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
            images: Vec::new(),
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(MessageRole::User, text)
    }

    /// A user message carrying the images staged with `/image attach` (spec §9.10).
    /// Builder-style, because the images are known only at send time — staging lives in
    /// the orchestrator until the turn that consumes it.
    pub fn with_images(mut self, images: Vec<MessageImage>) -> Self {
        self.images = images;
        self
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
        // No is_markdown/tool_calls — the defaults should apply.
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
    fn images_are_additive_and_not_serialized_when_empty() {
        // A message without images must serialize exactly as it did before the field
        // existed — that is what keeps every stored chat readable by both binaries.
        let plain = Message::user("hi");
        assert!(!serde_json::to_string(&plain).unwrap().contains("images"));
        // And an old chat file, which has no such key, reads with no migration.
        let raw = format!(
            r#"{{"id":"{}","role":"user","text":"hi","timestamp":"2026-06-14T00:00:00Z"}}"#,
            Uuid::nil()
        );
        let m: Message = serde_json::from_str(&raw).unwrap();
        assert!(m.images.is_empty());

        // With an image the payload round-trips whole (it is the chat file that stores
        // it — fork F2 of docs/research/multimodal-images.md).
        let with_image = Message::user("look").with_images(vec![
            crate::entities::message_image::MessageImage::new(
                "a.png",
                "D:\\a.png",
                "image/png",
                40,
                30,
                "AAAA".into(),
            ),
        ]);
        let json = serde_json::to_string(&with_image).unwrap();
        assert!(json.contains("\"images\""));
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with_image);
    }

    #[test]
    fn tool_call_thought_signature_defaults_and_roundtrips() {
        // An old record with no field reads as None (no migration).
        let raw = r#"{"id":"c1","name":"calc","arguments":{}}"#;
        let rec: ToolCallRecord = serde_json::from_str(raw).unwrap();
        assert!(rec.thought_signature.is_none());
        // With a signature — round-trip; an empty field isn't serialized.
        let with_sig = ToolCallRecord {
            id: "c1".into(),
            name: "calc".into(),
            arguments: serde_json::json!({}),
            result: None,
            thought_signature: Some("SIG".into()),
            images: 0,
        };
        let json = serde_json::to_string(&with_sig).unwrap();
        assert!(json.contains("thought_signature"));
        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.thought_signature.as_deref(), Some("SIG"));
        // Without a signature — the key is absent (skip_serializing_if).
        let no_sig = ToolCallRecord {
            id: "c1".into(),
            name: "calc".into(),
            arguments: serde_json::json!({}),
            result: None,
            thought_signature: None,
            images: 0,
        };
        assert!(
            !serde_json::to_string(&no_sig)
                .unwrap()
                .contains("thought_signature")
        );
    }
}
