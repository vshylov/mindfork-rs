//! A local Ollama or LM Studio, found and offered
//! (docs/research/local-servers.md §4, stage 2). The look goes through the
//! supervisor, so these tests answer it instead of the machine.

use super::*;

use crate::shared::api::ServerKind;
use crate::shared::api::local_servers::{Found, FoundModel, LocalOffer};
use crate::shared::config::ServerMode;
use crate::shared::i18n::locale;

const LM_STUDIO: &str = "http://127.0.0.1:1234/v1";
const GEMMA: &str = "google_gemma-4-e4b-it";
const NOMIC: &str = "text-embedding-nomic-embed-text-v1.5";

/// LM Studio as measured: one chat model, loaded, and the embedder it ships.
fn lm_studio() -> Vec<Found> {
    vec![Found {
        server: ServerKind::LmStudio,
        url: LM_STUDIO.into(),
        chat: vec![FoundModel {
            name: GEMMA.into(),
            loaded: true,
        }],
        embedders: vec![FoundModel {
            name: NOMIC.into(),
            loaded: false,
        }],
    }]
}

fn offer(embedder: Option<&str>) -> LocalOffer {
    LocalOffer {
        server: ServerKind::LmStudio,
        url: LM_STUDIO.into(),
        model: GEMMA.into(),
        loaded: true,
        embedder: embedder.map(Into::into),
    }
}

/// The orchestrator over a given supervisor.
fn spawn_with(
    sup: MockSupervisor,
) -> (
    tempfile::TempDir,
    Arc<MockSupervisor>,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let sup = Arc::new(sup);
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let handle = tokio::spawn(run(OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: AppConfig::default(),
        supervisor: sup.clone(),
        default_language: crate::shared::i18n::Lang::default(),
        extra_tools: Vec::new(),
    }));
    (dir, sup, cmd_tx, evt_rx, handle)
}

async fn offers_in(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<LocalOffer> {
    match wait_for(rx, |e| matches!(e, AppEvent::LocalServers(_))).await {
        Some(AppEvent::LocalServers(offers)) => offers,
        other => panic!("no list: {other:?}"),
    }
}

async fn note_in(rx: &mut UnboundedReceiver<AppEvent>) -> String {
    match wait_for(rx, |e| matches!(e, AppEvent::Notice(_))).await {
        Some(AppEvent::Notice(text)) => text,
        other => panic!("no note: {other:?}"),
    }
}

fn ui() -> &'static crate::shared::i18n::Locale {
    locale(AppConfig::default().interface.language)
}

/// A chat with no engine and no embedder is offered what answers at the start,
/// the embedder of the same server with it.
#[tokio::test]
async fn a_chat_with_no_engine_is_offered_what_answers() {
    let sup = MockSupervisor::with_backend_no_embedder(None).with_local_servers(lm_studio());
    let (_d, sup, cmd_tx, mut rx, handle) = spawn_with(sup);
    assert_eq!(offers_in(&mut rx).await, [offer(Some(NOMIC))]);
    assert_eq!(sup.local_look_count(), 1, "one look, at the start");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A configured engine is not second-guessed at the start; `/local` looks
/// whatever is configured, and with an embedder already there offers none.
#[tokio::test]
async fn a_configured_engine_is_asked_only_by_the_command() {
    let sup = MockSupervisor::with_backend(Some(Arc::new(MockBackend::scripted(vec![]))))
        .with_local_servers(lm_studio());
    let (_d, sup, cmd_tx, mut rx, handle) = spawn_with(sup);
    wait_for(&mut rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    assert_eq!(sup.local_look_count(), 0, "a Ready chat is not looked past");

    cmd_tx.send(AppCommand::FindLocalServers).unwrap();
    assert_eq!(offers_in(&mut rx).await, [offer(None)]);
    assert_eq!(sup.local_look_count(), 1);
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Nothing found is silence at the start and a note for the command; a server
/// that answered with no chat model is not "nothing".
#[tokio::test]
async fn the_command_is_owed_an_answer() {
    let (_d, sup, cmd_tx, mut rx, handle) = spawn_with(MockSupervisor::with_backend(None));
    wait_for(&mut rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::FindLocalServers).unwrap();
    assert_eq!(note_in(&mut rx).await, ui().t("ui.local.none"));
    assert_eq!(
        sup.local_look_count(),
        2,
        "the start's look, then the command's"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let embedder_only = vec![Found {
        chat: vec![],
        ..lm_studio().remove(0)
    }];
    let sup = MockSupervisor::with_backend(None).with_local_servers(embedder_only);
    let (_d, _sup, cmd_tx, mut rx, handle) = spawn_with(sup);
    cmd_tx.send(AppCommand::FindLocalServers).unwrap();
    assert_eq!(note_in(&mut rx).await, ui().t("ui.local.no_models"));
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// A pick writes the chat's external section and, with no embedder
/// configured, the embedder's — saved, re-emitted, and said.
#[tokio::test]
async fn a_pick_writes_the_external_sections() {
    let (d, _sup, cmd_tx, mut rx, handle) =
        spawn_with(MockSupervisor::with_backend_no_embedder(None));
    wait_for(&mut rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::UseLocalServer(offer(Some(NOMIC))))
        .unwrap();
    let settings = wait_for(&mut rx, |e| {
        matches!(e, AppEvent::Settings { config, .. } if config.engine.mode == ServerMode::External)
    })
    .await
    .expect("the settings re-emitted");
    let AppEvent::Settings { config, .. } = settings else {
        unreachable!()
    };
    assert_eq!(config.engine.external.url.as_deref(), Some(LM_STUDIO));
    assert_eq!(config.engine.external.model_name.as_deref(), Some(GEMMA));
    assert_eq!(config.embed.mode, ServerMode::External);
    assert_eq!(config.embed.external.url.as_deref(), Some(LM_STUDIO));
    assert_eq!(config.embed.external.model_name.as_deref(), Some(NOMIC));
    let said = note_in(&mut rx).await;
    assert!(
        said.contains("LM Studio") && said.contains(GEMMA) && said.contains(NOMIC),
        "{said}"
    );
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    let saved = Storage::open(Paths::with_root(d.path()))
        .unwrap()
        .json()
        .load_config()
        .unwrap();
    assert_eq!(saved.engine.external.model_name.as_deref(), Some(GEMMA));
    assert_eq!(saved.embed.external.model_name.as_deref(), Some(NOMIC));
}

/// An embedder already configured is never replaced, even by a row that
/// carries one — the list may be older than the embedder (F5).
#[tokio::test]
async fn a_configured_embedder_is_never_replaced() {
    let (_d, _sup, cmd_tx, mut rx, handle) = spawn_with(MockSupervisor::with_backend(None));
    wait_for(&mut rx, |e| matches!(e, AppEvent::Settings { .. }))
        .await
        .unwrap();
    cmd_tx
        .send(AppCommand::UseLocalServer(offer(Some(NOMIC))))
        .unwrap();
    let settings = wait_for(&mut rx, |e| {
        matches!(e, AppEvent::Settings { config, .. } if config.engine.mode == ServerMode::External)
    })
    .await
    .unwrap();
    let AppEvent::Settings { config, .. } = settings else {
        unreachable!()
    };
    assert_eq!(config.embed.mode, ServerMode::Managed, "left as it was");
    assert_eq!(config.embed.external.model_name, None);
    let said = note_in(&mut rx).await;
    assert!(!said.contains(NOMIC), "{said}");
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}
