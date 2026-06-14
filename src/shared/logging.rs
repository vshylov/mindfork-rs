//! Логирование в файл (stdout занят TUI). См. spec §2.2 и plan M0.
//!
//! Логи пишутся в `logs/mindfork.log` рядом с бинарником, с суточной ротацией.
//! Уровень настраивается переменной окружения `MINDFORK_LOG`
//! (формат `tracing_subscriber::EnvFilter`, например `mindfork_rs=debug`).

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::shared::paths::Paths;

/// Держатель фонового потока записи логов.
///
/// Должен жить весь срок работы процесса, иначе хвост логов может потеряться.
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Инициализирует подсистему логирования.
pub fn init(paths: &Paths) -> Result<LogGuard> {
    std::fs::create_dir_all(paths.log_dir())
        .with_context(|| format!("creating log dir {}", paths.log_dir().display()))?;

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
