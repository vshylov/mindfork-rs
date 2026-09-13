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

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::attachment::{
    AttachMode, Attachment, AttachmentChunk, Resolved, decide_mode, inline_tokens_excluding,
    name_is_shared, prompt_tokens,
};
use crate::entities::chat_file::{ChatFile, FileOrigin};
use crate::features::file_command::{FileProgress, OpenedInstead, StoredInfo};
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
    /// What keeping the file's own bytes did (fork F8a) — done in the same background task
    /// as the read. `None` when there was nothing to keep: plain text, or a read that failed.
    pub(super) stored: Option<Result<crate::features::chat_files::Stored, String>>,
}

impl AttachResult {
    /// The background half of `/file attach`, in the order it has to happen: the read and
    /// the extraction, then — when the text is not the file — the store of the file's own
    /// bytes into the chat's folder. The store is why this half has to hold it: up to
    /// [`MAX_ATTACH_BYTES`] hashed, written and synced, which ran on the command loop while
    /// only the read was off it.
    ///
    /// `dir` and `listed` are the chat's folder and list as the command found them
    /// ([`Orchestrator::attach_snapshot`]). A store against that snapshot is safe whatever
    /// lands meanwhile: `store_as` never overwrites and moves to the next version on a taken
    /// name — a call's outputs are stored against the turn's list the same way. What the
    /// loop has to check on landing is only the listing a store answered from.
    pub(super) fn prepare(
        chat_id: Uuid,
        mut outcome: Result<ExtractedFile, String>,
        dir: &std::path::Path,
        listed: &[ChatFile],
        loc: &'static Locale,
    ) -> Self {
        let stored = match &mut outcome {
            Ok(file) => file
                .original
                .take()
                .map(|bytes| store_original(dir, listed, file, &bytes, loc)),
            Err(_) => None,
        };
        Self {
            chat_id,
            outcome,
            stored,
        }
    }
}

/// Keeps the user's own file with the chat (fork F8a): a sanitized name, versioned on a
/// collision, listed like a call's output. The same bytes under the same name are kept once,
/// so re-attaching an unchanged file moves nothing. Blocking: [`AttachResult::prepare`] runs
/// it on the blocking pool, and the listing lands on the loop
/// ([`Orchestrator::land_original`]).
///
/// The copy carries the mark the user's file carries (§13 U11): a document downloaded from
/// the web opens in Protected View from where it was saved, and must not open as a local,
/// trusted file from the chat's folder.
fn store_original(
    dir: &std::path::Path,
    listed: &[ChatFile],
    file: &ExtractedFile,
    bytes: &[u8],
    loc: &'static Locale,
) -> Result<crate::features::chat_files::Stored, String> {
    // A real file name survives sanitizing; the fallback is for what no path should
    // produce, and gives the file a name code can open rather than a refusal.
    let name =
        crate::entities::chat_file::sanitize_name(&file.name).unwrap_or_else(|| "file".to_string());
    let zone = crate::shared::os_open::zone_of(std::path::Path::new(&file.source));
    crate::features::chat_files::store_as(
        dir,
        listed,
        &name,
        bytes,
        FileOrigin::Attached,
        zone.as_deref(),
    )
    .map_err(|e| {
        loc.tf(
            "ui.err.file_store_failed",
            &[("name", &name), ("err", &e.to_string())],
        )
    })
}

/// What a launch handed to the desktop's handler came back as (internal channel, §13 U5).
pub(super) struct OpenResult {
    /// The chat the command was issued in. The launch runs off the loop and can outlast a
    /// switch of chats, so the note is addressed rather than sent to whichever feed is open
    /// when it lands ([`Orchestrator::handle_open_result`]).
    pub(super) chat_id: Uuid,
    pub(super) progress: FileProgress,
}

