//! Tools over files attached to the chat (`/file attach`,
//! docs/file-attachments.md).
//!
//! A file that fits the budget is already in the prompt in full; a large one is
//! attached "by reference" — the pinned block carries its metadata and the head
//! excerpt, and two complementary tools reach the rest:
//!
//! - **`attachment_read`** (stage 2) reads the stored snapshot **page by page**.
//!   Pages rather than character offsets are deliberate: they are discrete and
//!   enumerable, so the model can walk `1..M` and *know* it has read everything —
//!   the guarantee semantic retrieval cannot give.
//! - **`attachment_search`** (stage 3) finds the right place by meaning in the
//!   chat-scoped index. On a file of a few hundred pages, paging to the answer is
//!   hopeless; search answers *where* to look, `attachment_read` guarantees
//!   *everything* can be read.
//!
//! Both **narrow** access compared with `fs_read`: they can only reach files the
//! user explicitly attached, never the filesystem — hence no gate and enabled by
//! default.

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::shared::api::EmbedRole;

use super::{Tool, ToolContext, ToolOutcome};

/// Tool name (a stable wire protocol — not localized).
pub const ATTACHMENT_READ_ID: &str = "attachment_read";
/// Tool name (a stable wire protocol — not localized).
pub const ATTACHMENT_SEARCH_ID: &str = "attachment_search";
/// Default number of fragments `attachment_search` returns.
const DEFAULT_TOP_K: usize = 5;

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
        let hits: Vec<&crate::entities::attachment::Attachment> =
            ctx.attachments.iter().filter(|a| a.matches(name)).collect();
        // Several files answer to the same name (two pages of one documentation
        // site whose `<h1>` is site-wide, say). Taking the first would read a
        // *different* file than the one asked for and say nothing — so report the
        // ambiguity with each one's source, which is what tells them apart.
        if hits.len() > 1 {
            let sources: Vec<&str> = hits.iter().map(|a| a.source.as_str()).collect();
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.attachment_read.ambiguous",
                &[("name", name), ("sources", &sources.join(", "))],
            )));
        }
        let Some(att) = hits.into_iter().next() else {
            let names: Vec<&str> = ctx.attachments.iter().map(|a| a.name.as_str()).collect();
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.attachment_read.unknown",
                &[("name", name), ("names", &names.join(", "))],
            )));
        };
        let page_tokens = ctx.attachment_cfg.page_tokens;
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

/// `attachment_search` — semantic search over the chat's by-reference
/// attachments (the chat-scoped index, stage 3).
pub struct AttachmentSearch;

