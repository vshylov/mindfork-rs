//! A story as the chat drives it (docs/fabula.md §4, PR 9): its id, its
//! creation with P1 open and no call, the draft's revision, and the answer
//! read from the person's rows — or refused — on a mock engine.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Answering, ContinueRefusal, Continued, CreateRefusal, Listener, PendingAnswer, Resolved,
    StoryFiles, create, dialect_problems, prepare, story_id,
};
use crate::entities::story::{
    Canon, ChapterArc, Decision, Question, QuestionBody, VersionOrigin, fixture_canon,
};
use crate::features::story::answer::UserRow;
use crate::features::story::board::StoryBoard;
use crate::shared::api::contract::{ChatChunk, FinishReason};
use crate::shared::api::mock::MockBackend;
use crate::shared::storage::story::{StoryDb, story_status_of};

/// A story's files in a directory of their own.
struct Fixture {
    _dir: tempfile::TempDir,
    files: StoryFiles,
    board: StoryBoard,
    chat: Uuid,
    budget: crate::features::story::chapter::Budget,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let files = StoryFiles {
        db: dir.path().join("stories").join("c").join("harbour.db"),
        log: dir.path().join("story-logs").join("c").join("harbour.db"),
    };
    Fixture {
        _dir: dir,
        files,
        board: StoryBoard::new(),
        chat: Uuid::new_v4(),
        budget: Default::default(),
    }
}

impl Fixture {
    async fn create(&self, canon: &Canon, revise: bool) -> Result<Question, CreateRefusal> {
        create(
            &self.files,
            &self.board,
            (self.chat, "harbour"),
            canon,
            revise,
        )
        .await
    }

    async fn prepare(
        &self,
        answering: Answering<'_>,
        backend: Arc<MockBackend>,
    ) -> (Uuid, Result<Continued, ContinueRefusal>) {
        let run = Uuid::new_v4();
        let prepared = prepare(
            &self.files,
            &self.board,
            (self.chat, "harbour", run),
            answering,
            Listener {
                backend,
                model: "mock".to_owned(),
                sessions: None,
                cancel: CancellationToken::new(),
                fallback_only: false,
            },
            self.budget,
        )
        .await;
        (run, prepared)
    }

    fn status(&self) -> crate::shared::storage::story::StoryStatus {
        story_status_of(&self.files.db, &self.files.log).unwrap()
    }

    async fn db(&self) -> StoryDb {
        StoryDb::open(&self.files.db, &self.files.log)
            .await
            .unwrap()
    }
}

/// The reading's reply, as the engine streams it.
fn reading(json: serde_json::Value) -> Arc<MockBackend> {
    Arc::new(MockBackend::sequence(vec![vec![
        ChatChunk::Text(json.to_string()),
        ChatChunk::Finished(FinishReason::Stop),
    ]]))
}

fn rows(texts: &[&str]) -> Vec<UserRow> {
    texts
        .iter()
        .map(|t| UserRow {
            id: Uuid::new_v4(),
            at: Utc::now(),
            text: (*t).to_owned(),
            voice: false,
        })
        .collect()
}

fn pending(question: &Question, texts: &[&str]) -> PendingAnswer {
    PendingAnswer {
        story: "harbour".to_owned(),
        question: question.clone(),
        rows: rows(texts),
    }
}

fn answering(pending: &PendingAnswer) -> Answering<'_> {
    Answering {
        pending: Some(pending),
        answering: true,
        notes: None,
    }
}

/// A story's id is its title's words, safe as a file name on every system,
/// in any script; a taken one is numbered.
#[test]
fn a_story_id_is_its_titles_words_as_a_safe_file_name() {
    let none = |_: &str| false;
    assert_eq!(
        story_id("The Harbour at Night!", none),
        "the-harbour-at-night"
    );
    assert_eq!(story_id("  Маяк и шторм  ", none), "маяк-и-шторм");
    assert_eq!(story_id("../../etc/passwd", none), "etcpasswd");
    assert_eq!(story_id("a  —  b__c", none), "a-b-c");
    assert_eq!(story_id("???", none), "story");
    assert_eq!(story_id("", none), "story");
    assert_eq!(story_id("CON", none), "con-story");
    let long = story_id(&"word ".repeat(20), none);
    assert!(long.chars().count() <= 40, "{long}");
    assert!(!long.ends_with('-'), "{long}");
    let taken = |id: &str| id == "harbour" || id == "harbour-2";
    assert_eq!(story_id("Harbour", taken), "harbour-3");
    for id in ["the-harbour-at-night", "маяк-и-шторм", "harbour-3"] {
        assert!(!id.contains(['/', '\\', ':', '.', ' ']), "{id}");
    }
}

