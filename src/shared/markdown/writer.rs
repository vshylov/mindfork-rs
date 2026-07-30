//! Markdown — pulldown-cmark event walker → lines (Writer). Part of module
//! [`super`]; split out of the markdown.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 6).

use super::*;

/// Line accumulator: unrolls the pulldown-cmark event stream into `Vec<Line>`.
pub(super) struct Writer {
    palette: Palette,
    /// Panel width in columns (table layout).
    width: usize,
    pub(super) lines: Vec<Line<'static>>,
    /// Inline-style stack (top — current).
    inline_styles: Vec<Style>,
    /// Line prefixes (for quotes: `>`), applied in [`Writer::push_line`].
    line_prefixes: Vec<Span<'static>>,
    /// Line-style stack (quotes/code block).
    line_styles: Vec<Style>,
    /// List-index stack (`None` — bulleted, `Some` — numbered).
    list_indices: Vec<Option<u64>>,
    /// Accumulated link URL (appended when the tag closes).
    link: Option<String>,
    /// Accumulated image URL (separate from `link` — an image can sit inside
    /// a link).
    image: Option<String>,
    /// Highlighter for the active code block.
    code_highlighter: Option<HighlightLines<'static>>,
    /// Active table collection (`None` outside a table).
    table: Option<TableBuilder>,
    /// Raw source of the HTML block being collected (`None` outside one).
    /// Block-level HTML arrives as opaque `Html` chunks split by line, and a
    /// tag can straddle two of them — so the block is accumulated whole and
    /// converted once, at [`Writer::end_html_block`]. See [`super::html`].
    html: Option<String>,
    /// Whether a blank separator is needed before the next block.
    needs_newline: bool,
    /// A list item was just opened (the marker line `1. `/`- ` is already
    /// added), and this item's first paragraph should continue **on the
    /// marker's line**, not start a new one. In "loose" lists pulldown-cmark
    /// wraps the item's content in a `Paragraph`; without this flag the
    /// number would stay on one line and the text would slide to the next.
    /// Reset at the start of any `Start(tag)` and at `End(Item)` — otherwise
    /// in a **tight** list, which has no `Paragraph` wrapper, nothing would
    /// consume the flag and it would "leak" onto the next block.
    item_marker_open: bool,
    /// Treat a "soft" break (a single `\n`) as a real line break (GFM style).
    /// For user messages — `true`. See [`RenderOpts`].
    pub(super) soft_break_as_newline: bool,
    /// Horizontal separators between table body rows. See [`RenderOpts`].
    pub(super) table_row_separators: bool,
    /// Render ```mermaid blocks as a diagram. See [`RenderOpts`] and the
    /// [`super::mermaid`] submodule.
    pub(super) render_mermaid: bool,
    /// Active collection of a ```mermaid block: `(fence info string, source)`.
    /// While `Some`, `Text` events accumulate here (modeled on
    /// [`TableBuilder`]), and [`Writer::end_codeblock`] decides "diagram or
    /// fallback source". The info string is stored whole
    /// (` ```mermaid title=x `) — the fallback prints it into the fence
    /// byte-for-byte, like the old path.
    mermaid: Option<(String, String)>,
    /// Whether the fence of the code block currently being opened is closed
    /// (set in [`Writer::run`] before every `Start(CodeBlock)`, from the
    /// source, see [`fenced_block_is_closed`]). Needed only by the mermaid
    /// path: pulldown-cmark itself closes an unclosed fence at the end of the
    /// document, so a stream-truncated block is indistinguishable from a
    /// complete one by events alone — and a diagram can't be rendered from a
    /// stub (flicker "partial diagram ↔ source" as chunks arrive). An
    /// unclosed block goes the source path; once the closing fence arrives,
    /// the feed's cache recomputes the message and swaps the source for the
    /// diagram.
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
            html: None,
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

