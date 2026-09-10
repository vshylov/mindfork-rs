//! Segments a string into words for spellchecking. See spec §11.5.
//!
//! We use our own character-by-character "letter run" scanner instead of full
//! UAX#29 segmentation: spellchecking specifically needs sequences of
//! letters (ru/en), and connectors (an apostrophe `'`/`’`, a hyphen `-`/`‑`) count
//! only INSIDE a word (`don't`, `well-known` — one token; a trailing hyphen on a
//! word is dropped). Indices are by character (matching the `InputBox` model).
//!
//! **A combining mark belongs to the letter before it** ([`is_mark`]): a
//! Russian word carrying a stress sign is one word, not the two halves the mark
//! would otherwise split it into. `char::is_alphabetic` is false for every one
//! of those marks, so without the rule a letter run ends at the stress — see
//! [`strip_marks`] for the other half of the answer.
//!
//! **URLs and email addresses are skipped whole**: an address isn't prose, and
//! its host and path fragments (`github`, `mindfork`, `rs`, `blob`, …) would
//! otherwise be underlined word by word — noise on exactly the text a user
//! pastes rather than types.

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

/// Is the character a combining mark a letter carries?
///
/// The Combining Diacritical Marks block, and only it: the Russian stress sign
/// `U+0301` (and `U+0300` for a secondary one), the decomposed halves of `ё` and
/// `й` (`U+0308`, `U+0306`), and the Latin diacritics of a decomposed `café`.
/// Unicode calls them `Mn`; `char` exposes no general category, so the block
/// range is the whole test — one comparison on a path that runs over every word
/// of every line. See spec §11.5.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}')
}

/// The word without its combining marks — `None` when it carries none, so an
/// ordinary word allocates nothing.
///
/// This is what makes a stress mark not a spelling: the three bundled `.dic`
/// files hold **zero** combining marks (measured — docs/research/
/// spellcheck-stress-marks.md §2.2), so a lookup on the stripped form can only
/// accept what the marked one rejected, never the reverse. Precomposed letters
/// are single characters and are left alone — `en_GB`'s `café` (`U+00E9`) keeps
/// answering for itself.
pub fn strip_marks(word: &str) -> Option<String> {
    word.chars()
        .any(is_mark)
        .then(|| word.chars().filter(|c| !is_mark(*c)).collect())
}

/// Does a whitespace-delimited token look like a URL or an email address?
///
/// Deliberately a heuristic over four shapes, not a parser: an explicit scheme
/// (`https://example.com`), a `www.` prefix, a bare domain (`example.com/path`)
/// and an address (`user@example.com`, with an optional `mailto:`). A bare
/// domain additionally has to be **lowercase ASCII** — that restriction is what
/// keeps a run-on sentence (`end.Next`, and its far more common Cyrillic
/// equivalent: a missing space after a period) from reading as a domain and
/// silently switching the check off for a typo.
fn is_link_token(token: &str) -> bool {
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
    if core.contains('@') {
        return is_email(core);
    }
    is_bare_domain(core, DomainCase::AsWritten)
}

/// `user@example.com`, `Name.Surname+tag@mail.example.co.uk`, `mailto:user@x.io`.
///
/// The `@` makes the shape unambiguous, so unlike a bare domain the host may be
/// written in any case — there is no run-on sentence to confuse it with. The
/// local part keeps its own case either way (`Vladimir.Shylov@…`).
fn is_email(core: &str) -> bool {
    let core = match core.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("mailto:") => &core[7..],
        _ => core,
    };
    // Split at the last `@`: the local part may legally contain one, the host may not.
    let Some((local, host)) = core.rsplit_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._%+-'".contains(c));
    local_ok && is_bare_domain(host, DomainCase::Insensitive)
}

/// Whether a domain has to be written in lowercase to count as one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DomainCase {
    /// A bare domain: lowercase only, so `end.Next` stays a typo (see [`is_link_token`]).
    AsWritten,
    /// After an `@`: the shape is already unambiguous, so any case will do.
    Insensitive,
}

/// `example.com`, `sub.example.co.uk/path?q=1`, `example.com:8080` — a host of
/// ASCII labels ending in an alphabetic TLD.
fn is_bare_domain(core: &str, case: DomainCase) -> bool {
    let host = core.split(['/', '?', '#', ':']).next().unwrap_or_default();
    let host = match case {
        DomainCase::AsWritten => host.to_string(),
        DomainCase::Insensitive => host.to_ascii_lowercase(),
    };
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

/// Character ranges `[start, end)` of link-like tokens in the line.
fn link_spans(chars: &[char]) -> Vec<(usize, usize)> {
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
        if is_link_token(&token) {
            spans.push((start, i));
        }
    }
    spans
}

