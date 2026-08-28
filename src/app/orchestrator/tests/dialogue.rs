//! Orchestrator tests — the directed dialogue (`run_dialogue`, spec §9.13,
//! docs/research/two-agent-dialogue.md §3): the loop runs two persona
//! contexts and a director context strictly in turn on one scripted engine,
//! and the run lands on the call's record. Fixtures shared with the sub-agent
//! suite ([`super::subagent`]) — the same engine-script shape, one source.

use super::subagent::{Script, call, load, reopen, run_turn, text};
use super::*;
use crate::entities::subagent::{RunKind, RunOutcome};
use crate::shared::api::contract::ToolCallDelta;

/// A checkpoint reply carrying one or more verdict calls.
fn verdicts(spec: &[(&str, &str)]) -> Script {
    let mut chunks: Vec<ChatChunk> = spec
        .iter()
        .enumerate()
        .map(|(i, (name, args))| {
            ChatChunk::ToolCall(ToolCallDelta {
                index: i,
                id: Some(format!("v{i}")),
                name: Some((*name).into()),
                arguments: (*args).to_string(),
                thought_signature: None,
            })
        })
        .collect();
    chunks.push(ChatChunk::Finished(FinishReason::ToolCalls));
    Script {
        chunks,
        hang: false,
    }
}

const CAFE: &str = r#"{"a":{"name":"Mara","system_message":"barista"},
    "b":{"name":"Jonas","system_message":"customer"},
    "opening":{"speaker":"b","text":"Wrong drink?"},
    "scene":"Cafe.","direction":"Stop at the goodbye.",
    "max_messages":6,"moderate_every":2}"#;

/// The happy path: lines alternate, the director continues once and stops at
/// the goodbye; the run lands on the record with the participants, the
/// role-encoded transcript, the closing intervention and a result that names
/// the reason and the address.
#[tokio::test]
async fn dialogue_runs_alternating_and_lands_on_the_record() {
    let (dir, backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", CAFE),
            text("Sorry! Remaking it now."),
            text("Thanks — quick please."),
            verdicts(&[("dialogue_continue", "{}")]),
            text("Here you go!"),
            text("Perfect, bye!"),
            verdicts(&[(
                "dialogue_stop",
                r#"{"reason":"Resolved","summary":"All good"}"#,
            )]),
            text("Dialogue staged."),
        ],
        no_auto_cfg(),
    )
    .await;

    let chat = load(dir.path(), chat_id);
    let record = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .expect("the call's record");
    let run = record.subagent.as_deref().expect("the run on the record");
    assert_eq!(run.kind, RunKind::Dialogue);
    assert_eq!(run.outcome, Some(RunOutcome::Completed));
    assert_eq!(run.participants.len(), 2);
    assert_eq!(run.participants[0].name.as_deref(), Some("Mara"));
    assert_eq!(run.participants[1].name.as_deref(), Some("Jonas"));
    assert_eq!(run.title, "Mara ↔ Jonas");
    // The role-encoded transcript: b = User (opening included), a = Assistant,
    // the stop verdict as a closing System intervention.
    let roles: Vec<MessageRole> = run.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::System,
        ]
    );
    assert!(run.messages[5].text.contains("Resolved"));
    assert!(run.tokens > 0, "the run records what it cost");
    // The result closes the door: the reason, the summary, the address.
    let result = record.result.as_deref().unwrap();
    assert!(result.contains("Resolved"), "{result}");
    assert!(result.contains("All good"), "{result}");
    assert!(result.contains("chat://"), "{result}");
    // The parent's reply followed the tool result.
    assert!(chat.messages.iter().any(|m| m.text == "Dialogue staged."));

    // The wire shapes (research §3.2): every view strictly alternating, the
    // scene merged into the first user turn where the neighbour's line
    // follows it, no tools on participants, the verdict five on checkpoints.
    let reqs = backend.requests();
    assert_eq!(reqs.len(), 8);
    // Participant a's first view: scene + b's opening merged into one user turn.
    assert_eq!(reqs[1].system.as_deref(), Some("barista"));
    assert!(reqs[1].tools.is_empty());
    assert_eq!(reqs[1].messages.len(), 1);
    assert_eq!(reqs[1].messages[0].content, "Cafe.\n\nWrong drink?");
    // Participant b's view: the scene, its own opening as assistant, a's line.
    assert_eq!(reqs[2].system.as_deref(), Some("customer"));
    assert_eq!(reqs[2].messages.len(), 3);
    assert_eq!(reqs[2].messages[1].content, "Wrong drink?");
    // The first checkpoint: the verdict vocabulary, thinking muted, the
    // director's system carrying the direction, the script naming the lines.
    let cp = &reqs[3];
    let names: Vec<&str> = cp.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "dialogue_continue",
            "dialogue_stop",
            "dialogue_note",
            "dialogue_retry",
            "dialogue_rewrite"
        ]
    );
    assert_eq!(cp.sampling.reasoning_budget, Some(0));
    assert!(
        cp.system
            .as_deref()
            .unwrap()
            .contains("Stop at the goodbye.")
    );
    assert!(cp.messages[0].content.contains("Jonas: Wrong drink?"));
    // The second checkpoint continues the director's own conversation: the
    // first ask, its verdict turn, the tool acknowledgement, the new lines.
    let cp2 = &reqs[6];
    assert_eq!(cp2.messages.len(), 4);
    assert!(cp2.messages[3].content.contains("Here you go!"));
}

