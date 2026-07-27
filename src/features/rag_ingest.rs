//! Scans files to index into the knowledge base (RAG, the `/rag add` command).
//! Pure, testable file logic: walking a path, selecting supported
//! extensions (txt/md/html/pdf/docx), optional recursion, and reading content.
//! The indexing process itself (embedding + writing) is driven by the
//! orchestrator's background task. See spec §9.3. Extracting text from HTML/PDF/DOCX
//! is the `app` layer's responsibility (`orchestrator/rag.rs`), not this module's
//! (otherwise `features → features/tools` would be a sideways import,
//! forbidden by FSD); here only the extension is recognized.
//!
//! [`RagProgress`] also lives here — the indexing-progress type. It's defined in the
//! `features` layer so both `app` (emits events) and `screens`
//! (renders the indicator) can use it, without breaking FSD's dependency direction.

use std::path::{Path, PathBuf};

/// Supported file extensions (lowercase, no dot): text, markdown,
/// HTML, PDF, and DOCX. Plain text is extracted from HTML/PDF/DOCX at the `app` layer
/// (`orchestrator/rag.rs`); this module only recognizes the extension.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["txt", "md", "html", "htm", "pdf", "docx"];

/// Progress of background file indexing into RAG. Sent by the orchestrator's task and
/// displayed by the chat screen (a spinner banner + a final note).
#[derive(Debug, Clone, PartialEq)]
pub enum RagProgress {
    /// Scanning finished — starting indexing of `total` files.
    Started { total: usize },
    /// Indexing file `index` of `total` (1-based) named `name` from `dir`.
    /// `chunks_done`/`chunks_total` — embedding progress **within** this file:
    /// how many chunks have already been embedded and written, and how many total (embedding runs
    /// in sub-batches, see `orchestrator/rag.rs::EMBED_BATCH_CHUNKS`). `chunks_total == 0`
    /// — the file has just started (not yet chunked) or has no chunks.
    Indexing {
        index: usize,
        total: usize,
        name: String,
        dir: String,
        chunks_done: usize,
        chunks_total: usize,
    },
    /// Indexing finished (or was interrupted, if `cancelled`).
    Finished {
        files: usize,
        chunks: usize,
        errors: usize,
        cancelled: bool,
    },
    /// Re-embedding finished (`/reindex`): `rows` vectors rewritten with the
    /// current model, `errors` rows skipped. `cancelled` — interrupted, which is
    /// safe: every rewritten row is stamped with the current generation, so a
    /// rerun picks up exactly where this one stopped.
    Reembedded {
        rows: usize,
        errors: usize,
        cancelled: bool,
    },
    /// Deletion from the base finished (`/rag remove`): removed `chunks` fragments
    /// (0 — nothing found at the given path).
    Removed { chunks: usize },
    /// The list of knowledge-base sources (`/rag list`): a chunk count and date per
    /// source. An empty list — the base is empty.
    Listed {
        sources: Vec<crate::entities::rag::RagSourceInfo>,
    },
    /// The operation failed (an inaccessible path, no files, the embedder isn't
    /// configured, etc.).
    Failed(String),
}

/// Is the file supported by extension (case-insensitive)?
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Does the path's extension match `ext` (case-insensitive)?
fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Is it an HTML file by extension (`html`/`htm`, case-insensitive)? The `app` layer uses this
/// flag to decide whether to extract readable text (otherwise reads the content as-is).
pub fn is_html(path: &Path) -> bool {
    has_ext(path, "html") || has_ext(path, "htm")
}

/// Is it a PDF file by extension (`pdf`, case-insensitive)? The `app` layer extracts its
/// text via the `pdf-extract` crate (see `orchestrator/rag.rs`, `features/doc_extract.rs`).
pub fn is_pdf(path: &Path) -> bool {
    has_ext(path, "pdf")
}

/// Is it a DOCX file by extension (`docx`, case-insensitive)? The `app` layer extracts its
/// text (ZIP + `word/document.xml`, see `features/doc_extract.rs`).
pub fn is_docx(path: &Path) -> bool {
    has_ext(path, "docx")
}

/// Collects the list of supported files at a path:
/// - a file path → itself, if the extension is supported (otherwise empty);
/// - a directory path → all supported files (recursively when `recursive`).
///
/// The result is sorted for determinism. Errors reading nested directories are
/// skipped (the walk is resilient), while the root path being inaccessible is an error.
pub fn scan(root: &Path, recursive: bool) -> std::io::Result<Vec<PathBuf>> {
    let meta = std::fs::metadata(root)?;
    let mut out = Vec::new();
    if meta.is_file() {
        if is_supported(root) {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    collect_dir(root, recursive, &mut out);
    out.sort();
    Ok(out)
}

/// Recursive directory walk (errors on individual entries are skipped).
fn collect_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            if recursive {
                collect_dir(&path, recursive, out);
            }
        } else if ft.is_file() && is_supported(&path) {
            out.push(path);
        }
    }
}

