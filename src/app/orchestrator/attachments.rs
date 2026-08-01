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
//!
//! A file attached **by reference** is additionally indexed into the chat-scoped
//! semantic index (stage 3): another background task chunks and embeds it, so
//! `attachment_search` can find the right place by meaning instead of walking
//! pages. Indexing is **best-effort** — with no embedder configured it is simply
//! skipped and everything else keeps working (the ADR 0002 degradation pattern).

use std::sync::Arc;

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::attachment::{
    AttachMode, Attachment, AttachmentChunk, decide_mode, inline_tokens_excluding, prompt_tokens,
};
use crate::features::file_command::{FileProgress, resolve_target};
use crate::features::tools::rag::ChunkParams;
use crate::shared::api::{EmbedRole, Embedder};
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;

use super::Orchestrator;
use super::rag::{EMBED_BATCH_CHUNKS, is_markdown_source};

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
        let est = crate::shared::tokens::estimate_text(&file.text) as usize;
        // What is spent inline *excluding* a previous copy of this same file:
        // re-attaching replaces it (the dedupe happens in `insert_attachment`),
        // so counting the old copy would push the new one by reference for no
        // reason.
        let used = inline_tokens_excluding(&chat.attachments, &file.source);
        // Over the per-file budget, or over what's left of the chat's total →
        // by reference. Attaching never fails on size (docs/file-attachments.md §4.2).
        let mode = decide_mode(est, used, &cfg);
        let attachment = Attachment::new(file.name, file.source, file.text, file.bytes, mode);
        self.insert_attachment(res.chat_id, attachment);
    }

    /// Puts an **already-built** attachment into a chat and does everything that
    /// follows: dedupe by source, persistence, index prune, the feed note, the
    /// status chip, and background indexing.
    ///
    /// Two entry points share it: `/file attach` above, and a tool that produced
    /// an attachment of its own and returned it as a `ChatEffect::AddAttachment`
    /// (a video transcript — spec §9.9, docs/youtube-transcript.md §3 F1). The
    /// mode is **not** re-decided here: the tool already told the model what it
    /// did, and the object described and the object stored have to be the same
    /// one — down to the `id`, which is the key the index is written under.
    pub(super) fn insert_attachment(&mut self, chat_id: Uuid, attachment: Attachment) {
        let cfg = self.config.attachments;
        let Some(chat) = self.chat_mut(chat_id) else {
            return; // the chat is gone (deleted while the tool was running)
        };
        // Re-attaching the same source replaces the previous snapshot
        // (idempotent, like re-adding a source to RAG).
        chat.attachments.retain(|a| a.source != attachment.source);

        let info = attachment.info(cfg.excerpt_tokens);
        // Only a by-reference file is indexed (fork F13): an inline one is
        // already in the prompt in full, so search would return duplicates of
        // what the model can see anyway.
        let index = (attachment.mode == AttachMode::ByReference).then(|| AttachIndex {
            chat_id,
            attachment_id: attachment.id,
            name: attachment.name.clone(),
            source: attachment.source.clone(),
            text: attachment.text.clone(),
        });
        chat.attachments.push(attachment);
        // What the chat's attachments now cost per request — inline text in full
        // plus by-reference excerpts (which are NOT free, see `prompt_tokens`).
        let total = prompt_tokens(&chat.attachments, cfg.excerpt_tokens);
        let keep: Vec<Uuid> = chat.attachments.iter().map(|a| a.id).collect();

        self.mark_dirty(chat_id);
        self.prune_attachment_index(chat_id, &keep);
        self.emit_file_progress(FileProgress::Attached {
            info,
            total_tokens: total,
        });
        self.emit_attachments();
        if let Some(task) = index {
            self.spawn_attachment_index(task);
        }
    }

    /// Starts background indexing of a by-reference attachment. Fire-and-forget:
    /// the task is bounded (one file), and correctness against a removal that
    /// races it is enforced where it matters — `attachment_search` only shows
    /// hits for files still in the turn's snapshot, and
    /// [`Self::prune_attachment_index`] collects the leftovers.
    fn spawn_attachment_index(&self, task: AttachIndex) {
        spawn_attachment_index(AttachIndexTask {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            params: ChunkParams::from_settings(&self.config.rag),
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
            index: task,
        });
    }

    /// Drops index chunks of files that are no longer attached to the chat.
    /// Errors are logged, not surfaced: the index is derived data and the user's
    /// command already succeeded.
    fn prune_attachment_index(&self, chat_id: Uuid, keep: &[Uuid]) {
        if let Err(err) = self.storage.db().attachment_prune(chat_id, keep) {
            tracing::warn!(error = %err, "attachments: failed to prune the search index");
        }
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
        let keep: Vec<Uuid> = chat.attachments.iter().map(|a| a.id).collect();
        self.mark_dirty(chat_id);
        // The removed file's index chunks go with it — otherwise the model could
        // still find fragments of a file the user took out of the conversation.
        self.prune_attachment_index(chat_id, &keep);
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

/// What to index: the attachment's identity and its text snapshot.
struct AttachIndex {
    chat_id: Uuid,
    attachment_id: Uuid,
    name: String,
    /// The canonical path — only to pick the chunker (markdown by headings).
    source: String,
    text: String,
}

/// Parameters of the background attachment-indexing task.
struct AttachIndexTask {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    params: ChunkParams,
    /// The interface language (axis B) — the progress/outcome notes are the user's.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
    index: AttachIndex,
}

/// Chunks, embeds, and writes one attachment into the chat-scoped index. Reuses
/// RAG's chunking (`chunk_text`/`chunk_markdown`) and its sub-batched embedding,
/// so progress moves while a large file is being embedded.
///
/// **Graceful degradation** (ADR 0002): if the embedder isn't configured or the
/// dimensionality doesn't match the DB, indexing is skipped with a clear note —
/// the pinned block and `attachment_read` keep working in full. The feature never
/// *depends* on RAG being set up.
fn spawn_attachment_index(task: AttachIndexTask) {
    let AttachIndexTask {
        embedder,
        storage,
        params,
        loc,
        evt_tx,
        index,
    } = task;

    tokio::spawn(async move {
        let send = |p: FileProgress| {
            let _ = evt_tx.send(AppEvent::FileProgress(p));
        };
        let skip = |reason: String| {
            let _ = evt_tx.send(AppEvent::FileProgress(FileProgress::IndexSkipped {
                name: index.name.clone(),
                reason,
            }));
        };

        let chunks = if is_markdown_source(&index.source) {
            crate::features::tools::rag::chunk_markdown(&index.text, params)
        } else {
            crate::features::tools::rag::chunk_text(&index.text, params)
        };
        if chunks.is_empty() {
            return;
        }
        // Precheck — a fast, clear answer when there's no embedder (the common
        // case: RAG isn't configured at all).
        if embedder
            .embed(vec!["ping".into()], EmbedRole::Passage)
            .await
            .is_err()
        {
            skip(loc.t("ui.file.index_no_embedder").to_string());
            return;
        }
        // Re-indexing replaces the previous run's chunks instead of duplicating them.
        if let Err(err) = storage
            .db()
            .attachment_delete(index.chat_id, index.attachment_id)
        {
            skip(err.to_string());
            return;
        }

        let total = chunks.len();
        send(FileProgress::Indexing {
            name: index.name.clone(),
            done: 0,
            total,
        });
        let mut done = 0usize;
        for batch in chunks.chunks(EMBED_BATCH_CHUNKS) {
            let embeddings = match embedder.embed(batch.to_vec(), EmbedRole::Passage).await {
                Ok(v) if v.len() == batch.len() => v,
                Ok(_) => {
                    skip(loc.t("ui.err.rag_wrong_vector_count").to_string());
                    return;
                }
                Err(err) => {
                    skip(err.to_string());
                    return;
                }
            };
            for (chunk, embedding) in batch.iter().zip(embeddings) {
                let doc = AttachmentChunk::new(
                    index.chat_id,
                    index.attachment_id,
                    &index.name,
                    chunk,
                    embedding,
                );
                if let Err(err) = storage.db().attachment_insert(&doc) {
                    // A dimensionality mismatch with the RAG base lands here —
                    // an honest note beats a half-built index.
                    tracing::warn!(error = %err, name = %index.name, "attachments: indexing failed");
                    skip(err.to_string());
                    return;
                }
            }
            done += batch.len();
            send(FileProgress::Indexing {
                name: index.name.clone(),
                done,
                total,
            });
        }
        send(FileProgress::Indexed {
            name: index.name,
            chunks: total,
        });
    });
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
