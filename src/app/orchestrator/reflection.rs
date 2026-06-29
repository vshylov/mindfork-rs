//! Авто-рефлексия «модели себя» (Tier 3): каждые N ответов ассистента в чате
//! фоновая задача просит модель пересмотреть недавний разговор и **самой**
//! обновить свою «модель себя» (через инструменты SelfModel). В отличие от
//! авто-названия (одноходовый запрос без инструментов) это **мини agentic-loop**:
//! модель вызывает `update_self_model`/`update_user_model`/`add_insight`, петля их
//! исполняет (инструменты пишут напрямую в `Storage`). Чат не мутируется, в UI
//! ничего не стримится — рефлексия молчалива и опциональна (`config.self_model.
//! auto_reflect_every`, по умолчанию выкл). См. docs/self-model-mvp.md.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::self_model::SelfModelParams;
use crate::features::tools::{ToolContext, ToolRegistry, self_model};
use crate::shared::api::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason, ToolCallAccumulator,
};

use super::Orchestrator;
use super::request::last_user_message_at;

/// Потолок токенов ответа на раунд рефлексии (с запасом на «мысли» перед вызовом).
const REFLECT_MAX_TOKENS: usize = 2048;
/// Лимит раундов мини agentic-loop рефлексии (бэкстоп от зацикливания).
const REFLECT_MAX_ROUNDS: u32 = 6;
/// Лимит времени на всю рефлексию.
const REFLECT_TIMEOUT: Duration = Duration::from_secs(120);

/// Инструменты, доступные рефлексии (подмножество SelfModel; пересекается с
/// набором профиля). `reflect` (рубрика) не нужен — авто-режим уже «рефлексирует».
const REFLECT_TOOL_IDS: &[&str] = &[
    self_model::GET_SELF_MODEL_ID,
    self_model::UPDATE_SELF_MODEL_ID,
    self_model::UPDATE_USER_MODEL_ID,
    self_model::ADD_INSIGHT_ID,
];

/// Системное сообщение фоновой саморефлексии.
const REFLECT_SYSTEM_MESSAGE: &str = "Ты проводишь тихую фоновую саморефлексию. Ниже — фрагмент недавнего разговора. \
     При необходимости сначала вызови get_self_model, затем обнови свою «модель себя»: \
     уточни краткое описание себя (update_self_model.summary), цели (add_goals/\
     complete_goals/abandon_goals), представление о собеседнике (update_user_model) и, \
     если есть важное наблюдение или замеченное противоречие, запиши его (add_insight). \
     Меняй только то, что действительно изменилось; если менять нечего — не вызывай \
     ничего. Не пиши ответ пользователю — только вызывай инструменты.";

/// Пора ли запускать рефлексию: фича включена (`every > 0`) и накоплено достаточно
/// ответов. Чистая функция — тестируема.
pub(super) fn due(count: u32, every: usize) -> bool {
    every > 0 && (count as usize) >= every
}

impl Orchestrator {
    /// Вызывается после успешной генерации (`handle_done`): считает ответы ассистента
    /// в чате и при достижении порога запускает фоновую рефлексию. Тихо ничего не
    /// делает, если фича выключена, профиль не включил инструменты модели себя,
    /// рефлексия уже идёт, сервер не готов или переписки недостаточно.
    pub(super) fn maybe_auto_reflect(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.self_model.auto_reflect_every;
        if every == 0 {
            return;
        }

        // Снимок данных чата/профиля (борроу освобождается до правок полей self).
        let profile_id;
        let system_message;
        let last_user;
        let digest;
        let allowed: Vec<ToolId>;
        {
            let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
                return;
            };
            profile_id = chat.profile_id;
            let Some(profile) = self.profiles.iter().find(|p| p.id == profile_id) else {
                return;
            };
            // Гейт: профиль включает инструменты модели себя (как и инъекция в промпт).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == self_model::GET_SELF_MODEL_ID)
            {
                return;
            }
            allowed = REFLECT_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
            let Some(d) = crate::features::rename_chat::build_conversation_digest(&chat.messages)
            else {
                return; // переписки недостаточно
            };
            digest = d;
        }

        // Счётчик ответов с прошлой рефлексии (порог → сброс).
        let trigger = {
            let count = self.reflect_counts.entry(chat_id).or_insert(0);
            *count += 1;
            if due(*count, every) {
                *count = 0;
                true
            } else {
                false
            }
        };
        if !trigger || self.reflect_cancel.is_some() {
            return;
        }
        // Сервер готов? Иначе тихо пропускаем (счётчик уже сброшен — повторим позже).
        let Ok(backend) = self.engines.backend_if_ready() else {
            return;
        };

