//! Глобальная конфигурация приложения (`settings.json`). См. spec §12.1.
//! Версионируется полем `schema_version` для будущих миграций.

use serde::{Deserialize, Serialize};

use crate::entities::sampling::SamplingConfig;

/// Текущая версия схемы конфигурации.
pub const SCHEMA_VERSION: u32 = 1;

/// Режим подключения к движку инференса. Локальные (`Managed`/`External`) и
/// облачные провайдеры (`OpenAi`/`Gemini`) — равноправные варианты одного селектора
/// (плоская таксономия, [ADR 0004](decisions/0004-engine-contract-multi-provider.md)).
/// Claude добавляется на Фазе 2 (отдельный протокол `/v1/messages`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    /// Приложение само запускает дочерний процесс `llama-server`.
    #[default]
    Managed,
    /// Подключение к уже запущенному OpenAI-совместимому серверу (любой: llama.cpp,
    /// vLLM, LM Studio…). Поля сэмплинга шлются «как есть» (lenient-диалект).
    External,
    /// Облако OpenAI (`platform.openai.com`). Строгий OpenAI-диалект + Bearer-ключ.
    #[serde(rename = "openai")]
    OpenAi,
    /// Облако Google Gemini через OpenAI-совместимый endpoint. Строгий диалект + ключ.
    Gemini,
    /// Облако Anthropic (`platform.claude.com`). Отдельный протокол Messages API
    /// (`/v1/messages`), `x-api-key`. См. ADR 0004, Фаза 2.
    Claude,
}

/// Облачный провайдер инференса. `OpenAi`/`Gemini` говорят на OpenAI-протоколе,
/// `Claude` — на Anthropic Messages API. Несёт базовый URL по умолчанию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudProvider {
    OpenAi,
    Gemini,
    Claude,
}

impl CloudProvider {
    /// Базовый URL для **эмбеддингов**/совместимого доступа (OpenAI-совместимый
    /// endpoint). У Gemini это compat-путь `…/v1beta/openai` (эмбеддинги RAG идут
    /// через него, `OpenAiClient`). Можно переопределить полем `url`.
    pub fn base_url(self) -> &'static str {
        match self {
            CloudProvider::OpenAi => "https://api.openai.com/v1",
            CloudProvider::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            // Anthropic-клиент сам добавляет `/v1/messages`, поэтому без суффикса.
            CloudProvider::Claude => "https://api.anthropic.com",
        }
    }

    /// Базовый URL для **чата** (нативный протокол клиента). У Gemini — `…/v1beta`
    /// (нативный `GeminiClient` добавляет `/models/{model}:streamGenerateContent`), в
    /// отличие от compat-пути эмбеддингов ([`base_url`](Self::base_url)). У OpenAI
    /// (Responses) и Claude совпадает с `base_url`. Можно переопределить полем `url`.
    pub fn chat_base_url(self) -> &'static str {
        match self {
            CloudProvider::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            CloudProvider::OpenAi | CloudProvider::Claude => self.base_url(),
        }
    }

    /// Стабильный строковый ключ провайдера — им индексируются сохранённые API-ключи
    /// (`AppConfig::api_keys`, см. `shared::secrets`). Ключ **общий** для чата,
    /// имперсонации и эмбеддингов этого провайдера. Значения персистятся в
    /// `settings.json` — не переименовывать.
    pub fn key(self) -> &'static str {
        match self {
            CloudProvider::OpenAi => "openai",
            CloudProvider::Gemini => "gemini",
            CloudProvider::Claude => "claude",
        }
    }
}

impl ServerMode {
    /// Облачный провайдер для этого режима (`None` — локальный managed/external).
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            ServerMode::OpenAi => Some(CloudProvider::OpenAi),
            ServerMode::Gemini => Some(CloudProvider::Gemini),
            ServerMode::Claude => Some(CloudProvider::Claude),
            ServerMode::Managed | ServerMode::External => None,
        }
    }
}

/// Число GPU-слоёв по умолчанию (`-ngl`): всё на GPU.
pub const DEFAULT_GPU_LAYERS: i32 = 99;
/// Размер контекста по умолчанию (`-c`).
pub const DEFAULT_CONTEXT_SIZE: u32 = 8192;

/// Режим FlashAttention (`--flash-attn`) managed-сервера llama.cpp. `Auto` — флаг
/// не передаётся (llama.cpp решает сам, это его дефолт); `On`/`Off` — принудительно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashAttn {
    #[default]
    Auto,
    On,
    Off,
}

impl FlashAttn {
    /// Все варианты в порядке перебора UI (для Choice-попапа и цикла).
    pub const ALL: [FlashAttn; 3] = [FlashAttn::Auto, FlashAttn::On, FlashAttn::Off];

    /// Значение для `--flash-attn`; `None` (Auto) — флаг не передавать.
    pub fn as_arg(self) -> Option<&'static str> {
        match self {
            FlashAttn::Auto => None,
            FlashAttn::On => Some("on"),
            FlashAttn::Off => Some("off"),
        }
    }

    /// Подпись для UI (Choice-поле).
    pub fn label(self) -> &'static str {
        match self {
            FlashAttn::Auto => "auto",
            FlashAttn::On => "on",
            FlashAttn::Off => "off",
        }
    }

    /// Циклический перебор с учётом направления (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Тип спекулятивного декодирования (`--spec-type`) managed-сервера llama.cpp.
/// `None` — выключено (флаг не передаётся). Типы `draft-*` требуют черновую модель
/// (`-md`) — для MTP-моделей (например `mtp-gemma-4-12B-it.gguf`) это `draft-mtp`;
/// `ngram-*` отдельной модели не требуют (черновик берётся из истории контекста).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecType {
    #[default]
    None,
    DraftSimple,
    DraftEagle3,
    DraftMtp,
    NgramSimple,
    NgramMapK,
    NgramMapK4v,
    NgramMod,
    NgramCache,
}

impl SpecType {
    /// Все варианты в порядке перебора UI (для Choice-попапа и цикла).
    pub const ALL: [SpecType; 9] = [
        SpecType::None,
        SpecType::DraftSimple,
        SpecType::DraftEagle3,
        SpecType::DraftMtp,
        SpecType::NgramSimple,
        SpecType::NgramMapK,
        SpecType::NgramMapK4v,
        SpecType::NgramMod,
        SpecType::NgramCache,
    ];

    /// Значение для `--spec-type`; `None` — флаг не передавать (выключено).
    pub fn as_arg(self) -> Option<&'static str> {
        match self {
            SpecType::None => Option::None,
            SpecType::DraftSimple => Some("draft-simple"),
            SpecType::DraftEagle3 => Some("draft-eagle3"),
            SpecType::DraftMtp => Some("draft-mtp"),
            SpecType::NgramSimple => Some("ngram-simple"),
            SpecType::NgramMapK => Some("ngram-map-k"),
            SpecType::NgramMapK4v => Some("ngram-map-k4v"),
            SpecType::NgramMod => Some("ngram-mod"),
            SpecType::NgramCache => Some("ngram-cache"),
        }
    }

    /// Требует ли тип отдельную черновую модель (`-md`): только `draft-*`.
    pub fn needs_draft_model(self) -> bool {
        matches!(
            self,
            SpecType::DraftSimple | SpecType::DraftEagle3 | SpecType::DraftMtp
        )
    }

    /// Подпись для UI (Choice-поле).
    pub fn label(self) -> &'static str {
        self.as_arg().unwrap_or("none")
    }

    /// Циклический перебор с учётом направления (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Настройки локального managed-сервера `llama-server` (llama.cpp): приложение
