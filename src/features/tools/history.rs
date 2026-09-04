//! Read-back tools over the compacted-away part of a conversation (spec §6.7,
//! docs/research/history-compression.md fork F9(b), stage 3).
//!
//! History compression folds the older part of a chat into a rolling summary, so
//! the model stops seeing those messages verbatim. A summary is lossy by
//! construction; these two tools are what turn that loss from permanent into
//! **paged**:
//!
//! - **`history_read`** walks the compressed range page by page. Pages rather
//!   than offsets for the same reason `attachment_read` uses them: they are
//!   discrete and enumerable, so the model can walk `1..M` and *know* it has
//!   read everything.
//! - **`history_search`** finds where to look. It runs over the **full-text
//!   index the application already maintains** (`cache.db`, FTS5 with a trigram
//!   tokenizer) rather than over an embedding index — deliberately (S11): an
//!   embedding server is a separate server (ADR 0002) and is routinely
//!   unconfigured, while the local user with an 8k window is exactly who
//!   compression exists for. Trigram also matches substrings, so `8823` finds
//!   `ZARYA-8823` — identifiers being the first thing a summary loses.
//!
//! Both are **narrower** than any file tool: they reach only this conversation's
//! own older messages, which the user is looking at in the feed the whole time.
//! Hence no gate. They are registered only while the chat actually has a
//! compressed range (S12, [`super::effective_tool_ids`]), which is the same
//! condition that puts the summary block in the prompt — so the block can name
//! them without ever promising a tool that is absent.

use anyhow::Result;

use crate::entities::profile::ToolId;
use crate::features::chat_search::{SNIPPET_BUDGET_CHARS, build_snippet, to_fts_query};
use crate::shared::i18n::Locale;
use crate::shared::storage::cache::IndexScope;

use super::{Tool, ToolContext, ToolOutcome};

/// Tool name (a stable wire protocol — not localized).
pub const HISTORY_READ_ID: &str = "history_read";
/// Tool name (a stable wire protocol — not localized).
pub const HISTORY_SEARCH_ID: &str = "history_search";

/// Default number of hits `history_search` returns.
const DEFAULT_TOP_K: usize = 5;

/// Characters of context around a match. Wider than the chat-search screen's
/// [`SNIPPET_BUDGET_CHARS`] (tuned for two wrapped terminal lines): the reader
/// here is the model, which can use the extra context and pays only tokens for
/// it — and a snippet too tight to be conclusive costs a whole page read.
const SNIPPET_CHARS: usize = SNIPPET_BUDGET_CHARS * 3;

/// The answer when this conversation has nothing compressed — every tool result
/// has to say what *is* possible, or the model improvises (four case studies in
/// docs/lessons.md §4). Here the good news is that nothing is missing at all.
fn nothing_compressed(loc: &Locale) -> ToolOutcome {
    ToolOutcome::text(loc.t("tool.history.none"))
}

/// `history_read` — one page of the compacted-away part of the conversation.
pub struct HistoryRead;

#[async_trait::async_trait]
impl Tool for HistoryRead {
    fn id(&self) -> ToolId {
        HISTORY_READ_ID.into()
    }
    /// Pages a rendered snapshot held in memory
    /// (docs/research/concurrent-tools.md §2.3).
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "read the summarized history"
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.history_read.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": loc.t("tool.history_read.param.page")
                }
            },
            "required": []
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let Some(view) = ctx.history.as_ref() else {
            return Ok(nothing_compressed(ctx.loc));
        };
        let page_tokens = ctx.history_page_tokens;
        let total = view.page_count(page_tokens);
        let page = args
            .get("page")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1);
        let Some(text) = view.page(page_tokens, page) else {
            // Report the real count instead of failing blindly, so the retry can
            // succeed (the `attachment_read` rule).
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.history_read.bad_page",
                &[("page", &page.to_string()), ("total", &total.to_string())],
            )));
        };
        let header = ctx.loc.tf(
            "tool.history_read.header",
            &[("page", &page.to_string()), ("total", &total.to_string())],
        );
        Ok(ToolOutcome::text(format!("{header}\n{text}")))
    }
}

/// `history_search` — full-text search over the compacted-away range.
pub struct HistorySearch;

