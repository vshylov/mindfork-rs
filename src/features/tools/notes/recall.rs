//! Notes — note_recall + the semantic path, related blocks, formatting. Part of
//! the [`super`] module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;

/// `note_recall` — searches the profile's notes by text/tags.
pub struct NoteRecall;

#[async_trait::async_trait]
impl Tool for NoteRecall {
    fn id(&self) -> ToolId {
        NOTE_RECALL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "recall notes"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_recall.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": loc.t("tool.note_recall.param.query")},
                "tags": {"type": "array", "items": {"type": "string"}},
                "limit": {"type": "integer", "minimum": 1}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let tags = parse_tags(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        // The semantic path: there's a query and the embedder is available.
        // Otherwise (no query, the embedder is unavailable, or notes have no
        // vectors) — fall back to substring/tags. Both paths exclude self-notes
        // (@self) from user-facing output.
        let notes = match query {
            Some(q) => match semantic_recall(ctx, q, &tags, limit).await {
                Some(n) => n,
                None => list_user_notes(ctx, query, &tags, limit)?,
            },
            None => list_user_notes(ctx, None, &tags, limit)?,
        };

        let mut outcome = format_notes(&notes, ctx.loc);
        // Spreading activation: mix in notes linked by the graph (Tier 2), so
        // recall surfaces the cluster, not just isolated atoms.
        if let Some(block) = related_block(ctx, &notes) {
            outcome.result.push_str(&block);
        }
        // Notes' citations of RAG sources (Tier 3, Path 3): show what a note is
        // based on (linking the memory organs).
        let ids: Vec<Uuid> = notes.iter().map(|n| n.id).collect();
        if let Some(block) = cited_sources_block(ctx, &ids) {
            outcome.result.push_str(&block);
        }
        Ok(outcome)
    }
}

/// The mixed-in "Related notes" block: neighbors of the top hits via the graph
/// (both directions), excluding ones already shown and superseded ones. `None` if
/// there are no links. A pure DB read.
///
/// **Cross-organ links (Tier 3):** a neighbor "about self" observation (`@self`),
/// explicitly linked by the model to a user note, **is shown** marked `[about
/// self]`. This is NOT a reversal of Tier 1's hiding: regular search/spreading
/// still doesn't pull in self-notes — only an edge **deliberately created** by
/// the model between organs surfaces. See docs/history/narrative-as-notes.md
/// (Tier 3, cross-organ links).
pub(crate) fn related_block(ctx: &ToolContext, hits: &[Note]) -> Option<String> {
    let mut seen: std::collections::HashSet<Uuid> = hits.iter().map(|n| n.id).collect();
    let mut lines: Vec<String> = Vec::new();
    for hit in hits.iter().take(3) {
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, hit.id, None)
            .unwrap_or_default();
        for (note, relation, outgoing) in nb {
            if !seen.insert(note.id) {
                continue;
            }
            let arrow = if outgoing { "→" } else { "←" };
            // A cross-organ neighbor (an "about self" observation) is marked — so
            // the model sees the note's link to the observation without mixing
            // organs in the general output.
            let mark = if is_self_note(&note) {
                format!("{} ", ctx.loc.t("notes.mark.self"))
            } else {
                String::new()
            };
            lines.push(format!(
                "- {arrow}{relation} {mark}(id={}) {}",
                note.id, note.content
            ));
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
            ctx.loc.t("notes.block.related"),
            lines.join("\n")
        ))
    }
}

/// The "Source citations" block: for the shown notes (by id), lists the RAG
/// sources they cite (Tier 3, Path 3 — linking the memory organs). `None` if there
/// are no citations. A pure DB read. `pub(crate)` — used by both `note_recall` and
/// the "self-model" read (`self_model::render_self_read`).
pub(crate) fn cited_sources_block(ctx: &ToolContext, note_ids: &[Uuid]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for id in note_ids {
        let srcs = ctx
            .storage
            .db()
            .note_cited_sources(ctx.profile_id, *id)
            .unwrap_or_default();
        for s in srcs {
            lines.push(ctx.loc.tf(
                "notes.block.cited_sources.item",
                &[("id", &id.to_string()), ("s", &s)],
            ));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!(
            "\n{}:\n{}",
            ctx.loc.t("notes.block.cited_sources"),
            lines.join("\n")
        ))
    }
}

