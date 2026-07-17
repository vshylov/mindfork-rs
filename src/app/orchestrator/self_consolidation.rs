//! Авто-консолидация «модели себя» («сон» модели себя, этап A1): каждые N ответов
//! ассистента в чате фоновая задача просит модель пересмотреть свою «модель себя» и
//! **самой** её консолидировать — слить дубли наблюдений (`@self`-заметок), сжать
//! раздутое описание (`summary`), связать противоречия. Как авто-рефлексия и
//! авто-консолидация заметок, это **мини agentic-loop**: модель вызывает
//! self-model/note-инструменты, петля их исполняет (пишут напрямую в `Storage`). Чат не
//! мутируется, в UI ничего не стримится — «сон» молчалив и опционален
//! (`config.self_model.auto_consolidate_every`, по умолчанию выкл). Отдельный тумблер
//! от авто-рефлексии: гейты/данные модели себя и заметок уже разведены, свой счётчик
//! точнее. См. docs/self-model-consolidation.md (этап A1).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::events::BackgroundKind;
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::self_model::SelfModelParams;
use crate::features::tools::{ToolContext, ToolParams, TurnInfo, notes, self_model};
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

/// Потолок токенов ответа на раунд «сна» модели себя (с запасом на «мысли» перед вызовом).
const SELF_CONSOLIDATE_MAX_TOKENS: usize = 2048;
/// Лимит раундов мини agentic-loop «сна» модели себя (бэкстоп от зацикливания).
const SELF_CONSOLIDATE_MAX_ROUNDS: u32 = 8;
/// Лимит времени на всю консолидацию модели себя.
const SELF_CONSOLIDATE_TIMEOUT: Duration = Duration::from_secs(180);

/// Инструменты, доступные «сну» модели себя (пересекаются с набором профиля). Над
/// наблюдениями-заметками (`@self`): переписать почти-дубль (`note_revise`), заместить
/// со «шрамом» (`note_supersede`), слить (`note_merge`); граф — связать
/// противоречащие/уточняющие (`note_link`/`note_neighbors`). Плюс `update_self_model`
/// (сжать раздутый `summary`) и `update_user_model` (привести собеседника).
/// `get_self_model` даёт полные id наблюдений и текущий `summary`. `note_recall`
/// **не даём** — он скрывает `@self`; полные id модель берёт из `get_self_model`
/// (как рефлексия). См. docs/self-model-consolidation.md (этап A1).
const SELF_CONSOLIDATE_TOOL_IDS: &[&str] = &[
    self_model::GET_SELF_MODEL_ID,
    self_model::UPDATE_SELF_MODEL_ID,
    self_model::UPDATE_USER_MODEL_ID,
    notes::NOTE_REVISE_ID,
    notes::NOTE_SUPERSEDE_ID,
    notes::NOTE_MERGE_ID,
    notes::NOTE_LINK_ID,
    notes::NOTE_NEIGHBORS_ID,
];

/// Системное сообщение фоновой консолидации модели себя: обрамление + единый
/// `POLICY_CORE` (те же правила ведения, что у протокола/рефлексии). Строится в
/// рантайме, поскольку склеивает `const`-фрагмент с константой правил.
fn self_consolidate_system_message(loc: &crate::shared::i18n::Locale) -> String {
    loc.tf(
        "prompt.self_consolidate.system",
        &[("core", self_model::policy_core(loc))],
    )
}

impl Orchestrator {
    /// Вызывается после успешной генерации (`handle_done`): считает ответы ассистента
    /// и при достижении порога запускает фоновую консолидацию «модели себя». Тихо
    /// ничего не делает, если фича выключена, профиль не включил инструменты модели
    /// себя, «сон» уже идёт, нечего консолидировать (наблюдений < 2 и `summary` не
    /// раздут) или сервер не готов. Счётчик каденции сбрасывается **только при
    /// фактическом спавне** — пропуск по гейту не теряет накопленный цикл.
    pub(super) fn maybe_auto_self_consolidate(&mut self, chat_id: uuid::Uuid) {
        let every = self.config.self_model.auto_consolidate_every;
        if every == 0 {
            return;
        }

        let profile_id;
        let lang; // язык служебного каркаса профиля (ось A)
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
            lang = profile.language;
            // Гейт: профиль включает инструменты модели себя (как и инъекция/рефлексия).
            if !profile
                .enabled_tools
                .iter()
                .any(|t| t == self_model::GET_SELF_MODEL_ID)
            {
                return;
            }
            allowed = SELF_CONSOLIDATE_TOOL_IDS
                .iter()
                .filter(|id| profile.enabled_tools.iter().any(|t| t == **id))
                .map(|id| id.to_string())
                .collect();
            system_message = chat.system_message.clone();
            last_user = last_user_message_at(chat);
        }

        // Счётчик ответов с прошлого «сна»: инкремент; если порог не достигнут — выходим
        // (счётчик копится дальше). Сброс — только при фактическом спавне (ниже), чтобы
        // пропуск по гейту не терял накопленный цикл.
        {
            let count = self.self_consolidate_counts.entry(chat_id).or_insert(0);
            *count += 1;
            if !tool_loop::due(*count, every) {
                return;
            }
        }
        if self.bg_running(BackgroundKind::SelfConsolidation) {
            return; // уже идёт — пропускаем без сброса (повторим на след. ходу)
        }

