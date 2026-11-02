//! The chapter's loop (docs/research/fabula-engine.md §7; §9 *Resumable
//! steps*; §14 *Pauses*; docs/fabula.md §4, PR 6): a run of the story from
//! where it stands to its next pause.
//!
//! 1. **P1**: an unapproved canon asks for its approval and stops the run.
//! 2. **The outline**: the chapter's goals, set once.
//! 3. **Scene by scene**: the Author plans the next scene from the scenes kept
//!    so far, or ends the chapter; the world plays it as K branches; the
//!    Author picks one against its intent and the rubric, or asks for the
//!    scene again under changed circumstances, at most `max_reruns` times — on
//!    the last attempt it may only pick. A scene whose every branch fails is
//!    played again as it was, and after `1 + max_reruns` such attempts it asks
//!    the human (**P3**).
//! 4. **The draft**, from the kept scenes' full traces.
//! 5. **Review**: the Reader and the Continuity checker, side by side; the
//!    engine's own check for a planted marker in the prose rides with the
//!    checker. The Author decides each note, and revises while an accepted
//!    high-severity note stands, at most `max_revisions` times; the last
//!    revision is not reviewed again (spec §7's loop).
//! 6. **The summary** of the final version, in the human's language, and
//!    **P2**: accept it, or revise it with the human's notes.
//!
//! **The stores are the state.** Nothing the run decided lives only in
//! memory: each step reads what is stored before it calls a model, so a run
//! killed anywhere — a quit, a crash, a dropped future — resumes from the
//! first step no run finished, as a new run (spec §9). A branch that was in
//! flight is lost, as the spec has it, and another plays in its place, so an
//! interruption never costs the scene an attempt.
//! A step whose result has no row of its own (the Author ending the chapter
//! at a plan) is known by its step.
//!
//! **The budget** (docs/fabula.md §6, T1) counts every call under the
//! chapter's addresses, in every run and attempt: tokens on every provider,
//! dollars where the provider reports them. It is checked before each call,
//! and a spent one stops the run there.
//!
//! **A pause ends the run** (spec §14): its question is a row, the run
//! returns, and the next run starts only once the question is answered
//! ([`resolve`]).

use std::collections::BTreeMap;

use futures_util::future::join;
use serde_json::{Value, json};

use super::author::{
    ContinuityReply, OutlineReply, PickChoice, PickReply, PlanReply, ReaderRemark, ReaderReply,
    SceneDraft, TriageReply,
};
use super::author_prompts::{self as prompts, ChapterContext, Kept, Trace};
use super::engine::SharedLog;
use super::leaks;
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role, decide};
use super::scene::SceneError;
use super::schema::{CanonKeys, Schema, StorySchema};
use super::simulation::{SimConfig, play_scene};
use super::steps::Steps;
use crate::entities::story::{
    Assumption, BranchId, Canon, ChapterOutline, ChapterVersion, Decided, Decision, EventId,
    NewNote, NoteDecision, NoteKind, NoteSeverity, Question, QuestionBody, QuestionId, Resolution,
    ReviewNote, Reviewer, RunId, VersionOrigin, WorldState,
};
use crate::shared::storage::story::{AuthorWriter, StepStatus, StoredScene, StoryDb, StoryDbError};

/// A chapter's spending limits (docs/fabula.md §6, T1 (a)); none is no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Budget {
    /// Prompt and reply tokens, every provider.
    pub tokens: Option<u64>,
    /// Dollars, where the provider reports them.
    pub usd: Option<f64>,
}

/// How a chapter is made; not the Author's to choose (spec §6, `RunConfig`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterConfig {
    /// How each scene is simulated.
    pub sim: SimConfig,
    /// The most scenes a chapter has.
    pub max_scenes: u32,
    /// The most reruns a scene may have at the Author's request.
    pub max_reruns: u32,
    /// The most revisions after review.
    pub max_revisions: u32,
    /// The chapter's length, in words, as asked of the Author.
    pub words: (u32, u32),
    /// The spending limits.
    pub budget: Budget,
    /// The language of what the human reads: the summary, a pick's note, the
    /// assumptions.
    pub human_language: String,
}

impl Default for ChapterConfig {
    /// The spec's values (§13, *Configuration sketch*), and an MVP chapter's
    /// length (spec §11: stories of 1,500–3,000 words).
    fn default() -> Self {
        Self {
            sim: SimConfig::default(),
            max_scenes: 4,
            max_reruns: 2,
            max_revisions: 2,
            words: (1200, 2500),
            budget: Budget::default(),
            human_language: "English".to_owned(),
        }
    }
}

