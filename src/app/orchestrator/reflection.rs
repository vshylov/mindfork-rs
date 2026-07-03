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

use chrono::{DateTime, Utc};

use crate::entities::chat::{Chat, DeletedCause};
use crate::entities::message::{Message, MessageRole};
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
    self_model::CONSOLIDATE_NARRATIVE_ID,
];

/// Системное сообщение фоновой саморефлексии.
const REFLECT_SYSTEM_MESSAGE: &str = "Ты проводишь тихую фоновую саморефлексию. Ниже — фрагмент недавнего разговора. \
     Сначала вызови get_self_model (там цели с #id). Затем обнови свою «модель себя», \
     если что-то устойчивое изменилось: уточни описание себя (update_self_model.summary — \
     интегрируй прежнее с новым, не переписывай с нуля); веди цели по #id — закрывай \
     выполненные (complete_goals) и неактуальные (abandon_goals), добавляй новые \
     (add_goals); правь модель собеседника (update_user_model) частями — add_/remove_ \
     черт и интересов, не перетирая; важное наблюдение или замеченное противоречие \
     запиши прозой (add_insight). Мимолётное (настроение, разовая реакция) — только в \
     add_insight, не в модель собеседника. Если наблюдений накопилось много или есть \
     дубли/устаревшее — подними устойчивое в summary/черты и вычисти сырое через \
     consolidate_narrative. Точность важнее угодливости: фиксируй то, что верно, а не что \
     польстит. Если ниже есть блок «Поведенческие сигналы» — учти их как свидетельства о \
     собеседнике (update_user_model) или наблюдение (add_insight): это факты поведения, а \
     не осуждение. Меняй только действительно изменившееся; нечего — не вызывай ничего. Не \
     пиши ответ пользователю — только вызывай инструменты.";

/// Пора ли запускать рефлексию: фича включена (`every > 0`) и накоплено достаточно
/// ответов. Чистая функция — тестируема.
pub(super) fn due(count: u32, every: usize) -> bool {
    every > 0 && (count as usize) >= every
}

/// Начало окна рефлексии (кламп ватермарка к длине истории — устойчиво к усечению
/// `Ctrl+R`/`Ctrl+E`) и число ответов ассистента в этом окне. Каденция считается по
/// окну `messages[wm..]`, а не по всей истории — чтобы каждый цикл не перечитывал уже
/// отрефлексированный материал. Чистая функция — тестируема.
pub(super) fn reflect_window(messages: &[Message], reflected_upto: Option<usize>) -> (usize, u32) {
    let wm = reflected_upto.unwrap_or(0).min(messages.len());
    let count = messages[wm..]
        .iter()
        .filter(|m| m.role == MessageRole::Assistant && !m.text.trim().is_empty())
        .count() as u32;
    (wm, count)
}

/// Сводка поведенческих сигналов за окно рефлексии (удаления с `deleted_at > since`):
/// сколько раз собеседник перегенерировал/удалил ответ (косвенные свидетельства «ответ
/// не устроил») и сколько раз сам ассистент переписывал реплику. `None`, если сигналов
/// нет. Даёт рефлексии реальное поведение вместо одних самоописаний. Записи без причины
/// (старые) не считаются. Чистая функция — тестируема.
pub(super) fn behavior_markers(chat: &Chat, since: Option<DateTime<Utc>>) -> Option<String> {
    let (mut regen, mut del, mut rewrite) = (0u32, 0u32, 0u32);
    for d in &chat.deleted {
        if let Some(s) = since
            && d.deleted_at <= s
        {
            continue; // до прошлой рефлексии — уже учтено
        }
        match d.cause {
            Some(DeletedCause::Regenerate) => regen += 1,
            Some(DeletedCause::DeleteExchange) => del += 1,
            Some(DeletedCause::Rewrite) => rewrite += 1,
            None => {}
        }
    }
    if regen == 0 && del == 0 && rewrite == 0 {
        return None;
    }
    // Сигналы собеседника (перегенерация/удаление) отделяем от собственного поведения
    // (переписывание) — их нельзя приписывать собеседнику.
    let mut about_user: Vec<String> = Vec::new();
    if regen > 0 {
        about_user.push(format!(
            "перегенерировал твой ответ ×{regen} (вероятно, ответ не устроил)"
        ));
    }
    if del > 0 {
        about_user.push(format!("удалил обмен ×{del}"));
    }
    let mut out = String::new();
    if !about_user.is_empty() {
        out.push_str("Поведенческие сигналы собеседника за окно: ");
        out.push_str(&about_user.join("; "));
        out.push('.');
    }
    if rewrite > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("Ты сам переписывал свой ответ ×{rewrite}."));
    }
    Some(out)
}

