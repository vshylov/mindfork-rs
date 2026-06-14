//! Глобальная конфигурация приложения (`settings.json`). См. spec §12.1.
//! Версионируется полем `schema_version` для будущих миграций.

use serde::{Deserialize, Serialize};

use crate::entities::sampling::SamplingConfig;

/// Текущая версия схемы конфигурации.
pub const SCHEMA_VERSION: u32 = 1;

/// Режим подключения к серверу инференса. См. docs/xinfer-contract.md §1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    /// Приложение само запускает дочерний процесс xinfer.
    #[default]
    Managed,
    /// Подключение к уже запущенному серверу.
    External,
}

/// Настройки сервера/модели xinfer (расширяются на M8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct XinferSettings {
    pub mode: ServerMode,
    /// URL для external-режима (например `http://127.0.0.1:8000/v1`).
    pub url: Option<String>,
    /// Путь к бинарнику xinfer для managed-режима.
    pub binary: Option<String>,
    pub model_id: Option<String>,
    pub weight_path: Option<String>,
    pub weight_file: Option<String>,
    pub isq: Option<String>,
    pub device_ids: Vec<usize>,
    pub cpu: bool,
    pub port: u16,
}

impl Default for XinferSettings {
    fn default() -> Self {
        Self {
            mode: ServerMode::Managed,
            url: None,
            binary: None,
            model_id: None,
            weight_path: None,
            weight_file: None,
            isq: None,
            device_ids: vec![0],
            cpu: false,
            port: 8000,
        }
    }
}

/// Настройки выделенного embedding-сервера для RAG (ADR 0002). Отдельный
/// процесс/порт; если не настроен (`UnavailableEmbedder`) — RAG отдаёт ошибку.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub mode: ServerMode,
    /// URL для external-режима (например `http://127.0.0.1:8001/v1`).
    pub url: Option<String>,
    /// Путь к бинарнику xinfer для managed-режима.
    pub binary: Option<String>,
    pub model_id: Option<String>,
    pub port: u16,
}

impl Default for EmbedSettings {
    fn default() -> Self {
        Self {
            mode: ServerMode::Managed,
            url: None,
            binary: None,
            model_id: None,
            port: 8001,
        }
    }
}

/// Лимит токенов ответа саб-агента по умолчанию (`call_subagent`, spec §9.3.2).
pub const DEFAULT_SUBAGENT_MAX_TOKENS: usize = 1024;
/// Лимит времени на один вызов саб-агента по умолчанию (секунды).
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 60;

/// Глобальные «мастер-выключатели» внешних инструментов (безопасность/приватность,
/// spec §9.4, §13.2). Эффективный набор = `Profile.enabled_tools ∩ глобально вкл.`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSettings {
    /// Web-поиск (DuckDuckGo). Включён по умолчанию (read-only).
    pub web_enabled: bool,
    /// Исполнение Python. Выключено по умолчанию (нет OS-песочницы, spec §13.2).
    pub python_enabled: bool,
    /// Путь к интерпретатору Python (`None` → системный `python3`/`python`).
    pub python_path: Option<String>,
    /// Лимит токенов ответа саб-агента (`call_subagent`).
    pub subagent_max_tokens: usize,
    /// Лимит времени на вызов саб-агента (секунды).
    pub subagent_timeout_secs: u64,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            web_enabled: true,
            python_enabled: false,
            python_path: None,
            subagent_max_tokens: DEFAULT_SUBAGENT_MAX_TOKENS,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
        }
    }
}

/// Тема оформления TUI. Реальное применение в виджетах — на M9 (`shared/theme.rs`);
/// здесь хранится выбор пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// По системной настройке (по умолчанию).
    #[default]
    Auto,
    Dark,
    Light,
}

/// Настройки интерфейса (тема, спелл-чек, словари). См. spec §11.6.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceSettings {
    pub theme: Theme,
    /// Включён ли спелл-чек ввода (словари грузятся из `dictionaries/`).
    pub spellcheck_enabled: bool,
    /// Базовые имена выбранных словарей (например `en_US`, `ru_RU`). Пусто —
    /// использовать все найденные в каталоге.
    pub selected_dictionaries: Vec<String>,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            spellcheck_enabled: true,
            selected_dictionaries: Vec::new(),
        }
    }
}

/// Глобальная конфигурация приложения.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub default_sampling: SamplingConfig,
    pub xinfer: XinferSettings,
    /// Настройки выделенного embedding-сервера (RAG, ADR 0002).
    pub embed: EmbedSettings,
    /// Лимит раундов клиентского agentic-loop (spec §6.3).
    pub max_tool_rounds: u32,
    /// Глобальные выключатели внешних инструментов.
    pub tools: ToolSettings,
    /// Настройки интерфейса (тема, спелл-чек, словари).
    pub interface: InterfaceSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            default_sampling: SamplingConfig {
                max_tokens: Some(2048),
                thinking: Some(true),
                ..Default::default()
            },
            xinfer: XinferSettings::default(),
            embed: EmbedSettings::default(),
            max_tool_rounds: 8,
            tools: ToolSettings::default(),
            interface: InterfaceSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_current_schema_version() {
        assert_eq!(AppConfig::default().schema_version, SCHEMA_VERSION);
        assert_eq!(AppConfig::default().max_tool_rounds, 8);
    }

    #[test]
    fn serde_roundtrip() {
        let c = AppConfig::default();
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let c: AppConfig = serde_json::from_str(r#"{"max_tool_rounds":4}"#).unwrap();
        assert_eq!(c.max_tool_rounds, 4);
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        assert_eq!(c.xinfer.port, 8000);
        // Новые секции наполняются дефолтами при их отсутствии в файле.
        assert_eq!(c.embed.port, 8001);
        assert_eq!(c.tools.subagent_max_tokens, DEFAULT_SUBAGENT_MAX_TOKENS);
        assert_eq!(c.tools.subagent_timeout_secs, DEFAULT_SUBAGENT_TIMEOUT_SECS);
        assert!(c.interface.spellcheck_enabled);
        assert_eq!(c.interface.theme, Theme::Auto);
    }

    #[test]
    fn extended_sections_roundtrip() {
        let c = AppConfig {
            embed: EmbedSettings {
                mode: ServerMode::External,
                url: Some("http://127.0.0.1:8001/v1".into()),
                model_id: Some("Qwen/Qwen3-Embedding-0.6B".into()),
                ..Default::default()
            },
            tools: ToolSettings {
                python_enabled: true,
                subagent_max_tokens: 512,
                subagent_timeout_secs: 30,
                ..Default::default()
            },
            interface: InterfaceSettings {
                theme: Theme::Dark,
                spellcheck_enabled: false,
                selected_dictionaries: vec!["en_US".into(), "ru_RU".into()],
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn theme_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), "\"dark\"");
    }
}
