//! Serde types for the Anthropic Messages protocol (`/v1/messages`) and translating the domain
//! [`ChatRequest`] into its format. Differences from OpenAI (see ADR 0004, Phase 2):
//! - the system message — a **top-level** `system` field (not a role in `messages`);
//! - only `user`/`assistant` roles; tool results — `tool_result` blocks
//!   **inside a user message** (there's no `tool` role);
//! - assistant tool calls — `tool_use` blocks in its `content`;
//! - `max_tokens` is **required**; a tool's schema is `input_schema` (not `parameters`).
//!
//! Adjacent messages of the same role are merged (Anthropic requires strict
//! user/assistant alternation; our agentic-loop produces several `tool` messages in a row →
//! one user block).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::entities::sampling::ReasoningEffort;
use crate::shared::api::contract::{ApiRole, ChatRequest};

/// The default `max_tokens` if unset in sampling (Anthropic requires the field).
pub const DEFAULT_MAX_TOKENS: u64 = 4096;

// ---------- request ----------

#[derive(Debug, Serialize)]
pub struct AntRequest {
    pub model: String,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AntMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AntTool>>,
    /// Extended thinking. Modern Claude models (Opus 4.6+/Sonnet 4.6/Fable 5) accept
    /// only `{type:"adaptive"}` — the old `budget_tokens` gets a `400`. `display:
    /// "summarized"` is needed so the "thoughts" text arrives non-empty (default `omitted`).
    /// Sent only when reasoning is enabled. See docs/journal/engine.md (CoT for Claude).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AntThinking>,
    /// Reasoning depth (`output_config.effort`, GA). Sent only with thinking and
    /// a set `reasoning_effort` (except `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<AntOutputConfig>,
}

/// Anthropic's extended-thinking config. `type` is always `adaptive` (the only
/// "on" mode for 4.6+ models); `display` — `summarized` for a visible CoT.
#[derive(Debug, Serialize)]
pub struct AntThinking {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub display: &'static str,
}

/// Anthropic's `output_config`: effort level (`low`/`medium`/`high`).
#[derive(Debug, Serialize)]
pub struct AntOutputConfig {
    pub effort: &'static str,
}

#[derive(Debug, Serialize)]
pub struct AntMessage {
    pub role: &'static str,
    pub content: Vec<AntBlock>,
}

/// A message content block. Serialized with an internal `type` tag.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntBlock {
    Text {
        text: String,
    },
    /// A reasoning block (extended thinking) for resending. Must come **first**
    /// in an assistant turn with `tool_use`; the signature is mandatory (otherwise `400`). See
    /// [`ThinkingBlock`](crate::shared::api::contract::ThinkingBlock).
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
pub struct AntTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Builds the Anthropic request body from the domain [`ChatRequest`]. `model` is required.
/// From sampling, **only `max_tokens`** is sent (the required field): the newest
/// Claude models (4.x) have "locked in" sampling and reject `temperature`/`top_p`/`top_k`
/// as deprecated (HTTP 400), so these fields aren't sent at all. The UI marks them
/// as unsupported for Claude. See ADR 0004 and docs/journal/engine.md (Phase 2).
pub fn build_request(req: &ChatRequest, model: &str, stream: bool) -> AntRequest {
    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| AntTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect(),
        )
    };
    // Extended thinking is enabled by the sampling `thinking` flag. The budget/`reasoning_budget`
    // isn't used — 4.x models reject `budget_tokens`; depth is set by `effort`.
    let thinking = (req.sampling.thinking == Some(true)).then_some(AntThinking {
        kind: "adaptive",
        display: "summarized",
    });
    let output_config = thinking.as_ref().and_then(|_| {
        req.sampling
            .reasoning_effort
            .and_then(ant_effort)
            .map(|effort| AntOutputConfig { effort })
    });
    AntRequest {
        model: model.to_string(),
        max_tokens: req
            .sampling
            .max_tokens
            .map(|m| m as u64)
            .unwrap_or(DEFAULT_MAX_TOKENS),
        system: req.system.clone(),
        messages: build_messages(req),
        stream,
        tools,
        thinking,
        output_config,
    }
}

/// The effort level → Anthropic's `output_config.effort` value (`low`/`medium`/`high`).
/// `None` (including `ReasoningEffort::None`) — the field isn't sent.
fn ant_effort(e: ReasoningEffort) -> Option<&'static str> {
    match e {
        ReasoningEffort::None => None,
        // Anthropic only understands low/medium/high — OpenAI's extreme steps (minimal/
        // xhigh) are mapped to the nearest supported one.
        ReasoningEffort::Minimal | ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High | ReasoningEffort::XHigh => Some("high"),
    }
}

