//! The write capabilities (docs/research/fabula-engine.md §6, §9): each can
//! write only its own rows, none is `Clone`, and the actor refuses a second
//! one for the same store while the first lives. Dropping one gives its store
//! back.
//!
//! "Not `Clone`" is checked at compile time below: the crate has no library
//! target, so neither `trybuild` nor a `compile_fail` doctest can run, and the
//! ambiguity trick `static_assertions::assert_not_impl_any!` uses stands in
//! (docs/fabula.md §5).

use serde_json::Value;

use super::{NewCall, StepStatus, StoryDb, StoryDbError};
use crate::entities::story::{
    Assumption, BranchId, Canon, ChapterId, ChapterOutline, ChapterVersion, CharacterKey, Decided,
    NewEvent, NewNote, NoteId, Proposal, ProposalId, ProposalVerdict, Question, QuestionBody,
    QuestionId, Resolution, Reviewer, RunId, SceneId, SceneSetup, VersionOrigin,
};

use super::CommittedEvent;
use super::actor::NewProposal;

/// Fails to compile when one of the types implements `Clone`: with an impl
/// for every type and another for every `Clone` type, the inferred parameter
/// is ambiguous exactly when both apply.
macro_rules! assert_not_clone {
    ($($t:ty),+ $(,)?) => {$(
        const _: fn() = || {
            trait AmbiguousIfClone<A> {
                fn some_item() {}
            }
            impl<T: ?Sized> AmbiguousIfClone<()> for T {}
            #[allow(dead_code)]
            struct IsClone;
            impl<T: ?Sized + Clone> AmbiguousIfClone<IsClone> for T {}
            let _ = <$t as AmbiguousIfClone<_>>::some_item;
        };
    )+};
}

assert_not_clone!(
    BranchWriter,
    ReflectionWriter,
    ReviewWriter,
    AuthorWriter,
    RunLogWriter
);

/// The capabilities move into the tasks that use them, and the handle is
/// shared between those tasks.
const _: fn() = || {
    fn send<T: Send>() {}
    fn shared<T: Send + Sync>() {}
    send::<BranchWriter>();
    send::<ReflectionWriter>();
    send::<ReviewWriter>();
    send::<AuthorWriter>();
    send::<RunLogWriter>();
    shared::<StoryDb>();
};

/// The World Master's capability over one branch: it records the branch's
/// proposals and commits its events (spec §5).
pub struct BranchWriter {
    db: StoryDb,
    branch: BranchId,
    token: u64,
}

impl BranchWriter {
    pub(super) fn new(db: StoryDb, branch: BranchId, token: u64) -> Self {
        Self { db, branch, token }
    }

    /// The branch this writer writes.
    pub fn branch(&self) -> &BranchId {
        &self.branch
    }

    /// Records a proposal with the World Master's ruling on it.
    pub async fn record_proposal(
        &mut self,
        round: u32,
        character: &CharacterKey,
        proposal: &Proposal,
        verdict: ProposalVerdict,
        reason: Option<String>,
    ) -> Result<ProposalId, StoryDbError> {
        let (branch, token) = (self.branch.clone(), self.token);
        let row = NewProposal {
            round,
            character: character.clone(),
            proposal: proposal.clone(),
            verdict,
            reason,
        };
        self.db
            .run(move |a| a.record_proposal(&branch, token, &row))
            .await
    }

    /// Checks the event against the world at the branch's tip, computes its
    /// witnesses and appends it, all on the actor's thread (spec §6).
    pub async fn commit(&mut self, event: NewEvent) -> Result<CommittedEvent, StoryDbError> {
        let (branch, token) = (self.branch.clone(), self.token);
        self.db.run(move |a| a.commit(&branch, token, &event)).await
    }
}

impl Drop for BranchWriter {
    fn drop(&mut self) {
        let (branch, token) = (self.branch.clone(), self.token);
        self.db.post(move |a| a.release_branch(&branch, token));
    }
}

/// A character's capability over their own reflections on one branch.
pub struct ReflectionWriter {
    db: StoryDb,
    branch: BranchId,
    character: CharacterKey,
    token: u64,
}

impl ReflectionWriter {
    pub(super) fn new(db: StoryDb, branch: BranchId, character: CharacterKey, token: u64) -> Self {
        Self {
            db,
            branch,
            character,
            token,
        }
    }

