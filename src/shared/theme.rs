//! TUI theme palette (spec §11.6). Semantic color roles resolved from
//! [`Theme`]. Widgets take colors from the palette instead of hardcoding
//! `.cyan()`/`.green()`/… — this gives readability on a light/dark background.
//!
//! The palette and helpers implement the TUI redesign (`docs/redesign/`): calm
//! rounded borders, colored message-role rails, status "pills", and a quiet
//! hotkey line with "keycap" labels. Exact dark-theme shades come from the
//! design mockup (oklch → sRGB); `Auto` uses named ANSI colors that adapt to
//! the terminal's theme, `Light` uses darkened variants.
//!
//! Modifier attributes (`dim`/`bold`/`reversed`/`underlined`) are
//! theme-independent (the terminal adapts them) and are left in widgets as-is —
//! the palette sets only the colors themselves.
//!
//! Besides colors, the palette carries a **glyph set** ([`GlyphSet`], method
//! [`Palette::glyphs`]): in compatibility mode for old terminals
//! (`config.interface.terminal_compat`, spec §11.6) decorative emoji and rare
//! Unicode characters are replaced with safe ones, and rounded borders with
//! straight ones.
//!
//! **Colour modes** (spec §11.6, docs/history/theme-modes.md). In the *system* mode
//! the palette is foregrounds only and the terminal supplies the background.
//! In the *full* mode the palette also names a **canvas**
//! ([`Palette::canvas`]), which `shared/ui.rs::paint_canvas` puts under every
//! cell of the finished frame. In the *monochrome* mode
//! ([`Palette::mono`]) nothing of the palette's colours reaches the terminal —
//! `shared/ui.rs::strip_styles` drops every colour and attribute from the
//! finished frame — and the palette's job is the other half: the **glyphs**
//! that say what styling said ([`Palette::selected_mark`],
//! [`Palette::tab_label`], the bracketed [`Palette::keycap`]) and the one mark
//! that stays reverse video ([`MONO_MARK`]). [`Palette::for_interface`] is
//! the one place that turns the settings into a palette.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, OnceLock};

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::shared::config::{InterfaceSettings, Theme, ThemeMode};
use crate::shared::osc11::Background;
#[cfg(test)]
use crate::shared::wrap;

/// What the terminal answered when asked for its background at startup
/// (`shared/osc11`), or `None` when it did not answer or was never asked.
///
/// A process-wide `OnceLock` rather than a parameter threaded through
/// [`Palette::for_theme`]: the value is one immutable fact about the terminal
/// the process is attached to, and `for_theme` is called from render paths —
/// `screens/settings/render.rs` calls it **per frame**. Being fixed for the
/// life of the process also keeps `Palette`'s `Hash` stable, which matters
/// because it keys the syntect theme cache in `shared/markdown`.
static DETECTED_BACKGROUND: OnceLock<Option<Background>> = OnceLock::new();

/// Records the detected background. Called once, from startup; later calls are
/// ignored, so a second one cannot re-theme a running UI.
pub fn set_detected_background(background: Option<Background>) {
    let _ = DETECTED_BACKGROUND.set(background);
}

/// The detected background, or `None` if nothing was detected. Read by
/// [`Palette::auto`]; before startup has set it, this is `None` — which is the
/// same answer as "the terminal stayed silent", and yields today's dark `Auto`.
pub fn detected_background() -> Option<Background> {
    DETECTED_BACKGROUND.get().copied().flatten()
}

/// Interface glyph set. [`UNICODE_GLYPHS`] — the redesign look (emoji and
/// decorative characters, requires a modern terminal/font with fallback:
/// Windows Terminal etc.); [`COMPAT_GLYPHS`] — compatibility mode for old
/// emulators (conhost Windows 10 and others), where emoji and rare glyphs
/// render as "tofu" boxes.
///
/// The compat set's target is **WGL4** (the base repertoire of Windows fonts:
/// Consolas/Lucida Console cover it) plus ASCII: hence `●`/`○`/`►`/`▼`/`♦`/`√`/
/// `×`/`≡`/`»` are allowed here, along with box-drawing (`─│└┼`), blocks
/// (`▌█░`), and arrows (`←↑↓→`) — these stay unreplaced elsewhere in the UI too
/// (rails, markdown tables, the scrollbar). Outside the set — emoji (`⚒`, `✻`,
/// `➕`), "rare" characters (`✦❯▸▾◆▤⌨⚙⌕▏✕⚠✓✗⟳`), the Braille spinner (`⠋⠙…`), and
/// rounded-border arc segments (`╭╮╰╯`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSet {
    /// Panel border type: rounded / straight (arc segments `╭╮╰╯` aren't in
    /// every console font).
    pub border: BorderType,
    /// Assistant icon ("✦") — reply header, panel titles.
    pub assistant_icon: &'static str,
    /// User icon ("❯") — reply header.
    pub user_icon: &'static str,
    /// System-message icon ("§") — the bubble a sub-agent transcript opens
    /// with (spec §11.3). WGL4 in both modes: it is one of the few glyphs that
    /// already is.
    pub system_icon: &'static str,
    /// Input-box prompt column ("❯ "; 2 columns wide in both sets, see
    /// `input_box::PROMPT_W`).
    pub prompt: &'static str,
    /// First-row prefix of a tool card ("⚒  ": the emoji renders 2 columns
    /// wide, hence two spaces after it — see `message_feed::push_tool`).
    pub tool_head: &'static str,
    /// Indent for tool-card continuations — the same **counted** width as
    /// [`Self::tool_head`] (aligning wraps).
    pub tool_cont: &'static str,
    /// Collapsed-block marker ("▸") — the "thoughts" pill, the "Sections" title.
    pub collapsed: &'static str,
    /// Expanded-block marker ("▾") — the "thoughts" block.
    pub expanded: &'static str,
    /// Panel/section title marker ("◆") — the chat feed, the settings section.
    pub title_marker: &'static str,
    /// Chat-list panel icon ("▤").
    pub chats_icon: &'static str,
    /// Settings panel icon ("⚙  ", width-2 emoji → two spaces).
    pub settings_icon: &'static str,
    /// Hotkey help popup icon ("⌨  ", width-2 emoji → two spaces).
    pub help_icon: &'static str,
    /// Confirmation/success ("✓").
    pub ok: &'static str,
    /// Refusal/abandoned goal ("✗").
    pub failed: &'static str,
    /// Warning/error ("⚠").
    pub warn: &'static str,
    /// "Connecting…" status in the server chip ("◐"; readiness is `●`, which
    /// is in WGL4 and isn't replaced).
    pub status_connecting: &'static str,
    /// "No connection/not configured" status in the server chip ("✕").
    pub status_off: &'static str,
    /// Active-generation indicator in the status bar ("⟳").
    pub busy: &'static str,
    /// Background-task indicator in the status bar ("✻"); 1 column wide — the
    /// hotkey grid layout depends on it.
    pub background: &'static str,
    /// Search-line icon ("⌕").
    pub search: &'static str,
    /// Search-line pseudo-cursor ("▏").
    pub caret: &'static str,
    /// "Add to dictionary" item in spelling suggestions ("➕").
    pub add: &'static str,
    /// Spinner frames for background operations (RAG indexing, impersonation).
    pub spinner: &'static [char],
}

