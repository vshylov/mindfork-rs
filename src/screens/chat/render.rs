//! The chat screen — rendering the chat screen. Part of the [`super`] module;
//! split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::popups::{render_confirm, render_suggest, render_tool_confirm};
use super::*;
use crate::shared::wrap;

impl ChatScreen {
    // ---------- rendering ----------

    /// The feed title's right-hand caption: the active engine mode's model. For
    /// managed — "name.gguf · Nk ctx" (context is meaningful); for
    /// external/cloud — the cloud/multi-model model's name with no ctx. Empty
    /// until the snapshot arrives or nothing can name a model.
    ///
    /// The configuration answers first; when it says nothing — `external` with a
    /// blank "Model (opt.)" — the name the **engine** reported is used instead
    /// (`AppEvent::EngineModel`, docs/research/external-model-name.md §4). Both
    /// empty still means an empty caption, byte-for-byte the header drawn before
    /// the engine was ever asked.
    pub(super) fn model_meta(&self) -> String {
        use crate::shared::config::ServerMode;
        let Some((cfg, _, _, _, _)) = &self.settings_snapshot else {
            return String::new();
        };
        let Some(name) = cfg
            .engine
            .active_model_name()
            .or_else(|| self.engine_model.clone())
        else {
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
    /// call site outlives the borrow) — and so are the two attachment chips, for
    /// the same reason.
    pub(super) fn status_model<'a>(
        &'a self,
        background: Option<&'a str>,
        attachments: Option<&'a str>,
        staged_images: Option<&'a str>,
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
            staged_images,
            esc_target: self.esc_target,
            read_only: self.read_only(),
            background_run: self.background_run,
        }
    }

