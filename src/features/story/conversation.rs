//! A story as the chat drives it (docs/research/fabula-engine.md §14, *The
//! chat tools and the human in the loop*; docs/fabula.md §4, PR 9): what
//! `story_create` and `story_continue` do to the story's files, worded by the
//! tools (`features::tools::story`) and started by the orchestrator.
//!
//! - **Creating** stores the canon as a draft and opens P1 at once, with no
//!   model call: the canon is not approved yet, so the engine's first step
//!   asks for its approval. A draft is revised by creating it again under its
//!   id, until the canon is approved.
//! - **Continuing** answers the pending question from the person's own rows
//!   ([`super::answer`]) and gives the run its story; with no question
//!   pending it resumes a run that stopped short — on a turn that answers the
//!   person, never on one the app woke (T14). A turn that answers no
//!   message of the person's, or rows that read as unclear, resolve nothing.
//!   At a chapter's checkpoint the reading is shown the chapter's scenes by
//!   number, so a reply may ask for one to be played again (PR 10).
//!
//! Both claim the story on the board first, as a run does, so a chat tool
//! and a run never hold one story's files at once.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::answer::{ReadingReply, UserRow, read_reply, resolution};
use super::author::{
    ContinuityReply, OutlineReply, PickReply, PlanReply, ReaderReply, TriageReply,
};
use super::board::{RunView, StoryBoard};
use super::chapter::{self, Budget, Outcome};
use super::contracts::{InitiativeRuling, ReactionsRuling, ReflectionReply};
use super::dialect::{self, DialectProblem};
use super::engine::{EngineAdapter, SharedLog};
use super::port::{CallAddress, LlmPort, PortError, Prompt, Role};
use super::schema::{CanonKeys, Property, Schema, StorySchema};
use super::single::SingleConfig;
use super::single_story::{self, Story};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::entities::story::{Canon, Decision, Proposal, Question, QuestionBody};
use crate::shared::api::contract::EngineBackend;
use crate::shared::session_budget::SessionBudget;
use crate::shared::storage::story::{KeptPick, StoryDb, StoryDbError};

/// A story's two files: its database and its call log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryFiles {
    /// `stories/<chat-id>/<story-id>.db`.
    pub db: PathBuf,
    /// `story-logs/<chat-id>/<story-id>.db`.
    pub log: PathBuf,
}

/// The longest story id, in characters.
const MAX_ID_CHARS: usize = 40;

/// Names Windows keeps for devices, which no file may take.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The id a story titled `title` is given: its words in lower case, joined by
/// `-`, letters and digits of any script kept and everything else dropped, at
/// most 40 characters; `story` when nothing is left. A taken id gets `-2`,
/// `-3`… It names the story's files, so it is safe as a file name everywhere.
pub fn story_id(title: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut slug = String::new();
    for c in title.trim().chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if (c.is_whitespace() || c == '-' || c == '_')
            && !slug.is_empty()
            && !slug.ends_with('-')
        {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(MAX_ID_CHARS).collect();
    let slug = slug.trim_end_matches('-');
    let base = match slug {
        "" => "story".to_owned(),
        s if RESERVED.contains(&s) => format!("{s}-story"),
        s => s.to_owned(),
    };
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("some suffix is free")
}

/// Every rule of the schema dialect the replies' schemas break under
/// `canon`'s keys (spec §8, *One schema dialect*): the canon a model composes
/// is held to the same rules as the engine's own schemas, so a key no provider
/// would compile is refused before a run meets it.
pub fn dialect_problems(canon: &Canon) -> Vec<DialectProblem> {
    let keys = CanonKeys::of(canon);
    let branches: Vec<String> = (0..3).map(|k| format!("k{k}")).collect();
    let notes: Vec<String> = (1..=3).map(|n| format!("n{n}")).collect();
    let schemas = [
        Proposal::schema(&keys),
        ReflectionReply::schema(&keys),
        InitiativeRuling::schema(&keys),
        ReactionsRuling::schema(&keys),
        OutlineReply::schema(&keys),
        PlanReply::schema_for(&keys, true),
        PlanReply::schema_for(&keys, false),
        PickReply::schema_for(&keys, &branches, true),
        PickReply::schema_for(&keys, &branches, false),
        ReaderReply::schema(&keys),
        ContinuityReply::schema(&keys),
        TriageReply::schema_for(&keys, &notes),
    ];
    let mut found = Vec::new();
    for schema in schemas {
        // A union is never a root (spec §8): one is checked as a property.
        let root = match schema {
            Schema::Object(_) => schema,
            other => Schema::Object(vec![Property::new("value", other)]),
        };
        for problem in dialect::check(&root.to_value()) {
            if !found.contains(&problem) {
                found.push(problem);
            }
        }
    }
    found
}

/// Why `story_create` stored nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateRefusal {
    /// The canon cannot be used: every problem, in the engine's English.
    Problems(Vec<String>),
    /// A revision of a story the chat does not have.
    Unknown,
    /// A revision of a canon already approved: its changes go through a
    /// chapter's checkpoint now.
    Approved,
    /// A run is writing the story.
    Busy(RunView),
    /// The store failed.
    Failed(String),
}

