//! Load/delete/list/rebuild the knowledge base (RAG) via the
//! `/rag add|remove|list|rebuild` commands (spec §9.3). Indexing and rebuilding are
//! cancellable background tasks with progress; delete and list are fast in-place DB
//! operations. Everything is isolated by `profile_id`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppEvent, RagProgress};
use crate::features::tools::rag::ChunkParams;
use crate::shared::api::{EmbedRole, Embedder};
use crate::shared::i18n::Locale;
use crate::shared::storage::Storage;

use super::Orchestrator;

/// Max chunks in a single embedder request. Bounds each request's size and gives
/// progress as chunks become ready (the banner moves *during* embedding of a large
/// file). A file with ≤16 chunks is still embedded in one request (as before) —
/// behavior for small files is unchanged.
pub(super) const EMBED_BATCH_CHUNKS: usize = 16;

impl Orchestrator {
    /// The active chat's profile (RAG is isolated by `profile_id`, §9.5). `None` —
    /// no active chat.
    pub(super) fn active_profile_id(&self) -> Option<Uuid> {
        self.active_id
            .and_then(|id| self.chats.iter().find(|c| c.id == id))
            .map(|c| c.profile_id)
    }

    /// Chunking parameters from the current settings (`config.rag`).
    fn chunk_params(&self) -> ChunkParams {
        ChunkParams::from_settings(&self.config.rag)
    }

    /// Starts background indexing of a file/directory into the active profile's
    /// RAG (the `/rag add` command, spec §9.3). Scanning, reading, embedding, and
    /// writing run in a separate task; progress — via [`RagProgress`] events. A
    /// previous unfinished indexing run is cancelled (one at a time).
    pub(super) fn handle_rag_add(&mut self, path: String, recursive: bool) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };

        let cancel = self.reset_rag_cancel();
        spawn_rag_ingest(RagIngest {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            profile_id,
            root: std::path::PathBuf::from(path),
            recursive,
            params: self.chunk_params(),
            cancel,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
        });
    }

    /// Removes a file or directory (and everything under it) from the active
    /// profile's RAG by path (the `/rag remove` command, spec §9.3). A fast
    /// operation (DB only, no embedding), so it runs in place. If the path exists
    /// on disk — its canonical key is used (as at add time); otherwise it's
    /// matched by the entered string (the DB itself normalizes separators/case),
    /// which lets records of already-deleted files be cleaned up.
    pub(super) fn handle_rag_delete(&mut self, path: String) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let p = std::path::Path::new(&path);
        let needle = if p.exists() {
            crate::features::rag_ingest::canonical_source(p)
        } else {
            path.clone()
        };
        let progress = match self.storage.db().rag_delete_under(profile_id, &needle) {
            Ok(chunks) => {
                // Emptying the base also clears any "indexed by a previous
                // embedding model" mark — nothing stale is left to protect
                // against. Without this, a user who removed everything and
                // re-added it under the new model would still be refused by
                // `rag_search` (the mark is otherwise only lifted by
                // `/rag rebuild`, which needs sources to rebuild from).
                if self.storage.db().rag_count(profile_id).unwrap_or(1) == 0
                    && let Err(err) = self.storage.db().clear_rag_stale_profile(profile_id)
                {
                    tracing::warn!(error = %err, "failed to clear the stale knowledge-base mark");
                }
                RagProgress::Removed { chunks }
            }
            Err(err) => RagProgress::Failed(
                self.ui_locale()
                    .tf("ui.err.rag_delete_failed", &[("err", &err.to_string())]),
            ),
        };
        let _ = self.evt_tx.send(AppEvent::RagProgress(progress));
    }

    /// Shows the active profile's knowledge-base sources (the `/rag list`
    /// command): per source — chunk count and date. A fast in-place DB operation.
    pub(super) fn handle_rag_list(&mut self) {
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let progress = match self.storage.db().rag_list_sources(profile_id) {
            Ok(sources) => RagProgress::Listed { sources },
            Err(err) => RagProgress::Failed(
                self.ui_locale()
                    .tf("ui.err.rag_read_kb_failed", &[("err", &err.to_string())]),
            ),
        };
        let _ = self.evt_tx.send(AppEvent::RagProgress(progress));
    }

    /// Rebuilds the active profile's knowledge base (the `/rag rebuild` command,
    /// spec §9.3): re-chunks and re-embeds the stored sources with the current
    /// parameters/embedding model. Needed after changing the chunk size/overlap or
    /// the embedding model (a different dimensionality). Runs as a background
    /// task; cancels the previous RAG operation (one at a time).
    pub(super) fn handle_rag_rebuild(&mut self) {
        let Some(profile_id) = self.active_profile_id() else {
            self.fail_rag(self.ui_locale().t("ui.err.rag_no_active_chat"));
            return;
        };
        let cancel = self.reset_rag_cancel();
        spawn_rag_rebuild(RagRebuild {
            embedder: self.engines.embedder(),
            storage: self.storage.clone(),
            profile_id,
            params: self.chunk_params(),
            cancel,
            loc: self.ui_locale(),
            evt_tx: self.evt_tx.clone(),
        });
    }

    /// Cancels the previous background RAG task (if one was running) and starts a
    /// new token.
    pub(super) fn reset_rag_cancel(&mut self) -> CancellationToken {
        if let Some(token) = self.rag_cancel.take() {
            token.cancel();
        }
        let cancel = CancellationToken::new();
        self.rag_cancel = Some(cancel.clone());
        cancel
    }

    /// Sends a RAG-operation error to the UI (banner/note).
    pub(super) fn fail_rag(&self, msg: &str) {
        let _ = self
            .evt_tx
            .send(AppEvent::RagProgress(RagProgress::Failed(msg.to_string())));
    }
}

