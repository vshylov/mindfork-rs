//! Reading files attached to the chat (`attachment_read`, stage 2 of
//! docs/file-attachments.md).
//!
//! A file that fits the budget is already in the prompt in full; a large one is
//! attached "by reference" — the pinned block carries its metadata and the head
//! excerpt, and this tool reads the rest **page by page** from the stored
//! snapshot. Pages (rather than character offsets) are deliberate: they are
//! discrete and enumerable, so the model can walk `1..M` and *know* it has read
//! everything — the guarantee semantic retrieval cannot give.
//!
//! Note this **narrows** access compared with `fs_read`: the tool can only reach
//! files the user explicitly attached, never the filesystem — hence no gate and
//! enabled by default.

use anyhow::Result;

use crate::entities::profile::ToolId;

use super::{Tool, ToolContext, ToolOutcome};

/// Tool name (a stable wire protocol — not localized).
pub const ATTACHMENT_READ_ID: &str = "attachment_read";

/// `attachment_read` — returns one page of a file attached to the chat.
pub struct AttachmentRead;

#[async_trait::async_trait]
impl Tool for AttachmentRead {
    fn id(&self) -> ToolId {
        ATTACHMENT_READ_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "read an attached file"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.attachment_read.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": loc.t("tool.attachment_read.param.name")
                },
                "page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": loc.t("tool.attachment_read.param.page")
                }
            },
            "required": ["name"]
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        if ctx.attachments.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.attachment_read.none")));
        }
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        // No name (or an unknown one) — list what IS attached instead of a bare
        // error, so the next call can succeed.
        let Some(att) = ctx.attachments.iter().find(|a| a.matches(name)) else {
            let names: Vec<&str> = ctx.attachments.iter().map(|a| a.name.as_str()).collect();
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.attachment_read.unknown",
                &[("name", name), ("names", &names.join(", "))],
            )));
        };
        let page_tokens = ctx.attachment_page_tokens;
        let total = att.page_count(page_tokens);
        let page = args
            .get("page")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1);
        let Some(text) = att.page(page_tokens, page) else {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.attachment_read.bad_page",
                &[
                    ("page", &page.to_string()),
                    ("name", &att.name),
                    ("total", &total.to_string()),
                ],
            )));
        };
        let header = ctx.loc.tf(
            "tool.attachment_read.header",
            &[
                ("name", &att.name),
                ("page", &page.to_string()),
                ("total", &total.to_string()),
            ],
        );
        Ok(ToolOutcome::text(format!("{header}\n{text}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::attachment::{AttachMode, Attachment};
    use uuid::Uuid;

    /// Page size used by the fixtures: 10 estimated tokens ≈ 40 bytes ≈ 20
    /// Cyrillic characters — a short line fits, a few lines don't, so fixtures
    /// span several pages.
    const PAGE: usize = 10;

    /// A tool context with the given attachments, paginated at [`PAGE`].
    fn ctx_with(attachments: Vec<Attachment>) -> (tempfile::TempDir, ToolContext) {
        let (dir, _storage, mut ctx) = super::super::testkit::ctx_with_storage(Uuid::new_v4());
        ctx.attachments = attachments.into();
        ctx.attachment_page_tokens = PAGE;
        (dir, ctx)
    }

    fn att(name: &str, text: &str) -> Attachment {
        Attachment::new(
            name,
            format!("/tmp/{name}"),
            text.to_string(),
            text.len(),
            AttachMode::ByReference,
        )
    }

    #[tokio::test]
    async fn reads_pages_and_reports_the_range() {
        let text = "первая строка\nвторая строка\nтретья строка\nчетвёртая строка\n";
        let (_d, ctx) = ctx_with(vec![att("doc.txt", text)]);
        let tool = AttachmentRead;

        let first = tool
            .invoke(&ctx, serde_json::json!({"name": "doc.txt"}))
            .await
            .unwrap()
            .result;
        assert!(first.contains("первая строка"), "{first}");
        // The header states the range, so the model knows how far it can walk.
        assert!(first.contains("1"), "{first}");

        // Every page is reachable and together they cover the whole file.
        let total = ctx.attachments[0].page_count(PAGE);
        assert!(total > 1, "the fixture must span several pages");
        let mut joined = String::new();
        for p in 1..=total {
            let out = tool
                .invoke(&ctx, serde_json::json!({"name": "doc.txt", "page": p}))
                .await
                .unwrap()
                .result;
            // Strip the header line to reassemble the source.
            joined.push_str(out.split_once('\n').unwrap().1);
        }
        assert_eq!(joined, text, "walking 1..M reads the whole file");
    }

    #[tokio::test]
    async fn page_defaults_to_the_first_one() {
        let (_d, ctx) = ctx_with(vec![att("doc.txt", "начало файла и продолжение текста")]);
        let with = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "doc.txt", "page": 1}))
            .await
            .unwrap()
            .result;
        let without = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "doc.txt"}))
            .await
            .unwrap()
            .result;
        assert_eq!(with, without);
    }

    #[tokio::test]
    async fn out_of_range_and_unknown_name_explain_themselves() {
        let (_d, ctx) = ctx_with(vec![att("doc.txt", "коротко")]);

        // A page past the end reports the real count instead of failing blindly.
        let out = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "doc.txt", "page": 99}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("99") && out.contains('1'), "{out}");

        // An unknown name lists what IS attached, so the retry can succeed.
        let out = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "other.txt"}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("doc.txt"), "{out}");
    }

    #[tokio::test]
    async fn reports_clearly_when_nothing_is_attached() {
        let (_d, ctx) = ctx_with(Vec::new());
        let out = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "any.txt"}))
            .await
            .unwrap()
            .result;
        assert!(!out.is_empty());
    }

    #[tokio::test]
    async fn description_and_results_are_localized() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let desc = AttachmentRead.description(loc);
            assert!(
                !desc.contains('{') && !desc.contains('}'),
                "{lang:?}: {desc}"
            );
            if lang == crate::shared::i18n::Lang::En {
                assert!(
                    !desc.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                    "Cyrillic leaked into the en description: {desc}"
                );
            }
        }
    }
}
