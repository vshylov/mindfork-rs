//! The replies of the Author and the reviewers (docs/research/fabula-engine.md
//! §7, §8 *Agent contracts*; docs/fabula.md §4, PR 6): the chapter's outline,
//! the next scene's plan, the pick among a scene's branches, the Reader's and
//! the Continuity checker's notes, and the Author's decisions on them. The
//! chapter's prose comes back as text, not as one of these.
//!
//! - **The deciding fields come last** (spec §8, *Property order is
//!   generation order*): a scene's private intent before its circumstances,
//!   the readings of the branches and the note for the human before the
//!   choice, a note's text before its kind and severity, a decision's
//!   critique before the decision.
//! - **Story content, not reasoning.** No field asks for the model's own
//!   reasoning, which Claude 5.x refuses as `reasoning_extraction` (spec §8),
//!   and none is named like one (the spec's `rationale` and `reason` are
//!   `human_note` and `critique` here): an intent is the Author's plan for the
//!   story, a note for the human is theirs to read, an assumption a question
//!   that could have been put to them.
//! - **Built per call where a call's options differ**: the first scene's plan
//!   cannot end the chapter, the last attempt's pick cannot ask for another
//!   rerun, a pick names only the branches that played, and a triage only the
//!   notes it is shown. Every other schema is stable per story (spec §8).
//! - **A shape is not a sense.** The engine checks a planned scene against the
//!   world before it is stored ([`super::chapter`]).

use serde::{Deserialize, Serialize};

use super::schema::{CanonKeys, Property, Schema, StorySchema, Variant};
use crate::entities::story::{
    Assumption, CharacterKey, LocationKey, NoteDecision, NoteKind, NoteSeverity, SceneSetup,
};

/// What every assumption field tells the model.
const ASSUMPTIONS: &str = "Questions about the story you could have put to its human author \
    and did not, each with the answer you chose for now; empty when you had none. Written in \
    the human author's language.";

impl StorySchema for Assumption {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Object(vec![
            Property::described(
                "doubt",
                "The question about the story, in a sentence.",
                String::schema(keys),
            ),
            Property::described(
                "choice",
                "The answer you chose for now, in a sentence.",
                String::schema(keys),
            ),
        ])
    }
}

fn assumptions(keys: &CanonKeys) -> Property {
    Property::described("assumptions", ASSUMPTIONS, Vec::<Assumption>::schema(keys))
}

/// A tagged union of `variants`, or with one variant left the closed object
/// it is, its tag a one-value `enum`: the dialect wants two branches in a
/// union, and serde reads the object as the variant it names either way.
fn union(variants: Vec<Variant>) -> Schema {
    let tag = "type";
    match <[Variant; 1]>::try_from(variants) {
        Ok([only]) => {
            let mut properties = vec![Property::new(tag, Schema::Enum(vec![only.name]))];
            properties.extend(only.properties);
            Schema::Object(properties)
        }
        Err(variants) => Schema::Tagged {
            tag: tag.to_owned(),
            variants,
        },
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// The chapter's outline (spec §7, step 1): its goals, not its scenes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutlineReply {
    /// What the chapter has to achieve, one sentence each.
    pub goals: Vec<String>,
    /// The doubts met, and the choices made.
    pub assumptions: Vec<Assumption>,
}

impl StorySchema for OutlineReply {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Object(vec![
            Property::described(
                "goals",
                "What this chapter has to achieve for the story, two to four goals of one \
                 sentence each. Goals, not scenes: what must have changed by the chapter's end.",
                Vec::<String>::schema(keys),
            ),
            assumptions(keys),
        ])
    }
}

/// A scene as the Author drafts it: the private intent first, then the given
/// circumstances (spec §4, rule 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDraft {
    /// What the Author hopes the scene does; no character and no World Master
    /// sees it.
    pub intent: String,
    /// Where it happens.
    pub location: LocationKey,
    /// Who is there.
    pub present: Vec<CharacterKey>,
    /// What has just happened.
    pub just_happened: String,
    /// What the world pushes.
    pub pressure: String,
}

impl SceneDraft {
    /// The circumstances, without the intent.
    pub fn setup(&self) -> SceneSetup {
        SceneSetup {
            location: self.location.clone(),
            present: self.present.clone(),
            just_happened: self.just_happened.clone(),
            pressure: self.pressure.clone(),
        }
    }