/// запускает его дочерним процессом. Своя под-секция в каждом движке, чтобы
/// переключение режима не теряло этих значений. См. docs/install.md §3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedSettings {
    /// Путь к бинарнику `llama-server`.
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
    /// FlashAttention (`--flash-attn`): оптимизация внимания. По умолчанию `Auto`.
    pub flash_attn: FlashAttn,
    /// Тип спекулятивного декодирования (`--spec-type`). По умолчанию выключено.
    pub spec_type: SpecType,
    /// Черновая модель для спекулятивного декодирования (`-md`/`--model-draft`).
    /// Нужна для типов `draft-*`; для MTP-моделей — путь к соответствующему GGUF.
    pub draft_model: Option<String>,
    /// GPU-слои черновой модели (`-ngld`); `None` — авто (флаг не передаётся).
    pub draft_gpu_layers: Option<i32>,
    /// Сколько токенов набрасывать черновиком за шаг (`--spec-draft-n-max`); `None` —
    /// дефолт llama.cpp (3).
    pub draft_n_max: Option<u32>,
    /// Минимум черновых токенов за шаг (`--spec-draft-n-min`); `None` — дефолт (0).
    pub draft_n_min: Option<u32>,
    /// Интерфейс bind (`--host`), например `127.0.0.1` или `0.0.0.0`.
    pub host: String,
    pub port: u16,
}

impl Default for ManagedSettings {
    fn default() -> Self {
        Self {
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            context_size: DEFAULT_CONTEXT_SIZE,
            jinja: true,
            reasoning_format: None,
            no_mmap: false,
            flash_attn: FlashAttn::default(),
            spec_type: SpecType::default(),
            draft_model: None,
            draft_gpu_layers: None,
            draft_n_max: None,
            draft_n_min: None,
            host: "127.0.0.1".to_string(),
            port: 8000,
        }
    }
}

/// Настройки external-режима: подключение к уже запущенному OpenAI-совместимому
/// серверу (любой: llama.cpp, vLLM, LM Studio…).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalSettings {
    /// URL сервера (например `http://127.0.0.1:8000/v1`).
    pub url: Option<String>,
    /// Имя модели для мульти-модельного сервера (опционально).
    pub model_name: Option<String>,
    /// Имя env-переменной с Bearer-ключом (опционально) — для OpenAI-совместимого
    /// прокси/шлюза, требующего авторизацию. Хранится **имя**, не секрет (ADR 0004).
    /// `None`/пусто — без авторизации (локальный `llama-server` её не требует).
    pub api_key_env: Option<String>,
}

/// Настройки одного облачного провайдера (OpenAI/Gemini/Claude). Хранятся
/// отдельно на каждого, чтобы переключение провайдера не теряло чужих значений.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    /// Имя модели у провайдера (`gpt-4o`, `gemini-2.5-pro`, `claude-opus-4-8`).
    pub model_name: Option<String>,
    /// Имя env-переменной с API-ключом (например `OPENAI_API_KEY`). Хранится
    /// **имя**, а не сам секрет — ключ читается из окружения (ADR 0004).
    pub api_key_env: Option<String>,
    /// Переопределение базового URL провайдера (опционально); `None` — дефолт провайдера.
    pub url: Option<String>,
}

/// Настройки chat-сервера инференса. Под-секция на каждый режим/провайдера
/// (managed/external/openai/gemini/claude), чтобы переключение режима не теряло
/// чужих значений. Транспорт — OpenAI-совместимый HTTP (кроме Claude — Messages
/// API). См. docs/install.md §3, ADR 0004.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    pub mode: ServerMode,
    pub managed: ManagedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl EngineSettings {
    /// Облачные настройки активного провайдера (`None` — локальный managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Изменяемые облачные настройки активного провайдера (`None` — локальный).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }

    /// Имя активной модели для текущего режима (для снимка в `Message.metadata` и
    /// подписи ленты). Managed — базовое имя GGUF без пути и расширения `.gguf`;
    /// external/облако — заданное `model_name`. `None`, если модель не задана.
    pub fn active_model_name(&self) -> Option<String> {
        match self.mode {
            ServerMode::Managed => self.managed.model_path.as_deref().and_then(|p| {
                let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
                let name = name.trim_end_matches(".gguf");
                (!name.is_empty()).then(|| name.to_string())
            }),
            ServerMode::External => self.external.model_name.clone().filter(|m| !m.is_empty()),
            ServerMode::OpenAi | ServerMode::Gemini | ServerMode::Claude => self
                .cloud()
                .and_then(|c| c.model_name.clone())
                .filter(|m| !m.is_empty()),
        }
    }
}

/// Активная облачная под-структура по провайдеру (общий хелпер для всех движков).
fn cloud_ref<'a>(
    provider: Option<CloudProvider>,
    openai: &'a CloudSettings,
    gemini: &'a CloudSettings,
    claude: &'a CloudSettings,
) -> Option<&'a CloudSettings> {
    match provider? {
        CloudProvider::OpenAi => Some(openai),
        CloudProvider::Gemini => Some(gemini),
        CloudProvider::Claude => Some(claude),
    }
}

fn cloud_mut<'a>(
    provider: Option<CloudProvider>,
    openai: &'a mut CloudSettings,
    gemini: &'a mut CloudSettings,
    claude: &'a mut CloudSettings,
) -> Option<&'a mut CloudSettings> {
    match provider? {
        CloudProvider::OpenAi => Some(openai),
        CloudProvider::Gemini => Some(gemini),
        CloudProvider::Claude => Some(claude),
    }
}

/// Режим сервера имперсонации (написание сообщения от имени пользователя).
/// Отличается от [`ServerMode`] третьим вариантом `Shared`. См. spec §11.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpersonationMode {
    /// Использовать тот же движок, что и для ответов ассистента (любой режим), но
    /// с семплингом из подсекции «Имперсонация».
    #[default]
    Shared,
    /// Поднять отдельный дочерний процесс `llama-server`.
    Managed,
    /// Подключиться к отдельному удалённому OpenAI-совместимому серверу.
    External,
    /// Облако OpenAI (отдельно от ассистента).
    #[serde(rename = "openai")]
    OpenAi,
    /// Облако Google Gemini через OpenAI-совместимый endpoint.
    Gemini,
    /// Облако Anthropic (Claude, Messages API).
    Claude,
}

impl ImpersonationMode {
    /// Облачный провайдер для этого режима (`None` — shared/managed/external).
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            ImpersonationMode::OpenAi => Some(CloudProvider::OpenAi),
            ImpersonationMode::Gemini => Some(CloudProvider::Gemini),
            ImpersonationMode::Claude => Some(CloudProvider::Claude),
            ImpersonationMode::Shared
            | ImpersonationMode::Managed
            | ImpersonationMode::External => None,
        }
    }
}

/// Порт по умолчанию для managed-сервера имперсонации (отдельный инстанс).
pub const DEFAULT_IMPERSONATION_PORT: u16 = 8002;

