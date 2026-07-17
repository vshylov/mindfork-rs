//! Слой `shared` (FSD): инфраструктура и утилиты, не зависящие от верхних слоёв.
//! См. spec §4.2.

pub mod api;
pub mod config;
pub mod i18n;
pub mod instance;
pub mod keys;
pub mod logging;
pub mod markdown;
/// Мини-клиент MCP — **зонд** направления «плагины» (docs/research/plugin-system.md
/// §7, этап 2). Пока только в тест-сборках (юнит-тесты протокола + живой смоук);
/// в бинарь войдёт на этапе 3 (`feat/mcp-host`) после GO.
#[cfg(test)]
pub mod mcp;
pub mod paths;
pub mod sandbox;
pub mod server;
pub mod storage;
pub mod theme;
pub mod tokens;
pub mod ui;
pub mod wrap;
