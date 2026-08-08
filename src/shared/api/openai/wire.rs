//! Serde types for the xinfer HTTP protocol (`/v1/chat/completions`, `/v1/embeddings`)
//! and building the request body. Matches docs/xinfer-contract.md §3, §6 exactly.
//!
//! Invariant: the `stop` field is NOT sent (anti-self-cutoff on EOS text —
//! see spec §7, docs/xinfer-contract.md §5).

use serde::{Deserialize, Serialize};

use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::ChatRequest;

// The only consumer of this client is the local/external llama.cpp `llama-server`
// (managed/external). The clouds moved to their own protocols: OpenAI → Responses
// ([`ResponsesClient`](super::ResponsesClient)), Gemini → native
// [`GeminiClient`](crate::shared::api::gemini::GeminiClient), Claude → Anthropic
// Messages. So there's no longer a sampling dialect/filter — send everything set
// (llama.cpp ignores unknown fields). See ADR 0004.

// ---------- chat request ----------

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    /// The model name. Mandatory for the cloud; for `llama-server` it's ignored (it takes the
    /// loaded model), so it's sent only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<WireMessage>,
    pub stream: bool,
    /// Stream options: ask the server to send a final `usage` with the token counter
    /// (`include_usage`). Sent only while streaming (see [`StreamOptions`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynatemp_range: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynatemp_exponent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    // llama.cpp `llama-server` extensions (see [`SamplingConfig`]); a strict
    // third-party OpenAI server ignores or rejects them — so they're sent only
    // when the user sets them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_n_sigma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_target: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_decay: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_multiplier: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_base: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_allowed_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_penalty_last_n: Option<i64>,
    /// DRY breakers (an array of strings); only a non-empty list is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_sequence_breakers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_probability: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat_tau: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirostat_eta: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// The sampler order (an array of names); only a non-empty list is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samplers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
    /// The "thoughts" budget (llama.cpp): `0` disables thinking. See [`SamplingConfig`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_budget: Option<i64>,
    /// Extra variables for the Jinja chat template (llama.cpp `chat_template_kwargs`).
    /// Used for `{"enable_thinking": false}` — different templates disable
    /// "thoughts" differently (built-in formats read `reasoning_budget`, many
    /// Jinja templates — `enable_thinking`), so both signals are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<serde_json::Value>,
    /// Tool schemas (absent if tool-calling isn't used).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<&'static str>,
}

/// OpenAI stream options (`stream_options`). `include_usage=true` makes the server
/// send a final chunk with a `usage` block (the token counter) — otherwise it isn't
/// in the stream. llama.cpp `llama-server` supports this.
#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
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

/// The OpenAI wrapper for a tool schema (`{type:"function", function:{...}}`).
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

/// A tool call in a history assistant message.
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

/// Builds the chat request body from the domain [`ChatRequest`]. `model` is substituted into
/// the `model` field (for an external proxy if desired; for llama-server `None` works —
/// the server takes the loaded model). Everything set in sampling is sent (llama.cpp
/// ignores what it doesn't know). `omit_effort_none` — drop a `reasoning_effort` of
/// `"none"` rather than send it (xAI rejects the value; see
/// [`OpenAiClient::with_effort_none_omitted`](super::OpenAiClient::with_effort_none_omitted)).
pub fn build_chat_request(
    req: &ChatRequest,
    stream: bool,
    model: Option<&str>,
    omit_effort_none: bool,
) -> ChatCompletionRequest {
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
            // For an assistant with tool_calls, content can be empty.
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
    // The request to disable "thoughts" (reasoning_budget=0) is also sent via
    // chat_template_kwargs.enable_thinking=false: llama.cpp's built-in formats read
    // reasoning_budget, while models' Jinja templates read enable_thinking; send both.
    let chat_template_kwargs =
        (s.reasoning_budget == Some(0)).then(|| serde_json::json!({ "enable_thinking": false }));
    // List fields (DRY breakers, sampler order): an empty list isn't sent —
    // otherwise the server would interpret it as "no breakers"/"disable all samplers".
    let non_empty = |v: &Option<Vec<String>>| v.clone().filter(|x| !x.is_empty());
    ChatCompletionRequest {
        model: model.map(str::to_string),
        messages,
        stream,
        // The token counter is only needed during a streaming generation turn.
        stream_options: stream.then_some(StreamOptions {
            include_usage: true,
        }),
        temperature: s.temperature,
        dynatemp_range: s.dynatemp_range,
        dynatemp_exponent: s.dynatemp_exponent,
        max_tokens: s.max_tokens,
        top_k: s.top_k,
        top_p: s.top_p,
        min_p: s.min_p,
        top_n_sigma: s.top_n_sigma,
        typical_p: s.typical_p,
        adaptive_target: s.adaptive_target,
        adaptive_decay: s.adaptive_decay,
        frequency_penalty: s.frequency_penalty,
        presence_penalty: s.presence_penalty,
        repeat_penalty: s.repeat_penalty,
        repeat_last_n: s.repeat_last_n,
        dry_multiplier: s.dry_multiplier,
        dry_base: s.dry_base,
        dry_allowed_length: s.dry_allowed_length,
        dry_penalty_last_n: s.dry_penalty_last_n,
        dry_sequence_breakers: non_empty(&s.dry_sequence_breakers),
        xtc_probability: s.xtc_probability,
        xtc_threshold: s.xtc_threshold,
        mirostat: s.mirostat,
        mirostat_tau: s.mirostat_tau,
        mirostat_eta: s.mirostat_eta,
        seed: s.seed,
        samplers: non_empty(&s.samplers),
        thinking: s.thinking,
        reasoning_effort: s
            .reasoning_effort
            .filter(|r| !(omit_effort_none && *r == ReasoningEffort::None))
            .map(|r| r.as_wire()),
        reasoning_budget: s.reasoning_budget,
        chat_template_kwargs,
        tools,
        tool_choice,
    }
}