#[async_trait::async_trait]
impl Tool for HistorySearch {
    fn id(&self) -> ToolId {
        HISTORY_SEARCH_ID.into()
    }
    /// A read of the full-text index over the same snapshot.
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "search the summarized history"
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.history_search.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        super::search_parameters(
            loc,
            "tool.history_search.param.query",
            "tool.history_search.param.top_k",
        )
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let Some(view) = ctx.history.as_ref() else {
            return Ok(nothing_compressed(ctx.loc));
        };
        let (query, k) = super::search_args(
            &args,
            ctx.loc,
            "tool.history_search.err.query_empty",
            DEFAULT_TOP_K,
        )?;

        // Raw input must never reach `MATCH`: `C++`, `cost-benefit` and `50%` are
        // all FTS5 syntax errors on ordinary text. The escaper lives in one place
        // (`features::chat_search`) precisely so a second one cannot drift from
        // it. `None` — every token was below the trigram floor.
        let Some(fts) = to_fts_query(query) else {
            return Ok(ToolOutcome::text(
                ctx.loc.t("tool.history_search.too_short"),
            ));
        };
        let hits = match ctx
            .storage
            .cache()
            .matching_messages_in_chat(&fts, IndexScope::Chat(ctx.chat_id))
        {
            Ok(hits) => hits,
            // The index is disposable and self-healing (deleting `cache.db` is a
            // supported repair), so a failure here degrades to the guaranteed
            // path rather than failing the turn — the `attachment_search` rule.
            Err(err) => {
                tracing::warn!(%err, "history_search: the full-text index is unavailable");
                return Ok(ToolOutcome::text(
                    ctx.loc.t("tool.history_search.unavailable"),
                ));
            }
        };

