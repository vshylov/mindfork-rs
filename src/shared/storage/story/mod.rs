//! A story's database: Fabula's stores behind one actor
//! (docs/research/fabula-engine.md §5, §6, §9; docs/fabula.md §3).
//!
//! - **One physical writer.** A thread of its own owns the only connections
//!   to the story's two files (the database and its call log); rusqlite calls
//!   block, so they never run on the async runtime. Every read and write is a
//!   job sent to that thread and awaited through a oneshot, so jobs run one at
//!   a time and a commit's check and its append see the same state.
//! - **One logical writer per store.** The actor hands out capabilities that
//!   are not `Clone` (one [`AuthorWriter`], one [`RunLogWriter`]), refuses a
//!   second while the first lives, and checks every write against the one it
//!   issued. Dropping a capability frees its store for the next one.
//! - **One open per file.** A second [`StoryDb::open`] of a file already open
//!   in this process is refused; across processes the application's
//!   single-instance lock holds, and the evaluation's runner writes into a
//!   directory of its own (spec §15).

mod actor;
mod schema;
mod stats;
mod status;
mod writers;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, mpsc};

use serde_json::Value;
use tokio::sync::oneshot;

use crate::entities::story::{
    Canon, CanonProblem, ChapterId, ChapterVersion, Question, QuestionId, Resolution, RunId,
};
use actor::{Actor, Job};

pub use stats::{StoryStats, story_stats_of_file, story_stats_of_image};
#[cfg(test)]
pub use status::ChapterState;
pub use status::{StoryStatus, story_status_of};
pub use writers::{AuthorWriter, RunLogWriter};

/// Why the story database refused or failed. The texts are for logs and for
/// the engine; a person sees what the story tools make of them.
#[derive(Debug, thiserror::Error)]
pub enum StoryDbError {
    /// SQLite failed.
    #[error("story database: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A stored value or an argument did not serialize.
    #[error("story database: {0}")]
    Json(#[from] serde_json::Error),
    /// The file system failed.
    #[error("story database: {0}")]
    Io(#[from] std::io::Error),
    /// A file that is not a SQLite database, judged by its header.
    #[error("{file} {} is not a SQLite database", path.display())]
    NotADatabase {
        /// Which of the story's files.
        file: &'static str,
        /// Its path.
        path: PathBuf,
    },
    /// A file a newer build wrote.
    #[error(
        "{file} {} is from a newer version of mindfork (schema {found}, supported {supported})",
        path.display()
    )]
    NewerSchema {
        /// Which of the story's files.
        file: &'static str,
        /// Its path.
        path: PathBuf,
        /// Its version.
        found: u32,
        /// This build's.
        supported: u32,
    },
    /// The story is open already in this process.
    #[error("the story {} is open already", .0.display())]
    AlreadyOpen(PathBuf),
    /// A path that cannot name a story's file.
    #[error("{} cannot hold a story", .0.display())]
    BadPath(PathBuf),
    /// The database was closed, or its thread ended.
    #[error("the story database is closed")]
    Closed,
    /// A capability that is held already.
    #[error("{0} is held already")]
    WriterHeld(String),
    /// A capability the actor no longer recognizes.
    #[error("the writer was revoked")]
    WriterRevoked,
    /// The canon has problems; all of them.
    #[error("the canon was refused: {}", join(.0))]
    Canon(Vec<CanonProblem>),
    /// The canon cannot change once it is approved.
    #[error("the canon cannot change once it is approved")]
    CanonFrozen,
    /// A run the story does not have.
    #[error("there is no run {0}")]
    UnknownRun(RunId),
    /// A call recorded twice at one address and attempt.
    #[error("call {address} attempt {attempt} of {run} is recorded already")]
    CallExists {
        /// Its run.
        run: RunId,
        /// Its address.
        address: String,
        /// Its attempt.
        attempt: u32,
    },
    /// Stored data that does not read back, or no longer folds.
    #[error("the story database is inconsistent: {0}")]
    Corrupt(String),
    /// A row of the manuscript written twice: an answer.
    #[error("{0} exists already")]
    Exists(String),
    /// A row of the manuscript the story does not have.
    #[error("there is no {0}")]
    Unknown(String),
    /// A chapter version out of its order: a draft after a version, a
    /// revision before one.
    #[error("out of order: {0}")]
    OutOfOrder(String),
    /// A question asked while another waits for its answer.
    #[error("question {0} waits for its answer")]
    QuestionPending(QuestionId),
}

fn join(problems: &[CanonProblem]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// What a set of calls cost, as the call log recorded it (docs/fabula.md §6,
/// T1: tokens on every provider, dollars where the provider reports them).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spend {
    /// The attempts recorded.
    pub calls: u64,
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Reply tokens.
    pub output_tokens: u64,
    /// Dollars, where any call reported them.
    pub cost_usd: Option<f64>,
}

