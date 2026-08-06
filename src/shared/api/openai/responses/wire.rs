//! Serde types for the OpenAI Responses protocol (`/v1/responses`) and translating the domain
//! [`ChatRequest`] into its format. Differences from Chat Completions (see ADR 0004,
//! docs/research/openai-responses-client.md):
//! - the system message — top-level `instructions` (not a role in `messages`);
//! - history — an `input` array of **items** (messages, `reasoning`,
//!   `function_call`, `function_call_output`), not `messages` with `tool_calls`;
//! - a tool result — a `function_call_output` item (there's no `tool` role);
//! - the token limit — `max_output_tokens` (includes reasoning tokens!);
//! - a reasoning item with `encrypted_content` is returned **before** its own
//!   `function_call` (stateless mode `store:false`, an analog of Anthropic's thinking signature).
//!
//! Event-based SSE: the event tag is the `type` field inside `data` (like Anthropic).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::shared::api::contract::{ApiMessage, ApiRole, ChatRequest};

// ---------- request ----------

#[derive(Debug, Serialize)]
pub struct RespRequest {
    pub model: String,
    /// The system message (top-level, not in `input`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub input: Vec<Value>,
    pub stream: bool,
    /// The app keeps its own history → don't ask the server to store replies (privacy,
    /// stateless). With `store:false`, reasoning items are returned in `input`.
    pub store: bool,
    /// `include: ["reasoning.encrypted_content"]` — ask for the encrypted reasoning
    /// in reasoning items (needed for resending on tool-use). Only sent when
    /// reasoning is enabled (otherwise pointless/extraneous on non-reasoning models).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<&'static str>,
    /// The reply's token limit (includes reasoning tokens — with a stingy value the reasoning
    /// eats the budget and `output_text` comes back empty; see docs/research/openai-responses-client.md).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<RespReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<RespText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<RespTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<&'static str>,
}

/// The reasoning config. `effort` — depth (`none`/`minimal`/`low`/`medium`/`high`/
/// `xhigh`); `summary` — `auto` for a visible "thoughts" summary (the API doesn't return raw CoT).
#[derive(Debug, Serialize)]
pub struct RespReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
}

/// `text.verbosity` — how verbose the reply is.
#[derive(Debug, Serialize)]
pub struct RespText {
    pub verbosity: &'static str,
}

/// A function tool in Responses' flat form (`{type,name,description,parameters,strict}`).
/// `strict:false` — our schemas don't satisfy strict mode's requirements
/// (`additionalProperties:false` + every field in `required`), and by default Responses
/// "tries strict" — disable it explicitly.
#[derive(Debug, Serialize)]
pub struct RespTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub strict: bool,
}

/// Builds the Responses request body from the domain [`ChatRequest`]. `model` is required.
/// From sampling, only `max_tokens`→`max_output_tokens`, `thinking`/
/// `reasoning_effort`→`reasoning`, `verbosity`→`text` apply — Responses has no other fields.
pub fn build_request(req: &ChatRequest, model: &str, stream: bool) -> RespRequest {
    let s = &req.sampling;
    // reasoning_budget==0 forces "thoughts" off (impersonation/auto-title),
    // like llama.cpp's reasoning_budget=0: effort=none, no summary.
    let force_off = s.reasoning_budget == Some(0);
    let want_summary = !force_off && s.thinking == Some(true);
    let effort = if force_off {
        Some("none")
    } else {
        s.reasoning_effort.map(|e| e.as_wire())
    };
    // summary: "detailed", not "auto" — some models return an EMPTY summary at "auto",
    // but text at "detailed" (per developer reports; gpt-5.x supports detailed).
    // IMPORTANT: reasoning summaries only reach organizations that passed verification
    // (platform.openai.com/settings/organization/general) — otherwise the "thoughts" stream is empty
    // (or an unverified org gets a 400 on the mere presence of `reasoning.summary`).
    let reasoning = (want_summary || effort.is_some()).then_some(RespReasoning {
        effort,
        summary: want_summary.then_some("detailed"),
    });
    // The encrypted reasoning is only needed when reasoning is actually enabled.
    let include = if reasoning.is_some() {
        vec!["reasoning.encrypted_content"]
    } else {
        Vec::new()
    };

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| RespTool {
                    kind: "function",
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                    strict: false,
                })
                .collect(),
        )
    };
    let tool_choice = tools.as_ref().map(|_| "auto");

    RespRequest {
        model: model.to_string(),
        instructions: req.system.clone(),
        input: build_input(req),
        stream,
        store: false,
        include,
        max_output_tokens: s.max_tokens,
        reasoning,
        text: s.verbosity.map(|v| RespText {
            verbosity: v.as_wire(),
        }),
        tools,
        tool_choice,
    }
}

