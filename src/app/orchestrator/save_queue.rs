//! [`SaveQueue`] — дебаунс-очередь отложенного сохранения чатов на диск. Выделена
//! из оркестратора (Фаза 3): держит множество «грязных» чатов и дедлайн записи.
//! Сама запись остаётся у оркестратора (владельца `Chat` и `Storage`) — очередь
//! лишь учитывает, что и когда сбрасывать.

use std::collections::HashSet;
use std::time::Duration;

use tokio::time::Instant;
use uuid::Uuid;

/// Дебаунс сохранения изменённых чатов на диск.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Default)]
pub(super) struct SaveQueue {
    /// Чаты, ожидающие записи на диск.
    dirty: HashSet<Uuid>,
    /// Момент, когда нужно сбросить очередь (продлевается каждой пометкой).
    deadline: Option<Instant>,
}

impl SaveQueue {
    /// Помечает чат для отложенного сохранения и продлевает дедлайн (дебаунс).
    pub(super) fn mark(&mut self, id: Uuid) {
        self.dirty.insert(id);
        self.deadline = Some(Instant::now() + SAVE_DEBOUNCE);
    }

    /// Убирает чат из очереди (например, при его удалении — сохранять нечего).
    pub(super) fn forget(&mut self, id: Uuid) {
        self.dirty.remove(&id);
    }

    /// Текущий дедлайн сброса (для `select!`-таймера петли). `None` — очередь пуста.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Забирает все ожидающие id и сбрасывает дедлайн (для записи на диск).
    pub(super) fn take(&mut self) -> Vec<Uuid> {
        self.deadline = None;
        self.dirty.drain().collect()
    }

    /// Стоит ли чат в очереди (используется тестами).
    #[cfg(test)]
    pub(super) fn is_dirty(&self, id: Uuid) -> bool {
        self.dirty.contains(&id)
    }
}
