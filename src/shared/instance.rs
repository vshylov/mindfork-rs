//! Запуск приложения в единственном экземпляре (single instance).
//! См. spec §1.4 (single instance) и plan M0.
//!
//! Блокировка — OS-уровня: именованный мьютекс на Windows, abstract unix-socket
//! на Linux (деталь крейта `single-instance`). Имя без префикса попадает в
//! пространство имён текущего сеанса входа — этого достаточно, чтобы один
//! пользователь не запустил приложение дважды; путь к бинарнику на блокировку не
//! влияет (две копии в разных каталогах всё равно конфликтуют по имени).

use single_instance::SingleInstance;
use thiserror::Error;

/// Уникальный идентификатор блокировки.
const INSTANCE_ID: &str = "mindfork-rs-single-instance";

/// Держатель блокировки единственного экземпляра.
///
/// Должен жить весь срок работы процесса: при его `drop` блокировка
/// освобождается, и можно запустить новый экземпляр.
pub struct InstanceGuard {
    _inner: SingleInstance,
}

/// Ошибка захвата блокировки единственного экземпляра.
#[derive(Debug, Error)]
pub enum InstanceError {
    /// Приложение уже запущено в другом экземпляре — это не сбой, а отказ запуска.
    #[error("приложение уже запущено")]
    AlreadyRunning,
    /// Не удалось инициализировать блокировку (системная ошибка крейта).
    #[error("не удалось инициализировать блокировку единственного экземпляра: {0}")]
    Init(String),
}

/// Пытается захватить блокировку единственного экземпляра.
///
/// `Err(InstanceError::AlreadyRunning)` — приложение уже запущено (вызывающий
/// должен показать сообщение и завершиться); `Err(InstanceError::Init)` —
/// настоящий сбой инициализации.
pub fn acquire() -> Result<InstanceGuard, InstanceError> {
    acquire_named(INSTANCE_ID)
}

/// Реализация `acquire` с явным именем — для тестов (чтобы не конфликтовать с
/// реальной блокировкой запущенного приложения на той же машине).
fn acquire_named(name: &str) -> Result<InstanceGuard, InstanceError> {
    let inner = SingleInstance::new(name).map_err(|e| InstanceError::Init(e.to_string()))?;
    if !inner.is_single() {
        return Err(InstanceError::AlreadyRunning);
    }
    Ok(InstanceGuard { _inner: inner })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_reports_already_running() {
        // Уникальное имя на тест — не задевает реальную блокировку приложения.
        let name = "mindfork-rs-test-second-acquire-reports-already-running";

        let first = acquire_named(name).expect("первый захват должен удаться");
        match acquire_named(name) {
            Err(InstanceError::AlreadyRunning) => {}
            Err(other) => panic!("ожидался AlreadyRunning, получено: {other:?}"),
            Ok(_) => panic!("второй захват не должен был удаться, пока жив первый"),
        }

        // После освобождения первой блокировки захват снова возможен.
        drop(first);
        let _again = acquire_named(name).expect("после drop захват снова возможен");
    }
}
