//! Note tools: `note_save`, `note_recall`. The assistant's memory about the
//! user/context, **isolated by `profile_id`** (spec §9.3, §9.5).

use anyhow::Result;
use uuid::Uuid;

use crate::entities::note::Note;
use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Name of the note-recall tool (reflection needs it for cross-organ links,
/// Tier 3 — ids of user notes).
pub const NOTE_RECALL_ID: &str = "note_recall";
/// Name of the note-revision tool (DB-only, gated by the profile's set).
pub const NOTE_REVISE_ID: &str = "note_revise";
/// The link graph and revision history (Tier 2, DB-only, gated by the profile's set).
pub const NOTE_LINK_ID: &str = "note_link";
pub const NOTE_NEIGHBORS_ID: &str = "note_neighbors";
pub const NOTE_SUPERSEDE_ID: &str = "note_supersede";
pub const NOTE_MERGE_ID: &str = "note_merge";
pub const CONSOLIDATE_NOTES_ID: &str = "consolidate_notes";
/// A note's link to a RAG source (Tier 3, Path 3 — linking the memory organs).
pub const NOTE_CITE_SOURCE_ID: &str = "note_cite_source";

/// Similarity threshold above which two notes count as a possible duplicate (for
/// the consolidation overview). Chosen empirically against bge-m3 — pairs above
/// it are worth considering.
///
/// A position in the **reference (bge-m3) scale**, not an absolute cosine: on a
/// model with a narrower range the same intent sits elsewhere (~0.96 on
/// `multilingual-e5-large-instruct`, where used raw it would call antonyms
/// duplicates — research §6). Read through
/// [`crate::shared::embed_calibration::SimilarityScale`] before use; that map is
/// the identity until a model change is actually recorded.
const CONSOLIDATE_SIMILARITY: f32 = 0.85;
/// Max number of items to show in each section of the consolidation overview.
const CONSOLIDATE_LIST_CAP: usize = 8;

/// Types of links between notes (directed). Mirror the `note_link` tool's schema.
const RELATIONS: [&str; 4] = ["supports", "contradicts", "refines", "relates"];

/// How many notes `note_recall` returns by default (if no limit is given).
const DEFAULT_RECALL: usize = 5;

/// Batch size when backfilling embeddings for "old" notes.
const NOTE_BACKFILL_BATCH: usize = 32;

/// Max number of related notes to mix into `note_recall` (spreading activation).
const RELATED_IN_RECALL: usize = 5;

/// The reserved "about-self notes" tag: the "self-model" narrative moved into
/// regular notes (see docs/history/narrative-as-notes.md, Tier 1). Self-notes
/// share tables, embeddings, the graph, and consolidation with regular ones, but
/// are **hidden** from user-facing `note_recall`, the `note_save` gate, and the
/// consolidation overview by filtering on this tag — memory about oneself ≠
/// memory about the interlocutor, mixing the output is risky. A leading `@`
/// doesn't occur in natural tags; a collision is rare and harmless (such a note
/// would simply be treated as an insight). Self-notes are read via a separate
/// path (`self_notes_recent`).
pub const SELF_NOTE_TAG: &str = "@self";

/// Does the note carry the reserved [`SELF_NOTE_TAG`] ("a note about self")?
pub(crate) fn is_self_note(note: &Note) -> bool {
    note.tags.iter().any(|t| t == SELF_NOTE_TAG)
}

/// Parses a uuid from a string argument field, with a clear error (in the language `loc`).
fn parse_id(
    args: &serde_json::Value,
    key: &str,
    loc: &crate::shared::i18n::Locale,
) -> Result<Uuid> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    Uuid::parse_str(raw).map_err(|_| {
        anyhow::anyhow!(loc.tf("notes.err.bad_id_field", &[("key", key), ("raw", raw)]))
    })
}

/// Cosine similarity of two vectors (0 for mismatched length/zero norm).
/// `pub(crate)` — reused by the `user_model` near-duplicate-traits gate
/// (`self_model::UpdateUserModel`), where trait vectors are embedded on the fly.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Truncates a string by character (for a compact overview).
fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Extracts a string-tag array from arguments (empty if missing).
fn parse_tags(args: &serde_json::Value) -> Vec<String> {
    args.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------- submodules (a god-object split: docs/history/refactoring-god-objects.md, stage 4) ----------

mod cite;
mod edit;
mod graph;
mod overview;
mod recall;
mod save;
mod self_notes;

// Re-export the whole `notes::*` external surface (tools + pub(crate) helpers),
// so external `use crate::features::tools::notes::X` sites don't change.
pub(crate) use self::{cite::*, edit::*, graph::*, overview::*, recall::*, save::*, self_notes::*};

#[cfg(test)]
mod tests;
