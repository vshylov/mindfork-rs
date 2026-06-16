//! Оркестратор: единственный владелец доменного состояния (профили/чаты,
//! [`Storage`]) и автомат генерации. Принимает [`AppCommand`], исполняет
//! генерацию (в отдельной задаче) и рассылает [`AppEvent`].
//! См. spec §4.4 (однонаправленный поток, `generation_id`, автомат
//! `Idle/Generating/Cancelling`) и §4.4.2 (оркестратор — единственный писатель).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, ServerStatus};
use crate::app::supervisor::ServerSupervisor;
use crate::entities::chat::{Chat, ChatSummary};
use crate::entities::message::{Message, MessageMetadata, MessageRole, ToolCallRecord};
use crate::entities::profile::{Profile, ToolId};
use crate::entities::sampling::{ReasoningEffort, SamplingConfig};
use crate::features::profiles::ProfileEdit;
use crate::features::tools::{
    ChatEffect, ToolConfig, ToolContext, ToolRegistry, effective_tool_ids, standard_registry,
};
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, Embedder, EngineBackend, FinishReason,
    ServerHandle, ToolCallAccumulator,
};
use crate::shared::config::AppConfig;
use crate::shared::storage::Storage;

/// Системное сообщение профиля по умолчанию (создаётся при пустом хранилище).
const DEFAULT_SYSTEM_MESSAGE: &str =
    "Ты — полезный ассистент. Отвечай ясно и по существу на языке пользователя.";

/// Дебаунс сохранения изменённых чатов на диск.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

/// Потолок токенов ответа при генерации авто-названия. Пытаемся выключить «мысли»
/// (`reasoning_budget=0` + `chat_template_kwargs.enable_thinking=false`), но
/// некоторые модели (вшитый в GGUF thinking, напр. Gemma `peg-gemma4`) их
/// игнорируют и всё равно «рассуждают» сотни токенов перед ответом — поэтому
/// бюджет щедрый, чтобы модель успела завершить «мысли» и выдать заголовок.
const TITLE_MAX_TOKENS: usize = 2048;

/// Лимит времени на генерацию авто-названия чата (с запасом на «думающие» модели).
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Параметры запуска оркестратора. Серверы (chat/embedding) и реестр инструментов
/// оркестратор настраивает сам из [`AppConfig`] через [`ServerSupervisor`] — это
/// позволяет перезапускать их при правках настроек (spec §11.6).
pub struct OrchestratorDeps {
    pub cmd_rx: UnboundedReceiver<AppCommand>,
    pub evt_tx: UnboundedSender<AppEvent>,
    pub storage: Arc<Storage>,
    /// Полная конфигурация приложения (оркестратор — её единственный писатель).
    pub config: AppConfig,
    /// Супервайзер серверов инференса/эмбеддингов (real или mock в тестах).
    pub supervisor: Arc<dyn ServerSupervisor>,
}

/// Состояние генерации (автомат на активный чат).
enum State {
    Idle,
    Generating { id: Uuid, cancel: CancellationToken },
    Cancelling { id: Uuid },
}

impl State {
    fn current_id(&self) -> Option<Uuid> {
        match self {
            State::Idle => None,
            State::Generating { id, .. } | State::Cancelling { id, .. } => Some(*id),
        }
    }
}

/// Результат завершившейся задачи генерации (внутренний канал).
struct GenResult {
    id: Uuid,
    chat_id: Uuid,
    /// Новые доменные сообщения (assistant с tool_calls, tool-результаты, финал) —
    /// в порядке появления; оркестратор дописывает их в `Chat`.
    messages: Vec<Message>,
    /// Эффекты инструментов (применяются оркестратором — владельцем `Chat`).
    effects: Vec<ChatEffect>,
}

/// Результат фоновой задачи авто-названия чата (внутренний канал).
struct TitleResult {
    chat_id: Uuid,
    /// Сырой текст ответа модели (или сообщение об ошибке для показа в UI).
    text: Result<String, String>,
}

/// Главный цикл оркестратора. Завершается при закрытии канала команд или
/// получении [`AppCommand::Quit`].
pub async fn run(deps: OrchestratorDeps) {
    let OrchestratorDeps {
        mut cmd_rx,
        evt_tx,
        storage,
        config,
        supervisor,
    } = deps;

    let (done_tx, mut done_rx) = unbounded_channel::<GenResult>();
    // Внутренний канал статуса сервера: фоновый probe супервайзера досылает в него
    // готовность (Ready/Disconnected), петля транслирует в AppEvent::ServerStatus.
    let (status_tx, mut status_rx) = unbounded_channel::<ServerStatus>();
    // Внутренний канал авто-названий: фоновая задача присылает сгенерированный
    // заголовок (или ошибку), петля применяет его к чату.
    let (title_tx, mut title_rx) = unbounded_channel::<TitleResult>();
    let registry = Arc::new(build_registry(&config));
    let mut orch = Orchestrator {
        evt_tx,
        supervisor,
        backend: None,
        chat_handle: None,
        embed_handle: None,
        embedder: Arc::new(crate::shared::api::UnavailableEmbedder),
        storage,
        config,
        registry,
        status_tx,
        title_tx,
        server_status: ServerStatus::NotConfigured,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        state: State::Idle,
        done_tx,
        dirty: HashSet::new(),
        save_deadline: None,
    };

    // Поднимаем серверы по конфигу и эмитим стартовые события/настройки.
    orch.apply_chat_settings();
    orch.apply_embed_settings();
    if let Err(err) = orch.bootstrap() {
        let _ = orch
            .evt_tx
            .send(AppEvent::Error(format!("Ошибка загрузки данных: {err}")));
    }
    orch.emit_settings();

    loop {
        let deadline = orch.save_deadline;
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(cmd) => if orch.handle_command(cmd) { break },
                }
            }
            done = done_rx.recv() => {
                if let Some(res) = done {
                    orch.handle_done(res);
                }
            }
            status = status_rx.recv() => {
                if let Some(s) = status {
                    orch.server_status = s.clone();
                    let _ = orch.evt_tx.send(AppEvent::ServerStatus(s));
                }
            }
            title = title_rx.recv() => {
                if let Some(res) = title {
                    orch.handle_title_result(res);
                }
            }
            _ = sleep_until_opt(deadline) => orch.flush_saves(),
        }
    }
    orch.flush_saves();
}

/// Строит реестр инструментов из конфигурации (`config.tools`).
fn build_registry(config: &AppConfig) -> ToolRegistry {
    standard_registry(&ToolConfig {
        python_path: config.tools.python_path.clone(),
        subagent_max_tokens: config.tools.subagent_max_tokens,
        subagent_timeout: Duration::from_secs(config.tools.subagent_timeout_secs),
    })
}

/// Спит до `deadline`, либо «висит вечно», если дедлайна нет (нет грязных чатов).
async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending::<()>().await,
    }
}

struct Orchestrator {
    evt_tx: UnboundedSender<AppEvent>,
    /// Супервайзер серверов (для перезапуска при смене модели/сервера).
    supervisor: Arc<dyn ServerSupervisor>,
    backend: Option<Arc<dyn EngineBackend>>,
    /// Опора на managed chat-процесс (drop → kill). `None` — external/не настроен.
    chat_handle: Option<ServerHandle>,
    /// Опора на managed embedding-процесс.
    embed_handle: Option<ServerHandle>,
    /// Источник эмбеддингов для RAG (выделенный сервер — ADR 0002).
    embedder: Arc<dyn Embedder>,
    storage: Arc<Storage>,
    /// Полная конфигурация (оркестратор — единственный писатель в `settings.json`).
    config: AppConfig,
    /// Реестр инструментов (пересобирается при правках `config.tools`).
    registry: Arc<ToolRegistry>,
    /// Канал статуса сервера для фонового probe супервайзера.
    status_tx: UnboundedSender<ServerStatus>,
    /// Канал результатов фоновой генерации авто-названий чатов.
    title_tx: UnboundedSender<TitleResult>,
    /// Текущий статус chat-сервера. Генерация стартует только в `Ready`: запрос к
    /// ещё загружающемуся (`Connecting`) managed-серверу вернул бы 503 («error
    /// status»), а для перегенерации — ещё и снёс бы прежний ответ впустую.
    server_status: ServerStatus,
    profiles: Vec<Profile>,
    /// Видимые чаты, целиком в памяти (оркестратор — единственный писатель).
    chats: Vec<Chat>,
    active_id: Option<Uuid>,
    state: State,
    done_tx: UnboundedSender<GenResult>,
    /// Чаты, ожидающие записи на диск (дебаунс).
    dirty: HashSet<Uuid>,
    save_deadline: Option<Instant>,
}

