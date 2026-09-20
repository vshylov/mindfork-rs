//! `mindfork stats --compare` — what each of two copies holds that the other
//! lacks (docs/data-stats.md §5, spec §12.4).
//!
//! The one answer this must never give falsely is *"this copy has everything"*:
//! it is the answer a user deletes the other copy on. So a chat is not compared
//! by its counts or its dates but by the **ids of its messages**, live and
//! deleted together (G1):
//!
//! * every id the other side holds is held here too → this side is **ahead**
//!   (or the two are the same);
//! * each side holds an id the other lacks → the chat **diverged** — continued
//!   on two machines from one base, which `(count, last message)` reports as
//!   "newer there" while losing what was written here.
//!
//! A regenerated or rewritten reply keeps its id in the chat's deleted archive,
//! which nothing prunes, so regenerating reads as *ahead* rather than as a
//! divergence. `/continue` is the one operation that changes a message under
//! its id; it only appends, so the longer text is the later one and counts as
//! held by that side alone.
//!
//! Notes, knowledge-base sources and self-models are edited in place and have
//! no such history: they are lined up by key, told apart by a content digest
//! and ordered by their own timestamp (G5).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::{ChatRow, DataStats, DatabaseStats, SNAPSHOT_FORMAT, Source};
use crate::shared::i18n::Locale;
use crate::shared::storage::db::KeyedRow;

// ---------------------------------------------------------------- the result

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Here,
    There,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing differs — what equal fingerprints promise.
    Identical,
    /// The same messages and rows everywhere; some chats differ in a title, a
    /// deleted mark, an attachment count or where a message sits.
    DetailsOnly,
    /// Keeping this copy loses nothing of the other.
    HereHasAll,
    /// Keeping the other copy loses nothing of this one.
    ThereHasAll,
    /// Keeping either alone loses something.
    EachHasSomething,
}

/// One of the two copies, as the comparison's header names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CopyInfo {
    /// The snapshot file this copy was read from, when it was one.
    pub snapshot: Option<String>,
    pub source: Source,
    pub app_version: String,
    pub taken_at: DateTime<Utc>,
    pub fingerprint: String,
}

/// Something that could not be compared. With one present the verdict covers
/// only what was, and the text says so before it says anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Caveat {
    UnreadableChats { side: Side, count: usize },
    DatabaseNotCompared { side: Side, error: String },
}

/// What differs between two copies of a chat holding the same messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Title,
    DeletedMark,
    Attachments,
    StoredFiles,
    /// The same ids, but not in the same place: an exchange deleted on one side.
    Deletions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatDiff {
    pub id: Uuid,
    pub title: String,
    pub deleted: bool,
    /// Feed bubbles on each side; `None` — the chat is not there at all.
    pub messages_here: Option<usize>,
    pub messages_there: Option<usize>,
    /// Messages — live or deleted — held on this side only, or longer here.
    pub here: usize,
    pub there: usize,
    /// For a chat with no such messages on either side: what differs.
    pub details: Vec<Detail>,
    /// Which copy of the chat has the later `modified_at`, when they differ.
    /// A hint for the reader, never the identity: a rename does not move it.
    pub later: Option<Side>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChatComparison {
    pub same: usize,
    pub only_here: Vec<ChatDiff>,
    pub only_there: Vec<ChatDiff>,
    pub ahead_here: Vec<ChatDiff>,
    pub ahead_there: Vec<ChatDiff>,
    pub diverged: Vec<ChatDiff>,
    pub details: Vec<ChatDiff>,
}

/// One database row that is not the same on both sides, with each side's time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyedDiff {
    pub key: String,
    pub here: Option<String>,
    pub there: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct KeyedComparison {
    pub same: usize,
    pub only_here: Vec<KeyedDiff>,
    pub only_there: Vec<KeyedDiff>,
    pub newer_here: Vec<KeyedDiff>,
    pub newer_there: Vec<KeyedDiff>,
    /// Different content under one timestamp: which is later cannot be said.
    pub differing: Vec<KeyedDiff>,
}

/// Everything `mindfork stats --compare` reports. Serialized as-is by `--json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    pub format: u32,
    pub app_version: String,
    pub here: CopyInfo,
    pub there: CopyInfo,
    pub verdict: Verdict,
    pub caveats: Vec<Caveat>,
    pub chats: ChatComparison,
    pub notes: KeyedComparison,
    pub knowledge_base: KeyedComparison,
    pub self_models: KeyedComparison,
}

