//! Сегментация строки на слова для спелл-чека. См. spec §11.5.
//!
//! Используем собственный посимвольный сканер «буквенных пробегов» вместо полной
//! UAX#29-сегментации: для проверки орфографии нужны именно последовательности
//! букв (ru/en), а соединители (апостроф `'`/`’`, дефис `-`/`‑`) учитываются
//! только ВНУТРИ слова (`don't`, `well-known` — один токен; `слово-` — без
//! хвостового дефиса). Индексы — по символам (совпадают с моделью `InputBox`).

/// Слово в строке: диапазон по индексам символов `[start, end)` и его текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Является ли символ внутрисловным соединителем (учитывается между буквами).
fn is_connector(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '-' | '\u{2010}')
}

/// Разбивает строку на слова (буквенные пробеги с внутренними соединителями).
pub fn words(line: &str) -> Vec<Word> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
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
        // "ёж и" → "ёж" [0,2), "и" [3,4)
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
}
