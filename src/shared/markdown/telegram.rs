//! Markdown → Telegram's HTML subset, cut into messages (ADR 0013, rule 5).
//!
//! A third consumer of the same `pulldown-cmark` events as [`Writer`] and the
//! speech extractor: the feed draws, speech reads aloud, this hands a reply to
//! a messenger. Telegram's HTML knows `b`, `i`, `s`, `code`, `pre`, `a` and
//! `blockquote` and nothing else — no paragraphs, lists or headings — so a
//! heading is bold, a list item a line with a marker, a table a monospaced
//! block, and the rest plain text with its line breaks.
//!
//! A reply longer than one message is cut **between top-level blocks**, never
//! inside a tag; a block too long for a message on its own is cut by lines — a
//! code block into several, each a block again — and anything else falls back
//! to its own source as plain text. Every part carries that plain text beside
//! the markup, for a server that refuses the markup.

use super::*;

/// One message of a reply: its markup, and the same content as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePart {
    pub html: String,
    pub plain: String,
}

/// `markdown` as messages of at most `limit` characters each.
pub fn telegram_parts(markdown: &str, limit: usize) -> Vec<MessagePart> {
    let normalized = normalize_delimiters(markdown);
    pack(blocks(&normalized), limit.max(64))
}

/// One top-level block: its markup and its source; a code block also keeps
/// its language and text, so a long one can be cut into code blocks.
struct Block {
    html: String,
    source: String,
    code: Option<(String, String)>,
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A link Telegram will open: the web, its own `tg:` links, mail.
fn linkable(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "tg://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p))
}

fn code_block(lang: &str, code: &str) -> String {
    let code = code.trim_end_matches('\n');
    if lang.is_empty() {
        format!("<pre>{}</pre>", escape(code))
    } else {
        format!(
            "<pre><code class=\"language-{}\">{}</code></pre>",
            escape(lang),
            escape(code)
        )
    }
}

/// The walker's state inside one top-level block.
#[derive(Default)]
struct Walker {
    out: String,
    /// Open lists, innermost last: the next number of an ordered one.
    lists: Vec<Option<u64>>,
    /// Quotes open; Telegram nests none, so only the outermost is drawn.
    quotes: usize,
    /// Links open: whether each was drawn as `<a>`.
    links: Vec<bool>,
    /// A code block's language and text, while inside one.
    code: Option<(String, String)>,
    /// A table's rows and the cell being read, while inside one.
    table: Option<(Vec<Vec<String>>, String)>,
    /// An image's address and its alt text so far, while inside one.
    image: Option<(String, String)>,
}

impl Walker {
    fn text(&mut self, text: &str) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
        } else if let Some((_, cell)) = &mut self.table {
            cell.push_str(text);
        } else if let Some((_, code)) = &mut self.code {
            code.push_str(text);
        } else {
            self.out.push_str(&escape(text));
        }
    }

    fn newline(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { .. } => self.out.push_str("<b>"),
            Tag::Strong => self.out.push_str("<b>"),
            Tag::Emphasis => self.out.push_str("<i>"),
            Tag::Strikethrough => self.out.push_str("<s>"),
            Tag::BlockQuote(_) => {
                if self.quotes == 0 {
                    self.newline();
                    self.out.push_str("<blockquote>");
                }
                self.quotes += 1;
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([',', ' ', '\t'])
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.newline();
                self.lists.push(start);
            }
            Tag::Item => {
                self.newline();
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"  ".repeat(depth));
                match self.lists.last_mut() {
                    Some(Some(n)) => {
                        self.out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => self.out.push_str("• "),
                }
            }
            Tag::Link { dest_url, .. } => {
                let drawn = linkable(&dest_url);
                if drawn {
                    self.out
                        .push_str(&format!("<a href=\"{}\">", escape(dest_url.trim())));
                }
                self.links.push(drawn);
            }
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            Tag::Table(_) => self.table = Some((Vec::new(), String::new())),
            Tag::TableHead | Tag::TableRow => {
                if let Some((rows, _)) = &mut self.table {
                    rows.push(Vec::new());
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) | TagEnd::Strong => self.out.push_str("</b>"),
            TagEnd::Emphasis => self.out.push_str("</i>"),
            TagEnd::Strikethrough => self.out.push_str("</s>"),
            TagEnd::Paragraph => {
                if self.quotes > 0 || !self.lists.is_empty() {
                    self.newline();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.quotes = self.quotes.saturating_sub(1);
                if self.quotes == 0 {
                    while self.out.ends_with('\n') {
                        self.out.pop();
                    }
                    self.out.push_str("</blockquote>");
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.newline();
                    self.out.push_str(&code_block(&lang, &code));
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Link => {
                if self.links.pop() == Some(true) {
                    self.out.push_str("</a>");
                }
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    let label = if alt.trim().is_empty() {
                        url.clone()
                    } else {
                        alt
                    };
                    if linkable(&url) {
                        self.out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            escape(url.trim()),
                            escape(&label)
                        ));
                    } else {
                        self.out.push_str(&escape(&label));
                    }
                }
            }
            TagEnd::TableCell => {
                if let Some((rows, cell)) = &mut self.table
                    && let Some(row) = rows.last_mut()
                {
                    row.push(std::mem::take(cell).trim().to_string());
                }
            }
            TagEnd::Table => {
                if let Some((rows, _)) = self.table.take() {
                    let lines: Vec<String> = rows.iter().map(|r| r.join(" | ")).collect();
                    self.newline();
                    self.out
                        .push_str(&format!("<pre>{}</pre>", escape(&lines.join("\n"))));
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if self.table.is_some() || self.image.is_some() {
                    self.text(&code);
                } else {
                    self.out
                        .push_str(&format!("<code>{}</code>", escape(&code)));
                }
            }
            Event::InlineMath(content) => {
                let text = if looks_like_price_fragment(&content) {
                    format!("${content}$")
                } else {
                    latex_to_unicode(&content)
                };
                self.text(&text);
            }
            Event::DisplayMath(content) => {
                self.newline();
                self.out.push_str(&format!(
                    "<pre>{}</pre>",
                    escape(&latex_to_unicode_display(&content))
                ));
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, cell)) = &mut self.table {
                    cell.push(' ');
                } else if let Some((_, code)) = &mut self.code {
                    code.push('\n');
                } else {
                    self.out.push('\n');
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if is_br(&html) {
                    self.out.push('\n');
                } else {
                    self.text(&html);
                }
            }
            Event::TaskListMarker(done) => self.out.push_str(if done { "☑ " } else { "☐ " }),
            Event::Rule => {
                self.newline();
                self.out.push_str("———");
            }
            _ => {}
        }
    }
}

