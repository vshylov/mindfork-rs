//! Character cards — the persona exchange format of the community's character
//! apps (spec §10, docs/research/companion-platform.md §4.5 and C6): read from a
//! JSON file (V1, V2 `chara_card_v2`, V3 `chara_card_v3`), from a PNG carrying
//! the card in a text chunk (`ccv3`, else `chara`), or from a CHARX zip
//! (`card.json`), and mapped onto a new profile ([`profile`]).
//!
//! A card is untrusted text. It is read and nothing in it is run, rendered or
//! fetched: no HTML, no scripts, no asset URLs — a CHARX's assets are not even
//! opened. Lenient about the field types the wild writes (a `null` where a
//! string belongs reads as empty), strict about the shape: a file that is not a
//! card says so.

mod png;
pub mod profile;

use std::io::{Cursor, Read};

use base64::Engine;
use serde_json::{Map, Value};

use crate::shared::i18n::Locale;

/// The largest card file read whole. A card is text and a picture; one past
/// this is not a card anyone made by hand.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The most `card.json` inside a CHARX may hold.
const MAX_CHARX_JSON: u64 = 16 * 1024 * 1024;

/// Which edition of the format a card was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spec {
    /// The flat, unversioned original (TavernAI, Pygmalion's field names).
    V1,
    /// `chara_card_v2`.
    V2,
    /// `chara_card_v3`, or a later `chara_card_v*` read as one.
    V3,
}

impl Spec {
    pub fn label(self) -> &'static str {
        match self {
            Spec::V1 => "V1",
            Spec::V2 => "V2",
            Spec::V3 => "V3",
        }
    }
}

/// Where the card was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Json,
    /// A PNG's text chunk, by keyword (`ccv3` or `chara`).
    Png(&'static str),
    Charx,
}

/// One entry of a card's lorebook (`character_book`), as much of it as the
/// import uses: an always-on entry joins the system message, a keyed one is
/// counted and left out (F7 (a): keyed lorebooks deferred).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookEntry {
    pub content: String,
    /// On whatever the conversation says: `constant`, or an `@@activate`
    /// decorator. Disabled entries are not kept at all.
    pub always: bool,
    pub order: i64,
}

/// A character card, every edition read onto one shape. Empty strings are
/// fields the card left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub spec: Spec,
    pub container: Container,
    pub name: String,
    /// V3: what `{{char}}` stands for when set.
    pub nickname: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    /// The character's first message.
    pub first_mes: String,
    /// Example exchanges, `<START>`-separated.
    pub mes_example: String,
    /// The card's own system prompt; `{{original}}` inside stands for what it
    /// replaces.
    pub system_prompt: String,
    /// Text the card means to sit after the conversation.
    pub post_history_instructions: String,
    pub alternate_greetings: Vec<String>,
    /// For the person, never for the model.
    pub creator_notes: String,
    /// Enabled lorebook entries, in their insertion order.
    pub book: Vec<BookEntry>,
}

impl Card {
    /// What `{{char}}` names: the nickname when the card sets one, else the name.
    pub fn char_name(&self) -> &str {
        if self.nickname.trim().is_empty() {
            self.name.trim()
        } else {
            self.nickname.trim()
        }
    }
}

/// What a file holds, as far as `mindfork import` is concerned.
pub enum Sniffed {
    /// A character card.
    Card(Box<Card>),
    /// Not a card: the caller tries the neutral import format.
    NotCard,
}

/// Reads a file's bytes as a card. A PNG or a zip is a card or an error; JSON
/// that looks like neither edition is [`Sniffed::NotCard`], so the
/// mindfork-import reader can have it.
pub fn sniff(bytes: &[u8], loc: &Locale) -> anyhow::Result<Sniffed> {
    if bytes.starts_with(png::SIGNATURE) {
        return read_png(bytes, loc).map(|c| Sniffed::Card(Box::new(c)));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return read_charx(bytes, loc).map(|c| Sniffed::Card(Box::new(c)));
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Ok(Sniffed::NotCard);
    };
    match parse_value(&value, Container::Json, loc) {
        Ok(Some(card)) => Ok(Sniffed::Card(Box::new(card))),
        Ok(None) => Ok(Sniffed::NotCard),
        Err(e) => Err(e),
    }
}