// ---------- streaming response ----------

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub choices: Vec<ChatChoiceChunk>,
    /// The token counter: sent as the final chunk when
    /// `stream_options.include_usage=true` (such a chunk's `choices` is usually empty).
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// The `usage` block of the server response (the token counter). `completion_tokens_details.
/// reasoning_tokens` is returned by OpenAI-compat/llama.cpp servers with a reasoning model
/// (included in `completion_tokens`); absent → `0`.
#[derive(Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub completion_tokens_details: CompletionTokensDetails,
}

/// Token breakdown of the Chat Completions response (only reasoning tokens matter).
#[derive(Debug, Default, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
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

// ---------- embeddings ----------

#[derive(Debug, Serialize)]
pub struct EmbeddingRequest {
    /// The embedding model's name. Mandatory for the cloud (OpenAI/Gemini); for
    /// `llama-server` it's ignored — sent only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    use crate::shared::api::contract::ApiMessage;

    #[test]
    fn omits_stop_and_none_fields() {
        let req = ChatRequest {
            system: Some("sys".into()),
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let body = build_chat_request(&req, true, None, false);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("stop").is_none(), "stop must never be sent");
        assert!(json.get("temperature").is_none());
        assert_eq!(json["stream"], true);
        // Streaming → ask for usage to be sent (the token counter).
        assert_eq!(json["stream_options"]["include_usage"], true);
        // system must go as the first message
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
                dynatemp_range: Some(0.4),
                dynatemp_exponent: Some(1.0),
                top_k: Some(40),
                top_p: Some(0.95),
                min_p: Some(0.03),
                top_n_sigma: Some(1.5),
                adaptive_target: Some(0.1),
                adaptive_decay: Some(0.9),
                frequency_penalty: Some(0.1),
                presence_penalty: Some(0.2),
                repeat_penalty: Some(1.0),
                dry_multiplier: Some(0.8),
                dry_base: Some(1.75),
                dry_allowed_length: Some(2),
                xtc_probability: Some(0.3),
                xtc_threshold: Some(0.15),
                seed: Some(-1),
                max_tokens: Some(256),
                thinking: Some(true),
                reasoning_effort: Some(ReasoningEffort::High),
                reasoning_budget: Some(0),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        // f32→f64 widening makes exact comparison unreliable — compare approximately.
        let approx = |v: &serde_json::Value, want: f64| (v.as_f64().unwrap() - want).abs() < 1e-6;
        assert!(approx(&json["temperature"], 0.8));
        assert!(approx(&json["dynatemp_range"], 0.4));
        assert!(approx(&json["dynatemp_exponent"], 1.0));
        assert_eq!(json["top_k"], 40);
        assert!(approx(&json["top_p"], 0.95));
        assert!(approx(&json["min_p"], 0.03));
        assert!(approx(&json["top_n_sigma"], 1.5));
        assert!(approx(&json["adaptive_target"], 0.1));
        assert!(approx(&json["adaptive_decay"], 0.9));
        assert!(approx(&json["frequency_penalty"], 0.1));
        assert!(approx(&json["presence_penalty"], 0.2));
        assert!(approx(&json["repeat_penalty"], 1.0));
        assert!(approx(&json["dry_multiplier"], 0.8));
        assert!(approx(&json["dry_base"], 1.75));
        assert_eq!(json["dry_allowed_length"], 2);
        assert!(approx(&json["xtc_probability"], 0.3));
        assert!(approx(&json["xtc_threshold"], 0.15));
        assert_eq!(json["seed"], -1);
        assert_eq!(json["max_tokens"], 256);
        assert_eq!(json["thinking"], true);
        assert_eq!(json["reasoning_effort"], "high");
        assert_eq!(json["reasoning_budget"], 0);
        // reasoning_budget=0 is also signaled for Jinja templates.
        assert_eq!(json["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(json["stream"], false);
        // Without streaming, usage isn't requested.
        assert!(json.get("stream_options").is_none());
        // Unset extensions aren't serialized.
        assert!(json.get("typical_p").is_none());
        assert!(json.get("mirostat").is_none());
    }

    #[test]
    fn list_fields_sent_as_arrays_and_empty_omitted() {
        // Non-empty lists → JSON arrays.
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                dry_sequence_breakers: Some(vec!["\n".into(), ":".into()]),
                samplers: Some(vec!["penalties".into(), "temperature".into()]),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = serde_json::to_value(build_chat_request(&req, false, None, false)).unwrap();
        assert_eq!(json["dry_sequence_breakers"][0], "\n");
        assert_eq!(json["dry_sequence_breakers"][1], ":");
        assert_eq!(json["samplers"][0], "penalties");
        assert_eq!(json["samplers"][1], "temperature");

        // Empty lists are NOT sent (otherwise the server would take them as "disable everything").
        let req_empty = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                dry_sequence_breakers: Some(vec![]),
                samplers: Some(vec![]),
                ..Default::default()
            },
            tools: vec![],
        };
        let json_empty =
            serde_json::to_value(build_chat_request(&req_empty, false, None, false)).unwrap();
        assert!(json_empty.get("dry_sequence_breakers").is_none());
        assert!(json_empty.get("samplers").is_none());
    }

