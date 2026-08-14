//! Tools over the *other* chats of the current profile (spec §9.11,
//! docs/research/cross-chat-search-tool.md).
//!
//! The chat-list content search (spec §11.2.1) is the user's face of the
//! full-text index; this pair is the model's, with two boundaries the UI stack
//! never needed. The index is **profile-blind** and includes the current chat,
//! so both boundaries live in one place — the turn snapshot
//! [`ToolContext::other_chats`], built by [`snapshot_other_chats`]: the current
//! profile's chats only (spec §9.5), the current chat excluded (its visible
//! half is the model's own context; its folded half belongs to
//! `history_search`), hidden chats dropped. Whatever is absent from the
//! snapshot does not exist for either tool.
//!
//! - **`chat_search`** finds where something was said: hits grouped by
//!   conversation (the UI's "group, don't rank" decision — trigram `bm25` is a
//!   weak relevance proxy), each hit naming the page of that conversation's
//!   transcript it lands on.
//! - **`chat_read`** reads one conversation page by page — the same renderer
//!   ([`HistoryView`]) and page size as `history_read`, which is what makes
//!   the page a hit names the page a read returns.
//!
//! Both are **off by default** (`Tool::enabled_by_default` = `false`): reading
//! across conversations is a deliberate per-profile opt-in, and while it is
//! off the pair is not advertised to the model at all — no schema in the
//! prompt, no temptation (the S12 rationale, spec §9.4).

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::chat::Chat;
use crate::entities::profile::ToolId;
use crate::features::chat_links;
use crate::features::chat_search::{HIT_CAP, SNIPPET_BUDGET_CHARS, build_snippet, to_fts_query};
use crate::features::compaction::HistoryView;
use crate::shared::i18n::Locale;
use crate::shared::storage::cache::MessageHit;

use super::{Tool, ToolContext, ToolOutcome};

/// Tool name (a stable wire protocol — not localized).
pub const CHAT_SEARCH_ID: &str = "chat_search";
/// Tool name (a stable wire protocol — not localized).
pub const CHAT_READ_ID: &str = "chat_read";

/// Default number of hits `chat_search` returns (the shared search contract).
const DEFAULT_TOP_K: usize = 5;

/// Ceiling on `top_k`. `history_search` has none because one conversation
/// bounds it naturally; here the scope is the whole profile, and a snippet is
/// ~[`SNIPPET_CHARS`] characters — twenty of them is already a heavy result.
const MAX_TOP_K: usize = 20;

/// Characters of context around a match — the model-reader budget
/// `history_search` established (three UI snippet widths).
const SNIPPET_CHARS: usize = SNIPPET_BUDGET_CHARS * 3;

/// SQL-level cap on scanned hits, mirroring the UI's [`HIT_CAP`] reasoning: a
/// safety valve, with the honest total counted when it bites.
const SCAN_CAP: usize = HIT_CAP;

/// How many candidates an ambiguous `chat_read` reference lists at most.
const AMBIGUOUS_CAP: usize = 6;

/// One other chat of the profile, as the turn snapshot carries it. Also the
/// address book: `chat_search` scopes its query by these ids, `chat_read`
/// resolves references against them — so a conversation a result names is
/// guaranteed readable by the companion tool.
#[derive(Debug, Clone)]
pub struct ChatRef {
    pub id: Uuid,
    pub title: String,
    /// Last activity, for recency ordering and display.
    pub modified_at: DateTime<Utc>,
}

/// Builds the turn's [`ChatRef`] snapshot from the orchestrator's chat list:
/// this profile's chats, minus the current one, minus hidden ones. The single
/// place the scope is decided (spec §9.5, §9.11) — the tools trust the
/// snapshot blindly, so every boundary must hold here.
pub fn snapshot_other_chats(chats: &[Chat], profile_id: Uuid, current: Uuid) -> Vec<ChatRef> {
    chats
        .iter()
        .filter(|c| c.profile_id == profile_id && c.id != current && !c.is_hidden)
        .map(|c| ChatRef {
            id: c.id,
            title: c.title.clone(),
            modified_at: c.modified_at,
        })
        .collect()
}

/// The address shown next to a conversation — `chat://` plus a prefix of the
/// uuid. The same string the model cites back to the user and the feed turns
/// into a link (spec §11.3), so it has a single producer in
/// [`chat_links`](crate::features::chat_links).
fn address(id: Uuid) -> String {
    chat_links::uri(id)
}

