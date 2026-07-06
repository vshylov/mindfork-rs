//! Тесты оркестратора (без UI и реальной модели): автомат генерации, гонки,
//! agentic-loop, приоритеты семплинга, операции списка/профилей/RAG.

use super::engines::EngineManager;
use super::impersonation::{build_impersonation_request, swap_role_message};
use super::request::build_request;
use super::restart_queue::RestartQueue;
use super::save_queue::SaveQueue;
use super::title::salvage_title_source;
use super::*;

use crate::app::events::RagProgress;
use crate::entities::message::{Message, MessageRole};
use crate::features::profiles::ProfileEdit;
use crate::shared::api::{ChatChunk, Embedder, EngineBackend};

use crate::app::supervisor::MockSupervisor;
use crate::shared::api::mock::{MockBackend, MockEmbedder};
use crate::shared::paths::Paths;

fn test_embedder() -> Arc<dyn Embedder> {
    Arc::new(MockEmbedder::new(16))
}

/// Поднимает оркестратор на временном хранилище. Возвращает каналы и handle.
fn spawn_orch(
    backend: Option<Arc<dyn EngineBackend>>,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    spawn_orch_cfg(backend, AppConfig::default())
}

/// Как [`spawn_orch`], но с заданной конфигурацией (max_tool_rounds и т.п.).
fn spawn_orch_cfg(
    backend: Option<Arc<dyn EngineBackend>>,
    config: AppConfig,
) -> (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor: Arc::new(MockSupervisor::with_backend(backend)),
    };
    let handle = tokio::spawn(run(deps));
    (dir, cmd_tx, evt_rx, handle)
}

/// Дренирует события до первого, удовлетворяющего предикату (или закрытия).
async fn wait_for<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> Option<AppEvent> {
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            return Some(ev);
        }
    }
    None
}

/// Собирает «голый» оркестратор для юнит-тестов чистых методов (без петли),
/// отбрасывая поток событий.
fn bare_orch() -> (tempfile::TempDir, Orchestrator) {
    let (dir, orch, _rx) = bare_orch_rx();
    (dir, orch)
}

/// Как [`bare_orch`], но возвращает и приёмник событий (для проверки эмиссии).
fn bare_orch_rx() -> (tempfile::TempDir, Orchestrator, UnboundedReceiver<AppEvent>) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (evt_tx, evt_rx) = unbounded_channel();
    let (done_tx, _done_rx) = unbounded_channel();
    let (status_tx, _status_rx) = unbounded_channel();
    let (title_tx, _title_rx) = unbounded_channel();
    let (imp_status_tx, _imp_status_rx) = unbounded_channel();
    let (imp_done_tx, _imp_done_rx) = unbounded_channel();
    let config = AppConfig {
        default_sampling: SamplingConfig {
            temperature: Some(0.1),
            ..Default::default()
        },
        ..Default::default()
    };
    let registry = Arc::new(build_registry(&config));
    // Менеджер серверов: сразу «готов» с тестовым эмбеддером (как было у голого
    // оркестратора). chat-движок тесты при необходимости проставляют сами.
    let mut engines = EngineManager::new(
        Arc::new(MockSupervisor::with_backend(None)),
        status_tx,
        imp_status_tx,
    );
    engines.server_status = ServerStatus::Ready;
    engines.embedder = test_embedder();
    let orch = Orchestrator {
        evt_tx,
        engines,
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        storage,
        config,
        registry,
        title_tx,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        reflect_cancel: None,
        reflect_done_tx: unbounded_channel().0,
        reflect_failures: 0,
        consolidate_cancel: None,
        consolidate_counts: std::collections::HashMap::new(),
        consolidate_done_tx: unbounded_channel().0,
        consolidate_failures: 0,
        saves: SaveQueue::default(),
        restarts: RestartQueue::default(),
    };
    (dir, orch, evt_rx)
}

