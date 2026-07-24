//! Speech extractor: markdown → plain text for speech synthesis (TTS).
//! Rules — the table in docs/research/tts.md §5 (source of truth).
//!
//! This is a **second consumer** of the same `pulldown-cmark` events as
//! [`Writer`] (ADR 0003): the feed render draws, the extractor speaks aloud.
//! They only share the parser and the pure LaTeX helpers
//! ([`latex_to_unicode`], [`normalize_delimiters`]); `Writer` is left alone —
//! its own walker is simpler and doesn't drag in styles/width/palette.
//!
//! What the extractor does (see §5):
//! - code blocks (incl. ` ```mermaid `), tables, and block formulas `$$…$$`
//!   are **skipped** with a short voice note in the profile's language (i18n
//!   axis A);
//! - inline math converts to unicode, inline code is read as text;
//! - for a link only the text is read, a bare URL/autolink becomes "link:
//!   domain";
//! - for an image the alt text is read (otherwise a note);
//! - headings/lists/quotes are read as text, boundaries get a period/line
//!   break — so the engine pauses.
//!
//! The function is pure (no engine/audio) — covered by golden tests.

use super::*;
use crate::shared::i18n::Locale;

/// Extracts text suitable for speech synthesis from markdown.
///
/// `loc` — the **profile-language** locale (axis A): voice notes about
/// skipped blocks are part of the speech content, not UI chrome.
///
/// An empty/whitespace-only input gives an empty string (nothing for the
/// caller to synthesize).
pub fn speakable_text(markdown: &str, loc: &'static Locale) -> String {
    let normalized = normalize_delimiters(markdown);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_TABLES);
    let mut speaker = Speaker::new(loc);
    for event in Parser::new_ext(&normalized, opts) {
        speaker.handle(event);
    }
    speaker.finish()
}

/// Accumulator of speech "blocks": each block is a complete phrase, blocks
/// are separated by a line break (a pause on any engine).
struct Speaker {
    loc: &'static Locale,
    blocks: Vec<String>,
    /// The current (unfinished) block.
    cur: String,
    /// A code block is being skipped (content isn't spoken).
    skip_code: bool,
    /// A table is being skipped (cells aren't spoken).
    skip_table: bool,
    /// Collecting an image's alt text (inside `![…](…)`).
    image_alt: Option<String>,
}