/// Настройки сервера имперсонации. Под-секции идентичны [`EngineSettings`], но
/// режим — [`ImpersonationMode`] (добавлен `shared`). В режиме `shared` под-секции
/// не используются — берётся chat-сервер ассистента. См. spec §11.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImpersonationEngineSettings {
    pub mode: ImpersonationMode,
    pub managed: ManagedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl Default for ImpersonationEngineSettings {
    fn default() -> Self {
        Self {
            mode: ImpersonationMode::Shared,
            // Отдельный managed-инстанс имперсонации слушает свой порт.
            managed: ManagedSettings {
                port: DEFAULT_IMPERSONATION_PORT,
                ..Default::default()
            },
            external: ExternalSettings::default(),
            openai: CloudSettings::default(),
            gemini: CloudSettings::default(),
            claude: CloudSettings::default(),
        }
    }
}

impl ImpersonationEngineSettings {
    /// Облачные настройки активного провайдера (`None` — shared/managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Изменяемые облачные настройки активного провайдера (`None` — локальный).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }
}

/// Порт embedding-сервера по умолчанию.
pub const DEFAULT_EMBED_PORT: u16 = 8001;

/// Настройки managed embedding-сервера: тот же `llama-server` с `--embeddings`.
/// У эмбеддинг-сервера нет chat-template/host/no_mmap — супервайзер их фиксирует,
/// поэтому полей меньше, чем у [`ManagedSettings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedEmbedSettings {
    /// Путь к бинарнику `llama-server`.
    pub binary: Option<String>,
    /// Путь к GGUF embedding-модели (`-m`).
    pub model_path: Option<String>,
    /// Слои на GPU (`-ngl`).
    pub gpu_layers: i32,
    pub port: u16,
}

impl Default for ManagedEmbedSettings {
    fn default() -> Self {
        Self {
            binary: None,
            model_path: None,
            gpu_layers: DEFAULT_GPU_LAYERS,
            port: DEFAULT_EMBED_PORT,
        }
    }
}

/// Настройки выделенного embedding-сервера для RAG (ADR 0002). Отдельный
/// процесс/порт; если не настроен (`UnavailableEmbedder`) — RAG отдаёт ошибку.
/// Под-секция на режим/провайдера (как у [`EngineSettings`]); облачные эмбеддинги
/// есть у OpenAI/Gemini (у Anthropic нет — RAG отключится). См. ADR 0004.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub mode: ServerMode,
    pub managed: ManagedEmbedSettings,
    pub external: ExternalSettings,
    pub openai: CloudSettings,
    pub gemini: CloudSettings,
    pub claude: CloudSettings,
}

impl EmbedSettings {
    /// Облачные настройки активного провайдера (`None` — локальный managed/external).
    pub fn cloud(&self) -> Option<&CloudSettings> {
        cloud_ref(
            self.mode.cloud_provider(),
            &self.openai,
            &self.gemini,
            &self.claude,
        )
    }

    /// Изменяемые облачные настройки активного провайдера (`None` — локальный).
    pub fn cloud_mut(&mut self) -> Option<&mut CloudSettings> {
        cloud_mut(
            self.mode.cloud_provider(),
            &mut self.openai,
            &mut self.gemini,
            &mut self.claude,
        )
    }
}

/// Лимит токенов ответа саб-агента по умолчанию (`call_subagent`, spec §9.3.2).
pub const DEFAULT_SUBAGENT_MAX_TOKENS: usize = 1024;
/// Лимит времени на один вызов саб-агента по умолчанию (секунды).
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 60;
/// Таймаут исполнения кода в песочнице Wasmer по умолчанию (секунды). Щедрее, чем у
/// локального интерпретатора (10с): WASM-интерпретация в ~2–5× медленнее нативной.
/// См. docs/research/python-wasmer-sandbox.md.
pub const DEFAULT_PYTHON_WASM_TIMEOUT_SECS: u64 = 30;

/// Режим исполнения `python_exec`: изолированная песочница Wasmer/WASIX (по
/// умолчанию — нет доступа к файлам машины, предустановленные пакеты) либо локальный
/// системный интерпретатор (прежнее поведение). См.
/// docs/research/python-wasmer-sandbox.md (Фаза 0 → сайдкар `wasmer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PythonMode {
    /// Изолированная песочница на бандленном `wasmer` (сайдкар). Дефолт.
    #[default]
    Wasmer,
    /// Локальный системный интерпретатор (`python`/`python3`), без изоляции.
    Local,
}

impl PythonMode {
    /// Все варианты в порядке перебора UI (для Choice-попапа и цикла).
    pub const ALL: [PythonMode; 2] = [PythonMode::Wasmer, PythonMode::Local];

    /// Подпись для UI (Choice-поле).
    pub fn label(self) -> &'static str {
        match self {
            PythonMode::Wasmer => "Wasmer-песочница",
            PythonMode::Local => "локальный интерпретатор",
        }
    }

    /// Циклический перебор с учётом направления (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Глобальные «мастер-выключатели» внешних инструментов (безопасность/приватность,
/// spec §9.4, §13.2). Эффективный набор = `Profile.enabled_tools ∩ глобально вкл.`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSettings {
    /// Web-поиск (DuckDuckGo). Включён по умолчанию (read-only).
    pub web_enabled: bool,
    /// Загружать страницы результатов, извлекать текст и переупорядочивать по
    /// релевантности (`web_search`, spec §9.3.1). Включено по умолчанию; даёт модели
    /// содержимое страниц, но добавляет задержку (загрузка до `max_results` страниц).
    /// Аргумент `fetch_content` вызова переопределяет это значение.
    pub web_fetch_content: bool,
    /// Исполнение Python. Выключено по умолчанию (мастер-гейт инструмента).
    pub python_enabled: bool,
    /// Режим исполнения `python_exec`: песочница Wasmer (по умолчанию) или локальный
    /// интерпретатор. См. [`PythonMode`].
    pub python_mode: PythonMode,
    /// Путь к интерпретатору Python (`None` → системный `python3`/`python`).
    /// Используется только в режиме [`PythonMode::Local`].
    pub python_path: Option<String>,
    /// Разрешить сеть внутри песочницы Wasmer (`--net`). По умолчанию включено —
    /// главная ценность предустановленного `requests`; но код в песочнице сможет
    /// ходить в сеть. Режим [`PythonMode::Local`] это поле не использует (там сеть
    /// всегда есть). См. docs/research/python-wasmer-sandbox.md §7.
    pub python_net_enabled: bool,
    /// Таймаут исполнения в песочнице Wasmer (секунды). Локальный режим держит свой
    /// (меньший) таймаут. См. [`DEFAULT_PYTHON_WASM_TIMEOUT_SECS`].
    pub python_wasm_timeout_secs: u64,
    /// Жёсткий OS-level лимит памяти песочницы Wasmer (МБ; `None`/0 — без лимита).
    /// Защита хоста от OOM при рантайм-скрипте: при превышении процесс `wasmer`
    /// убивается (не graceful — V8 падает с «Fatal out of memory»). **Только
    /// Windows** (Job Object); на Unix не применяется (rlimit ненадёжен с V8, ADR
    /// 0005). Минимум ~1024 (меньше — песочница может не стартовать: V8+CPython
    /// требует ~768 МБ). По умолчанию без лимита (защита в глубину поверх таймаута
    /// и wasm32 ~4 ГБ).
    pub python_wasm_memory_mb: Option<u64>,
    /// Доступ к локальным файлам (`fs_read`/`fs_write`/`fs_list`). Выключен по
    /// умолчанию (инструмент может прочитать/перезаписать любой файл — приватность/
    /// безопасность, как у Python). См. spec §9.3, §13.2.
    pub fs_enabled: bool,
    /// Каталог-«песочница» для файловых инструментов (`None` → без ограничения).
    /// Если задан, все пути обязаны лежать внутри него (защита от выхода `..`).
    pub fs_root: Option<String>,
    /// Лимит токенов ответа саб-агента (`call_subagent`).
    pub subagent_max_tokens: usize,
    /// Лимит времени на вызов саб-агента (секунды).
    pub subagent_timeout_secs: u64,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            web_enabled: true,
            web_fetch_content: true,
            python_enabled: false,
            python_mode: PythonMode::default(),
            python_path: None,
            python_net_enabled: true,
            python_wasm_timeout_secs: DEFAULT_PYTHON_WASM_TIMEOUT_SECS,
            python_wasm_memory_mb: None,
            fs_enabled: false,
            fs_root: None,
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

