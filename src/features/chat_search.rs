//! Chat search: a safe FTS5 `MATCH` query out of raw user input, plus the
//! message-level result types and their snippets (pure logic, tested without a
//! database). See `docs/research/chat-content-search.md` §4 (query syntax) and
//! §7a (stage 1 design), and `docs/history/chat-search-stage2.md` §4 (stage 2b).
//!
//! Raw input cannot be handed to `MATCH`: FTS5 reads it as *syntax*, and
//! ordinary text is routinely invalid syntax — `C++` is a syntax error near
//! `+`, and `cost-benefit` is read as a column filter, so the error blames a
//! column the user never typed. The fix is to treat every token as **text**:
//! quote it, doubling any inner quote. Multiple tokens then mean implicit AND,
//! which is what users expect.
//!
//! FSD note: `shared/storage` may not depend on `features`, so `CacheDb` takes
//! an already-escaped query and the orchestrator (in `app`) calls this.

use std::ops::Range;

use uuid::Uuid;

/// Shortest token the trigram tokenizer can match.
///
/// The index is built with `tokenize='trigram'`, so a token of one or two
/// characters cannot match anything at all (research §5).
pub const MIN_TOKEN_CHARS: usize = 3;

/// How many message hits the message-level search returns at most.
///
/// Measured on the real corpus (docs/history/chat-search-stage2.md §2): a common word
/// matches 163 messages across 50 chats, so this is a safety valve rather than
/// an everyday limit. The true count travels alongside as `total`, so the
/// screen can say "showing N of M" instead of silently truncating.
pub const HIT_CAP: usize = 200;

/// Characters of context a hit's snippet is built with. Two wrapped lines at a
/// typical terminal width — enough to see what the match sits in without the
/// list turning into a wall of text.
pub const SNIPPET_BUDGET_CHARS: usize = 160;

/// How far the snippet window may be nudged to land on a word boundary, as a
/// divisor of the budget. Beyond that we cut mid-word rather than lose context.
const BOUNDARY_SLACK_DIV: usize = 4;

/// Builds a safe FTS5 MATCH query from raw user input.
///
/// Every token is wrapped in double quotes (inner quotes doubled), so FTS5
/// operators and punctuation are matched literally rather than parsed. Tokens
/// shorter than [`MIN_TOKEN_CHARS`] are dropped instead of being allowed to
/// zero out the whole query — so `C++ ok` searches for `C++`.
///
/// Returns `None` when the input cannot produce a usable query — the caller
/// treats that exactly like an empty query (show everything unfiltered).
pub fn to_fts_query(input: &str) -> Option<String> {
    let query = input
        .split_whitespace()
        // Count characters, not bytes: a 3-character Cyrillic token is 6 bytes.
        .filter(|t| t.chars().count() >= MIN_TOKEN_CHARS)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    (!query.is_empty()).then_some(query)
}

/// A message-level hit's excerpt, with the positions of the matched tokens
/// inside it.
///
/// Built in Rust from the text the index already stores rather than by SQLite's
/// `snippet()` (fork **S4**): under the trigram tokenizer `snippet()`'s budget
/// counts 3-grams rather than words — 64 "tokens" yields ~70 characters — and
/// 64 is the documented ceiling even though this build accepts more. Doing it
/// here also gives exact offsets for highlighting and a budget in the unit the
/// screen actually cares about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snippet {
    /// The excerpt, with a leading/trailing `…` when it was cut.
    pub text: String,
    /// Matched spans as **byte** ranges into [`Snippet::text`] — ready to slice
    /// for highlighting. Empty when nothing matched (see [`build_snippet`]).
    pub matches: Vec<Range<usize>>,
}

/// A "open this chat with the feed on this message" request.
///
/// One value rather than two parallel event fields because the halves are one
/// thought: the query's matches are highlighted **inside the focused message
/// only** (docs/history/chat-search-stage2.md §4a S3(b)), so a query with no
/// target has nothing to mean, and a jump carries at most one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedFocus {
    /// The domain message to put the view on.
    pub message: Uuid,
    /// The raw search query the jump came from; its matches are highlighted
    /// inside that message. Empty when there is nothing to highlight.
    pub query: String,
}