    /// The properties of a scene, as a plan and a rerun share them.
    fn properties(keys: &CanonKeys) -> Vec<Property> {
        vec![
            Property::described(
                "intent",
                "What you, the Author, hope this scene does for the chapter, in one or two \
                 sentences. Private: no character and no World Master ever sees it.",
                String::schema(keys),
            ),
            Property::described(
                "location",
                "Where the scene happens.",
                LocationKey::schema(keys),
            ),
            Property::described(
                "present",
                "Who is there when it opens; two to four characters, alive.",
                Vec::<CharacterKey>::schema(keys),
            ),
            Property::described(
                "just_happened",
                "What has just happened, in two or three sentences of the story's narration: \
                 the characters perceive it as the scene opens. Never what a character will do.",
                String::schema(keys),
            ),
            Property::described(
                "pressure",
                "What the world pushes now: a deadline, a danger, an arrival, a need. One or two \
                 sentences; never a character's choice.",
                String::schema(keys),
            ),
        ]
    }
}

/// The Author's next step in a chapter (spec §7, step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NextScene {
    /// Another scene.
    Scene {
        /// The Author's private intent.
        intent: String,
        /// Where.
        location: LocationKey,
        /// Who.
        present: Vec<CharacterKey>,
        /// What has just happened.
        just_happened: String,
        /// What the world pushes.
        pressure: String,
    },
    /// The chapter has what it needs.
    End,
}

impl NextScene {
    /// The scene drafted, or none at the chapter's end.
    pub fn draft(&self) -> Option<SceneDraft> {
        match self {
            NextScene::Scene {
                intent,
                location,
                present,
                just_happened,
                pressure,
            } => Some(SceneDraft {
                intent: intent.clone(),
                location: location.clone(),
                present: present.clone(),
                just_happened: just_happened.clone(),
                pressure: pressure.clone(),
            }),
            NextScene::End => None,
        }
    }
}

/// The plan of the chapter's next scene, or its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanReply {
    /// The next scene, or the end.
    pub next: NextScene,
    /// The doubts met, and the choices made.
    pub assumptions: Vec<Assumption>,
}

impl PlanReply {
    /// The plan's schema: with `may_end`, the chapter may end instead of
    /// having another scene; the first scene's plan cannot end it.
    pub fn schema_for(keys: &CanonKeys, may_end: bool) -> Schema {
        let mut variants = vec![Variant::new(
            "scene",
            "Another scene: your intent for it, then its given circumstances.",
            SceneDraft::properties(keys),
        )];
        if may_end {
            variants.push(Variant::new(
                "end",
                "The chapter has achieved its goals: end it here.",
                Vec::new(),
            ));
        }
        Schema::Object(vec![
            Property::described("next", "What comes next in the chapter.", union(variants)),
            assumptions(keys),
        ])
    }
}

/// The Author's reading of one branch against the rubric (spec §7, step 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchReading {
    /// Which branch, `k0`.
    pub branch: String,
    /// What the branch does for the chapter, in a sentence or two.
    pub note: String,
    /// Tension, from 1 to 5.
    pub tension: i64,
    /// Surprise, from 1 to 5.
    pub surprise: i64,
    /// Truth to character, from 1 to 5.
    pub truth: i64,
    /// Consequences for what follows, from 1 to 5.
    pub consequences: i64,
}

impl BranchReading {
    /// The four scores, each held to 1–5, added up: the dialect has no
    /// numeric bounds, so a score outside them is clamped.
    pub fn total(&self) -> i64 {
        [self.tension, self.surprise, self.truth, self.consequences]
            .into_iter()
            .map(|s| s.clamp(1, 5))
            .sum()
    }
}

/// What the Author keeps of a scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PickChoice {
    /// This branch extends the main line.
    Branch {
        /// Which, `k1`.
        branch: String,
    },
    /// None of them: the scene is played again under changed circumstances.
    Rerun {
        /// The Author's private intent.
        intent: String,
        /// Where.
        location: LocationKey,
        /// Who.
        present: Vec<CharacterKey>,
        /// What has just happened.
        just_happened: String,
        /// What the world pushes.
        pressure: String,
    },
}

