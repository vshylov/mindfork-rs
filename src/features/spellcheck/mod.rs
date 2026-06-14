//! Спелл-чек (features/spellcheck): Hunspell-словари (`spellbook`) + сегментация
//! + персональный словарь. Слово корректно, если принято хотя бы одним активным
//! словарём. См. spec §11.5.
//!
//! - [`segment`] — разбиение строки на слова (учёт апострофов/дефисов, ru/en).
//! - [`dict`] — загрузка словарей из `dictionaries/` и персонального словаря.
//! - [`check`] — [`SpellChecker`]: проверка, подсказки, добавление в словарь.

pub mod check;
pub mod dict;
pub mod segment;

pub use check::SpellChecker;
pub use segment::Word;
