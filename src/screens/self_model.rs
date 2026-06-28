//! Экран просмотра «модели себя» (FSD "page"): read-only полноэкранный вид
//! текущего представления агента о себе для активного профиля — описание, цели,
//! модель собеседника и нарратив инсайтов. Открывается из чата по `F3`,
//! закрывается `Esc`. Данные приходят снимком от оркестратора (он владеет
//! `Storage`) событием `AppEvent::SelfModelView`. Правка — пока только силами
//! модели (инструменты SelfModel); ручное редактирование — задел Tier 3.
//! См. [docs/self-model-mvp.md](../../docs/self-model-mvp.md).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::entities::self_model::{GoalStatus, SelfModel};
use crate::shared::keys;
use crate::shared::theme::Palette;

/// Намерение экрана просмотра модели себя (транслируется `app`). Параллель к
/// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent).
#[derive(Debug, Clone, PartialEq)]
pub enum SelfModelIntent {
    /// Закрыть вид, вернуться к чату (`Esc`).
    Close,
    /// Выйти из приложения (`Ctrl+C`).
    Quit,
}

/// Экран просмотра модели себя: снимок модели + контекст отрисовки.
pub struct SelfModelScreen {
    /// Снимок модели активного профиля (`None` — ещё не создавалась).
    model: Option<SelfModel>,
    palette: Palette,
    /// Вертикальная прокрутка (в строках содержимого).
    scroll: u16,
}

impl SelfModelScreen {
    /// Открывает экран со снимком модели.
    pub fn new(model: Option<SelfModel>, palette: Palette) -> Self {
        Self {
            model,
            palette,
            scroll: 0,
        }
    }

    /// Обновляет палитру темы (событие `AppEvent::Settings`).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Обрабатывает нажатие клавиши, возвращая намерение для `app` (или `None`,
    /// если обработано внутри: прокрутка).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SelfModelIntent> {
        // Ctrl+C — выход (раскладко-независимо).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
            && keys::physical_char(c) == 'c'
        {
            return Some(SelfModelIntent::Quit);
        }
        match key.code {
            KeyCode::Esc => return Some(SelfModelIntent::Close),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Home => self.scroll = 0,
            _ => {}
        }
        None
    }

    /// Строит строки содержимого по снимку модели.
    fn content_lines(&self) -> Vec<Line<'static>> {
        let p = &self.palette;
        let Some(m) = self.model.as_ref().filter(|m| !m.is_empty()) else {
            return vec![Line::styled(
                "Модель себя пока пуста (или инструменты модели себя выключены в профиле).",
                p.muted_style(),
            )];
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        let header = |text: &str| Line::styled(text.to_string(), p.accent_style());

        if !m.summary.trim().is_empty() {
            lines.push(header("О себе"));
            lines.push(Line::raw(m.summary.trim().to_string()));
            lines.push(Line::raw(""));
        }

        if !m.goals.is_empty() {
            lines.push(header("Цели"));
            for g in &m.goals {
                let (marker, style) = match g.status {
                    GoalStatus::Active => ("●", p.success_style()),
                    GoalStatus::Completed => ("✓", p.muted_style()),
                    GoalStatus::Abandoned => ("✗", p.muted_style()),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{marker} "), style),
                    Span::raw(g.description.trim().to_string()),
                ]));
            }
            lines.push(Line::raw(""));
        }

        let u = &m.user_model;
        if !u.is_empty() {
            lines.push(header("О собеседнике"));
            if !u.perceived_traits.is_empty() {
                lines.push(Line::raw(format!(
                    "Черты: {}",
                    u.perceived_traits.join(", ")
                )));
            }
            if !u.current_interests.is_empty() {
                lines.push(Line::raw(format!(
                    "Интересы: {}",
                    u.current_interests.join(", ")
                )));
            }
            if !u.relationship_dynamic.trim().is_empty() {
                lines.push(Line::raw(format!(
                    "Отношения: {}",
                    u.relationship_dynamic.trim()
                )));
            }
            lines.push(Line::raw(""));
        }

        if !m.narrative.is_empty() {
            lines.push(header("Нарратив (новые сверху)"));
            for seg in m.narrative.iter().rev() {
                let date = seg.created_at.format("%Y-%m-%d").to_string();
                lines.push(Line::from(vec![
                    Span::styled(format!("[{date}] "), p.muted_style()),
                    Span::raw(seg.text.trim().to_string()),
                ]));
            }
        }
        lines
    }

    /// Рисует вид во весь экран: скруглённая панель + содержимое + строка хоткеев.
    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let p = &self.palette;
        let block = p.panel("✦ Модель себя", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        let lines = self.content_lines();
        let body = Paragraph::new(lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(body, chunks[0]);

        // Строка хоткеев.
        let mut hint: Vec<Span<'static>> = Vec::new();
        hint.extend(p.hint("Esc", "закрыть"));
        hint.push(Span::raw("  "));
        hint.extend(p.hint("↑↓/PgUp/PgDn", "прокрутка"));
        hint.push(Span::raw("  "));
        hint.extend(p.hint("Ctrl+C", "выход"));
        frame.render_widget(Paragraph::new(Line::from(hint)), chunks[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use uuid::Uuid;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn esc_closes_ctrl_c_quits() {
        let mut s = SelfModelScreen::new(None, Palette::default());
        assert_eq!(
            s.handle_key(key(KeyCode::Esc)),
            Some(SelfModelIntent::Close)
        );
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(SelfModelIntent::Quit)
        );
        // Ctrl+C раскладко-независим (физ. C = русская «с»).
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Char('с'), KeyModifiers::CONTROL)),
            Some(SelfModelIntent::Quit)
        );
    }

    #[test]
    fn scroll_keys_return_none_and_clamp_at_top() {
        let mut s = SelfModelScreen::new(None, Palette::default());
        assert_eq!(s.handle_key(key(KeyCode::Up)), None);
        assert_eq!(s.scroll, 0); // не уходит ниже нуля
        assert_eq!(s.handle_key(key(KeyCode::Down)), None);
        assert_eq!(s.scroll, 1);
        assert_eq!(s.handle_key(key(KeyCode::Home)), None);
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn empty_model_renders_placeholder_without_panic() {
        let s = SelfModelScreen::new(None, Palette::default());
        let lines = s.content_lines();
        assert_eq!(lines.len(), 1);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }

    #[test]
    fn populated_model_renders_sections() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю ясность".into();
        m.add_goal("помочь с проектом");
        m.user_model.perceived_traits = vec!["скептичный".into()];
        m.add_insight("замечен интерес к Rust", 50);
        let s = SelfModelScreen::new(Some(m), Palette::default());

        // Содержимое включает заголовки секций.
        let text: String = s
            .content_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.clone()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(text.contains("О себе"));
        assert!(text.contains("ценю ясность"));
        assert!(text.contains("Цели"));
        assert!(text.contains("помочь с проектом"));
        assert!(text.contains("О собеседнике"));
        assert!(text.contains("Нарратив"));
        assert!(text.contains("замечен интерес к Rust"));

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
    }
}
