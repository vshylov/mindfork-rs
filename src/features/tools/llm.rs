//! Tools about the **language model** running the assistant (spec §9.14):
//! its current name (`get_llm_name`) and the profile's history of model
//! changes (`get_llm_history`). Deliberately the naming mirror of
//! [`super::self_model`] — the `llm_*` pair is about the underlying LLM, the
//! `*_self_model` family is about the stored personality (spec §17); neither
//! name contains a bare "model" the other could be mistaken for.
//!
//! Both are read-only. The current name comes from the turn snapshot
//! ([`ToolContext::model_name`] — the same single read the bubble header and
//! `MessageMetadata.model` use), never from asking the engine mid-turn. The
//! history is read from `data.db` through `ctx.storage`, like every other
//! DB-backed organ; it is written by the orchestrator after a completed
//! exchange, not by these tools.

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::introspection::empty_object;
use super::{Tool, ToolContext, ToolOutcome};

/// The name of the current-LLM tool.
pub const GET_LLM_NAME_ID: &str = "get_llm_name";
/// The name of the LLM-history tool.
pub const GET_LLM_HISTORY_ID: &str = "get_llm_history";

/// `get_llm_name` — the name of the language model generating this turn, plus
/// the engine mode. Answers "cannot say" honestly when the engine does not
/// report a name (spec §9.14).
pub struct GetLlmName;

#[async_trait::async_trait]
impl Tool for GetLlmName {
    fn id(&self) -> ToolId {
        GET_LLM_NAME_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "show LLM name"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.get_llm_name.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let mode = ctx.engine_mode.key();
        let text = match &ctx.model_name {
            Some(name) => ctx.loc.tf(
                "tool.get_llm_name.result",
                &[("name", name), ("mode", mode)],
            ),
            // The door-closing text: no other route this turn, and the one
            // that works (naming the model in settings) is the user's.
            None => ctx.loc.tf("tool.get_llm_name.unknown", &[("mode", mode)]),
        };
        Ok(ToolOutcome::text(text))
    }
}

/// `get_llm_history` — the profile's language-model history (dated records,
/// oldest first), recorded by the orchestrator after completed exchanges
/// (spec §9.14).
pub struct GetLlmHistory;

