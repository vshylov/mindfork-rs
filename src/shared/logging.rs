//! Logging to a file (stdout is taken by the TUI). See spec §2.2 and plan M0.
//!
//! Logs are written to `logs/mindfork.log` next to the binary, with daily
//! rotation. The level is set via the `MINDFORK_LOG` env variable (format:
//! `tracing_subscriber::EnvFilter`, e.g. `mindfork=debug` — the filter's target
//! is the crate name, which is also the package name).

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;

/// Holder of the background log-writing thread.
///
/// Must live for the process's whole run, otherwise the log tail may be lost.
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Initializes the logging subsystem. `loc` — the interface language for
/// user-facing error text (creating the log directory).
pub fn init(paths: &Paths, loc: &Locale) -> Result<LogGuard> {
    std::fs::create_dir_all(paths.log_dir()).with_context(|| {
        loc.tf(
            "cli.ctx.log_dir",
            &[("path", &paths.log_dir().display().to_string())],
        )
    })?;

    let file_appender = tracing_appender::rolling::daily(paths.log_dir(), "mindfork.log");
    let (non_blocking, worker) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("MINDFORK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    Ok(LogGuard { _worker: worker })
}
