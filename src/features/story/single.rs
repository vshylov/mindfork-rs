//! M3's B0 (docs/research/fabula-engine.md §11; docs/fabula.md §6, T21):
//! one agent, one session. It is given the canon as the Author is, and the
//! same rules of the prose. It writes the story, then reads it as an editor
//! and writes it again, each turn seeing the whole session, until it has
//! spent what F spent on its premise (the equal cost of spec §11), or the
//! next turn would not fit the model's window. The story is the last
//! version written that does not break off mid-sentence.
//!
//! The story tools write each chapter as such a session (docs/fabula.md §6,
//! T25–T27; [`super::single_story`]): an [`Ask`] names the chapter, its place
//! in the arc, the accepted chapters before it and the person's notes for it.
//! A story of one chapter is asked what M3 asked, word for word.
//!
//! - **The stores are the state**, as for the chapter's loop (spec §9): each
//!   version written is a chapter version, every reply is in the call log
//!   under its turn's address (`b0/t1`, `b0/t2`, … for M3; `ch2/b0/t1`, … for
//!   a chapter of the tools'), and a run killed anywhere rebuilds the session
//!   from them and goes on at the first turn no run finished.
//! - **One role.** Every turn is the Author's, with the Author's sampling.

use std::collections::BTreeMap;

use serde_json::json;

use super::author_prompts;
use super::engine::SharedLog;
use super::port::{CallAddress, LlmPort, PortError, Role, Turn};
use super::single_story::Budget;
use crate::entities::story::{Canon, RunId, VersionOrigin};
use crate::shared::storage::story::{AuthorWriter, CallStatus, Spend, StoryDb, StoryDbError};

/// What the session spends to: F's average on the same premise. Dollars
/// where both sides report them, tokens otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Target {
    /// Dollars, where the provider reports them.
    pub usd: Option<f64>,
    /// Prompt and reply tokens.
    pub tokens: u64,
}

impl Target {
    /// Whether `spent` reaches the target: in dollars when both sides have
    /// them, in tokens otherwise.
    pub fn reached(&self, spent: &Spend) -> bool {
        match (self.usd, spent.cost_usd) {
            (Some(usd), Some(cost)) => cost >= usd,
            _ => spent.tokens() >= self.tokens,
        }
    }
}

/// How the session runs.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleConfig {
    /// The story's length, in words, as asked.
    pub words: (u32, u32),
    /// What it spends to: M3's equal cost. None for the story tools, whose
    /// session ends on its revisions (T25).
    pub target: Option<Target>,
    /// The chapter's budget, checked before every turn against every call of
    /// the chapter (`ch{n}/`), as `story_continue` checks it: a spent one ends
    /// the session at the last version written. None for M3.
    pub budget: Budget,
    /// The model's window, in tokens: the session stops before a turn that
    /// would not fit.
    pub window: u64,
    /// The most versions after the draft: M3's backstop, should the spend
    /// never be measured; the story tools' end (T25).
    pub most_revisions: u32,
}

impl Default for SingleConfig {
    fn default() -> Self {
        Self {
            words: (1200, 2500),
            target: None,
            budget: Budget::default(),
            window: 131_072,
            most_revisions: 40,
        }
    }
}

impl SingleConfig {
    /// The configuration as a run records it.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "engine": "single",
            "words": [self.words.0, self.words.1],
            "target": self.target.map(|t| json!({"usd": t.usd, "tokens": t.tokens})),
            "budget": {"tokens": self.budget.tokens, "usd": self.budget.usd},
            "window": self.window,
            "most_revisions": self.most_revisions,
        })
    }
}

/// Why the session stopped where it did.
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    /// It spent its target.
    Spent,
    /// The chapter's budget was spent.
    Budget,
    /// The next turn would not have fit the window.
    Window,
    /// It wrote its most revisions.
    Revisions,
    /// A call was cancelled, at its address: a stop, a quit, a time limit.
    Cancelled(String),
    /// A call gave no usable answer, at its address.
    Failed(String, String),
}

/// What the session left.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// Why it stopped.
    pub ended: Ended,
    /// The versions written: the draft and every revision.
    pub versions: u32,
    /// What the whole session cost.
    pub spent: Spend,
}

/// What a session writes: M3's story of one chapter, or chapter `n` of a
/// story the chat's tools write (docs/fabula.md §6, T27).
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    /// The chapter, from 1.
    pub n: u32,
    /// The chapters the arc plans.
    pub of: u32,
    /// The text of each accepted chapter before it, in order.
    pub earlier: Vec<String>,
    /// What the person asked of it at the checkpoint before.
    pub notes: Option<String>,
    /// What each of the session's addresses starts with.
    pub prefix: String,
}

