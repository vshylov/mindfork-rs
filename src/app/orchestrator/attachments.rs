//! Chat file attachments (`/file attach|remove|list`, docs/file-attachments.md).
//!
//! Reading and extracting text runs in a **background task** (a large PDF must
//! not block the orchestrator's command loop) and comes back through an internal
//! channel; the orchestrator — the sole owner of `Chat` — decides the mode
//! against the budget and inserts the attachment. Removing and listing are pure
//! in-memory operations, done in place.
//!
//! Where the text then goes: `request::inject_attachments` puts it into the
//! request's system prompt on every turn (spec §9.7).

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::attachment::{AttachMode, Attachment, inline_tokens, prompt_tokens};
use crate::features::file_command::{FileProgress, resolve_target};

use super::Orchestrator;

/// Hard ceiling on the size of a file we will even read (before extraction).
/// Attachments live in memory and in the chat file; anything of this order is a
/// job for `/rag add`, not for the prompt.
const MAX_ATTACH_BYTES: u64 = 32 * 1024 * 1024;

/// A file read and extracted by the background task (internal channel).
pub(super) struct AttachResult {
    /// The chat the command was issued in (it may no longer be active when the
    /// task finishes — the attachment still belongs to it).
    pub(super) chat_id: Uuid,
    pub(super) outcome: Result<ExtractedFile, String>,
}

/// Text successfully extracted from a file (the mode is decided by the
/// orchestrator, which knows the chat's budget).
#[derive(Debug)]
pub(super) struct ExtractedFile {
    pub(super) name: String,
    pub(super) source: String,
    pub(super) text: String,
    pub(super) bytes: usize,
}

impl Orchestrator {
    /// Starts attaching a file to the active chat (`/file attach <path>`).
    /// Reading/extraction happen in a background task; the result arrives as
    /// [`AttachResult`].
    pub(super) fn handle_file_attach(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(chat_id) = self.active_id else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return;
        };
        let loc = self.ui_locale();
        let tx = self.attach_tx.clone();
        tokio::task::spawn_blocking(move || {
            let outcome = extract_file(std::path::Path::new(&path), loc);
            let _ = tx.send(AttachResult { chat_id, outcome });
        });
    }

    /// Applies the result of a background read: decides the mode against the
    /// budget, replaces a previous attachment of the same file, and reports.
    pub(super) fn handle_attach_result(&mut self, res: AttachResult) {
        let file = match res.outcome {
            Ok(f) => f,
            Err(err) => {
                self.fail_file(&err);
                return;
            }
        };
        let cfg = self.config.attachments;
        let Some(chat) = self.chat_mut(res.chat_id) else {
            return; // the chat is gone (deleted while reading)
        };
        // Re-attaching the same file replaces the previous snapshot (idempotent,
        // like re-adding a source to RAG) — and frees its budget first.
        chat.attachments.retain(|a| a.source != file.source);

        let est = crate::shared::tokens::estimate_text(&file.text) as usize;
        let used = inline_tokens(&chat.attachments);
        // Over the per-file budget, or over what's left of the chat's total →
        // by reference. Attaching never fails on size (docs/file-attachments.md §4.2).
        let mode = if est <= cfg.max_file_tokens && used + est <= cfg.max_total_tokens {
            AttachMode::Inline
        } else {
            AttachMode::ByReference
        };
        let attachment = Attachment::new(file.name, file.source, file.text, file.bytes, mode);
        let info = attachment.info(cfg.excerpt_tokens);
        chat.attachments.push(attachment);
        // What the chat's attachments now cost per request — inline text in full
        // plus by-reference excerpts (which are NOT free, see `prompt_tokens`).
        let total = prompt_tokens(&chat.attachments, cfg.excerpt_tokens);

        self.mark_dirty(res.chat_id);
        self.emit_file_progress(FileProgress::Attached {
            info,
            total_tokens: total,
        });
        self.emit_attachments();
    }

    /// Removes an attachment by name/path/`#N` (`/file remove <target>`).
    pub(super) fn handle_file_remove(&mut self, target: String) {
        let Some(chat_id) = self.active_id else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return;
        };
        let Some(chat) = self.chat_mut(chat_id) else {
            return;
        };
        let Some(idx) = resolve_target(&chat.attachments, &target) else {
            let msg = self
                .ui_locale()
                .tf("ui.err.file_not_attached", &[("target", target.trim())]);
            self.fail_file(&msg);
            return;
        };
        let removed = chat.attachments.remove(idx);
        self.mark_dirty(chat_id);
        self.emit_file_progress(FileProgress::Removed { name: removed.name });
        self.emit_attachments();
    }

    /// Lists the active chat's attachments (`/file list`).
    pub(super) fn handle_file_list(&mut self) {
        let Some(chat) = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
        else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return;
        };
        let excerpt = self.config.attachments.excerpt_tokens;
        let items = chat.attachments.iter().map(|a| a.info(excerpt)).collect();
        self.emit_file_progress(FileProgress::Listed { items });
    }

    /// Sends the active chat's attachment cards to the UI (the status-bar chip).
    /// Emitted on chat activation and after every attach/remove — the same
    /// pattern as `CharacterNames`.
    pub(super) fn emit_attachments(&self) {
        let excerpt = self.config.attachments.excerpt_tokens;
        let items = self
            .active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.attachments.iter().map(|a| a.info(excerpt)).collect())
            .unwrap_or_default();
        let _ = self.evt_tx.send(AppEvent::Attachments(items));
    }

    fn emit_file_progress(&self, progress: FileProgress) {
        let _ = self.evt_tx.send(AppEvent::FileProgress(progress));
    }

    fn fail_file(&self, msg: &str) {
        self.emit_file_progress(FileProgress::Failed(msg.to_string()));
    }
}

