//! The Canon keeper's first part (docs/research/fabula-engine.md §5;
//! docs/fabula.md §4, PR 11): the story's register, and a chapter's summary,
//! both made from the version's text alone.
//!
//! - **The text alone.** The keeper sees no canon, no record and no plan,
//!   as the Reader does (spec §8): G2's summary, written by the Author with
//!   the whole canon before it, put a secret's hiding place into a chapter
//!   that shows the opposite.
//! - **The register** is what a chapter adds to the story's bookkeeping: the
//!   threads it opens, the earlier ones it pays off, and the facts of time,
//!   age and kinship it states, each with its quote and who states it. The
//!   ids are the engine's, by position (`ch1.thread2`, `ch1.fact3`), so a
//!   reply cannot name a thread that is not open.
//! - **The summary is checked** against the text by a call of its own, and
//!   written once more against what that call found.
//! - **What a chapter is shown** ([`SoFar`]) is the accepted chapters'
//!   registers, oldest first: the threads still open and every fact stated.
//!   The Author's prompts and the Continuity checker read it; in the arc's
//!   last chapter the checker reports each thread still open, and the Author
//!   pays it off or leaves it open with its reason (docs/fabula.md §6, T16).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::port::Prompt;
use super::scene::bullets;
use super::schema::{CanonKeys, Property, Schema, StorySchema};
use crate::entities::story::{
    ChapterVersion, Fact, NoteDecision, NoteKind, Payoff, Register, Thread,
};
use crate::shared::storage::story::{StoryDb, StoryDbError};

/// A thread as the keeper notes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadReply {
    /// Where the text plants it.
    pub quote: String,
    /// What the reader is left to wonder.
    pub question: String,
}

/// An open thread the chapter pays off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayoffReply {
    /// The thread's id, as listed.
    pub thread: String,
    /// How.
    pub how: String,
}

/// A fact as the keeper notes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactReply {
    /// Where the text states it.
    pub quote: String,
    /// The narration, or a character's name.
    pub stated_by: String,
    /// The fact, with its numbers.
    pub fact: String,
}

/// The keeper's reading of a chapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterReply {
    /// The open threads it pays off; absent when none is open.
    #[serde(default)]
    pub paid: Vec<PayoffReply>,
    /// The threads it opens.
    pub opened: Vec<ThreadReply>,
    /// The facts it states.
    pub facts: Vec<FactReply>,
}

fn quote_of(keys: &CanonKeys, what: &str) -> Property {
    Property::described(
        "quote",
        &format!("A short quote, a few words, of where the text {what}."),
        String::schema(keys),
    )
}

impl RegisterReply {
    /// The schema over the threads open before the chapter (`ch1.thread2`):
    /// with none open, the reply has nothing to pay off.
    pub fn schema_for(keys: &CanonKeys, open: &[String]) -> Schema {
        let mut properties = Vec::new();
        if !open.is_empty() {
            properties.push(Property::described(
                "paid",
                "The open threads listed above that this chapter pays off: answers, or settles \
                 for good. Empty when it pays off none.",
                Schema::Array(Box::new(Schema::Object(vec![
                    Property::described("thread", "Which thread.", Schema::Enum(open.to_vec())),
                    Property::described(
                        "how",
                        "How the chapter pays it off, in a sentence.",
                        String::schema(keys),
                    ),
                ]))),
            ));
        }
        properties.push(Property::described(
            "opened",
            "The threads this chapter opens and leaves open at its end. Empty when none.",
            Schema::Array(Box::new(Schema::Object(vec![
                quote_of(keys, "plants it"),
                Property::described(
                    "question",
                    "What the reader is left to wonder, in a sentence.",
                    String::schema(keys),
                ),
            ]))),
        ));
        properties.push(Property::described(
            "facts",
            "Every fact of time, age and kinship this chapter states. Empty when none.",
            Schema::Array(Box::new(Schema::Object(vec![
                quote_of(keys, "states it"),
                Property::described(
                    "stated_by",
                    "Who states it: \"narration\", or the name of the character who says it.",
                    String::schema(keys),
                ),
                Property::described(
                    "fact",
                    "The fact, plainly, with its numbers: who, what, when or how long ago, how \
                     old, whose kin.",
                    String::schema(keys),
                ),
            ]))),
        ));
        Schema::Object(properties)
    }
}

/// A sentence of a summary the text does not bear out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryFault {
    /// The summary's words at fault.
    pub sentence: String,
    /// What the text says instead.
    pub text_says: String,
}

/// The check of a summary against its chapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryCheck {
    /// Every fault; empty when the summary holds.
    pub faults: Vec<SummaryFault>,
}

