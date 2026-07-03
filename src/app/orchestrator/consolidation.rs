//! Авто-консолидация заметок («сон», Ярус 3): каждые N ответов ассистента в чате
//! фоновая задача просит модель пересмотреть базу знаний и **самой** её
//! консолидировать — слить дубли, переписать/заместить устаревшее, связать
//! родственное. Как и авто-рефлексия, это **мини agentic-loop**: модель вызывает
//! note-инструменты, петля их исполняет (пишут напрямую в `Storage`). Чат не
//! мутируется, в UI ничего не стримится — консолидация молчалива и опциональна
//! (`config.notes.auto_consolidate_every`, по умолчанию выкл).
//! См. docs/notes-connectivity.md (Ярус 3).

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::events::{AppEvent, BackgroundKind};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::self_model::SelfModelParams;
use crate::features::tools::{ToolContext, ToolRegistry, notes};
use crate::shared::api::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason, ToolCallAccumulator,
};

use super::Orchestrator;
use super::request::last_user_message_at;

/// Потолок токенов ответа на раунд консолидации (с запасом на «мысли» перед вызовом).
const CONSOLIDATE_MAX_TOKENS: usize = 2048;
/// Лимит раундов мини agentic-loop консолидации (бэкстоп от зацикливания).
const CONSOLIDATE_MAX_ROUNDS: u32 = 8;
/// Лимит времени на всю консолидацию.
const CONSOLIDATE_TIMEOUT: Duration = Duration::from_secs(180);

/// Инструменты, доступные консолидации (пересекаются с набором профиля).
const CONSOLIDATE_TOOL_IDS: &[&str] = &[
    "note_recall",
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

/// Системное сообщение фоновой консолидации.
const CONSOLIDATE_SYSTEM_MESSAGE: &str = "Ты проводишь тихую фоновую консолидацию своей базы знаний (заметок). Ниже — обзор: \
     похожие пары (возможные дубли), связи contradicts, заметки без связей. Слей явные \
     дубли (note_merge), мелкое поправь (note_revise) или для смысловой переработки \
     замести (note_supersede, сохранит «шрам»), свяжи родственное (note_link). При \
     сомнении свериться через note_recall/note_neighbors. Действуй консервативно: \
     объединяй только действительно дублирующее, не теряй нюансы. Если всё в порядке — \
     не вызывай ничего. Не пиши ответ пользователю — только вызывай инструменты.";

/// Пора ли запускать консолидацию: фича включена (`every > 0`) и накоплено достаточно
/// ответов. Чистая функция — тестируема.
pub(super) fn due(count: u32, every: usize) -> bool {
    every > 0 && (count as usize) >= every
}

impl Orchestrator {
    /// Вызывается после успешной генерации (`handle_done`): считает ответы ассистента
    /// и при достижении порога запускает фоновую консолидацию. Тихо ничего не делает,
    /// если фича выключена, профиль не включил инструменты заметок, консолидация уже
    /// идёт, активных заметок меньше двух или сервер не готов.
    pub(super) fn maybe_auto_consolidate(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.notes.auto_consolidate_every;
        if every == 0 {
            return;
        }

        let profile_id;
        let system_message;
        let last_user;
        let allowed: Vec<ToolId>;
        {
            let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
                return;
            };
            profile_id = chat.profile_id;
            let Some(profile) = self.profiles.iter().find(|p| p.id == profile_id) else {
                return;
            };
            // Гейт: профиль включает консолидацию (note_merge — ядро операции).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == notes::NOTE_MERGE_ID)
            {
                return;
            }
            allowed = CONSOLIDATE_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
        }

        // Счётчик ответов с прошлой консолидации: инкремент; если порог не достигнут —
        // выходим (счётчик копится дальше). Сброс — только при фактическом спавне
        // (ниже), чтобы пропуск по гейту не терял накопленный цикл.
        {
            let count = self.consolidate_counts.entry(chat_id).or_insert(0);
            *count += 1;
            if !due(*count, every) {
                return;
            }
        }
        if self.consolidate_cancel.is_some() {
            return; // уже идёт — пропускаем без сброса (повторим на след. ходу)
        }
        // Нечего консолидировать, если активных заметок меньше двух (счётчик не сброшен
        // — повторим на следующем цикле).
        let active = self
            .storage
            .db()
            .note_list(profile_id, None, &[], None)
            .unwrap_or_default();
        if active.len() < 2 {
            return;
        }
        // Сервер готов? Иначе тихо пропускаем (счётчик не сброшен).
        let Ok(backend) = self.engines.backend_if_ready() else {
            return;
        };
        // Все гейты пройдены — сбрасываем счётчик и запускаем.
        self.consolidate_counts.insert(chat_id, 0);
        let overview = notes::build_consolidation_overview(&self.storage, profile_id);

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
            self_model_params: SelfModelParams::from_settings(&self.config.self_model),
        };
        let sampling = SamplingConfig {
            max_tokens: Some(CONSOLIDATE_MAX_TOKENS),
            temperature: Some(0.3),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(CONSOLIDATE_SYSTEM_MESSAGE.to_string()),
            messages: vec![ApiMessage::user(overview)],
            sampling,
            tools: self.registry.schemas_for(&allowed),
        };

        let cancel = CancellationToken::new();
        self.consolidate_cancel = Some(cancel.clone());
        spawn_consolidation(ConsolidateSpawn {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel,
            done_tx: self.consolidate_done_tx.clone(),
            profile_id,
        });
        // Тихий индикатор «идёт консолидация» в статус-баре.
        let _ = self.evt_tx.send(AppEvent::BackgroundTask {
            kind: BackgroundKind::Consolidation,
            active: true,
        });
    }

    /// Фоновая консолидация завершилась — снимаем «идёт консолидация», гасим индикатор
    /// и ведём серию неудач (как рефлексия). `SelfModelChanged` **не** шлём — меняются
    /// заметки, не «модель себя». Инструменты уже записали изменения в `Storage`.
    pub(super) fn handle_consolidate_done(&mut self, result: Result<(), String>) {
        self.consolidate_cancel = None;
        let _ = self.evt_tx.send(AppEvent::BackgroundTask {
            kind: BackgroundKind::Consolidation,
            active: false,
        });
        match result {
            Ok(()) => self.consolidate_failures = 0,
            Err(reason) => {
                self.consolidate_failures += 1;
                if self.consolidate_failures == super::BACKGROUND_FAILURE_ALERT {
                    let _ = self.evt_tx.send(AppEvent::Error(format!(
                        "Авто-консолидация трижды подряд завершилась ошибкой: {reason}"
                    )));
                }
            }
        }
    }
}