/// The date half of an RFC 3339 timestamp the index stores (locale-neutral —
/// the reader is the model).
fn date_of(ts: &str) -> &str {
    ts.split_once('T').map_or(ts, |(d, _)| d)
}

/// The answer when the profile has no other chats at all — "nothing here", as
/// distinct from "no hits" (docs/lessons.md §4). Shared by the pair like
/// `tool.history.none`.
fn no_other_chats(loc: &Locale) -> ToolOutcome {
    ToolOutcome::text(loc.t("tool.chats.none"))
}

/// What a `chat` reference resolved to.
enum Resolved<'a> {
    One(&'a ChatRef),
    Ambiguous(Vec<&'a ChatRef>),
    None,
}

/// Resolves a `chat_read` reference: id prefix first (the address
/// `chat_search` prints — robust under duplicate titles), then exact title,
/// then title substring, case-folded. Several matches at any rung stop there
/// and report the ambiguity (the `attachment_read` rule: reading a *different*
/// conversation than the one asked for and saying nothing would be worse than
/// asking again).
///
/// The id rung reads **both** forms of the address — `chat://a1b2c3d4` and the
/// bare `a1b2c3d4` — because a model taught to cite the scheme hands the
/// scheme back ([`chat_links::hex_needle`]).
fn resolve<'a>(refs: &'a [ChatRef], needle: &str) -> Resolved<'a> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Resolved::None;
    }
    if let Some(hex) = chat_links::hex_needle(needle) {
        let hits: Vec<&ChatRef> = refs
            .iter()
            .filter(|r| r.id.simple().to_string().starts_with(&hex))
            .collect();
        match hits.len() {
            1 => return Resolved::One(hits[0]),
            0 => {} // fall through: a hex-looking title reference
            _ => return Resolved::Ambiguous(hits),
        }
    }
    let folded = needle.to_lowercase();
    let exact: Vec<&ChatRef> = refs
        .iter()
        .filter(|r| r.title.trim().to_lowercase() == folded)
        .collect();
    match exact.len() {
        1 => return Resolved::One(exact[0]),
        0 => {}
        _ => return Resolved::Ambiguous(exact),
    }
    let sub: Vec<&ChatRef> = refs
        .iter()
        .filter(|r| r.title.to_lowercase().contains(&folded))
        .collect();
    match sub.len() {
        1 => Resolved::One(sub[0]),
        0 => Resolved::None,
        _ => Resolved::Ambiguous(sub),
    }
}

/// `chat_search` — full-text search across the other chats of the profile.
pub struct ChatSearch;