impl StorySchema for SummaryCheck {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Object(vec![Property::described(
            "faults",
            "Each sentence of the summary that says what the text does not bear out, \
             contradicts it, or contradicts the summary itself. Empty when it holds.",
            Schema::Array(Box::new(Schema::Object(vec![
                Property::described(
                    "sentence",
                    "The summary's words at fault, quoted.",
                    String::schema(keys),
                ),
                Property::described(
                    "text_says",
                    "What the text says instead, or that it says nothing of it, in a sentence.",
                    String::schema(keys),
                ),
            ]))),
        )])
    }
}

/// The register a reply makes for chapter `chapter`: the ids assigned by
/// position, blanks dropped, a payoff kept only for a thread open in
/// `so_far` and only once.
pub fn register_of(chapter: u32, reply: RegisterReply, so_far: &SoFar) -> Register {
    let open = so_far.open_ids();
    let mut paid: Vec<Payoff> = Vec::new();
    for p in reply.paid {
        let thread = p.thread.trim().trim_matches('`').to_owned();
        if open.contains(&thread) && !paid.iter().any(|q| q.thread == thread) {
            paid.push(Payoff {
                thread,
                how: p.how.trim().to_owned(),
            });
        }
    }
    let opened = reply
        .opened
        .into_iter()
        .filter(|t| !t.question.trim().is_empty())
        .enumerate()
        .map(|(i, t)| Thread {
            id: Register::thread_id(chapter, i + 1),
            question: t.question.trim().to_owned(),
            quote: t.quote.trim().to_owned(),
        })
        .collect();
    let facts = reply
        .facts
        .into_iter()
        .filter(|f| !f.fact.trim().is_empty())
        .enumerate()
        .map(|(i, f)| Fact {
            id: Register::fact_id(chapter, i + 1),
            fact: f.fact.trim().to_owned(),
            stated_by: match f.stated_by.trim() {
                "" => "narration".to_owned(),
                who => who.to_owned(),
            },
            quote: f.quote.trim().to_owned(),
        })
        .collect();
    Register {
        opened,
        paid,
        facts,
    }
}

/// The story's register before a chapter: the threads still open and every
/// fact stated, each with the chapter it came from, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoFar {
    /// The threads opened and not yet paid off.
    pub open: Vec<(u32, Thread)>,
    /// The facts stated.
    pub facts: Vec<(u32, Fact)>,
}

impl SoFar {
    /// Chapter `chapter`'s register added: what it pays off closed, what it
    /// opens and states appended.
    pub fn add(&mut self, chapter: u32, register: &Register) {
        self.open
            .retain(|(_, t)| !register.paid.iter().any(|p| p.thread == t.id));
        self.open
            .extend(register.opened.iter().map(|t| (chapter, t.clone())));
        self.facts
            .extend(register.facts.iter().map(|f| (chapter, f.clone())));
    }

    /// The ids of the open threads.
    pub fn open_ids(&self) -> Vec<String> {
        self.open.iter().map(|(_, t)| t.id.clone()).collect()
    }

    /// The register as the Author and the checker read it; empty when the
    /// published chapters noted nothing.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if !self.open.is_empty() {
            let lines: Vec<String> = self
                .open
                .iter()
                .map(|(n, t)| format!("`{}` (chapter {n}): {} (\"{}\")", t.id, t.question, t.quote))
                .collect();
            text.push_str(&format!(
                "\n\nThreads the published chapters left open, each a promise to the reader:\n{}",
                bullets(lines.iter().map(String::as_str))
            ));
        }
        if !self.facts.is_empty() {
            let lines: Vec<String> = self
                .facts
                .iter()
                .map(|(n, f)| {
                    format!(
                        "`{}` (chapter {n}, stated by {}): {} (\"{}\")",
                        f.id, f.stated_by, f.fact, f.quote
                    )
                })
                .collect();
            text.push_str(&format!(
                "\n\nWhat the published chapters state of time, age and kinship:\n{}",
                bullets(lines.iter().map(String::as_str))
            ));
        }
        text
    }
}

/// The keeper's prompt for chapter `chapter`'s register: the register so
/// far and the version's text, nothing else.
pub fn register_prompt(so_far: &SoFar, chapter: u32, text: &str, human_language: &str) -> Prompt {
    let before = match so_far.text() {
        t if t.is_empty() => "\n\nThe published chapters left no thread open.".to_owned(),
        t => t,
    };
    Prompt {
        system: format!(
            "You keep the register of a story published a chapter at a time. You know only its \
             published text: no plan, no notes, nothing of how it was made. From each new \
             chapter you note three things.\n\
             - The threads it opens: what the text leaves its reader to wonder, and so promises \
             to answer. A clue, a sound or a sight with no source, an act left unexplained, a \
             question a character leaves hanging, a promise made. Not the story's whole premise, \
             and not what the chapter answers before it ends; an excuse or a half-truth a character \
             offers, which the text leaves in doubt, answers nothing.\n\
             - The open threads it pays off: answers, or settles for good.\n\
             - The facts of time, age and kinship it states: ages, births, deaths, how long ago \
             something happened, how long something lasted, the order of past events, who is \
             whose kin. Each plainly, with its numbers, and who states it: the narration, or the \
             character who says it.\n\
             Write what you note in {human_language}."
        ),
        user: format!(
            "{}\n\nChapter {chapter}:\n\n{text}\n\nNote the open threads it pays off, the \
             threads it opens, and the facts of time, age and kinship it states.",
            before.trim_start()
        ),
    }
}

