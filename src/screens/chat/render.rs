//! The chat screen — rendering the chat screen. Part of the [`super`] module;
//! split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::popups::{
    confirm_prompt, render_confirm, render_suggest, render_tool_confirm, suggest_min_size,
    tool_prompt,
};
use super::*;
use crate::shared::wrap;
use crate::widgets::impersonation_preview::Preview;
use crate::widgets::input_box::RenderOpts;
use crate::widgets::message_feed::{FeedCaption, FeedFrame};

/// The smallest window the chat is drawn in (spec §11.1.1): a row of the
/// conversation, a row of the draft and a row between them for the banner or
/// a second row of either. Twenty columns are where wrapped text stops being
/// a word a row. Below it the runtime draws the placeholder.
///
/// A constant, not the sum of what this frame happens to show: a minimum that
/// followed the banner or the status bar's second row would put the
/// placeholder up and take it down as they came and went
/// (docs/research/small-terminal.md §4.3).
pub(super) const CHAT_MIN_SIZE: MinSize = MinSize::new(20, 3);

/// What the chat draws around its rows in a window `rows` tall — the ladder
/// of docs/research/small-terminal.md §5 F1(b), spec §11.1.1. Nothing
/// changes from eleven rows up; below that the chrome goes one piece at a
/// time, the piece that says the least first, so that the feed keeps four
/// rows of the conversation for as long as there is chrome left to give:
///
/// | rows | status bar | input | feed |
/// |---|---|---|---|
/// | ≥ 11 | up to two rows | bordered | bordered |
/// | 10 | one row | bordered | bordered |
/// | 8–9 | one row | a bare row | bordered |
/// | 6–7 | one row | a bare row | bare |
/// | 3–5 | none | a bare row | bare |
///
/// A function of the window's rows and nothing else — not of the draft, the
/// pill or the banner — so the shape changes when the user drags the window's
/// edge and at no other time (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChatChrome {
    /// The most rows the status bar may take.
    pub status_rows: u16,
    /// The input box has its border and title.
    pub input_border: bool,
    /// The feed has its border and title.
    pub feed_border: bool,
}

impl ChatChrome {
    pub(super) const fn at(rows: u16) -> Self {
        Self {
            status_rows: match rows {
                0..=5 => 0,
                6..=10 => 1,
                _ => status_bar::HINT_ROWS_MAX,
            },
            input_border: rows >= 10,
            feed_border: rows >= 8,
        }
    }

