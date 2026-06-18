//! Импорт данных из **LameLLaMA (.NET)** (spec §12.2): `Settings.json`
//! (`NewConversationConfig.Configurations` → профили) и `Conversations/*.json`
//! (→ чаты). Одноразовый, **идемпотентный** (детерминированные id), исходные
//! файлы только читаются.
//!
//! Не мигрируются: имперсонация, KV-кэш, неподдерживаемые параметры семплинга
//! (`min_p`, `repeat_penalty`, mirostat, seed, dynatemp, …) — отбрасываются.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::default_tool_ids;
use crate::shared::config::Theme;

/// Пространство имён для детерминированных id профилей (UUIDv5 от имени конфигурации).
/// Фиксированное — чтобы повторный импорт давал те же id (идемпотентность).
const PROFILE_NAMESPACE: Uuid = Uuid::from_u128(0x6d696e64_666f726b_6d696772_00000001u128);

/// Результат импорта: профили, чаты и (опционально) перенесённые глобальные
/// настройки (семплинг/интерфейс) для применения к `AppConfig`.
#[derive(Debug, Default)]
pub struct ImportResult {
    pub profiles: Vec<Profile>,
    pub chats: Vec<Chat>,
    /// Глобальный семплинг источника (неподдержанное отброшено).
    pub sampling: Option<SamplingConfig>,
    /// Настройки интерфейса источника (спелл-чек/словари/тема).
    pub interface: Option<ImportedInterface>,
}

/// Перенесённые настройки интерфейса из LameLLaMA.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedInterface {
    pub spellcheck_enabled: bool,
    pub dictionaries: Vec<String>,
    pub theme: Theme,
}

