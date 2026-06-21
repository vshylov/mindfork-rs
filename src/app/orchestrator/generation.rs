//! Генерация ответа ассистента: команды отправки/перегенерации/удаления обмена,
//! запуск хода и фоновая задача клиентского agentic-loop (spec §6.3).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::message::{Message, MessageMetadata, MessageRole, ToolCallRecord};
use crate::entities::profile::ToolId;
use crate::features::tools::{ChatEffect, ToolContext, ToolRegistry, effective_tool_ids};
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, FinishReason,
    ToolCallAccumulator,
};

use super::Orchestrator;
use super::request::{build_request, last_user_message_at};

/// Результат завершившейся задачи генерации (внутренний канал).
pub(super) struct GenResult {
    pub(super) id: Uuid,
    pub(super) chat_id: Uuid,
    /// Новые доменные сообщения (assistant с tool_calls, tool-результаты, финал) —
    /// в порядке появления; оркестратор дописывает их в `Chat`.
    pub(super) messages: Vec<Message>,
    /// Эффекты инструментов (применяются оркестратором — владельцем `Chat`).
    pub(super) effects: Vec<ChatEffect>,
}

impl Orchestrator {
    pub(super) fn handle_send(&mut self, text: String) {
        if !self.gen_state.is_idle() {
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

        // Добавляем сообщение пользователя в историю и эхо в ленту. Поле ввода UI
        // очистил при отправке — чистим и сохранённый черновик чата.
        {
            let Some(chat) = self.chat_mut(active_id) else {
                return;
            };
            chat.push_message(Message::user(&text));
            chat.draft.clear();
        }
        self.mark_dirty(active_id);
        let _ = self.evt_tx.send(AppEvent::UserMessage(text));

        self.start_generation(active_id, backend);
    }

    /// Перегенерирует последний ответ ассистента (spec §11.7): удаляет всё после
    /// последнего сообщения пользователя (старый ответ + tool-сообщения) и
    /// запускает генерацию заново из того же запроса. Лента перестраивается через
    /// переэмит `ChatActivated`. Во время генерации — игнорируется.
    pub(super) fn handle_regenerate(&mut self) {
        if !self.gen_state.is_idle() {
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
    pub(super) fn handle_delete_last(&mut self) {
        if !self.gen_state.is_idle() {
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

    /// Возвращает движок, если chat-сервер готов; иначе эмитит понятную ошибку в
    /// ленту чата (`AppEvent::Error`) и возвращает `None`. Гейтит и отправку, и
    /// перегенерацию — чтобы запрос не уходил на ещё загружающийся сервер (иначе
    /// 503 → «engine returned an error status»). Для операций списка чатов
    /// (авто-название) ошибка должна идти в оверлей — там используется
    /// [`EngineManager::backend_if_ready`](super::engines::EngineManager) напрямую.
    pub(super) fn ready_backend(&self) -> Option<Arc<dyn EngineBackend>> {
        match self.engines.backend_if_ready() {
            Ok(backend) => Some(backend),
            Err(msg) => {
                let _ = self.evt_tx.send(AppEvent::Error(msg));
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
                embedder: self.engines.embedder(),
            };
        }

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let _ = self
            .evt_tx
            .send(AppEvent::GenerationStarted { generation_id: id });
        self.gen_state.begin(id, cancel.clone());
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

    pub(super) fn handle_done(&mut self, res: GenResult) {
        // Применяем только результат текущей генерации (защита от устаревших):
        // finish() переходит в Idle лишь при совпадении id.
        if !self.gen_state.finish(res.id) {
            return;
        }

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
                    ChatEffect::SetSamplingOverride(s) => chat.sampling_override = Some(*s),
                }
            }
            self.mark_dirty(res.chat_id);
            self.emit_chat_list();
        }
    }
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
