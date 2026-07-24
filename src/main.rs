//! mindfork-rs — a console (TUI) AI chat application.
//! Entry point: the "peek" phase (language/root before parsing arguments) → CLI
//! parsing → single-instance → logging → tokio runtime → orchestrator → TUI.
//! See spec §4.2, §4.4, plan M1, and docs/history/i18n-cli.md (all CLI text is in locale bundles).

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
    // The "peek" phase: determine the data root and CLI language **before** parsing
    // arguments and without creating directories (`--help`/`--version` shouldn't touch
    // disk — docs/history/i18n-cli.md §3.2). The language is needed first of all, so even
    // help and parse errors come out in it.
    let (paths, lang) = match Paths::resolve() {
        Ok(paths) => {
            let settings_lang = try_settings_language(&paths.settings_file());
            let lang = cli_lang(settings_lang, paths.default_language());
            (paths, lang)
        }
        // A resolve failure (realistically — only a corrupt `defaults.json`): the language
        // is unknown → total uncertainty → print in English (user's decision).
        Err(err) => {
            print_error(Lang::En, &err);
            return ExitCode::FAILURE;
        }
    };

    // External locales (`data/locales/*.json`) are scanned before parsing, so `--help`
    // respects their overrides/new languages. `init` runs before `logging::init`, so
    // warnings about corrupt files are returned and logged later (in `real_main`);
    // on early-exit paths (`--help`) they're silently dropped.
    let locale_warnings = i18n::init(&paths.locales_dir());
    let loc = i18n::locale(lang);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args, loc) {
        Ok(cmd) => cmd,
        // `Err` — an already print-ready localized message (§ features/cli).
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    // A fast path with no side effects: help/version create no directories or logs.
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

/// Side-effectful commands: creates directories, brings up logging, dispatches.
/// Errors are printed by the caller (`main`) via [`print_error`] — a single localized
/// prefix + a single-line chain of causes (`{:#}`), no English `Error:`/`Caused by:`.
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
    // Warnings about external locales (collected before the log subscriber is set up).
    for w in locale_warnings {
        tracing::warn!("{w}");
    }

    match command {
        CliCommand::Import { file } => {
            run_import(paths, &file, loc)?;
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
        CliCommand::Help { .. } | CliCommand::Version => unreachable!("handled in main"),
    }
}

/// Launches the main TUI (a command with no subcommand).
fn run_tui(paths: &Paths, loc: &Locale) -> anyhow::Result<ExitCode> {
    // Single instance per machine/session: a second launch exits with a clear
    // message before the runtime/TUI even starts. See spec §1.4.
    let _instance = match instance::acquire() {
        Ok(guard) => guard,
        Err(instance::InstanceError::AlreadyRunning) => {
            eprintln!("{}", loc.t("cli.instance.already_running"));
            tracing::warn!("startup refused: another instance of the app is already running");
            return Ok(ExitCode::SUCCESS);
        }
        // A structured error → localized here (the variant's Display isn't user-facing).
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

    // Data schema migration at startup (before opening storage): downgrade guard,
    // hardening against corrupt files, a pre-migration backup when the plan is non-empty.
    // The error is an already-localized message (features/data_migration). See release-engineering.md §3.4.
    features::data_migration::run(paths, loc)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .with_context(|| loc.t("cli.ctx.build_runtime").to_string())?;

    // Storage (JSON + SQLite) next to the binary. The sole writer is the
    // orchestrator (spec §4.4.2). Arc — needed by tools in ToolContext.
    let storage = Arc::new(
        Storage::open(paths.clone()).with_context(|| loc.t("cli.ctx.open_storage").to_string())?,
    );

    let (cmd_tx, cmd_rx) = unbounded_channel::<AppCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<AppEvent>();

    // Config from settings.json + seeding via environment variables (dev-workflow contract §9).
    // A fresh install (no settings.json) → interface language from defaults.json (axis B,
    // docs/i18n-ui.md §3.2); an old settings.json without the field → Ru via serde-default.
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
        // New profiles' scaffold language — from defaults.json (axis A, docs/history/i18n.md).
        default_language: paths.default_language(),
    }));

    // Spellcheck dictionaries are loaded by `runtime` itself in the background per the
    // interface settings (on/off + dictionary selection) and reloaded when they change.
    // The fallback directory (next to the binary) is needed when data isn't portable
    // (`system`/`path`): the installer/package places dictionaries next to the binary,
    // not in the data root (P1).
    let dict_dir = paths.dictionaries_dir();
    let bundled_dict_dir = paths.bundled_dictionaries_dir();
    let personal = paths.personal_dictionary();

    let result = app::runtime::run(cmd_tx.clone(), evt_rx, dict_dir, bundled_dict_dir, personal);

    // Stop the orchestrator; it tears down managed servers itself (kill_on_drop when
    // its task ends). Give background tasks a chance to finish.
    let _ = cmd_tx.send(AppCommand::Quit);
    runtime.shutdown_timeout(Duration::from_secs(2));

    match &result {
        Ok(()) => tracing::info!("mindfork-rs exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork-rs exited with error"),
    }
    result.map(|()| ExitCode::SUCCESS)
}

