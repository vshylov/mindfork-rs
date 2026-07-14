//! Авто-рефлексия «модели себя» (Tier 3): каждые N ответов ассистента в чате
//! фоновая задача просит модель пересмотреть недавний разговор и **самой**
//! обновить свою «модель себя» (через инструменты SelfModel). В отличие от
//! авто-названия (одноходовый запрос без инструментов) это **мини agentic-loop**:
//! модель вызывает `update_self_model`/`update_user_model`/`add_insight`, петля их
//! исполняет (инструменты пишут напрямую в `Storage`). Чат не мутируется, в UI
//! ничего не стримится — рефлексия молчалива и опциональна (`config.self_model.
//! auto_reflect_every`, по умолчанию выкл). См. docs/history/self-model-mvp.md.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use chrono::{DateTime, Utc};

use crate::app::events::BackgroundKind;
use crate::entities::chat::{Chat, DeletedCause};
use crate::entities::message::{Message, MessageRole};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::features::tools::{ToolContext, ToolParams, TurnInfo, notes, self_model};
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

/// Потолок токенов ответа на раунд рефлексии (с запасом на «мысли» перед вызовом).
const REFLECT_MAX_TOKENS: usize = 2048;
/// Лимит раундов мини agentic-loop рефлексии (бэкстоп от зацикливания).
const REFLECT_MAX_ROUNDS: u32 = 6;
/// Лимит времени на всю рефлексию.
const REFLECT_TIMEOUT: Duration = Duration::from_secs(120);