/// The fixture canon breaks no rule of the dialect, and a canon with a
/// problem names it.
#[test]
fn the_canons_schemas_keep_the_dialect() {
    assert_eq!(dialect_problems(&fixture_canon()), Vec::new());
}

/// Creating stores the canon as a draft and opens P1 at once, with no model
/// call; nothing is claimed afterwards.
#[tokio::test]
async fn creating_stores_the_draft_and_opens_p1_with_no_call() {
    let f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    assert_eq!(question.body, QuestionBody::Canon);
    let status = f.status();
    assert_eq!(status.pending.as_ref(), Some(&question));
    assert!(!status.canon_approved);
    assert!(status.pending_since.is_some());
    assert_eq!(status.spend.calls, 0);
    assert!(f.board.of_chat(f.chat).is_empty());
    // A second creation under the same id is a store refusal, not an overwrite.
    assert!(matches!(
        f.create(&fixture_canon(), false).await,
        Err(CreateRefusal::Failed(_))
    ));
}

/// A canon with a problem stores nothing and names every problem.
#[tokio::test]
async fn a_canon_with_problems_stores_nothing() {
    let f = fixture();
    let mut canon = fixture_canon();
    canon.locations.clear();
    canon.arc.chapters.clear();
    let Err(CreateRefusal::Problems(problems)) = f.create(&canon, false).await else {
        panic!("refused with its problems")
    };
    assert!(
        problems.iter().any(|p| p.contains("no locations")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("no chapters")),
        "{problems:?}"
    );
    assert!(!f.files.db.exists());
    // An item that starts in two places is named once, as the canon's own
    // problem: the schemas built on the repeated key would echo it at length.
    let mut twice = fixture_canon();
    let lantern = twice.locations[1].items[0].clone();
    twice.characters[0].start.holds.push(lantern);
    let Err(CreateRefusal::Problems(problems)) = f.create(&twice, false).await else {
        panic!("refused with its problem")
    };
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("starts in more than one place"),
        "{problems:?}"
    );
    // A revision of a story that does not exist is refused too.
    assert_eq!(
        f.create(&fixture_canon(), true).await,
        Err(CreateRefusal::Unknown)
    );
}

/// The draft is revised under its id while P1 waits: the question stays, and
/// stands since the revision; once the canon is approved, it is refused.
#[tokio::test]
async fn a_draft_is_revised_until_it_is_approved() {
    let f = fixture();
    let first = f.create(&fixture_canon(), false).await.unwrap();
    let before = f.status().pending_since.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let mut canon = fixture_canon();
    canon.arc.premise = "A storm, a debt, and a lighthouse.".to_owned();
    let again = f.create(&canon, true).await.unwrap();
    assert_eq!(again, first, "the same question waits");
    let status = f.status();
    assert_eq!(
        status.premise.as_deref(),
        Some("A storm, a debt, and a lighthouse.")
    );
    assert!(
        status.pending_since.unwrap() > before,
        "it stands since the revision"
    );
    // Approved: the canon no longer changes this way.
    let p = pending(&first, &["Approved."]);
    let (run, prepared) = f
        .prepare(
            answering(&p),
            reading(json!({"answers": [], "changes": [], "decision": "approve"})),
        )
        .await;
    assert!(matches!(prepared, Ok(Continued::Start(_))));
    f.board.release(run);
    assert_eq!(f.create(&canon, true).await, Err(CreateRefusal::Approved));
}

/// No answer is read without the person's own rows for the question that
/// waits, nor on a turn that answers none of their messages; each refusal
/// gives the story back and asks no model.
#[tokio::test]
async fn no_answer_without_the_persons_rows_or_on_a_woken_turn() {
    let f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    let backend = reading(json!({"answers": [], "changes": [], "decision": "approve"}));
    let p = pending(&question, &["Yes, approved."]);
    let mut other = p.clone();
    other.question.id = crate::entities::story::QuestionId::new("q9");
    let empty = pending(&question, &[]);
    for answering in [
        Answering {
            pending: None,
            answering: true,
            notes: None,
        },
        Answering {
            pending: Some(&p),
            answering: false,
            notes: None,
        },
        Answering {
            pending: Some(&other),
            answering: true,
            notes: None,
        },
        Answering {
            pending: Some(&empty),
            answering: true,
            notes: None,
        },
    ] {
        let (_, prepared) = f.prepare(answering, backend.clone()).await;
        assert_eq!(prepared, Err(ContinueRefusal::NotAnswered));
        assert!(f.board.of_chat(f.chat).is_empty());
    }
    assert!(!f.status().canon_approved);
    assert_eq!(f.status().pending, Some(question));
}

