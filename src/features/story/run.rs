//! A story run as the app starts it (docs/fabula.md §4, PR 8;
//! docs/research/fabula-engine.md §14, *The chat tools and the human in the
//! loop*): the story opened from its chat's directory and run from where it
//! stands to its next pause, on the chat's own engine under the app's session
//! budget, within its time limit and its chapter's budget, and what the
//! landing carries back to the chat.
//!
//! - **One model, every role.** The chat's backend serves the characters, the
//!   World Master, the Author and the reviewers; a role differs only by its
//!   sampling ([`role_sampling`]), as G1 and PR 6 measured it (docs/journal/
//!   stories.md).
//! - **The time limit cancels, it does not kill.** When it ends, the calls are
//!   cancelled and the engine stops at the step in flight, which it records,
//!   so the next run resumes there (spec §9). The run's own token — a stop, a
//!   quit — reaches the calls the same way.
//! - **The landing is data.** What the chapter is, what the Author picked and
//!   assumed, what the pause asks; the orchestrator words it in the profile's
//!   language and puts the chapter into the chat (docs/fabula.md §6, T3).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::board::StoryBoard;
use super::chapter::{self, Budget, ChapterConfig, Engine, Outcome, Stop};
use super::engine::{EngineAdapter, SharedLog};
use super::keeper::{LeftOpen, left_open};
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role};
use super::schema::Schema;
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{Assumption, Question, QuestionBody};
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
    /// How the chapter is made, the budget included.
    pub config: ChapterConfig,
    /// The run's time limit (`tools.story_run_timeout_secs`).
    pub timeout: Duration,
    /// Never ask the engine to constrain the decoding (docs/fabula.md §5):
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
    /// The Author's summary of it, in the human's language.
    pub summary: Option<String>,
}

/// What a run brings back to the chat.
#[derive(Debug, Clone, PartialEq)]
pub struct Landed {
    /// How it ended.
    pub ending: Ending,
    /// The chapter the pause asks about, at P2.
    pub chapter: Option<LandedChapter>,
    /// At a pause about a chapter, the Author's note on each scene it keeps
    /// in its latest take, by the scene's number, in order: the numbers a
    /// reply may name to play a scene again (PR 10).
    pub picks: Vec<(u32, String)>,
    /// And that chapter's assumptions: what a role chose where it could have
    /// asked (spec §14, *Pauses*). Neither rides on a run that stopped short.
    pub assumptions: Vec<Assumption>,
    /// At the arc's last chapter, the threads the story leaves open, with the
    /// Author's reasons (docs/fabula.md §6, T16).
    pub left_open: Vec<LeftOpen>,
    /// What this run's calls cost.
    pub spent: Spend,
}

impl Landed {
    fn failed(why: impl Into<String>) -> Self {
        Self {
            ending: Ending::Failed(why.into()),
            chapter: None,
            picks: Vec::new(),
            assumptions: Vec::new(),
            left_open: Vec::new(),
            spent: Spend::default(),
        }
    }
}

/// The characters' sampling: thinking off, as the application mutes a turn
/// (spec §8, *Thinking is a role's setting*; G1).
pub fn character_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.8),
        thinking: Some(false),
        reasoning_effort: Some(ReasoningEffort::None),
        reasoning_budget: Some(0),
        max_tokens: Some(4096),
        ..SamplingConfig::default()
    }
}

/// Every other role's sampling, as G1 and PR 6 ran them: the World Master
/// and the reviewers think at a low effort, the Author at a medium one with
/// room for a chapter. PR 6 measured on `gemini-3.8-flash` that a role's
/// effort, not its schema, decides how much it thinks (docs/fabula.md §4).
pub fn role_sampling() -> Vec<(Role, SamplingConfig)> {
    let thinking = |temperature: f32, effort: ReasoningEffort, max_tokens: usize| SamplingConfig {
        temperature: Some(temperature),
        thinking: Some(true),
        reasoning_effort: Some(effort),
        max_tokens: Some(max_tokens),
        ..SamplingConfig::default()
    };
    // The reviewers' cap was 16,384 until PR 10 found the Continuity
    // checker's reply cut there four times on Qwen 3.8 27B (PR 11).
    let reviewer = thinking(0.4, ReasoningEffort::Low, 32_000);
    vec![
        (
            Role::WorldMaster,
            thinking(0.6, ReasoningEffort::Low, 16_384),
        ),
        (Role::Author, thinking(0.8, ReasoningEffort::Medium, 32_000)),
        (Role::Reader, reviewer.clone()),
        (Role::Continuity, reviewer.clone()),
        (Role::CanonKeeper, reviewer),
    ]
}

/// How a chapter is made under these settings: the engine's own shape, the
/// settings' budget (`0` tokens — none), and the human's language for what
/// the human reads.
pub fn chapter_config(tools: &ToolSettings, human_language: &str) -> ChapterConfig {
    ChapterConfig {
        budget: Budget {
            tokens: (tools.story_budget_tokens > 0).then_some(tools.story_budget_tokens),
            usd: tools.story_budget_usd.filter(|usd| *usd > 0.0),
        },
        human_language: human_language.to_owned(),
        ..ChapterConfig::default()
    }
}

