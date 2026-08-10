//! Demo fixture — the showcase conversation behind generated screenshots
//! (docs/demo-screenshots.md) and, from stage 3 on, the interactive
//! `mindfork demo` mode.
//!
//! The content is honest fabrication: a plausible conversation that walks the
//! renderer through its range — a GFM table, a highlighted code block, a
//! Mermaid flowchart, delimiter-scoped LaTeX, an expanded "thoughts" block and
//! a tool call with its result. Ids and timestamps are fixed so a captured
//! frame is byte-stable across runs (the drift gate depends on it).

use chrono::{DateTime, TimeZone, Utc};
use uuid::Uuid;

use crate::entities::chat::FeedView;
use crate::entities::message::{Message, MessageRole, ToolCallRecord};

/// Title of the showcase chat (shown in the feed header).
pub const CHAT_TITLE: &str = "Gemma 4 on a 12 GB GPU";

/// Fixed chat id — captures must not depend on a fresh `Uuid::new_v4()`.
pub fn chat_id() -> Uuid {
    Uuid::from_u128(0x6d69_6e64_666f_726b_5f64_656d_6f5f_3031)
}

/// The feed view for captures: thoughts and tool-call details expanded —
/// collapsed pills demonstrate nothing.
pub fn feed_view() -> FeedView {
    FeedView {
        thoughts: true,
        tools: true,
    }
}

/// Fixed timestamp base; messages step forward a minute at a time.
fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 1, 10, minute, 0).unwrap()
}

fn message(role: MessageRole, minute: u32, text: &str) -> Message {
    let mut m = Message::new(role, text);
    // Fixed id derived from the slot, fixed time: determinism over realism.
    m.id = Uuid::from_u128(0x6d66_5f64_656d_6f00 + minute as u128);
    m.timestamp = at(minute);
    m
}

/// The showcase conversation. Composed so that the *tail* — what a capture at
/// the stage-1 frame size actually shows — contains the table's neighbourhood,
/// the flowchart, the LaTeX line, the expanded thoughts and the tool card,
/// while the opening exchange scrolls off the top (a conversation visibly has
/// history). `app/demo_shots.rs` asserts the load-bearing pieces are on
/// screen, so edits here cannot silently push them out of frame.
pub fn showcase_messages() -> Vec<Message> {
    let m1 = message(
        MessageRole::User,
        0,
        "Which Gemma 4 12B quant fits a 12 GB card with room for a 16k context?",
    );

    let m2 = message(
        MessageRole::Assistant,
        1,
        "Three candidates, weights only — the KV cache comes on top:\n\n\
         | Quant | Weights | Quality cost |\n\
         |---|---|---|\n\
         | Q4_K_M | ~7.3 GB | small but measurable |\n\
         | Q5_K_M | ~8.4 GB | negligible |\n\
         | Q6_K | ~9.7 GB | noise level |\n\n\
         At 16k context the KV cache adds roughly 1.5 GB, so **Q5_K_M** is the \
         sweet spot — quality headroom *and* a safety margin:\n\n\
         ```bash\n\
         llama-server -m gemma-4-12B-it-Q5_K_M.gguf -ngl 99 -c 16384 --jinja\n\
         ```",
    );

    let m3 = message(
        MessageRole::User,
        2,
        "Give me the bottom line — a small table plus a decision diagram, and \
         save a note about my setup.",
    );

    let mut m4 = message(
        MessageRole::Assistant,
        3,
        "The short version:\n\n\
         | Quant | 16k context? | Verdict |\n\
         |---|---|---|\n\
         | Q5_K_M | fits, ~1.5 GB spare | **sweet spot** |\n\
         | Q6_K | tight | quality first |\n\n\
         ```mermaid\n\
         flowchart TD\n\
             A{Need the full 16k?} -->|yes| B[Q5_K_M, KV headroom]\n\
             A -->|no| C[Q6_K, quality first]\n\
         ```\n\n\
         Note saved. The cache rule to remember: $M_{kv} \\propto L$ — double \
         the window, double the cache.",
    );
    m4.thoughts = Some(
        "The note should record the 12 GB budget and the Q5_K_M choice, so \
         future sizing questions start from the hardware."
            .into(),
    );
    m4.tool_calls = vec![ToolCallRecord {
        id: "call_demo_1".into(),
        name: "note_save".into(),
        arguments: serde_json::json!({
            "title": "Hardware budget",
            "text": "12 GB GPU; Gemma 4 12B at Q5_K_M, 16k context."
        }),
        result: Some("Note saved: \"Hardware budget\".".into()),
        thought_signature: None,
    }];

    vec![m1, m2, m3, m4]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture is frozen: same ids, same timestamps, same text on every
    /// call — the property every capture and the drift gate stand on.
    #[test]
    fn fixture_is_deterministic() {
        let a = showcase_messages();
        let b = showcase_messages();
        assert_eq!(a, b);
        assert_eq!(a[0].timestamp, at(0));
        assert_ne!(a[0].id, a[1].id, "slot-derived ids must not collide");
    }

    /// The showcase must keep exercising the renderer's range — losing a
    /// feature from the fixture silently demotes the screenshots.
    #[test]
    fn fixture_covers_the_showcase_features() {
        let all = showcase_messages();
        let text: String = all.iter().map(|m| m.text.as_str()).collect();
        for needle in ["| Quant |", "```bash", "```mermaid", "$M_{kv}"] {
            assert!(text.contains(needle), "fixture lost {needle:?}");
        }
        let last = all.last().unwrap();
        assert!(
            last.thoughts.is_some(),
            "the thoughts block is part of the showcase"
        );
        assert_eq!(
            last.tool_calls.len(),
            1,
            "the tool card is part of the showcase"
        );
    }
}