/// Потолок хранения нарратива «модели себя» (инсайтов) по умолчанию.
pub const DEFAULT_SELF_MODEL_MAX_NARRATIVE: usize = 50;
/// Сколько свежих инсайтов подмешивать в системный промпт по умолчанию.
pub const DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT: usize = 3;
/// Потолок символов рендера «модели себя» в системный промпт по умолчанию.
pub const DEFAULT_SELF_MODEL_PROMPT_CAP: usize = 1200;
/// Подмешивать ли нейтральный к персоне «протокол ведения модели» по умолчанию.
pub const DEFAULT_SELF_MODEL_MAINTENANCE_PROTOCOL: bool = true;
/// Сколько закрытых целей держать в структуре по умолчанию (старейшие сверх этого
/// сворачиваются в нарратив-шрам и удаляются — потолок закрытых целей).
pub const DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS: usize = 10;
/// Ориентир размера описания себя (summary) в символах по умолчанию. Сверх него
/// инструменты и протокол ведения мягко предлагают сократить описание (это ворота,
/// а не потолок — данные не усекаются). См. docs/summary-as-snapshot.md (этап 2).
pub const DEFAULT_SELF_MODEL_SUMMARY_TARGET: usize = 1000;

/// Настройки «модели себя» (SelfModel): размеры нарратива и объём инъекции в
/// системный промпт. См. [docs/history/self-model-mvp.md] и spec §9.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfModelSettings {
    /// Сколько инсайтов хранить в нарративе (старые вытесняются при добавлении).
    pub max_narrative: usize,
    /// Сколько свежих инсайтов подмешивать в системный промпт.
    pub narrative_in_prompt: usize,
    /// Потолок символов компактного рендера модели в системный промпт.
    pub prompt_cap: usize,
    /// Сколько закрытых (выполненных/неактуальных) целей держать в структуре;
    /// старейшие сверх этого сворачиваются в нарратив-шрам и удаляются.
    pub max_closed_goals: usize,
    /// Ориентир размера описания себя (summary) в символах: сверх него инструменты и
    /// протокол ведения мягко предлагают сократить описание. Ворота, не потолок —
    /// данные не усекаются. См. docs/summary-as-snapshot.md (этап 2).
    pub summary_target_chars: usize,
    /// Авто-рефлексия: запускать фоновую рефлексию каждые N ответов ассистента в
    /// чате (модель сама обновляет «модель себя»). `0` — выключено (по умолчанию).
    /// Срабатывает только в профилях с включёнными инструментами модели себя.
    pub auto_reflect_every: usize,
    /// Авто-консолидация «модели себя» («сон»): запускать фоновую консолидацию каждые
    /// N ответов ассистента в чате (модель сама сливает дубли наблюдений, сжимает
    /// раздутое описание, связывает противоречия). `0` — выключено (по умолчанию).
    /// Отдельная от `auto_reflect_every` (свой тумблер точнее — гейты/данные модели
    /// себя и заметок уже разведены). Срабатывает только в профилях с включёнными
    /// инструментами модели себя. См. docs/history/self-model-consolidation.md (этап A1).
    pub auto_consolidate_every: usize,
    /// Подмешивать ли в системный промпт нейтральный к персоне «протокол ведения
    /// модели» (когда фиксировать изменения, «мимолётное — в наблюдения», «точность
    /// важнее угодливости»). Стабилизирует использование инструментов независимо от
    /// персоны профиля. По умолчанию включён; действует только когда профиль включил
    /// инструменты модели себя.
    pub maintenance_protocol: bool,
}

impl Default for SelfModelSettings {
    fn default() -> Self {
        Self {
            max_narrative: DEFAULT_SELF_MODEL_MAX_NARRATIVE,
            narrative_in_prompt: DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT,
            prompt_cap: DEFAULT_SELF_MODEL_PROMPT_CAP,
            max_closed_goals: DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS,
            summary_target_chars: DEFAULT_SELF_MODEL_SUMMARY_TARGET,
            auto_reflect_every: 0,
            auto_consolidate_every: 0,
            maintenance_protocol: DEFAULT_SELF_MODEL_MAINTENANCE_PROTOCOL,
        }
    }
}

/// Настройки заметок: авто-консолидация («сон»). См. docs/history/notes-connectivity.md (Ярус 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotesSettings {
    /// Авто-консолидация: запускать фоновую «спящую» консолидацию каждые N ответов
    /// ассистента в чате (модель сама сливает дубли / переписывает устаревшее /
    /// связывает родственное). `0` — выключено (по умолчанию). Срабатывает только в
    /// профилях с включёнными инструментами заметок.
    pub auto_consolidate_every: usize,
    /// Показывать ли наблюдения «о себе» (`@self`) в общем `note_recall` — с пометкой
    /// `[о себе]`. По умолчанию **выключено**: память о себе ≠ память о собеседнике
    /// (решение Яруса 1). Тумблер даёт «полное смешение выдачи» (Ярус 3, Путь 2) для
    /// проверки, безопасно ли это; при выключенном self-заметки скрыты, как раньше.
    /// См. docs/history/narrative-as-notes.md (Ярус 3, Путь 2).
    pub recall_includes_self: bool,
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
    /// Спрашивать подтверждение перед перегенерацией (`Ctrl+R`) и удалением
    /// последнего обмена (`Ctrl+E`) — обе операции необратимы в UI. По умолчанию
    /// выключено (комбинации срабатывают сразу). См. spec §11.7.
    pub confirm_destructive_keys: bool,
    /// Режим совместимости со старыми эмуляторами терминала (conhost Windows 10
    /// и т.п.): вместо эмодзи и редких символов Юникода — глифы из безопасного
    /// набора (WGL4/ASCII), прямые рамки вместо скруглённых, ASCII-спиннер,
    /// затемнение фона попапов цветом вместо `DIM`. По умолчанию выключен.
    /// См. spec §11.6 и [`crate::shared::theme::GlyphSet`].
    pub terminal_compat: bool,
    /// Горизонтальные разделители между строками Markdown-таблиц в ленте
    /// (`├───┼───┤`, «сеточный» вид). По умолчанию выключены (компактный вид —
    /// разделитель только под заголовком); включение даёт «сеточный» вид.
    /// См. spec §11.4.
    pub table_row_separators: bool,
    /// Рендерить ```mermaid-блоки ленты диаграммой (Unicode/ASCII-графика,
    /// крейт `mermaid-text`) вместо исходника. Только flowchart/sequence
    /// (whitelist); при любом сбое (не распарсилось / не влезло по ширине / тип
    /// вне whitelist) — жёсткий фолбэк на исходник код-блоком, как при
    /// выключенном тумблере. Благодаря фолбэку по умолчанию **включён** (худший
    /// случай = прежнее поведение). См. spec §11.4 и
    /// docs/research/mermaid-ascii-rendering.md.
    pub render_mermaid: bool,
    /// Язык **интерфейса** (ось B, docs/i18n-ui.md) — тексты для человека
    /// (статус-бар, настройки, справка, заголовки ролей ленты). **Независим** от
    /// языка агентов (`Profile.language`, ось A): русский UI + англоязычные агенты —
    /// законная комбинация. По умолчанию `Ru` (старый `settings.json` без поля);
    /// при свежей установке — из `defaults.json` (`main.rs`). Переиспользует
    /// `i18n::Lang` (UI-язык — это «из какого бандла читать `ui.*`-ключи»).
    pub language: crate::shared::i18n::Lang,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            spellcheck_enabled: true,
            selected_dictionaries: Vec::new(),
            confirm_destructive_keys: false,
            terminal_compat: false,
            table_row_separators: false,
            render_mermaid: true,
            language: crate::shared::i18n::Lang::default(),
        }
    }
}

