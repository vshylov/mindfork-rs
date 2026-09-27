//! Themes of the user's own for the **full** colour mode (spec §11.6,
//! docs/history/theme-modes.md §10): `data/themes/<name>.json`.
//!
//! A theme file names its canvas and as many of the palette's roles as its
//! author cares to. The rest are **fitted**: a text role starts as the
//! built-in theme's colour — `dark` or `light`, by the canvas's polarity — and
//! is moved along OKLab lightness just far enough to clear its contrast floor
//! on the canvas and on the selection backdrop; the two roles that are not
//! text keep the built-in theme's distance from its canvas. **A colour the
//! file names is never altered**: where it falls short of its floor that is
//! reported, and it is drawn as written (the user's decision, 2026-09-27).
//!
//! What the fit did is a value, [`ThemeReport`] — the start-up log gets its
//! shortfalls, `mindfork themes check` prints it whole.
//!
//! The themes are scanned once, at start-up, into a process-wide registry
//! ([`init`]) — the shape `shared/i18n.rs` has for external locales, and for
//! the same reason: [`Palette::full`] is called from render paths, and what a
//! name means must not change under a running interface.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::Color;

use crate::shared::oklab::{self, contrast_ratio};
use crate::shared::osc11::Rgb;
use crate::shared::theme::{FULL_THEMES, Palette};

/// The floors of docs/history/theme-modes.md §4.4 — the ones the built-in palettes
/// are held to by a test, and a user theme's fitted roles by [`parse`].
pub const BODY_FLOOR: f32 = 7.0; // WCAG AAA
pub const TEXT_FLOOR: f32 = 4.5; // WCAG AA
pub const COMPONENT_FLOOR: f32 = 3.0; // WCAG 1.4.11, non-text

/// The key a theme file names its background by.
pub const CANVAS: &str = "canvas";

/// The longest name a theme may have: it goes into `settings.json`, onto a
/// command line and into a settings row.
pub const NAME_LIMIT: usize = 32;

/// What a role is, which decides what it is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Body text: [`BODY_FLOOR`], on the canvas and on the selection backdrop.
    Body,
    /// Any other text: [`TEXT_FLOOR`], on both.
    Text,
    /// Seen, not read — the focused border: [`COMPONENT_FLOOR`], on the canvas.
    Component,
    /// The selection backdrop: no floor (one at 3:1 would leave no room for
    /// 4.5:1 text on top of it), derived from the canvas when missing.
    Backdrop,
    /// The plain border: no floor, derived from the canvas when missing.
    Border,
}

impl Kind {
    /// The contrast the role is held to, if it is held to one.
    pub fn floor(self) -> Option<f32> {
        match self {
            Kind::Body => Some(BODY_FLOOR),
            Kind::Text => Some(TEXT_FLOOR),
            Kind::Component => Some(COMPONENT_FLOOR),
            Kind::Backdrop | Kind::Border => None,
        }
    }

    /// Whether the role is drawn on the selection backdrop as well as on the
    /// canvas — any row of text can be the selected one.
    fn on_selection(self) -> bool {
        matches!(self, Kind::Body | Kind::Text)
    }
}

/// One role of the palette, as a theme file knows it.
pub struct Role {
    /// The key in the file — the palette's field name.
    pub name: &'static str,
    pub kind: Kind,
    pub get: fn(&Palette) -> Color,
    set: fn(&mut Palette, Color),
}

macro_rules! role {
    ($field:ident, $kind:expr) => {
        Role {
            name: stringify!($field),
            kind: $kind,
            get: |p| p.$field,
            set: |p, c| p.$field = c,
        }
    };
}

/// Every role a theme file may name besides its canvas, in the order a
/// report and an exported file list them. The backdrop comes first because
/// the text roles are fitted against it.
pub const ROLES: [Role; 19] = [
    role!(keycap_bg, Kind::Backdrop),
    role!(border, Kind::Border),
    role!(border_focus, Kind::Component),
    role!(text, Kind::Body),
    role!(muted, Kind::Text),
    role!(user, Kind::Text),
    role!(assistant, Kind::Text),
    role!(tool, Kind::Text),
    role!(success, Kind::Text),
    role!(warning, Kind::Text),
    role!(error, Kind::Text),
    role!(accent, Kind::Text),
    role!(user_soft, Kind::Text),
    role!(assistant_soft, Kind::Text),
    role!(tool_soft, Kind::Text),
    role!(keycap_fg, Kind::Text),
    role!(keycap_danger, Kind::Text),
    role!(code_text, Kind::Text),
    role!(code_comment, Kind::Text),
];

