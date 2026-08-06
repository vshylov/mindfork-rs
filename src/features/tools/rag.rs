//! Knowledge-base (RAG) tools: `rag_add`, `rag_search`. Chunking → embedding
//! (a dedicated server, ADR 0002) → write/kNN in sqlite-vec, **isolation by
//! `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::entities::rag::{RagDocument, RagHit};

use crate::shared::api::EmbedRole;
use crate::shared::config::{
    DEFAULT_CHUNK_MAX_CHARS, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_TARGET_CHARS, RagSettings,
};

use super::{Tool, ToolContext, ToolOutcome};

/// Minimum length of a verbatim match to stitch adjacent chunks together on
/// retrieval (shorter — likely coincidence, not the built-in overlap).
const MIN_STITCH_OVERLAP: usize = 24;
/// Default top-K for search.
const DEFAULT_TOP_K: usize = 5;

/// Chunking parameters (sizes in characters). Configurable via settings
/// (`config.rag`, see spec §9.3): passed into [`chunk_text`]/[`chunk_markdown`]
/// instead of the previously hardcoded constants. `Default` matches the former values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    /// Target ("soft") chunk size — units are packed up to it.
    pub target: usize,
    /// Overlap of adjacent chunks: the previous chunk's tail repeats at the start
    /// of the next one. RAG best practice — a query near a chunk boundary doesn't
    /// lose context (and duplication is removed on retrieval by stitching, see
    /// [`stitch_hits`]).
    pub overlap: usize,
    /// Hard ceiling for an indivisible run (a very long word/line with no punctuation).
    pub max: usize,
}

impl Default for ChunkParams {
    fn default() -> Self {
        Self {
            target: DEFAULT_CHUNK_TARGET_CHARS,
            overlap: DEFAULT_CHUNK_OVERLAP_CHARS,
            max: DEFAULT_CHUNK_MAX_CHARS,
        }
    }
}

impl ChunkParams {
    /// Parameters from RAG settings (`config.rag`). Invalid values (a zero target
    /// size) are replaced with the default, so the chunker doesn't loop/return empty.
    pub fn from_settings(rag: &RagSettings) -> Self {
        let target = if rag.chunk_target_chars == 0 {
            DEFAULT_CHUNK_TARGET_CHARS
        } else {
            rag.chunk_target_chars
        };
        // The ceiling can't be smaller than the target — otherwise the target
        // packing is impossible.
        let max = rag.chunk_max_chars.max(target);
        Self {
            target,
            overlap: rag.chunk_overlap_chars.min(target.saturating_sub(1)),
            max,
        }
    }
}

/// `rag_add` — adds text to the knowledge base (chunking + embedding). Returns the
/// number of chunks written.
pub struct RagAdd;

#[async_trait::async_trait]
impl Tool for RagAdd {
    fn id(&self) -> ToolId {
        "rag_add".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "add to knowledge base"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.rag_add.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"},
                "source": {"type": "string", "description": loc.t("tool.rag_add.param.source")}
            },
            "required": ["text"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.rag_add.err.text_string")))?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| ctx.loc.t("tool.rag_add.no_source").to_string());

        let chunks = chunk_text(text, ctx.chunk_params);
        if chunks.is_empty() {
            anyhow::bail!(ctx.loc.t("tool.rag_add.err.no_content"));
        }
        let embeddings = ctx
            .embedder
            .embed(chunks.clone(), EmbedRole::Passage)
            .await?;
        if embeddings.len() != chunks.len() {
            anyhow::bail!(ctx.loc.t("tool.rag_add.err.embed_count"));
        }
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            let doc = RagDocument::new(ctx.profile_id, &source, chunk, embedding);
            ctx.storage.db().rag_insert(&doc)?;
        }
        // Save the source text for possible reindexing (`/rag rebuild`). The tool
        // accumulates the source's chunks — so we append, not replace.
        ctx.storage
            .db()
            .rag_source_append(ctx.profile_id, &source, text, chrono::Utc::now())?;
        Ok(ToolOutcome::text(ctx.loc.tf(
            "tool.rag_add.result.added",
            &[("n", &chunks.len().to_string())],
        )))
    }
}

/// `rag_search` — semantic search over the profile's knowledge base.
pub struct RagSearch;

