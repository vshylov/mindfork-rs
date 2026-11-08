//! A story's state for `story_status` (docs/history/fabula.md §4, PR 8;
//! docs/research/fabula-engine.md §14, *Tools*): where the story stands, read
//! while a run may be writing it.
//!
//! The files are opened read-only through [`Actor::read_only`], never through
//! [`super::StoryDb`]: a story has one actor at a time, and while a run holds
//! it a second open would be refused. The reads are the actor's own, so the
//! status and the run cannot read a row differently.

use std::path::Path;

use chrono::{DateTime, Utc};

use super::Spend;
use super::StoryDbError;
use super::actor::Actor;
use crate::entities::story::{Question, QuestionBody};

/// One chapter as far as it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterState {
    /// Its number, from 1.
    pub number: u32,
    /// How many versions it has: the draft, the revisions.
    pub versions: u32,
    /// Whether the human accepted a version of it at P2.
    pub accepted: bool,
}

/// Where a story stands.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryStatus {
    /// What the story is about, from its arc; `None` while it has no canon.
    pub premise: Option<String>,
    /// How many chapters its arc plans.
    pub planned: u32,
    /// Whether the canon was approved at P1.
    pub canon_approved: bool,
    /// The chapters written so far, in order.
    pub chapters: Vec<ChapterState>,
    /// The question waiting for the human's answer, if one is.
    pub pending: Option<Question>,
    /// Since when the pending question stands as the human sees it: when it
    /// was asked, or, for the canon's, when the canon last changed, if later
    /// (spec §14, *The answer is the user's own words*). A reply before it
    /// answers something else.
    pub pending_since: Option<DateTime<Utc>>,
    /// The first chapter of the arc with no accepted version: the one a run
    /// works on; `None` once every planned chapter is accepted.
    pub open_chapter: Option<u32>,
    /// What the open chapter has cost so far, in every run of its latest take
    /// (docs/history/fabula.md §6, T1, T12); nothing when the call log is gone.
    pub spend: Spend,
}

/// The state of the story at `story`, its call log at `calls`.
pub fn story_status_of(story: &Path, calls: &Path) -> Result<StoryStatus, StoryDbError> {
    let mut actor = Actor::read_only(story, calls)?;
    let canon = actor.canon()?;
    let planned = canon.as_ref().map_or(0, |c| {
        u32::try_from(c.arc.chapters.len()).unwrap_or(u32::MAX)
    });
    let mut chapters = Vec::new();
    for number in 1.. {
        let versions = actor.chapter_versions(number)?;
        if versions.is_empty() {
            break;
        }
        let mut accepted = false;
        for v in &versions {
            accepted |= actor.accepted(&v.id)?;
        }
        chapters.push(ChapterState {
            number,
            versions: u32::try_from(versions.len()).unwrap_or(u32::MAX),
            accepted,
        });
    }
    let open_chapter =
        (1..=planned).find(|n| !chapters.iter().any(|c| c.number == *n && c.accepted));
    let spend = match open_chapter {
        Some(n) => actor.chapter_spend(n)?,
        None => Spend::default(),
    };
    let pending = actor
        .questions()?
        .into_iter()
        .find(|(_, answer)| answer.is_none())
        .map(|(q, _)| q);
    let pending_since = match &pending {
        Some(q) => asked_since(&actor, q)?,
        None => None,
    };
    Ok(StoryStatus {
        premise: canon.as_ref().map(|c| c.arc.premise.clone()),
        planned,
        canon_approved: actor.canon_approved()?,
        chapters,
        pending,
        pending_since,
        open_chapter,
        spend,
    })
}

/// Since when `question` stands: when it was asked, or, for the canon's,
/// when the canon last changed — a revision of the draft canon is a question
/// put again.
pub(super) fn asked_since(
    actor: &Actor,
    question: &Question,
) -> Result<Option<DateTime<Utc>>, StoryDbError> {
    let asked = actor.asked_at(&question.id)?;
    let changed = match question.body {
        QuestionBody::Canon => actor.canon_changed_at()?,
        _ => None,
    };
    Ok(asked.max(changed))
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