/// Redesign glyphs (default): emoji and decorative Unicode characters.
pub static UNICODE_GLYPHS: GlyphSet = GlyphSet {
    border: BorderType::Rounded,
    assistant_icon: "✦",
    user_icon: "❯",
    system_icon: "§",
    prompt: "❯ ",
    tool_head: "⚒  ",
    tool_cont: "   ",
    collapsed: "▸",
    expanded: "▾",
    title_marker: "◆",
    chats_icon: "▤",
    settings_icon: "⚙  ",
    help_icon: "⌨  ",
    ok: "✓",
    failed: "✗",
    warn: "⚠",
    status_connecting: "◐",
    status_off: "✕",
    busy: "⟳",
    background: "✻",
    search: "⌕",
    caret: "▏",
    add: "➕",
    spinner: &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
};

/// Compatibility-mode glyphs: WGL4/ASCII only (see [`GlyphSet`]'s doc comment).
pub static COMPAT_GLYPHS: GlyphSet = GlyphSet {
    border: BorderType::Plain,
    assistant_icon: "*",
    user_icon: ">",
    system_icon: "§",
    prompt: "> ",
    tool_head: "# ",
    tool_cont: "  ",
    collapsed: "►",
    expanded: "▼",
    title_marker: "♦",
    chats_icon: "≡",
    settings_icon: "# ",
    help_icon: "# ",
    ok: "√",
    failed: "×",
    warn: "!",
    status_connecting: "○",
    status_off: "×",
    busy: "»",
    background: "*",
    search: "?",
    caret: "│",
    add: "+",
    spinner: &['|', '/', '-', '\\'],
};

/// The backgrounds the Dark/Light palettes are tuned against — and, in the
/// **full** colour mode, the ones the app paints (docs/history/theme-modes.md §4.4).
///
/// They began as the canvas of the generated screenshots
/// (docs/history/demo-screenshots.md): in the system mode the app paints no
/// background — a real terminal supplies it — so a capture needs a concrete
/// one, and `shared/shot.rs` still takes it from here. The full mode is that
/// same picture made true on a terminal: the palette's contrast is measured
/// against these values (the floor tests below), so they are the ones to put
/// under it.
pub const CANVAS_DARK: Color = Color::Rgb(15, 17, 21);
pub const CANVAS_LIGHT: Color = Color::Rgb(250, 250, 252);

/// The full mode's built-in themes, by the name `interface.full_theme` holds,
/// in the order the settings row lists them. The first is the fallback for a
/// name nothing answers to.
pub const FULL_THEMES: [&str; 2] = ["dark", "light"];

/// What a cell is drawn on to stay **reverse video** in the monochrome mode
/// (fork A of docs/history/theme-modes.md §6: a text selection and a search match —
/// the two things with no glyph to fall back on). Not a colour anybody sees:
/// `shared/ui.rs::strip_styles` turns a cell with this background into a
/// reversed one and drops the styling of every other cell, so reverse video
/// is something a widget asks for by name ([`Palette::selection`],
/// [`Palette::search_match`]) rather than something that survives by
/// accident — a popup's `reversed()` row and an unhighlighted code block are
/// stripped like the rest. An indexed colour because nothing else in the app
/// is one: the palettes are named ANSI or RGB, and highlighted code is RGB.
pub const MONO_MARK: Color = Color::Indexed(255);

/// The marker of a selected row, where the row is a list's and styling alone
/// said which one ([`Palette::selected_mark`]). The glyph the settings
/// screen's choice popups already mark theirs with; WGL4, so it needs no
/// compatibility twin.
pub const SELECTED_MARK: &str = "› ";