fn read_png(bytes: &[u8], loc: &Locale) -> anyhow::Result<Card> {
    let chunks = match png::card_chunks(bytes) {
        Ok(c) => c,
        Err(png::PngError::NoCard) => anyhow::bail!("{}", loc.t("import.card.png_no_card")),
        Err(png::PngError::Truncated) => anyhow::bail!("{}", loc.t("import.card.png_truncated")),
    };
    // `ccv3` first, as the V3 spec asks; a `ccv3` that does not read falls back
    // to `chara` rather than failing a card whose V2 copy is whole.
    let mut last_err = None;
    for keyword in png::KEYWORDS {
        for (_, text) in chunks.iter().filter(|(k, _)| k == keyword) {
            match chunk_json(text)
                .ok_or_else(|| anyhow::anyhow!("{}", loc.t("import.card.png_unreadable")))
                .and_then(|v| parse_value(&v, Container::Png(keyword), loc))
            {
                Ok(Some(card)) => return Ok(card),
                Ok(None) => last_err = Some(anyhow::anyhow!("{}", loc.t("import.card.not_card"))),
                Err(e) => last_err = Some(e),
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{}", loc.t("import.card.png_unreadable"))))
}

/// A chunk's text as JSON: base64 by the specs, and raw JSON from a few
/// writers that skip the encoding.
fn chunk_json(text: &[u8]) -> Option<Value> {
    let compact: Vec<u8> = text
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&compact))
        .unwrap_or_else(|_| text.to_vec());
    let s = String::from_utf8_lossy(&decoded);
    serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
}

fn read_charx(bytes: &[u8], loc: &Locale) -> anyhow::Result<Card> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| anyhow::anyhow!("{}", loc.t("import.card.zip_unreadable")))?;
    let has = |zip: &zip::ZipArchive<Cursor<&[u8]>>, name: &str| zip.index_for_name(name).is_some();
    if !has(&zip, "card.json") {
        // A companion archive or a backup is a zip too, and each has its own
        // command; a person who reached for `import` is pointed at it.
        let key = if has(&zip, "companion.json") {
            "import.card.zip_is_companion"
        } else if has(&zip, "manifest.json") {
            "import.card.zip_is_backup"
        } else {
            "import.card.zip_no_card"
        };
        anyhow::bail!("{}", loc.t(key));
    }
    let mut entry = zip
        .by_name("card.json")
        .map_err(|_| anyhow::anyhow!("{}", loc.t("import.card.zip_unreadable")))?;
    let mut raw = Vec::new();
    entry
        .by_ref()
        .take(MAX_CHARX_JSON)
        .read_to_end(&mut raw)
        .map_err(|_| anyhow::anyhow!("{}", loc.t("import.card.zip_unreadable")))?;
    let text = String::from_utf8_lossy(&raw);
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| anyhow::anyhow!("{}", loc.t("import.card.not_card")))?;
    parse_value(&value, Container::Charx, loc)?
        .ok_or_else(|| anyhow::anyhow!("{}", loc.t("import.card.not_card")))
}

/// A JSON value as a card: `Ok(None)` when it is not one at all, an error when
/// it says it is one and is not readable as one.
fn parse_value(value: &Value, container: Container, loc: &Locale) -> anyhow::Result<Option<Card>> {
    let Some(top) = value.as_object() else {
        return Ok(None);
    };
    if let Some(spec) = top.get("spec").and_then(Value::as_str) {
        let spec = match spec {
            "chara_card_v2" => Spec::V2,
            s if s.starts_with("chara_card_v") => Spec::V3,
            // Another family's `spec` (a standalone lorebook, say).
            _ => return Ok(None),
        };
        let Some(data) = top.get("data").and_then(Value::as_object) else {
            anyhow::bail!("{}", loc.t("import.card.no_data"));
        };
        return finish(read_fields(data, spec, container), loc).map(Some);
    }
    // V1 is flat and unlabelled: a name and at least one field that only a card has.
    let is_v1 = (has_text(top, "name") || has_text(top, "char_name"))
        && [
            "description",
            "personality",
            "first_mes",
            "mes_example",
            "char_persona",
            "char_greeting",
            "example_dialogue",
        ]
        .iter()
        .any(|k| top.contains_key(*k));
    if !is_v1 {
        return Ok(None);
    }
    finish(read_fields(top, Spec::V1, container), loc).map(Some)
}

fn finish(card: Card, loc: &Locale) -> anyhow::Result<Card> {
    if card.name.trim().is_empty() {
        anyhow::bail!("{}", loc.t("import.card.no_name"));
    }
    Ok(card)
}

