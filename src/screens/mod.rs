//! Слой `screens` (FSD, аналог "pages"): целостные экраны (chat, chat_list,
//! settings). См. spec §4.2, §11.1.
//!
//! `settings.rs` — экран настроек (M8); `chat_list.rs` — полноэкранный список
//! чатов (открывается из чата по `Esc`).

pub mod chat;
pub mod chat_list;
pub mod settings;
