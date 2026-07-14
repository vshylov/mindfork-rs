//! Контракт системы инструментов (`features/tools`): трейт [`Tool`], снимок
//! [`ToolContext`], результат [`ToolOutcome`] с эффектами [`ChatEffect`] и
//! реестр [`ToolRegistry`]. См. spec §9.2, §6.3.
//!
//! Инструменты **не** мутируют `Chat` напрямую: мутирующие возвращают `effects`,
//! которые применяет оркестратор (единственный владелец `Chat`, spec §4.4.2).
//! Инструменты памяти/знаний обязаны фильтровать по `ctx.profile_id` (изоляция,
//! инвариант репозиториев, spec §9.5).

pub mod calc;
pub mod control;
pub mod datetime;
pub mod fetch;
pub mod fs;
pub mod introspection;
pub mod meta;
pub mod notes;
pub mod present;
pub mod python;
pub mod rag;
pub mod self_model;
pub mod subagent;
pub mod web;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::entities::sampling::{SamplingConfig, supported_sampling_fields};
use crate::entities::self_model::SelfModelParams;
use crate::shared::api::{Embedder, EngineBackend, ToolSchema};
use crate::shared::config::{AppConfig, CloudProvider, PythonMode};
use crate::shared::sandbox::WasmerSandbox;
use crate::shared::storage::Storage;

pub use introspection::{GET_SAMPLING_ID, SET_SAMPLING_ID};

/// Неизменяемый снимок состояния хода (без разделяемых локов). См. spec §9.2.
#[derive(Clone)]
pub struct ToolContext {
    pub profile_id: Uuid,
    /// Id текущего чата (часть снимка хода; доступен инструментам).
    #[allow(dead_code)]
    pub chat_id: Uuid,
    /// Снимок `Chat.system_message` на начало хода.
    pub system_message: String,
    /// Действующий семплинг (после приоритетов §8.3).
    pub effective_sampling: SamplingConfig,
    /// Время последнего user-сообщения (если есть).
    pub last_user_message_at: Option<DateTime<Utc>>,
    pub storage: Arc<Storage>,
    /// Chat-движок (для `call_subagent`, M6).
    pub engine: Arc<dyn EngineBackend>,
    /// Источник эмбеддингов (RAG); выделенный сервер — см. ADR 0002.
    pub embedder: Arc<dyn Embedder>,
    /// Параметры чанкинга RAG из настроек (`config.rag`, spec §9.3).
    pub chunk_params: rag::ChunkParams,
    /// Параметры рендера/хранения «модели себя» из настроек (`config.self_model`).
    /// Снимок самой модели в контекст **не** кладётся: инструменты SelfModel
    /// читают/пишут свежее состояние напрямую через `storage` (чтобы видеть правки
    /// внутри хода), а инъекция модели в промпт — отдельным путём в оркестраторе.
    pub self_model_params: SelfModelParams,
    /// Показывать ли self-заметки (`@self`) в общем `note_recall` (Ярус 3, Путь 2).
    /// Из `config.notes.recall_includes_self`; по умолчанию `false` (self скрыты).
    pub recall_includes_self: bool,
    /// Язык **служебного каркаса** для этого хода (из `Profile.language`, ось A,
    /// docs/history/i18n.md). Тексты, которые читает модель (каркас «модели себя», результаты
    /// инструментов), локализуются им. `&'static` — вшитый бандл.
    pub loc: &'static crate::shared::i18n::Locale,
}

/// Долгоживущие разделяемые зависимости инструментов (пучок `Arc`; меняется при
/// рестарте серверов, не от хода к ходу). Собирается в один блок, чтобы новая
/// зависимость не правила каждый сайт сборки [`ToolContext`]. См.
/// docs/history/refactoring-solid.md §3.
#[derive(Clone)]
pub struct ToolDeps {
    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,
    pub embedder: Arc<dyn Embedder>,
}

/// Параметры инструментов из конфига (снимок на ход). Единственное место маппинга
/// `AppConfig` → параметры инструментов — [`ToolParams::from_config`].
#[derive(Clone)]
pub struct ToolParams {
    pub chunk_params: rag::ChunkParams,
    pub self_model_params: SelfModelParams,
    pub recall_includes_self: bool,
}

