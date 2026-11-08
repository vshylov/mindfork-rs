//! B0 as the story tools run it (docs/history/fabula.md §4, *After M3*; §6,
//! T25–T27): a story from where it stands to its next pause.
//!
//! 1. **P1**: an unapproved canon asks for its approval and stops the run.
//! 2. **A chapter is a session of B0's** ([`super::single`]), its own: given
//!    the canon, its goal in the arc, the accepted chapters' text whole and
//!    what the person asked of it (T27), it writes a draft, then a critique
//!    and a revision at a time, at most [`MOST_REVISIONS`] revisions, within
//!    the chapter's budget and the model's window (T25).
//! 3. **P2** asks about the chapter's last whole version. Accepted, the next
//!    chapter opens with the notes left on it. Asked to change, the version
//!    is revised in one turn — the chapter's ask, the version as the writer's
//!    reply, the person's notes — and P2 asks again (T26). No scene is played
//!    again and nothing asks P3: B0 has no scenes.
//!
//! **The stores are the state**, as for F's loop (spec §9): a session is
//! rebuilt from its calls, a person's revision is a version of its own, and a
//! run killed anywhere resumes at the first turn no run finished.

use super::author_prompts::words_of;
use super::engine::SharedLog;
use super::port::{CallAddress, LlmPort, PortError, Role, Turn};
use super::single::{self, Ask, Ended, Session, SingleConfig};
use crate::entities::story::{
    Canon, ChapterVersion, Decision, Question, QuestionBody, Resolution, RunId, VersionOrigin,
};
use crate::shared::storage::story::{AuthorWriter, Spend, StoryDb, StoryDbError};

/// A chapter's spending limits (docs/history/fabula.md §6, T1 (a)); none is no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Budget {
    /// Prompt and reply tokens, every provider.
    pub tokens: Option<u64>,
    /// Dollars, where the provider reports them.
    pub usd: Option<f64>,
}

impl Budget {
    /// What `spend` has spent of this budget, in words, once it is spent;
    /// `None` while there is room — the check before every turn, and
    /// `story_continue`'s before a run (docs/history/fabula.md §4, PR 9).
    pub fn spent(&self, spend: &Spend) -> Option<String> {
        if let Some(limit) = self.tokens
            && spend.tokens() >= limit
        {
            return Some(format!("{} tokens of {limit}", spend.tokens()));
        }
        if let (Some(limit), Some(cost)) = (self.usd, spend.cost_usd)
            && cost >= limit
        {
            return Some(format!("${cost:.4} of ${limit:.2}"));
        }
        None
    }
}

/// Why a run stopped short of a pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// It was cancelled.
    Cancelled,
    /// The chapter's budget was spent: what was spent, in words.
    Budget(String),
    /// A call gave no usable answer.
    Failed(String),
}

/// Where a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// At a pause: the question waits for its answer.
    Paused(Question),
    /// Every chapter of the arc is accepted.
    Finished,
    /// Short of a pause, at a step a resume starts from.
    Stopped {
        /// The step.
        step: String,
        /// Why.
        why: Stop,
    },
}

/// Why a run could not go at all.
#[derive(Debug, thiserror::Error)]
pub enum StoryError {
    /// No canon was imported yet.
    #[error("the story has no canon yet")]
    NoCanon,
    /// The story database refused or failed.
    #[error(transparent)]
    Store(#[from] StoryDbError),
}

/// The latest accepted version of chapter `n`, if one is.
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

/// The most revisions a chapter's session writes after its draft: the B0 the
/// judges read wrote 25 versions on the mean (docs/history/fabula.md §6, T25).
pub const MOST_REVISIONS: u32 = 24;

/// What a run works with: the story, the Author's capability, the run log
/// shared with the port, the port and the session's configuration.
pub struct Story<'a> {
    /// The story.
    pub db: &'a StoryDb,
    /// The Author's capability.
    pub author: &'a mut AuthorWriter,
    /// The run log, shared with the port that records the calls.
    pub log: &'a SharedLog,
    /// The model.
    pub port: &'a dyn LlmPort,
    /// How each chapter's session runs.
    pub config: &'a SingleConfig,
}

/// Runs the story from where it stands to its next pause, to its end, or to
/// a stop; a pending question returns at once and starts no run.
pub async fn advance(story: Story<'_>) -> Result<Outcome, StoryError> {
    let Story {
        db,
        author,
        log,
        port,
        config,
    } = story;
    if let Some(question) = db.pending_question().await? {
        return Ok(Outcome::Paused(question));
    }
    let canon = db.canon().await?.ok_or(StoryError::NoCanon)?;
    let run = log.lock().await.start_run(&config.to_json()).await?;
    if !db.canon_approved().await? {
        let question = author
            .add_question(&run, "canon", &QuestionBody::Canon)
            .await?;
        return Ok(Outcome::Paused(question));
    }
    let of = u32::try_from(canon.arc.chapters.len()).unwrap_or(u32::MAX);
    let mut earlier = Vec::new();
    let mut notes = None;
    for n in 1..=of {
        if let Some(accepted) = accepted_version(db, n).await? {
            notes = checkpoint_of(db, n, accepted.version)
                .await?
                .and_then(|(_, answer)| answer)
                .map(|answer| answer.notes)
                .filter(|notes| !notes.trim().is_empty());
            earlier.push(accepted.text);
            continue;
        }
        let ask = Ask::chapter(n, of, earlier, notes);
        let mut writing = Writing {
            db,
            author,
            run,
            port,
            config,
            canon,
        };
        return Ok(writing.chapter(&ask).await?);
    }
    Ok(Outcome::Finished)
}