/// Where a role's colour came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The file names it. Drawn as written, whatever its contrast.
    Named,
    /// The built-in theme's colour, which clears on this canvas as it is.
    BuiltIn,
    /// The built-in theme's colour, moved in lightness to clear.
    Fitted,
    /// The canvas, moved in lightness by the built-in role's distance from
    /// the built-in canvas.
    Derived,
}

/// One role of a theme, as it came out.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleReport {
    pub role: &'static str,
    pub color: Rgb,
    pub origin: Origin,
    pub floor: Option<f32>,
    pub on_canvas: f32,
    /// `None` for a role that is never drawn on the backdrop.
    pub on_selection: Option<f32>,
    /// `false` — below its floor on one of its grounds.
    pub clears: bool,
}

impl RoleReport {
    /// The contrast that decides: the lesser of the two, where there are two.
    pub fn least(&self) -> f32 {
        self.on_selection
            .map_or(self.on_canvas, |s| s.min(self.on_canvas))
    }
}

/// Something in a theme file that was not used as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A key that is no role's name — a typo names nothing.
    UnknownKey(String),
    /// A role whose value is not `#rrggbb`; the role was fitted instead.
    NotAColour { role: String, value: String },
}

/// What reading a theme did (docs/history/theme-modes.md §10.4).
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeReport {
    pub name: String,
    pub canvas: Rgb,
    /// The canvas's polarity — which built-in theme the missing roles
    /// started from.
    pub dark: bool,
    pub roles: Vec<RoleReport>,
    pub notes: Vec<Note>,
}

impl ThemeReport {
    /// The roles below their floor.
    pub fn shortfalls(&self) -> impl Iterator<Item = &RoleReport> {
        self.roles.iter().filter(|r| !r.clears)
    }

    /// Whether the theme is as the floors want it, and its file as it was read.
    pub fn is_clean(&self) -> bool {
        self.shortfalls().next().is_none() && self.notes.is_empty()
    }

    /// What the log is told about the theme at start-up: one line, or none
    /// for a theme with nothing to say.
    pub fn log_line(&self) -> Option<String> {
        if self.is_clean() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        for note in &self.notes {
            parts.push(match note {
                Note::UnknownKey(key) => format!("`{key}` is not a role (a typo?)"),
                Note::NotAColour { role, value } => {
                    format!("`{role}` is {value}, not a colour (#rrggbb) — fitted instead")
                }
            });
        }
        for r in self.shortfalls() {
            let floor = r.floor.unwrap_or_default();
            parts.push(match r.origin {
                Origin::Named => format!(
                    "`{}` is {:.2}:1 where the floor is {floor} — named in the file, drawn as written",
                    r.role,
                    r.least()
                ),
                _ => format!(
                    "`{}` could not be fitted: {:.2}:1 at best where the floor is {floor}",
                    r.role,
                    r.least()
                ),
            });
        }
        Some(format!("theme {}: {}", self.name, parts.join("; ")))
    }
}

/// A theme read from a file.
#[derive(Debug, Clone, PartialEq)]
pub struct UserTheme {
    pub palette: Palette,
    pub report: ThemeReport,
}

/// Why a file is not a theme.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("not JSON: {0}")]
    NotJson(String),
    #[error("not a JSON object of role: colour")]
    NotAnObject,
    #[error("no `canvas` — a theme of the full mode is its background first")]
    NoCanvas,
    #[error("`canvas` is {0}, not a colour (#rrggbb)")]
    BadCanvas(String),
}