impl PickChoice {
    /// The rerun's scene, or none for a branch.
    pub fn rerun(&self) -> Option<SceneDraft> {
        match self {
            PickChoice::Rerun {
                intent,
                location,
                present,
                just_happened,
                pressure,
            } => Some(SceneDraft {
                intent: intent.clone(),
                location: location.clone(),
                present: present.clone(),
                just_happened: just_happened.clone(),
                pressure: pressure.clone(),
            }),
            PickChoice::Branch { .. } => None,
        }
    }
}

/// The Author's pick among a scene's branches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickReply {
    /// One reading per branch shown.
    pub readings: Vec<BranchReading>,
    /// The note for the human: why this choice serves the story.
    pub human_note: String,
    /// The branch kept, or a rerun.
    pub choice: PickChoice,
    /// The doubts met, and the choices made.
    pub assumptions: Vec<Assumption>,
}

impl PickReply {
    /// The pick's schema over the branches that played (`k0`, `k2`); with
    /// `may_rerun`, the Author may ask for the scene again instead.
    pub fn schema_for(keys: &CanonKeys, branches: &[String], may_rerun: bool) -> Schema {
        let branch = || Schema::Enum(branches.to_vec());
        let score = |name: &str, what: &str| {
            Property::described(name, &format!("{what}, from 1 to 5."), Schema::Integer)
        };
        let reading = Schema::Object(vec![
            Property::described("branch", "Which version.", branch()),
            Property::described(
                "note",
                "What this version does for the chapter, in one or two sentences of story \
                 critique.",
                String::schema(keys),
            ),
            score("tension", "How much it strains against the pressure"),
            score("surprise", "How little a reader would see it coming"),
            score("truth", "How true each character stays to who they are"),
            score("consequences", "How much it changes what can follow"),
        ]);
        let mut variants = vec![Variant::new(
            "branch",
            "Keep this version: it becomes what happened.",
            vec![Property::described("branch", "Which version.", branch())],
        )];
        if may_rerun {
            variants.push(Variant::new(
                "rerun",
                "Keep none: play the scene again under changed circumstances, written as a new \
                 scene's intent and setup.",
                SceneDraft::properties(keys),
            ));
        }
        Schema::Object(vec![
            Property::described(
                "readings",
                "One reading for each version shown, in the order shown.",
                Schema::Array(Box::new(reading)),
            ),
            Property::described(
                "human_note",
                "A note for the story's human author, in their language: why your choice serves \
                 the chapter, in one or two sentences.",
                String::schema(keys),
            ),
            Property::described(
                "choice",
                "The version the story keeps, or a rerun.",
                union(variants),
            ),
            assumptions(keys),
        ])
    }
}

/// What the Reader says about one passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderRemark {
    /// A short quote of the passage.
    pub anchor: String,
    /// What the Reader found.
    pub note: String,
    /// How much it weighs.
    pub severity: NoteSeverity,
}

/// The Reader's notes (spec §8): a clean context that knows only the
/// published text and the new chapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderReply {
    /// Passages the Reader could not follow.
    pub confusions: Vec<ReaderRemark>,
    /// Passages that dragged.
    pub slow_passages: Vec<ReaderRemark>,
    /// What the Reader expects to happen next.
    pub predictions: Vec<String>,
    /// What the Reader liked best.
    pub best_moment: String,
}

fn severity_schema() -> Schema {
    Schema::Enum(strings(&["low", "high"]))
}

impl StorySchema for ReaderReply {
    fn schema(keys: &CanonKeys) -> Schema {
        let remark = |what: &str| {
            Schema::Array(Box::new(Schema::Object(vec![
                Property::described(
                    "anchor",
                    "A short quote, a few words, of the passage.",
                    String::schema(keys),
                ),
                Property::described("note", what, String::schema(keys)),
                Property::described(
                    "severity",
                    "high when it would make a reader stop reading or misread the story; low \
                     otherwise.",
                    severity_schema(),
                ),
            ])))
        };
        Schema::Object(vec![
            Property::described(
                "confusions",
                "Passages you could not follow: who is speaking, where they are, what happened. \
                 Empty when none.",
                remark("What confused you, in a sentence."),
            ),
            Property::described(
                "slow_passages",
                "Passages that dragged. Empty when none.",
                remark("Why it dragged, in a sentence."),
            ),
            Property::described(
                "predictions",
                "What you expect to happen next in the story, one sentence each.",
                Vec::<String>::schema(keys),
            ),
            Property::described(
                "best_moment",
                "The moment of the new chapter you liked best, in a sentence.",
                String::schema(keys),
            ),
        ])
    }
}