/// Включает в профиле весь каталог инструментов (в т.ч. управляющие followup/
/// rewrite) — для тестов управляющих инструментов. Возвращает id профиля.
async fn enable_all_tools(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
) -> Uuid {
    use crate::features::tools::all_tool_ids;
    let pl = wait_for(evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let pid = match pl {
        AppEvent::ProfileList(v) => v[0].id,
        _ => unreachable!(),
    };
    cmd_tx
        .send(AppCommand::UpdateProfile {
            id: pid,
            edit: Box::new(ProfileEdit {
                enabled_tools: Some(all_tool_ids()),
                ..Default::default()
            }),
        })
        .unwrap();
    pid
}

/// Реальный chat-движок из `MINDFORK_ENGINE_URL` (для end-to-end смоуков на живой
/// модели). `None` — переменная не задана (тест пропускается).
fn live_backend() -> Option<Arc<dyn EngineBackend>> {
    let url = std::env::var("MINDFORK_ENGINE_URL").ok()?;
    Some(Arc::new(crate::shared::api::OpenAiClient::new(url)) as Arc<dyn EngineBackend>)
}

/// Реальный эмбеддер из `MINDFORK_EMBED_URL` (для живых смоуков — bge-m3 и т.п.);
/// `None`, если не задан → живой смоук берёт тестовый `MockEmbedder`.
fn live_embedder() -> Option<Arc<dyn Embedder>> {
    let url = std::env::var("MINDFORK_EMBED_URL").ok()?;
    Some(Arc::new(crate::shared::api::OpenAiClient::new(url)) as Arc<dyn Embedder>)
}

/// Кортеж поднятого оркестратора (как у [`spawn_orch`]): каталог данных, канал
/// команд, приёмник событий, handle петли.
type OrchHandle = (
    tempfile::TempDir,
    UnboundedSender<AppCommand>,
    UnboundedReceiver<AppEvent>,
    tokio::task::JoinHandle<()>,
);

/// Поднимает оркестратор для живого смоука: chat из `MINDFORK_ENGINE_URL`, эмбеддер
/// из `MINDFORK_EMBED_URL` (реальный сервер; иначе детерминированный `MockEmbedder`).
/// `None`, если `MINDFORK_ENGINE_URL` не задан (смоук пропускается).
fn spawn_orch_live() -> Option<OrchHandle> {
    let backend = live_backend()?;
    let embedder = live_embedder();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (evt_tx, evt_rx) = unbounded_channel();
    let deps = OrchestratorDeps {
        cmd_rx,
        evt_tx,
        storage,
        config: AppConfig::default(),
        supervisor: Arc::new(MockSupervisor::with_backend_and_embedder(
            Some(backend),
            embedder,
        )),
    };
    let handle = tokio::spawn(run(deps));
    Some((dir, cmd_tx, evt_rx, handle))
}

/// Дренирует события до `Finished` (или закрытия), помечая, встретилось ли
/// `pred`-событие по пути. Для end-to-end смоуков управляющих инструментов.
async fn drain_until_finished<F: Fn(&AppEvent) -> bool>(
    rx: &mut UnboundedReceiver<AppEvent>,
    pred: F,
) -> bool {
    let mut saw = false;
    while let Some(ev) = rx.recv().await {
        if pred(&ev) {
            saw = true;
        }
        if matches!(ev, AppEvent::Finished { .. }) {
            break;
        }
    }
    saw
}

/// Прогоняет один ход: шлёт сообщение, дренирует события до `Finished`, собирая
/// текст ответа и имена вызванных инструментов. Для live-смоуков.
#[cfg(test)]
async fn run_turn_live(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<String>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut tools = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall { name, .. } => tools.push(name.clone()),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, tools)
}

/// Как [`run_turn_live`], но собирает пары (имя инструмента, результат) — чтобы
/// проверить текст результата (напр. срабатывание ворот `add_insight`).
async fn run_turn_capture(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    text: &str,
) -> (String, Vec<(String, String)>) {
    cmd_tx.send(AppCommand::SendMessage(text.into())).unwrap();
    let mut out = String::new();
    let mut calls = Vec::new();
    while let Some(ev) = evt_rx.recv().await {
        match &ev {
            AppEvent::Chunk { text, .. } => out.push_str(text),
            AppEvent::ToolCall { name, result, .. } => calls.push((name.clone(), result.clone())),
            AppEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (out, calls)
}

/// Готовит голый оркестратор с профилем + активным чатом (для F3-правок).
fn orch_with_active_profile() -> (tempfile::TempDir, Orchestrator, Uuid) {
    let (dir, mut orch) = bare_orch();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "t");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, pid)
}

/// Готовит оркестратор с чатом (user+assistant) и профилем, включившим модель себя;
/// `auto_reflect_every=1`. Возвращает `(dir, orch, chat_id)`.
fn orch_ready_for_reflection() -> (tempfile::TempDir, Orchestrator, Uuid) {
    use crate::features::tools::self_model::GET_SELF_MODEL_ID;
    let (dir, mut orch) = bare_orch();
    orch.config.self_model.auto_reflect_every = 1;
    let mut profile = Profile::new("P", "sys");
    profile.enabled_tools = vec![GET_SELF_MODEL_ID.into()];
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    chat.push_message(Message::assistant("здравствуй"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    (dir, orch, chat_id)
}

// ---------- подмодули тестов (разбор god-object: docs/refactoring-god-objects.md, этап 3) ----------

mod chats;
mod generation;
mod impersonation;
mod live;
mod profiles;
mod rag;
mod reflection;
mod request;
mod self_model;
mod settings;
mod title;
