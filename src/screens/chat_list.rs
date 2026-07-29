//! Chat-list screen (FSD "page"): a full-screen list with search, sorting,
//! in-place renaming, create/clone/copy/delete.
//! Opened from the chat via `Esc`, closed via `Esc`. See spec §11.2.
//!
//! A thin wrapper over the [`ChatListState`] widget: holds the render context
//! (the active chat — for the marker, the theme palette) and translates the widget's
//! [`ChatListAction`] into a [`ChatListIntent`] — an intent that `app` translates into an
//! `AppCommand` or into screen management. The screen doesn't know about `app`/channels
//! (FSD, dependencies flow downward) — like `ChatScreen`/`SettingsScreen`.

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::features::spellcheck::SpellChecker;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::widgets::chat_list::{ChatListAction, ChatListState};

/// Intent from the chat-list screen (translated by `app`). A counterpart to
/// [`ChatIntent`](crate::screens::chat::ChatIntent)/`SettingsIntent`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatListIntent {
    /// Close the list, return to the chat (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Make a chat active and return to it (`Enter`).
    Switch(Uuid),
    /// Create a new chat (profile selection is delegated to the chat screen) (`Ctrl+N`).
    NewChat,
    /// Clone a chat and return to it (`Ctrl+D`).
    Clone(Uuid),
    /// Copy a chat's conversation to the clipboard (the list stays open) (`F5`).
    Copy(Uuid),
    /// Soft-delete a chat (the list stays open) (`Del`).
    Delete(Uuid),
    /// Rename a chat (the list stays open) (`F2`).
    Rename { id: Uuid, title: String },
    /// Auto-title a chat via the model (the list stays open) (`Ctrl+R`).
    AutoRename(Uuid),
    /// Run a full-text search over chat content with this raw query (content
    /// mode, `Ctrl+F`; the list stays open). The result arrives as
    /// `AppEvent::ChatSearchResults` → [`ChatListScreen::set_search_results`].
    /// See docs/research/chat-content-search.md.
    SearchContent(String),
    /// Open the message-level search screen for this raw query (`Ctrl+G` in
    /// content mode). The result arrives as `AppEvent::MessageSearchResults`,
    /// which opens the screen. See docs/chat-search-stage2.md.
    SearchMessages(String),
    /// Open a chat at its first message matching the query (`Enter` in content
    /// mode); the orchestrator resolves which message that is.
    OpenFirstMatch { chat: Uuid, query: String },
}

/// Chat-list screen: widget state + render context.
pub struct ChatListScreen {
    state: ChatListState,
    /// Active chat (the `●` marker in the list). Updated on `ChatActivated`.
    active: Option<Uuid>,
    /// Theme palette for rendering (updated on `Settings`).
    palette: Palette,
    /// Interface locale (updated on `Settings`). See docs/i18n-ui.md.
    loc: &'static Locale,
    /// Waiting for the just-created chat's activation (`Ctrl+N`): the list stays on
    /// screen until its `ChatActivated` arrives, so as not to flash the previous chat
    /// before the new one. Cleared in `take_pending_new_chat`. See spec §11.2.
    pending_new_chat: bool,
}

impl ChatListScreen {
    /// Opens the screen with a snapshot of the list; the selection is on the active chat.
    pub fn new(
        chats: Vec<ChatSummary>,
        active: Option<Uuid>,
        palette: Palette,
        loc: &'static Locale,
    ) -> Self {
        Self {
            state: ChatListState::new(chats, active),
            active,
            palette,
            loc,
            pending_new_chat: false,
        }
    }

    /// Marks that a new chat was created, waiting for its `ChatActivated`, so we can
    /// switch to it atomically (without showing the previous chat). See `dispatch_chat_list`.
    pub fn set_pending_new_chat(&mut self) {
        self.pending_new_chat = true;
    }

    /// Takes the "waiting for a new chat" flag (resetting it). `true` means the
    /// activation that just arrived belongs to the just-created chat — time to close the list.
    pub fn take_pending_new_chat(&mut self) -> bool {
        std::mem::take(&mut self.pending_new_chat)
    }

    /// Updates the list snapshot (after the chat set changes) — the
    /// `AppEvent::ChatList` event. Keeps the selection on the same chat where possible.
    pub fn set_chats(&mut self, chats: Vec<ChatSummary>) {
        self.state.set_chats(chats);
    }

