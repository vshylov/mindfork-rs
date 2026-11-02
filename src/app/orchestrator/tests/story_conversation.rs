//! Orchestrator tests — a story driven from the chat (docs/fabula.md §4, PR 9;
//! docs/research/fabula-engine.md §14, *The chat tools and the human in the
//! loop*). The model's turns are scripted; the story is real, on disk.
//!
//! `story_create` stores the canon and opens P1 with no call; a
//! `story_continue` in the same turn is refused, since the person has not
//! answered; the next turn carries the question's reminder at its tail, the
//! person's own words are read by the Listener — and only those written after
//! the question — and the run goes out in the background with its record on
//! the call. The run's first call hangs until the app quits.

use super::parallel::KeyedRecorder;
use super::subagent::{Script, call, hang, load, text};
use super::*;
use crate::entities::story::{Decision, QuestionBody, fixture_canon};
use crate::entities::subagent::{RunKind, RunOutcome};
use crate::features::story::conversation::{ContinueRefusal, PendingAnswer};
use crate::features::tools::story::conversation as words;
use crate::shared::api::contract::ApiRole;
use crate::shared::storage::story::{StoryDb, story_status_of};

const FIRST: &str = "Write me a story about a harbour town. My sketches are attached.";
const APPROVAL: &str = "Yes, the canon is approved, go ahead.";

fn create_args() -> String {
    serde_json::json!({"title": "The Harbour", "canon": fixture_canon()}).to_string()
}

fn cfg() -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.tools.story = true;
    cfg.tools.subagent_background_wake = false;
    // One slot: a refusal that kept it would leave the approval's run none.
    cfg.tools.subagent_background_max = 1;
    cfg
}

/// The engine: the chat's turns by order, the Listener and the Author by
/// their prompts.
fn backend(parent: Vec<Script>) -> Arc<KeyedRecorder> {
    KeyedRecorder::new(
        vec![
            (
                "You read what a person wrote",
                vec![text(
                    r#"{"answers": [], "changes": [], "decision": "approve"}"#,
                )],
            ),
            ("You are the Author of a story", vec![hang("")]),
            ("", parent),
        ],
        1,
    )
}