impl Orchestrator {
    /// Загружает профили/чаты, гарантирует наличие хотя бы одного из каждого,
    /// выбирает активный чат и шлёт стартовые события.
    fn bootstrap(&mut self) -> anyhow::Result<()> {
        self.profiles = self
            .storage
            .json()
            .load_profiles()?
            .into_iter()
            .filter(|p| !p.is_hidden)
            .collect();
        if self.profiles.is_empty() {
            let mut profile = Profile::new("Ассистент", DEFAULT_SYSTEM_MESSAGE);
            // Включаем базовые инструменты M5 в дефолтном профиле.
            profile.enabled_tools = crate::features::tools::default_tool_ids();
            self.storage.json().upsert_profile(&profile)?;
            self.profiles.push(profile);
        }

        self.chats = self
            .storage
            .json()
            .load_chats()?
            .into_iter()
            .filter(|c| !c.is_hidden)
            .collect();
        if self.chats.is_empty() {
            let chat = self.new_chat_value(None);
            self.storage.json().save_chat(&chat)?;
            self.chats.push(chat);
        }
        self.chats.sort_by_key(|c| std::cmp::Reverse(c.modified_at));

        let active = self.chats.first().map(|c| c.id);
        self.emit_profile_list();
        self.emit_chat_list();
        if let Some(id) = active {
            self.activate(id);
        }
        Ok(())
    }

    /// Обрабатывает команду. Возвращает `true`, если нужно завершить цикл.
    fn handle_command(&mut self, cmd: AppCommand) -> bool {
        match cmd {
            AppCommand::Quit => {
                if let State::Generating { cancel, .. } = &self.state {
                    cancel.cancel();
                }
                return true;
            }
            AppCommand::Cancel => {
                if let State::Generating { id, cancel, .. } = &self.state {
                    cancel.cancel();
                    self.state = State::Cancelling { id: *id };
                }
            }
            AppCommand::SendMessage(text) => self.handle_send(text),
            AppCommand::RegenerateLast => self.handle_regenerate(),
            AppCommand::DeleteLastExchange => self.handle_delete_last(),
            AppCommand::NewChat { profile_id } => self.handle_new_chat(profile_id),
            AppCommand::SwitchChat(id) => self.handle_switch(id),
            AppCommand::RenameChat { id, title } => self.handle_rename(id, title),
            AppCommand::AutoRenameChat(id) => self.handle_auto_rename(id),
            AppCommand::CloneChat(id) => self.handle_clone(id),
            AppCommand::CopyChat(id) => self.handle_copy_chat(id),
            AppCommand::DeleteChat(id) => self.handle_delete(id),
            AppCommand::CreateProfile {
                name,
                system_message,
            } => self.handle_create_profile(name, system_message),
            AppCommand::DeleteProfile(id) => self.handle_delete_profile(id),
            AppCommand::UpdateConfig(config) => self.handle_update_config(*config),
            AppCommand::UpdateProfile { id, edit } => self.handle_update_profile(id, *edit),
        }
        false
    }

    fn handle_send(&mut self, text: String) {
        if !matches!(self.state, State::Idle) {
            return;
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(active_id) = self.active_id else {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Нет активного чата".into()));
            return;
        };
        let Some(backend) = self.ready_backend() else {
            // Сервер не готов: поле ввода уже очищено экраном — возвращаем текст,
            // чтобы пользователь не потерял сообщение (ошибка показана отдельно).
            let _ = self.evt_tx.send(AppEvent::RestoreInput(text));
            return;
        };

        // Добавляем сообщение пользователя в историю и эхо в ленту.
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.push_message(Message::user(&text));
        }
        self.mark_dirty(active_id);
        let _ = self.evt_tx.send(AppEvent::UserMessage(text));

        self.start_generation(active_id, backend);
    }