#[async_trait::async_trait]
impl Tool for GetLlmHistory {
    fn id(&self) -> ToolId {
        GET_LLM_HISTORY_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Introspection
    }
    fn ui_label(&self) -> &'static str {
        "LLM history"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.get_llm_history.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        empty_object()
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let records = ctx.storage.db().llm_history(ctx.profile_id)?;
        if records.is_empty() {
            // "Nothing here" is not an error; say why it is empty and — when
            // the turn knows it — what the current model is, so the answer
            // does not depend on another tool being enabled.
            let mut text: String = ctx.loc.t("tool.get_llm_history.empty").into();
            if let Some(name) = &ctx.model_name {
                text.push(' ');
                text.push_str(&ctx.loc.tf(
                    "tool.get_llm_history.current",
                    &[("name", name), ("mode", ctx.engine_mode.key())],
                ));
            }
            return Ok(ToolOutcome::text(text));
        }
        let mut out = ctx.loc.tf(
            "tool.get_llm_history.header",
            &[("count", &records.len().to_string())],
        );
        for r in &records {
            // The date is data, not prose — rendered the same in every
            // language, absolute UTC (the record is stored in UTC).
            out.push_str(&format!(
                "\n{} — {} ({})",
                r.changed_at.format("%Y-%m-%d %H:%M UTC"),
                r.model,
                r.mode.key()
            ));
        }
        Ok(ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::profile::LlmChange;
    use crate::features::tools::testkit;
    use crate::shared::config::ServerMode;
    use crate::shared::i18n::{Lang, locale};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    #[test]
    fn llm_tools_are_in_catalog_and_defaults() {
        // Both tools are offered (Introspection group) and — unlike the
        // self-model family — enabled by default (fork F7 of
        // docs/research/language-model-history.md).
        let all = crate::features::tools::all_tool_ids();
        let defaults = crate::features::tools::default_tool_ids();
        for id in [GET_LLM_NAME_ID, GET_LLM_HISTORY_ID] {
            assert!(all.iter().any(|t| t == id), "{id} missing from catalog");
            assert!(defaults.iter().any(|t| t == id), "{id} not in defaults");
        }
    }

    #[test]
    fn descriptions_are_localized() {
        // ru != en catches a forgotten `loc` (the gate every tool family
        // carries); en must stay free of Cyrillic.
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        for tool in [&GetLlmName as &dyn Tool, &GetLlmHistory] {
            let (d_ru, d_en) = (tool.description(ru), tool.description(en));
            assert_ne!(d_ru, d_en, "{} description is not localized", tool.id());
            assert!(
                !d_en.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                "{} en description contains Cyrillic",
                tool.id()
            );
        }
    }

    #[tokio::test]
    async fn get_llm_name_reports_the_turn_snapshot() {
        let (_dir, _storage, mut ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        ctx.model_name = Some("gemma-4-31B".into());
        ctx.engine_mode = ServerMode::Managed;
        let out = GetLlmName
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("gemma-4-31B"), "{}", out.result);
        assert!(out.result.contains("managed"), "{}", out.result);
    }

    #[tokio::test]
    async fn get_llm_name_closes_the_door_when_the_engine_does_not_say() {
        let (_dir, _storage, mut ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        ctx.model_name = None;
        ctx.engine_mode = ServerMode::External;
        let out = GetLlmName
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        // The degradation text names the mode and points at settings — the
        // route that works (docs/lessons.md §4).
        assert_eq!(
            out.result,
            ctx.loc
                .tf("tool.get_llm_name.unknown", &[("mode", "external")])
        );
    }

    #[tokio::test]
    async fn get_llm_history_renders_records_oldest_first() {
        let pid = Uuid::new_v4();
        let (_dir, storage, ctx) = testkit::ctx_with_storage(pid);
        for (ts, model, mode) in [
            ("2026-01-01T12:00:00Z", "gemma-4", ServerMode::Managed),
            ("2026-02-01T12:00:00Z", "qwen-3.6", ServerMode::External),
        ] {
            storage
                .db()
                .llm_history_note(
                    pid,
                    &LlmChange {
                        changed_at: ts.parse().unwrap(),
                        model: model.into(),
                        mode,
                    },
                )
                .unwrap();
        }
        let out = GetLlmHistory
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        let gemma = out.result.find("2026-01-01 12:00 UTC — gemma-4 (managed)");
        let qwen = out
            .result
            .find("2026-02-01 12:00 UTC — qwen-3.6 (external)");
        assert!(gemma.is_some() && qwen.is_some(), "{}", out.result);
        assert!(
            gemma < qwen,
            "history must render oldest first: {}",
            out.result
        );
        let header = ctx.loc.tf("tool.get_llm_history.header", &[("count", "2")]);
        assert!(out.result.starts_with(&header), "{}", out.result);
    }

    #[tokio::test]
    async fn get_llm_history_empty_names_the_current_model_when_known() {
        let (_dir, _storage, mut ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        ctx.model_name = Some("gpt-5.2".into());
        ctx.engine_mode = ServerMode::OpenAi;
        let out = GetLlmHistory
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            out.result,
            format!(
                "{} {}",
                ctx.loc.t("tool.get_llm_history.empty"),
                ctx.loc.tf(
                    "tool.get_llm_history.current",
                    &[("name", "gpt-5.2"), ("mode", "openai")]
                )
            )
        );
        // …and with no name known, the empty text stands alone.
        ctx.model_name = None;
        let out = GetLlmHistory
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(out.result, ctx.loc.t("tool.get_llm_history.empty"));
    }

    #[test]
    fn history_line_format_is_stable() {
        // The line is composed in code (dates and names are data, not prose):
        // pin its shape so a rewording cannot silently change what the model
        // has already seen in stored transcripts.
        let rec = LlmChange {
            changed_at: Utc.with_ymd_and_hms(2026, 8, 29, 9, 5, 0).unwrap(),
            model: "m".into(),
            mode: ServerMode::Grok,
        };
        let line = format!(
            "{} — {} ({})",
            rec.changed_at.format("%Y-%m-%d %H:%M UTC"),
            rec.model,
            rec.mode.key()
        );
        assert_eq!(line, "2026-08-29 09:05 UTC — m (grok)");
    }
}