    /// Updates the active-chat marker (the `AppEvent::ChatActivated` event, e.g.
    /// after deleting the active chat while the list is open).
    pub fn set_active(&mut self, active: Option<Uuid>) {
        self.active = active;
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    /// Shows a list-operation error in its status area (fades on keypress).
    pub fn set_error(&mut self, message: String) {
        self.state.set_error(message);
    }

    /// Shows an operation confirmation (success) in its status area.
    pub fn set_notice(&mut self, message: String) {
        self.state.set_notice(message);
    }

    /// Handles a keypress, returning an intent for `app` (or `None` if the
    /// key was handled internally: navigation, search/rename input).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatListIntent> {
        match self.state.on_key(key) {
            ChatListAction::None => None,
            ChatListAction::Close => Some(ChatListIntent::Close),
            ChatListAction::Quit => Some(ChatListIntent::Quit),
            ChatListAction::Switch(id) => Some(ChatListIntent::Switch(id)),
            ChatListAction::New => Some(ChatListIntent::NewChat),
            ChatListAction::Clone(id) => Some(ChatListIntent::Clone(id)),
            ChatListAction::Copy(id) => Some(ChatListIntent::Copy(id)),
            ChatListAction::Delete(id) => Some(ChatListIntent::Delete(id)),
            ChatListAction::Rename { id, title } => Some(ChatListIntent::Rename { id, title }),
            ChatListAction::AutoRename(id) => Some(ChatListIntent::AutoRename(id)),
            ChatListAction::SearchContent(query) => Some(ChatListIntent::SearchContent(query)),
            ChatListAction::SearchMessages(query) => Some(ChatListIntent::SearchMessages(query)),
            ChatListAction::OpenFirstMatch { chat, query } => {
                Some(ChatListIntent::OpenFirstMatch { chat, query })
            }
        }
    }

    /// Reopens the list still searching message content for `query` — used when
    /// the message-level results screen closes (`Esc`), so the user lands back
    /// on the search they were doing.
    pub fn restore_content_query(&mut self, query: String) {
        self.state.restore_content_query(query);
    }

    /// Applies a content-search result (the `AppEvent::ChatSearchResults`
    /// event). `chat_ids: None` — not a searchable query, don't filter.
    pub fn set_search_results(&mut self, query: String, chat_ids: Option<Vec<Uuid>>) {
        self.state.set_search_results(query, chat_ids);
    }

    /// Inserts clipboard text into the rename field (if open).
    /// Outside rename mode — a no-op. See spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        self.state.handle_paste(text);
    }

    /// Rechecks spelling in the rename field (if open and changed).
    /// Returns `true` if the highlighting was updated (a redraw is needed). The checker
    /// is lent from the chat screen — its owner (`app` reconciles this in the loop).
    pub fn recheck_spelling(&mut self, spell: &SpellChecker) -> bool {
        self.state.recheck_rename_spelling(spell)
    }

    /// Draws the list full-screen. `&mut self` — the rename field draws
    /// an [`InputBox`](crate::widgets::input_box::InputBox), which needs `&mut`.
    pub fn render(&mut self, frame: &mut Frame) {
        self.state
            .render(frame, frame.area(), self.active, &self.palette, self.loc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn chat(title: &str) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn esc_maps_to_close() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default(), ru());
        assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(ChatListIntent::Close));
    }

    #[test]
    fn ctrl_q_and_f10_map_to_quit() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default(), ru());
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('q'))),
            Some(ChatListIntent::Quit)
        );
        assert_eq!(
            s.handle_key(key(KeyCode::F(10))),
            Some(ChatListIntent::Quit)
        );
    }

    #[test]
    fn enter_maps_to_switch_of_selected() {
        let chats = vec![chat("A")];
        let id = chats[0].id;
        let mut s = ChatListScreen::new(chats, Some(id), Palette::default(), ru());
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(ChatListIntent::Switch(id))
        );
    }

    #[test]
    fn ctrl_n_maps_to_new_chat() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default(), ru());
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('n'))),
            Some(ChatListIntent::NewChat)
        );
    }

    #[test]
    fn typing_filters_and_returns_none() {
        let mut s = ChatListScreen::new(
            vec![chat("Альфа"), chat("Бета")],
            None,
            Palette::default(),
            ru(),
        );
        // Typing a character into the search field is handled internally (no intent).
        assert_eq!(s.handle_key(key(KeyCode::Char('Б'))), None);
    }

    #[test]
    fn pending_new_chat_flag_set_and_taken_once() {
        let mut s = ChatListScreen::new(vec![chat("A")], None, Palette::default(), ru());
        assert!(!s.take_pending_new_chat());
        s.set_pending_new_chat();
        // Taken exactly once (resets) — a second activation shouldn't close it.
        assert!(s.take_pending_new_chat());
        assert!(!s.take_pending_new_chat());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut s = ChatListScreen::new(vec![chat("Альфа")], None, Palette::default(), ru());
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}