/// The whole road from a canon to a run: P1 opened with no call, a premature
/// continue refused, the reminder at the next request's tail and in no stored
/// row, the answer read from the person's words after the question only, the
/// resolution's evidence naming their message, and the run out with its
/// record — cut short by the quit, which names where it was.
#[tokio::test]
async fn a_story_goes_from_its_canon_to_a_run_on_the_persons_own_words() {
    let backend = backend(vec![
        call("c1", "story_create", &create_args()),
        call("c2", "story_continue", r#"{"story": "the-harbour"}"#),
        text("Here is the canon. Do you approve it?"),
        call(
            "c3",
            "story_continue",
            r#"{"story": "the-harbour", "notes": "They want it tense."}"#,
        ),
        text("Chapter 1 is being written."),
    ]);
    let (dir, cmd_tx, mut rx, handle) =
        spawn_orch_cfg(Some(backend.clone() as Arc<dyn EngineBackend>), cfg());
    let active = wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };

    cmd_tx.send(AppCommand::SendMessage(FIRST.into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // P1 is open, and no model call was made for it.
    let root = dir.path().to_path_buf();
    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let json = storage.json();
    let (db, log) = (
        json.story_db(&chat_id.to_string(), "the-harbour"),
        json.story_log(&chat_id.to_string(), "the-harbour"),
    );
    let status = story_status_of(&db, &log).unwrap();
    assert_eq!(
        status.pending.as_ref().map(|q| &q.body),
        Some(&QuestionBody::Canon)
    );
    assert_eq!(status.spend.calls, 0);
    let p1 = status.pending.clone().unwrap();

    cmd_tx
        .send(AppCommand::SendMessage(APPROVAL.into()))
        .unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // The run is in its first call, the Author's, when the app quits.
    let author_asked = || {
        backend.requests().iter().any(|r| {
            r.system
                .as_deref()
                .is_some_and(|s| s.contains("You are the Author of a story"))
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !author_asked() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the run makes its first call");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(&root, chat_id);
    let result_of = |id: &str| -> String {
        chat.messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .map(|m| m.text.clone())
            .unwrap_or_else(|| panic!("the result of {id}"))
    };
    // The profile's language words every result (axis A).
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    assert_eq!(
        result_of("c1"),
        loc.tf(
            "tool.story_create.result.created",
            &[("title", "The Harbour"), ("id", "the-harbour")]
        )
    );
    // The same turn asked no question of the person yet: refused.
    assert_eq!(
        result_of("c2"),
        words::continue_refusal_text(loc, "the-harbour", &ContinueRefusal::NotAnswered, None)
    );
    let started = result_of("c3");
    assert!(
        started.contains(loc.t("tool.story_continue.resolved.canon")),
        "{started}"
    );

    // The second turn's first request carries the reminder at its tail, on the
    // person's message, and no stored row does.
    let line = words::reminder(
        loc,
        &[PendingAnswer {
            story: "the-harbour".to_owned(),
            question: p1,
            rows: Vec::new(),
        }],
    )
    .unwrap();
    let requests = backend.requests();
    let turn_two = requests
        .iter()
        .find(|r| {
            r.messages
                .iter()
                .any(|m| m.role == ApiRole::User && m.content.contains(APPROVAL))
        })
        .expect("the second turn's request");
    let last_user = turn_two
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ApiRole::User)
        .unwrap();
    assert_eq!(
        last_user.content,
        format!(
            "{APPROVAL}

{line}"
        )
    );
    let first_turn = requests
        .iter()
        .find(|r| r.messages.iter().all(|m| !m.content.contains(APPROVAL)))
        .unwrap();
    assert!(
        !first_turn
            .messages
            .iter()
            .any(|m| m.content.contains(&line)),
        "no question waited before the canon"
    );
    assert!(chat.messages.iter().all(|m| !m.text.contains(&line)));

    // The Listener read the person's words after the question, and only those.
    let listener = requests
        .iter()
        .find(|r| {
            r.system
                .as_deref()
                .is_some_and(|s| s.contains("You read what a person wrote"))
        })
        .expect("one reading");
    let asked = &listener.messages[0].content;
    assert!(asked.contains(APPROVAL), "{asked}");
    assert!(
        !asked.contains(FIRST),
        "a message before the question: {asked}"
    );
    assert!(
        !asked.contains("They want it tense."),
        "the assistant's notes: {asked}"
    );

    // The answer is recorded with the person's message as its evidence.
    let approval = chat.messages.iter().find(|m| m.text == APPROVAL).unwrap();
    // The cancelled run closes its story on its own task, after the quit.
    let store = loop {
        match StoryDb::open(&db, &log).await {
            Ok(store) => break store,
            Err(crate::shared::storage::story::StoryDbError::AlreadyOpen(_)) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(e) => panic!("{e}"),
        }
    };
    let (question, answer) = store.questions().await.unwrap().remove(0);
    assert_eq!(question.body, QuestionBody::Canon);
    let answer = answer.expect("answered");
    assert_eq!(answer.decision, Decision::Approve);
    assert_eq!(
        answer.evidence["messages"][0]["id"],
        approval.id.to_string()
    );
    assert_eq!(answer.evidence["assistant_notes"], "They want it tense.");
    assert!(store.canon_approved().await.unwrap());
    store.close().await.unwrap();

    // The run went out with its record on the call, and the quit cut it
    // short where it was: the Author's outline.
    let record = chat
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .find(|c| c.id == "c3")
        .and_then(|c| c.subagent.as_deref())
        .expect("the record on the call");
    assert_eq!(record.kind, RunKind::Story);
    assert!(record.background);
    assert_eq!(record.outcome, Some(RunOutcome::Cancelled));
    assert!(started.contains(&crate::features::chat_links::uri(record.id)));
    let closing = record.messages.last().unwrap().text.clone();
    assert!(closing.contains("ch1/outline"), "{closing}");
}

/// A turn the app woke on a notification answers no message of the
/// person's: `story_continue` there is refused even when their reply sits in
/// the chat, and no reading is made.
#[tokio::test]
async fn a_woken_turn_cannot_answer_for_the_person() {
    let (dir, mut orch, _rx, chat) = bare_with_chat(vec![Message::user(FIRST)]);
    orch.config.tools.story = true;
    let json = orch.storage.json();
    let files = crate::features::story::conversation::StoryFiles {
        db: json.story_db(&chat.to_string(), "the-harbour"),
        log: json.story_log(&chat.to_string(), "the-harbour"),
    };
    crate::features::story::conversation::create(
        &files,
        &orch.story_board,
        (chat, "the-harbour"),
        &fixture_canon(),
        false,
        "English",
    )
    .await
    .unwrap();
    // The person answered; then a notification arrived, the chat's last
    // user-side row.
    orch.chats[0].push_message(Message::user(APPROVAL));
    orch.chats[0].push_message(Message::notification(Uuid::new_v4(), "[Task notification]"));
    let pending =
        super::super::story_runs::pending_answers(&orch.storage, &orch.story_board, &orch.chats[0]);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rows.len(), 1, "the person's reply is there");
    assert_eq!(pending[0].rows[0].text, APPROVAL);
    let times = super::super::request::user_message_times(&orch.chats[0]);
    assert_eq!(times.answering, None, "a woken turn answers none of theirs");
    let refused = crate::features::story::conversation::prepare(
        &files,
        &orch.story_board,
        (chat, "the-harbour", Uuid::new_v4()),
        crate::features::story::conversation::Answering {
            pending: pending.first(),
            answering: times.answering.is_some(),
            notes: None,
        },
        crate::features::story::conversation::Listener {
            backend: Arc::new(crate::shared::api::mock::MockBackend::sequence(Vec::new())),
            model: "m".to_owned(),
            sessions: None,
            cancel: Default::default(),
            fallback_only: false,
        },
        Default::default(),
    )
    .await;
    assert_eq!(
        refused,
        Err(crate::features::story::conversation::ContinueRefusal::NotAnswered)
    );
    drop(dir);
}

/// The snapshot counts only the person's rows after the question was put:
/// for a run's question, after the run's landing; notifications never.
#[tokio::test]
async fn only_rows_after_the_question_count() {
    use crate::features::story::run::tests::{at_checkpoint, write_story};
    let (_dir, mut orch, _rx, chat) = bare_with_chat(vec![Message::user("Before anything.")]);
    let json = orch.storage.json();
    let (db, log) = (
        json.story_db(&chat.to_string(), "s1"),
        json.story_log(&chat.to_string(), "s1"),
    );
    write_story(&db, &log, async |db, author| {
        at_checkpoint(db, author).await
    })
    .await;
    // A reply written while the run was out, before its landing, is not an
    // answer to the question the landing carries.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    orch.chats[0].push_message(Message::user("Typed while it ran."));
    let mut carrier = Message::assistant("");
    let mut record = crate::entities::subagent::SubagentRun {
        id: Uuid::new_v4(),
        kind: RunKind::Story,
        title: "s1".into(),
        renamed_manually: false,
        name: Some("s1".into()),
        created_at: chrono::Utc::now(),
        finished_at: None,
        system_message: String::new(),
        sampling_override: None,
        messages: Vec::new(),
        outcome: None,
        tokens: 0,
        participants: Vec::new(),
        background: true,
    }
    .on_record();
    record.name = "story_continue".into();
    record.arguments = serde_json::json!({"story": "s1"});
    let run = record.subagent.as_ref().unwrap().id;
    carrier.tool_calls = vec![record];
    orch.chats[0].messages.insert(1, carrier);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    orch.chats[0].push_message(Message::notification(run, "[Task notification] chapter 1"));
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    orch.chats[0].push_message(Message::user("Accept it."));
    let pending =
        super::super::story_runs::pending_answers(&orch.storage, &orch.story_board, &orch.chats[0]);
    assert_eq!(pending.len(), 1);
    let texts: Vec<&str> = pending[0].rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["Accept it."]);
    // A story a run is writing waits on nothing here.
    orch.story_board.claim(chat, "s1", Uuid::new_v4()).unwrap();
    assert!(
        super::super::story_runs::pending_answers(&orch.storage, &orch.story_board, &orch.chats[0])
            .is_empty()
    );
}

/// The turn the app wakes on a notification answers no message of the
/// person's, so its `story_continue` is refused even though their reply waits
/// in the chat — and no reading is made; its request carries the reminder on
/// the notification it answers.
#[tokio::test]
async fn a_woken_turn_cannot_call_story_continue_for_the_person() {
    let critic = r#"{"name":"Critic","system_message":"be harsh","message":"what time is it"}"#;
    let backend = KeyedRecorder::new(
        vec![
            (
                "You read what a person wrote",
                vec![text(
                    r#"{"answers": [], "changes": [], "decision": "approve"}"#,
                )],
            ),
            ("You are the Author of a story", vec![hang("")]),
            ("be harsh", vec![text("It is noon.")]),
            (
                "",
                vec![
                    call("c1", "story_create", &create_args()),
                    text("Here is the canon. Do you approve it?"),
                    call("c2", "start_subagent", critic),
                    text("Noted; a critic is reading it meanwhile."),
                    call("c3", "story_continue", r#"{"story": "the-harbour"}"#),
                    text("I will wait for you."),
                ],
            ),
        ],
        1,
    );
    let mut cfg = cfg();
    cfg.tools.subagent_background = true;
    cfg.tools.subagent_background_wake = true;
    cfg.tools.subagent_background_max = 2;
    let (dir, cmd_tx, mut rx, handle) =
        spawn_orch_cfg(Some(backend.clone() as Arc<dyn EngineBackend>), cfg);
    let active = wait_for(&mut rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let AppEvent::ChatActivated { id: chat_id, .. } = active else {
        unreachable!()
    };
    cmd_tx.send(AppCommand::SendMessage(FIRST.into())).unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // The person approves; the reply calls nothing for the story.
    cmd_tx
        .send(AppCommand::SendMessage(APPROVAL.into()))
        .unwrap();
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    // The critic lands and the app wakes the assistant.
    wait_for(&mut rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let root = dir.path().to_path_buf();
    let chat = load(&root, chat_id);
    let refused = chat
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("c3"))
        .map(|m| m.text.clone())
        .expect("the woken turn's call");
    let storage = Storage::open(Paths::with_root(&root)).unwrap();
    let json = storage.json();
    let status = story_status_of(
        &json.story_db(&chat_id.to_string(), "the-harbour"),
        &json.story_log(&chat_id.to_string(), "the-harbour"),
    )
    .unwrap();
    let p1 = status.pending.clone().expect("P1 still waits");
    assert!(!status.canon_approved);
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::default());
    let pending = PendingAnswer {
        story: "the-harbour".to_owned(),
        question: p1,
        rows: Vec::new(),
    };
    assert_eq!(
        refused,
        words::continue_refusal_text(
            loc,
            "the-harbour",
            &ContinueRefusal::NotAnswered,
            Some(&pending)
        )
    );
    let requests = backend.requests();
    assert!(
        !requests.iter().any(|r| r
            .system
            .as_deref()
            .is_some_and(|s| s.contains("You read what a person wrote"))),
        "no reading on a woken turn"
    );
    // The woken turn answered the notification, which carried the reminder.
    let line = words::reminder(loc, std::slice::from_ref(&pending)).unwrap();
    let woken = requests
        .iter()
        .rev()
        .find(|r| {
            r.messages
                .last()
                .is_some_and(|m| m.role == ApiRole::User && m.content.contains(&line))
        })
        .expect("the woken turn's request");
    assert!(
        !woken.messages.last().unwrap().content.starts_with(APPROVAL),
        "the woken turn answers the notification"
    );
}
