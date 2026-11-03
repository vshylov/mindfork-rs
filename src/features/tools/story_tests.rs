//! The story tools' texts and `story_status` (docs/fabula.md §4, PR 8): what
//! the assistant reads of a story, in both languages the app ships, with no
//! placeholder left unfilled.

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use super::{
    STORY_STATUS_ID, StoryStatusTool, attachment_name, attachment_source, human_language,
    landing_text, question_text, quit_text, report,
};
use crate::entities::story::{
    Assumption, Decision, Question, QuestionBody, QuestionId, Resolution, RunId, fixture_canon,
};
use crate::features::story::board::RunView;
use crate::features::story::chapter::Stop;
use crate::features::story::run::{Ending, Landed, LandedChapter};
use crate::features::tools::Tool;
use crate::features::tools::testkit::ctx_with_storage_lang;
use crate::shared::i18n::{Lang, Locale, locale};
use crate::shared::storage::story::{ChapterState, Spend, StoryDb, StoryStatus};

fn both() -> [&'static Locale; 2] {
    [locale(Lang::En), locale(Lang::Ru)]
}

/// No `{name}` of a template is left in `text`.
fn filled(text: &str) {
    assert!(
        !text.contains('{') && !text.contains('}'),
        "an unfilled placeholder: {text}"
    );
}

fn question(body: QuestionBody) -> Question {
    Question {
        id: QuestionId::new("q1"),
        run: RunId::nth(1),
        step: "ch1/v1/checkpoint".to_owned(),
        body,
    }
}

fn status() -> StoryStatus {
    StoryStatus {
        premise: Some("A harbour town hides a debt.".to_owned()),
        planned: 3,
        canon_approved: true,
        chapters: vec![
            ChapterState {
                number: 1,
                versions: 2,
                accepted: true,
            },
            ChapterState {
                number: 2,
                versions: 1,
                accepted: false,
            },
        ],
        pending: Some(question(QuestionBody::Chapter {
            chapter: 2,
            version: 1,
        })),
        pending_since: None,
        open_chapter: Some(2),
        spend: Spend {
            calls: 4,
            input_tokens: 1000,
            output_tokens: 234,
            cost_usd: Some(0.0412),
        },
    }
}

fn view(step: Option<&str>) -> RunView {
    RunView {
        chat: Uuid::nil(),
        story: "s1".to_owned(),
        run: Uuid::nil(),
        started_at: chrono::DateTime::parse_from_rfc3339("2026-11-02T10:00:00Z")
            .unwrap()
            .to_utc(),
        step: step.map(str::to_owned),
    }
}

/// The status of a story says where it stands, line by line; the run's step
/// where one is out; the cost where the provider reported one.
#[test]
fn the_status_reports_each_story_in_the_profiles_language() {
    let en = locale(Lang::En);
    let stories = vec![("s1".to_owned(), Ok(status()))];
    let text = report(en, &stories, &[view(Some("ch2/sc1/plan"))]);
    filled(&text);
    for want in [
        "Story «s1»: A harbour town hides a debt.",
        "Chapters accepted: 1 of 3.",
        "Chapter 2: 1 version(s) written",
        "1234 tokens so far, $0.04",
        "since 2026-11-02 10:00 UTC; it is now at ch2/sc1/plan",
        "Waiting for the user's answer: chapter 2 (version 1)",
    ] {
        assert!(text.contains(want), "{want:?} not in:\n{text}");
    }
    assert!(!text.contains("not approved"), "{text}");

    let idle = report(en, &stories, &[]);
    assert!(idle.contains("No run is writing it now."), "{idle}");
    let starting = report(en, &stories, &[view(None)]);
    assert!(
        starting.contains("has not made its first call yet"),
        "{starting}"
    );

    for loc in both() {
        filled(&report(loc, &stories, &[view(Some("ch2/sc1/plan"))]));
        filled(&report(loc, &[], &[]));
    }
}

