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
    /// Приложение само запускает дочерний процесс `llama-server`.
    #[default]
    Managed,
    /// Подключение к уже запущенному серверу.
    External,
}

/// Число GPU-слоёв по умолчанию (`-ngl`): всё на GPU.
pub const DEFAULT_GPU_LAYERS: i32 = 99;
/// Размер контекста по умолчанию (`-c`).
pub const DEFAULT_CONTEXT_SIZE: u32 = 8192;

/// Настройки chat-сервера инференса. Транспорт — OpenAI-совместимый HTTP, поэтому
/// в external-режиме подойдёт любой такой сервер (llama.cpp `llama-server`, vLLM,
/// LM Studio, …). В managed-режиме mindfork запускает **`llama-server`** (llama.cpp)
/// дочерним процессом. См. docs/install.md §3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub mode: ServerMode,
    /// URL для external-режима (например `http://127.0.0.1:8000/v1`).
    pub url: Option<String>,
    /// Путь к бинарнику `llama-server` для managed-режима.
    pub binary: Option<String>,
    /// Путь к GGUF-модели (`-m`).
    pub model_path: Option<String>,
    /// Слои на GPU (`-ngl`).
    pub gpu_layers: i32,
    /// Размер контекста (`-c`).
    pub context_size: u32,
    /// Использовать встроенный chat-template модели (`--jinja`) — нужен для
    /// корректного формата и tool-calling.
    pub jinja: bool,
    /// Формат reasoning (`--reasoning-format`, например `auto`); `None` — не задавать.
    pub reasoning_format: Option<String>,
    /// Не использовать mmap при загрузке модели (`--no-mmap`): веса грузятся в RAM
    /// целиком. Помогает на сетевых/медленных дисках. По умолчанию выключено.
    pub no_mmap: bool,
    /// Интерфейс bind (`--host`), например `127.0.0.1` или `0.0.0.0`.
    pub host: String,
    pub port: u16,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            mode: ServerMode::Managed,
            url: None,
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            context_size: DEFAULT_CONTEXT_SIZE,
            jinja: true,
            reasoning_format: None,
            no_mmap: false,
            host: "127.0.0.1".to_string(),
            port: 8000,
        }
    }
}

/// Режим сервера имперсонации (написание сообщения от имени пользователя).
/// Отличается от [`ServerMode`] третьим вариантом `Shared`. См. spec §11.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpersonationMode {
    /// Использовать тот же сервер, что и для ответов ассистента (managed или
    /// external), но с семплингом из подсекции «Имперсонация».
    #[default]
    Shared,
    /// Поднять отдельный дочерний процесс `llama-server`.
    Managed,
    /// Подключиться к отдельному удалённому серверу.
    External,
}

/// Порт по умолчанию для managed-сервера имперсонации (отдельный инстанс).
pub const DEFAULT_IMPERSONATION_PORT: u16 = 8002;

/// Настройки сервера имперсонации. Поля идентичны [`EngineSettings`], но режим —
/// [`ImpersonationMode`] (добавлен `shared`). В режиме `shared` остальные поля
/// (url/binary/model/…) не используются — берётся chat-сервер ассистента. См.
/// spec §11.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpersonationEngineSettings {
    pub mode: ImpersonationMode,
    /// URL для external-режима (например `http://127.0.0.1:8002/v1`).
    pub url: Option<String>,
    /// Путь к бинарнику `llama-server` для managed-режима.
    pub binary: Option<String>,
    /// Путь к GGUF-модели (`-m`).
    pub model_path: Option<String>,
    /// Слои на GPU (`-ngl`).
    pub gpu_layers: i32,
    /// Размер контекста (`-c`).
    pub context_size: u32,
    /// Использовать встроенный chat-template модели (`--jinja`).
    pub jinja: bool,
    /// Формат reasoning (`--reasoning-format`); `None` — не задавать.
    pub reasoning_format: Option<String>,
    /// Не использовать mmap при загрузке модели (`--no-mmap`).
    pub no_mmap: bool,
    /// Интерфейс bind (`--host`).
    pub host: String,
    pub port: u16,
}

impl Default for ImpersonationEngineSettings {
    fn default() -> Self {
        Self {
            mode: ImpersonationMode::Shared,
            url: None,
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            context_size: DEFAULT_CONTEXT_SIZE,
            jinja: true,
            reasoning_format: None,
            no_mmap: false,
            host: "127.0.0.1".to_string(),
            port: DEFAULT_IMPERSONATION_PORT,
        }
    }
}

/// Настройки выделенного embedding-сервера для RAG (ADR 0002). Отдельный
/// процесс/порт; если не настроен (`UnavailableEmbedder`) — RAG отдаёт ошибку.
/// В managed-режиме — тот же `llama-server` с `--embeddings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub mode: ServerMode,
    /// URL для external-режима (например `http://127.0.0.1:8001/v1`).
    pub url: Option<String>,
    /// Путь к бинарнику `llama-server` для managed-режима.
    pub binary: Option<String>,
    /// Путь к GGUF embedding-модели (`-m`).
    pub model_path: Option<String>,
    /// Слои на GPU (`-ngl`).
    pub gpu_layers: i32,
    pub port: u16,
}