/// Substring/tag selection of notes for `note_recall`: read with no limit,
/// **drop self-notes** (except when `ctx.recall_includes_self` — Tier 3, Path 2),
/// then truncate — otherwise self-notes would occupy limit slots and crowd user
/// notes out of the output.
pub(crate) fn list_user_notes(
    ctx: &ToolContext,
    query: Option<&str>,
    tags: &[String],
    limit: Option<usize>,
) -> Result<Vec<Note>> {
    let mut notes = ctx
        .storage
        .db()
        .note_list(ctx.profile_id, query, tags, None)?;
    if !ctx.recall_includes_self {
        notes.retain(|n| !is_self_note(n));
    }
    if let Some(l) = limit {
        notes.truncate(l);
    }
    Ok(notes)
}

/// Semantic search of notes by the query's embedding. `None` if the embedder is
/// unavailable (graceful degradation — the caller falls back to substring search)
/// or the result is empty (e.g. notes don't have vectors yet). Tags are applied as
/// a filter on top of the ranking.
pub(crate) async fn semantic_recall(
    ctx: &ToolContext,
    query: &str,
    tags: &[String],
    limit: Option<usize>,
) -> Option<Vec<Note>> {
    // Backfill: pull in embeddings for notes with no vectors (old/imported),
    // otherwise semantic search won't see them.
    ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
    let emb = ctx
        .embedder
        .embed(vec![query.to_string()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let want = limit.unwrap_or(DEFAULT_RECALL);
    // Take a candidate margin: the tag filter and self-note exclusion thin them out.
    let cand = want.max(30);
    let hits = ctx
        .storage
        .db()
        .note_search_semantic(ctx.profile_id, &emb, cand)
        .ok()?;
    let mut notes: Vec<Note> = hits
        .into_iter()
        .map(|(n, _)| n)
        // Self-notes are excluded, except with `recall_includes_self` (Tier 3, Path 2).
        .filter(|n| {
            (ctx.recall_includes_self || !is_self_note(n))
                && (tags.is_empty() || tags.iter().all(|t| n.tags.contains(t)))
        })
        .collect();
    notes.truncate(want);
    if notes.is_empty() { None } else { Some(notes) }
}

/// Formats a list of notes into the tool's text result. Shows the **id** of every
/// note — so the model can reference it in `note_link`/`note_revise`/
/// `note_supersede` (including cross-organ: linking a user note to an "about
/// self" observation, Tier 3). Previously the id wasn't printed, and notes from
/// recall were unaddressable, even though `note_link`'s description promised
/// "id from note_recall".
///
/// "About self" observations (`@self`, only reach the output with
/// `recall_includes_self` — Tier 3, Path 2) are marked with the `[about self]`
/// prefix, and the internal `@self` tag is dropped from the tag display (the
/// marker replaces it).
pub(crate) fn format_notes(notes: &[Note], loc: &crate::shared::i18n::Locale) -> ToolOutcome {
    if notes.is_empty() {
        return ToolOutcome::text(loc.t("tool.note_recall.result.empty"));
    }
    let mut out = format!(
        "{}\n",
        loc.tf(
            "tool.note_recall.result.header",
            &[("n", &notes.len().to_string())]
        )
    );
    for n in notes {
        let mark = if is_self_note(n) {
            format!("{} ", loc.t("notes.mark.self"))
        } else {
            String::new()
        };
        out.push_str(&format!("- (id={}) {mark}{}", n.id, n.content));
        // Hide the internal @self tag — the [about self] marker plays its role.
        let tags: Vec<&str> = n
            .tags
            .iter()
            .filter(|t| t.as_str() != SELF_NOTE_TAG)
            .map(String::as_str)
            .collect();
        if !tags.is_empty() {
            out.push_str(&format!("  [{}]", tags.join(", ")));
        }
        out.push('\n');
    }
    ToolOutcome::text(out.trim_end().to_string())
}
