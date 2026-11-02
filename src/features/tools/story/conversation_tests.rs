//! `story_create` and `story_continue` as the model meets them (docs/fabula.md
//! §4, PR 9): their arguments, their schema against the canon's own shape,
//! the tool end to end on a chat's storage, and every text in both languages
//! with no placeholder left unfilled.

use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::entities::story::{Question, QuestionId, RunId, fixture_canon};
use crate::features::story::answer::{Reading, ReadingReply, UserRow};
use crate::features::story::board::RunView;
use crate::features::tools::Tool;
use crate::features::tools::testkit::ctx_with_storage_lang;
use crate::shared::i18n::{Lang, Locale, locale};
use crate::shared::storage::story::story_status_of;

fn both() -> [&'static Locale; 2] {
    [locale(Lang::En), locale(Lang::Ru)]
}

fn filled(text: &str) {
    assert!(
        !text.contains('{') && !text.contains('}'),
        "an unfilled placeholder: {text}"
    );
}

fn canon_value() -> Value {
    serde_json::to_value(fixture_canon()).unwrap()
}

fn question(body: QuestionBody) -> Question {
    Question {
        id: QuestionId::new("q1"),
        run: RunId::nth(1),
        step: "canon".to_owned(),
        body,
    }
}

/// The canon arrives as an object, or as a JSON string some models send for
/// a nested object; a misspelt field is refused by name, not read as absent.
#[test]
fn the_arguments_read_the_canon_in_either_shape() {
    let object =
        CreateArgs::parse(&json!({"title": " The Harbour ", "canon": canon_value()})).unwrap();
    assert_eq!(object.title.as_deref(), Some("The Harbour"));
    assert_eq!(object.story, None);
    assert_eq!(object.canon, fixture_canon());
    let string = CreateArgs::parse(&json!({
        "story": "harbour",
        "canon": canon_value().to_string(),
    }))
    .unwrap();
    assert_eq!(string.story.as_deref(), Some("harbour"));
    assert_eq!(string.canon, fixture_canon());
    let mut misspelt = canon_value();
    misspelt["arc"]["premiss"] = json!("x");
    let err = CreateArgs::parse(&json!({"canon": misspelt})).unwrap_err();
    assert!(err.contains("premiss"), "{err}");
    // A title inside the canon is the story's, the outer one first.
    let mut titled = canon_value();
    titled["title"] = json!(" Inside ");
    let inner = CreateArgs::parse(&json!({"canon": titled})).unwrap();
    assert_eq!(inner.title.as_deref(), Some("Inside"));
    assert_eq!(inner.canon, fixture_canon());
    let outer = CreateArgs::parse(&json!({"title": "Outer", "canon": titled})).unwrap();
    assert_eq!(outer.title.as_deref(), Some("Outer"));
    assert!(CreateArgs::parse(&json!({"title": "x"})).is_err());
    assert!(CreateArgs::parse(&json!({"canon": 3})).is_err());
    assert_eq!(
        ContinueArgs::parse(&json!({"story": " harbour ", "notes": ""})).unwrap(),
        ContinueArgs {
            story: "harbour".to_owned(),
            notes: None,
        }
    );
    assert!(ContinueArgs::parse(&json!({"notes": "x"})).is_err());
}

/// Every key of a canon the fixture holds is a property the parameters
/// describe, at the same place: the schema the model reads cannot drift from
/// the shape the engine parses.
#[test]
fn the_parameters_describe_every_field_of_a_canon() {
    fn walk(value: &Value, schema: &Value, path: &str) {
        match value {
            Value::Object(map) => {
                let props = schema["properties"]
                    .as_object()
                    .unwrap_or_else(|| panic!("{path} is described as an object"));
                for (key, inner) in map {
                    let at = format!("{path}.{key}");
                    let described = props
                        .get(key)
                        .unwrap_or_else(|| panic!("{at} is not described"));
                    walk(inner, described, &at);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, &schema["items"], &format!("{path}[]"));
                }
            }
            _ => assert!(schema.get("type").is_some(), "{path} has a type"),
        }
    }
    for loc in both() {
        let params = StoryCreateTool.parameters(loc);
        walk(&canon_value(), &params["properties"]["canon"], "canon");
        assert_eq!(params["required"], json!(["canon"]));
        // Every description says something, in this language.
        let text = params.to_string();
        assert!(!text.contains("\"description\":\"\""), "{text}");
        assert!(!text.contains("param."), "a missing key: {text}");
    }
    let params = StoryContinueTool.parameters(locale(Lang::En));
    assert_eq!(params["required"], json!(["story"]));
}