/// Full-theme names already reported as unknown — [`Palette::for_interface`]
/// runs per frame on the settings screen, and the log wants one line per name,
/// not one per frame (the `shared/i18n.rs` missing-key precedent).
static WARNED_UNKNOWN_THEME: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Semantic interface colors. `Copy` — cheap to pass into render by value.
/// `Hash` — the palette serves as a cache key (e.g. the built syntect
/// code-highlighting theme in `shared/markdown.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Rail/accent of a user message (blue).
    pub user: Color,
    /// Rail/accent of an assistant message (green).
    pub assistant: Color,
    /// Tool blocks (tool name/arguments; amber).
    pub tool: Color,
    /// Success/readiness (the "ready" status, the active-chat marker).
    pub success: Color,
    /// Warning (connecting/not configured, the editing indicator).
    pub warning: Color,
    /// Error (no connection, a spellcheck error).
    pub error: Color,
    /// Accent (the generation/token indicator).
    pub accent: Color,
    /// "Soft" (lighter/less saturated) variant of the user color — for the
    /// reply-header text (the rail stays the saturated `user`).
    pub user_soft: Color,
    /// "Soft" variant of the assistant color (reply-header text).
    pub assistant_soft: Color,
    /// "Soft" variant of the tool color (name in a tool-call card).
    pub tool_soft: Color,
    /// Primary text color.
    pub text: Color,
    /// Muted text (secondary captions, hotkey descriptions).
    pub muted: Color,
    /// Color of a regular panel border.
    pub border: Color,
    /// Border color of a focused panel (input, the active list).
    pub border_focus: Color,
    /// "Keycap" text in the hotkey line.
    pub keycap_fg: Color,
    /// "Keycap" background in the hotkey line — **and the selection backdrop
    /// throughout the app**: the chat list, search, the self-model screen,
    /// settings rows and fields, the emoji picker, the help dialog's tabs and
    /// leaders, and chat popups all draw their selected row on it. Changing it
    /// is never a hotkey-line-only change, which is why it is one of the two
    /// values `Auto` has to get right per background (see [`Palette::auto`]).
    pub keycap_bg: Color,
    /// Text of a "dangerous" key (e.g. `Del` for delete) on the `keycap_bg`
    /// background. Separate from `error`: the "keycap" is muted and dark, so
    /// its red is taken **brighter** than regular `error`, to stay readable and
    /// not blend into the pill's dark background.
    pub keycap_danger: Color,
    /// Whether the theme's background is dark. Needed wherever a color must be
    /// given as absolute RGB (no named ANSI that adapts to the terminal) — e.g.
    /// "default" gray and the comment color in code highlighting: light on a
    /// dark background, dark on a light one. Under `Auto` this follows what the
    /// terminal reported over OSC 11 ([`Palette::auto`]), falling back to dark
    /// when nothing answered.
    pub dark: bool,
    /// Old-terminal compatibility mode (`config.interface.terminal_compat`):
    /// glyphs come from [`COMPAT_GLYPHS`] (see [`Palette::glyphs`]), and popup
    /// background dimming goes by color instead of `DIM`
    /// (`shared/ui.rs::dim_background`). Not a color, but lives in the palette
    /// (like `dark`): it is already threaded through every render.
    pub compat: bool,
    /// The background the app paints under every cell — the **full** colour
    /// mode (spec §11.6). `Color::Reset` is the system mode: the terminal's own
    /// background, and `shared/ui.rs::paint_canvas` leaves the frame alone.
    /// Widgets do not read it: they keep drawing foregrounds, and the pass
    /// over the finished frame fills in what they left at the default.
    pub canvas: Color,
    /// Plain text inside a highlighted code block (`shared/markdown/code.rs`)
    /// — what the syntax gives no colour of its own. A grey, light on a dark
    /// background and dark on a light one; a role of the palette because a
    /// user theme's canvas is not one of the two these were tuned on
    /// (docs/history/theme-modes.md §10.2).
    pub code_text: Color,
    /// Comments in highlighted code. Text like any other, and held to the
    /// same floor.
    pub code_comment: Color,
    /// The **monochrome** mode (spec §11.6): the finished frame loses every
    /// colour and attribute (`shared/ui.rs::strip_styles`), so whatever
    /// styling alone would have said has to be a glyph. Widgets read it for
    /// that — through the helpers below where one fits — and keep setting
    /// their colours as always: those are dropped after them.
    pub mono: bool,
}

impl Palette {
    /// The palette the interface settings ask for: the colour mode, that
    /// mode's theme, and the compatibility flag. The one place the settings
    /// become a palette — the chat screen, the settings screen and the
    /// broadcast to the overlay screens all come through here.
    pub fn for_interface(interface: &InterfaceSettings) -> Self {
        let palette = match interface.mode() {
            ThemeMode::System => Self::for_theme(interface.theme),
            ThemeMode::Full => Self::full(&interface.full_theme),
            ThemeMode::Mono => Self::mono(),
        };
        palette.with_compat(interface.terminal_compat)
    }

    /// The full mode's palette for a theme name: a built-in palette on the
    /// canvas it was tuned against, or a theme of the user's
    /// (`data/themes/<name>.json`, `shared/user_theme.rs`). A name nothing
    /// answers to — a theme from another machine, a file that went away —
    /// draws as the first built-in one and says so in the log, once.
    pub fn full(name: &str) -> Self {
        if let Some(palette) = Self::built_in(name) {
            return palette;
        }
        if let Some(theme) = crate::shared::user_theme::registry().get(name) {
            return theme.palette;
        }
        let mut warned = WARNED_UNKNOWN_THEME
            .lock()
            .expect("unknown-theme set poisoned");
        if warned.insert(name.to_string()) {
            tracing::warn!(
                theme = name,
                fallback = FULL_THEMES[0],
                "interface.full_theme names a theme that is neither built in nor in data/themes"
            );
        }
        Self::dark().on_canvas(CANVAS_DARK)
    }