impl ToolParams {
    /// Снимает параметры инструментов из конфигурации приложения.
    pub fn from_config(cfg: &AppConfig) -> Self {
        Self {
            chunk_params: rag::ChunkParams::from_settings(&cfg.rag),
            self_model_params: SelfModelParams::from_settings(&cfg.self_model),
            recall_includes_self: cfg.notes.recall_includes_self,
        }
    }
}

/// Снимок хода: что инструмент видит о текущем чате (идентичность + снимок `Chat`).
pub struct TurnInfo {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: Option<DateTime<Utc>>,
    /// Язык служебного каркаса хода (из `Profile.language`, ось A).
    pub lang: crate::shared::i18n::Lang,
}

impl ToolContext {
    /// Разворачивает строительные блоки в прежние плоские поля. Плоская форма
    /// сохранена сознательно — код инструментов (`ctx.storage`, `ctx.chunk_params`,
    /// …) не меняется. См. docs/history/refactoring-solid.md §3.
    pub fn new(deps: ToolDeps, params: ToolParams, turn: TurnInfo) -> Self {
        Self {
            profile_id: turn.profile_id,
            chat_id: turn.chat_id,
            system_message: turn.system_message,
            effective_sampling: turn.effective_sampling,
            last_user_message_at: turn.last_user_message_at,
            storage: deps.storage,
            engine: deps.engine,
            embedder: deps.embedder,
            chunk_params: params.chunk_params,
            self_model_params: params.self_model_params,
            recall_includes_self: params.recall_includes_self,
            loc: crate::shared::i18n::locale(turn.lang),
        }
    }
}

/// Эффект, изменяющий `Chat`; возвращается инструментом, применяется оркестратором.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEffect {
    /// Заменить системное сообщение чата (действует со следующего построения запроса).
    SetSystemMessage(String),
    /// Заменить override семплинга чата (действует со следующего хода).
    /// `Box`, т.к. `SamplingConfig` крупнее прочих вариантов (clippy
    /// `large_enum_variant`).
    SetSamplingOverride(Box<SamplingConfig>),
}

/// Результат вызова инструмента: строка для модели + эффекты для оркестратора.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub result: String,
    pub effects: Vec<ChatEffect>,
}

impl ToolOutcome {
    /// Результат без эффектов (чистый инструмент).
    pub fn text(result: impl Into<String>) -> Self {
        Self {
            result: result.into(),
            effects: Vec::new(),
        }
    }

    /// Результат с эффектами (мутирующий инструмент).
    pub fn with_effects(result: impl Into<String>, effects: Vec<ChatEffect>) -> Self {
        Self {
            result: result.into(),
            effects,
        }
    }
}

/// Инструмент, исполняемый клиентским agentic-loop. См. spec §9.2.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Уникальное имя (совпадает с именем функции в OpenAI-схеме).
    fn id(&self) -> ToolId;
    /// Человекочитаемое описание для модели **на языке служебного каркаса** `loc`
    /// (ось A, docs/history/i18n.md). Инструменты, ещё не переведённые (Ярус 2 идёт по
    /// группам), возвращают русский текст независимо от `loc`.
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String;
    /// JSON Schema объекта параметров (описания полей — на языке `loc`).
    fn parameters(&self, loc: &crate::shared::i18n::Locale) -> serde_json::Value;
    /// Исполняет вызов. `args` — распарсенный JSON аргументов модели.
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome>;

    /// Схема для передачи серверу (по умолчанию из `id`/`description`/`parameters`)
    /// на языке `loc`.
    fn schema(&self, loc: &crate::shared::i18n::Locale) -> ToolSchema {
        ToolSchema {
            name: self.id(),
            description: self.description(loc),
            parameters: self.parameters(loc),
        }
    }

    /// Смысловая группа для тумблеров профиля (UI настроек).
    fn group(&self) -> meta::ToolGroup;

    /// Короткий (2–4 слова) лейбл для тумблера профиля (в отличие от
    /// LLM-ориентированного [`Tool::description`]).
    fn ui_label(&self) -> &'static str;

    /// Глобальный выключатель, гейтящий инструмент (`None` — негейтимый). См.
    /// [`effective_tool_ids`].
    fn gate(&self) -> Option<meta::ToolGate> {
        None
    }

    /// Включён ли инструмент в профиле по умолчанию (`false` — опциональный,
    /// включается вручную). См. [`default_tool_ids`]/[`all_tool_ids`].
    fn enabled_by_default(&self) -> bool {
        true
    }
}

