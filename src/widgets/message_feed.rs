//! Лента сообщений: markdown-рендер тел (через [`crate::shared::markdown`]),
//! сворачиваемый блок «мыслей» (CoT) и вертикальный скролл. См. spec §11.3–11.4.
//!
//! «Мысли» сворачиваются глобальным переключателем (`show_thoughts`); tool-блоки
//! (имя/аргументы/результат) показываются внутри сообщения ассистента (M5).
//! Выделение отдельных сообщений/блоков — позже. Виджет хранит только состояние
//! просмотра (скролл, «следовать за хвостом», показ мыслей); сами сообщения
//! принадлежат UI-состоянию и передаются на отрисовку.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::entities::message::{Message, MessageRole};
use crate::shared::markdown;
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Роль элемента ленты (UI-проекция; системные сообщения не показываются).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedRole {
    User,
    Assistant,
    /// Служебная заметка (ошибки, «генерация отменена»).
    Note,
}

/// Tool-блок в ленте: имя инструмента, аргументы и результат (сворачиваемо). См. spec §11.3.
#[derive(Debug, Clone)]
pub struct FeedToolCall {
    pub name: String,
    pub arguments: String,
    pub result: String,
}

/// Элемент ленты сообщений.
#[derive(Debug, Clone)]
pub struct FeedMessage {
    pub role: FeedRole,
    pub text: String,
    pub thoughts: String,
    /// Вызовы инструментов этого сообщения ассистента (tool-блоки).
    pub tools: Vec<FeedToolCall>,
    /// Идёт ли стриминг этого сообщения (показываем «…» вместо пустого тела).
    pub streaming: bool,
}

impl FeedMessage {
    /// Служебная заметка для ленты.
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            role: FeedRole::Note,
            text: text.into(),
            thoughts: String::new(),
            tools: Vec::new(),
            streaming: false,
        }
    }

    /// Проекция доменного сообщения (для перестроения ленты при активации чата).
    /// Системные и tool-сообщения отбрасываются (`None`): tool-вызовы показываются
    /// как блоки внутри сообщения ассистента (`tool_calls`), а не отдельно.
    pub fn from_message(msg: &Message) -> Option<Self> {
        let role = match msg.role {
            MessageRole::User => FeedRole::User,
            MessageRole::Assistant => FeedRole::Assistant,
            MessageRole::Tool | MessageRole::System => return None,
        };
        let tools = msg
            .tool_calls
            .iter()
            .map(|tc| FeedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.to_string(),
                result: tc.result.clone().unwrap_or_default(),
            })
            .collect();
        Some(Self {
            role,
            text: msg.text.clone(),
            thoughts: msg.thoughts.clone().unwrap_or_default(),
            tools,
            streaming: false,
        })
    }
}

/// Состояние просмотра ленты (скролл, показ мыслей).
pub struct MessageFeed {
    /// Смещение прокрутки (в строках от начала).
    scroll: usize,
    /// Следовать за хвостом (автопрокрутка к низу при новом контенте).
    follow: bool,
    /// Показывать развёрнутый блок «мыслей».
    show_thoughts: bool,
}

impl Default for MessageFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFeed {
    pub fn new() -> Self {
        Self {
            scroll: 0,
            follow: true,
            show_thoughts: false,
        }
    }

    /// Переключает показ блоков «мыслей».
    pub fn toggle_thoughts(&mut self) {
        self.show_thoughts = !self.show_thoughts;
    }

    /// Прокрутка вверх (отключает «следование за хвостом»).
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
        self.follow = false;
    }

    /// Прокрутка вниз (у самого низа снова включает «следование»).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        // Фактический кламп и повторное включение follow — в render (там известна высота).
    }

    /// Сбрасывает прокрутку к низу (при смене чата/отправке).
    pub fn scroll_to_bottom(&mut self) {
        self.follow = true;
    }

    /// Тест-аксессор: следует ли лента за хвостом (прокрутка к низу).
    #[cfg(test)]
    pub(crate) fn is_following(&self) -> bool {
        self.follow
    }

    /// Рисует ленту. `messages` — текущее содержимое активного чата.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        messages: &[FeedMessage],
        palette: &Palette,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {title} "));
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        // Переносим строки по ширине ленты заранее: так число визуальных рядов
        // совпадает с `lines.len()`, и математика скролла/«следования за хвостом»
        // ниже остаётся row-based (см. shared::wrap, ADR 0001).
        let view_w = inner.width.max(1) as usize;
        let lines: Vec<Line> = self
            .build_lines(messages, palette)
            .iter()
            .flat_map(|l| wrap::wrap_line(l, view_w))
            .collect();
        let total = lines.len();
        let view_h = inner.height.max(1) as usize;
        let max_scroll = total.saturating_sub(view_h);

        if self.follow {
            self.scroll = max_scroll;
        } else if self.scroll >= max_scroll {
            // докрутили до низа — снова следуем за хвостом
            self.scroll = max_scroll;
            self.follow = true;
        }

        let paragraph = Paragraph::new(Text::from(lines)).scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph, inner);
    }

    /// Собирает строки ленты: заголовки ролей, свёрнутые/развёрнутые «мысли»,
    /// markdown-рендер тела, разделители.
    fn build_lines(&self, messages: &[FeedMessage], palette: &Palette) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if messages.is_empty() {
            lines.push(Line::from("Начните диалог — введите сообщение ниже.").dim());
            return lines;
        }
        for item in messages {
            match item.role {
                FeedRole::User => lines.push(Line::from("Вы:").bold().fg(palette.user)),
                FeedRole::Assistant => {
                    lines.push(Line::from("Ассистент:").bold().fg(palette.assistant))
                }
                FeedRole::Note => {}
            }
            push_thoughts(&mut lines, &item.thoughts, self.show_thoughts);
            push_tools(&mut lines, &item.tools, palette);
            push_body(&mut lines, item);
            lines.push(Line::from(""));
        }
        lines
    }
}

