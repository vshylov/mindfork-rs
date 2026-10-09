//! The generation in flight on a conversation being activated
//! (docs/history/subagent-live.md §3.4–§3.5, §8): which turn and which stream
//! the feed resumes, and the round so far — text, thoughts, the tool calls it
//! has opened — so a feed opened mid-round starts with what has already
//! streamed. Plain data the orchestrator snapshots and the chat screen seeds
//! itself from; defined here, below both, and re-exported by `app::events`
//! beside `AppEvent::ChatActivated` (docs/architecture.md §2,
//! docs/research/fsd-layer-gate.md).

use uuid::Uuid;

use crate::entities::message::MessageRole;

/// The generation running on the conversation being activated
/// ([`AppEvent::ChatActivated::live_turn`](crate::app::events::AppEvent::ChatActivated)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTurn {
    /// The turn's generation id — what the sub-agent chip is keyed on.
    pub turn: Uuid,
    /// The stream this conversation's feed accepts: the turn's own on its
    /// chat, the sub-agent's own on its transcript (docs/history/subagent-live.md §8).
    pub stream: Uuid,
    /// The round in progress so far — text and thoughts — so a feed opened
    /// mid-round starts with what has already streamed.
    pub partial: Option<LivePartial>,
    /// The round in progress **continues** the feed's last assistant bubble
    /// (`/continue` before its first tool round): the partial is appended
    /// there, with no separator, instead of opening a bubble of its own.
    pub continues: bool,
    /// Which side of the conversation the round in progress streams into:
    /// `Assistant` for a turn's own chat and a sub-agent's transcript; a
    /// dialogue's line carries its speaker's side (spec §9.13 — participant
    /// `b`'s lines are the transcript's `User` role).
    pub role: MessageRole,
    /// The conversation is the transcript of a run out in the **background**
    /// (spec §9.3.2, docs/research/background-subagents.md §4.5): its stream
    /// is no turn's, so on it `Esc` goes back instead of cancelling and `F6`
    /// stops the run. `false` on a turn's own chat and on a turn child's
    /// transcript, where `Esc` cancels the turn.
    pub background: bool,
}

/// A round in progress (see [`LiveTurn::partial`]): its text and thoughts
/// so far, and the tool calls it has opened — running or already answered —
/// which are not in any filed message yet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LivePartial {
    pub text: String,
    pub thoughts: String,
    pub tools: Vec<LiveTool>,
}

/// One tool call of a round in progress (see [`LivePartial::tools`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTool {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    /// `None` while the call is running.
    pub result: Option<(String, usize)>,
}
