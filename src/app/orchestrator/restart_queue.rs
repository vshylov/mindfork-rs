//! [`RestartQueue`] — дебаунс-очередь отложенного (пере)запуска серверов
//! инференса/эмбеддингов при правках настроек движка (зеркало `SaveQueue`).
//! Экран настроек применяет правку при коммите каждого поля, поэтому серия
//! «бинарник → модель → -ngl» без дебаунса давала бы три тяжёлых рестарта
//! managed `llama-server` подряд. Конфиг при этом сохраняется и переэмитится
//! в UI сразу (см. `Orchestrator::handle_update_config`) — откладывается только
//! дорогой рестарт процесса; сам он остаётся у оркестратора
//! (`Orchestrator::flush_restarts`), очередь лишь учитывает, какие серверы и
//! когда перезапускать. Стартовый подъём серверов (до петли `run`) очередь не
//! проходит — он немедленный.

use std::time::Duration;

use tokio::time::Instant;

/// Пауза тишины после последней правки настроек движка до (пере)запуска.
const RESTART_DEBOUNCE: Duration = Duration::from_millis(1200);

#[derive(Default)]
pub(super) struct RestartQueue {
    /// Ожидает ли (пере)запуска chat-сервер (`config.engine`).
    chat: bool,
    /// Ожидает ли (пере)запуска embedding-сервер (`config.embed`).
    embed: bool,
    /// Ожидает ли (пере)запуска сервер имперсонации (`config.impersonation_engine`).
    impersonation: bool,
    /// Ожидают ли (пере)поднятия MCP-серверы (`config.mcp`).
    mcp: bool,
    /// Момент срабатывания (продлевается каждой пометкой — дебаунс от последней).
    deadline: Option<Instant>,
}

impl RestartQueue {
    /// Помечает chat-сервер для отложенного (пере)запуска и продлевает дедлайн.
    pub(super) fn mark_chat(&mut self) {
        self.chat = true;
        self.bump();
    }

    /// Помечает embedding-сервер для отложенного (пере)запуска и продлевает дедлайн.
    pub(super) fn mark_embed(&mut self) {
        self.embed = true;
        self.bump();
    }

    /// Помечает сервер имперсонации для отложенного (пере)запуска и продлевает дедлайн.
    pub(super) fn mark_impersonation(&mut self) {
        self.impersonation = true;
        self.bump();
    }

    /// Помечает MCP-серверы для отложенного (пере)поднятия и продлевает дедлайн.
    pub(super) fn mark_mcp(&mut self) {
        self.mcp = true;
        self.bump();
    }

    fn bump(&mut self) {
        self.deadline = Some(Instant::now() + RESTART_DEBOUNCE);
    }

    /// Текущий дедлайн (для `select!`-таймера петли). `None` — очередь пуста.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Забирает флаги `(chat, embed, impersonation, mcp)` и сбрасывает очередь.
    pub(super) fn take(&mut self) -> (bool, bool, bool, bool) {
        self.deadline = None;
        let out = (self.chat, self.embed, self.impersonation, self.mcp);
        self.chat = false;
        self.embed = false;
        self.impersonation = false;
        self.mcp = false;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Повторные пометки коалесируются: `take` отдаёт каждый сервер по одному
    /// разу, не помеченные не трогаются, дедлайн сбрасывается.
    #[tokio::test]
    async fn marks_coalesce_and_take_resets() {
        let mut q = RestartQueue::default();
        assert!(q.deadline().is_none(), "пустая очередь — без дедлайна");
        q.mark_chat();
        q.mark_chat();
        q.mark_impersonation();
        q.mark_mcp();
        assert!(q.deadline().is_some());
        assert_eq!(q.take(), (true, false, true, true));
        assert!(q.deadline().is_none(), "take сбрасывает дедлайн");
        assert_eq!(
            q.take(),
            (false, false, false, false),
            "повторный take пуст"
        );
    }

    /// Каждая пометка продлевает дедлайн — рестарт идёт от последней правки,
    /// а не от первой (иначе длинная серия правок словила бы рестарт посередине).
    #[tokio::test(start_paused = true)]
    async fn each_mark_extends_deadline() {
        let mut q = RestartQueue::default();
        q.mark_chat();
        let d1 = q.deadline().unwrap();
        tokio::time::advance(Duration::from_millis(500)).await;
        q.mark_embed();
        let d2 = q.deadline().unwrap();
        assert_eq!(d2 - d1, Duration::from_millis(500), "дедлайн продлён");
        assert_eq!(q.take(), (true, true, false, false));
    }
}
