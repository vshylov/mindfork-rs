//! The `chat://` reference picker (`Ctrl+L`). See spec §11.3 and
//! docs/research/chat-uri-links.md §4.5.
//!
//! The feed draws a resolvable `chat://` address as a link; this overlay is how
//! it is *followed*. A keyboard route rather than a click, because mouse
//! capture is `Ctrl+W` and off by default — turning it on costs native terminal
//! selection, so a click would be unreachable for most users (fork F4). The
//! mouse is a second stage, not the way in.
//!
//! Self-contained like the profile picker it is modelled on: it holds a
//! snapshot and answers key presses with a [`ChatLinkAction`] the layer above
//! executes.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState};
use uuid::Uuid;

use crate::entities::chat::ChatSummary;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;

/// Action the overlay asks the layer above to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatLinkAction {
    /// The key press was handled inside the overlay.
    None,
    /// Close the overlay without following a reference.
    Cancel,
    /// Open the picked conversation.
    Open(Uuid),
}

/// State of the reference picker.
pub struct ChatLinkPickerState {
    /// The conversations this chat links to, in the order the feed offers them
    /// (the most recently drawn first).
    chats: Vec<ChatSummary>,
    /// The open conversation, when the feed links to it: picking it is not a
    /// switch, and saying so is the overlay's job rather than a silent no-op
    /// (docs/lessons.md §4).
    current: Option<Uuid>,
    selected: usize,
}

impl ChatLinkPickerState {
    /// Opens the picker over a reference snapshot. `current` — the open chat's
    /// id, so an entry pointing back at it can be marked.
    pub fn new(chats: Vec<ChatSummary>, current: Option<Uuid>) -> Self {
        Self {
            chats,
            current,
            selected: 0,
        }
    }

    /// Id of the selected conversation (if the list isn't empty).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.chats.get(self.selected).map(|c| c.id)
    }

    /// Handles a key press, returning the action to execute.
    pub fn on_key(&mut self, key: KeyEvent) -> ChatLinkAction {
        if key.kind != KeyEventKind::Press {
            return ChatLinkAction::None;
        }
        match key.code {
            KeyCode::Esc => ChatLinkAction::Cancel,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ChatLinkAction::Open(id),
                None => ChatLinkAction::Cancel,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ChatLinkAction::None
            }
            KeyCode::Down => {
                if !self.chats.is_empty() {
                    self.selected = (self.selected + 1).min(self.chats.len() - 1);
                }
                ChatLinkAction::None
            }
            _ => ChatLinkAction::None,
        }
    }

    /// Draws the overlay centered in `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette, loc: &'static Locale) {
        let rows = (self.chats.len() as u16 + 2).clamp(5, area.height);
        let popup = centered_rect(60, 40, rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(
                // The chat-list icon rather than a new one: this is a list of
                // conversations, and new chrome glyphs carry a width/WGL4
                // discipline of their own (docs/lessons.md §5).
                format!(
                    "{} {}",
                    palette.glyphs().chats_icon,
                    loc.t("ui.chat_links.title")
                ),
                true,
            )
            .title_bottom(Line::from(Span::styled(
                loc.t("ui.chat_links.footer"),
                palette.muted_style(),
            )));
        let items: Vec<ListItem> = self
            .chats
            .iter()
            .map(|c| {
                let mut spans = vec![Span::styled(c.title.clone(), Style::new().fg(palette.text))];
                // The date is what tells two same-named conversations apart —
                // the same thing the chat list leans on.
                spans.push(Span::styled(
                    format!("  {}", c.modified_at.format("%Y-%m-%d")),
                    palette.muted_style(),
                ));
                if self.current == Some(c.id) {
                    spans.push(Span::styled(
                        format!("  {}", loc.t("ui.chat_links.current")),
                        palette.muted_style(),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        if !self.chats.is_empty() {
            state.select(Some(self.selected.min(self.chats.len() - 1)));
        }
        frame.render_stateful_widget(list, popup, &mut state);
    }
}

/// A rectangle centered in `area`: `pct_x` percent width (≥`min_w`), fixed height.
fn centered_rect(pct_x: u16, min_w: u16, height: u16, area: Rect) -> Rect {
    let w = area.width.saturating_mul(pct_x) / 100;
    let [h_area] = Layout::horizontal([Constraint::Length(w.max(min_w).min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v_area] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h_area);
    v_area
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn chat(title: &str) -> ChatSummary {
        ChatSummary {
            id: Uuid::new_v4(),
            profile_id: Uuid::nil(),
            title: title.to_string(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
            message_count: 1,
        }
    }

    #[test]
    fn arrows_move_and_stay_inside_the_list() {
        let chats = vec![chat("a"), chat("b")];
        let (first, last) = (chats[0].id, chats[1].id);
        let mut s = ChatLinkPickerState::new(chats, None);

        assert_eq!(s.selected_id(), Some(first));
        s.on_key(key(KeyCode::Up));
        assert_eq!(s.selected_id(), Some(first), "no wrap past the top");
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected_id(), Some(last), "no wrap past the bottom");
    }

    #[test]
    fn enter_opens_and_esc_cancels() {
        let chats = vec![chat("a")];
        let id = chats[0].id;
        let mut s = ChatLinkPickerState::new(chats, None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatLinkAction::Open(id));
        assert_eq!(s.on_key(key(KeyCode::Esc)), ChatLinkAction::Cancel);
    }

    /// A repeat (`KeyEventKind::Repeat`) or release must not open anything —
    /// the same guard the other overlays carry.
    #[test]
    fn only_presses_are_handled() {
        let mut s = ChatLinkPickerState::new(vec![chat("a")], None);
        let mut ev = key(KeyCode::Enter);
        ev.kind = KeyEventKind::Release;
        assert_eq!(s.on_key(ev), ChatLinkAction::None);
    }

    /// An empty list cannot be opened into: `Enter` closes instead of picking
    /// nothing.
    #[test]
    fn an_empty_list_cancels_on_enter() {
        let mut s = ChatLinkPickerState::new(Vec::new(), None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ChatLinkAction::Cancel);
        assert_eq!(s.selected_id(), None);
    }
}
