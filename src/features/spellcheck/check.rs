//! Спелл-чекер: набор активных словарей + персональный словарь. Слово корректно,
//! если принято **хотя бы одним** словарём или персональным (поддержка смешанного
//! ru/en текста). См. spec §11.5.

use std::collections::HashSet;
use std::path::PathBuf;

use spellbook::Dictionary;

use super::dict;
use super::segment::{self, Word};

/// Максимум подсказок, отдаваемых на одно слово.
const MAX_SUGGESTIONS: usize = 7;

/// Спелл-чекер. Создаётся в фоне через [`dict::load`].
pub struct SpellChecker {
    dicts: Vec<Dictionary>,
    personal: HashSet<String>,
    /// Путь к персональному словарю (для дозаписи при «добавить в словарь»).
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

    /// Включён ли спелл-чек (есть хотя бы один словарь). При выключенном
    /// [`misspellings`](Self::misspellings) ничего не подчёркивает.
    pub fn is_enabled(&self) -> bool {
        !self.dicts.is_empty()
    }

    /// Корректно ли слово (персональный словарь или любой из активных).
    pub fn check_word(&self, word: &str) -> bool {
        if self.personal.contains(word) {
            return true;
        }
        self.dicts.iter().any(|d| d.check(word))
    }

    /// Диапазоны (по индексам символов) слов с ошибками в строке. Пусто, если
    /// спелл-чек выключен.
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

    /// Возвращает слово, чей диапазон содержит позицию символа `col` (для попапа
    /// подсказок по слову под курсором). `None`, если слово корректно/отсутствует.
    pub fn misspelled_word_at(&self, line: &str, col: usize) -> Option<Word> {
        if !self.is_enabled() {
            return None;
        }
        segment::words(line)
            .into_iter()
            .find(|w| col >= w.start && col <= w.end && !self.check_word(&w.text))
    }

    /// Подсказки исправления (объединение из всех словарей, без дублей).
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

    /// Добавляет слово в персональный словарь (в память + дозапись в файл).
    pub fn add_to_personal(&mut self, word: &str) -> std::io::Result<()> {
        if !self.personal.insert(word.to_string()) {
            return Ok(()); // уже есть
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
        // "hello helo world" → ошибка только в "helo" [6,10)
        let bad = c.misspellings("hello helo world");
        assert_eq!(bad, vec![(6, 10)]);
    }

    #[test]
    fn disabled_checker_flags_nothing() {
        let c = SpellChecker::new(Vec::new(), HashSet::new(), None);
        assert!(!c.is_enabled());
        // При отсутствии словарей ничего не подчёркиваем (misspellings — UI-вход).
        assert!(c.misspellings("definitely wrng words zzz").is_empty());
        assert!(c.misspelled_word_at("wrng", 1).is_none());
    }

    #[test]
    fn misspelled_word_at_cursor() {
        let c = checker();
        let line = "hello helo world";
        // курсор внутри "helo" (позиции 6..10)
        let w = c.misspelled_word_at(line, 8).unwrap();
        assert_eq!(w.text, "helo");
        // курсор внутри корректного "hello" → None
        assert!(c.misspelled_word_at(line, 2).is_none());
    }

    #[test]
    fn suggest_offers_alternatives() {
        let c = checker();
        let s = c.suggest("helo");
        assert!(s.iter().any(|w| w == "hello"));
    }
}
