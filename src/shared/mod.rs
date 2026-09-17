//! `shared` layer (FSD): infrastructure and utilities, independent of the
//! upper layers. See spec §4.2.

pub mod api;
pub mod child_env;
pub mod cmdline;
pub mod config;
pub mod credits;
pub mod embed_calibration;
pub mod embed_identity;
pub mod embed_prefix;
pub mod gguf;
pub mod http_text;
pub mod i18n;
pub mod instance;
pub mod keys;
pub mod logging;
pub mod markdown;
pub mod mcp;
pub mod net;
pub mod os_open;
pub mod osc11;
pub mod osc52;
pub mod paths;
pub mod proc;
pub mod sandbox;
pub mod secrets;
pub mod server;
pub mod session_budget;
#[cfg(test)]
pub mod shot;
pub mod storage;
pub mod text_decode;
pub mod theme;
pub mod title;
pub mod tokens;
pub mod tts;
pub mod ui;
pub mod video;
pub mod wrap;