    /// Runs the event stream with byte ranges (`Parser::into_offset_iter`
    /// over `src`). The ranges are needed by only one check — is the fence of
    /// the code block being opened closed (a `Start(Tag)` range covers the
    /// whole element); the check itself only runs with mermaid rendering
    /// enabled.
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
            // A single line break: in the user's feed keep it as a real break
            // (like HardBreak), otherwise — the standard soft break (a
            // space). In a table cell it's always a space (the table does
            // its own row layout).
            Event::SoftBreak if self.soft_break_as_newline && !self.in_table_cell() => {
                self.push_line(Line::default())
            }
            Event::SoftBreak => self.push_span(Span::raw(" ")),
            // No line break inside a cell — continue with a space.
            Event::HardBreak if self.in_table_cell() => self.push_span(Span::raw(" ")),
            Event::HardBreak => self.push_line(Line::default()),
            // A block of raw HTML: collect it verbatim, and let
            // `end_html_block` extract its text. This arm comes **before**
            // `is_br` on purpose — a `<br>` line inside a larger block belongs
            // in the buffer, in order, not pushed out ahead of it.
            Event::Html(html) if self.html.is_some() => {
                if let Some(buf) = &mut self.html {
                    buf.push_str(&html);
                }
            }
            // `<br>` (frequent in models' table cells) — like HardBreak. Inline
            // HTML needs nothing else: its inner text arrives as `Text`, which
            // is exactly what block HTML does *not* do.
            Event::InlineHtml(html) if is_br(&html) => {
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
                    // A false math match on a price range "$5-$10": pulldown
                    // hands us content="5-". Print it literally with dollar
                    // signs, not as a formula (otherwise the dollars would
                    // vanish and "5-10" would be torn into "5-" and "10").
                    self.push_span(Span::styled(format!("${content}$"), style));
                } else {
                    self.push_span(Span::styled(latex_to_unicode(&content), style));
                }
            }
            Event::DisplayMath(content) => self.display_math(&content),
            // HTML, footnotes — ignored.
            _ => {}
        }
    }

    pub(super) fn start_tag(&mut self, tag: Tag<'_>) {
        // Any block `Start` "closes" the wait for a list item's content;
        // save the value for the first paragraph (it continues the marker's
        // line).
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
                // An autolink (`<url>`) and email print the URL as text
                // themselves — a ` (url)` suffix would duplicate it. Don't
                // remember the link for them.
                if !matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    self.link = Some(dest_url.into_string());
                }
            }
            Tag::Image { dest_url, .. } => self.image = Some(dest_url.into_string()),
            Tag::HtmlBlock => self.html = Some(String::new()),
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
            TagEnd::HtmlBlock => self.end_html_block(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            // Reset "marker open": in a TIGHT list, an item's content is
            // inline with no `Paragraph` wrapper, so nothing consumes the
            // flag, and it would "leak" onto the next block (gluing it to
            // the marker line and swallowing a blank line).
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
        // The first paragraph of a "loose" list item continues on the marker
        // line (`1. `/`- `) rather than starting a new one — otherwise the
        // number would separate from the text.
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
        // This item's first paragraph should continue on the marker's line.
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
        // Stretch the line to the panel width (self.width — already the
        // inner width under the rail), so `---` doesn't look like a stub
        // next to full-width tables. Line ≤ width → the re-wrap in
        // `message_feed` is a no-op.
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
        // The info string can carry more than the language: ` ```rust,no_run `,
        // ` ```py title=x `. Resolve the syntax from the first token, but
        // print the whole label into the fence.
        let lang = info.split([',', ' ', '\t']).next().unwrap_or("");
        // A ```mermaid block, when diagram rendering is enabled, is NOT
        // printed right away: content accumulates in a buffer (like table
        // cells in TableBuilder), and the fence/highlighting are left alone —
        // end_codeblock decides "diagram or source" once the whole block is
        // visible. A block with an unclosed fence (a streaming tail of the
        // reply) is NOT taken into the buffer — it goes the regular source
        // path until the server finishes the closing fence (otherwise a
        // partial diagram would flicker). See super::mermaid and spec §11.4.
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
        // The block's content must start on a new line under the opening
        // `​```​`, not glue onto it. In the unhighlighted path (`text`) the
        // first line would otherwise append to the fence line (`i==0`,
        // `needs_newline==false`); the highlighted path ignores this flag
        // (lays out lines itself).
        self.needs_newline = true;
    }

    pub(super) fn end_codeblock(&mut self) {
        // A buffered ```mermaid block: try the diagram, on any failure (type
        // outside the whitelist / the parser / width) — the source as a code
        // block, as with rendering disabled. The fence/highlighting of this
        // path were never opened, so the regular closing below doesn't run.
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

    /// Prints a code block's source in the previous "unhighlighted" look
    /// (reversed style, DIM fences with the info string) — the ```mermaid
    /// block's fallback. The look repeats the old path byte-for-byte
    /// (`start_codeblock` with no syntax + `text` + `end_codeblock`), see the
    /// golden test `mermaid_fallback_matches_disabled_render`.
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
        // Collecting a ```mermaid block: accumulate the source as-is (with
        // line breaks — resilient to the parser splitting the text into
        // events however it likes).
        if let Some((_, buf)) = &mut self.mermaid {
            buf.push_str(&text);
            return;
        }
        if let Some(highlighter) = &mut self.code_highlighter {
            // On a syntect/ansi error, don't drop the line — show it flat
            // (`highlight_line_or_plain`) — otherwise a line of code would
            // silently disappear.
            let mut rendered: Vec<Line<'static>> = Vec::new();
            for line in LinesWithEndings::from(&text) {
                rendered.extend(highlight_line_or_plain(highlighter, line));
            }
            self.lines.extend(rendered);
            self.needs_newline = false;
            return;
        }
        let style = self.current_style();
        // No line breaks inside a table cell — put it as one span (line
        // breaks collapse into a space; the layout does the real width-based
        // wrapping).
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

    /// Block formula `$$…$$`: each line of the converted content — on its own
    /// feed line. The first line is put **into the current (empty) paragraph
    /// line** via `push_span` — otherwise there would be a double gap between
    /// the prose and the formula (`start_paragraph` already opened an empty
    /// line, and the old code put content on a new one). In a table cell the
    /// formula stays **in the cell** (lines are joined with "; "), otherwise
    /// `push_line` would send its lines into the feed above the table.
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

    /// Closing an image `![alt](url)`: the alt text has already been printed
    /// by `Text` events, append the URL in parens (like a link). A separate
    /// `image` field — an image can be nested in a link, `self.link` must
    /// not be overwritten.
    pub(super) fn end_image(&mut self) {
        if let Some(url) = self.image.take() {
            self.push_span(Span::from(" ("));
            self.push_span(Span::styled(url, link_style(&self.palette)));
            self.push_span(Span::from(")"));
        }
    }

    /// Closing a raw HTML block: show its **text**.
    ///
    /// Before this the block was dropped whole, so a pasted `<table>` — prose
    /// and all — rendered as nothing at all (see [`super::html`] for the
    /// measurement and for why text, rather than the markup or a rebuilt
    /// table). Laid out like a paragraph: logical lines, wrapped later by the
    /// feed.
    pub(super) fn end_html_block(&mut self) {
        let Some(raw) = self.html.take() else { return };
        let lines = html_block_to_lines(&raw);
        if lines.is_empty() {
            // A block that is only a forced break (`<br>` on its own line) kept
            // its blank line before this change, and keeps it now.
            if is_break_only(&raw) {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            return;
        }
        if self.needs_newline {
            self.push_line(Line::default());
        }
        let style = self.current_style();
        for text in lines {
            self.push_line(Line::default());
            self.push_span(Span::styled(text, style));
        }
        self.needs_newline = true;
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

    /// Is a table cell's content currently being collected.
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
        // Line prefixes (quotes) go at the front, in reverse stack order.
        for prefix in self.line_prefixes.iter().rev().cloned() {
            line.spans.insert(0, prefix);
        }
        self.lines.push(line);
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        // Inside a table cell, spans accumulate into the cell, not the feed.
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

/// Whether a fenced code block is closed in the source. `range` — the byte
/// range of the **whole** block from pulldown-cmark's `OffsetIter` (a
/// `Start(CodeBlock)`'s range covers the whole element). CommonMark stretches
/// an unclosed fence to the end of the document, so a stream cutoff is
/// indistinguishable from a complete block by parser events alone — look at
/// the source instead: for a closed block, the range's last line is the
/// closing fence (the same character, no shorter than the opener); for a
/// truncated one, it's a content line.
///
/// The check is deliberately simple (CommonMark precision isn't needed): a
/// block followed by more text in the document is closed by construction; a
/// false "closed" on an edge case (a content line indistinguishable from a
/// fence) only leads to an attempted render, which fails in the diagram
/// parser → the standard fallback to the source.
pub(super) fn fenced_block_is_closed(src: &str, range: &std::ops::Range<usize>) -> bool {
    if range.end < src.len() {
        return true; // there's text after the block — the fence is closed (a cutoff would stretch to the end)
    }
    let block = &src[range.clone()];
    let mut lines = block.lines();
    let Some(open) = lines.next() else {
        return false;
    };
    // A container prefix (a `> ` quote) and fence indent (≤3 spaces) don't
    // interfere.
    let open = open.trim_start_matches(['>', ' ', '\t']);
    let Some(fence @ ('`' | '~')) = open.chars().next() else {
        return true; // an indented block with no fence — nothing to "close"
    };
    let open_len = open.chars().take_while(|&c| c == fence).count();
    let Some(last) = lines.last() else {
        return false; // one line — only the opening fence
    };
    let last = last.trim_start_matches(['>', ' ', '\t']);
    let close_len = last.chars().take_while(|&c| c == fence).count();
    // Fence characters are ASCII — a slice by character count is safe.
    close_len >= open_len && last[close_len..].trim().is_empty()
}

/// Heuristic for "this is a price range/fraction, not a formula": the math
/// parser hands `$5-$10` to us as `InlineMath("5-")`. Content made only of
/// digits/dots/commas/spaces/dashes/slashes AND ending in a separator
/// (`-`/`–`/`/`) is a price-range signature; a legitimate number ends in a
/// digit, while a formula contains math symbols.
pub(super) fn looks_like_price_fragment(content: &str) -> bool {
    let t = content.trim();
    if t.is_empty() {
        return false;
    }
    t.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | ' ' | '-' | '–' | '/'))
        && t.ends_with(['-', '–', '/'])
}

/// Recognizes the line-break tag `<br>` in its forms (case-insensitive).
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

    /// A "loose" ordered list — items separated by a blank line, so
    /// pulldown-cmark wraps the content in a `Paragraph`. The number and the
    /// text must stay on **one** line (`1. text`), not split apart
    /// (regression: `start_paragraph` unconditionally added a new line after
    /// the marker).
    #[test]
    fn loose_ordered_list_keeps_number_with_text() {
        let md = "1. **Первый.** Текст первого пункта.\n\n\
                  2. **Второй.** Текст второго пункта.\n\n\
                  3. **Третий.** Текст третьего пункта.";
        let collected = rendered_text(md);
        // The number is glued to its text on one feed line.
        assert!(
            collected.contains("1. Первый."),
            "the number separated from the text:\n{collected}"
        );
        assert!(collected.contains("2. Второй."));
        assert!(collected.contains("3. Третий."));
        // There should be no blank line between the marker and its text.
        assert!(
            !collected.contains("1. \n"),
            "a line break appeared after the marker:\n{collected}"
        );
    }

    /// A multi-paragraph item of a "loose" list: the first paragraph — on the
    /// marker line, subsequent ones — on their own lines (the marker isn't
    /// duplicated).
    #[test]
    fn loose_list_item_second_paragraph_on_own_line() {
        let md = "1. Первый абзац.\n\n   Второй абзац того же пункта.\n\n2. Другой пункт.";
        let collected = rendered_text(md);
        assert!(collected.contains("1. Первый абзац."));
        assert!(collected.contains("Второй абзац того же пункта."));
        assert!(collected.contains("2. Другой пункт."));
    }

    /// Render lines as a vector of strings (to check row layout).
    fn rows(input: &str, width: usize) -> Vec<String> {
        render(input, width, &Palette::default())
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Exactly one blank line between prose and a block formula `$$…$$`
    /// (regression: there were two — `start_paragraph` opened a blank line,
    /// and `display_math` put content on a new one, leaving that one blank).
    #[test]
    fn display_math_single_blank_line_before() {
        let lines = rows("текст\n\n$$E=mc^2$$\n\nконец", 80);
        let text_idx = lines.iter().position(|l| l.contains("текст")).unwrap();
        let math_idx = lines.iter().position(|l| l.contains("E=mc")).unwrap();
        assert_eq!(
            math_idx - text_idx,
            2,
            "expected one blank line between prose and the formula:\n{lines:?}"
        );
        assert!(lines[text_idx + 1].trim().is_empty());
    }

    /// A block formula inside a quote keeps the `> ` prefix.
    #[test]
    fn display_math_in_blockquote_keeps_prefix() {
        let lines = rows("> $$x+1$$", 80);
        let math = lines.iter().find(|l| l.contains("x+1")).unwrap();
        assert!(math.starts_with("> "), "quote prefix lost: {math:?}");
    }

    /// A `$$…$$` formula inside a table cell stays **in the cell**, not
    /// leaking as a line above the table (regression: `display_math` called
    /// `push_line` without checking `in_table_cell`).
    #[test]
    fn display_math_in_table_cell_stays_in_cell() {
        let md = "| A | B |\n| :--- | :--- |\n| $$x+1$$ | y |";
        let lines = rows(md, 60);
        let first = lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .expect("empty render");
        assert!(
            first.contains('┌'),
            "the first line should be the table's top border, not the formula: {first:?}"
        );
        let joined = lines.join("\n");
        assert!(joined.contains("x+1"), "formula lost: {joined}");
    }

    /// An autolink `<url>` prints the URL once (regression: `end_link`
    /// appended ` (url)` on top of the URL text → "url (url)").
    #[test]
    fn autolink_url_not_duplicated() {
        let collected = rendered_text("см. <https://example.com> тут");
        assert_eq!(
            collected.matches("example.com").count(),
            1,
            "autolink URL duplicated: {collected}"
        );
    }

    /// A regular link `[t](u)` still prints the URL in parens.
    #[test]
    fn inline_link_keeps_url_suffix() {
        let collected = rendered_text("[текст](https://example.com)");
        assert!(
            collected.contains("текст (https://example.com)"),
            "{collected}"
        );
    }

    /// `<br>` in a paragraph — a line break.
    #[test]
    fn br_tag_breaks_line_in_paragraph() {
        let lines = rows("a<br>b", 80);
        assert!(lines.iter().any(|l| l.trim() == "a"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "b"), "{lines:?}");
    }

    /// `<br>` inside a table cell — a space (no line breaks in a cell).
    #[test]
    fn br_tag_in_table_cell_becomes_space() {
        let md = "| A | B |\n| :--- | :--- |\n| x<br>y | z |";
        let joined = rendered_text_w(md, 60);
        assert!(
            joined.contains("x y"),
            "<br> in a cell should become a space: {joined}"
        );
    }

    /// A language label with an info-string tail (` ```rust,no_run `) resolves
    /// by the first token — the block is highlighted (regression: the whole
    /// info string didn't match the syntax).
    #[test]
    fn code_block_info_string_resolves_language() {
        use crate::shared::config::Theme;
        let colors = fg_colors(
            "```rust,no_run\nfn main() {}\n```",
            &Palette::for_theme(Theme::Dark),
        );
        assert!(
            colors.iter().any(|c| matches!(c, Color::Rgb(..))),
            "expected highlighting by the language from the info string"
        );
    }

    /// A horizontal rule `---` stretches to the panel width (not a stub `───`).
    #[test]
    fn rule_spans_panel_width() {
        let w = 40;
        let text = render("текст\n\n---\n\nещё", w, &Palette::default());
        let rule = text
            .lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains('─')))
            .expect("no separator line");
        let width: usize = rule
            .spans
            .iter()
            .map(|s| wrap::display_width(&s.content.chars().collect::<Vec<_>>()))
            .sum();
        assert_eq!(width, w, "the line should span the panel width");
    }

    /// A tight list item doesn't "leak" onto the next block: after
    /// `3. Title` with a blank line, the following paragraph doesn't glue
    /// to the marker line, and the blank line is preserved (regression:
    /// `item_marker_open` in a tight list was never consumed — the item's
    /// content is inline, with no `Paragraph` wrapper). Scene — an album
    /// tracklist: `3. Track` + Suno tags on the following lines.
    #[test]
    fn tight_list_item_does_not_bleed_into_next_block() {
        let md = "3. Взгляд из ниоткуда\n\n[Intro - Ambient]\nЯ — тишина.";
        let lines = rows(md, 100);
        let marker = lines
            .iter()
            .find(|l| l.contains("3. Взгляд из ниоткуда"))
            .expect("no marker line");
        assert!(
            !marker.contains("[Intro"),
            "the next block glued onto the marker line: {marker:?}"
        );
        let marker_idx = lines.iter().position(|l| l.contains("3. Взгляд")).unwrap();
        let intro_idx = lines.iter().position(|l| l.contains("[Intro")).unwrap();
        assert!(
            intro_idx > marker_idx,
            "the tags aren't on their own line: {lines:?}"
        );
        // between the marker and the tags — the preserved blank line
        assert!(
            lines[marker_idx + 1..intro_idx]
                .iter()
                .any(|l| l.trim().is_empty()),
            "lost the blank line after the track title: {lines:?}"
        );
    }

    /// An ordinary tight list (several items) isn't broken: each item — on
    /// its own line, the next one doesn't glue onto the previous one.
    #[test]
    fn tight_list_items_stay_on_separate_lines() {
        let lines = rows("- один\n- два\n- три", 80);
        assert!(lines.iter().any(|l| l.trim() == "- один"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "- два"), "{lines:?}");
        assert!(lines.iter().any(|l| l.trim() == "- три"), "{lines:?}");
    }

    /// An image prints the alt text and the URL in parens (like a link).
    #[test]
    fn image_prints_alt_and_url() {
        let collected = rendered_text("![схема](http://x/i.png)");
        assert!(collected.contains("схема (http://x/i.png)"), "{collected}");
    }

    /// A price range/fraction `$5-$10` isn't swallowed by the math extension
    /// (dollars remain, "5-10" isn't torn apart).
    #[test]
    fn price_range_not_treated_as_math() {
        let c = rendered_text("товар $5-$10 или $5/$7");
        assert!(c.contains("$5-$10"), "{c}");
        assert!(c.contains("$5/$7"), "{c}");
    }

    /// Render with the mermaid flag on (other flags default).
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

    /// A valid mermaid block with rendering enabled — a diagram instead of
    /// the source: box-drawing frames are present, no fences ``` or raw
    /// `-->` remain.
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
        assert!(joined.contains('┌'), "no diagram frames:\n{joined}");
        assert!(
            !joined.contains("```"),
            "the fence should not be printed:\n{joined}"
        );
        assert!(
            !joined.contains("->>"),
            "source leaked into the feed:\n{joined}"
        );
        assert!(
            joined.contains("до") && joined.contains("после"),
            "{joined}"
        );
    }

    /// Golden fallback: on any failure (garbage / a type outside the
    /// whitelist / doesn't fit the width) the output with the flag enabled
    /// **exactly matches** the output with it disabled — lines AND styles
    /// (Line: PartialEq). The guarantee "worst case = previous behavior".
    #[test]
    fn mermaid_fallback_matches_disabled_render() {
        let cases = [
            // garbage in the block (the LLM left the syntax unfinished/broken)
            ("```mermaid\nпросто текст без диаграммы\n```", 90),
            // a type outside the whitelist (pie)
            ("```mermaid\npie title X\n    \"A\" : 1\n```", 90),
            // valid, but doesn't fit a narrow panel
            (
                "```mermaid\nsequenceDiagram\n    participant Client\n    participant Server\n    Client->>Server: GET /api/data\n```",
                20,
            ),
            // an info string with a tail after the language is preserved in the fallback's fence
            ("```mermaid title=x\nне диаграмма\n```", 90),
        ];
        for (md, w) in cases {
            let on = render_mermaid_on(md, w);
            let off = render(md, w, &Palette::default());
            assert_eq!(
                on.lines, off.lines,
                "fallback diverged from the previous look (w={w}):\n{md}"
            );
        }
    }

    /// The flag off (Default) — the previous behavior: the source as a code
    /// block.
    #[test]
    fn mermaid_flag_off_keeps_source() {
        let md = "```mermaid\nsequenceDiagram\n    A->>B: hi\n```";
        let joined = rendered_text(md);
        assert!(joined.contains("```mermaid"), "{joined}");
        assert!(joined.contains("A->>B: hi"), "{joined}");
    }

    /// Streaming: a block with an unclosed fence (the server is still
    /// finishing the diagram) prints as the source **byte-for-byte as with
    /// rendering disabled** — no "partial diagram ↔ source" flicker as
    /// chunks arrive. Regression: pulldown-cmark stretches an unclosed fence
    /// to the end of the document, and a syntactically valid stub (the first
    /// case) used to render as a diagram.
    #[test]
    fn mermaid_unclosed_fence_streams_as_source() {
        let cases = [
            // a valid stub: without the closure check it would render as a diagram
            "текст\n\n```mermaid\nflowchart LR\n    A[Старт] --> B[Конец]",
            // the same with a trailing line break
            "```mermaid\nsequenceDiagram\n    A->>B: hi\n",
            // cut off mid-word
            "```mermaid\nsequenceDiagram\n    participant Ser",
            // only the opening fence
            "```mermaid",
            // a tilde fence
            "~~~mermaid\nflowchart LR\n    A --> B",
            // the closing fence is shorter than the opening one — the block is NOT closed
            "````mermaid\nflowchart LR\n    A --> B\n```",
        ];
        for md in cases {
            let on = render_mermaid_on(md, 90);
            let off = render(md, 90, &Palette::default());
            assert_eq!(
                on.lines, off.lines,
                "an unclosed block should go the source path:\n{md}"
            );
        }
    }

    /// The closing fence has arrived — the block renders as a diagram, even
    /// when the message keeps streaming after it (closure is a property of
    /// the block, not of the end of the message). A tilde fence is treated
    /// equally.
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
                "the fence should not be printed: {joined}"
            );
            assert!(
                !joined.contains("-->"),
                "source leaked into the feed: {joined}"
            );
        }
    }

    /// A Cyrillic flowchart renders as a diagram (upstream regression in
    /// 0.56.0 — a panic/corrupted labels on multibyte input; our fix #29/#30).
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
        assert!(!joined.contains("[Конец]"), "label corrupted: {joined}");
    }

    /// Real math still converts (dollar signs stripped).
    #[test]
    fn real_math_still_converts() {
        let c = rendered_text(r"значения $3.14$, $2+2$ и $x^2$");
        assert!(c.contains("3.14"), "{c}");
        assert!(
            !c.contains("$3.14$"),
            "dollars not stripped from a real formula: {c}"
        );
        assert!(c.contains("x²"), "{c}");
    }
}