    #[test]
    fn model_is_sent_when_some_and_omitted_when_none() {
        // For an external proxy, the model name is set; for llama-server (None) — it isn't.
        let req = ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig::default(),
            tools: vec![],
        };
        let with_model =
            serde_json::to_value(build_chat_request(&req, true, Some("some-model"), false))
                .unwrap();
        assert_eq!(with_model["model"], "some-model");
        let no_model = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert!(no_model.get("model").is_none());
    }

    /// `reasoning_effort: "none"` is how the orchestrator says "don't think" on its
    /// auxiliary turns (title, compaction, impersonation) — llama.cpp obeys it, xAI
    /// answers `400`. With the flag on, the field is omitted rather than sent, and
    /// **only** that value is affected: an explicit `low`/`high` still goes out.
    #[test]
    fn effort_none_is_omitted_only_when_asked() {
        let req = |e: ReasoningEffort| ChatRequest {
            system: None,
            messages: vec![ApiMessage::user("hi")],
            sampling: SamplingConfig {
                reasoning_effort: Some(e),
                ..Default::default()
            },
            tools: vec![],
        };
        let json = |e: ReasoningEffort, omit: bool| {
            serde_json::to_value(build_chat_request(&req(e), true, None, omit)).unwrap()
        };
        // Default (llama.cpp): "none" is a meaningful value, keep sending it.
        assert_eq!(
            json(ReasoningEffort::None, false)["reasoning_effort"],
            "none"
        );
        // Grok: dropped entirely — the model reasons at its default depth.
        assert!(
            json(ReasoningEffort::None, true)
                .get("reasoning_effort")
                .is_none()
        );
        // Every other level is untouched by the flag.
        assert_eq!(json(ReasoningEffort::Low, true)["reasoning_effort"], "low");
        assert_eq!(
            json(ReasoningEffort::XHigh, true)["reasoning_effort"],
            "xhigh"
        );
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
    fn parses_usage_chunk() {
        // The final include_usage chunk: choices is empty, usage is present.
        let raw = r#"{"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49}}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert!(chunk.choices.is_empty());
        let u = chunk.usage.unwrap();
        assert_eq!(u.prompt_tokens, 42);
        assert_eq!(u.completion_tokens, 7);
    }

    #[test]
    fn chunk_without_usage_is_none() {
        let raw = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        assert!(chunk.usage.is_none());
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
        use crate::shared::api::contract::ToolSchema;
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
        let json = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["function"]["name"], "note_save");
    }

    #[test]
    fn serializes_assistant_tool_calls_in_history() {
        use crate::shared::api::contract::ApiToolCall;
        let req = ChatRequest {
            system: None,
            messages: vec![
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
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
        let json = serde_json::to_value(build_chat_request(&req, true, None, false)).unwrap();
        assert_eq!(json["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            json["messages"][0]["tool_calls"][0]["function"]["name"],
            "f"
        );
        assert_eq!(json["messages"][1]["role"], "tool");
        assert_eq!(json["messages"][1]["tool_call_id"], "c1");
        // A request without tools must not contain tool_choice.
        assert!(json.get("tool_choice").is_none());
    }
}
