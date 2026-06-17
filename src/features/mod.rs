//! Слой `features` (FSD): пользовательские сценарии (send_message, regenerate,
//! delete_last, edit_message, chat_search_sort, spellcheck, tools, …).
//! См. spec §4.2.
//!
//! Наполняется на M3+. Логика фич — чистые, тестируемые без UI функции.

pub mod chat_export;
pub mod chat_search_sort;
pub mod migration;
pub mod profiles;
pub mod rag_command;
pub mod rag_ingest;
pub mod rename_chat;
pub mod spellcheck;
pub mod tools;
