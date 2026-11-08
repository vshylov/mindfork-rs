//! The story's stores beyond the canon: a chapter's versions, and the pauses —
//! the questions a run stops at and the human's answers (docs/research/
//! fabula-engine.md §14 *Pauses*; docs/history/fabula.md §4, PR 6; §6, T26).
//!
//! Every row is written once and never changed: a revision is a new version
//! of its chapter, an answer a row beside its question.

use serde::{Deserialize, Serialize};

use super::RunId;

/// A string newtype for the ids below; the same shape as the module's
/// siblings' (`super::text_newtype`), kept local so this file stands alone.
macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps an id as it is, read back from the story database.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// The id itself.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_newtype!(
    /// A chapter version's id: its number and version, `ch1/v2`.
    ChapterId
);
id_newtype!(
    /// A question's id, `q1`, `q2`: one per pause, in the story's order.
    QuestionId
);

impl ChapterId {
    /// Version `version` of chapter `number`.
    pub fn of(number: u32, version: u32) -> Self {
        Self(format!("ch{number}/v{version}"))
    }
}

impl QuestionId {
    /// The `n`-th question (from 1) of a story.
    pub fn nth(n: u64) -> Self {
        Self(format!("q{n}"))
    }
}

/// Spells a fieldless enum as the names the database stores, and reads it
/// back.
macro_rules! stored_names {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $name {
            /// Every value, in order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The stored name.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            /// The value with this stored name.
            pub fn parse(s: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.as_str() == s)
            }
        }
    };
}

/// Where a chapter's version came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionOrigin {
    /// The first draft.
    Draft,
    /// A revision after the writer's own critique.
    Revision,
    /// A revision the human asked for at the chapter's checkpoint (P2).
    Human,
}

stored_names!(VersionOrigin {
    Draft => "draft",
    Revision => "revision",
    Human => "human",
});

/// One version of a chapter, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterVersion {
    /// Its id.
    pub id: ChapterId,
    /// The chapter, from 1.
    pub number: u32,
    /// The version, from 1.
    pub version: u32,
    /// Where it came from.
    pub origin: VersionOrigin,
    /// The text, in markdown.
    pub text: String,
}

/// Where a run stops for the human (spec §14, *Pauses*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionBody {
    /// P1, before chapter 1: the canon and the arc approved, or changed.
    Canon,
    /// P2, after each chapter: accept the version, or revise it with notes.
    Chapter {
        /// The chapter.
        chapter: u32,
        /// The version asked about.
        version: u32,
    },
}

/// The question kinds' stored names, the `kind` tag of [`QuestionBody`].
pub const QUESTION_KINDS: &[&str] = &["canon", "chapter"];

impl QuestionBody {
    /// The stored name of the kind.
    pub fn kind(&self) -> &'static str {
        match self {
            QuestionBody::Canon => QUESTION_KINDS[0],
            QuestionBody::Chapter { .. } => QUESTION_KINDS[1],
        }
    }
}

/// A pause's question as stored: what it asks, and the run and step it
/// stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Its id.
    pub id: QuestionId,
    /// The run that asked it.
    pub run: RunId,
    /// The step it stopped at.
    pub step: String,
    /// What it asks.
    pub body: QuestionBody,
}

/// What the human decided (spec §14, *The answer is the user's own words*):
/// the decision the conservative reading of their messages returns, `unclear`
/// aside, which resolves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Yes: the canon approved, the chapter accepted.
    Approve,
    /// Not like this: the canon to be changed, the chapter revised with the
    /// notes.
    Change,
}

stored_names!(Decision {
    Approve => "approve",
    Change => "change",
});

/// A question's answer: the decision, the human's notes for the writer, and
/// the evidence the decision rests on — the messages' ids and quotes, which
/// the chat tools fill (docs/history/fabula.md §4, PR 9).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The decision.
    pub decision: Decision,
    /// What the human asked for, for the writer; empty when nothing.
    pub notes: String,
    /// What it rests on.
    pub evidence: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_derived_from_their_position() {
        assert_eq!(ChapterId::of(1, 2).as_str(), "ch1/v2");
        assert_eq!(QuestionId::nth(4).to_string(), "q4");
    }

    #[test]
    fn every_stored_name_reads_back() {
        for v in VersionOrigin::ALL {
            assert_eq!(VersionOrigin::parse(v.as_str()), Some(*v));
        }
        for v in Decision::ALL {
            assert_eq!(Decision::parse(v.as_str()), Some(*v));
        }
        assert_eq!(VersionOrigin::parse("final"), None);
    }

    #[test]
    fn a_questions_kind_is_its_tag() {
        let bodies = [
            QuestionBody::Canon,
            QuestionBody::Chapter {
                chapter: 1,
                version: 2,
            },
        ];
        for body in bodies {
            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["kind"], body.kind());
            let back: QuestionBody = serde_json::from_value(json).unwrap();
            assert_eq!(back, body);
        }
    }
}