    /// Перегенерирует последний ответ ассистента (spec §11.7): удаляет всё после
    /// последнего сообщения пользователя (старый ответ + tool-сообщения) и
    /// запускает генерацию заново из того же запроса. Лента перестраивается через
    /// переэмит `ChatActivated`. Во время генерации — игнорируется.
    fn handle_regenerate(&mut self) {
        if !matches!(self.state, State::Idle) {
            return;
        }
        let Some(active_id) = self.active_id else {
            return;
        };
        // Готовность сервера проверяем ДО усечения истории: иначе на не-готовом
        // сервере (загрузка модели) старый ответ был бы снесён, а новый не пришёл бы.
        let Some(backend) = self.ready_backend() else {
            return;
        };
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User)
            else {
                return; // нет запроса пользователя — нечего перегенерировать
            };
            chat.messages.truncate(idx + 1);
            chat.modified_at = chrono::Utc::now();
        }
        self.mark_dirty(active_id);
        self.activate(active_id); // перестроить ленту без старого ответа
        self.emit_chat_list();
        self.start_generation(active_id, backend);
    }

    /// Удаляет последний обмен: ответ ассистента вместе с вызвавшим его сообщением
    /// пользователя (spec §11.7). Текст пользователя возвращается в поле ввода
    /// (`RestoreInput`), чтобы его можно было отредактировать и отправить заново.
    /// Во время генерации — игнорируется.
    fn handle_delete_last(&mut self) {
        if !matches!(self.state, State::Idle) {
            return;
        }
        let Some(active_id) = self.active_id else {
            return;
        };
        let user_text;
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == MessageRole::User)
            else {
                return; // нет сообщения пользователя — удалять нечего
            };
            user_text = chat.messages[idx].text.clone();
            chat.messages.truncate(idx);
            chat.modified_at = chrono::Utc::now();
        }
        self.mark_dirty(active_id);
        self.activate(active_id); // перестроить ленту без удалённого обмена
        self.emit_chat_list();
        let _ = self.evt_tx.send(AppEvent::RestoreInput(user_text));
    }

    /// Возвращает движок, если chat-сервер готов к генерации (`Ready`); иначе
    /// эмитит понятную ошибку (не настроен / ещё подключается / недоступен) и
    /// возвращает `None`. Гейтит и отправку, и перегенерацию — чтобы запрос не
    /// уходил на ещё загружающийся сервер (иначе 503 → «engine returned an error
    /// status»). См. spec §7.
    fn ready_backend(&self) -> Option<Arc<dyn EngineBackend>> {
        match &self.server_status {
            ServerStatus::Ready => match self.backend.clone() {
                Some(backend) => Some(backend),
                None => {
                    let _ = self
                        .evt_tx
                        .send(AppEvent::Error("LLM-сервер не настроен".into()));
                    None
                }
            },
            ServerStatus::Connecting => {
                let _ = self.evt_tx.send(AppEvent::Error(
                    "Сервер ещё подключается — дождитесь готовности и повторите".into(),
                ));
                None
            }
            ServerStatus::NotConfigured => {
                let _ = self
                    .evt_tx
                    .send(AppEvent::Error("LLM-сервер не настроен".into()));
                None
            }
            ServerStatus::Disconnected(reason) => {
                let _ = self
                    .evt_tx
                    .send(AppEvent::Error(format!("Сервер недоступен: {reason}")));
                None
            }
        }
    }

    /// Запускает генерацию из текущего состояния чата (история уже подготовлена:
    /// добавлено сообщение пользователя или усечён старый ответ). Общая часть для
    /// отправки нового сообщения и перегенерации.
    fn start_generation(&mut self, active_id: Uuid, backend: Arc<dyn EngineBackend>) {
        // Снимок на начало хода: семплинг, доступные инструменты, контекст.
        let sampling = self.effective_sampling(active_id);
        let Some(chat_ref) = self.chats.iter().find(|c| c.id == active_id) else {
            return;
        };
        let profile_id = chat_ref.profile_id;
        let enabled = self
            .profiles
            .iter()
            .find(|p| p.id == profile_id)
            .map(|p| p.enabled_tools.clone())
            .unwrap_or_default();
        // Эффективный набор = профиль ∩ глобальные выключатели (spec §9.4).
        let allowed = effective_tool_ids(
            &enabled,
            self.config.tools.web_enabled,
            self.config.tools.python_enabled,
        );
        let schemas = self.registry.schemas_for(&allowed);

        // Строим запрос/контекст инструмента из текущей истории чата.
        let request;
        let ctx;
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            request = build_request(chat, sampling.clone(), schemas);
            ctx = ToolContext {
                profile_id,
                chat_id: active_id,
                system_message: chat.system_message.clone(),
                effective_sampling: sampling,
                last_user_message_at: last_user_message_at(chat),
                storage: self.storage.clone(),
                engine: backend.clone(),
                embedder: self.embedder.clone(),
            };
        }

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let _ = self
            .evt_tx
            .send(AppEvent::GenerationStarted { generation_id: id });
        self.state = State::Generating {
            id,
            cancel: cancel.clone(),
        };
        spawn_generation(GenSpawn {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            cancel,
            id,
            chat_id: active_id,
            max_rounds: self.config.max_tool_rounds,
            allowed,
            evt_tx: self.evt_tx.clone(),
            done_tx: self.done_tx.clone(),
        });
    }

    fn handle_done(&mut self, res: GenResult) {
        // Применяем только результат текущей генерации (защита от устаревших).
        if self.state.current_id() != Some(res.id) {
            return;
        }
        self.state = State::Idle;

        if res.messages.is_empty() && res.effects.is_empty() {
            return;
        }
        if let Some(chat) = self.chat_mut(res.chat_id) {
            for msg in res.messages {
                chat.push_message(msg);
            }
            // Эффекты инструментов применяет оркестратор (владелец Chat, §4.4.2).
            for effect in res.effects {
                match effect {
                    ChatEffect::SetSystemMessage(s) => chat.system_message = s,
                    ChatEffect::SetSamplingOverride(s) => chat.sampling_override = Some(s),
                }
            }
            self.mark_dirty(res.chat_id);
            self.emit_chat_list();
        }
    }

    fn handle_new_chat(&mut self, profile_id: Option<Uuid>) {
        let chat = self.new_chat_value(profile_id);
        let id = chat.id;
        if let Err(err) = self.storage.json().save_chat(&chat) {
            let _ = self
                .evt_tx
                .send(AppEvent::Error(format!("Не удалось создать чат: {err}")));
            return;
        }
        self.chats.insert(0, chat);
        self.emit_chat_list();
        self.activate(id);
    }

    fn handle_switch(&mut self, id: Uuid) {
        if self.active_id == Some(id) {
            return;
        }
        // Если идёт генерация — отменяем её (частичный ответ сохранится для
        // исходного чата по приходу GenResult).
        if let State::Generating {
            id: gid, cancel, ..
        } = &self.state
        {
            cancel.cancel();
            self.state = State::Cancelling { id: *gid };
        }
        if self.chats.iter().any(|c| c.id == id) {
            self.activate(id);
        }
    }

    fn handle_rename(&mut self, id: Uuid, title: String) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        if let Some(chat) = self.chat_mut(id) {
            chat.title = title.clone();
            self.mark_dirty(id);
            self.emit_chat_list();
            let _ = self.evt_tx.send(AppEvent::ChatRenamed { id, title });
        }
    }

    /// Авто-название чата (spec §11.2): модель читает переписку (или её начало и
    /// конец, если она длинная) и придумывает короткий заголовок. Запрос идёт
    /// фоновой задачей; результат прилетает в [`Orchestrator::handle_title_result`].
    /// Чат-сервер должен быть готов (`Ready`) — иначе понятная ошибка.
    fn handle_auto_rename(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(digest) = crate::features::rename_chat::build_conversation_digest(&chat.messages)
        else {
            let _ = self.evt_tx.send(AppEvent::ChatListError(
                "Недостаточно сообщений для авто-названия".into(),
            ));
            return;
        };
        let Some(backend) = self.ready_backend() else {
            return;
        };
        // Свежий компактный семплинг (не наследуем override чата): короткий ответ,
        // умеренная температура, reasoning выключен (заголовку «мысли» не нужны и
        // только съедают бюджет токенов), без инструментов. Ключевое — `reasoning_
        // budget=0`: для моделей со «вшитым» в шаблон thinking (Gemma `peg-gemma4`,
        // Qwen) только он реально гасит «мысли»; поля `thinking`/`reasoning_effort`
        // сервер для таких шаблонов игнорирует (иначе модель тратила весь бюджет на
        // «мысли» и ответный текст приходил пустым).
        let sampling = SamplingConfig {
            max_tokens: Some(TITLE_MAX_TOKENS),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(crate::features::rename_chat::TITLE_SYSTEM_MESSAGE.to_string()),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: Vec::new(),
        };
        spawn_title(backend, request, id, self.title_tx.clone());
    }

    /// Применяет результат фоновой генерации авто-названия: чистит/нормализует
    /// заголовок и переименовывает чат (или показывает ошибку).
    fn handle_title_result(&mut self, res: TitleResult) {
        match res.text {
            Ok(raw) => {
                let Some(title) = crate::features::rename_chat::clean_generated_title(&raw) else {
                    let _ = self.evt_tx.send(AppEvent::ChatListError(
                        "Модель не вернула название чата".into(),
                    ));
                    return;
                };
                if let Some(chat) = self.chat_mut(res.chat_id) {
                    chat.title = title.clone();
                    self.mark_dirty(res.chat_id);
                    self.emit_chat_list();
                    let _ = self.evt_tx.send(AppEvent::ChatRenamed {
                        id: res.chat_id,
                        title,
                    });
                }
            }
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(msg));
            }
        }
    }

    fn handle_clone(&mut self, id: Uuid) {
        let Some(src) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        let now = chrono::Utc::now();
        let mut clone = src.clone();
        clone.id = Uuid::new_v4();
        clone.title = format!("{} (копия)", src.title);
        clone.created_at = now;
        clone.modified_at = now;
        let new_id = clone.id;
        if let Err(err) = self.storage.json().save_chat(&clone) {
            let _ = self.evt_tx.send(AppEvent::ChatListError(format!(
                "Не удалось клонировать чат: {err}"
            )));
            return;
        }
        self.chats.insert(0, clone);
        self.emit_chat_list();
        self.activate(new_id);
    }

    /// Копирование всей переписки чата в буфер обмена (spec §11.2): оркестратор
    /// (владелец `Chat`) формирует текст и эмитит `CopyToClipboard` — запись в буфер
    /// и подтверждение делает UI-слой (`runtime`). Пустой чат → понятная ошибка.
    fn handle_copy_chat(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        match crate::features::chat_export::format_conversation(&chat.title, &chat.messages) {
            Some(text) => {
                let _ = self.evt_tx.send(AppEvent::CopyToClipboard(text));
            }
            None => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(
                    "Нечего копировать — в чате нет сообщений".into(),
                ));
            }
        }
    }

    fn handle_delete(&mut self, id: Uuid) {
        match self.storage.json().hide_chat(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::ChatListError(format!(
                    "Не удалось удалить чат: {err}"
                )));
                return;
            }
            Ok(true) => {}
        }
        self.chats.retain(|c| c.id != id);
        self.dirty.remove(&id);

        // Если удалили активный — выбираем другой (или создаём новый).
        if self.active_id == Some(id) {
            self.active_id = None;
            if let Some(next) = self.chats.first().map(|c| c.id) {
                self.emit_chat_list();
                self.activate(next);
            } else {
                self.handle_new_chat(None);
            }
        } else {
            self.emit_chat_list();
        }
    }

    /// Создаёт новый профиль (валидирует имя), сохраняет и обновляет список.
    fn handle_create_profile(&mut self, name: String, system_message: String) {
        let Some(profile) = crate::features::profiles::create(&name, system_message) else {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Имя профиля не может быть пустым".into()));
            return;
        };
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось создать профиль: {err}"
            )));
            return;
        }
        self.profiles.push(profile);
        self.emit_profile_list();
    }

    /// Мягко удаляет профиль с каскадом: скрываются его чаты, а заметки/RAG
    /// становятся недостижимы (профиль скрыт). См. spec §10, §12.3.
    fn handle_delete_profile(&mut self, id: Uuid) {
        // Нельзя удалить последний профиль — иначе не из чего создавать чаты.
        if self.profiles.len() <= 1 {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Нельзя удалить последний профиль".into()));
            return;
        }
        match self.storage.hide_profile_cascade(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self.evt_tx.send(AppEvent::Error(format!(
                    "Не удалось удалить профиль: {err}"
                )));
                return;
            }
            Ok(true) => {}
        }
        self.profiles.retain(|p| p.id != id);
        // Убираем из памяти чаты удалённого профиля.
        let removed: Vec<Uuid> = self
            .chats
            .iter()
            .filter(|c| c.profile_id == id)
            .map(|c| c.id)
            .collect();
        self.chats.retain(|c| c.profile_id != id);
        for cid in &removed {
            self.dirty.remove(cid);
        }
        self.emit_profile_list();

        // Если активный чат принадлежал удалённому профилю — переключаемся.
        let active_removed = self.active_id.is_some_and(|a| removed.contains(&a));
        if active_removed {
            self.active_id = None;
            if let Some(next) = self.chats.first().map(|c| c.id) {
                self.emit_chat_list();
                self.activate(next);
            } else {
                self.handle_new_chat(None);
            }
        } else {
            self.emit_chat_list();
        }
    }

    /// Применяет правки конфигурации: сохраняет, перезапускает сервер/реестр при
    /// необходимости и переэмитит настройки. Единственный писатель в `settings.json`.
    fn handle_update_config(&mut self, config: AppConfig) {
        let old = std::mem::replace(&mut self.config, config);
        if let Err(err) = self.storage.json().save_config(&self.config) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось сохранить настройки: {err}"
            )));
            self.config = old; // откат к прежнему состоянию
            return;
        }
        // Смена настроек chat-сервера (модель/режим/порт/…) — перезапуск (spec §11.6).
        if self.config.engine != old.engine {
            self.apply_chat_settings();
        }
        // Смена настроек embedding-сервера — пере-подключение/перезапуск.
        if self.config.embed != old.embed {
            self.apply_embed_settings();
        }
        // Смена параметров инструментов — пересборка реестра (python_path, лимиты).
        if self.config.tools != old.tools {
            self.registry = Arc::new(build_registry(&self.config));
        }
        self.emit_settings();
    }

    /// Применяет правки профиля (не затрагивает уже созданные чаты — у них свои
    /// копии, spec §10). Сохраняет и переэмитит список профилей/настройки.
    fn handle_update_profile(&mut self, id: Uuid, edit: ProfileEdit) {
        let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) else {
            return;
        };
        if !crate::features::profiles::apply_edit(profile, edit) {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Имя профиля не может быть пустым".into()));
            return;
        }
        let profile = profile.clone();
        if let Err(err) = self.storage.json().upsert_profile(&profile) {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось сохранить профиль: {err}"
            )));
            return;
        }
        self.emit_profile_list();
        self.emit_settings();
    }

    /// (Пере)поднимает chat-сервер по `config.engine`: гасит прежний процесс,
    /// просит супервайзер настроить новый, эмитит немедленный статус.
    fn apply_chat_settings(&mut self) {
        self.chat_handle = None; // drop старого managed-процесса (kill_on_drop)
        let setup = self
            .supervisor
            .apply_chat(&self.config.engine, self.status_tx.clone());
        self.backend = setup.backend;
        self.chat_handle = setup.handle;
        self.server_status = setup.status.clone();
        let _ = self.evt_tx.send(AppEvent::ServerStatus(setup.status));
    }

    /// (Пере)поднимает embedding-сервер по `config.embed`.
    fn apply_embed_settings(&mut self) {
        self.embed_handle = None;
        let setup = self.supervisor.apply_embed(&self.config.embed);
        self.embedder = setup.embedder;
        self.embed_handle = setup.handle;
    }

    /// Эмитит полный снимок настроек (конфиг + полные профили) для экрана настроек.
    fn emit_settings(&self) {
        let visible: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| !p.is_hidden)
            .cloned()
            .collect();
        let _ = self.evt_tx.send(AppEvent::Settings {
            config: Box::new(self.config.clone()),
            profiles: visible,
        });
    }

    // ---------- вспомогательное ----------

    /// Создаёт новый чат из профиля (по `id` или первого) с приветствием.
    fn new_chat_value(&self, profile_id: Option<Uuid>) -> Chat {
        let profile = profile_id
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .or_else(|| self.profiles.first())
            .cloned()
            .unwrap_or_else(|| Profile::new("Ассистент", DEFAULT_SYSTEM_MESSAGE));
        let mut chat = Chat::from_profile(&profile, "Новый чат");
        if let Some(greeting) = &profile.greeting
            && !greeting.is_empty()
        {
            chat.push_message(Message::assistant(greeting.clone()));
        }
        chat
    }

    fn chat_mut(&mut self, id: Uuid) -> Option<&mut Chat> {
        self.chats.iter_mut().find(|c| c.id == id)
    }

    /// Разрешает фактический семплинг для чата: `Chat.sampling_override` →
    /// `Profile.default_sampling` → глобальный (spec §8.3).
    fn effective_sampling(&self, chat_id: Uuid) -> SamplingConfig {
        let chat = self.chats.iter().find(|c| c.id == chat_id);
        let chat_override = chat.and_then(|c| c.sampling_override.as_ref());
        let profile_default = chat
            .and_then(|c| self.profiles.iter().find(|p| p.id == c.profile_id))
            .and_then(|p| p.default_sampling.as_ref());
        crate::entities::sampling::resolve(
            chat_override,
            profile_default,
            &self.config.default_sampling,
        )
    }

    /// Делает чат активным и шлёт его сообщения в UI.
    fn activate(&mut self, id: Uuid) {
        let Some(chat) = self.chats.iter().find(|c| c.id == id) else {
            return;
        };
        self.active_id = Some(id);
        let _ = self.evt_tx.send(AppEvent::ChatActivated {
            id,
            title: chat.title.clone(),
            messages: chat.messages.clone(),
        });
    }

    fn emit_chat_list(&self) {
        let mut summaries: Vec<ChatSummary> = self.chats.iter().map(|c| c.summary()).collect();
        summaries.sort_by_key(|s| std::cmp::Reverse(s.modified_at));
        let _ = self.evt_tx.send(AppEvent::ChatList(summaries));
    }

    fn emit_profile_list(&self) {
        let _ = self.evt_tx.send(AppEvent::ProfileList(
            self.profiles.iter().map(|p| p.summary()).collect(),
        ));
    }

    /// Помечает чат для отложенного сохранения (дебаунс).
    fn mark_dirty(&mut self, id: Uuid) {
        self.dirty.insert(id);
        self.save_deadline = Some(Instant::now() + SAVE_DEBOUNCE);
    }

    /// Сохраняет все грязные чаты на диск.
    fn flush_saves(&mut self) {
        self.save_deadline = None;
        if self.dirty.is_empty() {
            return;
        }
        let ids: Vec<Uuid> = self.dirty.drain().collect();
        for id in ids {
            if let Some(chat) = self.chats.iter().find(|c| c.id == id)
                && let Err(err) = self.storage.json().save_chat(chat)
            {
                tracing::error!(chat = %id, error = %err, "не удалось сохранить чат");
            }
        }
    }
}

