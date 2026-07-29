//! The chat screen — rendering the chat screen. Part of the [`super`] module;
//! split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::popups::{render_confirm, render_help, render_suggest};
use super::*;

impl ChatScreen {
    // ---------- rendering ----------

    /// The feed title's right-hand caption: the active engine mode's model. For
    /// managed — "name.gguf · Nk ctx" (context is meaningful); for
    /// external/cloud — the cloud/multi-model model's name with no ctx. Empty
    /// until the snapshot arrives or no model is set.
    pub(super) fn model_meta(&self) -> String {
        use crate::shared::config::ServerMode;
        let Some((cfg, _, _, _, _)) = &self.settings_snapshot else {
            return String::new();
        };
        let Some(name) = cfg.engine.active_model_name() else {
            return String::new();
        };
        // For managed, context is meaningful — add "· Nk ctx".
        let ctx = cfg.engine.managed.context_size;
        if cfg.engine.mode == ServerMode::Managed && ctx > 0 {
            format!("{name} · {}k ctx", (ctx + 512) / 1024)
        } else {
            name
        }
    }

    /// Assembles the status-bar state snapshot in one place — a new indicator
    /// adds a field here rather than extending `status_bar::render`/`height`'s
    /// signatures. `background` is passed separately (the owning `String` at the
    /// call site outlives the borrow).
    fn status_model<'a>(
        &'a self,
        background: Option<&'a str>,
        attachments: Option<&'a str>,
    ) -> status_bar::StatusModel<'a> {
        status_bar::StatusModel {
            statuses: &self.statuses,
            generating: self.generating,
            tokens: self.gen_tokens,
            context: self.gen_context,
            context_exact: self.gen_context_exact,
            reasoning: self.gen_reasoning,
            mouse_scroll: self.mouse_scroll,
            background,
            speaking: self.speaking,
            attachments,
            esc_target: self.esc_target,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let _ = self.maybe_recheck_spelling();

        // Spinner animation for the indexing indicator (the loop draws every
        // tick while `is_rag_active()`); the divisor slows the frame change to
        // a pleasant pace.
        if let Some(banner) = &mut self.rag {
            banner.tick = banner.tick.wrapping_add(1);
        }
        // Spinner animation for the impersonation preview (while `is_impersonating()`).
        if let Some(imp) = &mut self.impersonation {
            imp.tick = imp.tick.wrapping_add(1);
        }

        // The input/preview height grows to fit content with wrapping (1–6
        // rows + border). The impersonation preview wraps by "width minus
        // border" (no prompt column), while the input box has a `❯` column —
        // so its height is computed via `content_rows`, which subtracts both
        // the border and `PROMPT_W` (the same text width as at render time,
        // otherwise the field grows with a lag).
        let area_w = frame.area().width;
        // `if let` (not `match &self.impersonation`): the `else` branch needs
        // `&mut self.input` (`content_rows` caches rows), and a borrow of
        // `&self.impersonation` wouldn't extend into it — the fields don't
        // overlap.
        let content_lines = if let Some(imp) = &self.impersonation {
            visual_line_count(&imp.text, area_w.saturating_sub(2).max(1) as usize)
        } else {
            self.input.content_rows(area_w)
        };
        let input_h = (content_lines.clamp(1, 6) + 2) as u16;
        // The RAG-indexing banner takes a row only when active (otherwise 0 —
        // an empty rectangle, rendering into it is harmless).
        let banner_h: u16 = if self.rag.is_some() { 1 } else { 0 };
        // The status bar's height depends on width: hotkeys wrap when they
        // don't fit. The status snapshot is built as a temporary via
        // `status_model` on each call (a short-lived `&self` borrow — doesn't
        // overlap `&mut self.feed_view` below).
        let background = self.background_hint();
        let files = self.attachments_hint();
        let status_h = status_bar::height(
            frame.area().width as usize,
            &self.status_model(background.as_deref(), files.as_deref()),
            &self.palette,
            self.loc,
        );
        let [feed_area, banner_area, input_area, status_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(banner_h),
            Constraint::Length(input_h),
            Constraint::Length(status_h),
        ])
        .areas(frame.area());

        let title = if self.title.is_empty() {
            "mindfork-rs".to_string()
        } else {
            self.title.clone()
        };
        let meta = self.model_meta();
        self.feed_view.render(
            frame,
            feed_area,
            &title,
            &meta,
            &self.feed,
            &self.palette,
            self.loc,
        );

        if let Some(banner) = &self.rag {
            // Spinner frames — from the palette's glyph set (Braille;
            // compat — ASCII).
            let frames = self.palette.glyphs().spinner;
            let spinner = frames[(banner.tick / 2) % frames.len()];
            let line = Line::from(vec![
                Span::styled(format!("{spinner} RAG: "), self.palette.accent_style()),
                Span::from(banner.text.clone()),
            ]);
            frame.render_widget(line, banner_area);
        }

        status_bar::render(
            frame,
            status_area,
            &self.status_model(background.as_deref(), files.as_deref()),
            &self.palette,
            self.loc,
        );

        // During impersonation the input box is hidden — a streaming reply
        // preview sits in its place. See spec §11.8.
        if let Some(imp) = &self.impersonation {
            impersonation_preview::render(
                frame,
                input_area,
                &imp.text,
                imp.tick,
                imp.done,
                &self.palette,
                self.loc,
            );
        } else {
            let input_title = if self.generating {
                self.loc.t("ui.chat.input.generating")
            } else {
                self.loc.t("ui.chat.input.idle")
            };
            let focused = self.profile_overlay.is_none()
                && self.suggest.is_none()
                && self.emoji.is_none()
                && self.confirm.is_none();
            let command = self.input_is_command();
            self.input.render(
                frame,
                input_area,
                crate::widgets::input_box::RenderOpts {
                    title: input_title,
                    focused,
                    command,
                    placeholder: self.loc.t("ui.chat.input.placeholder"),
                },
                &self.palette,
            );
        }

        if let Some(overlay) = &self.profile_overlay {
            overlay.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(popup) = &self.suggest {
            dim_background(frame, &self.palette);
            render_suggest(frame, popup, &self.palette, self.loc);
        }
        if let Some(picker) = &self.emoji {
            dim_background(frame, &self.palette);
            picker.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(action) = self.confirm {
            dim_background(frame, &self.palette);
            render_confirm(frame, action, &self.palette, self.loc);
        }
        if let Some(help) = &mut self.help {
            dim_background(frame, &self.palette);
            render_help(frame, help, &self.palette, self.loc);
        }
    }
}

/// The number of visual rows `text` will take when wrapped to `width` (for
/// computing the impersonation preview's height). Minimum 1.
pub(super) fn visual_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    text.split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            crate::shared::wrap::wrap_ranges(&chars, width).len().max(1)
        })
        .sum::<usize>()
        .max(1)
}

/// A fixed-width/height rectangle centered in `area` (clamped).
pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    v
}
