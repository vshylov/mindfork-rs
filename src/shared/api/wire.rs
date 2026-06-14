//! Serde-типы HTTP-протокола xinfer (`/v1/chat/completions`, `/v1/embeddings`)
//! и сборка тела запроса. Точно соответствует docs/xinfer-contract.md §3, §6.
//!
//! Инвариант: поле `stop` НЕ отправляется (анти-самообрыв на тексте EOS —
//! см. spec §7, docs/xinfer-contract.md §5).

use serde::{Deserialize, Serialize};

use super::backend::ChatRequest;

// ---------- запрос чата ----------

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub messages: Vec<WireMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct WireMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Строит тело запроса чата из доменного [`ChatRequest`].
pub fn build_chat_request(req: &ChatRequest, stream: bool) -> ChatCompletionRequest {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if let Some(system) = &req.system {
        messages.push(WireMessage {
            role: "system",
            content: Some(system.clone()),
            tool_call_id: None,
        });
    }
    for m in &req.messages {
        messages.push(WireMessage {
            role: m.role.as_wire(),
            content: Some(m.content.clone()),
            tool_call_id: m.tool_call_id.clone(),
        });
    }

    let s = &req.sampling;
    ChatCompletionRequest {
        messages,
        stream,
        temperature: s.temperature,
        max_tokens: s.max_tokens,
        top_k: s.top_k,
        top_p: s.top_p,
        frequency_penalty: s.frequency_penalty,
        presence_penalty: s.presence_penalty,
        thinking: s.thinking,
        reasoning_effort: s.reasoning_effort.map(|r| r.as_wire()),
    }
}

// ---------- стриминговый ответ ----------

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub choices: Vec<ChatChoiceChunk>,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoiceChunk {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    // tool_calls — на M5.
}

// ---------- эмбеддинги ----------

#[derive(Debug, Serialize)]
pub struct EmbeddingRequest {
    pub input: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingResponse {
    #[serde(default)]
    pub data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingData {
    pub embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::backend::ApiMessage;

    #[test]
    fn omits_stop_and_none_fields() {
        let req = ChatRequest {
            system: Some("sys".into()),
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
        };
        let body = build_chat_request(&req, true);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("stop").is_none(), "stop must never be sent");
        assert!(json.get("temperature").is_none());
        assert_eq!(json["stream"], true);
        // system должно идти первым сообщением
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "hi");
    }

    #[test]
    fn maps_supported_sampling_fields() {
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                temperature: Some(0.8),
                top_k: Some(40),
                top_p: Some(0.95),
                frequency_penalty: Some(0.1),
                presence_penalty: Some(0.2),
                max_tokens: Some(256),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::High),
            },
        };
        let json = serde_json::to_value(build_chat_request(&req, false)).unwrap();
        // f32→f64 расширение делает точное сравнение ненадёжным — сравниваем приближённо.
        let approx = |v: &serde_json::Value, want: f64| (v.as_f64().unwrap() - want).abs() < 1e-6;
        assert!(approx(&json["temperature"], 0.8));
        assert_eq!(json["top_k"], 40);
        assert!(approx(&json["top_p"], 0.95));
        assert!(approx(&json["frequency_penalty"], 0.1));
        assert!(approx(&json["presence_penalty"], 0.2));
        assert_eq!(json["max_tokens"], 256);
        assert_eq!(json["thinking"], true);
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["stream"], false);
    }

    #[test]
    fn parses_streaming_chunk() {
        let raw = r#"{"choices":[{"delta":{"content":"hello","reasoning_content":"hmm"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let c = &chunk.choices[0];
        assert_eq!(c.delta.content.as_deref(), Some("hello"));
        assert_eq!(c.delta.reasoning_content.as_deref(), Some("hmm"));
        assert!(c.finish_reason.is_none());
    }

    #[test]
    fn parses_finish_chunk() {
        let raw = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
    }
}
