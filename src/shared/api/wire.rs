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
    /// Бюджет «мыслей» (llama.cpp): `0` выключает thinking. См. [`SamplingConfig`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_budget: Option<i64>,
    /// Доп. переменные для Jinja chat-template (llama.cpp `chat_template_kwargs`).
    /// Используем для `{"enable_thinking": false}` — разные шаблоны выключают
    /// «мысли» по-разному (built-in форматы читают `reasoning_budget`, многие
    /// Jinja-шаблоны — `enable_thinking`), поэтому шлём оба сигнала.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<serde_json::Value>,
    /// Схемы инструментов (отсутствуют, если tool-calling не используется).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct WireMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

/// OpenAI-обёртка схемы инструмента (`{type:"function", function:{...}}`).
#[derive(Debug, Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Debug, Serialize)]
pub struct WireFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Вызов инструмента в assistant-сообщении истории.
#[derive(Debug, Serialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunctionCall,
}

#[derive(Debug, Serialize)]
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Строит тело запроса чата из доменного [`ChatRequest`].
pub fn build_chat_request(req: &ChatRequest, stream: bool) -> ChatCompletionRequest {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if let Some(system) = &req.system {
        messages.push(WireMessage {
            role: "system",
            content: Some(system.clone()),
            tool_call_id: None,
            tool_calls: None,
        });
    }
    for m in &req.messages {
        let tool_calls = if m.tool_calls.is_empty() {
            None
        } else {
            Some(
                m.tool_calls
                    .iter()
                    .map(|tc| WireToolCall {
                        id: tc.id.clone(),
                        kind: "function",
                        function: WireFunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    })
                    .collect(),
            )
        };
        messages.push(WireMessage {
            role: m.role.as_wire(),
            // Для assistant с tool_calls контент может быть пустым.
            content: Some(m.content.clone()),
            tool_call_id: m.tool_call_id.clone(),
            tool_calls,
        });
    }

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| WireTool {
                    kind: "function",
                    function: WireFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    },
                })
                .collect(),
        )
    };
    let tool_choice = tools.as_ref().map(|_| "auto");

    let s = &req.sampling;
    // Просьбу выключить «мысли» (reasoning_budget=0) дублируем через
    // chat_template_kwargs.enable_thinking=false: built-in форматы llama.cpp читают
    // reasoning_budget, а Jinja-шаблоны моделей — enable_thinking; шлём оба.
    let chat_template_kwargs =
        (s.reasoning_budget == Some(0)).then(|| serde_json::json!({ "enable_thinking": false }));
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
        reasoning_budget: s.reasoning_budget,
        chat_template_kwargs,
        tools,
        tool_choice,
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
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaToolCall {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DeltaFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
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
            tools: vec![],
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
                reasoning_budget: Some(0),
            },
            tools: vec![],
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
        assert_eq!(json["reasoning_budget"], 0);
        // reasoning_budget=0 дублируется сигналом для Jinja-шаблонов.
        assert_eq!(json["chat_template_kwargs"]["enable_thinking"], false);
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

    #[test]
    fn parses_tool_call_delta() {
        let raw = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"note_save","arguments":"{\"x\":1}"}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, 0);
        assert_eq!(tc[0].id.as_deref(), Some("c1"));
        let f = tc[0].function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("note_save"));
        assert_eq!(f.arguments.as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn builds_tools_and_tool_choice() {
        use crate::shared::api::backend::ToolSchema;
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![ToolSchema {
                name: "note_save".into(),
                description: "Сохранить заметку".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
        };
        let json = serde_json::to_value(build_chat_request(&req, true)).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["function"]["name"], "note_save");
    }

    #[test]
    fn serializes_assistant_tool_calls_in_history() {
        use crate::shared::api::backend::ApiToolCall;
        let req = ChatRequest {
            system: None,
            messages: vec![
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        id: "c1".into(),
                        name: "f".into(),
                        arguments: "{}".into(),
                    }],
                ),
                ApiMessage::tool("c1", "result"),
            ],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, true)).unwrap();
        assert_eq!(json["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            json["messages"][0]["tool_calls"][0]["function"]["name"],
            "f"
        );
        assert_eq!(json["messages"][1]["role"], "tool");
        assert_eq!(json["messages"][1]["tool_call_id"], "c1");
        // Запрос без tools не должен содержать tool_choice.
        assert!(json.get("tool_choice").is_none());
    }
}
