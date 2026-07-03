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
mod consolidation;
mod engines;
mod generation;
mod impersonation;
mod profiles;
mod rag;
mod reflection;
mod request;
mod save_queue;
mod settings;
mod title;
mod tool_loop;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
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
    // Внутренний канал «авто-рефлексия завершена» (фоновая задача → петля); несёт
    // исход (`Ok`/`Err(причина)`) для наблюдаемости.
    let (reflect_done_tx, mut reflect_done_rx) = unbounded_channel::<Result<(), String>>();
    // Внутренний канал «авто-консолидация завершена» (фоновая задача → петля).
    let (consolidate_done_tx, mut consolidate_done_rx) = unbounded_channel::<Result<(), String>>();
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
        reflect_cancel: None,
        reflect_done_tx,
        reflect_failures: 0,
        consolidate_cancel: None,
        consolidate_counts: HashMap::new(),
        consolidate_done_tx,
        consolidate_failures: 0,
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
                    orch.engines.set_chat_status(s);
                    orch.emit_server_status();
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
                    orch.emit_server_status();
                }
            }
            done = imp_done_rx.recv() => {
                if let Some((id, reason)) = done {
                    orch.handle_imp_done(id, reason);
                }
            }
            done = reflect_done_rx.recv() => {
                if let Some(res) = done {
                    orch.handle_reflect_done(res);
                }
            }
            done = consolidate_done_rx.recv() => {
                if let Some(res) = done {
                    orch.handle_consolidate_done(res);
                }
            }
            _ = sleep_until_opt(deadline) => orch.flush_saves(),
        }
    }
    orch.flush_saves();
}

