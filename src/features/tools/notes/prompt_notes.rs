//! Notes — the companion's notes picked for the system prompt (C2 (a),
//! docs/research/companion-platform.md §3.4, spec §6.6): the notes about the
//! person and their world nearest the conversation's opening. A profile switch
//! (`Profile.companion.notes_in_prompt`), off by default; rendered by
//! [`crate::features::companion_memory::render_notes`].

use super::*;
use crate::shared::api::EmbedRole;

/// The cosine a note must reach against the conversation's opening to be put
/// into the prompt — **every** pick, not only the lead (a fact about the person
/// carried into a conversation it has nothing to do with is what the model then
/// brings up). Measured on a planted corpus with bge-m3: an opening about one of
/// the person's facts put that note at 0.49–0.58, while openings about anything
/// else put no note above 0.38 (docs/research/companion-platform.md §3.4).
///
/// A position in the **reference (bge-m3) scale**, read through
/// [`crate::shared::embed_calibration::SimilarityScale`] like the other
/// similarity gates.
pub(crate) const NOTES_FLOOR: f32 = 0.45;

/// The profile's notes — not its `@self` observations — nearest `query` (the
/// conversation's opening, or its summary while one is in force) that clear
/// [`NOTES_FLOOR`], nearest first, at most `limit`. Only notes written before
/// `cut` are candidates ([`crate::features::companion_memory::memory_cut`]):
/// what this conversation writes is in the conversation already, and leaving it
/// out keeps the block — and the prompt's prefix — the same from turn to turn.
/// Superseded notes are never candidates (the store's search skips them).
///
/// Empty with an empty query, an unavailable embedder, or nothing over the
/// floor: then the prompt has no block, and the request is what it was without
/// the feature.
pub(crate) async fn notes_for_prompt(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    query: &str,
    cut: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<Note> {
    if query.trim().is_empty() || limit == 0 {
        return Vec::new();
    }
    ensure_note_vectors(storage, embedder, profile_id).await;
    let Ok(vecs) = embedder
        .embed(vec![query.to_string()], EmbedRole::Query)
        .await
    else {
        return Vec::new();
    };
    let Some(emb) = vecs.into_iter().next() else {
        return Vec::new();
    };
    let Ok(hits) = storage
        .db()
        .note_search_semantic(profile_id, &emb, usize::MAX)
    else {
        return Vec::new();
    };
    let floor = storage.db().similarity_scale().map(NOTES_FLOOR);
    hits.into_iter()
        .take_while(|(_, score)| *score >= floor)
        .filter(|(n, _)| !is_self_note(n) && n.created_at < cut)
        .map(|(n, _)| n)
        .take(limit)
        .collect()
}
