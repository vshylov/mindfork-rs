//! mindfork-rs — консольное (TUI) приложение ИИ-чата.
//! Точка входа: single-instance → логирование → tokio-рантайм → оркестратор → TUI.
//! См. spec §4.2, §4.4 и plan M1.

mod app;
mod entities;
mod features;
mod screens;
mod shared;
mod widgets;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use tokio::sync::mpsc::unbounded_channel;

use crate::app::events::{AppCommand, AppEvent};
use crate::app::orchestrator::{self, OrchestratorDeps};
use crate::app::supervisor::LlamaSupervisor;
use crate::features::backup::{self, RestoreOutcome};
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::storage::{JsonStore, Storage};
use crate::shared::{instance, logging, paths::Paths};

/// Аргументы командной строки. Без подкоманды запускается обычный TUI.
#[derive(Parser)]
#[command(name = "mindfork-rs", about = "Консольный (TUI) ИИ-чат", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Создать резервную копию пользовательских данных (zip-архив).
    Backup {
        /// Путь к создаваемому архиву (по умолчанию `backups/mindfork-backup-<дата>.zip`).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Степень сжатия `0..=9` (0 — без сжатия).
        #[arg(short, long, default_value_t = 9, value_parser = clap::value_parser!(i64).range(0..=9))]
        compression: i64,
    },
    /// Восстановить пользовательские данные из резервной копии.
    Restore {
        /// Путь к архиву резервной копии.
        archive: PathBuf,
    },
    /// Одноразовый импорт данных из LameLLaMA (.NET).
    ImportLamellama {
        /// Каталог с данными LameLLaMA.
        dir: PathBuf,
    },
    /// Управление песочницей Python (Wasmer/WASIX).
    Sandbox {
        #[command(subcommand)]
        action: SandboxAction,
    },
}

#[derive(Subcommand)]
enum SandboxAction {
    /// Установить/обновить песочницу: скачать `wasmer`, `python.webc` и пакеты
    /// (numpy, requests и др.) в `data/sandbox/`.
    Setup {
        /// Перекачать/переустановить всё, даже если уже на месте.
        #[arg(short, long)]
        force: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover().context("resolving data paths")?;
    let _logging = logging::init(&paths).context("initializing logging")?;

    // CLI-подкоманды выполняются без TUI и завершают процесс. См. spec §12.2, §12.3.
    match cli.command {
        Some(Command::ImportLamellama { dir }) => return run_import(&paths, &dir),
        Some(Command::Backup {
            output,
            compression,
        }) => return run_backup(&paths, output, compression),
        Some(Command::Restore { archive }) => return run_restore(&paths, &archive),
        Some(Command::Sandbox { action }) => return run_sandbox(&paths, action),
        None => {}
    }

    // Единственный экземпляр на машину/сеанс: второй запуск завершается с понятным
    // сообщением (не сырым дампом ошибки) ещё до старта рантайма/TUI. См. spec §1.4.
    let _instance = match instance::acquire() {
        Ok(guard) => guard,
        Err(instance::InstanceError::AlreadyRunning) => {
            eprintln!(
                "mindfork-rs уже запущен на этом компьютере. \
                 Закройте предыдущий экземпляр и попробуйте снова."
            );
            tracing::warn!("отказ запуска: другой экземпляр приложения уже работает");
            return Ok(());
        }
        Err(err @ instance::InstanceError::Init(_)) => {
            return Err(anyhow::Error::new(err).context("single-instance check"));
        }
    };
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

/// Захватывает блокировку единственного экземпляра для CLI-операции над данными
/// (бэкап/восстановление). Если приложение запущено — отказ (защита целостности
/// `data.db` от гонки с работающим оркестратором).
fn acquire_cli_guard(action: &str) -> anyhow::Result<instance::InstanceGuard> {
    match instance::acquire() {
        Ok(guard) => Ok(guard),
        Err(instance::InstanceError::AlreadyRunning) => {
            bail!("mindfork-rs запущен — закройте приложение, прежде чем {action}")
        }
        Err(err @ instance::InstanceError::Init(_)) => {
            Err(anyhow::Error::new(err).context("single-instance check"))
        }
    }
}

/// Песочница файловых инструментов из конфига (для включения/очистки при бэкапе).
fn config_fs_root(paths: &Paths) -> Option<PathBuf> {
    JsonStore::new(paths.clone())
        .load_config()
        .unwrap_or_default()
        .tools
        .fs_root
        .map(PathBuf::from)
}

/// CLI: создание резервной копии. Вывод — в stdout (TUI не запущен).
fn run_backup(paths: &Paths, output: Option<PathBuf>, compression: i64) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard("создавать резервную копию")?;
    let fs_root = config_fs_root(paths);
    let out = backup::create_backup(paths, output, compression, fs_root.as_deref())
        .context("создание резервной копии")?;
    println!("Резервная копия создана: {}", out.display());
    Ok(())
}

/// CLI: восстановление из резервной копии (транзакционно, с pre-restore копией и
/// откатом при сбое). Вывод — в stdout/stderr (TUI не запущен).
fn run_restore(paths: &Paths, archive: &Path) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard("восстанавливать данные")?;
    let fs_root = config_fs_root(paths);