    /// A **built-in** full theme by its name ([`FULL_THEMES`]); `None` for
    /// any other. What a user theme's missing roles start from, and what
    /// [`Palette::full`] answers with before it asks the registry — a file
    /// cannot take a built-in theme's name.
    pub fn built_in(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark().on_canvas(CANVAS_DARK)),
            "light" => Some(Self::light().on_canvas(CANVAS_LIGHT)),
            _ => None,
        }
    }

    /// The monochrome mode's palette. Its colours never reach the terminal,
    /// so they are one fixed set — the dark theme's — whatever
    /// `interface.theme` says: one palette, and one entry in each cache a
    /// palette keys.
    pub fn mono() -> Self {
        Self {
            mono: true,
            ..Self::dark()
        }
    }

    /// The same palette, painting `canvas` under the frame.
    fn on_canvas(mut self, canvas: Color) -> Self {
        self.canvas = canvas;
        self
    }

    /// Palette for the selected theme of the **system** mode: foregrounds
    /// only, over whatever background the terminal has.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Auto => Self::auto(),
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    /// "Auto" — named ANSI colors: their shades are set by the terminal
    /// itself, so the palette adapts to the terminal's theme (the pre-redesign
    /// behavior). Structural colors (borders/muted) are neutral ANSI grays.
    ///
    /// The two things that cannot be expressed in named ANSI — the `dark` flag
    /// and the "keycap" trio — follow the **detected** background
    /// ([`detected_background`], spec §11.6): the terminal is asked over OSC 11
    /// at startup, and when it answers, `Auto` borrows whichever of
    /// [`Palette::dark`]/[`Palette::light`]'s tuned values matches. Nothing
    /// answered (legacy conhost, a pipe, the query suppressed) → dark, which is
    /// what those hosts are. See
    /// [docs/terminal-background-detection.md](../../docs/terminal-background-detection.md).
    ///
    /// Deliberately *only* those two: the role colors stay named ANSI even on a
    /// light background, because a terminal themed light supplies its own
    /// legible shades and that adaptivity is the whole point of `Auto` (design
    /// plan §5, F1 — the user's decision, 2026-08-26). Wanting the tuned light
    /// palette instead is what picking `Light` in settings is for.
    fn auto() -> Self {
        Self::auto_with(detected_background())
    }

    /// [`Palette::auto`] for an explicitly given background.
    ///
    /// The seam exists for the tests: the detected value lives in a
    /// process-wide `OnceLock`, which by construction cannot be set to two
    /// different things in one test binary.
    pub(crate) fn auto_with(background: Option<Background>) -> Self {
        let light = background == Some(Background::Light);
        // Not new colors: the same pair `dark()`/`light()` already carry, so a
        // detected polarity looks like the theme designed for it rather than
        // like a third, half-tuned variant.
        let reference = if light { Self::light() } else { Self::dark() };
        Self {
            user: Color::Cyan,
            assistant: Color::Green,
            tool: Color::Yellow,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            accent: Color::Magenta,
            user_soft: Color::LightCyan,
            assistant_soft: Color::LightGreen,
            tool_soft: Color::LightYellow,
            text: Color::Reset,
            muted: Color::DarkGray,
            border: Color::DarkGray,
            border_focus: Color::Gray,
            // "Keycaps" — quiet pills, and the backdrop every selected row in
            // the app is drawn on (see the `keycap_bg` field docs). Named ANSI
            // has no step suitable for either polarity, so these are absolute
            // RGB, taken from whichever theme matches the detected background.
            keycap_fg: reference.keycap_fg,
            keycap_bg: reference.keycap_bg,
            keycap_danger: reference.keycap_danger,
            // The same reasoning: greys with no adaptable ANSI counterpart.
            code_text: reference.code_text,
            code_comment: reference.code_comment,
            dark: !light,
            compat: false,
            canvas: Color::Reset,
            mono: false,
        }
    }

    /// Dark theme — shades from the design mockup (oklch → sRGB). Calm,
    /// "terminal": a deep background, soft borders, saturated rails.
    ///
    /// Tuned against [`CANVAS_DARK`], and held to the contrast floors by a
    /// test (`built_in_palettes_clear_the_contrast_floors`): `muted` and
    /// `keycap_fg` were each a shade lighter in the mockup and fell under
    /// 4.5:1 on a selected row (docs/history/theme-modes.md §3.1).
    fn dark() -> Self {
        Self {
            user: Color::Rgb(121, 169, 219),           // blue
            assistant: Color::Rgb(111, 192, 130),      // green
            tool: Color::Rgb(220, 175, 97),            // amber
            success: Color::Rgb(111, 192, 130),        // green
            warning: Color::Rgb(220, 175, 97),         // amber
            error: Color::Rgb(223, 105, 92),           // red
            accent: Color::Rgb(220, 175, 97),          // amber (generation/tokens)
            user_soft: Color::Rgb(144, 188, 233),      // blueSoft
            assistant_soft: Color::Rgb(164, 209, 172), // greenSoft
            tool_soft: Color::Rgb(230, 201, 154),      // amberSoft
            text: Color::Rgb(201, 204, 210),           // #c9ccd2
            muted: Color::Rgb(133, 139, 146), // #858b92 — the mockup's #7e848b, lifted to the floor
            border: Color::Rgb(54, 58, 66), // a bit brighter than the mockup's #24272e for visibility
            border_focus: Color::Rgb(110, 117, 128), // #6e7580 (focus)
            keycap_fg: Color::Rgb(133, 139, 147), // #858b93 — quiet, and 4.5:1 on its own pill
            keycap_bg: Color::Rgb(33, 36, 42), // darker than the previous #2a2d34
            keycap_danger: Color::Rgb(232, 116, 104), // brighter than error for readability on the pill
            code_text: Color::Rgb(212, 212, 212),
            code_comment: Color::Rgb(138, 138, 138), // #8a8a8a — 128 was under the floor on a selected row
            dark: true,
            compat: false,
            canvas: Color::Reset,
            mono: false,
        }
    }

    /// Light theme — darkened colors (yellow/bright ones are unreadable on white).
    ///
    /// Tuned against [`CANVAS_LIGHT`] and held to the same floors as
    /// [`Palette::dark`]. Every colour is absolute: `user` used to be the
    /// named ANSI blue, the one value here that a terminal chose — which a
    /// "fixed" palette should not have, and a palette that owns its background
    /// cannot (docs/history/theme-modes.md §3.1).
    fn light() -> Self {
        Self {
            user: Color::Rgb(0, 55, 218), // #0037da — the blue Windows Terminal draws for ANSI blue
            assistant: Color::Rgb(0, 116, 0), // #007400
            tool: Color::Rgb(144, 85, 0), // #905500
            success: Color::Rgb(0, 116, 0),
            warning: Color::Rgb(144, 85, 0),
            error: Color::Rgb(180, 0, 0),
            accent: Color::Rgb(140, 0, 140),
            user_soft: Color::Rgb(40, 80, 170),
            assistant_soft: Color::Rgb(0, 110, 0),
            tool_soft: Color::Rgb(142, 87, 0), // #8e5700
            text: Color::Rgb(30, 32, 36),
            muted: Color::Rgb(95, 100, 108), // #5f646c
            border: Color::Rgb(190, 193, 198),
            border_focus: Color::Rgb(120, 124, 130),
            keycap_fg: Color::Rgb(74, 78, 84), // softer than black — "keycaps" don't shout
            keycap_bg: Color::Rgb(222, 224, 228),
            keycap_danger: Color::Rgb(178, 34, 34),
            code_text: Color::Rgb(40, 40, 40),
            code_comment: Color::Rgb(99, 99, 99), // #636363 — 110 was under the floor on a selected row
            dark: false,
            compat: false,
            canvas: Color::Reset,
            mono: false,
        }
    }

    /// The text roles — everything that is read rather than merely seen — with
    /// the names the floor test reports them by. `border` and `keycap_bg` are
    /// not text and are deliberately absent (docs/history/theme-modes.md §4.4). Taken
    /// from the table a theme file is read by, so a role added there is held
    /// to a floor here without being listed twice.
    #[cfg(test)]
    fn text_roles(&self) -> Vec<(&'static str, Color)> {
        use crate::shared::user_theme::{Kind, ROLES};
        ROLES
            .iter()
            .filter(|role| matches!(role.kind, Kind::Body | Kind::Text))
            .map(|role| (role.name, (role.get)(self)))
            .collect()
    }

    /// The same palette with the compatibility mode flag set (builder style;
    /// handy inline: `Palette::for_theme(t).with_compat(flag)`).
    pub fn with_compat(mut self, compat: bool) -> Self {
        self.compat = compat;
        self
    }

    /// The active glyph set: unicode by default, safe in old-terminal
    /// compatibility mode. See [`GlyphSet`].
    pub fn glyphs(&self) -> &'static GlyphSet {
        if self.compat {
            &COMPAT_GLYPHS
        } else {
            &UNICODE_GLYPHS
        }
    }

    // ---- What the monochrome mode says with glyphs ----

    /// The marker a list puts before its selected row, where styling alone
    /// said which row that is — `None` outside the monochrome mode. The
    /// unselected rows are indented by as much (`shared/ui.rs::mark_selected`).
    pub fn selected_mark(&self) -> Option<&'static str> {
        self.mono.then_some(SELECTED_MARK)
    }

    /// A tab's label as the strip draws it: ` label `, and `[label]` for the
    /// active tab in the monochrome mode — the same width, so the strip does
    /// not move when the tab changes.
    pub fn tab_label(&self, label: &str, active: bool) -> String {
        if self.mono && active {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        }
    }

    /// `style` as the **text selection** draws it: on the selection backdrop,
    /// or — monochrome — on the mark that stays reverse video.
    pub fn selection(&self, style: Style) -> Style {
        style.bg(if self.mono { MONO_MARK } else { self.keycap_bg })
    }

    /// `style` as a **search match** draws it: recoloured to the accent, or —
    /// monochrome — on the mark that stays reverse video. Only that is
    /// patched, so the markdown styling under a match survives.
    pub fn search_match(&self, style: Style) -> Style {
        if self.mono {
            style.bg(MONO_MARK)
        } else {
            style.fg(self.accent)
        }
    }

    // ---- Convenient style constructors (foreground by role) ----
    pub fn success_style(&self) -> Style {
        Style::new().fg(self.success)
    }
    pub fn accent_style(&self) -> Style {
        Style::new().fg(self.accent)
    }
    /// Style for muted text (secondary captions/descriptions).
    pub fn muted_style(&self) -> Style {
        Style::new().fg(self.muted)
    }

    /// Panel border style (`focused` → the highlighted border).
    pub fn border_style(&self, focused: bool) -> Style {
        Style::new().fg(if focused {
            self.border_focus
        } else {
            self.border
        })
    }

    /// A rounded panel (`Block`) with a title and optional focus — the
    /// redesign's unified "frame". The title is drawn on the top line, the
    /// border in palette color; in compatibility mode the border is straight
    /// (see [`GlyphSet::border`]).
    pub fn panel(&self, title: impl Into<String>, focused: bool) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(self.glyphs().border)
            .border_style(self.border_style(focused))
            .title(Span::styled(
                format!(" {} ", title.into()),
                Style::new().fg(self.text),
            ))
    }

    /// A hotkey "keycap" — a label on a muted background (like a pill in the
    /// mockup).
    pub fn keycap(&self, label: impl Into<String>) -> Span<'static> {
        Span::styled(
            self.keycap_text(&label.into()),
            Style::new().fg(self.keycap_fg).bg(self.keycap_bg),
        )
    }

    /// A keycap's text: the label with a column of the pill on either side,
    /// and in the monochrome mode — where there is no pill — in brackets, the
    /// same width. Without them a hint bar is keys and descriptions told
    /// apart by the number of spaces between them.
    fn keycap_text(&self, label: &str) -> String {
        if self.mono {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        }
    }

    /// A hotkey hint: "keycap" + a muted description. Returns spans to insert
    /// into a line (with a leading indent space before the keycap).
    pub fn hint(&self, key: &str, desc: &str) -> Vec<Span<'static>> {
        vec![
            self.keycap(key),
            Span::styled(format!(" {desc}"), self.muted_style()),
        ]
    }

    /// Like `hint`, but the **value after the colon** is highlighted in
    /// `color`, while the label (the colon itself included) stays muted — for
    /// highlighting the value of the active mode (e.g. "scroll" in "mouse:
    /// scroll" in `accent` color, like markdown headings in the feed). Splits
    /// on `:`, not on a space, which is correct for multi-word values
    /// (important for future localization). With no colon — the whole
    /// description is highlighted.
    pub fn hint_highlight_value(&self, key: &str, desc: &str, color: Color) -> Vec<Span<'static>> {
        let mut spans = vec![self.keycap(key)];
        match desc.split_once(':') {
            Some((label, value)) => {
                spans.push(Span::styled(format!(" {label}: "), self.muted_style()));
                spans.push(Span::styled(
                    value.trim().to_string(),
                    Style::new().fg(color),
                ));
            }
            None => spans.push(Span::styled(format!(" {desc}"), Style::new().fg(color))),
        }
        spans
    }

    /// Like [`Palette::hint`], but a "dangerous" key (delete and friends) gets a
    /// red keycap. The one place the two keycap styles are chosen between:
    /// every hint in the application is drawn by
    /// [`crate::shared::ui::render_hint_grid`], which calls this.
    pub fn hint_marked(&self, key: &str, desc: &str, danger: bool) -> Vec<Span<'static>> {
        if !danger {
            return self.hint(key, desc);
        }
        vec![
            Span::styled(
                self.keycap_text(key),
                Style::new().fg(self.keycap_danger).bg(self.keycap_bg),
            ),
            Span::styled(format!(" {desc}"), self.muted_style()),
        ]
    }
}

