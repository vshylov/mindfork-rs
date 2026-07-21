//! Слой `features` (FSD): пользовательские сценарии (send_message, regenerate,
//! delete_last, edit_message, chat_search_sort, spellcheck, tools, …).
//! См. spec §4.2.
//!
//! Наполняется на M3+. Логика фич — чистые, тестируемые без UI функции.

pub mod backup;
pub mod chat_export;
pub mod chat_search_sort;
pub mod cli;
pub mod data_migration;
pub mod doc_extract;
pub mod import;
pub mod profiles;
pub mod rag_command;
pub mod rag_ingest;
pub mod rename_chat;
pub mod sandbox_setup;
pub mod spellcheck;
pub mod tools;
pub mod tts_command;