    // Err только до разрушительных действий (нет файла / повреждён / небезопасен).
    let outcome = backup::restore_backup(paths, archive, fs_root.as_deref())?;

    match outcome {
        RestoreOutcome::Restored { pre_restore } => {
            if let Some(pre) = pre_restore {
                println!(
                    "Прежние данные сохранены в резервную копию: {}",
                    pre.display()
                );
                println!("Пользовательские данные очищены.");
            }
            println!("Восстановление из {} завершено.", archive.display());
            Ok(())
        }
        RestoreOutcome::RolledBack {
            pre_restore,
            restore_error,
        } => {
            eprintln!(
                "Не удалось восстановить {}: {restore_error:#}",
                archive.display()
            );
            eprintln!(
                "Выполнен откат: прежние данные восстановлены из {}.",
                pre_restore.display()
            );
            bail!("восстановление не выполнено (прежние данные возвращены)")
        }
        RestoreOutcome::Failed {
            pre_restore,
            restore_error,
            rollback_error,
        } => {
            eprintln!(
                "Не удалось восстановить {}: {restore_error:#}",
                archive.display()
            );
            if let Some(rb) = rollback_error {
                eprintln!("Откат к прежним данным тоже не удался: {rb:#}");
            }
            match pre_restore {
                Some(pre) => bail!(
                    "данные в несогласованном состоянии; восстановите вручную из {}",
                    pre.display()
                ),
                None => bail!("восстановление не выполнено"),
            }
        }
    }
}

/// CLI: управление песочницей Python. Вывод — в stdout (TUI не запущен).
fn run_sandbox(paths: &Paths, action: SandboxAction) -> anyhow::Result<()> {
    match action {
        SandboxAction::Setup { force } => run_sandbox_setup(paths, force),
    }
}

/// CLI: установка/обновление песочницы Python (скачивание wasmer + python.webc +
/// пакетов в `data/sandbox/`). Требует собственный tokio-рантайм (сетевой async).
fn run_sandbox_setup(paths: &Paths, force: bool) -> anyhow::Result<()> {
    // Гард единственного экземпляра: не переустанавливаем песочницу, пока приложение
    // работает (могло бы читать заменяемый бинарь/ассеты во время индексации/запуска).
    let _instance = acquire_cli_guard("устанавливать песочницу")?;
    let dir = paths.sandbox_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    runtime.block_on(features::sandbox_setup::setup(
        &dir,
        &features::sandbox_setup::SetupOptions { force },
        |msg| println!("{msg}"),
    ))?;
    Ok(())
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
        config.engine.external.url = Some(url);
    } else if let Ok(bin) = std::env::var("MINDFORK_LLAMA_BIN") {
        config.engine.mode = ServerMode::Managed;
        config.engine.managed.binary = Some(bin);
        if let Ok(m) = std::env::var("MINDFORK_MODEL") {
            config.engine.managed.model_path = Some(m);
        }
        if let Some(ngl) = std::env::var("MINDFORK_NGL")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            config.engine.managed.gpu_layers = ngl;
        }
        if let Some(ctx) = std::env::var("MINDFORK_CTX")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            config.engine.managed.context_size = ctx;
        }
        if let Some(port) = std::env::var("MINDFORK_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            config.engine.managed.port = port;
        }
    }

    if let Ok(url) = std::env::var("MINDFORK_EMBED_URL") {
        config.embed.mode = ServerMode::External;
        config.embed.external.url = Some(url);
    } else if let Ok(bin) = std::env::var("MINDFORK_EMBED_BIN") {
        config.embed.mode = ServerMode::Managed;
        config.embed.managed.binary = Some(bin);
        if let Ok(m) = std::env::var("MINDFORK_EMBED_MODEL") {
            config.embed.managed.model_path = Some(m);
        }
        if let Some(port) = std::env::var("MINDFORK_EMBED_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            config.embed.managed.port = port;
        }
    }
}
