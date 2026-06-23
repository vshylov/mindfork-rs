//! Контракт системы инструментов (`features/tools`): трейт [`Tool`], снимок
//! [`ToolContext`], результат [`ToolOutcome`] с эффектами [`ChatEffect`] и
//! реестр [`ToolRegistry`]. См. spec §9.2, §6.3.
//!
//! Инструменты **не** мутируют `Chat` напрямую: мутирующие возвращают `effects`,
//! которые применяет оркестратор (единственный владелец `Chat`, spec §4.4.2).
//! Инструменты памяти/знаний обязаны фильтровать по `ctx.profile_id` (изоляция,
//! инвариант репозиториев, spec §9.5).

pub mod introspection;
pub mod notes;
pub mod python;
pub mod rag;
pub mod subagent;
pub mod web;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{Embedder, EngineBackend, ToolSchema};
use crate::shared::storage::Storage;

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
    /// Человекочитаемое описание для модели.
    fn description(&self) -> String;
    /// JSON Schema объекта параметров.
    fn parameters(&self) -> serde_json::Value;
    /// Исполняет вызов. `args` — распарсенный JSON аргументов модели.
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome>;

    /// Схема для передачи серверу (по умолчанию из `id`/`description`/`parameters`).
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.id(),
            description: self.description(),
            parameters: self.parameters(),
        }
    }
}

/// Имя web-инструмента (гейтится глобальным выключателем `tools.web_enabled`).
pub const WEB_SEARCH_ID: &str = "web_search";
/// Имя Python-инструмента (гейтится `tools.python_enabled`).
pub const PYTHON_EXEC_ID: &str = "python_exec";

/// Идентификаторы инструментов, включаемых в профиле по умолчанию (M5–M7).
/// Внешние (`web_search`/`python_exec`) дополнительно гейтятся глобальными
/// выключателями — см. [`effective_tool_ids`].
pub fn default_tool_ids() -> Vec<ToolId> {
    [
        "get_sampling",
        "set_sampling",
        "get_system_message",
        "set_system_message",
        "get_last_user_message_time",
        "note_save",
        "note_recall",
        "rag_add",
        "rag_search",
        "call_subagent",
        WEB_SEARCH_ID,
        PYTHON_EXEC_ID,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Эффективный набор инструментов: `enabled` минус внешние, отключённые
/// глобальными выключателями (spec §9.4). Порядок `enabled` сохраняется.
pub fn effective_tool_ids(
    enabled: &[ToolId],
    web_enabled: bool,
    python_enabled: bool,
) -> Vec<ToolId> {
    enabled
        .iter()
        .filter(|id| match id.as_str() {
            WEB_SEARCH_ID => web_enabled,
            PYTHON_EXEC_ID => python_enabled,
            _ => true,
        })
        .cloned()
        .collect()
}

/// Параметры построения реестра инструментов из конфигурации (`config.tools`,
/// spec §11.6). Позволяют пересобирать реестр при правках настроек (live).
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Путь к интерпретатору Python для `python_exec` (`None` → системный).
    pub python_path: Option<String>,
    /// Лимит токенов ответа `call_subagent`.
    pub subagent_max_tokens: usize,
    /// Лимит времени на вызов `call_subagent`.
    pub subagent_timeout: Duration,
    /// Значение по умолчанию для `web_search.fetch_content` (загрузка/реранк страниц,
    /// `config.tools.web_fetch_content`). Аргумент вызова переопределяет.
    pub web_fetch_content: bool,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            python_path: None,
            subagent_max_tokens: crate::shared::config::DEFAULT_SUBAGENT_MAX_TOKENS,
            subagent_timeout: Duration::from_secs(
                crate::shared::config::DEFAULT_SUBAGENT_TIMEOUT_SECS,
            ),
            web_fetch_content: true,
        }
    }
}

/// Реестр со всеми инструментами (M5–M7) по параметрам [`ToolConfig`]. Глобальные
/// выключатели применяются не здесь, а при отборе эффективного набора (см.
/// [`effective_tool_ids`]).
pub fn standard_registry(cfg: &ToolConfig) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(introspection::GetSampling));
    reg.register(Arc::new(introspection::SetSampling));
    reg.register(Arc::new(introspection::GetSystemMessage));
    reg.register(Arc::new(introspection::SetSystemMessage));
    reg.register(Arc::new(introspection::GetLastUserMessageTime));
    reg.register(Arc::new(notes::NoteSave));
    reg.register(Arc::new(notes::NoteRecall));
    reg.register(Arc::new(rag::RagAdd));
    reg.register(Arc::new(rag::RagSearch));
    reg.register(Arc::new(subagent::CallSubagent::new(
        cfg.subagent_max_tokens,
        cfg.subagent_timeout,
    )));
    reg.register(Arc::new(web::WebSearch::new(cfg.web_fetch_content)));
    reg.register(Arc::new(python::PythonExec::new(cfg.python_path.clone())));
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

    /// Схемы для подмножества включённых инструментов (профиль ∩ глобально), с
    /// сохранением порядка `enabled`. Неизвестные имена игнорируются.
    pub fn schemas_for(&self, enabled: &[ToolId]) -> Vec<ToolSchema> {
        enabled
            .iter()
            .filter_map(|id| self.tools.get(id))
            .map(|t| t.schema())
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

    /// Контекст инструмента поверх временного хранилища. Возвращает также
    /// `TempDir` (держать живым) и `Arc<Storage>` (для проверок в тесте).
    pub fn ctx_with_storage(profile_id: Uuid) -> (tempfile::TempDir, Arc<Storage>, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let engine: Arc<dyn EngineBackend> = Arc::new(MockBackend::scripted(vec![]));
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(16));
        let ctx = ToolContext {
            profile_id,
            chat_id: Uuid::new_v4(),
            system_message: "системное сообщение".into(),
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: None,
            storage: storage.clone(),
            engine,
            embedder,
            chunk_params: rag::ChunkParams::default(),
        };
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
        fn description(&self) -> String {
            "Возвращает аргумент text".into()
        }
        fn parameters(&self) -> serde_json::Value {
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
        for id in default_tool_ids() {
            assert!(reg.get(&id).is_some(), "инструмент {id} не зарегистрирован");
        }
        // Схемы для дефолтного набора покрывают все id.
        assert_eq!(
            reg.schemas_for(&default_tool_ids()).len(),
            default_tool_ids().len()
        );
    }

    #[test]
    fn effective_tool_ids_gates_external_tools() {
        let enabled = default_tool_ids();
        // web on, python off → есть web_search, нет python_exec.
        let eff = effective_tool_ids(&enabled, true, false);
        assert!(eff.iter().any(|t| t == WEB_SEARCH_ID));
        assert!(!eff.iter().any(|t| t == PYTHON_EXEC_ID));
        // оба off → ни одного внешнего, но внутренние остаются.
        let eff = effective_tool_ids(&enabled, false, false);
        assert!(
            !eff.iter()
                .any(|t| t == WEB_SEARCH_ID || t == PYTHON_EXEC_ID)
        );
        assert!(eff.iter().any(|t| t == "note_save"));
    }

    #[test]
    fn schemas_for_filters_and_orders() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        // Только включённые имена попадают в схемы; неизвестные игнорируются.
        let schemas = reg.schemas_for(&["echo".into(), "missing".into()]);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert!(reg.schemas_for(&[]).is_empty());
    }
}
