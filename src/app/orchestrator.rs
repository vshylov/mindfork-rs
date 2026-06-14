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
use crate::entities::chat::{Chat, ChatSummary};
use crate::entities::message::{Message, MessageMetadata, MessageRole};
use crate::entities::profile::Profile;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason};
use crate::shared::storage::Storage;

/// Системное сообщение профиля по умолчанию (создаётся при пустом хранилище).
const DEFAULT_SYSTEM_MESSAGE: &str =
    "Ты — полезный ассистент. Отвечай ясно и по существу на языке пользователя.";

/// Дебаунс сохранения изменённых чатов на диск.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

/// Параметры запуска оркестратора.
pub struct OrchestratorDeps {
    pub cmd_rx: UnboundedReceiver<AppCommand>,
    pub evt_tx: UnboundedSender<AppEvent>,
    pub backend: Option<Arc<dyn EngineBackend>>,
    pub storage: Storage,
    /// Глобальный семплинг по умолчанию (низший уровень приоритета: переопределяется
    /// дефолтом профиля и override чата — см. `effective_sampling`, spec §8.3).
    pub default_sampling: SamplingConfig,
    pub status: ServerStatus,
}

/// Состояние генерации (автомат на активный чат).
enum State {
    Idle,
    Generating {
        id: Uuid,
        chat_id: Uuid,
        cancel: CancellationToken,
    },
    Cancelling {
        id: Uuid,
        chat_id: Uuid,
    },
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
    text: String,
    thoughts: String,
    reason: FinishReason,
}

