//! Text extraction from **raw HTML blocks**.
//!
//! CommonMark hands a block-level run of HTML to the renderer as opaque
//! [`Event::Html`](pulldown_cmark::Event::Html) chunks with **no `Text` events
//! inside** — unlike *inline* HTML (`<strong>x</strong>` in a paragraph), whose
//! inner text does arrive as `Text` and has always rendered fine. So a pasted
//! `<table>` used to render as *nothing at all*: the whole block, prose
//! included, was dropped on the floor. Measured on the real dev corpus before
//! this was written — one message in 1213, but 1574 characters of prose
//! silently invisible in it (see the CLAUDE.md journal entry).
//!
//! We show the block's **text**, not its markup and not a reconstructed table:
//!
//! - Rendering the source verbatim (like a code block) is honest but turns a
//!   30-row table into a wall of tags — worse than the bug for the common case,
//!   which is a model or a README emitting a layout table.
//! - Parsing HTML into [`super::table`]'s layout is a different feature an order
//!   of magnitude larger, and it would still have to fall back to text for
//!   everything that is not a table.
//!
//! This mirrors what `features::tools::web::extract_readable` already does for
//! RAG and `fetch_url`. It is deliberately **not** reused: that lives in
//! `features`, which `shared` may not depend on (FSD), and pulling `scraper`
//! into the renderer to strip tags would be heavy for the job. A small state
//! machine is the same call the project made for `calc.rs` and `latex.rs`.
//!
//! What this is not: an HTML parser. It never builds a tree, so malformed
//! markup degrades into text rather than into an error — which is the right
//! failure for a chat feed.

/// Elements whose **content is not prose**: showing it would dump CSS or
/// JavaScript into the conversation. Their entire content is skipped, the same
/// call `extract_readable` makes for `nav`/`header`/`footer`/`aside`.
const DROPPED_ELEMENTS: [&str; 2] = ["script", "style"];

/// Elements that end the current text line. Both the opening and the closing
/// tag break, so `<p>a</p><p>b</p>` and a stray unclosed `<p>` both come out as
/// two lines.
const LINE_BREAKING: [&str; 21] = [
    "br",
    "p",
    "div",
    "tr",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "ul",
    "ol",
    "blockquote",
    "hr",
    "section",
    "article",
];

/// Elements that separate words without ending the line — table cells, so a row
/// reads as one line (`<td>a</td><td>b</td>` → `a b`) instead of `ab`.
const WORD_SEPARATING: [&str; 2] = ["td", "th"];

/// The text of a raw HTML block, as logical lines (already whitespace-collapsed
/// and entity-decoded; never empty strings). Lines are *logical* — the feed
/// wraps them to the panel width, like any other paragraph.
pub(super) fn html_block_to_lines(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    // Set while inside an element whose content we drop; holds its name, so
    // nesting of a *different* element cannot end the skip early.
    let mut skip_until: Option<String> = None;
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(next) = skip_comment(&chars, i) {
                i = next;
                continue;
            }
            let Some((inner, next)) = read_tag(&chars, i) else {
                // A `<` that never closes is not markup — show it.
                push_char(&mut cur, '<');
                i += 1;
                continue;
            };
            i = next;
            apply_tag(&inner, &mut cur, &mut out, &mut skip_until);
            continue;
        }
        if skip_until.is_none() {
            push_char(&mut cur, chars[i]);
        }
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

/// Applies one parsed tag to the extraction state — what a tag *means* for the
/// output: ending or entering a dropped element, printing an image, breaking
/// the line or separating words. The scanner itself (indices, quotes,
/// comments) stays in [`html_block_to_lines`].
fn apply_tag(
    inner: &str,
    cur: &mut String,
    out: &mut Vec<String>,
    skip_until: &mut Option<String>,
) {
    let tag = Tag::parse(inner);

    if let Some(open) = &*skip_until {
        if tag.closing && tag.name == *open {
            *skip_until = None;
        }
        return;
    }
    if !tag.closing && !tag.self_closing && DROPPED_ELEMENTS.contains(&tag.name.as_str()) {
        *skip_until = Some(tag.name);
        return;
    }
    if tag.name == "img" && !tag.closing {
        push_image(cur, inner);
        return;
    }
    if LINE_BREAKING.contains(&tag.name.as_str()) {
        flush(cur, out);
    } else if WORD_SEPARATING.contains(&tag.name.as_str()) {
        push_char(cur, ' ');
    }
}

