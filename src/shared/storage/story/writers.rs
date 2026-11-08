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

use super::{NewCall, StoryDb, StoryDbError};
use crate::entities::story::{
    Canon, ChapterVersion, Question, QuestionBody, QuestionId, Resolution, RunId, VersionOrigin,
};

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

assert_not_clone!(AuthorWriter, RunLogWriter);

/// The capabilities move into the tasks that use them, and the handle is
/// shared between those tasks.
const _: fn() = || {
    fn send<T: Send>() {}
    fn shared<T: Send + Sync>() {}
    send::<AuthorWriter>();
    send::<RunLogWriter>();
    shared::<StoryDb>();
};

/// The Author's capability: the canon, the chapters' versions, the pauses
/// (spec §5).
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
    /// once the canon is approved.
    pub async fn import_canon(&mut self, canon: &Canon) -> Result<usize, StoryDbError> {
        let (canon, token) = (canon.clone(), self.token);
        self.db.run(move |a| a.import_canon(token, &canon)).await
    }
}

impl AuthorWriter {
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
