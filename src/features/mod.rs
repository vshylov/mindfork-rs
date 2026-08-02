//! The `features` layer (FSD): user scenarios (send_message, regenerate,
//! delete_last, edit_message, chat_search_sort, spellcheck, tools, …).
//! See spec §4.2.
//!
//! Filled in starting at M3. Feature logic — pure functions, testable without a UI.

pub mod backup;
pub mod chat_export;
pub mod chat_search;
pub mod chat_search_sort;
pub mod cli;
pub mod data_migration;
pub mod doc_extract;
pub mod file_command;
pub mod import;
pub mod mcp_import;
pub mod password_prompt;
pub mod profiles;
pub mod rag_command;
pub mod rag_ingest;
pub mod reindex_command;
pub mod rename_chat;
pub mod sandbox_setup;
pub mod spellcheck;
pub mod tools;
pub mod tts_command;
