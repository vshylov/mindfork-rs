//! `branch_read` and `branch_search` — the reader and the search of a
//! conversation's parked branches (spec §9.17, docs/research/chat-branches.md
//! §9, §10.2).
//!
//! Every other tool reads a conversation's **line**: the request, the folded
//! part behind `history_read`, another chat's transcript behind `chat_read`,
//! the full-text index behind the two searches. The tails that left the line
//! ([`Chat::branches`](crate::entities::chat::Chat::branches)) are reached
//! through these two alone, by a call that asks for them — a fork means
//! "continue as if what followed had not been said", so a parked tail is read
//! on request and is never something a search of a line turns up.
//!
//! **`branch_read`:**
//!
//! - **No `branch`** — the list, in the order `/branch` shows: a tail, then the
//!   tails that continue from inside it. An entry quotes the place the tail
//!   continues from and its own first line, because the request carries no
//!   message numbers — `#11` alone would tell the model nothing.
//! - **`branch`** — one tail as a transcript, through the renderer and the
//!   pager `chat_read` uses.
//! - **`chat`** — another conversation's branches, on a turn that has the
//!   cross-chat snapshot ([`ToolContext::other_chats`]): crossing
//!   conversations stays the consent of the tools that own that snapshot.
//!
//! **An address is what `/branch N K` takes**, written `N/K`: the number the
//! tail's first bubble has on its line and its variant among the variants of
//! that place — the figure a message's mark shows. A tail inside another is
//! addressed by the path from the line (`5/1 > 7/1`).
//!
//! **`branch_search`** finds the tail that says a word, where the list shows
//! first lines: the words of a query matched against the text of every parked
//! message of one conversation, by the rule the other searches use
//! ([`QueryWords`]). It scans — the full-text index keeps the parked rows in
//! tables that are the person's search's alone, which no tool's query reads
//! (`shared/storage/cache`) — and a hit names its tail's address and the page
//! of that tail's transcript, which is what the reader takes.
//!
//! **Both are off by default** (`enabled_by_default` = `false`), each a switch
//! of its own: since `Ctrl+R` and `Ctrl+E` park what they cut, a tail is also
//! a reply the person had redone or an exchange they took back, and reading
//! those back — a snippet as much as a page — is a switch the person makes.
//! While the reader is off, nothing says a chat has branches: the notice the
//! two reads of a line carry names it, so it is said only on a turn that
//! offers it ([`ToolContext::branch_reader`]). By the same rule a search hit
//! names the reader, and the reader's list the search, only on a turn that
//! has the other ([`ToolContext::branch_search`]).

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::entities::attachment::paginate;
use crate::entities::chat::Chat;
use crate::entities::chat_branch::{BranchPath, ParkCause, branch_address as address};
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::ToolId;
use crate::features::chat_search::{
    QueryWords, Relaxed, SNIPPET_BUDGET_CHARS, build_word_snippet, relax_query,
};
use crate::features::compaction::HistoryView;
use crate::shared::i18n::Locale;

use super::chats::{self, ChatRef, Resolved};
use super::{Tool, ToolContext, ToolOutcome};

/// Tool name (a stable wire protocol — not localized).
pub const BRANCH_READ_ID: &str = "branch_read";
/// Tool name (a stable wire protocol — not localized).
pub const BRANCH_SEARCH_ID: &str = "branch_search";

/// How many fragments a search returns unless asked — the other searches'
/// default.
const DEFAULT_TOP_K: usize = 5;

/// The most fragments one search returns, whatever `top_k` asks: each is up to
/// [`SNIPPET_CHARS`] characters (`chat_search`'s ceiling).
const MAX_TOP_K: usize = 20;

/// A fragment's width — the other search tools' (three of the UI's snippets).
const SNIPPET_CHARS: usize = SNIPPET_BUDGET_CHARS * 3;

/// Characters of a quoted first line: enough to recognize a message by, short
/// enough that a list of many tails stays a list.
const QUOTE_CHARS: usize = 160;

/// How many addresses an answer to an unknown one names at most.
const KNOWN_CAP: usize = 12;

/// How many candidates an ambiguous `chat` reference lists at most.
const AMBIGUOUS_CAP: usize = 6;

/// One parked tail, as the reader lists and reads it.
pub struct ParkedBranch {
    path: BranchPath,
    /// How many variants its place has, the line's own among them.
    of: u16,
    cause: ParkCause,
    parked_at: DateTime<Utc>,
    /// Its messages, counted as the feed draws them.
    bubbles: usize,
    /// Its anchor names no message that can reach the line.
    detached: bool,
    /// The last thing said before it, where it continues from one: who, and
    /// the first line. `None` — it starts the conversation, or is detached.
    after: Option<(MessageRole, String)>,
    /// Its own first line with text.
    begins: Option<(MessageRole, String)>,
    /// Its transcript. `None` — nothing in it renders (an empty round).
    view: Option<HistoryView>,
    /// The text of its messages, in the order they were said — what the search
    /// matches. A message's own text and not its rendered block: the block
    /// begins with the speaker's label, which a query for "user" would match
    /// in every one. It is what the full-text index holds of a line.
    said: Vec<Said>,
}

/// One parked message with text, as the search reads it.
struct Said {
    id: uuid::Uuid,
    role: MessageRole,
    at: DateTime<Utc>,
    text: String,
}

/// A search hit: the tail, the message, and the page of the tail's transcript
/// the message is on (`None` when the tail does not render).
struct Hit<'a> {
    branch: &'a ParkedBranch,
    said: &'a Said,
    page: Option<usize>,
}

/// The parked branches of one conversation, in `/branch`'s order — the turn
/// snapshot behind `branch_read` for the open chat
/// ([`ToolContext::branches`]), and what a call builds from another chat's
/// file.
///
/// Rendered to text when it is made: the orchestrator stays the sole owner of
/// `Chat`, a message carries its images inline, and the chat's file can be
/// behind the chat in memory — the reasons the folded part reaches its tools
/// as a rendered snapshot too ([`ToolContext::history`]).
pub struct BranchShelf {
    branches: Vec<ParkedBranch>,
}

impl BranchShelf {
    /// The shelf of `chat`'s parked tails; `None` when it has none.
    pub fn of(chat: &Chat, loc: &Locale) -> Option<Self> {
        let view = chat.branch_view();
        let branches: Vec<ParkedBranch> = view
            .parked
            .iter()
            .zip(view.paths())
            .filter_map(|(info, path)| {
                let branch = chat
                    .branches
                    .iter()
                    .find(|b| b.first() == Some(info.first))?;
                Some(ParkedBranch {
                    path,
                    of: info.of,
                    cause: branch.cause,
                    parked_at: branch.parked_at,
                    bubbles: info.bubbles,
                    detached: info.detached,
                    after: branch
                        .after
                        .filter(|_| !info.detached)
                        .and_then(|id| said_before(chat, id)),
                    begins: first_said(branch.messages.iter()),
                    view: HistoryView::render(&branch.messages, loc),
                    said: branch
                        .messages
                        .iter()
                        .filter(|m| !m.text.trim().is_empty())
                        .map(|m| Said {
                            id: m.id,
                            role: m.role,
                            at: m.timestamp,
                            text: m.text.clone(),
                        })
                        .collect(),
                })
            })
            .collect();
        (!branches.is_empty()).then_some(Self { branches })
    }

    /// How many tails are parked.
    pub fn len(&self) -> usize {
        self.branches.len()
    }

    fn find(&self, path: &[(u32, u16)]) -> Option<&ParkedBranch> {
        self.branches.iter().find(|b| b.path == path)
    }

