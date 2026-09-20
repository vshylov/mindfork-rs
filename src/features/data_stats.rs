//! `mindfork stats` — a read-only summary of the user data, live or inside a
//! backup archive (docs/data-stats.md, spec §12.4).
//!
//! The command answers "which of my copies is the newest?" for data that lives
//! on several machines, so two properties carry the design:
//!
//! * **It changes nothing.** No directory is created, no migration runs, no
//!   database is opened read-write, and nothing of an archive is unpacked —
//!   chats are parsed from the zip entries and the database is loaded into
//!   memory ([`db::stats_of_image`]). A copy the summary had quietly upgraded
//!   would be a different copy.
//! * **It reads whatever is there.** Chat files go through a *projection*
//!   ([`ChatLite`]) that names only the counted fields, every one defaulted and
//!   the rest skipped without allocation — so a file from an older schema, or a
//!   newer one, is still counted, and the base64 image payloads are never
//!   materialized. One unreadable file is reported, not fatal; an unreadable
//!   database leaves the chat half of the summary standing.
//!
//! Messages are counted the way the chat list counts them — the bubbles the
//! feed draws, not storage rows — through the entity's own rule
//! ([`visible_row_count`]), pinned to the domain type by a test below.
//!
//! The `features` layer prints nothing: [`collect_root`]/[`collect_archive`]
//! return a [`DataStats`], and [`render_text`]/[`render_json`] turn it into the
//! string the CLI writes.
//!
//! **Stage 2** (docs/data-stats.md §5): a `DataStats` also carries what two
//! copies are lined up by — per chat the ids of its messages, per note a key
//! and a digest — so that [`compare`] can tell a copy that is *ahead* from one
//! that has *diverged*. The `--json` form is such a `DataStats` written out, and
//! [`read_snapshot`] reads it back; [`DataStats::fingerprint`] is a digest of
//! exactly that identity, so equal fingerprints mean an identical comparison.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;

use crate::entities::attachment::format_bytes;
use crate::entities::chat::visible_row_count;
use crate::entities::chat_file::sha256_hex;
use crate::entities::message::MessageRole;
use crate::features::backup::{self, ArchiveReader, BackupManifest};
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::db::{self, DbStats, KeyedRow};
use crate::shared::storage::schema::CHAT_SCHEMA;

/// The version of the `--json` shape. Additive fields do not bump it; a
/// renamed or re-defined one does — and so does one `--compare` cannot work
/// without: **2** added the per-chat message ids and the database rows, and a
/// format 1 snapshot is refused rather than compared by counts, which is the
/// comparison that reports a diverged chat as merely "newer there" (G1, G3).
pub const SNAPSHOT_FORMAT: u32 = 2;

/// How many hex digits of the SHA-256 the fingerprint shows.
const FINGERPRINT_HEX: usize = 16;

/// The entries of a backup archive this module reads (paths inside the
/// archive are relative to the data root — `features/backup.rs`).
const PROFILES_ENTRY: &str = "profiles.json";
const DB_ENTRY: &str = "data.db";
const CHATS_PREFIX: &str = "chats/";
/// What makes a zip *a backup of this app*: any one of these, or a chat.
const BACKUP_MARKERS: &[&str] = &["manifest.json", "settings.json", PROFILES_ENTRY, DB_ENTRY];

// ---------------------------------------------------------------- the result

/// Everything `mindfork stats` reports. Serialized as-is by `--json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataStats {
    pub format: u32,
    /// The version of the binary that produced the summary.
    pub app_version: String,
    /// When the summary was taken — what tells two snapshots of one machine apart.
    pub taken_at: DateTime<Utc>,
    /// A digest of what `--compare` treats as the copy's identity
    /// ([`DataStats::fingerprint`]).
    pub fingerprint: String,
    pub source: Source,
    pub profiles: ProfileTotals,
    pub chats: ChatTotals,
    pub database: DatabaseStats,
    pub sizes: Sizes,
    /// One row per readable chat, sorted by id — what a `diff` of two
    /// machines' snapshots lines up.
    pub chat_list: Vec<ChatRow>,
}

