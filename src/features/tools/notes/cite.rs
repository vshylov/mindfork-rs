//! Notes — note_cite_source (a note's link to a RAG source). Part of the [`super`]
//! module; split out of the notes.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 4).

use super::*;

/// `note_cite_source` — links a note to a RAG source (Tier 3, Path 3: linking the
/// memory organs). The link targets the **source's name** (stable across
/// reindexing, unlike chunk ids). The source must actually exist in the profile's
/// knowledge base.
pub struct NoteCiteSource;

#[async_trait::async_trait]
impl Tool for NoteCiteSource {
    fn id(&self) -> ToolId {
        NOTE_CITE_SOURCE_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "сослаться на источник"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.note_cite_source.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "note_id": {"type": "string", "description": loc.t("tool.note_cite_source.param.note_id")},
                "source": {"type": "string", "description": loc.t("tool.note_cite_source.param.source")}
            },
            "required": ["note_id", "source"]
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let note_id = parse_id(&args, "note_id", ctx.loc)?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if source.is_empty() {
            anyhow::bail!(ctx.loc.t("tool.note_cite_source.err.source_empty"));
        }
        let db = ctx.storage.db();
        if !db.note_is_active(ctx.profile_id, note_id)? {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.note_cite_source.result.note_missing",
                &[("id", &note_id.to_string())],
            )));
        }
        if !db.rag_source_exists(ctx.profile_id, &source)? {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.note_cite_source.result.source_missing",
                &[("source", &source)],
            )));
        }
        let created = db.note_cite_source_insert(ctx.profile_id, note_id, &source)?;
        let verb = if created {
            ctx.loc.t("tool.note_cite_source.result.created")
        } else {
            ctx.loc.t("tool.note_cite_source.result.existed")
        };
        Ok(ToolOutcome::text(ctx.loc.tf(
            "tool.note_cite_source.result.line",
            &[
                ("verb", verb),
                ("note_id", &note_id.to_string()),
                ("source", &source),
            ],
        )))
    }
}