/// Добавляет блок «мыслей» (свёрнутый — одной строкой-индикатором).
fn push_thoughts(lines: &mut Vec<Line<'static>>, thoughts: &str, expanded: bool) {
    if thoughts.is_empty() {
        return;
    }
    if !expanded {
        let count = thoughts.lines().count();
        lines.push(
            Line::from(format!("  ▸ мысли ({count} стр., Ctrl+T)"))
                .dim()
                .italic(),
        );
        return;
    }
    lines.push(Line::from("  ▾ мысли:").dim().italic());
    for t in thoughts.lines() {
        lines.push(Line::from(format!("  │ {t}")).dim().italic());
    }
}

/// Добавляет tool-блоки сообщения: имя + аргументы + результат (кратко, dim).
fn push_tools(lines: &mut Vec<Line<'static>>, tools: &[FeedToolCall], palette: &Palette) {
    for tool in tools {
        lines.push(
            Line::from(format!(
                "  🔧 {}({})",
                tool.name,
                truncate(&tool.arguments, 80)
            ))
            .dim()
            .fg(palette.tool),
        );
        if !tool.result.is_empty() {
            for line in tool.result.lines().take(6) {
                lines.push(Line::from(format!("  │ {}", truncate(line, 100))).dim());
            }
        }
    }
}

/// Обрезает строку до `max` символов с многоточием.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Добавляет тело сообщения: markdown для user/assistant, dim-текст для заметок.
fn push_body(lines: &mut Vec<Line<'static>>, item: &FeedMessage) {
    if item.text.is_empty() {
        if item.streaming {
            lines.push(Line::from("…").dim());
        }
        return;
    }
    match item.role {
        FeedRole::Note => {
            for line in item.text.split('\n') {
                lines.push(Line::from(Span::from(line.to_string()).dim()));
            }
        }
        _ => {
            // markdown → Text; переносим строки в общий буфер
            let rendered = markdown::render(&item.text);
            lines.extend(rendered.lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn msg(role: FeedRole, text: &str, thoughts: &str) -> FeedMessage {
        FeedMessage {
            role,
            text: text.to_string(),
            thoughts: thoughts.to_string(),
            tools: Vec::new(),
            streaming: false,
        }
    }

    #[test]
    fn tool_blocks_render() {
        let feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "готово", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: "{\"content\":\"x\"}".into(),
            result: "Заметка сохранена".into(),
        });
        let lines = feed.build_lines(&[m], &Palette::default());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("🔧 note_save"));
        assert!(joined.contains("Заметка сохранена"));
    }

    #[test]
    fn from_message_skips_system() {
        let sys = Message::new(MessageRole::System, "s");
        assert!(FeedMessage::from_message(&sys).is_none());
        let user = Message::user("hi");
        assert_eq!(
            FeedMessage::from_message(&user).unwrap().role,
            FeedRole::User
        );
    }

    #[test]
    fn collapsed_thoughts_show_indicator_not_content() {
        let feed = MessageFeed::new(); // show_thoughts = false
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет\nмысль")],
            &Palette::default(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("▸ мысли"));
        assert!(!joined.contains("секрет"));
    }

    #[test]
    fn expanded_thoughts_show_content() {
        let mut feed = MessageFeed::new();
        feed.toggle_thoughts();
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "ответ", "секрет")],
            &Palette::default(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("секрет"));
    }

    #[test]
    fn markdown_is_applied_to_body() {
        let feed = MessageFeed::new();
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "формула x^2", "")],
            &Palette::default(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("x²"));
    }

    #[test]
    fn scroll_up_disables_follow() {
        let mut feed = MessageFeed::new();
        assert!(feed.follow);
        feed.scroll_up(3);
        assert!(!feed.follow);
    }

    #[test]
    fn render_does_not_panic() {
        let mut feed = MessageFeed::new();
        let messages = vec![
            msg(FeedRole::User, "привет", ""),
            msg(
                FeedRole::Assistant,
                "# Заголовок\n\nответ с `кодом`",
                "мысль",
            ),
            FeedMessage::note("(генерация отменена)"),
        ];
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| feed.render(f, f.area(), "Чат", &messages, &Palette::default()))
            .unwrap();
    }
}