/// What was summarized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    DataRoot {
        path: String,
    },
    Archive {
        path: String,
        /// `None` — a backup older than the manifest, or one whose manifest
        /// does not parse.
        manifest: Option<BackupManifest>,
        /// The archive was made by a newer version of the app: the numbers
        /// are what this version could read of it.
        newer_than_this_app: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileTotals {
    pub total: usize,
    /// Soft-deleted (`is_hidden`).
    pub deleted: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTotals {
    pub total: usize,
    /// Soft-deleted (`is_hidden`).
    pub deleted: usize,
    /// Chat files that could not be read, by file name.
    pub unreadable: Vec<String>,
    /// Feed bubbles over **all** chats — the number the chat list's cards sum to.
    pub messages: usize,
    /// The part of `messages` that sits in soft-deleted chats.
    pub messages_in_deleted_chats: usize,
    /// Storage rows behind `messages` (tool results and agentic rounds included).
    pub message_rows: usize,
    /// Bubbles in the `deleted[]` archive (`Ctrl+E`, `Ctrl+R`, a rewrite).
    pub deleted_messages: usize,
    pub deleted_exchanges: usize,
    /// `/file attach` attachments.
    pub attachments: usize,
    /// Files stored with chats (`files/<chat-id>/`).
    pub stored_files: usize,
    /// Images carried by messages.
    pub images: usize,
    /// Distinct attached project directories.
    pub projects: usize,
    pub chats_with_project: usize,
    /// Sub-agent and dialogue transcripts nested on tool calls; their
    /// messages are **not** part of `messages`, as they are not part of the
    /// list's count.
    pub subagent_runs: usize,
    pub subagent_messages: usize,
    pub last_message_at: Option<DateTime<Utc>>,
    /// The newest `modified_at` — moves on a rename or a deletion too.
    pub last_change_at: Option<DateTime<Utc>>,
    /// The highest chat-file schema seen (0 — no chats).
    pub newest_schema: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DatabaseStats {
    /// There is no `data.db` — a copy that never stored a note.
    Missing,
    Ok(DbStats),
    Unreadable {
        error: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sizes {
    /// The counted chat files, uncompressed.
    pub chats_bytes: u64,
    pub database_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRow {
    pub id: Uuid,
    pub profile_id: Option<Uuid>,
    pub title: String,
    pub deleted: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub messages: usize,
    pub message_rows: usize,
    pub deleted_messages: usize,
    pub last_message_at: Option<DateTime<Utc>>,
    pub attachments: usize,
    pub stored_files: usize,
    /// The chat's messages in stored order, each as `<id>:<length>` — the
    /// identity a comparison works on (G1). The id is what tells "ahead" from
    /// "diverged"; the length of the text (with the thoughts) is what orders
    /// two versions of one message, since `/continue` is the one operation that
    /// changes a message under its id, and it only appends.
    pub message_index: Vec<String>,
    /// The same for the `deleted[]` archive. A regenerated reply lives on
    /// here, which is why regenerating reads as *ahead* and not as *diverged*.
    pub deleted_index: Vec<String>,
}

/// What a comparison treats as one chat's identity — and therefore what the
/// fingerprint hashes. Deliberately without the timestamps and the counts that
/// follow from these: two copies holding the same things are the same copy,
/// whenever each was last saved.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ChatIdentity<'a> {
    id: Uuid,
    title: &'a str,
    deleted: bool,
    attachments: usize,
    stored_files: usize,
    message_index: &'a [String],
    deleted_index: &'a [String],
}

impl ChatRow {
    fn identity(&self) -> ChatIdentity<'_> {
        ChatIdentity {
            id: self.id,
            title: &self.title,
            deleted: self.deleted,
            attachments: self.attachments,
            stored_files: self.stored_files,
            message_index: &self.message_index,
            deleted_index: &self.deleted_index,
        }
    }
}

/// The database's three lists, or `None` when the database could not be read —
/// which is not "no notes". A copy with **no** `data.db` has none of them, and
/// says so with three empty lists.
fn keyed_lists(database: &DatabaseStats) -> Option<[&[KeyedRow]; 3]> {
    match database {
        DatabaseStats::Missing => Some([&[], &[], &[]]),
        DatabaseStats::Ok(d) => Some([&d.note_list, &d.source_list, &d.self_model_list]),
        DatabaseStats::Unreadable { .. } => None,
    }
}

impl DataStats {
    /// A digest of everything [`compare`] looks at to call two copies
    /// identical: each chat's [`ChatIdentity`] and each database row's key and
    /// content digest. So **equal fingerprints mean `--compare` would say
    /// "identical"** — two machines can be checked by reading one line on each.
    fn fingerprint(chat_list: &[ChatRow], database: &DatabaseStats) -> String {
        let chats: Vec<_> = chat_list.iter().map(ChatRow::identity).collect();
        let rows: Vec<Vec<(&str, &str)>> = keyed_lists(database)
            .unwrap_or_default()
            .iter()
            .map(|list| {
                list.iter()
                    .map(|row| (row.key.as_str(), row.digest.as_str()))
                    .collect()
            })
            .collect();
        // Structs, strings and numbers: nothing in it can fail to serialize.
        let material = serde_json::to_vec(&(chats, rows)).unwrap_or_default();
        let mut digest = sha256_hex(&material);
        digest.truncate(FINGERPRINT_HEX);
        digest
    }

    /// Nothing was found at all — the difference between "this copy is empty"
    /// and "every number happens to be zero" that the text output spells out.
    pub fn is_empty(&self) -> bool {
        self.profiles.total == 0
            && self.chats.total == 0
            && self.chats.unreadable.is_empty()
            && self.database == DatabaseStats::Missing
    }
}

// ------------------------------------------------------------ the projection

/// The counted part of a chat file. Every field is defaulted: a file that
/// lacks one (an older schema) reads as "none of those", never as an error.
#[derive(Deserialize)]
struct ChatLite {
    #[serde(default = "chat_schema_v1")]
    v: u32,
    #[serde(default)]
    profile_id: Option<Uuid>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    modified_at: Option<DateTime<Utc>>,
    #[serde(default)]
    messages: Vec<MessageLite>,
    #[serde(default)]
    attachments: Vec<IgnoredAny>,
    #[serde(default)]
    files: Vec<IgnoredAny>,
    #[serde(default)]
    deleted: Vec<DeletedLite>,
    #[serde(default)]
    workspace: Option<WorkspaceLite>,
    #[serde(default)]
    is_hidden: bool,
}

/// A file without `v` predates the first chat-file step (`Chat::v`).
fn chat_schema_v1() -> u32 {
    1
}

#[derive(Deserialize)]
struct MessageLite {
    #[serde(default)]
    id: Option<Uuid>,
    #[serde(default)]
    text: Length,
    #[serde(default)]
    thoughts: Length,
    #[serde(default)]
    role: RoleLite,
    #[serde(default)]
    new_bubble: bool,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    images: Vec<IgnoredAny>,
    #[serde(default)]
    tool_calls: Vec<ToolCallLite>,
}

/// The length of a string, in characters, without keeping the string: a chat
/// file is mostly message text, and the summary needs none of it. `null` and
/// an absent field are zero.
#[derive(Default, Clone, Copy)]
struct Length(usize);

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Measure;
        impl serde::de::Visitor<'_> for Measure {
            type Value = Length;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or null")
            }

            fn visit_str<E>(self, text: &str) -> Result<Length, E> {
                Ok(Length(text.chars().count()))
            }

            fn visit_unit<E>(self) -> Result<Length, E> {
                Ok(Length(0))
            }
        }
        deserializer.deserialize_any(Measure)
    }
}