#[async_trait::async_trait]
impl Tool for RagSearch {
    fn id(&self) -> ToolId {
        "rag_search".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "search knowledge base"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.rag_search.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "minimum": 1}
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.rag_search.err.query_empty")))?;
        let k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_TOP_K);

        // The knowledge base was indexed by a different embedding model, so its
        // vectors live in another vector space and searching them returns noise
        // with no error of its own (docs/research/embedding-model-change-reindex.md).
        // Refuse plainly instead — and say what fixes it, since only the user can
        // run `/reindex`. Unlike notes and attachments, the knowledge base is
        // never retired silently: it is the user's own data.
        if ctx
            .storage
            .db()
            .rag_is_stale(ctx.profile_id)
            .unwrap_or(false)
        {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.rag_search.err.stale")));
        }

        let mut embeddings = ctx
            .embedder
            .embed(vec![query.to_string()], EmbedRole::Query)
            .await?;
        let query_vec = embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("tool.rag_search.err.no_query_vec")))?;
        let hits = ctx.storage.db().rag_search(ctx.profile_id, &query_vec, k)?;
        if hits.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.rag_search.result.empty")));
        }
        // Stitch adjacent chunks of the same source (by the built-in overlap):
        // saves context and doesn't confuse the model with a repeat (see
        // [`stitch_hits`]). Then remove near-identical passages from DIFFERENT
        // sources (a repeat of the same content), so the model isn't fed a
        // duplicate; ranking order is preserved (see [`dedup_passages`]).
        let passages = dedup_passages(stitch_hits(hits));
        let mut out = ctx.loc.tf(
            "tool.rag_search.result.header",
            &[("n", &passages.len().to_string())],
        );
        // Numbered, each on its own line, separated by a blank one — a passage is
        // a whole chunk (or several stitched together) and is routinely
        // multi-line, so a single leading `- [source] ` marker left its
        // continuation unmarked and passages ran together. The number separates,
        // it doesn't address: no tool takes a passage index (notes below carry
        // real ids, which is why they stay a plain `-` list).
        for (i, p) in passages.iter().enumerate() {
            out.push_str(&format!("\n\n{}. [{}]\n{}", i + 1, p.source, p.text));
        }
        out.push('\n');
        // The reverse direction (Tier 3, Path 3): notes/observations citing the
        // found sources — "search through both organs". Self-observations are
        // marked [about self] (the organs stay distinguishable). Dedup notes by id.
        let mut seen: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
        let mut linked: Vec<String> = Vec::new();
        for p in &passages {
            let notes = ctx
                .storage
                .db()
                .notes_citing_source(ctx.profile_id, &p.source)
                .unwrap_or_default();
            for n in notes {
                if !seen.insert(n.id) {
                    continue;
                }
                let mark = if super::notes::is_self_note(&n) {
                    format!("{} ", ctx.loc.t("notes.mark.self"))
                } else {
                    String::new()
                };
                linked.push(format!("- {mark}(id={}) {}", n.id, n.content));
            }
        }
        if !linked.is_empty() {
            out.push('\n');
            out.push_str(ctx.loc.t("tool.rag_search.block.linked_notes"));
            out.push_str(":\n");
            out.push_str(&linked.join("\n"));
            out.push('\n');
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}

/// Length of a string in characters (not bytes — correct for Cyrillic/Unicode).
fn clen(s: &str) -> usize {
    s.chars().count()
}

/// Cuts arbitrary text into chunks with overlap along sentence/word boundaries
/// (RAG best practice). `pub(crate)` — reused by background file indexing
/// (`/rag add`) and the `rag_add` tool. For markdown there's [`chunk_markdown`].
///
/// Algorithm: the text is segmented into atomic units (a whole paragraph if it
/// fits the target; otherwise sentences; too-long sentences — word windows, and a
/// single gigantic word — by character), then the units are packed into chunks up
/// to `CHUNK_TARGET_CHARS`, and each next chunk starts with the tail of the
/// previous one (overlap ≤ `CHUNK_OVERLAP_CHARS`). Small neighboring paragraphs are
/// grouped into one chunk this way (rather than spawning tiny line-chunks).
pub(crate) fn chunk_text(text: &str, params: ChunkParams) -> Vec<String> {
    let units = segment_units(text, params);
    pack_units(&units, params.target, params.overlap)
}