/// The person's rows are read, the answer is recorded with its evidence, and
/// the story stays claimed for the run; the reading is a call of its own in
/// the log.
#[tokio::test]
async fn an_approval_is_recorded_and_the_story_kept_for_its_run() {
    let f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    let p = pending(&question, &["Looks good — approved, go ahead."]);
    let (run, prepared) = f
        .prepare(
            Answering {
                pending: Some(&p),
                answering: true,
                notes: Some("The user likes the storm."),
            },
            reading(json!({"answers": [], "changes": [], "decision": "approve"})),
        )
        .await;
    assert_eq!(
        prepared,
        Ok(Continued::Start(Some(Resolved {
            body: QuestionBody::Canon,
            decision: Decision::Approve,
            rerun: None,
        })))
    );
    let held = f.board.of_chat(f.chat);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].run, run, "claimed for the run that follows");
    f.board.release(run);
    let status = f.status();
    assert!(status.canon_approved);
    assert_eq!(status.pending, None);
    let db = f.db().await;
    let (asked, answer) = db.questions().await.unwrap().remove(0);
    assert_eq!(asked.id, question.id);
    let answer = answer.unwrap();
    assert_eq!(answer.decision, Decision::Approve);
    assert_eq!(
        answer.evidence["messages"][0]["id"],
        p.rows[0].id.to_string()
    );
    assert_eq!(
        answer.evidence["assistant_notes"],
        "The user likes the storm."
    );
    assert!(answer.notes.contains("The user likes the storm."));
    assert_eq!(
        db.spend("").await.unwrap().calls,
        1,
        "the reading's one call"
    );
    db.close().await.unwrap();
}

/// At a chapter's checkpoint the reading is shown the chapter's scenes by
/// number, and a reply that names one opens the chapter's next take; the run
/// goes on with the rerun (docs/fabula.md §4, PR 10).
#[tokio::test]
async fn a_reply_naming_a_scene_plays_it_again() {
    let f = fixture();
    for dir in [f.files.db.parent(), f.files.log.parent()]
        .into_iter()
        .flatten()
    {
        std::fs::create_dir_all(dir).unwrap();
    }
    let question = {
        let db = f.db().await;
        let mut author = db.author_writer().await.unwrap();
        author.import_canon(&fixture_canon()).await.unwrap();
        crate::features::story::run::tests::at_checkpoint(&db, &mut author).await;
        let question = db.pending_question().await.unwrap().unwrap();
        drop(author);
        db.close().await.unwrap();
        question
    };
    let p = pending(
        &question,
        &["Переиграй первую сцену: пусть Мара промолчит."],
    );
    let (run, prepared) = f
        .prepare(
            answering(&p),
            reading(
                json!({"answers": [], "changes": ["Мара промолчит."], "scene": "1",
                           "decision": "rerun"}),
            ),
        )
        .await;
    assert_eq!(
        prepared,
        Ok(Continued::Start(Some(Resolved {
            body: question.body.clone(),
            decision: Decision::Change,
            rerun: Some(1),
        })))
    );
    f.board.release(run);
    let db = f.db().await;
    let take = db.take_of(1).await.unwrap().expect("the take is stored");
    assert_eq!(take.from_scene, 1);
    assert!(take.notes.contains("Мара промолчит."), "{}", take.notes);
    // The reading was shown the scenes by number, and offered the rerun.
    let reading_run = crate::entities::story::RunId::nth(3);
    let calls = db.calls(&reading_run).await.unwrap();
    assert_eq!(calls.len(), 1);
    let request = calls[0].call.request.to_string();
    assert!(
        request.contains("The chapter's scenes, by number:")
            && request.contains("1. At Harbour, with Mara, Silas. The quarrel shows the debt."),
        "{request}"
    );
    assert!(request.contains("rerun"), "{request}");
    db.close().await.unwrap();
}

/// An unclear reply resolves nothing and gives the story back.
#[tokio::test]
async fn an_unclear_reply_resolves_nothing() {
    let f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    let p = pending(&question, &["Hmm, what does Silas want again?"]);
    let (_, prepared) = f
        .prepare(
            answering(&p),
            reading(json!({"answers": [], "changes": [], "decision": "unclear"})),
        )
        .await;
    assert!(matches!(prepared, Err(ContinueRefusal::Unclear(_))));
    assert!(f.board.of_chat(f.chat).is_empty());
    assert_eq!(f.status().pending, Some(question));
}