impl ChapterConfig {
    fn to_json(&self) -> Value {
        json!({
            "engine": "chapter",
            "branches": self.sim.branches,
            "max_rounds": self.sim.max_rounds,
            "abnormal_p": self.sim.abnormal_p,
            "reflect_every": self.sim.reflect_every,
            "max_scenes": self.max_scenes,
            "max_reruns": self.max_reruns,
            "max_revisions": self.max_revisions,
            "words": [self.words.0, self.words.1],
            "budget": {"tokens": self.budget.tokens, "usd": self.budget.usd},
            "human_language": self.human_language,
        })
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

/// What a run works with: the story, the Author's capability, the run log
/// shared with the port, the port and the configuration.
pub struct Engine<'a> {
    /// The story.
    pub db: &'a StoryDb,
    /// The Author's capability.
    pub author: &'a mut AuthorWriter,
    /// The run log, shared with the port that records the calls.
    pub log: &'a SharedLog,
    /// The model.
    pub port: &'a dyn LlmPort,
    /// How chapters are made.
    pub config: &'a ChapterConfig,
}

/// Runs the story from where it stands to its next pause, to its end, or to
/// a stop; a pending question returns at once and starts no run.
pub async fn advance(engine: Engine<'_>) -> Result<Outcome, StoryError> {
    let Engine {
        db,
        author,
        log,
        port,
        config,
    } = engine;
    if let Some(question) = db.pending_question().await? {
        return Ok(Outcome::Paused(question));
    }
    let canon = db.canon().await?.ok_or(StoryError::NoCanon)?;
    let run = log.lock().await.start_run(&config.to_json()).await?;
    let steps = Steps::load(db, log.clone(), run.clone()).await?;
    let mut story = Run {
        db,
        author,
        steps,
        port: Budgeted {
            inner: port,
            db,
            prefix: String::new(),
            budget: config.budget,
        },
        config,
        keys: CanonKeys::of(&canon),
        canon,
        run,
    };
    match story.go().await {
        Ok(outcome) => Ok(outcome),
        Err(Halt::Pause(question)) => Ok(Outcome::Paused(question)),
        Err(Halt::Stop { step, why }) => {
            story.steps.finish(&step, StepStatus::Failed).await?;
            Ok(Outcome::Stopped { step, why })
        }
        Err(Halt::Store(e)) => Err(e.into()),
    }
}

/// Records the human's answer to the pending question `question` (spec §14):
/// P1 approved opens chapter 1, P2 approved accepts the chapter and its notes
/// reach the next one, a change revises the chapter with them, and an
/// answered P3 plans the scene again. The next [`advance`] acts on it.
pub async fn resolve(
    author: &mut AuthorWriter,
    question: &QuestionId,
    resolution: &Resolution,
) -> Result<(), StoryDbError> {
    author.resolve(question, resolution).await
}

/// Why a step does not go on.
enum Halt {
    /// At a pause.
    Pause(Question),
    /// Short of one.
    Stop { step: String, why: Stop },
    /// The store failed.
    Store(StoryDbError),
}

impl From<StoryDbError> for Halt {
    fn from(e: StoryDbError) -> Self {
        Halt::Store(e)
    }
}

type Go<T> = Result<T, Halt>;

/// A port error at `step` as the run sees it.
fn halt(step: &str, e: PortError) -> Halt {
    let why = match e {
        PortError::Cancelled { .. } => Stop::Cancelled,
        PortError::Budget { spent, .. } => Stop::Budget(spent),
        PortError::Store(e) => return Halt::Store(e),
        other => Stop::Failed(other.to_string()),
    };
    Halt::Stop {
        step: step.to_owned(),
        why,
    }
}

fn scene_halt(step: &str, e: SceneError) -> Halt {
    match e {
        SceneError::Port(e) => halt(step, e),
        SceneError::Store(e) => Halt::Store(e),
        SceneError::NoCanon => Halt::Store(StoryDbError::NoCanon),
    }
}

/// The port with the chapter's budget checked before every call.
struct Budgeted<'a> {
    inner: &'a dyn LlmPort,
    db: &'a StoryDb,
    /// The chapter's addresses, `ch1/`.
    prefix: String,
    budget: Budget,
}