/// `story_create` end to end on a chat's storage: a story is stored under its
/// title's id with P1 open, a second under the next free id; a revision names
/// a story of the chat or is refused, a path included; a broken canon stores
/// nothing and says why.
#[tokio::test]
async fn story_create_stores_drafts_under_their_titles() {
    let (_dir, _storage, ctx) = ctx_with_storage_lang(Uuid::new_v4(), Lang::En);
    let args = json!({"title": "The Harbour", "canon": canon_value()});
    let created = StoryCreateTool
        .invoke(&ctx, args.clone())
        .await
        .unwrap()
        .result;
    assert!(created.contains("with the id «the-harbour»"), "{created}");
    assert!(created.contains("story_continue"), "{created}");
    filled(&created);
    let files = files_of(&ctx, "the-harbour");
    let status = story_status_of(&files.db, &files.log).unwrap();
    assert_eq!(status.pending.unwrap().body, QuestionBody::Canon);
    assert!(has_story(&ctx, "the-harbour"));
    let second = StoryCreateTool.invoke(&ctx, args).await.unwrap().result;
    assert!(second.contains("«the-harbour-2»"), "{second}");

    let revised = StoryCreateTool
        .invoke(
            &ctx,
            json!({"story": "the-harbour", "canon": canon_value()}),
        )
        .await
        .unwrap()
        .result;
    assert!(revised.contains("is revised"), "{revised}");
    for story in ["nowhere", "../the-harbour", "the-harbour.db"] {
        let refused = StoryCreateTool
            .invoke(&ctx, json!({"story": story, "canon": canon_value()}))
            .await
            .unwrap()
            .result;
        assert!(
            refused.contains("This chat has no story"),
            "{story}: {refused}"
        );
    }
    // Another chat's story is out of reach, even by a path that names its file.
    let other = Uuid::new_v4().to_string();
    let json = ctx.storage.json();
    let theirs = StoryFiles {
        db: json.story_db(&other, "x"),
        log: json.story_log(&other, "x"),
    };
    crate::features::story::conversation::create(
        &theirs,
        &ctx.story_board,
        (Uuid::new_v4(), "x"),
        &fixture_canon(),
        false,
        "English",
    )
    .await
    .unwrap();
    let before = std::fs::read(&theirs.db).unwrap();
    let mut changed = canon_value();
    changed["arc"]["premise"] = json!("Taken over.");
    let reached = StoryCreateTool
        .invoke(
            &ctx,
            json!({"story": format!("../{other}/x"), "canon": changed}),
        )
        .await
        .unwrap()
        .result;
    assert!(reached.contains("This chat has no story"), "{reached}");
    assert_eq!(std::fs::read(&theirs.db).unwrap(), before, "untouched");
    let mut broken = canon_value();
    broken["locations"] = json!([]);
    let problems = StoryCreateTool
        .invoke(&ctx, json!({"title": "Broken", "canon": broken}))
        .await
        .unwrap()
        .result;
    assert!(
        problems.contains("the canon has no locations"),
        "{problems}"
    );
    assert!(!has_story(&ctx, "broken"));
    let untitled = StoryCreateTool
        .invoke(&ctx, json!({"canon": canon_value()}))
        .await
        .unwrap()
        .result;
    assert_eq!(untitled, "A new story needs a title. Nothing was stored.");
    let unreadable = StoryCreateTool
        .invoke(&ctx, json!({"title": "x", "canon": "{not json"}))
        .await
        .unwrap()
        .result;
    assert!(
        unreadable.starts_with("The canon could not be read"),
        "{unreadable}"
    );
    // Outside a turn's loop, story_continue says so and starts nothing.
    let outside = StoryContinueTool
        .invoke(&ctx, json!({"story": "the-harbour"}))
        .await
        .unwrap()
        .result;
    assert!(
        outside.contains("only runs inside a chat turn"),
        "{outside}"
    );
}

