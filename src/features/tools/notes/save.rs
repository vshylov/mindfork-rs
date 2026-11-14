//! Notes — note_save + writing a note, the similarity gate, embedding. Part of
//! the [`super`] module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;
use crate::shared::api::EmbedRole;

/// `note_save` — saves a profile note. Returns its id.
pub struct NoteSave;

#[async_trait::async_trait]
impl Tool for NoteSave {
    fn id(&self) -> ToolId {
        "note_save".into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "save note"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_save.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": loc.t("tool.note_save.param.content")},
                "tags": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("notes.err.content_string")))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!(ctx.loc.t("notes.err.content_empty"));
        }
        let tags = parse_tags(&args);
        let note = Note::new(ctx.profile_id, content, tags);
        let id = note.id;
        ctx.storage.db().note_insert(&note)?;

        let mut out = ctx
            .loc
            .tf("tool.note_save.result.saved", &[("id", &id.to_string())]);
        // Embedding (best-effort) + a compatibility gate: show semantically close
        // existing notes, so the model can merge a repeat it has just saved into
        // the one it repeats (note_merge) instead of keeping a near-copy. The note
        // is already written when the gate runs. Without an embedder — softly skip
        // (like RAG reranking), the note is saved either way.
        // Passage: this vector is stored, and the gate below compares it against
        // other stored notes — both sides of that comparison are passages.
        if let Ok(vecs) = ctx
            .embedder
            .embed(vec![note.content.clone()], EmbedRole::Passage)
            .await
            && let Some(emb) = vecs.into_iter().next()
        {
            let _ = ctx
                .storage
                .db()
                .note_vector_upsert(id, ctx.profile_id, &emb);
            // Pull in embeddings for "old" notes with no vectors, so they
            // participate in the gate (and subsequent semantic search).
            ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
            // Self-notes (@self) are excluded — a regular save shouldn't run into
            // "self-model" observations.
            let similar = gate_neighbours(ctx, &emb, id, |n| !is_self_note(n));
            if !similar.is_empty() {
                out.push('\n');
                out.push_str(ctx.loc.t("tool.note_save.gate.similar"));
                for n in similar {
                    out.push_str(&format!("\n- (id={}) {}", n.id, n.content));
                }
            }
        }
        Ok(ToolOutcome::text(out).wrote())
    }
}

/// The write gates' list: stored notes at or above [`WRITE_GATE_SIMILARITY`] to
/// `emb`, nearest first, at most [`WRITE_GATE_CAP`] — minus `exclude` (the note just
/// written) and whatever `keep` refuses. Empty when the search fails: the note is
/// written either way, the gate is advice.
///
/// The threshold is applied **before** the kind filter, over every live note: the
/// search scores them all anyway, and a fixed short candidate list let the other
/// kind crowd a real neighbour out — `note_save` took the nearest four and then
/// dropped the @self ones, so on a profile where most notes are observations a
/// duplicate in fifth place was never shown.
fn gate_neighbours(
    ctx: &ToolContext,
    emb: &[f32],
    exclude: Uuid,
    keep: impl Fn(&Note) -> bool,
) -> Vec<Note> {
    let threshold = ctx
        .storage
        .db()
        .similarity_scale()
        .map(WRITE_GATE_SIMILARITY);
    let Ok(hits) = ctx
        .storage
        .db()
        .note_search_semantic(ctx.profile_id, emb, usize::MAX)
    else {
        return Vec::new();
    };
    hits.into_iter()
        .take_while(|(_, score)| *score >= threshold)
        .map(|(n, _)| n)
        .filter(|n| n.id != exclude && keep(n))
        .take(WRITE_GATE_CAP)
        .collect()
}

/// Creates a note (insert + best-effort embedding) and returns its id. Used by
/// `note_supersede`/`note_merge` for the new/merged version of a note, and by
/// "self-model" tools for self-notes (tag [`SELF_NOTE_TAG`]).
pub(crate) async fn create_note(
    ctx: &ToolContext,
    content: String,
    tags: Vec<String>,
) -> Result<Uuid> {
    let note = Note::new(ctx.profile_id, content, tags);
    let id = note.id;
    ctx.storage.db().note_insert(&note)?;
    if let Ok(vecs) = ctx
        .embedder
        .embed(vec![note.content.clone()], EmbedRole::Passage)
        .await
        && let Some(emb) = vecs.into_iter().next()
    {
        let _ = ctx
            .storage
            .db()
            .note_vector_upsert(id, ctx.profile_id, &emb);
    }
    Ok(id)
}

/// Semantically close self-notes to `content` — the `add_insight` tool's gate:
/// pulls in self-note vectors (backfill), embeds the query, keeps the self-notes at
/// or above [`WRITE_GATE_SIMILARITY`] ([`gate_neighbours`]), excludes `exclude`.
/// Empty if the embedder is unavailable (graceful degradation) or nothing is that
/// close — a direct mirror of the `note_save` gate, but over "about self" memory.
pub(crate) async fn self_note_similar(
    ctx: &ToolContext,
    content: &str,
    exclude: Uuid,
) -> Vec<Note> {
    ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
    // Passage, despite reading like a query: the other side of this comparison
    // is the stored @self notes, and the `add_insight` gate it feeds is one of
    // the calibrated thresholds (docs/research/embedding-input-prefixes.md §5.3).
    let Ok(vecs) = ctx
        .embedder
        .embed(vec![content.to_string()], EmbedRole::Passage)
        .await
    else {
        return Vec::new();
    };
    let Some(emb) = vecs.into_iter().next() else {
        return Vec::new();
    };
    gate_neighbours(ctx, &emb, exclude, is_self_note)
}

/// Pulls in embeddings for profile notes that don't have them yet (created before
/// vector search existed, imported, or saved when the embedder was unavailable at
/// the time). Without this, semantic search/gates don't see them. **Best-effort**:
/// the embedder is unavailable or the batch failed — just return (search works off
/// what's there, plus a fallback to substring). Effectively a one-time cost per
/// profile: after backfilling, the "no vectors" list is empty and the call is
/// nearly free (a single SELECT).
pub(crate) async fn ensure_note_vectors(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
) {
    let missing = match storage.db().notes_missing_vectors(profile_id) {
        Ok(m) => m,
        Err(_) => return,
    };
    for chunk in missing.chunks(NOTE_BACKFILL_BATCH) {
        let texts: Vec<String> = chunk.iter().map(|(_, c)| c.clone()).collect();
        let Ok(vecs) = embedder.embed(texts, EmbedRole::Passage).await else {
            return; // the embedder is unavailable — no point continuing
        };
        for ((id, _), emb) in chunk.iter().zip(vecs) {
            let _ = storage.db().note_vector_upsert(*id, profile_id, &emb);
        }
    }
}
