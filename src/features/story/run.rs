//! A story run as the app starts it (docs/history/fabula.md §4, PR 8;
//! docs/research/fabula-engine.md §14, *The chat tools and the human in the
//! loop*): the story opened from its chat's directory and run from where it
//! stands to its next pause, on the chat's own engine under the app's session
//! budget, within its time limit and its chapter's budget, and what the
//! landing carries back to the chat.
//!
//! - **B0 writes it** (docs/history/fabula.md §6, T25–T27): each chapter is a session
//!   of one writer on the chat's backend, thinking at a medium effort
//!   ([`writer_sampling`]), as M3's judges preferred it to the simulation
//!   ([`super::single_story`]).
//! - **The time limit cancels, it does not kill.** When it ends, the calls are
//!   cancelled and the engine stops at the step in flight, which it records,
//!   so the next run resumes there (spec §9). The run's own token — a stop, a
//!   quit — reaches the calls the same way.
//! - **The landing is data.** What the chapter is and what the pause asks;
//!   the orchestrator words it in the profile's language and puts the chapter
//!   into the chat (docs/history/fabula.md §6, T3).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::board::StoryBoard;
use super::engine::{EngineAdapter, SharedLog};
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role, Turn};
use super::schema::Schema;
use super::single::SingleConfig;
use super::single_story::{self, Budget, MOST_REVISIONS, Outcome, Stop, Story, StoryError};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{Question, QuestionBody};
use crate::shared::api::contract::EngineBackend;
use crate::shared::config::{ServerMode, SpecType, ToolSettings};
use crate::shared::session_budget::SessionBudget;
use crate::shared::storage::story::{Spend, StoryDb, StoryDbError};

/// One run of one story, as the app starts it.
#[derive(Debug, Clone)]
pub struct StoryRun {
    /// The chat the story belongs to.
    pub chat: Uuid,
    /// The story's id within the chat.
    pub story: String,
    /// The run's id: its record's in the chat, its address, its seat on the
    /// board.
    pub run: Uuid,
    /// The story's database and its call log.
    pub db: PathBuf,
    pub log: PathBuf,
    /// The model's name, as the call log records it.
    pub model: String,
    /// How each chapter's session runs, the budget included.
    pub config: SingleConfig,
    /// The model's window as the settings name it; none — asked of the
    /// engine when the run starts, else [`SingleConfig`]'s default.
    pub window: Option<u64>,
    /// The run's time limit (`tools.story_run_timeout_secs`).
    pub timeout: Duration,
    /// Never ask the engine to constrain the decoding (docs/history/fabula.md §5):
    /// a managed server with a draft model, whose grammar crashes under it.
    pub fallback_only: bool,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// At a pause: its question waits for the human's answer.
    Paused(Question),
    /// Every chapter of the arc is accepted.
    Finished,
    /// Short of a pause, at a step the next run resumes from.
    Stopped {
        /// The step.
        step: String,
        /// Why: cancelled, the budget spent, a call that gave nothing usable.
        why: Stop,
    },
    /// Its time limit ended it at `step`.
    TimedOut {
        /// The step.
        step: String,
    },
    /// It could not run: the story could not be opened, or its store failed.
    Failed(String),
}

/// The chapter a pause asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandedChapter {
    /// Its number.
    pub number: u32,
    /// The version asked about.
    pub version: u32,
    /// The version's text, markdown.
    pub text: String,
}

/// What a run brings back to the chat.
#[derive(Debug, Clone, PartialEq)]
pub struct Landed {
    /// How it ended.
    pub ending: Ending,
    /// The chapter the pause asks about, at P2.
    pub chapter: Option<LandedChapter>,
    /// What this run's calls cost.
    pub spent: Spend,
}

impl Landed {
    fn failed(why: impl Into<String>) -> Self {
        Self {
            ending: Ending::Failed(why.into()),
            chapter: None,
            spent: Spend::default(),
        }
    }
}

/// The writer's sampling: thinking at a medium effort, with room for a
/// chapter — the Author's as G1 and PR 6 ran it, which M3's B0 had too. PR 6
/// measured on `gemini-3.8-flash` that the effort, not a schema, decides how
/// much a model thinks (docs/history/fabula.md §4).
pub fn writer_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(true),
        reasoning_effort: Some(ReasoningEffort::Medium),
        max_tokens: Some(32_000),
        ..SamplingConfig::default()
    }
}

