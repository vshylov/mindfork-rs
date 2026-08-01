//! The spellchecker: a set of active dictionaries + a personal dictionary. A word is correct
//! if accepted by **at least one** dictionary or the personal one (support for mixed
//! ru/en text). See spec §11.5.

use std::collections::HashSet;
use std::path::PathBuf;

use spellbook::Dictionary;

use super::dict;
use super::segment::{self, Word};

/// The maximum number of suggestions returned for one word.
const MAX_SUGGESTIONS: usize = 7;

/// The spellchecker. Created in the background via [`dict::load`].
pub struct SpellChecker {
    dicts: Vec<Dictionary>,
    personal: HashSet<String>,
    /// The personal dictionary's path (for appending on "add to dictionary").
    personal_path: Option<PathBuf>,
}

impl SpellChecker {
    pub fn new(
        dicts: Vec<Dictionary>,
        personal: HashSet<String>,
        personal_path: Option<PathBuf>,
    ) -> Self {
        Self {
            dicts,
            personal,
            personal_path,
        }
    }

    /// Is spellcheck enabled (at least one dictionary is present)? When it's off,
    /// [`misspellings`](Self::misspellings) doesn't underline anything.
    pub fn is_enabled(&self) -> bool {
        !self.dicts.is_empty()
    }

    /// Is the word correct (in the personal dictionary or any of the active ones)?
    pub fn check_word(&self, word: &str) -> bool {
        if self.personal.contains(word) {
            return true;
        }
        self.dicts.iter().any(|d| d.check(word))
    }

    /// Ranges (by character index) of misspelled words in the string. Empty if
    /// spellcheck is off.
    pub fn misspellings(&self, line: &str) -> Vec<(usize, usize)> {
        if !self.is_enabled() {
            return Vec::new();
        }
        segment::words(line)
            .into_iter()
            .filter(|w| !self.check_word(&w.text))
            .map(|w| (w.start, w.end))
            .collect()
    }

    /// Returns the word whose range contains the character position `col` (for the
    /// suggestions popup on the word under the cursor). `None` if the word is correct/absent.
    pub fn misspelled_word_at(&self, line: &str, col: usize) -> Option<Word> {
        if !self.is_enabled() {
            return None;
        }
        segment::words(line)
            .into_iter()
            .find(|w| col >= w.start && col <= w.end && !self.check_word(&w.text))
    }

    /// Correction suggestions (a union across all dictionaries, no duplicates).
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        for dict in &self.dicts {
            let mut local = Vec::new();
            dict.suggest(word, &mut local);
            for s in local {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            if out.len() >= MAX_SUGGESTIONS {
                break;
            }
        }
        out.truncate(MAX_SUGGESTIONS);
        out
    }

    /// Adds a word to the personal dictionary (in memory + appended to the file).
    pub fn add_to_personal(&mut self, word: &str) -> std::io::Result<()> {
        if !self.personal.insert(word.to_string()) {
            return Ok(()); // already present
        }
        if let Some(path) = &self.personal_path {
            dict::append_personal(path, word)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFF: &str = "SET UTF-8\n";
    const DIC: &str = "3\nhello\nworld\ncolour\n";

    fn checker() -> SpellChecker {
        let dict = Dictionary::new(AFF, DIC).unwrap();
        SpellChecker::new(vec![dict], HashSet::new(), None)
    }

    #[test]
    fn known_and_unknown_words() {
        let c = checker();
        assert!(c.check_word("hello"));
        assert!(!c.check_word("helo"));
    }

    #[test]
    fn personal_word_accepted() {
        let mut c = checker();
        assert!(!c.check_word("мойтермин"));
        c.add_to_personal("мойтермин").unwrap();
        assert!(c.check_word("мойтермин"));
    }

    #[test]
    fn misspellings_returns_ranges() {
        let c = checker();
        // "hello helo world" → the only error is in "helo" [6,10)
        let bad = c.misspellings("hello helo world");
        assert_eq!(bad, vec![(6, 10)]);
    }

    #[test]
    fn a_url_is_not_underlined() {
        let c = checker();
        // Nothing in the link is a dictionary word, yet only the prose is flagged.
        let line = "hello https://github.com/vshylov/mindfork-rs helo";
        assert_eq!(c.misspellings(line), vec![(45, 49)]);
        // The suggestions popup doesn't offer anything inside the link either.
        assert!(c.misspelled_word_at(line, 20).is_none());
    }

    #[test]
    fn disabled_checker_flags_nothing() {
        let c = SpellChecker::new(Vec::new(), HashSet::new(), None);
        assert!(!c.is_enabled());
        // With no dictionaries we underline nothing (misspellings — a UI input).
        assert!(c.misspellings("definitely wrng words zzz").is_empty());
        assert!(c.misspelled_word_at("wrng", 1).is_none());
    }

    #[test]
    fn misspelled_word_at_cursor() {
        let c = checker();
        let line = "hello helo world";
        // the cursor inside "helo" (positions 6..10)
        let w = c.misspelled_word_at(line, 8).unwrap();
        assert_eq!(w.text, "helo");
        // the cursor inside the correct "hello" → None
        assert!(c.misspelled_word_at(line, 2).is_none());
    }

    #[test]
    fn suggest_offers_alternatives() {
        let c = checker();
        let s = c.suggest("helo");
        assert!(s.iter().any(|w| w == "hello"));
    }
}
