//! Managing the chat list: creation, switching, renaming, cloning,
//! copying the conversation, exporting it to a file, deletion, and saving the
//! draft.

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;

use crate::app::events::{AppEvent, FeedFocus};
use crate::entities::chat::FeedView;
use crate::features::export_command::ExportFormat;

use super::Orchestrator;

/// A path as the user should see it: absolute where that can be worked out, and
/// the path as given when it cannot (`canonicalize` needs the file to exist, so
/// this is called *after* the write — and on the failure paths it falls back
/// rather than hiding the name).
fn display_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .map(|p| {
            // Windows' canonical form carries the `\\?\` verbatim prefix, which
            // is correct and unreadable; the user is going to paste this
            // somewhere.
            let s = p.display().to_string();
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
        })
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(path).display().to_string())
                .unwrap_or_else(|_| path.display().to_string())
        })
}

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

    /// Saves the feed's collapse state on the active chat (`Ctrl+T`/`Ctrl+O`).
    /// Same rules as the draft above: debounced write, `modified_at` untouched —
    /// folding a block away isn't a change to the conversation. See spec §11.3.
    pub(super) fn handle_set_feed_view(&mut self, view: FeedView) {
        let Some(active_id) = self.active_id else {
            return;
        };
        if let Some(chat) = self.chat_mut(active_id) {
            if chat.feed_view == view {
                return;
            }
            chat.feed_view = view;
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
    /// `ChatActivated`, carrying the query so the feed can highlight it inside
    /// that message. See docs/history/chat-search-stage2.md §3 and §4a S3(b).
    pub(super) fn handle_open_chat_at(&mut self, chat: Uuid, message: Uuid, query: String) {
        self.switch_to(chat, Some(FeedFocus { message, query }));
    }

    /// Opens a chat on its **first message matching `query`** (`Enter` in the
    /// chat list's content mode). Resolving the match needs both the index and
    /// the chat, so it happens here rather than in the widget; when nothing
    /// resolves the chat opens at its tail — a plain switch, not an error.
    pub(super) fn handle_open_chat_at_first_match(&mut self, chat: Uuid, query: &str) {
        let focus = self
            .first_match_in_chat(chat, query)
            .map(|message| FeedFocus {
                message,
                query: query.to_string(),
            });
        self.switch_to(chat, focus);
    }

    /// The shared switch path. `focus` — a message to put the feed on, plus the
    /// query to highlight inside it.
    fn switch_to(&mut self, id: Uuid, focus: Option<FeedFocus>) {
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

    /// Writes a chat to a file (`/export`, docs/history/chat-export-file.md).
    ///
    /// The orchestrator finishes this one itself rather than handing content
    /// back to the UI: it owns `Chat`, it already does disk I/O, and the answer
    /// the user needs is a **path**. A relative path — or the generated name a
    /// bare `/export` gets — resolves against the process's current working
    /// directory (fork F3), which is where the user launched the app.
    pub(super) fn handle_export_chat(
        &mut self,
        id: Uuid,
        format: ExportFormat,
        path: Option<&str>,
    ) {
        let ui = self.ui_locale();
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let content = match format {
            ExportFormat::Markdown => {
                let names = self.active_character_names(chat);
                match crate::features::chat_export::format_conversation(
                    &chat.title,
                    &chat.messages,
                    &self.config.copy,
                    &names,
                    ui,
                ) {
                    Some(text) => text,
                    // The same refusal the clipboard gives for an empty chat:
                    // writing a file with nothing in it would be worse.
                    None => {
                        let _ = self
                            .evt_tx
                            .send(AppEvent::Error(ui.t("ui.err.nothing_to_copy").into()));
                        return;
                    }
                }
            }
            ExportFormat::Json => {
                // The format requires the chat's profile in the same file.
                let Some(profile) = self.profiles.iter().find(|p| p.id == chat.profile_id) else {
                    let _ = self
                        .evt_tx
                        .send(AppEvent::Error(ui.t("ui.export.err.no_profile").into()));
                    return;
                };
                let doc = crate::features::chat_export::to_import_json(chat, profile);
                match serde_json::to_string_pretty(&doc) {
                    Ok(text) => text,
                    Err(err) => {
                        let _ = self.evt_tx.send(AppEvent::Error(
                            ui.tf("ui.export.err.failed", &[("err", &err.to_string())]),
                        ));
                        return;
                    }
                }
            }
        };

        let target = match path {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(crate::features::chat_export::export_filename(
                &chat.title,
                format.extension(),
                Utc::now(),
            )),
        };
        // Refuse an existing file (fork F5): overwriting somebody's export
        // silently is the kind of loss this project avoids everywhere else.
        if target.exists() {
            let _ = self.evt_tx.send(AppEvent::Error(
                ui.tf("ui.export.err.exists", &[("path", &display_path(&target))]),
            ));
            return;
        }
        if let Err(err) = std::fs::write(&target, content) {
            let _ = self.evt_tx.send(AppEvent::Error(ui.tf(
                "ui.export.err.write",
                &[("path", &display_path(&target)), ("err", &err.to_string())],
            )));
            return;
        }
        // The absolute path, because a relative one answers "where?" with the
        // question again — and finding the file is the whole point when the
        // terminal is on another machine.
        let note = match format {
            ExportFormat::Markdown => ui.tf("ui.export.done", &[("path", &display_path(&target))]),
            // Said on every JSON export, not buried in the docs: the format has
            // nowhere to put tool calls, and noticing that later is worse.
            ExportFormat::Json => ui.tf("ui.export.done_json", &[("path", &display_path(&target))]),
        };
        let _ = self.evt_tx.send(AppEvent::Notice(note));
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
        // Staging is keyed by chat, so a deleted chat's slot has to go with it —
        // otherwise its images would be carried into whatever the screen shows next.
        self.forget_staged_images(id);
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
