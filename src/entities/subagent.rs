//! A sub-agent run: the transcript of one `call_subagent` call, stored **on
//! the tool-call record that made it** inside the parent chat's file
//! (docs/research/subagent-chats.md §3.1, spec §9.3.2). It looks like a chat
//! of its own — a persona, a conversation, a title — and is inseparable from
//! the parent by construction: it has no file, no profile, no hidden flag, and
//! it leaves the list exactly when the exchange that spawned it is taken back
//! or regenerated, because it travels with that exchange's messages.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::message::{Message, MessageRole};
use crate::entities::sampling::SamplingConfig;

/// What kind of run a record carries. The list, the read-only screen and the
/// deletion rule key off "this record has a run", never off the kind; what
/// *does* key off it is the feed's role labels and the transcript's system
/// bubble (spec §9.13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// One `call_subagent` delegation (spec §9.3.2).
    Subagent,
    /// A director-led dialogue of two personas (`run_dialogue`, spec §9.13,
    /// docs/research/two-agent-dialogue.md). Writing this variant is what the
    /// `CHAT_SCHEMA` 2→3 bump exists for: an older binary fails to parse it,
    /// and the version guard turns that into a polite refusal.
    Dialogue,
}

/// One of a dialogue run's two speakers (spec §9.13): the display name the
/// feed labels their side with, and the persona the caller composed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    /// The display name (the call's optional `name`); `None` → a localized
    /// fallback label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The persona's system message, exactly as the caller wrote it
    /// (director notes are appended at request-derivation time, never here).
    pub system_message: String,
}

/// How a run ended. `None` on the record means the run never reported — the
/// turn was interrupted before it could, which reads the same as a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// The sub-agent answered.
    Completed,
    /// The turn was cancelled (`Esc`, a chat switch, quit) while it ran.
    Cancelled,
    /// The whole-run time limit (`tools.subagent_run_timeout_secs`) ended it.
    TimedOut,
    /// The engine failed on one of its rounds.
    Failed,
    /// The round budget ran out; the last message is the "sum up" round.
    RoundLimit,
}

/// One sub-agent run. See the module doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentRun {
    /// Identity for `chat://` references, the list, search and activation —
    /// never a file name.
    pub id: Uuid,
    pub kind: RunKind,
    /// The list row's title: the persona's name or the first line of the
    /// instruction until a person or the model names it (spec §11.2).
    pub title: String,
    /// The title was chosen by the user — the automatic titling never touches
    /// it (the rule [`crate::entities::chat::Chat::renamed_manually`] states).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub renamed_manually: bool,
    /// The persona's display name (the call's optional `name` argument). The
    /// feed labels the sub-agent's replies with it; `None` → the localized label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// The persona the parent composed; `set_system_message` inside the run
    /// replaces it, like any chat's. For a dialogue run this holds the
    /// **director's resolved brief** — the personas live in `participants`.
    pub system_message: String,
    /// A dialogue run's two speakers, in order: `[0]` is stored as the
    /// `Assistant` role of `messages`, `[1]` as `User` (research §3.5, fork
    /// F2 — the role-encoded transcript). Empty for a sub-agent run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<Participant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_override: Option<SamplingConfig>,
    /// `User(instruction)`, then the run's rounds exactly as any chat stores
    /// them: assistant messages with their own tool records, tool messages,
    /// thoughts. Never a record with a run of its own — there is no nesting.
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// Completion tokens the run cost, for the card and the list.
    #[serde(default, skip_serializing_if = "crate::entities::message::is_zero_u64")]
    pub tokens: u64,
    /// The run was started in the **background** (`start_subagent`, spec
    /// §9.3.2, docs/research/background-subagents.md §4.2): its record landed
    /// with the parent's turn as a placeholder and was filled in when the run
    /// ended. With `outcome: None` on a stored record this reads as
    /// *unfinished* — the app was closed or crashed while the run was out —
    /// never as running, which only the orchestrator's mirror can say.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

impl SubagentRun {
    /// A finished run for tests: `User(texts[0])`, then the rest alternating
    /// assistant/user, so the fixture has text at both roles to index and
    /// render. Shared by the search, list and tool tests.
    #[cfg(test)]
    pub fn fixture(title: &str, texts: &[&str]) -> Self {
        let messages = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i % 2 == 0 {
                    Message::user(*t)
                } else {
                    Message::assistant(*t)
                }
            })
            .collect();
        Self {
            id: Uuid::new_v4(),
            kind: RunKind::Subagent,
            title: title.into(),
            renamed_manually: false,
            name: Some(title.into()),
            created_at: Utc::now(),
            finished_at: Some(Utc::now()),
            system_message: "persona".into(),
            sampling_override: None,
            messages,
            outcome: Some(RunOutcome::Completed),
            tokens: 0,
            participants: Vec::new(),
            background: false,
        }
    }

    /// A `call_subagent` record carrying this run — the shape a landed call
    /// leaves on the parent's assistant message.
    #[cfg(test)]
    pub fn on_record(self) -> crate::entities::message::ToolCallRecord {
        let bare = r#"{"id":"c1","name":"call_subagent","arguments":{"message":"x"},"result":"y"}"#;
        let mut rec: crate::entities::message::ToolCallRecord = serde_json::from_str(bare).unwrap();
        rec.subagent = Some(Box::new(self));
        rec
    }

    /// The sub-agent's final answer — the text of its last substantive
    /// assistant message — or `None` when it never wrote one.
    pub fn final_reply(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty())
            .map(|m| m.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::{Message, ToolCallRecord};

    fn run() -> SubagentRun {
        SubagentRun {
            id: Uuid::new_v4(),
            kind: RunKind::Subagent,
            title: "Критик".into(),
            renamed_manually: false,
            name: Some("Критик".into()),
            created_at: Utc::now(),
            finished_at: Some(Utc::now()),
            system_message: "Ты — критик.".into(),
            sampling_override: None,
            messages: vec![Message::user("Оцени X."), Message::assistant("X слаб.")],
            outcome: Some(RunOutcome::Completed),
            tokens: 12,
            participants: Vec::new(),
            background: false,
        }
    }

    #[test]
    fn final_reply_is_the_last_substantive_assistant_text() {
        let mut r = run();
        assert_eq!(r.final_reply(), Some("X слаб."));
        r.messages.push(Message::assistant("   "));
        assert_eq!(r.final_reply(), Some("X слаб."));
        r.messages.clear();
        assert_eq!(r.final_reply(), None);
    }

    /// The additive contract (ADR 0006 F12): a record written before runs
    /// existed reads with `None`, a record without a run writes no new key,
    /// and a run round-trips whole — inside a message, which is where it lives.
    #[test]
    fn run_on_a_record_is_additive_and_round_trips() {
        let old = r#"{"id":"c1","name":"call_subagent","arguments":{"message":"x"},"result":"y"}"#;
        let rec: ToolCallRecord = serde_json::from_str(old).unwrap();
        assert!(rec.subagent.is_none());
        let json = serde_json::to_string(&rec).unwrap();
        assert!(!json.contains("\"subagent\""), "{json}");

        let mut with = rec;
        with.subagent = Some(Box::new(run()));
        let mut msg = Message::assistant("");
        msg.tool_calls = vec![with];
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
        let run = back.tool_calls[0].subagent.as_ref().unwrap();
        assert_eq!(run.messages.len(), 2);
        assert_eq!(run.outcome, Some(RunOutcome::Completed));
    }
}
