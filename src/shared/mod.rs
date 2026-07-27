//! `shared` layer (FSD): infrastructure and utilities, independent of the
//! upper layers. See spec §4.2.

pub mod api;
pub mod config;
pub mod credits;
pub mod embed_identity;
pub mod i18n;
pub mod instance;
pub mod keys;
pub mod logging;
pub mod markdown;
pub mod mcp;
pub mod paths;
pub mod sandbox;
pub mod secrets;
pub mod server;
pub mod storage;
pub mod theme;
pub mod tokens;
pub mod tts;
pub mod ui;
pub mod wrap;