/// Translates history into Responses' `input` array. A reasoning item (with `id`+
/// `encrypted_content`) is placed **before** the `function_call` items of the same
/// assistant turn — Responses expects it immediately before the call (see
/// [`ThinkingBlock`](crate::shared::api::contract::ThinkingBlock)).
fn build_input(req: &ChatRequest) -> Vec<Value> {
    let mut items = Vec::new();
    for m in &req.messages {
        match m.role {
            // The system message goes into top-level `instructions`.
            ApiRole::System => continue,
            ApiRole::User => items.push(json!({
                "type": "message", "role": "user", "content": m.content,
            })),
            ApiRole::Assistant => push_assistant_items(m, &mut items),
            ApiRole::Tool => items.push(json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.clone().unwrap_or_default(),
                "output": m.content,
            })),
        }
    }
    items
}

/// An assistant turn's `input` items: the reasoning item (which must precede
/// the calls), the text message, then the `function_call` items.
fn push_assistant_items(m: &ApiMessage, items: &mut Vec<Value>) {
    // The current turn's reasoning item (only if there's an id — only
    // OpenAI Responses carries it; other backends have thinking.id == None).
    if let Some(tb) = &m.thinking
        && let Some(id) = &tb.id
    {
        // `summary` is a REQUIRED field of a reasoning item in the Responses API
        // (otherwise 400 `Missing required parameter: 'input[N].summary'`). Send an
        // empty array: the meaning is carried by `encrypted_content`, and the summary text
        // isn't needed for resending (and for an unverified org it's empty, §7a
        // docs/research/openai-responses-client.md).
        items.push(json!({
            "type": "reasoning",
            "id": id,
            "summary": [],
            "encrypted_content": tb.signature,
        }));
    }
    if !m.content.is_empty() {
        items.push(json!({
            "type": "message", "role": "assistant", "content": m.content,
        }));
    }
    for tc in &m.tool_calls {
        // arguments — a JSON string; an argument-less call → an empty object.
        let args = if tc.arguments.is_empty() {
            "{}"
        } else {
            tc.arguments.as_str()
        };
        items.push(json!({
            "type": "function_call",
            "call_id": tc.id,
            "name": tc.name,
            "arguments": args,
        }));
    }
}

// ---------- streaming events ----------

/// A Responses SSE event (the tag is the `type` field in `data`). Uninteresting events
/// (`response.created`, `*.part.added`, `*.done` other than `output_item.done`, `ping`)
/// land in [`RespEvent::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RespEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryDelta {
        #[serde(default)]
        delta: String,
    },
    /// Some reasoning models/settings stream the reasoning via this event rather than the
    /// summary event — handle both (both → `ChatChunk::Thoughts`).
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        #[serde(default)]
        output_index: usize,
        item: RespItem,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { item: RespItem },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionArgsDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: RespBody },
    #[serde(rename = "response.incomplete")]
    Incomplete {
        #[serde(default)]
        response: Option<RespBody>,
    },
    #[serde(rename = "response.failed")]
    Failed,
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// An output item (`output_item.added`/`done`). Only `function_call` (gives
/// `call_id`+name) and `reasoning` (in `done` carries `encrypted_content`) matter; anything else (a text
/// message etc.) — [`RespItem::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RespItem {
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        id: String,
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// The `response` object in terminal events (`completed`/`incomplete`). Only the
/// token counter matters — `status`/other fields are ignored (the client infers the finish reason).
#[derive(Debug, Default, Deserialize)]
pub struct RespBody {
    #[serde(default)]
    pub usage: Option<RespUsage>,
}