impl From<StoryDbError> for CreateRefusal {
    fn from(e: StoryDbError) -> Self {
        match e {
            StoryDbError::Canon(problems) => {
                CreateRefusal::Problems(problems.iter().map(ToString::to_string).collect())
            }
            StoryDbError::CanonFrozen => CreateRefusal::Approved,
            other => CreateRefusal::Failed(other.to_string()),
        }
    }
}

/// A port that makes no call: a story's creation reaches P1 before any.
struct NoCalls;

impl NoCalls {
    fn refuse(call: &CallAddress) -> PortError {
        PortError::Failed {
            address: call.address.clone(),
            message: "no model call is made while a story is created".to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl LlmPort for NoCalls {
    async fn json(
        &self,
        call: &CallAddress,
        _: Role,
        _: &Schema,
        _: &Prompt,
    ) -> Result<Value, PortError> {
        Err(Self::refuse(call))
    }

    async fn text(&self, call: &CallAddress, _: Role, _: &Prompt) -> Result<String, PortError> {
        Err(Self::refuse(call))
    }
}

/// Stores `canon` as the draft of story `story` of chat `chat` — a new story,
/// or with `revise` the draft of one the chat has — and returns the P1
/// question that waits for the person's approval. Nothing is stored when the
/// canon has a problem.
pub async fn create(
    files: &StoryFiles,
    board: &StoryBoard,
    (chat, story): (Uuid, &str),
    canon: &Canon,
    revise: bool,
) -> Result<Question, CreateRefusal> {
    // The dialect is checked only for a canon that passes its own checks: a
    // repeated key breaks the schemas built on it too, and the model needs
    // the cause, not each schema's echo of it.
    let mut problems: Vec<String> = canon.check().iter().map(ToString::to_string).collect();
    if problems.is_empty() {
        problems.extend(dialect_problems(canon).iter().map(ToString::to_string));
    }
    if !problems.is_empty() {
        return Err(CreateRefusal::Problems(problems));
    }
    if revise != files.db.is_file() {
        return Err(if revise {
            CreateRefusal::Unknown
        } else {
            CreateRefusal::Failed(format!("story {story} exists already"))
        });
    }
    let claim = Uuid::new_v4();
    board
        .claim(chat, story, claim)
        .map_err(CreateRefusal::Busy)?;
    let created = create_claimed(files, canon, revise).await;
    board.release(claim);
    created
}

async fn create_claimed(
    files: &StoryFiles,
    canon: &Canon,
    revise: bool,
) -> Result<Question, CreateRefusal> {
    for dir in [files.db.parent(), files.log.parent()]
        .into_iter()
        .flatten()
    {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| CreateRefusal::Failed(e.to_string()))?;
    }
    let db = StoryDb::open(&files.db, &files.log).await?;
    let opened = open_p1(&db, canon, revise).await;
    db.close().await?;
    opened
}

async fn open_p1(db: &StoryDb, canon: &Canon, revise: bool) -> Result<Question, CreateRefusal> {
    if revise && db.canon_approved().await? {
        return Err(CreateRefusal::Approved);
    }
    let mut author = db.author_writer().await?;
    author.import_canon(canon).await?;
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await?));
    let outcome = single_story::advance(Story {
        db,
        author: &mut author,
        log: &log,
        port: &NoCalls,
        config: &SingleConfig::default(),
    })
    .await
    .map_err(|e| CreateRefusal::Failed(e.to_string()))?;
    match outcome {
        Outcome::Paused(question) => Ok(question),
        other => Err(CreateRefusal::Failed(format!(
            "the story's first step did not ask for the canon's approval: {other:?}"
        ))),
    }
}

