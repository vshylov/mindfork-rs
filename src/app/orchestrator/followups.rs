//! The companion's follow-ups in the orchestrator (spec §9.3.6;
//! docs/research/companion-platform.md C4 (a), §3.5): when the next one falls due,
//! delivering it as a notice row, and waking the companion on it.
//!
//! - **Scheduling.** `followup_plan` writes to storage during a turn; after every
//!   turn, at start, and on every change that can move one (a cancel, a deleted
//!   conversation, a profile's switch or limits), [`Orchestrator::refresh_followups`]
//!   reads the pending ones and keeps the earliest moment one may be delivered —
//!   due, outside its profile's quiet hours, and on a day under its cap. The loop
//!   sleeps until then, at most a minute at a time: a follow-up is due by the wall
//!   clock, and a monotonic timer does not run while the machine sleeps.
//! - **Delivery.** A due follow-up becomes a notice row in its conversation — the
//!   time now, when it was planned and what about, and when it was due if it is
//!   late — and is marked delivered, once: one due while the app was closed is
//!   delivered at the next start, never repeated. Several due in one conversation
//!   are rows of their own and one wake. A conversation the person is not in is
//!   marked unread. A turn running in the open conversation holds the delivery
//!   until it ends, so a notice never lands inside an exchange.
//! - **The wake.** The companion is woken on the open conversation whose last row
//!   is a follow-up's notice, when nothing is generating and the engine is ready:
//!   at the delivery, when the person opens that conversation (F5 (i) there), when
//!   the engine comes up, and when the turn that held it ends. Its `<silent>` —
//!   the choice the notice offers — leaves no trace (`handle_done`).

use chrono::{DateTime, Local, Utc};
use uuid::Uuid;

use super::Orchestrator;
use crate::entities::followup::{Followup, FollowupState};
use crate::entities::message::Message;
use crate::features::followups as rules;

/// The longest the loop sleeps while a follow-up is pending (see the module).
const RECHECK: std::time::Duration = std::time::Duration::from_secs(60);

/// A stored instant on the local wall clock.
fn local(at: DateTime<Utc>) -> chrono::NaiveDateTime {
    at.with_timezone(&Local).naive_local()
}

impl Orchestrator {
    /// When the loop should next look at the follow-ups; `None` while none can be
    /// delivered (none pending, or every one waits on a switch or a turn).
    pub(super) fn followup_deadline(&self) -> Option<tokio::time::Instant> {
        let next = self.followup_next?;
        let wait = (next - Utc::now())
            .to_std()
            .unwrap_or_default()
            .min(RECHECK);
        Some(tokio::time::Instant::now() + wait)
    }

    /// The loop's tick: delivers what is due, then works out the next moment.
    pub(super) fn tick_followups(&mut self) {
        self.deliver_due_followups();
        self.refresh_followups();
    }

    /// Re-reads the pending follow-ups and keeps the earliest moment one of them
    /// may be delivered.
    pub(super) fn refresh_followups(&mut self) {
        let pending = self.storage.db().followups_pending().unwrap_or_default();
        self.followup_next = pending
            .iter()
            .filter(|f| !self.followup_held(f))
            .filter_map(|f| self.followup_deliverable_at(f))
            .min();
    }

    /// When `f` may be delivered — `None` while it cannot be at all: its
    /// conversation is gone or hidden, or its profile has the switch off.
    fn followup_deliverable_at(&self, f: &Followup) -> Option<DateTime<Utc>> {
        let chat = self.chats.iter().find(|c| c.id == f.chat_id)?;
        let profile = self.profiles.iter().find(|p| p.id == chat.profile_id)?;
        if !profile.companion.followups {
            return None;
        }
        let db = self.storage.db();
        let at = rules::deliverable_at(
            local(f.due_at),
            profile.companion.quiet_hours(),
            profile.companion.followups_per_day,
            |day| {
                let (from, to) = rules::local_day_bounds(day);
                db.followups_delivered_between(profile.id, from, to)
                    .unwrap_or(0)
            },
        );
        Some(rules::local_to_utc(at))
    }

    /// A turn in the open conversation holds its follow-ups until it ends.
    fn followup_held(&self, f: &Followup) -> bool {
        self.active_id == Some(f.chat_id) && !self.gen_state.is_idle()
    }