    /// What an address that names no parked tail does name: the variant that
    /// stands at its place — on the line, or inside the tail it is under —
    /// or nothing.
    fn standing<'p>(&self, path: &'p [(u32, u16)]) -> Standing<'p> {
        let Some((&(number, variant), parent)) = path.split_last() else {
            return Standing::Nothing;
        };
        if !parent.is_empty() && self.find(parent).is_none() {
            return Standing::Nothing;
        }
        // The tails parked at that very place say how many variants it has; a
        // figure within that count that no tail carries is the one standing.
        let of = self
            .branches
            .iter()
            .filter(|b| !b.detached)
            .filter(|b| {
                b.path
                    .split_last()
                    .is_some_and(|(&(n, _), p)| n == number && p == parent)
            })
            .map(|b| b.of)
            .next();
        match of {
            Some(of) if (1..=of).contains(&variant) => {
                if parent.is_empty() {
                    Standing::OnLine
                } else {
                    Standing::InParent(parent)
                }
            }
            _ => Standing::Nothing,
        }
    }

    /// The parked messages that hold every word of `words`: tails in the
    /// list's order, a tail's messages in the order they were said.
    fn matching(&self, words: &QueryWords, page_tokens: usize) -> Vec<Hit<'_>> {
        self.branches
            .iter()
            .flat_map(|branch| {
                branch
                    .said
                    .iter()
                    .filter(|said| words.matches(&said.text))
                    .map(move |said| Hit {
                        branch,
                        said,
                        page: branch
                            .view
                            .as_ref()
                            .and_then(|v| v.locate(page_tokens, said.id))
                            .map(|(page, _)| page),
                    })
            })
            .collect()
    }

    /// The list's body: one entry per tail, a blank line between them.
    fn list(&self, loc: &Locale) -> String {
        let mut out = String::new();
        for (i, b) in self.branches.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            let address = address(&b.path);
            let (number, variant) = b.path.last().copied().unwrap_or_default();
            let (i, count, when) = (
                (i + 1).to_string(),
                b.bubbles.to_string(),
                date(b.parked_at),
            );
            let (number, variant, of) = (number.to_string(), variant.to_string(), b.of.to_string());
            let parent = address_of_parent(&b.path);
            // `tf` takes exactly the arguments its template names, so each
            // shape of entry brings its own on top of the shared ones.
            let mut args: Vec<(&str, &str)> = vec![
                ("i", &i),
                ("address", &address),
                ("cause", loc.t(cause_key(b.cause))),
                ("date", &when),
                ("count", &count),
            ];
            let place = [
                ("number", number.as_str()),
                ("variant", &variant),
                ("of", &of),
            ];
            let key = if b.detached {
                "tool.branch_read.list.entry_detached"
            } else if let Some(parent) = parent.as_deref() {
                args.extend(place);
                args.push(("parent", parent));
                "tool.branch_read.list.entry_nested"
            } else {
                args.extend(place);
                "tool.branch_read.list.entry"
            };
            out.push_str(&loc.tf(key, &args));
            if !b.detached {
                out.push_str("\n   ");
                out.push_str(&after_line(b, loc));
            }
            if let Some((role, line)) = &b.begins {
                out.push_str("\n   ");
                out.push_str(&loc.tf(
                    "tool.branch_read.begins",
                    &[("who", who(*role, loc)), ("line", line)],
                ));
            }
        }
        out
    }
}

/// What an address with no parked tail under it names ([`BranchShelf::standing`]).
enum Standing<'a> {
    /// A variant of a place on the line: the line itself.
    OnLine,
    /// A variant of a place inside the tail with this path: part of that tail.
    InParent(&'a [(u32, u16)]),
    Nothing,
}

/// Whose branches a call reads.
enum Whose {
    /// The open conversation's.
    Own,
    /// Another conversation's: its title and its `chat://` address.
    Other { title: String, id: String },
}

/// `branch_read` — the list of a conversation's parked branches, or one of
/// them as a transcript.
pub struct BranchRead;

#[async_trait::async_trait]
impl Tool for BranchRead {
    fn id(&self) -> ToolId {
        BRANCH_READ_ID.into()
    }
    /// Pages a rendered snapshot, or another chat's file under the cross-chat
    /// snapshot (docs/research/concurrent-tools.md §2.3).
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "read the chat's parked branches"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    /// The switch is a consent — it makes what the person took back readable —
    /// and the settings say so under it.
    fn ui_note(&self) -> Option<&'static str> {
        Some("ui.tool.note.branch_read")
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.branch_read.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": loc.t("tool.branch_read.param.branch")
                },
                "page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": loc.t("tool.branch_read.param.page")
                },
                "chat": {
                    "type": "string",
                    "description": loc.t("tool.branch_read.param.chat")
                }
            },
            "required": []
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let loc = ctx.loc;
        let text_arg = |name: &str| {
            args.get(name)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let page = args
            .get("page")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1);

        // Whose branches: the open chat's snapshot, or another chat's file.
        let other;
        let (shelf, whose) = match text_arg("chat") {
            None => (ctx.branches.as_deref(), Whose::Own),
            Some(reference) => match other_shelf(ctx, reference, &READER_REFUSALS) {
                Ok((shelf, whose)) => {
                    other = shelf;
                    (other.as_ref(), whose)
                }
                Err(answer) => return Ok(ToolOutcome::text(answer)),
            },
        };
        let Some(shelf) = shelf else {
            return Ok(ToolOutcome::text(no_branches(loc, &whose)));
        };
        let page_tokens = ctx.history_page_tokens;

        let Some(wanted) = text_arg("branch") else {
            // The list, paged like a transcript: a chat whose replies were
            // redone a hundred times has a hundred entries.
            let body = shelf.list(loc);
            let pages = paginate(&body, page_tokens);
            let Some(text) = page.checked_sub(1).and_then(|i| pages.get(i)) else {
                return Ok(bad_page(loc, page, pages.len()));
            };
            let n = shelf.len().to_string();
            let (page, total) = (page.to_string(), pages.len().to_string());
            let mut args = vec![("n", n.as_str()), ("page", &page), ("total", &total)];
            let (header, hint) = match &whose {
                Whose::Own => ("tool.branch_read.list.header", "tool.branch_read.list.hint"),
                Whose::Other { title, id } => {
                    args.push(("title", title));
                    args.push(("id", id));
                    (
                        "tool.branch_read.list.header_other",
                        "tool.branch_read.list.hint_other",
                    )
                }
            };
            let mut out = format!(
                "{}\n\n{}\n\n{}",
                loc.tf(header, &args),
                text.trim_end(),
                loc.t(hint)
            );
            // A tail is found by a word with the search — said where the turn
            // has it, and nowhere else (spec §6.7).
            if ctx.branch_search {
                out.push(' ');
                out.push_str(loc.t("tool.branch_read.list.hint_search"));
            }
            return Ok(ToolOutcome::text(out));
        };

        let Some(path) = parse_address(wanted) else {
            return Ok(ToolOutcome::text(
                loc.tf("tool.branch_read.bad_address", &[("branch", wanted)]),
            ));
        };
        let shown = address(&path);
        let Some(branch) = shelf.find(&path) else {
            let (number, variant) = path.last().copied().unwrap_or_default();
            let (number, variant) = (number.to_string(), variant.to_string());
            let place = [("number", number.as_str()), ("variant", variant.as_str())];
            return Ok(ToolOutcome::text(match shelf.standing(&path) {
                Standing::OnLine => loc.tf(
                    match whose {
                        Whose::Own => "tool.branch_read.on_line",
                        Whose::Other { .. } => "tool.branch_read.on_line_other",
                    },
                    &place,
                ),
                Standing::InParent(parent) => {
                    let parent = address(parent);
                    let mut args = place.to_vec();
                    args.push(("parent", &parent));
                    loc.tf("tool.branch_read.in_parent", &args)
                }
                Standing::Nothing => {
                    let known: Vec<String> = shelf
                        .branches
                        .iter()
                        .take(KNOWN_CAP)
                        .map(|b| format!("\"{}\"", address(&b.path)))
                        .collect();
                    let known = known.join(", ");
                    let total = shelf.len().to_string();
                    let mut args = vec![("address", shown.as_str()), ("known", known.as_str())];
                    let key = if shelf.len() > KNOWN_CAP {
                        args.push(("total", &total));
                        "tool.branch_read.unknown_more"
                    } else {
                        "tool.branch_read.unknown"
                    };
                    loc.tf(key, &args)
                }
            }));
        };
        let Some(view) = branch.view.as_ref() else {
            return Ok(ToolOutcome::text(
                loc.tf("tool.branch_read.empty", &[("address", &shown)]),
            ));
        };
        let total = view.page_count(page_tokens);
        let Some(text) = view.page(page_tokens, page) else {
            return Ok(bad_page(loc, page, total));
        };
        let (page, total) = (page.to_string(), total.to_string());
        let when = date(branch.parked_at);
        let mut args = vec![
            ("address", shown.as_str()),
            ("page", &page),
            ("total", &total),
            ("cause", loc.t(cause_key(branch.cause))),
            ("date", &when),
        ];
        let key = match &whose {
            Whose::Own => "tool.branch_read.header",
            Whose::Other { title, id } => {
                args.push(("title", title));
                args.push(("id", id));
                "tool.branch_read.header_other"
            }
        };
        let mut out = loc.tf(key, &args);
        if !branch.detached {
            out.push('\n');
            out.push_str(&after_line(branch, loc));
        }
        Ok(ToolOutcome::text(format!("{out}\n\n{text}")))
    }
}