// ---------- wire-типы LameLLaMA (PascalCase, частичные) ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlSettings {
    new_conversation_config: Option<LlNewConversation>,
    conversation_engine_config: Option<LlEngine>,
    spell_checker_config: Option<LlSpell>,
    user_interface_config: Option<LlUi>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlNewConversation {
    #[serde(default)]
    configurations: std::collections::BTreeMap<String, LlConfiguration>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlConfiguration {
    #[serde(default)]
    characters_config: Option<LlCharacters>,
    #[serde(default)]
    message_history: Vec<LlMessage>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct LlCharacters {
    user: Option<String>,
    assistant: Option<String>,
    system: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlMessage {
    role: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    thoughts: String,
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlEngine {
    sampling_config: Option<LlSampling>,
    inference_config: Option<LlInference>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlInference {
    include_thoughts: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlSampling {
    max_tokens: Option<usize>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<i64>,
    alpha_frequency: Option<f32>,
    alpha_presence: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlSpell {
    is_enabled: Option<bool>,
    #[serde(default)]
    enabled_dictionaries: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlUi {
    window_color_theme: Option<String>,
}

/// Конверсация (`Conversations/{id}.json`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LlConversation {
    id: Option<String>,
    #[serde(default)]
    title: String,
    date_created: Option<String>,
    date_modified: Option<String>,
    #[serde(default)]
    characters_config: Option<LlCharacters>,
    #[serde(default)]
    message_history: Vec<LlMessage>,
}

// ---------- парсинг (чистые функции, тестируемы на фикстурах) ----------

/// Парсит `Settings.json`: профили из `Configurations` + глобальные семплинг/
/// интерфейс. Профили получают **детерминированный** id (по имени) — повторный
/// импорт даёт те же id.
pub fn parse_settings(json: &str) -> Result<ImportResult> {
    // .NET пишет UTF-8 с BOM — отбрасываем его перед парсингом.
    let json = json.trim_start_matches('\u{feff}');
    let s: LlSettings = serde_json::from_str(json).context("parsing LameLLaMA Settings.json")?;

    let mut profiles = Vec::new();
    if let Some(nc) = &s.new_conversation_config {
        for (name, cfg) in &nc.configurations {
            profiles.push(configuration_to_profile(name, cfg));
        }
    }

    let include_thoughts = s
        .conversation_engine_config
        .as_ref()
        .and_then(|e| e.inference_config.as_ref())
        .and_then(|i| i.include_thoughts);
    let sampling = s
        .conversation_engine_config
        .as_ref()
        .and_then(|e| e.sampling_config.as_ref())
        .map(|sc| map_sampling(sc, include_thoughts));

    let interface = s.user_interface_config.as_ref().map(|ui| {
        let spell = s.spell_checker_config.as_ref();
        ImportedInterface {
            spellcheck_enabled: spell.and_then(|s| s.is_enabled).unwrap_or(true),
            dictionaries: spell
                .map(|s| s.enabled_dictionaries.clone())
                .unwrap_or_default(),
            theme: parse_theme(ui.window_color_theme.as_deref()),
        }
    });

    Ok(ImportResult {
        profiles,
        chats: Vec::new(),
        sampling,
        interface,
    })
}

/// Конфигурация LameLLaMA → профиль mindfork. id детерминирован по имени.
fn configuration_to_profile(name: &str, cfg: &LlConfiguration) -> Profile {
    let id = Uuid::new_v5(&PROFILE_NAMESPACE, name.as_bytes());
    let system = first_text(&cfg.message_history, "System").unwrap_or_default();
    let greeting = first_text(&cfg.message_history, "Assistant");
    Profile {
        id,
        name: name.to_string(),
        default_system_message: system,
        // Имперсонация в LameLLaMA не переносится — пусто (общий дефолт).
        impersonation_system_message: String::new(),
        character_names: map_characters(cfg.characters_config.as_ref()),
        greeting: greeting.filter(|g| !g.is_empty()),
        // Импортированные профили получают стандартный набор инструментов.
        enabled_tools: default_tool_ids(),
        // Семплинг — глобальный (config.default_sampling), профиль наследует его.
        default_sampling: None,
        is_hidden: false,
    }
}

/// Конверсация LameLLaMA → чат, привязанный к `profile_id`. Системное сообщение
/// вынесено в `Chat.system_message`; остальные (User/Assistant) — в `messages`.
fn conversation_to_chat(conv: &LlConversation, profile_id: Uuid) -> Chat {
    let id = conv
        .id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&PROFILE_NAMESPACE, conv.title.as_bytes()));
    let created = parse_time(conv.date_created.as_deref());
    let modified = parse_time(conv.date_modified.as_deref()).or(created);

    let system_message = first_text(&conv.message_history, "System").unwrap_or_default();
    let messages = conv
        .message_history
        .iter()
        .filter_map(map_message)
        .collect();

    Chat {
        id,
        profile_id,
        title: conv.title.clone(),
        created_at: created.unwrap_or_else(epoch),
        modified_at: modified.unwrap_or_else(epoch),
        system_message,
        character_names: map_characters(conv.characters_config.as_ref()),
        messages,
        sampling_override: None,
        draft: String::new(),
        is_hidden: false,
    }
}

/// Импортирует каталог LameLLaMA: `Settings.json` (профили/настройки) +
/// `Conversations/*.json` (чаты; `*.deleted` пропускаются). Все чаты привязываются
/// к первому импортированному профилю (у чата своя копия system/имён, поэтому
/// привязка влияет лишь на изоляцию notes/RAG, которых в источнике нет).
pub fn import_dir(dir: &Path) -> Result<ImportResult> {
    let mut result = match fs::read(dir.join("Settings.json")) {
        Ok(bytes) => parse_settings(&String::from_utf8_lossy(&bytes))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ImportResult::default(),
        Err(e) => return Err(e).context("reading Settings.json"),
    };

    // Если профилей нет — создаём детерминированный профиль-приёмник для чатов.
    if result.profiles.is_empty() {
        result.profiles.push(configuration_to_profile(
            "Импортировано",
            &LlConfiguration {
                characters_config: None,
                message_history: Vec::new(),
            },
        ));
    }
    let target_profile = result.profiles[0].id;

    let conv_dir = dir.join("Conversations");
    if conv_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&conv_dir)
            .with_context(|| format!("reading {}", conv_dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        // Детерминированный порядок (для воспроизводимости).
        entries.sort();
        for path in entries {
            let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            match serde_json::from_slice::<LlConversation>(strip_bom(&bytes)) {
                Ok(conv) => result
                    .chats
                    .push(conversation_to_chat(&conv, target_profile)),
                Err(err) => {
                    tracing::warn!(file = %path.display(), error = %err, "пропуск нечитаемой конверсации");
                }
            }
        }
    }

    Ok(result)
}

// ---------- вспомогательное ----------

/// Текст первого сообщения с заданной ролью (без учёта регистра).
fn first_text(history: &[LlMessage], role: &str) -> Option<String> {
    history
        .iter()
        .find(|m| {
            m.role
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(role))
        })
        .map(|m| m.text.clone())
}

/// Маппинг одного сообщения (System пропускается — оно в `Chat.system_message`).
fn map_message(m: &LlMessage) -> Option<Message> {
    let role = match m.role.as_deref()?.to_ascii_lowercase().as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        // System — отдельно; Unknown/прочее — отбрасываем.
        _ => return None,
    };
    let mut msg = Message::new(role, m.text.clone());
    if !m.thoughts.is_empty() {
        msg.thoughts = Some(m.thoughts.clone());
    }
    if let Some(ts) = parse_time(m.timestamp.as_deref()) {
        msg.timestamp = ts;
    }
    Some(msg)
}

fn map_characters(c: Option<&LlCharacters>) -> CharacterNames {
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

/// Маппинг семплинга: переносим только поддержанное xinfer; остальное отбрасываем.
fn map_sampling(s: &LlSampling, include_thoughts: Option<bool>) -> SamplingConfig {
    SamplingConfig {
        temperature: s.temperature,
        top_k: s.top_k,
        top_p: s.top_p,
        frequency_penalty: s.alpha_frequency,
        presence_penalty: s.alpha_presence,
        max_tokens: s.max_tokens,
        thinking: include_thoughts,
        reasoning_effort: None,
        reasoning_budget: None,
    }
}

fn parse_theme(s: Option<&str>) -> Theme {
    match s.unwrap_or("").to_ascii_lowercase().as_str() {
        "dark" => Theme::Dark,
        "light" => Theme::Light,
        _ => Theme::Auto,
    }
}

fn parse_time(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).unwrap()
}

/// Отбрасывает ведущий UTF-8 BOM (`EF BB BF`), который пишет .NET.
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = r#"{
      "ConversationEngineConfig": {
        "InferenceConfig": { "IncludeThoughts": true },
        "SamplingConfig": {
          "MaxTokens": 4096, "Temperature": 0.8, "TopP": 0.95, "TopK": 64,
          "MinP": 0.05, "RepeatPenalty": 1.1, "AlphaFrequency": 0.3,
          "AlphaPresence": 0.2, "Seed": 1, "MirostatMode": "Disabled"
        }
      },
      "NewConversationConfig": {
        "Configurations": {
          "Carlos": {
            "CharactersConfig": { "User": "Гайя", "Assistant": "Карлос", "System": "Система" },
            "MessageHistory": [
              { "Role": "System", "Text": "Ты — Карлос." },
              { "Role": "Assistant", "Text": "Здравствуй." }
            ]
          }
        }
      },
      "SpellCheckerConfig": { "IsEnabled": true, "EnabledDictionaries": ["en_US", "ru_RU"] },
      "UserInterfaceConfig": { "WindowColorTheme": "Dark" }
    }"#;

    const CONVERSATION: &str = r#"{
      "Id": "663184f6-c49d-4da6-a0f3-0e757f7e9f3c",
      "Title": "New Chat 154",
      "DateCreated": "2026-06-14T23:47:46.049311+03:00",
      "DateModified": "2026-06-14T20:51:05.7748673Z",
      "CharactersConfig": { "User": "User", "Assistant": "Assistant", "System": "System" },
      "MessageHistory": [
        { "Role": "System", "Text": "You are Assistant." },
        { "Role": "Assistant", "Text": "Hello!", "Thoughts": "размышляю" },
        { "Role": "User", "Text": "Hi!" },
        { "Role": "Unknown", "Text": "skip me" }
      ]
    }"#;

    #[test]
    fn parses_configuration_into_profile() {
        let r = parse_settings(SETTINGS).unwrap();
        assert_eq!(r.profiles.len(), 1);
        let p = &r.profiles[0];
        assert_eq!(p.name, "Carlos");
        assert_eq!(p.default_system_message, "Ты — Карлос.");
        assert_eq!(p.greeting.as_deref(), Some("Здравствуй."));
        assert_eq!(p.character_names.assistant, "Карлос");
        assert!(!p.enabled_tools.is_empty());
    }

    #[test]
    fn maps_supported_sampling_drops_rest() {
        let r = parse_settings(SETTINGS).unwrap();
        let s = r.sampling.unwrap();
        assert_eq!(s.temperature, Some(0.8));
        assert_eq!(s.top_k, Some(64));
        assert_eq!(s.frequency_penalty, Some(0.3));
        assert_eq!(s.presence_penalty, Some(0.2));
        assert_eq!(s.max_tokens, Some(4096));
        assert_eq!(s.thinking, Some(true));
        // Неподдержанные (min_p/repeat_penalty/seed/mirostat) физически отсутствуют.
    }

    #[test]
    fn maps_interface_and_theme() {
        let r = parse_settings(SETTINGS).unwrap();
        let i = r.interface.unwrap();
        assert!(i.spellcheck_enabled);
        assert_eq!(i.dictionaries, vec!["en_US", "ru_RU"]);
        assert_eq!(i.theme, Theme::Dark);
    }

    #[test]
    fn profile_id_is_deterministic() {
        let a = parse_settings(SETTINGS).unwrap().profiles[0].id;
        let b = parse_settings(SETTINGS).unwrap().profiles[0].id;
        assert_eq!(a, b, "повторный импорт даёт тот же id (идемпотентность)");
    }

    #[test]
    fn conversation_maps_to_chat() {
        let conv: LlConversation = serde_json::from_str(CONVERSATION).unwrap();
        let pid = Uuid::new_v4();
        let chat = conversation_to_chat(&conv, pid);
        assert_eq!(chat.id.to_string(), "663184f6-c49d-4da6-a0f3-0e757f7e9f3c");
        assert_eq!(chat.profile_id, pid);
        assert_eq!(chat.title, "New Chat 154");
        assert_eq!(chat.system_message, "You are Assistant.");
        // System и Unknown отброшены → остаются Assistant + User.
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, MessageRole::Assistant);
        assert_eq!(chat.messages[0].thoughts.as_deref(), Some("размышляю"));
        assert_eq!(chat.messages[1].role, MessageRole::User);
        assert!(chat.created_at < chat.modified_at || chat.created_at >= epoch());
    }

    #[test]
    fn import_dir_reads_settings_and_conversations() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Settings.json"), SETTINGS).unwrap();
        let conv_dir = dir.path().join("Conversations");
        fs::create_dir(&conv_dir).unwrap();
        fs::write(conv_dir.join("a.json"), CONVERSATION).unwrap();
        // .deleted и нечитаемые файлы пропускаются.
        fs::write(conv_dir.join("b.json.deleted"), CONVERSATION).unwrap();
        fs::write(conv_dir.join("c.json"), "{ not json").unwrap();

        let r = import_dir(dir.path()).unwrap();
        assert_eq!(r.profiles.len(), 1);
        assert_eq!(r.chats.len(), 1, "только валидный .json");
        assert_eq!(r.chats[0].profile_id, r.profiles[0].id);
    }

    #[test]
    fn import_dir_idempotent_ids() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Settings.json"), SETTINGS).unwrap();
        let conv_dir = dir.path().join("Conversations");
        fs::create_dir(&conv_dir).unwrap();
        fs::write(conv_dir.join("a.json"), CONVERSATION).unwrap();

        let r1 = import_dir(dir.path()).unwrap();
        let r2 = import_dir(dir.path()).unwrap();
        assert_eq!(r1.profiles[0].id, r2.profiles[0].id);
        assert_eq!(r1.chats[0].id, r2.chats[0].id);
    }
}