/// One matching message.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub message_id: Uuid,
    /// `user` / `assistant` / … as the index stores it.
    pub role: String,
    /// Timestamp, RFC 3339 (the screen formats it).
    pub ts: String,
    pub snippet: Snippet,
}

/// The hits of one chat. Results are **grouped by chat** (fork **S2**): with
/// 163 hits for a common word a flat list needs either grouping or ranking, and
/// trigram's `bm25` is a weak proxy for relevance (research §5).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchGroup {
    pub chat_id: Uuid,
    pub title: String,
    /// In chat order (the orchestrator, which owns the chats, resolves it).
    pub hits: Vec<SearchHit>,
}

/// Builds an excerpt of `text` around the first place `query` matches.
///
/// Tokenized exactly like [`to_fts_query`] — whitespace-separated, tokens
/// shorter than [`MIN_TOKEN_CHARS`] dropped — and matched case-insensitively as
/// a substring, which is what the trigram index does for a token of three
/// characters or more.
///
/// **Nothing matching is a normal outcome, not a bug**: trigram matches
/// 3-grams, so FTS5 can return a message whose text contains every 3-gram of
/// the token without containing the token itself. Then the head of the text is
/// returned with no highlights, rather than an empty snippet.
///
/// The window is centred on the first match, cut on **character** boundaries
/// and nudged onto word boundaries where that costs little, with `…` marking
/// each end that was cut.
pub fn build_snippet(text: &str, query: &str, budget_chars: usize) -> Snippet {
    let chars: Vec<char> = text.chars().collect();
    if budget_chars == 0 || chars.is_empty() {
        return Snippet::default();
    }
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    let hits = find_matches(&folded, &query_tokens(query));

    let (start, end) = snap_to_words(
        &chars,
        window(chars.len(), hits.first(), budget_chars),
        hits.first(),
    );

    // Build the excerpt and, in the same pass, the byte offset of every window
    // character within it — the highlight ranges index the *snippet*, not the
    // source, and a char/byte mix-up here panics on the first Cyrillic hit.
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut byte_at: Vec<usize> = Vec::with_capacity(end - start + 1);
    for &c in &chars[start..end] {
        byte_at.push(out.len());
        out.push(c);
    }
    byte_at.push(out.len());
    let matches = hits
        .iter()
        // Only a match lying wholly inside the window: a clipped range would
        // highlight the wrong characters.
        .filter(|m| m.start >= start && m.end <= end)
        .map(|m| byte_at[m.start - start]..byte_at[m.end - start])
        .collect();
    if end < chars.len() {
        out.push('…');
    }
    Snippet { text: out, matches }
}

/// Every place `query` matches inside `haystack`, as **byte** ranges — ready to
/// slice, or to split a rendered line's spans on (the feed's post-render
/// highlight, fork **S3(b)**).
///
/// Shares its matcher with [`build_snippet`] — the same tokenization
/// (whitespace-separated, tokens under [`MIN_TOKEN_CHARS`] dropped), the same
/// case folding and the same overlap merging, all of it in [`find_matches`] —
/// so the results list and the feed cannot drift apart on what counts as a
/// match. `build_snippet` cuts its window in **characters** and therefore keeps
/// using that core directly; this is its byte-range face.
///
/// Ranges come out sorted, non-overlapping, and always on character boundaries
/// — a byte/char mix-up here would panic on the first Cyrillic slice.
pub fn match_ranges(haystack: &str, query: &str) -> Vec<Range<usize>> {
    let chars: Vec<char> = haystack.chars().collect();
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    let hits = find_matches(&folded, &query_tokens(query));
    if hits.is_empty() {
        return Vec::new();
    }
    let byte_at = byte_offsets(&chars);
    hits.iter()
        .map(|m| byte_at[m.start]..byte_at[m.end])
        .collect()
}

