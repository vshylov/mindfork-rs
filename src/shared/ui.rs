//! Слой `shared` (FSD): мелкие переиспользуемые помощники отрисовки TUI,
//! не зависящие от верхних слоёв.

use ratatui::Frame;
use ratatui::style::Modifier;

/// Притеняет весь экран (модификатор `DIM` на все ячейки буфера), чтобы попап
/// поверх не сливался с фоном. Вызывать **перед** `Clear`+рендером попапа:
/// `Clear` затем сбрасывает ячейки попапа к дефолтному (не приглушённому) стилю,
/// так что притеняется только фон, а сам попап остаётся ярким.
///
/// Эффект `DIM` терминало-зависим (Windows Terminal поддерживает).
pub fn dim_background(frame: &mut Frame) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].modifier |= Modifier::DIM;
        }
    }
}