        let page_tokens = ctx.history_page_tokens;
        // Only matches inside the compressed range: the verbatim tail is already
        // in the prompt, and a hit there would spend the answer on text the model
        // is holding. The index returns hits in an arbitrary order, so they are
        // put back into conversation order — the order the model reads in.
        let mut found: Vec<(usize, usize, &str)> = hits
            .into_iter()
            .filter_map(|id| {
                let order = view.order_of(id)?;
                let (page, block) = view.locate(page_tokens, id)?;
                Some((order, page, block))
            })
            .collect();
        found.sort_by_key(|(order, _, _)| *order);
        found.dedup_by_key(|(order, _, _)| *order);
        if found.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.history_search.empty")));
        }
        let shown = found.len().min(k);
        let mut out = ctx.loc.tf(
            "tool.history_search.header",
            &[
                ("n", &shown.to_string()),
                ("total", &found.len().to_string()),
            ],
        );
        // Numbered, each on its own line, separated by a blank one — a snippet is
        // routinely multi-line, and without that ten of them run together into one
        // wall of text (learned on `attachment_search`, live). Each hit names its
        // **page**: that is what makes the pair compose — search says where to
        // look, `history_read` guarantees everything can be read.
        for (i, (_, page, block)) in found.iter().take(k).enumerate() {
            let snippet = build_snippet(block, query, SNIPPET_CHARS);
            out.push_str("\n\n");
            out.push_str(&ctx.loc.tf(
                "tool.history_search.hit",
                &[("n", &(i + 1).to_string()), ("page", &page.to_string())],
            ));
            out.push('\n');
            out.push_str(&snippet.text);
        }
        out.push_str("\n\n");
        out.push_str(ctx.loc.t("tool.history_search.hint"));
        Ok(ToolOutcome::text(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::{Message, MessageRole, ToolCallRecord};
    use crate::features::compaction::HistoryView;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::storage::cache::IndexedMessage;
    use std::sync::Arc;
    use uuid::Uuid;

    /// A small page so the fixtures span several pages (10 estimated tokens ≈ 40
    /// bytes ≈ 20 Cyrillic characters).
    const PAGE: usize = 10;

    fn assistant_with_call(text: &str, id: &str, name: &str, result: &str) -> Message {
        let mut m = Message::assistant(text);
        m.tool_calls = vec![ToolCallRecord {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({}),
            result: Some(result.into()),
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        m
    }

    fn tool_msg(id: &str, name: &str, text: &str) -> Message {
        let mut m = Message::new(MessageRole::Tool, text);
        m.tool_call_id = Some(id.into());
        m.tool_name = Some(name.into());
        m
    }

    /// A context whose folded range is `messages[..upto]`, with the **whole**
    /// conversation in the full-text index — exactly as the application leaves
    /// it, so a test can tell "outside the folded range" from "not indexed".
    fn ctx_with(messages: &[Message], upto: usize) -> (tempfile::TempDir, ToolContext) {
        let (dir, _storage, mut ctx) = super::super::testkit::ctx_with_storage(Uuid::new_v4());
        let indexed: Vec<IndexedMessage> = messages
            .iter()
            .filter(|m| !m.text.trim().is_empty())
            .map(|m| IndexedMessage {
                id: m.id,
                sub_id: None,
                role: format!("{:?}", m.role).to_lowercase(),
                ts: m.timestamp.to_rfc3339(),
                text: m.text.clone(),
            })
            .collect();
        ctx.storage
            .cache()
            .index_chat(ctx.chat_id, 0, 0, &indexed)
            .unwrap();
        ctx.history = HistoryView::render(&messages[..upto], locale(Lang::Ru)).map(Arc::new);
        ctx.history_page_tokens = PAGE;
        (dir, ctx)
    }

    /// A conversation whose folded part (indices 0..4) carries a planted
    /// identifier inside a **tool result**, and whose verbatim tail mentions a
    /// different one.
    fn conversation() -> Vec<Message> {
        vec![
            Message::user("какой у нас внутренний код сборки"),
            assistant_with_call("сейчас посмотрю", "c1", "fs_read", "сборка ZARYA-8823"),
            tool_msg("c1", "fs_read", "сборка ZARYA-8823"),
            Message::assistant("код сборки нашёлся в файле"),
            Message::user("а теперь про другое: планы на релиз"),
            Message::assistant("релиз назначен на КАЛИНА-4417"),
        ]
    }

    #[tokio::test]
    async fn read_walks_every_page_and_reports_the_range() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let total = ctx.history.as_ref().unwrap().page_count(PAGE);
        assert!(total > 1, "the fixture must span several pages");

        let mut joined = String::new();
        for p in 1..=total {
            let out = HistoryRead
                .invoke(&ctx, serde_json::json!({ "page": p }))
                .await
                .unwrap()
                .result;
            // The header states the range, so the model knows how far to walk.
            assert!(out.contains(&total.to_string()), "{out}");
            joined.push_str(out.split_once('\n').unwrap().1);
        }
        let view = ctx.history.as_ref().unwrap();
        let whole: String = (1..=total).map(|p| view.page(PAGE, p).unwrap()).collect();
        assert_eq!(joined, whole, "walking 1..M reads the folded range whole");
        assert!(
            joined.contains("ZARYA-8823"),
            "the folded text is reachable"
        );
    }

    #[tokio::test]
    async fn read_defaults_to_the_first_page_and_reports_a_bad_one() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let with = HistoryRead
            .invoke(&ctx, serde_json::json!({ "page": 1 }))
            .await
            .unwrap()
            .result;
        let without = HistoryRead
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap()
            .result;
        assert_eq!(with, without);

        let total = ctx.history.as_ref().unwrap().page_count(PAGE);
        let out = HistoryRead
            .invoke(&ctx, serde_json::json!({ "page": 99 }))
            .await
            .unwrap()
            .result;
        assert!(
            out.contains("99") && out.contains(&total.to_string()),
            "{out}"
        );
    }

    /// Nothing folded is the normal state of most chats, and the answer has to
    /// say what *is* possible — here, that nothing is missing at all.
    #[tokio::test]
    async fn both_tools_explain_a_conversation_with_nothing_folded() {
        let msgs = conversation();
        let (_d, mut ctx) = ctx_with(&msgs, 4);
        ctx.history = None;
        for out in [
            HistoryRead
                .invoke(&ctx, serde_json::json!({}))
                .await
                .unwrap()
                .result,
            HistorySearch
                .invoke(&ctx, serde_json::json!({ "query": "ZARYA" }))
                .await
                .unwrap()
                .result,
        ] {
            assert_eq!(out, locale(Lang::Ru).t("tool.history.none"), "{out}");
        }
    }

    /// The property sub-decision S11 rests on: the index the application already
    /// keeps covers `Tool`-role messages at full length, so a search reaches the
    /// tool output that is the bulk of a long conversation — and the hit comes
    /// back with the page that holds it.
    #[tokio::test]
    async fn search_finds_a_tool_result_and_names_its_page() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let out = HistorySearch
            .invoke(&ctx, serde_json::json!({ "query": "ZARYA-8823" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("ZARYA-8823"), "{out}");
        // The page is what lets `history_read` widen the fragment.
        let page = ctx
            .history
            .as_ref()
            .unwrap()
            .locate(PAGE, msgs[2].id)
            .unwrap()
            .0;
        assert!(out.contains(&format!("[страница {page}]")), "{out}");
        assert!(out.contains("history_read"), "{out}");
    }

    /// Trigram matches substrings, which is exactly what an identifier needs —
    /// and what a summary loses first.
    #[tokio::test]
    async fn search_matches_a_substring_of_an_identifier() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let out = HistorySearch
            .invoke(&ctx, serde_json::json!({ "query": "8823" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("ZARYA-8823"), "{out}");
    }

    /// The verbatim tail is already in the prompt: a hit there would spend the
    /// answer on text the model is holding.
    #[tokio::test]
    async fn search_ignores_matches_outside_the_folded_range() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let out = HistorySearch
            .invoke(&ctx, serde_json::json!({ "query": "КАЛИНА-4417" }))
            .await
            .unwrap()
            .result;
        assert!(
            !out.contains("КАЛИНА-4417"),
            "the tail must not surface: {out}"
        );
        assert!(
            out.contains("history_read"),
            "the answer must not dead-end: {out}"
        );
    }

    /// Every refusal points at the guaranteed path rather than reading as "there
    /// is nothing there".
    #[tokio::test]
    async fn a_short_or_missing_query_is_answered_usefully() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        // Below the trigram floor: no token survives, so the index cannot be asked.
        let out = HistorySearch
            .invoke(&ctx, serde_json::json!({ "query": "по" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("history_read"), "{out}");

        assert!(
            HistorySearch
                .invoke(&ctx, serde_json::json!({ "query": "   " }))
                .await
                .is_err(),
            "an empty query is a usage error"
        );

        // Nothing matches — still not a dead end.
        let out = HistorySearch
            .invoke(&ctx, serde_json::json!({ "query": "черепаха" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("history_read"), "{out}");
    }

    /// FTS5 reads punctuation as syntax; raw input reaching `MATCH` is a SQL
    /// error, not a miss (measured when `chat_search`'s escaper was built).
    ///
    /// `is_ok()` is **not** the assertion: the tool degrades a failed index query
    /// into a normal answer, so an unescaped query would pass that check while
    /// silently never running — the first version of this test did exactly that
    /// and survived the mutation. What tells the two apart is *which* answer
    /// comes back, so the query must not land on the "index unavailable" path.
    #[tokio::test]
    async fn a_query_full_of_punctuation_reaches_the_index() {
        let msgs = conversation();
        let (_d, ctx) = ctx_with(&msgs, 4);
        let unavailable = locale(Lang::Ru).t("tool.history_search.unavailable");
        for q in ["C++ и cost-benefit", "50% AND (", "\"кавычки\"", "a:b"] {
            let out = HistorySearch
                .invoke(&ctx, serde_json::json!({ "query": q }))
                .await
                .expect("a punctuated query must not fail the turn")
                .result;
            assert_ne!(out, unavailable, "query {q:?} was not escaped: {out}");
        }
    }

    #[tokio::test]
    async fn descriptions_are_localized() {
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for desc in [HistoryRead.description(loc), HistorySearch.description(loc)] {
                assert!(
                    !desc.contains('{') && !desc.contains('}'),
                    "{lang:?}: {desc}"
                );
                if lang == Lang::En {
                    assert!(
                        !desc.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
                        "Cyrillic leaked into the en description: {desc}"
                    );
                }
            }
        }
    }
}