    /// Records a reflection after the branch path's latest event; its id.
    pub async fn write(
        &mut self,
        body: &str,
        goal_changes: &[String],
    ) -> Result<String, StoryDbError> {
        let (branch, character, token) = (self.branch.clone(), self.character.clone(), self.token);
        let (body, goal_changes) = (body.to_owned(), goal_changes.to_vec());
        self.db
            .run(move |a| a.write_reflection(&branch, &character, token, &body, &goal_changes))
            .await
    }
}

impl Drop for ReflectionWriter {
    fn drop(&mut self) {
        let (branch, character, token) = (self.branch.clone(), self.character.clone(), self.token);
        self.db
            .post(move |a| a.release_reflection(&branch, &character, token));
    }
}

/// The Author's capability: the canon, the scenes, the picks (spec §5).
pub struct AuthorWriter {
    db: StoryDb,
    token: u64,
}

impl AuthorWriter {
    pub(super) fn new(db: StoryDb, token: u64) -> Self {
        Self { db, token }
    }

    /// Stores the canon after its deterministic checks: a new version of
    /// every document that changed, a removal for every one that went. The
    /// number of document versions written; 0 when nothing changed. Refused
    /// once the story has events.
    pub async fn import_canon(&mut self, canon: &Canon) -> Result<usize, StoryDbError> {
        let (canon, token) = (canon.clone(), self.token);
        self.db.run(move |a| a.import_canon(token, &canon)).await
    }

    /// Stores the setup of scene `scene` of chapter `chapter` at rerun
    /// `rerun`, with the Author's private intent for it; the scene's id.
    pub async fn add_scene(
        &mut self,
        (chapter, scene, rerun): (u32, u32, u32),
        setup: &SceneSetup,
        intent: &str,
    ) -> Result<SceneId, StoryDbError> {
        let (setup, intent, token) = (setup.clone(), intent.to_owned(), self.token);
        self.db
            .run(move |a| a.add_scene(token, (chapter, scene, rerun), &setup, &intent))
            .await
    }

    /// Records the pick for a scene: one of its branches, which extends the
    /// main line, or none when a rerun is wanted.
    pub async fn record_pick(
        &mut self,
        scene: &SceneId,
        branch: Option<&BranchId>,
        rationale: &str,
    ) -> Result<(), StoryDbError> {
        let (scene, branch, rationale, token) = (
            scene.clone(),
            branch.cloned(),
            rationale.to_owned(),
            self.token,
        );
        self.db
            .run(move |a| a.record_pick(token, &scene, branch.as_ref(), &rationale))
            .await
    }
}

impl AuthorWriter {
    /// Stores chapter `chapter`'s outline; refused once it has one.
    pub async fn add_outline(
        &mut self,
        chapter: u32,
        outline: &ChapterOutline,
    ) -> Result<(), StoryDbError> {
        let (outline, token) = (outline.clone(), self.token);
        self.db
            .run(move |a| a.add_outline(token, chapter, &outline))
            .await
    }

    /// Stores the next version of chapter `number`: a draft as its first, a
    /// revision or the human's revision after one.
    pub async fn add_chapter(
        &mut self,
        number: u32,
        origin: VersionOrigin,
        text: &str,
    ) -> Result<ChapterVersion, StoryDbError> {
        let (text, token) = (text.to_owned(), self.token);
        self.db
            .run(move |a| a.add_chapter(token, number, origin, &text))
            .await
    }

    /// Stores a final version's summary.
    pub async fn add_summary(
        &mut self,
        chapter: &ChapterId,
        summary: &str,
    ) -> Result<(), StoryDbError> {
        let (chapter, summary, token) = (chapter.clone(), summary.to_owned(), self.token);
        self.db
            .run(move |a| a.add_summary(token, &chapter, &summary))
            .await
    }

    /// Records the decisions on a version's notes, all in one go.
    pub async fn record_decisions(
        &mut self,
        chapter: &ChapterId,
        decisions: &[Decided],
    ) -> Result<(), StoryDbError> {
        let (chapter, decisions, token) = (chapter.clone(), decisions.to_vec(), self.token);
        self.db
            .run(move |a| a.record_decisions(token, &chapter, &decisions))
            .await
    }