impl Budgeted<'_> {
    async fn check(&self, call: &CallAddress) -> Result<(), PortError> {
        if self.budget == Budget::default() {
            return Ok(());
        }
        let spend = self.db.spend(&self.prefix).await?;
        let over = |spent: String| PortError::Budget {
            address: call.address.clone(),
            spent,
        };
        if let Some(limit) = self.budget.tokens
            && spend.tokens() >= limit
        {
            return Err(over(format!("{} tokens of {limit}", spend.tokens())));
        }
        if let (Some(limit), Some(cost)) = (self.budget.usd, spend.cost_usd)
            && cost >= limit
        {
            return Err(over(format!("${cost:.4} of ${limit:.2}")));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl LlmPort for Budgeted<'_> {
    async fn json(
        &self,
        call: &CallAddress,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Result<Value, PortError> {
        self.check(call).await?;
        self.inner.json(call, role, schema, prompt).await
    }

    async fn text(
        &self,
        call: &CallAddress,
        role: Role,
        prompt: &Prompt,
    ) -> Result<String, PortError> {
        self.check(call).await?;
        self.inner.text(call, role, prompt).await
    }
}

/// A scene the chapter kept, with the trace of its pick.
struct KeptScene {
    stored: StoredScene,
    trace: Trace,
}

/// What a chapter's prompts share, read from the stores.
struct Context {
    summaries: Vec<(u32, String)>,
    previous: Option<String>,
    human_notes: Option<String>,
    goals: Vec<String>,
}

/// One run.
struct Run<'a> {
    db: &'a StoryDb,
    author: &'a mut AuthorWriter,
    steps: Steps,
    port: Budgeted<'a>,
    config: &'a ChapterConfig,
    canon: Canon,
    keys: CanonKeys,
    run: RunId,
}

impl Run<'_> {
    fn call(&self, address: &str) -> CallAddress {
        CallAddress {
            run: self.run.clone(),
            address: address.to_owned(),
        }
    }

    async fn go(&mut self) -> Go<Outcome> {
        if !self.db.canon_approved().await? {
            return Err(self.ask("canon", QuestionBody::Canon).await?);
        }
        loop {
            let Some(n) = self.open_chapter().await? else {
                return Ok(Outcome::Finished);
            };
            self.chapter(n).await?;
        }
    }

    /// Asks a pause's question at `step`: the run ends with it.
    async fn ask(&mut self, step: &str, body: QuestionBody) -> Go<Halt> {
        let question = self.author.add_question(&self.run, step, &body).await?;
        self.steps.finish(step, StepStatus::Done).await?;
        Ok(Halt::Pause(question))
    }

    /// The first chapter of the arc with no accepted version.
    async fn open_chapter(&self) -> Go<Option<u32>> {
        for n in 1..=self.canon.arc.chapters.len() as u32 {
            if self.accepted_version(n).await?.is_none() {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }

    async fn accepted_version(&self, n: u32) -> Go<Option<ChapterVersion>> {
        for version in self.db.chapter_versions(n).await?.into_iter().rev() {
            if self.db.accepted(&version.id).await? {
                return Ok(Some(version));
            }
        }
        Ok(None)
    }

    /// The P2 question about version `version` of chapter `n`, with its
    /// answer.
    async fn checkpoint_of(
        &self,
        n: u32,
        version: u32,
    ) -> Go<Option<(Question, Option<Resolution>)>> {
        Ok(self.db.questions().await?.into_iter().find(|(q, _)| {
            q.body
                == QuestionBody::Chapter {
                    chapter: n,
                    version,
                }
        }))
    }

    /// The prompts' shared part for chapter `n`: the accepted chapters'
    /// summaries, the previous chapter, the human's notes on it, the goals.
    async fn context(&self, n: u32, goals: Vec<String>) -> Go<Context> {
        let mut summaries = Vec::new();
        let mut previous = None;
        let mut human_notes = None;
        for k in 1..n {
            let Some(version) = self.accepted_version(k).await? else {
                continue;
            };
            if let Some(summary) = self.db.summary(&version.id).await? {
                summaries.push((k, summary));
            }
            if k + 1 == n {
                human_notes = self
                    .checkpoint_of(k, version.version)
                    .await?
                    .and_then(|(_, a)| a)
                    .map(|a| a.notes)
                    .filter(|notes| !notes.trim().is_empty());
                previous = Some(version.text);
            }
        }
        Ok(Context {
            summaries,
            previous,
            human_notes,
            goals,
        })
    }

    fn view<'c>(&'c self, n: u32, ctx: &'c Context) -> ChapterContext<'c> {
        ChapterContext {
            canon: &self.canon,
            number: n,
            human_language: &self.config.human_language,
            summaries: &ctx.summaries,
            previous: ctx.previous.as_deref(),
            human_notes: ctx.human_notes.as_deref(),
            goals: &ctx.goals,
        }
    }

    async fn assume(&mut self, step: &str, assumptions: &[Assumption]) -> Go<()> {
        for a in assumptions {
            let a = Assumption {
                doubt: a.doubt.trim().to_owned(),
                choice: a.choice.trim().to_owned(),
            };
            if !a.doubt.is_empty() {
                self.author
                    .add_assumption(&self.run, step, Role::Author.as_str(), &a)
                    .await?;
            }
        }
        Ok(())
    }

    /// A JSON call that halts the run at `address` when it gives no answer.
    async fn ask_json<T: serde::de::DeserializeOwned>(
        &self,
        address: &str,
        role: Role,
        schema: &Schema,
        prompt: &Prompt,
    ) -> Go<T> {
        decide(&self.port, &self.call(address), role, schema, prompt)
            .await
            .map_err(|e| halt(address, e))
    }

    async fn ask_text(&self, address: &str, role: Role, prompt: &Prompt) -> Go<String> {
        self.port
            .text(&self.call(address), role, prompt)
            .await
            .map(|t| t.trim().to_owned())
            .map_err(|e| halt(address, e))
    }

    // ---------- the chapter ----------

    /// Chapter `n` from where it stands to its checkpoint; `Ok` when it is
    /// accepted already.
    async fn chapter(&mut self, n: u32) -> Go<()> {
        self.port.prefix = format!("ch{n}/");
        let goals = self.outline(n).await?;
        let ctx = self.context(n, goals).await?;
        let kept = self.scenes(n, &ctx).await?;
        let versions = self.db.chapter_versions(n).await?;
        if let Some(latest) = versions.last() {
            match self.checkpoint_of(n, latest.version).await? {
                Some((_, Some(answer))) if answer.decision == Decision::Approve => return Ok(()),
                Some((question, None)) => return Err(Halt::Pause(question)),
                Some((_, Some(answer))) => {
                    let notes = answer.notes;
                    let human = self
                        .revise(n, &ctx, &kept, latest, &[], Some(&notes))
                        .await?;
                    return Err(self.checkpoint(n, &ctx, &human).await?);
                }
                None if latest.origin == VersionOrigin::Human => {
                    return Err(self.checkpoint(n, &ctx, latest).await?);
                }
                None => {}
            }
        }
        let version = match versions.last() {
            Some(v) => v.clone(),
            None => self.draft(n, &ctx, &kept).await?,
        };
        let last = self.review_loop(n, &ctx, &kept, version).await?;
        Err(self.checkpoint(n, &ctx, &last).await?)
    }

    /// The chapter's goals, from the store or from the Author.
    async fn outline(&mut self, n: u32) -> Go<Vec<String>> {
        if let Some(outline) = self.db.outline(n).await? {
            return Ok(outline.goals);
        }
        let address = format!("ch{n}/outline");
        let ctx = self.context(n, Vec::new()).await?;
        let prompt = prompts::outline(&self.view(n, &ctx));
        let reply: OutlineReply = self
            .ask_json(
                &address,
                Role::Author,
                &OutlineReply::schema(&self.keys),
                &prompt,
            )
            .await?;
        let goals: Vec<String> = reply
            .goals
            .iter()
            .map(|g| g.trim().to_owned())
            .filter(|g| !g.is_empty())
            .collect();
        self.author
            .add_outline(
                n,
                &ChapterOutline {
                    goals: goals.clone(),
                },
            )
            .await?;
        self.steps.finish(&address, StepStatus::Done).await?;
        self.assume(&address, &reply.assumptions).await?;
        Ok(goals)
    }

    /// The trace of `branch`: its own events and reflections.
    async fn trace(&self, branch: &BranchId) -> Go<Trace> {
        let mut events = self.db.path(branch).await?;
        events.retain(|e| &e.branch == branch);
        let mut reflections = Vec::new();
        for c in &self.canon.characters {
            for r in self.db.reflections(branch, &c.key).await? {
                if &r.branch == branch {
                    reflections.push((c.key.clone(), r));
                }
            }
        }
        Ok(Trace {
            branch: branch.clone(),
            events,
            reflections,
        })
    }

    /// The world at the main line's tip.
    async fn world_now(&self) -> Go<WorldState> {
        match self.db.main_line_tip().await? {
            Some(tip) => Ok(self.db.world(&tip).await?),
            None => Ok(WorldState::initial(&self.canon)),
        }
    }

    /// Chapter `n`'s scenes, kept one by one (spec §7, steps 2–5).
    async fn scenes(&mut self, n: u32, ctx: &Context) -> Go<Vec<KeptScene>> {
        let mut kept: Vec<KeptScene> = Vec::new();
        for m in 1..=self.config.max_scenes {
            let planned = self.attempts(n, m).await?;
            if planned.is_empty() {
                let address = format!("ch{n}/sc{m}/plan");
                if self.steps.done(&address) {
                    break;
                }
                let Some(draft) = self.plan(n, m, ctx, &kept, None, &address).await? else {
                    self.steps.finish(&address, StepStatus::Done).await?;
                    break;
                };
                self.author
                    .add_scene((n, m, 0), &draft.setup(), draft.intent.trim())
                    .await?;
                self.steps.finish(&address, StepStatus::Done).await?;
            }
            let (stored, branch) = self.keep_scene(n, m, ctx, &kept).await?;
            let trace = self.trace(&branch).await?;
            kept.push(KeptScene { stored, trace });
        }
        Ok(kept)
    }

    /// Scene `m`'s attempts, by rerun.
    async fn attempts(&self, n: u32, m: u32) -> Go<Vec<StoredScene>> {
        let mut rows = self.db.scenes_of(n).await?;
        rows.retain(|s| s.scene == m);
        Ok(rows)
    }

    fn kept_view<'k>(kept: &'k [KeptScene]) -> Vec<Kept<'k>> {
        kept.iter()
            .map(|k| Kept {
                scene: k.stored.scene,
                setup: &k.stored.setup,
                intent: &k.stored.intent,
                trace: &k.trace,
            })
            .collect()
    }

    /// The Author's plan of scene `m`, held to the world: none when the
    /// chapter ends. A plan the world refuses is asked for once more, with
    /// the reason.
    async fn plan(
        &mut self,
        n: u32,
        m: u32,
        ctx: &Context,
        kept: &[KeptScene],
        notes: Option<&str>,
        address: &str,
    ) -> Go<Option<SceneDraft>> {
        let world = self.world_now().await?;
        let may_end = m > 1 && notes.is_none();
        let schema = PlanReply::schema_for(&self.keys, may_end);
        let mut prompt = prompts::plan(
            &self.view(n, ctx),
            &Self::kept_view(kept),
            &world,
            (m, self.config.max_scenes),
            notes,
        );
        let mut at = address.to_owned();
        for attempt in 0..2 {
            let reply: PlanReply = self.ask_json(&at, Role::Author, &schema, &prompt).await?;
            let Some(draft) = reply.next.draft() else {
                self.assume(address, &reply.assumptions).await?;
                return Ok(None);
            };
            match held_to_the_world(&world, &draft) {
                Ok(draft) => {
                    self.assume(address, &reply.assumptions).await?;
                    return Ok(Some(draft));
                }
                Err(reason) if attempt == 0 => {
                    prompt.user.push_str(&format!(
                        "\n\nThe world refuses that plan: {reason}. Plan the scene again."
                    ));
                    at = format!("{address}/again");
                }
                Err(reason) => {
                    return Err(Halt::Stop {
                        step: address.to_owned(),
                        why: Stop::Failed(format!("the plan was refused twice: {reason}")),
                    });
                }
            }
        }
        unreachable!("the loop returns on its second pass")
    }

    /// The P3 question about scene `m` of chapter `n`, the latest asked,
    /// with its answer.
    async fn scene_question(&self, n: u32, m: u32) -> Go<Option<(u32, Option<Resolution>)>> {
        Ok(self
            .db
            .questions()
            .await?
            .into_iter()
            .filter_map(|(q, a)| match q.body {
                QuestionBody::Scene {
                    chapter,
                    scene,
                    rerun,
                    ..
                } if chapter == n && scene == m => Some((rerun, a)),
                _ => None,
            })
            .next_back())
    }

    /// Scene `m` played until one of its branches is kept: the stored
    /// attempt and the branch.
    async fn keep_scene(
        &mut self,
        n: u32,
        m: u32,
        ctx: &Context,
        kept: &[KeptScene],
    ) -> Go<(StoredScene, BranchId)> {
        loop {
            let attempts = self.attempts(n, m).await?;
            let current = attempts
                .last()
                .cloned()
                .expect("a scene's first attempt is stored before it is kept");
            if let Some(pick) = self.db.pick_of(&current.id).await? {
                return match pick.branch {
                    Some(branch) => Ok((current, branch)),
                    None => Err(Halt::Store(StoryDbError::Corrupt(format!(
                        "scene {} asked for a rerun that was never stored",
                        current.id
                    )))),
                };
            }
            let asked = self.scene_question(n, m).await?;
            // The human answered P3 for this very attempt: plan it again.
            if let Some((rerun, Some(answer))) = &asked
                && *rerun == current.rerun
            {
                let address = format!("{}/replan", current.id);
                let notes = Some(answer.notes.as_str()).filter(|s| !s.trim().is_empty());
                let notes = notes.unwrap_or("Try the scene again.");
                let draft = self
                    .plan(n, m, ctx, kept, Some(notes), &address)
                    .await?
                    .expect("a plan asked again has no end");
                self.next_attempt(&current, &draft, "the human asked for the scene again")
                    .await?;
                continue;
            }
            let since = asked.map_or(0, |(rerun, _)| rerun + 1);
            let attempt = current.rerun + 1 - since;
            let most = 1 + self.config.max_reruns;
            let address = current.id.to_string();
            let tip = match self.db.main_line_tip().await? {
                Some(tip) => tip,
                None => BranchId::genesis(),
            };
            let outcomes = play_scene(
                self.db,
                &self.port,
                &self.steps,
                (&current.id, &current.setup),
                &tip,
                &self.config.sim,
            )
            .await
            .map_err(|e| scene_halt(&address, e))?;
            let played: Vec<&BranchId> = outcomes
                .iter()
                .filter(|o| o.failed.is_none())
                .map(|o| &o.branch)
                .collect();
            if played.is_empty() {
                let errors: Vec<String> = outcomes
                    .iter()
                    .map(|o| format!("{}: {}", o.branch, o.failed.as_deref().unwrap_or_default()))
                    .collect();
                if attempt < most {
                    let draft = SceneDraft {
                        intent: current.intent.clone(),
                        location: current.setup.location.clone(),
                        present: current.setup.present.clone(),
                        just_happened: current.setup.just_happened.clone(),
                        pressure: current.setup.pressure.clone(),
                    };
                    self.next_attempt(&current, &draft, "every branch failed")
                        .await?;
                    continue;
                }
                let body = QuestionBody::Scene {
                    chapter: n,
                    scene: m,
                    rerun: current.rerun,
                    errors,
                };
                return Err(self.ask(&format!("{address}/failed"), body).await?);
            }
            let played: Vec<BranchId> = played.into_iter().cloned().collect();
            let reruns_left = most.saturating_sub(attempt);
            match self.pick(n, ctx, &current, &played, reruns_left).await? {
                Picked::Branch(branch, note) => {
                    self.author
                        .record_pick(&current.id, Some(&branch), &note)
                        .await?;
                    self.steps
                        .finish(&format!("{address}/pick"), StepStatus::Done)
                        .await?;
                    return Ok((current, branch));
                }
                Picked::Rerun(draft, note) => {
                    self.next_attempt(&current, &draft, &note).await?;
                    self.steps
                        .finish(&format!("{address}/pick"), StepStatus::Done)
                        .await?;
                }
            }
        }
    }

    /// Stores the scene's next attempt, then its current attempt's pick of
    /// none: in that order, so a run killed between finds the next attempt.
    async fn next_attempt(
        &mut self,
        current: &StoredScene,
        draft: &SceneDraft,
        why: &str,
    ) -> Go<()> {
        self.author
            .add_scene(
                (current.chapter, current.scene, current.rerun + 1),
                &draft.setup(),
                draft.intent.trim(),
            )
            .await?;
        self.author.record_pick(&current.id, None, why).await?;
        Ok(())
    }

    /// The Author's pick among the branches that played.
    async fn pick(
        &mut self,
        n: u32,
        ctx: &Context,
        current: &StoredScene,
        played: &[BranchId],
        reruns_left: u32,
    ) -> Go<Picked> {
        let address = format!("{}/pick", current.id);
        let mut traces = Vec::with_capacity(played.len());
        for branch in played {
            traces.push((label_of(branch), self.trace(branch).await?));
        }
        let labels: Vec<String> = traces.iter().map(|(l, _)| l.clone()).collect();
        let shown: Vec<(String, &Trace)> = traces.iter().map(|(l, t)| (l.clone(), t)).collect();
        let prompt = prompts::pick(
            &self.view(n, ctx),
            (current.scene, &current.setup, &current.intent),
            &shown,
            reruns_left,
        );
        let schema = PickReply::schema_for(&self.keys, &labels, reruns_left > 0);
        let reply: PickReply = self
            .ask_json(&address, Role::Author, &schema, &prompt)
            .await?;
        self.assume(&address, &reply.assumptions).await?;
        let note = reply.human_note.trim().to_owned();
        let label = match &reply.choice {
            PickChoice::Branch { branch } => branch,
            PickChoice::Rerun { .. } => {
                let rerun = reply.choice.rerun().expect("a rerun carries its scene");
                let world = self.world_now().await?;
                // A rerun with no one alive present keeps the scene's cast.
                let draft = held_to_the_world(&world, &rerun).unwrap_or_else(|_| SceneDraft {
                    present: current.setup.present.clone(),
                    ..rerun
                });
                return Ok(Picked::Rerun(draft, note));
            }
        };
        let branch = played
            .iter()
            .find(|b| &label_of(b) == label)
            .cloned()
            .expect("the schema names only the branches shown");
        Ok(Picked::Branch(branch, note))
    }

    // ---------- the text ----------

    async fn draft(&mut self, n: u32, ctx: &Context, kept: &[KeptScene]) -> Go<ChapterVersion> {
        let address = format!("ch{n}/v1/write");
        let prompt = prompts::write(
            &self.view(n, ctx),
            &Self::kept_view(kept),
            self.config.words,
        );
        let text = self.ask_text(&address, Role::Author, &prompt).await?;
        let version = self
            .author
            .add_chapter(n, VersionOrigin::Draft, &text)
            .await?;
        self.steps.finish(&address, StepStatus::Done).await?;
        Ok(version)
    }

    /// The next version of chapter `n`: from the accepted notes, or with the
    /// human's notes at the checkpoint.
    async fn revise(
        &mut self,
        n: u32,
        ctx: &Context,
        kept: &[KeptScene],
        current: &ChapterVersion,
        accepted: &[(&ReviewNote, &Decided)],
        human: Option<&str>,
    ) -> Go<ChapterVersion> {
        let address = format!("ch{n}/v{}/revise", current.version + 1);
        let mut view = self.view(n, ctx);
        if human.is_some() {
            view.human_notes = human;
        }
        let prompt = prompts::revise(
            &view,
            &Self::kept_view(kept),
            &current.text,
            accepted,
            self.config.words,
        );
        let text = self.ask_text(&address, Role::Author, &prompt).await?;
        let origin = if human.is_some() {
            VersionOrigin::Human
        } else {
            VersionOrigin::Revision
        };
        let version = self.author.add_chapter(n, origin, &text).await?;
        self.steps.finish(&address, StepStatus::Done).await?;
        Ok(version)
    }

    /// Reviews and revisions until no accepted high-severity note stands, at
    /// most `max_revisions` times: the version that ends the loop.
    async fn review_loop(
        &mut self,
        n: u32,
        ctx: &Context,
        kept: &[KeptScene],
        mut version: ChapterVersion,
    ) -> Go<ChapterVersion> {
        loop {
            if version.origin == VersionOrigin::Human || version.version > self.config.max_revisions
            {
                return Ok(version);
            }
            let notes = self.review(n, kept, &version).await?;
            let triaged: Vec<&ReviewNote> =
                notes.iter().filter(|n| n.note.kind.is_triaged()).collect();
            if triaged.is_empty() {
                return Ok(version);
            }
            let decisions = self.triage(n, ctx, &version, &triaged).await?;
            let accepted: Vec<(&ReviewNote, &Decided)> = triaged
                .iter()
                .filter_map(|note| {
                    decisions
                        .iter()
                        .find(|d| d.note == note.id && d.decision == NoteDecision::Accepted)
                        .map(|d| (*note, d))
                })
                .collect();
            if !accepted
                .iter()
                .any(|(n, _)| n.note.severity == NoteSeverity::High)
            {
                return Ok(version);
            }
            version = self.revise(n, ctx, kept, &version, &accepted, None).await?;
        }
    }

    /// The version's notes: both reviews, each written once (spec §7, step
    /// 7).
    async fn review(
        &mut self,
        n: u32,
        kept: &[KeptScene],
        version: &ChapterVersion,
    ) -> Go<Vec<ReviewNote>> {
        let notes = self.db.notes(&version.id).await?;
        let reader_at = format!("{}/reader", version.id);
        let checker_at = format!("{}/continuity", version.id);
        let need = |reviewer: Reviewer, at: &str| {
            !notes.iter().any(|n| n.reviewer == reviewer) && !self.steps.done(at)
        };
        let (need_reader, need_checker) = (
            need(Reviewer::Reader, &reader_at),
            need(Reviewer::Continuity, &checker_at),
        );
        if !need_reader && !need_checker {
            return Ok(notes);
        }
        let published = self.published(n).await?;
        let opened = match kept.first() {
            Some(k) => self.world_before(&k.trace.branch).await?,
            None => self.world_now().await?,
        };
        let reader_prompt = prompts::reader(&published, &version.text);
        let checker_prompt =
            prompts::continuity(&self.canon, &opened, &Self::kept_view(kept), &version.text);
        let reader_schema = ReaderReply::schema(&self.keys);
        let checker_schema = ContinuityReply::schema(&self.keys);
        let reader = async {
            if need_reader {
                Some(
                    self.ask_json::<ReaderReply>(
                        &reader_at,
                        Role::Reader,
                        &reader_schema,
                        &reader_prompt,
                    )
                    .await,
                )
            } else {
                None
            }
        };
        let checker = async {
            if need_checker {
                Some(
                    self.ask_json::<ContinuityReply>(
                        &checker_at,
                        Role::Continuity,
                        &checker_schema,
                        &checker_prompt,
                    )
                    .await,
                )
            } else {
                None
            }
        };
        let (read, checked) = join(reader, checker).await;
        if let Some(reply) = read {
            let written = reader_notes(reply?);
            self.db
                .review_writer(&version.id, Reviewer::Reader)
                .await?
                .write(&written)
                .await?;
            self.steps.finish(&reader_at, StepStatus::Done).await?;
        }
        if let Some(reply) = checked {
            let known: Vec<&EventId> = kept
                .iter()
                .flat_map(|k| k.trace.events.iter().map(|e| &e.id))
                .collect();
            let mut written = markers_in(&self.canon, &version.text);
            written.extend(checker_notes(reply?, &known));
            self.db
                .review_writer(&version.id, Reviewer::Continuity)
                .await?
                .write(&written)
                .await?;
            self.steps.finish(&checker_at, StepStatus::Done).await?;
        }
        Ok(self.db.notes(&version.id).await?)
    }

    /// The accepted versions of the chapters before `n`, as published.
    async fn published(&self, n: u32) -> Go<Vec<(u32, String)>> {
        let mut out = Vec::new();
        for k in 1..n {
            if let Some(v) = self.accepted_version(k).await? {
                out.push((k, v.text));
            }
        }
        Ok(out)
    }

    /// The world as `branch` forked from its parent.
    async fn world_before(&self, branch: &BranchId) -> Go<WorldState> {
        match self.db.branch(branch).await?.and_then(|b| b.parent) {
            Some(parent) => Ok(self.db.world(&parent).await?),
            None => Ok(WorldState::initial(&self.canon)),
        }
    }

    /// The Author's decisions on the triaged notes, stored once (spec §7,
    /// step 8). A note the reply leaves out is deferred.
    async fn triage(
        &mut self,
        n: u32,
        ctx: &Context,
        version: &ChapterVersion,
        triaged: &[&ReviewNote],
    ) -> Go<Vec<Decided>> {
        let stored = self.db.decisions(&version.id).await?;
        if !stored.is_empty() {
            return Ok(stored);
        }
        let address = format!("{}/triage", version.id);
        let shown: Vec<(String, &ReviewNote)> = triaged
            .iter()
            .enumerate()
            .map(|(i, note)| (format!("n{}", i + 1), *note))
            .collect();
        let labels: Vec<String> = shown.iter().map(|(l, _)| l.clone()).collect();
        let prompt = prompts::triage(&self.view(n, ctx), &version.text, &shown);
        let schema = TriageReply::schema_for(&self.keys, &labels);
        let reply: TriageReply = self
            .ask_json(&address, Role::Author, &schema, &prompt)
            .await?;
        let decisions = decisions_of(&shown, &reply);
        self.author
            .record_decisions(&version.id, &decisions)
            .await?;
        self.steps.finish(&address, StepStatus::Done).await?;
        Ok(decisions)
    }

    /// The version's summary, then P2.
    async fn checkpoint(&mut self, n: u32, ctx: &Context, version: &ChapterVersion) -> Go<Halt> {
        if self.db.summary(&version.id).await?.is_none() {
            let address = format!("{}/summary", version.id);
            let prompt = prompts::summary(&self.view(n, ctx), &version.text);
            let summary = self.ask_text(&address, Role::Author, &prompt).await?;
            self.author.add_summary(&version.id, &summary).await?;
            self.steps.finish(&address, StepStatus::Done).await?;
        }
        let body = QuestionBody::Chapter {
            chapter: n,
            version: version.version,
        };
        self.ask(&format!("{}/checkpoint", version.id), body).await
    }
}

