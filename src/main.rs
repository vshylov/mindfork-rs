//! mindfork-rs — консольное (TUI) приложение ИИ-чата.
//! Точка входа: single-instance → логирование → tokio-рантайм → оркестратор → TUI.
//! См. spec §4.2, §4.4 и plan M1.

// На этапе каркаса часть публичного API слоёв опережает своих потребителей
// (paths, error, …) — это нормально для FSD-скелета. TODO(M3): убрать, когда
// все слои будут связаны.
#![allow(dead_code)]

mod app;
mod entities;
mod features;
mod screens;
mod shared;
mod widgets;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::sync::mpsc::unbounded_channel;

use crate::app::events::{AppCommand, AppEvent, ServerStatus};
use crate::app::orchestrator::{self, OrchestratorDeps};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{
    EngineBackend, ManagedConfig, ServerHandle, XinferClient, wait_until_ready,
};
use crate::shared::storage::Storage;
use crate::shared::{instance, logging, paths::Paths};

fn main() -> anyhow::Result<()> {
    let paths = Paths::discover().context("resolving data paths")?;
    let _instance = instance::acquire().context("single-instance check")?;
    let _logging = logging::init(&paths).context("initializing logging")?;
    tracing::info!(root = %paths.root().display(), "mindfork-rs starting");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    // Хранилище (JSON + SQLite) рядом с бинарником. Единственный писатель —
    // оркестратор (spec §4.4.2).
    let storage = Storage::open(paths.clone()).context("opening storage")?;

    let (cmd_tx, cmd_rx) = unbounded_channel::<AppCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<AppEvent>();

    // Подключение к серверу инференса (по переменным окружения; настройки — M8).
    let (backend, status, server) = resolve_backend(runtime.handle(), &evt_tx);
    if let Some(server) = &server {
        tracing::info!(
            base_url = server.base_url(),
            "managed xinfer server launched"
        );
    }

    // Глобальный семплинг по умолчанию (читается из конфига; чат может переопределить).
    let default_sampling = storage
        .json()
        .load_config()
        .map(|c| c.default_sampling)
        .unwrap_or_else(|_| SamplingConfig {
            max_tokens: Some(2048),
            thinking: Some(true),
            ..Default::default()
        });

    runtime.spawn(orchestrator::run(OrchestratorDeps {
        cmd_rx,
        evt_tx: evt_tx.clone(),
        backend,
        storage,
        default_sampling,
        status,
    }));

    // Фоновая загрузка словарей спелл-чека (парсинг .dic тяжёлый — не блокируем UI).
    let (spell_tx, spell_rx) = std::sync::mpsc::channel();
    let dict_dir = paths.dictionaries_dir();
    let personal = paths.personal_dictionary();
    std::thread::spawn(move || {
        let checker = features::spellcheck::dict::load(&dict_dir, &personal);
        let _ = spell_tx.send(checker);
    });

    let result = app::runtime::run(cmd_tx.clone(), evt_rx, spell_rx);

    // Останавливаем оркестратор и даём фоновым задачам завершиться.
    let _ = cmd_tx.send(AppCommand::Quit);
    runtime.shutdown_timeout(Duration::from_secs(2));
    drop(server); // kill managed xinfer (kill_on_drop)

    match &result {
        Ok(()) => tracing::info!("mindfork-rs exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork-rs exited with error"),
    }
    result
}

/// Определяет бэкенд инференса по переменным окружения (временно, до настроек):
/// - `MINDFORK_XINFER_URL` — подключение к запущенному серверу (external);
/// - `MINDFORK_XINFER_BIN` (+ `MINDFORK_MODEL`, `MINDFORK_XINFER_PORT`, `MINDFORK_ISQ`)
///   — managed-запуск дочернего процесса;
/// - иначе сервер не настроен.
///
/// Готовность проверяется в фоне; статус доставляется событием `ServerStatus`.
fn resolve_backend(
    rt: &tokio::runtime::Handle,
    evt_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
) -> (
    Option<Arc<dyn EngineBackend>>,
    ServerStatus,
    Option<ServerHandle>,
) {
    if let Ok(url) = std::env::var("MINDFORK_XINFER_URL") {
        let client = Arc::new(XinferClient::new(url));
        spawn_readiness_probe(rt, evt_tx, client.clone(), Duration::from_secs(15));
        return (Some(client), ServerStatus::Connecting, None);
    }

    if let Ok(bin) = std::env::var("MINDFORK_XINFER_BIN") {
        let cfg = ManagedConfig {
            binary: bin.into(),
            model_id: std::env::var("MINDFORK_MODEL").ok(),
            weight_path: None,
            weight_file: None,
            isq: std::env::var("MINDFORK_ISQ").ok(),
            device_ids: vec![0],
            cpu: false,
            port: std::env::var("MINDFORK_XINFER_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8000),
            extra_args: vec![],
        };
        match ServerHandle::launch(&cfg) {
            Ok(handle) => {
                let client = Arc::new(XinferClient::new(handle.base_url()));
                // Загрузка модели может занять минуты — даём щедрый таймаут.
                spawn_readiness_probe(rt, evt_tx, client.clone(), Duration::from_secs(600));
                (Some(client), ServerStatus::Connecting, Some(handle))
            }
            Err(err) => (None, ServerStatus::Disconnected(err.to_string()), None),
        }
    } else {
        (None, ServerStatus::NotConfigured, None)
    }
}

fn spawn_readiness_probe(
    rt: &tokio::runtime::Handle,
    evt_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    client: Arc<XinferClient>,
    timeout: Duration,
) {
    let evt_tx = evt_tx.clone();
    rt.spawn(async move {
        let status = match wait_until_ready(&client, timeout).await {
            Ok(()) => ServerStatus::Ready,
            Err(err) => ServerStatus::Disconnected(err.to_string()),
        };
        let _ = evt_tx.send(AppEvent::ServerStatus(status));
    });
}
