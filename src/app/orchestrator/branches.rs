//! Forks and switches of the open chat's line
//! (docs/research/chat-branches.md §3.3–§3.4, spec §11.7).
//!
//! The two operations are the chat's own ([`crate::entities::chat_branch`]);
//! what is here is everything around them: the conditions a take-back checks,
//! the answers a refusal owes, and what the rest of the app is told.
//!
//! **To everything outside the chat file, a fork and a switch are the
//! truncation it already handles** — followed, for a switch, by an append. So
//! both go through the calls `Ctrl+E` and `Ctrl+R` make
//! ([`Orchestrator::after_line_moved`]): the feed is rebuilt, a background run
//! whose record left the line is cancelled, a pending follow-up planned in
//! what left is cancelled. Nothing here is a filter added for branches: the
//! request, the tools' snapshots and the full-text index read
//! `chat.messages`, which is the line.

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::visible_message_count;
use crate::entities::chat_branch::SwitchError;

use super::Orchestrator;

/// What a fork came to, decided while the chat is borrowed.
enum Forked {
    /// The line shows no such message any more.
    Gone,
    /// The message is the line's last: nothing follows it to fork from.
    NothingAfter,
    /// Done; the person's own message, when the fork was at one.
    Done { restore: Option<String> },
}

impl Orchestrator {
    /// `/fork N` (research §3.6): the conversation continues from the bubble
    /// that shows `message`, and what followed is parked as a branch. The
    /// screen resolved N to the message its numbered bubble carries, so what
    /// the person saw numbered is what is acted on. Ignored during generation
    /// and on a transcript, like `/takeback`; the screen answers those.
    pub(super) fn handle_fork_at(&mut self, message: Uuid) {
        if !self.gen_state.is_idle() || self.seat_holds_open_chat() {
            return;
        }
        let Some(active_id) = self.active_id else {
            return;
        };
        let now = chrono::Utc::now();
        let forked = {
            // A transcript's id is not in the list: it fails closed here,
            // which is the read-only behaviour by construction.
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            match chat.fork_cut(message) {
                None => Forked::Gone,
                Some(cut) if chat.fork_at(cut.cut, now) => Forked::Done {
                    restore: cut.restore,
                },
                Some(_) => Forked::NothingAfter,
            }
        };
        match forked {
            Forked::Gone => self.notice("ui.branch.message_gone"),
            Forked::NothingAfter => self.notice("ui.branch.nothing_after"),
            Forked::Done { restore } => {
                // Unconditional (not a setting): what is being spoken may have
                // just left the line.
                self.stop_tts();
                self.after_line_moved(active_id);
                if let Some(text) = restore {
                    let _ = self.evt_tx.send(AppEvent::RestoreInput(text));
                }
            }
        }
    }

    /// `/branch N K` and a click on a variants mark (research §3.6–§3.7): the
    /// variant that starts with `first` goes on the line, and what it
    /// displaces is parked. Ignored where a fork is.
    pub(super) fn handle_switch_branch(&mut self, first: Uuid) {
        if !self.gen_state.is_idle() || self.seat_holds_open_chat() {
            return;
        }
        let Some(active_id) = self.active_id else {
            return;
        };
        let now = chrono::Utc::now();
        let switched = {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.switch_to(first, now).map(|()| {
                // Where the variant stands now, in the line's own numbers.
                let at = chat
                    .messages
                    .iter()
                    .position(|m| m.id == first)
                    .unwrap_or_default();
                let number = visible_message_count(&chat.messages[..at]) + 1;
                let mark = chat
                    .branch_view()
                    .marks
                    .into_iter()
                    .find(|m| m.message == first);
                (number, mark)
            })
        };
        match switched {
            Err(SwitchError::Unknown) => self.notice("ui.branch.variant_gone"),
            Err(SwitchError::Detached) => self.notice("ui.branch.detached"),
            Ok((number, mark)) => {
                self.stop_tts();
                self.after_line_moved(active_id);
                let loc = self.ui_locale();
                let number = number.to_string();
                let text = match mark {
                    Some(mark) => loc.tf(
                        "ui.branch.switched",
                        &[
                            ("number", number.as_str()),
                            ("variant", &mark.variant.to_string()),
                            ("of", &mark.of.to_string()),
                        ],
                    ),
                    // Nothing was displaced and nothing else continues from
                    // there: the place has one variant again.
                    None => loc.tf("ui.branch.returned", &[("number", number.as_str())]),
                };
                let _ = self.evt_tx.send(AppEvent::Notice(text));
            }
        }
    }

    /// What the rest of the app is told when the line's tail changed under a
    /// fork or a switch — the calls a take-back makes, in its order.
    fn after_line_moved(&mut self, chat_id: Uuid) {
        self.mark_dirty(chat_id);
        // Rebuilds the feed, and sends the chat's branches with it.
        self.activate(chat_id);
        self.emit_chat_list();
        // A background run whose exchange just left the line ends with it
        // (research F5); it lands on the parked copy of its record.
        self.cancel_orphaned_background_runs(chat_id);
        self.cancel_orphaned_followups(chat_id);
    }

    /// The open conversation's branches, for the screen's marks and lists
    /// (research §3.7). Sent with every activation: a transcript has none, and
    /// says so, so the previous chat's do not linger.
    pub(super) fn emit_branches(&self) {
        let view = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|chat| chat.branch_view())
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::Branches(Box::new(view)));
    }

    fn notice(&self, key: &str) {
        let _ = self
            .evt_tx
            .send(AppEvent::Notice(self.ui_locale().t(key).to_string()));
    }
}