#[async_trait::async_trait]
impl Tool for ChatSearch {
    fn id(&self) -> ToolId {
        CHAT_SEARCH_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "search other conversations"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.chat_search.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        super::search_parameters(
            loc,
            "tool.chat_search.param.query",
            "tool.chat_search.param.top_k",
        )
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        if ctx.other_chats.is_empty() {
            return Ok(no_other_chats(ctx.loc));
        }
        let (query, k) = super::search_args(
            &args,
            ctx.loc,
            "tool.chat_search.err.query_empty",
            DEFAULT_TOP_K,
        )?;
        let k = k.min(MAX_TOP_K);

        // Raw input must never reach `MATCH` (FTS5 reads it as syntax); the
        // escaper lives in one place — `features::chat_search`.
        let Some(fts) = to_fts_query(query) else {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.chat_search.too_short")));
        };
        let ids: Vec<Uuid> = ctx.other_chats.iter().map(|r| r.id).collect();
        let hits = match ctx.storage.cache().search_messages_in(&fts, &ids, SCAN_CAP) {
            Ok(hits) => hits,
            // The index is disposable and self-healing; degrade to the routes
            // that still work rather than failing the turn.
            Err(err) => {
                tracing::warn!(%err, "chat_search: the full-text index is unavailable");
                return Ok(ToolOutcome::text(ctx.loc.t("tool.chat_search.unavailable")));
            }
        };
        if hits.is_empty() {
            return Ok(ToolOutcome::text(ctx.loc.t("tool.chat_search.empty")));
        }
        let total = if hits.len() >= SCAN_CAP {
            // Only pay for the count when the cap actually bit (the
            // orchestrator's rule for the UI search).
            ctx.storage
                .cache()
                .count_matching_messages_in(&fts, &ids)
                .unwrap_or(hits.len())
        } else {
            hits.len()
        };

        // Group by conversation, conversations by recency, hits inside one by
        // timestamp (the index's insertion order is only approximate).
        let mut by_chat: std::collections::HashMap<Uuid, Vec<MessageHit>> =
            std::collections::HashMap::new();
        for hit in hits {
            by_chat.entry(hit.chat_id).or_default().push(hit);
        }
        let mut refs: Vec<&ChatRef> = ctx.other_chats.iter().collect();
        refs.sort_by_key(|r| std::cmp::Reverse(r.modified_at));

        let mut body = String::new();
        let mut shown = 0usize;
        for chat_ref in refs {
            if shown >= k {
                break;
            }
            let Some(mut chat_hits) = by_chat.remove(&chat_ref.id) else {
                continue;
            };
            chat_hits.sort_by(|a, b| a.ts.cmp(&b.ts));
            // One load + render per shown conversation gives every hit its page
            // address — what makes the pair compose the way the history pair
            // does. Best-effort: a chat that fails to load still yields its
            // snippets, just without page numbers.
            let view = ctx
                .storage
                .json()
                .load_chat(chat_ref.id)
                .ok()
                .flatten()
                .and_then(|c| HistoryView::render(&c.messages, ctx.loc));
            body.push_str("\n\n");
            body.push_str(&ctx.loc.tf(
                "tool.chat_search.chat",
                &[
                    ("title", chat_ref.title.as_str()),
                    ("id", &address(chat_ref.id)),
                    ("date", &chat_ref.modified_at.format("%Y-%m-%d").to_string()),
                ],
            ));
            for hit in chat_hits {
                if shown >= k {
                    break;
                }
                shown += 1;
                let snippet = build_snippet(&hit.text, query, SNIPPET_CHARS);
                let page = view
                    .as_ref()
                    .and_then(|v| v.locate(ctx.history_page_tokens, hit.message_id))
                    .map(|(page, _)| page);
                body.push_str("\n\n");
                let n = shown.to_string();
                let date = date_of(&hit.ts);
                body.push_str(&match page {
                    Some(page) => ctx.loc.tf(
                        "tool.chat_search.hit",
                        &[
                            ("n", n.as_str()),
                            ("role", &hit.role),
                            ("date", date),
                            ("page", &page.to_string()),
                        ],
                    ),
                    None => ctx.loc.tf(
                        "tool.chat_search.hit_unpaged",
                        &[("n", n.as_str()), ("role", &hit.role), ("date", date)],
                    ),
                });
                body.push('\n');
                body.push_str(&snippet.text);
            }
        }

        let mut out = ctx.loc.tf(
            "tool.chat_search.header",
            &[("n", &shown.to_string()), ("total", &total.to_string())],
        );
        out.push_str(&body);
        out.push_str("\n\n");
        out.push_str(ctx.loc.t("tool.chat_search.hint"));
        Ok(ToolOutcome::text(out))
    }
}

/// `chat_read` — one page of another conversation's transcript.
pub struct ChatRead;

