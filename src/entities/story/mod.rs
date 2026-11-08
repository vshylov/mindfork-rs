//! Fabula's domain: a story's canon, its chapters' versions and its pauses
//! (docs/research/fabula-engine.md §4–§5, §14; docs/fabula.md §3).
//!
//! Pure data and pure functions; the story database
//! (`shared::storage::story`) stores these types. Keys name what the canon
//! fixes (characters, locations, items, secrets) and are compared exactly;
//! ids name what grows during a story (runs, chapter versions, questions)
//! and are derived from their position, never drawn at random.

mod canon;
mod manuscript;

// The domain's whole surface.
#[allow(unused_imports)]
pub use canon::{
    Canon, CanonField, CanonProblem, ChapterArc, Character, CharacterStart, Holder, KeyKind,
    Location, Route, Secret, StoryArc, Style, World,
};
#[allow(unused_imports)]
pub use manuscript::{
    ChapterId, ChapterVersion, Decision, QUESTION_KINDS, Question, QuestionBody, QuestionId,
    Resolution, VersionOrigin,
};

use serde::{Deserialize, Serialize};

/// A string newtype: a key the canon fixes, or an id a run derives.
macro_rules! text_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a string as it is: a key from the canon, or an id read back
            /// from the story database.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// The string itself.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

text_newtype!(
    /// A cast member's key in the canon (`"mara"`).
    CharacterKey
);
text_newtype!(
    /// A location's key in the canon (`"harbour"`).
    LocationKey
);
text_newtype!(
    /// An item's key (`"ledger"`): an item starts in exactly one place, held
    /// by a character or lying at a location.
    ItemKey
);
text_newtype!(
    /// A secret's key; a secret belongs to the character whose sheet lists it.
    SecretKey
);
text_newtype!(
    /// A run's id, `run1`, `run2`: one per background run of the engine.
    RunId
);

impl RunId {
    /// The `n`-th run (from 1) of a story.
    pub fn nth(n: u64) -> Self {
        Self(format!("run{n}"))
    }
}

/// The canon fixture the story tests share: three characters, five locations,
/// a planted secret token on every sheet (spec §11, §13).
#[cfg(test)]
pub(crate) fn fixture_canon() -> Canon {
    Canon::from_json(include_str!("fixtures/harbour.json")).expect("the fixture parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_derived_from_their_position() {
        assert_eq!(RunId::nth(2).to_string(), "run2");
    }

    #[test]
    fn a_key_serializes_as_its_bare_string() {
        let key = CharacterKey::from("mara");
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"mara\"");
        let back: CharacterKey = serde_json::from_str("\"mara\"").unwrap();
        assert_eq!(back, key);
        assert_eq!(CharacterKey::new(String::from("mara")), key);
    }
}
