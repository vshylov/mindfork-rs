//! Dangerous-tool confirmation through the real `run` loop (spec §9.7).
//!
//! These drive the whole path — a mocked model calls `fs_write`, the loop parks,
//! the answer comes back as an `AppCommand` — because the piece worth testing is
//! the round trip *into* a running task (fork F8 of docs/tool-confirmation.md),
//! and nothing shorter exercises it.
//!
//! `fs_write` is the tool under test rather than `python_exec`: it is dangerous
//! by the same rule, and whether it ran is a fact on disk rather than a sandbox
//! that has to be provisioned.

use super::*;
use crate::features::tools::confirm::ToolDecision;
use crate::shared::api::contract::ToolCallDelta;

/// A mocked turn: round 1 calls `fs_write`, round 2 answers.
fn write_then_answer(calls: &[(&str, &str)]) -> Arc<dyn EngineBackend> {
    let mut rounds: Vec<Vec<ChatChunk>> = Vec::new();
    for (id, path) in calls {
        rounds.push(vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some((*id).into()),
                name: Some("fs_write".into()),
                arguments: format!("{{\"path\":\"{path}\",\"content\":\"written\"}}"),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ]);
    }
    rounds.push(vec![
        ChatChunk::Text("done".into()),
        ChatChunk::Finished(FinishReason::Stop),
    ]);
    Arc::new(MockBackend::sequence(rounds)) as Arc<dyn EngineBackend>
}

/// Config with the file tools usable and confirmation on (or off).
fn fs_config(root: &std::path::Path, confirm: bool) -> AppConfig {
    let mut config = AppConfig::default();
    config.tools.fs_enabled = true;
    config.tools.fs_root = Some(root.display().to_string());
    config.tools.confirm_dangerous = confirm;
    config
}

/// Boots an orchestrator whose sandbox root is its own data root, with every
/// tool enabled in the profile, and returns the id of the turn it starts.
async fn start_turn(
    dir: &std::path::Path,
    backend: Arc<dyn EngineBackend>,
    confirm: bool,
) -> (
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(dir, Some(backend), fs_config(dir, confirm));
    enable_all_tools(&cmd_tx, &mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::SendMessage("write it".into()))
        .unwrap();
    (cmd_tx, evt_rx, handle)
}

/// The `(generation_id, call_id)` of the next confirmation request.
async fn next_request(evt_rx: &mut UnboundedReceiver<AppEvent>) -> (Uuid, String) {
    let ev = wait_for(evt_rx, |e| matches!(e, AppEvent::ToolConfirmRequest { .. }))
        .await
        .unwrap();
    match ev {
        AppEvent::ToolConfirmRequest {
            generation_id,
            call_id,
            name,
            ..
        } => {
            assert_eq!(name, "fs_write", "asked about the wrong tool");
            (generation_id, call_id)
        }
        _ => unreachable!(),
    }
}

/// The result text of the next executed tool call.
async fn next_result(evt_rx: &mut UnboundedReceiver<AppEvent>) -> String {
    let ev = wait_for(evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
        .await
        .unwrap();
    match ev {
        AppEvent::ToolCall { result, .. } => result,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn an_approved_call_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx, handle) =
        start_turn(dir.path(), write_then_answer(&[("c1", "note.txt")]), true).await;

    let (generation_id, call_id) = next_request(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision: ToolDecision::Allow,
        })
        .unwrap();

    next_result(&mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        dir.path().join("note.txt").exists(),
        "the file was not written"
    );
}

#[tokio::test]
async fn a_declined_call_does_not_run_and_the_turn_continues() {
    let dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx, handle) =
        start_turn(dir.path(), write_then_answer(&[("c1", "note.txt")]), true).await;

    let (generation_id, call_id) = next_request(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision: ToolDecision::Deny,
        })
        .unwrap();

    // The model is told, in its own language (axis A) — and the loop carries on
    // to the final message rather than ending the turn (fork F5).
    let result = next_result(&mut evt_rx).await;
    assert!(
        result.contains("fs_write"),
        "the refusal must name the call: {result:?}"
    );
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        !dir.path().join("note.txt").exists(),
        "a declined call wrote the file anyway"
    );
}

#[tokio::test]
async fn allow_for_turn_stops_asking_about_that_tool() {
    let dir = tempfile::tempdir().unwrap();
    let backend = write_then_answer(&[("c1", "one.txt"), ("c2", "two.txt")]);
    let (cmd_tx, mut evt_rx, handle) = start_turn(dir.path(), backend, true).await;

    let (generation_id, call_id) = next_request(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision: ToolDecision::AllowForTurn,
        })
        .unwrap();

    // The second call must go through without a second question. Collecting
    // until the turn ends is what proves the *absence* of one.
    let mut asked_again = false;
    loop {
        match evt_rx.recv().await.unwrap() {
            AppEvent::ToolConfirmRequest { .. } => asked_again = true,
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        !asked_again,
        "asked twice about a tool allowed for the turn"
    );
    assert!(dir.path().join("one.txt").exists());
    assert!(
        dir.path().join("two.txt").exists(),
        "the second call never ran"
    );
}

#[tokio::test]
async fn replies_for_another_turn_or_another_call_are_ignored() {
    // Both guards at once, and the assertion discriminates: each bogus reply says
    // *deny*, so if either were honoured the file would not be written.
    let dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx, handle) =
        start_turn(dir.path(), write_then_answer(&[("c1", "note.txt")]), true).await;

    let (generation_id, call_id) = next_request(&mut evt_rx).await;
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id: Uuid::new_v4(),
            call_id: call_id.clone(),
            decision: ToolDecision::Deny,
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id: "not-this-call".into(),
            decision: ToolDecision::Deny,
        })
        .unwrap();
    cmd_tx
        .send(AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision: ToolDecision::Allow,
        })
        .unwrap();

    next_result(&mut evt_rx).await;
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(
        dir.path().join("note.txt").exists(),
        "a reply from another turn or another call was honoured"
    );
}

#[tokio::test]
async fn nothing_is_asked_when_the_setting_is_off() {
    // The switch must turn the feature *fully* off: no question, no gate, the
    // loop exactly as it was before the feature existed.
    let dir = tempfile::tempdir().unwrap();
    let (cmd_tx, mut evt_rx, handle) =
        start_turn(dir.path(), write_then_answer(&[("c1", "note.txt")]), false).await;

    let mut asked = false;
    loop {
        match evt_rx.recv().await.unwrap() {
            AppEvent::ToolConfirmRequest { .. } => asked = true,
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(!asked, "asked although confirmation is off");
    assert!(dir.path().join("note.txt").exists(), "the call did not run");
}

#[tokio::test]
async fn a_safe_tool_is_never_asked_about() {
    // `note_save` writes to our own storage: visible in the UI, scoped to the
    // profile, reversible — deliberately not dangerous (fork F1).
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::sequence(vec![
        vec![
            ChatChunk::ToolCall(ToolCallDelta {
                thought_signature: None,
                index: 0,
                id: Some("c1".into()),
                name: Some("note_save".into()),
                arguments: "{\"content\":\"remember this\"}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ],
        vec![
            ChatChunk::Text("done".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ],
    ])) as Arc<dyn EngineBackend>;
    let (cmd_tx, mut evt_rx, handle) = start_turn(dir.path(), backend, true).await;

    let mut asked = false;
    loop {
        match evt_rx.recv().await.unwrap() {
            AppEvent::ToolConfirmRequest { .. } => asked = true,
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    assert!(!asked, "asked about a tool that is not dangerous");
}
