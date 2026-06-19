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
use ratatui::style::{Modifier, Style, Stylize};
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

/// Tool-блок в ленте: имя инструмента, аргументы и результат. См. spec §11.3.
///
/// `text_offset` — позиция вызова **в байтах** внутри `FeedMessage::text`: сколько
/// текста ответа ассистента было сгенерировано ДО этого вызова. Так tool-блок
/// рисуется ровно на месте вызова (между фрагментами текста), а не в «шапке».
#[derive(Debug, Clone)]
pub struct FeedToolCall {
    pub name: String,
    pub arguments: String,
    pub result: String,
    /// Смещение (в байтах) в `FeedMessage::text`, после которого был сделан вызов.
    pub text_offset: usize,
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
    ///
    /// Все вызовы инструментов раунда сделаны ПОСЛЕ его текста, поэтому их
    /// `text_offset` = длина текста (вызовы рисуются под ним).
    pub fn from_message(msg: &Message) -> Option<Self> {
        let role = match msg.role {
            MessageRole::User => FeedRole::User,
            MessageRole::Assistant => FeedRole::Assistant,
            MessageRole::Tool | MessageRole::System => return None,
        };
        let off = msg.text.len();
        let tools = msg
            .tool_calls
            .iter()
            .map(|tc| FeedToolCall {
                name: tc.name.clone(),
                arguments: tc.arguments.to_string(),
                result: tc.result.clone().unwrap_or_default(),
                text_offset: off,
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

    /// Проекция списка доменных сообщений в ленту со **склейкой раундов**: подряд
    /// идущие сообщения ассистента (раунды agentic-loop, между которыми в истории
    /// лежат tool-сообщения — они отбрасываются) сливаются в один блок «Ассистент:».
    /// Тексты раундов конкатенируются (через пустую строку), а `text_offset`
    /// вызовов каждого раунда сдвигается на накопленную длину — так live-стрим и
    /// перезагрузка из истории дают одинаковую инлайн-раскладку вызовов.
    pub fn from_messages(messages: &[Message]) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        for msg in messages {
            let Some(mut fm) = Self::from_message(msg) else {
                continue;
            };
            // Сливаем раунд ассистента с предыдущим блоком ассистента.
            if fm.role == FeedRole::Assistant
                && let Some(last) = out.last_mut()
                && last.role == FeedRole::Assistant
            {
                if !fm.text.is_empty() {
                    if !last.text.is_empty() {
                        last.text.push_str("\n\n");
                    }
                    last.text.push_str(&fm.text);
                }
                let base = last.text.len();
                for mut tc in fm.tools.drain(..) {
                    tc.text_offset = base;
                    last.tools.push(tc);
                }
                if !fm.thoughts.is_empty() {
                    if !last.thoughts.is_empty() {
                        last.thoughts.push('\n');
                    }
                    last.thoughts.push_str(&fm.thoughts);
                }
                continue;
            }
            out.push(fm);
        }
        out
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
            .build_lines(messages, palette, view_w)
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
    fn build_lines(
        &self,
        messages: &[FeedMessage],
        palette: &Palette,
        width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if messages.is_empty() {
            lines.push(Line::from("Начните диалог — введите сообщение ниже.").dim());
            return lines;
        }
        for item in messages {
            match item.role {
                FeedRole::User => {
                    lines.push(Line::from("Вы:").bold().fg(palette.user));
                    push_body(&mut lines, item, palette, width);
                }
                FeedRole::Assistant => {
                    lines.push(Line::from("Ассистент:").bold().fg(palette.assistant));
                    push_thoughts(&mut lines, &item.thoughts, self.show_thoughts);
                    push_assistant_body(&mut lines, item, palette, width);
                }
                FeedRole::Note => push_body(&mut lines, item, palette, width),
            }
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

/// Добавляет тело ответа ассистента с tool-блоками **на местах вызова**: фрагмент
/// текста до вызова → tool-блок → следующий фрагмент и т.д. (см. spec §11.3).
/// `text_offset` каждого вызова делит `item.text` на фрагменты markdown.
///
/// Tool-блоки отделяются от текста (и от соседних tool-блоков) пустой строкой
/// сверху и снизу, чтобы не сливались с сообщением; идущие подряд вызовы делит
/// ровно одна пустая строка (`ensure_blank_line` схлопывает соседние).
fn push_assistant_body(
    lines: &mut Vec<Line<'static>>,
    item: &FeedMessage,
    palette: &Palette,
    width: usize,
) {
    let text = item.text.as_str();
    let mut pos = 0usize;
    let mut produced = false;
    for tool in &item.tools {
        let off = clamp_boundary(text, tool.text_offset.min(text.len())).max(pos);
        if off > pos {
            push_markdown_fragment(lines, &text[pos..off], palette, width);
        }
        // Пустая строка перед вызовом (схлопывается, если предыдущая уже пуста —
        // напр. между двумя подряд идущими вызовами).
        ensure_blank_line(lines);
        push_tool(lines, tool, palette, width);
        produced = true;
        pos = off;
    }
    if pos < text.len() {
        // Текст после последнего вызова отделяем пустой строкой.
        if !item.tools.is_empty() {
            ensure_blank_line(lines);
        }
        if push_markdown_fragment(lines, &text[pos..], palette, width) {
            produced = true;
        }
    }
    // Пустой стримящийся ответ (ещё ни текста, ни вызовов) — индикатор «…».
    if !produced && item.streaming {
        lines.push(Line::from("…").dim());
    }
}

/// Добавляет пустую строку-разделитель, если последняя строка ещё не пуста.
/// Так соседние разделители (напр. «после вызова» + «перед следующим вызовом»)
/// схлопываются в одну пустую строку. На пустом буфере — no-op (без ведущей пустой).
fn ensure_blank_line(lines: &mut Vec<Line<'static>>) {
    let blank = lines
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(true);
    if !blank {
        lines.push(Line::from(""));
    }
}

/// Рендерит фрагмент текста как markdown (если он не пустой). Возвращает, выдал ли строки.
fn push_markdown_fragment(
    lines: &mut Vec<Line<'static>>,
    fragment: &str,
    palette: &Palette,
    width: usize,
) -> bool {
    if fragment.trim().is_empty() {
        return false;
    }
    let rendered = markdown::render(fragment, width, palette);
    lines.extend(rendered.lines);
    true
}

/// Один tool-блок: заголовок `🔧 имя(аргументы)` (цвет инструмента) и результат на
/// гуттере `│`. Аргументы и результат **переносятся по ширине** (не обрезаются).
fn push_tool(lines: &mut Vec<Line<'static>>, tool: &FeedToolCall, palette: &Palette, width: usize) {
    let head_style = Style::default()
        .fg(palette.tool)
        .add_modifier(Modifier::DIM);
    let header = if tool.arguments.trim().is_empty() {
        format!("🔧 {}", tool.name)
    } else {
        format!("🔧 {}({})", tool.name, tool.arguments)
    };
    // Первый ряд с отступом «  », продолжения выравниваем под имя.
    push_wrapped(lines, "  ", "     ", &header, width, head_style);
    if !tool.result.is_empty() {
        let body_style = Style::default().add_modifier(Modifier::DIM);
        push_wrapped(lines, "  │ ", "  │ ", &tool.result, width, body_style);
    }
}

/// Переносит `text` по визуальной ширине с гуттер-префиксами и кладёт в `lines`.
/// `first` — префикс самого первого ряда блока, `cont` — всех последующих рядов
/// (и продолжений переноса, и новых логических строк исходника). Каждая строка
/// `text` переносится отдельно, сохраняя переводы строк.
fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    first: &str,
    cont: &str,
    text: &str,
    width: usize,
    style: Style,
) {
    let first_w = wrap::display_width(&first.chars().collect::<Vec<_>>());
    let cont_w = wrap::display_width(&cont.chars().collect::<Vec<_>>());
    let body_w = width.saturating_sub(first_w.max(cont_w)).max(1);
    let mut first_row = true;
    for src in text.split('\n') {
        let chars: Vec<char> = src.chars().collect();
        for (s, e) in wrap::wrap_ranges(&chars, body_w) {
            let prefix = if first_row { first } else { cont };
            first_row = false;
            let content: String = chars[s..e].iter().collect();
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(content, style),
            ]));
        }
    }
}

/// Ближайшая (вниз) валидная граница символа для байтового смещения.
fn clamp_boundary(text: &str, mut off: usize) -> usize {
    while off < text.len() && !text.is_char_boundary(off) {
        off += 1;
    }
    off.min(text.len())
}

/// Добавляет тело сообщения: markdown для user, dim-текст для заметок.
fn push_body(lines: &mut Vec<Line<'static>>, item: &FeedMessage, palette: &Palette, width: usize) {
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
            let rendered = markdown::render(&item.text, width, palette);
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
            text_offset: m.text.len(),
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("🔧 note_save"));
        assert!(joined.contains("Заметка сохранена"));
    }

