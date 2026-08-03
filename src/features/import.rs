//! Importing data from the neutral **mindfork-import** format (spec §12.2):
//! profiles and chats emitted by an external converter. The format spec is
//! [docs/import-format.md]; the choice of the "external converter →
//! documented file → import" pattern comes from the plugin-system research
//! (docs/research). One-shot, **idempotent** (deterministic UUIDv5 from
//! stable `key`s), the source file is only ever read.
//!
//! Strict about structure (a wrong `format`, duplicate keys, a reference to a
//! missing profile, an unknown role → a clear error), tolerant of extension
//! (unknown fields are ignored — compatible evolution with no version bump, a
//! mirror of the `#[serde(default)]` policy of our own schemas).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::entities::chat::{Chat, FeedView};
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::default_tool_ids;
use crate::shared::config::Theme;
use crate::shared::i18n::{Lang, Locale};

/// Maximum supported format version (see docs/import-format.md).
pub const FORMAT_VERSION: u32 = 1;
/// The required value of the `format` field.
const FORMAT_NAME: &str = "mindfork-import";

/// UUIDv5 namespace for profile ids: `UUIDv5(NS, key)`. ASCII
/// `mindfork import` + tag `0001`. Fixed — a repeat import gives the same ids
/// (idempotency); the constants are documented in docs/import-format.md.
const PROFILE_NAMESPACE: Uuid = Uuid::from_u128(0x6d69_6e64_666f_726b_696d_706f_7274_0001_u128);
/// UUIDv5 namespace for chat ids (tag `0002`). Separate from profiles — a
/// matching `key` for a profile and a chat doesn't collide into one UUID.
const CHAT_NAMESPACE: Uuid = Uuid::from_u128(0x6d69_6e64_666f_726b_696d_706f_7274_0002_u128);

/// Import result: profiles, chats, and (optionally) the source's global
/// settings (sampling/interface) to apply to `AppConfig`.
#[derive(Debug, Default)]
pub struct ImportResult {
    pub profiles: Vec<Profile>,
    pub chats: Vec<Chat>,
    /// The source's global sampling (unknown fields are dropped during parsing).
    pub sampling: Option<SamplingConfig>,
    /// The source's interface settings; every field is optional — only what's
    /// set is applied (a partial transfer doesn't overwrite the user's settings).
    pub interface: Option<ImportedInterface>,
}

/// Transferred interface settings (all fields optional).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedInterface {
    pub spellcheck_enabled: Option<bool>,
    pub dictionaries: Option<Vec<String>>,
    pub theme: Option<Theme>,
}

// ---------- format wire types (partial; unknown fields are ignored) ----------

#[derive(Debug, Deserialize)]
struct ImFile {
    format: Option<String>,
    version: Option<u32>,
    #[serde(default)]
    profiles: Vec<ImProfile>,
    #[serde(default)]
    chats: Vec<ImChat>,
    settings: Option<ImSettings>,
}

