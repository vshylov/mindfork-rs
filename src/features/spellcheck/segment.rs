//! Segments a string into words for spellchecking. See spec §11.5.
//!
//! We use our own character-by-character "letter run" scanner instead of full
//! UAX#29 segmentation: spellchecking specifically needs sequences of
//! letters (ru/en), and connectors (an apostrophe `'`/`’`, a hyphen `-`/`‑`) count
//! only INSIDE a word (`don't`, `well-known` — one token; a trailing hyphen on a
//! word is dropped). Indices are by character (matching the `InputBox` model).
//!
//! **URLs are skipped whole**: a link isn't prose, and its host and path
//! fragments (`github`, `mindfork`, `rs`, `blob`, …) would otherwise be
//! underlined word by word — noise on exactly the text a user pastes rather than
//! types.

/// A word in a string: a range of character indices `[start, end)` and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Is the character an in-word connector (counted between letters)?
fn is_connector(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '-' | '\u{2010}')
}

/// Does a whitespace-delimited token look like a URL?
///
/// Deliberately a heuristic over three shapes, not a parser: an explicit scheme
/// (`https://example.com`), a `www.` prefix, and a bare domain
/// (`example.com/path`). A bare domain additionally has to be **lowercase
/// ASCII** — that restriction is what keeps a run-on sentence (`end.Next`, and
/// its far more common Cyrillic equivalent: a missing space after a period)
/// from reading as a domain and silently switching the check off for a typo.
fn is_url_token(token: &str) -> bool {
    // A scheme is unambiguous even with punctuation around it: `(https://x)`.
    if token.contains("://") {
        return true;
    }
    // Strip surrounding punctuation: `«example.com»`, a trailing sentence period.
    let core = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
    if core
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("www."))
    {
        return core.len() > 4;
    }
    is_bare_domain(core)
}

/// `example.com`, `sub.example.co.uk/path?q=1`, `example.com:8080` — a host of
/// lowercase ASCII labels ending in an alphabetic TLD.
fn is_bare_domain(core: &str) -> bool {
    let host = core.split(['/', '?', '#', ':']).next().unwrap_or_default();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if !labels.iter().all(label_ok) {
        return false;
    }
    let tld = labels[labels.len() - 1];
    (2..=24).contains(&tld.len()) && tld.chars().all(|c| c.is_ascii_lowercase())
}

/// Character ranges `[start, end)` of URL-like tokens in the line.
fn url_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let token: String = chars[start..i].iter().collect();
        if is_url_token(&token) {
            spans.push((start, i));
        }
    }
    spans
}

/// Splits a string into words (letter runs with internal connectors), skipping
/// URLs.
pub fn words(line: &str) -> Vec<Word> {
    let chars: Vec<char> = line.chars().collect();
    let urls = url_spans(&chars);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A URL is skipped as a whole token, so no word can start inside one.
        if let Some(&(_, end)) = urls.iter().find(|&&(s, e)| i >= s && i < e) {
            i = end;
            continue;
        }
        if !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        loop {
            if i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            } else if i + 1 < chars.len() && is_connector(chars[i]) && chars[i + 1].is_alphabetic()
            {
                i += 2;
            } else {
                break;
            }
        }
        out.push(Word {
            start,
            end: i,
            text: chars[start..i].iter().collect(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str) -> Vec<String> {
        words(line).into_iter().map(|w| w.text).collect()
    }

    #[test]
    fn splits_plain_words() {
        assert_eq!(texts("hello world"), ["hello", "world"]);
    }

    #[test]
    fn mixed_ru_en() {
        assert_eq!(texts("привет hello мир"), ["привет", "hello", "мир"]);
    }

    #[test]
    fn keeps_internal_apostrophe_and_hyphen() {
        assert_eq!(texts("don't well-known"), ["don't", "well-known"]);
        assert_eq!(texts("don\u{2019}t"), ["don\u{2019}t"]);
    }

    #[test]
    fn drops_trailing_and_leading_connectors() {
        assert_eq!(texts("-word- 'quote'"), ["word", "quote"]);
    }

    #[test]
    fn ranges_are_char_indices() {
        // Two Cyrillic words: the first spans chars [0,2), the second [3,4).
        let w = words("ёж и");
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].start, w[0].end), (0, 2));
        assert_eq!((w[1].start, w[1].end), (3, 4));
    }

    #[test]
    fn punctuation_and_digits_separate_words() {
        assert_eq!(texts("foo, bar! 42 baz"), ["foo", "bar", "baz"]);
    }

    #[test]
    fn empty_and_no_words() {
        assert!(words("").is_empty());
        assert!(words("123 !!! ---").is_empty());
    }

    #[test]
    fn urls_are_skipped_whole() {
        // A scheme, `www.`, a bare domain and a path — none of their fragments
        // become words, while the prose around them still does.
        assert_eq!(
            texts("см https://github.com/vshylov/mindfork-rs/blob/main тут"),
            ["см", "тут"]
        );
        assert_eq!(texts("open www.Example.COM now"), ["open", "now"]);
        assert_eq!(texts("see example.com/path?q=1 ok"), ["see", "ok"]);
        assert_eq!(texts("host example.com:8080 up"), ["host", "up"]);
        assert_eq!(texts("ftp://host/file.txt done"), ["done"]);
    }

    #[test]
    fn punctuation_around_a_url_does_not_hide_it() {
        assert_eq!(texts("(https://example.com), да"), ["да"]);
        assert_eq!(texts("«example.com». Всё"), ["Всё"]);
    }

    #[test]
    fn a_run_on_sentence_is_not_a_domain() {
        // The case the lowercase-ASCII restriction exists for: a missing space
        // after a period must stay checkable.
        assert_eq!(texts("конец.Начало"), ["конец", "Начало"]);
        assert_eq!(texts("end.Next"), ["end", "Next"]);
        // Ordinary prose with punctuation is untouched.
        assert_eq!(texts("и т.д. дальше"), ["и", "т", "д", "дальше"]);
        assert_eq!(texts("версия 3.14 сборки"), ["версия", "сборки"]);
    }

    #[test]
    fn word_ranges_survive_a_url_in_the_line() {
        // The offsets after a skipped URL still address the original line.
        let w = words("а https://x.com б");
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].start, w[0].end), (0, 1));
        assert_eq!((w[1].start, w[1].end), (16, 17));
    }
}