/// A chapter's budget under these settings: `0` tokens — none.
pub fn budget_of(tools: &ToolSettings) -> Budget {
    Budget {
        tokens: (tools.story_budget_tokens > 0).then_some(tools.story_budget_tokens),
        usd: tools.story_budget_usd.filter(|usd| *usd > 0.0),
    }
}

/// How a chapter's session runs under these settings: the settings' budget
/// as its ceiling and [`MOST_REVISIONS`] as its end (T25).
pub fn single_config(tools: &ToolSettings) -> SingleConfig {
    SingleConfig {
        budget: budget_of(tools),
        most_revisions: MOST_REVISIONS,
        ..SingleConfig::default()
    }
}

/// Whether the chat's engine must not be asked to constrain the decoding: a
/// managed `llama-server` with a draft model (docs/history/fabula.md §5,
/// llama.cpp#30419).
pub fn needs_fallback(mode: ServerMode, spec_type: SpecType) -> bool {
    mode == ServerMode::Managed && spec_type.needs_draft_model()
}

/// What a story's run takes from the settings, snapshotted with the turn
/// that starts it: the time limit, the chapter's budget, the model's window
/// where the settings name it, and whether the engine may be asked to
/// constrain the decoding.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryParams {
    /// `tools.story_run_timeout_secs`.
    pub timeout: Duration,
    /// The chapter's budget ([`single_config`]).
    pub tools: ToolSettings,
    /// The window the settings name: `compaction.context_tokens`, or a
    /// managed server's `-c`.
    pub window: Option<u64>,
    /// [`needs_fallback`] for the chat's engine.
    pub fallback_only: bool,
}

impl Default for StoryParams {
    fn default() -> Self {
        Self::of(
            &ToolSettings::default(),
            ServerMode::default(),
            SpecType::default(),
            None,
        )
    }
}

impl StoryParams {
    /// The parameters under `tools`, for an engine in `mode` with
    /// `spec_type`, whose window the settings name as `window`.
    pub fn of(
        tools: &ToolSettings,
        mode: ServerMode,
        spec_type: SpecType,
        window: Option<u64>,
    ) -> Self {
        Self {
            timeout: Duration::from_secs(tools.story_run_timeout_secs.max(1)),
            tools: tools.clone(),
            window: window.filter(|w| *w > 0),
            fallback_only: needs_fallback(mode, spec_type),
        }
    }

    /// The chapter's budget under these settings.
    pub fn budget(&self) -> Budget {
        budget_of(&self.tools)
    }

    /// The run of story `story` of chat `chat`, as `run`, over its `files`, on
    /// `model`.
    pub fn run(
        &self,
        (chat, story, run): (Uuid, &str, Uuid),
        files: &super::conversation::StoryFiles,
        model: &str,
    ) -> StoryRun {
        StoryRun {
            chat,
            story: story.to_owned(),
            run,
            db: files.db.clone(),
            log: files.log.clone(),
            model: model.to_owned(),
            config: single_config(&self.tools),
            window: self.window,
            timeout: self.timeout,
            fallback_only: self.fallback_only,
        }
    }
}

/// The port with the call in flight written to the board.
struct Watched<'a> {
    inner: &'a dyn LlmPort,
    board: &'a StoryBoard,
    run: Uuid,
}

#[async_trait::async_trait]
impl LlmPort for Watched<'_> {
    async fn json(
        &self,
        call: &CallAddress,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        self.board.step(self.run, &call.address);
        self.inner.json(call, role, schema, prompt).await
    }

    async fn text(
        &self,
        call: &CallAddress,
        role: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError> {
        self.board.step(self.run, &call.address);
        self.inner.text(call, role, prompt).await
    }

    async fn session(
        &self,
        call: &CallAddress,
        role: Role,
        system: &str,
        turns: &[Turn],
    ) -> Result<String, PortError> {
        self.board.step(self.run, &call.address);
        self.inner.session(call, role, system, turns).await
    }
}

/// Runs `spec` on `backend` until the story pauses, ends or stops, and gives
/// its story back on the board. `cancel` is the run's own token.
pub async fn run(
    spec: &StoryRun,
    backend: Arc<dyn EngineBackend>,
    sessions: Option<Arc<SessionBudget>>,
    board: &StoryBoard,
    cancel: CancellationToken,
) -> Landed {
    // `story_continue` claims the story for this very run while it reads the
    // person's answer, so the claim it hands over is the run's own.
    match board.claim(spec.chat, &spec.story, spec.run) {
        Ok(()) => {}
        Err(held) if held.run == spec.run => {}
        Err(held) => {
            return Landed::failed(format!(
                "the story is being written by another run already ({})",
                held.run
            ));
        }
    }
    let landed = match run_claimed(spec, backend, sessions, board, cancel).await {
        Ok(landed) => landed,
        Err(e) => Landed::failed(e.to_string()),
    };
    board.release(spec.run);
    landed
}

