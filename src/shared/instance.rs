//! Запуск приложения в единственном экземпляре (single instance).
//! См. spec §1.4 (single instance) и plan M0.

use anyhow::{Result, anyhow, bail};
use single_instance::SingleInstance;

/// Уникальный идентификатор блокировки (именованный мьютекс на Windows,
/// abstract socket / lock-файл на Linux — деталь крейта `single-instance`).
const INSTANCE_ID: &str = "mindfork-rs-single-instance";

/// Держатель блокировки единственного экземпляра.
///
/// Должен жить весь срок работы процесса: при его `drop` блокировка
/// освобождается, и можно запустить новый экземпляр.
pub struct InstanceGuard {
    _inner: SingleInstance,
}

/// Пытается захватить блокировку единственного экземпляра.
///
/// Возвращает ошибку, если приложение уже запущено.
pub fn acquire() -> Result<InstanceGuard> {
    let inner = SingleInstance::new(INSTANCE_ID)
        .map_err(|e| anyhow!("single-instance initialization failed: {e}"))?;
    if !inner.is_single() {
        bail!("another instance of mindfork-rs is already running");
    }
    Ok(InstanceGuard { _inner: inner })
}