/// A change to the canon is recorded and starts no run; the revised draft
/// opens P1 again, standing from the revision.
#[tokio::test]
async fn a_change_to_the_canon_starts_no_run_and_the_revision_asks_again() {
    let f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    let p = pending(&question, &["Make Mara older."]);
    let (_, prepared) = f
        .prepare(
            answering(&p),
            reading(json!({"answers": [], "changes": ["Make Mara older."], "decision": "change"})),
        )
        .await;
    let Ok(Continued::CanonChange(notes)) = prepared else {
        panic!("a canon change: {prepared:?}")
    };
    assert!(notes.contains("Make Mara older."), "{notes}");
    assert!(f.board.of_chat(f.chat).is_empty());
    assert_eq!(f.status().pending, None, "the question is answered");
    let again = f.create(&fixture_canon(), true).await.unwrap();
    assert_ne!(again.id, question.id, "a new question");
    assert_eq!(again.body, QuestionBody::Canon);
    assert_eq!(f.status().pending, Some(again));
}

/// With nothing pending a run resumes; a finished story and a story a run
/// holds are refused.
#[tokio::test]
async fn nothing_pending_resumes_and_finished_or_held_stories_are_refused() {
    let f = fixture();
    let mut canon = fixture_canon();
    canon.arc.chapters = vec![ChapterArc {
        goal: "Mara learns of the debt.".to_owned(),
    }];
    let question = f.create(&canon, false).await.unwrap();
    let none = Answering {
        pending: None,
        answering: true,
        notes: None,
    };
    let backend = reading(json!({}));
    // Approved by hand: nothing pending, chapter 1 not accepted — a resume.
    {
        let db = f.db().await;
        let mut author = db.author_writer().await.unwrap();
        author
            .resolve(
                &question.id,
                &crate::entities::story::Resolution {
                    decision: Decision::Approve,
                    notes: String::new(),
                    evidence: json!(null),
                    rerun: None,
                },
            )
            .await
            .unwrap();
        drop(author);
        db.close().await.unwrap();
    }
    // A turn the app woke on a notification resumes nothing: whether the
    // story goes on is the person's to say (T14); the claim is given back.
    let woken = Answering {
        pending: None,
        answering: false,
        notes: Some("It only stopped at a cut; going on."),
    };
    let (_, refused) = f.prepare(woken, backend.clone()).await;
    assert_eq!(refused, Err(ContinueRefusal::NotAsked));
    assert!(f.board.of_chat(f.chat).is_empty(), "nothing is held");
    let (run, prepared) = f.prepare(none, backend.clone()).await;
    assert_eq!(prepared, Ok(Continued::Start(None)));
    // Held by that run: a second is refused with it.
    let none = Answering {
        pending: None,
        answering: true,
        notes: None,
    };
    let (_, busy) = f.prepare(none, backend.clone()).await;
    assert!(matches!(busy, Err(ContinueRefusal::Busy(view)) if view.run == run));
    f.board.release(run);
    // Chapter 1 accepted: the story is finished.
    {
        let db = f.db().await;
        let mut author = db.author_writer().await.unwrap();
        let mut log = db.run_log_writer().await.unwrap();
        let r = log.start_run(&json!({})).await.unwrap();
        author
            .add_chapter(1, VersionOrigin::Draft, "## One")
            .await
            .unwrap();
        let q = author
            .add_question(
                &r,
                "ch1/v1/checkpoint",
                &QuestionBody::Chapter {
                    chapter: 1,
                    version: 1,
                },
            )
            .await
            .unwrap();
        author
            .resolve(
                &q.id,
                &crate::entities::story::Resolution {
                    decision: Decision::Approve,
                    notes: String::new(),
                    evidence: json!(null),
                    rerun: None,
                },
            )
            .await
            .unwrap();
        drop((author, log));
        db.close().await.unwrap();
    }
    let none = Answering {
        pending: None,
        answering: true,
        notes: None,
    };
    let (_, finished) = f.prepare(none, backend).await;
    assert_eq!(finished, Err(ContinueRefusal::Finished));
    assert!(f.board.of_chat(f.chat).is_empty());
    // A story the chat does not have.
    let other = Fixture {
        files: StoryFiles {
            db: f.files.db.with_file_name("nowhere.db"),
            log: f.files.log.with_file_name("nowhere.db"),
        },
        ..fixture()
    };
    let none = Answering {
        pending: None,
        answering: true,
        notes: None,
    };
    let (_, unknown) = other.prepare(none, reading(json!({}))).await;
    assert_eq!(unknown, Err(ContinueRefusal::Unknown));
}

