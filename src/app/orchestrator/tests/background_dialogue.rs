//! Orchestrator tests — a directed dialogue out in the **background**
//! (`start_dialogue`, spec §9.13, docs/research/background-dialogues.md §4):
//! the call answers at once and the scene outlives the turn, its record
//! lands with the turn as a `kind: Dialogue` placeholder and is filled in by
//! id when the director stops the scene, and the result reaches the model as
//! a task notification worded for a dialogue. The seat, the landing, the
//! stop and the wake are the sub-agent track's and are tested there
//! ([`super::background`]); what is tested here is what differs.
//!
//! Fixtures: the keyed engine of [`super::parallel`] plus the dialogue
//! suite's script helpers — one source for both.

use super::background::{begin, finished, next, nothing_like, running_run, runs_out};
use super::parallel::KeyedRecorder;
use super::subagent::{Script, load, text};
use super::*;
use crate::entities::subagent::{RunKind, RunOutcome};
use crate::shared::api::contract::{ApiRole, ToolCallDelta};

/// The scene the tests stage: two personas and a checkpoint after every
/// second line, with room to spare under the cap — so the scene ends by the
/// director's decision, which is the ending the notification has to carry.
const CAFE: &str = r#"{"a":{"name":"Mara","system_message":"barista"},
    "b":{"name":"Jonas","system_message":"customer"},
    "opening":{"speaker":"b","text":"Wrong drink?"},
    "scene":"Cafe.","direction":"Stop at the goodbye.",
    "max_messages":4,"moderate_every":2}"#;

/// One round with one `start_dialogue` call.
fn start(id: &str) -> Script {
    super::subagent::call(id, "start_dialogue", CAFE)
}

/// A checkpoint reply carrying one verdict.
fn verdict(name: &str, args: &str) -> Script {
    Script {
        chunks: vec![
            ChatChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("v0".into()),
                name: Some(name.into()),
                arguments: args.to_string(),
                thought_signature: None,
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    }
}

fn cfg(sessions: u32) -> AppConfig {
    let mut cfg = no_auto_cfg();
    cfg.tools.subagent_background = true;
    cfg.engine.managed.sessions = sessions;
    cfg.engine.managed.context_size = 65_536;
    cfg
}

/// The engine every scene test scripts: the parent's two rounds keyed by the
/// empty system, the participants keyed by their personas, and the director
/// keyed by the direction it carries.
fn scene_backend(parent: Vec<Script>) -> Arc<KeyedRecorder> {
    // The director's key comes first: its system carries the parent's
    // conversation brief, which quotes the call's own arguments — so the
    // personas' keys appear in it too, and the queue that wins is whichever
    // matches first (`KeyedRecorder`).
    KeyedRecorder::new(
        vec![
            (
                "Stop at the goodbye.",
                vec![verdict(
                    "dialogue_stop",
                    r#"{"reason":"Resolved","summary":"All good"}"#,
                )],
            ),
            ("barista", vec![text("Sorry! Remaking it now.")]),
            ("customer", vec![text("Thanks — bye!")]),
            ("", parent),
        ],
        10,
    )
}

/// The scene runs outside the turn and lands by id: the call answers with the
/// *started* line naming the transcript, the record lands as a `Dialogue`
/// placeholder with its participants and the caller's opening, and when the
/// director stops the scene the record is filled in — with the notification
/// carrying the closing result in the **dialogue's** wording, and the woken
/// assistant reporting it.
#[tokio::test]
async fn a_background_scene_lands_by_id_and_notifies_in_its_own_words() {
    let backend = scene_backend(vec![
        start("c1"),
        text("staged it"),
        text("they sorted the order out"),
    ]);
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;

    // The placeholder is a row the moment the turn lands: a running scene.
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let record = chat.messages[1].tool_calls[0]
        .subagent
        .as_deref()
        .expect("the placeholder became the run");
    assert_eq!(record.id, run_id);
    assert_eq!(record.kind, RunKind::Dialogue);
    assert!(record.background);
    assert_eq!(record.outcome, Some(RunOutcome::Completed));
    assert_eq!(record.participants.len(), 2);
    assert_eq!(record.title, "Mara ↔ Jonas");
    // The scene itself: the opening, the two lines, the closing intervention.
    let roles: Vec<MessageRole> = record.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::System,
        ]
    );

    // The call answered at once with the address, not with the scene.
    let address = crate::features::chat_links::uri(run_id);
    let started = &chat.messages[2].text;
    assert!(started.contains(&address), "{started}");
    assert!(!started.contains("Resolved"), "{started}");

    // The notification: a dialogue's wording, the closing result as its body.
    let note = chat
        .messages
        .iter()
        .find(|m| m.is_notification())
        .expect("a task notification row");
    assert_eq!(note.notification, Some(run_id));
    assert!(note.text.contains("Mara ↔ Jonas"), "{}", note.text);
    assert!(note.text.contains("Resolved"), "{}", note.text);
    assert!(note.text.contains("All good"), "{}", note.text);
    // The wording is the dialogue's, in whichever language the profile
    // speaks — and never the sub-agent's (fork F8). Both keys are rendered
    // with the run's own values, so the assertion is about which text the
    // landing chose, not about the words themselves.
    let rendered = |key: &str| -> Vec<String> {
        [crate::shared::i18n::Lang::En, crate::shared::i18n::Lang::Ru]
            .into_iter()
            .map(|lang| {
                crate::shared::i18n::locale(lang).tf(
                    key,
                    &[
                        ("name", "Mara ↔ Jonas"),
                        ("address", &address),
                        ("body", &record.messages[3].text.clone()),
                    ],
                )
            })
            .collect()
    };
    let head = |t: &str| t.split("\n\n").next().unwrap_or_default().to_string();
    let mine = head(&note.text);
    assert!(
        rendered("tool.start_dialogue.notification")
            .iter()
            .any(|t| head(t) == mine),
        "not the dialogue's wording: {}",
        note.text
    );
    assert!(
        !rendered("tool.start_subagent.notification")
            .iter()
            .any(|t| head(t) == mine),
        "the sub-agent's wording leaked into a dialogue's notification: {}",
        note.text
    );
    // ...and the woken turn reported it.
    assert_eq!(
        chat.messages.last().unwrap().text,
        "they sorted the order out"
    );
}