// ------------------------------------------------------------ the comparison

/// Compares two copies. `there_snapshot` — the file the other copy was read
/// from, when it was a snapshot (an archive names itself in its `Source`).
pub fn compare(here: &DataStats, there: &DataStats, there_snapshot: Option<String>) -> Comparison {
    let mut caveats = Vec::new();
    for (side, copy) in [(Side::Here, here), (Side::There, there)] {
        if !copy.chats.unreadable.is_empty() {
            caveats.push(Caveat::UnreadableChats {
                side,
                count: copy.chats.unreadable.len(),
            });
        }
        if let DatabaseStats::Unreadable { error } = &copy.database {
            caveats.push(Caveat::DatabaseNotCompared {
                side,
                error: error.clone(),
            });
        }
    }
    let chats = compare_chats(&here.chat_list, &there.chat_list);
    // A database that could not be read is not "no notes": with either side
    // unreadable the three families are left uncompared, and a caveat says so.
    let [notes, knowledge_base, self_models] = match (
        super::keyed_lists(&here.database),
        super::keyed_lists(&there.database),
    ) {
        (Some(h), Some(t)) => [0, 1, 2].map(|i| compare_keyed(h[i], t[i])),
        _ => Default::default(),
    };
    let families = [&notes, &knowledge_base, &self_models];
    Comparison {
        format: SNAPSHOT_FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        here: copy_info(here, None),
        there: copy_info(there, there_snapshot),
        verdict: verdict(&chats, families),
        caveats,
        chats,
        notes,
        knowledge_base,
        self_models,
    }
}

fn copy_info(copy: &DataStats, snapshot: Option<String>) -> CopyInfo {
    CopyInfo {
        snapshot,
        source: copy.source.clone(),
        app_version: copy.app_version.clone(),
        taken_at: copy.taken_at,
        fingerprint: copy.fingerprint.clone(),
    }
}

/// `id → length` over a chat's live **and** deleted messages. An entry that
/// does not read as `<id>:<length>` (a hand-edited snapshot) is left out.
fn held(row: &ChatRow) -> BTreeMap<&str, usize> {
    let mut held = BTreeMap::new();
    for entry in row.message_index.iter().chain(&row.deleted_index) {
        if let Some((id, length)) = entry.rsplit_once(':')
            && let Ok(length) = length.parse::<usize>()
        {
            let longest = held.entry(id).or_insert(length);
            *longest = (*longest).max(length);
        }
    }
    held
}

/// How many of `mine` the other side does not hold, or holds shorter.
fn surplus(mine: &BTreeMap<&str, usize>, theirs: &BTreeMap<&str, usize>) -> usize {
    mine.iter()
        .filter(|(id, length)| theirs.get(*id).is_none_or(|other| *length > other))
        .count()
}

fn later(here: &ChatRow, there: &ChatRow) -> Option<Side> {
    match here.modified_at.cmp(&there.modified_at) {
        std::cmp::Ordering::Greater => Some(Side::Here),
        std::cmp::Ordering::Less => Some(Side::There),
        std::cmp::Ordering::Equal => None,
    }
}

fn details(here: &ChatRow, there: &ChatRow) -> Vec<Detail> {
    let moved =
        here.message_index != there.message_index || here.deleted_index != there.deleted_index;
    [
        (Detail::Title, here.title != there.title),
        (Detail::DeletedMark, here.deleted != there.deleted),
        (Detail::Attachments, here.attachments != there.attachments),
        (Detail::StoredFiles, here.stored_files != there.stored_files),
        (Detail::Deletions, moved),
    ]
    .into_iter()
    .filter_map(|(detail, differs)| differs.then_some(detail))
    .collect()
}

impl ChatDiff {
    /// A chat that exists on one side only.
    fn lone(row: &ChatRow, side: Side) -> Self {
        let rows = row.message_index.len() + row.deleted_index.len();
        let (messages_here, messages_there, here, there) = match side {
            Side::Here => (Some(row.messages), None, rows, 0),
            Side::There => (None, Some(row.messages), 0, rows),
        };
        Self {
            id: row.id,
            title: row.title.clone(),
            deleted: row.deleted,
            messages_here,
            messages_there,
            here,
            there,
            details: Vec::new(),
            later: None,
        }
    }