/// A chapter whose budget is spent starts no run, which would stop before its
/// first call; an answer read on the way is kept for the run that follows a
/// raised budget.
#[tokio::test]
async fn a_spent_chapter_starts_no_run_and_keeps_the_answer() {
    let mut f = fixture();
    let question = f.create(&fixture_canon(), false).await.unwrap();
    // Chapter 1 spent what it had in an earlier run.
    {
        let db = f.db().await;
        let mut log = db.run_log_writer().await.unwrap();
        let r = log.start_run(&json!({})).await.unwrap();
        log.record_call(crate::shared::storage::story::NewCall {
            run: r,
            address: "ch1/outline".to_owned(),
            attempt: 1,
            role: "author".to_owned(),
            model: "m".to_owned(),
            params: json!({}),
            mechanism: crate::shared::storage::story::Mechanism::Schema,
            request: json!({}),
            prompt_sha256: "h".to_owned(),
            response: None,
            status: crate::shared::storage::story::CallStatus::Error,
            usage: crate::shared::storage::story::CallUsage {
                input_tokens: Some(900),
                output_tokens: Some(200),
                cached_tokens: None,
                cost_usd: None,
            },
            latency_ms: None,
        })
        .await
        .unwrap();
        drop(log);
        db.close().await.unwrap();
    }
    f.budget = crate::features::story::chapter::Budget {
        tokens: Some(1_000),
        usd: None,
    };
    let p = pending(&question, &["Approved."]);
    let (_, prepared) = f
        .prepare(
            answering(&p),
            reading(json!({"answers": [], "changes": [], "decision": "approve"})),
        )
        .await;
    assert_eq!(
        prepared,
        Err(ContinueRefusal::BudgetSpent {
            chapter: 1,
            spent: "1100 tokens of 1000".to_owned(),
        })
    );
    assert!(
        f.board.of_chat(f.chat).is_empty(),
        "the story is given back"
    );
    assert!(f.status().canon_approved, "the answer is kept");
    // With room, the same story resumes.
    f.budget = crate::features::story::chapter::Budget {
        tokens: Some(5_000),
        usd: None,
    };
    let none = Answering {
        pending: None,
        answering: true,
        notes: None,
    };
    let (run, resumed) = f.prepare(none, reading(json!({}))).await;
    assert_eq!(resumed, Ok(Continued::Start(None)));
    f.board.release(run);
}

/// Accepting the last chapter finishes the story: the answer is recorded and
/// no run starts, since one would only find the story finished.
#[tokio::test]
async fn accepting_the_last_chapter_finishes_the_story() {
    let f = fixture();
    let mut canon = fixture_canon();
    canon.arc.chapters = vec![ChapterArc {
        goal: "Mara learns of the debt.".to_owned(),
    }];
    let p1 = f.create(&canon, false).await.unwrap();
    let checkpoint = {
        let db = f.db().await;
        let mut author = db.author_writer().await.unwrap();
        author
            .resolve(
                &p1.id,
                &crate::entities::story::Resolution {
                    decision: Decision::Approve,
                    notes: String::new(),
                    evidence: json!(null),
                    rerun: None,
                },
            )
            .await
            .unwrap();
        let mut log = db.run_log_writer().await.unwrap();
        let r = log.start_run(&json!({})).await.unwrap();
        author
            .add_chapter(1, VersionOrigin::Draft, "## One")
            .await
            .unwrap();
        let q = author
            .add_question(
                &r,
                "ch1/v1/checkpoint",
                &QuestionBody::Chapter {
                    chapter: 1,
                    version: 1,
                },
            )
            .await
            .unwrap();
        drop((author, log));
        db.close().await.unwrap();
        q
    };
    let p = pending(&checkpoint, &["I accept it. Thank you."]);
    let (_, prepared) = f
        .prepare(
            answering(&p),
            reading(json!({"answers": [], "changes": [], "decision": "approve"})),
        )
        .await;
    assert_eq!(prepared, Ok(Continued::Finished(1)));
    assert!(f.board.of_chat(f.chat).is_empty(), "nothing is held");
    let status = f.status();
    assert_eq!(status.open_chapter, None, "finished");
    assert_eq!(status.pending, None);
}