/// `<id>:<length>` for each message ([`ChatRow::message_index`]). A message
/// without an id — no schema version ever wrote one — still gets a line, under
/// the nil id, so that it is counted rather than dropped.
fn index(messages: &[MessageLite]) -> Vec<String> {
    messages
        .iter()
        .map(|m| {
            let id = m.id.unwrap_or_default();
            format!("{id}:{}", m.text.0 + m.thoughts.0)
        })
        .collect()
}

/// [`MessageRole`] with room for a role this binary does not know: a newer
/// file must still count, so the unknown one draws a row of its own, the way
/// `System` does.
#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum RoleLite {
    System,
    User,
    Assistant,
    Tool,
    #[default]
    #[serde(other)]
    Other,
}

impl RoleLite {
    fn role(self) -> MessageRole {
        match self {
            Self::User => MessageRole::User,
            Self::Assistant => MessageRole::Assistant,
            Self::Tool => MessageRole::Tool,
            Self::System | Self::Other => MessageRole::System,
        }
    }
}

#[derive(Deserialize)]
struct ToolCallLite {
    #[serde(default)]
    subagent: Option<RunLite>,
}

#[derive(Deserialize)]
struct RunLite {
    #[serde(default)]
    messages: Vec<MessageLite>,
}

#[derive(Deserialize)]
struct DeletedLite {
    #[serde(default)]
    messages: Vec<MessageLite>,
}