/// Text successfully extracted from a file (the mode is decided by the
/// orchestrator, which knows the chat's budget).
#[derive(Debug)]
pub(super) struct ExtractedFile {
    pub(super) name: String,
    pub(super) source: String,
    pub(super) text: String,
    pub(super) bytes: usize,
    /// The encoding the file was read in, when it was not UTF-8 (the feed note names it).
    pub(super) encoding: Option<&'static encoding_rs::Encoding>,
    /// The file's own bytes, kept when its text is **not** the file (fork F8a,
    /// docs/history/sandbox-file-exchange.md §12 T9): a document an extractor read, or a binary
    /// that decodes as nothing and has no text at all. `None` for plain text, which needs
    /// no second copy — the snapshot is the file.
    pub(super) original: Option<Vec<u8>>,
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
        let hint = crate::shared::text_decode::tld_hint(self.config.interface.language);
        let tx = self.attach_tx.clone();
        let (dir, listed) = self.attach_snapshot(chat_id);
        tokio::task::spawn_blocking(move || {
            let outcome = extract_file(std::path::Path::new(&path), loc, hint);
            let _ = tx.send(AttachResult::prepare(chat_id, outcome, &dir, &listed, loc));
        });
    }

    /// The chat's stored-files folder and what it lists, as a store off the loop needs them
    /// ([`AttachResult::prepare`]).
    pub(super) fn attach_snapshot(&self, chat_id: Uuid) -> (std::path::PathBuf, Vec<ChatFile>) {
        let listed = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .map(|c| c.files.clone())
            .unwrap_or_default();
        (self.stored_files_dir(chat_id), listed)
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
        // The user's own bytes, when the text is not the file (fork F8a): already kept by
        // the background task, and listed **first** here, so an attachment that links them
        // never names a file that is not on disk.
        let stored = match res.stored {
            Some(Ok(stored)) => match self.land_original(res.chat_id, stored) {
                Some(listed) => Some(listed),
                None => {
                    let msg = self.ui_locale().tf(
                        "ui.err.file_removed_while_attaching",
                        &[("name", &file.name)],
                    );
                    self.fail_file(&msg);
                    return;
                }
            },
            Some(Err(msg)) => {
                self.fail_file(&msg);
                return;
            }
            None => None,
        };
        // A binary decodes as no text at all, so there is no attachment to make: the chat
        // keeps the file and the note says what reads it (docs/lessons.md §4). This is the
        // refusal D3 asked to lift — a workbook now reaches the code.
        if file.text.trim().is_empty() {
            if let Some(stored) = stored {
                let dir = self.stored_files_dir(res.chat_id).display().to_string();
                self.emit_file_progress(FileProgress::StoredFile {
                    name: stored.name,
                    bytes: stored.bytes,
                    mime: stored.mime,
                    dir,
                });
            }
            return;
        }
        let cfg = self.config.attachments;
        // What the attachment being replaced kept, if anything: dropped only **after** the
        // new one is listed, so no state exists in which neither is there (§12 T9).
        let previous = self
            .chats
            .iter()
            .find(|c| c.id == res.chat_id)
            .and_then(|c| c.attachments.iter().find(|a| a.source == file.source))
            .and_then(|a| a.file_id);
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
        let read_as = file.encoding.map(encoding_rs::Encoding::name);
        let mut attachment = Attachment::new(file.name, file.source, file.text, file.bytes, mode);
        if let Some(stored) = &stored {
            attachment = attachment.with_file(stored.id);
        }
        self.insert_attachment(res.chat_id, attachment, read_as);
        // The copy the replaced attachment kept — unless the new one is that very file,
        // which `store` reports as unchanged and nothing has to move.
        if let Some(old) = previous.filter(|old| stored.as_ref().is_none_or(|s| s.id != *old)) {
            self.drop_original(res.chat_id, old);
        }
    }

    /// Lands what the background task stored (fork F8a) — on the loop, which owns the list.
    /// A new copy is listed here. `None` when the store answered from a listing that is gone by
    /// now: the copy it found was removed while this file was being read, and linking the
    /// attachment to it would name a file the chat no longer lists.
    fn land_original(
        &mut self,
        chat_id: Uuid,
        stored: crate::features::chat_files::Stored,
    ) -> Option<ChatFile> {
        use crate::features::chat_files::Stored;

        match stored {
            Stored::New(file) => {
                if let Some(chat) = self.chat_mut(chat_id) {
                    chat.list_file(file.clone());
                }
                self.mark_dirty(chat_id);
                Some(file)
            }
            // Both mean "the chat already lists this file": the listing is right and keeps
            // its id, so nothing is added and the replaced original is not dropped. The
            // difference is only that a restore has just put the bytes back — which is the
            // whole point of re-attaching a file whose copy went missing, and what the
            // `missing` marker disappearing from `/file list` tells the user.
            Stored::Unchanged(file) | Stored::Restored(file) => {
                let still_listed = self
                    .chats
                    .iter()
                    .find(|c| c.id == chat_id)
                    .is_none_or(|c| c.files.iter().any(|f| f.id == file.id));
                still_listed.then_some(file)
            }
        }
    }

    /// Deletes our copy of a replaced original and drops its listing: the copy first, so a
    /// failed delete leaves it listed and removable rather than orphaned on disk
    /// (§11 S11's order, §12 T9).
    fn drop_original(&mut self, chat_id: Uuid, file_id: Uuid) {
        let dir = self.stored_files_dir(chat_id);
        let Some(name) = self
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.files.iter().find(|f| f.id == file_id))
            .map(|f| f.name.clone())
        else {
            return;
        };
        if crate::features::chat_files::remove(&dir, &name).is_err() {
            return; // still on disk and still listed — nothing lost, `/file remove` retries
        }
        if let Some(chat) = self.chat_mut(chat_id) {
            chat.files.retain(|f| f.id != file_id);
        }
        self.mark_dirty(chat_id);
    }

    /// Puts an **already-built** attachment into a chat and does everything that
    /// follows: dedupe by source, persistence, index prune, the feed note, the
    /// status chip, and background indexing.
    ///
    /// Two entry points share it: `/file attach` above, and a tool that produced
    /// an attachment of its own and returned it as a `ChatEffect::AddAttachment`
    /// (a video transcript — spec §9.9, docs/history/youtube-transcript.md §3 F1). The
    /// mode is **not** re-decided here: the tool already told the model what it
    /// did, and the object described and the object stored have to be the same
    /// one — down to the `id`, which is the key the index is written under.
    ///
    /// `read_as` names the encoding a file was read in when that was not UTF-8, for the
    /// feed note (docs/research/local-file-encoding.md F4b); `None` for a tool's own.
    pub(super) fn insert_attachment(
        &mut self,
        chat_id: Uuid,
        attachment: Attachment,
        read_as: Option<&'static str>,
    ) {
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
            read_as,
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

    /// Removes an attachment or a stored file by name/path/`#N` (`/file remove <target>`),
    /// numbered as `/file list` shows them. A name several of them share removes nothing:
    /// the refusal lists each one's `#N` and source, either of which reaches it alone
    /// (docs/research/remove-by-shared-name.md F1a).
    pub(super) fn handle_file_remove(&mut self, target: String) {
        let Some(chat_id) = self.active_id else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return;
        };
        let loc = self.ui_locale();
        let (items, dir) = self.chat_file_list(chat_id);
        let Some(item) = self.resolve_file_handle(&items, &target) else {
            return;
        };
        // An image belongs to the message that carries it: taking it out of the
        // conversation is not what `/file remove` does, and saying only "no" would leave
        // the user looking for the command that does (docs/lessons.md §4).
        if item.is_image() {
            let msg = loc.tf("ui.err.file_is_image", &[("name", &item.name)]);
            self.fail_file(&msg);
            return;
        }
        match (item.attachment, item.file.as_deref()) {
            // An attached document that kept its original — one item, both halves.
            (Some(at), Some(file)) => self.remove_pair(chat_id, &dir, at, file, &item.name),
            (Some(at), None) => {
                // Removed by `#N` or path, a file whose name another item shares is named
                // by its source too — the name alone would not say which of them went (F2a).
                let shared = name_is_shared(&items, item.handle - 1, |i| i.name.as_str());
                let Some(chat) = self.chat_mut(chat_id) else {
                    return;
                };
                let removed = chat.attachments.remove(at);
                let keep: Vec<Uuid> = chat.attachments.iter().map(|a| a.id).collect();
                self.mark_dirty(chat_id);
                // The removed file's index chunks go with it — otherwise the model could
                // still find fragments of a file the user took out of the conversation.
                self.prune_attachment_index(chat_id, &keep);
                self.emit_file_progress(FileProgress::Removed {
                    name: removed.name,
                    source: shared.then_some(removed.source),
                });
                self.emit_attachments();
            }
            (None, Some(file)) => self.remove_stored_file(chat_id, &dir, file.to_string()),
            (None, None) => {}
        }
    }

    /// The chat's files as one numbered list, and the folder its stored files live in —
    /// the single numbering `/file list`, `/file remove`, the pinned block and
    /// `python_exec` all take (docs/history/sandbox-file-exchange.md §12 T2). The images are
    /// borrowed, never cloned: a listing must not copy a conversation's base64 payloads.
    pub(super) fn chat_file_list(
        &self,
        chat_id: Uuid,
    ) -> (
        Vec<crate::features::chat_inputs::ChatInput>,
        std::path::PathBuf,
    ) {
        let dir = self.stored_files_dir(chat_id);
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            return (Vec::new(), dir);
        };
        let images: Vec<&crate::entities::message_image::MessageImage> =
            chat.messages.iter().flat_map(|m| m.images.iter()).collect();
        let items =
            crate::features::chat_inputs::items(&chat.attachments, &chat.files, &images, &dir);
        (items, dir)
    }

    /// Removes an attached document that kept its original (fork F8a, §12 T9): our copy of
    /// the file first, then both listings — the order in which a failed delete leaves
    /// everything listed and retryable (§11 S11). The user's own file is never touched.
    fn remove_pair(
        &mut self,
        chat_id: Uuid,
        dir: &std::path::Path,
        attachment_at: usize,
        file: &str,
        name: &str,
    ) {
        if let Err(err) = crate::features::chat_files::remove(dir, file) {
            let msg = self.ui_locale().tf(
                "ui.err.file_delete_failed",
                &[("name", file), ("err", &err.to_string())],
            );
            self.fail_file(&msg);
            return;
        }
        let mut keep = Vec::new();
        if let Some(chat) = self.chat_mut(chat_id) {
            chat.files.retain(|f| f.name != file);
            chat.attachments.remove(attachment_at);
            keep = chat.attachments.iter().map(|a| a.id).collect();
        }
        self.mark_dirty(chat_id);
        self.prune_attachment_index(chat_id, &keep);
        self.emit_file_progress(FileProgress::RemovedPair { name: name.into() });
        self.emit_attachments();
    }

    /// A shared name's candidates, one line each, with the `#N` and the source that reach
    /// each one alone — the refusal `/file remove` and `/image remove` both give.
    pub(super) fn candidate_lines<'a>(
        candidates: impl Iterator<Item = (usize, &'a str)>,
    ) -> String {
        candidates
            .map(|(i, source)| format!("\n• #{} {source}", i + 1))
            .collect()
    }

    /// Deletes our copy of a stored file, then drops its listing — the order in which a
    /// failed delete keeps the listing, so the removal can be retried and nothing is lost
    /// (docs/history/sandbox-file-exchange.md §11 S11, docs/lessons.md §8).
    fn remove_stored_file(&mut self, chat_id: Uuid, dir: &std::path::Path, name: String) {
        if let Err(err) = crate::features::chat_files::remove(dir, &name) {
            let msg = self.ui_locale().tf(
                "ui.err.file_delete_failed",
                &[("name", &name), ("err", &err.to_string())],
            );
            self.fail_file(&msg);
            return;
        }
        if let Some(chat) = self.chat_mut(chat_id) {
            chat.files.retain(|f| f.name != name);
        }
        self.mark_dirty(chat_id);
        self.emit_file_progress(FileProgress::RemovedStored { name });
    }

    /// Lists the active chat's attachments and stored files (`/file list`), numbered as
    /// `/file remove` takes them.
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
        let dir = self.stored_files_dir(chat.id);
        // A stored file an attachment links is shown on that attachment's line, not as an
        // item of its own — the pair is one item, here as everywhere (§12 T9).
        let linked: Vec<Uuid> = chat.attachments.iter().filter_map(|a| a.file_id).collect();
        let stored = chat
            .files
            .iter()
            .filter(|f| !linked.contains(&f.id))
            .map(|f| StoredInfo {
                name: f.name.clone(),
                bytes: f.bytes,
                mime: f.mime.clone(),
                missing: !crate::features::chat_files::exists(&dir, &f.name),
            })
            .collect();
        // The images the conversation carries, numbered on from the rest: the user sees
        // the `#N` the code can name (§12 T4).
        let images = chat
            .messages
            .iter()
            .flat_map(|m| m.images.iter())
            .map(crate::entities::message_image::ImageInfo::from)
            .collect();
        self.emit_file_progress(FileProgress::Listed {
            items,
            stored,
            images,
            dir: dir.display().to_string(),
        });
    }

    /// Opens one of the chat's files in the desktop environment (`/file open <name|#N>`,
    /// fork F9). The handle is the one numbered list's (§12 T2), what it means on disk is
    /// decided purely (§13 U2), and the file's type decides whether the file itself or the
    /// folder it sits in is handed to the shell (§13 U3).
    pub(super) fn handle_file_open(&mut self, target: String) {
        let Some((path, opened)) = self.plan_open(&target) else {
            return;
        };
        // `plan_open` refuses without an open chat, so there is one to address.
        if let Some(chat_id) = self.active_id {
            self.spawn_open(chat_id, path, opened);
        }
    }

    /// What `/file open <target>` hands to the shell, and the note it will leave — or
    /// `None`, every refusal having been reported here with nothing launched. The decision
    /// is separated from the launch so it can be tested without a window opening on the
    /// machine running the tests (§13 U10: the launch itself is the manual gate).
    pub(super) fn plan_open(&self, target: &str) -> Option<(std::path::PathBuf, FileProgress)> {
        let Some(chat_id) = self.active_id else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return None;
        };
        let (items, dir) = self.chat_file_list(chat_id);
        let item = self.resolve_file_handle(&items, target)?;
        let nothing_to_open = || {
            let msg = self.ui_locale().tf(
                "ui.err.file_nothing_to_open",
                &[("name", &item.name), ("source", &item.source)],
            );
            self.fail_file(&msg);
        };
        // The one disk check: a pasted image's source is `clipboard:<uuid>`, a fetched
        // page's attachment carries a URL, and a listed copy can be gone from the folder.
        // A stored name that is not one plain component names no file of ours at all.
        let Some(path) =
            crate::features::chat_inputs::open_path(&item, &dir).filter(|path| path.is_file())
        else {
            nothing_to_open();
            return None;
        };
        // `None` only for a refused type with no folder around it, which the absolute
        // paths above never are — refused rather than handed over if one ever is.
        let Some((opens, at)) = crate::shared::os_open::decide(&path) else {
            nothing_to_open();
            return None;
        };
        let progress = match opens {
            crate::shared::os_open::Opens::File => FileProgress::Opened {
                name: item.name.clone(),
                path: at.display().to_string(),
            },
            // Not a type we let a handler run: its folder opens instead, and the note says
            // why — a `run.bat` or a scripted `.html` a call wrote would otherwise *run*.
            // Whose file it is decides whether that is the reason to give (§13 U3).
            crate::shared::os_open::Opens::Folder => FileProgress::OpenedFolder {
                path: at.display().to_string(),
                instead_of: Some(OpenedInstead {
                    name: item.name.clone(),
                    by_a_call: self.written_by_a_call(chat_id, item.id),
                }),
            },
        };
        Some((at.to_path_buf(), progress))
    }

    /// Whether the chat's item `id` is a file a `python_exec` call wrote: stored from
    /// `/w/out`, or adopted from the folder as one. An attachment, its kept original and an
    /// image are the user's own.
    fn written_by_a_call(&self, chat_id: Uuid, id: Uuid) -> bool {
        self.chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| chat.files.iter().find(|file| file.id == id))
            .is_some_and(|file| matches!(file.origin, FileOrigin::Sandbox | FileOrigin::Recovered))
    }

    /// Opens the chat's stored-files folder (`/file folder`). A chat that has stored
    /// nothing has no folder yet: the command says so and prints the path rather than
    /// creating an empty directory for it (§13 U7).
    pub(super) fn handle_file_folder(&mut self) {
        let Some(chat_id) = self.active_id else {
            self.fail_file(self.ui_locale().t("ui.err.file_no_active_chat"));
            return;
        };
        let dir = self.stored_files_dir(chat_id);
        let path = dir.display().to_string();
        if !dir.is_dir() {
            let msg = self
                .ui_locale()
                .tf("ui.err.file_no_folder_yet", &[("path", &path)]);
            self.fail_file(&msg);
            return;
        }
        self.spawn_open(
            chat_id,
            dir,
            FileProgress::OpenedFolder {
                path,
                instead_of: None,
            },
        );
    }

    /// Resolves a handle typed after `/file remove` or `/file open` against the chat's one
    /// numbered list, reporting the two refusals they share: nothing of that name, and a
    /// name several items carry — which lists each candidate's `#N` and source rather than
    /// guessing which was meant (docs/research/remove-by-shared-name.md).
    fn resolve_file_handle(
        &self,
        items: &[crate::features::chat_inputs::ChatInput],
        target: &str,
    ) -> Option<crate::features::chat_inputs::ChatInput> {
        let loc = self.ui_locale();
        match crate::features::chat_inputs::resolve(items, target) {
            Resolved::One(at) => Some(items[at].clone()),
            Resolved::Shared(hits) => {
                // Each one's source is what tells them apart — an attachment's path, a
                // stored file's place in the folder, an image's origin.
                let sources = hits.iter().map(|&i| (i, items[i].source.as_str()));
                let candidates = Self::candidate_lines(sources);
                let msg = loc.tf(
                    "ui.err.file_name_shared",
                    &[("target", target.trim()), ("candidates", &candidates)],
                );
                self.fail_file(&msg);
                None
            }
            Resolved::Nothing => {
                let msg = loc.tf("ui.err.file_not_attached", &[("target", target.trim())]);
                self.fail_file(&msg);
                None
            }
        }
    }

    /// Hands a path to the desktop's handler off the command loop (§13 U5) and reports the
    /// outcome as a note. `ShellExecuteW` returns only once the shell has started the
    /// handler, and `xdg-open` is a script that execs another; a failure — no handler for
    /// the type, no `xdg-open` on the machine — arrives with the path, which is the half
    /// that makes it actionable (§13 U6). The outcome goes back to the loop addressed to
    /// `chat_id` ([`OpenResult`]), not straight to the feed.
    fn spawn_open(&self, chat_id: Uuid, path: std::path::PathBuf, opened: FileProgress) {
        let loc = self.ui_locale();
        let tx = self.open_tx.clone();
        tokio::task::spawn_blocking(move || {
            let progress = match crate::shared::os_open::open(&path) {
                Ok(()) => opened,
                Err(err) => FileProgress::Failed(loc.tf(
                    "ui.err.file_open_failed",
                    &[
                        ("path", &path.display().to_string()),
                        ("err", &err.to_string()),
                    ],
                )),
            };
            let _ = tx.send(OpenResult { chat_id, progress });
        });
    }

    /// Lands a launch's outcome (§13 U5/U6). The launch ran off the loop and may have
    /// outlasted a switch of chats, so a **success** is noted only in the chat it was asked
    /// in — the rule `list_stored_files` keeps — and dropped otherwise: the window that
    /// opened is its own confirmation, and "opened report.pdf" in another chat's feed reads
    /// as that chat's file. A **failure** is noted wherever the user is: it answers a
    /// command given a moment ago, it carries the path that makes it actionable, and a
    /// note is not kept with a chat — holding it for the chat it belongs to would lose it,
    /// not deliver it later.
    pub(super) fn handle_open_result(&self, res: OpenResult) {
        let addressed = self.active_id == Some(res.chat_id);
        if addressed || matches!(res.progress, FileProgress::Failed(_)) {
            self.emit_file_progress(res.progress);
        }
    }

    /// The folder of a chat's stored files (`data/files/<chat-id>/`).
    pub(super) fn stored_files_dir(&self, chat_id: Uuid) -> std::path::PathBuf {
        self.storage.json().files_dir().join(chat_id.to_string())
    }

    /// Lists the files a tool stored in a chat's folder (docs/history/sandbox-file-exchange.md
    /// §11 S7) — a name already listed is skipped, so a landing is idempotent — and names
    /// them in one feed note when the chat is the open one. The one path both a turn's
    /// landing and a background run's take.
    pub(super) fn list_stored_files(&mut self, chat_id: Uuid, files: Vec<ChatFile>) {
        if files.is_empty() {
            return;
        }
        let dir = self.stored_files_dir(chat_id);
        let Some(chat) = self.chat_mut(chat_id) else {
            return;
        };
        let names: Vec<String> = files
            .into_iter()
            .filter_map(|f| {
                let name = f.name.clone();
                chat.list_file(f).then_some(name)
            })
            .collect();
        if names.is_empty() {
            return;
        }
        self.mark_dirty(chat_id);
        if self.active_id == Some(chat_id) {
            self.emit_file_progress(FileProgress::Saved {
                names,
                dir: dir.display().to_string(),
            });
        }
    }

    /// Lists every file in a loaded chat's folder that the chat does not list
    /// (docs/history/sandbox-file-exchange.md §11 S6): a call stored it and the chat was not saved
    /// before the app stopped. Adopted, never deleted — the user may have seen it on the
    /// call's card. Run at startup, when no run can be writing one.
    pub(super) fn adopt_unlisted_files(&mut self) {
        let root = self.storage.json().files_dir();
        if !root.is_dir() {
            return;
        }
        let mut adopted = Vec::new();
        for chat in &mut self.chats {
            let dir = root.join(chat.id.to_string());
            let found = crate::features::chat_files::unlisted(&dir, &chat.files);
            if found.is_empty() {
                continue;
            }
            // Nothing says an adopted file is the user's — it is a call's output whose chat
            // was not saved, or something put in the folder by hand — so it carries the mark
            // a call's output does (§13 U11).
            for file in &found {
                crate::features::chat_files::mark(
                    &dir.join(&file.name),
                    Some(crate::shared::os_open::FROM_ELSEWHERE),
                );
            }
            tracing::info!(
                chat = %chat.id,
                files = found.len(),
                "stored files: adopted files the chat did not list"
            );
            chat.files.extend(found);
            adopted.push(chat.id);
        }
        for id in adopted {
            self.mark_dirty(id);
        }
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
pub(super) struct AttachIndex {
    pub(super) chat_id: Uuid,
    pub(super) attachment_id: Uuid,
    pub(super) name: String,
    /// The canonical path — only to pick the chunker (markdown by headings).
    pub(super) source: String,
    pub(super) text: String,
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

/// Chunks, embeds and writes one attachment into the chat-scoped index — the
/// single place that turns an [`AttachIndex`] into rows. Reuses RAG's chunking
/// (`chunk_text`/`chunk_markdown`) and its sub-batched embedding, calling
/// `progress(done, total)` after each batch so a large file does not look stuck.
///
/// Shared with the `/reindex` backfill (`super::reembed`), which rebuilds
/// attachments the index has no rows for at all — after a data move without
/// `data.db`, say. Two callers, one writer: the chunking, the replace-don't-
/// duplicate delete and the row shape cannot drift apart between the file the
/// user attaches now and the file the repair job restores later.
///
/// The embedder precheck is deliberately **not** here: `/file attach` needs it
/// (there is usually no embedder at all), while `/reindex` has already pinged
/// once for the whole job and must not ping per file.
///
/// `Ok(n)` — rows written (`0` when the text yields no chunks). `Err` — a reason
/// to show, already a sentence; nothing partial is left behind that a rerun
/// would not replace.
pub(super) async fn index_attachment(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    params: ChunkParams,
    index: &AttachIndex,
    loc: &'static Locale,
    cancel: &CancellationToken,
    mut progress: impl FnMut(usize, usize),
) -> Result<usize, String> {
    let chunks = if is_markdown_source(&index.source) {
        crate::features::tools::rag::chunk_markdown(&index.text, params)
    } else {
        crate::features::tools::rag::chunk_text(&index.text, params)
    };
    if chunks.is_empty() {
        return Ok(0);
    }
    // Re-indexing replaces the previous run's chunks instead of duplicating them.
    storage
        .db()
        .attachment_delete(index.chat_id, index.attachment_id)
        .map_err(|e| e.to_string())?;

    let total = chunks.len();
    progress(0, total);
    let mut done = 0usize;
    for batch in chunks.chunks(EMBED_BATCH_CHUNKS) {
        if cancel.is_cancelled() {
            // Stopping mid-file leaves fewer rows than the file has. That is
            // safe by construction: the next run's `attachment_delete` above
            // wipes them before writing, and until then the partial set is
            // simply a smaller index over the same text.
            return Ok(done);
        }
        let embeddings = match embedder.embed(batch.to_vec(), EmbedRole::Passage).await {
            Ok(v) if v.len() == batch.len() => v,
            Ok(_) => return Err(loc.t("ui.err.rag_wrong_vector_count").to_string()),
            Err(err) => return Err(err.to_string()),
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
                return Err(err.to_string());
            }
        }
        done += batch.len();
        progress(done, total);
    }
    Ok(done)
}

/// Indexes one attachment in the background, reporting through `FileProgress` —
/// the `/file attach` path. The work itself is [`index_attachment`].
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
        let skip = |reason: String| {
            let _ = evt_tx.send(AppEvent::FileProgress(FileProgress::IndexSkipped {
                name: index.name.clone(),
                reason,
            }));
        };

        // Precheck — a fast, clear answer when there's no embedder (the common
        // case: RAG isn't configured at all). Before the chunking, which is
        // pointless without one.
        if embedder
            .embed(vec!["ping".into()], EmbedRole::Passage)
            .await
            .is_err()
        {
            skip(loc.t("ui.file.index_no_embedder").to_string());
            return;
        }

        // Nothing cancels a single attach: the task is bounded by one file.
        let cancel = CancellationToken::new();
        let progress_tx = evt_tx.clone();
        let name = index.name.clone();
        let outcome = index_attachment(
            &embedder,
            &storage,
            params,
            &index,
            loc,
            &cancel,
            |done, total| {
                let _ = progress_tx.send(AppEvent::FileProgress(FileProgress::Indexing {
                    name: name.clone(),
                    done,
                    total,
                }));
            },
        )
        .await;

        match outcome {
            // No chunks at all: the file had nothing to index, and the attach
            // itself already reported success — saying more would be noise.
            Ok(0) => {}
            Ok(chunks) => {
                let _ = evt_tx.send(AppEvent::FileProgress(FileProgress::Indexed {
                    name: index.name,
                    chunks,
                }));
            }
            Err(reason) => skip(reason),
        }
    });
}