/// The person's rows after a pending question, as the turn found them when it
/// began (spec §14): what `story_continue` may read as the answer, and what
/// the request's reminder line names.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAnswer {
    /// The story.
    pub story: String,
    /// The question waiting.
    pub question: Question,
    /// The person's messages since it was put to them, oldest first; never a
    /// notification, an attachment or a tool's result.
    pub rows: Vec<UserRow>,
}

/// What `story_continue` was asked with, beside the story.
pub struct Answering<'a> {
    /// The turn's snapshot of the person's rows for the question that waits,
    /// when it holds one ([`PendingAnswer`]).
    pub pending: Option<&'a PendingAnswer>,
    /// The turn answers a message of the person's: false on a turn the app
    /// woke on a notification (`UserMessageTimes::answering` empty).
    pub answering: bool,
    /// The assistant's own notes, recorded as its own.
    pub notes: Option<&'a str>,
}

/// The model the person's reply is read with: the chat's own, under the
/// app's session budget.
pub struct Listener {
    /// The chat's backend.
    pub backend: Arc<dyn EngineBackend>,
    /// The model's name, as the call log records it.
    pub model: String,
    /// The app's session budget.
    pub sessions: Option<Arc<SessionBudget>>,
    /// The turn's token: `Esc` cancels the reading.
    pub cancel: CancellationToken,
    /// Never ask the engine to constrain the decoding.
    pub fallback_only: bool,
}

/// The reading's sampling: a short judgement, thinking at a low effort.
pub fn listener_sampling() -> SamplingConfig {
    SamplingConfig {
        temperature: Some(0.2),
        thinking: Some(true),
        reasoning_effort: Some(ReasoningEffort::Low),
        max_tokens: Some(8_192),
        ..SamplingConfig::default()
    }
}

/// What the person decided, as the run goes on with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The question answered.
    pub body: QuestionBody,
    /// The decision.
    pub decision: Decision,
    /// At P2, the scene asked to be played again with the ones after it.
    pub rerun: Option<u32>,
}

/// What `story_continue` settled.
#[derive(Debug, Clone, PartialEq)]
pub enum Continued {
    /// The run may start: with what the person decided, or, with no question
    /// pending, to resume where the last run stopped. The story stays claimed
    /// for the run.
    Start(Option<Resolved>),
    /// The person asked for the canon to change: recorded, and no run starts —
    /// the assistant revises the draft with `story_create` and asks again.
    CanonChange(String),
    /// The person accepted the last chapter: the story is finished, and a run
    /// would only find that out (found in G2, PR 9).
    Finished(u32),
}

/// Why `story_continue` started nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum ContinueRefusal {
    /// The chapter the run would write has spent its budget: a run would
    /// stop at its first call (found live: G2 restarted seven times at a
    /// spent budget, docs/journal/stories.md).
    BudgetSpent {
        /// The chapter.
        chapter: u32,
        /// What it spent, against what limit, in words.
        spent: String,
    },
    /// The chat has no such story.
    Unknown,
    /// A run is writing the story.
    Busy(RunView),
    /// Every chapter of the arc is accepted.
    Finished,
    /// A question waits and the person has not answered it: no message of
    /// theirs since it was put, or a turn that answers none.
    NotAnswered,
    /// No question waits, and the turn answers no message of the person's:
    /// one the app woke on a run's notification may not resume the run,
    /// since whether a stopped story goes on is the person's to say (the
    /// owner's decision, docs/fabula.md §6, T14).
    NotAsked,
    /// Their messages read as no decision.
    Unclear(ReadingReply),
    /// The reading's call failed.
    Failed(String),
}

impl From<StoryDbError> for ContinueRefusal {
    fn from(e: StoryDbError) -> Self {
        ContinueRefusal::Failed(e.to_string())
    }
}