#[derive(Deserialize)]
struct WorkspaceLite {
    #[serde(default)]
    root: String,
}

fn bubbles(messages: &[MessageLite]) -> usize {
    visible_row_count(messages.iter().map(|m| (m.role.role(), m.new_bubble)))
}

/// The runs carried by `messages`' tool calls, and the bubbles they hold. One
/// level is all there is: a run is never given `call_subagent` (ADR 0010), so
/// it cannot start another.
fn runs(messages: &[MessageLite]) -> (usize, usize) {
    messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .filter_map(|call| call.subagent.as_ref())
        .fold((0, 0), |(count, held), run| {
            (count + 1, held + bubbles(&run.messages))
        })
}

// ---------------------------------------------------------------- the tally

/// Chat files, added one at a time, from either source.
#[derive(Default)]
struct Tally {
    totals: ChatTotals,
    project_roots: BTreeSet<String>,
    rows: Vec<ChatRow>,
    bytes: u64,
}

impl Tally {
    /// Adds the chat file `name` whose id (its file stem) is `id`.
    fn add(&mut self, name: &str, id: Uuid, content: &[u8]) {
        self.bytes += content.len() as u64;
        match serde_json::from_slice::<ChatLite>(content) {
            Ok(chat) => self.count(id, chat),
            Err(_) => self.unreadable(name),
        }
    }

    fn unreadable(&mut self, name: &str) {
        self.totals.unreadable.push(name.to_string());
    }

    fn count(&mut self, id: Uuid, chat: ChatLite) {
        let t = &mut self.totals;
        let messages = bubbles(&chat.messages);
        let deleted_messages: usize = chat.deleted.iter().map(|d| bubbles(&d.messages)).sum();
        let last_message_at = chat.messages.iter().filter_map(|m| m.timestamp).max();
        let (run_count, run_messages) = runs(&chat.messages);

        t.total += 1;
        t.messages += messages;
        t.message_rows += chat.messages.len();
        if chat.is_hidden {
            t.deleted += 1;
            t.messages_in_deleted_chats += messages;
        }
        t.deleted_messages += deleted_messages;
        t.deleted_exchanges += chat.deleted.len();
        t.attachments += chat.attachments.len();
        t.stored_files += chat.files.len();
        t.images += chat.messages.iter().map(|m| m.images.len()).sum::<usize>();
        t.subagent_runs += run_count;
        t.subagent_messages += run_messages;
        t.last_message_at = t.last_message_at.max(last_message_at);
        t.last_change_at = t.last_change_at.max(chat.modified_at);
        t.newest_schema = t.newest_schema.max(chat.v);
        if let Some(workspace) = &chat.workspace {
            t.chats_with_project += 1;
            self.project_roots.insert(workspace.root.clone());
        }

        self.rows.push(ChatRow {
            id,
            profile_id: chat.profile_id,
            title: chat.title,
            deleted: chat.is_hidden,
            created_at: chat.created_at,
            modified_at: chat.modified_at,
            messages,
            message_rows: chat.messages.len(),
            deleted_messages,
            last_message_at,
            attachments: chat.attachments.len(),
            stored_files: chat.files.len(),
            message_index: index(&chat.messages),
            deleted_index: chat
                .deleted
                .iter()
                .flat_map(|d| index(&d.messages))
                .collect(),
        });
    }

    fn finish(mut self) -> (ChatTotals, Vec<ChatRow>, u64) {
        self.totals.projects = self.project_roots.len();
        self.totals.unreadable.sort();
        self.rows.sort_by_key(|row| row.id);
        (self.totals, self.rows, self.bytes)
    }
}

/// The chat id a file name stands for: `<uuid>.json` — the rule
/// `JsonStore::chat_files` applies, so `*.bak` and stray files are not chats.
fn chat_id(file_name: &str) -> Option<Uuid> {
    Uuid::parse_str(file_name.strip_suffix(".json")?).ok()
}

