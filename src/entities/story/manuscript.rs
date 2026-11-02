//! The Author's stores beyond the canon and the fabula: the chapter's outline,
//! its versions and their summaries, the reviewers' notes and the Author's
//! decisions on them, and the pauses — the questions a run stops at, the
//! human's answers, and the assumptions a role records instead of stopping
//! (docs/research/fabula-engine.md §5–§7, §14 *Pauses*; docs/fabula.md §4,
//! PR 6).
//!
//! Every row is written once and never changed: a revision is a new version
//! of its chapter, an answer a row beside its question.

use serde::{Deserialize, Serialize};

use super::{EventId, RunId};

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
    /// A review note's id: its chapter version, its reviewer and its ordinal,
    /// `ch1/v1/reader/n2`.
    NoteId
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

impl NoteId {
    /// The `n`-th note (from 1) `reviewer` wrote on `chapter`.
    pub fn nth(chapter: &ChapterId, reviewer: Reviewer, n: u64) -> Self {
        Self(format!("{chapter}/{}/n{n}", reviewer.as_str()))
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

/// A chapter's goals, set once before its scenes are planned (spec §7, step
/// 1): what the chapter has to achieve, not which scenes it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChapterOutline {
    /// The goals, one sentence each.
    pub goals: Vec<String>,
}

/// Where a chapter's version came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionOrigin {
    /// The first draft (spec §7, step 6).
    Draft,
    /// A revision after the reviewers' notes (step 8).
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

/// Who wrote a review note (spec §5, *Review*). Character voice waits for M4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reviewer {
    /// A clean context that reads only the published text.
    Reader,
    /// The text held to the log and the canon.
    Continuity,
}

stored_names!(Reviewer {
    Reader => "reader",
    Continuity => "continuity",
});

/// What a review note is about (spec §8, *Agent contracts*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// The Reader could not follow a passage.
    Confusion,
    /// The Reader found a passage slow.
    SlowPassage,
    /// What the Reader expects to happen next: a twist it predicted is no
    /// twist (spec §8). Not triaged.
    Prediction,
    /// What the Reader liked best. Not triaged.
    BestMoment,
    /// The text says what the log or the canon contradicts.
    Contradiction,
    /// A character acts on, or the text tells through them, what they cannot
    /// know yet.
    KnowledgeLeak,
    /// A character in the wrong place, or one the scene does not have.
    CastError,
    /// Events told in an order the log does not allow.
    Timeline,
    /// A planted secret's marker copied into the prose: the engine's own
    /// deterministic check, not a model's.
    Token,
}

stored_names!(NoteKind {
    Confusion => "confusion",
    SlowPassage => "slow_passage",
    Prediction => "prediction",
    BestMoment => "best_moment",
    Contradiction => "contradiction",
    KnowledgeLeak => "knowledge_leak",
    CastError => "cast_error",
    Timeline => "timeline",
    Token => "token",
});

impl NoteKind {
    /// Whether the Author decides on notes of this kind: the Reader's
    /// predictions and best moment are observations, kept for the evaluation.
    pub fn is_triaged(self) -> bool {
        !matches!(self, NoteKind::Prediction | NoteKind::BestMoment)
    }
}

/// How much a note weighs: an accepted high-severity note asks for a revision
/// (spec §7, step 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSeverity {
    /// Worth fixing if the chapter is revised anyway.
    Low,
    /// The chapter is not done while it stands.
    High,
}

stored_names!(NoteSeverity {
    Low => "low",
    High => "high",
});

/// A review note as its reviewer writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNote {
    /// What it is about.
    pub kind: NoteKind,
    /// How much it weighs.
    pub severity: NoteSeverity,
    /// A short quote of the text it is about; none when it is about the
    /// whole.
    pub anchor: Option<String>,
    /// The note.
    pub body: String,
    /// The events it cites, by id.
    pub evidence: Vec<EventId>,
}

/// A review note as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNote {
    /// Its id.
    pub id: NoteId,
    /// The version it is about.
    pub chapter: ChapterId,
    /// Who wrote it.
    pub reviewer: Reviewer,
    /// The note.
    pub note: NewNote,
}

/// The Author's decision on a note (spec §7, step 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteDecision {
    /// The revision acts on it.
    Accepted,
    /// It is wrong, or not worth acting on.
    Rejected,
    /// For a later chapter.
    Deferred,
}

stored_names!(NoteDecision {
    Accepted => "accepted",
    Rejected => "rejected",
    Deferred => "deferred",
});

