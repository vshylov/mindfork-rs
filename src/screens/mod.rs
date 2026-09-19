//! The `screens` layer (FSD, analogous to "pages"): whole screens (chat, chat_list,
//! settings). See spec §4.2, §11.1.
//!
//! `settings.rs` — the settings screen (M8); `chat_list.rs` — the fullscreen chat
//! list (opened from the chat via `Esc`).

pub mod awaited_chat;
pub mod changes;
pub mod chat;
pub mod chat_list;
pub mod search;
pub mod self_model;
pub mod settings;
pub mod tasks;
