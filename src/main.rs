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

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use tokio::sync::mpsc::unbounded_channel;

use crate::app::events::{AppCommand, AppEvent};
use crate::app::orchestrator::{self, OrchestratorDeps};
use crate::app::supervisor::{DemoSupervisor, LlamaSupervisor, ServerSupervisor};
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
            // Double-clicked, this window is about to vanish with the message in
            // it (docs/research/robustness-and-defaults.md D3).
            shared::console::hold_if_sole_owner(i18n::locale(Lang::En));
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
            shared::console::hold_if_sole_owner(loc);
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
            shared::console::hold_if_sole_owner(loc);
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
    // A full-screen launch with nowhere to draw is refused before anything is
    // created, the way `--help` touches nothing. Without this the app entered
    // the alternate screen, wrote escape codes into a redirected stdout and waited
    // for keys forever (docs/research/public-release-readiness.md §2.2 B8).
    if refuses_tui_launch(&command, std::io::stdout().is_terminal()) {
        eprintln!("{}", loc.t("cli.tui.no_terminal"));
        return Ok(ExitCode::from(2));
    }
    // The demo never touches the real data root — branch off before the real
    // root's directories or log file are even created. Its own root, logging
    // and cleanup live in `run_demo`.
    if matches!(command, CliCommand::Demo) {
        return run_demo(loc, locale_warnings);
    }
    // `stats` only reads (spec §12.4): it is answered here, before the data
    // root's directories and the log file are created, so that summarizing a
    // copy leaves that copy byte for byte what it was — and a machine with no
    // data yet still has none afterwards.
    if let CliCommand::Stats {
        archive,
        password,
        json,
    } = command
    {
        return run_stats(paths, archive.as_deref(), password, json, loc);
    }
    paths.ensure_dirs(loc).with_context(|| {
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
            password,
        } => {
            run_backup(paths, output, compression, password, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::Restore { archive, password } => {
            run_restore(paths, &archive, password, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::SandboxSetup {
            force,
            enable_python,
        } => {
            run_sandbox_setup(paths, force, enable_python, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::LlamaBackends { build } => {
            run_llama_backends(paths, build.as_deref(), loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::LlamaSetup {
            backend,
            build,
            cudart,
            force,
            set_binary,
        } => run_llama_setup(paths, backend, build, cudart, force, set_binary, loc),
        CliCommand::LlamaInstalled => {
            run_llama_installed(paths, loc);
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::LlamaRemove { id, force } => {
            run_llama_remove(paths, &id, force, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::LocalesExport { code, output } => {
            run_locales_export(&code, &output, loc)?;
            Ok(ExitCode::SUCCESS)
        }
        CliCommand::Run => run_tui(paths, loc),
        CliCommand::Demo | CliCommand::Stats { .. } => {
            unreachable!("handled above, before the real root is touched")
        }
        CliCommand::Help { .. } | CliCommand::Version => unreachable!("handled in main"),
    }
}

/// Whether `command` would start the full-screen interface with no terminal to
/// draw it on. Only stdout is asked (docs/research/public-release-readiness.md
/// §3.4, fork F1(a)): the
/// screen goes there, while keys never come from stdin — crossterm opens the
/// console itself (`CONIN$` on Windows, `/dev/tty` on unix), so a piped stdin is
/// a launch that works and must not be refused.
fn refuses_tui_launch(command: &CliCommand, stdout_is_terminal: bool) -> bool {
    matches!(command, CliCommand::Run | CliCommand::Demo) && !stdout_is_terminal
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
            // The likeliest double-click of all: the user clicks the shortcut
            // again because the first window is behind something.
            shared::console::hold_if_sole_owner(loc);
            // 2 — "refused to start", the code the CLI already answers a wrong
            // invocation and a launch without a terminal (B8) with. It used to be
            // 0, which told a script that the app had run and exited cleanly
            // (docs/research/robustness-and-defaults.md D4).
            return Ok(ExitCode::from(2));
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
        "mindfork starting"
    );

    // Data schema migration at startup (before opening storage): downgrade guard,
    // hardening against corrupt files, a pre-migration backup when the plan is non-empty.
    // The error is an already-localized message (features/data_migration). See release-engineering.md §3.4.
    features::data_migration::run(paths, loc)?;

    launch_tui(paths, Arc::new(LlamaSupervisor::new(paths)), true, loc)
}

/// `mindfork demo` — the real TUI on a throwaway root with a scripted engine
/// (the demo-screenshots track, stage 3). Deliberately skipped relative to
/// [`run_tui`]: the single-instance guard (a demo may run next to the real
/// app), data migration (the root is born current) and the `MINDFORK_*` env
/// overrides (the environment belongs to the real app). The root is removed
/// on a clean exit; a crash leaves at most a folder in the OS temp dir.
fn run_demo(loc: &Locale, locale_warnings: &[String]) -> anyhow::Result<ExitCode> {
    let root = std::env::temp_dir().join(format!("mindfork-demo-{}", std::process::id()));
    let paths = Paths::with_root(&root);
    paths.ensure_dirs(loc).with_context(|| {
        loc.tf(
            "cli.ctx.ensure_dirs",
            &[("path", &root.display().to_string())],
        )
    })?;
    let _logging =
        logging::init(&paths, loc).with_context(|| loc.t("cli.ctx.init_logging").to_string())?;
    for w in locale_warnings {
        tracing::warn!("{w}");
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        root = %root.display(),
        "mindfork demo starting"
    );

    // Provision, then drop this handle: the orchestrator inside `launch_tui`
    // opens its own storage and must stay the sole writer (spec §4.4.2).
    {
        let storage = Storage::open(paths.clone())
            .with_context(|| loc.t("cli.ctx.open_storage").to_string())?;
        features::demo::provision(&storage)
            .with_context(|| loc.t("cli.ctx.provision_demo").to_string())?;
    }

    let backend = Arc::new(crate::shared::api::mock::MockBackend::cycling(
        features::demo::demo_replies(),
        DEMO_STREAM_DELAY_MS,
    ));
    let result = launch_tui(&paths, Arc::new(DemoSupervisor::new(backend)), false, loc);
    // Leave nothing behind: the promise is "your machine, untouched".
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// Pause between the demo engine's chunks: streaming should look like
/// streaming, not like a page load.
const DEMO_STREAM_DELAY_MS: u64 = 18;

/// The shared TUI launch: tokio runtime, storage, orchestrator, UI loop,
/// shutdown. [`run_tui`] wraps it with the single-instance guard and data
/// migration; [`run_demo`] boots it on a throwaway root with the scripted
/// supervisor and `apply_env: false`.
fn launch_tui(
    paths: &Paths,
    supervisor: Arc<dyn ServerSupervisor>,
    apply_env: bool,
    loc: &Locale,
) -> anyhow::Result<ExitCode> {
    // Ask the terminal for its background **first**, and collect the answer
    // once the UI is up (`app::runtime::run`). JupyterLab needs 382 ms cold for
    // the reply — it round-trips to a browser — so asking synchronously would
    // put that on startup; asking here hides the wait behind the tokio runtime,
    // storage and the orchestrator, all of which have to happen anyway. On
    // Windows this already *is* the whole exchange (16-31 ms there, and the
    // console mode it needs must not outlive it). Silence is normal — legacy
    // conhost never answers. See docs/terminal-background-detection.md.
    let background_query = crate::shared::osc11::begin();

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
    if apply_env {
        apply_env_overrides(&mut config);
    }

    // The handle is kept rather than dropped: when the UI loop ends because its
    // event channel closed (D1), this is what holds the reason — a panic payload
    // that would otherwise reach only the panic hook's output, which the
    // alternate screen has already eaten.
    let orchestrator = runtime.spawn(orchestrator::run(OrchestratorDeps {
        cmd_rx,
        evt_tx: evt_tx.clone(),
        storage,
        config,
        supervisor,
        // New profiles' scaffold language — from defaults.json (axis A, docs/history/i18n.md).
        default_language: paths.default_language(),
        extra_tools: Vec::new(),
    }));

    // Spellcheck dictionaries are loaded by `runtime` itself in the background per the
    // interface settings (on/off + dictionary selection) and reloaded when they change.
    // The fallback directory (next to the binary) is needed when data isn't portable
    // (`system`/`path`): the installer/package places dictionaries next to the binary,
    // not in the data root (P1).
    let dict_dir = paths.dictionaries_dir();
    let bundled_dict_dir = paths.bundled_dictionaries_dir();
    let personal = paths.personal_dictionary();

    let result = app::runtime::run(
        cmd_tx.clone(),
        evt_rx,
        dict_dir,
        bundled_dict_dir,
        personal,
        paths.log_dir(),
        background_query,
    );

    // A failed session with a finished orchestrator is the D1 case: take the
    // task's own error into the log before anything else is printed. `is_finished`
    // first, so a clean exit never waits on a task that is still shutting down.
    if result.is_err() && orchestrator.is_finished() {
        match runtime.block_on(orchestrator) {
            Err(join) if join.is_panic() => {
                tracing::error!(error = %join, "the orchestrator task panicked");
            }
            Err(join) => tracing::error!(error = %join, "the orchestrator task ended abnormally"),
            Ok(()) => tracing::error!("the orchestrator task ended while the session was running"),
        }
    }

    // Stop the orchestrator; it tears down managed servers itself (kill_on_drop when
    // its task ends). Give background tasks a chance to finish.
    let _ = cmd_tx.send(AppCommand::Quit);
    runtime.shutdown_timeout(Duration::from_secs(2));

    match &result {
        Ok(()) => tracing::info!("mindfork exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork exited with error"),
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

/// What the backup/restore commands need out of the config: the file-tools
/// sandbox (for inclusion/cleanup) and the stored backup password.
struct BackupConfig {
    fs_root: Option<PathBuf>,
    stored_password: Option<String>,
}

fn backup_config(paths: &Paths) -> BackupConfig {
    let config = JsonStore::new(paths.clone())
        .load_config()
        .unwrap_or_default();
    BackupConfig {
        fs_root: config.tools.fs_root.map(PathBuf::from),
        stored_password: crate::shared::secrets::stored_key(
            &config.api_keys,
            crate::shared::secrets::BACKUP_PASSWORD_KEY,
        ),
    }
}

/// The run's one effective password: the argument wins over the stored setting
/// (docs/history/backup-password.md §4 F8). Empty means "no password".
fn effective_password(arg: Option<String>, stored: Option<String>) -> Option<String> {
    arg.or(stored).filter(|p| !p.is_empty())
}

/// CLI: creating a backup. Output goes to stdout (the TUI isn't running).
fn run_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    compression: i64,
    password: Option<String>,
    loc: &Locale,
) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.backup"))?;
    let cfg = backup_config(paths);
    let password = effective_password(password, cfg.stored_password);
    let out = backup::create_backup(
        paths,
        output,
        compression,
        cfg.fs_root.as_deref(),
        password.as_deref(),
        loc,
        |msg| println!("{msg}"),
    );
    // Whatever the outcome: keys pressed during the packing were typed at us,
    // not at the shell that inherits the terminal when we exit.
    features::terminal_input::discard_type_ahead();
    let out = out.with_context(|| loc.t("cli.ctx.backup").to_string())?;
    if password.is_some() {
        println!("{}", loc.t("cli.backup.encrypted"));
    }
    println!(
        "{}",
        loc.tf(
            "cli.backup.created",
            &[("path", &out.display().to_string())]
        )
    );
    Ok(())
}

/// CLI: `stats` — a summary of the user data, live or inside a backup archive
/// (docs/data-stats.md, spec §12.4). Takes no single-instance guard: nothing is
/// written, a chat file is replaced atomically by the running app (a reader
/// sees the old or the new one, never half), and the database is opened
/// read-only — so the command works next to a running app, which is when the
/// question "is this copy the newest?" tends to come up.
///
/// An archive's password is settled exactly as `restore` settles it — the
/// argument, else the one stored in the settings, else a prompt — through the
/// same function, so the two commands cannot come to disagree about one file.
fn run_stats(
    paths: &Paths,
    archive: Option<&Path>,
    password: Option<String>,
    json: bool,
    loc: &Locale,
) -> anyhow::Result<ExitCode> {
    use crate::features::data_stats;

    let stats = match archive {
        None => data_stats::collect_root(paths, loc)?,
        Some(archive) => {
            let stored = backup_config(paths).stored_password;
            let password = resolve_restore_password(archive, password, stored, loc)?;
            data_stats::collect_archive(archive, password.as_deref(), loc)?
        }
    };
    if json {
        println!("{}", data_stats::render_json(&stats));
    } else {
        println!("{}", data_stats::render_text(&stats, loc));
    }
    Ok(ExitCode::SUCCESS)
}

/// How many times the restore password may be re-entered before giving up.
const PASSWORD_ATTEMPTS: usize = 3;

/// Settles the password to restore `archive` with, prompting when needed.
///
/// The password we already have (argument, else the setting) is tried first; a
/// prompt only appears when it is missing or wrong **and** stdin is a terminal
/// — restoring an archive from another machine is exactly the case where no
/// stored password can apply (ADR 0008: secrets don't travel). Returning the
/// unusable password rather than erroring here keeps every "no/wrong password"
/// message in one place — `backup::restore_backup`'s pre-flight validation.
///
/// One exception, found on a live run of `stats`: when the password that failed
/// is the **stored** one, "wrong backup password" accuses the user of a password
/// they never typed, and names no way forward. Each machine keeps its own
/// (ADR 0008), so an archive from another machine meets exactly this. That case
/// says what happened and that `--password` is the route ([`stored_password_failed`]).
fn resolve_restore_password(
    archive: &Path,
    argument: Option<String>,
    stored: Option<String>,
    loc: &Locale,
) -> anyhow::Result<Option<String>> {
    use crate::features::backup::ArchivePassword;
    use crate::features::terminal_input;

    let from_settings = argument.is_none();
    let mut current = effective_password(argument, stored);
    for attempt in 0..PASSWORD_ATTEMPTS {
        // A read failure here is not ours to report: restore_backup validates
        // the archive properly and produces the localized error.
        let Ok(status) = backup::check_password(archive, current.as_deref()) else {
            return Ok(current);
        };
        match status {
            ArchivePassword::NotNeeded | ArchivePassword::Ok => return Ok(current),
            ArchivePassword::Required | ArchivePassword::Wrong => {
                let stored_failed = stored_password_failed(from_settings, attempt, status);
                if !terminal_input::is_interactive() {
                    if stored_failed {
                        bail!("{}", loc.t("cli.restore.stored_password_wrong"));
                    }
                    return Ok(current);
                }
                if stored_failed {
                    eprintln!("{}", loc.t("cli.restore.stored_password_wrong"));
                } else if status == ArchivePassword::Wrong {
                    eprintln!("{}", loc.t("backup.err.wrong_password"));
                }
                match terminal_input::read_password(loc.t("cli.restore.password_prompt"))? {
                    Some(entered) => current = Some(entered),
                    // Cancelled: hand back what we had, so the refusal is the
                    // regular localized one rather than a bare exit.
                    None => return Ok(current),
                }
            }
        }
    }
    Ok(current)
}

/// Whether the password the archive just refused is the one from the settings
/// rather than one the user gave: no `--password` argument, the first attempt
/// (a later one was typed at the prompt), and a password that was *tried* —
/// `Required` means there was none to blame.
fn stored_password_failed(
    from_settings: bool,
    attempt: usize,
    status: crate::features::backup::ArchivePassword,
) -> bool {
    from_settings && attempt == 0 && status == crate::features::backup::ArchivePassword::Wrong
}

/// CLI: restoring from a backup (transactionally, with a pre-restore copy and a
/// rollback on failure). Output goes to stdout/stderr (the TUI isn't running).
fn run_restore(
    paths: &Paths,
    archive: &Path,
    password: Option<String>,
    loc: &Locale,
) -> anyhow::Result<()> {
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.restore"))?;
    let cfg = backup_config(paths);
    let password = resolve_restore_password(archive, password, cfg.stored_password, loc)?;

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

    // Err only before any destructive action (missing file / corrupt / unsafe /
    // missing or wrong password).
    let outcome = backup::restore_backup(
        paths,
        archive,
        cfg.fs_root.as_deref(),
        password.as_deref(),
        loc,
        |msg| println!("{msg}"),
    );
    // Whatever the outcome: keys pressed during the restore were typed at us
    // (an impatient `Enter` after the password prompt, above all), not at the
    // shell that inherits the terminal when we exit.
    features::terminal_input::discard_type_ahead();
    let outcome = outcome?;

    match outcome {
        RestoreOutcome::Restored { pre_restore } => {
            // Where the previous data went, repeated after the progress lines
            // have scrolled past: this is the path to undo a restore with.
            if let Some(pre) = pre_restore {
                println!(
                    "{}",
                    loc.tf(
                        "cli.restore.pre_saved",
                        &[("path", &pre.display().to_string())]
                    )
                );
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
fn run_sandbox_setup(
    paths: &Paths,
    force: bool,
    enable_python: bool,
    loc: &Locale,
) -> anyhow::Result<()> {
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
    // Only after a successful provisioning: `?` above means we never get here
    // otherwise, which is what keeps ADR 0005 §5's "enabled but not provisioned is
    // worse than disabled" true.
    if enable_python {
        enable_python_tool(paths, loc)?;
        println!("{}", loc.t("cli.sandbox.python_enabled"));
    }
    Ok(())
}

/// A tokio runtime for a CLI command whose work is async (the network).
fn cli_runtime(loc: &Locale) -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .with_context(|| loc.t("cli.ctx.build_runtime").to_string())
}

/// CLI: the llama.cpp backends published for this OS and architecture
/// (`mindfork llama backends`). Read-only — no data is touched, so no guard.
fn run_llama_backends(paths: &Paths, build: Option<&str>, loc: &Locale) -> anyhow::Result<()> {
    let runtime = cli_runtime(loc)?;
    let listing = runtime.block_on(features::llama_setup::list_backends(build, loc))?;
    for line in features::llama_setup::render_backends(&listing, &paths.llama_dir(), loc) {
        println!("{line}");
    }
    Ok(())
}

/// CLI: downloading a llama.cpp backend into `data/llama/`
/// (`mindfork llama setup --backend <id>`). Needs its own tokio runtime
/// (network async), as `sandbox setup` does.
fn run_llama_setup(
    paths: &Paths,
    backend: Option<String>,
    build: Option<String>,
    cudart: bool,
    force: bool,
    set_binary: bool,
    loc: &Locale,
) -> anyhow::Result<ExitCode> {
    let root = paths.llama_dir();
    let runtime = cli_runtime(loc)?;
    // There is deliberately no default backend (docs/research/llama-cpp-download.md
    // §6 F3): the choice costs between 18 MB and 645 MB, so an omitted
    // `--backend` prints what is on offer and exits non-zero instead of
    // deciding for the user.
    let Some(backend) = backend else {
        let listing =
            runtime.block_on(features::llama_setup::list_backends(build.as_deref(), loc))?;
        for line in features::llama_setup::render_backends(&listing, &root, loc) {
            println!("{line}");
        }
        eprintln!("{}", loc.t("cli.llama.pick_backend"));
        return Ok(ExitCode::from(2));
    };
    // Single-instance guard, for the reason `sandbox setup` takes one: a
    // reinstall replaces files the running app may be about to launch, and on
    // Windows a `llama-server` started from the same directory holds them open.
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.llama"))?;
    let installed = runtime.block_on(features::llama_setup::setup(
        &root,
        &features::llama_setup::SetupOptions {
            backend,
            build,
            force,
            cudart,
        },
        loc,
        |msg| println!("{msg}"),
    ))?;
    // Past the `?`: a failed install must never leave a path behind pointing at
    // something that is not there — the ordering `--enable-python` fixed for the
    // same reason (ADR 0005 §5).
    if set_binary {
        for line in set_engine_binary(paths, &installed.binary, loc)? {
            println!("{line}");
        }
    } else {
        println!(
            "{}",
            loc.tf(
                "cli.llama.binary_hint",
                &[("path", &installed.binary.display().to_string())],
            )
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// CLI: deleting a downloaded build (`mindfork llama remove <id>`).
///
/// Reads `settings.json` but never writes it: a build the settings point at is
/// **refused** unless `--force`, because removing it silently leaves three
/// fields aiming at nothing and the failure would surface a launch later. The
/// closing line says what an empty binary field resolves to now — the meaning
/// of that field changes when the build it was resolving to is the one that
/// just went away.
fn run_llama_remove(paths: &Paths, id: &str, force: bool, loc: &Locale) -> anyhow::Result<()> {
    use crate::features::llama_setup as llama;

    let root = paths.llama_dir();
    // The same guard `llama setup` takes: on Windows a running server holds the
    // very files this deletes.
    let _instance = acquire_cli_guard(loc, loc.t("cli.guard.action.llama_remove"))?;

    let all = llama::installed(&root);
    let Some(install) = llama::find_install(&root, id) else {
        let names: Vec<String> = all
            .iter()
            .map(|i| llama::install_name(&i.backend, &i.tag))
            .collect();
        if names.is_empty() {
            bail!(
                "{}",
                loc.tf(
                    "llamacpp.remove.none",
                    &[("path", &root.display().to_string())]
                )
            );
        }
        // A bare backend that names two builds is a different mistake from a
        // name that matches nothing, and gets its own sentence.
        let ambiguous: Vec<String> = all
            .iter()
            .filter(|i| i.backend == id)
            .map(|i| llama::install_name(&i.backend, &i.tag))
            .collect();
        if ambiguous.len() > 1 {
            bail!(
                "{}",
                loc.tf(
                    "llamacpp.remove.ambiguous",
                    &[("id", id), ("list", &ambiguous.join(", "))],
                )
            );
        }
        bail!(
            "{}",
            loc.tf(
                "llamacpp.remove.unknown",
                &[
                    ("id", id),
                    ("path", &root.display().to_string()),
                    ("list", &names.join(", ")),
                ],
            )
        );
    };

    let config = JsonStore::new(paths.clone())
        .load_config()
        .unwrap_or_default();
    let uses = llama::binary_uses(&config, &install.dir);
    if !uses.is_empty() && !force {
        bail!("{}", llama::render_in_use(&uses, &install, loc));
    }

    println!(
        "{}",
        loc.tf(
            "llamacpp.remove.removing",
            &[
                ("path", &install.dir.display().to_string()),
                ("size", &(install.bytes >> 20).to_string()),
            ],
        )
    );
    llama::remove_install(&install.dir, loc)?;
    for line in llama::render_removed(&install, &root, paths.exe_dir(), loc) {
        println!("{line}");
    }
    Ok(())
}

/// Writes the installed binary's path into the managed engine settings
/// (`llama setup --set-binary`). One of the two CLI paths that write **user**
/// data, so it takes the same precautions as `--enable-python` — see
/// [`open_config_for_cli_write`] — and reports what it wrote.
fn set_engine_binary(paths: &Paths, binary: &Path, loc: &Locale) -> anyhow::Result<Vec<String>> {
    let ctx = "cli.ctx.set_binary";
    let (store, mut config) = open_config_for_cli_write(paths, loc, ctx)?;
    let targets = features::llama_setup::set_engine_binary(&mut config, binary);
    store
        .save_config(&config)
        .with_context(|| loc.t(ctx).to_string())?;
    Ok(features::llama_setup::render_binary_targets(
        &targets, binary, loc,
    ))
}

/// Opens `settings.json` for a CLI write. The two precautions the TUI takes
/// before touching user data, and the reason both CLI writers go through here:
///
///  * [`features::data_migration::run`] first — the ADR 0006 downgrade guard.
///    Without it a `settings.json` from a newer version would be read leniently
///    and rewritten, silently dropping the fields this build doesn't know about;
///  * a config created from scratch here seeds `interface.language` from
///    `defaults.json`, mirroring `run_tui`: the seeding is gated on the file's
///    *absence*, so creating one without it would lose the language the
///    installer just asked the user for.
fn open_config_for_cli_write(
    paths: &Paths,
    loc: &Locale,
    ctx_key: &str,
) -> anyhow::Result<(JsonStore, AppConfig)> {
    features::data_migration::run(paths, loc)?;
    let store = JsonStore::new(paths.clone());
    let fresh = !paths.settings_file().exists();
    let mut config = store
        .load_config()
        .with_context(|| loc.t(ctx_key).to_string())?;
    if fresh {
        config.interface.language = paths.default_language();
    }
    Ok((store, config))
}

/// CLI: the llama.cpp builds already downloaded (`mindfork llama installed`).
fn run_llama_installed(paths: &Paths, loc: &Locale) {
    let root = paths.llama_dir();
    let found = features::llama_setup::installed(&root);
    for line in features::llama_setup::render_installed(&found, &root, loc) {
        println!("{line}");
    }
}

/// Turns `tools.python_enabled` on in `settings.json` (`sandbox setup --enable-python`,
/// which the Windows installer's checkbox passes). This is the only CLI path that writes
/// **user** data, so it takes the same two precautions the TUI does before touching it:
///
///  * [`features::data_migration::run`] first — the downgrade guard. Without it a
///    `settings.json` from a newer version would be read leniently and rewritten,
///    silently dropping the fields this build doesn't know about;
///  * a config created from scratch here seeds `interface.language` from
///    `defaults.json`, mirroring `run_tui`: that seeding is gated on the file's
///    *absence*, so creating one without it would lose the language the installer
///    just asked the user for.
fn enable_python_tool(paths: &Paths, loc: &Locale) -> anyhow::Result<()> {
    let fresh = !paths.settings_file().exists();
    let (store, mut config) = open_config_for_cli_write(paths, loc, "cli.ctx.enable_python")?;
    if config.tools.python_enabled && !fresh {
        return Ok(()); // already on — don't rewrite the user's file for nothing
    }
    config.tools.python_enabled = true;
    store
        .save_config(&config)
        .with_context(|| loc.t("cli.ctx.enable_python").to_string())
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
/// - `MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_MMPROJ` vision
///   projector, `MINDFORK_NGL`, `MINDFORK_CTX`, `MINDFORK_PORT`) — a managed
///   `llama-server`;
/// - `MINDFORK_EMBED_URL` / `MINDFORK_EMBED_BIN` (+ `MINDFORK_EMBED_MODEL`,
///   `MINDFORK_EMBED_PORT`) — the embedding server.
fn apply_env_overrides(config: &mut AppConfig) {
    apply_engine_env(config);
    apply_embed_env(config);
}

/// The chat-server half of [`apply_env_overrides`].
fn apply_engine_env(config: &mut AppConfig) {
    if let Ok(url) = std::env::var("MINDFORK_ENGINE_URL") {
        config.engine.mode = ServerMode::External;
        config.engine.external.url = Some(url);
    } else if let Ok(bin) = std::env::var("MINDFORK_LLAMA_BIN") {
        config.engine.mode = ServerMode::Managed;
        config.engine.managed.binary = Some(bin);
        if let Ok(m) = std::env::var("MINDFORK_MODEL") {
            config.engine.managed.model_path = Some(m);
        }
        // The vision projector that ships next to the model's GGUF (spec §9.10):
        // without it the managed server is text-only and says so on `/props`.
        if let Ok(p) = std::env::var("MINDFORK_MMPROJ") {
            config.engine.managed.mmproj = Some(p);
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
}

/// The embedding-server half of [`apply_env_overrides`].
fn apply_embed_env(config: &mut AppConfig) {
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

    /// `sandbox setup --enable-python` on a data root that has never been used:
    /// settings.json is created with the tool on — and, critically, with the
    /// interface language seeded from defaults.json. `run_tui` only seeds it when
    /// the file is **absent**, so creating one here without seeding would silently
    /// discard the language the installer just asked the user for.
    #[test]
    fn enable_python_on_a_fresh_root_keeps_the_installer_language() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path()).with_default_language(Lang::En);
        assert!(!paths.settings_file().exists());

        enable_python_tool(&paths, i18n::locale(Lang::En)).unwrap();

        let cfg = JsonStore::new(paths.clone()).load_config().unwrap();
        assert!(cfg.tools.python_enabled, "the tool was not enabled");
        assert_eq!(
            cfg.interface.language,
            Lang::En,
            "the installer's language choice was lost by creating settings.json"
        );
    }

    /// An existing config is edited, not replaced: unrelated settings survive.
    #[test]
    fn enable_python_preserves_an_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        let store = JsonStore::new(paths.clone());
        let mut cfg = AppConfig::default();
        cfg.interface.language = Lang::En;
        cfg.max_tool_rounds = 7;
        store.save_config(&cfg).unwrap();

        enable_python_tool(&paths, i18n::locale(Lang::Ru)).unwrap();

        let after = store.load_config().unwrap();
        assert!(after.tools.python_enabled);
        assert_eq!(after.max_tool_rounds, 7, "an unrelated setting was reset");
        assert_eq!(
            after.interface.language,
            Lang::En,
            "an existing language was overwritten by the defaults.json seeding"
        );
    }

    /// Already on → the user's file is not rewritten at all. Asserted against a
    /// hand-written minimal config, because re-saving a config loaded from a file
    /// this program wrote reproduces it byte for byte — so a full `AppConfig`
    /// fixture could not tell a rewrite from a no-op.
    #[test]
    fn enable_python_is_a_no_op_when_already_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        // At the current schema: an older file is legitimately rewritten by the
        // migration that runs first, and that is not the rewrite this test is about.
        let minimal = format!(
            r#"{{"schema_version":{},"tools":{{"python_enabled":true}}}}"#,
            crate::shared::config::SCHEMA_VERSION
        )
        .into_bytes();
        std::fs::write(paths.settings_file(), &minimal).unwrap();

        enable_python_tool(&paths, i18n::locale(Lang::Ru)).unwrap();

        assert_eq!(
            std::fs::read(paths.settings_file()).unwrap(),
            minimal,
            "settings.json was rewritten (and a hand-edited file expanded) for nothing"
        );
    }

    /// A corrupt settings.json is refused, not replaced with defaults.
    #[test]
    fn enable_python_refuses_a_corrupt_config_without_touching_it() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        std::fs::write(paths.settings_file(), b"{ this is not json").unwrap();

        assert!(enable_python_tool(&paths, i18n::locale(Lang::Ru)).is_err());
        assert_eq!(
            std::fs::read(paths.settings_file()).unwrap(),
            b"{ this is not json",
            "a corrupt config was overwritten instead of being reported"
        );
    }

    /// A settings.json written by a **newer** version is refused rather than
    /// rewritten. This is what the `data_migration::run` call in `enable_python_tool`
    /// is for, and the case a corrupt-file test cannot cover: such a file is valid
    /// JSON, so it would be read leniently and saved back **without the fields this
    /// build does not know about** (ADR 0006 F10 — the downgrade guard).
    #[test]
    fn enable_python_refuses_a_config_from_a_newer_version() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path());
        let from_the_future = format!(
            r#"{{"schema_version":{},"tools":{{"python_enabled":false}},"a_setting_we_do_not_know":42}}"#,
            crate::shared::storage::schema::SETTINGS_SCHEMA + 1
        );
        std::fs::write(paths.settings_file(), &from_the_future).unwrap();

        assert!(enable_python_tool(&paths, i18n::locale(Lang::Ru)).is_err());
        assert_eq!(
            std::fs::read_to_string(paths.settings_file()).unwrap(),
            from_the_future,
            "a newer config was rewritten, dropping the fields this build cannot see"
        );
    }

    /// Precedence for the run's one effective password: the argument wins over
    /// the stored setting, and an empty value means "no password" from either
    /// source (docs/history/backup-password.md §4 F8).
    /// The contract is an order of calls in `real_main`, so it is tested there
    /// (docs/lessons.md §2): a summary of a data root that does not exist must
    /// not bring it into existence, which is what `ensure_dirs` a few lines
    /// further down would do.
    #[test]
    fn stats_is_answered_before_the_data_root_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(tmp.path().join("never-created"));
        let command = CliCommand::Stats {
            archive: None,
            password: None,
            json: true,
        };
        real_main(command, &paths, i18n::locale(Lang::En), &[]).unwrap();
        assert!(!paths.root().exists());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    /// "Wrong password" is only the stored password's fault when nobody typed
    /// one: not with `--password`, not on a retry, and not when there was no
    /// password to try at all.
    #[test]
    fn only_an_untyped_first_attempt_blames_the_stored_password() {
        use crate::features::backup::ArchivePassword::{Required, Wrong};
        assert!(stored_password_failed(true, 0, Wrong));
        assert!(
            !stored_password_failed(false, 0, Wrong),
            "--password was given"
        );
        assert!(
            !stored_password_failed(true, 1, Wrong),
            "typed at the prompt"
        );
        assert!(
            !stored_password_failed(true, 0, Required),
            "nothing was tried"
        );
    }

    #[test]
    fn effective_password_prefers_the_argument_then_the_setting() {
        let arg = || Some("from-arg".to_string());
        let stored = || Some("from-settings".to_string());
        assert_eq!(
            effective_password(arg(), stored()).as_deref(),
            Some("from-arg")
        );
        assert_eq!(
            effective_password(None, stored()).as_deref(),
            Some("from-settings")
        );
        assert_eq!(effective_password(arg(), None).as_deref(), Some("from-arg"));
        assert_eq!(effective_password(None, None), None);
        // An empty password is "no password", not an empty-string key — from
        // either source, so clearing the setting really returns to plain archives.
        assert_eq!(effective_password(Some(String::new()), None), None);
        assert_eq!(effective_password(None, Some(String::new())), None);
        // An explicitly empty argument overrides a stored password: that is how
        // one makes a deliberately unencrypted copy without clearing the setting.
        assert_eq!(effective_password(Some(String::new()), stored()), None);
    }

    /// Only the two full-screen commands need a terminal, and only a missing one
    /// refuses them; every other command prints lines and works redirected.
    #[test]
    fn only_a_full_screen_launch_without_a_terminal_is_refused() {
        for command in [CliCommand::Run, CliCommand::Demo] {
            assert!(refuses_tui_launch(&command, false), "{command:?}");
            assert!(!refuses_tui_launch(&command, true), "{command:?}");
        }
        for command in [
            CliCommand::LlamaInstalled,
            CliCommand::Import {
                file: PathBuf::from("chats.json"),
            },
        ] {
            assert!(!refuses_tui_launch(&command, false), "{command:?}");
        }
    }

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
