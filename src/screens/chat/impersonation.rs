//! Экран чата — имперсонация (Ctrl+U): предпросмотр реплики за пользователя. Часть модуля [`super`]; разбито из
//! монолита chat.rs (см. docs/refactoring-god-objects.md, этап 2).

use super::*;

impl ChatScreen {
    // ---------- имперсонация (Ctrl+U, spec §11.8) ----------

    /// Начинает имперсонацию: прячет поле ввода и показывает потоковый предпросмотр,
    /// затравленный уже введённым текстом (модель продолжит его).
    pub fn begin_impersonation(&mut self, generation_id: Uuid) {
        self.impersonation = Some(ImpersonationState {
            generation_id,
            text: self.input.text(),
            tick: 0,
            done: false,
        });
    }

    /// Дописывает дельту текста имперсонации в предпросмотр.
    pub fn push_impersonation_chunk(&mut self, generation_id: Uuid, text: &str) {
        if let Some(imp) = &mut self.impersonation
            && imp.generation_id == generation_id
        {
            imp.text.push_str(text);
        }
    }

    /// Завершает имперсонацию. Накопленный текст вставляется в поле ввода при любом
    /// завершении, **кроме явной отмены пользователем** (`Cancelled` — `Esc`): тогда
    /// поле сохраняет исходный текст-затравку. В частности, обрезанная по таймауту
    /// (`Length`) или прерванная ошибкой потока (`Error`) реплика **не теряется** —
    /// неполный текст полезнее пустого поля (пользователь допишет сам). См. spec §11.8.
    pub fn finish_impersonation(&mut self, generation_id: Uuid, reason: FinishReason) {
        match &self.impersonation {
            Some(imp) if imp.generation_id == generation_id => {}
            _ => return,
        }
        let imp = self.impersonation.take().unwrap();
        if reason != FinishReason::Cancelled {
            // set_text ставит курсор в конец вставленного текста.
            self.input.set_text(&imp.text);
            self.mark_input_changed();
        }
    }

    /// Идёт ли имперсонация (петля перерисовывает кадры для анимации спиннера).
    pub fn is_impersonating(&self) -> bool {
        self.impersonation.is_some()
    }
}