/// `branch_search` — the parked messages of a conversation that say the words
/// of a query.
pub struct BranchSearch;

#[async_trait::async_trait]
impl Tool for BranchSearch {
    fn id(&self) -> ToolId {
        BRANCH_SEARCH_ID.into()
    }
    /// Scans the snapshot the reader pages, or another chat's file under the
    /// cross-chat snapshot (docs/research/concurrent-tools.md §2.3).
    fn concurrent(&self) -> bool {
        true
    }
    fn group(&self) -> super::meta::ToolGroup {
        super::meta::ToolGroup::Conversation
    }
    fn ui_label(&self) -> &'static str {
        "search the chat's parked branches"
    }
    fn enabled_by_default(&self) -> bool {
        false
    }
    /// The reader's consent, for a fragment instead of a page: what the person
    /// took back becomes findable.
    fn ui_note(&self) -> Option<&'static str> {
        Some("ui.tool.note.branch_search")
    }
    fn description(&self, loc: &Locale) -> String {
        loc.t("tool.branch_search.desc").into()
    }
    fn parameters(&self, loc: &Locale) -> serde_json::Value {
        let mut schema = super::search_parameters(
            loc,
            "tool.branch_search.param.query",
            "tool.branch_search.param.top_k",
        );
        schema["properties"]["chat"] = serde_json::json!({
            "type": "string",
            "description": loc.t("tool.branch_search.param.chat")
        });
        schema
    }

    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let loc = ctx.loc;
        let (query, k) = super::search_args(
            &args,
            loc,
            "tool.branch_search.err.query_empty",
            DEFAULT_TOP_K,
        )?;
        let k = k.clamp(1, MAX_TOP_K);
        let reference = args
            .get("chat")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // Whose branches: the open chat's snapshot, or another chat's file.
        let other;
        let (shelf, whose) = match reference {
            None => (ctx.branches.as_deref(), Whose::Own),
            Some(reference) => match other_shelf(ctx, reference, &SEARCH_REFUSALS) {
                Ok((shelf, whose)) => {
                    other = shelf;
                    (other.as_ref(), whose)
                }
                Err(answer) => return Ok(ToolOutcome::text(answer)),
            },
        };
        let Some(shelf) = shelf else {
            return Ok(ToolOutcome::text(no_branches(loc, &whose)));
        };

        let words = QueryWords::parse(query);
        if words.is_empty() {
            return Ok(ToolOutcome::text(loc.t("tool.branch_search.too_short")));
        }
        let page_tokens = ctx.history_page_tokens;
        // Nothing here can fail — the tails are in memory — so the relaxation
        // only ever answers `Ok`; its error arm is the other searches' index.
        let relaxed = relax_query(
            &words,
            |w| Ok(Some(shelf.matching(w, page_tokens)).filter(|hits| !hits.is_empty())),
            |w| Ok(shelf.matching(&QueryWords::only(w), page_tokens).len()),
        )?;
        let (hits, words, dropped) = match relaxed {
            Relaxed::Found {
                result,
                words,
                dropped,
            } => (result, words, dropped),
            Relaxed::Apart { counts, missing } => {
                return Ok(ToolOutcome::text(chats::apart(
                    loc,
                    "tool.branch_search.apart",
                    "tool.branch_search.apart_missing",
                    &counts,
                    &missing,
                )));
            }
            Relaxed::Nothing => {
                return Ok(ToolOutcome::text(match &whose {
                    Whose::Own => loc.t("tool.branch_search.empty").to_string(),
                    Whose::Other { title, .. } => {
                        loc.tf("tool.branch_search.empty_other", &[("title", title)])
                    }
                }));
            }
        };

        let mut out = String::new();
        if !dropped.is_empty() {
            out.push_str(&loc.tf(
                "tool.branch_search.dropped",
                &[("words", &super::quoted_words(loc, &dropped))],
            ));
            out.push('\n');
        }
        let (shown, total) = (hits.len().min(k).to_string(), hits.len().to_string());
        let mut counts = vec![("n", shown.as_str()), ("total", total.as_str())];
        out.push_str(&match &whose {
            Whose::Own => loc.tf("tool.branch_search.header", &counts),
            Whose::Other { title, id } => {
                counts.push(("title", title));
                counts.push(("id", id));
                loc.tf("tool.branch_search.header_other", &counts)
            }
        });
        out.push_str(&render_hits(loc, &hits[..hits.len().min(k)], &words));
        // What reads a hit in full is the reader — named where the turn has
        // it; without it, the person's own way to a branch (spec §6.7).
        let hint = match (&whose, ctx.branch_reader) {
            (Whose::Own, true) => Some("tool.branch_search.hint"),
            (Whose::Other { .. }, true) => Some("tool.branch_search.hint_other"),
            (Whose::Own, false) => Some("tool.branch_search.hint_no_reader"),
            (Whose::Other { .. }, false) => None,
        };
        if let Some(hint) = hint {
            out.push_str("\n\n");
            out.push_str(loc.t(hint));
        }
        Ok(ToolOutcome::text(out))
    }
}

