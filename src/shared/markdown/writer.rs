//! Markdown — walker событий pulldown-cmark → строки (Writer). Часть модуля [`super`]; разбито из монолита
//! markdown.rs (см. docs/history/refactoring-god-objects.md, этап 6).

use super::*;

/// Накопитель строк: разворачивает поток событий pulldown-cmark в `Vec<Line>`.
pub(super) struct Writer {
    palette: Palette,
    /// Ширина панели в колонках (раскладка таблиц).
    width: usize,
    pub(super) lines: Vec<Line<'static>>,
    /// Стек инлайн-стилей (вершина — текущий).
    inline_styles: Vec<Style>,
    /// Префиксы строк (для цитат: `>`), применяются в [`Writer::push_line`].
    line_prefixes: Vec<Span<'static>>,
    /// Стек стилей строк (цитаты/блок кода).
    line_styles: Vec<Style>,
    /// Стек индексов списков (`None` — маркированный, `Some` — нумерованный).
    list_indices: Vec<Option<u64>>,
    /// Накопленный URL ссылки (добавляется при закрытии тега).
    link: Option<String>,
    /// Накопленный URL изображения (отдельно от `link` — картинка бывает в ссылке).
    image: Option<String>,
    /// Подсветчик активного блока кода.
    code_highlighter: Option<HighlightLines<'static>>,
    /// Активный сбор таблицы (вне таблицы — `None`).
    table: Option<TableBuilder>,
    /// Нужен ли пустой разделитель перед следующим блоком.
    needs_newline: bool,
    /// Только что открыт элемент списка (строка маркера `1. `/`- ` уже добавлена),
    /// и первый абзац этого элемента должен продолжаться **на строке маркера**, а не
    /// на новой строке. В «рыхлых» (loose) списках pulldown-cmark оборачивает
    /// содержимое элемента в `Paragraph`; без этого флага номер оставался бы на одной
    /// строке, а текст уезжал на следующую. Сбрасывается в начале любого `Start(tag)`
    /// и на `End(Item)` — иначе в **тугом** (tight) списке, где обёртки `Paragraph`
    /// нет, флаг некому потребить и он «утекал» на следующий блок.
    item_marker_open: bool,
    /// Трактовать «мягкий» перенос (одиночный `\n`) как реальный перенос строки
    /// (GFM-стиль). Для сообщений пользователя — `true`. См. [`RenderOpts`].
    pub(super) soft_break_as_newline: bool,
    /// Горизонтальные разделители между строками тела таблиц. См. [`RenderOpts`].
    pub(super) table_row_separators: bool,
    /// Рендерить ```mermaid-блоки диаграммой. См. [`RenderOpts`] и подмодуль
    /// [`super::mermaid`].
    pub(super) render_mermaid: bool,
    /// Активный сбор ```mermaid-блока: `(инфо-строка забора, исходник)`.
    /// Пока `Some`, события `Text` копятся сюда (по образцу [`TableBuilder`]), а
    /// решение «диаграмма или фолбэк-исходник» принимает [`Writer::end_codeblock`].
    /// Инфо-строка хранится целиком (` ```mermaid title=x `) — фолбэк печатает её
    /// в заборе байт-в-байт, как прежний путь.
    mermaid: Option<(String, String)>,
    /// Закрыт ли забор код-блока, который сейчас открывается (выставляется в
    /// [`Writer::run`] перед каждым `Start(CodeBlock)` по исходнику, см.
    /// [`fenced_block_is_closed`]). Нужен только mermaid-пути: pulldown-cmark
    /// закрывает незакрытый забор в конце документа сам, поэтому недописанный
    /// стримом блок по событиям неотличим от полного — а рендерить диаграмму из
    /// огрызка нельзя (мерцание «частичная диаграмма ↔ исходник» по мере прихода
    /// чанков). Незакрытый блок идёт путём исходника; когда закрывающий забор
    /// доедет, кэш ленты пересчитает сообщение и подменит исходник диаграммой.
    codeblock_closed: bool,
}

impl Writer {
    pub(super) fn new(palette: Palette, width: usize) -> Self {
        Self {
            palette,
            width,
            lines: Vec::new(),
            inline_styles: Vec::new(),
            line_prefixes: Vec::new(),
            line_styles: Vec::new(),
            list_indices: Vec::new(),
            link: None,
            image: None,
            code_highlighter: None,
            table: None,
            needs_newline: false,
            item_marker_open: false,
            soft_break_as_newline: false,
            table_row_separators: false,
            render_mermaid: false,
            mermaid: None,
            codeblock_closed: true,
        }
    }

