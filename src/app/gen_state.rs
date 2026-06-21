//! Автомат жизненного цикла генерации ответа ассистента (`Idle/Generating/
//! Cancelling`) — выделен из оркестратора. Чистый тип без I/O: переходы валидны
//! по построению, side-effect'ы (spawn задачи, отмена токена, рассылка событий,
//! запись в `Chat`) остаются у оркестратора — единственного владельца состояния
//! (spec §4.4, §4.4.2). Это держит автомат юнит-тестируемым без tokio-рантайма.
//!
//! Имперсонация и RAG-индексация — отдельные конкурентные подсостояния
//! оркестратора (`imp_gen`, `rag_cancel`), они **сознательно** не входят в этот
//! автомат: он описывает только жизненный цикл ответа ассистента на активный чат.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Состояние генерации (автомат на активный чат). Данные носят сами варианты:
/// `id` генерации (для отбрасывания устаревших стрим-событий, spec §4.4) и токен
/// отмены HTTP-стрима.
pub enum GenState {
    /// Генерация не идёт; принимаются `SendMessage`/`RegenerateLast`/`Impersonate`.
    Idle,
    /// Идёт генерация с данным `id`; `cancel` прерывает HTTP-стрим.
    Generating { id: Uuid, cancel: CancellationToken },
    /// Отмена запрошена, но задача ещё «доезжает»; частичный ответ сохранится по
    /// приходу `GenResult` с тем же `id`.
    Cancelling { id: Uuid },
}

impl GenState {
    /// `id` текущей генерации (для гейта «применять только результат своего
    /// запроса»). `None` в `Idle`.
    pub fn current_id(&self) -> Option<Uuid> {
        match self {
            GenState::Idle => None,
            GenState::Generating { id, .. } | GenState::Cancelling { id, .. } => Some(*id),
        }
    }

    /// `true`, если генерация не идёт (гейт отправки/перегенерации/имперсонации).
    pub fn is_idle(&self) -> bool {
        matches!(self, GenState::Idle)
    }

    /// `Idle → Generating`. Возвращает `false` (без перехода), если автомат уже
    /// занят — защита от параллельного запуска. Вызывается после гейта `is_idle`.
    pub fn begin(&mut self, id: Uuid, cancel: CancellationToken) -> bool {
        if !self.is_idle() {
            return false;
        }
        *self = GenState::Generating { id, cancel };
        true
    }

    /// `Generating → Cancelling`. Возвращает токен отмены (его дёргает
    /// оркестратор — отмена это side-effect). `None`, если генерация не идёт или
    /// отмена уже запрошена (повторный `Cancel` — no-op).
    pub fn request_cancel(&mut self) -> Option<CancellationToken> {
        if let GenState::Generating { id, cancel } = self {
            let id = *id;
            let token = cancel.clone();
            *self = GenState::Cancelling { id };
            Some(token)
        } else {
            None
        }
    }

    /// Токен отмены текущей генерации, не меняя состояния (для `Quit` — глушим
    /// стрим на выходе, переход в `Cancelling` не нужен).
    pub fn active_cancel(&self) -> Option<&CancellationToken> {
        match self {
            GenState::Generating { cancel, .. } => Some(cancel),
            GenState::Idle | GenState::Cancelling { .. } => None,
        }
    }

    /// `Generating|Cancelling → Idle`, только если `id` совпал с текущей генерацией
    /// (анти-устаревание: хвост старого стрима после `Stop → Send` не сбросит
    /// новое состояние). Возвращает `true`, если переход случился.
    pub fn finish(&mut self, id: Uuid) -> bool {
        if self.current_id() == Some(id) {
            *self = GenState::Idle;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_only_from_idle() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        assert!(s.begin(id, CancellationToken::new()));
        assert_eq!(s.current_id(), Some(id));
        assert!(!s.is_idle());

        // Повторный begin не перетирает идущую генерацию.
        let other = Uuid::new_v4();
        assert!(!s.begin(other, CancellationToken::new()));
        assert_eq!(s.current_id(), Some(id));
    }

    #[test]
    fn request_cancel_transitions_and_returns_token() {
        let mut s = GenState::Idle;
        // В Idle отменять нечего.
        assert!(s.request_cancel().is_none());

        let id = Uuid::new_v4();
        let token = CancellationToken::new();
        s.begin(id, token.clone());

        let returned = s.request_cancel().expect("токен отмены из Generating");
        returned.cancel();
        assert!(
            token.is_cancelled(),
            "вернулся именно живой токен генерации"
        );
        assert!(matches!(s, GenState::Cancelling { .. }));
        assert_eq!(s.current_id(), Some(id));

        // Повторная отмена из Cancelling — no-op.
        assert!(s.request_cancel().is_none());
    }

    #[test]
    fn finish_matches_current_id() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        s.begin(id, CancellationToken::new());

        // Устаревший id не сбрасывает состояние.
        assert!(!s.finish(Uuid::new_v4()));
        assert!(!s.is_idle());

        // Свой id завершает генерацию.
        assert!(s.finish(id));
        assert!(s.is_idle());
    }

    #[test]
    fn finish_works_from_cancelling() {
        let mut s = GenState::Idle;
        let id = Uuid::new_v4();
        s.begin(id, CancellationToken::new());
        s.request_cancel();
        // Частичный результат отменённой генерации всё равно завершает автомат.
        assert!(s.finish(id));
        assert!(s.is_idle());
    }

    #[test]
    fn active_cancel_only_in_generating() {
        let mut s = GenState::Idle;
        assert!(s.active_cancel().is_none());

        let token = CancellationToken::new();
        s.begin(Uuid::new_v4(), token.clone());
        assert!(s.active_cancel().is_some());
        s.active_cancel().unwrap().cancel();
        assert!(token.is_cancelled());

        s.request_cancel();
        assert!(
            s.active_cancel().is_none(),
            "в Cancelling токен не отдаётся"
        );
    }
}