    fn pair(here: &ChatRow, there: &ChatRow) -> Self {
        let (mine, theirs) = (held(here), held(there));
        let (more_here, more_there) = (surplus(&mine, &theirs), surplus(&theirs, &mine));
        // Details are what is left to say when neither side has a message the
        // other lacks; next to such messages they would only restate them (the
        // two id lists always differ then).
        let same_messages = more_here == 0 && more_there == 0;
        Self {
            id: here.id,
            title: here.title.clone(),
            deleted: here.deleted,
            messages_here: Some(here.messages),
            messages_there: Some(there.messages),
            here: more_here,
            there: more_there,
            details: if same_messages {
                details(here, there)
            } else {
                Vec::new()
            },
            later: later(here, there),
        }
    }
}

fn compare_chats(here: &[ChatRow], there: &[ChatRow]) -> ChatComparison {
    let mut others: BTreeMap<Uuid, &ChatRow> = there.iter().map(|row| (row.id, row)).collect();
    let mut out = ChatComparison::default();
    for row in here {
        let Some(other) = others.remove(&row.id) else {
            out.only_here.push(ChatDiff::lone(row, Side::Here));
            continue;
        };
        let diff = ChatDiff::pair(row, other);
        match (diff.here > 0, diff.there > 0) {
            (true, true) => out.diverged.push(diff),
            (true, false) => out.ahead_here.push(diff),
            (false, true) => out.ahead_there.push(diff),
            // "The same" is the identity the fingerprint hashes — one
            // definition, so equal fingerprints and "identical" cannot drift
            // apart. `details` only *names* what differs.
            (false, false) if row.identity() == other.identity() => out.same += 1,
            (false, false) => out.details.push(diff),
        }
    }
    out.only_there = others
        .into_values()
        .map(|row| ChatDiff::lone(row, Side::There))
        .collect();
    out
}

/// Orders two stored timestamps: as instants when both parse, else as text —
/// which agrees for the UTC RFC 3339 the app writes.
fn order(here: &str, there: &str) -> std::cmp::Ordering {
    let parsed = |at: &str| DateTime::parse_from_rfc3339(at).ok();
    match (parsed(here), parsed(there)) {
        (Some(h), Some(t)) => h.cmp(&t),
        _ => here.cmp(there),
    }
}

fn compare_keyed(here: &[KeyedRow], there: &[KeyedRow]) -> KeyedComparison {
    let mut others: BTreeMap<&str, &KeyedRow> =
        there.iter().map(|row| (row.key.as_str(), row)).collect();
    let mut out = KeyedComparison::default();
    for row in here {
        let other = others.remove(row.key.as_str());
        let diff = KeyedDiff {
            key: row.key.clone(),
            here: Some(row.at.clone()),
            there: other.map(|o| o.at.clone()),
        };
        match other {
            None => out.only_here.push(diff),
            Some(o) if o.digest == row.digest => out.same += 1,
            Some(o) => match order(&row.at, &o.at) {
                std::cmp::Ordering::Greater => out.newer_here.push(diff),
                std::cmp::Ordering::Less => out.newer_there.push(diff),
                std::cmp::Ordering::Equal => out.differing.push(diff),
            },
        }
    }
    out.only_there = others
        .into_values()
        .map(|row| KeyedDiff {
            key: row.key.clone(),
            here: None,
            there: Some(row.at.clone()),
        })
        .collect();
    out
}

/// Whether a side holds something the other lacks. A difference whose
/// direction is unknown counts for **both**: the verdict errs towards "each
/// has something", never towards "this one has everything".
fn verdict(chats: &ChatComparison, families: [&KeyedComparison; 3]) -> Verdict {
    let unordered = families.iter().any(|f| !f.differing.is_empty());
    let here = !chats.only_here.is_empty()
        || !chats.ahead_here.is_empty()
        || !chats.diverged.is_empty()
        || unordered
        || families
            .iter()
            .any(|f| !f.only_here.is_empty() || !f.newer_here.is_empty());
    let there = !chats.only_there.is_empty()
        || !chats.ahead_there.is_empty()
        || !chats.diverged.is_empty()
        || unordered
        || families
            .iter()
            .any(|f| !f.only_there.is_empty() || !f.newer_there.is_empty());
    match (here, there) {
        (true, true) => Verdict::EachHasSomething,
        (true, false) => Verdict::HereHasAll,
        (false, true) => Verdict::ThereHasAll,
        (false, false) if chats.details.is_empty() => Verdict::Identical,
        (false, false) => Verdict::DetailsOnly,
    }
}

// ---------------------------------------------------------------- rendering

/// The `--json` form of a comparison.
pub fn render_comparison_json(comparison: &Comparison) -> String {
    // Strings, numbers, options and unit enums: nothing in it can fail.
    serde_json::to_string_pretty(comparison).unwrap_or_default()
}