/// The top-level blocks of `markdown`, each with its markup and its source.
fn blocks(markdown: &str) -> Vec<Block> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_TABLES);
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut walker = Walker::default();
    let mut source = 0..0;
    let mut is_code = false;
    for (event, range) in Parser::new_ext(markdown, opts).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    source = range;
                    is_code = matches!(tag, Tag::CodeBlock(_));
                }
                depth += 1;
                walker.start(tag);
            }
            Event::End(tag) => {
                // A code block's text is read before its end takes it.
                let code = if depth == 1 && is_code {
                    walker.code.clone()
                } else {
                    None
                };
                walker.end(tag);
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    push_block(&mut out, &mut walker, &markdown[source.clone()], code);
                }
            }
            // A block with no start of its own: a rule, a stray piece of HTML.
            other if depth == 0 => {
                walker.event(other);
                push_block(&mut out, &mut walker, &markdown[range], None);
            }
            other => walker.event(other),
        }
    }
    out
}

fn push_block(
    out: &mut Vec<Block>,
    walker: &mut Walker,
    source: &str,
    code: Option<(String, String)>,
) {
    let html = std::mem::take(&mut walker.out).trim().to_string();
    *walker = Walker::default();
    if html.is_empty() {
        return;
    }
    out.push(Block {
        html,
        source: source.trim_end().to_string(),
        code,
    });
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `text` cut into pieces of at most `limit` characters, at line ends where
/// it can be, mid-line where a line is longer than a piece.
fn by_lines(text: &str, limit: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut cur = String::new();
    for line in text.split('\n') {
        let mut line = line.to_string();
        loop {
            let room = limit.saturating_sub(chars(&cur) + usize::from(!cur.is_empty()));
            if chars(&line) <= room {
                if !cur.is_empty() {
                    cur.push('\n');
                }
                cur.push_str(&line);
                break;
            }
            if !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
                continue;
            }
            // A line longer than a whole piece: cut it.
            let head: String = line.chars().take(limit).collect();
            line = line.chars().skip(limit).collect();
            pieces.push(head);
        }
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    pieces
}

/// A block too long for one message, as several that each fit.
fn cut(block: Block, limit: usize) -> Vec<MessagePart> {
    if let Some((lang, code)) = &block.code {
        // Room for the wrapper and the escapes: a quarter is generous.
        let room = limit.saturating_sub(64) * 3 / 4;
        return by_lines(code.trim_end_matches('\n'), room.max(16))
            .into_iter()
            .map(|piece| MessagePart {
                html: code_block(lang, &piece),
                plain: piece,
            })
            .collect();
    }
    let room = limit * 3 / 4;
    by_lines(&block.source, room.max(16))
        .into_iter()
        .map(|piece| MessagePart {
            html: escape(&piece),
            plain: piece,
        })
        .collect()
}

/// Blocks into messages: as many whole blocks as fit, a blank line between.
fn pack(blocks: Vec<Block>, limit: usize) -> Vec<MessagePart> {
    let mut parts: Vec<MessagePart> = Vec::new();
    let mut cur: Option<MessagePart> = None;
    for block in blocks {
        if chars(&block.html) > limit {
            parts.extend(cur.take());
            parts.extend(cut(block, limit));
            continue;
        }
        match &mut cur {
            Some(part) if chars(&part.html) + 2 + chars(&block.html) <= limit => {
                part.html.push_str("\n\n");
                part.html.push_str(&block.html);
                part.plain.push_str("\n\n");
                part.plain.push_str(&block.source);
            }
            _ => {
                parts.extend(cur.take());
                cur = Some(MessagePart {
                    html: block.html,
                    plain: block.source,
                });
            }
        }
    }
    parts.extend(cur);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(md: &str) -> String {
        let parts = telegram_parts(md, 4096);
        assert_eq!(parts.len(), 1, "{parts:?}");
        parts[0].html.clone()
    }

    #[test]
    fn inline_styles_become_telegram_tags_and_text_is_escaped() {
        assert_eq!(
            html("**bold** *it* ~~gone~~ `a<b` & 1 < 2"),
            "<b>bold</b> <i>it</i> <s>gone</s> <code>a&lt;b</code> &amp; 1 &lt; 2"
        );
    }

    #[test]
    fn a_heading_is_bold_and_blocks_are_a_blank_line_apart() {
        assert_eq!(html("# Title\n\nText."), "<b>Title</b>\n\nText.");
    }

    #[test]
    fn lists_are_lines_with_markers_nested_by_indent() {
        assert_eq!(
            html("- one\n- two\n  1. a\n  2. b\n- [x] done"),
            "• one\n• two\n  1. a\n  2. b\n• ☑ done"
        );
    }

    #[test]
    fn a_code_block_keeps_its_language_and_its_text_unescaped_once() {
        assert_eq!(
            html("```rust\nlet x = a < b;\n```"),
            "<pre><code class=\"language-rust\">let x = a &lt; b;</code></pre>"
        );
        assert_eq!(html("    plain\n"), "<pre>plain</pre>");
    }

    #[test]
    fn links_open_only_where_telegram_can_follow_them() {
        assert_eq!(
            html("[site](https://ex.com/?a=1&b=2) [local](./x.md)"),
            "<a href=\"https://ex.com/?a=1&amp;b=2\">site</a> local"
        );
    }

    #[test]
    fn a_quote_is_one_blockquote_even_when_nested() {
        assert_eq!(
            html("> said\n> > inner"),
            "<blockquote>said\ninner</blockquote>"
        );
    }

    #[test]
    fn a_table_is_a_monospaced_block() {
        assert_eq!(
            html("| a | b |\n|---|---|\n| 1 | 2 |"),
            "<pre>a | b\n1 | 2</pre>"
        );
    }

    #[test]
    fn line_breaks_inside_a_paragraph_are_kept() {
        assert_eq!(html("one\ntwo"), "one\ntwo");
    }

    #[test]
    fn a_long_reply_is_cut_between_blocks() {
        let para = "word ".repeat(30);
        let md = format!("{para}\n\n{para}\n\n{para}");
        let parts = telegram_parts(&md, 200);
        assert_eq!(parts.len(), 3);
        for p in &parts {
            assert!(chars(&p.html) <= 200);
            assert_eq!(p.html, p.html.trim());
        }
    }

    #[test]
    fn a_code_block_longer_than_a_message_becomes_several_code_blocks() {
        let code: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let parts = telegram_parts(&format!("```\n{code}```"), 400);
        assert!(parts.len() > 3);
        for p in &parts {
            assert!(chars(&p.html) <= 400, "{}", p.html);
            assert!(p.html.starts_with("<pre>") && p.html.ends_with("</pre>"));
        }
        let rejoined: Vec<String> = parts.iter().map(|p| p.plain.clone()).collect();
        assert_eq!(rejoined.join("\n"), code.trim_end());
    }

    #[test]
    fn an_overlong_paragraph_falls_back_to_its_source() {
        let md = format!("**{}**", "x".repeat(900));
        let parts = telegram_parts(&md, 300);
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| chars(&p.html) <= 300));
        assert!(parts[0].plain.starts_with("**x"));
    }

    #[test]
    fn every_part_carries_its_plain_text() {
        let parts = telegram_parts("**hi** there\n\n- a", 4096);
        assert_eq!(parts[0].plain, "**hi** there\n\n- a");
    }

    #[test]
    fn nothing_to_say_is_no_message() {
        assert!(telegram_parts("", 4096).is_empty());
        assert!(telegram_parts("  \n\n ", 4096).is_empty());
    }

    #[test]
    fn a_rule_is_a_line_of_its_own() {
        assert_eq!(html("a\n\n---\n\nb"), "a\n\n———\n\nb");
    }
}
