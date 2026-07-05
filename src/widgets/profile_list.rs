//! Оверлей выбора профиля при создании чата (`Ctrl+N`). См. spec §10, §11.2.
//!
//! Виджет самодостаточен: хранит снимок списка профилей и состояние выбора, на
//! нажатия отвечает [`ProfileListAction`] (его исполняет `app`). Полноценная
//! секция управления профилями (CRUD) — на M8; здесь только выбор.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState};
use uuid::Uuid;

use crate::entities::profile::ProfileSummary;
use crate::shared::theme::Palette;

/// Действие, которое оверлей просит выполнить вышестоящий слой.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileListAction {
    /// Нажатие обработано внутри оверлея.
    None,
    /// Закрыть оверлей без выбора.
    Cancel,
    /// Создать чат из выбранного профиля.
    Pick(Uuid),
}

/// Состояние оверлея выбора профиля.
pub struct ProfileListState {
    profiles: Vec<ProfileSummary>,
    selected: usize,
}

impl ProfileListState {
    /// Открывает оверлей со снимком профилей (выделение — на первом).
    pub fn new(profiles: Vec<ProfileSummary>) -> Self {
        Self {
            profiles,
            selected: 0,
        }
    }

    /// Id выделенного профиля (если список не пуст).
    pub fn selected_id(&self) -> Option<Uuid> {
        self.profiles.get(self.selected).map(|p| p.id)
    }

    /// Обрабатывает нажатие клавиши, возвращая действие для исполнения.
    pub fn on_key(&mut self, key: KeyEvent) -> ProfileListAction {
        if key.kind != KeyEventKind::Press {
            return ProfileListAction::None;
        }
        match key.code {
            KeyCode::Esc => ProfileListAction::Cancel,
            KeyCode::Enter => match self.selected_id() {
                Some(id) => ProfileListAction::Pick(id),
                None => ProfileListAction::Cancel,
            },
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ProfileListAction::None
            }
            KeyCode::Down => {
                if !self.profiles.is_empty() {
                    self.selected = (self.selected + 1).min(self.profiles.len() - 1);
                }
                ProfileListAction::None
            }
            _ => ProfileListAction::None,
        }
    }

    /// Рисует оверлей по центру `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let rows = (self.profiles.len() as u16 + 2).clamp(5, area.height);
        let popup = centered_rect(50, 30, rows, area);
        frame.render_widget(Clear, popup);

        let block = palette
            .panel(
                format!(
                    "{} Новый чат · выбор профиля",
                    palette.glyphs().assistant_icon
                ),
                true,
            )
            .title_bottom(Line::from(Span::styled(
                " ↑↓ выбор · Enter создать · Esc отмена ",
                palette.muted_style(),
            )));
        let items: Vec<ListItem> = self
            .profiles
            .iter()
            .map(|p| {
                ListItem::new(Line::from(Span::styled(
                    p.name.clone(),
                    Style::new().fg(palette.text),
                )))
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        if !self.profiles.is_empty() {
            state.select(Some(self.selected.min(self.profiles.len() - 1)));
        }
        frame.render_stateful_widget(list, popup, &mut state);
    }
}

/// Прямоугольник по центру `area`: `pct_x` процентов ширины (≥`min_w`), фикс. высота.
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
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn profile(name: &str) -> ProfileSummary {
        ProfileSummary {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    #[test]
    fn enter_picks_selected_profile() {
        let profiles = vec![profile("A"), profile("B")];
        let b_id = profiles[1].id;
        let mut s = ProfileListState::new(profiles);
        assert_eq!(s.on_key(key(KeyCode::Down)), ProfileListAction::None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), ProfileListAction::Pick(b_id));
    }

    #[test]
    fn esc_cancels() {
        let mut s = ProfileListState::new(vec![profile("A")]);
        assert_eq!(s.on_key(key(KeyCode::Esc)), ProfileListAction::Cancel);
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut s = ProfileListState::new(vec![profile("A"), profile("B")]);
        s.on_key(key(KeyCode::Up)); // уже наверху — остаёмся
        assert_eq!(s.selected_id(), Some(s.profiles[0].id));
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down)); // ниже последнего нельзя
        assert_eq!(s.selected_id(), Some(s.profiles[1].id));
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let s = ProfileListState::new(vec![profile("Альфа"), profile("Бета")]);
        for (w, h) in [(80u16, 24u16), (20, 6)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| s.render(f, f.area(), &Palette::default()))
                .unwrap();
        }
    }
}