/// The end of the letter run that begins at `start` — the character index one
/// past the word, i.e. its `Word::end`.
///
/// The run grows over letters, the combining marks they carry ([`is_mark`]) and
/// a connector with a letter behind it ([`is_connector`]); `chars[start]` is
/// assumed to be a letter, which is what [`words`] checks before calling. Split
/// out of `words` so neither is a nest of branches (Sonar `rust:S3776`).
fn word_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    loop {
        // A word *starts* at a letter, so a mark here always has one behind it.
        if i < chars.len() && (chars[i].is_alphabetic() || is_mark(chars[i])) {
            i += 1;
        } else if i + 1 < chars.len() && is_connector(chars[i]) && chars[i + 1].is_alphabetic() {
            i += 2;
        } else {
            return i;
        }
    }
}

/// Splits a string into words (letter runs with internal connectors and the
/// combining marks their letters carry), skipping URLs and email addresses.
pub fn words(line: &str) -> Vec<Word> {
    let chars: Vec<char> = line.chars().collect();
    let links = link_spans(&chars);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A link is skipped as a whole token, so no word can start inside one.
        if let Some(&(_, end)) = links.iter().find(|&&(s, e)| i >= s && i < e) {
            i = end;
            continue;
        }
        if !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        i = word_end(&chars, start);
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
    fn a_combining_mark_stays_inside_its_word() {
        // `U+0301` is how Russian marks stress, and `char::is_alphabetic` is
        // false for it — the word used to end there (spec §11.5).
        assert_eq!(texts("И\u{301}стинно так"), ["И\u{301}стинно", "так"]);
        assert_eq!(texts("по-мо\u{301}ему"), ["по-мо\u{301}ему"]);
        // A trailing mark belongs to the letter before it, punctuation doesn't.
        assert_eq!(texts("хорошо\u{301}!"), ["хорошо\u{301}"]);
        // A decomposed `ё` and a decomposed `café` are the same shape.
        assert_eq!(texts("е\u{308}жик"), ["е\u{308}жик"]);
        assert_eq!(texts("cafe\u{301}"), ["cafe\u{301}"]);
    }

    #[test]
    fn a_marked_word_range_covers_its_mark() {
        // Eight characters, the mark at index 1 — the range has to span it, or
        // the underline would start mid-cluster.
        let w = words("И\u{301}стинно");
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].start, w[0].end), (0, 8));
    }

    #[test]
    fn an_orphaned_mark_starts_no_word() {
        // A word begins at a letter; a mark left on its own (deleting the letter
        // it sat on can leave one) is skipped like any other non-letter.
        assert_eq!(texts("да \u{301} нет"), ["да", "нет"]);
        assert!(words("\u{301}").is_empty());
    }

    #[test]
    fn strip_marks_leaves_an_unmarked_word_alone() {
        assert_eq!(strip_marks("И\u{301}стинно").as_deref(), Some("Истинно"));
        assert_eq!(strip_marks("е\u{308}жик").as_deref(), Some("ежик"));
        // Nothing to strip — no allocation, and the caller keeps its own string.
        assert_eq!(strip_marks("Истинно"), None);
        // A precomposed letter is one character, not a letter plus a mark.
        assert_eq!(strip_marks("café"), None);
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
    fn emails_are_skipped_whole() {
        assert_eq!(
            texts("пиши vladimir.shylov@outlook.com сюда"),
            ["пиши", "сюда"]
        );
        // The `@` makes the shape unambiguous, so the host may be in any case,
        // and the local part keeps its own.
        assert_eq!(texts("на Vladimir.Shylov@Outlook.COM ок"), ["на", "ок"]);
        assert_eq!(texts("тег user+tag@mail.example.co.uk да"), ["тег", "да"]);
        assert_eq!(texts("mailto:user@example.io готово"), ["готово"]);
        assert_eq!(texts("<user@example.com>, ок"), ["ок"]);
    }

    #[test]
    fn an_at_sign_alone_is_not_an_address() {
        // A mention has no domain; a missing space around `@` isn't one either.
        assert_eq!(
            texts("привет @username и @self"),
            ["привет", "username", "и", "self"]
        );
        assert_eq!(texts("собака@дома"), ["собака", "дома"]);
        assert_eq!(texts("a@b пиши"), ["a", "b", "пиши"]);
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
