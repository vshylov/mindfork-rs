//! mindfork-rs — консольное (TUI) приложение ИИ-чата.
//! Точка входа: «peek»-фаза (язык/корень до разбора аргументов) → разбор CLI →
//! single-instance → логирование → tokio-рантайм → оркестратор → TUI.
//! См. spec §4.2, §4.4, plan M1 и docs/history/i18n-cli.md (весь текст CLI — в бандлах локалей).

mod app;
mod entities;
mod features;
mod screens;
mod shared;
mod widgets;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use tokio::sync::mpsc::unbounded_channel;

use crate::app::events::{AppCommand, AppEvent};
use crate::app::orchestrator::{self, OrchestratorDeps};
use crate::app::supervisor::LlamaSupervisor;
use crate::features::backup::{self, RestoreOutcome};
use crate::features::cli::{self, CliCommand};
use crate::shared::config::{AppConfig, ServerMode};
use crate::shared::i18n::{self, Lang, Locale};
use crate::shared::storage::{JsonStore, Storage};
use crate::shared::{instance, logging, paths::Paths};

fn main() -> ExitCode {
    // «Peek»-фаза: определить корень данных и язык CLI **до** разбора аргументов и без
    // создания каталогов (`--help`/`--version` не должны трогать диск — docs/history/i18n-cli.md
    // §3.2). Язык нужен раньше всего, чтобы даже справка и ошибки разбора были на нём.
    let (paths, lang) = match Paths::resolve() {
        Ok(paths) => {
            let settings_lang = try_settings_language(&paths.settings_file());
            let lang = cli_lang(settings_lang, paths.default_language());
            (paths, lang)
        }
        // Сбой resolve (реалистично — только битый `defaults.json`): язык неизвестен →
        // полная неопределённость → печать на английском (решение пользователя).
        Err(err) => {
            print_error(Lang::En, &err);
            return ExitCode::FAILURE;
        }
    };

    // Внешние локали (`data/locales/*.json`) сканируются до разбора, чтобы `--help`
    // уважал их override/новые языки. `init` идёт до `logging::init`, поэтому
    // предупреждения о битых файлах возвращаются и логируются позже (в `real_main`);
    // на путях раннего выхода (`--help`) молча отбрасываются.
    let locale_warnings = i18n::init(&paths.locales_dir());
    let loc = i18n::locale(lang);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args, loc) {
        Ok(cmd) => cmd,
        // `Err` — уже готовое к печати локализованное сообщение (§ features/cli).
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    // Быстрый путь без побочных эффектов: справка/версия не создают каталогов и логов.
    match command {
        CliCommand::Help { topic } => {
            println!("{}", cli::render_help(topic, loc));
            return ExitCode::SUCCESS;
        }
        CliCommand::Version => {
            println!(
                "{}",
                loc.tf(
                    "cli.version.line",
                    &[("version", env!("CARGO_PKG_VERSION"))]
                )
            );
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    match real_main(command, &paths, loc, &locale_warnings) {
        Ok(code) => code,
        Err(err) => {
            print_error(lang, &err);
            ExitCode::FAILURE
        }
    }
}

/// Стороне-эффектные команды: создаёт каталоги, поднимает логи, диспетчеризует.
/// Ошибки печатаются вызывающим (`main`) через [`print_error`] — единый локализованный
/// префикс + однострочная цепочка причин (`{:#}`), без английского `Error:`/`Caused by:`.
fn real_main(
    command: CliCommand,
    paths: &Paths,
    loc: &Locale,
    locale_warnings: &[String],
) -> anyhow::Result<ExitCode> {
    paths.ensure_dirs().with_context(|| {
        loc.tf(
            "cli.ctx.ensure_dirs",
            &[("path", &paths.root().display().to_string())],
        )
    })?;
    let _logging =
        logging::init(paths, loc).with_context(|| loc.t("cli.ctx.init_logging").to_string())?;
    // Предупреждения о внешних локалях (собраны до установки лог-подписчика).
    for w in locale_warnings {
        tracing::warn!("{w}");
    }

    match command {
        CliCommand::ImportLamellama { dir } => {
            run_import(paths, &dir, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::Backup {
            output,
            compression,
        } => {
            run_backup(paths, output, compression, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::Restore { archive } => {
            run_restore(paths, &archive, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::SandboxSetup { force } => {
            run_sandbox_setup(paths, force, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::LocalesExport { code, output } => {
            run_locales_export(&code, &output, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::Run => run_tui(paths, loc),
        CliCommand::Help { .. } | CliCommand::Version => unreachable!("обработаны в main"),
    }
}

/// Запуск основного TUI (команда без подкоманды).
fn run_tui(paths: &Paths, loc: &Locale) -> anyhow::Result<ExitCode> {
    // Единственный экземпляр на машину/сеанс: второй запуск завершается с понятным
    // сообщением ещё до старта рантайма/TUI. См. spec §1.4.
    let _instance = match instance::acquire() {
        Ok(guard) => guard,
        Err(instance::InstanceError::AlreadyRunning) => {
            eprintln!("{}", loc.t("cli.instance.already_running"));
            tracing::warn!("отказ запуска: другой экземпляр приложения уже работает");
            return Ok(ExitCode::SUCCESS);
        }
        // Структурная ошибка → локализуем здесь (Display варианта — не для пользователя).
        Err(instance::InstanceError::Init(e)) => {
            return Err(anyhow!(
                "{}",
                loc.tf("cli.instance.init_failed", &[("err", e.as_str())])
            ));
        }
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        root = %paths.root().display(),
        "mindfork-rs starting"
    );

    // Миграция схем данных при старте (до открытия хранилища): downgrade-guard,
    // упрочнение битых файлов, pre-migration бэкап при непустом плане. Ошибка —
    // уже локализованное сообщение (features/data_migration). См. release-engineering.md §3.4.
    features::data_migration::run(paths, loc)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .with_context(|| loc.t("cli.ctx.build_runtime").to_string())?;

    // Хранилище (JSON + SQLite) рядом с бинарником. Единственный писатель —
    // оркестратор (spec §4.4.2). Arc — нужен инструментам в ToolContext.
    let storage = Arc::new(
        Storage::open(paths.clone()).with_context(|| loc.t("cli.ctx.open_storage").to_string())?,
    );

    let (cmd_tx, cmd_rx) = unbounded_channel::<AppCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<AppEvent>();

    // Конфиг из settings.json + посев переменными окружения (dev-workflow contract §9).
    // Свежая установка (нет settings.json) → язык интерфейса из defaults.json (ось B,
    // docs/i18n-ui.md §3.2); старый settings.json без поля → Ru через serde-default.
    let fresh_config = !paths.settings_file().exists();
    let mut config = storage.json().load_config().unwrap_or_default();
    if fresh_config {
        config.interface.language = paths.default_language();
    }
    apply_env_overrides(&mut config);

    runtime.spawn(orchestrator::run(OrchestratorDeps {
        cmd_rx,
        evt_tx: evt_tx.clone(),
        storage,
        config,
        supervisor: Arc::new(LlamaSupervisor),
        // Язык каркаса новых профилей — из defaults.json (ось A, docs/history/i18n.md).
        default_language: paths.default_language(),
    }));

    // Словари спелл-чека грузит сам `runtime` в фоне по настройкам интерфейса
    // (вкл/выкл + выбор словарей) и перегружает при их изменении. Резервный каталог
    // (рядом с бинарником) нужен, когда данные не портативны (`system`/`path`): словари
    // положены инсталлятором/пакетом рядом с бинарём, а не в корень данных (П1).
    let dict_dir = paths.dictionaries_dir();
    let bundled_dict_dir = paths.bundled_dictionaries_dir();
    let personal = paths.personal_dictionary();

    let result = app::runtime::run(cmd_tx.clone(), evt_rx, dict_dir, bundled_dict_dir, personal);

    // Останавливаем оркестратор; managed-серверы он гасит сам (kill_on_drop при
    // завершении его задачи). Даём фоновым задачам завершиться.
    let _ = cmd_tx.send(AppCommand::Quit);
    runtime.shutdown_timeout(Duration::from_secs(2));

    match &result {
        Ok(()) => tracing::info!("mindfork-rs exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork-rs exited with error"),
    }
    result.map(|()| ExitCode::SUCCESS)
}

/// Язык интерфейса CLI: явный из `settings.json` (сильнейший сигнал), иначе —
/// разрешённый язык умолчаний (`default_language` из `defaults.json`, а при его
/// отсутствии — определённый по локали ОС в [`Paths::resolve`]). Свежий бинарь без
/// конфигурации печатает на языке системы (по-английски, если локаль не `ru`).
fn cli_lang(settings_language: Option<Lang>, default_language: Lang) -> Lang {
    settings_language.unwrap_or(default_language)
}

/// Язык интерфейса из `settings.json`, если файл существует и парсится. `None` при
/// отсутствии/повреждении (та же терпимость, что `load_config().unwrap_or_default()`,
/// но отличает «файла нет» от «есть» — нужно для выбора языка CLI).
fn try_settings_language(settings_file: &Path) -> Option<Lang> {
    let bytes = std::fs::read(settings_file).ok()?;
    let cfg: AppConfig = serde_json::from_slice(&bytes).ok()?;
    Some(cfg.interface.language)
}

/// Печатает ошибку на заданном языке: `{локализованный префикс}: {цепочка причин}`
/// одной строкой (`{:#}`) — вместо английского `Error:`/`Caused by:` от std/anyhow.
fn print_error(lang: Lang, err: &anyhow::Error) {
    eprintln!("{}", cli_error_line(i18n::locale(lang), err));
}

/// Строка ошибки CLI (тестируемо, без запуска бинарника).
fn cli_error_line(loc: &Locale, err: &anyhow::Error) -> String {
    format!("{}: {err:#}", loc.t("cli.err.prefix"))
}

/// Захватывает блокировку единственного экземпляра для CLI-операции над данными
/// (бэкап/восстановление/песочница). Если приложение запущено — отказ (защита
/// целостности `data.db` от гонки с работающим оркестратором). `action` — уже
/// локализованное название действия (подставляется в сообщение).
fn acquire_cli_guard(loc: &Locale, action: &str) -> anyhow::Result<instance::InstanceGuard> {
    match instance::acquire() {
        Ok(guard) => Ok(guard),
        Err(instance::InstanceError::AlreadyRunning) => {
            bail!("{}", loc.tf("cli.guard.busy", &[("action", action)]))
        }
        Err(instance::InstanceError::Init(e)) => Err(anyhow!(
            "{}",
            loc.tf("cli.instance.init_failed", &[("err", e.as_str())])
        )),
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
fn run_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    compression: i64,
    loc: &Locale,
) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.backup"))?;
    let fs_root = config_fs_root(paths);
    let out = backup::create_backup(paths, output, compression, fs_root.as_deref(), loc)
        .with_context(|| loc.t("cli.ctx.backup").to_string())?;
    println!(
        "{}",
        loc.tf(
            "cli.backup.created",
            &[("path", &out.display().to_string())]
        )
    );
    Ok(())
}

/// CLI: восстановление из резервной копии (транзакционно, с pre-restore копией и
/// откатом при сбое). Вывод — в stdout/stderr (TUI не запущен).
fn run_restore(paths: &Paths, archive: &Path, loc: &Locale) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.restore"))?;
    let fs_root = config_fs_root(paths);

    // Предупреждение, если копия сделана более новой версией приложения: данные целы, но
    // текущая версия может отказаться их открыть (downgrade-guard, ADR 0006). Ошибку
    // чтения манифеста глотаем — старый бэкап без манифеста это нормально.
    if let Ok(Some(m)) = backup::read_manifest(archive)
        && m.is_newer_than_current()
    {
        eprintln!(
            "{}",
            loc.tf("backup.warn.newer_manifest", &[("version", &m.app_version)])
        );
    }

    // Err только до разрушительных действий (нет файла / повреждён / небезопасен).
    let outcome = backup::restore_backup(paths, archive, fs_root.as_deref(), loc)?;

    match outcome {
        RestoreOutcome::Restored { pre_restore } => {
            if let Some(pre) = pre_restore {
                println!(
                    "{}",
                    loc.tf(
                        "cli.restore.pre_saved",
                        &[("path", &pre.display().to_string())]
                    )
                );
                println!("{}", loc.t("cli.restore.cleared"));
            }
            println!(
                "{}",
                loc.tf(
                    "cli.restore.done",
                    &[("path", &archive.display().to_string())]
                )
            );
            Ok(())
        }
        RestoreOutcome::RolledBack {
            pre_restore,
            restore_error,
        } => {
            eprintln!(
                "{}",
                loc.tf(
                    "cli.restore.failed",
                    &[
                        ("path", &archive.display().to_string()),
                        ("err", &format!("{restore_error:#}")),
                    ],
                )
            );
            eprintln!(
                "{}",
                loc.tf(
                    "cli.restore.rolled_back",
                    &[("path", &pre_restore.display().to_string())]
                )
            );
            bail!("{}", loc.t("cli.restore.err_rolled_back"))
        }
        RestoreOutcome::Failed {
            pre_restore,
            restore_error,
            rollback_error,
        } => {
            eprintln!(
                "{}",
                loc.tf(
                    "cli.restore.failed",
                    &[
                        ("path", &archive.display().to_string()),
                        ("err", &format!("{restore_error:#}")),
                    ],
                )
            );
            if let Some(rb) = rollback_error {
                eprintln!(
                    "{}",
                    loc.tf(
                        "cli.restore.rollback_failed",
                        &[("err", &format!("{rb:#}"))]
                    )
                );
            }
            match pre_restore {
                Some(pre) => bail!(
                    "{}",
                    loc.tf(
                        "cli.restore.err_inconsistent",
                        &[("path", &pre.display().to_string())]
                    )
                ),
                None => bail!("{}", loc.t("cli.restore.err_failed")),
            }
        }
    }
}

/// CLI: установка/обновление песочницы Python (скачивание wasmer + python.webc +
/// пакетов в `data/sandbox/`). Требует собственный tokio-рантайм (сетевой async).
fn run_sandbox_setup(paths: &Paths, force: bool, loc: &Locale) -> anyhow::Result<()> {
    // Гард единственного экземпляра: не переустанавливаем песочницу, пока приложение
    // работает (могло бы читать заменяемый бинарь/ассеты во время индексации/запуска).
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.sandbox"))?;
    let dir = paths.sandbox_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .with_context(|| loc.t("cli.ctx.build_runtime").to_string())?;
    runtime.block_on(features::sandbox_setup::setup(
        &dir,
        &features::sandbox_setup::SetupOptions { force },
        loc,
        |msg| println!("{msg}"),
    ))?;
    Ok(())
}

/// CLI: экспорт бандла локали в файл-шаблон. `i18n::init` уже вызван в `main` (реестр
/// с внешними готов). Вывод — в stdout (TUI не запущен).
fn run_locales_export(code: &str, output: &Path, loc: &Locale) -> anyhow::Result<()> {
    if output.exists() {
        bail!(
            "{}",
            loc.tf(
                "cli.locales.file_exists",
                &[("path", &output.display().to_string())]
            )
        );
    }
    let lang = Lang::from_code(code);
    let content = i18n::export_bundle(lang);
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| {
            loc.tf(
                "cli.ctx.create_dir",
                &[("path", &parent.display().to_string())],
            )
        })?;
    }
    std::fs::write(output, content).with_context(|| {
        loc.tf(
            "cli.ctx.write_file",
            &[("path", &output.display().to_string())],
        )
    })?;
    println!(
        "{}",
        loc.tf(
            "cli.locales.exported",
            &[("code", code), ("path", &output.display().to_string())]
        )
    );
    Ok(())
}

/// Одноразовый импорт данных LameLLaMA (.NET) в хранилище mindfork (spec §12.2).
/// Идемпотентно (детерминированные id), исходные файлы только читаются. Вывод —
/// в stdout (TUI не запущен), не в лог.
fn run_import(paths: &Paths, dir: &Path, loc: &Locale) -> anyhow::Result<()> {
    // Существующие данные могут требовать миграции (или быть из более новой версии) —
    // мигрируем перед открытием хранилища, как при обычном старте (release-engineering.md §3.4).
    features::data_migration::run(paths, loc)?;
    let storage =
        Storage::open(paths.clone()).with_context(|| loc.t("cli.ctx.open_storage").to_string())?;
    let result = features::migration::import_dir(dir, loc)
        .with_context(|| loc.tf("cli.ctx.import", &[("dir", &dir.display().to_string())]))?;

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
        "{}",
        loc.tf(
            "cli.import.done",
            &[
                ("profiles", &result.profiles.len().to_string()),
                ("chats", &result.chats.len().to_string()),
            ]
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_lang_prefers_settings_then_resolved_default() {
        // Явный язык из settings.json — сильнейший сигнал (перекрывает умолчание).
        assert_eq!(cli_lang(Some(Lang::Ru), Lang::En), Lang::Ru);
        assert_eq!(cli_lang(Some(Lang::En), Lang::Ru), Lang::En);
        // Нет settings → разрешённый язык умолчаний (явный из defaults.json ИЛИ
        // определённый по локали ОС в Paths::resolve — сюда приходит уже готовым).
        assert_eq!(cli_lang(None, Lang::Ru), Lang::Ru);
        assert_eq!(cli_lang(None, Lang::En), Lang::En);
    }

    #[test]
    fn cli_error_line_is_localized_single_line() {
        let err = anyhow!("outer").context("wrapper");
        let en = cli_error_line(i18n::locale(Lang::En), &err);
        // Локализованный префикс + однострочная цепочка причин (нет многострочного Debug).
        assert!(en.starts_with("Error: "), "{en}");
        assert!(en.contains("wrapper") && en.contains("outer"), "{en}");
        assert!(!en.contains('\n'), "{en}");
        // Русский префикс на ru-локали.
        let ru = cli_error_line(i18n::locale(Lang::Ru), &err);
        assert!(ru.starts_with("Ошибка: "), "{ru}");
    }
}
