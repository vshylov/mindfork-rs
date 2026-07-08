//! Runtime — чтение/запись системного буфера обмена (arboard-слот). Часть модуля [`super`]; разбито из монолита
//! runtime.rs (см. docs/history/refactoring-god-objects.md, этап 7).

/// Читает текст системного буфера обмена (лениво создавая клиент). `None`, если буфер
/// недоступен/пуст/не текстовый. Используется только на Windows для восстановления
/// эмодзи во вставке (см. [`reconcile_paste`]).
#[cfg(windows)]
pub(super) fn read_clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(|c| c.get_text().ok())
}

/// Пишет текст в системный буфер обмена, создавая клиент лениво и переиспользуя
/// его. Возвращает текст ошибки (вместо паники), если буфер недоступен — на
/// headless-Linux без X11/Wayland конструктор `arboard` может упасть.
pub(super) fn write_clipboard(
    slot: &mut Option<arboard::Clipboard>,
    text: &str,
) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    // `unwrap` безопасен: только что гарантировали `Some`.
    slot.as_mut()
        .unwrap()
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}