    /// Прогоняет поток событий с байтовыми диапазонами (`Parser::into_offset_iter`
    /// над `src`). Диапазоны нужны единственной проверке — закрыт ли забор
    /// открываемого код-блока (у `Start(Tag)` диапазон покрывает элемент целиком);
    /// сама проверка делается только при включённом рендере mermaid.
    pub(super) fn run<'a, I: Iterator<Item = (Event<'a>, std::ops::Range<usize>)>>(
        &mut self,
        src: &str,
        iter: I,
    ) {
        for (event, range) in iter {
            if self.render_mermaid && matches!(event, Event::Start(Tag::CodeBlock(_))) {
                self.codeblock_closed = fenced_block_is_closed(src, &range);
            }
            self.handle_event(event);
        }
    }

    pub(super) fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            // Одиночный перевод строки: в ленте пользователя сохраняем как реальный
            // перенос (как HardBreak), иначе — стандартный мягкий перенос (пробел).
            // В ячейке таблицы всегда пробел (раскладку строк делает таблица).
            Event::SoftBreak if self.soft_break_as_newline && !self.in_table_cell() => {
                self.push_line(Line::default())
            }
            Event::SoftBreak => self.push_span(Span::raw(" ")),
            // В ячейке перенос строки не делаем — продолжаем пробелом.
            Event::HardBreak if self.in_table_cell() => self.push_span(Span::raw(" ")),
            Event::HardBreak => self.push_line(Line::default()),
            // `<br>` (частый в ячейках таблиц у моделей) — как HardBreak; прочий
            // inline/block HTML игнорируем (текст между тегами приходит как `Text`).
            Event::InlineHtml(html) | Event::Html(html) if is_br(&html) => {
                if self.in_table_cell() {
                    self.push_span(Span::raw(" "));
                } else {
                    self.push_line(Line::default());
                }
            }
            Event::Rule => self.rule(),
            Event::TaskListMarker(checked) => self.task_list_marker(checked),
            Event::InlineMath(content) => {
                let style = self.current_style();
                if looks_like_price_fragment(&content) {
                    // Ложное срабатывание math на диапазоне цен «$5-$10»: pulldown
                    // отдаёт content="5-". Печатаем литералом с долларами, а не как
                    // формулу (иначе доллары исчезли бы, «5-10» рвалось на «5-» и «10»).
                    self.push_span(Span::styled(format!("${content}$"), style));
                } else {
                    self.push_span(Span::styled(latex_to_unicode(&content), style));
                }
            }
            Event::DisplayMath(content) => self.display_math(&content),
            // HTML, сноски — игнорируем.
            _ => {}
        }
    }

    pub(super) fn start_tag(&mut self, tag: Tag<'_>) {
        // Любой блочный `Start` «закрывает» ожидание содержимого элемента списка;
        // значение сохраняем для первого абзаца (он продолжает строку маркера).
        let marker_open = std::mem::take(&mut self.item_marker_open);
        match tag {
            Tag::Paragraph => self.start_paragraph(marker_open),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::CodeBlock(kind) => self.start_codeblock(kind),
            Tag::List(start_index) => self.start_list(start_index),
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.push_inline_style(Style::new().italic()),
            Tag::Strong => self.push_inline_style(Style::new().bold()),
            Tag::Strikethrough => self.push_inline_style(Style::new().crossed_out()),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                // Автолинк (`<url>`) и email печатают URL как текст сами — суффикс
                // ` (url)` дал бы дубль. Для них ссылку не запоминаем.
                if !matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    self.link = Some(dest_url.into_string());
                }
            }
            Tag::Image { dest_url, .. } => self.image = Some(dest_url.into_string()),
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead => {
                if let Some(tb) = &mut self.table {
                    tb.current_row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(tb) = &mut self.table {
                    tb.current_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(tb) = &mut self.table {
                    tb.current_cell = Some(Vec::new());
                }
            }
            _ => {}
        }
    }

    pub(super) fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.needs_newline = true,
            TagEnd::Heading(_) => self.needs_newline = true,
            TagEnd::BlockQuote(_) => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            // Сброс «маркер открыт»: в ТУГОМ списке содержимое элемента — инлайн без
            // обёртки `Paragraph`, поэтому флаг некому потребить, и он «утекал» на
            // следующий блок (склеивая его с строкой маркера и глотая пустую строку).
            TagEnd::Item => self.item_marker_open = false,
            TagEnd::TableCell => {
                if let Some(tb) = &mut self.table {
                    let cell = tb.current_cell.take().unwrap_or_default();
                    tb.current_row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(tb) = &mut self.table {
                    tb.head = std::mem::take(&mut tb.current_row);
                }
            }
            TagEnd::TableRow => {
                if let Some(tb) = &mut self.table {
                    let row = std::mem::take(&mut tb.current_row);
                    tb.rows.push(row);
                }
            }
            TagEnd::Table => self.end_table(),
            _ => {}
        }
    }

    pub(super) fn start_paragraph(&mut self, marker_open: bool) {
        // Первый абзац «рыхлого» элемента списка продолжается на строке маркера
        // (`1. `/`- `), а не начинает новую — иначе номер отрывается от текста.
        if marker_open {
            self.needs_newline = false;
            return;
        }
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.push_line(Line::default());
        self.needs_newline = false;
    }

    pub(super) fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        let lvl = heading_number(level);
        let style = heading_style(lvl, &self.palette);
        let prefix = format!("{} ", "#".repeat(lvl as usize));
        self.push_line(Line::styled(prefix, style));
        self.needs_newline = false;
    }

    pub(super) fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        self.line_prefixes.push(Span::from("> "));
        self.line_styles.push(blockquote_style());
    }

    pub(super) fn end_blockquote(&mut self) {
        self.line_prefixes.pop();
        self.line_styles.pop();
        self.needs_newline = true;
    }

    pub(super) fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    pub(super) fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    pub(super) fn start_item(&mut self) {
        self.push_line(Line::default());
        let width = self.list_indices.len() * 4 - 3;
        if let Some(last_index) = self.list_indices.last_mut() {
            let span = match last_index {
                None => Span::from(" ".repeat(width - 1) + "- "),
                Some(index) => {
                    *index += 1;
                    Span::styled(
                        format!("{:width$}. ", *index - 1),
                        Style::new().fg(self.palette.accent),
                    )
                }
            };
            self.push_span(span);
        }
        self.needs_newline = false;
        // Первый абзац этого элемента должен продолжиться на строке маркера.
        self.item_marker_open = true;
    }

    pub(super) fn task_list_marker(&mut self, checked: bool) {
        let marker = if checked { 'x' } else { ' ' };
        if let Some(line) = self.lines.last_mut() {
            if let Some(first) = line.spans.first_mut() {
                let content = first.content.to_mut();
                if content.ends_with("- ") {
                    let len = content.len();
                    content.truncate(len - 2);
                    content.push_str(&format!("- [{marker}] "));
                    return;
                }
            }
            line.spans.insert(1, Span::from(format!("[{marker}] ")));
        }
    }

    pub(super) fn rule(&mut self) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        // Тянем линию на ширину панели (self.width — уже внутренняя ширина под рейл),
        // чтобы `---` не выглядел обрубком рядом с полноширинными таблицами. Линия
        // ≤ ширины → повторный перенос в `message_feed` — no-op.
        let rule = "─".repeat(self.width.max(3));
        self.push_line(Line::from(rule).add_modifier(Modifier::DIM));
        self.needs_newline = true;
    }

    pub(super) fn start_codeblock(&mut self, kind: CodeBlockKind<'_>) {
        if !self.lines.is_empty() {
            self.push_line(Line::default());
        }
        let info = match kind {
            CodeBlockKind::Fenced(ref lang) => lang.as_ref(),
            CodeBlockKind::Indented => "",
        };
        // Инфо-строка может нести не только язык: ` ```rust,no_run `, ` ```py title=x `.
        // Синтаксис резолвим по первому токену, а в заборчик печатаем метку целиком.
        let lang = info.split([',', ' ', '\t']).next().unwrap_or("");
        // ```mermaid-блок при включённом рендере диаграмм НЕ печатается сразу:
        // содержимое копится в буфер (как ячейки таблицы в TableBuilder), а забор/
        // подсветка не трогаются — решение «диаграмма или исходник» принимает
        // end_codeblock, когда виден весь блок. Блок с незакрытым забором
        // (стримящийся хвост ответа) в буфер НЕ берётся — идёт обычным путём
        // исходника, пока сервер не допишет закрывающий забор (иначе частичная
        // диаграмма мерцала бы). См. super::mermaid и spec §11.4.
        if self.render_mermaid
            && self.codeblock_closed
            && lang.eq_ignore_ascii_case("mermaid")
            && self.table.is_none()
        {
            self.mermaid = Some((info.to_string(), String::new()));
            return;
        }
        if let Some(syntax) = resolve_syntax(lang) {
            let theme = code_theme(&self.palette);
            self.code_highlighter = Some(HighlightLines::new(syntax, theme));
        } else {
            self.line_styles.push(code_style());
        }
        self.push_line(Line::from(format!("```{info}")).add_modifier(Modifier::DIM));
        // Содержимое блока должно начаться на новой строке под открывающим `​```​`, а
        // не приклеиться к нему. В неподсвеченном пути (`text`) первая строка иначе
        // допишется в строку заборчика (`i==0`, `needs_newline==false`); подсвеченный
        // путь этот флаг игнорирует (кладёт строки сам).
        self.needs_newline = true;
    }

    pub(super) fn end_codeblock(&mut self) {
        // Буферизованный ```mermaid-блок: пробуем диаграмму, при любом отказе
        // (тип вне whitelist / парсер / ширина) — исходник код-блоком, как при
        // выключенном рендере. Забор/подсветка этого пути не открывались, поэтому
        // обычное закрытие ниже не выполняется.
        if let Some((info, src)) = self.mermaid.take() {
            match render_mermaid_block(&src, self.width, &self.palette) {
                Some(lines) => {
                    for line in lines {
                        self.push_line(line);
                    }
                }
                None => self.emit_fenced_source(&info, &src),
            }
            self.needs_newline = true;
            return;
        }
        self.push_line(Line::from("```").add_modifier(Modifier::DIM));
        self.needs_newline = true;
        if self.code_highlighter.take().is_none() {
            self.line_styles.pop();
        }
    }

    /// Печатает исходник код-блока в прежнем «неподсвеченном» виде (реверс-стиль,
    /// DIM-заборы с инфо-строкой) — фолбэк ```mermaid-блока. Вид байт-в-байт
    /// повторяет старый путь `start_codeblock`(без синтаксиса)+`text`+`end_codeblock`,
    /// см. golden-тест `mermaid_fallback_matches_disabled_render`.
    fn emit_fenced_source(&mut self, info: &str, src: &str) {
        self.line_styles.push(code_style());
        self.push_line(Line::from(format!("```{info}")).add_modifier(Modifier::DIM));
        for line in src.lines() {
            self.push_line(Line::default());
            self.push_span(Span::styled(line.to_string(), Style::default()));
        }
        self.push_line(Line::from("```").add_modifier(Modifier::DIM));
        self.line_styles.pop();
    }

    pub(super) fn text(&mut self, text: CowStr<'_>) {
        // Сбор ```mermaid-блока: копим исходник как есть (с переводами строк —
        // устойчиво к любому дроблению текста на события парсером).
        if let Some((_, buf)) = &mut self.mermaid {
            buf.push_str(&text);
            return;
        }
        if let Some(highlighter) = &mut self.code_highlighter {
            // На ошибку syntect/ansi строку не выбрасываем, а показываем плоской
            // (`highlight_line_or_plain`) — иначе строка кода молча пропадала бы.
            let mut rendered: Vec<Line<'static>> = Vec::new();
            for line in LinesWithEndings::from(&text) {
                rendered.extend(highlight_line_or_plain(highlighter, line));
            }
            self.lines.extend(rendered);
            self.needs_newline = false;
            return;
        }
        let style = self.current_style();
        // В ячейке таблицы переносов нет — кладём как один спан (переносы строк
        // схлопываем в пробел; реальный перенос по ширине делает раскладка).
        if self.in_table_cell() {
            self.push_span(Span::styled(text.replace('\n', " "), style));
            return;
        }
        for (i, line) in text.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            self.push_span(Span::styled(line.to_string(), style));
        }
    }

    pub(super) fn code(&mut self, code: CowStr<'_>) {
        self.push_span(Span::styled(
            code.into_string(),
            inline_code_style(&self.palette),
        ));
    }

    /// Блочная формула `$$…$$`: каждая строка преобразованного содержимого — на
    /// своей строке ленты. Первую строку кладём **в текущую (пустую) строку** абзаца
    /// через `push_span` — иначе между прозой и формулой оставался бы двойной пропуск
    /// (`start_paragraph` уже открыл пустую строку, а прежний код клал контент новой).
    /// В ячейке таблицы формула остаётся **в ячейке** (строки склеиваем `«; »`),
    /// иначе `push_line` увёл бы её строки в ленту над таблицей.
    pub(super) fn display_math(&mut self, content: &str) {
        let converted = latex_to_unicode_display(content);
        if self.in_table_cell() {
            let joined = converted.split('\n').collect::<Vec<_>>().join("; ");
            self.push_span(Span::raw(joined));
            return;
        }
        for (i, line) in converted.split('\n').enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            self.push_span(Span::raw(line.to_string()));
        }
        self.needs_newline = true;
    }

    pub(super) fn end_link(&mut self) {
        if let Some(url) = self.link.take() {
            self.push_span(Span::from(" ("));
            self.push_span(Span::styled(url, link_style(&self.palette)));
            self.push_span(Span::from(")"));
        }
    }

    /// Закрытие изображения `![alt](url)`: alt-текст уже напечатан событиями `Text`,
    /// дописываем URL в скобках (как у ссылки). Отдельное поле `image` — картинка
    /// бывает вложена в ссылку, `self.link` перезатирать нельзя.
    pub(super) fn end_image(&mut self) {
        if let Some(url) = self.image.take() {
            self.push_span(Span::from(" ("));
            self.push_span(Span::styled(url, link_style(&self.palette)));
            self.push_span(Span::from(")"));
        }
    }

    pub(super) fn start_table(&mut self, alignments: Vec<Alignment>) {
        if self.needs_newline {
            self.push_line(Line::default());
        }
        self.table = Some(TableBuilder {
            alignments,
            head: Vec::new(),
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: None,
        });
        self.needs_newline = false;
    }

    pub(super) fn end_table(&mut self) {
        if let Some(tb) = self.table.take() {
            for line in render_table(&tb, self.width, &self.palette, self.table_row_separators) {
                self.lines.push(line);
            }
        }
        self.needs_newline = true;
    }

    /// Идёт ли сейчас сбор содержимого ячейки таблицы.
    pub(super) fn in_table_cell(&self) -> bool {
        self.table
            .as_ref()
            .is_some_and(|tb| tb.current_cell.is_some())
    }

    pub(super) fn current_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    pub(super) fn push_inline_style(&mut self, style: Style) {
        let merged = self.current_style().patch(style);
        self.inline_styles.push(merged);
    }

    pub(super) fn push_line(&mut self, line: Line<'static>) {
        let style = self.line_styles.last().copied().unwrap_or_default();
        let mut line = line.patch_style(style);
        // Префиксы строк (цитаты) — в начало, в обратном порядке стека.
        for prefix in self.line_prefixes.iter().rev().cloned() {
            line.spans.insert(0, prefix);
        }
        self.lines.push(line);
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        // Внутри ячейки таблицы спаны накапливаются в ячейку, а не в ленту.
        if let Some(tb) = &mut self.table
            && let Some(cell) = &mut tb.current_cell
        {
            cell.push(span);
            return;
        }
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }
}

