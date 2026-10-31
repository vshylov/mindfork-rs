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
    BranchId, Canon, CharacterKey, NewEvent, Proposal, ProposalId, ProposalVerdict, RunId, SceneId,
    SceneSetup,
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

assert_not_clone!(BranchWriter, ReflectionWriter, AuthorWriter, RunLogWriter);

/// The capabilities move into the tasks that use them, and the handle is
/// shared between those tasks.
const _: fn() = || {
    fn send<T: Send>() {}
    fn shared<T: Send + Sync>() {}
    send::<BranchWriter>();
    send::<ReflectionWriter>();
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

impl Drop for AuthorWriter {
    fn drop(&mut self) {
        let token = self.token;
        self.db.post(move |a| a.release_author(token));
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
