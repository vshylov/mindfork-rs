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

/// What kind of run a record carries. `Subagent` today; the director-led
/// dialogue of two personas is the planned second kind (research §3.14) — the
/// list, the read-only screen and the deletion rule key off "this record has a
/// run", never off the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Subagent,
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
    /// replaces it, like any chat's.
    pub system_message: String,
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
}

impl SubagentRun {
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