/// Закрыт ли fenced-код-блок в исходнике. `range` — байтовый диапазон **всего**
/// блока из `OffsetIter` pulldown-cmark (у `Start(CodeBlock)` диапазон покрывает
/// элемент целиком). CommonMark дотягивает незакрытый забор до конца документа,
/// поэтому по событиям парсера обрыв стрима неотличим от полного блока — смотрим
/// в исходник: у закрытого последняя строка диапазона — закрывающий забор (та же
/// литера, не короче открывающего), у оборванного — строка содержимого.
///
/// Проверка нарочно простая (точность CommonMark не нужна): блок, за которым в
/// документе есть ещё текст, закрыт по построению; ложное «закрыт» на экзотике
/// (контентная строка, неотличимая от забора) лишь приведёт к попытке рендера,
/// которая упадёт парсером диаграммы → штатный фолбэк на исходник.
pub(super) fn fenced_block_is_closed(src: &str, range: &std::ops::Range<usize>) -> bool {
    if range.end < src.len() {
        return true; // за блоком есть текст — забор закрыт (обрыв тянулся бы до конца)
    }
    let block = &src[range.clone()];
    let mut lines = block.lines();
    let Some(open) = lines.next() else {
        return false;
    };
    // Префикс контейнера (цитата `> `) и отступ забора (≤3 пробелов) не мешают.
    let open = open.trim_start_matches(['>', ' ', '\t']);
    let Some(fence @ ('`' | '~')) = open.chars().next() else {
        return true; // indented-блок без забора — «закрывать» нечего
    };
    let open_len = open.chars().take_while(|&c| c == fence).count();
    let Some(last) = lines.last() else {
        return false; // одна строка — только открывающий забор
    };
    let last = last.trim_start_matches(['>', ' ', '\t']);
    let close_len = last.chars().take_while(|&c| c == fence).count();
    // Литеры забора — ASCII, срез по счётчику символов безопасен.
    close_len >= open_len && last[close_len..].trim().is_empty()
}