/// A decision as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided {
    /// The note.
    pub note: NoteId,
    /// The decision.
    pub decision: NoteDecision,
    /// Why, in a sentence.
    pub reason: String,
}

/// Where a run stops for the human (spec §14, *Pauses*). P4, the canon's
/// deltas, waits for M4.
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
    /// P3: every branch of a scene failed on every attempt (spec §7, step 4).
    Scene {
        /// The chapter.
        chapter: u32,
        /// The scene.
        scene: u32,
        /// The last attempt's rerun.
        rerun: u32,
        /// What failed, one line a branch of the last attempt.
        errors: Vec<String>,
    },
}

/// The question kinds' stored names, the `kind` tag of [`QuestionBody`].
pub const QUESTION_KINDS: &[&str] = &["canon", "chapter", "scene"];

impl QuestionBody {
    /// The stored name of the kind.
    pub fn kind(&self) -> &'static str {
        match self {
            QuestionBody::Canon => QUESTION_KINDS[0],
            QuestionBody::Chapter { .. } => QUESTION_KINDS[1],
            QuestionBody::Scene { .. } => QUESTION_KINDS[2],
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
    /// Yes: the canon approved, the chapter accepted, the scene tried again.
    Approve,
    /// Not like this: the canon to be changed, the chapter revised with the
    /// notes, the scene planned again with them.
    Change,
    /// A question answered: the scene tried again with the notes.
    Answer,
}

stored_names!(Decision {
    Approve => "approve",
    Change => "change",
    Answer => "answer",
});

/// A question's answer: the decision, the human's notes for the engine, and
/// the evidence the decision rests on — the messages' ids and quotes, which
/// the chat tools fill (docs/fabula.md §4, PR 9).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The decision.
    pub decision: Decision,
    /// What the human asked for, for the Author; empty when nothing.
    pub notes: String,
    /// What it rests on.
    pub evidence: serde_json::Value,
}

/// A doubt a role met inside a chapter and the choice it made instead of
/// stopping (spec §14, *Pauses*); the chapter's checkpoint lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assumption {
    /// What the human could have been asked.
    pub doubt: String,
    /// What was chosen.
    pub choice: String,
}

/// An assumption as stored, with where it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAssumption {
    /// Its id.
    pub id: String,
    /// The run.
    pub run: RunId,
    /// The step, `ch1/sc2/plan`.
    pub step: String,
    /// The role that made it.
    pub role: String,
    /// The assumption.
    pub assumption: Assumption,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_derived_from_their_position() {
        let chapter = ChapterId::of(1, 2);
        assert_eq!(chapter.as_str(), "ch1/v2");
        assert_eq!(
            NoteId::nth(&chapter, Reviewer::Continuity, 3).as_str(),
            "ch1/v2/continuity/n3"
        );
        assert_eq!(QuestionId::nth(4).to_string(), "q4");
    }

    #[test]
    fn every_stored_name_reads_back() {
        for v in VersionOrigin::ALL {
            assert_eq!(VersionOrigin::parse(v.as_str()), Some(*v));
        }
        for v in Reviewer::ALL {
            assert_eq!(Reviewer::parse(v.as_str()), Some(*v));
        }
        for v in NoteKind::ALL {
            assert_eq!(NoteKind::parse(v.as_str()), Some(*v));
            let json = serde_json::to_value(v).unwrap();
            assert_eq!(
                json,
                serde_json::json!(v.as_str()),
                "serde spells it the same"
            );
        }
        for v in NoteSeverity::ALL {
            assert_eq!(NoteSeverity::parse(v.as_str()), Some(*v));
        }
        for v in NoteDecision::ALL {
            assert_eq!(NoteDecision::parse(v.as_str()), Some(*v));
        }
        for v in Decision::ALL {
            assert_eq!(Decision::parse(v.as_str()), Some(*v));
        }
        assert_eq!(VersionOrigin::parse("final"), None);
    }

    #[test]
    fn only_the_readers_observations_escape_the_triage() {
        let untriaged: Vec<_> = NoteKind::ALL.iter().filter(|k| !k.is_triaged()).collect();
        assert_eq!(untriaged, [&NoteKind::Prediction, &NoteKind::BestMoment]);
    }

    #[test]
    fn a_questions_kind_is_its_tag() {
        let bodies = [
            QuestionBody::Canon,
            QuestionBody::Chapter {
                chapter: 1,
                version: 2,
            },
            QuestionBody::Scene {
                chapter: 1,
                scene: 2,
                rerun: 2,
                errors: vec!["k0: cut".to_owned()],
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
