//! **Throwaway probe code** for the code-workspace track's stage-5 go/no-go
//! (docs/code-workspace.md §3.7, fork F4): does a semantic index over the
//! attached project actually beat `code_grep` for the local model families this
//! project targets?
//!
//! It exists to answer that question and nothing else, so it takes every
//! shortcut a shipped index could not: the corpus lives in memory behind a
//! `OnceLock`, the vectors are cached in a scratch file rather than in
//! `cache.db`, there is no incremental reindex, no settings toggle and no
//! background task. Stage 0 took the same shape for the edit contract, and for
//! the same reason — if the measurement says no, almost nothing is thrown away.
//!
//! The **fair-comparison** rules the probe has to keep, or it measures nothing:
//!
//! - both arms see the same corpus, walked the same way as `code_list`/
//!   `code_grep` (`.gitignore` honoured, `require_git(false)`);
//! - both return text, not just locations — `code_grep` returns the matching
//!   line, so a search returning only `path:line` would be handicapped;
//! - the treatment arm keeps `code_grep`. The question is whether the index
//!   *adds* anything, not whether it can replace grep.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

pub const CODE_SEARCH_ID: &str = "code_search";

/// Lines per chunk, and how many of them the next chunk repeats
/// (docs/code-workspace.md §3.7: "line windows (~40 lines, ~10 overlap)").
const CHUNK_LINES: usize = 40;
const CHUNK_OVERLAP: usize = 8;
/// Hard cap on one chunk's characters — and the **binding** constraint, not a
/// safety net.
///
/// Measured twice against the real embedder. It refuses a single input over its
/// physical batch (`input (N tokens) is too large to process … current batch
/// size: 512`), and **1200 characters of code is 514 tokens** — so this project's
/// own RAG chunk size (`DEFAULT_CHUNK_MAX_CHARS`, 1200) does not carry over.
/// It is calibrated for prose, where 1200 characters is 300–400 tokens; code
/// runs about 2.3 characters per token, dense with punctuation and short
/// identifiers. 900 leaves room for the densest real files.
const CHUNK_MAX_CHARS: usize = 900;
/// Files larger than this are skipped by the indexer.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Hits returned by one search, unless the call asks for fewer.
const DEFAULT_TOP_K: usize = 8;

/// One indexed window of a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    /// Project-relative, forward slashes.
    pub path: String,
    /// 1-based line of the first line in the window.
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub vector: Vec<f32>,
}

/// The probe's whole index: every chunk of one project, in memory.
pub struct Index {
    pub chunks: Vec<Chunk>,
}

static INDEX: OnceLock<Index> = OnceLock::new();

/// Installs the index the probe's `code_search` will answer from. Called by the
/// smoke before the turn; a second call is ignored (the probe indexes once).
pub fn install(index: Index) {
    let _ = INDEX.set(index);
}

/// Splits `text` into overlapping windows, bounded by **characters first**.
///
/// A fixed line count cannot work against a hard per-input ceiling: 40 lines of
/// dense Rust is well past it while 40 lines of a sparse header is a fraction of
/// it. So a window grows until it would exceed [`CHUNK_MAX_CHARS`] or reaches
/// [`CHUNK_LINES`], and the next one starts [`CHUNK_OVERLAP`] lines back — the
/// overlap is what keeps a fact straddling a boundary retrievable, and it has to
/// be in lines because that is the unit a reader is given back.
pub fn windows(path: &str, text: &str) -> Vec<(usize, usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    // The path rides inside every embedded window: half of "where does X live"
    // is answered by the file's own name, and a vector that does not carry it
    // cannot use that.
    let header = format!("{path}\n");
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let mut body = header.clone();
        let mut end = start;
        while end < lines.len() && end - start < CHUNK_LINES {
            let next = lines[end].chars().count() + 1;
            // At least one line per window, however long that line is: a
            // 3000-character generated line would otherwise stall the walk.
            if end > start && body.chars().count() + next > CHUNK_MAX_CHARS {
                break;
            }
            body.push_str(lines[end]);
            body.push('\n');
            end += 1;
        }
        out.push((start + 1, end, body));
        if end >= lines.len() {
            break;
        }
        // Step back for the overlap, but always forward overall.
        start = (end.saturating_sub(CHUNK_OVERLAP)).max(start + 1);
    }
    out
}

/// Walks `root` and cuts every text file into windows — the corpus, before it
/// is embedded.
pub fn corpus(root: &Path) -> Vec<(String, usize, usize, String)> {
    let mut out = Vec::new();
    for entry in super::code::probe_walker(root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        for (start, end, body) in windows(&rel, &text) {
            out.push((rel.clone(), start, end, body));
        }
    }
    out
}

