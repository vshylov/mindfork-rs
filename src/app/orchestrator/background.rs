//! Реестр слотов «тихих» фоновых задач (авто-рефлексия «модели себя» и
//! авто-консолидация заметок). Обе задачи — мини agentic-loop без UI (общий раннер
//! [`tool_loop::spawn_silent_loop`](super::tool_loop)); их жизненный цикл (флаг
//! «идёт», серия неудач, гашение индикатора, оповещение об ошибке) раньше
//! дублировался полями и обработчиками на каждую задачу. Здесь он один — ключ
//! реестра — существующий [`BackgroundKind`]. Добавление задачи №3 (авто-консолидация
//! «модели себя», roadmap — architecture.md §9.9) не трогает каркас `run()`/`Quit`.
//! См. docs/history/refactoring-solid.md §4.

use tokio_util::sync::CancellationToken;

use crate::app::events::{AppEvent, BackgroundKind};

use super::Orchestrator;

/// Слот тихой фоновой задачи: токен активного запуска + серия неудач. Серия живёт
/// дольше запуска (переживает завершения) — потому слот, а не отдельная задача.
#[derive(Default)]
pub(super) struct BgSlot {
    /// `Some` — задача идёт (одна за раз); токен отмены (для ветки `Quit`).
    cancel: Option<CancellationToken>,
    /// Число подряд идущих неудач; на пороге [`BACKGROUND_FAILURE_ALERT`](super::BACKGROUND_FAILURE_ALERT)
    /// один раз показываем ошибку в UI, дальше молчим до первого успеха.
    failures: u32,
}

impl Orchestrator {
    /// Идёт ли фоновая задача этого вида (гейт «одна за раз»).
    pub(super) fn bg_running(&self, kind: BackgroundKind) -> bool {
        self.bg.get(&kind).is_some_and(|s| s.cancel.is_some())
    }

    /// Фиксирует запуск: слот помечается активным (`cancel = Some`) и в статус-бар
    /// уходит тихий индикатор «идёт …». Зовётся спавн-хвостами
    /// `maybe_auto_reflect`/`maybe_auto_consolidate`.
    pub(super) fn begin_bg(&mut self, kind: BackgroundKind, cancel: CancellationToken) {
        self.bg.entry(kind).or_default().cancel = Some(cancel);
        let _ = self
            .evt_tx
            .send(AppEvent::BackgroundTask { kind, active: true });
    }

    /// Общий обработчик исхода фоновой задачи (бывшие `handle_reflect_done`/
    /// `handle_consolidate_done`): снимает флаг «идёт», гасит индикатор, ведёт серию
    /// неудач (на пороге — одна ошибка в UI, наблюдаемость без спама). При успехе
    /// **рефлексии** дополнительно шлёт `SelfModelChanged` (открытый экран `F3`
    /// перезапросит свежий снимок); консолидация — нет (меняет заметки, не «модель
    /// себя»). Инструменты задачи уже записали изменения в `Storage`; чат/ленту это
    /// не трогает.
    pub(super) fn handle_bg_done(&mut self, kind: BackgroundKind, result: Result<(), String>) {
        // Мутируем слот и вычисляем, нужна ли ошибка-оповещение, ДО отправки событий
        // (заём `self.bg` не пересекается с `self.evt_tx` при отправке ниже).
        let alert = {
            let slot = self.bg.entry(kind).or_default();
            slot.cancel = None;
            match &result {
                Ok(()) => {
                    slot.failures = 0;
                    None
                }
                Err(reason) => {
                    slot.failures += 1;
                    (slot.failures == super::BACKGROUND_FAILURE_ALERT).then(|| reason.clone())
                }
            }
        };
        let _ = self.evt_tx.send(AppEvent::BackgroundTask {
            kind,
            active: false,
        });
        if result.is_ok() && kind == BackgroundKind::Reflection {
            let _ = self.evt_tx.send(AppEvent::SelfModelChanged);
        }
        if let Some(reason) = alert {
            let _ = self.evt_tx.send(AppEvent::Error(format!(
                "{} трижды подряд завершилась ошибкой: {reason}",
                kind_label(kind)
            )));
        }
    }

    /// Отменяет все идущие фоновые задачи семейства (ветка `Quit`).
    pub(super) fn cancel_all_bg(&self) {
        for slot in self.bg.values() {
            if let Some(token) = &slot.cancel {
                token.cancel();
            }
        }
    }

    /// Число подряд идущих неудач задачи (для тестов оповещения об ошибке).
    #[cfg(test)]
    pub(super) fn bg_failures(&self, kind: BackgroundKind) -> u32 {
        self.bg.get(&kind).map_or(0, |s| s.failures)
    }
}

/// Человекочитаемая метка вида задачи — из неё собираются тексты ошибок (**байт-в-байт**
/// с прежними `handle_reflect_done`/`handle_consolidate_done`; на них смотрят тесты).
fn kind_label(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Reflection => "Авто-рефлексия",
        BackgroundKind::Consolidation => "Авто-консолидация",
    }
}