/// `Esc` ends the turn and leaves the scene running; `/subagents stop` — the
/// route `F6` on the open transcript takes — ends it, and the partial
/// transcript lands as *cancelled*.
#[tokio::test]
async fn esc_leaves_the_scene_out_and_the_stop_lands_it_cancelled() {
    let backend = KeyedRecorder::new(
        vec![
            ("barista", vec![super::subagent::hang("thinking")]),
            ("", vec![start("c1"), text("staged it"), text("noted")]),
        ],
        10,
    );
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    let run_id = running_run(&mut rx).await;
    next(&mut rx, finished).await;

    cmd_tx.send(AppCommand::Cancel).unwrap();
    nothing_like(&mut rx, runs_out(0)).await;

    cmd_tx
        .send(AppCommand::StopSubagentRun { id: run_id })
        .unwrap();
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let run = chat.messages[1].tool_calls[0].subagent.as_deref().unwrap();
    assert_eq!(run.kind, RunKind::Dialogue);
    assert_eq!(run.outcome, Some(RunOutcome::Cancelled));
    assert!(chat.messages.iter().any(|m| m.is_notification()));
}

/// The scene's streams are priced by the app's session budget (fork F4): at
/// one session the scene's requests and the parent's turns take turns, and
/// the counting engine never sees two at once. Before the budget reached
/// `dialogue_stream` this was the one overlap nothing prevented.
#[tokio::test]
async fn with_one_session_the_scene_and_the_turns_take_turns() {
    let backend = scene_backend(vec![
        start("c1"),
        text("staged it"),
        text("they sorted the order out"),
    ]);
    let (_dir, cmd_tx, mut rx, handle, _chat_id) = begin(backend.clone(), cfg(1)).await;
    // In the order the events actually arrive: `wait_for` drains, so the
    // turn's end has to be pulled before the scene's (docs/lessons.md §2).
    next(&mut rx, runs_out(1)).await;
    next(&mut rx, finished).await;
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    assert_eq!(
        backend.max_in_flight(),
        1,
        "a scene and a turn overlapped on a one-session engine"
    );
}