impl Spend {
    /// Prompt and reply tokens together: what the budget counts.
    pub fn tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// How a call's structured output was obtained (spec §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// Schema-constrained decoding.
    Schema,
    /// The schema in the prompt, a lenient parse and one repair.
    Fallback,
    /// Free text: chapter prose.
    Text,
}

impl Mechanism {
    /// The stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Mechanism::Schema => "schema",
            Mechanism::Fallback => "fallback",
            Mechanism::Text => "text",
        }
    }

    /// The mechanism with this stored name.
    pub fn parse(s: &str) -> Option<Self> {
        [Mechanism::Schema, Mechanism::Fallback, Mechanism::Text]
            .into_iter()
            .find(|v| v.as_str() == s)
    }
}

/// How a call ended (spec §8, *What still breaks a reply*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// A reply came back.
    Ok,
    /// The reply was cut at its cap.
    Cut,
    /// The model or a filter refused.
    Refused,
    /// The provider or the transport failed.
    Error,
}

impl CallStatus {
    /// The stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            CallStatus::Ok => "ok",
            CallStatus::Cut => "cut",
            CallStatus::Refused => "refused",
            CallStatus::Error => "error",
        }
    }

    /// The status with this stored name.
    pub fn parse(s: &str) -> Option<Self> {
        [
            CallStatus::Ok,
            CallStatus::Cut,
            CallStatus::Refused,
            CallStatus::Error,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }
}

/// What a call cost, as far as the provider said.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CallUsage {
    /// Prompt tokens.
    pub input_tokens: Option<u64>,
    /// Reply tokens, thinking included where it is billed so.
    pub output_tokens: Option<u64>,
    /// Prompt tokens served from the cache.
    pub cached_tokens: Option<u64>,
    /// Dollars, where the provider reports them (OpenRouter).
    pub cost_usd: Option<f64>,
}

/// One model call for the call log, keyed by its logical address (spec §9).
#[derive(Debug, Clone, PartialEq)]
pub struct NewCall {
    /// The run it belongs to.
    pub run: RunId,
    /// Its logical address, `ch1/b0/t3`.
    pub address: String,
    /// Its attempt at that address, from 1.
    pub attempt: u32,
    /// The role that made it.
    pub role: String,
    /// The model it went to.
    pub model: String,
    /// Its sampling and thinking parameters.
    pub params: Value,
    /// How its output was obtained.
    pub mechanism: Mechanism,
    /// The request as sent.
    pub request: Value,
    /// The prompt's hash.
    pub prompt_sha256: String,
    /// The reply; none when there was none.
    pub response: Option<Value>,
    /// How it ended.
    pub status: CallStatus,
    /// What it cost.
    pub usage: CallUsage,
    /// How long it took.
    pub latency_ms: Option<u64>,
}

/// A recorded call.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredCall {
    /// Its id, a UUIDv5 of its run, address and attempt.
    pub id: String,
    /// The call as recorded.
    pub call: NewCall,
    /// When it was recorded (RFC 3339).
    pub created_at: String,
}

/// The files open in this process, by [`schema::registry_key`].
static OPEN: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// A story file's place in [`OPEN`], given back when the actor's thread ends,
/// however it ends.
struct Claim(PathBuf);