impl Speaker {
    fn new(loc: &'static Locale) -> Self {
        Self {
            loc,
            blocks: Vec::new(),
            cur: String::new(),
            skip_code: false,
            skip_table: false,
            image_alt: None,
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        // Inside a block being skipped, wait only for its end — content isn't spoken.
        if self.skip_code {
            if matches!(event, Event::End(TagEnd::CodeBlock)) {
                self.skip_code = false;
            }
            return;
        }
        if self.skip_table {
            if matches!(event, Event::End(TagEnd::Table)) {
                self.skip_table = false;
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            // Inline code is read as regular text (short identifiers are
            // useful in speech), URL substitution doesn't apply to it.
            Event::Code(code) => self.push(&code),
            Event::InlineMath(content) => {
                if looks_like_price_fragment(&content) {
                    // A false math match on a price range "$5-$10" — read as-is.
                    let literal = format!("${content}$");
                    self.push(&literal);
                } else {
                    let converted = latex_to_unicode(&content);
                    self.push(&converted);
                }
            }
            // A block formula isn't read (speaking "\begin{aligned}" is pointless).
            Event::DisplayMath(_) => self.note("speak.skip.formula"),
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.flush(),
            Event::InlineHtml(html) | Event::Html(html) if is_br(&html) => self.flush(),
            Event::Rule => self.flush(),
            // Other HTML, footnotes, task markers — silently skipped.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::CodeBlock(kind) => {
                let info = match kind {
                    CodeBlockKind::Fenced(ref lang) => lang.as_ref(),
                    CodeBlockKind::Indented => "",
                };
                // ` ```mermaid ` is a regular code block with an info string;
                // no separate detection is needed, just its own note (a
                // diagram, not code).
                let lang = info.split([',', ' ', '\t']).next().unwrap_or("");
                let key = if lang.eq_ignore_ascii_case("mermaid") {
                    "speak.skip.mermaid"
                } else {
                    "speak.skip.code"
                };
                self.note(key);
                self.skip_code = true;
            }
            Tag::Table(_) => {
                self.note("speak.skip.table");
                self.skip_table = true;
            }
            // Start of an image: the alt text arrives via `Text` events, intercept them.
            Tag::Image { .. } => self.image_alt = Some(String::new()),
            // Block boundaries — a pause (period/line break).
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::List(_)
            | Tag::Item => self.flush(),
            // A link: the URL isn't read, the link text arrives via `Text` events.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Image => {
                let alt = self.image_alt.take().unwrap_or_default();
                if alt.trim().is_empty() {
                    let note = self.loc.t("speak.skip.image").to_string();
                    self.push(&note);
                } else {
                    self.push(&alt);
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::List(_)
            | TagEnd::Item => self.flush(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(alt) = &mut self.image_alt {
            alt.push_str(text);
            return;
        }
        // A bare URL and an autolink (`<https://…>`, whose text is the URL
        // itself) are read as "link: domain".
        let spoken = rewrite_urls(text, self.loc);
        self.push(&spoken);
    }

    fn push(&mut self, text: &str) {
        self.cur.push_str(text);
    }

    /// Closes the current block: collapses whitespace and appends a period
    /// (a pause) if the phrase doesn't already end in punctuation. An empty
    /// block is dropped.
    fn flush(&mut self) {
        let collapsed = collapse_ws(&self.cur);
        self.cur.clear();
        if collapsed.is_empty() {
            return;
        }
        let ends_sentence = collapsed.ends_with(['.', '!', '?', ':', ';', '…']);
        self.blocks.push(if ends_sentence {
            collapsed
        } else {
            format!("{collapsed}.")
        });
    }

    /// A voice note about a skipped block — its own block (a pause on both sides).
    fn note(&mut self, key: &str) {
        self.flush();
        let note = self.loc.t(key).to_string();
        self.push(&note);
        self.flush();
    }

    fn finish(mut self) -> String {
        self.flush();
        self.blocks.join("\n")
    }
}

/// Collapses any whitespace run into a single space and trims the edges.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces bare http(s) URLs with "link: domain", preserving surrounding
/// whitespace (text arrives in chunks, edges can't be trimmed — words would
/// glue together).
fn rewrite_urls(text: &str, loc: &Locale) -> String {
    if !text.contains("://") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    // `split_inclusive` keeps each chunk's trailing whitespace character.
    for piece in text.split_inclusive(char::is_whitespace) {
        let core = piece.trim_end();
        out.push_str(&rewrite_token(core, loc));
        out.push_str(&piece[core.len()..]);
    }
    out
}

/// Replaces a single token if it's an http(s) URL (surrounding punctuation is preserved).
fn rewrite_token(token: &str, loc: &Locale) -> String {
    const LEAD: [char; 4] = ['(', '[', '«', '"'];
    const TRAIL: [char; 9] = [')', ']', '»', '"', '.', ',', ';', '!', '?'];
    let lead_len = token.len() - token.trim_start_matches(LEAD).len();
    let (lead, rest) = token.split_at(lead_len);
    let core_len = rest.trim_end_matches(TRAIL).len();
    let (core, trail) = rest.split_at(core_len);
    let lower = core.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return token.to_string();
    }
    let domain = domain_of(core);
    let spoken = loc.tf("speak.link", &[("domain", &domain)]);
    format!("{lead}{spoken}{trail}")
}

/// A URL's domain: without the scheme, path, port, or `www.` prefix.
fn domain_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(rest)
        .trim_start_matches("www.");
    host.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn speak(md: &str) -> String {
        speakable_text(md, ru())
    }

    /// Golden: a representative LLM reply — prose + code + mermaid + a table
    /// + formulas + links + a list + a heading. Checks the WHOLE output at once.
    #[test]
    fn golden_typical_llm_answer() {
        let md = "\
# Отчёт

Сложность равна $O(n \\log n)$, см. `sort()`.

```rust
fn main() {}
```

```mermaid
flowchart LR
    A --> B
```

| A | B |
| :--- | :--- |
| 1 | 2 |

$$E = mc^2$$

Подробности — [в документации](https://docs.example.com/guide) и на https://example.org/page.

- первый пункт
- второй пункт";
        let got = speak(md);
        let expected = "\
Отчёт.
Сложность равна O(n log n), см. sort().
(блок кода пропущен).
(диаграмма пропущена).
(таблица пропущена).
(формула пропущена).
Подробности — в документации и на ссылка: example.org.
первый пункт.
второй пункт.";
        assert_eq!(got, expected);
    }

    /// Per-locale gate: the same text in English contains no Cyrillic and
    /// differs from the Russian version (catches a forgotten localization of
    /// the notes).
    #[test]
    fn english_output_has_no_cyrillic() {
        let md = "Text.\n\n```rust\nfn main() {}\n```\n\n| A |\n| - |\n| 1 |\n\n$$x$$\n\n![](i.png)\n\nhttps://example.com";
        let en = speakable_text(md, locale(Lang::En));
        let ru_out = speak(md);
        assert_ne!(en, ru_out, "the notes aren't localized");
        assert!(
            !en.chars()
                .any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')),
            "Cyrillic in the English output: {en}"
        );
        assert!(en.contains("(code block skipped)"), "{en}");
        assert!(en.contains("(table skipped)"), "{en}");
        assert!(en.contains("(formula skipped)"), "{en}");
        assert!(en.contains("(image)"), "{en}");
        assert!(en.contains("link: example.com"), "{en}");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert_eq!(speak(""), "");
        assert_eq!(speak("   \n\n\t "), "");
    }

    /// A message made of a single code block — only the note (nothing to speak).
    #[test]
    fn only_code_block_gives_note() {
        assert_eq!(
            speak("```python\nprint('hi')\n```"),
            "(блок кода пропущен)."
        );
        // An indented block — also code.
        assert_eq!(speak("    let x = 1;"), "(блок кода пропущен).");
    }

    /// An unclosed fence (a stream cutoff) — the content still isn't read.
    #[test]
    fn unclosed_fence_is_skipped() {
        let got = speak("Начало.\n\n```rust\nfn main() {\n");
        assert_eq!(got, "Начало.\n(блок кода пропущен).");
    }

    /// Nested lists: each item — its own phrase, markers aren't spoken.
    #[test]
    fn nested_lists_speak_item_by_item() {
        let got = speak("- один\n  - вложенный\n- два");
        assert_eq!(got, "один.\nвложенный.\nдва.");
    }

    /// An autolink `<url>` and a bare URL are read as the domain; `www.` is dropped.
    #[test]
    fn autolink_and_bare_url_read_as_domain() {
        assert_eq!(
            speak("см. <https://www.example.com/a/b?q=1> тут"),
            "см. ссылка: example.com тут."
        );
        assert_eq!(
            speak("сайт https://example.org, потом текст"),
            "сайт ссылка: example.org, потом текст."
        );
    }

    /// A link's text is read, the URL is dropped.
    #[test]
    fn link_reads_text_without_url() {
        assert_eq!(
            speak("открой [документацию](https://docs.example.com)"),
            "открой документацию."
        );
    }

    /// An image: alt is read, no alt — a note.
    #[test]
    fn image_reads_alt_or_note() {
        assert_eq!(speak("![схема сети](i.png)"), "схема сети.");
        assert_eq!(speak("![](i.png)"), "(изображение).");
    }

    /// Inline code within a sentence is read as text and doesn't break the phrase.
    #[test]
    fn inline_code_stays_in_sentence() {
        assert_eq!(
            speak("вызови `note_save` перед выходом"),
            "вызови note_save перед выходом."
        );
    }

    /// Headings and quotes — separate phrases with a pause.
    #[test]
    fn headings_and_quotes_are_separate_phrases() {
        assert_eq!(
            speak("## Итог\n\n> цитата без точки\n\nконец!"),
            "Итог.\nцитата без точки.\nконец!"
        );
    }

    /// Inline math converts, a price range isn't swallowed by the math extension.
    #[test]
    fn inline_math_and_price_range() {
        assert_eq!(speak("формула $x^2$ тут"), "формула x² тут.");
        assert_eq!(speak("товар $5-$10 сегодня"), "товар $5-$10 сегодня.");
    }

    /// Emoji pass through as-is (engines read them by name or ignore them).
    #[test]
    fn emoji_pass_through() {
        assert_eq!(speak("готово 🎉"), "готово 🎉.");
    }

    /// A multiline paragraph (soft breaks) collapses into one phrase with no
    /// dangling spaces or double gaps.
    #[test]
    fn soft_breaks_collapse_to_single_spaces() {
        assert_eq!(
            speak("первая\nвторая\n\n\nтретья"),
            "первая вторая.\nтретья."
        );
    }

    /// The domain is computed without the scheme/path/port.
    #[test]
    fn domain_extraction() {
        assert_eq!(domain_of("https://example.com/a?b=1#c"), "example.com");
        assert_eq!(domain_of("http://www.example.com:8080/x"), "example.com");
    }
}
