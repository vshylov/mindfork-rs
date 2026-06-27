//! Оркестратор: единственный владелец доменного состояния (профили/чаты,
//! [`Storage`]) и автомат генерации. Принимает [`AppCommand`], исполняет
//! генерацию (в отдельной задаче) и рассылает [`AppEvent`].
//! См. spec §4.4 (однонаправленный поток, `generation_id`, автомат
//! `Idle/Generating/Cancelling`) и §4.4.2 (оркестратор — единственный писатель).
//!
//! Модуль разбит по фичам (god-объект расслоён, владелец `Chat` остался один):
//! - [`mod.rs`](self) — каркас: [`Orchestrator`], петля [`run`], диспетчер
//!   [`Orchestrator::handle_command`], общие хелперы (эмиттеры, `chat_mut`);
//! - [`engines`] — [`EngineManager`]: жизненный цикл серверов и готовность;
//! - [`save_queue`] — [`SaveQueue`]: дебаунс отложенного сохранения чатов;
//! - [`generation`] — отправка/перегенерация/удаление обмена + задача agentic-loop;
//! - [`chats`] — управление списком чатов и черновиком;
//! - [`profiles`] — создание/правка/удаление профилей;
//! - [`settings`] — конфиг и (пере)запуск серверов через супервайзер;
//! - [`title`] — авто-название чата (фоновая задача);
//! - [`impersonation`] — написание реплики «за пользователя» (фоновая задача);
//! - [`rag`] — индексация/удаление файлов в базе знаний;
//! - [`request`] — маппинг доменных сообщений в формат движка.

mod chats;
mod engines;
mod generation;
mod impersonation;
mod profiles;
mod rag;
mod request;
mod save_queue;
mod settings;
mod title;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, ServerStatus};
use crate::app::gen_state::GenState;
use crate::app::supervisor::ServerSupervisor;
use crate::entities::chat::{Chat, ChatSummary};
use crate::entities::message::Message;
use crate::entities::profile::Profile;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::FinishReason;
use crate::shared::config::AppConfig;
use crate::shared::storage::Storage;

use self::engines::EngineManager;
use self::generation::GenResult;
use self::save_queue::SaveQueue;
use self::title::TitleResult;

/// Системное сообщение профиля по умолчанию (создаётся при пустом хранилище).
const DEFAULT_SYSTEM_MESSAGE: &str =
    "Ты — полезный ассистент. Отвечай ясно и по существу на языке пользователя.";

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
    // Внутренний канал статуса сервера имперсонации (фоновый probe).
    let (imp_status_tx, mut imp_status_rx) = unbounded_channel::<ServerStatus>();
    // Внутренний канал «имперсонация завершена» (фоновая задача → петля).
    let (imp_done_tx, mut imp_done_rx) = unbounded_channel::<(Uuid, FinishReason)>();
    let registry = Arc::new(build_registry(&config));
    let mut orch = Orchestrator {
        evt_tx,
        engines: EngineManager::new(supervisor, status_tx, imp_status_tx),
        imp_cancel: None,
        imp_gen: None,
        imp_done_tx,
        storage,
        config,
        registry,
        title_tx,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        gen_state: GenState::Idle,
        done_tx,
        rag_cancel: None,
        saves: SaveQueue::default(),
    };

    // Поднимаем серверы по конфигу и эмитим стартовые события/настройки.
    orch.apply_chat_settings();
    orch.apply_impersonation_settings();
    orch.apply_embed_settings();
    if let Err(err) = orch.bootstrap() {
        let _ = orch
            .evt_tx
            .send(AppEvent::Error(format!("Ошибка загрузки данных: {err}")));
    }
    orch.emit_settings();

    loop {
        let deadline = orch.saves.deadline();
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
                    orch.engines.set_chat_status(s.clone());
                    let _ = orch.evt_tx.send(AppEvent::ServerStatus(s));
                }
            }
            title = title_rx.recv() => {
                if let Some(res) = title {
                    orch.handle_title_result(res);
                }
            }
            status = imp_status_rx.recv() => {
                if let Some(s) = status {
                    orch.engines.set_imp_status(s);
                }
            }
            done = imp_done_rx.recv() => {
                if let Some((id, reason)) = done {
                    orch.handle_imp_done(id, reason);
                }
            }
            _ = sleep_until_opt(deadline) => orch.flush_saves(),
        }
    }
    orch.flush_saves();
}