/// A story without a canon, one waiting for its canon's approval, a finished
/// one and an unreadable one each get their own line; no story at all says so.
#[test]
fn the_status_names_every_state_of_a_story() {
    let en = locale(Lang::En);
    assert_eq!(report(en, &[], &[]), "This chat has no story.");
    let bare = StoryStatus {
        premise: None,
        planned: 0,
        canon_approved: false,
        chapters: Vec::new(),
        pending: None,
        pending_since: None,
        open_chapter: None,
        spend: Spend::default(),
    };
    let unapproved = StoryStatus {
        canon_approved: false,
        chapters: Vec::new(),
        pending: Some(question(QuestionBody::Canon)),
        open_chapter: Some(1),
        spend: Spend::default(),
        ..status()
    };
    let finished = StoryStatus {
        pending: None,
        open_chapter: None,
        ..status()
    };
    let stories = vec![
        ("a".to_owned(), Ok(bare)),
        ("b".to_owned(), Ok(unapproved)),
        ("c".to_owned(), Ok(finished)),
        ("d".to_owned(), Err("the file is broken".to_owned())),
    ];
    let text = report(en, &stories, &[]);
    for want in [
        "Story «a» has no canon yet.",
        "Its canon and arc are not approved yet.",
        "approve the story's canon and arc",
        "the story is finished.",
        "Story «d» cannot be read: the file is broken",
    ] {
        assert!(text.contains(want), "{want:?} not in:\n{text}");
    }
    assert_eq!(text.matches("\n\n").count(), 3, "one paragraph a story");
    for loc in both() {
        filled(&report(loc, &stories, &[]));
    }
}

fn landed(ending: Ending) -> Landed {
    Landed {
        ending,
        chapter: None,
        picks: Vec::new(),
        assumptions: Vec::new(),
        spent: Spend::default(),
    }
}

/// A chapter at its checkpoint lands with the attachment's name, its summary,
/// the picks, the assumptions, the question and what the run cost — and the
/// question asks the assistant to put it to the user and wait.
#[test]
fn a_checkpoint_lands_with_everything_the_user_decides_on() {
    let en = locale(Lang::En);
    let at_checkpoint = Landed {
        chapter: Some(LandedChapter {
            number: 1,
            version: 2,
            text: "## One".to_owned(),
            summary: Some("  Mara learns of the debt. ".to_owned()),
        }),
        picks: vec![(2, "The quarrel shows the debt.".to_owned())],
        assumptions: vec![Assumption {
            doubt: "Whether Silas lies.".to_owned(),
            choice: "He lies.".to_owned(),
        }],
        spent: Spend {
            calls: 12,
            input_tokens: 9000,
            output_tokens: 1000,
            cost_usd: None,
        },
        ..landed(Ending::Paused(question(QuestionBody::Chapter {
            chapter: 1,
            version: 2,
        })))
    };
    let text = landing_text(en, &at_checkpoint, Some("s1 — chapter 1.md"));
    filled(&text);
    for want in [
        "Chapter 1 is written (version 2) and attached to this chat as «s1 — chapter 1.md».",
        "Summary: Mara learns of the debt.\n",
        "The chapter's scenes by number, and why each was kept:\n- Scene 2: The quarrel shows \
         the debt.",
        "could have been asked:\n- Whether Silas lies. — chosen: He lies.",
        "chapter 1 (version 2): accept it",
        "Ask them in your own words",
        "This run made 12 model call(s), 10000 tokens.",
    ] {
        assert!(text.contains(want), "{want:?} not in:\n{text}");
    }
    // Without the attachment nothing claims the chapter is in the chat.
    let unattached = landing_text(en, &at_checkpoint, None);
    assert!(!unattached.contains("attached"), "{unattached}");
    for loc in both() {
        filled(&landing_text(loc, &at_checkpoint, Some("x.md")));
    }
}

/// A cost is shown where the provider reported one and something was spent,
/// to the hundredth of a cent below a cent.
#[test]
fn a_cost_reads_as_spent_however_small() {
    let en = locale(Lang::En);
    let spent = |cost_usd| Landed {
        spent: Spend {
            calls: 3,
            input_tokens: 2000,
            output_tokens: 952,
            cost_usd,
        },
        ..landed(Ending::Finished)
    };
    let text = |cost| landing_text(en, &spent(cost), None);
    assert!(text(Some(0.0013329)).contains("2952 tokens, $0.0013."));
    assert!(text(Some(0.0232)).contains("2952 tokens, $0.02."));
    assert!(
        text(Some(0.0)).contains("2952 tokens."),
        "nothing spent shows no cost"
    );
    assert!(text(None).contains("2952 tokens."));
}

/// Every other ending says where the run stopped and what continuing does.
#[test]
fn every_ending_says_where_the_run_stopped() {
    let en = locale(Lang::En);
    let step = || "ch1/sc2".to_owned();
    let cases = [
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Cancelled,
            },
            "stopped at step ch1/sc2",
        ),
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Budget("2000000 tokens of 2000000".to_owned()),
            },
            "budget is spent (2000000 tokens of 2000000)",
        ),
        (
            Ending::Stopped {
                step: step(),
                why: Stop::Failed("the reply was cut".to_owned()),
            },
            "at step ch1/sc2: the reply was cut",
        ),
        (Ending::TimedOut { step: step() }, "time limit"),
        (Ending::Finished, "the story is finished"),
        (
            Ending::Failed("the story is open already".to_owned()),
            "could not run: the story is open already",
        ),
        (
            Ending::Paused(question(QuestionBody::Scene {
                chapter: 1,
                scene: 2,
                rerun: 2,
                errors: vec!["a".to_owned(), "b".to_owned()],
            })),
            "scene 2 of chapter 1 failed on every attempt (a; b)",
        ),
    ];
    for (ending, want) in cases {
        let landed = landed(ending);
        let text = landing_text(en, &landed, None);
        assert!(text.contains(want), "{want:?} not in:\n{text}");
        for loc in both() {
            filled(&landing_text(loc, &landed, None));
        }
    }
}