/// CLI interface language: explicit from `settings.json` (the strongest signal),
/// otherwise the resolved default language (`default_language` from `defaults.json`,
/// or, absent that, the one detected from the OS locale in [`Paths::resolve`]). A fresh
/// binary with no configuration prints in the system's language (English if the locale
/// isn't `ru`).
fn cli_lang(settings_language: Option<Lang>, default_language: Lang) -> Lang {
    settings_language.unwrap_or(default_language)
}

/// The interface language from `settings.json`, if the file exists and parses. `None`
/// when it's missing/corrupt (the same leniency as `load_config().unwrap_or_default()`,
/// but distinguishes "no file" from "there is one" — needed for choosing the CLI language).
fn try_settings_language(settings_file: &Path) -> Option<Lang> {
    let bytes = std::fs::read(settings_file).ok()?;
    let cfg: AppConfig = serde_json::from_slice(&bytes).ok()?;
    Some(cfg.interface.language)
}

/// Prints an error in the given language: `{localized prefix}: {chain of causes}`
/// on a single line (`{:#}`) — instead of the English `Error:`/`Caused by:` from std/anyhow.
fn print_error(lang: Lang, err: &anyhow::Error) {
    eprintln!("{}", cli_error_line(i18n::locale(lang), err));
}

/// The CLI error line (testable, without running the binary).
fn cli_error_line(loc: &Locale, err: &anyhow::Error) -> String {
    format!("{}: {err:#}", loc.t("cli.err.prefix"))
}

/// Acquires the single-instance lock for a CLI operation over the data
/// (backup/restore/sandbox). If the app is running — a refusal (protects
/// `data.db`'s integrity from a race with the running orchestrator). `action` — an
/// already-localized action name (substituted into the message).
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

/// The file-tools sandbox from the config (for inclusion/cleanup during backup).
fn config_fs_root(paths: &Paths) -> Option<PathBuf> {
    JsonStore::new(paths.clone())
        .load_config()
        .unwrap_or_default()
        .tools
        .fs_root
        .map(PathBuf::from)
}

/// CLI: creating a backup. Output goes to stdout (the TUI isn't running).
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

/// CLI: restoring from a backup (transactionally, with a pre-restore copy and a
/// rollback on failure). Output goes to stdout/stderr (the TUI isn't running).
fn run_restore(paths: &Paths, archive: &Path, loc: &Locale) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.restore"))?;
    let fs_root = config_fs_root(paths);

    // Warn if the backup was made by a newer version of the app: the data is intact, but
    // the current version might refuse to open it (downgrade guard, ADR 0006). We swallow
    // a manifest-read error — an old backup with no manifest is normal.
    if let Ok(Some(m)) = backup::read_manifest(archive)
        && m.is_newer_than_current()
    {
        eprintln!(
            "{}",
            loc.tf("backup.warn.newer_manifest", &[("version", &m.app_version)])
        );
    }

    // Err only before any destructive action (missing file / corrupt / unsafe).
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