impl Orchestrator {
    /// Вызывается после успешной генерации (`handle_done`): считает ответы ассистента
    /// **в окне с прошлой рефлексии** и при достижении порога запускает фоновую
    /// рефлексию. Тихо ничего не делает, если фича выключена, профиль не включил
    /// инструменты модели себя, рефлексия уже идёт, сервер не готов или переписки в
    /// окне недостаточно. Ватермарк (`Chat.reflected_upto`) сдвигается **только при
    /// фактическом спавне** — пропуск по гейту не теряет накопленный цикл.
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
        let watermark; // длина истории на момент охвата — фиксируем при спавне
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
            // Каденция по окну: ответы ассистента с прошлой рефлексии. Не накопилось —
            // выходим, ватермарк не трогаем.
            let (wm, count) = reflect_window(&chat.messages, chat.reflected_upto);
            if !due(count, every) {
                return;
            }
            allowed = REFLECT_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
            // Дайджест — только по окну (не по всей истории): иначе каждый цикл
            // перечитывал бы уже отрефлексированное и плодил дубли инсайтов.
            let Some(d) =
                crate::features::rename_chat::build_conversation_digest(&chat.messages[wm..])
            else {
                return; // переписки в окне недостаточно — ватермарк не трогаем
            };
            // Поведенческие сигналы за окно (перегенерации/удаления с прошлой
            // рефлексии) — пища для модели собеседника. `since` = прежний `reflected_at`
            // (ещё не перезаписан спавном ниже).
            digest = match behavior_markers(chat, chat.reflected_at) {
                Some(markers) => format!("{d}\n\n{markers}"),
                None => d,
            };
            watermark = chat.messages.len();
        }

        // Уже идёт рефлексия? Пропускаем без сдвига ватермарка (повторим на след. ходу).
        if self.reflect_cancel.is_some() {
            return;
        }
        // Сервер готов? Иначе пропускаем без сдвига ватермарка (повторим позже).
        let Ok(backend) = self.engines.backend_if_ready() else {
            return;
        };

        // Все гейты пройдены — фиксируем ватермарк (окно охвачено) и сохраняем чат.
        // `modified_at` не трогаем: рефлексия не должна поднимать чат в списке.
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.reflected_upto = Some(watermark);
            chat.reflected_at = Some(Utc::now());
        }
        self.mark_dirty(chat_id);

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
    use super::{Chat, DeletedCause, Message, behavior_markers, due, reflect_window};

    #[test]
    fn behavior_markers_counts_by_cause_and_filters_since() {
        use crate::entities::chat::DeletedExchange;
        use crate::entities::profile::Profile;
        use chrono::{Duration, Utc};

        let p = Profile::new("P", "s");
        let mut chat = Chat::from_profile(&p, "t");
        let base = Utc::now();
        let mk = |at, cause| DeletedExchange {
            deleted_at: at,
            messages: vec![Message::user("x")],
            draft: String::new(),
            cause: Some(cause),
        };
        chat.deleted = vec![
            mk(base - Duration::hours(1), DeletedCause::Regenerate),
            mk(base - Duration::hours(2), DeletedCause::Regenerate),
            mk(base - Duration::hours(3), DeletedCause::DeleteExchange),
            mk(base - Duration::days(5), DeletedCause::Rewrite), // до since
            DeletedExchange {
                deleted_at: base - Duration::hours(1),
                messages: vec![Message::user("x")],
                draft: String::new(),
                cause: None, // старая запись без причины — не считается
            },
        ];
        // since = 4 ч назад → последние 3 (2 regen + 1 delete); rewrite (5 дн.) отсечён.
        let out = behavior_markers(&chat, Some(base - Duration::hours(4))).unwrap();
        assert!(out.contains("перегенерировал твой ответ ×2"));
        assert!(out.contains("удалил обмен ×1"));
        assert!(!out.contains("переписывал"));
        // since=None → учитываем всё, собственное поведение (rewrite) — отдельной фразой.
        let all = behavior_markers(&chat, None).unwrap();
        assert!(all.contains("Ты сам переписывал свой ответ ×1"));
        // Нет сигналов → None.
        let empty = Chat::from_profile(&p, "t2");
        assert!(behavior_markers(&empty, None).is_none());
    }

    #[test]
    fn due_respects_threshold_and_disabled() {
        assert!(!due(5, 0)); // выключено
        assert!(!due(1, 3));
        assert!(!due(2, 3));
        assert!(due(3, 3)); // достигли порога
        assert!(due(4, 3)); // и выше
    }

    #[test]
    fn reflect_window_counts_assistant_from_watermark() {
        let msgs = vec![
            Message::user("u1"),
            Message::assistant("a1"),
            Message::user("u2"),
            Message::assistant("a2"),
            Message::assistant(""), // пустой ответ не считается
            Message::user("u3"),
            Message::assistant("a3"),
        ];
        // Без ватермарка — считаем все непустые ответы ассистента (a1,a2,a3).
        assert_eq!(reflect_window(&msgs, None), (0, 3));
        // Ватермарк после a2 (индекс 4): в окне только a3.
        assert_eq!(reflect_window(&msgs, Some(4)), (4, 1));
        // Ватермарк в конце — окно пусто.
        assert_eq!(reflect_window(&msgs, Some(msgs.len())), (7, 0));
    }

    #[test]
    fn reflect_window_clamps_past_watermark_after_truncation() {
        // История усечена (Ctrl+R/Ctrl+E) — ватермарк больше длины: кламп к len,
        // окно пусто, паники нет.
        let msgs = vec![Message::user("u1"), Message::assistant("a1")];
        assert_eq!(reflect_window(&msgs, Some(99)), (2, 0));
    }
}