    /// Delivers every follow-up whose moment has come.
    fn deliver_due_followups(&mut self) {
        let now = Utc::now();
        let Ok(pending) = self.storage.db().followups_pending() else {
            return;
        };
        let mut touched: Vec<Uuid> = Vec::new();
        for f in pending {
            let due = self.followup_deliverable_at(&f).is_some_and(|at| at <= now);
            if due
                && !self.followup_held(&f)
                && self.deliver_followup(&f, now)
                && !touched.contains(&f.chat_id)
            {
                touched.push(f.chat_id);
            }
        }
        if touched.is_empty() {
            return;
        }
        tracing::info!(chats = touched.len(), "follow-ups delivered");
        self.emit_chat_list();
        self.emit_task_list();
        // A conversation the person is not in waits for them: say so (spec §11.12).
        for chat in touched.iter().filter(|c| self.active_id != Some(**c)) {
            self.notify_unasked(*chat, super::notify::Unasked::Waiting);
        }
        if let Some(open) = self.active_id.filter(|id| touched.contains(id)) {
            // The open feed shows the row at once, then the companion is woken on it.
            self.activate(open);
            self.maybe_wake_followup(open);
        }
    }

    /// One delivery: the follow-up closed as delivered, and its notice appended to
    /// its conversation — in the profile's language, since the model reads it.
    fn deliver_followup(&mut self, f: &Followup, now: DateTime<Utc>) -> bool {
        let loc = self.profile_locale_of(f.chat_id);
        let text = rules::notice(
            local(now),
            local(f.created_at),
            local(f.due_at),
            &f.about,
            loc,
        );
        // Closed first: a follow-up cancelled in the same instant is not delivered.
        if !self
            .storage
            .db()
            .followup_close(f.id, FollowupState::Delivered, now)
            .unwrap_or(false)
        {
            return false;
        }
        let open = self.active_id == Some(f.chat_id);
        let Some(chat) = self.chat_mut(f.chat_id) else {
            return false;
        };
        chat.push_message(Message::followup_notice(f.id, text));
        if !open {
            chat.unread = true;
        }
        self.mark_dirty(f.chat_id);
        true
    }

    /// Wakes the companion on `chat_id` when it is the open conversation, nothing
    /// is generating, its last row is a follow-up's notice, and the engine is
    /// ready — quietly otherwise: the notice waits for the next of those moments,
    /// or travels in front of the person's next message.
    pub(super) fn maybe_wake_followup(&mut self, chat_id: Uuid) {
        if self.active_id != Some(chat_id) || !self.gen_state.is_idle() {
            return;
        }
        let waiting = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.messages.last())
            .is_some_and(Message::is_followup_notice);
        if !waiting {
            return;
        }
        let Ok(backend) = self.engines.backend_if_ready(self.ui_locale()) else {
            return;
        };
        self.start_woken_generation(chat_id, backend);
    }

    /// The person's cancel from the tasks screen (`AppCommand::CancelFollowup`).
    pub(super) fn handle_cancel_followup(&mut self, id: Uuid) {
        let _ = self
            .storage
            .db()
            .followup_close(id, FollowupState::Cancelled, Utc::now());
        self.refresh_followups();
        self.emit_task_list();
    }

    /// Cancels the conversation's pending follow-ups planned in an exchange that
    /// has just left it — taken back or regenerated: whatever was planned at or
    /// after its last remaining row (every one, when none remains).
    pub(super) fn cancel_orphaned_followups(&mut self, chat_id: Uuid) {
        let since = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.messages.last())
            .map(|m| m.timestamp);
        if self
            .storage
            .db()
            .followups_cancel_in_chat(chat_id, since)
            .unwrap_or(0)
            > 0
        {
            self.refresh_followups();
            self.emit_task_list();
        }
    }

    /// Cancels every pending follow-up of a conversation being deleted.
    pub(super) fn cancel_followups_of(&mut self, chat_id: Uuid) {
        let _ = self.storage.db().followups_cancel_in_chat(chat_id, None);
        self.refresh_followups();
    }

    /// Takes a woken turn's `<silent>` back out (spec §9.3.6): the reply is
    /// dropped, and when the turn left nothing else, the notices it answered go
    /// too — silence leaves no trace. Answers whether anything was dropped.
    pub(super) fn drop_silent_reply(&mut self, chat_id: Uuid, messages: &mut Vec<Message>) -> bool {
        let silent = messages.last().is_some_and(|m| {
            m.role == crate::entities::message::MessageRole::Assistant
                && m.tool_calls.is_empty()
                && rules::is_silent(&m.text)
        });
        if !silent {
            return false;
        }
        messages.pop();
        if messages.is_empty()
            && let Some(chat) = self.chat_mut(chat_id)
        {
            while chat
                .messages
                .last()
                .is_some_and(Message::is_followup_notice)
            {
                chat.messages.pop();
            }
        }
        tracing::info!(%chat_id, "a woken turn chose silence");
        true
    }
}
