//! Orchestrator tests — the code project's command slots
//! (`/project build-cmd|run-cmd|test-cmd`, `/project clear <slot>`). Part of the
//! [`super`] module (fixtures in mod.rs). See spec §9.12,
//! docs/code-workspace.md §3.3.

use super::*;
use crate::entities::workspace::CommandSlot;
use crate::features::project_command::{ProjectProgress, SlotAction};

/// A running orchestrator with a project attached, and the directory it points
/// at. Every test here starts this way, so it is a fixture rather than a test
/// opening (docs/lessons.md §2).
async fn with_project() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let project = tempfile::tempdir().unwrap();
    let (data, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::ProjectAttach {
            path: project.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    wait_for(&mut evt_rx, |e| {
        matches!(
            e,
            AppEvent::ProjectProgress(ProjectProgress::Attached { .. })
        )
    })
    .await
    .unwrap();
    (project, data, cmd_tx, evt_rx, handle)
}

/// Sends one slot command and returns the outcome it reported.
async fn slot(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    slot: CommandSlot,
    action: SlotAction,
) -> ProjectProgress {
    cmd_tx
        .send(AppCommand::ProjectSlot { slot, action })
        .unwrap();
    match wait_for(evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_))).await {
        Some(AppEvent::ProjectProgress(p)) => p,
        other => panic!("expected a project outcome, got {other:?}"),
    }
}

/// The whole round trip through the route the user actually takes: set a line,
/// read it back, clear it, and be told there was nothing left to clear.
///
/// Looped over the vocabulary rather than written three times — a per-slot trio
/// is the sliding self-duplicate the duplication gate reads as one block written
/// three times, and looping is what makes a fourth slot impossible to forget.
#[tokio::test]
async fn a_slot_is_set_shown_and_cleared_through_the_orchestrator() {
    let (_project, _data, cmd_tx, mut evt_rx, handle) = with_project().await;
    for s in CommandSlot::ALL {
        let line = format!("cargo {} --offline", s.key());
        assert_eq!(
            slot(&cmd_tx, &mut evt_rx, s, SlotAction::Set(line.clone())).await,
            ProjectProgress::CommandSet {
                slot: s,
                line: line.clone()
            }
        );
        assert_eq!(
            slot(&cmd_tx, &mut evt_rx, s, SlotAction::Show).await,
            ProjectProgress::CommandShown {
                slot: s,
                line: Some(line)
            }
        );
        assert_eq!(
            slot(&cmd_tx, &mut evt_rx, s, SlotAction::Clear).await,
            ProjectProgress::CommandCleared { slot: s, had: true }
        );
        // Clearing an empty slot is not an error, and must not be silent
        // either: saying so is what tells the user their earlier command never
        // landed (docs/lessons.md §4).
        assert_eq!(
            slot(&cmd_tx, &mut evt_rx, s, SlotAction::Clear).await,
            ProjectProgress::CommandCleared {
                slot: s,
                had: false
            }
        );
    }
    drop(cmd_tx);
    handle.await.unwrap();
}

/// A pipeline is refused **when the line is set**, not three turns later when a
/// model first tries to run it — and the refusal names the character it found,
/// so the answer is not a dead end (docs/lessons.md §4). Nothing is stored.
#[tokio::test]
async fn a_pipeline_is_refused_at_the_moment_it_is_typed() {
    let (_project, _data, cmd_tx, mut evt_rx, handle) = with_project().await;
    let line = "cargo build 2>&1 | tee log.txt".to_string();
    let refused = slot(
        &cmd_tx,
        &mut evt_rx,
        CommandSlot::Build,
        SlotAction::Set(line.clone()),
    )
    .await;
    match refused {
        ProjectProgress::CommandRefused { line: got, ch } => {
            assert_eq!(got, line);
            assert!("|&><;".contains(ch), "an unexpected character: {ch}");
        }
        other => panic!("a pipeline must be refused, got {other:?}"),
    }
    // And it really did not land: the slot is still empty, so `code_build` is
    // still not offered at all.
    assert_eq!(
        slot(&cmd_tx, &mut evt_rx, CommandSlot::Build, SlotAction::Show).await,
        ProjectProgress::CommandShown {
            slot: CommandSlot::Build,
            line: None
        }
    );
    drop(cmd_tx);
    handle.await.unwrap();
}

/// `/project status` lists every slot, the empty ones included: the question it
/// answers is what the assistant can run here, and an answer that hides the
/// empty slots cannot say "none of them".
#[tokio::test]
async fn status_lists_every_slot_including_the_empty_ones() {
    let (_project, _data, cmd_tx, mut evt_rx, handle) = with_project().await;
    slot(
        &cmd_tx,
        &mut evt_rx,
        CommandSlot::Test,
        SlotAction::Set("cargo test".into()),
    )
    .await;
    cmd_tx.send(AppCommand::ProjectStatus).unwrap();
    let status = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProjectProgress(_))).await;
    let Some(AppEvent::ProjectProgress(ProjectProgress::Status { root, commands })) = status else {
        panic!("expected a status, got {status:?}");
    };
    assert!(root.is_some());
    assert_eq!(commands.len(), CommandSlot::ALL.len());
    assert_eq!(
        commands
            .iter()
            .find(|(s, _)| *s == CommandSlot::Test)
            .and_then(|(_, line)| line.clone()),
        Some("cargo test".to_string())
    );
    assert!(
        commands
            .iter()
            .filter(|(s, _)| *s != CommandSlot::Test)
            .all(|(_, line)| line.is_none()),
        "only the slot that was set may carry a line: {commands:?}"
    );
    drop(cmd_tx);
    handle.await.unwrap();
}

/// A command line belongs to a directory, so a chat with nothing attached is
/// told to attach something rather than being allowed to configure a project
/// that does not exist.
#[tokio::test]
async fn a_slot_command_without_a_project_says_so() {
    let (_data, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let out = slot(
        &cmd_tx,
        &mut evt_rx,
        CommandSlot::Build,
        SlotAction::Set("cargo build".into()),
    )
    .await;
    assert_eq!(
        out,
        ProjectProgress::Status {
            root: None,
            commands: Vec::new()
        }
    );
    drop(cmd_tx);
    handle.await.unwrap();
}