#[derive(Debug, Deserialize)]
struct ImProfile {
    #[serde(default)]
    key: String,
    #[serde(default)]
    name: String,
    /// An explicit UUID instead of a deterministic one (continuity with previously
    /// imported data — see docs/import-format.md §Identifiers).
    id: Option<Uuid>,
    /// Agent-scaffold language (`ru`/`en`/an external locale code, axis A).
    language: Option<String>,
    #[serde(default)]
    system_message: String,
    greeting: Option<String>,
    character_names: Option<ImCharacters>,
    sampling: Option<SamplingConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct ImCharacters {
    user: Option<String>,
    assistant: Option<String>,
    system: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImChat {
    #[serde(default)]
    key: String,
    #[serde(default)]
    profile_key: String,
    /// An explicit UUID instead of a deterministic one (as with a profile).
    id: Option<Uuid>,
    #[serde(default)]
    title: String,
    created_at: Option<String>,
    modified_at: Option<String>,
    /// An explicit system message; missing/empty → the first system message from `messages`.
    system_message: Option<String>,
    character_names: Option<ImCharacters>,
    #[serde(default)]
    messages: Vec<ImMessage>,
}

#[derive(Debug, Deserialize)]
struct ImMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    text: String,
    thoughts: Option<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImSettings {
    sampling: Option<SamplingConfig>,
    interface: Option<ImInterface>,
}

#[derive(Debug, Deserialize)]
struct ImInterface {
    spellcheck_enabled: Option<bool>,
    dictionaries: Option<Vec<String>>,
    theme: Option<String>,
}

// ---------- parsing (pure functions, testable on fixtures) ----------

/// Parses a mindfork-import format file (content as a string). Validation:
/// `format`/`version`, non-empty unique keys, chat references to profiles in this
/// same file, known message roles. A leading BOM is dropped.
pub fn parse_import(json: &str, loc: &Locale) -> Result<ImportResult> {
    let json = json.trim_start_matches('\u{feff}');
    let f: ImFile =
        serde_json::from_str(json).with_context(|| loc.t("import.ctx.parse").to_string())?;

    match f.format.as_deref() {
        Some(FORMAT_NAME) => {}
        other => bail!(
            "{}",
            loc.tf(
                "import.err.not_import_file",
                &[("found", other.unwrap_or("—"))]
            )
        ),
    }
    match f.version {
        Some(v) if (1..=FORMAT_VERSION).contains(&v) => {}
        Some(v) if v > FORMAT_VERSION => bail!(
            "{}",
            loc.tf(
                "import.err.newer_version",
                &[
                    ("version", &v.to_string()),
                    ("supported", &FORMAT_VERSION.to_string()),
                ]
            )
        ),
        _ => bail!("{}", loc.t("import.err.no_version")),
    }

    // Profiles: non-empty unique keys → deterministic ids.
    let mut profile_ids: HashMap<&str, Uuid> = HashMap::new();
    let mut profiles = Vec::new();
    for (i, p) in f.profiles.iter().enumerate() {
        if p.key.is_empty() {
            bail!(
                "{}",
                loc.tf("import.err.profile_empty_key", &[("index", &i.to_string())])
            );
        }
        let profile = map_profile(p);
        if profile_ids.insert(&p.key, profile.id).is_some() {
            bail!(
                "{}",
                loc.tf("import.err.dup_profile_key", &[("key", &p.key)])
            );
        }
        profiles.push(profile);
    }

    // Chats: non-empty unique keys + a reference to a profile from THIS file.
    let mut chat_keys: HashSet<&str> = HashSet::new();
    let mut chats = Vec::new();
    for (i, c) in f.chats.iter().enumerate() {
        if c.key.is_empty() {
            bail!(
                "{}",
                loc.tf("import.err.chat_empty_key", &[("index", &i.to_string())])
            );
        }
        if !chat_keys.insert(&c.key) {
            bail!("{}", loc.tf("import.err.dup_chat_key", &[("key", &c.key)]));
        }
        let Some(&profile_id) = profile_ids.get(c.profile_key.as_str()) else {
            bail!(
                "{}",
                loc.tf(
                    "import.err.unknown_profile_key",
                    &[("chat", &c.key), ("profile_key", &c.profile_key)]
                )
            );
        };
        chats.push(map_chat(c, profile_id, loc)?);
    }

    let (sampling, interface) = match f.settings {
        Some(s) => (s.sampling, s.interface.map(map_interface)),
        None => (None, None),
    };

    Ok(ImportResult {
        profiles,
        chats,
        sampling,
        interface,
    })
}

/// Reads and parses a mindfork-import format file from disk.
pub fn import_file(path: &Path, loc: &Locale) -> Result<ImportResult> {
    let bytes = fs::read(path)
        .with_context(|| loc.tf("import.ctx.read", &[("path", &path.display().to_string())]))?;
    parse_import(&String::from_utf8_lossy(&bytes), loc)
}

// ---------- mapping into domain entities ----------

/// A format profile → a mindfork profile. id is derived from `key` (or explicit).
fn map_profile(p: &ImProfile) -> Profile {
    let id =
        p.id.unwrap_or_else(|| Uuid::new_v5(&PROFILE_NAMESPACE, p.key.as_bytes()));
    Profile {
        id,
        name: p.name.clone(),
        default_system_message: p.system_message.clone(),
        language: p
            .language
            .as_deref()
            .map(Lang::from_code)
            .unwrap_or_default(),
        // Impersonation isn't carried over by format v1 — the shared default.
        impersonation_profile_id: None,
        impersonation_system_message: String::new(),
        character_names: map_characters(p.character_names.as_ref()),
        greeting: p.greeting.clone().filter(|g| !g.is_empty()),
        // Imported profiles get the standard tool set;
        // the "known" registry = the same set (new ones will be added by reconcile_tools,
        // without re-enabling disabled ones). See spec §9.4.
        enabled_tools: default_tool_ids(),
        known_tools: default_tool_ids(),
        default_sampling: p.sampling.clone(),
        is_hidden: false,
    }
}

/// A format chat → a mindfork chat, bound to `profile_id`. System message:
/// the explicit field takes priority, else the first system message in the history; system messages
/// don't go into the history (in mindfork the system one is stored separately).
fn map_chat(c: &ImChat, profile_id: Uuid, loc: &Locale) -> Result<Chat> {
    let id =
        c.id.unwrap_or_else(|| Uuid::new_v5(&CHAT_NAMESPACE, c.key.as_bytes()));
    let created = parse_time(c.created_at.as_deref());
    let modified = parse_time(c.modified_at.as_deref()).or(created);

    let explicit_system = c.system_message.clone().filter(|s| !s.is_empty());
    let mut first_system: Option<String> = None;
    let mut messages = Vec::new();
    for (i, m) in c.messages.iter().enumerate() {
        let role = match m.role.to_ascii_lowercase().as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => {
                if first_system.is_none() {
                    first_system = Some(m.text.clone());
                }
                continue;
            }
            other => bail!(
                "{}",
                loc.tf(
                    "import.err.bad_role",
                    &[("chat", &c.key), ("index", &i.to_string()), ("role", other)]
                )
            ),
        };
        let mut msg = Message::new(role, m.text.clone());
        msg.thoughts = m.thoughts.clone().filter(|t| !t.is_empty());
        if let Some(ts) = parse_time(m.timestamp.as_deref()) {
            msg.timestamp = ts;
        }
        messages.push(msg);
    }

    Ok(Chat {
        id,
        profile_id,
        title: c.title.clone(),
        created_at: created.unwrap_or_else(epoch),
        modified_at: modified.unwrap_or_else(epoch),
        system_message: explicit_system.or(first_system).unwrap_or_default(),
        character_names: map_characters(c.character_names.as_ref()),
        messages,
        sampling_override: None,
        draft: String::new(),
        // View state isn't part of the exchange format either — an imported chat
        // opens with everything collapsed, like a new one.
        feed_view: FeedView::default(),
        // The import format (v1) carries no attachments — see docs/import-format.md.
        attachments: Vec::new(),
        deleted: Vec::new(),
        reflected_upto: None,
        reflected_at: None,
        is_hidden: false,
    })
}

fn map_characters(c: Option<&ImCharacters>) -> CharacterNames {
    let def = CharacterNames::default();
    match c {
        Some(c) => CharacterNames {
            user: c.user.clone().unwrap_or(def.user),
            assistant: c.assistant.clone().unwrap_or(def.assistant),
            system: c.system.clone().unwrap_or(def.system),
        },
        None => def,
    }
}

fn map_interface(i: ImInterface) -> ImportedInterface {
    ImportedInterface {
        spellcheck_enabled: i.spellcheck_enabled,
        dictionaries: i.dictionaries,
        theme: i.theme.as_deref().map(parse_theme),
    }
}

fn parse_theme(s: &str) -> Theme {
    match s.to_ascii_lowercase().as_str() {
        "dark" => Theme::Dark,
        "light" => Theme::Light,
        _ => Theme::Auto,
    }
}

/// Timestamps — RFC 3339; unreadable ones aren't treated as an error (see the format §Semantics).
fn parse_time(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::locale;

    /// The reference locale for tests (ru — byte-for-byte with the bundle).
    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    /// A golden fixture of format v1 (mirroring docs/import-format.md; unknown
    /// fields are deliberately present — tolerance for extension).
    const FULL: &str = r#"{
      "format": "mindfork-import",
      "version": 1,
      "profiles": [
        {
          "key": "anna",
          "name": "Anna",
          "language": "en",
          "system_message": "You are Anna.",
          "greeting": "Hello!",
          "character_names": { "user": "Гайя", "assistant": "Анна" },
          "sampling": { "temperature": 0.8, "top_k": 64, "unknown_knob": 1 },
          "unknown_field": true
        }
      ],
      "chats": [
        {
          "key": "c-1",
          "profile_key": "anna",
          "title": "First",
          "created_at": "2026-06-14T23:47:46+03:00",
          "modified_at": "2026-06-14T20:51:05Z",
          "messages": [
            { "role": "System", "text": "sys from msg" },
            { "role": "user", "text": "Hi", "timestamp": "2026-06-14T20:00:00Z" },
            { "role": "Assistant", "text": "Hello", "thoughts": "thinking" }
          ]
        }
      ],
      "settings": {
        "sampling": { "temperature": 0.5 },
        "interface": { "theme": "dark" }
      }
    }"#;