/// The human form, in the CLI's language.
pub fn render_comparison_text(c: &Comparison, loc: &Locale) -> String {
    let mut out = vec![loc.tf("cli.stats.cmp.title", &[("version", &c.app_version)])];
    out.extend(copy_lines("cli.stats.cmp.here", &c.here, loc));
    out.extend(copy_lines("cli.stats.cmp.there", &c.there, loc));
    out.push(String::new());
    if !c.caveats.is_empty() {
        out.push(loc.t("cli.stats.cmp.partial").to_string());
        out.extend(c.caveats.iter().map(|caveat| caveat_line(caveat, loc)));
    }
    out.push(loc.t(verdict_key(c.verdict)).to_string());
    out.push(String::new());

    let families = [
        ("cli.stats.label.notes", &c.notes),
        ("cli.stats.label.rag", &c.knowledge_base),
        ("cli.stats.label.self_models", &c.self_models),
    ];
    out.push(format!(
        "{}: {}",
        loc.t("cli.stats.label.chats"),
        chat_counts(&c.chats, loc)
    ));
    for (label, family) in families {
        out.push(format!("{}: {}", loc.t(label), keyed_counts(family, loc)));
    }

    out.extend(chat_sections(&c.chats, loc));
    for (label, family) in families {
        out.extend(keyed_sections(loc.t(label), family, loc));
    }
    out.join("\n")
}

fn verdict_key(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Identical => "cli.stats.cmp.verdict.identical",
        Verdict::DetailsOnly => "cli.stats.cmp.verdict.details_only",
        Verdict::HereHasAll => "cli.stats.cmp.verdict.here_has_all",
        Verdict::ThereHasAll => "cli.stats.cmp.verdict.there_has_all",
        Verdict::EachHasSomething => "cli.stats.cmp.verdict.each_has_something",
    }
}

fn side_name(side: Side, loc: &Locale) -> &str {
    match side {
        Side::Here => loc.t("cli.stats.cmp.side.here"),
        Side::There => loc.t("cli.stats.cmp.side.there"),
    }
}

fn caveat_line(caveat: &Caveat, loc: &Locale) -> String {
    match caveat {
        Caveat::UnreadableChats { side, count } => loc.tf(
            "cli.stats.cmp.caveat.unreadable_chats",
            &[
                ("side", side_name(*side, loc)),
                ("count", &count.to_string()),
            ],
        ),
        Caveat::DatabaseNotCompared { side, error } => loc.tf(
            "cli.stats.cmp.caveat.database",
            &[("side", side_name(*side, loc)), ("err", error)],
        ),
    }
}

/// The header block of one copy: its label, the snapshot it came from when it
/// did, and the summary's own source lines, indented.
fn copy_lines(label: &str, copy: &CopyInfo, loc: &Locale) -> Vec<String> {
    let mut lines = vec![loc.t(label).to_string()];
    if let Some(path) = &copy.snapshot {
        lines.push(format!(
            "  {}",
            loc.tf(
                "cli.stats.cmp.snapshot",
                &[
                    ("path", path),
                    ("taken", &super::utc(Some(copy.taken_at), loc)),
                    ("version", &copy.app_version),
                ],
            )
        ));
    }
    let source = super::source_lines(&copy.source, loc);
    lines.extend(source.into_iter().map(|line| format!("  {line}")));
    lines
}

fn chat_counts(chats: &ChatComparison, loc: &Locale) -> String {
    let n = |list: &Vec<ChatDiff>| list.len().to_string();
    loc.tf(
        "cli.stats.cmp.counts.chats",
        &[
            ("same", &chats.same.to_string()),
            ("only_here", &n(&chats.only_here)),
            ("only_there", &n(&chats.only_there)),
            ("ahead_here", &n(&chats.ahead_here)),
            ("ahead_there", &n(&chats.ahead_there)),
            ("diverged", &n(&chats.diverged)),
            ("details", &n(&chats.details)),
        ],
    )
}

fn keyed_counts(family: &KeyedComparison, loc: &Locale) -> String {
    let n = |list: &Vec<KeyedDiff>| list.len().to_string();
    loc.tf(
        "cli.stats.cmp.counts.keyed",
        &[
            ("same", &family.same.to_string()),
            ("only_here", &n(&family.only_here)),
            ("only_there", &n(&family.only_there)),
            ("newer_here", &n(&family.newer_here)),
            ("newer_there", &n(&family.newer_there)),
            ("differing", &n(&family.differing)),
        ],
    )
}