/// Settles story `story` of chat `chat` for its next run, `run`: the pending
/// question answered from the person's rows, or nothing pending. On
/// [`Continued::Start`] the story stays claimed for `run`, which the run's own
/// claim then finds; otherwise the claim is given back.
pub async fn prepare(
    files: &StoryFiles,
    board: &StoryBoard,
    (chat, story, run): (Uuid, &str, Uuid),
    answering: Answering<'_>,
    listener: Listener,
    budget: Budget,
) -> Result<Continued, ContinueRefusal> {
    if !files.db.is_file() {
        return Err(ContinueRefusal::Unknown);
    }
    board
        .claim(chat, story, run)
        .map_err(ContinueRefusal::Busy)?;
    let prepared = prepare_claimed(files, answering, listener, budget).await;
    if !matches!(prepared, Ok(Continued::Start(_))) {
        board.release(run);
    }
    prepared
}

async fn prepare_claimed(
    files: &StoryFiles,
    answering: Answering<'_>,
    listener: Listener,
    budget: Budget,
) -> Result<Continued, ContinueRefusal> {
    let db = StoryDb::open(&files.db, &files.log).await?;
    let prepared = match settle(&db, answering, listener).await {
        Ok(Continued::Start(resolved)) => room_for(&db, budget)
            .await
            .map(|()| Continued::Start(resolved)),
        other => other,
    };
    db.close().await?;
    prepared
}

/// Whether the chapter the next run writes has budget left: once it is
/// spent, the run would stop before its first call. An answer just recorded
/// stays: the run that follows a raised budget acts on it.
async fn room_for(db: &StoryDb, budget: Budget) -> Result<(), ContinueRefusal> {
    if budget == Budget::default() {
        return Ok(());
    }
    let Some(canon) = db.canon().await? else {
        return Ok(());
    };
    let Some(chapter) = open_chapter(db, &canon).await? else {
        return Ok(());
    };
    // A take's budget is its own (docs/fabula.md §6, T12).
    let spend = db.chapter_spend(chapter).await?;
    match budget.spent(&spend) {
        Some(spent) => Err(ContinueRefusal::BudgetSpent { chapter, spent }),
        None => Ok(()),
    }
}

async fn settle(
    db: &StoryDb,
    answering: Answering<'_>,
    listener: Listener,
) -> Result<Continued, ContinueRefusal> {
    let canon = db.canon().await?.ok_or(ContinueRefusal::Unknown)?;
    let Some(question) = db.pending_question().await? else {
        return if open_chapter(db, &canon).await?.is_none() {
            Err(ContinueRefusal::Finished)
        } else if !answering.answering {
            Err(ContinueRefusal::NotAsked)
        } else {
            Ok(Continued::Start(None))
        };
    };
    let rows = match answering.pending {
        Some(p) if answering.answering && p.question.id == question.id && !p.rows.is_empty() => {
            &p.rows
        }
        _ => return Err(ContinueRefusal::NotAnswered),
    };
    let kept = match question.body {
        QuestionBody::Chapter { chapter, .. } => db.kept_scenes(chapter).await?,
        _ => Vec::new(),
    };
    let scenes = u32::try_from(kept.len()).unwrap_or(u32::MAX);
    let shown = shown(db, &canon, &question.body, &kept).await?;
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await?));
    let reading_run = log
        .lock()
        .await
        .start_run(&json!({"engine": "listener", "question": question.id.as_str()}))
        .await?;
    let mut adapter = EngineAdapter::sharing(
        listener.backend,
        log.clone(),
        listener.model,
        listener_sampling(),
        listener.cancel,
    );
    if let Some(sessions) = listener.sessions {
        adapter = adapter.with_sessions(sessions);
    }
    if listener.fallback_only {
        adapter = adapter.fallback_only();
    }
    let call = CallAddress {
        run: reading_run,
        address: format!("{}/read", question.id),
    };
    let reply = read_reply(&adapter, &call, (&question.body, scenes), &shown, rows)
        .await
        .map_err(|e| ContinueRefusal::Failed(e.to_string()))?;
    let Some(resolved) = resolution((&question.body, scenes), &reply, rows, answering.notes) else {
        return Err(ContinueRefusal::Unclear(reply));
    };
    let mut author = db.author_writer().await?;
    chapter::resolve(&mut author, &question.id, &resolved).await?;
    if question.body == QuestionBody::Canon && resolved.decision == Decision::Change {
        return Ok(Continued::CanonChange(resolved.notes));
    }
    if let QuestionBody::Chapter { chapter, .. } = question.body
        && resolved.decision == Decision::Approve
        && open_chapter(db, &canon).await?.is_none()
    {
        return Ok(Continued::Finished(chapter));
    }
    Ok(Continued::Start(Some(Resolved {
        body: question.body,
        decision: resolved.decision,
        rerun: resolved.rerun,
    })))
}