    /// Asks a pause's question at `step` of `run`; refused while another
    /// waits.
    pub async fn add_question(
        &mut self,
        run: &RunId,
        step: &str,
        body: &QuestionBody,
    ) -> Result<Question, StoryDbError> {
        let (run, step, body, token) = (run.clone(), step.to_owned(), body.clone(), self.token);
        self.db
            .run(move |a| a.add_question(token, &run, &step, &body))
            .await
    }

    /// Records the human's answer to a question, with the approval or the
    /// acceptance it carries.
    pub async fn resolve(
        &mut self,
        question: &QuestionId,
        resolution: &Resolution,
    ) -> Result<(), StoryDbError> {
        let (question, resolution, token) = (question.clone(), resolution.clone(), self.token);
        self.db
            .run(move |a| a.resolve(token, &question, &resolution))
            .await
    }

    /// Records an assumption `role` made at `step` of `run`; its id.
    pub async fn add_assumption(
        &mut self,
        run: &RunId,
        step: &str,
        role: &str,
        assumption: &Assumption,
    ) -> Result<String, StoryDbError> {
        let (run, step, role, assumption, token) = (
            run.clone(),
            step.to_owned(),
            role.to_owned(),
            assumption.clone(),
            self.token,
        );
        self.db
            .run(move |a| a.add_assumption(token, &run, &step, &role, &assumption))
            .await
    }
}

impl Drop for AuthorWriter {
    fn drop(&mut self) {
        let token = self.token;
        self.db.post(move |a| a.release_author(token));
    }
}

/// A reviewer's capability over its notes on one chapter version (spec §5,
/// *Store ownership*: each reviewer writes its own notes).
pub struct ReviewWriter {
    db: StoryDb,
    chapter: ChapterId,
    reviewer: Reviewer,
    token: u64,
}

impl ReviewWriter {
    pub(super) fn new(db: StoryDb, chapter: ChapterId, reviewer: Reviewer, token: u64) -> Self {
        Self {
            db,
            chapter,
            reviewer,
            token,
        }
    }

    /// Writes the review's notes, all in one transaction; their ids. A
    /// version is reviewed once by each reviewer.
    pub async fn write(&mut self, notes: &[NewNote]) -> Result<Vec<NoteId>, StoryDbError> {
        let (chapter, reviewer, token, notes) = (
            self.chapter.clone(),
            self.reviewer,
            self.token,
            notes.to_vec(),
        );
        self.db
            .run(move |a| a.write_notes(&chapter, reviewer, token, &notes))
            .await
    }
}

impl Drop for ReviewWriter {
    fn drop(&mut self) {
        let (chapter, reviewer, token) = (self.chapter.clone(), self.reviewer, self.token);
        self.db
            .post(move |a| a.release_review(&chapter, reviewer, token));
    }
}

/// The engine's capability over the run log: runs, their steps, and the
/// model calls in the call-log file (spec §5).
pub struct RunLogWriter {
    db: StoryDb,
    token: u64,
}

impl RunLogWriter {
    pub(super) fn new(db: StoryDb, token: u64) -> Self {
        Self { db, token }
    }

    /// Starts a run with its configuration; its id.
    pub async fn start_run(&mut self, config: &Value) -> Result<RunId, StoryDbError> {
        let (config, token) = (config.clone(), self.token);
        self.db.run(move |a| a.start_run(token, &config)).await
    }

    /// Records how a step ended. A step recorded again — a failed one that a
    /// resume finished — keeps its place and takes the new status.
    pub async fn finish_step(
        &mut self,
        run: &RunId,
        address: &str,
        status: StepStatus,
    ) -> Result<(), StoryDbError> {
        let (run, address, token) = (run.clone(), address.to_owned(), self.token);
        self.db
            .run(move |a| a.finish_step(token, &run, &address, status))
            .await
    }

    /// Records a model call in the call log; its id.
    pub async fn record_call(&mut self, call: NewCall) -> Result<String, StoryDbError> {
        let token = self.token;
        self.db.run(move |a| a.record_call(token, &call)).await
    }
}

impl Drop for RunLogWriter {
    fn drop(&mut self) {
        let token = self.token;
        self.db.post(move |a| a.release_run_log(token));
    }
}