/// The cap counts both kinds (fork F7): with a background sub-agent already
/// out, a `start_dialogue` past the cap is refused with the setting's name
/// and no scene is started.
#[tokio::test]
async fn the_shared_cap_refuses_a_scene_past_it() {
    let both = Script {
        chunks: vec![
            ChatChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("s1".into()),
                name: Some("start_subagent".into()),
                arguments: r#"{"name":"Critic","system_message":"be harsh","message":"rate X"}"#
                    .into(),
                thought_signature: None,
            }),
            ChatChunk::ToolCall(ToolCallDelta {
                index: 1,
                id: Some("d1".into()),
                name: Some("start_dialogue".into()),
                arguments: CAFE.into(),
                thought_signature: None,
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        hang: false,
    };
    let backend = KeyedRecorder::new(
        vec![
            ("be harsh", vec![super::subagent::hang("thinking")]),
            ("", vec![both, text("one of them started")]),
        ],
        10,
    );
    let mut cfg = cfg(2);
    cfg.tools.subagent_background_max = 1;
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    let calls = &chat.messages[1].tool_calls;
    assert_eq!(calls.len(), 2);
    let refusal = calls[1].result.as_deref().unwrap();
    assert!(
        refusal.contains("subagent_background_max"),
        "the refusal names the setting: {refusal}"
    );
    assert!(
        calls[1].subagent.is_none(),
        "a refused scene leaves no record"
    );
}

/// The catalog is byte-identical with the switch off and carries the twin
/// with it on — the same gate `start_subagent` sits behind (fork F2).
#[test]
fn start_dialogue_is_gated_by_the_setting() {
    use crate::features::tools::{ToolGates, default_tool_ids, effective_tool_ids};
    let all = default_tool_ids();
    let gates = |background: bool| ToolGates {
        web: true,
        python: true,
        fs: true,
        mcp: true,
        background,
        history: false,
        workspace: false,
        workspace_commands: Default::default(),
        sampling_provider: None,
    };
    let off = effective_tool_ids(&all, &gates(false));
    let on = effective_tool_ids(&all, &gates(true));
    assert!(!off.iter().any(|t| t == "start_dialogue"));
    assert!(on.iter().any(|t| t == "start_dialogue"));
    assert!(
        off.iter().any(|t| t == "run_dialogue"),
        "the twin is not the gate"
    );
    let added: Vec<&String> = on.iter().filter(|t| !off.contains(t)).collect();
    assert_eq!(
        added.len(),
        2,
        "the switch adds exactly the two twins: {added:?}"
    );
}

/// A turn the app started on a notification whose first generation comes back
/// **empty** — the whole reply cap spent thinking, measured 2 in 5 on the gate
/// model (research §3) — is re-asked once with thinking muted, and the second
/// reply is what lands. An ordinary turn is not re-asked.
#[tokio::test]
async fn a_woken_turn_with_an_empty_first_round_is_re_asked_muted() {
    let empty = Script {
        chunks: vec![ChatChunk::Finished(FinishReason::Length)],
        hang: false,
    };
    let backend = scene_backend(vec![
        start("c1"),
        text("staged it"),
        empty,
        text("recovered: they sorted it out"),
    ]);
    let (dir, cmd_tx, mut rx, handle, chat_id) = begin(backend.clone(), cfg(2)).await;
    next(&mut rx, runs_out(1)).await;
    next(&mut rx, finished).await;
    next(&mut rx, runs_out(0)).await;
    next(&mut rx, finished).await;
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let chat = load(dir.path(), chat_id);
    assert_eq!(
        chat.messages.last().unwrap().text,
        "recovered: they sorted it out",
        "the muted re-ask is what the chat kept"
    );
    // The re-ask is the same request with thinking muted, and it is the last
    // one the parent's context sent.
    let parent: Vec<_> = backend
        .requests()
        .into_iter()
        .filter(|r| r.messages.iter().any(|m| m.role == ApiRole::User))
        .collect();
    let last = parent.last().unwrap();
    assert_eq!(last.sampling.reasoning_budget, Some(0));
}
