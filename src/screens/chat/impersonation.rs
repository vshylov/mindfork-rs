//! The chat screen — impersonation (Ctrl+U): a preview of a reply on the user's
//! behalf. Part of the [`super`] module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::*;

impl ChatScreen {
    // ---------- impersonation (Ctrl+U, spec §11.8) ----------

    /// Starts impersonation: hides the input box and shows a streaming preview,
    /// seeded with the text already typed (the model continues it).
    pub fn begin_impersonation(&mut self, generation_id: Uuid) {
        self.impersonation = Some(ImpersonationState {
            generation_id,
            text: self.input.text(),
            spinner: Spinner::new(),
            done: false,
        });
    }

    /// Appends an impersonation text delta to the preview.
    pub fn push_impersonation_chunk(&mut self, generation_id: Uuid, text: &str) {
        if let Some(imp) = &mut self.impersonation
            && imp.generation_id == generation_id
        {
            imp.text.push_str(text);
        }
    }

    /// Finishes impersonation. The accumulated text is inserted into the input
    /// box on any completion **except an explicit user cancel** (`Cancelled` —
    /// `Esc`): then the field keeps the original seed text. In particular, a
    /// reply truncated by timeout (`Length`) or interrupted by a stream error
    /// (`Error`) **isn't lost** — partial text is more useful than an empty
    /// field (the user can finish it themselves). See spec §11.8.
    pub fn finish_impersonation(&mut self, generation_id: Uuid, reason: FinishReason) {
        match &self.impersonation {
            Some(imp) if imp.generation_id == generation_id => {}
            _ => return,
        }
        let imp = self.impersonation.take().unwrap();
        if reason != FinishReason::Cancelled {
            // set_text places the cursor at the end of the inserted text.
            self.input.set_text(&imp.text);
            self.mark_input_changed();
        }
        // A draft the provider's filter stopped lands like a length-cut one, but
        // a fragment in the box with nothing saying why reads as the whole draft
        // (docs/research/content-filter-finish.md).
        if reason == FinishReason::Filtered {
            self.push_note(self.loc.t("ui.err.impersonation_filtered"));
        }
    }

    /// Whether impersonation is in progress — the preview stands in for the
    /// input box. The loop asks [`Self::spinner_due`] instead: a running spinner
    /// needs a frame only when its glyph changes.
    #[cfg(test)]
    pub fn is_impersonating(&self) -> bool {
        self.impersonation.is_some()
    }
}