#[async_trait::async_trait]
impl Tool for AttachmentSearch {
    fn id(&self) -> ToolId {
        ATTACHMENT_SEARCH_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Files
    }
    fn ui_label(&self) -> &'static str {
        "search attached files"
    }
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.attachment_search.desc").into()
    }
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        super::search_parameters(
            loc,
            "tool.attachment_search.param.query",
            "tool.attachment_search.param.top_k",
        )
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        if ctx.attachments.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.attachment_read.none")));
        }
        let (query, k) = super::search_args(
            &args,
            ctx.loc,
            "tool.attachment_search.err.query_empty",
            DEFAULT_TOP_K,
        )?;

        // None of the *currently attached* files is indexed (no embedder when
        // they were attached, or they are all inline) — say so and point at the
        // guaranteed path instead of returning a bare "nothing found", which
        // would read as "the file has nothing about it".
        let indexed = ctx.storage.db().attachment_indexed_ids(ctx.chat_id)?;
        let any_indexed = indexed
            .iter()
            .any(|id| ctx.attachments.iter().any(|a| a.id == *id));
        if !any_indexed {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.attachment_search.not_indexed"),
            ));
        }
        // The embedder may have gone away since indexing — degrade to the same
        // clear answer rather than failing the turn (ADR 0002).
        let Ok(mut embeddings) = ctx
            .embedder
            .embed(vec![query.to_string()], EmbedRole::Query)
            .await
        else {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.attachment_search.not_indexed"),
            ));
        };
        let Some(query_vec) = embeddings.pop() else {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.attachment_search.not_indexed"),
            ));
        };
        let hits = ctx
            .storage
            .db()
            .attachment_search(ctx.chat_id, &query_vec, k)?;
        // Only files still attached: an indexing task can finish after its file
        // was removed, and the model must not see fragments of something the user
        // took out of the conversation.
        let hits: Vec<_> = hits
            .into_iter()
            .filter(|h| ctx.attachments.iter().any(|a| a.id == h.attachment_id))
            .collect();
        if hits.is_empty() {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.attachment_search.result.empty"),
            ));
        }
        // Chunks are cut with overlap, so neighbouring hits from one file repeat
        // each other's edges — the same reason `rag_search` stitches and dedups.
        // Reusing that logic (it groups by `source`, which here is the file name).
        let hits: Vec<crate::entities::rag::RagHit> = hits
            .into_iter()
            .map(|h| crate::entities::rag::RagHit {
                id: h.attachment_id,
                source: h.name,
                chunk_text: h.text,
                distance: h.distance,
            })
            .collect();
        let passages = super::rag::dedup_passages(super::rag::stitch_hits(hits));
        let mut out = ctx.loc.tf(
            "tool.attachment_search.result.header",
            &[("n", &passages.len().to_string())],
        );
        // Numbered, each on its own line, separated by a blank one. A fragment is
        // a whole chunk (~800 characters), so it is routinely multi-line: with a
        // single leading `- [name] ` marker its continuation carried no marker at
        // all and ten fragments ran together into one wall of text — boundaries
        // lost for the reader in the feed *and* for the model parsing the result
        // (seen on a live run over a 1.6 MB collection). The number is there to
        // separate, not to address: no tool takes a fragment index.
        for (i, p) in passages.iter().enumerate() {
            out.push_str(&format!("\n\n{}. [{}]\n{}", i + 1, p.source, p.text));
        }
        // Search says *where* to look; the page reader guarantees the rest can be
        // read. Reminding the model of that keeps the two complementary.
        out.push_str("\n\n");
        out.push_str(ctx.loc.t("tool.attachment_search.result.hint"));
        Ok(ToolOutcome::text(out))
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
        ctx.attachment_cfg.page_tokens = PAGE;
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

    /// Two files can legitimately answer to one name (two pages of a
    /// documentation site whose `<h1>` is site-wide). Reading the first one and
    /// saying nothing would be a silently wrong answer — the class this whole
    /// change is about — so the ambiguity is reported with what tells them apart.
    #[tokio::test]
    async fn an_ambiguous_name_is_reported_instead_of_guessing() {
        let one = Attachment::new(
            "V Documentation",
            "https://docs.example.io/memory.html",
            "первый файл".into(),
            11,
            AttachMode::ByReference,
        );
        let two = Attachment::new(
            "V Documentation",
            "https://docs.example.io/concurrency.html",
            "второй файл".into(),
            11,
            AttachMode::ByReference,
        );
        let (_d, ctx) = ctx_with(vec![one, two]);
        let out = AttachmentRead
            .invoke(&ctx, serde_json::json!({"name": "V Documentation"}))
            .await
            .unwrap()
            .result;
        assert!(
            out.contains("memory.html"),
            "the sources must be shown: {out}"
        );
        assert!(out.contains("concurrency.html"), "{out}");
        assert!(
            !out.contains("первый файл"),
            "a page was read anyway: {out}"
        );

        // Addressing by source is unambiguous and still works.
        let out = AttachmentRead
            .invoke(
                &ctx,
                serde_json::json!({"name": "https://docs.example.io/concurrency.html"}),
            )
            .await
            .unwrap()
            .result;
        assert!(out.contains("второй файл"), "{out}");
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
            for desc in [
                AttachmentRead.description(loc),
                AttachmentSearch.description(loc),
            ] {
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

    // ---------- attachment_search (stage 3: the chat-scoped index) ----------

    /// Indexes fragments of `att` into the context's chat, embedding them with the
    /// context's own embedder (so a query embedded the same way can match).
    async fn index(ctx: &ToolContext, att: &Attachment, fragments: &[&str]) {
        let texts: Vec<String> = fragments.iter().map(|s| s.to_string()).collect();
        let vectors = ctx
            .embedder
            .embed(texts.clone(), EmbedRole::Passage)
            .await
            .unwrap();
        for (text, embedding) in texts.iter().zip(vectors) {
            let chunk = crate::entities::attachment::AttachmentChunk::new(
                ctx.chat_id,
                att.id,
                &att.name,
                text,
                embedding,
            );
            ctx.storage.db().attachment_insert(&chunk).unwrap();
        }
    }

    #[tokio::test]
    async fn search_finds_the_fragment_by_meaning_and_names_the_file() {
        let doc = att("doc.txt", "неважно — поиск идёт по индексу");
        let (_d, ctx) = ctx_with(vec![doc.clone()]);
        index(
            &ctx,
            &doc,
            &[
                "рецепт борща со свёклой и капустой",
                "инструкция по замене масла в двигателе",
            ],
        )
        .await;

        let out = AttachmentSearch
            .invoke(&ctx, serde_json::json!({"query": "борщ со свёклой"}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("рецепт борща"), "{out}");
        assert!(out.contains("doc.txt"), "the file must be named: {out}");
        // Search says *where*; the page reader stays the guaranteed path.
        assert!(out.contains("attachment_read"), "{out}");
    }

    /// A fragment is a whole chunk and is routinely multi-line, so the result has
    /// to keep the boundaries visible — otherwise ten fragments read as one wall
    /// of text (observed live on a 1.6 MB collection).
    #[tokio::test]
    async fn search_numbers_fragments_and_separates_them() {
        let doc = att("doc.txt", "неважно");
        let (_d, ctx) = ctx_with(vec![doc.clone()]);
        index(
            &ctx,
            &doc,
            &[
                "первая строка первого фрагмента\nвторая строка первого фрагмента",
                "совсем другое содержимое\nв несколько строк",
            ],
        )
        .await;

        let out = AttachmentSearch
            .invoke(&ctx, serde_json::json!({"query": "строка"}))
            .await
            .unwrap()
            .result;
        assert!(
            out.contains("1. [doc.txt]") && out.contains("2. [doc.txt]"),
            "{out}"
        );
        // Each fragment's text starts on its own line, and fragments are set
        // apart by a blank line.
        assert!(out.contains("[doc.txt]\nпервая строка"), "{out}");
        assert!(
            out.contains("\n\n2. ["),
            "fragments must be separated by a blank line: {out}"
        );
    }

    /// An indexing task can finish **after** its file was removed from the chat.
    /// The turn's snapshot is the authority: fragments of a file the user took out
    /// must never reach the model.
    #[tokio::test]
    async fn search_hides_files_that_are_no_longer_attached() {
        let removed = att("removed.txt", "x");
        let kept = att("kept.txt", "y");
        let (_d, ctx) = ctx_with(vec![kept.clone()]);
        index(&ctx, &removed, &["секретный текст удалённого файла"]).await;
        index(&ctx, &kept, &["обычный текст оставшегося файла"]).await;

        let out = AttachmentSearch
            .invoke(&ctx, serde_json::json!({"query": "секретный текст"}))
            .await
            .unwrap()
            .result;
        assert!(
            !out.contains("секретный текст удалённого файла") && !out.contains("removed.txt"),
            "a removed file must not surface: {out}"
        );
    }

    #[tokio::test]
    async fn search_reports_when_nothing_is_indexed() {
        // Attached but not indexed (no embedder at attach time, or inline files
        // only) — the answer must point at the guaranteed path, not read as "the
        // file has nothing about it".
        let (_d, ctx) = ctx_with(vec![att("doc.txt", "текст")]);
        let out = AttachmentSearch
            .invoke(&ctx, serde_json::json!({"query": "что угодно"}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("attachment_read"), "{out}");
    }

    #[tokio::test]
    async fn search_degrades_when_the_embedder_is_gone() {
        // Indexed earlier, embedder unavailable now (ADR 0002 degradation): a
        // clear answer instead of failing the turn.
        struct Dead;
        #[async_trait::async_trait]
        impl crate::shared::api::Embedder for Dead {
            async fn embed(
                &self,
                _texts: Vec<String>,
                _role: EmbedRole,
            ) -> anyhow::Result<Vec<Vec<f32>>> {
                anyhow::bail!("embedder unavailable")
            }
        }
        let doc = att("doc.txt", "текст");
        let (_d, mut ctx) = ctx_with(vec![doc.clone()]);
        index(&ctx, &doc, &["какое-то содержимое"]).await;
        ctx.embedder = std::sync::Arc::new(Dead);

        let out = AttachmentSearch
            .invoke(&ctx, serde_json::json!({"query": "содержимое"}))
            .await
            .unwrap()
            .result;
        assert!(out.contains("attachment_read"), "{out}");
    }

    #[tokio::test]
    async fn search_needs_a_query_and_reports_an_empty_chat() {
        let doc = att("doc.txt", "текст");
        let (_d, ctx) = ctx_with(vec![doc.clone()]);
        index(&ctx, &doc, &["содержимое"]).await;
        assert!(
            AttachmentSearch
                .invoke(&ctx, serde_json::json!({"query": "   "}))
                .await
                .is_err(),
            "an empty query is a usage error"
        );

        let (_d2, empty) = ctx_with(Vec::new());
        let out = AttachmentSearch
            .invoke(&empty, serde_json::json!({"query": "что-нибудь"}))
            .await
            .unwrap()
            .result;
        assert!(!out.is_empty());
    }
}
