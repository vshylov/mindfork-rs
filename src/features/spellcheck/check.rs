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
    ///
    /// Looked up **as typed** first, and only then without its combining marks
    /// ([`segment::strip_marks`]): a stress sign is not a spelling, and no
    /// dictionary entry carries a mark to lose, so the second lookup can only
    /// accept what the first rejected. The order is what keeps a *precomposed*
    /// diacritic answering for itself — `en_GB` holds `café` as `U+00E9`, and it
    /// is that entry that should accept it, not `en_US`'s `cafe`. See spec §11.5.
    pub fn check_word(&self, word: &str) -> bool {
        self.known(word) || segment::strip_marks(word).is_some_and(|plain| self.known(&plain))
    }

    /// One lookup of the word exactly as given: the personal dictionary, then
    /// the active ones.
    fn known(&self, word: &str) -> bool {
        self.personal.contains(word) || self.dicts.iter().any(|d| d.check(word))
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
    ///
    /// Asked about the word **without its combining marks**: no entry carries
    /// one, so a marked spelling has nothing to match against. The replacement
    /// therefore lands unmarked — someone correcting a misspelling is not asking
    /// to keep the stress they put on it. See spec §11.5.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let plain = segment::strip_marks(word);
        let word = plain.as_deref().unwrap_or(word);
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
    ///
    /// Stored **without its combining marks**, so one add covers every placement
    /// of the stress in that word — [`check_word`](Self::check_word)'s fallback
    /// is what finds it again — and the file stays a plain word list a person
    /// can edit. See spec §11.5.
    pub fn add_to_personal(&mut self, word: &str) -> std::io::Result<()> {
        let plain = segment::strip_marks(word);
        let word = plain.as_deref().unwrap_or(word);
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
    const DIC: &str = "4\nhello\nworld\ncolour\nистинно\n";

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

    #[test]
    fn a_stress_mark_neither_fails_a_word_nor_rescues_a_typo() {
        let c = checker();
        // Wherever the stress is put, the word is the same word (spec §11.5).
        assert!(c.check_word("и\u{301}стинно"));
        assert!(c.check_word("исти\u{301}нно"));
        assert!(c.check_word("hello\u{301}"));
        // The mark is ignored, not the spelling under it.
        assert!(!c.check_word("исти\u{301}но"));
        assert!(!c.check_word("helo\u{301}"));
    }

    #[test]
    fn a_stressed_word_is_underlined_whole_or_not_at_all() {
        let c = checker();
        assert!(c.misspellings("и\u{301}стинно").is_empty());
        // One wrong word, one range — and it spans the mark, so the underline
        // can't start mid-cluster.
        assert_eq!(c.misspellings("исти\u{301}но"), vec![(0, 7)]);
        // The popup offers the whole word too, not the half after the stress.
        let w = c.misspelled_word_at("исти\u{301}но", 2).unwrap();
        assert_eq!(w.text, "исти\u{301}но");
    }

    #[test]
    fn suggestions_come_from_the_unmarked_word() {
        let c = checker();
        assert_eq!(c.suggest("hel\u{301}o"), c.suggest("helo"));
        assert!(c.suggest("hel\u{301}o").iter().any(|w| w == "hello"));
    }

    #[test]
    fn a_personal_word_is_stored_without_its_marks() {
        let mut c = checker();
        c.add_to_personal("мойте\u{301}рмин").unwrap();
        assert!(c.personal.contains("мойтермин"));
        // …so every other placement of the stress is correct as well.
        assert!(c.check_word("мо\u{301}йтермин"));
        assert!(c.check_word("мойтермин"));
    }
}