/// Translates history into Anthropic messages, merging adjacent ones of the same role.
fn build_messages(req: &ChatRequest) -> Vec<AntMessage> {
    let mut out: Vec<AntMessage> = Vec::new();
    for m in &req.messages {
        let (role, blocks): (&'static str, Vec<AntBlock>) = match m.role {
            ApiRole::User => ("user", text_blocks(&m.content)),
            ApiRole::Assistant => {
                let mut blocks = Vec::new();
                // A thinking block (with a signature) must go FIRST in an assistant turn with
                // tool_use — otherwise Anthropic returns 400. Only placed on the current
                // agentic-loop turn (see ApiMessage::with_thinking).
                if let Some(tb) = &m.thinking {
                    blocks.push(AntBlock::Thinking {
                        thinking: tb.text.clone(),
                        signature: tb.signature.clone(),
                    });
                }
                blocks.extend(text_blocks(&m.content));
                for tc in &m.tool_calls {
                    // Our arguments are a JSON string; Anthropic STRICTLY requires
                    // `input` to be an OBJECT. An argument-less call could have been saved
                    // in history as `null` (old chats), and `"null"`/an array/a scalar
                    // would parse into a non-object → Anthropic 400 ("tool_use.input:
                    // Input should be an object"). Any non-object is coerced to `{}`.
                    let input = serde_json::from_str::<Value>(&tc.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| serde_json::json!({}));
                    blocks.push(AntBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input,
                    });
                }
                ("assistant", blocks)
            }
            // A tool result → a tool_result block inside a user message.
            ApiRole::Tool => (
                "user",
                vec![AntBlock::ToolResult {
                    tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                    content: m.content.clone(),
                }],
            ),
            // The system message goes as the top-level `system` field, not in messages.
            ApiRole::System => continue,
        };
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => out.push(AntMessage {
                role,
                content: blocks,
            }),
        }
    }
    out
}

fn text_blocks(content: &str) -> Vec<AntBlock> {
    if content.is_empty() {
        Vec::new()
    } else {
        vec![AntBlock::Text {
            text: content.to_string(),
        }]
    }
}

// ---------- streaming events ----------

/// An Anthropic SSE stream event (the tag is the `type` field in `data`). Uninteresting events
/// (`ping`, `content_block_stop`, `message_stop`) land in [`AntStreamEvent::Other`].
///
/// `error` is **not** one of them, though it used to be: Anthropic documents that a
/// failure after the stream opens arrives as an `error` event — an
/// `overloaded_error` there is the same condition as a pre-stream `529` — and
/// letting it fall into `Other` made an overloaded truncation identical to a normal
/// completion (docs/research/cloud-retry-backoff.md §1.2).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntStreamEvent {
    MessageStart {
        message: AntStartMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: AntStartBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: AntDelta,
    },
    MessageDelta {
        delta: AntStopDelta,
        #[serde(default)]
        usage: Option<AntUsage>,
    },
    Error {
        #[serde(default)]
        error: AntStreamError,
    },
    #[serde(other)]
    Other,
}

/// The payload of an in-stream `error` event. `type` names the condition
/// (`overloaded_error`, `api_error`, …) and is the half worth showing; `message` is
/// prose and can be empty.
#[derive(Debug, Default, Deserialize)]
pub struct AntStreamError {
    #[serde(default, rename = "type")]
    pub name: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct AntStartMessage {
    #[serde(default)]
    pub usage: Option<AntUsage>,
}

/// The start of a content block. Only `tool_use` matters (gives id+tool name).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntStartBlock {
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

/// A content block delta.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AntDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// The "thoughts" block's signature (arrives at the end of the thinking block). Needed for
    /// resending thinking on tool-use in the same turn.
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
pub struct AntStopDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AntUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
    use crate::shared::api::contract::{ApiMessage, ApiToolCall, ThinkingBlock, ToolSchema};

    fn req(messages: Vec<ApiMessage>) -> ChatRequest {
        ChatRequest {
            system: Some("Ты — ассистент.".into()),
            messages,
            sampling: SamplingConfig {
                temperature: Some(0.7),
                top_k: Some(40),
                max_tokens: Some(256),
                ..Default::default()
            },
            tools: vec![],
        }
    }