impl Ask {
    /// M3's: the whole story, one chapter, at `b0/`.
    pub fn story() -> Self {
        Self {
            n: 1,
            of: 1,
            earlier: Vec::new(),
            notes: None,
            prefix: "b0/".to_owned(),
        }
    }

    /// Chapter `n` of `of`, at `ch{n}/b0/`, where its chapter's budget counts
    /// every call.
    pub fn chapter(n: u32, of: u32, earlier: Vec<String>, notes: Option<String>) -> Self {
        Self {
            n,
            of,
            earlier,
            notes,
            prefix: format!("ch{n}/b0/"),
        }
    }

    /// Whether the arc is one chapter: asked as M3 asked.
    fn one(&self) -> bool {
        self.of <= 1
    }

    fn noun(&self) -> &'static str {
        if self.one() { "story" } else { "chapter" }
    }

    /// The session's system prompt.
    pub fn system(&self, canon: &Canon) -> String {
        if self.one() {
            return system(canon);
        }
        format!(
            "You are a writer. You write a story of {} chapters, one chapter at a time, from the \
             canon below: its world, its places, its cast with what each of them wants and \
             hides, how it is told, its premise and its arc. Each chapter you plan, write, read \
             as its editor and write again.\n\n{}",
            self.of,
            author_prompts::canon_brief(canon)
        )
    }

    /// The first turn: the chapter asked for, after the accepted ones, with
    /// what the person asked of it.
    pub fn draft(&self, canon: &Canon, words: (u32, u32)) -> String {
        if self.one() {
            return draft_ask(canon, words);
        }
        let goal = canon
            .arc
            .chapters
            .get(self.n.saturating_sub(1) as usize)
            .map_or("none set", |c| c.goal.as_str());
        let mut ask = format!(
            "Write chapter {} of {}. What it must achieve: {goal}\n\n",
            self.n, self.of
        );
        if !self.earlier.is_empty() {
            ask.push_str("The chapters before it, as the reader accepted them:\n\n");
            for text in &self.earlier {
                ask.push_str(text.trim());
                ask.push_str("\n\n");
            }
        }
        if let Some(notes) = self
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            ask.push_str(&format!(
                "What the reader asked of this chapter: {notes}\n\n"
            ));
        }
        ask.push_str(&format!(
            "How to write it: {}",
            author_prompts::writing_rules(canon, words)
        ));
        ask
    }

    /// The editor's turn.
    pub fn critique(&self) -> &'static str {
        if self.one() {
            CRITIQUE_ASK
        } else {
            CHAPTER_CRITIQUE_ASK
        }
    }

    /// The writer's turn after a critique.
    pub fn revise(&self, words: (u32, u32), last: u32) -> String {
        again(self.noun(), "mending what your critique found", words, last)
    }

    /// The turn that revises a version the person read, with their notes
    /// (T26): it follows the chapter's ask and the version, not the session.
    pub fn notes_ask(&self, words: (u32, u32), last: u32, notes: &str) -> String {
        format!(
            "The reader asks for these changes:\n\n{}\n\n{}",
            notes.trim(),
            again(self.noun(), "making them", words, last)
        )
    }

    /// The address of turn `k`, from 1: odd turns write, even ones critique.
    pub fn address(&self, k: u32) -> String {
        format!("{}t{k}", self.prefix)
    }
}

/// The session's system prompt: a writer, the canon as the Author reads it.
pub fn system(canon: &Canon) -> String {
    format!(
        "You are a writer. You write a short story, one chapter long, from the canon below: \
         its world, its places, its cast with what each of them wants and hides, how it is \
         told, and its premise. You plan it, write it, read it as its editor and write it \
         again.\n\n{}",
        author_prompts::canon_brief(canon)
    )
}

/// The first turn: the story asked for, under the prose's rules.
pub fn draft_ask(canon: &Canon, words: (u32, u32)) -> String {
    let goal = canon
        .arc
        .chapters
        .first()
        .map_or("none set", |c| c.goal.as_str());
    format!(
        "Write the story. What it must achieve: {goal}\n\nHow to write it: {}",
        author_prompts::writing_rules(canon, words)
    )
}

/// The editor's turn: what a reviewer of F would look for (spec §7's Reader
/// and Continuity checker, the craft's rules of PR 12, the staging pass's
/// names and languages and the engine's length of docs/fabula.md §4, *The last
/// version checked*), asked of the writer.
pub const CRITIQUE_ASK: &str = "Now read your story as its editor would, without rewriting \
it yet. Name each thing that does not work, and where: a conflict that goes slack or ends \
without a price, a secret spent too soon or too cheaply, a character who acts against their \
sheet or knows what they could not have learned, a line that announces a plan or a want \
instead of acting on it, or opens on thanks or agreement the moment does not call for, facts \
of time, place, age or kinship that disagree, a person named or addressed two ways, a word \
left in another language, a thread opened and dropped, a passage that drags or tells what it \
should show, a story shorter or longer than asked. Most important first.";