/// Whether the block carries a forced break and nothing else — a lone `<br>`,
/// which before this module produced a blank line and still should.
pub(super) fn is_break_only(raw: &str) -> bool {
    let mut seen_break = false;
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(next) = skip_comment(&chars, i) {
                i = next;
                continue;
            }
            if let Some((inner, next)) = read_tag(&chars, i) {
                i = next;
                seen_break |= LINE_BREAKING.contains(&Tag::parse(&inner).name.as_str());
                continue;
            }
            return false;
        }
        if !chars[i].is_whitespace() {
            return false;
        }
        i += 1;
    }
    seen_break
}

/// An `<img>`: the alt text plus the URL in parens — exactly how
/// [`Writer::end_image`](super::Writer::end_image) prints a markdown
/// `![alt](url)`, so an image reads the same whichever syntax produced it. An
/// image with neither attribute contributes nothing.
fn push_image(cur: &mut String, inner: &str) {
    if let Some(alt) = attr(inner, "alt").filter(|a| !a.trim().is_empty()) {
        for c in decode_entities(&alt).chars() {
            push_char(cur, c);
        }
    }
    if let Some(src) = attr(inner, "src").filter(|s| !s.trim().is_empty()) {
        push_char(cur, ' ');
        cur.push('(');
        cur.push_str(decode_entities(&src).trim());
        cur.push(')');
    }
}

/// Appends one text character, collapsing whitespace runs to a single space —
/// HTML's own rule, and what keeps a table's indentation out of the output.
fn push_char(cur: &mut String, c: char) {
    if c.is_whitespace() {
        if !cur.is_empty() && !cur.ends_with(' ') {
            cur.push(' ');
        }
    } else {
        cur.push(c);
    }
}

/// Ends the current line, dropping it when it holds nothing but spaces.
fn flush(cur: &mut String, out: &mut Vec<String>) {
    let line = std::mem::take(cur);
    let line = line.trim();
    if !line.is_empty() {
        out.push(decode_entities(line));
    }
}

/// If a comment starts at `at`, the index just past its `-->` (or the end of
/// input for an unterminated one). Comments are dropped whole: their content is
/// not shown, and `-->` is not a tag terminator [`read_tag`] would find.
fn skip_comment(chars: &[char], at: usize) -> Option<usize> {
    if chars[at..].starts_with(&['<', '!', '-', '-']) {
        let mut i = at + 4;
        while i + 2 < chars.len() {
            if chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>' {
                return Some(i + 3);
            }
            i += 1;
        }
        return Some(chars.len());
    }
    None
}

/// Reads the tag starting at `at` (`chars[at] == '<'`), returning its inside
/// (without the angle brackets) and the index just past `>`.
///
/// Quoted attribute values are respected, so a `>` inside one — `<td
/// title="a > b">` — does not end the tag early. An unterminated tag yields
/// `None`, and the `<` is then shown as text.
fn read_tag(chars: &[char], at: usize) -> Option<(String, usize)> {
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    for (i, &c) in chars.iter().enumerate().skip(at + 1) {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                buf.push(c);
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                buf.push(c);
            }
            None if c == '>' => return Some((buf, i + 1)),
            None => buf.push(c),
        }
    }
    None
}

/// A tag's shape — everything the extraction needs to know about it.
struct Tag {
    /// Lowercased element name (`td`, `img`, …); empty for `<!doctype …>` and
    /// other non-elements, which then match no list and are simply dropped.
    name: String,
    closing: bool,
    self_closing: bool,
}

impl Tag {
    fn parse(inner: &str) -> Self {
        let trimmed = inner.trim();
        let closing = trimmed.starts_with('/');
        let name: String = trimmed
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        Self {
            name: name.to_ascii_lowercase(),
            closing,
            self_closing: trimmed.ends_with('/'),
        }
    }
}

/// The value of attribute `name` in a tag's inside, quoted or bare.
fn attr(inner: &str, name: &str) -> Option<String> {
    let lower = inner.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let at = from + pos;
        from = at + name.len();
        // Must be a whole attribute name: preceded by whitespace (or the tag
        // name) and followed by `=`, so `src` does not match inside `datasrc`.
        let before_ok = at > 0 && lower[..at].ends_with(|c: char| c.is_whitespace());
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value_at = from + lower[from..].find('=')? + 1;
        let value = inner[value_at..].trim_start();
        let mut ch = value.chars();
        return match ch.next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().map(str::to_string),
            Some(_) => value
                .split(|c: char| c.is_whitespace())
                .next()
                .map(str::to_string),
            None => None,
        };
    }
    None
}