/// Сколько подряд идущих неудач фоновой задачи (рефлексия/консолидация) должно
/// накопиться, чтобы один раз показать ошибку в UI. Дальше — молчим до первого
/// успеха (сброс счётчика). Наблюдаемость без спама. См. этап 5 доводки.
pub(super) const BACKGROUND_FAILURE_ALERT: u32 = 3;

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
    /// Токен отмены текущей фоновой авто-рефлексии (`Some` — идёт; одна за раз).
    reflect_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Канал «авто-рефлексия завершена» (фоновая задача → петля), несёт исход. Каденция
    /// рефлексии ведётся ватермарком `Chat.reflected_upto` (переживает рестарт), а не
    /// in-memory счётчиком — см. `reflection::maybe_auto_reflect`.
    reflect_done_tx: UnboundedSender<Result<(), String>>,
    /// Число подряд идущих неудач авто-рефлексии; на пороге эмитим одну ошибку в UI,
    /// дальше молчим до первого успеха (сброс). Наблюдаемость без спама.
    reflect_failures: u32,
    /// Токен отмены текущей фоновой авто-консолидации заметок («сон»); одна за раз.
    consolidate_cancel: Option<tokio_util::sync::CancellationToken>,
    /// Счётчики ответов ассистента с прошлой авто-консолидации (по чату).
    consolidate_counts: HashMap<Uuid, u32>,
    /// Канал «авто-консолидация завершена» (фоновая задача → петля), несёт исход.
    consolidate_done_tx: UnboundedSender<Result<(), String>>,
    /// Число подряд идущих неудач авто-консолидации (как `reflect_failures`).
    consolidate_failures: u32,
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
                if let Some(token) = &self.reflect_cancel {
                    token.cancel();
                }
                if let Some(token) = &self.consolidate_cancel {
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
            AppCommand::RequestSelfModel => self.handle_request_self_model(),
            AppCommand::UpdateSelfModel(edit) => self.handle_update_self_model(edit),
        }
        false
    }

    /// Отдаёт снимок «модели себя» активного профиля для экрана просмотра (`F3`).
    /// Чтение из БД на месте (быстро); `None` — нет активного чата или модель ещё
    /// не создавалась. Ошибку чтения трактуем как «нет модели» (вид покажет пусто).
    /// Снимок «модели себя» профиля для экрана `F3`: блоб модели + наблюдения,
    /// реконструированные из self-заметок (нарратив переехал в заметки, Ярус 1).
    /// Наблюдения кладутся в поле `narrative` снимка **только для отображения** — сам
    /// снимок никогда не персистится (запись идёт через `self_model_update` над
    /// реальной, пустой по нарративу моделью). `None`, если нет ни модели, ни
    /// наблюдений. См. docs/history/narrative-as-notes.md.
    fn self_model_view_snapshot(
        &self,
        pid: uuid::Uuid,
    ) -> Option<crate::entities::self_model::SelfModel> {
        use crate::entities::self_model::{NarrativeSegment, SelfModel, SelfModelParams};
        let cap = SelfModelParams::from_settings(&self.config.self_model).max_narrative;
        let notes = crate::features::tools::notes::self_notes_recent(&self.storage, pid, cap);
        let model = self.storage.db().self_model_get(pid).ok().flatten();
        if model.is_none() && notes.is_empty() {
            return None;
        }
        let mut m = model.unwrap_or_else(|| SelfModel::new(pid));
        m.narrative = notes
            .into_iter()
            .map(|n| NarrativeSegment {
                id: n.id,
                text: n.content,
                created_at: n.created_at,
            })
            .collect();
        Some(m)
    }

    fn handle_request_self_model(&self) {
        let snapshot = self
            .active_profile_id()
            .and_then(|pid| self.self_model_view_snapshot(pid));
        let _ = self
            .evt_tx
            .send(AppEvent::SelfModelView(Box::new(snapshot)));
    }

    /// Применяет ручную правку «модели себя» активного профиля (UI-редактор `F3`):
    /// загружает (или создаёт пустую), применяет правку, при изменении — сохраняет,
    /// затем переэмитит обновлённый снимок (открытый экран обновится на месте).
    fn handle_update_self_model(&self, edit: crate::entities::self_model::SelfModelEdit) {
        use crate::entities::self_model::SelfModelEdit;
        let Some(pid) = self.active_profile_id() else {
            return;
        };
        // Наблюдения — self-заметки, поэтому их удаление/полная очистка идут по
        // заметкам, а не по блобу модели (нарратив переехал в заметки, Ярус 1).
        match &edit {
            SelfModelEdit::DeleteInsight(id) => {
                let _ = self.storage.db().note_delete(pid, *id);
                let snapshot = self.self_model_view_snapshot(pid);
                let _ = self
                    .evt_tx
                    .send(AppEvent::SelfModelView(Box::new(snapshot)));
                return;
            }
            SelfModelEdit::Clear => {
                // Полная очистка сносит и наблюдения-заметки (@self), и блоб (ниже).
                for n in
                    crate::features::tools::notes::self_notes_recent(&self.storage, pid, usize::MAX)
                {
                    let _ = self.storage.db().note_delete(pid, n.id);
                }
            }
            _ => {}
        }
        // Атомарная правка (под одним захватом мьютекса БД) — не даёт параллельной
        // авто-рефлексии затереть ручную правку гонкой load-modify-save. Заодно
        // сворачиваем старые закрытые цели (единообразно с инструментами): fold
        // возвращает шрамы, которые пишем self-заметками после правки.
        let params =
            crate::entities::self_model::SelfModelParams::from_settings(&self.config.self_model);
        let mut scars: Vec<String> = Vec::new();
        let _ = self.storage.db().self_model_update(pid, |m| {
            let mut changed = m.apply_edit(edit);
            scars = m.fold_closed_goals(params.max_closed_goals);
            if !scars.is_empty() {
                changed = true;
            }
            changed
        });
        // Шрамы свёрнутых закрытых целей → self-заметки. Sync insert (вектор лениво).
        for scar in scars {
            let note = crate::entities::note::Note::new(
                pid,
                scar,
                vec![crate::features::tools::notes::SELF_NOTE_TAG.to_string()],
            );
            let _ = self.storage.db().note_insert(&note);
        }
        // Переэмитим авторитетный снимок (модель + наблюдения из self-заметок).
        let snapshot = self.self_model_view_snapshot(pid);
        let _ = self
            .evt_tx
            .send(AppEvent::SelfModelView(Box::new(snapshot)));
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

    /// Эмитит снимок статусов всех серверов (чат/эмбеддинги/имперсонация) в строку
    /// статуса. Зовётся при любом изменении любого из статусов (probe/смена настроек).
    fn emit_server_status(&self) {
        let _ = self
            .evt_tx
            .send(AppEvent::ServerStatus(self.engines.statuses()));
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