/// Имя web-инструмента (гейтится глобальным выключателем `tools.web_enabled`).
pub const WEB_SEARCH_ID: &str = "web_search";
/// Имя инструмента загрузки URL (гейтится `tools.web_enabled` — сетевой доступ).
pub const FETCH_URL_ID: &str = "fetch_url";
/// Имя Python-инструмента (гейтится `tools.python_enabled`).
pub const PYTHON_EXEC_ID: &str = "python_exec";

/// Снимок метаданных всех инструментов (единый источник — сами инструменты через
/// трейт [`Tool`]). Метаданные (группа/лейбл/гейт/дефолт) не зависят от
/// [`ToolConfig`], поэтому каталог строится один раз на дефолтном конфиге — это
/// избавляет от пересборки реестра в горячем [`effective_tool_ids`] (зовётся на
/// каждый раунд agentic-loop).
static CATALOG: LazyLock<Vec<meta::ToolInfo>> =
    LazyLock::new(|| standard_registry(&ToolConfig::default()).infos());

/// Каталог метаданных всех известных инструментов (снимок [`CATALOG`]). Порядок —
/// алфавитный по id (реестр — `BTreeMap`). Потребители используют id по значению
/// (членство/итерация), не по позиции.
pub fn tool_catalog() -> Vec<meta::ToolInfo> {
    CATALOG.clone()
}

/// Идентификаторы инструментов, включаемых в профиле по умолчанию (M5–M7).
/// Внешние (`web_search`/`python_exec`) дополнительно гейтятся глобальными
/// выключателями — см. [`effective_tool_ids`]. Управляющие инструменты беседы и
/// «модель себя» опциональны (по умолчанию выкл, `Tool::enabled_by_default`), см.
/// [`all_tool_ids`]. Выводится из [`CATALOG`] (единый источник — сами инструменты).
pub fn default_tool_ids() -> Vec<ToolId> {
    CATALOG
        .iter()
        .filter(|i| i.enabled_by_default)
        .map(|i| i.id.clone())
        .collect()
}

/// Полный каталог id инструментов для тумблеров профиля: дефолтные + опциональные
/// (по умолчанию выключенные — управляющие инструменты беседы и «модель себя»). В
/// отличие от [`default_tool_ids`], сюда входят опциональные — так пользователь
/// видит их в настройках профиля и может включить, но `reconcile_tools` их **не**
/// включает автоматически. Выводится из [`CATALOG`]. См. spec §9.3.
// Каталог тумблеров профиля берёт метаданные через [`tool_catalog`]; этот
// id-хелпер сейчас используют тесты (фикстуры/каталог) — оставлен как публичный API.
#[allow(dead_code)]
pub fn all_tool_ids() -> Vec<ToolId> {
    CATALOG.iter().map(|i| i.id.clone()).collect()
}

/// Эффективный набор инструментов: `enabled` минус внешние, отключённые
/// глобальными выключателями (spec §9.4). Порядок `enabled` сохраняется.
/// `web_enabled` гейтит и `web_search`, и `fetch_url` (оба — сетевой доступ);
/// `fs_enabled` — файловые `fs_read`/`fs_write`/`fs_list` (гейт берётся из метаданных
/// инструмента, [`Tool::gate`]). Инструменты семплинга (`get_sampling`/`set_sampling`)
/// отключаются, если в текущем режиме движка нет ни одного доступного параметра
/// (`sampling_provider`, см. [`supported_sampling_fields`]) — это динамический гейт по
/// провайдеру, поэтому обрабатывается отдельно от статических [`meta::ToolGate`].
pub fn effective_tool_ids(
    enabled: &[ToolId],
    web_enabled: bool,
    python_enabled: bool,
    fs_enabled: bool,
    sampling_provider: Option<CloudProvider>,
) -> Vec<ToolId> {
    let sampling_available = !supported_sampling_fields(sampling_provider).is_empty();
    let gate_of = |id: &str| CATALOG.iter().find(|i| i.id == id).and_then(|i| i.gate);
    enabled
        .iter()
        .filter(|id| {
            if id.as_str() == GET_SAMPLING_ID || id.as_str() == SET_SAMPLING_ID {
                return sampling_available;
            }
            match gate_of(id) {
                Some(meta::ToolGate::Web) => web_enabled,
                Some(meta::ToolGate::Python) => python_enabled,
                Some(meta::ToolGate::Fs) => fs_enabled,
                None => true,
            }
        })
        .cloned()
        .collect()
}

