//! Notes — note_revise / note_supersede / note_merge (edit/replace/merge). Part of
//! the [`super`] module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;

/// `note_revise` — rewrites an existing note in place (a revision). The core of
/// integration: the new content replaces the old rather than piling up as a
/// near-duplicate.
pub struct NoteRevise;

#[async_trait::async_trait]
impl Tool for NoteRevise {
    fn id(&self) -> ToolId {
        NOTE_REVISE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "revise note"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_revise.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": loc.t("tool.note_revise.param.id")},
                "content": {"type": "string", "description": loc.t("tool.note_revise.param.content")}
            },
            "required": ["id", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let uuid = Uuid::parse_str(id).map_err(|_| {
            anyhow::anyhow!(ctx.loc.tf("tool.note_revise.err.bad_id", &[("id", id)]))
        })?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("notes.err.content_string")))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!(ctx.loc.t("notes.err.content_empty"));
        }
        if !ctx
            .storage
            .db()
            .note_update(uuid, ctx.profile_id, &content)?
        {
            return Ok(ToolOutcome::text(
                ctx.loc
                    .tf("notes.result.not_found", &[("id", &uuid.to_string())]),
            ));
        }
        // Re-embedding (best-effort): semantic search must see the new content.
        if let Ok(vecs) = ctx.embedder.embed(vec![content.clone()]).await
            && let Some(emb) = vecs.into_iter().next()
        {
            let _ = ctx
                .storage
                .db()
                .note_vector_upsert(uuid, ctx.profile_id, &emb);
        }
        let mut msg = ctx
            .loc
            .tf("tool.note_revise.result.done", &[("id", &uuid.to_string())]);
        // Graph-integrity warning: an in-place edit doesn't touch links, but if the
        // MEANING changed, incoming edges (e.g. contradicts) may become wrong — for
        // a semantic rewrite, note_supersede is more honest (it preserves the
        // superseded version the links refer to).
        if let Ok(links) = ctx.storage.db().note_link_count(ctx.profile_id, uuid)
            && links > 0
        {
            msg.push('\n');
            msg.push_str(&ctx.loc.tf(
                "tool.note_revise.warn.links",
                &[("links", &links.to_string())],
            ));
        }
        Ok(ToolOutcome::text(msg))
    }
}

/// `note_supersede` — replaces a note with a new version (the "scar" is preserved).
pub struct NoteSupersede;

#[async_trait::async_trait]
impl Tool for NoteSupersede {
    fn id(&self) -> ToolId {
        NOTE_SUPERSEDE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "supersede note"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_supersede.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "old_id": {"type": "string", "description": loc.t("tool.note_supersede.param.old_id")},
                "content": {"type": "string", "description": loc.t("tool.note_supersede.param.content")}
            },
            "required": ["old_id", "content"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let old_id = parse_id(&args, "old_id", ctx.loc)?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(ctx.loc.t("notes.err.content_string")))?
            .trim()
            .to_string();
        if content.is_empty() {
            anyhow::bail!(ctx.loc.t("notes.err.content_empty"));
        }
        if !ctx.storage.db().note_is_active(ctx.profile_id, old_id)? {
            return Ok(ToolOutcome::text(
                ctx.loc
                    .tf("notes.result.not_found", &[("id", &old_id.to_string())]),
            ));
        }
        // The new version inherits the superseded note's tags (including @self —
        // otherwise a self-note would "fall out" into user-facing output on
        // replacement).
        let tags = ctx
            .storage
            .db()
            .note_get(ctx.profile_id, old_id)?
            .map(|n| n.tags)
            .unwrap_or_default();
        let new_id = create_note(ctx, content, tags).await?;
        ctx.storage
            .db()
            .note_supersede_mark(ctx.profile_id, old_id, new_id)?;
        Ok(ToolOutcome::text(ctx.loc.tf(
            "tool.note_supersede.result.done",
            &[
                ("old_id", &old_id.to_string()),
                ("new_id", &new_id.to_string()),
            ],
        )))
    }
}

/// `note_merge` — folds several notes into one (the originals are superseded).
pub struct NoteMerge;

#[async_trait::async_trait]
impl Tool for NoteMerge {
    fn id(&self) -> ToolId {
        NOTE_MERGE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "merge notes"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_merge.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "ids": {"type": "array", "items": {"type": "string"}, "minItems": 2, "description": loc.t("tool.note_merge.param.ids")},
                "content": {"type": "string", "description": loc.t("tool.note_merge.param.content")}
            },
            "required": ["ids", "content"]
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
        let ids: Vec<Uuid> = args
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .filter_map(|s| Uuid::parse_str(s.trim()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let db = ctx.storage.db();
        let active: Vec<Uuid> = ids
            .into_iter()
            .filter(|id| db.note_is_active(ctx.profile_id, *id).unwrap_or(false))
            .collect();
        if active.len() < 2 {
            anyhow::bail!(ctx.loc.t("tool.note_merge.err.min_two"));
        }
        // The merged note inherits the union of the sources' tags (including @self
        // — merging self-notes stays a self-note, hidden from recall).
        let mut tags: Vec<String> = Vec::new();
        for old in &active {
            if let Ok(Some(n)) = ctx.storage.db().note_get(ctx.profile_id, *old) {
                for t in n.tags {
                    if !tags.contains(&t) {
                        tags.push(t);
                    }
                }
            }
        }
        let new_id = create_note(ctx, content, tags).await?;
        for old in &active {
            ctx.storage
                .db()
                .note_supersede_mark(ctx.profile_id, *old, new_id)?;
            // Move the sources' links onto the merged note — the graph doesn't get orphaned.
            ctx.storage
                .db()
                .note_links_retarget(ctx.profile_id, *old, new_id)?;
        }
        Ok(ToolOutcome::text(ctx.loc.tf(
            "tool.note_merge.result.done",
            &[
                ("n", &active.len().to_string()),
                ("new_id", &new_id.to_string()),
            ],
        )))
    }
}