/// Таймаут одного вызова инструмента MCP-сервера по умолчанию (секунды).
pub const DEFAULT_MCP_TOOL_TIMEOUT_SECS: u64 = 60;
/// Потолок символов результата MCP-инструмента по умолчанию (клип входа в промпт —
/// прецедент Claude Code: cap ~25k токенов). См. docs/research/plugin-system.md §4.4.
pub const DEFAULT_MCP_MAX_RESULT_CHARS: usize = 20_000;

/// Конфигурация одного MCP-сервера (stdio-подпроцесс,
/// docs/research/plugin-system.md §4.4). Серверы добавляются правкой
/// `settings.json` (развилка Р6); UI настроек показывает статусы и тумблеры.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// Короткий идентификатор (slug `[a-z0-9-]`, ≤32) — часть id инструментов
    /// `mcp__<id>__<tool>`. Пустой/невалидный — сервер не запускается.
    pub id: String,
    /// Команда запуска. `.bat`/`.cmd` запрещены (BatBadBut, CVE-2024-24576);
    /// `npx`-серверы на Windows — `cmd /c npx …` либо прямой exe-путь.
    pub command: String,
    /// Аргументы команды.
    pub args: Vec<String>,
    /// Окружение ребёнка: переменная → **имя** переменной-источника в окружении
    /// приложения (сам секрет в `settings.json` не пишется — прецедент
    /// `api_key_env`, развилка Р8). Отсутствующий источник — warn в лог, пропуск.
    pub env: std::collections::BTreeMap<String, String>,
    /// Включён ли сервер (выключенный не запускается, его инструменты недоступны).
    pub enabled: bool,
    /// Таймаут одного вызова инструмента (секунды). Стартовый handshake держит
    /// свой таймаут (константа клиента).
    pub tool_timeout_secs: u64,
    /// Клип результата инструмента (символы) — ограничение входа в промпт.
    pub max_result_chars: usize,
    /// TOFU-пин каталога инструментов (sha256 от имён+описаний+схем): ставится
    /// автоматически при первом подъёме сервера; при **изменении** каталога
    /// (rug-pull-детектор, tool poisoning) инструменты не регистрируются, пока
    /// пользователь не переподтвердит новый каталог в настройках. Пишется
    /// приложением (не редактируется в UI); ручное удаление поля = сброс доверия.
    /// См. docs/research/plugin-system.md §4.5 (Р7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_catalog: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            enabled: true,
            tool_timeout_secs: DEFAULT_MCP_TOOL_TIMEOUT_SECS,
            max_result_chars: DEFAULT_MCP_MAX_RESULT_CHARS,
            pinned_catalog: None,
        }
    }
}

/// Настройки MCP-хоста (плагины-инструменты, docs/research/plugin-system.md §4).
/// Мастер-выключатель **выключен по умолчанию** (как Python): MCP-сервер —
/// произвольная программа с правами пользователя; включение — осознанный opt-in,
/// а инструменты дополнительно opt-in per profile (двойной opt-in, развилка Р7).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Мастер-выключатель MCP-хоста.
    pub enabled: bool,
    /// Список серверов (правится в `settings.json`).
    pub servers: Vec<McpServerConfig>,
}

/// Модель озвучивания OpenAI по умолчанию (актуальная dedicated-TTS, `tts-1*` —
/// легаси). См. docs/research/tts.md §3.1.
pub const DEFAULT_TTS_OPENAI_MODEL: &str = "gpt-4o-mini-tts";
/// Голос OpenAI по умолчанию (рекомендованный в доках наравне с `cedar`).
pub const DEFAULT_TTS_OPENAI_VOICE: &str = "marin";
/// Модель озвучивания Gemini по умолчанию (GA-моделей TTS у Gemini нет — все
/// preview; берём flash: дешевле и есть бесплатный тир). См. docs/research/tts.md §3.2.
pub const DEFAULT_TTS_GEMINI_MODEL: &str = "gemini-2.5-flash-preview-tts";
/// Голос Gemini по умолчанию (из 30 prebuilt-голосов).
pub const DEFAULT_TTS_GEMINI_VOICE: &str = "Kore";

/// Режим озвучивания (TTS) — независимый «серверный слот», как эмбеддинги
/// (ADR 0002): у Anthropic TTS нет вовсе, поэтому провайдер озвучивания
/// конфигурируется отдельно от chat-движка. Локальный сайдкар (`managed`) —
/// этап 2 направления; в селектор он попадёт вместе с реализацией (прецедент:
/// `Claude` не показывали, пока не появился `AnthropicClient`).
/// См. docs/research/tts.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsMode {
    /// Облако OpenAI (`POST /v1/audio/speech`).
    #[default]
    OpenAi,
    /// Облако Google Gemini (нативный `generateContent` c `responseModalities:["AUDIO"]`).
    Gemini,
    /// Любой локальный/сторонний OpenAI-совместимый TTS-сервер (Kokoro-FastAPI,
    /// speaches, LocalAI, …). См. docs/research/tts.md §3.4.
    External,
}

impl TtsMode {
    /// Все варианты в порядке перебора UI (Choice-поле).
    pub const ALL: [TtsMode; 3] = [TtsMode::OpenAi, TtsMode::Gemini, TtsMode::External];

    /// Подпись для UI (Choice-поле).
    pub fn label(self) -> &'static str {
        match self {
            TtsMode::OpenAi => "openai",
            TtsMode::Gemini => "gemini",
            TtsMode::External => "external",
        }
    }

    /// Облачный провайдер режима (`None` — external). Им индексируется общий
    /// сохранённый API-ключ (ADR 0008): ключ, введённый для чата, доступен и TTS.
    pub fn cloud_provider(self) -> Option<CloudProvider> {
        match self {
            TtsMode::OpenAi => Some(CloudProvider::OpenAi),
            TtsMode::Gemini => Some(CloudProvider::Gemini),
            TtsMode::External => None,
        }
    }

    /// Циклический перебор с учётом направления (`dir` = +1/-1).
    pub fn cycle(self, dir: i32) -> Self {
        let idx = Self::ALL.iter().position(|x| *x == self).unwrap_or(0) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(((idx + dir) % n + n) % n) as usize]
    }
}

