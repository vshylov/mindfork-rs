//! A follow-up the companion planned (spec §9.3.6, docs/research/companion-platform.md
//! C4 (a)): a return to one conversation, at a time, about something the person
//! said. `followup_plan` writes one, the orchestrator delivers it when it falls due
//! — a notice row in the chat and a woken turn — and the person sees and cancels
//! the pending ones on the tasks screen. Stored in SQLite (`followups`), never in
//! the chat file: a chat holds what was said, this holds what is still to come.

use chrono::{DateTime, Duration, NaiveDateTime, NaiveTime, Timelike, Utc};
use uuid::Uuid;

/// Where a follow-up is in its life. A pending one waits for its time; a
/// delivered one put its notice row in the chat; a cancelled one never will.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowupState {
    Pending,
    Delivered,
    Cancelled,
}

impl FollowupState {
    /// The word stored in the `state` column.
    pub fn key(self) -> &'static str {
        match self {
            FollowupState::Pending => "pending",
            FollowupState::Delivered => "delivered",
            FollowupState::Cancelled => "cancelled",
        }
    }

    /// The state a stored word names; an unknown word (a newer binary's) reads
    /// as cancelled, so this one never delivers what it does not understand.
    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => FollowupState::Pending,
            "delivered" => FollowupState::Delivered,
            _ => FollowupState::Cancelled,
        }
    }
}

/// One planned return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followup {
    pub id: Uuid,
    pub profile_id: Uuid,
    /// The conversation it returns to — the one it was planned in.
    pub chat_id: Uuid,
    /// What to come back to, in the model's own words.
    pub about: String,
    /// When it falls due (a whole minute).
    pub due_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub state: FollowupState,
    /// When it was delivered or cancelled; `None` while pending.
    pub closed_at: Option<DateTime<Utc>>,
}

impl Followup {
    /// A new pending follow-up of `chat_id`, due at `due_at`.
    pub fn new(
        profile_id: Uuid,
        chat_id: Uuid,
        about: impl Into<String>,
        due_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            profile_id,
            chat_id,
            about: about.into(),
            due_at,
            created_at: Utc::now(),
            state: FollowupState::Pending,
            closed_at: None,
        }
    }
}

/// The hours no follow-up is delivered in, on the local wall clock: from `from`
/// o'clock up to `until` o'clock, across midnight when `from > until`. Equal hours
/// mean none. On by default, 22 to 8 (the research's §7, rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub from: u8,
    pub until: u8,
}

impl QuietHours {
    /// Whether `t` falls inside the quiet hours.
    pub fn contains(self, t: NaiveTime) -> bool {
        let (from, until, h) = (u32::from(self.from), u32::from(self.until), t.hour());
        match from.cmp(&until) {
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Less => h >= from && h < until,
            std::cmp::Ordering::Greater => h >= from || h < until,
        }
    }

    /// The first moment at or after `t` outside the quiet hours.
    pub fn after(self, t: NaiveDateTime) -> NaiveDateTime {
        if !self.contains(t.time()) {
            return t;
        }
        let end = NaiveTime::from_hms_opt(u32::from(self.until) % 24, 0, 0).unwrap_or_default();
        let today = t.date().and_time(end);
        if today > t {
            today
        } else {
            today + Duration::days(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_round_trips_and_an_unknown_word_never_delivers() {
        for s in [
            FollowupState::Pending,
            FollowupState::Delivered,
            FollowupState::Cancelled,
        ] {
            assert_eq!(FollowupState::parse(s.key()), s);
        }
        assert_eq!(FollowupState::parse("snoozed"), FollowupState::Cancelled);
    }
}