/// What the Continuity checker found (spec §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// The text says what the log or the canon contradicts.
    Contradiction,
    /// Someone acts on, or tells, what they cannot know yet.
    KnowledgeLeak,
    /// A character in the wrong place, or one the scene does not have.
    CastError,
    /// Events in an order the log does not allow.
    Timeline,
}

impl FindingKind {
    /// The note kind it is stored as.
    pub fn note_kind(self) -> NoteKind {
        match self {
            FindingKind::Contradiction => NoteKind::Contradiction,
            FindingKind::KnowledgeLeak => NoteKind::KnowledgeLeak,
            FindingKind::CastError => NoteKind::CastError,
            FindingKind::Timeline => NoteKind::Timeline,
        }
    }
}

/// One finding of the Continuity checker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// A short quote of the text it is about.
    pub anchor: String,
    /// The events it cites, by id.
    pub evidence: Vec<String>,
    /// What is wrong.
    pub note: String,
    /// What kind of fault.
    pub kind: FindingKind,
    /// How much it weighs.
    pub severity: NoteSeverity,
}

/// The Continuity checker's findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuityReply {
    /// Every finding; empty when the text holds.
    pub findings: Vec<Finding>,
}

impl StorySchema for ContinuityReply {
    fn schema(keys: &CanonKeys) -> Schema {
        let finding = Schema::Object(vec![
            Property::described(
                "anchor",
                "A short quote, a few words, of the passage at fault.",
                String::schema(keys),
            ),
            Property::described(
                "evidence",
                "The ids of the events that show the fault, as listed; empty when the canon \
                 alone shows it.",
                Vec::<String>::schema(keys),
            ),
            Property::described(
                "note",
                "What the text says against the record, in a sentence.",
                String::schema(keys),
            ),
            Property::described(
                "kind",
                "contradiction: the text says what the events or the canon contradict; \
                 knowledge_leak: a character acts on, or the text tells through them, what they \
                 have not perceived; cast_error: a character where the events do not put them; \
                 timeline: events in an order the record does not allow.",
                Schema::Enum(strings(&[
                    "contradiction",
                    "knowledge_leak",
                    "cast_error",
                    "timeline",
                ])),
            ),
            Property::described(
                "severity",
                "high when it breaks the story's logic; low for a detail.",
                severity_schema(),
            ),
        ]);
        Schema::Object(vec![Property::described(
            "findings",
            "Every place where the chapter departs from the record; empty when it holds.",
            Schema::Array(Box::new(finding)),
        )])
    }
}

/// The Author's decision on one note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageEntry {
    /// The note's label as shown, `n2`.
    pub note: String,
    /// Why, in a sentence of story critique.
    pub critique: String,
    /// The decision.
    pub decision: NoteDecision,
}

/// The Author's decisions on a version's notes (spec §7, step 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageReply {
    /// One decision per note shown.
    pub decisions: Vec<TriageEntry>,
}

impl TriageReply {
    /// The triage's schema over the notes shown (`n1`, `n2`).
    pub fn schema_for(keys: &CanonKeys, labels: &[String]) -> Schema {
        let entry = Schema::Object(vec![
            Property::described("note", "Which note.", Schema::Enum(labels.to_vec())),
            Property::described(
                "critique",
                "Why you decide so, in a sentence of story critique.",
                String::schema(keys),
            ),
            Property::described(
                "decision",
                "accepted: the revision acts on it; rejected: it is wrong, or not worth acting \
                 on; deferred: it belongs to a later chapter.",
                Schema::Enum(strings(&["accepted", "rejected", "deferred"])),
            ),
        ]);
        Schema::Object(vec![Property::described(
            "decisions",
            "One decision for each note listed, in the order listed.",
            Schema::Array(Box::new(entry)),
        )])
    }
}

#[cfg(test)]
#[path = "author_tests.rs"]
mod tests;