/// Конвертирует доменное сообщение в сообщение для модели. Системные сообщения
/// передаются через [`ChatRequest::system`] (здесь — `None`). Assistant с
/// tool-вызовами и tool-результаты восстанавливаются для корректной истории
/// (строгая валидация порядка сервером, contract §3.2).
fn message_to_api(message: &Message) -> Option<ApiMessage> {
    match message.role {
        MessageRole::System => None,
        MessageRole::User => Some(ApiMessage::user(&message.text)),
        MessageRole::Assistant => {
            if message.tool_calls.is_empty() {
                Some(ApiMessage::assistant(&message.text))
            } else {
                let calls = message.tool_calls.iter().map(record_to_api).collect();
                Some(ApiMessage::assistant_tool_calls(&message.text, calls))
            }
        }
        MessageRole::Tool => message
            .tool_call_id
            .as_ref()
            .map(|id| ApiMessage::tool(id, &message.text)),
    }
}

/// Доменная запись tool-вызова → форма для запроса (аргументы как JSON-строка).
fn record_to_api(rec: &ToolCallRecord) -> ApiToolCall {
    ApiToolCall {
        id: rec.id.clone(),
        name: rec.name.clone(),
        arguments: rec.arguments.to_string(),
    }
}

/// Время последнего user-сообщения чата (для `ToolContext`).
fn last_user_message_at(chat: &Chat) -> Option<chrono::DateTime<chrono::Utc>> {
    chat.messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::User)
        .map(|m| m.timestamp)
}

/// Строит запрос генерации из текущего состояния чата с набором схем инструментов.
fn build_request(
    chat: &Chat,
    sampling: SamplingConfig,
    tools: Vec<crate::shared::api::ToolSchema>,
) -> ChatRequest {
    let system = if chat.system_message.trim().is_empty() {
        None
    } else {
        Some(chat.system_message.clone())
    };
    ChatRequest {
        system,
        messages: chat.messages.iter().filter_map(message_to_api).collect(),
        sampling,
        tools,
    }
}