        // Нечего консолидировать? Сигнал есть, если наблюдений (`@self`) ≥ 2 (обзор
        // self-консолидации непуст) ИЛИ описание себя раздуто сверх ориентира. Иначе —
        // выходим без сброса счётчика (повторим позже).
        let loc = crate::shared::i18n::locale(lang);
        let params = SelfModelParams::from_settings(&self.config.self_model);
        let obs_count = self
            .storage
            .db()
            .note_list(profile_id, None, &[notes::SELF_NOTE_TAG.to_string()], None)
            .unwrap_or_default()
            .len();
        let summary_hint = self
            .storage
            .db()
            .self_model_get(profile_id)
            .ok()
            .flatten()
            .and_then(|m| m.summary_fill_hint(params.summary_target_chars, loc));
        if obs_count < 2 && summary_hint.is_none() {
            return;
        }

        // Сервер готов? Иначе тихо пропускаем (счётчик не сброшен).
        let Ok(backend) = self.engines.backend_if_ready() else {
            return;
        };
        // Все гейты пройдены — сбрасываем счётчик и запускаем.
        self.self_consolidate_counts.insert(chat_id, 0);

        // Дайджест: обзор self-консолидации (похожие пары наблюдений / contradicts / без
        // связей; `None` при наблюдениях < 2) + подсказка о раздутом описании (если есть).
        let overview = notes::build_self_consolidation_overview(&self.storage, profile_id, loc);
        let digest = match (overview, summary_hint) {
            (Some(o), Some(h)) => format!("{o}\n\n{h}"),
            (Some(o), None) => o,
            (None, Some(h)) => h,
            // Гейт выше это исключает; защитно — нечего консолидировать.
            (None, None) => return,
        };

        // Токен отмены — до контекста: его клон едет в `ToolContext.cancel`.
        let cancel = CancellationToken::new();
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
                cancel: cancel.clone(),
            },
        );
        let sampling = SamplingConfig {
            max_tokens: Some(SELF_CONSOLIDATE_MAX_TOKENS),
            temperature: Some(0.3),
            ..Default::default()
        };
        let request = ChatRequest {
            system: Some(self_consolidate_system_message(loc)),
            messages: vec![ApiMessage::user(digest)],
            sampling,
            tools: self.registry.schemas_for(&allowed, loc),
        };

        // Спавним задачу и фиксируем слот (флаг «идёт» + тихий индикатор в статус-баре).
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel: cancel.clone(),
            max_rounds: SELF_CONSOLIDATE_MAX_ROUNDS,
            timeout: SELF_CONSOLIDATE_TIMEOUT,
            label: "авто-консолидация себя",
            profile_id,
            kind: BackgroundKind::SelfConsolidation,
            done_tx: self.bg_done_tx.clone(),
            // A2: семантика summary↔наблюдения (эмбеддинг абзацев summary на лету в
            // задаче). См. docs/self-model-consolidation.md §A2.
            summary_semantics: Some(tool_loop::SummarySemantics {
                embedder: self.engines.embedder(),
                storage: self.storage.clone(),
                profile_id,
                loc,
            }),
        });
        self.begin_bg(BackgroundKind::SelfConsolidation, cancel);
    }
}

#[cfg(test)]
mod tests {
    use super::self_consolidate_system_message;

    /// Референсная локаль (ru) для ассертов на русские подстроки (пинят ru-бандл).
    fn ru() -> &'static crate::shared::i18n::Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    #[test]
    fn self_consolidate_system_message_composes_from_policy_core() {
        let msg = self_consolidate_system_message(ru());
        // Собрано из единого POLICY_CORE (те же правила, что у протокола ведения).
        assert!(msg.contains(crate::features::tools::self_model::policy_core(ru())));
        // Плюс специфичное обрамление «сна» модели себя.
        assert!(msg.contains("get_self_model"));
        assert!(msg.contains("note_merge"));
        assert!(msg.contains("update_self_model"));
    }

    /// Per-language (§3.5 docs/history/i18n.md): системное сообщение собирается на КАЖДОМ
    /// вшитом языке, встраивает `policy_core` того же языка, плейсхолдер `{core}`
    /// подставлен (без остатка), несёт tool-имена (стабильны, не переводятся).
    #[test]
    fn self_consolidate_system_message_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let msg = self_consolidate_system_message(l);
            assert!(
                msg.contains(crate::features::tools::self_model::policy_core(l)),
                "{lang:?}: policy_core не встроен"
            );
            assert!(
                !msg.contains("{core}"),
                "{lang:?}: плейсхолдер не подставлен"
            );
            assert!(
                msg.contains("get_self_model") && msg.contains("note_merge"),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn self_consolidate_tools_cover_observations_and_summary() {
        use super::SELF_CONSOLIDATE_TOOL_IDS;
        use crate::features::tools::{notes, self_model};
        // Наблюдения (граф/слияние) + сжатие summary — есть; note_recall нет (скрывает @self).
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_MERGE_ID));
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_LINK_ID));
        assert!(SELF_CONSOLIDATE_TOOL_IDS.contains(&self_model::UPDATE_SELF_MODEL_ID));
        assert!(!SELF_CONSOLIDATE_TOOL_IDS.contains(&notes::NOTE_RECALL_ID));
    }
}