/// Reads a file and extracts plain text from it (blocking — runs on the blocking
/// pool). Formats: any valid UTF-8 as-is, plus the html/pdf/docx extractors by
/// extension (fork F8) — reusing [`super::rag::read_source_text`]. Errors are
/// already localized (axis B): they go straight into a feed note.
fn extract_file(
    path: &std::path::Path,
    loc: &'static crate::shared::i18n::Locale,
    hint: Option<&str>,
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
    // Read once: the text comes from these bytes, and so does the original the chat keeps
    // when the text is not the file (fork F8a). The **rules** are the shared ones —
    // `doc_extract`, `text_decode::decode_file`, `extract_readable` — as in
    // `rag::read_source_text`, which reads for indexing and keeps no bytes; only the
    // orchestration differs, because only here does the file itself have to survive.
    let bytes = std::fs::read(path)
        .map_err(|e| loc.tf("ui.err.file_unavailable", &[("err", &e.to_string())]))?;
    let (text, encoding, original) = if crate::features::rag_ingest::is_pdf(path) {
        let text = crate::features::doc_extract::extract_pdf(&bytes)
            .map_err(|e| loc.tf("ui.err.file_unreadable", &[("err", &e.to_string())]))?;
        (text, None, Some(bytes))
    } else if crate::features::rag_ingest::is_docx(path) {
        let text = crate::features::doc_extract::extract_docx(&bytes)
            .map_err(|e| loc.tf("ui.err.file_unreadable", &[("err", &e.to_string())]))?;
        (text, None, Some(bytes))
    } else {
        // A file is read in its own encoding (docs/research/local-file-encoding.md F1a).
        let markup = crate::shared::text_decode::is_markup_path(path);
        match crate::shared::text_decode::decode_file(&bytes, markup, hint) {
            Some(file) => {
                let encoding = (file.encoding != encoding_rs::UTF_8).then_some(file.encoding);
                if crate::features::rag_ingest::is_html(path) {
                    let text =
                        crate::features::tools::web::extract_readable(&file.text, usize::MAX);
                    (text, encoding, Some(bytes))
                } else {
                    // Plain text: the snapshot **is** the file, and a second copy would
                    // only be another thing to keep in step.
                    (file.text, encoding, None)
                }
            }
            // Not text at all: the file itself is what the chat keeps, with no attachment
            // made of it — the refusal D3 asked to lift (§12 T9).
            None => (String::new(), None, Some(bytes)),
        }
    };
    if text.trim().is_empty() && original.is_none() {
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
        encoding,
        original,
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
        let file = extract_file(&path, ru(), None).unwrap();
        assert_eq!(file.name, "notes.md");
        assert!(file.text.contains("текст заметки"));
        assert_eq!(file.bytes, std::fs::metadata(&path).unwrap().len() as usize);
        // The source key is canonical (matches how RAG stores sources).
        assert!(file.source.ends_with("notes.md"));
    }

    /// The file `/file attach` refused as not valid UTF-8 (local-file-encoding.md §1) is
    /// read in its own encoding, and the encoding travels on for the feed note.
    #[test]
    fn a_legacy_encoded_file_is_attached_in_its_own_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.txt");
        let prose = "Выручка за март составила сто двадцать тысяч, за апрель немного больше.";
        std::fs::write(&path, encoding_rs::WINDOWS_1251.encode(prose).0).unwrap();
        let file = extract_file(&path, ru(), Some("ru")).unwrap();
        assert_eq!(file.text, prose);
        assert_eq!(
            file.encoding.map(encoding_rs::Encoding::name),
            Some("windows-1251")
        );
    }

    #[test]
    fn source_files_are_attachable_not_just_the_rag_allowlist() {
        // Fork F8: an extension outside RAG's txt/md/html/pdf/docx list is fine
        // as long as it decodes as UTF-8 — source files are the main use case.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn main() { println!(\"hi\"); }").unwrap();
        let file = extract_file(&path, ru(), None).unwrap();
        assert!(file.text.contains("fn main()"));
    }

    /// Fork F8a (docs/history/sandbox-file-exchange.md §12 T9): a binary is no longer turned away
    /// — it has no text, so no attachment is made of it, and the file itself is what the
    /// chat keeps for the code to read. What is still refused is a file that is not there
    /// and one that carries nothing at all.
    #[test]
    fn a_binary_is_kept_while_a_missing_or_empty_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();

        // Binary content → kept as bytes, with no text and no mojibake — even one opening
        // `FF FE`, which is not a whole UTF-16 file.
        let bin = dir.path().join("blob.bin");
        std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x01, 0x80]).unwrap();
        let kept = extract_file(&bin, ru(), None).expect("a binary is kept, not refused");
        assert!(kept.text.trim().is_empty(), "text: {:?}", kept.text);
        assert_eq!(
            kept.original.as_deref(),
            Some(&[0xff, 0xfe, 0x00, 0x01, 0x80][..])
        );

        // A missing path.
        assert!(extract_file(&dir.path().join("nope.txt"), ru(), None).is_err());

        // A directory is not a file.
        assert!(extract_file(dir.path(), ru(), None).is_err());

        // Whitespace-only content carries nothing for the model.
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, "   \n\t ").unwrap();
        assert!(extract_file(&empty, ru(), None).is_err());
    }

    /// The original is kept exactly where the text is **not** the file (§12 T9): a page an
    /// extractor read keeps its bytes; a source file, whose snapshot is the file, does not
    /// get a second copy.
    #[test]
    fn an_extracted_document_keeps_its_original_and_plain_text_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("page.html");
        // A real page, not a snippet: the readable-text extractor weighs a block against
        // the rest of the document, so a one-line body extracts to nothing at all.
        std::fs::write(
            &page,
            "<html><head><title>A page</title></head><body><article>\
             <p>The readable text of this page runs for several sentences, because that \
             is what the extractor weighs a block of prose against the markup around \
             it.</p>\
             <p>A second paragraph gives it something to keep: the file is read in its \
             own encoding, the prose becomes the attachment, and the page itself stays \
             with the chat for the code to open.</p>\
             </article></body></html>",
        )
        .unwrap();
        let extracted = extract_file(&page, ru(), None).unwrap();
        assert!(extracted.text.contains("readable text"), "{extracted:?}");
        assert!(
            extracted.original.is_some_and(|b| b.starts_with(b"<html>")),
            "an extracted page keeps the file it was read from"
        );

        let notes = dir.path().join("notes.md");
        std::fs::write(&notes, "# heading\n\ntext").unwrap();
        assert!(
            extract_file(&notes, ru(), None).unwrap().original.is_none(),
            "plain text needs no second copy — the snapshot is the file"
        );
    }

    #[test]
    fn extraction_errors_are_localized_for_all_langs() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let err = extract_file(&missing, loc, None).unwrap_err();
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