/// Semantic markdown chunking: cuts on ATX headings (`#`..`######`), protects
/// fenced code blocks (``` and ~~~), and prepends each section chunk with its
/// heading as a semantic anchor (noticeably improves retrieval). Inside a section —
/// the same overlap packer as in [`chunk_text`]. A document with no headings is
/// treated as regular text.
pub(crate) fn chunk_markdown(text: &str, params: ChunkParams) -> Vec<String> {
    let sections = split_sections(text);
    let mut chunks = Vec::new();
    for (heading, body) in &sections {
        let units = segment_units(body, params);
        let packed = if units.is_empty() {
            // A section with no body — the heading itself as a chunk (if present).
            vec![String::new()]
        } else {
            pack_units(&units, params.target, params.overlap)
        };
        for p in packed {
            let chunk = match (heading.is_empty(), p.is_empty()) {
                (true, _) => p,
                (false, true) => heading.clone(),
                (false, false) => format!("{heading}\n{p}"),
            };
            let chunk = chunk.trim();
            if !chunk.is_empty() {
                chunks.push(chunk.to_string());
            }
        }
    }
    if chunks.is_empty() {
        // No headings / an empty document — regular chunking.
        return chunk_text(text, params);
    }
    chunks
}

/// Atomic units for packing (see [`chunk_text`]). Empty ones are dropped.
fn segment_units(text: &str, params: ChunkParams) -> Vec<String> {
    let mut units = Vec::new();
    for paragraph in text.split("\n\n") {
        let p = paragraph.trim();
        if p.is_empty() {
            continue;
        }
        if clen(p) <= params.target {
            units.push(p.to_string());
            continue;
        }
        for sentence in split_sentences(p) {
            if clen(&sentence) <= params.max {
                units.push(sentence);
            } else {
                units.extend(break_long(&sentence, params.max));
            }
        }
    }
    units
}

/// Splits a paragraph into sentences by terminal punctuation (`. ! ? …` and their
/// CJK equivalents), keeping it. A boundary is punctuation followed by a
/// space/end.
pub(crate) fn split_sentences(paragraph: &str) -> Vec<String> {
    let chars: Vec<char> = paragraph.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let is_end = matches!(chars[i], '.' | '!' | '?' | '…' | '。' | '！' | '？');
        let next_ws = chars.get(i + 1).map(|c| c.is_whitespace()).unwrap_or(true);
        if is_end && next_ws {
            let seg: String = chars[start..=i].iter().collect();
            let seg = seg.trim();
            if !seg.is_empty() {
                out.push(seg.to_string());
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            start = j;
            i = j;
            continue;
        }
        i += 1;
    }
    if start < chars.len() {
        let seg: String = chars[start..].iter().collect();
        let seg = seg.trim();
        if !seg.is_empty() {
            out.push(seg.to_string());
        }
    }
    out
}

/// Splits a too-long line (with no terminal punctuation) into word windows; a
/// single word longer than the ceiling is torn by character (a last resort).
fn break_long(s: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in s.split_whitespace() {
        let wlen = clen(word);
        if wlen > max {
            flush_window(&mut cur, &mut cur_len, &mut out);
            out.extend(tear_word(word, max));
            continue;
        }
        let add = if cur.is_empty() { wlen } else { wlen + 1 };
        if cur_len + add > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_len = wlen;
        } else {
            if !cur.is_empty() {
                cur.push(' ');
                cur_len += 1;
            }
            cur.push_str(word);
            cur_len += wlen;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Pushes the current word window into `out` and resets it (a no-op when empty).
fn flush_window(cur: &mut String, cur_len: &mut usize, out: &mut Vec<String>) {
    if !cur.is_empty() {
        out.push(std::mem::take(cur));
        *cur_len = 0;
    }
}

/// Tears a single word longer than the ceiling by character (a last resort).
fn tear_word(word: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    chars.chunks(max).map(|w| w.iter().collect()).collect()
}

/// Packs units into chunks up to the target size, starting each next one with the
/// tail of the previous one (overlap ≤ `overlap` characters, on unit boundaries).
fn pack_units(units: &[String], target: usize, overlap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_len = 0usize;
    for unit in units {
        let ulen = clen(unit);
        let add = if cur.is_empty() { ulen } else { ulen + 1 };
        if !cur.is_empty() && cur_len + add > target {
            chunks.push(cur.join("\n"));
            let (tail, tlen) = overlap_tail(&cur, overlap);
            cur = tail;
            cur_len = tlen;
        }
        if !cur.is_empty() {
            cur_len += 1;
        }
        cur.push(unit);
        cur_len += ulen;
    }
    if !cur.is_empty() {
        chunks.push(cur.join("\n"));
    }
    chunks
}

/// The tail for overlap: the last units within `overlap` characters
/// (at least one — otherwise the packing loop wouldn't advance). Returns the
/// tail (in original order) and its length in characters.
fn overlap_tail<'a>(cur: &[&'a str], overlap: usize) -> (Vec<&'a str>, usize) {
    let mut tail: Vec<&str> = Vec::new();
    let mut tlen = 0usize;
    for &u in cur.iter().rev() {
        let a = if tail.is_empty() {
            clen(u)
        } else {
            clen(u) + 1
        };
        if tlen + a > overlap && !tail.is_empty() {
            break;
        }
        tail.push(u);
        tlen += a;
    }
    tail.reverse();
    (tail, tlen)
}