    /// Whether a spinner on this screen — the indexing banner's or the
    /// impersonation preview's — would show another glyph now than in the last
    /// frame. The one reason the loop repaints an otherwise idle chat, and it
    /// comes once a [`SPINNER_STEP`](crate::shared::ui::SPINNER_STEP), never on every tick: Windows Terminal
    /// needs quiet output to re-find the links it underlines (see the constant).
    pub fn spinner_due(&self) -> bool {
        self.rag.as_ref().is_some_and(|b| b.spinner.due())
            || self.impersonation.as_ref().is_some_and(|i| i.spinner.due())
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let _ = self.maybe_recheck_spelling();

        // The input/preview height grows to fit content with wrapping (from 1
        // row up to `interface.input_max_rows`, within half the window — see
        // `input_height`). The impersonation preview wraps by "width minus
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
        let input_h = input_height(
            content_lines,
            self.input_rows_ceiling(),
            frame.area().height,
        );
        // The RAG-indexing banner takes a row only when active (otherwise 0 —
        // an empty rectangle, rendering into it is harmless).
        let banner_h: u16 = if self.rag.is_some() { 1 } else { 0 };
        // The status bar's height depends on width: hotkeys wrap when they
        // don't fit. The status snapshot is built as a temporary via
        // `status_model` on each call (a short-lived `&self` borrow — doesn't
        // overlap `&mut self.feed_view` below).
        let background = self.background_hint();
        let files = self.attachments_hint();
        let images = self.staged_images_hint();
        let status_h = status_bar::height(
            frame.area().width as usize,
            &self.status_model(background.as_deref(), files.as_deref(), images.as_deref()),
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
            crate::shared::credits::APP_NAME.to_string()
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

        if let Some(banner) = &mut self.rag {
            // Spinner frames — from the palette's glyph set (Braille;
            // compat — ASCII); which one shows is the spinner's clock's call.
            let spinner = banner.spinner.glyph(self.palette.glyphs().spinner);
            let prefix = format!("{spinner} RAG: ");
            // The row is one line high and the widget would clip a long text at
            // the edge — counters last, which are the point of the banner. Fit
            // the text into the columns left after the prefix instead.
            let avail = (banner_area.width as usize).saturating_sub(wrap::str_width(&prefix));
            let line = Line::from(vec![
                Span::styled(prefix, self.palette.accent_style()),
                Span::from(banner.fit(avail)),
            ]);
            frame.render_widget(line, banner_area);
        }

        status_bar::render(
            frame,
            status_area,
            &self.status_model(background.as_deref(), files.as_deref(), images.as_deref()),
            &self.palette,
            self.loc,
        );

        self.render_input_area(frame, input_area);
        self.render_overlays(frame);
    }

    /// Renders whatever stands in the input row: the impersonation preview,
    /// the in-feed search field, or the input box itself.
    fn render_input_area(&mut self, frame: &mut Frame, input_area: Rect) {
        // During impersonation the input box is hidden — a streaming reply
        // preview sits in its place. See spec §11.8.
        if let Some(imp) = &mut self.impersonation {
            let spinner = imp.spinner.glyph(self.palette.glyphs().spinner);
            impersonation_preview::render(
                frame,
                input_area,
                &imp.text,
                spinner,
                imp.done,
                &self.palette,
                self.loc,
            );
        } else if let Some(field) = &mut self.search {
            // In-feed search stands in for the input box rather than taking a row
            // of its own: a fifth layout constraint would shrink the feed, change
            // the wrap width and rewrap the whole chat on open *and* close
            // (docs/history/in-feed-search.md §3, fork F3).
            let title = match self.feed_view.match_position() {
                Some((n, total)) => self.loc.tf(
                    "ui.chat.search.counter",
                    &[("n", &n.to_string()), ("total", &total.to_string())],
                ),
                None => self.loc.t("ui.chat.search.none").to_string(),
            };
            field.render(
                frame,
                input_area,
                crate::widgets::input_box::RenderOpts {
                    title: &title,
                    focused: true,
                    command: false,
                    placeholder: self.loc.t("ui.chat.search.placeholder"),
                },
                &self.palette,
            );
        } else {
            // The idle footer names the chord that this terminal can actually
            // deliver (`shared::keys::newline_chord`, spec §11.5): on a
            // terminal without the kitty protocol `Shift+Enter` never arrives,
            // and advertising it there is a promise the app cannot keep.
            let input_title = if self.read_only() {
                self.loc.t("ui.chat.input.read_only").to_string()
            } else if self.generating {
                self.loc.t("ui.chat.input.generating").to_string()
            } else {
                self.loc.tf(
                    "ui.chat.input.idle",
                    &[("newline", crate::shared::keys::newline_chord())],
                )
            };
            let focused = self.profile_overlay.is_none()
                && self.suggest.is_none()
                && self.emoji.is_none()
                && self.chat_links.is_none()
                && self.confirm.is_none();
            let command = self.input_is_command();
            self.input.render(
                frame,
                input_area,
                crate::widgets::input_box::RenderOpts {
                    title: input_title.as_str(),
                    focused,
                    command,
                    placeholder: self.loc.t("ui.chat.input.placeholder"),
                },
                &self.palette,
            );
        }
    }

    /// Renders the popups drawn over the whole screen (the profile picker,
    /// spellcheck suggestions, the emoji picker, the tool confirmation, the
    /// `Ctrl+R`/`Ctrl+E` confirmation, help) — in stacking order.
    fn render_overlays(&mut self, frame: &mut Frame) {
        if let Some(overlay) = &mut self.profile_overlay {
            overlay.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(popup) = &mut self.suggest {
            dim_background(frame, &self.palette);
            render_suggest(frame, popup, &self.palette, self.loc);
        }
        if let Some(picker) = &self.emoji {
            dim_background(frame, &self.palette);
            picker.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(picker) = &mut self.chat_links {
            dim_background(frame, &self.palette);
            picker.render(frame, frame.area(), &self.palette, self.loc);
        }
        if let Some(pending) = &self.tool_confirm {
            dim_background(frame, &self.palette);
            render_tool_confirm(frame, pending, &self.palette, self.loc);
        }
        if let Some(action) = &self.confirm {
            dim_background(frame, &self.palette);
            render_confirm(frame, action, &self.palette, self.loc);
        }
    }
}

/// The input row's height — text rows plus the border — for `content_rows` of
/// wrapped text under the `ceiling` setting (`interface.input_max_rows`), in a
/// window `screen_h` rows tall. See spec §11.5.
///
/// The box also never takes more than **half the window**. The setting is a
/// wish, and the layout alone would honour it at the feed's expense: the feed's
/// `Constraint::Min(3)` outranks the `Length`s of the input and the status bar,
/// so the solver shrinks the feed to exactly that minimum and hands the input
/// the rest (measured: a 20-row window with a ceiling of 50 gave the input 15
/// rows and the chat one visible line), and which `Length` gives way once even
/// that is not enough is the solver's call. Capping here keeps the feed in
/// view; the text past the ceiling scrolls inside the box (`InputBox` keeps the
/// cursor in view and draws a scrollbar), as it always did past six rows.
pub(super) fn input_height(content_rows: usize, ceiling: u16, screen_h: u16) -> u16 {
    const BORDER: u16 = 2;
    let half_window = (screen_h / 2).saturating_sub(BORDER);
    let rows = ceiling.min(half_window).max(1);
    // `rows` bounds the result, so the narrowing cast cannot truncate.
    content_rows.clamp(1, usize::from(rows)) as u16 + BORDER
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