/// `profiles.json`, read for two numbers. An array of objects today; a
/// wrapping object with a `profiles` array is accepted so a future envelope
/// does not turn into "0 profiles".
fn profile_totals(content: &[u8]) -> ProfileTotals {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(content) else {
        return ProfileTotals::default();
    };
    let list = value
        .as_array()
        .or_else(|| value.get("profiles").and_then(|p| p.as_array()));
    let Some(list) = list else {
        return ProfileTotals::default();
    };
    let hidden =
        |p: &&serde_json::Value| p.get("is_hidden").and_then(|h| h.as_bool()) == Some(true);
    ProfileTotals {
        total: list.len(),
        deleted: list.iter().filter(hidden).count(),
    }
}

fn assemble(
    source: Source,
    profiles: ProfileTotals,
    tally: Tally,
    database: DatabaseStats,
    database_bytes: u64,
) -> DataStats {
    let (chats, chat_list, chats_bytes) = tally.finish();
    DataStats {
        format: SNAPSHOT_FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        taken_at: Utc::now(),
        fingerprint: DataStats::fingerprint(&chat_list, &database),
        source,
        profiles,
        chats,
        database,
        sizes: Sizes {
            chats_bytes,
            database_bytes,
        },
        chat_list,
    }
}

// -------------------------------------------------------------- the sources

/// Summarizes the live data root. The root is only read — and may not exist
/// at all, which is a summary of its own ([`DataStats::is_empty`]). `Err` —
/// the chats directory exists and cannot be listed: that is not "no chats".
pub fn collect_root(paths: &Paths, loc: &Locale) -> Result<DataStats> {
    let mut tally = Tally::default();
    let dir = paths.chats_dir();
    if dir.is_dir() {
        let listing = fs::read_dir(&dir).with_context(|| {
            loc.tf(
                "cli.stats.ctx.list_chats",
                &[("path", &dir.display().to_string())],
            )
        })?;
        for entry in listing.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = chat_id(&name) else { continue };
            match fs::read(entry.path()) {
                Ok(content) => tally.add(&name, id, &content),
                Err(_) => tally.unreadable(&name),
            }
        }
    }

    let profiles = fs::read(paths.profiles_file())
        .map(|content| profile_totals(&content))
        .unwrap_or_default();

    let db_path = paths.data_db();
    let (database, database_bytes) = if db_path.is_file() {
        let size = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        (database_stats(db::stats_of_file(&db_path)), size)
    } else {
        (DatabaseStats::Missing, 0)
    };

    let source = Source::DataRoot {
        path: paths.root().display().to_string(),
    };
    Ok(assemble(source, profiles, tally, database, database_bytes))
}

/// Summarizes a backup archive without unpacking it. `password` — already
/// settled by the caller (argument, stored, prompt — the `restore` rule).
/// `Err` — the archive cannot be opened, the password is missing or wrong, or
/// the zip is not a backup of this app; all already localized.
pub fn collect_archive(archive: &Path, password: Option<&str>, loc: &Locale) -> Result<DataStats> {
    let mut reader = ArchiveReader::open(archive, password, loc)?;
    let entries = reader.entries();
    let is_backup = entries
        .iter()
        .any(|e| BACKUP_MARKERS.contains(&e.name.as_str()) || e.name.starts_with(CHATS_PREFIX));
    if !is_backup {
        bail!(
            "{}",
            loc.tf(
                "cli.stats.err.not_a_backup",
                &[("path", &archive.display().to_string())]
            )
        );
    }

    let mut tally = Tally::default();
    for entry in &entries {
        // Directly under `chats/` only — the rule the live listing applies.
        let Some(name) = entry.name.strip_prefix(CHATS_PREFIX) else {
            continue;
        };
        let Some(id) = chat_id(name) else { continue };
        match reader.with_entry(&entry.name, |stream, _| read_all(stream)) {
            Ok(Some(content)) => tally.add(name, id, &content),
            _ => tally.unreadable(name),
        }
    }

    let profiles = reader
        .with_entry(PROFILES_ENTRY, |stream, _| read_all(stream))
        .ok()
        .flatten()
        .map(|content| profile_totals(&content))
        .unwrap_or_default();

    let database_bytes = entries.iter().find(|e| e.name == DB_ENTRY).map(|e| e.size);
    let database = match database_bytes {
        None => DatabaseStats::Missing,
        Some(_) => database_stats(
            reader
                .with_entry(DB_ENTRY, |stream, size| {
                    db::stats_of_image(stream, usize::try_from(size)?)
                })
                .and_then(|found| found.context("the archive lists data.db and does not hold it")),
        ),
    };

    // An old backup has no manifest and a foreign one may not parse: neither
    // is a reason to refuse the numbers.
    let manifest = backup::read_manifest(archive).ok().flatten();
    let source = Source::Archive {
        path: archive.display().to_string(),
        newer_than_this_app: manifest
            .as_ref()
            .is_some_and(BackupManifest::is_newer_than_current),
        manifest,
    };
    Ok(assemble(
        source,
        profiles,
        tally,
        database,
        database_bytes.unwrap_or(0),
    ))
}

