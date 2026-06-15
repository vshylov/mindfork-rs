//! mindfork-rs — консольное (TUI) приложение ИИ-чата.
//! Точка входа: single-instance → логирование → tokio-рантайм → оркестратор → TUI.
//! См. spec §4.2, §4.4 и plan M1.

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

use crate::app::events::{AppCommand, AppEvent};
use crate::app::orchestrator::{self, OrchestratorDeps};
use crate::app::supervisor::LlamaSupervisor;
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::storage::Storage;
use crate::shared::{instance, logging, paths::Paths};

fn main() -> anyhow::Result<()> {
    let paths = Paths::discover().context("resolving data paths")?;
    let _logging = logging::init(&paths).context("initializing logging")?;

    // Одноразовый импорт из LameLLaMA (.NET): `mindfork --import-lamellama <dir>`.
    // Выполняется без TUI/инстанс-гарда и завершает процесс. См. spec §12.2.
    if let Some(dir) = parse_import_arg() {
        return run_import(&paths, &dir);
    }

    let _instance = instance::acquire().context("single-instance check")?;
    tracing::info!(root = %paths.root().display(), "mindfork-rs starting");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    // Хранилище (JSON + SQLite) рядом с бинарником. Единственный писатель —
    // оркестратор (spec §4.4.2). Arc — нужен инструментам в ToolContext.
    let storage = Arc::new(Storage::open(paths.clone()).context("opening storage")?);

    let (cmd_tx, cmd_rx) = unbounded_channel::<AppCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<AppEvent>();

    // Конфиг из settings.json + посев переменными окружения (dev-workflow contract §9).
    // Серверы инференса/эмбеддингов оркестратор поднимает сам через супервайзер.
    let mut config = storage.json().load_config().unwrap_or_default();
    apply_env_overrides(&mut config);

    runtime.spawn(orchestrator::run(OrchestratorDeps {
        cmd_rx,
        evt_tx: evt_tx.clone(),
        storage,
        config,
        supervisor: Arc::new(LlamaSupervisor),
    }));

    // Словари спелл-чека грузит сам `runtime` в фоне по настройкам интерфейса
    // (вкл/выкл + выбор словарей) и перегружает при их изменении.
    let dict_dir = paths.dictionaries_dir();
    let personal = paths.personal_dictionary();

    let result = app::runtime::run(cmd_tx.clone(), evt_rx, dict_dir, personal);

    // Останавливаем оркестратор; managed-серверы он гасит сам (kill_on_drop при
    // завершении его задачи). Даём фоновым задачам завершиться.
    let _ = cmd_tx.send(AppCommand::Quit);
    runtime.shutdown_timeout(Duration::from_secs(2));

    match &result {
        Ok(()) => tracing::info!("mindfork-rs exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork-rs exited with error"),
    }
    result
}

/// Возвращает каталог из аргумента `--import-lamellama <dir>` (если задан).
fn parse_import_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--import-lamellama" {
            return args.next().map(std::path::PathBuf::from);
        }
    }
    None
}

/// Одноразовый импорт данных LameLLaMA (.NET) в хранилище mindfork (spec §12.2).
/// Идемпотентно (детерминированные id), исходные файлы только читаются. Вывод —
/// в stdout (TUI не запущен), не в лог.
fn run_import(paths: &Paths, dir: &std::path::Path) -> anyhow::Result<()> {
    let storage = Storage::open(paths.clone()).context("opening storage")?;
    let result = features::migration::import_dir(dir)
        .with_context(|| format!("importing LameLLaMA data from {}", dir.display()))?;

    for profile in &result.profiles {
        storage.json().upsert_profile(profile)?;
    }
    for chat in &result.chats {
        storage.json().save_chat(chat)?;
    }

    // Переносим глобальный семплинг и настройки интерфейса источника.
    let mut config = storage.json().load_config().unwrap_or_default();
    if let Some(sampling) = result.sampling {
        config.default_sampling = sampling;
    }
    if let Some(interface) = result.interface {
        config.interface.spellcheck_enabled = interface.spellcheck_enabled;
        config.interface.selected_dictionaries = interface.dictionaries;
        config.interface.theme = interface.theme;
    }
    storage.json().save_config(&config)?;

    println!(
        "Импорт LameLLaMA завершён: профилей {}, чатов {}.",
        result.profiles.len(),
        result.chats.len()
    );
    Ok(())
}

/// Посев конфигурации переменными окружения (dev/смоук-workflow). Env имеет
/// приоритет над `settings.json`, чтобы быстрый запуск против сервера не требовал
/// правки файла. Затрагивает только chat/embedding-серверы:
/// - `MINDFORK_ENGINE_URL` — external chat-сервер (любой OpenAI-совместимый);
/// - `MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
///   `MINDFORK_PORT`) — managed `llama-server`;
/// - `MINDFORK_EMBED_URL` / `MINDFORK_EMBED_BIN` (+ `MINDFORK_EMBED_MODEL`,
///   `MINDFORK_EMBED_PORT`) — embedding-сервер.
fn apply_env_overrides(config: &mut AppConfig) {
    if let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") {
        config.engine.mode = ServerMode::External;
        config.engine.url = Some(url);
    } else if let Ok(bin) = std::env::var("MINDFORK_LLAMA_BIN") {
        config.engine.mode = ServerMode::Managed;
        config.engine.binary = Some(bin);
        if let Ok(m) = std::env::var("MINDFORK_MODEL") {
            config.engine.model_path = Some(m);
        }
        if let Some(ngl) = std::env::var("MINDFORK_NGL")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            config.engine.gpu_layers = ngl;
        }
        if let Some(ctx) = std::env::var("MINDFORK_CTX")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            config.engine.context_size = ctx;
        }
        if let Some(port) = std::env::var("MINDFORK_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            config.engine.port = port;
        }
    }

    if let Ok(url) = std::env::var("MINDFORK_EMBED_URL") {
        config.embed.mode = ServerMode::External;
        config.embed.url = Some(url);
    } else if let Ok(bin) = std::env::var("MINDFORK_EMBED_BIN") {
        config.embed.mode = ServerMode::Managed;
        config.embed.binary = Some(bin);
        if let Ok(m) = std::env::var("MINDFORK_EMBED_MODEL") {
            config.embed.model_path = Some(m);
        }
        if let Some(port) = std::env::var("MINDFORK_EMBED_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            config.embed.port = port;
        }
    }
}