/// WCAG 2.x contrast ratio of two absolute colours, 1 (none) to 21 (black on
/// white). What the floor tests measure the built-in palettes with; stage 3 of
/// the colour-modes track (user themes) is its first production consumer.
///
/// Panics on anything but `Color::Rgb`: a named ANSI colour has no contrast
/// until a terminal picks its shade, and a test that guessed one would be
/// measuring the guess.
#[cfg(test)]
pub(crate) fn contrast_ratio(a: Color, b: Color) -> f32 {
    use crate::shared::osc11::{Rgb, relative_luminance};
    let luminance = |c: Color| match c {
        Color::Rgb(r, g, b) => relative_luminance(Rgb { r, g, b }),
        other => panic!("{other:?} is not an absolute colour"),
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Visible width of a line in terminal columns. Only the glyph-set tests
/// measure anything here now — the hint grid does its own measuring in
/// [`crate::shared::ui`].
#[cfg(test)]
fn str_width(s: &str) -> usize {
    wrap::display_width(&s.chars().collect::<Vec<_>>())
}

impl Default for Palette {
    fn default() -> Self {
        Self::auto()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_named_ansi_colors() {
        let p = Palette::for_theme(Theme::Auto);
        assert_eq!(p.user, Color::Cyan);
        assert_eq!(p.error, Color::Red);
    }

    #[test]
    fn dark_and_light_differ_from_auto() {
        let auto = Palette::for_theme(Theme::Auto);
        assert_ne!(Palette::for_theme(Theme::Dark), auto);
        assert_ne!(Palette::for_theme(Theme::Light), auto);
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(Palette::default(), Palette::for_theme(Theme::Auto));
    }

    #[test]
    fn dark_flag_follows_theme() {
        // Dark counts as dark, Light as light (for absolute RGB colors in code
        // highlighting, which have no adaptable ANSI equivalent). Auto follows
        // the terminal — nothing is detected in a test binary, so it falls back
        // to dark, which is what the untested-for hosts are.
        assert!(Palette::for_theme(Theme::Auto).dark);
        assert!(Palette::for_theme(Theme::Dark).dark);
        assert!(!Palette::for_theme(Theme::Light).dark);
    }

    #[test]
    fn auto_undetected_is_byte_for_byte_the_old_palette() {
        // The fallback is the regression guard: on every host that does not
        // answer (legacy conhost, a pipe, `MINDFORK_TERMINAL_BG=off`) `Auto`
        // must render exactly as it did before detection existed — dark, with
        // the dark theme's keycap trio.
        let auto = Palette::auto_with(None);
        let dark = Palette::dark();
        assert!(auto.dark);
        assert_eq!(auto.keycap_fg, dark.keycap_fg);
        assert_eq!(auto.keycap_bg, dark.keycap_bg);
        assert_eq!(auto.keycap_danger, dark.keycap_danger);
        // …and the roles stay named ANSI, which is the half that always adapted.
        assert_eq!(auto.user, Color::Cyan);
        assert_eq!(auto.text, Color::Reset);
    }

    #[test]
    fn auto_detected_light_borrows_the_light_keycaps() {
        // The defect this track exists to fix: on a light terminal every
        // selected row was a dark bar on white, because `keycap_bg` is the
        // selection backdrop app-wide.
        let auto = Palette::auto_with(Some(Background::Light));
        let light = Palette::light();
        assert!(!auto.dark, "a light background must not claim to be dark");
        assert_eq!(auto.keycap_fg, light.keycap_fg);
        assert_eq!(auto.keycap_bg, light.keycap_bg);
        assert_eq!(auto.keycap_danger, light.keycap_danger);
    }

    #[test]
    fn auto_detected_light_keeps_the_named_ansi_roles() {
        // F1 (a), the user's decision: only `dark` and the keycap trio follow
        // the background. The role colors stay named ANSI even on a light
        // terminal, so `Auto` keeps adapting and stays distinct from `Light`.
        let auto = Palette::auto_with(Some(Background::Light));
        assert_eq!(auto.user, Color::Cyan);
        assert_eq!(auto.assistant, Color::Green);
        assert_eq!(auto.tool, Color::Yellow);
        assert_eq!(auto.text, Color::Reset);
        assert_ne!(
            auto,
            Palette::light(),
            "Auto on a light terminal must not collapse into Light"
        );
    }

    #[test]
    fn auto_detected_dark_is_the_fallback_palette() {
        // Detecting dark and detecting nothing must agree — otherwise the
        // fallback would be a third look nobody designed.
        assert_eq!(
            Palette::auto_with(Some(Background::Dark)),
            Palette::auto_with(None)
        );
    }

    fn interface(mode: ThemeMode, theme: Theme, full_theme: &str) -> InterfaceSettings {
        let mut i = InterfaceSettings {
            theme,
            full_theme: full_theme.to_string(),
            ..Default::default()
        };
        i.set_mode(mode);
        i
    }

    #[test]
    fn the_system_mode_is_the_palette_it_always_was() {
        // The regression guard for everyone who never touches the new row: the
        // settings as they are after an upgrade give exactly `for_theme`, with
        // no canvas — so the pass over the frame has nothing to do.
        for theme in [Theme::Auto, Theme::Dark, Theme::Light] {
            let p = Palette::for_interface(&interface(ThemeMode::System, theme, "light"));
            assert_eq!(p, Palette::for_theme(theme), "{theme:?}");
            assert_eq!(p.canvas, Color::Reset, "{theme:?} paints a background");
        }
        assert_eq!(
            Palette::for_interface(&InterfaceSettings::default()),
            Palette::default()
        );
    }

    #[test]
    fn the_full_mode_is_a_built_in_palette_on_its_canvas() {
        // Fork B (user's decision, 2026-09-27): the two modes share their
        // palettes, so the full mode is the system mode's colours plus the
        // canvas they were tuned against — and nothing else.
        for (name, base, canvas) in [
            ("dark", Palette::dark(), CANVAS_DARK),
            ("light", Palette::light(), CANVAS_LIGHT),
        ] {
            // `theme` is the *other* mode's field and must not leak in.
            let p = Palette::for_interface(&interface(ThemeMode::Full, Theme::Auto, name));
            assert_eq!(p.canvas, canvas, "{name}");
            assert_eq!(
                Palette {
                    canvas: Color::Reset,
                    ..p
                },
                base,
                "{name}: the full palette differs from the shared one in more than its canvas"
            );
        }
    }

    #[test]
    fn every_listed_full_theme_is_its_own_palette() {
        // A name in the list that nothing answers to would reach the fallback
        // and show `dark` under another label.
        let palettes: Vec<Palette> = FULL_THEMES.iter().map(|n| Palette::full(n)).collect();
        for (i, a) in palettes.iter().enumerate() {
            for b in &palettes[i + 1..] {
                assert_ne!(a, b, "two listed themes draw the same palette");
            }
        }
    }

    /// A theme of the user's answers to its name; the built-in names are
    /// answered before the registry is asked, so a registry holding one of
    /// them — which the loader refuses to build — still could not replace it.
    #[test]
    fn a_full_theme_is_built_in_or_the_users() {
        use crate::shared::user_theme::{Registry, parse, with_registry};
        let registry = Registry::of([
            parse("sepia", r##"{"canvas": "#f4ecd8"}"##).unwrap(),
            parse("dark", r##"{"canvas": "#ffffff"}"##).unwrap(),
        ]);
        let sepia = registry.get("sepia").unwrap().palette;
        with_registry(registry, || {
            assert_eq!(Palette::full("sepia"), sepia);
            assert_eq!(Palette::full("sepia").canvas, Color::Rgb(0xf4, 0xec, 0xd8));
            assert_eq!(Palette::full("dark").canvas, CANVAS_DARK);
            assert_eq!(Palette::full("gruvbox"), Palette::full("dark"));

            let p = Palette::for_interface(&interface(ThemeMode::Full, Theme::Dark, "sepia"));
            assert_eq!(p, sepia);
            // The other modes do not look at the name.
            let p = Palette::for_interface(&interface(ThemeMode::System, Theme::Dark, "sepia"));
            assert_eq!(p, Palette::for_theme(Theme::Dark));
            let p = Palette::for_interface(&interface(ThemeMode::Mono, Theme::Dark, "sepia"));
            assert_eq!(p, Palette::mono());
        });
        // Outside the test that brought it, the name answers to nothing.
        assert_eq!(Palette::full("sepia"), Palette::full("dark"));
    }

    #[test]
    fn the_built_in_themes_are_the_listed_ones() {
        for name in FULL_THEMES {
            assert_eq!(Palette::built_in(name), Some(Palette::full(name)), "{name}");
        }
        for name in ["", "Dark", "auto", "sepia", "mono"] {
            assert_eq!(Palette::built_in(name), None, "{name:?}");
        }
    }

    #[test]
    fn an_unknown_full_theme_draws_as_the_first_built_in_one() {
        assert_eq!(Palette::full("gruvbox"), Palette::full(FULL_THEMES[0]));
        // Asked twice (the settings screen asks per frame) — same answer, and
        // the name is remembered as reported.
        assert_eq!(Palette::full("gruvbox"), Palette::full("dark"));
        assert!(
            WARNED_UNKNOWN_THEME
                .lock()
                .unwrap()
                .contains(&"gruvbox".to_string())
        );
    }

    #[test]
    fn the_monochrome_mode_is_one_palette_whatever_the_themes_say() {
        // Its colours never reach the terminal, so there is nothing for a
        // theme to choose — and one palette is one entry in the caches it
        // keys, not one per theme passed through.
        let mono = Palette::mono();
        assert!(mono.mono);
        assert_eq!(mono.canvas, Color::Reset, "it paints no background");
        for theme in [Theme::Auto, Theme::Dark, Theme::Light] {
            for full in ["dark", "light", "gruvbox"] {
                let p = Palette::for_interface(&interface(ThemeMode::Mono, theme, full));
                assert_eq!(p, mono, "{theme:?} / {full}");
            }
        }
        // The other modes are not monochrome, and differ from it.
        for mode in [ThemeMode::System, ThemeMode::Full] {
            let p = Palette::for_interface(&interface(mode, Theme::Dark, "dark"));
            assert!(!p.mono, "{mode:?}");
            assert_ne!(p, mono, "{mode:?}");
        }
    }

    #[test]
    fn the_mark_is_a_colour_no_palette_has() {
        // `strip_styles` reads "drawn on the mark" off a cell's background:
        // a palette that used the value for anything else would get reverse
        // video it did not ask for.
        let palettes = [
            Palette::auto_with(None),
            Palette::auto_with(Some(Background::Light)),
            Palette::dark(),
            Palette::light(),
            Palette::mono(),
        ];
        for p in palettes {
            let all = p.text_roles().into_iter().chain([
                ("border", p.border),
                ("border_focus", p.border_focus),
                ("keycap_bg", p.keycap_bg),
                ("canvas", p.canvas),
            ]);
            for (role, color) in all {
                assert_ne!(color, MONO_MARK, "{role}");
            }
        }
        for canvas in [CANVAS_DARK, CANVAS_LIGHT] {
            assert_ne!(canvas, MONO_MARK);
        }
    }

    #[test]
    fn a_glyph_says_in_monochrome_what_styling_says_elsewhere() {
        let (plain, mono) = (Palette::dark(), Palette::mono());

        // A list's selected row.
        assert_eq!(plain.selected_mark(), None);
        assert_eq!(mono.selected_mark(), Some(SELECTED_MARK));

        // A tab: brackets for the active one, and no tab changes width — the
        // strip must not move when the tab does.
        assert_eq!(plain.tab_label("Keys", true), " Keys ");
        assert_eq!(plain.tab_label("Keys", false), " Keys ");
        assert_eq!(mono.tab_label("Keys", true), "[Keys]");
        assert_eq!(mono.tab_label("Keys", false), " Keys ");

        // A keycap, the dangerous one included.
        assert_eq!(plain.keycap("Enter").content, " Enter ");
        assert_eq!(mono.keycap("Enter").content, "[Enter]");
        assert_eq!(plain.hint_marked("Del", "delete", true)[0].content, " Del ");
        assert_eq!(mono.hint_marked("Del", "delete", true)[0].content, "[Del]");
        assert_eq!(mono.hint("F1", "help")[0].content, "[F1]");
    }

    #[test]
    fn the_two_marks_are_the_only_things_drawn_on_the_mark() {
        let (plain, mono) = (Palette::dark(), Palette::mono());
        let under = Style::new().bold().fg(Color::Red);

        // A search match keeps what is under it, in either mode.
        assert_eq!(plain.search_match(under), under.fg(plain.accent));
        assert_eq!(mono.search_match(under), under.bg(MONO_MARK));
        // A text selection.
        assert_eq!(plain.selection(under), under.bg(plain.keycap_bg));
        assert_eq!(mono.selection(under), under.bg(MONO_MARK));

        // Nothing else the palette hands out sits on it.
        let handed_out = [
            mono.keycap("K").style,
            mono.hint_marked("K", "d", true)[0].style,
            mono.muted_style(),
            mono.accent_style(),
            mono.success_style(),
            mono.border_style(true),
        ];
        for style in handed_out {
            assert_ne!(style.bg, Some(MONO_MARK));
        }
    }

    #[test]
    fn the_compatibility_flag_rides_on_either_mode() {
        for mode in ThemeMode::ALL {
            let mut i = interface(mode, Theme::Dark, "dark");
            i.terminal_compat = true;
            assert!(Palette::for_interface(&i).compat, "{mode:?}");
        }
    }

    #[test]
    fn a_full_palette_leaves_nothing_to_the_terminal() {
        // A palette that owns its background has to own every colour on it: a
        // named ANSI colour is whatever shade the terminal draws, picked for
        // the terminal's background, not for this canvas.
        for name in FULL_THEMES {
            let p = Palette::full(name);
            let all = p
                .text_roles()
                .into_iter()
                .chain([
                    ("border", p.border),
                    ("border_focus", p.border_focus),
                    ("keycap_bg", p.keycap_bg),
                    ("canvas", p.canvas),
                ])
                .collect::<Vec<_>>();
            for (role, color) in all {
                assert!(
                    matches!(color, Color::Rgb(..)),
                    "{name}: {role} is {color:?}, not an absolute colour"
                );
            }
        }
    }

    #[test]
    fn a_canvas_is_on_the_side_its_palette_says() {
        // `dark` drives the code-block greys; a palette claiming one polarity
        // on a canvas of the other would put dark grey on a dark background.
        for name in FULL_THEMES {
            let p = Palette::full(name);
            let is_dark = contrast_ratio(p.canvas, Color::Rgb(255, 255, 255))
                > contrast_ratio(p.canvas, Color::Rgb(0, 0, 0));
            assert_eq!(p.dark, is_dark, "{name}");
        }
    }

    /// The floors of docs/history/theme-modes.md §4.4, as the gate they were written
    /// to be. Every text role is measured on the canvas **and** on the
    /// selection backdrop, because any row can be the selected one.
    #[test]
    fn built_in_palettes_clear_the_contrast_floors() {
        const BODY: f32 = 7.0; // WCAG AAA
        const TEXT: f32 = 4.5; // WCAG AA
        const COMPONENT: f32 = 3.0; // WCAG 1.4.11, non-text

        let mut short: Vec<String> = Vec::new();
        for name in FULL_THEMES {
            let p = Palette::full(name);
            for (role, color) in p.text_roles() {
                let floor = if role == "text" { BODY } else { TEXT };
                for (ground, on) in [("canvas", p.canvas), ("selection", p.keycap_bg)] {
                    let got = contrast_ratio(color, on);
                    if got < floor {
                        short.push(format!(
                            "{name}: {role} on {ground} is {got:.2}, floor {floor}"
                        ));
                    }
                }
            }
            let focus = contrast_ratio(p.border_focus, p.canvas);
            if focus < COMPONENT {
                short.push(format!(
                    "{name}: border_focus is {focus:.2}, floor {COMPONENT}"
                ));
            }
        }
        assert!(short.is_empty(), "below the floor:\n{}", short.join("\n"));
    }

    #[test]
    fn the_contrast_ratio_is_the_wcag_one() {
        let (black, white) = (Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01, "order");
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.001);
        // A value measured outside the code (docs/history/theme-modes.md §3.1): the
        // dark palette's body text on its canvas.
        let got = contrast_ratio(Color::Rgb(201, 204, 210), CANVAS_DARK);
        assert!((got - 11.74).abs() < 0.01, "{got}");
    }

    #[test]
    fn keycap_and_hint_carry_label() {
        let p = Palette::default();
        let cap = p.keycap("Ctrl+N");
        assert!(cap.content.contains("Ctrl+N"));
        let hint = p.hint("Enter", "отправить");
        let joined: String = hint.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains("Enter") && joined.contains("отправить"));
    }

    #[test]
    fn glyphs_follow_compat_flag() {
        // By default — the unicode set and rounded borders; in compatibility
        // mode — the safe set and straight borders.
        let p = Palette::default();
        assert!(!p.compat);
        assert_eq!(p.glyphs(), &UNICODE_GLYPHS);
        assert_eq!(p.glyphs().border, BorderType::Rounded);
        let c = p.with_compat(true);
        assert_eq!(c.glyphs(), &COMPAT_GLYPHS);
        assert_eq!(c.glyphs().border, BorderType::Plain);
    }

    #[test]
    fn compat_glyphs_avoid_rare_symbols() {
        // No replaced emoji/rare characters should leak into the compat set
        // (they are exactly the reason for the mode: an old terminal draws
        // them as "tofu").
        let banned: Vec<char> = "✦❯⚒▸▾◆▤⚙⌨✓✗⚠◐✕⟳✻⌕▏➕".chars().collect();
        let g = &COMPAT_GLYPHS;
        let all = [
            g.assistant_icon,
            g.user_icon,
            g.system_icon,
            g.prompt,
            g.tool_head,
            g.tool_cont,
            g.collapsed,
            g.expanded,
            g.title_marker,
            g.chats_icon,
            g.settings_icon,
            g.help_icon,
            g.ok,
            g.failed,
            g.warn,
            g.status_connecting,
            g.status_off,
            g.busy,
            g.background,
            g.search,
            g.caret,
            g.add,
        ];
        for s in all {
            for ch in s.chars() {
                assert!(
                    !banned.contains(&ch),
                    "rare character in compat set: {ch:?}"
                );
            }
        }
        // The spinner is pure ASCII (old consoles don't render Braille frames).
        assert!(g.spinner.iter().all(|c| c.is_ascii()), "{:?}", g.spinner);
    }

    #[test]
    fn tool_head_and_cont_widths_match_in_both_sets() {
        // Tool-card continuations align under the first row — the counted
        // widths of the prefixes must match in both sets (see
        // message_feed::push_tool).
        for g in [&UNICODE_GLYPHS, &COMPAT_GLYPHS] {
            assert_eq!(str_width(g.tool_head), str_width(g.tool_cont));
        }
        // The input prompt column is always 2 columns (input_box::PROMPT_W).
        assert_eq!(str_width(UNICODE_GLYPHS.prompt), 2);
        assert_eq!(str_width(COMPAT_GLYPHS.prompt), 2);
    }
}
