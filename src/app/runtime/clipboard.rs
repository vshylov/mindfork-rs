//! Runtime — reading/writing the system clipboard (the arboard slot). Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

/// Reads the system clipboard's text (lazily creating the client). `None` if the clipboard
/// is unavailable/empty/non-text. Used only on Windows to restore
/// emoji in a paste (see [`reconcile_paste`]).
#[cfg(windows)]
pub(super) fn read_clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(|c| c.get_text().ok())
}

/// Writes text into the system clipboard, creating the client lazily and reusing
/// it. Returns error text (instead of panicking) if the clipboard is unavailable — on
/// headless Linux with no X11/Wayland, the `arboard` constructor can fail.
pub(super) fn write_clipboard(
    slot: &mut Option<arboard::Clipboard>,
    text: &str,
) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    // `unwrap` is safe: we just guaranteed `Some`.
    slot.as_mut()
        .unwrap()
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}
