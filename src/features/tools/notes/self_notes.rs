//! Notes — the self-notes (@self) subsystem: fresh/relevant, the graph, backfill.
//! Part of the [`super`] module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;
use crate::shared::api::EmbedRole;

/// The profile's fresh "notes about self" (the "self-model" narrative), newest
/// first (`updated_at DESC`), up to `limit`. A separate read path for self-notes
/// for injection/`get_self_model`/`reflect` — user-facing `note_recall` hides
/// them. See docs/history/narrative-as-notes.md.
pub(crate) fn self_notes_recent(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
    limit: usize,
) -> Vec<Note> {
    let mut notes = storage
        .db()
        .note_list(profile_id, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap_or_default();
    notes.truncate(limit);
    notes
}

/// Self-notes MOST RELEVANT to the query (relevance-based injection, Tier 2):
/// backfill vectors → embed the query → search among self-notes by cosine, top-
/// `limit` (descending by similarity). Empty on an empty query / an unavailable
/// embedder (the caller falls back to recency — graceful degradation, as in
/// Tier 1). See docs/history/narrative-as-notes.md (Tier 2, relevance-based injection).
pub(crate) async fn self_notes_relevant(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    query: &str,
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
    // A candidate margin for the @self filter (regular notes are among all of them too).
    let Ok(hits) = storage
        .db()
        .note_search_semantic(profile_id, &emb, limit.max(20))
    else {
        return Vec::new();
    };
    hits.into_iter()
        .map(|(n, _)| n)
        .filter(is_self_note)
        .take(limit)
        .collect()
}

/// The "Observation links" block for reading the "self-model" (the graph over
/// self-notes, Tier 2): the graph's **edges** touching the shown observations
/// (structure — "what relates to what" — that a flat observation list can't show).
/// A neighbor outside the shown set is brought in with its text (spreading
/// activation). Edges are deduped. `None` if there are no links. A pure DB read.
///
/// **Cross-organ links (Tier 3):** a neighbor **user** note (not `@self`),
/// explicitly linked by the model to an observation, is shown marked `[note]` —
/// so reading the "self-model" sees that an "about self" observation relates to
/// an "about the interlocutor" fact (a self↔user edge). The organs remain
/// separate in storage/search; only a deliberately created edge surfaces. See
/// docs/history/narrative-as-notes.md (Tier 3).
pub(crate) fn self_related_block(ctx: &ToolContext, shown: &[Uuid]) -> Option<String> {
    let shown_set: std::collections::HashSet<Uuid> = shown.iter().copied().collect();
    let mut seen_edges: std::collections::HashSet<(Uuid, Uuid, String)> =
        std::collections::HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    for id in shown {
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, *id, None)
            .unwrap_or_default();
        for (note, relation, outgoing) in nb {
            // Normalize the edge (from→to) and dedup (the same link arrives from both ends).
            let (from, to) = if outgoing {
                (*id, note.id)
            } else {
                (note.id, *id)
            };
            if !seen_edges.insert((from, to, relation.clone())) {
                continue;
            }
            // A cross-organ neighbor (a user note) is marked — reading the
            // "self-model" sees the observation's link to an "about the
            // interlocutor" fact.
            let mark = if is_self_note(&note) {
                String::new()
            } else {
                format!("{} ", ctx.loc.t("notes.mark.note"))
            };
            // A neighbor outside the shown set is brought in with its text (spreading activation).
            let tail = if shown_set.contains(&note.id) {
                format!("(id={})", note.id)
            } else {
                format!("{mark}(id={}) {}", note.id, note.content)
            };
            let arrow = if outgoing { "→" } else { "←" };
            lines.push(format!("- (id={id}) {arrow}{relation} {tail}"));
            if lines.len() >= RELATED_IN_RECALL {
                break;
            }
        }
        if lines.len() >= RELATED_IN_RECALL {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!(
            "\n{}:\n{}",
            ctx.loc.t("notes.block.self_related"),
            lines.join("\n")
        ))
    }
}

/// A one-time idempotent migration of the "self-model" narrative from the JSON
/// blob into self-notes (`@self`), preserving `created_at`. The narrative is
/// **atomically drained** (under the DB mutex acquisition) — a repeat pass sees it
/// empty (a no-op), so there won't be duplicates. Vectors are embedded lazily (on
/// the next recall/gate — `ensure_note_vectors`). The orchestrator calls this
/// best-effort before reading self-notes. See docs/history/narrative-as-notes.md,
/// step 6.
pub(crate) fn migrate_self_narrative(storage: &crate::shared::storage::Storage, profile_id: Uuid) {
    use crate::entities::self_model::NarrativeSegment;
    let mut segments: Vec<NarrativeSegment> = Vec::new();
    let _ = storage.db().self_model_update(profile_id, |m| {
        if m.narrative.is_empty() {
            return false;
        }
        segments = std::mem::take(&mut m.narrative);
        true
    });
    for seg in segments {
        // Preserve the original dates — the "fresh observations" order stays
        // intact after the migration.
        let note = Note {
            id: Uuid::new_v4(),
            profile_id,
            content: seg.text,
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: seg.created_at,
            updated_at: seg.created_at,
        };
        let _ = storage.db().note_insert(&note);
    }
}
