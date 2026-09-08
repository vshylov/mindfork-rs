//! Notes — note_link / note_neighbors (a typed link graph). Part of the [`super`]
//! module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;

/// `note_link` — links two notes with a directed, typed relation.
pub struct NoteLink;

#[async_trait::async_trait]
impl Tool for NoteLink {
    fn id(&self) -> ToolId {
        NOTE_LINK_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "link notes"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_link.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from_id": {"type": "string", "description": loc.t("tool.note_link.param.from_id")},
                "to_id": {"type": "string", "description": loc.t("tool.note_link.param.to_id")},
                "relation": {"type": "string", "enum": RELATIONS, "description": loc.t("tool.note_link.param.relation")}
            },
            "required": ["from_id", "to_id", "relation"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let from = parse_id(&args, "from_id", ctx.loc)?;
        let to = parse_id(&args, "to_id", ctx.loc)?;
        let relation = args
            .get("relation")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if !RELATIONS.contains(&relation) {
            anyhow::bail!(ctx.loc.tf(
                "notes.err.unknown_relation",
                &[
                    ("relation", relation),
                    ("allowed", &format!("{RELATIONS:?}"))
                ]
            ));
        }
        if from == to {
            anyhow::bail!(ctx.loc.t("tool.note_link.err.self"));
        }
        let db = ctx.storage.db();
        if !db.note_is_active(ctx.profile_id, from)? || !db.note_is_active(ctx.profile_id, to)? {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.note_link.result.missing"),
            ));
        }
        let created = db.note_link_insert(ctx.profile_id, from, to, relation)?;
        let verb = if created {
            ctx.loc.t("tool.note_link.result.created")
        } else {
            ctx.loc.t("tool.note_link.result.existed")
        };
        Ok(ToolOutcome::text(ctx.loc.tf(
            "tool.note_link.result.line",
            &[
                ("verb", verb),
                ("from", &from.to_string()),
                ("relation", relation),
                ("to", &to.to_string()),
            ],
        ))
        .wrote_if(created))
    }
}

/// `note_neighbors` — shows notes linked to a given note.
pub struct NoteNeighbors;

#[async_trait::async_trait]
impl Tool for NoteNeighbors {
    fn id(&self) -> ToolId {
        NOTE_NEIGHBORS_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "note links"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_neighbors.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": loc.t("tool.note_neighbors.param.id")},
                "relation": {"type": "string", "enum": RELATIONS, "description": loc.t("tool.note_neighbors.param.relation")}
            },
            "required": ["id"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let id = parse_id(&args, "id", ctx.loc)?;
        let relation = args
            .get("relation")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if let Some(r) = relation
            && !RELATIONS.contains(&r)
        {
            anyhow::bail!(ctx.loc.tf(
                "notes.err.unknown_relation",
                &[("relation", r), ("allowed", &format!("{RELATIONS:?}"))]
            ));
        }
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, id, relation)?;
        if nb.is_empty() {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.note_neighbors.result.empty"),
            ));
        }
        let mut out = format!(
            "{}\n",
            ctx.loc.tf(
                "tool.note_neighbors.result.header",
                &[("id", &id.to_string())]
            )
        );
        for (note, rel, outgoing) in &nb {
            let arrow = if *outgoing { "→" } else { "←" };
            out.push_str(&format!(
                "- {arrow}{rel} (id={}) {}\n",
                note.id, note.content
            ));
        }
        Ok(ToolOutcome::text(out.trim_end().to_string()))
    }
}