/// Whether the chat's engine must not be asked to constrain the decoding: a
/// managed `llama-server` with a draft model (docs/fabula.md §5,
/// llama.cpp#30419).
pub fn needs_fallback(mode: ServerMode, spec_type: SpecType) -> bool {
    mode == ServerMode::Managed && spec_type.needs_draft_model()
}

/// What a story's run takes from the settings, snapshotted with the turn
/// that starts it: the time limit, the chapter's budget, and whether the
/// engine may be asked to constrain the decoding.
#[derive(Debug, Clone, PartialEq)]
pub struct StoryParams {
    /// `tools.story_run_timeout_secs`.
    pub timeout: Duration,
    /// The chapter's budget and the engine's shape ([`chapter_config`]),
    /// before the human's language is known.
    pub tools: ToolSettings,
    /// [`needs_fallback`] for the chat's engine.
    pub fallback_only: bool,
}

impl Default for StoryParams {
    fn default() -> Self {
        Self::of(
            &ToolSettings::default(),
            ServerMode::default(),
            SpecType::default(),
        )
    }
}

impl StoryParams {
    /// The parameters under `tools`, for an engine in `mode` with `spec_type`.
    pub fn of(tools: &ToolSettings, mode: ServerMode, spec_type: SpecType) -> Self {
        Self {
            timeout: Duration::from_secs(tools.story_run_timeout_secs.max(1)),
            tools: tools.clone(),
            fallback_only: needs_fallback(mode, spec_type),
        }
    }

    /// The chapter's budget under these settings.
    pub fn budget(&self) -> Budget {
        chapter_config(&self.tools, "").budget
    }

    /// The run of story `story` of chat `chat`, as `run`, over its `files`, on
    /// `model`, telling the human in `human_language`.
    pub fn run(
        &self,
        (chat, story, run): (Uuid, &str, Uuid),
        files: &super::conversation::StoryFiles,
        model: &str,
        human_language: &str,
    ) -> StoryRun {
        StoryRun {
            chat,
            story: story.to_owned(),
            run,
            db: files.db.clone(),
            log: files.log.clone(),
            model: model.to_owned(),
            config: chapter_config(&self.tools, human_language),
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
    let mut author = db.author_writer().await?;
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await?));
    // The calls' own token: the time limit cancels the calls, not the run.
    let calls = cancel.child_token();
    let mut adapter = EngineAdapter::sharing(
        backend,
        log.clone(),
        spec.model.clone(),
        character_sampling(),
        calls.clone(),
    );
    for (role, sampling) in role_sampling() {
        adapter = adapter.with_role_sampling(role, sampling);
    }
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
    let mut advance = std::pin::pin!(chapter::advance(Engine {
        db,
        author: &mut author,
        log: &log,
        port: &port,
        config: &spec.config,
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
        Err(chapter::StoryError::Store(e)) => return Err(e),
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

/// The chapter a pause asks about; none for the canon's.
fn asked_about(question: &Question) -> Option<u32> {
    match question.body {
        QuestionBody::Canon => None,
        QuestionBody::Chapter { chapter, .. } | QuestionBody::Scene { chapter, .. } => {
            Some(chapter)
        }
    }
}

/// The landing's contents for `ending`, read from the stores.
async fn landing(db: &StoryDb, ending: Ending) -> Result<Landed, StoryDbError> {
    let mut landed = Landed {
        chapter: None,
        picks: Vec::new(),
        assumptions: Vec::new(),
        left_open: Vec::new(),
        spent: Spend::default(),
        ending,
    };
    // What the user decides on rides with the question (spec §14, *Pauses*):
    // a run that stopped short asks nothing, and its chapter's picks and
    // assumptions wait for the checkpoint, rather than repeating at every stop.
    if !matches!(landed.ending, Ending::Paused(_)) {
        return Ok(landed);
    }
    let Ending::Paused(question) = &landed.ending else {
        return Ok(landed);
    };
    let Some(n) = asked_about(question) else {
        return Ok(landed);
    };
    if let Ending::Paused(Question {
        body: QuestionBody::Chapter { version, .. },
        ..
    }) = &landed.ending
        && let Some(asked) = db
            .chapter_versions(n)
            .await?
            .into_iter()
            .find(|v| v.version == *version)
    {
        let chapters = db.canon().await?.map_or(0, |c| c.arc.chapters.len());
        if n as usize == chapters {
            landed.left_open = left_open(db, n, &asked).await?;
        }
        landed.chapter = Some(LandedChapter {
            number: n,
            version: *version,
            summary: db.summary(&asked.id).await?,
            text: asked.text,
        });
    }
    for kept in db.kept_scenes(n).await? {
        if !kept.rationale.trim().is_empty() {
            landed.picks.push((kept.scene.scene, kept.rationale));
        }
    }
    let made: Vec<Assumption> = db
        .assumptions(&format!("ch{n}/"))
        .await?
        .into_iter()
        .map(|a| a.assumption)
        .collect();
    landed.assumptions = Assumption::standing(&made);
    Ok(landed)
}

#[cfg(test)]
#[path = "run_tests.rs"]
pub(crate) mod tests;