/// Reads a file and extracts plain text from it (blocking — runs on the blocking
/// pool). Formats: any valid UTF-8 as-is, plus the html/pdf/docx extractors by
/// extension (fork F8) — reusing [`super::rag::read_source_text`]. Errors are
/// already localized (axis B): they go straight into a feed note.
fn extract_file(
    path: &std::path::Path,
    loc: &'static crate::shared::i18n::Locale,
) -> Result<ExtractedFile, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| loc.tf("ui.err.file_unavailable", &[("err", &e.to_string())]))?;
    if !meta.is_file() {
        return Err(loc.t("ui.err.file_not_a_file").to_string());
    }
    if meta.len() > MAX_ATTACH_BYTES {
        return Err(loc.tf(
            "ui.err.file_too_big",
            &[
                (
                    "size",
                    &crate::entities::attachment::format_bytes(meta.len() as usize),
                ),
                (
                    "max",
                    &crate::entities::attachment::format_bytes(MAX_ATTACH_BYTES as usize),
                ),
            ],
        ));
    }
    // Undecodable content (a binary) fails here with a clear message — the
    // honest boundary of "text files" (fork F8).
    let text = super::rag::read_source_text(path)
        .map_err(|e| loc.tf("ui.err.file_unreadable", &[("err", &e.to_string())]))?;
    if text.trim().is_empty() {
        return Err(loc.t("ui.err.file_empty").to_string());
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Ok(ExtractedFile {
        name,
        source: crate::features::rag_ingest::canonical_source(path),
        text,
        bytes: meta.len() as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn extracts_utf8_text_and_reports_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "# Заголовок\n\nтекст заметки").unwrap();
        let file = extract_file(&path, ru()).unwrap();
        assert_eq!(file.name, "notes.md");
        assert!(file.text.contains("текст заметки"));
        assert_eq!(file.bytes, std::fs::metadata(&path).unwrap().len() as usize);
        // The source key is canonical (matches how RAG stores sources).
        assert!(file.source.ends_with("notes.md"));
    }

    #[test]
    fn source_files_are_attachable_not_just_the_rag_allowlist() {
        // Fork F8: an extension outside RAG's txt/md/html/pdf/docx list is fine
        // as long as it decodes as UTF-8 — source files are the main use case.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn main() { println!(\"hi\"); }").unwrap();
        let file = extract_file(&path, ru()).unwrap();
        assert!(file.text.contains("fn main()"));
    }

    #[test]
    fn binary_and_missing_and_empty_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();

        // Invalid UTF-8 → a clear refusal, not a panic or mojibake.
        let bin = dir.path().join("blob.bin");
        std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x01, 0x80]).unwrap();
        assert!(extract_file(&bin, ru()).is_err());

        // A missing path.
        assert!(extract_file(&dir.path().join("nope.txt"), ru()).is_err());

        // A directory is not a file.
        assert!(extract_file(dir.path(), ru()).is_err());

        // Whitespace-only content carries nothing for the model.
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, "   \n\t ").unwrap();
        assert!(extract_file(&empty, ru()).is_err());
    }

    #[test]
    fn extraction_errors_are_localized_for_all_langs() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let err = extract_file(&missing, loc).unwrap_err();
            assert!(!err.contains('{') && !err.contains('}'), "{lang:?}: {err}");
            if lang == crate::shared::i18n::Lang::En {
                assert!(
                    !err.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                    "Cyrillic leaked into the en message: {err}"
                );
            }
        }
    }
}
