//! Tests of the story tools' engine after M3 (docs/history/fabula.md §6, T25–T27):
//! B0's session a chapter, over the engine adapter and a scripted engine, so
//! every turn is in the call log as a live run's is.

use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex;

use super::{Budget, MOST_REVISIONS, Outcome, Stop, Story, advance};
use crate::entities::story::{
    Canon, ChapterArc, Decision, QuestionBody, Resolution, VersionOrigin, fixture_canon,
};
use crate::features::story::engine::{EngineAdapter, SharedLog};
use crate::features::story::run::tests::approved;
use crate::features::story::single::tests::{failure, port, reply, turns};
use crate::features::story::single::{
    Ask, CHAPTER_CRITIQUE_ASK, SingleConfig, draft_ask, revise_ask, system,
};
use crate::shared::api::contract::ChatChunk;
use crate::shared::storage::story::{AuthorWriter, StoryDb};

/// A story of `canon` in a directory of its own, its canon approved when
/// `approve`.
struct Fixture {
    _dir: tempfile::TempDir,
    db: StoryDb,
    author: AuthorWriter,
    log: SharedLog,
}

async fn story_of(canon: &Canon, approve: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = StoryDb::open(
        &dir.path().join("story.db"),
        &dir.path().join("story.calls.db"),
    )
    .await
    .unwrap();
    let mut author = db.author_writer().await.unwrap();
    author.import_canon(canon).await.unwrap();
    if approve {
        approved(&db, &mut author).await;
    }
    let log: SharedLog = Arc::new(Mutex::new(db.run_log_writer().await.unwrap()));
    Fixture {
        _dir: dir,
        db,
        author,
        log,
    }
}

/// The fixture's canon with a second chapter in its arc.
fn two_chapters() -> Canon {
    let mut canon = fixture_canon();
    canon.arc.chapters.push(ChapterArc {
        goal: "The harbour learns who sold the shipment.".to_owned(),
    });
    canon
}

/// At most `revisions` revisions, no budget.
fn config(revisions: u32) -> SingleConfig {
    SingleConfig {
        most_revisions: revisions,
        ..SingleConfig::default()
    }
}

impl Fixture {
    async fn advance(&mut self, port: &EngineAdapter, config: &SingleConfig) -> Outcome {
        advance(Story {
            db: &self.db,
            author: &mut self.author,
            log: &self.log,
            port,
            config,
        })
        .await
        .unwrap()
    }

    /// Answers the question that waits with `decision` and `notes`.
    async fn answer(&mut self, decision: Decision, notes: &str) {
        let question = self.db.pending_question().await.unwrap().unwrap();
        let resolution = Resolution {
            decision,
            notes: notes.to_owned(),
            evidence: json!(null),
        };
        self.author
            .resolve(&question.id, &resolution)
            .await
            .unwrap();
    }

    /// Every address the story's calls were made at, in order.
    async fn addresses(&self) -> Vec<String> {
        let mut out = Vec::new();
        for run in self.db.runs().await.unwrap() {
            for c in self.db.calls(&run).await.unwrap() {
                out.push(c.call.address);
            }
        }
        out
    }