fn read_all(stream: &mut dyn Read) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    stream.read_to_end(&mut content)?;
    Ok(content)
}

fn database_stats(read: Result<DbStats>) -> DatabaseStats {
    match read {
        Ok(stats) => DatabaseStats::Ok(stats),
        Err(err) => DatabaseStats::Unreadable {
            error: format!("{err:#}"),
        },
    }
}

// ---------------------------------------------------------------- rendering

/// The `--json` form: pretty-printed with a stable key and row order, so two
/// machines' files line up under any `diff`.
pub fn render_json(stats: &DataStats) -> String {
    // A tree of strings, numbers and options: there is nothing in it that can
    // fail to serialize.
    serde_json::to_string_pretty(stats).unwrap_or_default()
}

/// Times are printed in UTC (docs/data-stats.md F9): the outputs of several
/// machines are laid side by side, and one zone makes that a string comparison.
fn utc(at: Option<DateTime<Utc>>, loc: &Locale) -> String {
    match at {
        Some(at) => at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => loc.t("cli.stats.none").to_string(),
    }
}

/// The human form, in the CLI's language.
pub fn render_text(stats: &DataStats, loc: &Locale) -> String {
    let mut out = vec![loc.tf(
        "cli.stats.title",
        &[("version", stats.app_version.as_str())],
    )];
    out.extend(source_lines(&stats.source, loc));
    out.push(String::new());
    if stats.is_empty() {
        out.push(loc.t("cli.stats.empty").to_string());
        return out.join("\n");
    }

    let rows = table_rows(stats, loc);
    let width = rows
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or(0);
    out.extend(rows.into_iter().map(|(label, value)| {
        let pad = " ".repeat(width - label.width());
        format!("{label}:{pad}  {value}")
    }));

    let c = &stats.chats;
    if !c.unreadable.is_empty() {
        out.push(String::new());
        out.push(loc.tf(
            "cli.stats.unreadable",
            &[
                ("count", &c.unreadable.len().to_string()),
                ("names", &c.unreadable.join(", ")),
            ],
        ));
    }
    if c.newest_schema > CHAT_SCHEMA {
        out.push(String::new());
        out.push(loc.t("cli.stats.newer_data").to_string());
    }
    out.join("\n")
}

fn source_lines(source: &Source, loc: &Locale) -> Vec<String> {
    match source {
        Source::DataRoot { path } => vec![loc.tf("cli.stats.source.root", &[("path", path)])],
        Source::Archive {
            path,
            manifest,
            newer_than_this_app,
        } => {
            let mut lines = vec![loc.tf("cli.stats.source.archive", &[("path", path)])];
            if let Some(m) = manifest {
                // Written as local RFC 3339; shown in UTC like every other
                // time here, and verbatim if it is not a timestamp at all.
                let created = DateTime::parse_from_rfc3339(&m.created_at)
                    .map(|at| utc(Some(at.with_timezone(&Utc)), loc))
                    .unwrap_or_else(|_| m.created_at.clone());
                lines.push(loc.tf(
                    "cli.stats.source.archive_made",
                    &[("created", &created), ("version", &m.app_version)],
                ));
            }
            if *newer_than_this_app {
                lines.push(loc.t("cli.stats.newer_data").to_string());
            }
            lines
        }
    }
}