/// Parameters of the background RAG file-indexing task (`/rag add`).
struct RagIngest {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    profile_id: Uuid,
    root: std::path::PathBuf,
    recursive: bool,
    params: ChunkParams,
    cancel: CancellationToken,
    /// The interface language (axis B) — for progress/error messages visible to the user.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
}

/// Starts background indexing (spec §9.3): scans the path, checks embedder
/// availability, then reads/chunks/embeds/writes each file in turn, emitting
/// [`RagProgress`]. Cancellable via `cancel` (between files). Storage is
/// thread-safe (an internal mutex), embedding is async — the task doesn't block
/// the orchestrator.
fn spawn_rag_ingest(task: RagIngest) {
    let RagIngest {
        embedder,
        storage,
        profile_id,
        root,
        recursive,
        params,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Scan files (txt/md/html). A path error / empty result — a clear refusal.
        let files = match crate::features::rag_ingest::scan(&root, recursive) {
            Ok(files) => files,
            Err(err) => {
                send(RagProgress::Failed(loc.tf(
                    "ui.err.rag_path_unavailable",
                    &[("err", &err.to_string())],
                )));
                return;
            }
        };
        if files.is_empty() {
            send(RagProgress::Failed(loc.t("ui.err.rag_no_files").into()));
            return;
        }

        // 2. Embedder precheck — a fast, clear refusal if RAG isn't configured.
        if let Err(err) = embedder
            .embed(vec!["ping".into()], EmbedRole::Passage)
            .await
        {
            send(RagProgress::Failed(loc.tf(
                "ui.err.rag_embedder_unavailable",
                &[("err", &err.to_string())],
            )));
            return;
        }

        let total = files.len();
        send(RagProgress::Started { total });

        let mut chunks_total = 0usize;
        let mut errors = 0usize;
        for (i, file) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            let (name, dir) = display_parts(file);
            // The file has just started (chunking is still ahead) — chunks_total=0
            // (the banner as before).
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name: name.clone(),
                dir: dir.clone(),
                chunks_done: 0,
                chunks_total: 0,
            });
            // Chunk progress: clone the name/dir into the closure (FnMut is called
            // repeatedly, and `send` borrows `evt_tx`).
            let progress = |done: usize, tot: usize| {
                let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
                    index: i + 1,
                    total,
                    name: name.clone(),
                    dir: dir.clone(),
                    chunks_done: done,
                    chunks_total: tot,
                }));
            };
            match index_file(&embedder, &storage, profile_id, file, params, loc, progress).await {
                Ok(n) => chunks_total += n,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(file = %file.display(), error = %err, "RAG: failed to index file");
                }
            }
        }

        send(RagProgress::Finished {
            files: total,
            chunks: chunks_total,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Parameters of the background rebuild task (`/rag rebuild`).
struct RagRebuild {
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    profile_id: Uuid,
    params: ChunkParams,
    cancel: CancellationToken,
    /// The interface language (axis B) — for progress/error messages visible to the user.
    loc: &'static Locale,
    evt_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,
}

/// Starts a background rebuild of the profile's knowledge base (spec §9.3).
/// Gathers sources (stored text, otherwise — reading the file by path), if needed
/// resets the vectors table (an embedding-model dimensionality change), then
/// re-chunks/re-embeds each source with the current parameters.
fn spawn_rag_rebuild(task: RagRebuild) {
    let RagRebuild {
        embedder,
        storage,
        profile_id,
        params,
        cancel,
        loc,
        evt_tx,
    } = task;

    tokio::spawn(async move {
        let send = |p: RagProgress| {
            let _ = evt_tx.send(AppEvent::RagProgress(p));
        };

        // 1. Gather sources: what's currently in the DB + their stored text.
        let infos = match storage.db().rag_list_sources(profile_id) {
            Ok(v) => v,
            Err(err) => {
                send(RagProgress::Failed(
                    loc.tf("ui.err.rag_read_kb", &[("err", &err.to_string())]),
                ));
                return;
            }
        };
        if infos.is_empty() {
            send(RagProgress::Failed(loc.t("ui.err.rag_kb_empty").into()));
            return;
        }
        let stored: HashMap<String, String> = match storage.db().rag_stored_sources(profile_id) {
            Ok(v) => v.into_iter().map(|s| (s.source, s.content)).collect(),
            Err(err) => {
                send(RagProgress::Failed(
                    loc.tf("ui.err.rag_read_sources", &[("err", &err.to_string())]),
                ));
                return;
            }
        };

        // 2. Resolve each source's content: stored text takes priority,
        //    otherwise try reading the file by path (legacy data predating text storage).
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut missing = 0usize;
        for info in &infos {
            if let Some(content) = stored.get(&info.source) {
                sources.push((info.source.clone(), content.clone()));
                continue;
            }
            let path = std::path::Path::new(&info.source);
            if path.is_file()
                && crate::features::rag_ingest::is_supported(path)
                && let Ok(content) = read_source_text(path)
            {
                sources.push((info.source.clone(), content));
            } else {
                // The source isn't stored and there's no file — this source can't be recovered.
                missing += 1;
                tracing::warn!(source = %info.source, "RAG rebuild: source unavailable, skipping it");
            }
        }
        if sources.is_empty() {
            send(RagProgress::Failed(
                loc.t("ui.err.rag_no_source_text").into(),
            ));
            return;
        }

        // 3. Embedder precheck and determining the new dimensionality.
        let new_dim = match embedder
            .embed(vec!["ping".into()], EmbedRole::Passage)
            .await
        {
            Ok(v) => v.first().map(|e| e.len()).unwrap_or(0),
            Err(err) => {
                send(RagProgress::Failed(loc.tf(
                    "ui.err.rag_embedder_unavailable",
                    &[("err", &err.to_string())],
                )));
                return;
            }
        };
        if new_dim == 0 {
            send(RagProgress::Failed(loc.t("ui.err.rag_empty_vector").into()));
            return;
        }

        // 4. If the dimensionality changed (a different embedding model), the
        //    vectors table needs to be recreated — but it's shared across the
        //    whole DB. If other profiles have documents, refuse (don't overwrite
        //    someone else's data); otherwise reset it.
        let current_dim = storage.db().rag_dimension().unwrap_or(None);
        let dim_changed = matches!(current_dim, Some(d) if d != new_dim);
        if dim_changed {
            match storage.db().rag_other_profiles_have_docs(profile_id) {
                Ok(true) => {
                    send(RagProgress::Failed(loc.t("ui.err.rag_dim_conflict").into()));
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    send(RagProgress::Failed(loc.tf(
                        "ui.err.rag_profiles_check",
                        &[("err", &err.to_string())],
                    )));
                    return;
                }
            }
        }

        // 5. Drop the profile's previous chunks (sources are kept); on a
        //    dimensionality change, additionally reset the vectors table (it's
        //    recreated on the first insert).
        if let Err(err) = storage.db().rag_delete_all_for_profile(profile_id) {
            send(RagProgress::Failed(
                loc.tf("ui.err.rag_clear_chunks", &[("err", &err.to_string())]),
            ));
            return;
        }
        // From here on the base holds no chunks from a previous embedding model —
        // every one that follows is written by the current one. So the "stale"
        // mark is lifted here rather than at the end: it stays correct even if the
        // rebuild is cancelled or some sources fail, since nothing old survives
        // either way (see `embed_guard`).
        if let Err(err) = storage.db().clear_rag_stale_profile(profile_id) {
            tracing::warn!(error = %err, "failed to clear the stale knowledge-base mark");
        }
        if dim_changed {
            // The dimensionality is shared with the chat attachment index
            // (`meta.rag_dim`), so its chunks go too — they were embedded by the
            // old model and their vectors are dropped with the table. Derived
            // data: re-attaching the file rebuilds it, and `attachment_read`
            // (the guaranteed path) is unaffected. See spec §9.7.
            match storage.db().reset_vectors() {
                Ok(0) => {}
                Ok(dropped) => tracing::warn!(
                    dropped,
                    "embedding dimensionality changed: the chat attachment index was dropped too"
                ),
                Err(err) => {
                    send(RagProgress::Failed(loc.tf(
                        "ui.err.rag_reset_vectors",
                        &[("err", &err.to_string())],
                    )));
                    return;
                }
            }
        }

        let total = sources.len();
        send(RagProgress::Started { total });

        let mut chunks_total = 0usize;
        let mut errors = missing;
        for (i, (source, content)) in sources.iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            let name = source_display(source);
            // The source has just started (chunking is ahead) — chunks_total=0
            // (the banner as before).
            send(RagProgress::Indexing {
                index: i + 1,
                total,
                name: name.clone(),
                dir: String::new(),
                chunks_done: 0,
                chunks_total: 0,
            });
            // Chunk progress (see spawn_rag_ingest): rebuild calls index_source directly.
            let progress = |done: usize, tot: usize| {
                let _ = evt_tx.send(AppEvent::RagProgress(RagProgress::Indexing {
                    index: i + 1,
                    total,
                    name: name.clone(),
                    dir: String::new(),
                    chunks_done: done,
                    chunks_total: tot,
                }));
            };
            match index_source(
                &embedder, &storage, profile_id, source, content, params, loc, progress,
            )
            .await
            {
                Ok(n) => chunks_total += n,
                Err(err) => {
                    errors += 1;
                    tracing::warn!(source = %source, error = %err, "RAG rebuild: failed to reindex source");
                }
            }
        }

        send(RagProgress::Finished {
            files: total,
            chunks: chunks_total,
            errors,
            cancelled: cancel.is_cancelled(),
        });
    });
}

/// Reads a source for indexing, extracting plain text by format:
/// - `.html`/`.htm` — readable text (reuses `web::extract_readable`, drops
///   nav/header/footer/aside/scripts; RAG chunks the source whole, hence no
///   truncation — `usize::MAX`);
/// - `.pdf` — the `pdf-extract` crate (best-effort quality);
/// - `.docx` — ZIP + `word/document.xml` (`features/doc_extract.rs`);
/// - everything else (txt/md) — as-is (BOM stripped by `read_text`).
///
/// Binary formats (pdf/docx) are read as raw bytes (`fs::read`), not via
/// `read_text` (which decodes as UTF-8). Extraction can fail with
/// context (the caller skips such a source).
///
/// Shared with chat attachments (`/file attach`, see [`super::attachments`]) —
/// both need the same per-format extraction, and it lives in `app` because it
/// reaches into `features/tools/web`, which `features` may not import sideways
/// (FSD).
pub(super) fn read_source_text(path: &std::path::Path) -> anyhow::Result<String> {
    use crate::features::{doc_extract, rag_ingest};
    if rag_ingest::is_html(path) {
        let raw = rag_ingest::read_text(path)?;
        Ok(crate::features::tools::web::extract_readable(
            &raw,
            usize::MAX,
        ))
    } else if rag_ingest::is_pdf(path) {
        doc_extract::extract_pdf(&std::fs::read(path)?)
    } else if rag_ingest::is_docx(path) {
        doc_extract::extract_docx(&std::fs::read(path)?)
    } else {
        Ok(rag_ingest::read_text(path)?)
    }
}

/// Indexes a single file: reads the text, chunks it, embeds it, and writes
/// documents into storage (isolated by `profile_id`). Returns the number of
/// chunks written.
async fn index_file(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    path: &std::path::Path,
    params: ChunkParams,
    loc: &'static Locale,
    progress: impl FnMut(usize, usize),
) -> anyhow::Result<usize> {
    let content = read_source_text(path)?;
    // Canonical source key + idempotency: re-adding the same file replaces its
    // previous chunks instead of duplicating them (see [`index_source`]).
    let source = crate::features::rag_ingest::canonical_source(path);
    index_source(
        embedder, storage, profile_id, &source, &content, params, loc, progress,
    )
    .await
}

/// Chunks/embeds/writes a single source as one unit (replacing its previous
/// chunks and stored text). Markdown (`*.md`) is chunked semantically (by
/// headings), everything else — by the text chunker. Returns the number of
/// chunks written. Shared logic for file indexing (`/rag add`) and rebuilding
/// (`/rag rebuild`).
///
/// Embedding runs in sub-batches of [`EMBED_BATCH_CHUNKS`]: after each batch
/// `progress(chunks_done, chunks_total)` is called, so the banner moves *during*
/// embedding of a large file. A file with ≤16 chunks — still a single request.
// The arguments are cohesive (indexing dependencies + the progress callback) and
// are passed positionally from two call sites; carving out a bundle just for one
// extra parameter would be needless churn.
#[allow(clippy::too_many_arguments)]
async fn index_source(
    embedder: &Arc<dyn Embedder>,
    storage: &Arc<Storage>,
    profile_id: Uuid,
    source: &str,
    content: &str,
    params: ChunkParams,
    loc: &'static Locale,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<usize> {
    let chunks = if is_markdown_source(source) {
        crate::features::tools::rag::chunk_markdown(content, params)
    } else {
        crate::features::tools::rag::chunk_text(content, params)
    };
    // Even for an empty source, update the stored text and drop the previous
    // chunks — otherwise stale chunks would remain in the DB after a rebuild.
    storage.db().rag_delete_by_source(profile_id, source)?;
    storage
        .db()
        .rag_source_upsert(profile_id, source, content, chrono::Utc::now())?;
    if chunks.is_empty() {
        progress(0, 0);
        return Ok(0);
    }
    let total = chunks.len();
    progress(0, total);
    // Embed and write in sub-batches: each request is bounded by
    // EMBED_BATCH_CHUNKS, and progress moves as batches become ready (smooth for
    // large files).
    let mut done = 0usize;
    for batch in chunks.chunks(EMBED_BATCH_CHUNKS) {
        let embeddings = embedder.embed(batch.to_vec(), EmbedRole::Passage).await?;
        if embeddings.len() != batch.len() {
            anyhow::bail!("{}", loc.t("ui.err.rag_wrong_vector_count"));
        }
        for (chunk, embedding) in batch.iter().zip(embeddings) {
            let doc = crate::entities::rag::RagDocument::new(profile_id, source, chunk, embedding);
            storage.db().rag_insert(&doc)?;
        }
        done += batch.len();
        progress(done, total);
    }
    Ok(total)
}

/// Is the source markdown (chunked by headings)? Decided by the `*.md` extension.
/// Shared with the chat attachment index (see [`super::attachments`]).
pub(super) fn is_markdown_source(source: &str) -> bool {
    std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// A short source name for progress indication (the file name, else the source itself).
fn source_display(source: &str) -> String {
    std::path::Path::new(source)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.to_string())
}

/// The file name and its parent directory (for indexing progress indication).
fn display_parts(path: &std::path::Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let dir = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    (name, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_source_text_extracts_html_and_passes_through_plain() {
        let dir = tempfile::tempdir().unwrap();

        // HTML: boilerplate (nav/header/script) is dropped, the article paragraph is extracted.
        let html = dir.path().join("page.html");
        std::fs::write(
            &html,
            "<html><head><script>var secret = 'скриптовый мусор';</script></head>\
             <body><nav>навигационное меню сайта здесь</nav>\
             <header>шапка страницы с логотипом</header>\
             <article><p>Осмысленный абзац содержимого статьи, достаточно длинный, \
             чтобы пройти порог отсева коротких фрагментов извлечения.</p></article>\
             </body></html>",
        )
        .unwrap();
        let extracted = read_source_text(&html).unwrap();
        assert!(
            extracted.contains("Осмысленный абзац содержимого статьи"),
            "extracted text should contain the article paragraph: {extracted:?}"
        );
        assert!(
            !extracted.contains("навигационное меню"),
            "nav must not appear in the extracted text: {extracted:?}"
        );
        assert!(
            !extracted.contains("шапка страницы"),
            "header must not appear in the extracted text: {extracted:?}"
        );
        assert!(
            !extracted.contains("скриптовый мусор"),
            "script must not appear in the extracted text: {extracted:?}"
        );

        // Non-HTML: content is returned verbatim (BOM stripped by read_text).
        let txt = dir.path().join("note.txt");
        let body = "<p>это не HTML</p>\nобычный текст с угловыми скобками";
        std::fs::write(&txt, body).unwrap();
        assert_eq!(read_source_text(&txt).unwrap(), body);
    }

    #[test]
    fn read_source_text_routes_docx_and_pdf() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();

        // DOCX: build a minimal archive with word/document.xml and extract the paragraph text.
        let docx = dir.path().join("doc.docx");
        let xml = "<?xml version=\"1.0\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body><w:p><w:r><w:t>Абзац из DOCX-документа</w:t></w:r></w:p></w:body></w:document>";
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        std::fs::write(&docx, zip.finish().unwrap().into_inner()).unwrap();
        assert_eq!(
            read_source_text(&docx).unwrap(),
            "Абзац из DOCX-документа",
            "DOCX should route through text extraction"
        );

        // PDF: the same fixture as in doc_extract — routed through extract_pdf.
        let pdf = dir.path().join("doc.pdf");
        std::fs::write(&pdf, include_bytes!("../../../tests/fixtures/hello.pdf")).unwrap();
        assert!(
            read_source_text(&pdf).unwrap().contains("Hello World"),
            "PDF should route through text extraction"
        );
    }

    /// Storage on a tempdir + a deterministic embedder for indexing tests. The
    /// SQLite halves are in memory — these tests index and query within one
    /// `Storage`, so nothing needs a file, and indexing is exactly the
    /// fsync-heavy work a slow disk punishes. The directory guard is still
    /// returned: the JSON half uses it.
    fn test_deps() -> (tempfile::TempDir, Arc<Storage>, Arc<dyn Embedder>) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            Storage::open_in_memory(crate::shared::paths::Paths::with_root(dir.path())).unwrap(),
        );
        let embedder: Arc<dyn Embedder> = Arc::new(crate::shared::api::mock::MockEmbedder::new(16));
        (dir, storage, embedder)
    }

    #[tokio::test]
    async fn index_source_reports_chunk_progress_in_subbatches() {
        let (_dir, storage, embedder) = test_deps();
        let profile_id = Uuid::new_v4();
        // A small target size → many chunks (> EMBED_BATCH_CHUNKS), so embedding
        // runs in several sub-batches and progress moves along the way.
        let params = ChunkParams::from_settings(&crate::shared::config::RagSettings {
            chunk_target_chars: 60,
            chunk_overlap_chars: 10,
            chunk_max_chars: 120,
        });
        let content = "Короткое предложение для проверки чанкинга номер. ".repeat(40);

        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let n = index_source(
            &embedder,
            &storage,
            profile_id,
            "kb.txt",
            &content,
            params,
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
            |done, total| ticks.push((done, total)),
        )
        .await
        .unwrap();

        assert!(
            n > EMBED_BATCH_CHUNKS,
            "the test should produce > {EMBED_BATCH_CHUNKS} chunks, got {n}"
        );
        // The first tick — (0, N), the last — (N, N).
        assert_eq!(
            ticks.first(),
            Some(&(0, n)),
            "the first tick — (0, N): {ticks:?}"
        );
        assert_eq!(
            ticks.last(),
            Some(&(n, n)),
            "the last tick — (N, N): {ticks:?}"
        );
        // `done` is monotonically non-decreasing, `total` is constant and equals the returned N.
        for w in ticks.windows(2) {
            assert!(w[1].0 >= w[0].0, "done is non-decreasing: {ticks:?}");
            assert_eq!(w[0].1, n, "total is constant and equals N");
        }
        // Sub-batching didn't lose anything: all N chunks are written and found by
        // search (rag_search — the same DB primitive as the RagSearch tool).
        assert_eq!(storage.db().rag_count(profile_id).unwrap(), n);
        let mut q = embedder
            .embed(vec!["предложение".into()], EmbedRole::Passage)
            .await
            .unwrap();
        let query = q.remove(0);
        let hits = storage.db().rag_search(profile_id, &query, 5).unwrap();
        assert!(!hits.is_empty(), "search finds the written chunks");
    }

    #[tokio::test]
    async fn index_source_empty_content_single_zero_tick() {
        let (_dir, storage, embedder) = test_deps();
        let profile_id = Uuid::new_v4();
        let mut ticks: Vec<(usize, usize)> = Vec::new();
        let n = index_source(
            &embedder,
            &storage,
            profile_id,
            "empty.txt",
            "",
            ChunkParams::default(),
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
            |done, total| ticks.push((done, total)),
        )
        .await
        .unwrap();
        assert_eq!(n, 0, "у пустого источника нет чанков");
        assert_eq!(ticks, vec![(0, 0)], "ровно один тик (0,0): {ticks:?}");
    }
}