    #[test]
    fn system_is_top_level_and_sampling_mapped() {
        let body = build_request(&req(vec![ApiMessage::user("привет")]), "claude-x", true);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "claude-x");
        assert_eq!(json["system"], "Ты — ассистент.");
        assert_eq!(json["max_tokens"], 256);
        // Claude 4.x doesn't accept temperature/top_p/top_k — don't send them at all.
        assert!(json.get("temperature").is_none());
        assert!(json.get("top_p").is_none());
        assert!(json.get("top_k").is_none());
        // The message — a user with a text block.
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"][0]["type"], "text");
        assert_eq!(json["messages"][0]["content"][0]["text"], "привет");
    }

    #[test]
    fn max_tokens_defaults_when_absent() {
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.sampling.max_tokens = None;
        let json = serde_json::to_value(build_request(&r, "claude-x", false)).unwrap();
        assert_eq!(json["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn tool_calls_become_tool_use_and_results_merge_into_user() {
        // assistant(tool_use) → tool → tool: two results merge into one user.
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            ),
            ApiMessage::tool("t1", "2"),
            ApiMessage::tool("t1", "доп"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        // [0] user, [1] assistant(tool_use), [2] user(2× tool_result).
        assert_eq!(json["messages"][1]["role"], "assistant");
        assert_eq!(json["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(json["messages"][1]["content"][0]["name"], "calc");
        assert_eq!(json["messages"][1]["content"][0]["input"]["x"], 1);
        assert_eq!(json["messages"][2]["role"], "user");
        assert_eq!(json["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(json["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(json["messages"][2]["content"][1]["content"], "доп");
    }

    #[test]
    fn tool_use_input_coerced_to_object_when_not_object() {
        // An argument-less call could have been saved in history as `null` → the string "null".
        // Anthropic strictly requires an object — coerce to `{}` (otherwise 400).
        for raw in ["null", "", "[1,2]", "42", "\"x\""] {
            let r = req(vec![
                ApiMessage::user("введи"),
                ApiMessage::assistant_tool_calls(
                    "",
                    vec![ApiToolCall {
                        thought_signature: None,
                        id: "t1".into(),
                        name: "get_sampling".into(),
                        arguments: raw.into(),
                    }],
                ),
            ]);
            let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
            let input = &json["messages"][1]["content"][0]["input"];
            assert!(
                input.is_object(),
                "input must be an object for arguments={raw:?}, got {input}"
            );
        }
    }

    #[test]
    fn tools_use_input_schema() {
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.tools = vec![ToolSchema {
            name: "calc".into(),
            description: "Считает".into(),
            parameters: serde_json::json!({"type":"object"}),
        }];
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["tools"][0]["name"], "calc");
        assert_eq!(json["tools"][0]["input_schema"]["type"], "object");
        assert!(json["tools"][0].get("parameters").is_none());
    }

    #[test]
    fn parses_stream_events() {
        let start =
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1}}}"#;
        match serde_json::from_str::<AntStreamEvent>(start).unwrap() {
            AntStreamEvent::MessageStart { message } => {
                assert_eq!(message.usage.unwrap().input_tokens, 25)
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }
        let td =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        match serde_json::from_str::<AntStreamEvent>(td).unwrap() {
            AntStreamEvent::ContentBlockDelta {
                delta: AntDelta::TextDelta { text },
                ..
            } => assert_eq!(text, "hi"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        let tu = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"calc","input":{}}}"#;
        match serde_json::from_str::<AntStreamEvent>(tu).unwrap() {
            AntStreamEvent::ContentBlockStart {
                index,
                content_block: AntStartBlock::ToolUse { id, name },
            } => {
                assert_eq!(index, 1);
                assert_eq!(id, "tu1");
                assert_eq!(name, "calc");
            }
            other => panic!("expected ToolUse start, got {other:?}"),
        }
        let md = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#;
        match serde_json::from_str::<AntStreamEvent>(md).unwrap() {
            AntStreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("tool_use"));
                assert_eq!(usage.unwrap().output_tokens, 15);
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
        // ping/other — Other (no crash).
        assert!(matches!(
            serde_json::from_str::<AntStreamEvent>(r#"{"type":"ping"}"#).unwrap(),
            AntStreamEvent::Other
        ));
    }

    #[test]
    fn thinking_off_by_default() {
        // Default sampling (thinking=None) — the thinking field isn't sent.
        let json = serde_json::to_value(build_request(
            &req(vec![ApiMessage::user("привет")]),
            "claude-x",
            true,
        ))
        .unwrap();
        assert!(json.get("thinking").is_none());
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn thinking_enabled_sends_adaptive_with_effort() {
        // thinking=true → adaptive+summarized; reasoning_effort → output_config.effort.
        // Claude 4.x rejects budget_tokens (reasoning_budget) — DON'T send it.
        let mut r = req(vec![ApiMessage::user("посчитай")]);
        r.sampling.thinking = Some(true);
        r.sampling.reasoning_effort = Some(ReasoningEffort::High);
        r.sampling.reasoning_budget = Some(0);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert_eq!(json["thinking"]["display"], "summarized");
        assert_eq!(json["output_config"]["effort"], "high");
        // reasoning_budget doesn't leak out (no budget_tokens key).
        assert!(json.get("budget_tokens").is_none());
        assert!(json["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn thinking_effort_none_omits_output_config() {
        // thinking is enabled, but effort isn't set → output_config isn't sent.
        let mut r = req(vec![ApiMessage::user("hi")]);
        r.sampling.thinking = Some(true);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["thinking"]["type"], "adaptive");
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn thinking_block_prepended_to_assistant_tool_use() {
        // An assistant turn with tool_use + a thinking block: thinking comes first (with a signature),
        // then tool_use. Anthropic requires exactly this order (otherwise 400).
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{\"x\":1}".into(),
                }],
            )
            .with_thinking(Some(ThinkingBlock {
                text: "надо сложить".into(),
                signature: "sig-abc".into(),
                id: None,
            })),
            ApiMessage::tool("t1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        let blocks = &json["messages"][1]["content"];
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "надо сложить");
        assert_eq!(blocks[0]["signature"], "sig-abc");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["name"], "calc");
    }

    #[test]
    fn assistant_without_thinking_has_no_thinking_block() {
        // Without a thinking block (a regular/historical turn) — no thinking block in the output.
        let r = req(vec![
            ApiMessage::user("посчитай"),
            ApiMessage::assistant_tool_calls(
                "",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "t1".into(),
                    name: "calc".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("t1", "2"),
        ]);
        let json = serde_json::to_value(build_request(&r, "claude-x", true)).unwrap();
        assert_eq!(json["messages"][1]["content"][0]["type"], "tool_use");
    }

    #[test]
    fn parses_signature_delta() {
        let sd = r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-xyz"}}"#;
        match serde_json::from_str::<AntStreamEvent>(sd).unwrap() {
            AntStreamEvent::ContentBlockDelta {
                delta: AntDelta::SignatureDelta { signature },
                ..
            } => assert_eq!(signature, "sig-xyz"),
            other => panic!("expected SignatureDelta, got {other:?}"),
        }
    }
}

/// In-stream `error` events — the shape Anthropic sheds load with once a stream is
/// open. See docs/research/cloud-retry-backoff.md §1.2.
#[cfg(test)]
mod stream_error_tests {
    use super::*;

    #[test]
    fn an_overloaded_error_event_is_parsed_rather_than_ignored() {
        // Verbatim from Anthropic's streaming docs.
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let event: AntStreamEvent = serde_json::from_str(data).unwrap();
        let AntStreamEvent::Error { error } = event else {
            panic!("an error event must not land in Other — that is the defect");
        };
        assert_eq!(error.name, "overloaded_error");
        assert_eq!(error.message, "Overloaded");
        assert!(crate::shared::api::error::stream_error_transient(
            &error.name,
            None
        ));
    }

    #[test]
    fn an_error_event_without_a_message_still_names_the_condition() {
        let data = r#"{"type":"error","error":{"type":"api_error"}}"#;
        let AntStreamEvent::Error { error } = serde_json::from_str(data).unwrap() else {
            panic!("expected an error event")
        };
        assert_eq!(
            crate::shared::api::error::stream_error_text(&error.name, &error.message),
            "api_error"
        );
    }

    /// The events that genuinely carry nothing must keep falling through — the new
    /// variant must not widen into them.
    #[test]
    fn uninteresting_events_still_land_in_other() {
        for data in [
            r#"{"type":"ping"}"#,
            r#"{"type":"message_stop"}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ] {
            assert!(matches!(
                serde_json::from_str::<AntStreamEvent>(data).unwrap(),
                AntStreamEvent::Other
            ));
        }
    }
}