/// Splits markdown into `(heading, body)` sections by ATX headings, without
/// touching `#` inside fenced code blocks. A preamble before the first heading →
/// `("", …)`.
fn split_sections(text: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(f) {
                fence = None;
            }
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if let Some(f) = fence_open(trimmed) {
            fence = Some(f);
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if is_atx_heading(trimmed) {
            flush_section(&mut sections, &mut heading, &mut body);
            heading = trimmed.trim_end().to_string();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush_section(&mut sections, &mut heading, &mut body);
    sections
}

/// The fence delimiter a line opens (``` or ~~~), if any.
fn fence_open(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Pushes the accumulated `(heading, body)` section, if it has any content, and
/// resets both accumulators.
fn flush_section(sections: &mut Vec<(String, String)>, heading: &mut String, body: &mut String) {
    if !heading.is_empty() || !body.trim().is_empty() {
        sections.push((std::mem::take(heading), std::mem::take(body)));
    }
}

/// Is this a markdown ATX-heading line (`#`..`######` + a space)?
fn is_atx_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// A coherent extraction fragment — one or several chunks of the same source
/// stitched together by overlap (see [`stitch_hits`]).
pub(crate) struct StitchedPassage {
    pub source: String,
    pub text: String,
    pub distance: f32,
}

/// Stitches adjacent chunks of the same source when the end of one verbatim-
/// matches the start of another (the overlap built in at chunking time): merges
/// them into one coherent fragment with no duplication — saves context and
/// doesn't confuse the model with a repeat. Fragments are ordered by best
/// (minimum) distance.
pub(crate) fn stitch_hits(hits: Vec<RagHit>) -> Vec<StitchedPassage> {
    // Group by source, preserving the order of first appearance.
    let mut by_source: Vec<(String, Vec<(String, f32)>)> = Vec::new();
    for h in hits {
        match by_source.iter_mut().find(|(s, _)| *s == h.source) {
            Some(g) => g.1.push((h.chunk_text, h.distance)),
            None => by_source.push((h.source, vec![(h.chunk_text, h.distance)])),
        }
    }
    let mut passages = Vec::new();
    for (source, mut items) in by_source {
        // Iteratively stitch any two pieces with a real overlap.
        while let Some((i, j, text, dist)) = find_mergeable(&items) {
            let (hi, lo) = (i.max(j), i.min(j));
            items.remove(hi);
            items.remove(lo);
            items.push((text, dist));
        }
        for (text, distance) in items {
            passages.push(StitchedPassage {
                source: source.clone(),
                text,
                distance,
            });
        }
    }
    passages.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    passages
}

/// Removes near-identical passages (usually the same content indexed from
/// DIFFERENT sources): in ranking order (ascending distance), keeps a passage only
/// if its normalized text is NEITHER equal to NOR wholly contained in the text of
/// an already-kept passage. This way the model isn't fed a repeat. Deterministic,
/// no embedder/threshold (dedup by text — rag_search is a hot path). Source-
/// agnostic (the main case is a cross-source duplicate, but an intra-source repeat
/// is noise too). See docs/history/rag-sources-retrieval.md §B2b.
pub(crate) fn dedup_passages(passages: Vec<StitchedPassage>) -> Vec<StitchedPassage> {
    let mut kept: Vec<StitchedPassage> = Vec::with_capacity(passages.len());
    let mut kept_norms: Vec<String> = Vec::with_capacity(passages.len());
    for p in passages {
        let norm = normalize_passage(&p.text);
        // Drop if equal to an already-kept (higher-ranked) one or wholly contained
        // in it (the current passage is a redundant subset of a more relevant one).
        if kept_norms.iter().any(|k| k.contains(&norm)) {
            continue;
        }
        kept_norms.push(norm);
        kept.push(p);
    }
    kept
}

/// Normalizes passage text for comparison: lowercase (Unicode-aware, works for
/// Cyrillic too) + collapsing any whitespace run into a single space + trim.
fn normalize_passage(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Finds the first pair of pieces `(i, j)` that can be stitched (the end of `i`
/// matches the start of `j`); returns the indices, the merged text, and the best
/// distance.
fn find_mergeable(items: &[(String, f32)]) -> Option<(usize, usize, String, f32)> {
    for i in 0..items.len() {
        for j in 0..items.len() {
            if i == j {
                continue;
            }
            if let Some(text) = merge_overlap(&items[i].0, &items[j].0) {
                return Some((i, j, text, items[i].1.min(items[j].1)));
            }
        }
    }
    None
}

/// If the end of `a` verbatim-matches the start of `b` (≥ `MIN_STITCH_OVERLAP`
/// chars), returns `a` + the tail of `b` with no repeat. Accounts for a repeated
/// markdown heading at the start of `b` (the same as `a`'s): strips it before
/// matching and doesn't duplicate it.
fn merge_overlap(a: &str, b: &str) -> Option<String> {
    if let Some(k) = overlap_len(a, b) {
        let tail: String = b.chars().skip(k).collect();
        return Some(format!("{a}{tail}"));
    }
    // `b` starts with the same heading as `a` — match the bodies.
    if let (Some(ha), Some((hb, rest_b))) = (leading_heading(a), strip_leading_heading(b))
        && ha == hb
        && let Some(k) = overlap_len(a, &rest_b)
    {
        let tail: String = rest_b.chars().skip(k).collect();
        return Some(format!("{a}{tail}"));
    }
    None
}

/// Length of the longest suffix of `a` that equals a prefix of `b` (≥ `MIN_STITCH_OVERLAP`).
fn overlap_len(a: &str, b: &str) -> Option<usize> {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let max = ac.len().min(bc.len());
    let mut k = max;
    while k >= MIN_STITCH_OVERLAP {
        if ac[ac.len() - k..] == bc[..k] {
            return Some(k);
        }
        k -= 1;
    }
    None
}

/// The leading ATX-heading line (if the first line is a heading).
fn leading_heading(s: &str) -> Option<String> {
    let first = s.lines().next()?;
    is_atx_heading(first.trim_start()).then(|| first.trim_end().to_string())
}

/// Strips the leading ATX heading: returns `(heading, remainder)`.
fn strip_leading_heading(s: &str) -> Option<(String, String)> {
    let mut lines = s.splitn(2, '\n');
    let first = lines.next()?;
    if is_atx_heading(first.trim_start()) {
        Some((
            first.trim_end().to_string(),
            lines.next().unwrap_or("").to_string(),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ctx_with_storage;
    use super::*;
    use uuid::Uuid;

    /// Is there no Cyrillic in the string (a proxy for "translated to en").
    fn no_cyr(s: &str) -> bool {
        !s.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
    }

    #[test]
    fn rag_tool_descriptions_are_localized() {
        // RAG tool descriptions differ on ru/en (catches a forgotten `_loc`), en
        // has no Cyrillic. §3.5 docs/history/i18n.md.
        use crate::shared::i18n::{Lang, locale};
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        for (r, e) in [
            (RagAdd.description(ru), RagAdd.description(en)),
            (RagSearch.description(ru), RagSearch.description(en)),
        ] {
            assert_ne!(r, e, "description not localized: {r}");
            assert!(no_cyr(&e), "Cyrillic in en description: {e}");
        }
    }

    #[tokio::test]
    async fn rag_add_result_localized_for_all_langs() {
        // "chunks added" confirmation renders in every built-in language.
        use crate::shared::i18n::{Lang, locale};
        for &lang in Lang::ALL {
            let (_d, _s, mut ctx) = ctx_with_storage(Uuid::new_v4());
            ctx.loc = locale(lang);
            let out = RagAdd
                .invoke(&ctx, serde_json::json!({"text": "hello world alpha beta"}))
                .await
                .unwrap();
            let prefix = locale(lang)
                .t("tool.rag_add.result.added")
                .split("{n}")
                .next()
                .unwrap();
            assert!(out.result.starts_with(prefix), "{lang:?}: {}", out.result);
        }
    }

    #[test]
    fn chunking_groups_small_paragraphs() {
        // Small neighboring paragraphs are grouped into one chunk (not spawning tiny ones).
        let chunks = chunk_text(
            "первый абзац\n\nвторой абзац\n\nтретий абзац",
            ChunkParams::default(),
        );
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert!(chunks[0].contains("первый абзац"));
        assert!(chunks[0].contains("третий абзац"));
    }

    #[test]
    fn chunking_splits_long_paragraph_with_overlap() {
        // A long paragraph made of sentences is cut into several chunks with overlap.
        let sentence = "Это предложение средней длины для проверки чанкинга. ";
        let text = sentence.repeat(60); // ~3000 characters
        let params = ChunkParams::default();
        let chunks = chunk_text(&text, params);
        assert!(
            chunks.len() >= 2,
            "expected several chunks: {}",
            chunks.len()
        );
        // Every chunk stays within reasonable bounds (ceiling + overlap).
        for c in &chunks {
            assert!(clen(c) <= params.max + params.overlap, "{}", clen(c));
        }
        // Overlap: the end of the first chunk verbatim-occurs at the start of the second.
        assert!(
            overlap_len(&chunks[0], &chunks[1]).is_some(),
            "expected an overlap between adjacent chunks"
        );
    }

    #[test]
    fn chunking_never_breaks_mid_word() {
        // A very long "word" (with no spaces) gets torn, but regular words stay whole.
        let text = format!("короткое начало {} конец", "ё".repeat(2500));
        let chunks = chunk_text(&text, ChunkParams::default());
        assert!(chunks.iter().any(|c| c.contains("короткое начало")));
        assert!(chunks.iter().any(|c| c.contains("конец")));
    }

    #[test]
    fn chunk_params_from_settings_respects_config() {
        // A smaller target size cuts the same text into more chunks.
        let text = "Это предложение средней длины для проверки чанкинга. ".repeat(20);
        let big = chunk_text(&text, ChunkParams::default());
        let small = chunk_text(
            &text,
            ChunkParams::from_settings(&RagSettings {
                chunk_target_chars: 200,
                chunk_overlap_chars: 40,
                chunk_max_chars: 400,
            }),
        );
        assert!(
            small.len() > big.len(),
            "a smaller target → more chunks: small={} big={}",
            small.len(),
            big.len()
        );
    }

    #[test]
    fn chunk_params_from_settings_sanitizes_invalid() {
        // A zero target is replaced with the default; overlap doesn't exceed target.
        let p = ChunkParams::from_settings(&RagSettings {
            chunk_target_chars: 0,
            chunk_overlap_chars: 9999,
            chunk_max_chars: 10,
        });
        assert_eq!(p.target, DEFAULT_CHUNK_TARGET_CHARS);
        assert!(p.overlap < p.target);
        assert!(p.max >= p.target, "ceiling not smaller than target");
    }

    #[test]
    fn markdown_chunks_carry_their_heading() {
        let md = "# Заголовок\n\nтекст раздела один\n\n## Подраздел\n\nтекст подраздела";
        let chunks = chunk_markdown(md, ChunkParams::default());
        assert!(chunks.iter().any(|c| c.starts_with("# Заголовок")));
        assert!(chunks.iter().any(|c| c.starts_with("## Подраздел")));
        // Every chunk starts with its own heading (a semantic anchor).
        assert!(chunks.iter().all(|c| c.starts_with('#')));
    }

    #[test]
    fn markdown_ignores_hash_inside_code_fence() {
        let md = "# Реальный заголовок\n\n```python\n# это комментарий, не заголовок\nx = 1\n```";
        let sections = split_sections(md);
        // One heading-section (a comment inside the code didn't become a heading).
        assert_eq!(sections.len(), 1, "{sections:?}");
        assert_eq!(sections[0].0, "# Реальный заголовок");
        assert!(sections[0].1.contains("# это комментарий"));
    }

    #[test]
    fn stitch_merges_overlapping_neighbors() {
        let mk = |text: &str, d: f32| RagHit {
            id: Uuid::new_v4(),
            source: "doc.md".into(),
            chunk_text: text.into(),
            distance: d,
        };
        // The end of A verbatim-matches the start of B (≥ MIN_STITCH_OVERLAP characters).
        let a = "альфа бета гамма дельта эпсилон дзета";
        let b = "гамма дельта эпсилон дзета эта тета йота";
        let merged = stitch_hits(vec![mk(a, 0.2), mk(b, 0.3)]);
        assert_eq!(merged.len(), 1, "should stitch into one fragment");
        assert_eq!(
            merged[0].text,
            "альфа бета гамма дельта эпсилон дзета эта тета йота"
        );
        assert_eq!(merged[0].distance, 0.2, "the best distance is taken");
    }

    #[test]
    fn stitch_keeps_unrelated_hits_separate() {
        let mk = |src: &str, text: &str| RagHit {
            id: Uuid::new_v4(),
            source: src.into(),
            chunk_text: text.into(),
            distance: 0.5,
        };
        let out = stitch_hits(vec![
            mk("a.txt", "совершенно разный текст один"),
            mk("b.txt", "никак не связанный текст два"),
        ]);
        assert_eq!(out.len(), 2, "different sources don't stitch");
    }

    fn passage(source: &str, text: &str, distance: f32) -> StitchedPassage {
        StitchedPassage {
            source: source.into(),
            text: text.into(),
            distance,
        }
    }

    #[test]
    fn dedup_drops_identical_from_different_sources_keeps_higher_ranked() {
        let out = dedup_passages(vec![
            passage("a.txt", "столица франции — париж", 0.1),
            passage("b.txt", "столица франции — париж", 0.3),
        ]);
        assert_eq!(
            out.len(),
            1,
            "an identical duplicate from another source is removed"
        );
        assert_eq!(
            out[0].source, "a.txt",
            "the more relevant one (smaller distance) remains"
        );
    }

    #[test]
    fn dedup_drops_passage_contained_in_higher_ranked() {
        let out = dedup_passages(vec![
            passage("a.txt", "полный текст с деталями про париж и францию", 0.1),
            passage("b.txt", "париж и францию", 0.4),
        ]);
        assert_eq!(
            out.len(),
            1,
            "a subset of the more relevant passage is removed"
        );
        assert_eq!(out[0].source, "a.txt");
    }

    #[test]
    fn dedup_keeps_distinct_passages_in_order() {
        let out = dedup_passages(vec![
            passage("a.txt", "первый совершенно уникальный текст", 0.1),
            passage("b.txt", "второй никак не связанный текст", 0.2),
        ]);
        assert_eq!(out.len(), 2, "distinct passages are left alone");
        assert_eq!(out[0].source, "a.txt");
        assert_eq!(out[1].source, "b.txt");
    }

    #[test]
    fn dedup_is_whitespace_and_case_insensitive() {
        let out = dedup_passages(vec![
            passage("a.txt", "Столица  Франции —\nПариж", 0.1),
            passage("b.txt", "столица франции — париж", 0.3),
        ]);
        assert_eq!(out.len(), 1, "equal up to case/whitespace → dedup");
        assert_eq!(out[0].source, "a.txt");
    }

    #[test]
    fn dedup_keeps_lower_ranked_superset() {
        // A (higher-ranked) is contained in B (lower-ranked): only a passage
        // contained in an EARLIER-kept one is dropped, so both survive (no loss
        // of information).
        let out = dedup_passages(vec![
            passage("a.txt", "париж", 0.1),
            passage("b.txt", "париж — столица франции и крупный город", 0.3),
        ]);
        assert_eq!(out.len(), 2, "a lower-ranked superset isn't removed");
        assert_eq!(out[0].source, "a.txt");
        assert_eq!(out[1].source, "b.txt");
    }

    #[test]
    fn dedup_empty_input_empty_output() {
        assert!(dedup_passages(Vec::new()).is_empty());
    }

    #[tokio::test]
    async fn add_then_search_returns_relevant_chunk() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({
                    "text": "кошки любят рыбу\n\nсобаки любят кости",
                    "source": "факты"
                }),
            )
            .await
            .unwrap();

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки рыба", "top_k": 1}))
            .await
            .unwrap();
        assert!(
            out.result.contains("кошки любят рыбу"),
            "got: {}",
            out.result
        );
        assert!(out.result.contains("факты"));
    }

    /// A knowledge base indexed by a previous embedding model cannot be searched:
    /// its vectors are in another space, so kNN returns plausible-looking noise
    /// with no error of its own. The tool must refuse and name the fix, rather
    /// than hand the model garbage (docs/research/embedding-model-change-reindex.md).
    #[tokio::test]
    async fn search_refuses_on_a_stale_knowledge_base() {
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({"text": "кошки любят рыбу", "source": "факты"}),
            )
            .await
            .unwrap();

        // Healthy base — the passage is found.
        let ok = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки"}))
            .await
            .unwrap();
        assert!(ok.result.contains("кошки любят рыбу"), "got: {}", ok.result);

        // The embedding model changed (recorded by the guard).
        storage.db().set_rag_stale_profiles(&[profile]).unwrap();
        let stale = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки"}))
            .await
            .unwrap();
        assert!(
            !stale.result.contains("кошки любят рыбу"),
            "a stale base must not return passages: {}",
            stale.result
        );
        assert!(
            stale.result.contains("/reindex"),
            "the refusal must name the fix: {}",
            stale.result
        );

        // Reindexing lifts the refusal.
        storage.db().clear_rag_stale_profile(profile).unwrap();
        let healed = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки"}))
            .await
            .unwrap();
        assert!(healed.result.contains("кошки любят рыбу"));
    }

    /// A passage is a whole chunk (or several stitched together) and is routinely
    /// multi-line, so the result has to keep the boundaries visible — otherwise
    /// passages run together and neither the reader nor the model can tell where
    /// one ends.
    #[tokio::test]
    async fn search_numbers_passages_and_separates_them() {
        let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
        for (text, source) in [
            ("кошки любят рыбу\nи спят на солнце", "про-кошек"),
            ("собаки любят кости\nи гулять во дворе", "про-собак"),
        ] {
            RagAdd
                .invoke(&ctx, serde_json::json!({"text": text, "source": source}))
                .await
                .unwrap();
        }

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "любят", "top_k": 5}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("1. ["), "{out}");
        assert!(out.contains("2. ["), "{out}");
        // The passage text starts on its own line, and passages are set apart by
        // a blank line.
        assert!(
            out.contains("]\nкошки любят рыбу") || out.contains("]\nсобаки любят кости"),
            "{out}"
        );
        assert!(
            out.contains("\n\n2. ["),
            "passages must be separated by a blank line: {out}"
        );
    }

    #[tokio::test]
    async fn search_surfaces_notes_citing_matched_source() {
        // Tier 3, Path 3 (the reverse direction): rag_search shows notes citing the
        // found source — "search through both organs".
        use crate::entities::note::Note;
        let profile = Uuid::new_v4();
        let (_d, storage, ctx) = ctx_with_storage(profile);
        RagAdd
            .invoke(
                &ctx,
                serde_json::json!({"text": "кошки любят рыбу", "source": "факты"}),
            )
            .await
            .unwrap();
        let note = Note::new(profile, "мой вывод о кошках", vec![]);
        let nid = note.id;
        storage.db().note_insert(&note).unwrap();
        storage
            .db()
            .note_cite_source_insert(profile, nid, "факты")
            .unwrap();

        let out = RagSearch
            .invoke(&ctx, serde_json::json!({"query": "кошки рыба", "top_k": 1}))
            .await
            .unwrap();
        assert!(out.result.contains("Заметки со ссылкой на эти источники"));
        assert!(out.result.contains("мой вывод о кошках"));
    }

    #[tokio::test]
    async fn search_isolated_by_profile() {
        // Profile A's documents shouldn't be found when profile B searches
        // (different profiles in one storage).
        let dir = tempfile::tempdir().unwrap();
        let storage = std::sync::Arc::new(
            crate::shared::storage::Storage::open_in_memory(
                crate::shared::paths::Paths::with_root(dir.path()),
            )
            .unwrap(),
        );
        let engine: std::sync::Arc<dyn crate::shared::api::EngineBackend> =
            std::sync::Arc::new(crate::shared::api::mock::MockBackend::scripted(vec![]));
        let embedder: std::sync::Arc<dyn crate::shared::api::Embedder> =
            std::sync::Arc::new(crate::shared::api::mock::MockEmbedder::new(16));

        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        // A shared dependency bundle: both profiles share one storage (checking
        // isolation by profile_id).
        let deps = crate::features::tools::ToolDeps {
            storage: storage.clone(),
            engine: engine.clone(),
            embedder: embedder.clone(),
        };
        let mk = |pid| super::super::testkit::ctx_with_deps(pid, deps.clone());

        RagAdd
            .invoke(&mk(a), serde_json::json!({"text": "секрет профиля A"}))
            .await
            .unwrap();
        let out = RagSearch
            .invoke(&mk(b), serde_json::json!({"query": "секрет профиля A"}))
            .await
            .unwrap();
        assert!(out.result.contains("ничего не найдено"));
    }
}