/// Decodes the HTML entities that actually turn up in chat content: the XML
/// five, `&nbsp;`, and numeric references. An unknown entity is left as written
/// — showing `&hellip;` beats swallowing it.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|e| *e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name.to_ascii_lowercase().as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => numeric_entity(name),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `#1234` / `#x1F600` → the character, if it is one.
fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_block_yields_its_prose() {
        // The measured defect: this used to render as nothing at all.
        let lines = html_block_to_lines(
            "<table>\n<tr><td>start with clarity</td><td>and why it matters</td></tr>\n\
             <tr><td>break your vision</td><td>into phases</td></tr>\n</table>",
        );
        assert_eq!(
            lines,
            vec![
                "start with clarity and why it matters",
                "break your vision into phases"
            ],
            "cells join with a space, rows break the line"
        );
    }

    #[test]
    fn script_and_style_content_is_dropped() {
        // Otherwise stripping tags would dump CSS and JavaScript into the feed
        // as if it were prose.
        let lines = html_block_to_lines(
            "<div><style>.a { color: red; }</style>visible<script>alert(1)</script></div>",
        );
        assert_eq!(lines, vec!["visible"]);
    }

    #[test]
    fn a_dropped_element_ends_only_on_its_own_closing_tag() {
        let lines = html_block_to_lines("<style>a { b: c }</p>still css</style>after");
        assert_eq!(lines, vec!["after"]);
    }

    #[test]
    fn attributes_never_reach_the_output() {
        // Where 68 of the 88 measured unhighlightable words came from: CSS in
        // `style=` and `width=` looked like prose to the index but is markup.
        let lines = html_block_to_lines(
            "<td width=\"50%\" style=\"background-color: white;\">real prose</td>",
        );
        assert_eq!(lines, vec!["real prose"]);
    }

    #[test]
    fn a_quoted_angle_bracket_does_not_end_the_tag() {
        let lines = html_block_to_lines("<td title=\"a > b\">text</td>");
        assert_eq!(lines, vec!["text"]);
    }

    #[test]
    fn comments_are_dropped_whole() {
        let lines = html_block_to_lines("<div>before<!-- a > b, hidden -->after</div>");
        assert_eq!(lines, vec!["beforeafter"]);
        // An unterminated comment swallows the rest rather than leaking it —
        // the text before it still shows.
        assert_eq!(html_block_to_lines("<div>x<!-- unterminated"), vec!["x"]);
    }

    #[test]
    fn an_image_prints_alt_and_url_like_a_markdown_image() {
        // Consistency with `Writer::end_image`: the same image reads the same
        // whichever syntax produced it.
        let lines = html_block_to_lines(
            "<img src=\"chaos.svg\" alt=\"chaos - without structure\" style=\"width: 500px\">",
        );
        assert_eq!(lines, vec!["chaos - without structure (chaos.svg)"]);
        assert_eq!(html_block_to_lines("<img src=\"a.png\">"), vec!["(a.png)"]);
        assert!(html_block_to_lines("<img>").is_empty());
    }

    #[test]
    fn attr_matches_whole_names_only() {
        assert_eq!(
            attr("img datasrc=\"x\" src=\"y\"", "src").as_deref(),
            Some("y")
        );
        assert_eq!(
            attr("img src=bare.png alt='q'", "src").as_deref(),
            Some("bare.png")
        );
        assert_eq!(attr("img alt='q'", "src"), None);
    }

    #[test]
    fn entities_are_decoded_and_unknown_ones_survive() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(decode_entities("&#39;x&#x41;"), "'xA");
        assert_eq!(decode_entities("&nbsp;gap"), " gap");
        assert_eq!(
            decode_entities("&hellip; &unknown; plain &"),
            "&hellip; &unknown; plain &",
            "an unknown entity is shown, not swallowed"
        );
    }

    #[test]
    fn whitespace_is_collapsed_and_blank_lines_dropped() {
        let lines = html_block_to_lines("<div>\n    a\n\n    b   c\n</div>\n\n<div>\n</div>");
        assert_eq!(lines, vec!["a b c"]);
    }

    #[test]
    fn malformed_markup_degrades_to_text() {
        // No tree is built, so this cannot error — the point of the design.
        assert_eq!(html_block_to_lines("a < b and 5<6"), vec!["a < b and 5<6"]);
        assert_eq!(html_block_to_lines("<td>unclosed"), vec!["unclosed"]);
        assert!(html_block_to_lines("<div></div>").is_empty());
    }

    #[test]
    fn is_break_only_recognizes_a_lone_break() {
        // Preserves the pre-existing behavior of a standalone `<br>`: a blank
        // line, not silence.
        assert!(is_break_only("<br>"));
        assert!(is_break_only("  <br />\n"));
        assert!(is_break_only("<hr>"));
        assert!(!is_break_only("<br>text"));
        assert!(!is_break_only("<td></td>"), "no break tag at all");
        assert!(!is_break_only("<img src=\"a\">"));
    }
}
