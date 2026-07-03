//! Генерация ответа ассистента: команды отправки/перегенерации/удаления обмена,
//! запуск хода и фоновая задача клиентского agentic-loop (spec §6.3).

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::DeletedCause;
use crate::entities::message::{Message, MessageMetadata, MessageRole, ToolCallRecord};
use crate::entities::profile::ToolId;
use crate::features::tools::{ChatEffect, ToolContext, ToolRegistry, control, effective_tool_ids};
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EngineBackend, FinishReason, ThinkingBlock,
    ToolCallAccumulator,
};
use crate::shared::tokens::estimate_prompt;

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
    /// Сообщения, отброшенные инструментом «переписать» (`rewrite_current_message`):
    /// прежняя (неверная) версия + её tool-сообщение. Сохраняются в `Chat.deleted`
    /// ради ручного восстановления; в инференсе/ленте не участвуют. См. spec §9.3.
    pub(super) deleted: Vec<Message>,
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
            // Сохраняем удалённое (ответ ассистента + tool-сообщения раунда) и
            // черновик ввода ради ручного восстановления (spec §11.7).
            let draft = chat.draft.clone();
            let removed = chat.messages.split_off(idx + 1);
            chat.record_deleted(removed, draft, DeletedCause::Regenerate);
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
            // Сохраняем удалённое (сообщение пользователя + ответ ассистента) и
            // черновик ввода ДО возврата текста пользователя в поле — ради ручного
            // восстановления (spec §11.7).
            let draft = chat.draft.clone();
            let removed = chat.messages.split_off(idx);
            chat.record_deleted(removed, draft, DeletedCause::DeleteExchange);
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
            self.config.tools.fs_enabled,
            self.config.engine.mode.cloud_provider(),
        );
        let schemas = self.registry.schemas_for(&allowed);

        // Снимок «модели себя» профиля на начало хода (SelfModel MVP). Инъекция в
        // системный промпт — только если профиль включил инструмент get_self_model
        // (opt-in). См. docs/self-model-mvp.md.
        let self_model = self.storage.db().self_model_get(profile_id).ok().flatten();
        let self_model_params =
            crate::entities::self_model::SelfModelParams::from_settings(&self.config.self_model);
        let inject_enabled = enabled
            .iter()
            .any(|t| t == crate::features::tools::self_model::GET_SELF_MODEL_ID);

        // Строим запрос/контекст инструмента из текущей истории чата.
        let mut request;
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
                chunk_params: crate::features::tools::rag::ChunkParams::from_settings(
                    &self.config.rag,
                ),
                self_model: self_model.clone(),
                self_model_params,
            };
        }

        // Подмешиваем рендер модели себя + протокол ведения в системный промпт.
        request.system = inject_self_model(
            request.system.take(),
            self_model.as_ref(),
            inject_enabled,
            self.config.self_model.maintenance_protocol,
            &self_model_params,
            chrono::Utc::now(),
        );

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let _ = self
            .evt_tx
            .send(AppEvent::GenerationStarted { generation_id: id });
        // Сразу показываем оценку токенов всей переписки (промпта) — точное число
        // придёт позже из `usage` сервера и заменит оценку. См. spec §11.1.
        let _ = self.evt_tx.send(AppEvent::TokenUsage {
            generation_id: id,
            completion: 0,
            context: Some(estimate_prompt_tokens(&request)),
            context_exact: false,
        });
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

        if res.messages.is_empty() && res.effects.is_empty() && res.deleted.is_empty() {
            return;
        }
        if let Some(chat) = self.chat_mut(res.chat_id) {
            // Отброшенное инструментом «переписать» — в архив удалённого (ручное
            // восстановление правкой JSON), как Ctrl+E/Ctrl+R. См. spec §9.3, §11.7.
            if !res.deleted.is_empty() {
                chat.record_deleted(res.deleted, String::new(), DeletedCause::Rewrite);
            }
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
        // После успешного ответа — возможно, пора фоновой авто-рефлексии (Tier 3)
        // и/или авто-консолидации заметок («сон», Ярус 3).
        self.maybe_auto_reflect(res.chat_id);
        self.maybe_auto_consolidate(res.chat_id);
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
    /// Подпись блока «мыслей» (Anthropic): нужна для переотправки thinking-блока в
    /// assistant-ходе с tool_use того же хода. `None` у бэкендов без extended thinking
    /// (llama.cpp/OpenAI) или когда «мыслей» не было.
    thoughts_signature: Option<String>,
    calls: Vec<ApiToolCall>,
    reason: FinishReason,
    /// Сгенерировано токенов за раунд: точное значение из `usage` сервера, иначе
    /// число потоковых дельт (приближение — у llama-server одна дельта ≈ один токен).
    tokens: u64,
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
        // Отброшенное инструментом «переписать» (для архива удалённого).
        let mut deleted: Vec<Message> = Vec::new();
        let mut round: u32 = 0;
        // Накопительный счётчик токенов ответа по всем раундам agentic-loop —
        // live-индикатор продолжает расти от раунда к раунду.
        let mut total_tokens: u64 = 0;
        // Следующее доменное сообщение ассистента начинает новый пузырь (после
        // `send_followup_message`). См. spec §9.3.
        let mut pending_new_bubble = false;
        let reason;

        let allowed_has = |name: &str| allowed.iter().any(|t| t == name);

        loop {
            let out = stream_round(
                &backend,
                request.clone(),
                &cancel,
                id,
                &evt_tx,
                total_tokens,
            )
            .await;
            total_tokens += out.tokens;

            // Раунд с вызовами инструментов — исполняем и продолжаем цикл.
            if out.reason == FinishReason::ToolCalls && !out.calls.is_empty() {
                if round >= max_rounds {
                    let _ = evt_tx.send(AppEvent::Error(format!(
                        "Достигнут лимит раундов инструментов ({max_rounds})."
                    )));
                    reason = FinishReason::Stop;
                    if let Some(mut m) = finalize_message(&out, &ctx) {
                        m.new_bubble = pending_new_bubble;
                        messages.push(m);
                    }
                    break;
                }
                round += 1;

                // Управляющие инструменты беседы (spec §9.3) распознаём только если
                // они реально включены в профиле — иначе обычный отказ ниже.
                // `rewrite` отбрасывает текущий раунд; `followup` начинает новый пузырь.
                let rewrite = out
                    .calls
                    .iter()
                    .any(|c| c.name == control::REWRITE_CURRENT_ID && allowed_has(&c.name));
                let followup = out
                    .calls
                    .iter()
                    .any(|c| c.name == control::SEND_FOLLOWUP_ID && allowed_has(&c.name));

                // assistant-ход с вызовами — в историю запроса (нужен и для инференса
                // следующего раунда продолжения/переписывания). При extended thinking
                // (Anthropic) прикрепляем thinking-блок с подписью: его обязан нести
                // assistant-ход с tool_use в этом же ходе, иначе следующий запрос → 400.
                // Подпись есть только если модель реально вернула «мысли»; прочие
                // бэкенды поле игнорируют.
                let thinking = out
                    .thoughts_signature
                    .clone()
                    .map(|signature| ThinkingBlock {
                        text: out.thoughts.clone(),
                        signature,
                    });
                request.messages.push(
                    ApiMessage::assistant_tool_calls(out.text.clone(), out.calls.clone())
                        .with_thinking(thinking),
                );
                let mut records: Vec<ToolCallRecord> = Vec::new();
                let mut tool_msgs: Vec<Message> = Vec::new();
                for call in &out.calls {
                    // Безаргументный вызов даёт пустую строку аргументов — храним как
                    // пустой ОБЪЕКТ, а не `Null`: иначе сериализация записи в историю
                    // даёт `"null"`, а строгие провайдеры (Anthropic) ждут объект в
                    // `input` (см. shared/api/anthropic/wire.rs). Объект и для invoke
                    // безопаснее (десериализация в struct из `null` падает).
                    let args: serde_json::Value = serde_json::from_str(&call.arguments)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    let is_control = control::is_control_tool(&call.name);
                    let result = if !allowed_has(&call.name) {
                        // Защита: инструмент выключен глобально/в профиле.
                        format!("Инструмент {} недоступен (выключен).", call.name)
                    } else if is_control {
                        // Управляющий инструмент: результат — «разрешение» (его
                        // увидит модель в следующем раунде). Исполняется петлёй, не
                        // через registry.
                        control::control_permission_text(&call.name).to_string()
                    } else if rewrite {
                        // Этот раунд отбрасывается — побочные инструменты не исполняем.
                        "(сообщение переписывается — вызов пропущен)".to_string()
                    } else {
                        match registry.invoke(&call.name, &ctx, args.clone()).await {
                            Ok(outcome) => {
                                effects.extend(outcome.effects);
                                outcome.result
                            }
                            Err(err) => format!("Ошибка инструмента {}: {err}", call.name),
                        }
                    };
                    // UI tool-блок — только для обычных исполненных вызовов
                    // (служебные followup/rewrite и пропущенные при переписывании — нет).
                    if !is_control && !rewrite {
                        let _ = evt_tx.send(AppEvent::ToolCall {
                            generation_id: id,
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                            result: result.clone(),
                        });
                    }
                    request.messages.push(ApiMessage::tool(&call.id, &result));
                    records.push(ToolCallRecord {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: args,
                        result: Some(result.clone()),
                    });
                    tool_msgs.push(tool_message(call, result));
                }

                // Доменное assistant-сообщение раунда (текст + мысли + tool-блоки).
                let mut am = Message::assistant(out.text.clone());
                if !out.thoughts.is_empty() {
                    am.thoughts = Some(out.thoughts.clone());
                }
                am.tool_calls = records;

                if rewrite {
                    // Отбрасываем раунд: assistant + tool-сообщения → архив удалённого.
                    // Live-лента очищает текущий пузырь под переписанный ответ.
                    // `pending_new_bubble` намеренно не трогаем (его поглотит финал).
                    deleted.push(am);
                    deleted.extend(tool_msgs);
                    let _ = evt_tx.send(AppEvent::AssistantRewrite { generation_id: id });
                } else {
                    // assistant ПЕРЕД tool-сообщениями этого раунда.
                    am.new_bubble = std::mem::take(&mut pending_new_bubble);
                    messages.push(am);
                    messages.extend(tool_msgs);
                    if followup {
                        // Следующее сообщение ассистента — отдельным пузырём.
                        pending_new_bubble = true;
                        let _ = evt_tx.send(AppEvent::AssistantContinue { generation_id: id });
                    }
                }
                continue;
            }

            // Финальный раунд (Stop/Length/Cancelled/Error или без вызовов).
            if let Some(mut m) = finalize_message(&out, &ctx) {
                m.new_bubble = pending_new_bubble;
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
            deleted,
        });
    });
}

