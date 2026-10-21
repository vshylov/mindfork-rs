//! Notifications for someone not looking (spec §11.12;
//! docs/research/companion-platform.md C5 (a)): when a companion with *Notify me*
//! on writes unasked — the reply of a woken turn — while the terminal is not in
//! focus, or a follow-up comes due in a conversation the person is not in, the
//! terminal gets an escape (and the bell) and a ntfy topic a push, as
//! `settings.json` → `notifications` says. The terminal's half is written by the
//! runtime ([`AppEvent::TerminalAttention`]); the push leaves from a task.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

use super::Orchestrator;
use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageRole};
use crate::shared::notify;

/// What the companion has for the person.
pub(super) enum Unasked<'a> {
    /// A woken turn's reply, landed in the open conversation.
    Reply(&'a str),
    /// A follow-up's notice, waiting in a conversation the person is not in.
    Waiting,
}

/// The text of a woken turn's reply among the rows it landed: its last
/// assistant message with words in it.
pub(super) fn woken_reply_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty())
        .map(|m| m.text.clone())
}

impl Orchestrator {
    /// `AppCommand::TerminalFocus`: the terminal reported its window gaining or
    /// losing focus (only terminals that report it do; until one does, the
    /// person is taken as not looking).
    pub(super) fn handle_terminal_focus(&mut self, focused: bool) {
        self.focused = Some(focused);
    }

    /// Sends a notification for `what` in `chat_id`, when its profile has *Notify
    /// me* on and the person is not looking: a reply in the open conversation
    /// while the terminal is not known to be in focus; a waiting follow-up
    /// whatever the focus — the person is elsewhere in the app, or away from it.
    pub(super) fn notify_unasked(&self, chat_id: Uuid, what: Unasked<'_>) {
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            return;
        };
        let Some(profile) = self.profiles.iter().find(|p| p.id == chat.profile_id) else {
            return;
        };
        if !profile.companion.notify {
            return;
        }
        let looking = self.focused == Some(true) && self.active_id == Some(chat_id);
        if matches!(what, Unasked::Reply(_)) && looking {
            return;
        }
        let cfg = &self.config.notifications;
        // The person reads it: the interface's language (axis B).
        let loc = self.ui_locale();
        let title = loc.tf("ui.notify.title", &[("name", &profile.name)]);
        let body = match what {
            Unasked::Reply(text) if cfg.with_text => text.to_string(),
            Unasked::Reply(_) => loc.tf("ui.notify.wrote", &[("chat", &chat.title)]),
            Unasked::Waiting => loc.tf("ui.notify.waiting", &[("chat", &chat.title)]),
        };
        let seq = notify::sequence(
            cfg.escape,
            cfg.bell,
            &title,
            &body,
            crate::shared::osc52::in_tmux(),
        );
        if !seq.is_empty() {
            let _ = self.evt_tx.send(AppEvent::TerminalAttention(seq));
        }
        let topic = cfg.ntfy_url.trim().to_string();
        if topic.is_empty() {
            return;
        }
        let token = crate::shared::secrets::stored_key(
            &self.config.api_keys,
            crate::shared::secrets::NTFY_TOKEN_KEY,
        );
        let failed_once: Arc<AtomicBool> = self.ntfy_failed_noted.clone();
        let evt_tx = self.evt_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = notify::ntfy_send(&topic, token.as_deref(), &title, &body).await {
                tracing::warn!(%err, "ntfy: the notification was not sent");
                // Said once a session: a topic that refuses refuses every time.
                if !failed_once.swap(true, Ordering::Relaxed) {
                    let _ = evt_tx.send(AppEvent::Notice(
                        loc.tf("ui.notify.ntfy_failed", &[("err", &err)]),
                    ));
                }
            }
        });
    }
}
