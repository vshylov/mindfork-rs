//! `mindfork themes` — the two commands for whoever writes a theme
//! (spec §11.6, docs/theme-modes.md §10.4): `export` writes a theme out
//! whole, `check` prints what reading one did. Neither starts the TUI.
//!
//! The reading and the fitting are `shared/user_theme.rs`; here is what is
//! around them — which file a name means, and the table a report is shown as.

use std::path::{Path, PathBuf};

use crate::shared::i18n::Locale;
use crate::shared::theme::{FULL_THEMES, Palette};
use crate::shared::user_theme::{
    self, Note, Origin, Registry, RoleReport, ThemeError, ThemeReport, UserTheme, colour_text,
};
use crate::shared::wrap;

/// Why a theme could not be exported or checked.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ThemesError {
    /// No theme of that name — with the names there are.
    #[error("{0}")]
    Unknown(String),
    /// The file is there and is not a theme.
    #[error("{0}")]
    NotATheme(String),
    #[error("{0}")]
    Unreadable(String),
}

/// The content `themes export <name>` writes: a built-in theme as a
/// template, a theme of `dir` as it was fitted.
pub fn export(name: &str, dir: &Path, loc: &Locale) -> Result<String, ThemesError> {
    if let Some(palette) = Palette::built_in(name) {
        return Ok(user_theme::export(&palette));
    }
    let (registry, _) = Registry::load(dir);
    match registry.get(name) {
        Some(theme) => Ok(user_theme::export(&theme.palette)),
        None => Err(ThemesError::Unknown(unknown(name, &registry, loc))),
    }
}

fn unknown(name: &str, registry: &Registry, loc: &Locale) -> String {
    let names: Vec<&str> = FULL_THEMES
        .iter()
        .copied()
        .chain(registry.names())
        .collect();
    loc.tf(
        "cli.themes.unknown",
        &[("name", name), ("names", &names.join(", "))],
    )
}

/// What `themes check` was pointed at.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    BuiltIn(&'static str),
    File(PathBuf),
}

/// A name is a theme of `dir`, or a built-in one; anything that looks like a
/// path — a separator in it, a `.json` at its end — or that is a file, is
/// that file. So a theme can be checked where it is being written, before it
/// is put among the others.
fn target(arg: &str, dir: &Path) -> Target {
    if let Some(name) = FULL_THEMES.iter().find(|n| **n == arg) {
        return Target::BuiltIn(name);
    }
    let as_path = Path::new(arg);
    let looks_like_a_path =
        arg.contains(['/', '\\']) || as_path.extension().is_some() || as_path.is_file();
    if looks_like_a_path {
        Target::File(as_path.to_path_buf())
    } else {
        Target::File(dir.join(format!("{arg}.json")))
    }
}

/// One theme checked: what to print, and whether there was nothing to say.
pub struct Checked {
    pub text: String,
    pub clean: bool,
}

/// `themes check [<name>|<file>]`. With no argument — every theme of `dir`,
/// in name order.
pub fn check(arg: Option<&str>, dir: &Path, loc: &Locale) -> Result<Vec<Checked>, ThemesError> {
    match arg {
        Some(arg) => Ok(vec![check_one(&target(arg, dir), dir, loc)?]),
        None => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .map(|entries| entries.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            files.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("json"));
            files.sort();
            if files.is_empty() {
                return Ok(vec![Checked {
                    text: loc.tf("cli.themes.none", &[("path", &dir.display().to_string())]),
                    clean: true,
                }]);
            }
            Ok(files
                .into_iter()
                .map(|file| {
                    // A file that is not a theme is one of the answers here,
                    // not the end of the listing.
                    check_one(&Target::File(file), dir, loc).unwrap_or_else(|e| Checked {
                        text: e.to_string(),
                        clean: false,
                    })
                })
                .collect())
        }
    }
}

fn check_one(target: &Target, dir: &Path, loc: &Locale) -> Result<Checked, ThemesError> {
    let (theme, source) = match target {
        Target::BuiltIn(name) => {
            let palette = Palette::built_in(name).expect("a built-in theme");
            let mut theme = user_theme::parse(name, &user_theme::export(&palette))
                .expect("an exported theme is a theme");
            // Read from its own export, every colour comes back as "named in
            // the file" — of a theme that has no file.
            for role in &mut theme.report.roles {
                role.origin = Origin::BuiltIn;
            }
            (theme, loc.t("cli.themes.built_in").to_string())
        }
        Target::File(path) => (read(path, dir, loc)?, path.display().to_string()),
    };
    Ok(Checked {
        clean: theme.report.is_clean(),
        text: render(
            &theme.report,
            &source,
            matches!(target, Target::BuiltIn(_)),
            loc,
        ),
    })
}

fn read(path: &Path, dir: &Path, loc: &Locale) -> Result<UserTheme, ThemesError> {
    let shown = path.display().to_string();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (registry, _) = Registry::load(dir);
            return Err(ThemesError::Unknown(unknown(name, &registry, loc)));
        }
        Err(e) => {
            return Err(ThemesError::Unreadable(loc.tf(
                "cli.themes.unreadable",
                &[("path", &shown), ("error", &e.to_string())],
            )));
        }
    };
    user_theme::parse(name, &src).map_err(|e| {
        ThemesError::NotATheme(loc.tf(
            "cli.themes.not_a_theme",
            &[("path", &shown), ("error", &why_not(&e, loc))],
        ))
    })
}

