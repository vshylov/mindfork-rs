//! A line that ends between two bubbles of one turn, on every wire
//! (docs/research/chat-branches.md §6, stage 2).
//!
//! A fork cuts the line at a bubble's edge. Between a reply's first bubble
//! and the follow-up bubble `send_followup_message` opened, that edge falls
//! after the control call's result — so the line ends on a tool result, and
//! the next thing in the conversation is the person's message. The same shape
//! is left by a turn cancelled after a tool round; a fork makes it a thing
//! the person does on purpose, at any depth, so each client's request for it
//! is pinned: the call answered by its result, the result first in what
//! follows the call, the person's words after it, nothing lost.
//!
//! The line and the order every wire must keep are here; each wire's own test
//! sits beside its builder, which is private to its client
//! (`a_line_cut_after_a_tool_result_takes_the_persons_message_next`).

use serde_json::Value;

use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{ApiMessage, ApiToolCall, ChatRequest};

/// `question → reply with a follow-up call → the call's result → [cut] →
/// another message`.
pub(crate) fn forked_line() -> ChatRequest {
    ChatRequest {
        continue_final: false,
        output_schema: None,
        system: Some("You are an assistant.".into()),
        messages: vec![
            ApiMessage::user("the first question"),
            ApiMessage::assistant_tool_calls(
                "The first message.",
                vec![ApiToolCall {
                    thought_signature: None,
                    id: "call-1".into(),
                    name: "send_followup_message".into(),
                    arguments: "{}".into(),
                }],
            ),
            ApiMessage::tool("call-1", "ok"),
            // The follow-up bubble stood here; a fork parked it.
            ApiMessage::user("another way from here"),
        ],
        sampling: SamplingConfig {
            max_tokens: Some(256),
            ..Default::default()
        },
        tools: vec![],
    }
}

/// Every string in `value`, in document order — where the person's words
/// are, whatever the wire calls the field.
fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

fn position(json: &Value, needle: &str) -> usize {
    let mut all = Vec::new();
    strings(json, &mut all);
    all.iter()
        .position(|s| s == needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in the request: {json}"))
}

/// The order every wire must keep, whatever its shape: the call, its result,
/// then the person's message.
pub(crate) fn assert_in_order(json: &Value) {
    let call = position(json, "send_followup_message");
    let result = position(json, "ok");
    let message = position(json, "another way from here");
    assert!(call < result && result < message, "{json}");
}
