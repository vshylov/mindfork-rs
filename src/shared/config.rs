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

/// Глобальная конфигурация приложения.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub default_sampling: SamplingConfig,
    pub xinfer: XinferSettings,
    /// Лимит раундов клиентского agentic-loop (spec §6.3).
    pub max_tool_rounds: u32,
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
            max_tool_rounds: 8,
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
    }
}