/// CLI: installing/updating the Python sandbox (downloading wasmer + python.webc +
/// packages into `data/sandbox/`). Needs its own tokio runtime (network async).
fn run_sandbox_setup(paths: &Paths, force: bool, loc: &Locale) -> anyhow::Result<()> {
    // Single-instance guard: don't reinstall the sandbox while the app is
    // running (it could read the binary/assets being replaced during indexing/startup).
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

/// CLI: exports a locale bundle to a template file. `i18n::init` was already called in `main` (the
/// registry with external ones is ready). Output — to stdout (the TUI isn't running).
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

/// A one-shot import from a mindfork-import format file (spec §12.2,
/// docs/import-format.md). Idempotent (deterministic ids), the source file
/// is only ever read. Output — to stdout (the TUI isn't running), not to the log.
fn run_import(paths: &Paths, file: &Path, loc: &Locale) -> anyhow::Result<()> {
    // Existing data may need migrating (or be from a newer version) — we
    // migrate before opening storage, as on a regular startup (release-engineering.md §3.4).
    features::data_migration::run(paths, loc)?;
    let storage =
        Storage::open(paths.clone()).with_context(|| loc.t("cli.ctx.open_storage").to_string())?;
    let result = features::import::import_file(file, loc)
        .with_context(|| loc.tf("cli.ctx.import", &[("file", &file.display().to_string())]))?;

    for profile in &result.profiles {
        storage.json().upsert_profile(profile)?;
    }
    for chat in &result.chats {
        storage.json().save_chat(chat)?;
    }

    // Carry over the source's global settings (only fields that are set — a partial
    // transfer doesn't overwrite the user's settings).
    let mut config = storage.json().load_config().unwrap_or_default();
    if let Some(sampling) = result.sampling {
        config.default_sampling = sampling;
    }
    if let Some(interface) = result.interface {
        if let Some(v) = interface.spellcheck_enabled {
            config.interface.spellcheck_enabled = v;
        }
        if let Some(v) = interface.dictionaries {
            config.interface.selected_dictionaries = v;
        }
        if let Some(v) = interface.theme {
            config.interface.theme = v;
        }
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

/// Seeds the config from environment variables (dev/smoke workflow). Env takes
/// priority over `settings.json`, so a quick launch against a server doesn't require
/// editing the file. Affects only the chat/embedding servers:
/// - `MINDFORK_ENGINE_URL` — an external chat server (any OpenAI-compatible one);
/// - `MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
///   `MINDFORK_PORT`) — a managed `llama-server`;
/// - `MINDFORK_EMBED_URL` / `MINDFORK_EMBED_BIN` (+ `MINDFORK_EMBED_MODEL`,
///   `MINDFORK_EMBED_PORT`) — the embedding server.
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
        // An explicit language from settings.json — the strongest signal (overrides the default).
        assert_eq!(cli_lang(Some(Lang::Ru), Lang::En), Lang::Ru);
        assert_eq!(cli_lang(Some(Lang::En), Lang::Ru), Lang::En);
        // No settings → the resolved default language (explicit from defaults.json OR
        // detected from the OS locale in Paths::resolve — arrives here already resolved).
        assert_eq!(cli_lang(None, Lang::Ru), Lang::Ru);
        assert_eq!(cli_lang(None, Lang::En), Lang::En);
    }

    #[test]
    fn cli_error_line_is_localized_single_line() {
        let err = anyhow!("outer").context("wrapper");
        let en = cli_error_line(i18n::locale(Lang::En), &err);
        // A localized prefix + a single-line cause chain (no multiline Debug).
        assert!(en.starts_with("Error: "), "{en}");
        assert!(en.contains("wrapper") && en.contains("outer"), "{en}");
        assert!(!en.contains('\n'), "{en}");
        // A Russian prefix in the ru locale.
        let ru = cli_error_line(i18n::locale(Lang::Ru), &err);
        assert!(ru.starts_with("Ошибка: "), "{ru}");
    }
}