/// Cosine similarity. The embedder returns normalized vectors, but the probe
/// does not depend on that — a normalization change elsewhere must not silently
/// turn this into a dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// The probe's `code_search`.
#[derive(Debug, Clone, Copy)]
pub struct CodeSearch;

#[async_trait::async_trait]
impl Tool for CodeSearch {
    fn id(&self) -> ToolId {
        CODE_SEARCH_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "search the project by meaning"
    }
    fn counts_toward_round_limit(&self) -> bool {
        false
    }
    fn enabled_by_default(&self) -> bool {
        false // the probe enables it explicitly, like stage 0's three
    }
    fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
        "Find where something is handled in the attached project by describing it \
         in your own words, rather than by guessing the exact identifier. Returns \
         the most relevant passages as `path:from-to` followed by their text. Use \
         it when you do not know what the code calls the thing you are looking \
         for; use code_grep when you do."
            .to_string()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What you are looking for, in your own words"},
                "top_k": {"type": "integer", "description": "How many passages to return"}
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let Some(index) = INDEX.get() else {
            anyhow::bail!("the project is not indexed");
        };
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if query.is_empty() {
            anyhow::bail!("a query is required");
        }
        let top_k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|k| k as usize)
            .unwrap_or(DEFAULT_TOP_K)
            .clamp(1, 20);
        let vector = ctx
            .embedder
            .embed(
                vec![query.clone()],
                crate::shared::api::contract::EmbedRole::Query,
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("the embedder returned nothing"))?;

        let mut scored: Vec<(f32, &Chunk)> = index
            .chunks
            .iter()
            .map(|c| (cosine(&vector, &c.vector), c))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out = format!("Passages matching “{query}”:\n");
        for (score, chunk) in scored.into_iter().take(top_k) {
            out.push_str(&format!(
                "\n{}:{}-{} (score {score:.2})\n{}\n",
                chunk.path, chunk.start, chunk.end, chunk.text
            ));
        }
        Ok(ToolOutcome::text(out))
    }
}

/// Where the probe caches its vectors between runs.
///
/// Embedding the whole project takes minutes; the measurement needs the same
/// corpus for both arms and across repeats, so it is paid once. Keyed by the
/// corpus's own digest, so a source change invalidates it.
pub fn cache_path(digest: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mindfork-code-search-probe-{digest}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_overlap_and_cover_every_line() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let w = windows("a.rs", &text);
        assert!(w.len() > 1, "100 lines must not be one window");
        assert_eq!(w[0].0, 1);
        assert_eq!(w[0].1, CHUNK_LINES, "short lines fill the line budget");
        // The second window starts inside the first — that overlap is what keeps
        // a fact that straddles a boundary retrievable.
        assert!(w[1].0 <= CHUNK_LINES, "no overlap: {:?}", &w[..2]);
        assert_eq!(w.last().unwrap().1, 100, "the tail must be covered");
        // Every chunk carries its path, so the vector knows where it came from.
        assert!(w.iter().all(|(_, _, body)| body.starts_with("a.rs\n")));
    }

    /// The binding constraint, measured against the real server: a single input
    /// over its physical batch is refused outright, so no window may exceed the
    /// character cap — whatever the lines look like.
    #[test]
    fn no_window_exceeds_the_character_cap() {
        let dense: String = (1..=200)
            .map(|i| format!("{} // {}\n", "x".repeat(100), i))
            .collect();
        let w = windows("dense.rs", &dense);
        assert!(w.len() > 1);
        for (start, end, body) in &w {
            assert!(
                body.chars().count() <= CHUNK_MAX_CHARS + 120,
                "{start}-{end} is {} chars",
                body.chars().count()
            );
            assert!(end > start, "an empty window at {start}");
        }
        // …and the walk still terminates, covering the file.
        assert_eq!(w.last().unwrap().1, 200);
    }

    /// One line longer than the whole budget still becomes a window rather than
    /// stalling the walk — a generated or minified file is the normal case.
    #[test]
    fn a_single_overlong_line_still_advances() {
        let text = format!("{}\nshort\n", "y".repeat(CHUNK_MAX_CHARS * 3));
        let w = windows("min.js", &text);
        assert!(!w.is_empty());
        assert_eq!(w.last().unwrap().1, 2, "the file must be covered");
    }

    #[test]
    fn cosine_is_one_for_identical_and_zero_for_orthogonal() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