/// Параметры построения реестра инструментов из конфигурации (`config.tools`,
/// spec §11.6). Позволяют пересобирать реестр при правках настроек (live).
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Режим исполнения `python_exec` (песочница Wasmer / локальный интерпретатор).
    pub python_mode: PythonMode,
    /// Путь к интерпретатору Python для `python_exec` (`None` → системный, режим Local).
    pub python_path: Option<String>,
    /// Разрешить сеть в песочнице Wasmer (`--net`).
    pub python_net: bool,
    /// Таймаут исполнения в песочнице Wasmer.
    pub python_wasm_timeout: Duration,
    /// Жёсткий лимит памяти песочницы (МБ; `None` — без лимита). Только Windows.
    pub python_wasm_memory_mb: Option<u64>,
    /// Каталог песочницы (`data/sandbox/`) с бинарём `wasmer` и ассетами (`None` —
    /// нет каталога, песочница только через env-override).
    pub sandbox_dir: Option<PathBuf>,
    /// Лимит токенов ответа `call_subagent`.
    pub subagent_max_tokens: usize,
    /// Лимит времени на вызов `call_subagent`.
    pub subagent_timeout: Duration,
    /// Значение по умолчанию для `web_search.fetch_content` (загрузка/реранк страниц,
    /// `config.tools.web_fetch_content`). Аргумент вызова переопределяет.
    pub web_fetch_content: bool,
    /// Каталог-«песочница» для файловых инструментов (`None` → без ограничения).
    pub fs_root: Option<String>,
    /// Облачный провайдер chat-движка (`None` — локальный/external). Определяет,
    /// какие параметры семплинга видят/меняют `get_sampling`/`set_sampling` (схема и
    /// фильтрация результата) — зеркало wire-диалекта. См. ADR 0004.
    pub sampling_provider: Option<CloudProvider>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            python_mode: PythonMode::default(),
            python_path: None,
            python_net: true,
            python_wasm_timeout: Duration::from_secs(
                crate::shared::config::DEFAULT_PYTHON_WASM_TIMEOUT_SECS,
            ),
            python_wasm_memory_mb: None,
            sandbox_dir: None,
            subagent_max_tokens: crate::shared::config::DEFAULT_SUBAGENT_MAX_TOKENS,
            subagent_timeout: Duration::from_secs(
                crate::shared::config::DEFAULT_SUBAGENT_TIMEOUT_SECS,
            ),
            web_fetch_content: true,
            fs_root: None,
            sampling_provider: None,
        }
    }
}