/// A canonical string source key for writing into RAG: an absolute path with no
/// `\\?\` verbatim prefix (Windows). Used both when adding (a stable
/// `source`) and when deleting (the same key regardless of how the path was entered —
/// relatively, with a different case, or a different separator). If canonicalization
/// failed (the file is already gone from disk) — the path as-is (lossy).
pub fn canonical_source(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(abs) => strip_verbatim(abs.to_string_lossy().into_owned()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Strips the `\\?\` verbatim prefix that `canonicalize` adds on Windows
/// (so the stored/displayed path is ordinary).
fn strip_verbatim(s: String) -> String {
    match s.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// Reads a text file into a string, dropping a leading UTF-8 BOM. A non-UTF-8 or
/// unreadable file → an error (the caller skips such a file).
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let mut content = std::fs::read_to_string(path)?;
    if content.starts_with('\u{feff}') {
        content.remove(0);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn is_supported_matches_txt_and_md_case_insensitively() {
        assert!(is_supported(Path::new("a.txt")));
        assert!(is_supported(Path::new("a.MD")));
        assert!(is_supported(Path::new("dir/b.Md")));
        assert!(!is_supported(Path::new("a.rtf")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn is_supported_and_helpers_match_pdf_and_docx_case_insensitively() {
        // PDF/DOCX are supported alongside txt/md/html.
        assert!(is_supported(Path::new("doc.pdf")));
        assert!(is_supported(Path::new("dir/report.DOCX")));
        assert!(is_supported(Path::new("a.PDF")));
        // is_pdf/is_docx match exactly their own extensions (case-insensitively)...
        assert!(is_pdf(Path::new("doc.pdf")));
        assert!(is_pdf(Path::new("dir/doc.PDF")));
        assert!(is_docx(Path::new("report.docx")));
        assert!(is_docx(Path::new("dir/report.DocX")));
        // ...and don't trigger on someone else's/unsupported extensions.
        assert!(!is_pdf(Path::new("report.docx")));
        assert!(!is_pdf(Path::new("a.txt")));
        assert!(!is_docx(Path::new("doc.pdf")));
        assert!(!is_docx(Path::new("a.md")));
        assert!(!is_pdf(Path::new("noext")));
        // .doc (legacy) is deliberately not supported.
        assert!(!is_supported(Path::new("old.doc")));
        assert!(!is_docx(Path::new("old.doc")));
    }

    #[test]
    fn is_supported_and_is_html_match_html_case_insensitively() {
        // HTML is supported alongside txt/md.
        assert!(is_supported(Path::new("page.html")));
        assert!(is_supported(Path::new("page.htm")));
        assert!(is_supported(Path::new("page.HTML")));
        // is_html matches exactly HTML extensions (case-insensitively)...
        assert!(is_html(Path::new("page.html")));
        assert!(is_html(Path::new("dir/page.Htm")));
        assert!(is_html(Path::new("page.HTML")));
        // ...and doesn't trigger on other supported/unsupported extensions.
        assert!(!is_html(Path::new("a.txt")));
        assert!(!is_html(Path::new("a.md")));
        assert!(!is_html(Path::new("noext")));
    }

    #[test]
    fn scan_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        write(&file, "hello");
        let found = scan(&file, false).unwrap();
        assert_eq!(found, vec![file]);

        // An unsupported file → an empty list (not an error).
        let other = dir.path().join("data.bin");
        write(&other, "x");
        assert!(scan(&other, false).unwrap().is_empty());
    }

    #[test]
    fn scan_directory_non_recursive_skips_subdirs_and_other_exts() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "a");
        write(&dir.path().join("b.md"), "b");
        write(&dir.path().join("c.rtf"), "c"); // not supported
        write(&dir.path().join("sub/d.txt"), "d"); // in a subfolder

        let found = scan(dir.path(), false).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.md"]);
    }

    #[test]
    fn scan_directory_recursive_includes_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "a");
        write(&dir.path().join("sub/d.txt"), "d");
        write(&dir.path().join("sub/deep/e.md"), "e");

        let found = scan(dir.path(), true).unwrap();
        assert_eq!(found.len(), 3, "{found:?}");
    }

    #[test]
    fn scan_missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(&dir.path().join("nope"), false).is_err());
    }

    #[test]
    fn read_text_strips_bom() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bom.txt");
        write(&file, "\u{feff}содержимое");
        assert_eq!(read_text(&file).unwrap(), "содержимое");
    }

    #[test]
    fn canonical_source_has_no_verbatim_prefix_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.txt");
        write(&file, "x");
        let src = canonical_source(&file);
        assert!(!src.starts_with(r"\\?\"), "{src}");
        // The canonical key matches for the same file.
        assert_eq!(src, canonical_source(&file));
        // A nonexistent path → returned as-is (doesn't panic).
        let missing = dir.path().join("nope.txt");
        assert_eq!(canonical_source(&missing), missing.to_string_lossy());
    }
}