/// Параметры запуска задачи консолидации.
struct ConsolidateSpawn {
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    ctx: ToolContext,
    request: ChatRequest,
    /// Разрешённые инструменты (защита от вызова чего-то вне набора консолидации).
    allowed: Vec<ToolId>,
    cancel: CancellationToken,
    done_tx: UnboundedSender<Result<(), String>>,
    /// Профиль (для диагностических логов).
    profile_id: uuid::Uuid,
}

/// Запускает фоновую задачу консолидации: мини agentic-loop, исполняющий вызовы
/// note-инструментов (они пишут напрямую в `Storage`). По завершении шлёт исход в
/// `done_tx` — снять флаг «идёт консолидация» и вести наблюдаемость.
fn spawn_consolidation(spawn: ConsolidateSpawn) {
    let ConsolidateSpawn {
        backend,
        registry,
        ctx,
        mut request,
        allowed,
        cancel,
        done_tx,
        profile_id,
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
                // Раунд без вызовов или достигнут лимит — консолидация окончена.
                if reason != FinishReason::ToolCalls
                    || calls.is_empty()
                    || round >= CONSOLIDATE_MAX_ROUNDS
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

        let outcome: Result<(), String> = match tokio::time::timeout(CONSOLIDATE_TIMEOUT, run).await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                tracing::warn!(%profile_id, "авто-консолидация: ошибка: {e}");
                Err(e.to_string())
            }
            Err(_) => {
                cancel.cancel();
                tracing::warn!(%profile_id, "авто-консолидация: превышен лимит времени");
                Err("превышен лимит времени".to_string())
            }
        };
        let _ = done_tx.send(outcome);
    });
}

#[cfg(test)]
mod tests {
    use super::due;

    #[test]
    fn due_respects_threshold_and_disabled() {
        assert!(!due(5, 0)); // выключено
        assert!(!due(1, 3));
        assert!(due(3, 3)); // достигли порога
        assert!(due(4, 3)); // и выше
    }
}