/// The token counter of a Responses reply (`input_tokens`/`output_tokens` +
/// `output_tokens_details.reasoning_tokens`).
#[derive(Debug, Default, Deserialize)]
pub struct RespUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub output_tokens_details: RespOutputTokensDetails,
}

/// Token breakdown of the reply (only "thoughts" reasoning tokens matter).
#[derive(Debug, Default, Deserialize)]
pub struct RespOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig, Verbosity};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

    fn base_req(messages: Vec<ApiMessage>) -> ChatRequest {
        ChatRequest {
            system: Some("Ты — ассистент.".into()),
            messages,
            sampling: SamplingConfig {
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        }
    }

    #[test]
    fn system_is_instructions_and_store_false() {
        let json = serde_json::to_value(build_request(
            &base_req(vec![ApiMessage::user("hi")]),
            "gpt-x",
            true,
        ))
        .unwrap();
        assert_eq!(json["model"], "gpt-x");
        assert_eq!(json["instructions"], "Ты — ассистент.");
        assert_eq!(json["store"], false);
        assert_eq!(json["max_output_tokens"], 256);
        assert!(json.get("max_tokens").is_none());
        // The first input item — a user message with string content.
        assert_eq!(json["input"][0]["type"], "message");
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"], "hi");
        // Without reasoning, don't ask for encrypted_content and don't send reasoning/text.
        assert!(json.get("include").is_none());
        assert!(json.get("reasoning").is_none());
        assert!(json.get("text").is_none());
    }

    #[test]
    fn reasoning_summary_and_effort_and_verbosity() {
        let mut r = base_req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::XHigh);
        r.sampling.verbosity = Some(Verbosity::Low);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "xhigh");
        assert_eq!(json["reasoning"]["summary"], "detailed");
        assert_eq!(json["text"]["verbosity"], "low");
        // Reasoning is enabled → ask for the encrypted reasoning.
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn reasoning_budget_zero_forces_off() {
        // thinking is enabled, but reasoning_budget=0 (impersonation/auto-title) →
        // effort=none, no summary; include isn't sent.
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_budget = Some(0);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "none");
        assert!(json["reasoning"].get("summary").is_none());
    }

    #[test]
    fn effort_without_summary_when_thinking_off() {
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.sampling.reasoning_effort = Some(ReasoningEffort::Low);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["reasoning"]["effort"], "low");
        assert!(json["reasoning"].get("summary").is_none());
        // effort is set (reasoning exists) → include is present.
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn tools_are_flat_with_strict_false() {
        let mut r = base_req(vec![ApiMessage::user("hi")]);
        r.tools = vec![ToolSchema {
            name: "calc".into(),
            description: "Считает".into(),
            parameters: json!({"type":"object"}),
        }];
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["name"], "calc");
        assert_eq!(json["tools"][0]["strict"], false);
        assert_eq!(json["tools"][0]["parameters"]["type"], "object");
        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn tool_call_and_result_become_items() {
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            ),
            ApiMessage::tool("call_1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        // [0] user message, [1] function_call, [2] function_call_output.
        assert_eq!(json["input"][1]["type"], "function_call");
        assert_eq!(json["input"][1]["call_id"], "call_1");
        assert_eq!(json["input"][1]["name"], "calc");
        assert_eq!(json["input"][1]["arguments"], "{\"x\":1}");
        assert_eq!(json["input"][2]["type"], "function_call_output");
        assert_eq!(json["input"][2]["call_id"], "call_1");
        assert_eq!(json["input"][2]["output"], "2");
    }

    #[test]
    fn reasoning_item_precedes_function_call() {
        // An assistant turn with thinking (id+encrypted) → a reasoning item before function_call.
        let r = base_req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "резюме".into(),
                signature: "gAAA-enc".into(),
                id: Some("rs_42".into()),
            })),
            ApiMessage::tool("call_1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        // [0] user, [1] reasoning (id+summary+encrypted), [2] function_call, [3] output.
        assert_eq!(json["input"][1]["type"], "reasoning");
        assert_eq!(json["input"][1]["id"], "rs_42");
        assert_eq!(json["input"][1]["encrypted_content"], "gAAA-enc");
        // `summary` is required for a reasoning item (otherwise 400) — send an empty array.
        assert_eq!(json["input"][1]["summary"], json!([]));
        assert_eq!(json["input"][2]["type"], "function_call");
    }

    #[test]
    fn thinking_without_id_omits_reasoning_item() {
        // A thinking block with no id (Anthropic-style) gives no reasoning item in Responses.
        let r = base_req(vec![
            ApiMessage::user("hi"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call_1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "x".into(),
                signature: "sig".into(),
                id: None,
            })),
        ]);
        let json = serde_json::to_value(build_request(&r, "gpt-x", true)).unwrap();
        assert_eq!(json["input"][1]["type"], "function_call");
    }

    #[test]
    fn parses_stream_events() {
        let td = r#"{"type":"response.output_text.delta","delta":"hi"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(td).unwrap(),
            RespEvent::OutputTextDelta { delta } if delta == "hi"
        ));
        let rd = r#"{"type":"response.reasoning_summary_text.delta","delta":"думаю"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(rd).unwrap(),
            RespEvent::ReasoningSummaryDelta { delta } if delta == "думаю"
        ));
        // The alternate reasoning event — also recognized.
        let rt = r#"{"type":"response.reasoning_text.delta","delta":"шаг"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(rt).unwrap(),
            RespEvent::ReasoningTextDelta { delta } if delta == "шаг"
        ));
        let added = r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"calc","arguments":""}}"#;
        match serde_json::from_str::<RespEvent>(added).unwrap() {
            RespEvent::OutputItemAdded {
                output_index,
                item: RespItem::FunctionCall { call_id, name },
            } => {
                assert_eq!(output_index, 1);
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "calc");
            }
            other => panic!("expected function_call added, got {other:?}"),
        }
        let done = r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_9","encrypted_content":"ENC"}}"#;
        match serde_json::from_str::<RespEvent>(done).unwrap() {
            RespEvent::OutputItemDone {
                item:
                    RespItem::Reasoning {
                        id,
                        encrypted_content: Some(enc),
                    },
                ..
            } => {
                assert_eq!(id, "rs_9");
                assert_eq!(enc, "ENC");
            }
            other => panic!("expected reasoning done, got {other:?}"),
        }
        let args = r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"x\":1}"}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(args).unwrap(),
            RespEvent::FunctionArgsDelta { output_index, delta } if output_index == 1 && delta == "{\"x\":1}"
        ));
        let completed = r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":42,"output_tokens":7}}}"#;
        match serde_json::from_str::<RespEvent>(completed).unwrap() {
            RespEvent::Completed { response } => {
                let u = response.usage.unwrap();
                assert_eq!(u.input_tokens, 42);
                assert_eq!(u.output_tokens, 7);
            }
            other => panic!("expected completed, got {other:?}"),
        }
        // An uninteresting event → Other (no crash).
        assert!(matches!(
            serde_json::from_str::<RespEvent>(r#"{"type":"response.created","response":{}}"#)
                .unwrap(),
            RespEvent::Other
        ));
        // A text message as an output item → RespItem::Other.
        let msg_added = r#"{"type":"response.output_item.added","output_index":2,"item":{"type":"message","role":"assistant","content":[]}}"#;
        assert!(matches!(
            serde_json::from_str::<RespEvent>(msg_added).unwrap(),
            RespEvent::OutputItemAdded {
                item: RespItem::Other,
                ..
            }
        ));
    }
}