async fn run_claimed(
    spec: &StoryRun,
    backend: Arc<dyn EngineBackend>,
    sessions: Option<Arc<SessionBudget>>,
    board: &StoryBoard,
    cancel: CancellationToken,
) -> Result<Landed, StoryDbError> {
    for dir in [spec.db.parent(), spec.log.parent()].into_iter().flatten() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let db = StoryDb::open(&spec.db, &spec.log).await?;
    let landed = advance_open(spec, &db, backend, sessions, board, cancel).await;
    db.close().await?;
    landed
}

/// The run over the open story; the capabilities are given back before the
/// story is closed.
async fn advance_open(
    spec: &StoryRun,
    db: &StoryDb,
    backend: Arc<dyn EngineBackend>,
    sessions: Option<Arc<SessionBudget>>,
    board: &StoryBoard,
    cancel: CancellationToken,
) -> Result<Landed, StoryDbError> {
    // The window, where the settings do not name it, is the engine's to say
    // (docs/history/fabula.md §6, T25).
    let mut config = spec.config.clone();
    if let Some(window) = match spec.window {
        Some(window) => Some(window),
        None => backend.context_budget().await.map(u64::from),
    } {
        config.window = window;
    }
    let mut author = db.author_writer().await?;
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await?));
    // The calls' own token: the time limit cancels the calls, not the run.
    let calls = cancel.child_token();
    let mut adapter = EngineAdapter::sharing(
        backend,
        log.clone(),
        spec.model.clone(),
        writer_sampling(),
        calls.clone(),
    );
    if let Some(sessions) = sessions {
        adapter = adapter.with_sessions(sessions);
    }
    if spec.fallback_only {
        adapter = adapter.fallback_only();
    }
    let port = Watched {
        inner: &adapter,
        board,
        run: spec.run,
    };
    let before = db.spend("").await?;
    let mut advance = std::pin::pin!(single_story::advance(Story {
        db,
        author: &mut author,
        log: &log,
        port: &port,
        config: &config,
    }));
    let mut deadline = std::pin::pin!(tokio::time::sleep(spec.timeout));
    let mut timed_out = false;
    let outcome = loop {
        tokio::select! {
            outcome = &mut advance => break outcome,
            () = &mut deadline, if !timed_out => {
                timed_out = true;
                calls.cancel();
            }
        }
    };
    let ending = match outcome {
        Ok(Outcome::Paused(question)) => Ending::Paused(question),
        Ok(Outcome::Finished) => Ending::Finished,
        Ok(Outcome::Stopped {
            step,
            why: Stop::Cancelled,
        }) if timed_out => Ending::TimedOut { step },
        Ok(Outcome::Stopped { step, why }) => Ending::Stopped { step, why },
        Err(StoryError::Store(e)) => return Err(e),
        Err(e) => Ending::Failed(e.to_string()),
    };
    let spent = spent_since(&before, &db.spend("").await?);
    let mut landed = landing(db, ending).await?;
    landed.spent = spent;
    Ok(landed)
}

/// What a run's calls added to the story's spend.
fn spent_since(before: &Spend, after: &Spend) -> Spend {
    let cost_usd = match (before.cost_usd, after.cost_usd) {
        (_, None) => None,
        (None, Some(after)) => Some(after),
        (Some(before), Some(after)) => Some((after - before).max(0.0)),
    };
    Spend {
        calls: after.calls.saturating_sub(before.calls),
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        cost_usd,
    }
}

/// The landing's contents for `ending`, read from the stores: at P2, the
/// chapter's version asked about (spec §14, *Pauses*); nothing else rides on
/// a run that stopped short.
async fn landing(db: &StoryDb, ending: Ending) -> Result<Landed, StoryDbError> {
    let mut landed = Landed {
        chapter: None,
        spent: Spend::default(),
        ending,
    };
    if let Ending::Paused(Question {
        body: QuestionBody::Chapter { chapter, version },
        ..
    }) = &landed.ending
        && let Some(asked) = db
            .chapter_versions(*chapter)
            .await?
            .into_iter()
            .find(|v| v.version == *version)
    {
        landed.chapter = Some(LandedChapter {
            number: *chapter,
            version: *version,
            text: asked.text,
        });
    }
    Ok(landed)
}

#[cfg(test)]
#[path = "run_tests.rs"]
pub(crate) mod tests;