#[async_trait::async_trait]
impl Tool for ChatRead {
    fn id(&self) -> ToolId {
        CHAT_READ_ID.into()
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "read another conversation"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.chat_read.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        super::paged_read_parameters(
            loc,
            "chat",
            "tool.chat_read.param.chat",
            "tool.chat_read.param.page",
        )
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        if ctx.other_chats.is_empty() {
            return Ok(no_other_chats(ctx.loc));
        }
        let reference = args
            .get("chat")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if reference.is_empty() {
            // A usage error, like an empty search query: the call was wrong,
            // and saying so is what lets the next one succeed.
            anyhow::bail!(ctx.loc.t("tool.chat_read.err.chat_empty").to_string());
        }
        let chat_ref = match resolve(&ctx.other_chats, reference) {
            Resolved::One(r) => r,
            Resolved::Ambiguous(candidates) => {
                let listed: Vec<String> = candidates
                    .iter()
                    .take(AMBIGUOUS_CAP)
                    .map(|r| format!("\"{}\" {}", r.title, address(r.id)))
                    .collect();
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.chat_read.ambiguous",
                    &[("chat", reference), ("candidates", &listed.join("; "))],
                )));
            }
            Resolved::None => {
                return Ok(ToolOutcome::text(
                    ctx.loc.tf("tool.chat_read.unknown", &[("chat", reference)]),
                ));
            }
        };
        let loaded = ctx.storage.json().load_chat(chat_ref.id).ok().flatten();
        // Belt and braces under the snapshot: the scope is decided in
        // `snapshot_other_chats`, but a conversation whose file now says
        // "different profile" or "hidden" must not be read on the strength of
        // a stale snapshot — refuse as unloadable rather than leak.
        let chat = match loaded {
            Some(c) if c.profile_id == ctx.profile_id && c.id != ctx.chat_id && !c.is_hidden => c,
            _ => {
                return Ok(ToolOutcome::text(ctx.loc.tf(
                    "tool.chat_read.unavailable",
                    &[("title", chat_ref.title.as_str())],
                )));
            }
        };
        let Some(view) = HistoryView::render(&chat.messages, ctx.loc) else {
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.chat_read.empty",
                &[("title", chat_ref.title.as_str())],
            )));
        };
        let page_tokens = ctx.history_page_tokens;
        let total = view.page_count(page_tokens);
        let page = args
            .get("page")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1);
        let Some(text) = view.page(page_tokens, page) else {
            // Report the real count instead of failing blindly, so the retry
            // can succeed (the `attachment_read` rule).
            return Ok(ToolOutcome::text(ctx.loc.tf(
                "tool.chat_read.bad_page",
                &[
                    ("page", &page.to_string()),
                    ("title", chat_ref.title.as_str()),
                    ("total", &total.to_string()),
                ],
            )));
        };
        let header = ctx.loc.tf(
            "tool.chat_read.header",
            &[
                ("title", chat_ref.title.as_str()),
                ("id", &address(chat_ref.id)),
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
    use crate::entities::message::Message;
    use crate::entities::profile::Profile;
    use crate::features::chat_links::short_id;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::storage::Storage;
    use crate::shared::storage::cache::IndexedMessage;
    use std::sync::Arc;

    /// A small page so multi-message chats span several pages.
    const PAGE: usize = 10;

    /// Builds a chat of `profile_id` on disk **and** in the index, exactly as
    /// the application leaves it, and returns its snapshot entry.
    fn seed_chat(
        storage: &Storage,
        profile_id: Uuid,
        title: &str,
        texts: &[&str],
    ) -> (ChatRef, Chat) {
        let mut profile = Profile::new("p", "");
        profile.id = profile_id;
        let mut chat = Chat::from_profile(&profile, title);
        for text in texts {
            chat.push_message(Message::user(*text));
        }
        storage.json().save_chat(&chat).unwrap();
        let indexed: Vec<IndexedMessage> = chat
            .messages
            .iter()
            .map(|m| IndexedMessage {
                id: m.id,
                role: "user".into(),
                ts: m.timestamp.to_rfc3339(),
                text: m.text.clone(),
            })
            .collect();
        storage.cache().index_chat(chat.id, 0, 0, &indexed).unwrap();
        let chat_ref = ChatRef {
            id: chat.id,
            title: chat.title.clone(),
            modified_at: chat.modified_at,
        };
        (chat_ref, chat)
    }

    /// A context whose snapshot holds the given refs, with `PAGE`-sized pages.
    fn ctx_with_refs(
        refs: Vec<ChatRef>,
    ) -> (tempfile::TempDir, Arc<Storage>, super::super::ToolContext) {
        let (dir, storage, mut ctx) = super::super::testkit::ctx_with_storage(Uuid::new_v4());
        ctx.other_chats = Arc::from(refs);
        ctx.history_page_tokens = PAGE;
        (dir, storage, ctx)
    }

    #[test]
    fn snapshot_scopes_profile_current_and_hidden() {
        let profile = Uuid::new_v4();
        let mut p = Profile::new("p", "");
        p.id = profile;
        let current = Chat::from_profile(&p, "current");
        let other = Chat::from_profile(&p, "other");
        let mut hidden = Chat::from_profile(&p, "hidden");
        hidden.is_hidden = true;
        let mut foreign_p = Profile::new("q", "");
        foreign_p.id = Uuid::new_v4();
        let foreign = Chat::from_profile(&foreign_p, "foreign");

        let chats = vec![current.clone(), other.clone(), hidden, foreign];
        let refs = snapshot_other_chats(&chats, profile, current.id);
        assert_eq!(
            refs.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![other.id],
            "only the profile's other visible chat may be in scope"
        );
    }

    #[tokio::test]
    async fn search_reaches_only_the_snapshot() {
        // The index is profile-blind; the snapshot is the boundary. A chat of
        // another profile holds the same phrase and must not surface even
        // though its rows sit in the same index.
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let (mine, _) = seed_chat(
            &storage,
            ctx.profile_id,
            "план запуска",
            &["код запуска РЕКА-7731 назначен на пятницу"],
        );
        let (_foreign, _) = seed_chat(
            &storage,
            Uuid::new_v4(),
            "чужой чат",
            &["код запуска РЕКА-7731 упомянут и здесь"],
        );
        ctx.other_chats = Arc::from(vec![mine]);

        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "РЕКА-7731" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("план запуска"), "{out}");
        assert!(!out.contains("чужой чат"), "{out}");
        assert!(out.contains("РЕКА-7731"), "{out}");
        assert!(
            out.contains("chat_read"),
            "the hint must name the reader: {out}"
        );
    }

    #[tokio::test]
    async fn search_groups_by_chat_and_names_pages() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let profile = ctx.profile_id;
        let (older, _) = seed_chat(
            &storage,
            profile,
            "старый разговор",
            &["про сорт яблок АНТОНОВКА и про погоду"],
        );
        let (newer_ref, newer_chat) = seed_chat(
            &storage,
            profile,
            "новый разговор",
            &[
                "первая страница ни о чём, просто длинный текст для объёма страницы",
                "яблоки сорта АНТОНОВКА обсуждались и тут",
            ],
        );
        ctx.other_chats = Arc::from(vec![older.clone(), newer_ref.clone()]);

        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "АНТОНОВКА" }))
            .await
            .unwrap()
            .result;
        // Both conversations surface, newest first, addressed by short id.
        assert!(out.contains("старый разговор"), "{out}");
        assert!(out.contains("новый разговор"), "{out}");
        assert!(out.contains(&short_id(newer_ref.id)), "{out}");
        let newer_at = out.find("новый разговор").unwrap();
        let older_at = out.find("старый разговор").unwrap();
        assert!(newer_at < older_at, "recency must order the groups: {out}");
        // The page the hit names is the page chat_read returns.
        let view = HistoryView::render(&newer_chat.messages, locale(Lang::Ru)).unwrap();
        let (page, _) = view.locate(PAGE, newer_chat.messages[1].id).unwrap();
        assert!(page > 1, "the fixture must push the hit off page 1");
        assert!(
            out.contains(&format!("страница {page}")),
            "the hit must name its page: {out}"
        );
    }

    #[tokio::test]
    async fn search_caps_hits_and_reports_the_honest_total() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let texts: Vec<String> = (1..=4)
            .map(|i| format!("повтор ГРАНАТ номер {i}"))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let (r, _) = seed_chat(&storage, ctx.profile_id, "гранаты", &text_refs);
        ctx.other_chats = Arc::from(vec![r]);

        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "ГРАНАТ", "top_k": 2 }))
            .await
            .unwrap()
            .result;
        let header = out.lines().next().unwrap();
        assert!(header.contains('2') && header.contains('4'), "{header}");
        assert_eq!(out.matches("ГРАНАТ").count(), 2, "{out}");
    }

    #[tokio::test]
    async fn nothing_here_and_no_hits_are_different_answers() {
        // "This profile has no other chats" and "no matches" call for different
        // next actions, so they must be different answers (docs/lessons.md §4).
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "что-нибудь" }))
            .await
            .unwrap()
            .result;
        assert_eq!(out, locale(Lang::Ru).t("tool.chats.none"), "{out}");

        let (r, _) = seed_chat(&storage, ctx.profile_id, "пустышка", &["ни о чём"]);
        ctx.other_chats = Arc::from(vec![r]);
        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "черепаха" }))
            .await
            .unwrap()
            .result;
        assert_eq!(out, locale(Lang::Ru).t("tool.chat_search.empty"), "{out}");
    }

    /// FTS5 reads punctuation as syntax; the escaped query must reach the
    /// index (`is_ok()` alone would pass on the degrade path — the
    /// `history_search` lesson).
    #[tokio::test]
    async fn a_query_full_of_punctuation_reaches_the_index() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let (r, _) = seed_chat(&storage, ctx.profile_id, "чат", &["обычный текст"]);
        ctx.other_chats = Arc::from(vec![r]);
        let unavailable = locale(Lang::Ru).t("tool.chat_search.unavailable");
        for q in ["C++ и cost-benefit", "50% AND (", "\"кавычки\"", "a:b"] {
            let out = ChatSearch
                .invoke(&ctx, serde_json::json!({ "query": q }))
                .await
                .expect("a punctuated query must not fail the turn")
                .result;
            assert_ne!(out, unavailable, "query {q:?} was not escaped: {out}");
        }
        let out = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "по" }))
            .await
            .unwrap()
            .result;
        assert_eq!(out, locale(Lang::Ru).t("tool.chat_search.too_short"));
        assert!(
            ChatSearch
                .invoke(&ctx, serde_json::json!({ "query": "  " }))
                .await
                .is_err(),
            "an empty query is a usage error"
        );
    }

    #[tokio::test]
    async fn read_resolves_by_id_title_and_reports_ambiguity() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let profile = ctx.profile_id;
        let (a, chat_a) = seed_chat(
            &storage,
            profile,
            "заметки о рыбалке",
            &["щука клюёт на живца"],
        );
        let (b, _) = seed_chat(&storage, profile, "заметки о грибах", &["опята в сентябре"]);
        ctx.other_chats = Arc::from(vec![a.clone(), b.clone()]);

        // Short id, the address chat_search prints.
        let out = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": short_id(a.id) }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("щука"), "{out}");
        assert!(out.contains(&chat_a.title), "{out}");

        // Exact title.
        let out = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": "заметки о грибах" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("опята"), "{out}");

        // A substring both titles share — ambiguity listing both addresses.
        let out = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": "заметки" }))
            .await
            .unwrap()
            .result;
        assert!(
            out.contains(&short_id(a.id)) && out.contains(&short_id(b.id)),
            "the ambiguity must list the candidates' addresses: {out}"
        );
        assert!(
            !out.contains("щука"),
            "no content on an ambiguous ask: {out}"
        );

        // Unknown — the door points back at chat_search.
        let out = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": "нет такого" }))
            .await
            .unwrap()
            .result;
        assert!(out.contains("chat_search"), "{out}");

        // Empty — a usage error.
        assert!(
            ChatRead
                .invoke(&ctx, serde_json::json!({ "chat": "  " }))
                .await
                .is_err()
        );
    }

    /// The scheme the model is taught to cite must survive the round trip: a
    /// `chat://` address handed back to `chat_read` is the same address it was
    /// given. Without the scheme strip this falls through the id rung into
    /// title matching and answers "unknown" (docs/research/chat-uri-links.md
    /// §3, gap 4).
    #[tokio::test]
    async fn read_accepts_its_own_chat_uri() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let profile = ctx.profile_id;
        let (a, _) = seed_chat(&storage, profile, "рыбалка", &["щука клюёт на живца"]);
        ctx.other_chats = Arc::from(vec![a.clone()]);

        for reference in [
            chat_links::uri(a.id),
            chat_links::uri(a.id).to_uppercase(),
            short_id(a.id),
            a.id.to_string(),
        ] {
            let out = ChatRead
                .invoke(&ctx, serde_json::json!({ "chat": reference }))
                .await
                .unwrap()
                .result;
            assert!(out.contains("щука"), "{reference}: {out}");
        }
    }

    /// Every address the pair prints carries the scheme — one form of an
    /// address everywhere, so the model never translates between what it was
    /// given and what it should cite (fork F7).
    #[tokio::test]
    async fn printed_addresses_carry_the_scheme() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let profile = ctx.profile_id;
        let (a, _) = seed_chat(&storage, profile, "рыбалка", &["щука клюёт на живца"]);
        let (b, _) = seed_chat(&storage, profile, "рыбалка зимой", &["окунь подо льдом"]);
        ctx.other_chats = Arc::from(vec![a.clone(), b.clone()]);
        let expected = chat_links::uri(a.id);

        let search = ChatSearch
            .invoke(&ctx, serde_json::json!({ "query": "щука" }))
            .await
            .unwrap()
            .result;
        assert!(search.contains(&expected), "search header: {search}");

        let read = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": expected.clone() }))
            .await
            .unwrap()
            .result;
        assert!(read.contains(&expected), "read header: {read}");

        // The ambiguity ladder addresses its candidates the same way.
        let ambiguous = ChatRead
            .invoke(&ctx, serde_json::json!({ "chat": "рыбал" }))
            .await
            .unwrap()
            .result;
        assert!(
            ambiguous.contains(&expected) && ambiguous.contains(&chat_links::uri(b.id)),
            "candidates: {ambiguous}"
        );
    }

    #[tokio::test]
    async fn read_pages_walk_the_whole_transcript() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let (r, chat) = seed_chat(
            &storage,
            ctx.profile_id,
            "длинный",
            &[
                "первое сообщение достаточно длинное для страницы",
                "второе сообщение тоже не короткое совсем",
                "третье сообщение с кодом ВИШНЯ-2210 внутри",
            ],
        );
        ctx.other_chats = Arc::from(vec![r.clone()]);
        let view = HistoryView::render(&chat.messages, locale(Lang::Ru)).unwrap();
        let total = view.page_count(PAGE);
        assert!(total > 1, "the fixture must span several pages");

        let mut joined = String::new();
        for page in 1..=total {
            let out = ChatRead
                .invoke(
                    &ctx,
                    serde_json::json!({ "chat": short_id(r.id), "page": page }),
                )
                .await
                .unwrap()
                .result;
            assert!(out.contains(&total.to_string()), "{out}");
            joined.push_str(out.split_once('\n').unwrap().1);
        }
        assert!(joined.contains("ВИШНЯ-2210"), "walking 1..M reads it all");

        let out = ChatRead
            .invoke(
                &ctx,
                serde_json::json!({ "chat": short_id(r.id), "page": 99 }),
            )
            .await
            .unwrap()
            .result;
        assert!(
            out.contains("99") && out.contains(&total.to_string()),
            "a bad page must report the real range: {out}"
        );
    }

    /// The belt under the snapshot: a stale entry whose file now belongs to
    /// another profile (or is hidden) is refused, not read.
    #[tokio::test]
    async fn read_refuses_a_stale_cross_profile_reference() {
        let (_d, storage, mut ctx) = ctx_with_refs(vec![]);
        let (foreign_ref, _) = seed_chat(
            &storage,
            Uuid::new_v4(),
            "чужой",
            &["секретный код ЛАВАНДА-9042"],
        );
        ctx.other_chats = Arc::from(vec![foreign_ref.clone()]);

        let out = ChatRead
            .invoke(
                &ctx,
                serde_json::json!({ "chat": short_id(foreign_ref.id) }),
            )
            .await
            .unwrap()
            .result;
        assert!(!out.contains("ЛАВАНДА-9042"), "must not leak: {out}");
        assert_eq!(
            out,
            locale(Lang::Ru).tf("tool.chat_read.unavailable", &[("title", "чужой")]),
            "{out}"
        );
    }

    #[tokio::test]
    async fn both_tools_explain_an_empty_profile() {
        let (_d, _s, ctx) = ctx_with_refs(vec![]);
        for out in [
            ChatSearch
                .invoke(&ctx, serde_json::json!({ "query": "что угодно" }))
                .await
                .unwrap()
                .result,
            ChatRead
                .invoke(&ctx, serde_json::json!({ "chat": "любой" }))
                .await
                .unwrap()
                .result,
        ] {
            assert_eq!(out, locale(Lang::Ru).t("tool.chats.none"), "{out}");
        }
    }

    #[test]
    fn the_pair_is_optional_and_catalogued() {
        // Off by default is the design's core promise: present in the catalog
        // (so the settings screen can offer it), absent from the default set
        // (so no profile gets it without the user's hand).
        let all = super::super::all_tool_ids();
        let default = super::super::default_tool_ids();
        for id in [CHAT_SEARCH_ID, CHAT_READ_ID] {
            assert!(all.iter().any(|t| t == id), "{id} missing from the catalog");
            assert!(
                !default.iter().any(|t| t == id),
                "{id} must not be enabled by default"
            );
        }
    }

    #[tokio::test]
    async fn descriptions_are_localized() {
        for &lang in Lang::ALL {
            let loc = locale(lang);
            for desc in [ChatSearch.description(loc), ChatRead.description(loc)] {
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