/// The P2 question about version `version` of chapter `n`, with its answer.
async fn checkpoint_of(
    db: &StoryDb,
    n: u32,
    version: u32,
) -> Result<Option<(Question, Option<Resolution>)>, StoryDbError> {
    let body = QuestionBody::Chapter {
        chapter: n,
        version,
    };
    Ok(db
        .questions()
        .await?
        .into_iter()
        .find(|(q, _)| q.body == body))
}

/// One chapter's run.
struct Writing<'a> {
    db: &'a StoryDb,
    author: &'a mut AuthorWriter,
    run: RunId,
    port: &'a dyn LlmPort,
    config: &'a SingleConfig,
    canon: Canon,
}

impl Writing<'_> {
    /// Chapter `ask.n` to its checkpoint: the person's answer acted on, or
    /// the session gone on with.
    async fn chapter(&mut self, ask: &Ask) -> Result<Outcome, StoryDbError> {
        if let Some(latest) = self.db.chapter_versions(ask.n).await?.pop() {
            match checkpoint_of(self.db, ask.n, latest.version).await? {
                Some((question, None)) => return Ok(Outcome::Paused(question)),
                Some((_, Some(answer))) if answer.decision == Decision::Approve => {
                    return Err(StoryDbError::Corrupt(format!(
                        "chapter {} is approved at version {} but not accepted",
                        ask.n, latest.version
                    )));
                }
                Some((_, Some(answer))) => {
                    return match self.revise(ask, &latest, &answer.notes).await? {
                        Ok(version) => self.checkpoint(ask.n, &version).await,
                        Err(stopped) => Ok(stopped),
                    };
                }
                None if latest.origin == VersionOrigin::Human => {
                    return self.checkpoint(ask.n, &latest).await;
                }
                None => {}
            }
        }
        let written = single::write_session(
            Session {
                db: self.db,
                author: self.author,
                run: &self.run,
                port: self.port,
                config: self.config,
            },
            &self.canon,
            ask,
        )
        .await?;
        let why = match written.ended {
            Ended::Failed(step, why) => Some((step, Stop::Failed(why))),
            Ended::Cancelled(step) => Some((step, Stop::Cancelled)),
            Ended::Spent | Ended::Budget | Ended::Window | Ended::Revisions => None,
        };
        if let Some((step, why)) = why {
            return Ok(Outcome::Stopped { step, why });
        }
        match self.db.chapter_versions(ask.n).await?.pop() {
            Some(latest) => self.checkpoint(ask.n, &latest).await,
            None => Ok(Outcome::Stopped {
                step: ask.address(1),
                why: match self
                    .config
                    .budget
                    .spent(&self.db.chapter_spend(ask.n).await?)
                {
                    Some(spent) => Stop::Budget(spent),
                    None => {
                        Stop::Failed(format!("no version of chapter {} was written whole", ask.n))
                    }
                },
            }),
        }
    }

    /// The version after `latest`, written with the person's `notes` (T26),
    /// or why the run stopped short of it.
    async fn revise(
        &mut self,
        ask: &Ask,
        latest: &ChapterVersion,
        notes: &str,
    ) -> Result<Result<ChapterVersion, Outcome>, StoryDbError> {
        let address = format!("{}h{}", ask.prefix, latest.version + 1);
        let words = self.config.words;
        let turns = [
            Turn {
                role: "user",
                content: ask.draft(&self.canon, words),
            },
            Turn {
                role: "assistant",
                content: latest.text.clone(),
            },
            Turn {
                role: "user",
                content: ask.notes_ask(words, words_of(&latest.text), notes),
            },
        ];
        let call = CallAddress {
            run: self.run.clone(),
            address: address.clone(),
        };
        let stopped = |why: Stop| {
            Ok(Err(Outcome::Stopped {
                step: address.clone(),
                why,
            }))
        };
        let text = match self
            .port
            .session(&call, Role::Author, &ask.system(&self.canon), &turns)
            .await
        {
            Ok(text) if !single::broken_off(&text) => text,
            Ok(_) => {
                return stopped(Stop::Failed(
                    "the revision broke off mid-sentence".to_owned(),
                ));
            }
            Err(PortError::Store(e)) => return Err(e),
            Err(PortError::Cancelled { .. }) => return stopped(Stop::Cancelled),
            Err(PortError::Budget { spent, .. }) => return stopped(Stop::Budget(spent)),
            Err(e) => return stopped(Stop::Failed(e.to_string())),
        };
        Ok(Ok(self
            .author
            .add_chapter(ask.n, VersionOrigin::Human, &text)
            .await?))
    }

    /// P2 about `version` of chapter `n`: the run ends with it.
    async fn checkpoint(
        &mut self,
        n: u32,
        version: &ChapterVersion,
    ) -> Result<Outcome, StoryDbError> {
        let body = QuestionBody::Chapter {
            chapter: n,
            version: version.version,
        };
        let question = self
            .author
            .add_question(&self.run, &format!("{}/checkpoint", version.id), &body)
            .await?;
        Ok(Outcome::Paused(question))
    }
}

#[cfg(test)]
#[path = "single_story_tests.rs"]
mod tests;