/// The summary's prompt: the version's text alone, and on the second
/// attempt the first summary with what its check found.
pub fn summary_prompt(
    human_language: &str,
    text: &str,
    again: Option<(&str, &[SummaryFault])>,
) -> Prompt {
    let ask = format!(
        "Summarize it in {human_language}, in three to six sentences: what happens, and who \
         learns what. Reply with the summary and nothing else."
    );
    let user = match again {
        None => format!("The chapter:\n\n{text}\n\n{ask}"),
        Some((first, faults)) => {
            let lines: Vec<String> = faults
                .iter()
                .map(|f| format!("\"{}\": {}", f.sentence.trim(), f.text_says.trim()))
                .collect();
            format!(
                "The chapter:\n\n{text}\n\nA first summary of it:\n\n{first}\n\nWhere that \
                 summary says what the text does not bear out:\n{}\n\nWrite the summary again, \
                 true to the text. {ask}",
                bullets(lines.iter().map(String::as_str))
            )
        }
    };
    Prompt {
        system: "You summarize a chapter of a story for its human author. You know only the \
                 chapter's text: no plan, no notes, nothing of how it was made. Say only what the \
                 text says; where it leaves something unclear or unseen, so does the summary."
            .to_owned(),
        user,
    }
}

/// The check of `summary` against the chapter's text.
pub fn summary_check_prompt(text: &str, summary: &str) -> Prompt {
    Prompt {
        system: "You check a summary of a chapter of a story against the chapter's text. The \
                 text is the truth. A summary may leave out and compress; it may not say what the \
                 text does not, contradict it, or contradict itself."
            .to_owned(),
        user: format!(
            "The chapter:\n\n{text}\n\nIts summary:\n\n{summary}\n\nList each sentence of the \
             summary that the text does not bear out."
        ),
    }
}

/// The version of chapter `n` the human accepted, the latest if several.
pub async fn accepted_version(
    db: &StoryDb,
    n: u32,
) -> Result<Option<ChapterVersion>, StoryDbError> {
    for version in db.chapter_versions(n).await?.into_iter().rev() {
        if db.accepted(&version.id).await? {
            return Ok(Some(version));
        }
    }
    Ok(None)
}

/// The register before chapter `before`, read from the accepted chapters'
/// registers; a chapter without one adds nothing.
pub async fn so_far(db: &StoryDb, before: u32) -> Result<SoFar, StoryDbError> {
    let mut so_far = SoFar::default();
    for k in 1..before {
        if let Some(version) = accepted_version(db, k).await?
            && let Some(register) = db.register(&version.id).await?
        {
            so_far.add(k, &register);
        }
    }
    Ok(so_far)
}

/// A thread the story leaves open at its last checkpoint, with the Author's
/// reason where its note was decided against (docs/fabula.md §6, T16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOpen {
    /// What the reader is left to wonder.
    pub question: String,
    /// Why the Author left it, in its own words.
    pub reason: Option<String>,
}

/// The threads still open after `version` of chapter `n`: those the
/// chapters before it left, less what it pays off, with what it opens. A
/// reason is the Author's last word in this take on the thread's note.
pub async fn left_open(
    db: &StoryDb,
    n: u32,
    version: &ChapterVersion,
) -> Result<Vec<LeftOpen>, StoryDbError> {
    let mut open = so_far(db, n).await?;
    if let Some(register) = db.register(&version.id).await? {
        open.add(n, &register);
    }
    let first = db.take_of(n).await?.map_or(1, |t| t.first_version);
    let mut reasons: BTreeMap<String, Option<String>> = BTreeMap::new();
    for v in db.chapter_versions(n).await? {
        if v.version < first || v.version > version.version {
            continue;
        }
        let decisions = db.decisions(&v.id).await?;
        for note in db.notes(&v.id).await? {
            let (NoteKind::OpenThread, Some(thread)) = (note.note.kind, &note.note.anchor) else {
                continue;
            };
            let Some(decided) = decisions.iter().find(|d| d.note == note.id) else {
                continue;
            };
            let reason = (decided.decision != NoteDecision::Accepted)
                .then(|| decided.reason.trim().to_owned())
                .filter(|r| !r.is_empty());
            reasons.insert(thread.clone(), reason);
        }
    }
    Ok(open
        .open
        .into_iter()
        .map(|(_, t)| LeftOpen {
            reason: reasons.get(&t.id).cloned().flatten(),
            question: t.question,
        })
        .collect())
}

#[cfg(test)]
#[path = "keeper_tests.rs"]
mod tests;
