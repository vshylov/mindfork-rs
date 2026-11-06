//! M3's B0 (docs/research/fabula-engine.md §11; docs/fabula.md §6, T21):
//! one agent, one session. It is given the canon as the Author is, and the
//! same rules of the prose. It writes the story, then reads it as an editor
//! and writes it again, each turn seeing the whole session, until it has
//! spent what F spent on its premise (the equal cost of spec §11), or the
//! next turn would not fit the model's window. The story is the last
//! version written that does not break off mid-sentence.
//!
//! - **The stores are the state**, as for the chapter's loop (spec §9): each
//!   version written is a chapter version, every reply is in the call log
//!   under its turn's address (`b0/t1`, `b0/t2`, …), and a run killed
//!   anywhere rebuilds the session from them and goes on at the first turn
//!   no run finished.
//! - **One role.** Every turn is the Author's, with the Author's sampling.

use std::collections::BTreeMap;

use serde_json::json;

use super::author_prompts::{self, Record};
use super::engine::SharedLog;
use super::port::{CallAddress, LlmPort, PortError, Role, Turn};
use crate::entities::story::{Canon, VersionOrigin};
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
    /// What it spends to.
    pub target: Target,
    /// The model's window, in tokens: the session stops before a turn that
    /// would not fit.
    pub window: u64,
    /// A backstop on the critiques, should the spend never be measured.
    pub most_revisions: u32,
}

impl Default for SingleConfig {
    fn default() -> Self {
        Self {
            words: (1200, 2500),
            target: Target::default(),
            window: 131_072,
            most_revisions: 40,
        }
    }
}

impl SingleConfig {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "engine": "single",
            "words": [self.words.0, self.words.1],
            "target": {"usd": self.target.usd, "tokens": self.target.tokens},
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
    /// The next turn would not have fit the window.
    Window,
    /// The backstop on revisions.
    Revisions,
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
        author_prompts::writing_rules(canon, words, Record::None)
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

/// The writer's turn after a critique; it keeps the names as F's reviser does
/// (F4, T21).
pub fn revise_ask(words: (u32, u32)) -> String {
    format!(
        "Write the story again, whole, mending what your critique found: by the same rules, \
         between {} and {} words, its title as a level-two markdown heading. {} Reply with \
         the story's text and nothing else.",
        words.0,
        words.1,
        author_prompts::KEEP_NAMES,
    )
}

/// Whether a version breaks off mid-sentence. A provider may end a reply it
/// cut as if it had finished: PR 13's pilot met one on Qwen 3.8 through
/// DeepInfra, a revision of 17,493 tokens ending in the middle of a word.
/// Such a version stays in the session, for the next critique to read, and
/// is not kept as the story's: B0's last version is read by no one before
/// the judges, as F's is by its reviewers.
fn broken_off(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .is_none_or(|c| c.is_alphanumeric() || matches!(c, ',' | ';' | ':' | '-'))
}

/// The address of turn `n`, from 1: odd turns write, even ones critique.
fn address(n: u32) -> String {
    format!("b0/t{n}")
}

/// What the session holds so far: each finished turn's reply, by number,
/// read from every run's calls.
async fn replies(db: &StoryDb) -> Result<BTreeMap<u32, (String, u64)>, StoryDbError> {
    let mut out = BTreeMap::new();
    for run in db.runs().await? {
        for c in db.calls(&run).await? {
            let Some(n) = c
                .call
                .address
                .strip_prefix("b0/t")
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
fn session(canon: &Canon, words: (u32, u32), replies: &BTreeMap<u32, (String, u64)>) -> Vec<Turn> {
    let mut turns = Vec::new();
    for (n, (text, _)) in replies {
        let ask = match n {
            1 => draft_ask(canon, words),
            n if n % 2 == 0 => CRITIQUE_ASK.to_owned(),
            _ => revise_ask(words),
        };
        turns.push(Turn {
            role: "user",
            content: ask,
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

/// Runs the session from where its stores leave it to its end.
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
    let system = system(&canon);
    // A turn is asked only after the one before it answered, so the replies
    // recorded run from 1 without a gap.
    let mut done = replies(db).await?;
    let writing = |n: u32| n % 2 == 1;
    let kept = |n: u32, text: &str| writing(n) && !broken_off(text);
    // A version written whose row was not stored before a kill.
    let stored = db.chapter_versions(1).await?.len() as u32;
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
        author.add_chapter(1, origin, text).await?;
    }
    let mut versions = stored.max(written);
    let ended = loop {
        let next = done.keys().next_back().map_or(1, |n| n + 1);
        let spent = db.spend("b0/").await?;
        if writing(next) && next > 1 {
            // A revision follows a critique; nothing to decide before it.
        } else if next > 1 {
            // Before a critique: the target, the backstop, the window.
            if config.target.reached(&spent) {
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
        let ask = match next {
            1 => draft_ask(&canon, config.words),
            n if writing(n) => revise_ask(config.words),
            _ => CRITIQUE_ASK.to_owned(),
        };
        let mut turns = session(&canon, config.words, &done);
        turns.push(Turn {
            role: "user",
            content: ask,
        });
        let call = CallAddress {
            run: run.clone(),
            address: address(next),
        };
        let reply = match port.session(&call, Role::Author, &system, &turns).await {
            Ok(text) => text,
            Err(PortError::Store(e)) => return Err(e),
            Err(e) => break Ended::Failed(address(next), e.to_string()),
        };
        if kept(next, &reply) {
            let origin = if next == 1 {
                VersionOrigin::Draft
            } else {
                VersionOrigin::Revision
            };
            author.add_chapter(1, origin, &reply).await?;
            versions += 1;
        }
        // The turn's size as the call log has it, for the window's check.
        done = replies(db).await?;
        done.retain(|n, _| *n <= next);
        done.entry(next).or_insert((reply, 0));
    };
    Ok(Written {
        ended,
        versions,
        spent: db.spend("b0/").await?,
    })
}

#[cfg(test)]
#[path = "single_tests.rs"]
mod tests;