    #[test]
    fn tool_call_renders_after_preceding_text_inline() {
        // Текст до вызова → tool-блок → текст после вызова: проверяем порядок строк.
        let feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "Ищу погоду.\n\nГотово: ясно.", "");
        let off = "Ищу погоду.".len();
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: "{}".into(),
            result: "ясно".into(),
            text_offset: off,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 80);
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx_before = rows.iter().position(|r| r.contains("Ищу погоду")).unwrap();
        let idx_tool = rows
            .iter()
            .position(|r| r.contains("🔧 web_search"))
            .unwrap();
        let idx_after = rows.iter().position(|r| r.contains("Готово")).unwrap();
        assert!(
            idx_before < idx_tool && idx_tool < idx_after,
            "ожидался порядок: текст-до < вызов < текст-после, было {idx_before}/{idx_tool}/{idx_after}"
        );
    }

    /// Хелпер: рендер строк в список «есть ли в строке непустой контент» —
    /// удобно искать пустые строки-разделители.
    fn row_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn tool_block_separated_from_text_by_blank_lines() {
        // текст-до → вызов → текст-после: вокруг вызова должны быть пустые строки.
        let feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "доПОСЛЕ", "");
        m.tools.push(FeedToolCall {
            name: "note_save".into(),
            arguments: String::new(),
            result: String::new(),
            text_offset: "до".len(),
        });
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80));
        let i_before = rows.iter().position(|r| r.contains("до")).unwrap();
        let i_tool = rows
            .iter()
            .position(|r| r.contains("🔧 note_save"))
            .unwrap();
        let i_after = rows.iter().position(|r| r.contains("ПОСЛЕ")).unwrap();
        // Между текстом-до и вызовом — ровно одна пустая строка.
        assert!(
            rows[i_before + 1..i_tool]
                .iter()
                .all(|r| r.trim().is_empty())
        );
        assert_eq!(
            i_tool - i_before,
            2,
            "ожидалась одна пустая строка перед вызовом"
        );
        // Между вызовом и текстом-после — ровно одна пустая строка.
        assert_eq!(
            i_after - i_tool,
            2,
            "ожидалась одна пустая строка после вызова"
        );
    }

    #[test]
    fn consecutive_tool_blocks_have_single_blank_between() {
        let feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "", "");
        for name in ["first_tool", "second_tool"] {
            m.tools.push(FeedToolCall {
                name: name.into(),
                arguments: String::new(),
                result: String::new(),
                text_offset: 0,
            });
        }
        let rows = row_texts(&feed.build_lines(&[m], &Palette::default(), 80));
        let i1 = rows
            .iter()
            .position(|r| r.contains("🔧 first_tool"))
            .unwrap();
        let i2 = rows
            .iter()
            .position(|r| r.contains("🔧 second_tool"))
            .unwrap();
        // Между двумя подряд идущими вызовами — ровно одна пустая строка.
        assert_eq!(
            i2 - i1,
            2,
            "между соседними вызовами должна быть одна пустая строка"
        );
        assert!(rows[i1 + 1..i2].iter().all(|r| r.trim().is_empty()));
    }

    #[test]
    fn long_tool_result_wraps_not_truncated() {
        let feed = MessageFeed::new();
        let mut m = msg(FeedRole::Assistant, "ок", "");
        let long = "слово ".repeat(40); // ~240 символов — заведомо шире узкой ленты
        m.tools.push(FeedToolCall {
            name: "web_search".into(),
            arguments: String::new(),
            result: long.trim_end().into(),
            text_offset: 0,
        });
        let lines = feed.build_lines(&[m], &Palette::default(), 30);
        // Результат не обрезан (нет «…») и разложен на несколько рядов гуттера.
        let gutter_rows = lines
            .iter()
            .filter(|l| {
                let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
                s.starts_with("  │ ")
            })
            .count();
        assert!(
            gutter_rows > 1,
            "длинный результат должен переноситься, рядов: {gutter_rows}"
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains('…'),
            "результат не должен обрезаться многоточием"
        );
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
            80,
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
            80,
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
        // LaTeX действует внутри $…$ (delimiter-scoped, см. shared::markdown)
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, "формула $x^2$", "")],
            &Palette::default(),
            80,
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
