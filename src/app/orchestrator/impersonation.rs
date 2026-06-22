//! Имперсонация (`Ctrl+U`, spec §11.8): модель пишет следующее сообщение «за
//! пользователя». Системное сообщение заменяется на имперсонационное, роли
//! user/assistant в истории меняются местами. Фоновая задача стримит реплику.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::Chat;
use crate::entities::message::{Message, MessageRole};
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::{ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason};

use super::Orchestrator;

/// Дефолтное системное сообщение режима имперсонации (когда у профиля поле пустое):
/// модель пишет короткую естественную реплику от лица пользователя.
const DEFAULT_IMPERSONATION_SYSTEM_MESSAGE: &str = "Ты — пользователь в этом диалоге. Напиши следующее сообщение от лица \
     пользователя: естественное, по теме разговора, без пояснений и кавычек. \
     Выведи только текст сообщения.";

/// Лимит времени на одну имперсонацию.
const IMPERSONATION_TIMEOUT: Duration = Duration::from_secs(120);

impl Orchestrator {
    /// Пишет сообщение от лица пользователя (имперсонация, `Ctrl+U`, spec §11.8):
    /// системное сообщение ассистента заменяется на имперсонационное из профиля, а
    /// роли user/assistant в истории меняются местами — модель продолжает диалог
    /// «за пользователя». Текст стримится в предпросмотр поля ввода. Игнорируется
    /// во время генерации/другой имперсонации.
    pub(super) fn handle_impersonate(&mut self, seed: String) {
        if !self.gen_state.is_idle() || self.imp_gen.is_some() {
            return;
        }
        let Some(active_id) = self.active_id else {
            let _ = self
                .evt_tx
                .send(AppEvent::Error("Нет активного чата".into()));
            return;
        };
        let backend = match self
            .engines
            .impersonation_backend_if_ready(self.config.impersonation_engine.mode)
        {
            Ok(backend) => backend,
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
                return;
            }
        };
        let Some(chat) = self.chats.iter().find(|c| c.id == active_id) else {
            return;
        };
        let imp_system = self
            .profiles
            .iter()
            .find(|p| p.id == chat.profile_id)
            .map(|p| p.impersonation_system_message.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_IMPERSONATION_SYSTEM_MESSAGE.to_string());
        let request = build_impersonation_request(
            chat,
            imp_system,
            &seed,
            self.config.impersonation_sampling.clone(),
        );

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        self.imp_gen = Some(id);
        self.imp_cancel = Some(cancel.clone());
        let _ = self
            .evt_tx
            .send(AppEvent::ImpersonationStarted { generation_id: id });
        spawn_impersonation(
            backend,
            request,
            id,
            cancel,
            self.evt_tx.clone(),
            self.imp_done_tx.clone(),
        );
    }

    /// Отменяет текущую имперсонацию (`Esc` в предпросмотре). Завершение придёт
    /// через `imp_done` и эмитит `ImpersonationFinished{Cancelled}`.
    pub(super) fn handle_cancel_impersonation(&mut self) {
        if let Some(token) = &self.imp_cancel {
            token.cancel();
        }
    }

    /// Завершение фоновой задачи имперсонации: чистит состояние и эмитит финал.
    pub(super) fn handle_imp_done(&mut self, id: Uuid, reason: FinishReason) {
        if self.imp_gen != Some(id) {
            return;
        }
        self.imp_gen = None;
        self.imp_cancel = None;
        let _ = self.evt_tx.send(AppEvent::ImpersonationFinished {
            generation_id: id,
            reason,
        });
    }
}

/// Строит запрос имперсонации (spec §11.8): системное сообщение — имперсонационное
/// (персона пользователя), роли user/assistant в истории меняются местами (модель
/// продолжает диалог «за пользователя»). Инструментов нет. Если `seed` не пуст,
/// модель просят продолжить уже начатый текст.
pub(super) fn build_impersonation_request(
    chat: &Chat,
    mut system: String,
    seed: &str,
    sampling: SamplingConfig,
) -> ChatRequest {
    let messages = chat.messages.iter().filter_map(swap_role_message).collect();
    let seed = seed.trim();
    if !seed.is_empty() {
        system.push_str(&format!(
            "\n\nПользователь уже начал писать своё сообщение: «{seed}». \
             Продолжи эту реплику естественно и выведи ТОЛЬКО продолжение, \
             без повтора уже написанного начала."
        ));
    }
    ChatRequest {
        system: Some(system),
        messages,
        sampling,
        tools: Vec::new(),
    }
}

/// Меняет роль сообщения местами для имперсонации (user↔assistant). System/Tool и
/// пустые сообщения отбрасываются (в режиме имперсонации инструментов нет).
pub(super) fn swap_role_message(message: &Message) -> Option<ApiMessage> {
    if message.text.trim().is_empty() {
        return None;
    }
    match message.role {
        MessageRole::User => Some(ApiMessage::assistant(&message.text)),
        MessageRole::Assistant => Some(ApiMessage::user(&message.text)),
        MessageRole::System | MessageRole::Tool => None,
    }
}

/// Запускает фоновую задачу имперсонации: стримит текст реплики в предпросмотр
/// (`ImpersonationChunk`), по завершении/таймауту/отмене шлёт `(id, reason)` в
/// `done_tx`. «Мысли» и tool-вызовы игнорируются (в поле ввода идёт только текст).
fn spawn_impersonation(
    backend: Arc<dyn EngineBackend>,
    request: ChatRequest,
    id: Uuid,
    cancel: CancellationToken,
    evt_tx: UnboundedSender<AppEvent>,
    done_tx: UnboundedSender<(Uuid, FinishReason)>,
) {
    tokio::spawn(async move {
        let run = async {
            let mut reason = FinishReason::Stop;
            let mut stream = backend.chat_stream(request, cancel.clone()).await?;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        let _ = evt_tx.send(AppEvent::ImpersonationChunk {
                            generation_id: id,
                            text: t,
                        });
                    }
                    ChatChunk::Thoughts(_) | ChatChunk::ToolCall(_) | ChatChunk::Usage(_) => {}
                    ChatChunk::Finished(r) => {
                        reason = r;
                        break;
                    }
                }
            }
            Ok::<FinishReason, anyhow::Error>(reason)
        };
        let reason = match tokio::time::timeout(IMPERSONATION_TIMEOUT, run).await {
            Ok(Ok(r)) => r,
            Ok(Err(err)) => {
                let _ = evt_tx.send(AppEvent::Error(format!("Ошибка имперсонации: {err}")));
                FinishReason::Error
            }
            Err(_) => {
                cancel.cancel();
                FinishReason::Cancelled
            }
        };
        // Отмена пользователем перекрывает причину завершения сервера.
        let reason = if cancel.is_cancelled() {
            FinishReason::Cancelled
        } else {
            reason
        };
        let _ = done_tx.send((id, reason));
    });
}