/// Every text the two tools give back reads in both languages, filled in.
#[test]
fn every_text_reads_filled_in_both_languages() {
    let view = RunView {
        chat: Uuid::nil(),
        story: "s".to_owned(),
        run: Uuid::nil(),
        started_at: chrono::Utc::now(),
        step: None,
    };
    let pending = PendingAnswer {
        story: "harbour".to_owned(),
        question: question(QuestionBody::Chapter {
            chapter: 2,
            version: 1,
        }),
        rows: vec![UserRow {
            id: Uuid::nil(),
            at: chrono::Utc::now(),
            text: "x".to_owned(),
            voice: false,
        }],
    };
    for loc in both() {
        for refusal in [
            CreateRefusal::Problems(vec!["one".to_owned(), "two".to_owned()]),
            CreateRefusal::Unknown,
            CreateRefusal::Approved,
            CreateRefusal::Busy(view.clone()),
            CreateRefusal::Failed("disk".to_owned()),
        ] {
            filled(&create_refusal_text(loc, "harbour", &refusal, false));
            filled(&create_refusal_text(loc, "harbour", &refusal, true));
        }
        let unclear = ReadingReply {
            answers: Vec::new(),
            changes: Vec::new(),
            decision: Reading::Unclear,
        };
        for refusal in [
            ContinueRefusal::Unknown,
            ContinueRefusal::Busy(view.clone()),
            ContinueRefusal::Finished,
            ContinueRefusal::BudgetSpent {
                chapter: 1,
                spent: "2000100 tokens of 2000000".to_owned(),
            },
            ContinueRefusal::NotAnswered,
            ContinueRefusal::Unclear(unclear),
            ContinueRefusal::Failed("refused".to_owned()),
        ] {
            let text = continue_refusal_text(loc, "harbour", &refusal, Some(&pending));
            filled(&text);
        }
        let asks = continue_refusal_text(
            loc,
            "harbour",
            &ContinueRefusal::NotAnswered,
            Some(&pending),
        );
        assert!(
            asks.contains(&question_text(loc, &pending.question.body)),
            "{asks}"
        );
        for resolved in [
            None,
            Some((QuestionBody::Canon, Decision::Approve)),
            Some((
                QuestionBody::Chapter {
                    chapter: 1,
                    version: 2,
                },
                Decision::Approve,
            )),
            Some((
                QuestionBody::Chapter {
                    chapter: 1,
                    version: 2,
                },
                Decision::Change,
            )),
            Some((
                QuestionBody::Scene {
                    chapter: 1,
                    scene: 2,
                    rerun: 2,
                    errors: Vec::new(),
                },
                Decision::Answer,
            )),
        ] {
            let started = started_text(loc, "harbour", resolved.as_ref(), "chat://abc");
            filled(&started);
            assert!(started.contains("chat://abc"), "{started}");
            filled(&brief_text(loc, resolved.as_ref()));
        }
        filled(&canon_change_text(loc, "harbour", "- Make Mara older."));
        filled(&finished_text(loc, "harbour", 2));
        let line = reminder(loc, &[pending.clone(), pending.clone()]).unwrap();
        filled(&line);
        assert_eq!(line.lines().count(), 2);
        assert!(line.contains("story_continue"), "{line}");
    }
    assert_eq!(reminder(locale(Lang::En), &[]), None);
}
