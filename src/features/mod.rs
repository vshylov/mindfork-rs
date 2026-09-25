//! The `features` layer (FSD): user scenarios as pure logic (chat_search_sort,
//! rename_chat, chat_export, backup, compaction, spellcheck, tools, the typed
//! routes in ui_command, …). Send, regenerate and take-back are orchestrator
//! handlers (`app/orchestrator/generation.rs`), since the orchestrator alone
//! owns `Chat`. See spec §4.2.
//!
//! Filled in starting at M3. Feature logic — pure functions, testable without a UI.

pub mod backup;
pub mod chat_export;
pub mod chat_files;
pub mod chat_inputs;
pub mod chat_links;
pub mod chat_search;
pub mod chat_search_sort;
pub mod cli;
pub mod compact_command;
pub mod compaction;
pub mod data_migration;
pub mod data_stats;
pub mod demo;
pub mod doc_extract;
pub mod exit_command;
pub mod export_command;
pub mod file_command;
pub mod image_command;
pub mod image_fetch;
pub mod image_prepare;
pub mod impersonation_command;
pub mod import;
pub mod llama_setup;
pub mod mcp_import;
pub mod profile_command;
pub mod profiles;
pub mod project_command;
pub mod provision;
pub mod rag_command;
pub mod rag_ingest;
pub mod reindex_command;
pub mod rename_chat;
pub mod sandbox_setup;
pub mod slash;
pub mod spellcheck;
pub mod terminal_input;
pub mod tools;
pub mod tts_command;
pub mod ui_command;
pub mod workspace_diff;
pub mod workspace_journal;
