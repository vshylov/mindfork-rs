//! The chat screen — rendering the chat screen. Part of the [`super`] module;
//! split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::popups::{
    confirm_prompt, render_confirm, render_suggest, render_tool_confirm, suggest_min_size,
    tool_prompt,
};
use super::*;
use crate::shared::wrap;
use crate::widgets::message_feed::FeedCaption;

/// The rows the feed always has: its border and one row of the conversation.
const FEED_MIN_ROWS: u16 = 3;

// The minimum is exactly the layout at its tallest — the feed's rows, the
// banner, the input box's and the status bar's two — so a window of that size
// never has to cut anything (`chat_areas`).
const _: () = assert!(CHAT_MIN_SIZE.height == FEED_MIN_ROWS + 1 + INPUT_MIN_ROWS + 2);

/// The smallest window the chat is drawn in (spec §11.1). Nine rows hold the
/// layout whole at its tallest: the feed's three, the indexing banner, the
/// input box with one row of text, and two rows of status bar. Twenty columns
/// are where wrapped text stops being a word a row.
///
/// A constant, not the sum of what this frame happens to show: a minimum that
/// followed the banner or the status bar's second row would put the
/// placeholder up and take it down as they came and went
/// (docs/research/small-terminal.md §4.3).
pub(super) const CHAT_MIN_SIZE: MinSize = MinSize::new(20, 9);

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
    pub(super) fn model_meta(&self) -> FeedCaption {
        use crate::shared::config::ServerMode;
        let Some((cfg, _, _, _, _)) = &self.settings_snapshot else {
            return FeedCaption::default();
        };
        let Some(name) = cfg
            .engine
            .active_model_name()
            .or_else(|| self.engine_model.clone())
        else {
            return FeedCaption::default();
        };
        // For managed, context is meaningful — "· Nk ctx" follows the name
        // where the header has room for it (`message_feed::fit_header`).
        let ctx = cfg.engine.managed.context_size;
        let detail = (cfg.engine.mode == ServerMode::Managed && ctx > 0)
            .then(|| format!("{}k ctx", (ctx + 512) / 1024));
        FeedCaption { name, detail }
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

    /// The smallest window this frame is drawn in: the chat's own layout,
    /// and the popup open over it — a list's key legend whole, a question
    /// whole at the width `area` gives it. The runtime draws the placeholder
    /// below it rather than a frame with parts missing (spec §11.1).
    pub fn min_size(&self, area: Rect) -> MinSize {
        let (palette, loc) = (&self.palette, self.loc);
        [
            self.profile_overlay
                .as_ref()
                .map(|_| ProfileListState::min_size(loc)),
            self.suggest.as_ref().map(|_| suggest_min_size(loc)),
            self.emoji.as_ref().map(|_| EmojiPickerState::min_size(loc)),
            self.chat_links
                .as_ref()
                .map(|_| ChatLinkPickerState::min_size(loc)),
            self.tool_confirm
                .as_ref()
                .map(|pending| tool_prompt(pending, palette, loc).min_size(palette, area)),
            self.confirm
                .as_ref()
                .map(|action| confirm_prompt(action, palette, loc).min_size(palette, area)),
        ]
        .into_iter()
        .flatten()
        .fold(CHAT_MIN_SIZE, MinSize::max)
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
        let [feed_area, banner_area, input_area, status_area] =
            chat_areas(frame.area(), banner_h, input_h, status_h);

        let title = if self.title.is_empty() {
            crate::shared::credits::APP_NAME.to_string()
        } else {
            self.title.clone()
        };
        let caption = self.model_meta();
        self.feed_view.render(
            frame,
            feed_area,
            &title,
            &caption,
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
            // A popup takes the keys, so the box under it is not where the
            // cursor is — the tool confirmation included: the cursor used to
            // keep blinking through its border.
            let focused = self.profile_overlay.is_none()
                && self.suggest.is_none()
                && self.emoji.is_none()
                && self.chat_links.is_none()
                && self.confirm.is_none()
                && self.tool_confirm.is_none();
            let command = self.input_is_command();
            // Whether `Enter` will run the text as a command is said by its
            // colour; where there is none, by a word in the title — a `/…`
            // that is not a command goes to the model as a message, so the
            // slash alone does not answer it (spec §11.5, §11.6).
            let input_title = if command && self.palette.mono {
                format!("{} · {input_title}", self.loc.t("ui.chat.input.command"))
            } else {
                input_title
            };
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

/// The input box's three rows: its border and one row of text.
const INPUT_MIN_ROWS: u16 = 3;

/// The chat's four areas in the window `area`, top to bottom: the feed, the
/// indexing banner, the input box, the status bar — given the rows the last
/// three ask for.
///
/// Arithmetic, not the layout solver. The solver was asked for
/// `[Min(3), Length, Length, Length]`, and when those did not add up to the
/// window it shortened whichever `Length` it liked: the input box lost its
/// text row while the status bar kept two rows of key hints
/// (docs/research/small-terminal.md §1). Here what gives way is written down:
/// **the input box is cut back first**, to what the feed's three rows, the
/// banner and the status bar leave, and the feed takes every row that is
/// left. In a window of [`CHAT_MIN_SIZE`] the box is left its own three and
/// the feed its three, whatever the banner, the draft and the status bar are
/// doing.
///
/// Below that size the chat is not drawn at all ([`ChatScreen::min_size`]);
/// the function is total anyway — the parts are fitted from the bottom up,
/// each taking what is left — so no size makes two of them overlap.
pub(super) fn chat_areas(area: Rect, banner_h: u16, input_h: u16, status_h: u16) -> [Rect; 4] {
    let input_h = input_h.min(
        area.height
            .saturating_sub(FEED_MIN_ROWS + banner_h + status_h),
    );
    let mut bottom = area.bottom();
    let mut above = |rows: u16| {
        let rows = rows.min(bottom - area.y);
        bottom -= rows;
        Rect {
            y: bottom,
            height: rows,
            ..area
        }
    };
    let status = above(status_h);
    let input = above(input_h);
    let banner = above(banner_h);
    let feed = above(u16::MAX);
    [feed, banner, input, status]
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