/// Главный цикл оркестратора. Завершается при закрытии канала команд или
/// получении [`AppCommand::Quit`].
pub async fn run(deps: OrchestratorDeps) {
    let OrchestratorDeps {
        mut cmd_rx,
        evt_tx,
        backend,
        storage,
        default_sampling,
        status,
    } = deps;

    let _ = evt_tx.send(AppEvent::ServerStatus(status));

    let (done_tx, mut done_rx) = unbounded_channel::<GenResult>();
    let mut orch = Orchestrator {
        evt_tx,
        backend,
        storage,
        default_sampling,
        profiles: Vec::new(),
        chats: Vec::new(),
        active_id: None,
        state: State::Idle,
        done_tx,
        dirty: HashSet::new(),
        save_deadline: None,
    };

    if let Err(err) = orch.bootstrap() {
        let _ = orch
            .evt_tx
            .send(AppEvent::Error(format!("Ошибка загрузки данных: {err}")));
    }

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
            _ = sleep_until_opt(deadline) => orch.flush_saves(),
        }
    }
    orch.flush_saves();
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
    backend: Option<Arc<dyn EngineBackend>>,
    storage: Storage,
    default_sampling: SamplingConfig,
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
            let profile = Profile::new("Ассистент", DEFAULT_SYSTEM_MESSAGE);
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
                if let State::Generating {
                    id,
                    chat_id,
                    cancel,
                } = &self.state
                {
                    cancel.cancel();
                    self.state = State::Cancelling {
                        id: *id,
                        chat_id: *chat_id,
                    };
                }
            }
            AppCommand::SendMessage(text) => self.handle_send(text),
            AppCommand::NewChat { profile_id } => self.handle_new_chat(profile_id),
            AppCommand::SwitchChat(id) => self.handle_switch(id),
            AppCommand::RenameChat { id, title } => self.handle_rename(id, title),
            AppCommand::CloneChat(id) => self.handle_clone(id),
            AppCommand::DeleteChat(id) => self.handle_delete(id),
            AppCommand::CreateProfile {
                name,
                system_message,
            } => self.handle_create_profile(name, system_message),
            AppCommand::DeleteProfile(id) => self.handle_delete_profile(id),
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
        let Some(backend) = self.backend.clone() else {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("LLM-сервер не настроен".into()));
            return;
        };

        // Добавляем сообщение пользователя в активный чат.
        let sampling = self.effective_sampling(active_id);
        let request;
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.push_message(Message::user(&text));
            request = build_request(chat, sampling.clone());
        }
        self.mark_dirty(active_id);
        let _ = self.evt_tx.send(AppEvent::UserMessage(text));

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let _ = self
            .evt_tx
            .send(AppEvent::GenerationStarted { generation_id: id });
        self.state = State::Generating {
            id,
            chat_id: active_id,
            cancel: cancel.clone(),
        };
        spawn_generation(
            backend,
            request,
            cancel,
            id,
            active_id,
            self.evt_tx.clone(),
            self.done_tx.clone(),
        );
    }

    fn handle_done(&mut self, res: GenResult) {
        // Применяем только результат текущей генерации (защита от устаревших).
        if self.state.current_id() != Some(res.id) {
            return;
        }
        self.state = State::Idle;

        if res.text.is_empty() && res.thoughts.is_empty() {
            return;
        }
        let sampling = self.effective_sampling(res.chat_id);
        if let Some(chat) = self.chat_mut(res.chat_id) {
            let mut msg = Message::assistant(res.text);
            if !res.thoughts.is_empty() {
                msg.thoughts = Some(res.thoughts);
            }
            msg.metadata = Some(MessageMetadata {
                sampling,
                model: None,
            });
            chat.push_message(msg);
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
            id: gid,
            chat_id,
            cancel,
        } = &self.state
        {
            cancel.cancel();
            self.state = State::Cancelling {
                id: *gid,
                chat_id: *chat_id,
            };
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
            chat.title = title;
            self.mark_dirty(id);
            self.emit_chat_list();
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
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "Не удалось клонировать чат: {err}"
            )));
            return;
        }
        self.chats.insert(0, clone);
        self.emit_chat_list();
        self.activate(new_id);
    }

    fn handle_delete(&mut self, id: Uuid) {
        match self.storage.json().hide_chat(id) {
            Ok(false) => return,
            Err(err) => {
                let _ = self
                    .evt_tx
                    .send(AppEvent::Error(format!("Не удалось удалить чат: {err}")));
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
        crate::entities::sampling::resolve(chat_override, profile_default, &self.default_sampling)
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
/// передаются через [`ChatRequest::system`] (здесь — `None`).
fn message_to_api(message: &Message) -> Option<ApiMessage> {
    match message.role {
        MessageRole::System => None,
        MessageRole::User => Some(ApiMessage::user(&message.text)),
        MessageRole::Assistant => Some(ApiMessage::assistant(&message.text)),
        MessageRole::Tool => message
            .tool_call_id
            .as_ref()
            .map(|id| ApiMessage::tool(id, &message.text)),
    }
}

/// Строит запрос генерации из текущего состояния чата.
fn build_request(chat: &Chat, sampling: SamplingConfig) -> ChatRequest {
    let system = if chat.system_message.trim().is_empty() {
        None
    } else {
        Some(chat.system_message.clone())
    };
    ChatRequest {
        system,
        messages: chat.messages.iter().filter_map(message_to_api).collect(),
        sampling,
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_generation(
    backend: Arc<dyn EngineBackend>,
    req: ChatRequest,
    cancel: CancellationToken,
    id: Uuid,
    chat_id: Uuid,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<GenResult>,
) {
    tokio::spawn(async move {
        let mut text = String::new();
        let mut thoughts = String::new();
        let mut reason = FinishReason::Stop;
        match backend.chat_stream(req, cancel).await {
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
                        ChatChunk::Finished(r) => {
                            reason = r;
                            let _ = evt_tx.send(AppEvent::Finished {
                                generation_id: id,
                                reason: r,
                            });
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                reason = FinishReason::Error;
                let _ = evt_tx.send(AppEvent::Error(format!("Ошибка генерации: {err}")));
                let _ = evt_tx.send(AppEvent::Finished {
                    generation_id: id,
                    reason: FinishReason::Error,
                });
            }
        }
        let _ = done_tx.send(GenResult {
            id,
            chat_id,
            text,
            thoughts,
            reason,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::api::mock::MockBackend;
    use crate::shared::paths::Paths;

    /// Поднимает оркестратор на временном хранилище. Возвращает каналы и handle.
    fn spawn_orch(
        backend: Option<Arc<dyn EngineBackend>>,
    ) -> (
        tempfile::TempDir,
        UnboundedSender<AppCommand>,
        UnboundedReceiver<AppEvent>,
        tokio::task::JoinHandle<()>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, evt_rx) = unbounded_channel();
        let deps = OrchestratorDeps {
            cmd_rx,
            evt_tx,
            backend,
            storage,
            default_sampling: SamplingConfig::default(),
            status: ServerStatus::Ready,
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

    /// Собирает «голый» оркестратор для юнит-тестов чистых методов (без петли).
    fn bare_orch() -> (tempfile::TempDir, Orchestrator) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
        let (evt_tx, _evt_rx) = unbounded_channel();
        let (done_tx, _done_rx) = unbounded_channel();
        let orch = Orchestrator {
            evt_tx,
            backend: None,
            storage,
            default_sampling: SamplingConfig {
                temperature: Some(0.1),
                ..Default::default()
            },
            profiles: Vec::new(),
            chats: Vec::new(),
            active_id: None,
            state: State::Idle,
            done_tx,
            dirty: HashSet::new(),
            save_deadline: None,
        };
        (dir, orch)
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
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let storage = Storage::open(Paths::with_root(&root)).unwrap();
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, mut evt_rx) = unbounded_channel();
        let handle = tokio::spawn(run(OrchestratorDeps {
            cmd_rx,
            evt_tx,
            backend: Some(backend),
            storage,
            default_sampling: SamplingConfig::default(),
            status: ServerStatus::Ready,
        }));

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

    #[test]
    fn build_request_puts_system_aside_and_maps_roles() {
        let mut p = Profile::new("X", "Ты — X.");
        p.greeting = Some("Здравствуйте!".into());
        let mut chat = Chat::from_profile(&p, "c");
        chat.push_message(Message::assistant("Здравствуйте!"));
        chat.push_message(Message::user("привет"));

        let req = build_request(&chat, SamplingConfig::default());
        assert_eq!(req.system.as_deref(), Some("Ты — X."));
        assert_eq!(req.messages.len(), 2);
    }
}
