//! Слой `shared` (FSD): инфраструктура и утилиты, не зависящие от верхних слоёв.
//! См. spec §4.2.

pub mod api;
pub mod config;
pub mod i18n;
pub mod instance;
pub mod keys;
pub mod logging;
pub mod markdown;
pub mod paths;
pub mod sandbox;
pub mod server;
pub mod storage;
pub mod theme;
pub mod tokens;
pub mod ui;
pub mod wrap;