/// The aligned `label: value` part. Wordings are number-neutral
/// (`Locale::tf`): a count never sits in front of a noun it would inflect.
fn table_rows(stats: &DataStats, loc: &Locale) -> Vec<(String, String)> {
    let c = &stats.chats;
    let n = |value: usize| value.to_string();
    let mut rows = vec![
        ("cli.stats.label.last_message", utc(c.last_message_at, loc)),
        ("cli.stats.label.last_change", utc(c.last_change_at, loc)),
        ("cli.stats.label.fingerprint", stats.fingerprint.clone()),
        (
            "cli.stats.label.profiles",
            with_deleted(loc, stats.profiles.total, stats.profiles.deleted),
        ),
        (
            "cli.stats.label.chats",
            with_deleted(loc, c.total, c.deleted),
        ),
        (
            "cli.stats.label.messages",
            loc.tf(
                "cli.stats.val.messages",
                &[
                    ("total", &n(c.messages)),
                    ("deleted", &n(c.messages_in_deleted_chats)),
                    ("rows", &n(c.message_rows)),
                ],
            ),
        ),
        (
            "cli.stats.label.deleted_messages",
            loc.tf(
                "cli.stats.val.deleted_messages",
                &[
                    ("total", &n(c.deleted_messages)),
                    ("exchanges", &n(c.deleted_exchanges)),
                ],
            ),
        ),
        ("cli.stats.label.attachments", n(c.attachments)),
        ("cli.stats.label.stored_files", n(c.stored_files)),
        ("cli.stats.label.images", n(c.images)),
        (
            "cli.stats.label.projects",
            loc.tf(
                "cli.stats.val.projects",
                &[
                    ("total", &n(c.projects)),
                    ("chats", &n(c.chats_with_project)),
                ],
            ),
        ),
        (
            "cli.stats.label.subagents",
            loc.tf(
                "cli.stats.val.subagents",
                &[
                    ("total", &n(c.subagent_runs)),
                    ("messages", &n(c.subagent_messages)),
                ],
            ),
        ),
    ];
    rows.extend(database_rows(&stats.database, loc));
    rows.push((
        "cli.stats.label.size",
        loc.tf(
            "cli.stats.val.size",
            &[
                ("chats", &size(stats.sizes.chats_bytes)),
                ("database", &size(stats.sizes.database_bytes)),
            ],
        ),
    ));
    rows.into_iter()
        .map(|(label, value)| (loc.t(label).to_string(), value))
        .collect()
}

fn database_rows(database: &DatabaseStats, loc: &Locale) -> Vec<(&'static str, String)> {
    let n = |value: u64| value.to_string();
    match database {
        DatabaseStats::Missing => vec![(
            "cli.stats.label.database",
            loc.t("cli.stats.val.database_missing").to_string(),
        )],
        DatabaseStats::Unreadable { error } => vec![(
            "cli.stats.label.database",
            loc.tf("cli.stats.val.database_unreadable", &[("err", error)]),
        )],
        DatabaseStats::Ok(d) => vec![
            (
                "cli.stats.label.notes",
                loc.tf(
                    "cli.stats.val.notes",
                    &[
                        ("total", &n(d.notes)),
                        ("superseded", &n(d.notes_superseded)),
                        ("links", &n(d.note_links)),
                    ],
                ),
            ),
            ("cli.stats.label.last_note", utc(d.last_note_change, loc)),
            (
                "cli.stats.label.rag",
                loc.tf(
                    "cli.stats.val.rag",
                    &[("sources", &n(d.rag_sources)), ("chunks", &n(d.rag_chunks))],
                ),
            ),
            ("cli.stats.label.self_models", n(d.self_models)),
        ],
    }
}

fn with_deleted(loc: &Locale, total: usize, deleted: usize) -> String {
    loc.tf(
        "cli.stats.val.with_deleted",
        &[
            ("total", &total.to_string()),
            ("deleted", &deleted.to_string()),
        ],
    )
}

fn size(bytes: u64) -> String {
    format_bytes(usize::try_from(bytes).unwrap_or(usize::MAX))
}

mod compare;
mod snapshot;

pub use compare::{compare, render_comparison_json, render_comparison_text};
pub use snapshot::{OtherCopy, other_copy, read_snapshot};

#[cfg(test)]
mod tests;
