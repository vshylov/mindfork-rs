//! The demo mode (`mindfork demo`) boots headlessly: a provisioned throwaway
//! root plus `DemoSupervisor` — the exact wiring `main::run_demo` performs,
//! minus the terminal. What can only be judged in a real terminal (colors,
//! streaming feel) stays a manual run; everything else is pinned here.

use super::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::app::supervisor::DemoSupervisor;
use crate::features::demo;
use crate::shared::api::contract::EngineBackend;
use crate::shared::api::mock::MockBackend;

async fn collect_reply(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> String {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    out
}

/// The whole demo loop, end to end: the seeded root bootstraps, the scripted
/// engine answers a user message with the reply that says what it is, and a
/// second message gets a *different* reply — the cycling engine rotating, not
/// `scripted`'s repetition running in place.
#[tokio::test]
async fn demo_boot_streams_cycling_canned_replies() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    demo::provision(&storage).unwrap();
    let config = storage.json().load_config().unwrap();
    assert_eq!(
        config.last_active_chat,
        Some(demo::chat_id()),
        "the demo opens on the showcase chat"
    );

    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, mut evt_rx) = unbounded_channel();
    let backend: Arc<dyn EngineBackend> = Arc::new(MockBackend::cycling(demo::demo_replies(), 0));
    let _handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(DemoSupervisor::new(backend)),
        default_language: crate::shared::i18n::Lang::En,
    }));

    let first = collect_reply(&cmd_tx, &mut evt_rx, "Who am I talking to?").await;
    assert!(
        first.contains("demo engine"),
        "the first canned reply says what it is: {first:?}"
    );

    let second = collect_reply(&cmd_tx, &mut evt_rx, "Show me something.").await;
    assert!(!second.is_empty(), "the engine never runs dry");
    assert_ne!(first, second, "cycling rotates to the next script");
}