/// Настройки облачного провайдера озвучивания (OpenAI/Gemini). Хранятся отдельно
/// на каждого, чтобы переключение режима не теряло чужих значений (как
/// [`CloudSettings`] у движка).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsCloudSettings {
    /// Имя TTS-модели у провайдера.
    pub model_name: Option<String>,
    /// Голос (имена свои у каждого провайдера).
    pub voice: Option<String>,
    /// Указания по тону/языку/скорости естественным языком. У OpenAI это поле
    /// `instructions` (и единственный рабочий способ задать скорость —
    /// `speed` у `gpt-4o-mini-tts` де-факто игнорируется); у Gemini — префикс
    /// к тексту запроса. См. docs/research/tts.md §3.
    pub instructions: Option<String>,
    /// Имя env-переменной с API-ключом (фолбэк, если ключ не введён в настройках).
    pub api_key_env: Option<String>,
    /// Переопределение базового URL провайдера (опционально).
    pub url: Option<String>,
}

/// Настройки внешнего (локального/стороннего) OpenAI-совместимого TTS-сервера.
/// Общий знаменатель параметров таких серверов — `model`+`input`+`voice`+
/// `response_format`+`speed`, причём `voice` у каждого свой, а `model` многие
/// игнорируют → шлём только заданное. См. docs/research/tts.md §3.4.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsExternalSettings {
    /// URL сервера (например `http://127.0.0.1:8880/v1`).
    pub url: Option<String>,
    /// Имя модели (опционально — многие серверы игнорируют).
    pub model_name: Option<String>,
    /// Голос — свободное текстовое поле (имена зависят от сервера).
    pub voice: Option<String>,
    /// Имя env-переменной с Bearer-ключом (опционально; локальный сервер не требует).
    pub api_key_env: Option<String>,
}

/// Настройки озвучивания сообщений чата (команда `/tts`, spec §11.9).
/// Всё через `#[serde(default)]` — старые `settings.json` читаются без миграции.
/// См. docs/research/tts.md §8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsSettings {
    /// Провайдер озвучивания.
    pub mode: TtsMode,
    pub openai: TtsCloudSettings,
    pub gemini: TtsCloudSettings,
    pub external: TtsExternalSettings,
    /// Скорость речи (где поддержана). У `gpt-4o-mini-tts` игнорируется — там
    /// скорость просят словами в `instructions`.
    pub speed: f32,
    /// Озвучивать префиксы ролей («Пользователь.»/«Ассистент.») — во **всех**
    /// вариантах команды, включая одиночное `/tts` (решение пользователя, Р6).
    pub speak_roles: bool,
    /// Прерывать озвучивание при переключении чата.
    pub stop_on_chat_switch: bool,
    /// Прерывать озвучивание при начале генерации ответа.
    pub stop_on_generation_start: bool,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            mode: TtsMode::default(),
            openai: TtsCloudSettings {
                model_name: Some(DEFAULT_TTS_OPENAI_MODEL.into()),
                voice: Some(DEFAULT_TTS_OPENAI_VOICE.into()),
                ..Default::default()
            },
            gemini: TtsCloudSettings {
                model_name: Some(DEFAULT_TTS_GEMINI_MODEL.into()),
                voice: Some(DEFAULT_TTS_GEMINI_VOICE.into()),
                ..Default::default()
            },
            external: TtsExternalSettings::default(),
            speed: 1.0,
            speak_roles: false,
            // Прерывать при переключении чата — да; при начале генерации — нет
            // (решение пользователя, Р8).
            stop_on_chat_switch: true,
            stop_on_generation_start: false,
        }
    }
}

impl TtsSettings {
    /// Настройки активного облачного провайдера (`None` — external).
    pub fn cloud(&self) -> Option<&TtsCloudSettings> {
        match self.mode.cloud_provider()? {
            CloudProvider::OpenAi => Some(&self.openai),
            CloudProvider::Gemini => Some(&self.gemini),
            CloudProvider::Claude => None,
        }
    }

    /// Изменяемые настройки активного облачного провайдера (`None` — external).
    pub fn cloud_mut(&mut self) -> Option<&mut TtsCloudSettings> {
        match self.mode.cloud_provider()? {
            CloudProvider::OpenAi => Some(&mut self.openai),
            CloudProvider::Gemini => Some(&mut self.gemini),
            CloudProvider::Claude => None,
        }
    }
}