    #[test]
    fn parses_full_fixture() {
        let r = parse_import(FULL, ru()).unwrap();
        assert_eq!(r.profiles.len(), 1);
        let p = &r.profiles[0];
        assert_eq!(p.name, "Anna");
        assert_eq!(p.language, Lang::En);
        assert_eq!(p.default_system_message, "You are Anna.");
        assert_eq!(p.greeting.as_deref(), Some("Hello!"));
        assert_eq!(p.character_names.user, "Гайя");
        assert_eq!(p.character_names.system, CharacterNames::default().system);
        assert!(!p.enabled_tools.is_empty());
        // Sampling: known fields are carried over, unknown ones are ignored.
        let s = p.default_sampling.as_ref().unwrap();
        assert_eq!(s.temperature, Some(0.8));
        assert_eq!(s.top_k, Some(64));

        assert_eq!(r.chats.len(), 1);
        let c = &r.chats[0];
        assert_eq!(c.profile_id, p.id);
        assert_eq!(c.title, "First");
        // system from the history (no explicit field); didn't end up in the messages.
        assert_eq!(c.system_message, "sys from msg");
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].role, MessageRole::User);
        assert_eq!(
            c.messages[0].timestamp.to_rfc3339(),
            "2026-06-14T20:00:00+00:00"
        );
        assert_eq!(c.messages[1].role, MessageRole::Assistant);
        assert_eq!(c.messages[1].thoughts.as_deref(), Some("thinking"));
        assert!(c.created_at.timestamp() > 0);

        // Settings: a partial transfer (only what's set).
        assert_eq!(r.sampling.unwrap().temperature, Some(0.5));
        let i = r.interface.unwrap();
        assert_eq!(i.theme, Some(Theme::Dark));
        assert_eq!(i.spellcheck_enabled, None);
        assert_eq!(i.dictionaries, None);
    }

    #[test]
    fn ids_deterministic_and_namespaced() {
        // The same key for a profile and a chat → DIFFERENT ids (separate namespaces);
        // a repeat parse → the same ids (idempotency).
        let json = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "x", "name": "X" }],
          "chats": [{ "key": "x", "profile_key": "x" }] }"#;
        let a = parse_import(json, ru()).unwrap();
        let b = parse_import(json, ru()).unwrap();
        assert_eq!(a.profiles[0].id, b.profiles[0].id);
        assert_eq!(a.chats[0].id, b.chats[0].id);
        assert_ne!(a.profiles[0].id, a.chats[0].id);
    }

    #[test]
    fn explicit_id_overrides_derived() {
        let json = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "p", "name": "P",
                         "id": "663184f6-c49d-4da6-a0f3-0e757f7e9f3c" }] }"#;
        let r = parse_import(json, ru()).unwrap();
        assert_eq!(
            r.profiles[0].id.to_string(),
            "663184f6-c49d-4da6-a0f3-0e757f7e9f3c"
        );
    }

    #[test]
    fn explicit_system_message_wins_over_history() {
        let json = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "p", "name": "P" }],
          "chats": [{ "key": "c", "profile_key": "p",
                      "system_message": "explicit",
                      "messages": [{ "role": "system", "text": "from history" }] }] }"#;
        let r = parse_import(json, ru()).unwrap();
        assert_eq!(r.chats[0].system_message, "explicit");
        assert!(r.chats[0].messages.is_empty());
    }

    #[test]
    fn rejects_wrong_format_localized() {
        let en = parse_import(r#"{ "format": "other", "version": 1 }"#, locale(Lang::En))
            .unwrap_err()
            .to_string();
        assert!(en.contains("mindfork-import"), "{en}");
        assert!(
            !en.chars()
                .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
            "{en}"
        );
        let no_field = parse_import(r#"{ "version": 1 }"#, ru())
            .unwrap_err()
            .to_string();
        assert!(no_field.contains("mindfork-import"), "{no_field}");
    }

    #[test]
    fn rejects_newer_or_missing_version() {
        let newer = parse_import(r#"{ "format": "mindfork-import", "version": 2 }"#, ru())
            .unwrap_err()
            .to_string();
        assert!(newer.contains('2'), "{newer}");
        assert!(
            parse_import(r#"{ "format": "mindfork-import" }"#, ru()).is_err(),
            "version is required"
        );
        assert!(parse_import(r#"{ "format": "mindfork-import", "version": 0 }"#, ru()).is_err());
    }

    #[test]
    fn rejects_duplicate_and_empty_keys() {
        let dup_p = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "dup-key", "name": "A" }, { "key": "dup-key", "name": "B" }] }"#;
        let err = parse_import(dup_p, ru()).unwrap_err().to_string();
        assert!(err.contains("dup-key"), "{err}");
        let dup_c = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "p", "name": "P" }],
          "chats": [{ "key": "c", "profile_key": "p" }, { "key": "c", "profile_key": "p" }] }"#;
        assert!(parse_import(dup_c, ru()).is_err());
        let empty = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "name": "A" }] }"#;
        assert!(parse_import(empty, ru()).is_err());
    }

    #[test]
    fn rejects_unknown_profile_key() {
        let json = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "p", "name": "P" }],
          "chats": [{ "key": "chat-7", "profile_key": "nope" }] }"#;
        let err = parse_import(json, ru()).unwrap_err().to_string();
        assert!(err.contains("nope") && err.contains("chat-7"), "{err}");
    }

    #[test]
    fn rejects_unknown_role_with_location() {
        let json = r#"{ "format": "mindfork-import", "version": 1,
          "profiles": [{ "key": "p", "name": "P" }],
          "chats": [{ "key": "c-9", "profile_key": "p",
                      "messages": [{ "role": "tool", "text": "x" }] }] }"#;
        let err = parse_import(json, ru()).unwrap_err().to_string();
        assert!(err.contains("c-9") && err.contains("tool"), "{err}");
    }

    #[test]
    fn import_file_tolerates_bom_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        // A name without the bundle keys' prefix: the i18n gate scans dotted literals.
        let path = dir.path().join("import-file.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(FULL.as_bytes());
        fs::write(&path, &bytes).unwrap();
        let r = import_file(&path, ru()).unwrap();
        assert_eq!(r.profiles.len(), 1);

        let missing = import_file(&dir.path().join("nope.json"), ru());
        assert!(missing.is_err());
    }
}