    /// What the call at `address` was told, its system prompt first.
    async fn told(&self, address: &str) -> (String, Vec<String>) {
        for run in self.db.runs().await.unwrap() {
            if let Some(c) = self.db.call(&run, address, 1).await.unwrap() {
                let said = c.call.request["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["content"].as_str().unwrap().to_owned())
                    .collect();
                return (c.call.request["system"].as_str().unwrap().to_owned(), said);
            }
        }
        panic!("no call at {address}");
    }
}

fn asks_about(outcome: &Outcome) -> Option<(u32, u32)> {
    match outcome {
        Outcome::Paused(q) => match q.body {
            QuestionBody::Chapter { chapter, version } => Some((chapter, version)),
            _ => None,
        },
        _ => None,
    }
}

#[tokio::test]
async fn an_unapproved_canon_asks_for_its_approval_and_makes_no_call() {
    let mut story = story_of(&fixture_canon(), false).await;
    let port = port(Vec::new(), &story.log);
    let outcome = story.advance(&port, &config(2)).await;
    assert!(
        matches!(&outcome, Outcome::Paused(q) if q.body == QuestionBody::Canon),
        "{outcome:?}"
    );
    assert!(story.addresses().await.is_empty());
    // The question waits: the next run returns it and asks nothing.
    let again = story.advance(&port, &config(2)).await;
    assert_eq!(again, outcome);
}

#[tokio::test]
async fn a_chapter_is_written_to_its_most_revisions_and_asked_about_as_m3_asked() {
    let mut story = story_of(&fixture_canon(), true).await;
    let port = port(turns(5), &story.log);
    let outcome = story.advance(&port, &config(2)).await;
    assert_eq!(asks_about(&outcome), Some((1, 3)), "{outcome:?}");
    assert_eq!(
        story.addresses().await,
        [
            "ch1/b0/t1",
            "ch1/b0/t2",
            "ch1/b0/t3",
            "ch1/b0/t4",
            "ch1/b0/t5"
        ]
    );
    let origins: Vec<VersionOrigin> = story
        .db
        .chapter_versions(1)
        .await
        .unwrap()
        .iter()
        .map(|v| v.origin)
        .collect();
    assert_eq!(
        origins,
        [
            VersionOrigin::Draft,
            VersionOrigin::Revision,
            VersionOrigin::Revision
        ]
    );
    // A story of one chapter is asked what M3's B0 was, word for word.
    let canon = fixture_canon();
    let words = SingleConfig::default().words;
    let (said_system, said) = story.told("ch1/b0/t5").await;
    assert_eq!(said_system, system(&canon));
    assert_eq!(said[0], draft_ask(&canon, words));
    assert_eq!(said[4], revise_ask(words, 4));
}

#[tokio::test]
async fn the_tools_write_twenty_four_revisions_after_the_draft() {
    assert_eq!(MOST_REVISIONS, 24);
    let mut story = story_of(&fixture_canon(), true).await;
    let port = port(turns(49), &story.log);
    let outcome = story.advance(&port, &config(MOST_REVISIONS)).await;
    assert_eq!(asks_about(&outcome), Some((1, 25)), "{outcome:?}");
    assert_eq!(story.addresses().await.len(), 49);
}

#[tokio::test]
async fn a_change_at_the_checkpoint_is_one_turn_over_the_version_and_the_notes() {
    let mut story = story_of(&fixture_canon(), true).await;
    let mut scripts = turns(3);
    scripts.push(reply("## The harbour, again\n\nThe gate stays shut."));
    let port = port(scripts, &story.log);
    let first = story.advance(&port, &config(1)).await;
    assert_eq!(asks_about(&first), Some((1, 2)));
    story
        .answer(Decision::Change, "Keep the gate shut until dawn.")
        .await;
    let outcome = story.advance(&port, &config(1)).await;
    assert_eq!(asks_about(&outcome), Some((1, 3)), "{outcome:?}");
    let latest = story.db.chapter_versions(1).await.unwrap().pop().unwrap();
    assert_eq!(latest.origin, VersionOrigin::Human);
    assert_eq!(latest.text, "## The harbour, again\n\nThe gate stays shut.");
    // One turn: the chapter's ask, the version the person read, their notes;
    // not the session's critiques.
    let (_, said) = story.told("ch1/b0/h3").await;
    let canon = fixture_canon();
    let words = SingleConfig::default().words;
    assert_eq!(said.len(), 3);
    assert_eq!(said[0], draft_ask(&canon, words));
    assert_eq!(said[1], "## Version 2\n\nText 3.");
    assert_eq!(
        said[2],
        Ask::story().notes_ask(words, 4, "Keep the gate shut until dawn.")
    );
    assert!(said[2].starts_with("The reader asks for these changes:\n\nKeep the gate shut"));
    // Asked again, nothing more is called.
    let calls = story.addresses().await.len();
    story.advance(&port, &config(1)).await;
    assert_eq!(story.addresses().await.len(), calls);
}

#[tokio::test]
async fn an_accepted_chapter_opens_the_next_with_its_text_and_the_notes_left_on_it() {
    let mut story = story_of(&two_chapters(), true).await;
    let mut scripts = turns(3);
    scripts.extend(turns(3));
    let port = port(scripts, &story.log);
    let first = story.advance(&port, &config(1)).await;
    assert_eq!(asks_about(&first), Some((1, 2)));
    story
        .answer(Decision::Approve, "Let the smuggler come back.")
        .await;
    let second = story.advance(&port, &config(1)).await;
    assert_eq!(asks_about(&second), Some((2, 2)), "{second:?}");
    let (said_system, said) = story.told("ch2/b0/t1").await;
    assert!(
        said_system.contains("a story of 2 chapters"),
        "{said_system}"
    );
    assert!(said[0].starts_with(
        "Write chapter 2 of 2. What it must achieve: The harbour learns who sold the shipment."
    ));
    assert!(said[0].contains("as the reader accepted them:\n\n## Version 2\n\nText 3."));
    assert!(said[0].contains("What the reader asked of this chapter: Let the smuggler come back."));
    let (_, critique) = story.told("ch2/b0/t2").await;
    assert_eq!(critique[2], CHAPTER_CRITIQUE_ASK);
    // The last chapter accepted, the story is finished.
    story.answer(Decision::Approve, "").await;
    assert_eq!(story.advance(&port, &config(1)).await, Outcome::Finished);
}

#[tokio::test]
async fn a_failed_turn_stops_the_run_there_and_the_next_run_goes_on_from_it() {
    let mut story = story_of(&fixture_canon(), true).await;
    let mut first = turns(2);
    first.push(failure());
    let broken = port(first, &story.log);
    let outcome = story.advance(&broken, &config(2)).await;
    assert!(
        matches!(&outcome, Outcome::Stopped { step, why: Stop::Failed(_) } if step == "ch1/b0/t3"),
        "{outcome:?}"
    );
    drop(broken);
    let rest: Vec<Vec<ChatChunk>> = turns(5).into_iter().skip(2).collect();
    let resumed = port(rest, &story.log);
    let outcome = story.advance(&resumed, &config(2)).await;
    assert_eq!(asks_about(&outcome), Some((1, 3)), "{outcome:?}");
    let addresses = story.addresses().await;
    assert_eq!(addresses.iter().filter(|a| *a == "ch1/b0/t1").count(), 1);
    assert_eq!(addresses.iter().filter(|a| *a == "ch1/b0/t3").count(), 2);
}

#[tokio::test]
async fn a_spent_budget_ends_the_session_at_its_last_version() {
    let mut story = story_of(&fixture_canon(), true).await;
    let port = port(turns(9), &story.log);
    // 150 tokens a turn: three turns spend 450, past 400.
    let budget = SingleConfig {
        budget: Budget {
            tokens: Some(400),
            usd: None,
        },
        ..config(MOST_REVISIONS)
    };
    let outcome = story.advance(&port, &budget).await;
    assert_eq!(asks_about(&outcome), Some((1, 2)), "{outcome:?}");
    assert_eq!(story.addresses().await.len(), 3);
}

#[tokio::test]
async fn a_revision_that_breaks_off_is_not_kept_and_is_asked_again() {
    let mut story = story_of(&fixture_canon(), true).await;
    let mut scripts = turns(1);
    scripts.push(reply("## The harbour\n\nAnd then she did not tu"));
    scripts.push(reply("## The harbour\n\nThe gate stays shut."));
    let port = port(scripts, &story.log);
    story.advance(&port, &config(0)).await;
    story.answer(Decision::Change, "Shut the gate.").await;
    let outcome = story.advance(&port, &config(0)).await;
    assert!(
        matches!(&outcome, Outcome::Stopped { step, .. } if step == "ch1/b0/h2"),
        "{outcome:?}"
    );
    assert_eq!(story.db.chapter_versions(1).await.unwrap().len(), 1);
    let outcome = story.advance(&port, &config(0)).await;
    assert_eq!(asks_about(&outcome), Some((1, 2)), "{outcome:?}");
}
