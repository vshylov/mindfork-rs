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
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::entities::message::{Message, MessageRole};
use crate::shared::markdown;
use crate::shared::theme::Palette;
use crate::shared::wrap;

/// Гуттер-рейл слева от каждой строки сообщения: цветная вертикальная черта +
/// пробел (редизайн: «Role rails — цветные ▌ в гаттере»). Ширина — 2 колонки.
const RAIL: &str = "▌ ";

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
        // Управляющие инструменты беседы (followup/rewrite) — служебные сигналы,
        // не контент: их tool-блоки в ленте не показываем. См. spec §9.3.
        let tools = msg
            .tool_calls
            .iter()
            .filter(|tc| !crate::features::tools::control::is_control_tool(&tc.name))
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
            // Сливаем раунд ассистента с предыдущим блоком ассистента — кроме
            // случая, когда сообщение помечено `new_bubble` (инструмент «написать
            // ещё сообщение»): тогда оно начинает отдельный пузырь. См. spec §9.3.
            if fm.role == FeedRole::Assistant
                && !msg.new_bubble
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
    /// Флаг «лента только что прокручена пользователем» — петля по нему делает
    /// полную перерисовку терминала (как при ресайзе). Нужен из-за артефактов
    /// некоторых терминалов (Command Prompt/conhost) на VS16-эмодзи (`🕸️`,
    /// `🗂️`): они рисуют такой кластер физически шире модели ratatui, контент
    /// «съезжает» по горизонтали, и при странично-скачковой прокрутке (PageUp/
    /// PageDown) на месте уехавшего символа остаётся «висячая» буква в физической
    /// ячейке, которую поячеечный diff ratatui больше не затрагивает. Полная
    /// перерисовка (`terminal.clear`) гарантированно её стирает. См. spec §11.3.
    scrolled: bool,
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
            scrolled: false,
        }
    }

    /// Забирает (и сбрасывает) флаг «прокручено пользователем». Петля `app/runtime`
    /// при `true` делает `terminal.clear()` перед отрисовкой — стирает артефакты
    /// «съехавших» VS16-эмодзи (см. поле [`MessageFeed::scrolled`]).
    pub fn take_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.scrolled)
    }

    /// Переключает показ блоков «мыслей».
    pub fn toggle_thoughts(&mut self) {
        self.show_thoughts = !self.show_thoughts;
    }

    /// Прокрутка вверх (отключает «следование за хвостом»).
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
        self.follow = false;
        self.scrolled = true;
    }

    /// Прокрутка вниз (у самого низа снова включает «следование»).
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_add(lines);
        self.scrolled = true;
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

    /// Рисует ленту. `messages` — текущее содержимое активного чата. `meta` —
    /// правая подпись титула (напр. «gemma-4 · 16k ctx»; пусто — не показывать).
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        meta: &str,
        messages: &[FeedMessage],
        palette: &Palette,
    ) {
        // Скруглённая панель: слева титул с маркером ◆, справа — мета (модель/ctx).
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(palette.border_style(false))
            .title(Line::from(vec![
                Span::styled(" ◆ ", palette.muted_style()),
                Span::styled(format!("{title} "), Style::new().fg(palette.text)),
            ]));
        if !meta.is_empty() {
            block = block.title(
                Line::from(Span::styled(format!(" {meta} "), palette.muted_style()))
                    .right_aligned(),
            );
        }
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
    /// markdown-рендер тела, разделители. Каждая строка сообщения получает цветной
    /// гуттер-рейл по роли (см. [`RAIL`]); содержимое строится в ширину `width - 2`,
    /// затем переносится и к каждому визуальному ряду прикрепляется рейл.
    fn build_lines(
        &self,
        messages: &[FeedMessage],
        palette: &Palette,
        width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if messages.is_empty() {
            lines.push(Line::from(Span::styled(
                "Начните диалог — введите сообщение ниже.",
                palette.muted_style(),
            )));
            return lines;
        }
        // Ширина содержимого под рейл (рейл = 2 колонки).
        let inner = width.saturating_sub(RAIL.chars().count()).max(1);
        for item in messages {
            let rail = match item.role {
                FeedRole::User => palette.user,
                FeedRole::Assistant => palette.assistant,
                FeedRole::Note => palette.muted,
            };
            // Тело сообщения собираем без рейла, в ширину `inner`.
            let mut body: Vec<Line<'static>> = Vec::new();
            match item.role {
                FeedRole::User => {
                    body.push(role_header("❯ ВЫ", palette.user_soft));
                    push_body(&mut body, item, palette, inner);
                }
                FeedRole::Assistant => {
                    body.push(role_header("✦ АССИСТЕНТ", palette.assistant_soft));
                    push_thoughts(&mut body, &item.thoughts, self.show_thoughts, palette);
                    push_assistant_body(&mut body, item, palette, inner);
                }
                FeedRole::Note => push_body(&mut body, item, palette, inner),
            }
            // Переносим по ширине содержимого и навешиваем рейл на каждый ряд.
            for line in body {
                for wrapped in wrap::wrap_line(&line, inner) {
                    lines.push(prepend_rail(wrapped, rail));
                }
            }
            // Разделитель между сообщениями — без рейла.
            lines.push(Line::from(""));
        }
        lines
    }
}