/// The editor's turn on a chapter of a longer arc: M3's, read against the
/// chapters before it and the ones the arc keeps for later.
pub const CHAPTER_CRITIQUE_ASK: &str = "Now read your chapter as its editor would, without \
rewriting it yet. Name each thing that does not work, and where: a conflict that goes slack or \
ends without a price, a secret spent too soon or too cheaply, a character who acts against their \
sheet or knows what they could not have learned, a line that announces a plan or a want instead \
of acting on it, or opens on thanks or agreement the moment does not call for, facts of time, \
place, age or kinship that disagree with each other or with the chapters before, a person named \
or addressed two ways, a word left in another language, a thread opened and dropped, a passage \
that drags or tells what it should show, a chapter shorter or longer than asked, a chapter that \
spends what the arc keeps for a later one. Most important first.";

/// The writer's turn after a critique: told its last version's words and
/// keeping the names, as F's reviser is (F2, F4, T21).
pub fn revise_ask(words: (u32, u32), last: u32) -> String {
    again("story", "mending what your critique found", words, last)
}

/// A writing turn after the first: the whole text again, `mending` what was
/// found, by the same rules.
fn again(noun: &str, mending: &str, words: (u32, u32), last: u32) -> String {
    format!(
        "Write the {noun} again, whole, {mending}: by the same rules, between {} and {} words \
         (your last version has {last}), its title as a level-two markdown heading. {} Reply \
         with the {noun}'s text and nothing else.",
        words.0,
        words.1,
        author_prompts::KEEP_NAMES,
    )
}

/// The words of the version turn `n` revises, the writing turn before its
/// critique, as F's reviser is told them (F2).
fn last_words(replies: &BTreeMap<u32, (String, u64)>, n: u32) -> u32 {
    replies
        .get(&n.saturating_sub(2))
        .map_or(0, |(text, _)| author_prompts::words_of(text))
}

/// Whether a version breaks off mid-sentence. A provider may end a reply it
/// cut as if it had finished: PR 13's pilot met one on Qwen 3.8 through
/// DeepInfra, a revision of 17,493 tokens ending in the middle of a word.
/// Such a version stays in the session, for the next critique to read, and
/// is not kept as the story's: B0's last version is read by no one before
/// the judges, as F's is by its reviewers.
pub(super) fn broken_off(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .is_none_or(|c| c.is_alphanumeric() || matches!(c, ',' | ';' | ':' | '-'))
}

/// What the session at `prefix` holds so far: each finished turn's reply, by
/// number, read from every run's calls.
async fn replies(db: &StoryDb, prefix: &str) -> Result<BTreeMap<u32, (String, u64)>, StoryDbError> {
    let turn = format!("{prefix}t");
    let mut out = BTreeMap::new();
    for run in db.runs().await? {
        for c in db.calls(&run).await? {
            let Some(n) = c
                .call
                .address
                .strip_prefix(turn.as_str())
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let text = c
                .call
                .response
                .as_ref()
                .and_then(|r| r["text"].as_str())
                .unwrap_or_default();
            if c.call.status == CallStatus::Ok && !text.trim().is_empty() {
                let size = c.call.usage.input_tokens.unwrap_or(0)
                    + c.call.usage.output_tokens.unwrap_or(0);
                out.insert(n, (text.to_owned(), size));
            }
        }
    }
    Ok(out)
}

/// The session's turns up to and including turn `last`'s reply.
fn session(
    ask: &Ask,
    canon: &Canon,
    words: (u32, u32),
    replies: &BTreeMap<u32, (String, u64)>,
) -> Vec<Turn> {
    let mut turns = Vec::new();
    for (n, (text, _)) in replies {
        let content = match n {
            1 => ask.draft(canon, words),
            n if n % 2 == 0 => ask.critique().to_owned(),
            n => ask.revise(words, last_words(replies, *n)),
        };
        turns.push(Turn {
            role: "user",
            content,
        });
        turns.push(Turn {
            role: "assistant",
            content: text.clone(),
        });
    }
    turns
}

/// Whether the next critique and revision would not fit the window: the
/// last call's whole size, the critique, and the revision twice over, since
/// the revision's prompt holds the last version and its reply writes
/// another.
fn would_overflow(window: u64, last_size: u64, last_reply: &str) -> bool {
    // About four characters a token in English, fewer in Russian: three, to
    // err on the side of fitting.
    let reply_tokens = last_reply.chars().count() as u64 / 3;
    let critique = 2_048;
    last_size + critique + 2 * reply_tokens + 1_024 > window
}