/// The body of a search's answer: the hits under their tail's heading — its
/// address, what parked it and when — each with its page and a fragment
/// around the match. Tails come in the list's order, so a heading is written
/// when the tail changes.
fn render_hits(loc: &Locale, hits: &[Hit<'_>], words: &QueryWords) -> String {
    let mut out = String::new();
    let mut heading: Option<&BranchPath> = None;
    for (i, hit) in hits.iter().enumerate() {
        if heading != Some(&hit.branch.path) {
            heading = Some(&hit.branch.path);
            out.push_str("\n\n");
            out.push_str(&loc.tf(
                "tool.branch_search.branch",
                &[
                    ("address", address(&hit.branch.path).as_str()),
                    ("cause", loc.t(cause_key(hit.branch.cause))),
                    ("date", &date(hit.branch.parked_at)),
                ],
            ));
        }
        let (n, when) = ((i + 1).to_string(), date(hit.said.at));
        let mut args = vec![
            ("n", n.as_str()),
            ("role", role_word(hit.said.role)),
            ("date", when.as_str()),
        ];
        let page = hit.page.map(|p| p.to_string());
        let key = match page.as_deref() {
            Some(page) => {
                args.push(("page", page));
                "tool.branch_search.hit"
            }
            None => "tool.branch_search.hit_unpaged",
        };
        out.push_str("\n\n");
        out.push_str(&loc.tf(key, &args));
        out.push('\n');
        out.push_str(&build_word_snippet(&hit.said.text, words, SNIPPET_CHARS).text);
    }
    out
}

/// A message's role as a hit names it — the word the full-text index stores,
/// which is what `chat_search` prints.
fn role_word(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// The shelf of the conversation `reference` names, with what to call it — or
/// the answer that says why there is none to read and what can be done.
///
/// Scoped by the turn's cross-chat snapshot exactly as `chat_read` is: the
/// same resolver, the same guards on the loaded file. An empty snapshot is a
/// turn that does not cross conversations at all, and the answer says what
/// the tool does without `chat`.
fn other_shelf(
    ctx: &ToolContext,
    reference: &str,
    refusals: &Refusals,
) -> std::result::Result<(Option<BranchShelf>, Whose), String> {
    let loc = ctx.loc;
    if ctx.other_chats.is_empty() {
        return Err(loc.t(refusals.no_cross_chat).to_string());
    }
    let chat_ref: &ChatRef = match chats::resolve(&ctx.other_chats, reference) {
        Resolved::One(r) => r,
        Resolved::Ambiguous(candidates) => {
            let listed: Vec<String> = candidates
                .iter()
                .take(AMBIGUOUS_CAP)
                .map(|r| format!("\"{}\" {}", r.title, chats::address(r.id)))
                .collect();
            return Err(loc.tf(
                "tool.branch_read.ambiguous_chat",
                &[("chat", reference), ("candidates", &listed.join("; "))],
            ));
        }
        Resolved::None => {
            return Err(loc.tf(refusals.unknown_chat, &[("chat", reference)]));
        }
    };
    let title = chat_ref.label(loc);
    // A sub-agent's transcript is one run, start to end: nothing is parked
    // beside it.
    if chat_ref.parent.is_some() {
        return Err(loc.tf("tool.branch_read.child", &[("title", &title)]));
    }
    let Some(chat) = chats::load_chat(ctx, chat_ref) else {
        return Err(loc.tf("tool.branch_read.unavailable", &[("title", &title)]));
    };
    let whose = Whose::Other {
        title,
        id: chats::address(chat_ref.id),
    };
    Ok((BranchShelf::of(&chat, loc), whose))
}

/// The two answers of [`other_shelf`] that say what the calling tool does
/// without `chat` — and so name it. Each tool brings its own, so that neither
/// names the other on a turn that may not have it (spec §6.7); the answers
/// that name no tool are shared.
struct Refusals {
    no_cross_chat: &'static str,
    unknown_chat: &'static str,
}

const READER_REFUSALS: Refusals = Refusals {
    no_cross_chat: "tool.branch_read.no_cross_chat",
    unknown_chat: "tool.branch_read.unknown_chat",
};

const SEARCH_REFUSALS: Refusals = Refusals {
    no_cross_chat: "tool.branch_search.no_cross_chat",
    unknown_chat: "tool.branch_search.unknown_chat",
};

/// The answer for a conversation that has no parked tail: all of it is what
/// the model already has, or what `chat_read` returns.
fn no_branches(loc: &Locale, whose: &Whose) -> String {
    match whose {
        Whose::Own => loc.t("tool.branch_read.none").to_string(),
        Whose::Other { title, .. } => loc.tf("tool.branch_read.none_other", &[("title", title)]),
    }
}

/// The answer to a page past the end: the real count, so the next call can
/// succeed (the `attachment_read` rule).
fn bad_page(loc: &Locale, page: usize, total: usize) -> ToolOutcome {
    ToolOutcome::text(loc.tf(
        "tool.branch_read.bad_page",
        &[("page", &page.to_string()), ("total", &total.to_string())],
    ))
}

/// The address of the tail a nested tail is inside; `None` for a tail that
/// continues from the line.
fn address_of_parent(path: &[(u32, u16)]) -> Option<String> {
    let (_, parent) = path.split_last()?;
    (!parent.is_empty()).then(|| address(parent))
}

/// Reads an address back. The integers of the text, in order, taken in pairs
/// — so `11/1`, `11 1`, `#11/1` and `5/1 > 7/1` all read as what they say, and
/// the separators a model varies do not matter. `None` — no pair, an odd
/// integer left over, or a figure too large to be one.
fn parse_address(text: &str) -> Option<BranchPath> {
    let figures: Vec<&str> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    if figures.is_empty() || !figures.len().is_multiple_of(2) {
        return None;
    }
    figures
        .chunks(2)
        .map(|pair| Some((pair[0].parse().ok()?, pair[1].parse().ok()?)))
        .collect()
}

/// Who said a line, in the words the transcript uses for them.
fn who(role: MessageRole, loc: &Locale) -> &str {
    match role {
        MessageRole::User => loc.t("digest.role.user"),
        _ => loc.t("digest.role.assistant"),
    }
}

/// The line that says what a tail continues from.
fn after_line(branch: &ParkedBranch, loc: &Locale) -> String {
    match &branch.after {
        Some((role, line)) => loc.tf(
            "tool.branch_read.after",
            &[("who", who(*role, loc)), ("line", line)],
        ),
        None => loc.t("tool.branch_read.after_start").to_string(),
    }
}

fn cause_key(cause: ParkCause) -> &'static str {
    match cause {
        ParkCause::Fork => "tool.branch_read.cause.fork",
        ParkCause::Switch => "tool.branch_read.cause.switch",
        ParkCause::Regenerate => "tool.branch_read.cause.regenerate",
        ParkCause::Takeback => "tool.branch_read.cause.takeback",
    }
}

/// A date as the model reads it (locale-neutral, like a search hit's).
fn date(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// The first line with text among the messages a person or the assistant
/// wrote, clipped to [`QUOTE_CHARS`], with who wrote it.
fn first_said<'a>(messages: impl Iterator<Item = &'a Message>) -> Option<(MessageRole, String)> {
    messages
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .find_map(|m| {
            let line = m.text.lines().map(str::trim).find(|l| !l.is_empty())?;
            Some((m.role, clip(line)))
        })
}

/// What was last said up to and including the message `id` — the place a tail
/// anchored there continues from. Looked for on the line and in every parked
/// tail, since a tail can continue from inside another; walked backwards from
/// the anchor because the anchor itself can be a round with no text.
fn said_before(chat: &Chat, id: uuid::Uuid) -> Option<(MessageRole, String)> {
    std::iter::once(chat.messages.as_slice())
        .chain(chat.branches.iter().map(|b| b.messages.as_slice()))
        .find_map(|list| {
            let at = list.iter().position(|m| m.id == id)?;
            Some(first_said(list[..=at].iter().rev()))
        })
        .flatten()
}

fn clip(line: &str) -> String {
    if line.chars().count() <= QUOTE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(QUOTE_CHARS).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::profile::Profile;
    use crate::entities::subagent::SubagentRun;
    use crate::features::tools::chats::ChatRef;
    use crate::shared::i18n::{Lang, locale};
    use crate::shared::storage::Storage;
    use std::sync::Arc;
    use uuid::Uuid;

    fn at(minutes: i64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::hours(2) + chrono::Duration::minutes(minutes)
    }

    fn said(role: MessageRole, text: &str, minutes: i64) -> Message {
        let mut m = Message::new(role, text);
        m.timestamp = at(minutes);
        m
    }

    /// `q1 a1 q2 a2 q3 a3`, a minute apart, in a chat of `profile_id`.
    fn three_exchanges(profile_id: Uuid) -> Chat {
        let mut profile = Profile::new("p", "");
        profile.id = profile_id;
        let mut chat = Chat::from_profile(&profile, "рыбалка");
        for (i, (role, text)) in [
            (MessageRole::User, "вопрос один"),
            (MessageRole::Assistant, "ответ один"),
            (MessageRole::User, "вопрос два"),
            (MessageRole::Assistant, "ответ два"),
            (MessageRole::User, "а пароль от лодки?"),
            (MessageRole::Assistant, "пароль — ЗАНЗИБАР-42"),
        ]
        .into_iter()
        .enumerate()
        {
            chat.push_message(said(role, text, i as i64));
        }
        chat
    }

    /// The same chat after `Ctrl+R` on its last reply: the old reply is
    /// variant 1 of message #6, the new one stands on the line as variant 2.
    fn redone(profile_id: Uuid) -> Chat {
        let mut chat = three_exchanges(profile_id);
        assert!(chat.park_tail(5, ParkCause::Regenerate, at(10)));
        chat.push_message(said(MessageRole::Assistant, "пароля я не помню", 11));
        chat
    }

    /// [`redone`], then a fork at the first reply and a new question: the
    /// redone reply's tail is now inside the forked one.
    fn forked_above(profile_id: Uuid) -> Chat {
        let mut chat = redone(profile_id);
        assert!(chat.fork_at(2, at(20)));
        chat.push_message(said(MessageRole::User, "начнём иначе", 21));
        chat
    }

    /// A context whose open chat is `chat`, with the reader on the turn.
    fn ctx_over(chat: &Chat, lang: Lang) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let (dir, storage, mut ctx) =
            super::super::testkit::ctx_with_storage_lang(chat.profile_id, lang);
        ctx.chat_id = chat.id;
        ctx.branch_reader = true;
        ctx.branches = BranchShelf::of(chat, locale(lang)).map(Arc::new);
        // One page for the whole of an answer, unless a test is about pages.
        ctx.history_page_tokens = 4000;
        (dir, storage, ctx)
    }

    async fn call(ctx: &ToolContext, args: serde_json::Value) -> String {
        BranchRead.invoke(ctx, args).await.unwrap().result
    }

    #[tokio::test]
    async fn the_list_names_a_tail_as_the_branch_command_does() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = call(&ctx, serde_json::json!({})).await;

        // The address is the pair `/branch` takes and the list of `/branch`
        // shows: the reader and the person name a tail by one figure.
        let view = chat.branch_view();
        assert_eq!((view.parked[0].number, view.parked[0].variant), (6, 1));
        assert!(out.contains("Branch \"6/1\""), "{out}");
        assert!(out.contains("message #6, variant 1 of 2"), "{out}");
        assert!(out.contains("A reply replaced by a regeneration"), "{out}");
        // What it continues from and how it begins, quoted: the request holds
        // no numbers to find `#6` by.
        assert!(
            out.contains("Continues after: User: \"а пароль от лодки?\""),
            "{out}"
        );
        assert!(
            out.contains("Begins: Assistant: \"пароль — ЗАНЗИБАР-42\""),
            "{out}"
        );
        assert!(
            out.starts_with("[Parked branches of this conversation: 1; page 1 of 1."),
            "{out}"
        );
        assert!(out.contains("/branch"), "the way back is named: {out}");
    }

    #[tokio::test]
    async fn a_read_returns_the_tail_and_not_the_line() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = call(&ctx, serde_json::json!({ "branch": "6/1" })).await;
        assert!(out.contains("ЗАНЗИБАР-42"), "{out}");
        assert!(!out.contains("не помню"), "the line's reply: {out}");
        assert!(
            out.starts_with("[Branch \"6/1\" of this conversation, page 1 of 1."),
            "{out}"
        );
        assert!(out.contains("Continues after: User:"), "{out}");
    }

    /// The variant that stands on the line is not parked, and the answer says
    /// where it is instead of "no such branch".
    #[tokio::test]
    async fn the_lines_own_variant_is_answered_as_the_line() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = call(&ctx, serde_json::json!({ "branch": "6/2" })).await;
        assert_eq!(
            out,
            "Variant 2 of message #6 is not parked: it is the conversation in front of you."
        );
    }

    #[tokio::test]
    async fn a_tail_inside_another_is_read_by_its_path() {
        let chat = forked_above(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let list = call(&ctx, serde_json::json!({})).await;
        assert!(list.contains("1. Branch \"3/1\" - message #3"), "{list}");
        assert!(
            list.contains("2. Branch \"3/1 > 6/1\" - inside branch \"3/1\": message #6"),
            "{list}"
        );
        assert!(list.contains("Set aside by a fork"), "{list}");

        // The outer tail is the line as it was left: the reply that replaced
        // the redone one is in it, the redone one is not.
        let outer = call(&ctx, serde_json::json!({ "branch": "3/1" })).await;
        assert!(outer.contains("вопрос два"), "{outer}");
        assert!(outer.contains("не помню"), "{outer}");
        assert!(!outer.contains("ЗАНЗИБАР-42"), "{outer}");

        let inner = call(&ctx, serde_json::json!({ "branch": "3/1 > 6/1" })).await;
        assert!(inner.contains("ЗАНЗИБАР-42"), "{inner}");
        assert!(!inner.contains("вопрос два"), "{inner}");

        // The variant standing inside the outer tail is part of that tail.
        let standing = call(&ctx, serde_json::json!({ "branch": "3/1 > 6/2" })).await;
        assert!(standing.contains("part of branch \"3/1\""), "{standing}");
        // Without its path the inner tail is not a variant of the line's #6.
        let bare = call(&ctx, serde_json::json!({ "branch": "6/1" })).await;
        assert!(bare.contains("There is no parked branch \"6/1\""), "{bare}");
        assert!(bare.contains("\"3/1\", \"3/1 > 6/1\""), "{bare}");
    }

    #[test]
    fn an_address_reads_the_same_however_it_is_written() {
        for text in ["11/1", "11 1", "#11/1", " 11 / 1 ", "branch 11/1"] {
            assert_eq!(parse_address(text), Some(vec![(11, 1)]), "{text}");
        }
        for text in ["5/1 > 7/1", "5/1, 7/1", "5/1 7/1", "5/1>7/1"] {
            assert_eq!(parse_address(text), Some(vec![(5, 1), (7, 1)]), "{text}");
        }
        // No pair, a figure left over, a figure too large to be one.
        for text in ["", "the last one", "11", "5/1/7", "5/99999999"] {
            assert_eq!(parse_address(text), None, "{text}");
        }
        // What is printed is what is read back.
        let path = vec![(5, 1), (7, 2)];
        assert_eq!(parse_address(&address(&path)), Some(path));
    }

    #[tokio::test]
    async fn an_address_that_names_nothing_is_answered_with_what_can_be_done() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let unknown = call(&ctx, serde_json::json!({ "branch": "9/9" })).await;
        assert_eq!(
            unknown,
            "There is no parked branch \"9/9\". The branches there are: \"6/1\"."
        );
        // A variant past the place's count is no variant of it.
        let past = call(&ctx, serde_json::json!({ "branch": "6/3" })).await;
        assert!(past.contains("There is no parked branch \"6/3\""), "{past}");
        let garbled = call(&ctx, serde_json::json!({ "branch": "the previous one" })).await;
        assert!(garbled.contains("is not a branch address"), "{garbled}");
        assert!(garbled.contains("11/1"), "the form is shown: {garbled}");
    }

    /// The variants of a place are ordered by their first message's time, so
    /// an address names the same text whichever of them is on the line.
    #[tokio::test]
    async fn an_address_means_the_same_after_a_switch() {
        let mut chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let before = call(&ctx, serde_json::json!({ "branch": "6/1" })).await;
        assert!(before.contains("ЗАНЗИБАР-42"), "{before}");

        let first = chat.branches[0].first().unwrap();
        chat.switch_to(first, at(30)).unwrap();
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let one = call(&ctx, serde_json::json!({ "branch": "6/1" })).await;
        assert!(
            one.contains("it is the conversation in front of you"),
            "{one}"
        );
        let two = call(&ctx, serde_json::json!({ "branch": "6/2" })).await;
        assert!(two.contains("не помню"), "{two}");
        assert!(two.contains("Set aside by a switch"), "{two}");
    }

    #[tokio::test]
    async fn the_list_and_a_tail_are_paged() {
        // A reply redone twelve times, and a long tail.
        let mut chat = three_exchanges(Uuid::new_v4());
        for n in 0..12 {
            let cut = chat.messages.len() - 1;
            assert!(chat.park_tail(cut, ParkCause::Regenerate, at(10 + n)));
            chat.push_message(said(
                MessageRole::Assistant,
                &format!("вариант ответа номер {n}, довольно длинный, чтобы занять место"),
                10 + n,
            ));
        }
        let (_d, _s, mut ctx) = ctx_over(&chat, Lang::En);
        ctx.history_page_tokens = 60;

        let first = call(&ctx, serde_json::json!({})).await;
        let total: usize = first
            .split("page 1 of ")
            .nth(1)
            .and_then(|rest| rest.split('.').next())
            .and_then(|n| n.parse().ok())
            .expect("the header states the page count");
        assert!(total > 1, "{first}");
        // Walked 1..=total, the pages hold every tail once.
        let mut seen = String::new();
        for page in 1..=total {
            seen.push_str(&call(&ctx, serde_json::json!({ "page": page })).await);
        }
        for k in 1..=12 {
            let entry = format!("Branch \"6/{k}\"");
            assert_eq!(seen.matches(&entry).count(), 1, "{entry} in {seen}");
        }
        let past = call(&ctx, serde_json::json!({ "page": total + 1 })).await;
        assert_eq!(
            past,
            format!("There is no page {}: there are {total} page(s).", total + 1)
        );
        // A tail's own pages answer a page past their end the same way.
        let past = call(&ctx, serde_json::json!({ "branch": "6/1", "page": 9 })).await;
        assert_eq!(past, "There is no page 9: there are 1 page(s).");
    }

    #[tokio::test]
    async fn a_chat_with_no_branch_says_so() {
        let chat = three_exchanges(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        assert!(ctx.branches.is_none());
        let out = call(&ctx, serde_json::json!({})).await;
        assert_eq!(
            out,
            "This conversation has no parked branches: everything said in it is in front of you."
        );
    }

    /// A tail that starts the chat anew continues after nothing, and says so
    /// rather than quoting a message that is not there.
    #[tokio::test]
    async fn a_tail_that_starts_the_chat_names_no_message_before_it() {
        let mut chat = three_exchanges(Uuid::new_v4());
        assert!(chat.fork_at(0, at(10)));
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = call(&ctx, serde_json::json!({})).await;
        assert!(out.contains("Branch \"1/1\""), "{out}");
        assert!(
            out.contains("Continues after: nothing - it is another beginning"),
            "{out}"
        );
    }

    /// A tail whose anchor names no message of the chat — a file edited by
    /// hand — is listed last, marked, and read by its place among such tails.
    /// One that holds nothing a transcript shows says so.
    #[tokio::test]
    async fn a_detached_tail_and_an_empty_one_are_listed_and_answered() {
        use crate::entities::chat_branch::Branch;
        let mut chat = redone(Uuid::new_v4());
        let lost = |messages: Vec<Message>| Branch {
            after: Some(Uuid::new_v4()),
            messages,
            parked_at: at(40),
            cause: ParkCause::Fork,
            compaction: None,
            reflected_upto: None,
        };
        chat.branches
            .push(lost(vec![said(MessageRole::User, "потерянный вопрос", 40)]));
        chat.branches
            .push(lost(vec![said(MessageRole::System, "заметка", 41)]));
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);

        let list = call(&ctx, serde_json::json!({})).await;
        assert!(list.contains("1. Branch \"6/1\" - message #6"), "{list}");
        assert!(
            list.contains("2. Branch \"0/1\" - its place in the conversation is unknown."),
            "{list}"
        );
        assert!(list.contains("3. Branch \"0/2\""), "{list}");
        let read = call(&ctx, serde_json::json!({ "branch": "0/1" })).await;
        assert!(read.contains("потерянный вопрос"), "{read}");
        assert!(
            !read.contains("Continues after"),
            "no place to quote: {read}"
        );
        let empty = call(&ctx, serde_json::json!({ "branch": "0/2" })).await;
        assert_eq!(empty, "Branch \"0/2\" has no readable text.");
    }

    /// Past a dozen tails an unknown address is answered with some of them
    /// and the call that lists them all.
    #[tokio::test]
    async fn an_unknown_address_among_many_tails_names_some_and_the_list() {
        let mut chat = three_exchanges(Uuid::new_v4());
        for n in 0..13 {
            let cut = chat.messages.len() - 1;
            assert!(chat.park_tail(cut, ParkCause::Regenerate, at(10 + n)));
            chat.push_message(said(MessageRole::Assistant, "ещё вариант", 10 + n));
        }
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = call(&ctx, serde_json::json!({ "branch": "2/1" })).await;
        assert!(
            out.contains("There are 13 branches, among them: \"6/1\""),
            "{out}"
        );
        assert!(out.contains("\"6/12\""), "{out}");
        assert!(!out.contains("\"6/13\""), "capped: {out}");
        assert!(out.contains("Call branch_read without branch"), "{out}");
    }

    fn other_ref(chat: &Chat) -> ChatRef {
        ChatRef {
            id: chat.id,
            title: chat.title.clone(),
            modified_at: chat.modified_at,
            parent: None,
        }
    }

    /// Another conversation's branches are read from its file, under the
    /// turn's cross-chat snapshot and nothing else.
    #[tokio::test]
    async fn another_chats_branches_are_read_through_the_cross_chat_snapshot() {
        let open = three_exchanges(Uuid::new_v4());
        let (_d, storage, mut ctx) = ctx_over(&open, Lang::En);
        let mut other = redone(open.profile_id);
        other.title = "лодка".into();
        storage.json().save_chat(&other).unwrap();
        let id = crate::features::chat_links::uri(other.id);

        // No snapshot: the turn does not cross conversations, and the answer
        // says what the tool does without `chat`.
        let refused = call(&ctx, serde_json::json!({ "chat": id })).await;
        assert!(
            refused.starts_with("Other conversations cannot be read on this turn."),
            "{refused}"
        );

        ctx.other_chats = Arc::from(vec![other_ref(&other)]);
        let list = call(&ctx, serde_json::json!({ "chat": id })).await;
        assert!(
            list.starts_with(&format!(
                "[Parked branches of the conversation \"лодка\" {id}: 1; page 1 of 1."
            )),
            "{list}"
        );
        assert!(list.contains("Branch \"6/1\""), "{list}");
        assert!(list.contains("and the same chat"), "{list}");
        // By title too, as `chat_read` resolves.
        let read = call(
            &ctx,
            serde_json::json!({ "chat": "лодка", "branch": "6/1" }),
        )
        .await;
        assert!(read.contains("ЗАНЗИБАР-42"), "{read}");
        assert!(
            read.starts_with(&format!(
                "[Branch \"6/1\" of the conversation \"лодка\" {id}"
            )),
            "{read}"
        );
        let own = call(
            &ctx,
            serde_json::json!({ "chat": "лодка", "branch": "6/2" }),
        )
        .await;
        assert!(
            own.contains("what the conversation's transcript holds"),
            "{own}"
        );

        let unknown = call(&ctx, serde_json::json!({ "chat": "охота" })).await;
        assert!(
            unknown.contains("No other conversation here matches"),
            "{unknown}"
        );
    }

    #[tokio::test]
    async fn another_chat_with_no_branch_and_a_transcript_say_so() {
        let open = three_exchanges(Uuid::new_v4());
        let (_d, storage, mut ctx) = ctx_over(&open, Lang::En);
        let mut plain = three_exchanges(open.profile_id);
        plain.title = "охота".into();
        storage.json().save_chat(&plain).unwrap();
        let run = SubagentRun::fixture("критик", &["вопрос"]);
        let transcript = ChatRef {
            id: run.id,
            title: run.title.clone(),
            modified_at: run.created_at,
            parent: Some(crate::features::tools::chats::ParentRef {
                id: plain.id,
                title: plain.title.clone(),
            }),
        };
        ctx.other_chats = Arc::from(vec![other_ref(&plain), transcript]);

        let none = call(&ctx, serde_json::json!({ "chat": "охота" })).await;
        assert_eq!(
            none,
            "The conversation \"охота\" has no parked branches: its transcript is all of it."
        );
        let child = call(
            &ctx,
            serde_json::json!({ "chat": crate::features::chat_links::uri(run.id) }),
        )
        .await;
        assert!(
            child.contains("a subagent transcript is one run"),
            "{child}"
        );
    }

    /// The loaded file is checked against the snapshot's boundary again: a
    /// stale snapshot reads nothing of another profile's chat, a hidden one,
    /// or the open one.
    #[tokio::test]
    async fn a_stale_snapshot_reads_no_branch_it_should_not() {
        let open = redone(Uuid::new_v4());
        let (_d, storage, mut ctx) = ctx_over(&open, Lang::En);
        storage.json().save_chat(&open).unwrap();
        let mut foreign = redone(Uuid::new_v4());
        foreign.title = "чужой".into();
        storage.json().save_chat(&foreign).unwrap();
        let mut hidden = redone(open.profile_id);
        hidden.title = "скрытый".into();
        hidden.is_hidden = true;
        storage.json().save_chat(&hidden).unwrap();
        ctx.other_chats = Arc::from(vec![
            other_ref(&foreign),
            other_ref(&hidden),
            other_ref(&open),
        ]);
        for chat in [&foreign, &hidden, &open] {
            let out = call(
                &ctx,
                serde_json::json!({ "chat": crate::features::chat_links::uri(chat.id) }),
            )
            .await;
            assert!(out.contains("cannot be read right now"), "{out}");
            assert!(!out.contains("6/1"), "{out}");
        }
    }

    #[test]
    fn the_reader_is_optional_catalogued_and_kept_from_a_subagent() {
        let all = crate::features::tools::all_tool_ids();
        assert!(all.iter().any(|t| t == BRANCH_READ_ID));
        assert!(
            !crate::features::tools::default_tool_ids()
                .iter()
                .any(|t| t == BRANCH_READ_ID),
            "reading what was set aside is a switch the person makes"
        );
        assert!(crate::features::tools::subagent::withheld_from_subagent(
            BRANCH_READ_ID
        ));
        let info = crate::features::tools::tool_catalog()
            .into_iter()
            .find(|i| i.id == BRANCH_READ_ID)
            .unwrap();
        assert_eq!(
            info.group,
            crate::features::tools::meta::ToolGroup::Conversation
        );
        assert!(info.concurrent);
        // The switch says what it lets the assistant reach, in both languages.
        let note = info.note.expect("a note under the switch");
        for &lang in Lang::ALL {
            assert!(locale(lang).has_key(note), "{lang:?}: {note}");
        }
    }

    // ── branch_search ──────────────────────────────────────────────────

    async fn find(ctx: &ToolContext, args: serde_json::Value) -> String {
        BranchSearch.invoke(ctx, args).await.unwrap().result
    }

    /// [`three_exchanges`] with its last reply redone `texts.len()` times:
    /// the original is variant 1, `texts` are the variants after it, and the
    /// last of them stands on the line.
    fn redone_with(profile_id: Uuid, texts: &[&str]) -> Chat {
        let mut chat = three_exchanges(profile_id);
        for (n, text) in texts.iter().enumerate() {
            let n = n as i64;
            let cut = chat.messages.len() - 1;
            assert!(chat.park_tail(cut, ParkCause::Regenerate, at(10 + 2 * n)));
            chat.push_message(said(MessageRole::Assistant, text, 11 + 2 * n));
        }
        chat
    }

    /// What the list cannot show — a word past a tail's first line — is found,
    /// under the address the list and `/branch` give that tail.
    #[tokio::test]
    async fn a_word_past_a_tails_first_line_is_found_under_its_address() {
        let chat = redone_with(
            Uuid::new_v4(),
            &[
                "Подумаю ещё раз.\n\nСпросите у начальника порта: его позывной — КЕСТРЕЛЬ-9.",
                "пароля я не помню",
            ],
        );
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let list = call(&ctx, serde_json::json!({})).await;
        assert!(
            !list.contains("КЕСТРЕЛЬ"),
            "the list shows first lines: {list}"
        );

        let out = find(&ctx, serde_json::json!({ "query": "кестрель" })).await;
        assert!(
            out.starts_with(
                "Fragments found in the parked branches of this conversation: 1 \
                 (matches in total: 1). They are not part of the conversation in front of you."
            ),
            "{out}"
        );
        assert!(
            out.contains("Branch \"6/2\". A reply replaced by a regeneration, "),
            "{out}"
        );
        assert!(out.contains("1. [assistant · "), "{out}");
        assert!(out.contains(" · page 1]"), "{out}");
        assert!(out.contains("КЕСТРЕЛЬ-9"), "{out}");
        assert!(
            !out.contains("ЗАНЗИБАР"),
            "the other tail does not match: {out}"
        );
        // The address is the list's and the person's.
        let view = chat.branch_view();
        assert!(view.parked.iter().any(|b| (b.number, b.variant) == (6, 2)));
        assert!(list.contains("Branch \"6/2\""), "{list}");
    }

    /// A hit's page is the page the reader returns that message on.
    #[tokio::test]
    async fn a_hit_names_the_page_the_reader_returns_it_on() {
        let mut chat = three_exchanges(Uuid::new_v4());
        assert!(chat.fork_at(2, at(20)));
        chat.push_message(said(MessageRole::User, "начнём иначе", 21));
        let (_d, _s, mut ctx) = ctx_over(&chat, Lang::En);
        // A page of some 60 bytes: the forked tail's four messages take
        // several, and each fits one — a hit names the page its message
        // begins on, which is then the page that holds it.
        ctx.history_page_tokens = 15;

        let out = find(&ctx, serde_json::json!({ "query": "занзибар" })).await;
        assert!(
            out.contains("Branch \"3/1\". Set aside by a fork, "),
            "{out}"
        );
        let page: usize = out
            .split(" · page ")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("a page in {out}"));
        assert!(page > 1, "the fixture spans pages: {out}");
        let read = call(&ctx, serde_json::json!({ "branch": "3/1", "page": page })).await;
        assert!(read.contains("ЗАНЗИБАР-42"), "{read}");
        assert!(
            out.ends_with(
                "To see a match in context, call branch_read with the branch's value and the \
                 page number."
            ),
            "{out}"
        );
    }

    /// The line is in front of the model and is no part of this search: a
    /// word said only on it finds nothing, and the answer says why.
    #[tokio::test]
    async fn the_line_is_never_a_hit() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        for query in ["помню", "вопрос", "лодки"] {
            let out = find(&ctx, serde_json::json!({ "query": query })).await;
            assert_eq!(
                out,
                "No parked branch of this conversation matches that query. Try other words; \
                 note the conversation in front of you is not covered by this search.",
                "{query}"
            );
        }
        // The same word in the tail is a hit, and only the tail's message is.
        let out = find(&ctx, serde_json::json!({ "query": "пароль" })).await;
        assert!(out.contains("(matches in total: 1)"), "{out}");
        assert!(out.contains("ЗАНЗИБАР-42"), "{out}");
    }

    #[tokio::test]
    async fn a_two_character_word_matches_only_as_a_whole_word() {
        let chat = redone_with(
            Uuid::new_v4(),
            &[
                "спросите в МИИТ про лодку",
                "спросите у ИИ про лодку",
                "не знаю",
            ],
        );
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = find(&ctx, serde_json::json!({ "query": "ии" })).await;
        assert!(out.contains("(matches in total: 1)"), "{out}");
        assert!(out.contains("Branch \"6/3\""), "{out}");
        assert!(out.contains("у ИИ про"), "{out}");
        assert!(!out.contains("МИИТ"), "{out}");
    }

    /// Words that never meet in one message are answered with their counts,
    /// and a word no tail says is dropped and named — the other searches' rule.
    #[tokio::test]
    async fn words_apart_and_a_word_found_nowhere_are_both_answered() {
        let chat = redone_with(Uuid::new_v4(), &["позывной — КЕСТРЕЛЬ-9", "не знаю"]);
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let apart = find(&ctx, serde_json::json!({ "query": "занзибар кестрель" })).await;
        assert!(
            apart.starts_with("No message of a parked branch contains all these words together."),
            "{apart}"
        );
        assert!(apart.contains("\"занзибар\": 1"), "{apart}");
        assert!(apart.contains("\"кестрель\": 1"), "{apart}");

        let dropped = find(&ctx, serde_json::json!({ "query": "занзибар бегемот" })).await;
        assert!(
            dropped.starts_with(
                "Found in no parked branch, so left out of the search: \"бегемот\".\n\
                 Fragments found in the parked branches of this conversation: 1"
            ),
            "{dropped}"
        );
        assert!(dropped.contains("ЗАНЗИБАР-42"), "{dropped}");
    }

    #[tokio::test]
    async fn top_k_caps_the_fragments_and_the_total_is_honest() {
        let texts = ["ещё вариант"; 13];
        let chat = redone_with(Uuid::new_v4(), &texts);
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        // Twelve of the thirteen are parked; the thirteenth is the line.
        let out = find(&ctx, serde_json::json!({ "query": "вариант" })).await;
        assert!(out.contains(": 5 (matches in total: 12)"), "{out}");
        assert_eq!(out.matches("[assistant · ").count(), 5, "{out}");
        let three = find(&ctx, serde_json::json!({ "query": "вариант", "top_k": 3 })).await;
        assert!(three.contains(": 3 (matches in total: 12)"), "{three}");
        assert_eq!(three.matches("[assistant · ").count(), 3, "{three}");
        // Tails in the list's order, one heading each.
        assert!(three.contains("Branch \"6/2\""), "{three}");
        assert!(three.contains("Branch \"6/4\""), "{three}");
        assert!(!three.contains("Branch \"6/5\""), "{three}");
        // Whatever is asked, the ceiling holds.
        let many = find(
            &ctx,
            serde_json::json!({ "query": "вариант", "top_k": 500 }),
        )
        .await;
        assert!(many.contains(": 12 (matches in total: 12)"), "{many}");
    }

    #[tokio::test]
    async fn nothing_to_search_in_or_for_is_answered_usefully() {
        let bare = three_exchanges(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&bare, Lang::En);
        assert_eq!(
            find(&ctx, serde_json::json!({ "query": "пароль" })).await,
            "This conversation has no parked branches: everything said in it is in front of you."
        );

        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        assert_eq!(
            find(&ctx, serde_json::json!({ "query": "a ?" })).await,
            "The query has nothing to search for: single characters are ignored. Use a word \
             of two characters or more."
        );
        let err = BranchSearch
            .invoke(&ctx, serde_json::json!({ "query": "  " }))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Empty query: say what to search for.");
    }

    /// A tool's result is text of the tail too — what the index holds of a
    /// line — and its hit lands on the page of the call that produced it.
    #[tokio::test]
    async fn a_tool_result_in_a_tail_is_found() {
        use crate::entities::message::ToolCallRecord;
        let mut chat = three_exchanges(Uuid::new_v4());
        let mut asked = said(MessageRole::Assistant, "", 6);
        asked.tool_calls = vec![ToolCallRecord {
            id: "c1".into(),
            name: "weather".into(),
            arguments: serde_json::json!({}),
            result: Some("в порту шторм, 451 мм осадков".into()),
            thought_signature: None,
            images: 0,
            subagent: None,
        }];
        let mut result = said(MessageRole::Tool, "в порту шторм, 451 мм осадков", 7);
        result.tool_call_id = Some("c1".into());
        result.tool_name = Some("weather".into());
        chat.push_message(said(MessageRole::User, "а погода?", 6));
        chat.push_message(asked);
        chat.push_message(result);
        chat.push_message(said(MessageRole::Assistant, "штормит", 8));
        // The whole exchange is taken back: it is a tail of four messages.
        assert!(chat.park_tail(6, ParkCause::Takeback, at(20)));
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);

        let out = find(&ctx, serde_json::json!({ "query": "451 осадков" })).await;
        assert!(
            out.contains("Branch \"7/1\". An exchange the user took back, "),
            "{out}"
        );
        assert!(out.contains("1. [tool · "), "{out}");
        assert!(out.contains(" · page 1]"), "{out}");
        assert!(out.contains("451 мм"), "{out}");
    }

    #[tokio::test]
    async fn a_tail_inside_another_is_found_under_its_path() {
        let chat = forked_above(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::En);
        let out = find(&ctx, serde_json::json!({ "query": "занзибар" })).await;
        assert!(out.contains("Branch \"3/1 > 6/1\"."), "{out}");
        assert!(out.contains("(matches in total: 1)"), "{out}");
        // The outer tail holds the reply that replaced it, and its own words.
        let outer = find(&ctx, serde_json::json!({ "query": "помню" })).await;
        assert!(
            outer.contains("Branch \"3/1\". Set aside by a fork"),
            "{outer}"
        );
    }

    /// Another conversation's branches are searched under the reader's rule
    /// for them — and every refusal names this tool, not the reader, which the
    /// turn may not have.
    #[tokio::test]
    async fn another_chats_branches_are_searched_through_the_cross_chat_snapshot() {
        let open = three_exchanges(Uuid::new_v4());
        let (_d, storage, mut ctx) = ctx_over(&open, Lang::En);
        ctx.branch_reader = false;
        ctx.branch_search = true;
        let mut other = redone(open.profile_id);
        other.title = "лодка".into();
        storage.json().save_chat(&other).unwrap();
        let id = crate::features::chat_links::uri(other.id);

        let refused = find(&ctx, serde_json::json!({ "query": "пароль", "chat": id })).await;
        assert_eq!(
            refused,
            "Other conversations cannot be read on this turn. Call branch_search without chat \
             to search the branches of the current conversation."
        );

        ctx.other_chats = Arc::from(vec![other_ref(&other)]);
        let unknown = find(
            &ctx,
            serde_json::json!({ "query": "пароль", "chat": "причал" }),
        )
        .await;
        assert!(
            unknown.contains("call branch_search without chat"),
            "{unknown}"
        );
        assert!(!unknown.contains("branch_read"), "{unknown}");

        let out = find(
            &ctx,
            serde_json::json!({ "query": "занзибар", "chat": "лодка" }),
        )
        .await;
        assert!(
            out.starts_with(&format!(
                "Fragments found in the parked branches of the conversation \"лодка\" {id}: 1 \
                 (matches in total: 1). They are not part of its transcript."
            )),
            "{out}"
        );
        assert!(out.contains("Branch \"6/1\"."), "{out}");
        assert!(out.contains("ЗАНЗИБАР-42"), "{out}");
        // No reader on the turn: nothing reads a hit in full, and nothing is
        // named — the person's own command belongs to the open chat alone.
        assert!(!out.contains("branch_read"), "{out}");
        assert!(!out.contains("/branch"), "{out}");
        ctx.branch_reader = true;
        let out = find(
            &ctx,
            serde_json::json!({ "query": "занзибар", "chat": "лодка" }),
        )
        .await;
        assert!(out.ends_with("the page number and the same chat."), "{out}");

        let none = find(
            &ctx,
            serde_json::json!({ "query": "бегемот", "chat": "лодка" }),
        )
        .await;
        assert_eq!(
            none,
            "No parked branch of the conversation \"лодка\" matches that query. Try other \
             words; note its own transcript is not covered by this search."
        );

        // A conversation with no tails, and a sub-agent's transcript.
        let mut plain = three_exchanges(open.profile_id);
        plain.title = "причал".into();
        storage.json().save_chat(&plain).unwrap();
        let run = ChatRef {
            id: Uuid::new_v4(),
            title: "критик".into(),
            modified_at: plain.modified_at,
            parent: Some(crate::features::tools::chats::ParentRef {
                id: plain.id,
                title: plain.title.clone(),
            }),
        };
        ctx.other_chats = Arc::from(vec![other_ref(&other), other_ref(&plain), run]);
        let plain_out = find(
            &ctx,
            serde_json::json!({ "query": "пароль", "chat": "причал" }),
        )
        .await;
        assert_eq!(
            plain_out,
            "The conversation \"причал\" has no parked branches: its transcript is all of it."
        );
        let child = find(
            &ctx,
            serde_json::json!({ "query": "пароль", "chat": "критик" }),
        )
        .await;
        assert!(
            child.contains("has no branches: a subagent transcript"),
            "{child}"
        );
    }

    /// Each of the pair names the other only on a turn that has it
    /// (spec §6.7): the search names the reader under its hits, the reader's
    /// list names the search.
    #[tokio::test]
    async fn each_of_the_pair_names_the_other_only_where_the_turn_has_it() {
        let chat = redone(Uuid::new_v4());
        let (_d, _s, mut ctx) = ctx_over(&chat, Lang::En);

        // The reader alone: its list says nothing of a search.
        let list = call(&ctx, serde_json::json!({})).await;
        assert!(!list.contains("branch_search"), "{list}");
        ctx.branch_search = true;
        let list = call(&ctx, serde_json::json!({})).await;
        assert!(
            list.ends_with("To find the branch that says something, call branch_search."),
            "{list}"
        );

        // The search alone: a hit is quoted, and the way to the branch is the
        // person's command.
        ctx.branch_reader = false;
        let out = find(&ctx, serde_json::json!({ "query": "занзибар" })).await;
        assert!(!out.contains("branch_read"), "{out}");
        assert!(
            out.ends_with(
                "The user brings a branch back in front of them with the /branch command."
            ),
            "{out}"
        );
    }

    #[test]
    fn the_search_is_optional_catalogued_and_kept_from_a_subagent() {
        let all = crate::features::tools::all_tool_ids();
        assert!(all.iter().any(|t| t == BRANCH_SEARCH_ID));
        assert!(
            !crate::features::tools::default_tool_ids()
                .iter()
                .any(|t| t == BRANCH_SEARCH_ID),
            "finding what was set aside is a switch the person makes"
        );
        assert!(crate::features::tools::subagent::withheld_from_subagent(
            BRANCH_SEARCH_ID
        ));
        let info = crate::features::tools::tool_catalog()
            .into_iter()
            .find(|i| i.id == BRANCH_SEARCH_ID)
            .unwrap();
        assert_eq!(
            info.group,
            crate::features::tools::meta::ToolGroup::Conversation
        );
        assert!(info.concurrent);
        let note = info.note.expect("a note under the switch");
        for &lang in Lang::ALL {
            assert!(locale(lang).has_key(note), "{lang:?}: {note}");
        }
        // The searches' shared schema, and the reader's `chat` beside it.
        let schema = BranchSearch.parameters(locale(Lang::En));
        assert_eq!(schema["required"], serde_json::json!(["query"]));
        for name in ["query", "top_k", "chat"] {
            assert!(schema["properties"][name].is_object(), "{name}: {schema}");
        }
    }

    #[tokio::test]
    async fn the_search_answers_in_the_profiles_language() {
        let (en, ru) = (locale(Lang::En), locale(Lang::Ru));
        assert_ne!(BranchSearch.description(en), BranchSearch.description(ru));
        let chat = redone(Uuid::new_v4());
        let (_d, _s, ctx) = ctx_over(&chat, Lang::Ru);
        let out = find(&ctx, serde_json::json!({ "query": "занзибар" })).await;
        assert!(
            out.starts_with("Найдено фрагментов в отложенных ветках этого разговора: 1"),
            "{out}"
        );
        assert!(
            out.contains("Ветка «6/1». Ответ, заменённый перегенерацией, "),
            "{out}"
        );
        assert!(out.contains(" · страница 1]"), "{out}");
        assert!(
            out.ends_with("вызови branch_read со значением ветки и номером страницы."),
            "{out}"
        );
    }

    #[test]
    fn the_description_and_the_answers_are_localized() {
        let (en, ru) = (locale(Lang::En), locale(Lang::Ru));
        assert_ne!(BranchRead.description(en), BranchRead.description(ru));
        let chat = redone(Uuid::new_v4());
        let shelf = BranchShelf::of(&chat, ru).unwrap();
        let list = shelf.list(ru);
        assert!(list.contains("Ветка «6/1»"), "{list}");
        assert!(list.contains("Ответ, заменённый перегенерацией"), "{list}");
        assert!(
            list.contains("Продолжается после: Пользователь: «а пароль от лодки?»"),
            "{list}"
        );
    }
}