/// What the Author's pick came to.
enum Picked {
    /// This branch, with the note for the human.
    Branch(BranchId, String),
    /// A rerun of this scene, with the note.
    Rerun(SceneDraft, String),
}

/// The label of a branch the pick names: `k1`.
fn label_of(branch: &BranchId) -> String {
    branch
        .as_str()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A planned scene held to the world: every one present alive, named once,
/// and someone present.
pub(crate) fn held_to_the_world(
    world: &WorldState,
    draft: &SceneDraft,
) -> Result<SceneDraft, String> {
    let mut present = Vec::with_capacity(draft.present.len());
    for who in &draft.present {
        if world.is_alive(who) && !present.contains(who) {
            present.push(who.clone());
        }
    }
    if present.is_empty() {
        return Err("no one alive is present".to_owned());
    }
    Ok(SceneDraft {
        present,
        intent: draft.intent.trim().to_owned(),
        just_happened: draft.just_happened.trim().to_owned(),
        pressure: draft.pressure.trim().to_owned(),
        location: draft.location.clone(),
    })
}

/// The Reader's reply as notes: confusions and slow passages for the triage,
/// predictions and the best moment kept beside them.
pub(crate) fn reader_notes(reply: ReaderReply) -> Vec<NewNote> {
    let remark = |kind, r: ReaderRemark| NewNote {
        kind,
        severity: r.severity,
        anchor: Some(r.anchor.trim().to_owned()).filter(|a| !a.is_empty()),
        body: r.note.trim().to_owned(),
        evidence: Vec::new(),
    };
    let observation = |kind, body: String| NewNote {
        kind,
        severity: NoteSeverity::Low,
        anchor: None,
        body,
        evidence: Vec::new(),
    };
    let mut notes: Vec<NewNote> = reply
        .confusions
        .into_iter()
        .map(|r| remark(NoteKind::Confusion, r))
        .chain(
            reply
                .slow_passages
                .into_iter()
                .map(|r| remark(NoteKind::SlowPassage, r)),
        )
        .collect();
    notes.extend(
        reply
            .predictions
            .into_iter()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .map(|p| observation(NoteKind::Prediction, p)),
    );
    let best = reply.best_moment.trim().to_owned();
    if !best.is_empty() {
        notes.push(observation(NoteKind::BestMoment, best));
    }
    notes.retain(|n| !n.body.is_empty());
    notes
}

/// The Continuity checker's findings as notes, the evidence held to the
/// events of the kept scenes: an id the record does not have is dropped.
pub(crate) fn checker_notes(reply: ContinuityReply, known: &[&EventId]) -> Vec<NewNote> {
    reply
        .findings
        .into_iter()
        .filter(|f| !f.note.trim().is_empty())
        .map(|f| NewNote {
            kind: f.kind.note_kind(),
            severity: f.severity,
            anchor: Some(f.anchor.trim().to_owned()).filter(|a| !a.is_empty()),
            body: f.note.trim().to_owned(),
            evidence: f
                .evidence
                .iter()
                .map(|e| e.trim().trim_matches('`'))
                .filter_map(|e| known.iter().find(|k| k.as_str() == e).map(|k| (*k).clone()))
                .collect(),
        })
        .collect()
}

/// The engine's own check: a planted secret's marker in the prose is a
/// high-severity note, whatever the checker says (spec §8, deterministic
/// checks first).
pub(crate) fn markers_in(canon: &Canon, text: &str) -> Vec<NewNote> {
    leaks::secrets_in(canon, text)
        .into_iter()
        .map(|secret| NewNote {
            kind: NoteKind::Token,
            severity: NoteSeverity::High,
            anchor: canon
                .characters
                .iter()
                .flat_map(|c| &c.secrets)
                .find(|s| &s.key == secret)
                .and_then(|s| leaks::token_of(&s.text))
                .map(str::to_owned),
            body: format!(
                "The marker of the secret `{secret}` is in the prose; it is the record's, not \
                 the story's. Take it out."
            ),
            evidence: Vec::new(),
        })
        .collect()
}

/// The triage reply as decisions on the notes shown: a note left out is
/// deferred, a second decision on one is ignored.
pub(crate) fn decisions_of(shown: &[(String, &ReviewNote)], reply: &TriageReply) -> Vec<Decided> {
    let mut by_label: BTreeMap<&str, (NoteDecision, String)> = BTreeMap::new();
    for entry in &reply.decisions {
        by_label
            .entry(entry.note.as_str())
            .or_insert((entry.decision, entry.critique.trim().to_owned()));
    }
    shown
        .iter()
        .map(|(label, note)| {
            let (decision, reason) = by_label.get(label.as_str()).cloned().unwrap_or((
                NoteDecision::Deferred,
                "the Author did not decide on it".to_owned(),
            ));
            Decided {
                note: note.id.clone(),
                decision,
                reason,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "chapter_tests.rs"]
mod tests;