fn read_fields(data: &Map<String, Value>, spec: Spec, container: Container) -> Card {
    // V1's Pygmalion spellings stand in for the fields they predate.
    let field = |keys: &[&str]| -> String {
        keys.iter()
            .map(|k| text(data, k))
            .find(|s| !s.trim().is_empty())
            .unwrap_or_default()
    };
    let mut card = Card {
        spec,
        container,
        name: field(&["name", "char_name"]),
        nickname: text(data, "nickname"),
        description: field(&["description", "char_persona"]),
        personality: text(data, "personality"),
        scenario: field(&["scenario", "world_scenario"]),
        first_mes: field(&["first_mes", "char_greeting"]),
        mes_example: field(&["mes_example", "example_dialogue"]),
        system_prompt: text(data, "system_prompt"),
        post_history_instructions: text(data, "post_history_instructions"),
        alternate_greetings: texts(data, "alternate_greetings"),
        creator_notes: text(data, "creator_notes"),
        book: data
            .get("character_book")
            .and_then(Value::as_object)
            .map(read_book)
            .unwrap_or_default(),
    };
    if spec == Spec::V1 {
        // V1's own placeholders, rewritten into the macros every later edition
        // uses, so one substitution serves every card.
        for s in [
            &mut card.description,
            &mut card.personality,
            &mut card.scenario,
            &mut card.first_mes,
            &mut card.mes_example,
        ] {
            *s = v1_placeholders(s);
        }
    }
    card
}

fn read_book(book: &Map<String, Value>) -> Vec<BookEntry> {
    let Some(entries) = book.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<BookEntry> = entries
        .iter()
        .filter_map(Value::as_object)
        .filter(|e| e.get("enabled").and_then(Value::as_bool) != Some(false))
        .filter_map(|e| {
            let (decorators, content) = split_decorators(&text(e, "content"));
            if decorators.iter().any(|d| d == "dont_activate") || content.trim().is_empty() {
                return None;
            }
            // `constant` is honoured whatever `use_regex` says: the V3 text says to
            // ignore it then, but SillyTavern — the largest writer — exports every
            // entry with `use_regex: true` and means the flag (§4.5).
            let always = e.get("constant").and_then(Value::as_bool) == Some(true)
                || decorators.iter().any(|d| d == "activate");
            let order = e
                .get("insertion_order")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            Some(BookEntry {
                content,
                always,
                order,
            })
        })
        .collect();
    out.sort_by_key(|e| e.order);
    out
}

/// A V3 lorebook entry's leading `@@decorator` lines, by name, and the content
/// after them.
fn split_decorators(content: &str) -> (Vec<String>, String) {
    let mut names = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.peek() {
        let Some(rest) = line.trim_start().strip_prefix("@@") else {
            break;
        };
        // `@@@` is a fallback for the decorator before it — not a name of its own.
        if !rest.starts_with('@') {
            let name = rest.split_whitespace().next().unwrap_or_default();
            names.push(name.to_ascii_lowercase());
        }
        lines.next();
    }
    let body: Vec<&str> = lines.collect();
    (names, body.join("\n").trim().to_string())
}

/// `<BOT>` and `<USER>` (any case) as `{{char}}` and `{{user}}`.
fn v1_placeholders(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let swap = [("<bot>", "{{char}}"), ("<user>", "{{user}}")]
            .into_iter()
            .find(|(from, _)| {
                tail.get(..from.len())
                    .is_some_and(|t| t.eq_ignore_ascii_case(from))
            });
        match swap {
            Some((from, to)) => {
                out.push_str(to);
                rest = &tail[from.len()..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn has_text(m: &Map<String, Value>, key: &str) -> bool {
    !text(m, key).trim().is_empty()
}

/// A field as text: a string as it is, a number or a bool written out, anything
/// else (absent, `null`, an object) as empty.
fn text(m: &Map<String, Value>, key: &str) -> String {
    match m.get(key) {
        Some(Value::String(s)) => s.replace("\r\n", "\n"),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// A list of strings, the non-string and blank items left out.
fn texts(m: &Map<String, Value>, key: &str) -> Vec<String> {
    m.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.replace("\r\n", "\n"))
                .collect()
        })
        .unwrap_or_default()
}

/// A minimal PNG carrying `json` as a card under `keyword` — for tests outside
/// this module (the live smoke builds the file a person would bring).
#[cfg(test)]
pub(crate) fn png_with(keyword: &str, json: &str) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    png::fixture(&[(b"tEXt", png::text(keyword, encoded.as_bytes()))])
}

#[cfg(test)]
mod tests;