/// Стримит один запрос, ретранслируя `Text`/`Thoughts` в UI, накапливая
/// tool-вызовы и счётчик токенов. `base_tokens` — токены, набранные предыдущими
/// раундами; счётчик в UI растёт накопительно. Возвращает накопленный раунд.
async fn stream_round(
    backend: &Arc<dyn EngineBackend>,
    request: ChatRequest,
    cancel: &CancellationToken,
    id: Uuid,
    evt_tx: &UnboundedSender<AppEvent>,
    base_tokens: u64,
) -> RoundOutput {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut thoughts_signature: Option<String> = None;
    let mut acc = ToolCallAccumulator::default();
    let mut reason = FinishReason::Stop;
    // Live-счёт: число дельт ответа (≈ токенов). Точное значение из `usage`
    // сервера, если придёт, заменяет приближение.
    let mut streamed: u64 = 0;
    let mut usage_tokens: Option<u64> = None;

    // Счётчик ответа: `context: None` оставляет прежнюю оценку переписки нетронутой
    // (её эмитит start_generation); точный `context` приходит лишь из usage сервера.
    let emit_completion = |completion: u64| {
        let _ = evt_tx.send(AppEvent::TokenUsage {
            generation_id: id,
            completion,
            context: None,
            context_exact: false,
        });
    };

    match backend.chat_stream(request, cancel.clone()).await {
        Ok(mut stream) => {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    ChatChunk::Text(t) => {
                        text.push_str(&t);
                        streamed += 1;
                        let _ = evt_tx.send(AppEvent::Chunk {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(base_tokens + streamed);
                    }
                    ChatChunk::Thoughts(t) => {
                        thoughts.push_str(&t);
                        streamed += 1;
                        let _ = evt_tx.send(AppEvent::Thoughts {
                            generation_id: id,
                            text: t,
                        });
                        emit_completion(base_tokens + streamed);
                    }
                    // Подпись «мыслей» (Anthropic) — не в UI, копим для переотправки.
                    ChatChunk::ThoughtsSignature(s) => {
                        thoughts_signature
                            .get_or_insert_with(String::new)
                            .push_str(&s);
                    }
                    ChatChunk::ToolCall(delta) => acc.push(delta),
                    ChatChunk::Usage(u) => {
                        // Точный счёт от сервера: и ответ, и переписку (prompt) —
                        // заменяет приближение по дельтам и оценку переписки.
                        usage_tokens = Some(u.completion_tokens as u64);
                        let _ = evt_tx.send(AppEvent::TokenUsage {
                            generation_id: id,
                            completion: base_tokens + u.completion_tokens as u64,
                            context: Some(u.prompt_tokens as u64),
                            context_exact: true,
                        });
                    }
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
        thoughts_signature,
        calls: acc.finish(),
        reason,
        tokens: usage_tokens.unwrap_or(streamed),
    }
}

/// Клиентская оценка числа токенов промпта (всей переписки) для live-индикатора
/// до прихода точного `usage.prompt_tokens` от сервера. Учитывает системное
/// сообщение, тексты реплик и аргументы вызовов инструментов в истории.
fn estimate_prompt_tokens(req: &ChatRequest) -> u64 {
    let mut parts: Vec<&str> = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        parts.push(m.content.as_str());
        for tc in &m.tool_calls {
            parts.push(tc.arguments.as_str());
        }
    }
    estimate_prompt(req.system.as_deref(), parts)
}

/// Нейтральный к персоне «протокол ведения модели»: короткая инструкция о том, когда
/// и чем фиксировать изменения. Едет в системном промпте поверх любой персоны профиля
/// → делает использование SelfModel-инструментов предсказуемым независимо от персоны
/// и прямо противодействует лести. Подмешивается по `config.self_model.
/// maintenance_protocol`. См. docs/self-model-mvp.md.
pub(super) const SELF_MODEL_MAINTENANCE_PROTOCOL: &str = "(Ты сам ведёшь эту «модель себя». Когда меняется что-то устойчивое — о тебе, о \
     собеседнике или о твоих целях — зафиксируй это инструментами: update_self_model \
     (описание себя; веди цели — закрывай выполненные и неактуальные по #id, а не только \
     ставь новые), update_user_model (черты/интересы собеседника — add_/remove_, не \
     перетирая прежнее), add_insight (наблюдение или противоречие прозой). Мимолётное \
     (настроение, разовая реакция) — в add_insight, не в модель собеседника. Точность \
     важнее угодливости: записывай то, что верно, а не то, что польстит.)";

/// Подмешивает «модель себя» в системный промпт хода (SelfModel MVP, см.
/// docs/self-model-mvp.md): компактный рендер текущей модели (если непуста) плюс,
/// при `maintenance_protocol`, нейтральный к персоне протокол ведения. Возвращает
/// прежний `system` без изменений, если инъекция выключена (профиль не включил
/// `get_self_model`) либо подмешивать нечего (пустая модель и протокол выключен).
/// Протокол подмешивается даже при пустой модели — чтобы модель начала её вести.
/// Чистая функция — тестируема без движка.
pub(super) fn inject_self_model(
    system: Option<String>,
    model: Option<&crate::entities::self_model::SelfModel>,
    enabled: bool,
    maintenance_protocol: bool,
    params: &crate::entities::self_model::SelfModelParams,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if !enabled {
        return system;
    }
    let block =
        model.and_then(|m| m.render_for_prompt(params.prompt_cap, params.narrative_in_prompt, now));
    // Собираем подмешиваемые части: рендер модели (если есть) + протокол (если включён).
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = block {
        parts.push(b);
    }
    if maintenance_protocol {
        parts.push(SELF_MODEL_MAINTENANCE_PROTOCOL.to_string());
        // data-aware приписка: если нарратив близок к потолку — подсказать
        // консолидацию прямо в системном промпте (протокол становится приборной
        // панелью, а не статичным плакатом).
        if let Some(h) = model.and_then(|m| m.narrative_fill_hint(params.max_narrative)) {
            parts.push(format!("({h})"));
        }
    }
    if parts.is_empty() {
        return system; // подмешивать нечего
    }
    let inject = parts.join("\n\n");
    Some(match system {
        Some(s) => format!("{s}\n\n{inject}"),
        None => inject,
    })
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