/// Реестр со всеми инструментами (M5–M7) по параметрам [`ToolConfig`]. Глобальные
/// выключатели применяются не здесь, а при отборе эффективного набора (см.
/// [`effective_tool_ids`]).
pub fn standard_registry(cfg: &ToolConfig) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(introspection::GetSampling::new(
        cfg.sampling_provider,
    )));
    reg.register(Arc::new(introspection::SetSampling::new(
        cfg.sampling_provider,
    )));
    reg.register(Arc::new(introspection::GetSystemMessage));
    reg.register(Arc::new(introspection::SetSystemMessage));
    reg.register(Arc::new(introspection::GetLastUserMessageTime));
    reg.register(Arc::new(notes::NoteSave));
    reg.register(Arc::new(notes::NoteRecall));
    reg.register(Arc::new(notes::NoteRevise));
    reg.register(Arc::new(notes::NoteLink));
    reg.register(Arc::new(notes::NoteNeighbors));
    reg.register(Arc::new(notes::NoteSupersede));
    reg.register(Arc::new(notes::NoteMerge));
    reg.register(Arc::new(notes::ConsolidateNotes));
    reg.register(Arc::new(notes::NoteCiteSource));
    reg.register(Arc::new(rag::RagAdd));
    reg.register(Arc::new(rag::RagSearch));
    reg.register(Arc::new(subagent::CallSubagent::new(
        cfg.subagent_max_tokens,
        cfg.subagent_timeout,
    )));
    reg.register(Arc::new(web::WebSearch::new(cfg.web_fetch_content)));
    reg.register(Arc::new(fetch::FetchUrl::new()));
    reg.register(Arc::new(python::PythonExec::new(
        cfg.python_mode,
        cfg.python_path.clone(),
        Arc::new(
            WasmerSandbox::new(cfg.sandbox_dir.clone())
                .with_memory_limit(cfg.python_wasm_memory_mb),
        ),
        cfg.python_net,
        cfg.python_wasm_timeout,
    )));
    reg.register(Arc::new(calc::Calculate));
    reg.register(Arc::new(datetime::CurrentTime));
    reg.register(Arc::new(fs::FsRead::new(cfg.fs_root.clone())));
    reg.register(Arc::new(fs::FsWrite::new(cfg.fs_root.clone())));
    reg.register(Arc::new(fs::FsList::new(cfg.fs_root.clone())));
    // Управляющие инструменты беседы (опциональны, гейтятся набором профиля).
    reg.register(Arc::new(control::SendFollowupMessage));
    reg.register(Arc::new(control::RewriteCurrentMessage));
    // Инструменты «модели себя» (опциональны, DB-only, гейтятся набором профиля).
    reg.register(Arc::new(self_model::GetSelfModel));
    reg.register(Arc::new(self_model::Reflect));
    reg.register(Arc::new(self_model::UpdateSelfModel));
    reg.register(Arc::new(self_model::UpdateUserModel));
    reg.register(Arc::new(self_model::AddInsight));
    reg
}