/// The first chapter of the arc with no accepted version; `None` once the
/// story is finished.
async fn open_chapter(db: &StoryDb, canon: &Canon) -> Result<Option<u32>, StoryDbError> {
    for n in 1..=u32::try_from(canon.arc.chapters.len()).unwrap_or(u32::MAX) {
        let mut accepted = false;
        for version in db.chapter_versions(n).await? {
            accepted |= db.accepted(&version.id).await?;
        }
        if !accepted {
            return Ok(Some(n));
        }
    }
    Ok(None)
}

/// What the person was shown with the question, as the reading quotes it:
/// the canon in brief at P1, the chapter's summary and its `kept` scenes by
/// number at P2; P3's question carries its own.
async fn shown(
    db: &StoryDb,
    canon: &Canon,
    body: &QuestionBody,
    kept: &[KeptPick],
) -> Result<String, StoryDbError> {
    let mut text = shown_head(db, canon, body).await?;
    if !kept.is_empty() {
        text.push_str("\n\n");
        text.push_str(&scenes_brief(canon, kept));
    }
    Ok(text)
}

/// The chapter's scenes as the person was shown them with its question: by
/// number, where and with whom, and the Author's note on keeping each, in
/// the person's language. A chapter keeps its scenes numbered from 1 with no
/// gap, so the numbers are the labels the reading may name.
pub fn scenes_brief(canon: &Canon, kept: &[KeptPick]) -> String {
    let mut out = "The chapter's scenes, by number:".to_owned();
    for k in kept {
        let place = canon
            .location(&k.scene.setup.location)
            .map_or(k.scene.setup.location.as_str(), |l| l.name.trim());
        let names: Vec<&str> = k
            .scene
            .setup
            .present
            .iter()
            .map(|key| match canon.character(key) {
                Some(c) if !c.name.trim().is_empty() => c.name.trim(),
                _ => key.as_str(),
            })
            .collect();
        out.push_str(&format!(
            "\n{}. At {place}, with {}.",
            k.scene.scene,
            names.join(", ")
        ));
        let note = k.rationale.trim();
        if !note.is_empty() {
            out.push(' ');
            out.push_str(note);
        }
    }
    out
}

async fn shown_head(
    db: &StoryDb,
    canon: &Canon,
    body: &QuestionBody,
) -> Result<String, StoryDbError> {
    Ok(match body {
        QuestionBody::Canon => canon_brief(canon),
        QuestionBody::Chapter { chapter, version } => {
            let summary = match db
                .chapter_versions(*chapter)
                .await?
                .into_iter()
                .find(|v| v.version == *version)
            {
                Some(v) => db.summary(&v.id).await?,
                None => None,
            };
            match summary {
                Some(summary) => format!(
                    "Chapter {chapter}, version {version}, attached to the chat. Its summary: \
                     {summary}"
                ),
                None => format!("Chapter {chapter}, version {version}, attached to the chat."),
            }
        }
        QuestionBody::Scene { .. } => "The failures named in the question.".to_owned(),
    })
}

/// The canon as the reading is told it: enough to know what "the second
/// chapter" or "Mara's secret" is.
pub fn canon_brief(canon: &Canon) -> String {
    let mut out = format!("Premise: {}", canon.arc.premise.trim());
    out.push_str("\nCharacters:");
    for c in &canon.characters {
        let name = if c.name.trim().is_empty() {
            c.key.as_str()
        } else {
            c.name.trim()
        };
        out.push_str(&format!("\n- {name}: {}", c.public.trim()));
    }
    out.push_str("\nPlaces:");
    for l in &canon.locations {
        out.push_str(&format!("\n- {}", l.name.trim()));
    }
    out.push_str("\nChapters planned:");
    for (i, ch) in canon.arc.chapters.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, ch.goal.trim()));
    }
    out
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