/// Запускает фоновую задачу авто-названия чата: один независимый запрос к модели
/// (без истории/инструментов), сбор текста, отправка результата в `title_tx`.
/// Лимит времени — [`TITLE_TIMEOUT`].
fn spawn_title(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    chat_id: Uuid,
    title_tx: UnboundedSender<TitleResult>,
) {
    tokio::spawn(async move {
        let cancel = CancellationToken::new();
        let collect = async {
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            let mut text = String::new();
            let mut thoughts = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => text.push_str(&t),
                    // Копим «мысли» как запасной источник: если модель так и не
                    // «завершила мысль» (выдала только reasoning), вытащим заголовок
                    // из последней содержательной строки рассуждений.
                    ChatChunk::Thoughts(t) => thoughts.push_str(&t),
                    ChatChunk::Finished(_) => break,
                    ChatChunk::ToolCall(_) => {}
                }
            }
            Ok::<(String, String), anyhow::Error>((text, thoughts))
        };
        let text = match tokio::time::timeout(TITLE_TIMEOUT, collect).await {
            Ok(Ok((text, thoughts))) => Ok(salvage_title_source(text, thoughts)),
            Ok(Err(err)) => Err(format!("Ошибка генерации названия: {err}")),
            Err(_) => {
                cancel.cancel();
                Err("Генерация названия превысила лимит времени".to_string())
            }
        };
        let _ = title_tx.send(TitleResult { chat_id, text });
    });
}

/// Выбирает сырой источник заголовка: основной ответ модели, а если он пуст
/// (модель не «завершила мысль» в рамках бюджета) — последнюю содержательную
/// строку рассуждений. Финальную нормализацию делает `clean_generated_title`.
fn salvage_title_source(text: String, thoughts: String) -> String {
    if !text.trim().is_empty() {
        return text;
    }
    thoughts
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Параметры запуска задачи генерации (agentic-loop).
struct GenSpawn {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    ctx: ToolContext,
    request: ChatRequest,
    cancel: CancellationToken,
    id: Uuid,
    chat_id: Uuid,
    max_rounds: u32,
    /// Эффективно разрешённые инструменты (защита от вызова отключённых).
    allowed: Vec<ToolId>,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenResult>,
}

/// Накопитель одного раунда стрима.
struct RoundOutput {
    text: String,
    thoughts: String,
    calls: Vec<ApiToolCall>,
    reason: FinishReason,
}

/// Запускает задачу клиентского agentic-loop (spec §6.3): стрим → при
/// `finish_reason=ToolCalls` исполнение инструментов → новый запрос, до
/// `max_rounds`. Эффекты и новые сообщения возвращаются оркестратору.
fn spawn_generation(spawn: GenSpawn) {
    let GenSpawn {
        backend,
        registry,
        ctx,
        mut request,
        cancel,
        id,
        chat_id,
        max_rounds,
        allowed,
        evt_tx,
        done_tx,
    } = spawn;

    tokio::spawn(async move {
        let mut messages: Vec<Message> = Vec::new();
        let mut effects: Vec<ChatEffect> = Vec::new();
        let mut round: u32 = 0;
        let reason;

        loop {
            let out = stream_round(&backend, request.clone(), &cancel, id, &evt_tx).await;

            // Раунд с вызовами инструментов — исполняем и продолжаем цикл.
            if out.reason == FinishReason::ToolCalls && !out.calls.is_empty() {
                if round >= max_rounds {
                    let _ = evt_tx.send(AppEvent::Error(format!(
                        "Достигнут лимит раундов инструментов ({max_rounds})."
                    )));
                    reason = FinishReason::Stop;
                    if let Some(m) = finalize_message(&out, &ctx) {
                        messages.push(m);
                    }
                    break;
                }
                round += 1;

                // assistant-ход с вызовами — в историю запроса и в домен.
                request.messages.push(ApiMessage::assistant_tool_calls(
                    out.text.clone(),
                    out.calls.clone(),
                ));
                let mut records: Vec<ToolCallRecord> = Vec::new();
                for call in &out.calls {
                    let args: serde_json::Value =
                        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                    let result = if !allowed.iter().any(|t| t == &call.name) {
                        // Защита: инструмент выключен глобально/в профиле.
                        format!("Инструмент {} недоступен (выключен).", call.name)
                    } else {
                        match registry.invoke(&call.name, &ctx, args.clone()).await {
                            Ok(outcome) => {
                                effects.extend(outcome.effects);
                                outcome.result
                            }
                            Err(err) => format!("Ошибка инструмента {}: {err}", call.name),
                        }
                    };
                    let _ = evt_tx.send(AppEvent::ToolCall {
                        generation_id: id,
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        result: result.clone(),
                    });
                    request.messages.push(ApiMessage::tool(&call.id, &result));
                    records.push(ToolCallRecord {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: args,
                        result: Some(result.clone()),
                    });
                    messages.push(tool_message(call, result));
                }
                // Доменное assistant-сообщение с tool-блоками (текст раунда + мысли).
                let mut am = Message::assistant(out.text.clone());
                if !out.thoughts.is_empty() {
                    am.thoughts = Some(out.thoughts.clone());
                }
                am.tool_calls = records;
                // Вставляем assistant ПЕРЕД tool-сообщениями этого раунда.
                let tool_msgs: Vec<Message> = messages.split_off(messages.len() - out.calls.len());
                messages.push(am);
                messages.extend(tool_msgs);
                continue;
            }

            // Финальный раунд (Stop/Length/Cancelled/Error или без вызовов).
            if let Some(m) = finalize_message(&out, &ctx) {
                messages.push(m);
            }
            reason = out.reason;
            break;
        }

        let _ = evt_tx.send(AppEvent::Finished {
            generation_id: id,
            reason,
        });
        let _ = done_tx.send(GenResult {
            id,
            chat_id,
            messages,
            effects,
        });
    });
}

/// Стримит один запрос, ретранслируя `Text`/`Thoughts` в UI и накапливая
/// tool-вызовы. Возвращает накопленный раунд.
async fn stream_round(
    backend: &Arc<dyn EngineBackend>,
    request: ChatRequest,
    cancel: &CancellationToken,
    id: Uuid,
    evt_tx: &UnboundedSender<AppEvent>,
) -> RoundOutput {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut acc = ToolCallAccumulator::default();
    let mut reason = FinishReason::Stop;

    match backend.chat_stream(request, cancel.clone()).await {
        Ok(mut stream) => {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        text.push_str(&t);
                        let _ = evt_tx.send(AppEvent::Chunk {
                            generation_id: id,
                            text: t,
                        });
                    }
                    ChatChunk::Thoughts(t) => {
                        thoughts.push_str(&t);
                        let _ = evt_tx.send(AppEvent::Thoughts {
                            generation_id: id,
                            text: t,
                        });
                    }
                    ChatChunk::ToolCall(delta) => acc.push(delta),
                    ChatChunk::Finished(r) => {
                        reason = r;
                        break;
                    }
                }
            }
        }
        Err(err) => {
            let _ = evt_tx.send(AppEvent::Error(format!("Ошибка генерации: {err}")));
            reason = FinishReason::Error;
        }
    }

    RoundOutput {
        text,
        thoughts,
        calls: acc.finish(),
        reason,
    }
}

/// Доменное tool-сообщение (роль `Tool`) с привязкой к вызову.
fn tool_message(call: &ApiToolCall, result: String) -> Message {
    let mut m = Message::new(MessageRole::Tool, result);
    m.tool_call_id = Some(call.id.clone());
    m.tool_name = Some(call.name.clone());
    m
}