    /// The rows the feed always keeps: its border, when it has one, and one
    /// row of the conversation.
    const fn feed_min_rows(self) -> u16 {
        if self.feed_border { 3 } else { 1 }
    }
}

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
    /// below it rather than a frame with parts missing (spec §11.1.1).
    pub fn min_size(&self, area: Rect) -> MinSize {
        self.popup_needs(area)
            .map_or(CHAT_MIN_SIZE, |popup| CHAT_MIN_SIZE.max(popup))
    }

    /// Whether a popup is open over the chat — one `Esc` closes, or whose
    /// question `Esc` declines. The placeholder lets that key through while
    /// one is ([`crate::shared::ui::WayOut`]).
    pub fn has_popup(&self) -> bool {
        self.popup_needs(Rect::default()).is_some()
    }

    /// The window the open popup needs; `None` — no popup is open. One list
    /// for [`Self::min_size`] and [`Self::has_popup`], so that a popup which
    /// raises the minimum is always one `Esc` can leave.
    fn popup_needs(&self, area: Rect) -> Option<MinSize> {
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
            self.local_servers
                .as_ref()
                .map(|_| LocalServerPickerState::min_size(loc)),
            self.tool_confirm
                .as_ref()
                .map(|pending| tool_prompt(pending, palette, loc).min_size(palette, area)),
            self.confirm
                .as_ref()
                .map(|action| confirm_prompt(action, palette, loc).min_size(palette, area)),
        ]
        .into_iter()
        .flatten()
        .reduce(MinSize::max)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let _ = self.maybe_recheck_spelling();

        let chrome = ChatChrome::at(frame.area().height);
        // The input/preview height grows to fit content with wrapping (from 1
        // row up to `interface.input_max_rows`, within a share of the window —
        // see `input_height`). The impersonation preview and the input box
        // each say how wide their text is at this width and this chrome
        // (`Preview::text_width`, `RenderOpts::text_width`): the height is
        // measured at the same text width the widget wraps at, otherwise the
        // field grows with a lag.
        let area_w = frame.area().width;
        let input_title = self.input_title(chrome);
        let input_opts = self.input_opts(&input_title, chrome);
        // `if let` (not `match &self.impersonation`): the `else` branch needs
        // `&mut self.input` (`content_rows` caches rows), and a borrow of
        // `&self.impersonation` wouldn't extend into it — the fields don't
        // overlap.
        let content_lines = if let Some(imp) = &self.impersonation {
            visual_line_count(&imp.text, Preview::text_width(area_w).max(1))
        } else {
            self.input.content_rows(area_w, &input_opts)
        };
        let input_h = input_height(
            content_lines,
            self.input_rows_ceiling(),
            frame.area().height,
            chrome.input_border,
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
            chrome.status_rows,
        );
        let [feed_area, banner_area, input_area, status_area] =
            chat_areas(frame.area(), chrome, banner_h, input_h, status_h);

        let title = if self.title.is_empty() {
            crate::shared::credits::APP_NAME.to_string()
        } else {
            self.title.clone()
        };
        let caption = self.model_meta();
        let header = if chrome.feed_border {
            FeedFrame::Bordered {
                title: &title,
                caption: &caption,
            }
        } else {
            FeedFrame::Bare
        };
        self.feed_view.render(
            frame,
            feed_area,
            header,
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

        self.render_input_area(frame, input_area, input_opts, chrome);
        self.render_overlays(frame);
    }

    /// The input box's title under `chrome`. On the border it is the key
    /// hint of the moment; a bare box has no border, and the hints are given
    /// up — `F1` lists the keys, and the status bar (while it has a row) says
    /// *generating* and *read-only* — except the one word colour cannot
    /// replace, which becomes the box's tag.
    ///
    /// The idle hint names the chord that this terminal can actually deliver
    /// (`shared::keys::newline_chord`, spec §11.5): on a terminal without the
    /// kitty protocol `Shift+Enter` never arrives, and advertising it there is
    /// a promise the app cannot keep. Whether `Enter` will run the text as a
    /// command is said by its colour; where there is none, by a word in the
    /// title — a `/…` that is not a command goes to the model as a message,
    /// so the slash alone does not answer it (spec §11.5, §11.6).
    fn input_title(&self, chrome: ChatChrome) -> String {
        let command_word = (self.input_is_command() && self.palette.mono)
            .then(|| self.loc.t("ui.chat.input.command").to_string());
        if !chrome.input_border {
            return command_word.unwrap_or_default();
        }
        let hint = if self.read_only() {
            self.loc.t("ui.chat.input.read_only").to_string()
        } else if self.generating {
            self.loc.t("ui.chat.input.generating").to_string()
        } else {
            self.loc.tf(
                "ui.chat.input.idle",
                &[("newline", crate::shared::keys::newline_chord())],
            )
        };
        match command_word {
            Some(word) => format!("{word} · {hint}"),
            None => hint,
        }
    }

    /// How the input box is drawn this frame: `title` on its border or as
    /// its tag, the focus, the command colouring, the frame `chrome` gives it.
    /// Built before the layout, because the rows the draft wraps into depend
    /// on it ([`RenderOpts::text_width`]).
    fn input_opts<'a>(&self, title: &'a str, chrome: ChatChrome) -> RenderOpts<'a> {
        // A popup takes the keys, so the box under it is not where the
        // cursor is — the tool confirmation included: the cursor used to
        // keep blinking through its border.
        let focused = self.profile_overlay.is_none()
            && self.suggest.is_none()
            && self.emoji.is_none()
            && self.chat_links.is_none()
            && self.local_servers.is_none()
            && self.confirm.is_none()
            && self.tool_confirm.is_none();
        RenderOpts {
            title,
            focused,
            command: self.input_is_command(),
            placeholder: self.loc.t("ui.chat.input.placeholder"),
            bare: !chrome.input_border,
        }
    }

    /// Renders whatever stands in the input row: the impersonation preview,
    /// the in-feed search field, or the input box itself.
    fn render_input_area(
        &mut self,
        frame: &mut Frame,
        input_area: Rect,
        input_opts: RenderOpts,
        chrome: ChatChrome,
    ) {
        // During impersonation the input box is hidden — a streaming reply
        // preview sits in its place. See spec §11.8.
        if let Some(imp) = &mut self.impersonation {
            // The glyph call records the spinner's step as drawn.
            let spinner = imp.spinner.glyph(self.palette.glyphs().spinner);
            let preview = Preview {
                text: &imp.text,
                spinner,
                done: imp.done,
                bare: !chrome.input_border,
            };
            impersonation_preview::render(frame, input_area, &preview, &self.palette, self.loc);
        } else if let Some(field) = &mut self.search {
            // In-feed search stands in for the input box rather than taking a row
            // of its own: a fifth layout constraint would shrink the feed, change
            // the wrap width and rewrap the whole chat on open *and* close
            // (docs/history/in-feed-search.md §3, fork F3). On the border the
            // title is the counter and the keys; a bare row has room for the
            // counter alone, as its tag.
            let position = self.feed_view.match_position();
            let title = match (chrome.input_border, position) {
                (true, Some((n, total))) => self.loc.tf(
                    "ui.chat.search.counter",
                    &[("n", &n.to_string()), ("total", &total.to_string())],
                ),
                (true, None) => self.loc.t("ui.chat.search.none").to_string(),
                (false, position) => {
                    let (n, total) = position.unwrap_or((0, 0));
                    self.loc.tf(
                        "ui.chat.search.tag",
                        &[("n", &n.to_string()), ("total", &total.to_string())],
                    )
                }
            };
            field.render(
                frame,
                input_area,
                RenderOpts {
                    title: &title,
                    focused: true,
                    command: false,
                    placeholder: self.loc.t("ui.chat.search.placeholder"),
                    bare: !chrome.input_border,
                },
                &self.palette,
            );
        } else {
            self.input
                .render(frame, input_area, input_opts, &self.palette);
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
        if let Some(list) = &mut self.local_servers {
            dim_background(frame, &self.palette);
            list.render(frame, frame.area(), &self.palette, self.loc);
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

/// The chat's four areas in the window `area`, top to bottom: the feed, the
/// indexing banner, the input box, the status bar — given the `chrome` the
/// window's rows allow and the rows the last three ask for.
///
/// Arithmetic, not the layout solver. The solver was asked for
/// `[Min(3), Length, Length, Length]`, and when those did not add up to the
/// window it shortened whichever `Length` it liked: the input box lost its
/// text row while the status bar kept two rows of key hints
/// (docs/research/small-terminal.md §1). Here what gives way is written down:
/// **the input box is cut back first**, to what the feed's minimum rows, the
/// banner and the status bar leave, and the feed takes every row that is
/// left. In a window of [`CHAT_MIN_SIZE`] the box is left its row of text
/// and the feed its row of the conversation, whatever the banner, the draft
/// and the status bar are doing.
///
/// Below that size the chat is not drawn at all ([`ChatScreen::min_size`]);
/// the function is total anyway — the parts are fitted from the bottom up,
/// each taking what is left — so no size makes two of them overlap.
pub(super) fn chat_areas(
    area: Rect,
    chrome: ChatChrome,
    banner_h: u16,
    input_h: u16,
    status_h: u16,
) -> [Rect; 4] {
    let input_h = input_h.min(
        area.height
            .saturating_sub(chrome.feed_min_rows() + banner_h + status_h),
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

/// The input row's height — text rows plus the border, when `bordered` — for
/// `content_rows` of wrapped text under the `ceiling` setting
/// (`interface.input_max_rows`), in a window `screen_h` rows tall. See spec
/// §11.5.
///
/// The box also never takes more than **half the window** — a **third** of a
/// window too short for its border (spec §11.1.1), where every row is one of
/// the conversation's. The setting is a wish, and the layout alone would
/// honour it at the feed's expense: the feed's `Constraint::Min(3)` outranked
/// the `Length`s of the input and the status bar, so the solver shrank the
/// feed to exactly that minimum and handed the input the rest (measured: a
/// 20-row window with a ceiling of 50 gave the input 15 rows and the chat one
/// visible line). Capping here keeps the feed in view; the text past the
/// ceiling scrolls inside the box (`InputBox` keeps the cursor in view and
/// draws a scrollbar), as it always did past six rows.
pub(super) fn input_height(
    content_rows: usize,
    ceiling: u16,
    screen_h: u16,
    bordered: bool,
) -> u16 {
    let (chrome, share) = if bordered {
        (2, screen_h / 2)
    } else {
        (0, screen_h / 3)
    };
    let rows = ceiling.min(share.saturating_sub(chrome)).max(1);
    // `rows` bounds the result, so the narrowing cast cannot truncate.
    content_rows.clamp(1, usize::from(rows)) as u16 + chrome
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