/// Эвристика «это диапазон/дробь цен, а не формула»: `$5-$10` парсер math отдаёт
/// как `InlineMath("5-")`. Содержимое из одних цифр/точек/запятых/пробелов/дефисов/
/// слэшей И оканчивающееся на разделитель (`-`/`–`/`/`) — сигнатура диапазона цен;
/// законное число оканчивается цифрой, а формула содержит math-символы.
pub(super) fn looks_like_price_fragment(content: &str) -> bool {
    let t = content.trim();
    if t.is_empty() {
        return false;
    }
    t.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | ' ' | '-' | '–' | '/'))
        && t.ends_with(['-', '–', '/'])
}

/// Распознаёт тег переноса строки `<br>` в его формах (регистр игнорируется).
pub(super) fn is_br(html: &str) -> bool {
    matches!(
        html.trim().to_ascii_lowercase().as_str(),
        "<br>" | "<br/>" | "<br />"
    )
}

pub(super) fn heading_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;

    #[test]
    fn render_produces_owned_text() {
        let text = render("# Заголовок\n\nабзац с `кодом`.", 80, &Palette::default());
        assert!(!text.lines.is_empty());
        let collected = rendered_text("# Заголовок\n\nабзац с `кодом`.");
        assert!(collected.contains("Заголовок"));
    }

    #[test]
    fn render_headings_lists_quotes() {
        let collected = rendered_text("## Заголовок\n\n- пункт раз\n- пункт два\n\n> цитата");
        assert!(collected.contains("## Заголовок"));
        assert!(collected.contains("- пункт раз"));
        assert!(collected.contains("> цитата"));
    }

    /// «Рыхлый» (loose) нумерованный список — элементы разделены пустой строкой,
    /// поэтому pulldown-cmark оборачивает содержимое в `Paragraph`. Номер и текст
    /// должны остаться на **одной** строке (`1. текст`), а не разъехаться (регрессия:
    /// `start_paragraph` безусловно добавлял новую строку после маркера).
    #[test]
    fn loose_ordered_list_keeps_number_with_text() {
        let md = "1. **Первый.** Текст первого пункта.\n\n\
                  2. **Второй.** Текст второго пункта.\n\n\
                  3. **Третий.** Текст третьего пункта.";
        let collected = rendered_text(md);
        // Номер приклеен к своему тексту на одной строке ленты.
        assert!(
            collected.contains("1. Первый."),
            "номер оторвался от текста:\n{collected}"
        );
        assert!(collected.contains("2. Второй."));
        assert!(collected.contains("3. Третий."));
        // Пустой строки между маркером и его текстом быть не должно.
        assert!(
            !collected.contains("1. \n"),
            "после маркера образовался перенос:\n{collected}"
        );
    }

    /// Многоабзацный элемент «рыхлого» списка: первый абзац — на строке маркера,
    /// последующие — на своих строках (маркер не дублируется).
    #[test]
    fn loose_list_item_second_paragraph_on_own_line() {
        let md = "1. Первый абзац.\n\n   Второй абзац того же пункта.\n\n2. Другой пункт.";
        let collected = rendered_text(md);
        assert!(collected.contains("1. Первый абзац."));
        assert!(collected.contains("Второй абзац того же пункта."));
        assert!(collected.contains("2. Другой пункт."));
    }

    /// Строки рендера как вектор строк (для проверки раскладки по рядам).
    fn rows(input: &str, width: usize) -> Vec<String> {
        render(input, width, &Palette::default())
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Между прозой и блочной формулой `$$…$$` — ровно одна пустая строка (регрессия:
    /// было две — `start_paragraph` открывал пустую строку, а `display_math` клал
    /// контент новой, оставляя ту пустой).
    #[test]
    fn display_math_single_blank_line_before() {
        let lines = rows("текст\n\n$$E=mc^2$$\n\nконец", 80);
        let text_idx = lines.iter().position(|l| l.contains("текст")).unwrap();
        let math_idx = lines.iter().position(|l| l.contains("E=mc")).unwrap();
        assert_eq!(
            math_idx - text_idx,
            2,
            "ожидалась одна пустая строка между прозой и формулой:\n{lines:?}"
        );
        assert!(lines[text_idx + 1].trim().is_empty());
    }

    /// Блочная формула внутри цитаты сохраняет префикс `> `.
    #[test]
    fn display_math_in_blockquote_keeps_prefix() {
        let lines = rows("> $$x+1$$", 80);
        let math = lines.iter().find(|l| l.contains("x+1")).unwrap();
        assert!(math.starts_with("> "), "префикс цитаты потерян: {math:?}");
    }

    /// Формула `$$…$$` в ячейке таблицы остаётся **в ячейке**, не утекая строкой над
    /// таблицей (регрессия: `display_math` звал `push_line` без учёта `in_table_cell`).
    #[test]
    fn display_math_in_table_cell_stays_in_cell() {
        let md = "| A | B |\n| :--- | :--- |\n| $$x+1$$ | y |";
        let lines = rows(md, 60);
        let first = lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .expect("пустой рендер");
        assert!(
            first.contains('┌'),
            "первая строка должна быть верхней рамкой таблицы, а не формулой: {first:?}"
        );
        let joined = lines.join("\n");
        assert!(joined.contains("x+1"), "формула потеряна: {joined}");
    }

    /// Автолинк `<url>` печатает URL один раз (регрессия: `end_link` дописывал
    /// ` (url)` поверх текста-URL → «url (url)»).
    #[test]
    fn autolink_url_not_duplicated() {
        let collected = rendered_text("см. <https://example.com> тут");
        assert_eq!(
            collected.matches("example.com").count(),
            1,
            "URL автолинка продублирован: {collected}"
        );
    }

    /// Обычная ссылка `[t](u)` по-прежнему печатает URL в скобках.
    #[test]
    fn inline_link_keeps_url_suffix() {
        let collected = rendered_text("[текст](https://example.com)");
        assert!(
            collected.contains("текст (https://example.com)"),
            "{collected}"
        );
    }

    /// `<br>` в абзаце — перенос строки.
    #[test]
    fn br_tag_breaks_line_in_paragraph() {
        let lines = rows("a<br>b", 80);
        assert!(lines.iter().any(|l| l.trim() == "a"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "b"), "{lines:?}");
    }

    /// `<br>` в ячейке таблицы — пробел (переносов в ячейке нет).
    #[test]
    fn br_tag_in_table_cell_becomes_space() {
        let md = "| A | B |\n| :--- | :--- |\n| x<br>y | z |";
        let joined = rendered_text_w(md, 60);
        assert!(
            joined.contains("x y"),
            "<br> в ячейке должен стать пробелом: {joined}"
        );
    }

    /// Метка языка с инфо-строкой (` ```rust,no_run `) резолвится по первому токену —
    /// блок подсвечивается (регрессия: вся инфо-строка не совпадала с синтаксисом).
    #[test]
    fn code_block_info_string_resolves_language() {
        use crate::shared::config::Theme;
        let colors = fg_colors(
            "```rust,no_run\nfn main() {}\n```",
            &Palette::for_theme(Theme::Dark),
        );
        assert!(
            colors.iter().any(|c| matches!(c, Color::Rgb(..))),
            "ожидалась подсветка по языку из инфо-строки"
        );
    }

    /// Горизонтальная линия `---` тянется на ширину панели (не обрубок `───`).
    #[test]
    fn rule_spans_panel_width() {
        let w = 40;
        let text = render("текст\n\n---\n\nещё", w, &Palette::default());
        let rule = text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('─')))
            .expect("нет линии-разделителя");
        let width: usize = rule
            .spans
            .iter()
            .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
            .sum();
        assert_eq!(width, w, "линия должна занимать ширину панели");
    }

    /// Тугой (tight) элемент списка не «утекает» на следующий блок: после
    /// `3. Название` с пустой строкой идущий следом абзац не приклеивается к строке
    /// маркера, а пустая строка сохраняется (регрессия: `item_marker_open` в тугом
    /// списке не потреблялся — контент элемента инлайновый, без обёртки `Paragraph`).
    /// Сцена — трек-лист альбома: `3. Трек` + теги Suno на следующих строках.
    #[test]
    fn tight_list_item_does_not_bleed_into_next_block() {
        let md = "3. Взгляд из ниоткуда\n\n[Intro - Ambient]\nЯ — тишина.";
        let lines = rows(md, 100);
        let marker = lines
            .iter()
            .find(|l| l.contains("3. Взгляд из ниоткуда"))
            .expect("нет строки маркера");
        assert!(
            !marker.contains("[Intro"),
            "следующий блок приклеился к строке маркера: {marker:?}"
        );
        let marker_idx = lines.iter().position(|l| l.contains("3. Взгляд")).unwrap();
        let intro_idx = lines.iter().position(|l| l.contains("[Intro")).unwrap();
        assert!(
            intro_idx > marker_idx,
            "теги не на отдельной строке: {lines:?}"
        );
        // между маркером и тегами — сохранённая пустая строка
        assert!(
            lines[marker_idx + 1..intro_idx]
                .iter()
                .any(|l| l.trim().is_empty()),
            "потеряна пустая строка после названия трека: {lines:?}"
        );
    }

    /// Обычный тугой список (несколько пунктов) не сломан: каждый пункт — на своей
    /// строке, следующий не приклеивается к предыдущему.
    #[test]
    fn tight_list_items_stay_on_separate_lines() {
        let lines = rows("- один\n- два\n- три", 80);
        assert!(lines.iter().any(|l| l.trim() == "- один"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "- два"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "- три"), "{lines:?}");
    }

    /// Изображение печатает alt-текст и URL в скобках (как ссылка).
    #[test]
    fn image_prints_alt_and_url() {
        let collected = rendered_text("![схема](http://x/i.png)");
        assert!(collected.contains("схема (http://x/i.png)"), "{collected}");
    }

    /// Диапазон/дробь цен `$5-$10` не съедается math-расширением (доллары остаются,
    /// «5-10» не рвётся).
    #[test]
    fn price_range_not_treated_as_math() {
        let c = rendered_text("товар $5-$10 или $5/$7");
        assert!(c.contains("$5-$10"), "{c}");
        assert!(c.contains("$5/$7"), "{c}");
    }

    /// Рендер с включённым флагом mermaid (остальные флаги дефолтные).
    fn render_mermaid_on(input: &str, width: usize) -> Text<'static> {
        render_with(
            input,
            width,
            &Palette::default(),
            RenderOpts {
                render_mermaid: true,
                ..Default::default()
            },
        )
    }

    /// Валидный mermaid-блок при включённом рендере — диаграмма вместо исходника:
    /// box-drawing рамки есть, заборов ``` и сырых `-->` нет.
    #[test]
    fn mermaid_block_renders_diagram_when_enabled() {
        let md = "до\n\n```mermaid\nsequenceDiagram\n    participant A\n    participant B\n    A->>B: hi\n```\n\nпосле";
        let text = render_mermaid_on(md, 90);
        let joined: String = text
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains('┌'), "нет рамок диаграммы:\n{joined}");
        assert!(
            !joined.contains("```"),
            "забор не должен печататься:\n{joined}"
        );
        assert!(!joined.contains("->>"), "исходник утёк в ленту:\n{joined}");
        assert!(
            joined.contains("до") && joined.contains("после"),
            "{joined}"
        );
    }

    /// Golden-фолбэк: при любом отказе (мусор / тип вне whitelist / не влезло по
    /// ширине) вывод с включённым флагом **побайтно равен** выводу с выключенным —
    /// строки И стили (Line: PartialEq). Гарантия «худший случай = прежнее поведение».
    #[test]
    fn mermaid_fallback_matches_disabled_render() {
        let cases = [
            // мусор в блоке (LLM недописал/сломал синтаксис)
            ("```mermaid\nпросто текст без диаграммы\n```", 90),
            // тип вне whitelist (pie)
            ("```mermaid\npie title X\n    \"A\" : 1\n```", 90),
            // валидная, но не влезает в узкую панель
            (
                "```mermaid\nsequenceDiagram\n    participant Client\n    participant Server\n    Client->>Server: GET /api/data\n```",
                20,
            ),
            // инфо-строка с хвостом после языка сохраняется в заборе фолбэка
            ("```mermaid title=x\nне диаграмма\n```", 90),
        ];
        for (md, w) in cases {
            let on = render_mermaid_on(md, w);
            let off = render(md, w, &Palette::default());
            assert_eq!(
                on.lines, off.lines,
                "фолбэк разошёлся с прежним видом (w={w}):\n{md}"
            );
        }
    }

    /// Выключенный флаг (Default) — прежнее поведение: исходник код-блоком.
    #[test]
    fn mermaid_flag_off_keeps_source() {
        let md = "```mermaid\nsequenceDiagram\n    A->>B: hi\n```";
        let joined = rendered_text(md);
        assert!(joined.contains("```mermaid"), "{joined}");
        assert!(joined.contains("A->>B: hi"), "{joined}");
    }

    /// Стрим: блок с незакрытым забором (сервер ещё дописывает диаграмму)
    /// печатается исходником **байт-в-байт как при выключенном рендере** — без
    /// мерцания «частичная диаграмма ↔ исходник» по мере прихода чанков.
    /// Регрессия: pulldown-cmark дотягивает незакрытый забор до конца документа,
    /// и синтаксически валидный огрызок (первый кейс) рендерился диаграммой.
    #[test]
    fn mermaid_unclosed_fence_streams_as_source() {
        let cases = [
            // валидный огрызок: без проверки закрытости отрендерился бы диаграммой
            "текст\n\n```mermaid\nflowchart LR\n    A[Старт] --> B[Конец]",
            // то же с хвостовым переводом строки
            "```mermaid\nsequenceDiagram\n    A->>B: hi\n",
            // обрыв на полуслове
            "```mermaid\nsequenceDiagram\n    participant Ser",
            // только открывающий забор
            "```mermaid",
            // тильда-забор
            "~~~mermaid\nflowchart LR\n    A --> B",
            // закрывающий короче открывающего — блок НЕ закрыт
            "````mermaid\nflowchart LR\n    A --> B\n```",
        ];
        for md in cases {
            let on = render_mermaid_on(md, 90);
            let off = render(md, 90, &Palette::default());
            assert_eq!(
                on.lines, off.lines,
                "незакрытый блок должен идти исходником:\n{md}"
            );
        }
    }

    /// Закрывающий забор доехал — блок рендерится диаграммой, даже когда после
    /// него сообщение продолжает стримиться (закрытость — свойство блока, не
    /// конца сообщения). Тильда-забор равноправен.
    #[test]
    fn mermaid_closed_fence_renders_even_while_tail_streams() {
        for md in [
            "```mermaid\nflowchart LR\n    A[Старт] --> B[Конец]\n```\n\nа дальше стримится тек",
            "~~~mermaid\nflowchart LR\n    A[Старт] --> B[Конец]\n~~~",
        ] {
            let text = render_mermaid_on(md, 90);
            let joined: String = text
                .lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect();
            assert!(joined.contains("Старт"), "{joined}");
            assert!(
                !joined.contains("```") && !joined.contains("~~~"),
                "забор не должен печататься: {joined}"
            );
            assert!(!joined.contains("-->"), "исходник утёк в ленту: {joined}");
        }
    }

    /// Кириллический flowchart рендерится диаграммой (регрессия апстрима 0.56.0 —
    /// паника/порча подписей на многобайтовом вводе; наш фикс #29/#30).
    #[test]
    fn mermaid_cyrillic_flowchart_renders() {
        let md = "```mermaid\nflowchart LR\n    A[Старт] -->|да| B[Конец]\n```";
        let text = render_mermaid_on(md, 90);
        let joined: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Конец"), "{joined}");
        assert!(!joined.contains("[Конец]"), "порча подписи: {joined}");
    }

    /// Настоящая математика по-прежнему конвертируется (доллары сняты).
    #[test]
    fn real_math_still_converts() {
        let c = rendered_text(r"значения $3.14$, $2+2$ и $x^2$");
        assert!(c.contains("3.14"), "{c}");
        assert!(
            !c.contains("$3.14$"),
            "доллары не сняты у настоящей формулы: {c}"
        );
        assert!(c.contains("x²"), "{c}");
    }
}