/// Инструменты, доступные рефлексии (пересекается с набором профиля). `reflect`
/// (рубрика) не нужен — авто-режим уже «рефлексирует». Наблюдения переехали в
/// заметки (Ярус 1), поэтому вместо удалённого `consolidate_narrative` рефлексии даны
/// note-инструменты для консолидации наблюдений-заметок: переписать почти-дубль
/// (`note_revise`), заместить со «шрамом» (`note_supersede`) или слить (`note_merge`).
/// Граф над наблюдениями (Ярус 2): `note_link`/`note_neighbors` — связать
/// противоречащие/уточняющие наблюдения (id из `get_self_model`). **Кросс-органные
/// связи (Ярус 3):** дан `note_recall` — он отдаёт id пользовательских заметок «о
/// собеседнике» (self-заметки по-прежнему скрывает), чтобы рефлексия могла связать
/// наблюдение «о себе» с фактом «о собеседнике» (`note_link` self↔user).
/// См. docs/history/narrative-as-notes.md.
const REFLECT_TOOL_IDS: &[&str] = &[
    self_model::GET_SELF_MODEL_ID,
    self_model::UPDATE_SELF_MODEL_ID,
    self_model::UPDATE_USER_MODEL_ID,
    self_model::ADD_INSIGHT_ID,
    notes::NOTE_RECALL_ID,
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

/// Системное сообщение фоновой саморефлексии: обрамление + единый `POLICY_CORE`
/// (этап 6 — те же правила, что у протокола ведения). Строится в рантайме, поскольку
/// склеивает `const`-фрагмент с константой правил.
fn reflect_system_message(loc: &crate::shared::i18n::Locale) -> String {
    loc.tf(
        "prompt.reflect.system",
        &[("core", self_model::policy_core(loc))],
    )
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
pub(super) fn behavior_markers(
    chat: &Chat,
    since: Option<DateTime<Utc>>,
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
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
        about_user.push(loc.tf("reflect.behavior.regen", &[("n", &regen.to_string())]));
    }
    if del > 0 {
        about_user.push(loc.tf("reflect.behavior.deleted", &[("n", &del.to_string())]));
    }
    let mut out = String::new();
    if !about_user.is_empty() {
        out.push_str(loc.t("reflect.behavior.header"));
        out.push_str(&about_user.join("; "));
        out.push('.');
    }
    if rewrite > 0 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&loc.tf("reflect.behavior.rewrite", &[("n", &rewrite.to_string())]));
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
        let lang; // язык служебного каркаса профиля (ось A)
        let system_message;
        let last_user;
        let mut digest;
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
            lang = profile.language;
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
            if !tool_loop::due(count, every) {
                return;
            }
            allowed = REFLECT_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
            let loc = crate::shared::i18n::locale(lang);
            // Дайджест — только по окну (не по всей истории): иначе каждый цикл
            // перечитывал бы уже отрефлексированное и плодил дубли инсайтов.
            let Some(d) =
                crate::features::rename_chat::build_conversation_digest(&chat.messages[wm..], loc)
            else {
                return; // переписки в окне недостаточно — ватермарк не трогаем
            };
            // Поведенческие сигналы за окно (перегенерации/удаления с прошлой
            // рефлексии) — пища для модели собеседника. `since` = прежний `reflected_at`
            // (ещё не перезаписан спавном ниже).
            digest = match behavior_markers(chat, chat.reflected_at, loc) {
                Some(markers) => format!("{d}\n\n{markers}"),
                None => d,
            };
            watermark = chat.messages.len();
        }

        // Обзор наблюдений для консолидации (похожие пары / contradicts / без связей) —
        // конкретные данные к рефлексии над памятью «о себе» (обзор self-консолидации,
        // отложенный в Ярусе 2; включён после подтверждения пользы связывания в Ярусе 3).
        // Пусто, если наблюдений < 2.
        if let Some(overview) = notes::build_self_consolidation_overview(
            &self.storage,
            profile_id,
            crate::shared::i18n::locale(lang),
        ) {
            digest = format!("{digest}\n\n{overview}");
        }

        // Уже идёт рефлексия? Пропускаем без сдвига ватермарка (повторим на след. ходу).
        if self.bg_running(BackgroundKind::Reflection) {
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

        let ctx = ToolContext::new(
            self.tool_deps(backend.clone()),
            ToolParams::from_config(&self.config),
            TurnInfo {
                profile_id,
                chat_id,
                system_message,
                effective_sampling: SamplingConfig::default(),
                last_user_message_at: last_user,
                lang,
            },
        );
        let sampling = SamplingConfig {
            max_tokens: Some(REFLECT_MAX_TOKENS),
            temperature: Some(0.4),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(reflect_system_message(crate::shared::i18n::locale(lang))),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: self
                .registry
                .schemas_for(&allowed, crate::shared::i18n::locale(lang)),
        };

        // Спавним задачу и фиксируем слот (флаг «идёт» + тихий индикатор в статус-баре).
        let cancel = CancellationToken::new();
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel: cancel.clone(),
            max_rounds: REFLECT_MAX_ROUNDS,
            timeout: REFLECT_TIMEOUT,
            label: "авто-рефлексия",
            profile_id,
            kind: BackgroundKind::Reflection,
            done_tx: self.bg_done_tx.clone(),
        });
        self.begin_bg(BackgroundKind::Reflection, cancel);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Chat, DeletedCause, Message, behavior_markers, reflect_system_message, reflect_window,
    };

    /// Референсная локаль (ru) для ассертов на русские подстроки (пинят ru-бандл).
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn reflect_system_message_composes_from_policy_core() {
        let msg = reflect_system_message(ru());
        // Собрано из единого POLICY_CORE (те же правила, что у протокола ведения).
        assert!(msg.contains(crate::features::tools::self_model::policy_core(ru())));
        // Плюс рефлексия-специфичное обрамление.
        assert!(msg.contains("get_self_model"));
        assert!(msg.contains("Поведенческие сигналы"));
        assert!(msg.contains("только вызывай инструменты"));
        // Ярус 2: рефлексии предложено связывать наблюдения (граф).
        assert!(msg.contains("note_link"));
    }

    /// Per-language (§3.5 docs/history/i18n.md): системное сообщение рефлексии собирается на
    /// КАЖДОМ вшитом языке, встраивает `policy_core` того же языка, плейсхолдер
    /// `{core}` подставлен (без остатка), и несёт tool-имена (стабильны, не переводятся).
    #[test]
    fn reflect_system_message_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let msg = reflect_system_message(l);
            assert!(
                msg.contains(crate::features::tools::self_model::policy_core(l)),
                "{lang:?}: policy_core не встроен"
            );
            assert!(
                !msg.contains("{core}"),
                "{lang:?}: плейсхолдер не подставлен"
            );
            assert!(
                msg.contains("get_self_model") && msg.contains("note_link"),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn reflect_tools_include_graph() {
        // Ярус 2: авто-рефлексии даны note_link/note_neighbors (граф над наблюдениями).
        // Ярус 3: + note_recall (id пользовательских заметок для кросс-органных связей).
        use super::{REFLECT_TOOL_IDS, notes};
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_LINK_ID));
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_NEIGHBORS_ID));
        assert!(REFLECT_TOOL_IDS.contains(&notes::NOTE_RECALL_ID));
    }

    #[test]
    fn reflect_message_nudges_cross_organ_linking() {
        // Ярус 3: рефлексии предложено связывать наблюдение «о себе» с фактом «о
        // собеседнике» (кросс-органное ребро через note_recall + note_link).
        let msg = super::reflect_system_message(ru());
        assert!(msg.contains("note_recall"));
        assert!(msg.contains("о собеседнике"));
        // Обзор self-консолидации: рефлексии указано использовать его блок.
        assert!(msg.contains("Обзор наблюдений для консолидации"));
    }

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
        let out = behavior_markers(&chat, Some(base - Duration::hours(4)), ru()).unwrap();
        assert!(out.contains("перегенерировал твой ответ ×2"));
        assert!(out.contains("удалил обмен ×1"));
        assert!(!out.contains("переписывал"));
        // since=None → учитываем всё, собственное поведение (rewrite) — отдельной фразой.
        let all = behavior_markers(&chat, None, ru()).unwrap();
        assert!(all.contains("Ты сам переписывал свой ответ ×1"));
        // Нет сигналов → None.
        let empty = Chat::from_profile(&p, "t2");
        assert!(behavior_markers(&empty, None, ru()).is_none());
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