/// The small words: a quit's line with and without a step, the attachment's
/// name and its source (one a chapter), the question kinds, the language name.
#[test]
fn the_small_words_are_filled_in_both_languages() {
    let en = locale(Lang::En);
    assert!(quit_text(en, Some("ch1/sc1/b1")).contains("at step ch1/sc1/b1"));
    assert!(quit_text(en, None).contains("before the run made its first call"));
    assert_eq!(attachment_name(en, "s1", 3), "s1 — chapter 3.md");
    assert_eq!(
        attachment_name(locale(Lang::Ru), "s1", 3),
        "s1 — глава 3.md"
    );
    assert_eq!(attachment_source("c", "s1", 3), "story://c/s1/chapter/3");
    assert_ne!(
        attachment_source("c", "s1", 3),
        attachment_source("c", "s1", 4)
    );
    assert_eq!(human_language(en), "English");
    assert_eq!(human_language(locale(Lang::Ru)), "Russian");
    for loc in both() {
        filled(&quit_text(loc, Some("x")));
        filled(&quit_text(loc, None));
        for body in [
            QuestionBody::Canon,
            QuestionBody::Chapter {
                chapter: 1,
                version: 1,
            },
        ] {
            filled(&question_text(loc, &body));
        }
    }
}

/// The tool reads the chat's stories from the data directory, read-only, with
/// the board's run beside them; a chat without stories has none, and another
/// chat's stories are not this one's.
#[tokio::test]
async fn the_tool_reads_the_chats_stories_and_the_run_out() {
    let (_dir, storage, mut ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
    ctx.chat_id = Uuid::new_v4();
    let chat = ctx.chat_id.to_string();
    let none = StoryStatusTool.invoke(&ctx, json!({})).await.unwrap();
    assert_eq!(none.result, "This chat has no story.");

    let json_store = storage.json();
    for (chat, story) in [(chat.as_str(), "s1"), ("another-chat", "s9")] {
        let (db, log) = (
            json_store.story_db(chat, story),
            json_store.story_log(chat, story),
        );
        for path in [&db, &log] {
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
        }
        let store = StoryDb::open(&db, &log).await.unwrap();
        let mut author = store.author_writer().await.unwrap();
        author.import_canon(&fixture_canon()).await.unwrap();
        let mut runs = store.run_log_writer().await.unwrap();
        let run = runs.start_run(&json!({})).await.unwrap();
        let asked = author
            .add_question(&run, "canon", &QuestionBody::Canon)
            .await
            .unwrap();
        author
            .resolve(
                &asked.id,
                &Resolution {
                    decision: Decision::Approve,
                    notes: String::new(),
                    evidence: json!(null),
                    rerun: None,
                },
            )
            .await
            .unwrap();
        drop((author, runs));
        store.close().await.unwrap();
    }
    let run = Uuid::new_v4();
    ctx.story_board = Arc::default();
    ctx.story_board.claim(ctx.chat_id, "s1", run).unwrap();
    ctx.story_board.step(run, "ch1/outline");

    let text = StoryStatusTool
        .invoke(&ctx, json!({}))
        .await
        .unwrap()
        .result;

    assert!(text.starts_with("Story «s1»: "), "{text}");
    assert!(text.contains("it is now at ch1/outline"), "{text}");
    assert!(!text.contains("s9"), "{text}");
    assert!(!text.contains("not approved"), "{text}");
}

/// `story_status` is in the catalog and a profile's tools by default, and
/// reaches a request only while `tools.story` is on (spec §15: the switch is
/// the control, since a new tool is enabled in every existing profile).
#[test]
fn story_status_is_offered_only_with_the_switch_on() {
    use crate::features::tools::{ToolGates, default_tool_ids, effective_tool_ids};
    let enabled = default_tool_ids();
    assert!(enabled.iter().any(|t| t == STORY_STATUS_ID));
    let offered = |story: bool| {
        effective_tool_ids(
            &enabled,
            &ToolGates {
                story,
                ..ToolGates::default()
            },
        )
        .iter()
        .any(|t| t == STORY_STATUS_ID)
    };
    assert!(!offered(false));
    assert!(offered(true));
}