/// Финальное assistant-сообщение хода (если есть текст/мысли) со снимком семплинга.
fn finalize_message(out: &RoundOutput, ctx: &ToolContext) -> Option<Message> {
    if out.text.is_empty() && out.thoughts.is_empty() {
        return None;
    }
    let mut m = Message::assistant(out.text.clone());
    if !out.thoughts.is_empty() {
        m.thoughts = Some(out.thoughts.clone());
    }
    m.metadata = Some(MessageMetadata {
        sampling: ctx.effective_sampling.clone(),
        model: None,
    });
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::supervisor::MockSupervisor;
    use crate::shared::api::mock::{MockBackend, MockEmbedder};
    use crate::shared::paths::Paths;

    fn test_embedder() -> Arc<dyn Embedder> {
        Arc::new(MockEmbedder::new(16))
    }

    /// Поднимает оркестратор на временном хранилище. Возвращает каналы и handle.
    fn spawn_orch(
        backend: Option<Arc<dyn EngineBackend>>,
    ) -> (
        tempfile::TempDir,
        UnboundedSender<AppCommand>,
        UnboundedReceiver<AppEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        spawn_orch_cfg(backend, AppConfig::default())
    }

    /// Как [`spawn_orch`], но с заданной конфигурацией (max_tool_rounds и т.п.).
    fn spawn_orch_cfg(
        backend: Option<Arc<dyn EngineBackend>>,
        config: AppConfig,
    ) -> (
        tempfile::TempDir,
        UnboundedSender<AppCommand>,
        UnboundedReceiver<AppEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, evt_rx) = unbounded_channel();
        let deps = OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config,
            supervisor: Arc::new(MockSupervisor::with_backend(backend)),
        };
        let handle = tokio::spawn(run(deps));
        (dir, cmd_tx, evt_rx, handle)
    }

    /// Дренирует события до первого, удовлетворяющего предикату (или закрытия).
    async fn wait_for<F: Fn(&AppEvent) -> bool>(
        rx: &mut UnboundedReceiver<AppEvent>,
        pred: F,
    ) -> Option<AppEvent> {
        while let Some(ev) = rx.recv().await {
            if pred(&ev) {
                return Some(ev);
            }
        }
        None
    }

    /// Собирает «голый» оркестратор для юнит-тестов чистых методов (без петли),
    /// отбрасывая поток событий.
    fn bare_orch() -> (tempfile::TempDir, Orchestrator) {
        let (dir, orch, _rx) = bare_orch_rx();
        (dir, orch)
    }

    /// Как [`bare_orch`], но возвращает и приёмник событий (для проверки эмиссии).
    fn bare_orch_rx() -> (tempfile::TempDir, Orchestrator, UnboundedReceiver<AppEvent>) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let (evt_tx, evt_rx) = unbounded_channel();
        let (done_tx, _done_rx) = unbounded_channel();
        let (status_tx, _status_rx) = unbounded_channel();
        let (title_tx, _title_rx) = unbounded_channel();
        let config = AppConfig {
            default_sampling: SamplingConfig {
                temperature: Some(0.1),
                ..Default::default()
            },
            ..Default::default()
        };
        let registry = Arc::new(build_registry(&config));
        let orch = Orchestrator {
            evt_tx,
            supervisor: Arc::new(MockSupervisor::with_backend(None)),
            backend: None,
            chat_handle: None,
            embed_handle: None,
            embedder: test_embedder(),
            storage,
            config,
            registry,
            status_tx,
            title_tx,
            server_status: ServerStatus::Ready,
            profiles: Vec::new(),
            chats: Vec::new(),
            active_id: None,
            state: State::Idle,
            done_tx,
            dirty: HashSet::new(),
            save_deadline: None,
        };
        (dir, orch, evt_rx)
    }

    #[test]
    fn effective_sampling_resolves_three_tiers() {
        let (_d, mut orch) = bare_orch();
        let mut profile = Profile::new("P", "sys");
        profile.default_sampling = Some(SamplingConfig {
            temperature: Some(0.5),
            ..Default::default()
        });
        let chat = Chat::from_profile(&profile, "c");
        let chat_id = chat.id;
        orch.profiles.push(profile);
        orch.chats.push(chat);

        // override = None → берётся дефолт профиля.
        assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.5));

        // override = Some → берётся он (приоритет чата).
        orch.chats[0].sampling_override = Some(SamplingConfig {
            temperature: Some(0.9),
            ..Default::default()
        });
        assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.9));

        // нет ни override, ни дефолта профиля → глобальный.
        orch.chats[0].sampling_override = None;
        orch.profiles[0].default_sampling = None;
        assert_eq!(orch.effective_sampling(chat_id).temperature, Some(0.1));
    }

    #[test]
    fn new_chat_value_uses_chosen_profile_with_greeting() {
        let (_d, mut orch) = bare_orch();
        let p1 = Profile::new("A", "sys A");
        let mut p2 = Profile::new("B", "sys B");
        p2.greeting = Some("Здравствуйте!".into());
        let (id1, id2) = (p1.id, p2.id);
        orch.profiles.push(p1);
        orch.profiles.push(p2);

        let chat = orch.new_chat_value(Some(id2));
        assert_eq!(chat.profile_id, id2);
        assert_eq!(chat.system_message, "sys B");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, MessageRole::Assistant);
        assert_eq!(chat.messages[0].text, "Здравствуйте!");

        // None → первый профиль, без приветствия.
        let chat = orch.new_chat_value(None);
        assert_eq!(chat.profile_id, id1);
        assert!(chat.messages.is_empty());
    }

    #[test]
    fn copy_chat_emits_clipboard_text_or_error_when_empty() {
        let (_d, mut orch, mut rx) = bare_orch_rx();
        let profile = Profile::new("P", "sys");

        // Чат с перепиской → событие CopyToClipboard с текстом ролей.
        let mut chat = Chat::from_profile(&profile, "Чат");
        let id = chat.id;
        chat.push_message(Message::user("привет"));
        chat.push_message(Message::assistant("здравствуйте"));
        orch.chats.push(chat);
        orch.handle_copy_chat(id);
        match rx.try_recv().unwrap() {
            AppEvent::CopyToClipboard(text) => {
                assert!(text.contains("Пользователь:\nпривет"));
                assert!(text.contains("Ассистент:\nздравствуйте"));
            }
            other => panic!("ожидался CopyToClipboard, получено {other:?}"),
        }

        // Пустой чат (нет сообщений) → ошибка списка, не текст.
        let empty = Chat::from_profile(&profile, "Пустой");
        let empty_id = empty.id;
        orch.chats.push(empty);
        orch.handle_copy_chat(empty_id);
        assert!(matches!(rx.try_recv().unwrap(), AppEvent::ChatListError(_)));
    }

    #[tokio::test]
    async fn bootstrap_emits_chat_list_and_active_chat() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);

        let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();
        if let AppEvent::ChatList(chats) = list {
            assert_eq!(chats.len(), 1, "должен создаться один чат по умолчанию");
        }
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        assert!(matches!(active, AppEvent::ChatActivated { .. }));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn send_streams_and_persists_assistant_message() {
        let backend = Arc::new(MockBackend::scripted(vec![
            ChatChunk::Thoughts("думаю".into()),
            ChatChunk::Text("Привет".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ])) as Arc<dyn EngineBackend>;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        let root = _d.path().to_path_buf();

        // Ждём активации и узнаём id активного чата.
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let chat_id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::SendMessage("привет".into()))
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();

        // Завершаем и проверяем, что чат сохранён с двумя сообщениями.
        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        let reopened = Storage::open(Paths::with_root(&root)).unwrap();
        let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, MessageRole::User);
        assert_eq!(chat.messages[1].role, MessageRole::Assistant);
        assert_eq!(chat.messages[1].text, "Привет");
        assert_eq!(chat.messages[1].thoughts.as_deref(), Some("думаю"));
    }

    #[tokio::test]
    async fn regenerate_replaces_last_assistant_message() {
        // Два разных ответа по очереди: исходный ход → «первый», перегенерация → «второй».
        let backend = Arc::new(MockBackend::sequence(vec![
            vec![
                ChatChunk::Text("первый".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
            vec![
                ChatChunk::Text("второй".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
        ])) as Arc<dyn EngineBackend>;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        let root = _d.path().to_path_buf();
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let chat_id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::SendMessage("вопрос".into()))
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        // ChatList после Finished — признак, что handle_done применил ответ (state Idle).
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::RegenerateLast).unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        // Старый ответ заменён новым; сообщение пользователя не дублируется.
        let reopened = Storage::open(Paths::with_root(&root)).unwrap();
        let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
        assert_eq!(chat.messages.len(), 2, "{:?}", chat.messages);
        assert_eq!(chat.messages[0].role, MessageRole::User);
        assert_eq!(chat.messages[0].text, "вопрос");
        assert_eq!(chat.messages[1].role, MessageRole::Assistant);
        assert_eq!(chat.messages[1].text, "второй");
    }

    #[tokio::test]
    async fn delete_last_exchange_restores_user_text() {
        let backend = Arc::new(MockBackend::scripted(vec![
            ChatChunk::Text("ответ".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ])) as Arc<dyn EngineBackend>;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        let root = _d.path().to_path_buf();
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let chat_id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::SendMessage("забудь это".into()))
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::DeleteLastExchange).unwrap();
        let restore = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::RestoreInput(_)))
            .await
            .unwrap();
        assert!(matches!(restore, AppEvent::RestoreInput(t) if t == "забудь это"));

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        // Обмен удалён полностью (дефолтный чат без приветствия → пусто).
        let reopened = Storage::open(Paths::with_root(&root)).unwrap();
        let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
        assert!(chat.messages.is_empty(), "{:?}", chat.messages);
    }

    #[tokio::test]
    async fn auto_rename_sets_title_from_model() {
        // Первый запрос (отправка) → «ответ»; второй (авто-название) → заголовок.
        let backend = Arc::new(MockBackend::sequence(vec![
            vec![
                ChatChunk::Text("ответ".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
            vec![
                ChatChunk::Text("«Тема разговора»".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
        ])) as Arc<dyn EngineBackend>;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let chat_id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        // Нужна хотя бы одна реплика, иначе нечего озаглавливать.
        cmd_tx
            .send(AppCommand::SendMessage("привет".into()))
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::AutoRenameChat(chat_id)).unwrap();
        let renamed = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatRenamed { .. }))
            .await
            .unwrap();
        match renamed {
            AppEvent::ChatRenamed { id, title } => {
                assert_eq!(id, chat_id);
                assert_eq!(title, "Тема разговора", "кавычки модели сняты");
            }
            _ => unreachable!(),
        }

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }

    #[test]
    fn salvage_prefers_text_else_last_thought_line() {
        // Есть основной ответ — берём его.
        assert_eq!(
            salvage_title_source("Заголовок".into(), "мысли".into()),
            "Заголовок"
        );
        // Ответ пуст — спасаем последнюю содержательную строку рассуждений.
        assert_eq!(
            salvage_title_source("  ".into(), "рассуждаю\nитог: Планы\n\n".into()),
            "итог: Планы"
        );
        // Совсем пусто — пустая строка (clean_generated_title вернёт None → ошибка).
        assert_eq!(salvage_title_source(String::new(), String::new()), "");
    }

    #[test]
    fn auto_rename_without_messages_emits_error() {
        let (_d, mut orch, mut rx) = bare_orch_rx();
        let profile = Profile::new("P", "sys");
        let chat = Chat::from_profile(&profile, "Новый чат"); // без сообщений
        let chat_id = chat.id;
        orch.profiles.push(profile);
        orch.chats.push(chat);

        orch.handle_auto_rename(chat_id);
        // Пустой чат → ошибка в область списка чатов, фоновая задача не запускается.
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, AppEvent::ChatListError(_)));
    }

    #[tokio::test]
    async fn regenerate_without_user_message_is_noop() {
        // Чат с приветствием-ассистентом, но без сообщения пользователя — нечего
        // перегенерировать; команда не должна стартовать генерацию.
        let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
            FinishReason::Stop,
        )])) as Arc<dyn EngineBackend>;
        let (_d, mut orch) = bare_orch();
        orch.backend = Some(backend);
        let mut profile = Profile::new("P", "sys");
        profile.greeting = Some("Привет!".into());
        let mut chat = Chat::from_profile(&profile, "c");
        chat.push_message(Message::assistant("Привет!"));
        let chat_id = chat.id;
        orch.profiles.push(profile);
        orch.chats.push(chat);
        orch.active_id = Some(chat_id);

        orch.handle_regenerate();
        // Состояние осталось Idle (генерация не запущена), история не тронута.
        assert!(matches!(orch.state, State::Idle));
        assert_eq!(orch.chats[0].messages.len(), 1);
    }

    #[test]
    fn regenerate_on_not_ready_server_keeps_reply() {
        // Сервер ещё подключается (managed грузит модель) — перегенерация не должна
        // ни сносить прежний ответ, ни уходить запросом на не-готовый сервер (иначе
        // 503 «engine returned an error status» и потеря ответа). Регресс-тест.
        let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
            FinishReason::Stop,
        )])) as Arc<dyn EngineBackend>;
        let (_d, mut orch) = bare_orch();
        orch.backend = Some(backend);
        orch.server_status = ServerStatus::Connecting; // ещё не готов
        let profile = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&profile, "c");
        chat.push_message(Message::user("вопрос"));
        chat.push_message(Message::assistant("старый ответ"));
        let chat_id = chat.id;
        orch.profiles.push(profile);
        orch.chats.push(chat);
        orch.active_id = Some(chat_id);

        orch.handle_regenerate();

        // Ответ сохранён, генерация не стартовала (история не усечена).
        assert!(matches!(orch.state, State::Idle));
        assert_eq!(
            orch.chats[0].messages.len(),
            2,
            "прежний ответ не должен быть снесён на не-готовом сервере"
        );
        assert_eq!(orch.chats[0].messages[1].text, "старый ответ");
    }

    #[test]
    fn send_on_not_ready_server_restores_input() {
        // Сервер ещё подключается — отправка отклоняется, но текст возвращается в
        // поле ввода (RestoreInput), а в чат сообщение не добавляется.
        let backend = Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>;
        let (_d, mut orch, mut rx) = bare_orch_rx();
        orch.backend = Some(backend);
        orch.server_status = ServerStatus::Connecting;
        let profile = Profile::new("P", "sys");
        let chat = Chat::from_profile(&profile, "c");
        let chat_id = chat.id;
        orch.profiles.push(profile);
        orch.chats.push(chat);
        orch.active_id = Some(chat_id);

        orch.handle_send("привет".into());

        assert!(matches!(orch.state, State::Idle));
        assert!(orch.chats[0].messages.is_empty(), "сообщение не добавлено");
        // Среди эмитнутых событий — ошибка и возврат текста в поле ввода.
        let mut got_error = false;
        let mut restored = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Error(_) => got_error = true,
                AppEvent::RestoreInput(t) => restored = Some(t),
                _ => {}
            }
        }
        assert!(got_error, "должна быть эмитнута ошибка о неготовности");
        assert_eq!(restored.as_deref(), Some("привет"));
    }

    #[test]
    fn ready_backend_gates_by_status() {
        let (_d, mut orch) = bare_orch();
        orch.backend = Some(Arc::new(MockBackend::scripted(vec![])) as Arc<dyn EngineBackend>);

        orch.server_status = ServerStatus::Ready;
        assert!(orch.ready_backend().is_some());

        for status in [
            ServerStatus::Connecting,
            ServerStatus::NotConfigured,
            ServerStatus::Disconnected("боль".into()),
        ] {
            orch.server_status = status;
            assert!(orch.ready_backend().is_none());
        }
    }

    #[tokio::test]
    async fn new_chat_adds_to_list_and_activates() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();

        cmd_tx
            .send(AppCommand::NewChat { profile_id: None })
            .unwrap();
        let list = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
        )
        .await
        .unwrap();
        assert!(matches!(list, AppEvent::ChatList(c) if c.len() == 2));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn rename_updates_chat_list() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::RenameChat {
                id,
                title: "Переименован".into(),
            })
            .unwrap();
        let list = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatList(c) if c.iter().any(|s| s.title == "Переименован")),
        )
        .await
        .unwrap();
        assert!(matches!(list, AppEvent::ChatList(_)));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn delete_active_chat_creates_replacement() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx.send(AppCommand::DeleteChat(id)).unwrap();
        // После удаления единственного чата создаётся новый и активируется.
        let active2 = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatActivated { id: nid, .. } if *nid != id),
        )
        .await
        .unwrap();
        assert!(matches!(active2, AppEvent::ChatActivated { .. }));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancel_stops_generation_and_saves_partial() {
        let backend = Arc::new(MockBackend::cancellable(vec![ChatChunk::Text(
            "часть".into(),
        )])) as Arc<dyn EngineBackend>;
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

        let mut sent_cancel = false;
        let mut cancelled = false;
        while let Some(ev) = evt_rx.recv().await {
            match ev {
                AppEvent::Chunk { .. } if !sent_cancel => {
                    cmd_tx.send(AppCommand::Cancel).unwrap();
                    sent_cancel = true;
                }
                AppEvent::Finished { reason, .. } => {
                    assert_eq!(reason, FinishReason::Cancelled);
                    cancelled = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(cancelled);

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn agentic_loop_executes_tool_then_finalizes() {
        use crate::shared::api::backend::ToolCallDelta;
        // Раунд 1: вызов note_save → раунд 2: финальный текст.
        let backend = Arc::new(MockBackend::sequence(vec![
            vec![
                ChatChunk::ToolCall(ToolCallDelta {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("note_save".into()),
                    arguments: "{\"content\":\"любит чай\"}".into(),
                }),
                ChatChunk::Finished(FinishReason::ToolCalls),
            ],
            vec![
                ChatChunk::Text("Запомнил.".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
        ])) as Arc<dyn EngineBackend>;

        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        let root = _d.path().to_path_buf();
        let active = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        let chat_id = match active {
            AppEvent::ChatActivated { id, .. } => id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::SendMessage("запомни про чай".into()))
            .unwrap();

        // Событие исполнения инструмента доходит до UI.
        let tool_ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
            .await
            .unwrap();
        assert!(matches!(tool_ev, AppEvent::ToolCall { name, .. } if name == "note_save"));
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        // История: user → assistant(tool_calls) → tool → assistant(финал).
        let reopened = Storage::open(Paths::with_root(&root)).unwrap();
        let chat = reopened.json().load_chat(chat_id).unwrap().unwrap();
        assert_eq!(chat.messages.len(), 4, "{:?}", chat.messages);
        assert_eq!(chat.messages[0].role, MessageRole::User);
        assert_eq!(chat.messages[1].role, MessageRole::Assistant);
        assert_eq!(chat.messages[1].tool_calls.len(), 1);
        assert_eq!(chat.messages[1].tool_calls[0].name, "note_save");
        assert_eq!(chat.messages[2].role, MessageRole::Tool);
        assert_eq!(chat.messages[3].text, "Запомнил.");

        // Заметка действительно сохранена инструментом (изоляция по профилю).
        let notes = reopened
            .db()
            .note_list(chat.profile_id, None, &[], None)
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.contains("любит чай"));
    }

    #[tokio::test]
    async fn disabled_tool_is_refused_by_loop() {
        use crate::shared::api::backend::ToolCallDelta;
        // python_exec выключен глобально (spawn_orch: python_enabled = false) —
        // даже если модель его вызовет, loop откажет, не исполняя.
        let backend = Arc::new(MockBackend::sequence(vec![
            vec![
                ChatChunk::ToolCall(ToolCallDelta {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("python_exec".into()),
                    arguments: "{\"code\":\"print(1)\"}".into(),
                }),
                ChatChunk::Finished(FinishReason::ToolCalls),
            ],
            vec![
                ChatChunk::Text("ок".into()),
                ChatChunk::Finished(FinishReason::Stop),
            ],
        ])) as Arc<dyn EngineBackend>;

        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(Some(backend));
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        cmd_tx
            .send(AppCommand::SendMessage("посчитай".into()))
            .unwrap();

        let tool_ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ToolCall { .. }))
            .await
            .unwrap();
        match tool_ev {
            AppEvent::ToolCall { name, result, .. } => {
                assert_eq!(name, "python_exec");
                assert!(result.contains("недоступен"), "got: {result}");
            }
            _ => unreachable!(),
        }
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn tool_round_limit_is_respected() {
        use crate::shared::api::backend::ToolCallDelta;
        // Движок всегда просит инструмент — должен сработать лимит раундов.
        let backend = Arc::new(MockBackend::scripted(vec![
            ChatChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("c1".into()),
                name: Some("get_sampling".into()),
                arguments: "{}".into(),
            }),
            ChatChunk::Finished(FinishReason::ToolCalls),
        ])) as Arc<dyn EngineBackend>;

        let config = AppConfig {
            max_tool_rounds: 2,
            ..Default::default()
        };
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch_cfg(Some(backend), config);
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        cmd_tx
            .send(AppCommand::SendMessage("зациклись".into()))
            .unwrap();

        // Дойдём до Finished; лимит породит ошибку-пометку, но генерация завершится.
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
            .await
            .unwrap();
        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn send_without_backend_emits_error() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();
        cmd_tx.send(AppCommand::SendMessage("hi".into())).unwrap();

        let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
            .await
            .unwrap();
        assert!(matches!(err, AppEvent::Error(_)));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn create_profile_appears_in_profile_list() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        // Бутстрап создаёт один профиль по умолчанию.
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
        )
        .await
        .unwrap();

        cmd_tx
            .send(AppCommand::CreateProfile {
                name: "  Второй  ".into(),
                system_message: "sys".into(),
            })
            .unwrap();
        let list = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
        )
        .await
        .unwrap();
        if let AppEvent::ProfileList(profiles) = list {
            assert!(profiles.iter().any(|p| p.name == "Второй")); // имя нормализовано
        }

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn delete_profile_cascades_to_its_chats() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
            .await
            .unwrap();

        // Создаём второй профиль и узнаём его id.
        cmd_tx
            .send(AppCommand::CreateProfile {
                name: "Второй".into(),
                system_message: "sys".into(),
            })
            .unwrap();
        let list = wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
        )
        .await
        .unwrap();
        let second_id = match list {
            AppEvent::ProfileList(profiles) => {
                profiles.iter().find(|p| p.name == "Второй").unwrap().id
            }
            _ => unreachable!(),
        };

        // Создаём чат из второго профиля → всего два чата.
        cmd_tx
            .send(AppCommand::NewChat {
                profile_id: Some(second_id),
            })
            .unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
        )
        .await
        .unwrap();

        // Удаляем второй профиль: его чат каскадно скрывается → остаётся один.
        cmd_tx.send(AppCommand::DeleteProfile(second_id)).unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
        )
        .await
        .unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::ChatList(c) if c.len() == 1),
        )
        .await
        .unwrap();

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cannot_delete_last_profile() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
            .await
            .unwrap();
        let only_id = match list {
            AppEvent::ProfileList(p) => p[0].id,
            _ => unreachable!(),
        };

        cmd_tx.send(AppCommand::DeleteProfile(only_id)).unwrap();
        let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
            .await
            .unwrap();
        assert!(matches!(err, AppEvent::Error(_)));

        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn bootstrap_emits_settings_snapshot() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
            .await
            .unwrap();
        if let AppEvent::Settings { config, profiles } = ev {
            assert_eq!(config.schema_version, AppConfig::default().schema_version);
            assert_eq!(profiles.len(), 1, "дефолтный профиль в снимке");
        }
        drop(cmd_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn update_config_persists_and_reemits_settings() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let root = _d.path().to_path_buf();
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
            .await
            .unwrap();

        let config = AppConfig {
            max_tool_rounds: 3,
            ..Default::default()
        };
        cmd_tx
            .send(AppCommand::UpdateConfig(Box::new(config)))
            .unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Settings { config, .. } if config.max_tool_rounds == 3),
        )
        .await
        .unwrap();

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();

        // Конфиг сохранён на диск.
        let reopened = Storage::open(Paths::with_root(&root)).unwrap();
        assert_eq!(reopened.json().load_config().unwrap().max_tool_rounds, 3);
    }

    #[tokio::test]
    async fn update_profile_persists_edit() {
        let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
        let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
            .await
            .unwrap();
        let id = match ev {
            AppEvent::Settings { profiles, .. } => profiles[0].id,
            _ => unreachable!(),
        };

        cmd_tx
            .send(AppCommand::UpdateProfile {
                id,
                edit: Box::new(ProfileEdit {
                    system_message: Some("новое sys".into()),
                    ..Default::default()
                }),
            })
            .unwrap();
        wait_for(&mut evt_rx, |e| {
            matches!(e, AppEvent::Settings { profiles, .. }
                if profiles.iter().any(|p| p.default_system_message == "новое sys"))
        })
        .await
        .unwrap();

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn model_change_restarts_chat_server() {
        let backend = Arc::new(MockBackend::scripted(vec![ChatChunk::Finished(
            FinishReason::Stop,
        )])) as Arc<dyn EngineBackend>;
        let sup = Arc::new(MockSupervisor::with_backend(Some(backend)));
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(Paths::with_root(dir.path())).unwrap());
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let handle = tokio::spawn(run(OrchestratorDeps {
            cmd_rx,
            evt_tx,
            storage,
            config: AppConfig::default(),
            supervisor: sup.clone(),
        }));
        wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Settings { .. }))
            .await
            .unwrap();
        // Бутстрап поднял сервер один раз.
        assert_eq!(sup.chat_call_count(), 1);

        // Смена модели → перезапуск (повторный apply_chat, spec §11.6 DoD).
        let config = AppConfig {
            engine: crate::shared::config::EngineSettings {
                model_path: Some("other.gguf".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        cmd_tx
            .send(AppCommand::UpdateConfig(Box::new(config)))
            .unwrap();
        wait_for(
            &mut evt_rx,
            |e| matches!(e, AppEvent::Settings { config, .. } if config.engine.model_path.as_deref() == Some("other.gguf")),
        )
        .await
        .unwrap();
        assert_eq!(
            sup.chat_call_count(),
            2,
            "смена модели должна перезапустить сервер"
        );

        cmd_tx.send(AppCommand::Quit).unwrap();
        handle.await.unwrap();
    }

    #[test]
    fn build_request_puts_system_aside_and_maps_roles() {
        let mut p = Profile::new("X", "Ты — X.");
        p.greeting = Some("Здравствуйте!".into());
        let mut chat = Chat::from_profile(&p, "c");
        chat.push_message(Message::assistant("Здравствуйте!"));
        chat.push_message(Message::user("привет"));

        let req = build_request(&chat, SamplingConfig::default(), vec![]);
        assert_eq!(req.system.as_deref(), Some("Ты — X."));
        assert_eq!(req.messages.len(), 2);
    }
}
