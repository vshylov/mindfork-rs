//! Spellcheck (features/spellcheck): Hunspell dictionaries (`spellbook`), segmentation,
//! and a personal dictionary. A word is correct if accepted by at least one active
//! dictionary. See spec §11.5.
//!
//! - [`segment`] — splitting a string into words (accounting for apostrophes/hyphens, ru/en).
//! - [`dict`] — loading dictionaries from `dictionaries/` and the personal dictionary.
//! - [`check`] — [`SpellChecker`]: checking, suggestions, adding to the dictionary.

pub mod check;
pub mod dict;
pub mod segment;

pub use check::SpellChecker;