/// Что включать при копировании всей переписки чата в буфер обмена (`F5`, spec
/// §11.2). По умолчанию копируется только текст сообщений (`Default` — все флаги
/// `false`); опционально добавляются «мысли» (CoT), параметры вызовов инструментов
/// (имя + аргументы) и их результаты.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CopySettings {
    /// Включать блок «мыслей» (CoT) ассистента.
    pub copy_thoughts: bool,
    /// Включать параметры вызовов инструментов (имя инструмента + аргументы).
    pub copy_tool_calls: bool,
    /// Включать результаты (ответы) вызовов инструментов.
    pub copy_tool_results: bool,
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
    /// Настройки «модели себя» (нарратив, объём инъекции в промпт).
    pub self_model: SelfModelSettings,
    /// Настройки заметок (авто-консолидация «сон»).
    pub notes: NotesSettings,
    /// Настройки интерфейса (тема, спелл-чек, словари).
    pub interface: InterfaceSettings,
    /// Что включать при копировании переписки чата в буфер обмена (`F5`).
    pub copy: CopySettings,
    /// MCP-хост: плагины-инструменты через внешние MCP-серверы (stdio).
    pub mcp: McpSettings,
    /// Озвучивание сообщений чата (команда `/tts`, spec §11.9).
    pub tts: TtsSettings,
    /// Последний открытый чат — восстанавливается при следующем запуске. Пишется
    /// оркестратором (не редактируется через экран настроек). `None` — нет памяти
    /// (первый запуск/чат удалён) → открывается самый недавний.
    pub last_active_chat: Option<uuid::Uuid>,
    /// Сохранённые API-ключи облачных провайдеров — **по записи на машину**,
    /// зашифрованы машинным ключом (Windows DPAPI / Linux HKDF(machine-id)+AEAD).
    /// Конфиг остаётся переносимым: чужая запись не расшифруется (ключ вводится
    /// заново своей записью), при возврате на прежнюю машину её запись читается.
    /// Пишется оркестратором (`AppCommand::SetApiKey`), в UI не редактируется —
    /// экран настроек шлёт сам ключ, а не эту структуру. См. `shared::secrets`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<crate::shared::secrets::ApiKeyEntry>,
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
            self_model: SelfModelSettings::default(),
            notes: NotesSettings::default(),
            interface: InterfaceSettings::default(),
            copy: CopySettings::default(),
            mcp: McpSettings::default(),
            tts: TtsSettings::default(),
            last_active_chat: None,
            api_keys: Vec::new(),
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
    fn active_model_name_by_mode() {
        // Managed — базовое имя GGUF без пути и расширения.
        let mut e = EngineSettings {
            mode: ServerMode::Managed,
            managed: ManagedSettings {
                model_path: Some("/models/gemma-4-it.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(e.active_model_name().as_deref(), Some("gemma-4-it"));
        // Не задан путь → None.
        e.managed.model_path = None;
        assert_eq!(e.active_model_name(), None);
        // External — имя модели как есть.
        e.mode = ServerMode::External;
        e.external.model_name = Some("qwen-3.6".into());
        assert_eq!(e.active_model_name().as_deref(), Some("qwen-3.6"));
        // Облако — имя из активной облачной подсекции.
        e.mode = ServerMode::OpenAi;
        e.openai.model_name = Some("gpt-4o".into());
        assert_eq!(e.active_model_name().as_deref(), Some("gpt-4o"));
        // Пустое имя трактуется как незаданное.
        e.openai.model_name = Some(String::new());
        assert_eq!(e.active_model_name(), None);
    }

    #[test]
    fn mcp_server_config_roundtrip_and_partial_defaults() {
        // Частичная запись сервера (как в реальном settings.json) наполняется
        // дефолтами: enabled=true, таймаут/клип — константы.
        let c: AppConfig = serde_json::from_str(
            r#"{"mcp":{"enabled":true,"servers":[{
                "id":"fs","command":"cmd","args":["/c","npx","-y","srv"],
                "env":{"TOKEN":"MINDFORK_FS_TOKEN"}}]}}"#,
        )
        .unwrap();
        assert!(c.mcp.enabled);
        let s = &c.mcp.servers[0];
        assert_eq!(s.id, "fs");
        assert_eq!(s.command, "cmd");
        assert_eq!(s.args, vec!["/c", "npx", "-y", "srv"]);
        assert_eq!(
            s.env.get("TOKEN").map(String::as_str),
            Some("MINDFORK_FS_TOKEN")
        );
        assert!(s.enabled);
        assert_eq!(s.tool_timeout_secs, DEFAULT_MCP_TOOL_TIMEOUT_SECS);
        assert_eq!(s.max_result_chars, DEFAULT_MCP_MAX_RESULT_CHARS);
        // Round-trip: сериализация → чтение даёт то же значение.
        let json = serde_json::to_string(&c.mcp).unwrap();
        let back: McpSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c.mcp);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let c: AppConfig = serde_json::from_str(r#"{"max_tool_rounds":4}"#).unwrap();
        assert_eq!(c.max_tool_rounds, 4);
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        assert_eq!(c.engine.managed.port, 8000);
        assert_eq!(c.engine.managed.gpu_layers, DEFAULT_GPU_LAYERS);
        assert!(c.engine.managed.jinja);
        // Новые секции наполняются дефолтами при их отсутствии в файле.
        assert_eq!(c.embed.managed.port, DEFAULT_EMBED_PORT);
        assert_eq!(c.tools.subagent_max_tokens, DEFAULT_SUBAGENT_MAX_TOKENS);
        assert_eq!(c.tools.subagent_timeout_secs, DEFAULT_SUBAGENT_TIMEOUT_SECS);
        // Файловые инструменты выключены по умолчанию (как Python).
        assert!(!c.tools.fs_enabled);
        assert_eq!(c.tools.fs_root, None);
        // Python: инструмент выключен, режим — песочница Wasmer, сеть в песочнице
        // включена, таймаст песочницы — дефолтный.
        assert!(!c.tools.python_enabled);
        assert_eq!(c.tools.python_mode, PythonMode::Wasmer);
        assert!(c.tools.python_net_enabled);
        assert_eq!(
            c.tools.python_wasm_timeout_secs,
            DEFAULT_PYTHON_WASM_TIMEOUT_SECS
        );
        // Лимит памяти песочницы по умолчанию отключён (opt-in, только Windows).
        assert_eq!(c.tools.python_wasm_memory_mb, None);
        assert_eq!(c.rag.chunk_target_chars, DEFAULT_CHUNK_TARGET_CHARS);
        assert_eq!(c.self_model.max_narrative, DEFAULT_SELF_MODEL_MAX_NARRATIVE);
        assert_eq!(
            c.self_model.narrative_in_prompt,
            DEFAULT_SELF_MODEL_NARRATIVE_IN_PROMPT
        );
        assert_eq!(c.self_model.prompt_cap, DEFAULT_SELF_MODEL_PROMPT_CAP);
        assert_eq!(
            c.self_model.max_closed_goals,
            DEFAULT_SELF_MODEL_MAX_CLOSED_GOALS
        );
        assert_eq!(
            c.self_model.summary_target_chars,
            DEFAULT_SELF_MODEL_SUMMARY_TARGET
        );
        // Протокол ведения модели себя включён по умолчанию, авто-рефлексия и
        // авто-консолидация «модели себя» — нет.
        assert!(c.self_model.maintenance_protocol);
        assert_eq!(c.self_model.auto_reflect_every, 0);
        assert_eq!(c.self_model.auto_consolidate_every, 0);
        // Заметки: авто-консолидация выкл, self-заметки в recall скрыты (Ярус 3, Путь 2).
        assert_eq!(c.notes.auto_consolidate_every, 0);
        assert!(!c.notes.recall_includes_self);
        assert_eq!(c.rag.chunk_overlap_chars, DEFAULT_CHUNK_OVERLAP_CHARS);
        assert_eq!(c.rag.chunk_max_chars, DEFAULT_CHUNK_MAX_CHARS);
        assert!(c.interface.spellcheck_enabled);
        assert_eq!(c.interface.theme, Theme::Auto);
        // Режим совместимости со старым терминалом по умолчанию выключен.
        assert!(!c.interface.terminal_compat);
        // Разделители строк Markdown-таблиц по умолчанию выключены.
        assert!(!c.interface.table_row_separators);
        // Рендер mermaid-диаграмм по умолчанию включён (жёсткий фолбэк на исходник
        // делает включение безопасным: худший случай = прежнее поведение).
        assert!(c.interface.render_mermaid);
        // Язык интерфейса (ось B) по умолчанию — русский (старый settings.json без
        // поля; свежая установка ставит его из defaults.json в main.rs).
        assert_eq!(c.interface.language, crate::shared::i18n::Lang::Ru);
        // Копирование переписки: по умолчанию только текст (все флаги выключены).
        assert!(!c.copy.copy_thoughts);
        assert!(!c.copy.copy_tool_calls);
        assert!(!c.copy.copy_tool_results);
        // MCP-хост: мастер-выключатель выкл, серверов нет (двойной opt-in, Р7).
        assert!(!c.mcp.enabled);
        assert!(c.mcp.servers.is_empty());
        // Озвучивание (TTS): режим по умолчанию — OpenAI с осмысленными моделью и
        // голосом («не настроено» = нет ключа), скорость 1.0; из поведения включено
        // только прерывание при переключении чата (Р6/Р8).
        assert_eq!(c.tts.mode, TtsMode::OpenAi);
        assert_eq!(
            c.tts.openai.model_name.as_deref(),
            Some(DEFAULT_TTS_OPENAI_MODEL)
        );
        assert_eq!(
            c.tts.openai.voice.as_deref(),
            Some(DEFAULT_TTS_OPENAI_VOICE)
        );
        assert_eq!(
            c.tts.gemini.model_name.as_deref(),
            Some(DEFAULT_TTS_GEMINI_MODEL)
        );
        assert_eq!(c.tts.speed, 1.0);
        assert!(!c.tts.speak_roles);
        assert!(c.tts.stop_on_chat_switch);
        assert!(!c.tts.stop_on_generation_start);
        // Имперсонация наполняется дефолтами при отсутствии в файле.
        assert_eq!(c.impersonation_engine.mode, ImpersonationMode::Shared);
        assert_eq!(
            c.impersonation_engine.managed.port,
            DEFAULT_IMPERSONATION_PORT
        );
        assert_eq!(c.impersonation_sampling.thinking, Some(false));
        // Память о последнем открытом чате: по умолчанию пусто.
        assert_eq!(c.last_active_chat, None);
    }

    #[test]
    fn impersonation_sections_roundtrip() {
        let c = AppConfig {
            impersonation_engine: ImpersonationEngineSettings {
                mode: ImpersonationMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("persona.gguf".into()),
                    port: 8002,
                    ..Default::default()
                },
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
    fn impersonation_managed_default_port_is_8002() {
        let s = ImpersonationEngineSettings::default();
        assert_eq!(s.managed.port, DEFAULT_IMPERSONATION_PORT);
    }

    #[test]
    fn flash_attn_arg_and_cycle() {
        assert_eq!(FlashAttn::default(), FlashAttn::Auto);
        assert_eq!(FlashAttn::Auto.as_arg(), None);
        assert_eq!(FlashAttn::On.as_arg(), Some("on"));
        assert_eq!(FlashAttn::Off.as_arg(), Some("off"));
        // Перебор по кругу в обе стороны.
        assert_eq!(FlashAttn::Auto.cycle(1), FlashAttn::On);
        assert_eq!(FlashAttn::Auto.cycle(-1), FlashAttn::Off);
    }

    #[test]
    fn python_mode_default_cycle_and_serde() {
        assert_eq!(PythonMode::default(), PythonMode::Wasmer);
        // Перебор по кругу (два варианта).
        assert_eq!(PythonMode::Wasmer.cycle(1), PythonMode::Local);
        assert_eq!(PythonMode::Local.cycle(1), PythonMode::Wasmer);
        assert_eq!(PythonMode::Wasmer.cycle(-1), PythonMode::Local);
        // serde — lowercase, round-trip.
        assert_eq!(
            serde_json::to_string(&PythonMode::Local).unwrap(),
            "\"local\""
        );
        let m: PythonMode = serde_json::from_str("\"wasmer\"").unwrap();
        assert_eq!(m, PythonMode::Wasmer);
        assert!(PythonMode::ALL.contains(&PythonMode::Local));
    }

    #[test]
    fn spec_type_arg_serde_and_draft_need() {
        assert_eq!(SpecType::default(), SpecType::None);
        assert_eq!(SpecType::None.as_arg(), None);
        assert_eq!(SpecType::DraftMtp.as_arg(), Some("draft-mtp"));
        assert_eq!(SpecType::NgramMapK4v.as_arg(), Some("ngram-map-k4v"));
        // serde-имя совпадает с CLI-значением (kebab-case).
        assert_eq!(
            serde_json::to_string(&SpecType::DraftMtp).unwrap(),
            "\"draft-mtp\""
        );
        assert_eq!(
            serde_json::to_string(&SpecType::NgramMapK4v).unwrap(),
            "\"ngram-map-k4v\""
        );
        // Черновая модель нужна только типам draft-*.
        assert!(SpecType::DraftMtp.needs_draft_model());
        assert!(!SpecType::NgramSimple.needs_draft_model());
        assert!(!SpecType::None.needs_draft_model());
    }

    #[test]
    fn managed_spec_fields_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::Managed,
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("mtp-gemma-4-12B-it.gguf".into()),
                    flash_attn: FlashAttn::On,
                    spec_type: SpecType::DraftMtp,
                    draft_model: Some("mtp-gemma-4-12B-it.gguf".into()),
                    draft_gpu_layers: Some(99),
                    draft_n_max: Some(5),
                    draft_n_min: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn per_provider_cloud_settings_are_independent() {
        // Каждый провайдер хранит свои поля — переключение режима не теряет чужих.
        let mut e = EngineSettings {
            mode: ServerMode::OpenAi,
            openai: CloudSettings {
                model_name: Some("gpt-4o".into()),
                api_key_env: Some("OPENAI_API_KEY".into()),
                ..Default::default()
            },
            gemini: CloudSettings {
                model_name: Some("gemini-2.5-pro".into()),
                api_key_env: Some("GEMINI_API_KEY".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Активный провайдер — OpenAI.
        assert_eq!(e.cloud().unwrap().model_name.as_deref(), Some("gpt-4o"));
        // Переключение на Gemini открывает его собственные поля, OpenAI цел.
        e.mode = ServerMode::Gemini;
        assert_eq!(
            e.cloud().unwrap().model_name.as_deref(),
            Some("gemini-2.5-pro")
        );
        assert_eq!(e.openai.model_name.as_deref(), Some("gpt-4o"));
        // Локальные режимы — без облака.
        e.mode = ServerMode::Managed;
        assert!(e.cloud().is_none());
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
                managed: ManagedSettings {
                    binary: Some("llama-server".into()),
                    model_path: Some("gemma.gguf".into()),
                    gpu_layers: 50,
                    context_size: 4096,
                    jinja: true,
                    reasoning_format: Some("auto".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            embed: EmbedSettings {
                mode: ServerMode::External,
                external: ExternalSettings {
                    url: Some("http://127.0.0.1:8001/v1".into()),
                    ..Default::default()
                },
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
                confirm_destructive_keys: true,
                terminal_compat: true,
                table_row_separators: true,
                render_mermaid: false,
                language: crate::shared::i18n::Lang::Ru,
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

    #[test]
    fn cloud_modes_serialize_and_map_to_provider() {
        assert_eq!(
            serde_json::to_string(&ServerMode::OpenAi).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&ServerMode::Gemini).unwrap(),
            "\"gemini\""
        );
        assert_eq!(
            ServerMode::OpenAi.cloud_provider(),
            Some(CloudProvider::OpenAi)
        );
        assert_eq!(
            ServerMode::Gemini.cloud_provider(),
            Some(CloudProvider::Gemini)
        );
        assert_eq!(ServerMode::Managed.cloud_provider(), None);
        assert_eq!(ServerMode::External.cloud_provider(), None);
        // Claude — облако, но не OpenAI-протокол.
        assert_eq!(
            serde_json::to_string(&ServerMode::Claude).unwrap(),
            "\"claude\""
        );
        assert_eq!(
            ServerMode::Claude.cloud_provider(),
            Some(CloudProvider::Claude)
        );
        assert!(CloudProvider::Claude.base_url().contains("anthropic"));
        // Имперсонация: те же облачные провайдеры, прочие режимы — None.
        assert_eq!(
            ImpersonationMode::OpenAi.cloud_provider(),
            Some(CloudProvider::OpenAi)
        );
        assert_eq!(ImpersonationMode::Shared.cloud_provider(), None);
        // Base URL провайдеров.
        assert!(
            CloudProvider::OpenAi
                .base_url()
                .starts_with("https://api.openai.com")
        );
        assert!(
            CloudProvider::Gemini
                .base_url()
                .contains("generativelanguage")
        );
    }

    #[test]
    fn cloud_engine_settings_roundtrip() {
        let c = AppConfig {
            engine: EngineSettings {
                mode: ServerMode::OpenAi,
                openai: CloudSettings {
                    model_name: Some("gpt-4o".into()),
                    api_key_env: Some("OPENAI_API_KEY".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        // Под-секции наполняются дефолтами при отсутствии в файле.
        let old: AppConfig = serde_json::from_str(r#"{"engine":{"mode":"managed"}}"#).unwrap();
        assert_eq!(old.engine.openai.model_name, None);
        assert_eq!(old.engine.openai.api_key_env, None);
        assert_eq!(old.engine.managed.binary, None);
    }
}
