//! Builds a safe FTS5 `MATCH` query out of raw user input (pure logic, tested
//! without a database). See `docs/research/chat-content-search.md` §4 (query
//! syntax) and §7a (stage 1 design).
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

// Ahead of its consumer: the orchestrator wiring that calls this lands with the
// rest of stage 1. Remove this attribute once it does.
#![allow(dead_code)]

/// Shortest token the trigram tokenizer can match.
///
/// The index is built with `tokenize='trigram'`, so a token of one or two
/// characters cannot match anything at all (research §5).
pub const MIN_TOKEN_CHARS: usize = 3;

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
}
