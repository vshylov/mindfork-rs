//! Авто-консолидация заметок («сон», Ярус 3): каждые N ответов ассистента в чате
//! фоновая задача просит модель пересмотреть базу знаний и **самой** её
//! консолидировать — слить дубли, переписать/заместить устаревшее, связать
//! родственное. Как и авто-рефлексия, это **мини agentic-loop**: модель вызывает
//! note-инструменты, петля их исполняет (пишут напрямую в `Storage`). Чат не
//! мутируется, в UI ничего не стримится — консолидация молчалива и опциональна
//! (`config.notes.auto_consolidate_every`, по умолчанию выкл).
//! См. docs/notes-connectivity.md (Ярус 3).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::app::events::{AppEvent, BackgroundKind};
use crate::entities::profile::ToolId;
use crate::entities::sampling::SamplingConfig;
use crate::entities::self_model::SelfModelParams;
use crate::features::tools::{ToolContext, notes};
use crate::shared::api::{ApiMessage, ChatRequest};

use super::Orchestrator;
use super::request::last_user_message_at;
use super::tool_loop;

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
            if !tool_loop::due(*count, every) {
                return;
            }
        }
        if self.consolidate_cancel.is_some() {
            return; // уже идёт — пропускаем без сброса (повторим на след. ходу)
        }
        // Нечего консолидировать, если пользовательских заметок меньше двух (self-заметки
        // не в счёт — консолидация над ними не работает). Счётчик не сброшен — повторим.
        let active_user = self
            .storage
            .db()
            .note_list(profile_id, None, &[], None)
            .unwrap_or_default()
            .iter()
            .filter(|n| !notes::is_self_note(n))
            .count();
        if active_user < 2 {
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
            recall_includes_self: self.config.notes.recall_includes_self,
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
        tool_loop::spawn_silent_loop(tool_loop::SilentLoop {
            backend,
            registry: self.registry.clone(),
            ctx,
            request,
            allowed,
            cancel,
            max_rounds: CONSOLIDATE_MAX_ROUNDS,
            timeout: CONSOLIDATE_TIMEOUT,
            label: "авто-консолидация",
            profile_id,
            done_tx: self.consolidate_done_tx.clone(),
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