/// Runs M3's session from where its stores leave it to its end.
pub async fn write(
    db: &StoryDb,
    author: &mut AuthorWriter,
    log: &SharedLog,
    port: &dyn LlmPort,
    config: &SingleConfig,
) -> Result<Written, StoryDbError> {
    let canon = db
        .canon()
        .await?
        .ok_or_else(|| StoryDbError::Corrupt("the story has no canon".to_owned()))?;
    let run = log.lock().await.start_run(&config.to_json()).await?;
    write_session(
        Session {
            db,
            author,
            run: &run,
            port,
            config,
        },
        &canon,
        &Ask::story(),
    )
    .await
}

/// What a session works with: the story, the Author's capability, the run it
/// records its calls under, the model and the configuration.
pub struct Session<'a> {
    /// The story.
    pub db: &'a StoryDb,
    /// The Author's capability.
    pub author: &'a mut AuthorWriter,
    /// The run.
    pub run: &'a RunId,
    /// The model.
    pub port: &'a dyn LlmPort,
    /// How it runs.
    pub config: &'a SingleConfig,
}

/// Runs the session `ask` names from where its stores leave it to its end.
pub async fn write_session(
    session_of: Session<'_>,
    canon: &Canon,
    ask: &Ask,
) -> Result<Written, StoryDbError> {
    let Session {
        db,
        author,
        run,
        port,
        config,
    } = session_of;
    let system = ask.system(canon);
    // A turn is asked only after the one before it answered, so the replies
    // recorded run from 1 without a gap.
    let mut done = replies(db, &ask.prefix).await?;
    let writing = |n: u32| n % 2 == 1;
    let kept = |n: u32, text: &str| writing(n) && !broken_off(text);
    // A version written whose row was not stored before a kill.
    let stored = db.chapter_versions(ask.n).await?.len() as u32;
    let written = done.iter().filter(|(n, (t, _))| kept(**n, t)).count() as u32;
    for (n, (text, _)) in done
        .iter()
        .filter(|(n, (t, _))| kept(**n, t))
        .skip(stored as usize)
    {
        let origin = if *n == 1 {
            VersionOrigin::Draft
        } else {
            VersionOrigin::Revision
        };
        author.add_chapter(ask.n, origin, text).await?;
    }
    let mut versions = stored.max(written);
    let ended = loop {
        let next = done.keys().next_back().map_or(1, |n| n + 1);
        if config
            .budget
            .spent(&db.chapter_spend(ask.n).await?)
            .is_some()
        {
            break Ended::Budget;
        }
        let spent = db.spend(&ask.prefix).await?;
        if writing(next) && next > 1 {
            // A revision follows a critique; nothing to decide before it.
        } else if next > 1 {
            // Before a critique: the target, the revisions, the window.
            if config.target.is_some_and(|t| t.reached(&spent)) {
                break Ended::Spent;
            }
            if versions > config.most_revisions {
                break Ended::Revisions;
            }
            let (last, size) = done.values().next_back().expect("a turn is done");
            if would_overflow(config.window, *size, last) {
                break Ended::Window;
            }
        }
        let content = match next {
            1 => ask.draft(canon, config.words),
            n if writing(n) => ask.revise(config.words, last_words(&done, n)),
            _ => ask.critique().to_owned(),
        };
        let mut turns = session(ask, canon, config.words, &done);
        turns.push(Turn {
            role: "user",
            content,
        });
        let call = CallAddress {
            run: run.clone(),
            address: ask.address(next),
        };
        let reply = match port.session(&call, Role::Author, &system, &turns).await {
            Ok(text) => text,
            Err(PortError::Store(e)) => return Err(e),
            Err(PortError::Cancelled { .. }) => break Ended::Cancelled(ask.address(next)),
            Err(e) => break Ended::Failed(ask.address(next), e.to_string()),
        };
        if kept(next, &reply) {
            let origin = if next == 1 {
                VersionOrigin::Draft
            } else {
                VersionOrigin::Revision
            };
            author.add_chapter(ask.n, origin, &reply).await?;
            versions += 1;
        }
        // The turn's size as the call log has it, for the window's check.
        done = replies(db, &ask.prefix).await?;
        done.retain(|n, _| *n <= next);
        done.entry(next).or_insert((reply, 0));
    };
    Ok(Written {
        ended,
        versions,
        spent: db.spend(&ask.prefix).await?,
    })
}

#[cfg(test)]
#[path = "single_tests.rs"]
pub(crate) mod tests;