        let ctx = ToolContext {
            profile_id,
            chat_id,
            system_message,
            effective_sampling: SamplingConfig::default(),
            last_user_message_at: last_user,
            storage: self.storage.clone(),
            engine: backend.clone(),
            embedder: self.engines.embedder(),
            chunk_params: crate::features::tools::rag::ChunkParams::from_settings(&self.config.rag),
            self_model: None,
            self_model_params: SelfModelParams::from_settings(&self.config.self_model),
        };
        let sampling = SamplingConfig {
            max_tokens: Some(REFLECT_MAX_TOKENS),
            temperature: Some(0.4),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(REFLECT_SYSTEM_MESSAGE.to_string()),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: self.registry.schemas_for(&allowed),
        };

        let cancel = CancellationToken::new();
        self.reflect_cancel = Some(cancel.clone());
        spawn_reflection(ReflectSpawn {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel,
            done_tx: self.reflect_done_tx.clone(),
        });
    }

    /// Фоновая рефлексия завершилась — снимаем «идёт рефлексия». Инструменты уже
    /// записали изменения в `Storage`; ленту/чат это не трогает.
    pub(super) fn handle_reflect_done(&mut self) {
        self.reflect_cancel = None;
    }
}

/// Параметры запуска задачи рефлексии.
struct ReflectSpawn {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    ctx: ToolContext,
    request: ChatRequest,
    /// Разрешённые инструменты (защита от вызова чего-то вне набора рефлексии).
    allowed: Vec<ToolId>,
    cancel: CancellationToken,
    done_tx: UnboundedSender<()>,
}

/// Запускает фоновую задачу рефлексии: мини agentic-loop, исполняющий вызовы
/// SelfModel-инструментов (они пишут напрямую в `Storage`). Результат не нужен —
/// по завершении шлёт сигнал в `done_tx`, чтобы снять флаг «идёт рефлексия».
fn spawn_reflection(spawn: ReflectSpawn) {
    let ReflectSpawn {
        backend,
        registry,
        ctx,
        mut request,
        allowed,
        cancel,
        done_tx,
    } = spawn;

    tokio::spawn(async move {
        let allowed_has = |name: &str| allowed.iter().any(|t| t == name);
        let run = async {
            let mut round: u32 = 0;
            loop {
                let mut stream = backend.chat_stream(request.clone(), cancel.clone()).await?;
                let mut acc = ToolCallAccumulator::default();
                let mut text = String::new();
                let mut reason = FinishReason::Stop;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        ChatChunk::ToolCall(d) => acc.push(d),
                        ChatChunk::Text(t) => text.push_str(&t),
                        ChatChunk::Finished(r) => {
                            reason = r;
                            break;
                        }
                        ChatChunk::Thoughts(_)
                        | ChatChunk::ThoughtsSignature(_)
                        | ChatChunk::Usage(_) => {}
                    }
                }
                let calls = acc.finish();
                // Раунд без вызовов или достигнут лимит — рефлексия окончена.
                if reason != FinishReason::ToolCalls
                    || calls.is_empty()
                    || round >= REFLECT_MAX_ROUNDS
                {
                    break;
                }
                round += 1;
                request.messages.push(ApiMessage::assistant_tool_calls(
                    text.clone(),
                    calls.clone(),
                ));
                for call in &calls {
                    let args: serde_json::Value = serde_json::from_str(&call.arguments)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    let result = if allowed_has(&call.name) {
                        match registry.invoke(&call.name, &ctx, args).await {
                            Ok(o) => o.result,
                            Err(e) => format!("Ошибка инструмента {}: {e}", call.name),
                        }
                    } else {
                        format!("Инструмент {} недоступен.", call.name)
                    };
                    request.messages.push(ApiMessage::tool(&call.id, &result));
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        match tokio::time::timeout(REFLECT_TIMEOUT, run).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::debug!("авто-рефлексия: ошибка: {e}"),
            Err(_) => {
                cancel.cancel();
                tracing::debug!("авто-рефлексия: превышен лимит времени");
            }
        }
        let _ = done_tx.send(());
    });
}

#[cfg(test)]
mod tests {
    use super::due;

    #[test]
    fn due_respects_threshold_and_disabled() {
        assert!(!due(5, 0)); // выключено
        assert!(!due(1, 3));
        assert!(!due(2, 3));
        assert!(due(3, 3)); // достигли порога
        assert!(due(4, 3)); // и выше
    }
}