/// Why a file is not a theme, in the language the command speaks. The error's
/// own text is the log's, which is English.
fn why_not(error: &ThemeError, loc: &Locale) -> String {
    match error {
        ThemeError::NotJson(detail) => loc.tf("cli.themes.error.not_json", &[("detail", detail)]),
        ThemeError::NotAnObject => loc.t("cli.themes.error.not_an_object").to_string(),
        ThemeError::NoCanvas => loc.t("cli.themes.error.no_canvas").to_string(),
        ThemeError::BadCanvas(value) => loc.tf("cli.themes.error.bad_canvas", &[("value", value)]),
    }
}

/// `text` in a column `width` wide — by what the terminal shows, which for a
/// translated header is not the number of its bytes.
fn pad(text: &str, width: usize) -> String {
    let gap = width.saturating_sub(wrap::str_width(text));
    format!("{text}{}", " ".repeat(gap))
}

fn origin_label(origin: Origin, loc: &Locale) -> &str {
    loc.t(match origin {
        Origin::Named => "cli.themes.origin.named",
        Origin::BuiltIn => "cli.themes.origin.built_in",
        Origin::Fitted => "cli.themes.origin.fitted",
        Origin::Derived => "cli.themes.origin.derived",
    })
}

fn ratio(value: Option<f32>) -> String {
    value.map_or_else(|| "—".to_string(), |v| format!("{v:.2}"))
}

fn row(r: &RoleReport, widths: &[usize; 6], loc: &Locale) -> String {
    let verdict = match (r.floor, r.clears) {
        (None, _) => "",
        (Some(_), true) => loc.t("cli.themes.clears"),
        (Some(_), false) => loc.t("cli.themes.below"),
    };
    let cells = [
        r.role.to_string(),
        colour_text(r.color),
        origin_label(r.origin, loc).to_string(),
        ratio(Some(r.on_canvas)),
        ratio(r.on_selection),
        r.floor
            .map_or_else(|| "—".to_string(), |f| format!("{f:.1}")),
    ];
    let mut line = String::from(" ");
    for (cell, width) in cells.iter().zip(widths) {
        line.push(' ');
        line.push_str(&pad(cell, *width));
        line.push(' ');
    }
    line.push_str(verdict);
    line.trim_end().to_string()
}

/// A report as `themes check` prints it. `built_in` — the theme has no file,
/// so nothing is said about what a file names.
pub fn render(report: &ThemeReport, source: &str, built_in: bool, loc: &Locale) -> String {
    let headers = [
        loc.t("cli.themes.col.role"),
        loc.t("cli.themes.col.colour"),
        loc.t("cli.themes.col.from"),
        loc.t("cli.themes.col.on_canvas"),
        loc.t("cli.themes.col.on_selection"),
        loc.t("cli.themes.col.floor"),
    ];
    let origins = [
        Origin::Named,
        Origin::BuiltIn,
        Origin::Fitted,
        Origin::Derived,
    ];
    let longest_role = report.roles.iter().map(|r| r.role.len()).max().unwrap_or(0);
    let longest_origin = origins
        .iter()
        .map(|o| wrap::str_width(origin_label(*o, loc)))
        .max()
        .unwrap_or(0);
    let content = [longest_role, 7, longest_origin, 5, 5, 3];
    let mut widths = [0usize; 6];
    for (i, width) in widths.iter_mut().enumerate() {
        *width = content[i].max(wrap::str_width(headers[i]));
    }

    let mut out = Vec::new();
    out.push(loc.tf(
        "cli.themes.title",
        &[("name", &report.name), ("source", source)],
    ));
    out.push(loc.tf(
        match (built_in, report.dark) {
            (true, _) => "cli.themes.canvas.built_in",
            (false, true) => "cli.themes.canvas.dark",
            (false, false) => "cli.themes.canvas.light",
        },
        &[("canvas", &colour_text(report.canvas))],
    ));
    out.push(String::new());
    let mut header = String::from(" ");
    for (title, width) in headers.iter().zip(&widths) {
        header.push(' ');
        header.push_str(&pad(title, *width));
        header.push(' ');
    }
    out.push(header.trim_end().to_string());
    out.extend(report.roles.iter().map(|r| row(r, &widths, loc)));

    if !report.notes.is_empty() {
        out.push(String::new());
        out.push(loc.t("cli.themes.notes").to_string());
        for note in &report.notes {
            out.push(format!(
                "  {}",
                match note {
                    Note::UnknownKey(key) => loc.tf("cli.themes.note.unknown_key", &[("key", key)]),
                    Note::NotAColour { role, value } => loc.tf(
                        "cli.themes.note.not_a_colour",
                        &[("role", role), ("value", value)]
                    ),
                }
            ));
        }
    }

    out.push(String::new());
    let below: Vec<String> = report
        .shortfalls()
        .map(|r| format!("{} ({})", r.role, origin_label(r.origin, loc)))
        .collect();
    out.push(if below.is_empty() {
        loc.t("cli.themes.all_clear").to_string()
    } else {
        loc.tf(
            "cli.themes.shortfalls",
            &[
                ("n", &below.len().to_string()),
                ("roles", &below.join(", ")),
            ],
        )
    });
    out.join("\n")
}

#[cfg(test)]
mod tests;