/// `{family}: {kind} ({count})`, then a line per item — for a non-empty list.
fn section(family: &str, kind: &str, lines: Vec<String>, loc: &Locale) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let heading = loc.tf(
        "cli.stats.cmp.heading",
        &[
            ("family", family),
            ("kind", loc.t(kind)),
            ("count", &lines.len().to_string()),
        ],
    );
    [String::new(), heading].into_iter().chain(lines).collect()
}

fn chat_sections(chats: &ChatComparison, loc: &Locale) -> Vec<String> {
    let family = loc.t("cli.stats.label.chats");
    [
        ("cli.stats.cmp.kind.only_here", &chats.only_here),
        ("cli.stats.cmp.kind.only_there", &chats.only_there),
        ("cli.stats.cmp.kind.ahead_here", &chats.ahead_here),
        ("cli.stats.cmp.kind.ahead_there", &chats.ahead_there),
        ("cli.stats.cmp.kind.diverged", &chats.diverged),
        ("cli.stats.cmp.kind.details", &chats.details),
    ]
    .into_iter()
    .flat_map(|(kind, list)| {
        let lines = list.iter().map(|diff| chat_line(diff, loc)).collect();
        section(family, kind, lines, loc)
    })
    .collect()
}

fn keyed_sections(family: &str, keyed: &KeyedComparison, loc: &Locale) -> Vec<String> {
    [
        ("cli.stats.cmp.kind.only_here", &keyed.only_here),
        ("cli.stats.cmp.kind.only_there", &keyed.only_there),
        ("cli.stats.cmp.kind.newer_here", &keyed.newer_here),
        ("cli.stats.cmp.kind.newer_there", &keyed.newer_there),
        ("cli.stats.cmp.kind.differing", &keyed.differing),
    ]
    .into_iter()
    .flat_map(|(kind, list)| {
        let lines = list.iter().map(|diff| keyed_line(diff, loc)).collect();
        section(family, kind, lines, loc)
    })
    .collect()
}

/// `  <id>  <what differs>  <title>` — the id first, being the one fixed width.
fn chat_line(diff: &ChatDiff, loc: &Locale) -> String {
    let mut what = match (diff.messages_here, diff.messages_there) {
        (Some(count), None) | (None, Some(count)) => loc.tf(
            "cli.stats.cmp.detail.messages",
            &[("count", &count.to_string())],
        ),
        _ if diff.details.is_empty() => loc.tf(
            "cli.stats.cmp.detail.delta",
            &[
                ("here", &diff.here.to_string()),
                ("there", &diff.there.to_string()),
            ],
        ),
        _ => details_text(diff, loc),
    };
    if diff.deleted {
        what = format!("{what}, {}", loc.t("cli.stats.cmp.detail.deleted_chat"));
    }
    format!("  {}  {what}  {}", diff.id, diff.title)
}

fn details_text(diff: &ChatDiff, loc: &Locale) -> String {
    let what: Vec<&str> = diff
        .details
        .iter()
        .map(|detail| match detail {
            Detail::Title => loc.t("cli.stats.cmp.what.title"),
            Detail::DeletedMark => loc.t("cli.stats.cmp.what.deleted_mark"),
            Detail::Attachments => loc.t("cli.stats.cmp.what.attachments"),
            Detail::StoredFiles => loc.t("cli.stats.cmp.what.stored_files"),
            Detail::Deletions => loc.t("cli.stats.cmp.what.deletions"),
        })
        .collect();
    let later = match diff.later {
        Some(Side::Here) => "cli.stats.cmp.detail.later_here",
        Some(Side::There) => "cli.stats.cmp.detail.later_there",
        None => "cli.stats.cmp.detail.later_unknown",
    };
    format!("{}; {}", what.join(", "), loc.t(later))
}

fn keyed_line(diff: &KeyedDiff, loc: &Locale) -> String {
    let shown = |at: &Option<String>| match at {
        None => loc.t("cli.stats.none").to_string(),
        Some(at) => DateTime::parse_from_rfc3339(at)
            .map(|at| super::utc(Some(at.with_timezone(&Utc)), loc))
            .unwrap_or_else(|_| at.clone()),
    };
    let times = loc.tf(
        "cli.stats.cmp.detail.times",
        &[("here", &shown(&diff.here)), ("there", &shown(&diff.there))],
    );
    format!("  {}  {times}", diff.key)
}

#[cfg(test)]
mod tests;
