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

use crate::entities::chat::{ChatSummary, FeedView};
use crate::entities::message::{Message, MessageRole, ToolCallRecord};
use crate::entities::profile::Profile;
use crate::entities::self_model::{Goal, GoalStatus, NarrativeSegment, SelfModel, UserModel};
use crate::features::tools::default_tool_ids;
use crate::shared::config::{AppConfig, Theme};
use crate::shared::i18n::Lang;

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

/// Fixed calendar date for the list/self-model fixtures.
fn date(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
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

/// Fixed profile id for the demo companion.
pub fn profile_id() -> Uuid {
    Uuid::from_u128(0x6d66_5f64_656d_6f5f_7072_6f66_0000_0001)
}

/// The demo companion: a named profile with the base tools enabled, so the
/// settings screen's Tools section shows a real toggle set.
pub fn profile() -> Profile {
    let mut p = Profile::new("Gaia", "You are Gaia — a thoughtful local companion.");
    p.enabled_tools = default_tool_ids();
    p
}

/// The app config the settings captures depict: a healthy managed llama.cpp
/// setup consistent with the showcase conversation (the same model and
/// context the hero chat recommends).
pub fn app_config(theme: Theme) -> AppConfig {
    let mut c = AppConfig::default();
    c.interface.theme = theme;
    c.interface.language = Lang::En;
    c.engine.managed.binary = Some("C:\\llama.cpp\\llama-server.exe".into());
    c.engine.managed.model_path = Some("C:\\models\\gemma-4-12B-it-Q5_K_M.gguf".into());
    c.engine.managed.gpu_layers = 99;
    c.engine.managed.context_size = 16384;
    c
}

/// The chat list: the showcase chat on top (active), then a spread of
/// plausible topics with fixed dates and counts — enough rows to fill the
/// stage-2 frame without scrolling.
pub fn chat_summaries() -> Vec<ChatSummary> {
    // Fixture rows as data — one constructor call site, one row per line. A
    // repeated constructor block per chat reads the same but trips the
    // duplication detector (eight structurally identical multi-line blocks in
    // a row are a sliding self-duplicate), and rustfmt would reflow the rows
    // right back into that shape — hence the skip: this is a table, and the
    // row-per-line layout is the point.
    // Columns: title, message count, created (m,d,h,min), modified (m,d,h,min).
    type Row = (
        &'static str,
        usize,
        (u32, u32, u32, u32),
        (u32, u32, u32, u32),
    );
    #[rustfmt::skip]
    const ROWS: [Row; 8] = [
        ("Sampler settings for livelier replies",  18, (7, 29,  9, 12), (7, 30, 21, 40)),
        ("Speculative decoding: draft models",     12, (7, 29,  8,  0), (7, 29, 19,  5)),
        ("Refactoring a god object in Rust",       41, (7, 26, 14, 30), (7, 27, 17, 52)),
        ("What does the DRY penalty actually do?",  9, (7, 25, 11,  3), (7, 25, 12, 44)),
        ("Trip notes: the Dolomites in October",   26, (7, 20, 18, 15), (7, 22, 20, 31)),
        ("Reading list: attention papers",         15, (7, 17,  7, 45), (7, 19, 23, 10)),
        ("Backup dry run before the update",        7, (7, 18, 16, 20), (7, 18, 16, 58)),
        ("Mermaid diagrams in the terminal",       11, (7, 15, 13,  0), (7, 16, 10, 27)),
    ];
    let mut chats = vec![ChatSummary {
        id: chat_id(),
        title: CHAT_TITLE.into(),
        created_at: date(8, 1, 10, 0),
        modified_at: date(8, 1, 10, 3),
        message_count: 4,
    }];
    chats.extend(ROWS.iter().enumerate().map(|(i, (title, count, c, m))| {
        ChatSummary {
            // Slots continue the stage-2 numbering (1-based after the hero).
            id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_6c69_7374_0000_0000 + i as u128 + 1),
            title: (*title).into(),
            created_at: date(c.0, c.1, c.2, c.3),
            modified_at: date(m.0, m.1, m.2, m.3),
            message_count: *count,
        }
    }));
    chats
}

/// The self-model the `F3` capture shows: a companion that has already lived
/// a little — a voice, three goals in two states, a user model, and a few
/// narrative observations consistent with the showcase chat.
pub fn self_model() -> SelfModel {
    let goal = |slot: u128,
                text: &str,
                status: GoalStatus,
                created: DateTime<Utc>,
                closed: Option<DateTime<Utc>>| Goal {
        id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_676f_616c_0000_0000 + slot),
        description: text.into(),
        status,
        created_at: created,
        closed_at: closed,
    };
    let segment = |slot: u128, text: &str, created: DateTime<Utc>| NarrativeSegment {
        id: Uuid::from_u128(0x6d66_5f64_656d_6f5f_6e61_7272_0000_0000 + slot),
        text: text.into(),
        created_at: created,
    };
    SelfModel {
        profile_id: profile_id(),
        version: 7,
        summary: "I run locally and help with practical engineering — model \
                  sizing, Rust, the occasional trip plan. I prefer measured \
                  numbers to adjectives, and I write things down: advice \
                  should start from remembered facts, not fresh guesses."
            .into(),
        goals: vec![
            goal(
                1,
                "Answer sizing questions from saved notes alone — no re-asking about hardware.",
                GoalStatus::Active,
                date(7, 22, 12, 0),
                None,
            ),
            goal(
                2,
                "Keep recommendations reproducible: name the exact quant, context and flags.",
                GoalStatus::Active,
                date(7, 25, 9, 30),
                None,
            ),
            goal(
                3,
                "Index the llama.cpp server docs into the knowledge base.",
                GoalStatus::Completed,
                date(7, 16, 15, 0),
                Some(date(7, 19, 11, 20)),
            ),
        ],
        user_model: UserModel {
            perceived_traits: vec!["methodical".into(), "impatient with vague answers".into()],
            current_interests: vec!["local model tuning".into(), "terminal tooling".into()],
            relationship_dynamic: "Collaborative and direct; jokes land better after the numbers."
                .into(),
        },
        narrative: vec![
            segment(
                1,
                "Tables beat prose here: comparisons get read, paragraphs get skimmed.",
                date(7, 24, 19, 40),
            ),
            segment(
                2,
                "The 12 GB VRAM budget keeps coming up — saved it as a note so I stop re-asking.",
                date(7, 28, 10, 15),
            ),
            segment(
                3,
                "Showing the exact command I ran turns out to be the fastest way to build trust.",
                date(8, 1, 10, 3),
            ),
        ],
        updated_at: date(8, 1, 10, 3),
    }
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