/// Реестр инструментов: связывает имена с реализациями, отдаёт схемы движку.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Регистрирует инструмент (перезаписывает при совпадении id).
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.id(), tool);
    }

    /// Инструмент по имени (используется тестами реестра).
    #[allow(dead_code)]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(id)
    }

    /// Снимок метаданных всех зарегистрированных инструментов (для каталога UI).
    pub fn infos(&self) -> Vec<meta::ToolInfo> {
        self.tools
            .values()
            .map(|t| meta::ToolInfo {
                id: t.id(),
                group: t.group(),
                label: t.ui_label(),
                gate: t.gate(),
                enabled_by_default: t.enabled_by_default(),
            })
            .collect()
    }

    /// Схемы для подмножества включённых инструментов (профиль ∩ глобально), с
    /// сохранением порядка `enabled`. Неизвестные имена игнорируются.
    pub fn schemas_for(
        &self,
        enabled: &[ToolId],
        loc: &crate::shared::i18n::Locale,
    ) -> Vec<ToolSchema> {
        enabled
            .iter()
            .filter_map(|id| self.tools.get(id))
            .map(|t| t.schema(loc))
            .collect()
    }

    /// Исполняет инструмент по имени. Ошибка, если инструмент неизвестен.
    pub async fn invoke(
        &self,
        id: &str,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolOutcome> {
        match self.tools.get(id) {
            Some(tool) => tool.invoke(ctx, args).await,
            None => anyhow::bail!("неизвестный инструмент: {id}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Утилиты для тестов инструментов: построение [`ToolContext`] на временном
    //! хранилище с mock-движком и детерминированным эмбеддером.

    use super::*;
    use crate::shared::api::mock::{MockBackend, MockEmbedder};
    use crate::shared::paths::Paths;

    /// Дефолтные параметры инструментов для тестов.
    fn test_params() -> ToolParams {
        ToolParams {
            chunk_params: rag::ChunkParams::default(),
            self_model_params: SelfModelParams::default(),
            recall_includes_self: false,
        }
    }

    /// Дефолтный снимок хода для тестов (профиль задан, чат — новый).
    fn test_turn(profile_id: Uuid) -> TurnInfo {
        TurnInfo {
            profile_id,
            chat_id: Uuid::new_v4(),
            system_message: "системное сообщение".into(),
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: None,
            lang: crate::shared::i18n::Lang::Ru,
        }
    }

    /// Контекст инструмента поверх временного хранилища. Возвращает также
    /// `TempDir` (держать живым) и `Arc<Storage>` (для проверок в тесте).
    pub fn ctx_with_storage(profile_id: Uuid) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        ctx_with_storage_lang(profile_id, crate::shared::i18n::Lang::Ru)
    }

    /// Как [`ctx_with_storage`], но с явным языком каркаса (для проверки локализации
    /// результатов инструментов — напр. `python_exec` на en-профиле).
    pub fn ctx_with_storage_lang(
        profile_id: Uuid,
        lang: crate::shared::i18n::Lang,
    ) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let deps = ToolDeps {
            storage: storage.clone(),
            engine: Arc::new(MockBackend::scripted(vec![])),
            embedder: Arc::new(MockEmbedder::new(16)),
        };
        let mut turn = test_turn(profile_id);
        turn.lang = lang;
        let ctx = ToolContext::new(deps, test_params(), turn);
        (dir, storage, ctx)
    }

    /// Контекст инструмента поверх готового пучка зависимостей (тесты, где
    /// несколько контекстов делят одно хранилище — напр. изоляция по профилю).
    pub fn ctx_with_deps(profile_id: Uuid, deps: ToolDeps) -> ToolContext {
        ToolContext::new(deps, test_params(), test_turn(profile_id))
    }

    /// Контекст инструмента с кастомными движком/эмбеддером (тесты web/subagent/
    /// rag/fetch), поверх временного хранилища.
    pub fn ctx_with_backends(
        profile_id: Uuid,
        engine: Arc<dyn EngineBackend>,
        embedder: Arc<dyn Embedder>,
    ) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let deps = ToolDeps {
            storage: storage.clone(),
            engine,
            embedder,
        };
        let ctx = ToolContext::new(deps, test_params(), test_turn(profile_id));
        (dir, storage, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn id(&self) -> ToolId {
            "echo".into()
        }
        fn description(&self, _loc: &crate::shared::i18n::Locale) -> String {
            "Возвращает аргумент text".into()
        }
        fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            })
        }
        async fn invoke(&self, _ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
            let text = args["text"].as_str().unwrap_or_default();
            Ok(ToolOutcome::text(text))
        }
        fn group(&self) -> meta::ToolGroup {
            meta::ToolGroup::Utils
        }
        fn ui_label(&self) -> &'static str {
            "эхо"
        }
    }

    #[tokio::test]
    async fn registry_invokes_registered_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        let out = reg
            .invoke("echo", &ctx, serde_json::json!({"text": "hi"}))
            .await
            .unwrap();
        assert_eq!(out.result, "hi");
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn registry_unknown_tool_errors() {
        let reg = ToolRegistry::new();
        let (_d, _s, ctx) = testkit::ctx_with_storage(Uuid::new_v4());
        assert!(
            reg.invoke("nope", &ctx, serde_json::json!({}))
                .await
                .is_err()
        );
    }

    #[test]
    fn standard_registry_has_all_default_tools() {
        let reg = standard_registry(&ToolConfig::default());
        // Реестр содержит весь каталог — и дефолтные, и опциональные управляющие.
        for id in all_tool_ids() {
            assert!(reg.get(&id).is_some(), "инструмент {id} не зарегистрирован");
        }
        // Схемы для полного каталога покрывают все id.
        assert_eq!(
            reg.schemas_for(
                &all_tool_ids(),
                crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
            )
            .len(),
            all_tool_ids().len()
        );
    }

    #[test]
    fn all_tool_descriptions_localized_to_en() {
        // Сильный гейт (§3.5 docs/history/i18n.md): описание КАЖДОГО инструмента на en не
        // содержит кириллицы и отличается от ru — ловит забытый `_loc` в любой группе.
        use crate::shared::i18n::{Lang, locale};
        let reg = standard_registry(&ToolConfig::default());
        let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
        for id in all_tool_ids() {
            let t = reg.get(&id).expect("инструмент в реестре");
            let d_en = t.description(en);
            assert!(
                !d_en
                    .chars()
                    .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
                "{id}: кириллица в en-описании: {d_en}"
            );
            assert_ne!(t.description(ru), d_en, "{id}: описание не локализовано");
        }
    }

    #[test]
    fn note_revise_is_default_tool() {
        // Ревизия заметки — центральна для интеграции, включена по умолчанию.
        assert!(
            default_tool_ids()
                .iter()
                .any(|t| t == notes::NOTE_REVISE_ID)
        );
    }

    #[test]
    fn control_tools_optional_not_in_defaults() {
        // Управляющие инструменты — в каталоге, но не среди дефолтных (выкл по умолч.).
        assert!(
            !default_tool_ids()
                .iter()
                .any(|t| t == control::SEND_FOLLOWUP_ID)
        );
        assert!(
            !default_tool_ids()
                .iter()
                .any(|t| t == control::REWRITE_CURRENT_ID)
        );
        assert!(
            all_tool_ids()
                .iter()
                .any(|t| t == control::SEND_FOLLOWUP_ID)
        );
        assert!(
            all_tool_ids()
                .iter()
                .any(|t| t == control::REWRITE_CURRENT_ID)
        );
    }

    #[test]
    fn self_model_tools_optional_not_in_defaults() {
        // Инструменты «модели себя» — в каталоге, но не среди дефолтных.
        for id in [
            self_model::GET_SELF_MODEL_ID,
            self_model::REFLECT_ID,
            self_model::UPDATE_SELF_MODEL_ID,
            self_model::UPDATE_USER_MODEL_ID,
            self_model::ADD_INSIGHT_ID,
        ] {
            assert!(
                !default_tool_ids().iter().any(|t| t == id),
                "{id} в дефолтах"
            );
            assert!(
                all_tool_ids().iter().any(|t| t == id),
                "{id} нет в каталоге"
            );
        }
        // DB-only: проходят эффективный набор без глобальных гейтов.
        let eff = effective_tool_ids(&all_tool_ids(), false, false, false, None);
        assert!(eff.iter().any(|t| t == self_model::GET_SELF_MODEL_ID));
        assert!(eff.iter().any(|t| t == self_model::UPDATE_SELF_MODEL_ID));
    }

    #[test]
    fn effective_tool_ids_gates_external_tools() {
        let enabled = default_tool_ids();
        // web on, python off, fs off → есть web_search/fetch_url, нет python/fs.
        let eff = effective_tool_ids(&enabled, true, false, false, None);
        assert!(eff.iter().any(|t| t == WEB_SEARCH_ID));
        assert!(eff.iter().any(|t| t == FETCH_URL_ID));
        assert!(!eff.iter().any(|t| t == PYTHON_EXEC_ID));
        assert!(!eff.iter().any(|t| t == fs::FS_READ_ID));
        // всё off → ни одного внешнего/файлового, но внутренние остаются.
        let eff = effective_tool_ids(&enabled, false, false, false, None);
        assert!(!eff.iter().any(|t| t == WEB_SEARCH_ID || t == FETCH_URL_ID));
        assert!(
            !eff.iter()
                .any(|t| t == fs::FS_READ_ID || t == fs::FS_WRITE_ID || t == fs::FS_LIST_ID)
        );
        assert!(eff.iter().any(|t| t == "note_save"));
        // безопасные инструменты доступны всегда.
        assert!(eff.iter().any(|t| t == "calculate"));
        assert!(eff.iter().any(|t| t == "current_time"));
        // fs on → файловые инструменты появляются.
        let eff = effective_tool_ids(&enabled, false, false, true, None);
        assert!(eff.iter().any(|t| t == fs::FS_READ_ID));
        assert!(eff.iter().any(|t| t == fs::FS_WRITE_ID));
        assert!(eff.iter().any(|t| t == fs::FS_LIST_ID));
    }

    #[test]
    fn effective_tool_ids_keeps_sampling_tools_when_params_available() {
        let enabled = default_tool_ids();
        // Любой текущий режим имеет хотя бы один доступный параметр (max_tokens) —
        // инструменты семплинга остаются доступны (локально и в облаке).
        for provider in [
            None,
            Some(CloudProvider::OpenAi),
            Some(CloudProvider::Gemini),
            Some(CloudProvider::Claude),
        ] {
            let eff = effective_tool_ids(&enabled, false, false, false, provider);
            assert!(
                eff.iter().any(|t| t == GET_SAMPLING_ID),
                "get_sampling должен быть доступен для {provider:?}"
            );
            assert!(eff.iter().any(|t| t == SET_SAMPLING_ID));
        }
    }

    #[test]
    fn schemas_for_filters_and_orders() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        // Только включённые имена попадают в схемы; неизвестные игнорируются.
        let schemas = reg.schemas_for(
            &["echo".into(), "missing".into()],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
        );
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert!(
            reg.schemas_for(
                &[],
                crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
            )
            .is_empty()
        );
    }
}
