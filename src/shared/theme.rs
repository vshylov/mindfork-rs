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

use std::sync::OnceLock;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::shared::config::Theme;
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

/// Canvas backgrounds for generated screenshots (docs/history/demo-screenshots.md).
/// The app itself never paints a background — a real terminal supplies it —
/// so captures need a concrete one. These are the backgrounds the Dark/Light
/// palettes are tuned against; they live here (not in the Python renderer) so
/// the intended canvas has one source of truth next to the palettes.
/// Test-gated with the capture pipeline (`shared/shot.rs`).
#[cfg(test)]
pub const SHOT_CANVAS_DARK: Color = Color::Rgb(15, 17, 21);
#[cfg(test)]
pub const SHOT_CANVAS_LIGHT: Color = Color::Rgb(250, 250, 252);

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
}

impl Palette {
    /// Palette for the selected theme.
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
            dark: !light,
            compat: false,
        }
    }

    /// Dark theme — exact shades from the design mockup (oklch → sRGB). Calm,
    /// "terminal": a deep background, soft borders, saturated rails.
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
            muted: Color::Rgb(126, 132, 139),          // #7e848b
            border: Color::Rgb(54, 58, 66), // a bit brighter than the mockup's #24272e for visibility
            border_focus: Color::Rgb(110, 117, 128), // #6e7580 (focus)
            keycap_fg: Color::Rgb(132, 138, 146), // quieter than the previous #9aa0a7
            keycap_bg: Color::Rgb(33, 36, 42), // darker than the previous #2a2d34
            keycap_danger: Color::Rgb(232, 116, 104), // brighter than error for readability on the pill
            dark: true,
            compat: false,
        }
    }

    /// Light theme — darkened colors (yellow/bright ones are unreadable on white).
    fn light() -> Self {
        Self {
            user: Color::Blue,
            assistant: Color::Rgb(0, 128, 0),
            tool: Color::Rgb(160, 100, 0),
            success: Color::Rgb(0, 128, 0),
            warning: Color::Rgb(160, 100, 0),
            error: Color::Rgb(180, 0, 0),
            accent: Color::Rgb(140, 0, 140),
            user_soft: Color::Rgb(40, 80, 170),
            assistant_soft: Color::Rgb(0, 110, 0),
            tool_soft: Color::Rgb(150, 95, 0),
            text: Color::Rgb(30, 32, 36),
            muted: Color::Rgb(110, 116, 124),
            border: Color::Rgb(190, 193, 198),
            border_focus: Color::Rgb(120, 124, 130),
            keycap_fg: Color::Rgb(74, 78, 84), // softer than black — "keycaps" don't shout
            keycap_bg: Color::Rgb(222, 224, 228),
            keycap_danger: Color::Rgb(178, 34, 34),
            dark: false,
            compat: false,
        }
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
            format!(" {} ", label.into()),
            Style::new().fg(self.keycap_fg).bg(self.keycap_bg),
        )
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
                format!(" {key} "),
                Style::new().fg(self.keycap_danger).bg(self.keycap_bg),
            ),
            Span::styled(format!(" {desc}"), self.muted_style()),
        ]
    }
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