/// Строит реестр инструментов из конфигурации (`config.tools`).
fn build_registry(config: &AppConfig) -> crate::features::tools::ToolRegistry {
    crate::features::tools::standard_registry(&crate::features::tools::ToolConfig {
        python_path: config.tools.python_path.clone(),
        subagent_max_tokens: config.tools.subagent_max_tokens,
        subagent_timeout: Duration::from_secs(config.tools.subagent_timeout_secs),
        web_fetch_content: config.tools.web_fetch_content,
        fs_root: config.tools.fs_root.clone(),
        // Режим chat-движка определяет доступные параметры семплинга в
        // get_sampling/set_sampling (схема + фильтрация). См. ADR 0004.
        sampling_provider: config.engine.mode.cloud_provider(),
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
    /// Серверы инференса/эмбеддингов и их готовность (выделено в Фазе 3).
    engines: EngineManager,
    /// Токен отмены текущей имперсонации и её generation_id (`None` — не идёт).
    imp_cancel: Option<tokio_util::sync::CancellationToken>,
    imp_gen: Option<Uuid>,
    /// Канал «имперсонация завершена» (фоновая задача → петля).
    imp_done_tx: UnboundedSender<(Uuid, FinishReason)>,
    storage: Arc<Storage>,
    /// Полная конфигурация (оркестратор — единственный писатель в `settings.json`).
    config: AppConfig,
    /// Реестр инструментов (пересобирается при правках `config.tools`).
    registry: Arc<crate::features::tools::ToolRegistry>,
    /// Канал результатов фоновой генерации авто-названий чатов.
    title_tx: UnboundedSender<TitleResult>,
    profiles: Vec<Profile>,
    /// Видимые чаты, целиком в памяти (оркестратор — единственный писатель).
    chats: Vec<Chat>,
    active_id: Option<Uuid>,
    /// Автомат жизненного цикла генерации ответа ассистента (см. `gen_state`).
    gen_state: GenState,
    done_tx: UnboundedSender<GenResult>,
    /// Токен отмены текущей фоновой индексации RAG (`/rag add`); `None` — не идёт.
    /// Снимается/отменяется при новой индексации и при завершении работы.
    rag_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Очередь отложенного сохранения чатов (дебаунс; выделено в Фазе 3).
    saves: SaveQueue,
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
        // Сверяем инструменты профилей с текущим набором по умолчанию: новые
        // инструменты приложения включаются в существующих профилях (выключенные
        // пользователем — нет). См. spec §9.4 и `features::profiles::reconcile_tools`.
        for profile in &mut self.profiles {
            if crate::features::profiles::reconcile_tools(profile) {
                let _ = self.storage.json().upsert_profile(profile);
            }
        }
        if self.profiles.is_empty() {
            let mut profile = Profile::new("Ассистент", DEFAULT_SYSTEM_MESSAGE);
            // Включаем все базовые инструменты в дефолтном профиле.
            crate::features::profiles::reconcile_tools(&mut profile);
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
                if let Some(token) = self.gen_state.active_cancel() {
                    token.cancel();
                }
                if let Some(token) = &self.rag_cancel {
                    token.cancel();
                }
                if let Some(token) = &self.imp_cancel {
                    token.cancel();
                }
                return true;
            }
            AppCommand::Cancel => {
                if let Some(token) = self.gen_state.request_cancel() {
                    token.cancel();
                }
            }
            AppCommand::SendMessage(text) => self.handle_send(text),
            AppCommand::Impersonate { seed } => self.handle_impersonate(seed),
            AppCommand::CancelImpersonation => self.handle_cancel_impersonation(),
            AppCommand::SetDraft(text) => self.handle_set_draft(text),
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
            AppCommand::RagAdd { path, recursive } => self.handle_rag_add(path, recursive),
            AppCommand::RagDelete { path } => self.handle_rag_delete(path),
            AppCommand::RagList => self.handle_rag_list(),
            AppCommand::RagRebuild => self.handle_rag_rebuild(),
        }
        false
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

    // ---------- вспомогательное (общее для подмодулей) ----------

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
            draft: chat.draft.clone(),
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
        self.saves.mark(id);
    }

    /// Сохраняет все грязные чаты на диск.
    fn flush_saves(&mut self) {
        for id in self.saves.take() {
            if let Some(chat) = self.chats.iter().find(|c| c.id == id)
                && let Err(err) = self.storage.json().save_chat(chat)
            {
                tracing::error!(chat = %id, error = %err, "не удалось сохранить чат");
            }
        }
    }
}
