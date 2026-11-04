//! The staging pass (docs/fabula.md §4, PR 12; §6, T17): a cheap read of a
//! chapter's text alone for what Gate G2's readers found and its reviewers
//! let pass.
//!
//! - **The text alone.** No canon, no record, no plan: what a reader cannot
//!   place in the text is a fault whatever the record says. The chapters
//!   before it come as their summaries, which the Canon keeper made from
//!   their text alone: a name a published chapter introduced is placed, and
//!   the context stays one chapter long. Read without them on G2's chapter 2,
//!   the pass took the heroine's brother, named in chapter 1, for a stranger.
//! - **What it reads**: who is where and holds what; to whom a pronoun, a
//!   "who" and a tie of kin point; and the lines — one that announces a plan,
//!   a decision or a boundary, one that opens on thanks, apology or agreement
//!   the moment does not call for. G2's chapter 2 had "son of Marya, who left
//!   with him", which no reviewer named, and seven thanks.
//! - **Its notes ride with the Continuity checker's** ([`super::chapter`]):
//!   one batch, so a resume asks both again or neither, and the triage reads
//!   them as the checker's. Every chapter is read, the first included.

use serde::{Deserialize, Serialize};

use super::port::Prompt;
use super::scene::bullets;
use super::schema::{CanonKeys, Property, Schema, StorySchema};
use crate::entities::story::{NewNote, NoteKind, NoteSeverity};

/// What the staging pass found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingKind {
    /// A character or a thing where the text did not put them.
    Staging,
    /// A pronoun, a "who" or a tie of kin a reader cannot place.
    Referent,
    /// A line that states a plan, a decision or a boundary aloud.
    Announced,
    /// A reply that opens on thanks, apology or agreement unbidden.
    Thanks,
}

impl StagingKind {
    /// The note kind it is stored as.
    pub fn note_kind(self) -> NoteKind {
        match self {
            StagingKind::Staging => NoteKind::Staging,
            StagingKind::Referent => NoteKind::Referent,
            StagingKind::Announced => NoteKind::Announced,
            StagingKind::Thanks => NoteKind::Thanks,
        }
    }
}

/// One finding of the staging pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingFinding {
    /// A short quote of the passage.
    pub anchor: String,
    /// What is wrong.
    pub note: String,
    /// What kind of fault.
    pub kind: StagingKind,
    /// How much it weighs.
    pub severity: NoteSeverity,
}

/// The staging pass's findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagingReply {
    /// Every finding; empty when the chapter holds.
    pub findings: Vec<StagingFinding>,
}

/// What each kind means, as the schema and the prompt say it.
const KINDS: &str = "staging: a character where the text did not take them, in two places at \
    once, holding what they put down or never took up, or seated, standing, come or gone \
    against what the text said just before; referent: a pronoun, a \"who\", a name or a tie of \
    kin a reader cannot place, or places on the wrong person, or a family tie the text's own \
    statements make impossible (whose son, whose mother, who left with whom); announced: a line \
    in which a character states a plan, a decision or a boundary aloud, as if reporting it, \
    where acting on it would show it; thanks: a reply that opens on thanks, apology or \
    agreement the moment does not call for.";

impl StorySchema for StagingReply {
    fn schema(keys: &CanonKeys) -> Schema {
        let finding = Schema::Object(vec![
            Property::described(
                "anchor",
                "A short quote, a few words, of the passage at fault.",
                String::schema(keys),
            ),
            Property::described(
                "note",
                "What is wrong, in a sentence: for a referent, whom a reader would take it to \
                 mean and whom the text means elsewhere; for staging, where the text last put \
                 the person or the thing.",
                String::schema(keys),
            ),
            Property::described(
                "kind",
                KINDS,
                Schema::Enum(
                    ["staging", "referent", "announced", "thanks"]
                        .map(str::to_owned)
                        .to_vec(),
                ),
            ),
            Property::described(
                "severity",
                "high when a reader would take a person, a place, a thing or a kin for the \
                 wrong one, or when the same fault of the lines runs through the chapter; low \
                 for a single line.",
                Schema::Enum(["low", "high"].map(str::to_owned).to_vec()),
            ),
        ]);
        Schema::Object(vec![Property::described(
            "findings",
            "Every fault of the chapter's staging, its referents and its lines; empty when it \
             holds.",
            Schema::Array(Box::new(finding)),
        )])
    }
}

/// The staging pass's prompt: the published chapters' summaries, by number,
/// and the version's text, nothing else.
pub fn staging_prompt(summaries: &[(u32, String)], text: &str) -> Prompt {
    let before = if summaries.is_empty() {
        "This is the story's first chapter.".to_owned()
    } else {
        let lines: Vec<String> = summaries
            .iter()
            .map(|(n, s)| format!("Chapter {n}: {s}"))
            .collect();
        format!(
            "The story so far, a summary of each published chapter:\n{}",
            bullets(lines.iter().map(String::as_str))
        )
    };
    Prompt {
        system: format!(
            "You read a new chapter of a story for its staging and its lines. You know only its \
             published text: the chapters before it in summaries, and the new one whole; no \
             plan, no notes, nothing of how it was made. A person, a place or a thing the \
             summaries name is known to the reader. Read the new chapter as a director reads a \
             scene before it is played: who is where, who holds what, who sits and who stands, \
             who has come and who has gone; to whom each pronoun, each \"who\" and each tie of \
             kin points; and what each line of dialogue does. Report each fault of these \
             kinds. {KINDS} A line that does what the moment calls for is no fault: thanks for \
             a gift, an apology for a wrong just done, a plan told to someone who must act on \
             it. Quote the passage. The plot, the pace and the style beyond these are not \
             yours to judge."
        ),
        user: format!(
            "{before}\n\nThe new chapter:\n\n{text}\n\nReport every fault of its staging, its \
             referents and its lines."
        ),
    }
}

/// The staging pass's findings as notes: a blank one dropped, an empty
/// anchor none.
pub fn staging_notes(reply: StagingReply) -> Vec<NewNote> {
    reply
        .findings
        .into_iter()
        .filter(|f| !f.note.trim().is_empty())
        .map(|f| NewNote {
            kind: f.kind.note_kind(),
            severity: f.severity,
            anchor: Some(f.anchor.trim().to_owned()).filter(|a| !a.is_empty()),
            body: f.note.trim().to_owned(),
            evidence: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;