/// The byte offset of every character, plus the total length — the char→byte
/// map [`match_ranges`] converts through.
fn byte_offsets(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::with_capacity(chars.len() + 1);
    let mut at = 0;
    for c in chars {
        out.push(at);
        at += c.len_utf8();
    }
    out.push(at);
    out
}

/// The query's searchable tokens, case-folded — the same set [`to_fts_query`]
/// hands to FTS5.
fn query_tokens(query: &str) -> Vec<Vec<char>> {
    query
        .split_whitespace()
        .filter(|t| t.chars().count() >= MIN_TOKEN_CHARS)
        .map(|t| t.chars().map(fold_char).collect())
        .collect()
}

/// Case folding for matching, one char in one char out.
///
/// `char::to_lowercase` may expand (`İ` → `i` + a combining dot), which would
/// break the 1:1 char mapping the offsets rest on; taking the first character
/// keeps the mapping and is exact for every script this application indexes.
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Every occurrence of any token, as char ranges, sorted and with overlaps
/// merged (so two tokens matching the same run yield one highlight, not nested
/// ones).
fn find_matches(folded: &[char], tokens: &[Vec<char>]) -> Vec<Range<usize>> {
    let mut found: Vec<Range<usize>> = Vec::new();
    for token in tokens {
        if token.is_empty() || token.len() > folded.len() {
            continue;
        }
        let mut i = 0;
        while i + token.len() <= folded.len() {
            if folded[i..i + token.len()] == token[..] {
                found.push(i..i + token.len());
                i += token.len();
            } else {
                i += 1;
            }
        }
    }
    found.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in found {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// The char window of width `budget` centred on the first match (or the head of
/// the text when nothing matched), clamped to the text.
fn window(len: usize, first: Option<&Range<usize>>, budget: usize) -> (usize, usize) {
    if budget >= len {
        return (0, len);
    }
    let Some(m) = first else {
        return (0, budget);
    };
    let mid = m.start + (m.end - m.start) / 2;
    let mut start = mid.saturating_sub(budget / 2).min(len - budget);
    // A match longer than the budget would otherwise start off-window; showing
    // its beginning beats showing its middle.
    if m.start < start {
        start = m.start.min(len - budget);
    }
    (start, start + budget)
}

/// Nudges the window's edges onto word boundaries when one is within reach,
/// never eating into `protect` — the first match, which is the whole reason the
/// window sits where it does.
fn snap_to_words(
    chars: &[char],
    (start, end): (usize, usize),
    protect: Option<&Range<usize>>,
) -> (usize, usize) {
    let slack = (end - start) / BOUNDARY_SLACK_DIV;
    let keep_from = protect.map_or(end, |m| m.start);
    let keep_to = protect.map_or(start, |m| m.end);

    let mut s = start;
    if s > 0 && !chars[s - 1].is_whitespace() {
        // `keep_from >= s` by construction (`window` pins the start at or
        // before the match), but the slice below must not be able to panic.
        let stop = (s + slack).min(end).min(keep_from).max(s);
        if let Some(offset) = chars[s..stop].iter().position(|c| c.is_whitespace()) {
            s += offset + 1;
        }
    }
    let mut e = end;
    if e < chars.len() && !chars[e].is_whitespace() {
        let floor = e.saturating_sub(slack).max(s).max(keep_to);
        let mut i = e;
        while i > floor {
            i -= 1;
            if chars[i].is_whitespace() {
                e = i;
                break;
            }
        }
    }
    (s, e.max(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of these is a measured FTS5 failure on raw input (research §4);
    /// quoting turns every one of them into a plain literal search.
    #[test]
    fn measured_syntax_failures_become_quoted_literals() {
        // `fts5: syntax error near "+"`
        assert_eq!(to_fts_query("C++").unwrap(), "\"C++\"");
        // `no such column: benefit` — read as a column filter.
        assert_eq!(to_fts_query("cost-benefit").unwrap(), "\"cost-benefit\"");
        // `fts5: syntax error near "%"`
        assert_eq!(to_fts_query("50%").unwrap(), "\"50%\"");
        // `fts5: syntax error near "AND"` — an operator, not a word.
        assert_eq!(to_fts_query("AND").unwrap(), "\"AND\"");
        // `no such column: a` — read as a column filter.
        assert_eq!(to_fts_query("a:b").unwrap(), "\"a:b\"");
    }

    /// `"quoted` is the unbalanced-string failure (`unterminated string`).
    /// Doubling the inner quote makes it a balanced literal: the outer pair
    /// delimits, and `""` inside means one literal `"`.
    #[test]
    fn unbalanced_quote_is_doubled_into_a_balanced_literal() {
        assert_eq!(to_fts_query("\"quoted").unwrap(), "\"\"\"quoted\"");
        assert_eq!(to_fts_query("he\"llo").unwrap(), "\"he\"\"llo\"");
    }

    /// `(` is also a measured failure (`syntax error near ""`), but it is one
    /// character, so the trigram floor drops it first and nothing survives.
    #[test]
    fn single_character_syntax_token_is_dropped_not_quoted() {
        assert_eq!(to_fts_query("("), None);
    }

    /// The char-vs-byte trap: the Cyrillic tokens below are 3 and 2 characters
    /// but 6 and 4 bytes. A byte-length floor would keep both.
    #[test]
    fn token_length_counts_characters_not_bytes() {
        assert_eq!("мир".len(), 6);
        assert_eq!(to_fts_query("мир").unwrap(), "\"мир\"");

        assert_eq!("ми".len(), 4);
        assert_eq!(to_fts_query("ми"), None);
    }

    #[test]
    fn multiple_tokens_are_quoted_and_space_joined() {
        // Space-joined means implicit AND in FTS5: both must occur in the row.
        assert_eq!(to_fts_query("hello world").unwrap(), "\"hello\" \"world\"");
    }

    /// A short token must not zero out the whole query.
    #[test]
    fn short_tokens_are_dropped_and_the_rest_survives() {
        assert_eq!(to_fts_query("C++ ok").unwrap(), "\"C++\"");
        assert_eq!(to_fts_query("a memory b").unwrap(), "\"memory\"");
    }

    #[test]
    fn nothing_to_search_yields_none() {
        assert_eq!(to_fts_query(""), None);
        assert_eq!(to_fts_query("   \t \n "), None);
        assert_eq!(to_fts_query("a bc d"), None);
    }

    #[test]
    fn surrounding_and_repeated_whitespace_is_normalized() {
        assert_eq!(
            to_fts_query("  alpha \t\n beta  ").unwrap(),
            "\"alpha\" \"beta\""
        );
    }

    // ---- build_snippet ----

    /// The pieces of `s.text` the highlight ranges point at. Slicing is the
    /// point of the assertion: a range on a non-character boundary panics here.
    fn highlighted(s: &Snippet) -> Vec<&str> {
        s.matches.iter().map(|r| &s.text[r.clone()]).collect()
    }

    #[test]
    fn snippet_centres_the_window_on_the_first_match() {
        let text = format!("{}МАРКЕР{}", "а".repeat(400), "б".repeat(400));
        let s = build_snippet(&text, "маркер", 60);
        assert!(
            s.text.starts_with('…') && s.text.ends_with('…'),
            "{}",
            s.text
        );
        assert_eq!(highlighted(&s), vec!["МАРКЕР"]);
        // Centred: comparable context on both sides, not the head of the text.
        let before = s
            .text
            .chars()
            .take_while(|c| *c == 'а' || *c == '…')
            .count();
        let after = s
            .text
            .chars()
            .rev()
            .take_while(|c| *c == 'б' || *c == '…')
            .count();
        assert!(
            before.abs_diff(after) <= 4,
            "the match should sit in the middle: {before} vs {after} in {}",
            s.text
        );
    }

    #[test]
    fn snippet_ranges_every_match_inside_the_window() {
        let s = build_snippet("alpha beta alpha gamma alpha", "alpha", 200);
        assert_eq!(highlighted(&s), vec!["alpha", "alpha", "alpha"]);
        // Several query tokens all highlight.
        let s = build_snippet("alpha beta gamma", "alpha gamma", 200);
        assert_eq!(highlighted(&s), vec!["alpha", "gamma"]);
    }

    /// The trap this whole design is shaped around: highlight ranges are BYTE
    /// ranges into the snippet, while the window is cut in CHARACTERS. Cyrillic
    /// is two bytes per character, so a char/byte mix-up slices mid-character
    /// and panics — which is what `highlighted` exercises.
    #[test]
    fn snippet_ranges_are_valid_byte_boundaries_for_cyrillic() {
        let text = "Начало текста, затем СЛОВО, и продолжение фразы дальше";
        let s = build_snippet(text, "слово", 24);
        assert_eq!(highlighted(&s), vec!["СЛОВО"]);
        for r in &s.matches {
            assert!(s.text.is_char_boundary(r.start) && s.text.is_char_boundary(r.end));
        }
        // And the excerpt itself never splits a character.
        assert!(s.text.chars().count() > 0);
    }

    #[test]
    fn snippet_truncation_marks_both_cut_ends() {
        let text = "a".repeat(300);
        let s = build_snippet(&text, "zzz", 50);
        assert!(s.text.starts_with('a'), "the head is not cut: {}", s.text);
        assert!(s.text.ends_with('…'));

        // Cut at the front too when the match sits deep in the text.
        let text = format!("{} target", "word ".repeat(100));
        let s = build_snippet(&text, "target", 40);
        assert!(s.text.starts_with('…'), "{}", s.text);
        assert!(!s.text.ends_with('…'), "the tail was reached: {}", s.text);
    }

    /// Trigram matches 3-grams, so FTS5 can hand us a message that does not
    /// contain the token literally. That must read as "here is the start of the
    /// message", never as an empty snippet or a panic.
    #[test]
    fn snippet_without_a_match_falls_back_to_the_head() {
        let s = build_snippet("совершенно другой текст сообщения", "нечто", 20);
        assert!(s.matches.is_empty());
        assert!(s.text.starts_with("совершенно"), "{}", s.text);
        assert!(s.text.ends_with('…'));
    }

    #[test]
    fn snippet_shorter_than_the_budget_is_returned_whole() {
        let s = build_snippet("short text", "text", 500);
        assert_eq!(s.text, "short text");
        assert_eq!(highlighted(&s), vec!["text"]);
        assert!(!s.text.contains('…'), "nothing was cut, so no ellipsis");
    }

    #[test]
    fn snippet_of_an_empty_query_is_the_head_with_no_matches() {
        // Nothing searchable — including a query whose tokens are all below the
        // trigram floor, which `to_fts_query` drops too.
        for query in ["", "   ", "ab c"] {
            let s = build_snippet("некоторый текст сообщения", query, 100);
            assert!(s.matches.is_empty(), "query {query:?}");
            assert_eq!(s.text, "некоторый текст сообщения");
        }
    }

    #[test]
    fn snippet_matches_case_insensitively_and_as_a_substring() {
        // Case folding both ways, and an infix — trigram's behaviour (§1.2).
        let s = build_snippet("Тестовое Сообщение", "ЕСТОВ", 100);
        assert_eq!(highlighted(&s), vec!["естов"]);
        let s = build_snippet("mixed CASE here", "case", 100);
        assert_eq!(highlighted(&s), vec!["CASE"]);
    }

    #[test]
    fn snippet_prefers_word_boundaries_without_losing_the_match() {
        let text = "первое второе третье МАРКЕР четвёртое пятое шестое седьмое";
        let s = build_snippet(text, "маркер", 30);
        assert_eq!(highlighted(&s), vec!["МАРКЕР"], "{}", s.text);
        // The trimmed excerpt starts and ends on whole words.
        let inner = s.text.trim_matches('…');
        assert!(!inner.starts_with(' ') || inner.trim().is_empty());
        assert!(
            text.contains(inner.trim()),
            "the excerpt must be a slice of the source: {inner:?}"
        );
    }

    #[test]
    fn snippet_of_a_zero_budget_is_empty_rather_than_a_panic() {
        assert_eq!(build_snippet("текст", "текст", 0), Snippet::default());
        assert_eq!(build_snippet("", "текст", 50), Snippet::default());
    }

    // ---- match_ranges (the feed's post-render highlight, fork S3(b)) ----

    /// The pieces `match_ranges` points at. Slicing is the assertion: a range on
    /// a non-character boundary panics here.
    fn matched<'a>(haystack: &'a str, query: &str) -> Vec<&'a str> {
        match_ranges(haystack, query)
            .into_iter()
            .map(|r| &haystack[r])
            .collect()
    }

    /// The reason `match_ranges` exists rather than a second matcher: the list
    /// and the feed must agree on what counts as a match. With a budget larger
    /// than the text the snippet is the text itself, so their ranges are
    /// directly comparable.
    #[test]
    fn match_ranges_agrees_with_build_snippet() {
        for (text, query) in [
            ("alpha beta alpha gamma", "alpha"),
            ("Тестовое Сообщение здесь", "сообщение"),
            ("alpha beta gamma", "alpha gamma"),
            ("совершенно другой текст", "нечто"),
            ("overlapping tokens: reference", "refer erence"),
        ] {
            let s = build_snippet(text, query, 10_000);
            assert_eq!(s.text, text, "precondition: nothing was cut ({query:?})");
            assert_eq!(
                match_ranges(text, query),
                s.matches,
                "the feed and the results list must find the same matches in \
                 {text:?} for {query:?}"
            );
        }
    }

    /// The trigram floor is counted in **characters**: a byte-length floor would
    /// keep the 2-character (4-byte) Cyrillic token and highlight noise the
    /// index could never have matched.
    #[test]
    fn match_ranges_drops_tokens_below_the_character_floor() {
        assert_eq!(matched("мир и мы", "мир"), vec!["мир"]);
        assert!(match_ranges("мир и мы", "мы").is_empty());
        // A short token must not zero out the rest of the query either.
        assert_eq!(matched("a memory b", "a memory b"), vec!["memory"]);
    }

    #[test]
    fn match_ranges_is_case_insensitive_and_matches_substrings() {
        assert_eq!(matched("mixed CASE here", "case"), vec!["CASE"]);
        assert_eq!(matched("Тестовое", "ЕСТОВ"), vec!["естов"]);
    }

    /// The trap the whole design rests on: ranges are BYTE offsets while
    /// matching runs over CHARACTERS. Cyrillic is two bytes per character, so a
    /// mix-up slices mid-character and panics.
    #[test]
    fn match_ranges_are_valid_byte_boundaries_for_cyrillic() {
        let text = "Начало, затем СЛОВО, и продолжение — СЛОВО снова";
        assert_eq!(matched(text, "слово"), vec!["СЛОВО", "СЛОВО"]);
        for r in match_ranges(text, "слово") {
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
        }
    }

    /// Sorted and non-overlapping, so a consumer can walk them once: two tokens
    /// matching the same run must yield one range, not nested ones.
    #[test]
    fn match_ranges_are_sorted_and_merged() {
        let ranges = match_ranges("reference material", "refer erence ference");
        assert_eq!(ranges, vec![0..9]);
        let ranges = match_ranges("alpha gamma alpha", "alpha gamma");
        assert!(
            ranges.windows(2).all(|w| w[0].end <= w[1].start),
            "{ranges:?}"
        );
    }

    #[test]
    fn match_ranges_of_nothing_searchable_is_empty() {
        for query in ["", "   ", "ab c"] {
            assert!(
                match_ranges("некоторый текст", query).is_empty(),
                "{query:?}"
            );
        }
        assert!(match_ranges("", "текст").is_empty());
    }
}