/// `#rrggbb`, in either case.
pub fn parse_colour(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(Rgb {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// A colour as a theme file writes it.
pub fn colour_text(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)
}

fn color_of(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// The built-in palettes are absolute throughout (a test in
/// `shared/theme.rs` holds them to it), so every role of one is an `Rgb`.
fn rgb_of(color: Color) -> Rgb {
    match color {
        Color::Rgb(r, g, b) => Rgb { r, g, b },
        other => unreachable!("a built-in full theme holds {other:?}, not an absolute colour"),
    }
}

/// The built-in theme of a polarity — what a user theme's missing roles start
/// from.
fn built_in(dark: bool) -> Palette {
    Palette::built_in(if dark { "dark" } else { "light" }).expect("a built-in theme")
}

/// Whether a file's stem can be a theme's name: letters, digits, `-` and `_`,
/// [`NAME_LIMIT`] at most.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_LIMIT
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Reads a theme. `name` is the file's stem; `src` its content.
pub fn parse(name: &str, src: &str) -> Result<UserTheme, ThemeError> {
    let value: serde_json::Value =
        serde_json::from_str(src).map_err(|e| ThemeError::NotJson(e.to_string()))?;
    let file = value.as_object().ok_or(ThemeError::NotAnObject)?;

    let canvas = match file.get(CANVAS) {
        None => return Err(ThemeError::NoCanvas),
        Some(value) => value
            .as_str()
            .and_then(parse_colour)
            .ok_or_else(|| ThemeError::BadCanvas(value.to_string()))?,
    };

    let mut notes = Vec::new();
    for key in file.keys() {
        let known = key == CANVAS || ROLES.iter().any(|role| role.name == key);
        // A key that starts with `_` is the author's own: a note, a credit.
        if !known && !key.starts_with('_') {
            notes.push(Note::UnknownKey(key.clone()));
        }
    }
    let mut named = |role: &str| -> Option<Rgb> {
        let value = file.get(role)?;
        let colour = value.as_str().and_then(parse_colour);
        if colour.is_none() {
            notes.push(Note::NotAColour {
                role: role.to_string(),
                value: value.to_string(),
            });
        }
        colour
    };

    let dark = oklab::is_dark(canvas);
    let base = built_in(dark);
    let base_canvas = rgb_of(base.canvas);
    let mut palette = Palette {
        canvas: color_of(canvas),
        dark,
        ..base
    };

    let mut roles = Vec::with_capacity(ROLES.len());
    // In the table's order: the backdrop is settled before the text that is
    // fitted against it.
    let mut backdrop = canvas;
    for role in &ROLES {
        let built_in = rgb_of((role.get)(&base));
        let grounds: &[Rgb] = if role.kind.on_selection() {
            &[canvas, backdrop]
        } else {
            &[canvas]
        };
        let (color, origin) = match (named(role.name), role.kind.floor()) {
            (Some(color), _) => (color, Origin::Named),
            (None, None) => (
                oklab::shifted(canvas, oklab::lightness_offset(built_in, base_canvas)),
                Origin::Derived,
            ),
            (None, Some(floor)) => {
                let fitted = oklab::fit(built_in, grounds, floor, dark);
                let origin = if fitted.color == built_in {
                    Origin::BuiltIn
                } else {
                    Origin::Fitted
                };
                (fitted.color, origin)
            }
        };
        if role.kind == Kind::Backdrop {
            backdrop = color;
        }
        (role.set)(&mut palette, color_of(color));

        let floor = role.kind.floor();
        let on_canvas = contrast_ratio(color, canvas);
        let on_selection = role
            .kind
            .on_selection()
            .then(|| contrast_ratio(color, backdrop));
        let least = on_selection.map_or(on_canvas, |s| s.min(on_canvas));
        roles.push(RoleReport {
            role: role.name,
            color,
            origin,
            floor,
            on_canvas,
            on_selection,
            clears: floor.is_none_or(|floor| least >= floor),
        });
    }

    Ok(UserTheme {
        palette,
        report: ThemeReport {
            name: name.to_string(),
            canvas,
            dark,
            roles,
            notes,
        },
    })
}

/// What `mindfork themes export` writes for a palette: every role named, in
/// the table's order, so that the file read back is that palette exactly.
/// Written by hand rather than through a JSON map, which would sort the keys
/// and put `accent` before the canvas everything else is chosen for.
pub fn export(palette: &Palette) -> String {
    let mut out = String::from("{\n");
    out.push_str(
        "  \"_about\": \"A mindfork theme for the full colour mode. Keep the colours you \
         want to set and delete the rest: they are fitted to your canvas. `mindfork themes \
         check` shows what came out.\",\n",
    );
    out.push_str(&format!(
        "  \"{CANVAS}\": \"{}\"",
        colour_text(rgb_of(palette.canvas))
    ));
    for role in &ROLES {
        out.push_str(&format!(
            ",\n  \"{}\": \"{}\"",
            role.name,
            colour_text(rgb_of((role.get)(palette)))
        ));
    }
    out.push_str("\n}\n");
    out
}

/// The themes read from a directory.
#[derive(Debug, Default)]
pub struct Registry {
    themes: BTreeMap<String, UserTheme>,
}

impl Registry {
    /// Reads every `*.json` of `dir`. A file that is not a theme is reported
    /// and skipped — the rest still load; no directory is no themes. Returns
    /// the registry and what there is to tell the log.
    pub fn load(dir: &Path) -> (Self, Vec<String>) {
        let mut registry = Self::default();
        let mut warnings = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (registry, warnings);
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        // The directory's own order is the file system's; the log's is not.
        paths.sort();
        for path in paths {
            if path.extension().and_then(OsStr::to_str) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(OsStr::to_str) else {
                continue;
            };
            let mut skip = |why: String| {
                warnings.push(format!("theme file {}: {why}, skipping", path.display()));
            };
            if !is_valid_name(name) {
                skip(format!(
                    "a theme's name is letters, digits, `-` and `_`, {NAME_LIMIT} at most"
                ));
                continue;
            }
            if FULL_THEMES.contains(&name) {
                skip(format!(
                    "`{name}` is a built-in theme's name — give the file another"
                ));
                continue;
            }
            let src = match std::fs::read_to_string(&path) {
                Ok(src) => src,
                Err(e) => {
                    skip(format!("read failed ({e})"));
                    continue;
                }
            };
            match parse(name, &src) {
                Ok(theme) => {
                    warnings.extend(theme.report.log_line());
                    registry.themes.insert(name.to_string(), theme);
                }
                Err(e) => skip(e.to_string()),
            }
        }
        (registry, warnings)
    }

    /// A registry of the themes given.
    #[cfg(test)]
    pub(crate) fn of(themes: impl IntoIterator<Item = UserTheme>) -> Self {
        Self {
            themes: themes
                .into_iter()
                .map(|theme| (theme.report.name.clone(), theme))
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&UserTheme> {
        self.themes.get(name)
    }

    /// The themes' names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.themes.keys().map(String::as_str)
    }
}

/// The themes of this run, read by [`init`].
static REGISTRY: OnceLock<Registry> = OnceLock::new();
/// What [`registry`] answers before [`init`], and in a test binary.
static NONE: Registry = Registry {
    themes: BTreeMap::new(),
};

#[cfg(test)]
thread_local! {
    /// A registry for the test on this thread ([`with_registry`]).
    static FOR_THIS_TEST: std::cell::Cell<Option<&'static Registry>> =
        const { std::cell::Cell::new(None) };
}

/// Reads the themes of `dir` into the process-wide registry. Called once, at
/// start-up, before the first palette is built; a repeat call is ignored and
/// returns nothing. Returns what there is to tell the log.
#[must_use]
pub fn init(dir: &Path) -> Vec<String> {
    let (registry, warnings) = Registry::load(dir);
    if REGISTRY.set(registry).is_err() {
        return Vec::new();
    }
    warnings
}

/// The themes of this run: none before [`init`].
pub fn registry() -> &'static Registry {
    #[cfg(test)]
    if let Some(registry) = FOR_THIS_TEST.with(std::cell::Cell::get) {
        return registry;
    }
    REGISTRY.get().unwrap_or(&NONE)
}

/// Runs `test` with `themes` as the registry — on this thread only, so a test
/// sees the themes it brought and no other test does. The process-wide
/// registry cannot do that: it is set once, for the whole test binary.
#[cfg(test)]
pub(crate) fn with_registry<T>(themes: Registry, test: impl FnOnce() -> T) -> T {
    struct Restore(Option<&'static Registry>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FOR_THIS_TEST.with(|cell| cell.set(self.0));
        }
    }
    let leaked: &'static Registry = Box::leak(Box::new(themes));
    let _restore = Restore(FOR_THIS_TEST.with(|cell| cell.replace(Some(leaked))));
    test()
}

#[cfg(test)]
mod tests;
