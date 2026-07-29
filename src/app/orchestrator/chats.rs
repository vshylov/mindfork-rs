//! Managing the chat list: creation, switching, renaming, cloning,
//! copying the conversation, deletion, and saving the draft.

use uuid::Uuid;

use crate::app::events::AppEvent;

use super::Orchestrator;

impl Orchestrator {
    /// Saves the input-box draft on the active chat (unsaved text). The write to
    /// disk is debounced (`mark_dirty`); `modified_at` is NOT touched — editing a
    /// draft shouldn't bump the chat up the list. See spec §11.7.
    pub(super) fn handle_set_draft(&mut self, text: String) {
        let Some(active_id) = self.active_id else {
            return;
        };
        if let Some(chat) = self.chat_mut(active_id) {
            if chat.draft == text {
                return;
            }
            chat.draft = text;
            self.mark_dirty(active_id);
        }
    }

    pub(super) fn handle_new_chat(&mut self, profile_id: Option<Uuid>) {
        let chat = self.new_chat_value(profile_id);
        let id = chat.id;
        if let Err(err) = self.storage.json().save_chat(&chat) {
            let _ = self.evt_tx.send(AppEvent::Error(
                self.ui_locale()
                    .tf("ui.err.chat_create_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.chats.insert(0, chat);
        self.emit_chat_list();
        self.activate(id);
    }

    pub(super) fn handle_switch(&mut self, id: Uuid) {
        self.switch_to(id, None);
    }

    /// Opens a chat **on a specific message** (a jump from a search hit). Same
    /// path as a plain switch — the focus rides along to the feed through
    /// `ChatActivated`. See docs/history/chat-search-stage2.md §3.
    pub(super) fn handle_open_chat_at(&mut self, chat: Uuid, message: Uuid) {
        self.switch_to(chat, Some(message));
    }

    /// Opens a chat on its **first message matching `query`** (`Enter` in the
    /// chat list's content mode). Resolving the match needs both the index and
    /// the chat, so it happens here rather than in the widget; when nothing
    /// resolves the chat opens at its tail — a plain switch, not an error.
    pub(super) fn handle_open_chat_at_first_match(&mut self, chat: Uuid, query: &str) {
        let focus = self.first_match_in_chat(chat, query);
        self.switch_to(chat, focus);
    }

    /// The shared switch path. `focus` — a message to put the feed on.
    fn switch_to(&mut self, id: Uuid, focus: Option<Uuid>) {
        if self.active_id == Some(id) {
            // A plain switch to the open chat is a no-op, but a jump still has
            // to move the feed — re-emit so the focus reaches it.
            if focus.is_some() {
                self.activate_focused(id, focus);
            }
            return;
        }
        // Speech is stopped per the setting (on by default — otherwise you'd
        // suddenly be listening to a different chat). See spec §11.9.
        if self.config.tts.stop_on_chat_switch {
            self.stop_tts();
        }
        // If generation is running — cancel it (the partial reply is saved for
        // the original chat when the GenResult arrives).
        if let Some(token) = self.gen_state.request_cancel() {
            token.cancel();
        }
        if self.chats.iter().any(|c| c.id == id) {
            self.activate_focused(id, focus);
        }
    }

    pub(super) fn handle_rename(&mut self, id: Uuid, title: String) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        if let Some(chat) = self.chat_mut(id) {
            chat.title = title.clone();
            self.mark_dirty(id);
            self.emit_chat_list();
            let _ = self.evt_tx.send(AppEvent::ChatRenamed { id, title });
        }
    }

    pub(super) fn handle_clone(&mut self, id: Uuid) {
        let Some(src) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let now = chrono::Utc::now();
        let mut clone = src.clone();
        clone.id = Uuid::new_v4();
        clone.title = self
            .ui_locale()
            .tf("ui.chat.clone_suffix", &[("orig", &src.title)]);
        clone.created_at = now;
        clone.modified_at = now;
        let new_id = clone.id;
        if let Err(err) = self.storage.json().save_chat(&clone) {
            let _ = self.evt_tx.send(AppEvent::ChatListError(
                self.ui_locale()
                    .tf("ui.err.chat_clone_failed", &[("err", &err.to_string())]),
            ));
            return;
        }
        self.chats.insert(0, clone);
        self.emit_chat_list();
        self.activate(new_id);
    }

    /// Copies the whole chat conversation to the clipboard (spec §11.2): the orchestrator
    /// (the owner of `Chat`) builds the text and emits `CopyToClipboard` — writing to the
    /// clipboard and the confirmation are done by the UI layer (`runtime`). An empty chat → a clear error.
    pub(super) fn handle_copy_chat(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        // Role labels come from the chat's profile (spec §5.1): a custom name replaces
        // the localized "User:"/"Assistant:".
        let names = self.active_character_names(chat);
        match crate::features::chat_export::format_conversation(
            &chat.title,
            &chat.messages,
            &self.config.copy,
            &names,
            self.ui_locale(),
        ) {
            Some(text) => {
                let _ = self.evt_tx.send(AppEvent::CopyToClipboard(text));
            }
            None => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(
                    self.ui_locale().t("ui.err.nothing_to_copy").into(),
                ));
            }
        }
    }

    pub(super) fn handle_delete(&mut self, id: Uuid) {
        // Unconditional (not a setting): the chat being spoken is about to disappear.
        if self.active_id == Some(id) {
            self.stop_tts();
        }
        match self.storage.json().hide_chat(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(
                    self.ui_locale()
                        .tf("ui.err.chat_delete_failed", &[("err", &err.to_string())]),
                ));
                return;
            }
            Ok(true) => {}
        }
        self.chats.retain(|c| c.id != id);
        self.saves.forget(id);
        // A hidden chat never appears in the list, so it must not appear in
        // content-search results either (see [`super::search`]).
        self.forget_chat_index(id);

        // If the active one was deleted — pick another (or create a new one).
        if self.active_id == Some(id) {
            self.active_id = None;
            if let Some(next) = self.chats.first().map(|c| c.id) {
                self.emit_chat_list();
                self.activate(next);
            } else {
                self.handle_new_chat(None);
            }
        } else {
            self.emit_chat_list();
        }
    }
}
