//! mindfork-rs — консольное (TUI) приложение ИИ-чата.
//! Точка входа: single-instance → логирование → запуск TUI-петли.
//! См. spec §4.2 и plan M0.

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

use anyhow::Context;

use crate::shared::{instance, logging, paths::Paths};

fn main() -> anyhow::Result<()> {
    let paths = Paths::discover().context("resolving data paths")?;

    // Блокировка единственного экземпляра удерживается до конца работы процесса.
    let _instance = instance::acquire().context("single-instance check")?;

    // Логгер удерживается до конца работы процесса (фоновый писатель).
    let _logging = logging::init(&paths).context("initializing logging")?;

    tracing::info!(root = %paths.root().display(), "mindfork-rs starting");

    let result = app::run();

    match &result {
        Ok(()) => tracing::info!("mindfork-rs exited cleanly"),
        Err(err) => tracing::error!(error = %err, "mindfork-rs exited with error"),
    }

    result
}