impl Default for EmbedSettings {
    fn default() -> Self {
        Self {
            mode: ServerMode::Managed,
            url: None,
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
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

/// Целевой («мягкий») размер чанка RAG в символах по умолчанию.
pub const DEFAULT_CHUNK_TARGET_CHARS: usize = 800;
/// Перекрытие между соседними чанками RAG в символах по умолчанию.
pub const DEFAULT_CHUNK_OVERLAP_CHARS: usize = 150;
/// Жёсткий потолок неделимого прогона чанка RAG в символах по умолчанию.
pub const DEFAULT_CHUNK_MAX_CHARS: usize = 1200;

/// Настройки чанкинга базы знаний (RAG). Влияют на нарезку при индексации
/// (`/rag add`, инструмент `rag_add`) и реиндексации (`/rag rebuild`). Размеры — в
/// символах (не байтах, корректно для кириллицы/Юникода). См. spec §9.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RagSettings {
    /// Целевой («мягкий») размер чанка: юниты пакуются до него.
    pub chunk_target_chars: usize,
    /// Перекрытие соседних чанков: хвост предыдущего повторяется в начале следующего.
    pub chunk_overlap_chars: usize,
    /// Жёсткий потолок неделимого прогона (очень длинное слово/строка без пунктуации).
    pub chunk_max_chars: usize,
}

impl Default for RagSettings {
    fn default() -> Self {
        Self {
            chunk_target_chars: DEFAULT_CHUNK_TARGET_CHARS,
            chunk_overlap_chars: DEFAULT_CHUNK_OVERLAP_CHARS,
            chunk_max_chars: DEFAULT_CHUNK_MAX_CHARS,
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
    /// Семплинг для режима имперсонации (написание сообщения от имени пользователя).
    /// Применяется во всех режимах сервера имперсонации (в т.ч. `shared`). См. spec §11.8.
    pub impersonation_sampling: SamplingConfig,
    /// Настройки chat-сервера инференса (llama.cpp managed или любой OpenAI external).
    pub engine: EngineSettings,
    /// Настройки сервера имперсонации (shared/managed/external). См. spec §11.8.
    pub impersonation_engine: ImpersonationEngineSettings,
    /// Настройки выделенного embedding-сервера (RAG, ADR 0002).
    pub embed: EmbedSettings,
    /// Лимит раундов клиентского agentic-loop (spec §6.3).
    pub max_tool_rounds: u32,
    /// Глобальные выключатели внешних инструментов.
    pub tools: ToolSettings,
    /// Настройки чанкинга базы знаний (RAG).
    pub rag: RagSettings,
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
            // Имперсонация пишет короткую реплику от лица пользователя — «мысли»
            // ей не нужны (только съели бы бюджет), лимит токенов скромный.
            impersonation_sampling: SamplingConfig {
                max_tokens: Some(1024),
                thinking: Some(false),
                ..Default::default()
            },
            engine: EngineSettings::default(),
            impersonation_engine: ImpersonationEngineSettings::default(),
            embed: EmbedSettings::default(),
            max_tool_rounds: 8,
            tools: ToolSettings::default(),
            rag: RagSettings::default(),
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
        assert_eq!(c.engine.port, 8000);
        assert_eq!(c.engine.gpu_layers, DEFAULT_GPU_LAYERS);
        assert!(c.engine.jinja);
        // Новые секции наполняются дефолтами при их отсутствии в файле.
        assert_eq!(c.embed.port, 8001);
        assert_eq!(c.tools.subagent_max_tokens, DEFAULT_SUBAGENT_MAX_TOKENS);
        assert_eq!(c.tools.subagent_timeout_secs, DEFAULT_SUBAGENT_TIMEOUT_SECS);
        assert_eq!(c.rag.chunk_target_chars, DEFAULT_CHUNK_TARGET_CHARS);
        assert_eq!(c.rag.chunk_overlap_chars, DEFAULT_CHUNK_OVERLAP_CHARS);
        assert_eq!(c.rag.chunk_max_chars, DEFAULT_CHUNK_MAX_CHARS);
        assert!(c.interface.spellcheck_enabled);
        assert_eq!(c.interface.theme, Theme::Auto);
        // Имперсонация наполняется дефолтами при отсутствии в файле.
        assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Shared);
        assert_eq!(c.impersonation_engine.port, DEFAULT_IMPERSONATION_PORT);
        assert_eq!(c.impersonation_sampling.thinking, Some(false));
    }

    #[test]
    fn impersonation_sections_roundtrip() {
        let c = AppConfig {
            impersonation_engine: ImpersonationEngineSettings {
                mode: ImpersonationMode::Managed,
                binary: Some("llama-server".into()),
                model_path: Some("persona.gguf".into()),
                port: 8002,
                ..Default::default()
            },
            impersonation_sampling: SamplingConfig {
                temperature: Some(0.8),
                max_tokens: Some(256),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn impersonation_mode_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ImpersonationMode::External).unwrap(),
            "\"external\""
        );
        assert_eq!(
            serde_json::to_string(&ImpersonationMode::Shared).unwrap(),
            "\"shared\""
        );
    }

    #[test]
    fn extended_sections_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::Managed,
                binary: Some("llama-server".into()),
                model_path: Some("gemma.gguf".into()),
                gpu_layers: 50,
                context_size: 4096,
                jinja: true,
                reasoning_format: Some("auto".into()),
                ..Default::default()
            },
            embed: EmbedSettings {
                mode: ServerMode::External,
                url: Some("http://127.0.0.1:8001/v1".into()),
                model_path: Some("embed.gguf".into()),
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