impl Claim {
    fn take(key: PathBuf) -> Result<Self, StoryDbError> {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        if !open.insert(key.clone()) {
            return Err(StoryDbError::AlreadyOpen(key));
        }
        Ok(Self(key))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        OPEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// A handle on an open story. Cloning it is cheap: every clone talks to the
/// same actor, which lives while any handle or capability does, or until
/// [`StoryDb::close`].
#[derive(Clone)]
pub struct StoryDb {
    tx: mpsc::Sender<Job>,
}

impl StoryDb {
    /// Opens a story: its database at `story` and its call log at `calls`,
    /// created when absent and brought to this build's schemas.
    pub async fn open(story: &Path, calls: &Path) -> Result<Self, StoryDbError> {
        let key = schema::registry_key(story)?;
        if schema::registry_key(calls)? == key {
            return Err(StoryDbError::BadPath(calls.to_owned()));
        }
        let claim = Claim::take(key)?;
        let (story, calls) = (story.to_owned(), calls.to_owned());
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = oneshot::channel();
        std::thread::Builder::new()
            .name("story-db".to_owned())
            .spawn(move || {
                let actor = match Actor::open(&story, &calls) {
                    Ok(actor) => actor,
                    Err(e) => {
                        // Given back before the error is reported, so the
                        // caller may try again at once.
                        drop(claim);
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let ack = actor.run(rx);
                drop(claim);
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            })?;
        ready_rx.await.map_err(|_| StoryDbError::Closed)??;
        Ok(Self { tx })
    }

    /// Closes the story: the actor stops, its connections close and the file
    /// can be opened again once this returns. Every other handle and
    /// capability gets [`StoryDbError::Closed`] from then on.
    pub async fn close(self) -> Result<(), StoryDbError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |actor: &mut Actor| actor.close_with(ack_tx)))
            .map_err(|_| StoryDbError::Closed)?;
        ack_rx.await.map_err(|_| StoryDbError::Closed)
    }

    /// Runs `f` on the actor's thread and returns its result.
    async fn run<T, F>(&self, f: F) -> Result<T, StoryDbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Actor) -> Result<T, StoryDbError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |actor: &mut Actor| {
                let _ = reply_tx.send(f(actor));
            }))
            .map_err(|_| StoryDbError::Closed)?;
        reply_rx.await.map_err(|_| StoryDbError::Closed)?
    }

    /// Sends `f` to the actor without waiting: a capability's release on drop.
    fn post(&self, f: impl FnOnce(&mut Actor) + Send + 'static) {
        // A closed actor has nothing left to release.
        let _ = self.tx.send(Box::new(f));
    }

    /// The Author's capability: the canon, the chapters, the pauses (spec §5).
    pub async fn author_writer(&self) -> Result<AuthorWriter, StoryDbError> {
        let token = self.run(|a| a.issue_author()).await?;
        Ok(AuthorWriter::new(self.clone(), token))
    }

    /// The engine's capability over the run log: runs and calls.
    pub async fn run_log_writer(&self) -> Result<RunLogWriter, StoryDbError> {
        let token = self.run(|a| a.issue_run_log()).await?;
        Ok(RunLogWriter::new(self.clone(), token))
    }

    /// The canon as its latest documents assemble it; none before an import.
    pub async fn canon(&self) -> Result<Option<Canon>, StoryDbError> {
        self.run(|a| Ok(a.canon()?.map(|c| (*c).clone()))).await
    }

    /// What chapter `chapter` has cost, against its budget (docs/fabula.md
    /// §6, T1).
    pub async fn chapter_spend(&self, chapter: u32) -> Result<Spend, StoryDbError> {
        self.run(move |a| a.chapter_spend(chapter)).await
    }

    /// The call recorded at `address` and `attempt` of `run`.
    #[cfg(test)]
    pub async fn call(
        &self,
        run: &RunId,
        address: &str,
        attempt: u32,
    ) -> Result<Option<StoredCall>, StoryDbError> {
        let (run, address) = (run.clone(), address.to_owned());
        self.run(move |a| a.call(&run, &address, attempt)).await
    }

    /// Every run of the story, the first first, whether or not it recorded a
    /// call.
    pub async fn runs(&self) -> Result<Vec<RunId>, StoryDbError> {
        let last = self.run(|a| a.last_run()).await?;
        Ok((1..=last).map(RunId::nth).collect())
    }

    /// Every call recorded for `run`, in the order recorded: what a fixture
    /// of the run exports (spec §9).
    pub async fn calls(&self, run: &RunId) -> Result<Vec<StoredCall>, StoryDbError> {
        let run = run.clone();
        self.run(move |a| a.calls(&run)).await
    }

    /// Whether every document of the canon as it stands is approved (P1).
    pub async fn canon_approved(&self) -> Result<bool, StoryDbError> {
        self.run(|a| a.canon_approved()).await
    }

    /// Chapter `number`'s versions, oldest first.
    pub async fn chapter_versions(&self, number: u32) -> Result<Vec<ChapterVersion>, StoryDbError> {
        self.run(move |a| a.chapter_versions(number)).await
    }

    /// Whether a chapter version is accepted (P2).
    pub async fn accepted(&self, chapter: &ChapterId) -> Result<bool, StoryDbError> {
        let chapter = chapter.clone();
        self.run(move |a| a.accepted(&chapter)).await
    }

    /// Every question with its answer, in the order asked.
    pub async fn questions(&self) -> Result<Vec<(Question, Option<Resolution>)>, StoryDbError> {
        self.run(|a| a.questions()).await
    }

    /// The question that waits for its answer, if one does.
    pub async fn pending_question(&self) -> Result<Option<Question>, StoryDbError> {
        Ok(self
            .questions()
            .await?
            .into_iter()
            .find(|(_, a)| a.is_none())
            .map(|(q, _)| q))
    }

    /// What the calls at addresses under `prefix` cost, across every run.
    pub async fn spend(&self, prefix: &str) -> Result<Spend, StoryDbError> {
        let prefix = prefix.to_owned();
        self.run(move |a| a.spend(&prefix)).await
    }

    /// Runs raw SQL on the story database, for tests of what the schema
    /// itself refuses.
    #[cfg(test)]
    async fn execute_raw(&self, sql: &str) -> Result<usize, StoryDbError> {
        let sql = sql.to_owned();
        self.run(move |a| a.execute_raw(&sql, false)).await
    }

    /// The same on the call log.
    #[cfg(test)]
    async fn execute_raw_calls(&self, sql: &str) -> Result<usize, StoryDbError> {
        let sql = sql.to_owned();
        self.run(move |a| a.execute_raw(&sql, true)).await
    }
}

#[cfg(test)]
mod manuscript_tests;
#[cfg(test)]
mod tests;