/// Строка-заголовок роли: иконка + название капсом, цветом «мягкого» варианта роли.
fn role_header(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    ))
}

/// Прикрепляет цветной гуттер-рейл [`RAIL`] к строке (в начало), сохраняя стиль и
/// выравнивание исходной строки.
///
/// Стиль уровня строки (`line.style`) **вплавляется в спаны содержимого**, а
/// `out.style` сбрасывается в дефолт — иначе line-level модификаторы (напр. `DIM`
/// у разделителей `───` и рамок таблиц, см. `shared::markdown`) затекали бы и на
/// сам рейл (его спан задаёт только `fg`, не трогая модификаторы), и он выглядел бы
/// другим цветом напротив таких строк.
fn prepend_rail(line: Line<'static>, rail: Color) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(RAIL.to_string(), Style::new().fg(rail)));
    // Финальный стиль спана = line.style.patch(span.style); складываем то же самое в
    // сам спан, чтобы рейл не зависел от стиля строки.
    for span in line.spans {
        let merged = line.style.patch(span.style);
        spans.push(Span::styled(span.content, merged));
    }
    let mut out = Line::from(spans);
    out.alignment = line.alignment;
    out
}

/// Добавляет блок «мыслей»: свёрнутый — «пилюлей» с числом строк и клавишей,
/// развёрнутый — содержимым на гуттере `│`.
fn push_thoughts(
    lines: &mut Vec<Line<'static>>,
    thoughts: &str,
    expanded: bool,
    palette: &Palette,
) {
    if thoughts.is_empty() {
        return;
    }
    let muted = palette.muted_style();
    if !expanded {
        let count = thoughts.lines().count();
        lines.push(Line::from(vec![
            Span::styled("▸ ", muted),
            Span::styled("мысли", muted.add_modifier(Modifier::ITALIC)),
            Span::styled(format!(" · {count} стр. · "), muted),
            palette.keycap("Ctrl+T"),
        ]));
        return;
    }
    lines.push(Line::from(Span::styled(
        "▾ мысли",
        muted.add_modifier(Modifier::ITALIC),
    )));
    for t in thoughts.lines() {
        lines.push(Line::from(Span::styled(
            format!("│ {t}"),
            muted.add_modifier(Modifier::ITALIC),
        )));
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

/// Один tool-блок (карточка тул-колла): заголовок `⚒ имя(аргументы)` цветом
/// инструмента и результат на гуттере `└`. Аргументы и результат **переносятся
/// по ширине** (не обрезаются).
fn push_tool(lines: &mut Vec<Line<'static>>, tool: &FeedToolCall, palette: &Palette, width: usize) {
    let head_style = Style::default()
        .fg(palette.tool_soft)
        .add_modifier(Modifier::BOLD);
    let header = if tool.arguments.trim().is_empty() {
        tool.name.clone()
    } else {
        format!("{}({})", tool.name, tool.arguments)
    };
    // Первый ряд с иконкой ⚒ (эмодзи-глиф шириной 2 — за ним два пробела, чтобы он
    // не сливался с именем), продолжения выравниваем под имя.
    push_wrapped(lines, "⚒  ", "   ", &header, width, head_style);
    if !tool.result.is_empty() {
        let body_style = Style::default().fg(palette.muted);
        push_wrapped(lines, "└ ", "  ", &tool.result, width, body_style);
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
            // markdown → Text; переносим строки в общий буфер. Для сообщения
            // пользователя одиночные переводы строки (Shift+Enter) сохраняем как
            // реальные переносы (GFM-стиль), иначе текст слился бы в один абзац.
            let rendered = markdown::render_with(&item.text, width, palette, true);
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
        assert!(joined.contains("⚒") && joined.contains("note_save"));
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
        let idx_tool = rows.iter().position(|r| r.contains("web_search")).unwrap();
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

    /// Пустой ли визуальный ряд после снятия гуттер-рейла (`▌` + пробелы).
    fn is_blank_row(r: &str) -> bool {
        r.trim_matches(|c| c == '▌' || c == ' ').is_empty()
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
        let i_tool = rows.iter().position(|r| r.contains("note_save")).unwrap();
        let i_after = rows.iter().position(|r| r.contains("ПОСЛЕ")).unwrap();
        // Между текстом-до и вызовом — ровно одна пустая строка (с рейлом-гуттером).
        assert!(rows[i_before + 1..i_tool].iter().all(|r| is_blank_row(r)));
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
        let i1 = rows.iter().position(|r| r.contains("first_tool")).unwrap();
        let i2 = rows.iter().position(|r| r.contains("second_tool")).unwrap();
        // Между двумя подряд идущими вызовами — ровно одна пустая строка.
        assert_eq!(
            i2 - i1,
            2,
            "между соседними вызовами должна быть одна пустая строка"
        );
        assert!(rows[i1 + 1..i2].iter().all(|r| is_blank_row(r)));
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
        // Результат не обрезан (нет «…») и разложен на несколько рядов (рейл + гуттер
        // `└`/отступ продолжения).
        let gutter_rows = lines
            .iter()
            .filter(|l| {
                let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
                s.contains("слово")
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
    fn followup_starts_new_bubble_and_hides_control_tool() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};

        // A1 (с вызовом send_followup_message) → tool → A2 (new_bubble).
        let mut a1 = Message::assistant("Первое сообщение.");
        a1.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "send_followup_message".into(),
            arguments: serde_json::json!({}),
            result: Some("ок".into()),
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ок");
            m.tool_call_id = Some("c1".into());
            m.tool_name = Some("send_followup_message".into());
            m
        };
        let mut a2 = Message::assistant("Второе сообщение.");
        a2.new_bubble = true;

        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        // Два отдельных пузыря ассистента (не склеены).
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].text, "Первое сообщение.");
        assert_eq!(feed[1].text, "Второе сообщение.");
        // Служебный вызов управляющего инструмента в ленте не показывается.
        assert!(
            feed[0].tools.is_empty(),
            "control-вызов не должен давать tool-блок"
        );
    }

    #[test]
    fn assistant_rounds_without_new_bubble_still_merge() {
        use crate::entities::message::{Message, MessageRole, ToolCallRecord};
        // Обычный agentic-раунд (note_save) по-прежнему склеивается в один пузырь.
        let mut a1 = Message::assistant("Ищу.");
        a1.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "note_save".into(),
            arguments: serde_json::json!({}),
            result: Some("ok".into()),
        }];
        let tool_msg = {
            let mut m = Message::new(MessageRole::Tool, "ok");
            m.tool_call_id = Some("c1".into());
            m
        };
        let a2 = Message::assistant("Готово.");
        let feed = FeedMessage::from_messages(&[a1, tool_msg, a2]);
        assert_eq!(feed.len(), 1, "обычные раунды склеиваются");
        assert_eq!(feed[0].tools.len(), 1, "обычный tool-блок виден");
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
    fn user_message_preserves_single_newlines() {
        // Сообщение пользователя с Shift+Enter (одиночный \n) должно сохранять
        // переносы строк, а не сливаться в один абзац (GFM-стиль).
        let feed = MessageFeed::new();
        let lines = feed.build_lines(
            &[msg(FeedRole::User, "Привет!\nКак дела?", "")],
            &Palette::default(),
            80,
        );
        let rows = row_texts(&lines);
        let i_first = rows.iter().position(|r| r.contains("Привет!")).unwrap();
        let i_second = rows.iter().position(|r| r.contains("Как дела?")).unwrap();
        assert_ne!(
            i_first, i_second,
            "две строки должны оказаться на разных визуальных рядах"
        );
        // И ни одна строка не должна содержать обе фразы (не склеены в одну).
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("Привет!") && r.contains("Как дела?")),
            "строки не должны сливаться в один ряд"
        );
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
    fn rail_is_not_dimmed_next_to_table_borders() {
        // Рейл слева должен иметь чистый цвет роли без line-level модификаторов
        // (напр. DIM у рамок таблиц/разделителей), иначе он другого цвета напротив них.
        let feed = MessageFeed::new();
        let table = "| a | b |\n|---|---|\n| 1 | 2 |";
        let lines = feed.build_lines(
            &[msg(FeedRole::Assistant, table, "")],
            &Palette::default(),
            80,
        );
        for line in &lines {
            // Рейл — первый спан с символом «▌».
            if let Some(rail) = line.spans.first()
                && rail.content.starts_with('▌')
            {
                assert!(
                    !rail.style.add_modifier.contains(Modifier::DIM),
                    "рейл не должен наследовать DIM от строки таблицы/разделителя"
                );
            }
        }
    }

    #[test]
    fn scroll_up_disables_follow() {
        let mut feed = MessageFeed::new();
        assert!(feed.follow);
        feed.scroll_up(3);
        assert!(!feed.follow);
    }

    #[test]
    fn scroll_sets_take_once_flag() {
        // Прокрутка (вверх/вниз) взводит флаг, `take_scrolled` забирает его один раз
        // (петля по нему делает полную перерисовку — стирает артефакты VS16-эмодзи).
        let mut feed = MessageFeed::new();
        assert!(!feed.take_scrolled(), "до прокрутки флаг не взведён");
        feed.scroll_up(3);
        assert!(feed.take_scrolled(), "после прокрутки флаг взведён");
        assert!(!feed.take_scrolled(), "флаг забирается однократно");
        feed.scroll_down(3);
        assert!(feed.take_scrolled());
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
        term.draw(|f| {
            feed.render(
                f,
                f.area(),
                "Чат",
                "gemma-4 · 16k ctx",
                &messages,
                &Palette::default(),
            )
        })
        .unwrap();
    }
}
