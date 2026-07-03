//! Общий «тихий» agentic-loop фоновых задач (авто-рефлексия «модели себя» и
//! авто-консолидация заметок). Обе задачи — мини agentic-loop без стриминга в UI:
//! стрим → аккумулятор вызовов → исполнение разрешённых инструментов → следующий
//! раунд; толерантны к `Thoughts`/`ThoughtsSignature`/`Usage` (игнор). Раньше это
//! тело дублировалось в `reflection.rs` и `consolidation.rs` дословно (различались
//! лишь лимиты и метка лога) — здесь оно одно. Основную петлю генерации сознательно
//! **не** трогаем: у неё стриминг в UI, control-flow-инструменты, thinking-подписи
//! Anthropic, usage, эффекты — её сложность не окупает общий сток сейчас.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::entities::profile::ToolId;
use crate::features::tools::{ToolContext, ToolRegistry};
use crate::shared::api::{
    ApiMessage, ChatChunk, ChatRequest, EngineBackend, FinishReason, ToolCallAccumulator,
};

/// Пора ли запускать периодическую фоновую задачу: фича включена (`every > 0`) и
/// накоплено достаточно ответов. Чистая функция — тестируема. Общая для рефлексии и
/// консолидации.
pub(super) fn due(count: u32, every: usize) -> bool {
    every > 0 && (count as usize) >= every
}

/// Параметры запуска тихой фоновой задачи.
pub(super) struct SilentLoop {
    pub backend: Arc<dyn EngineBackend>,
    pub registry: Arc<ToolRegistry>,
    pub ctx: ToolContext,
    pub request: ChatRequest,
    /// Разрешённые инструменты (защита от вызова чего-то вне набора задачи).
    pub allowed: Vec<ToolId>,
    pub cancel: CancellationToken,
    /// Бэкстоп от зацикливания (число раундов).
    pub max_rounds: u32,
    /// Лимит времени на всю задачу.
    pub timeout: Duration,
    /// Метка для диагностических логов («авто-рефлексия»/«авто-консолидация»).
    pub label: &'static str,
    /// Профиль (для логов).
    pub profile_id: Uuid,
    /// Канал исхода: `Ok(())` при успехе, `Err(причина)` при ошибке/таймауте.
    pub done_tx: UnboundedSender<Result<(), String>>,
}

/// Запускает тихую фоновую задачу: мини agentic-loop под таймаутом. По завершении
/// шлёт исход в `done_tx` (снять флаг «идёт …» и вести наблюдаемость: серия неудач →
/// одна ошибка в UI). Инструменты пишут напрямую в `Storage`; чат/лента не трогаются.
pub(super) fn spawn_silent_loop(spawn: SilentLoop) {
    let SilentLoop {
        backend,
        registry,
        ctx,
        mut request,
        allowed,
        cancel,
        max_rounds,
        timeout,
        label,
        profile_id,
        done_tx,
    } = spawn;

    tokio::spawn(async move {
        let run = run_rounds(
            &backend,
            &registry,
            &ctx,
            &mut request,
            &allowed,
            &cancel,
            max_rounds,
        );
        let outcome: Result<(), String> = match tokio::time::timeout(timeout, run).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                tracing::warn!(%profile_id, "{label}: ошибка: {e}");
                Err(e.to_string())
            }
            Err(_) => {
                cancel.cancel();
                tracing::warn!(%profile_id, "{label}: превышен лимит времени");
                Err("превышен лимит времени".to_string())
            }
        };
        let _ = done_tx.send(outcome);
    });
}

/// Тело мини agentic-loop: раунды стрим→вызовы→исполнение до `max_rounds` или
/// первого раунда без вызовов. Разрешает только инструменты из `allowed`.
async fn run_rounds(
    backend: &Arc<dyn EngineBackend>,
    registry: &Arc<ToolRegistry>,
    ctx: &ToolContext,
    request: &mut ChatRequest,
    allowed: &[ToolId],
    cancel: &CancellationToken,
    max_rounds: u32,
) -> Result<(), anyhow::Error> {
    let allowed_has = |name: &str| allowed.iter().any(|t| t == name);
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
                ChatChunk::Thoughts(_) | ChatChunk::ThoughtsSignature(_) | ChatChunk::Usage(_) => {}
            }
        }
        let calls = acc.finish();
        // Раунд без вызовов или достигнут лимит — задача окончена.
        if reason != FinishReason::ToolCalls || calls.is_empty() || round >= max_rounds {
            break;
        }
        round += 1;
        request.messages.push(ApiMessage::assistant_tool_calls(
            text.clone(),
            calls.clone(),
        ));
        for call in &calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
            let result = if allowed_has(&call.name) {
                match registry.invoke(&call.name, ctx, args).await {
                    Ok(o) => o.result,
                    Err(e) => format!("Ошибка инструмента {}: {e}", call.name),
                }
            } else {
                format!("Инструмент {} недоступен.", call.name)
            };
            request.messages.push(ApiMessage::tool(&call.id, &result));
        }
    }
    Ok(())
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