/// The cap is the backstop: with `max_messages` spent the run lands as
/// `RoundLimit` and the result names the cap.
#[tokio::test]
async fn dialogue_cap_lands_round_limit() {
    let args = r#"{"a":{"system_message":"x"},"b":{"system_message":"y"},
        "opening":{"text":"go"},"max_messages":2,"moderate_every":2}"#;
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", args),
            text("line one"),
            text("line two"),
            text("done"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let record = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .unwrap();
    let run = record.subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::RoundLimit));
    assert_eq!(run.messages.len(), 3, "opening + two generated lines");
    let result = record.result.as_deref().unwrap();
    assert!(result.contains('2'), "{result}");
    assert!(result.contains("chat://"), "{result}");
}

const HAGGLE: &str = r#"{"a":{"name":"Vera","system_message":"seller"},
    "b":{"name":"Tomas","system_message":"buyer"},
    "opening":{"speaker":"a","text":"It works. 150 euros."},
    "max_messages":6,"moderate_every":1}"#;

/// The steering ladder (research §3.4): a note lands in its target's system
/// appendix from the next line on, a retry discards and regenerates the last
/// line under a one-shot note, a rewrite replaces its text outright — and
/// every intervention stays visible in the transcript as a `System` row.
#[tokio::test]
async fn dialogue_note_retry_rewrite_steer_and_stay_visible() {
    let (dir, backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", HAGGLE),
            text("Would you take 90?"),
            verdicts(&[
                ("dialogue_note", r#"{"to":"b","text":"be brief"}"#),
                ("dialogue_continue", "{}"),
            ]),
            text("I could go to 140, with the case thrown in, and film."),
            verdicts(&[("dialogue_retry", r#"{"note":"shorter"}"#)]),
            text("140 with the case."),
            verdicts(&[("dialogue_rewrite", r#"{"text":"120. Final."}"#)]),
            text("Deal at 120."),
            verdicts(&[("dialogue_stop", r#"{"reason":"Deal struck"}"#)]),
            text("Scene done."),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .unwrap()
        .subagent
        .as_deref()
        .unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Completed));
    // Transcript: opening(A), b line, note row, a line (later rewritten),
    // retry row before its regeneration, rewrite row, b line, stop row.
    let shape: Vec<(MessageRole, &str)> = run
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    assert_eq!(shape[0].0, MessageRole::Assistant);
    assert_eq!(shape[1], (MessageRole::User, "Would you take 90?"));
    assert_eq!(shape[2].0, MessageRole::System, "the note row");
    assert!(shape[2].1.contains("be brief"));
    assert_eq!(shape[3].0, MessageRole::System, "the retry row");
    assert!(shape[3].1.contains("shorter"));
    assert_eq!(shape[4], (MessageRole::Assistant, "120. Final."));
    assert_eq!(shape[5].0, MessageRole::System, "the rewrite row");
    assert_eq!(shape[6], (MessageRole::User, "Deal at 120."));
    assert_eq!(shape[7].0, MessageRole::System, "the stop row");

    let reqs = backend.requests();
    // The retried line's request carries the one-shot note in the system
    // (request order: call, b, cp, a, cp, retried a, cp, b, cp, final).
    let retried = &reqs[5];
    assert!(retried.system.as_deref().unwrap().contains("shorter"));
    // Tomas's next line carries the standing note; Vera's requests never do.
    let toms = &reqs[7];
    assert!(toms.system.as_deref().unwrap().starts_with("buyer"));
    assert!(toms.system.as_deref().unwrap().contains("be brief"));
    assert!(!retried.system.as_deref().unwrap().contains("be brief"));
}

/// The all-thinking empty turn (research §5.1): an empty reply is re-asked
/// once with thinking muted, both generations count, and the recovered line
/// lands as if nothing happened.
#[tokio::test]
async fn dialogue_empty_line_recovers_muted() {
    let args = r#"{"a":{"system_message":"x"},"b":{"system_message":"y"},
        "opening":{"speaker":"b","text":"go"},"max_messages":2,"moderate_every":8}"#;
    let spiral = Script {
        chunks: vec![
            ChatChunk::Thoughts("endless deliberation".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
        hang: false,
    };
    let (dir, backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", args),
            spiral,
            text("recovered line"),
            text("closing line"),
            text("done"),
        ],
        no_auto_cfg(),
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let run = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .unwrap()
        .subagent
        .as_deref()
        .unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::RoundLimit));
    assert!(run.messages.iter().any(|m| m.text == "recovered line"));
    let reqs = backend.requests();
    // The first ask ran at the chat's thinking; the re-ask muted it.
    assert_eq!(reqs[1].sampling.reasoning_budget, None);
    assert_eq!(reqs[2].sampling.reasoning_budget, Some(0));
    let view = |r: &crate::shared::api::ChatRequest| -> Vec<(_, String)> {
        r.messages
            .iter()
            .map(|m| (m.role, m.content.clone()))
            .collect()
    };
    assert_eq!(
        view(&reqs[1]),
        view(&reqs[2]),
        "the re-ask repeats the same view"
    );
}

/// The whole-run timeout ends a hung dialogue as `TimedOut`, keeping the
/// partial transcript, while the parent's turn goes on to its reply.
#[tokio::test]
async fn dialogue_timeout_lands_partial_as_timed_out() {
    let args = r#"{"a":{"system_message":"x"},"b":{"system_message":"y"},
        "opening":{"text":"go"}}"#;
    let mut cfg = no_auto_cfg();
    cfg.tools.dialogue_run_timeout_secs = 1;
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", args),
            super::subagent::hang("stuck mid-line"),
            text("done"),
        ],
        cfg,
    )
    .await;
    let chat = load(dir.path(), chat_id);
    let record = chat
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|r| r.name == "run_dialogue")
        .unwrap();
    let run = record.subagent.as_deref().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::TimedOut));
    assert_eq!(run.messages.len(), 1, "the opening alone landed");
    assert!(record.result.as_deref().unwrap().contains("chat://"));
    assert!(chat.messages.iter().any(|m| m.text == "done"));
}

/// Opening a landed dialogue transcript: the sides are the participants'
/// names (spec §9.13), and the system bubble composes both personas and the
/// director's brief.
#[tokio::test]
async fn transcript_opens_with_participant_names_and_composed_persona() {
    let (dir, _backend, _events, chat_id) = run_turn(
        vec![
            call("c1", "run_dialogue", CAFE),
            text("Sorry! Remaking it now."),
            text("Thanks — quick please."),
            verdicts(&[("dialogue_stop", r#"{"reason":"Done"}"#)]),
            text("Staged."),
        ],
        no_auto_cfg(),
    )
    .await;
    let run_id = load(dir.path(), chat_id)
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find_map(|r| r.subagent.as_deref())
        .unwrap()
        .id;
    let (cmd_tx, mut evt_rx, handle, chats, _) = reopen(dir.path()).await;
    let parent = chats.iter().find(|c| c.id == chat_id).unwrap();
    assert_eq!(parent.children.len(), 1);
    assert_eq!(parent.children[0].title, "Mara ↔ Jonas");
    cmd_tx.send(AppCommand::SwitchChat(run_id)).unwrap();
    let ev = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == run_id),
    )
    .await
    .unwrap();
    let AppEvent::ChatActivated { child, .. } = ev else {
        unreachable!()
    };
    let child = child.expect("a transcript announces its parent");
    // The composed bubble: both personas and the director's brief in one text.
    assert!(child.system_message.contains("barista"));
    assert!(child.system_message.contains("customer"));
    assert!(child.system_message.contains("Stop at the goodbye."));
    let names = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::CharacterNames(_)))
        .await
        .unwrap();
    let AppEvent::CharacterNames(names) = names else {
        unreachable!()
    };
    assert_eq!(names.assistant, "Mara");
    assert_eq!(names.user, "Jonas");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
