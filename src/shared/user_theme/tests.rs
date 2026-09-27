use super::*;
use crate::shared::oklab::{is_dark, least_contrast, lightness_offset};

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// Solarized's dark base: a canvas neither built-in theme was tuned on.
const SOLARIZED: &str = "#002b36";

fn theme(src: &str) -> UserTheme {
    parse("test", src).unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn role<'a>(report: &'a ThemeReport, name: &str) -> &'a RoleReport {
    report
        .roles
        .iter()
        .find(|r| r.role == name)
        .unwrap_or_else(|| panic!("no role {name}"))
}

fn colour(palette: &Palette, name: &str) -> Rgb {
    let role = ROLES.iter().find(|r| r.name == name).unwrap();
    rgb_of((role.get)(palette))
}

#[test]
fn a_colour_is_six_hex_digits_after_a_hash() {
    assert_eq!(parse_colour("#002b36"), Some(rgb(0, 43, 54)));
    assert_eq!(parse_colour("#FFffFF"), Some(rgb(255, 255, 255)));
    for not in [
        "002b36", "#002b3", "#002b367", "#002b3g", "#fff", "", "red", "#",
    ] {
        assert_eq!(parse_colour(not), None, "{not:?}");
    }
    // What is written is what is read back.
    for c in [rgb(0, 43, 54), rgb(255, 255, 255), rgb(1, 2, 3)] {
        assert_eq!(parse_colour(&colour_text(c)), Some(c));
    }
}

/// The table is the palette's fields by their names, every one once — it is
/// what a file's keys are read by, what a report lists and what an export
/// writes, so a role missing here is a colour nobody can set.
#[test]
fn the_roles_are_the_palettes_colours_each_once() {
    let mut names: Vec<&str> = ROLES.iter().map(|r| r.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "a role listed twice");
    assert!(
        !names.contains(&CANVAS),
        "the canvas is not fitted to itself"
    );

    // Setting each role to a colour of its own changes that role and no
    // other: the getter and the setter of a row are the same field.
    let base = Palette::built_in("dark").unwrap();
    let mut palette = base;
    for (i, role) in ROLES.iter().enumerate() {
        (role.set)(&mut palette, Color::Rgb(i as u8, 200, 100));
    }
    for (i, role) in ROLES.iter().enumerate() {
        assert_eq!(
            (role.get)(&palette),
            Color::Rgb(i as u8, 200, 100),
            "{}",
            role.name
        );
    }
    // And they are every colour there is: what is left of the palette after
    // them is its canvas and its three flags.
    let rest = Palette {
        canvas: base.canvas,
        dark: base.dark,
        compat: base.compat,
        mono: base.mono,
        ..palette
    };
    assert_eq!(
        rest, palette,
        "the premise: the four are not colours set above"
    );
    assert_ne!(palette, base);
    // The backdrop is settled before the text fitted against it.
    assert_eq!(ROLES[0].kind, Kind::Backdrop);
}

/// Fork D (user's decision, 2026-09-27), the first half: a theme that names
/// only its canvas is a whole theme, and every role held to a floor clears
/// it — on a canvas neither built-in theme was tuned on.
#[test]
fn a_theme_of_its_canvas_alone_is_fitted_to_the_floors() {
    for (canvas, dark) in [
        (SOLARIZED, true),
        ("#f4ecd8", false), // sepia
        ("#000000", true),
        ("#ffffff", false),
        ("#282a36", true),  // Dracula
        ("#fdf6e3", false), // Solarized light
    ] {
        let t = theme(&format!(r#"{{"canvas": "{canvas}"}}"#));
        assert_eq!(t.report.dark, dark, "{canvas}");
        assert_eq!(t.palette.dark, dark, "{canvas}");
        assert_eq!(t.palette.canvas, color_of(parse_colour(canvas).unwrap()));
        assert!(!t.palette.mono && !t.palette.compat, "{canvas}");
        assert!(t.report.is_clean(), "{canvas}: {:?}", t.report.log_line());

        let ground = parse_colour(canvas).unwrap();
        let backdrop = colour(&t.palette, "keycap_bg");
        for r in &t.report.roles {
            assert_ne!(r.origin, Origin::Named, "{canvas}: {}", r.role);
            assert!(r.clears, "{canvas}: {}", r.role);
            // The report says what the palette holds, measured afresh.
            assert_eq!(r.color, colour(&t.palette, r.role), "{canvas}: {}", r.role);
            let kind = ROLES.iter().find(|x| x.name == r.role).unwrap().kind;
            assert_eq!(r.floor, kind.floor(), "{canvas}: {}", r.role);
            if let Some(floor) = r.floor {
                let grounds: &[Rgb] = if kind.on_selection() {
                    &[ground, backdrop]
                } else {
                    &[ground]
                };
                let got = least_contrast(r.color, grounds);
                assert!(got >= floor, "{canvas}: {} is {got:.2}", r.role);
                assert!((r.least() - got).abs() < 0.001, "{canvas}: {}", r.role);
            }
        }
        assert_eq!(role(&t.report, "text").floor, Some(BODY_FLOOR));
        assert_eq!(role(&t.report, "muted").floor, Some(TEXT_FLOOR));
        assert_eq!(role(&t.report, "border_focus").floor, Some(COMPONENT_FLOOR));
        assert_eq!(role(&t.report, "border_focus").on_selection, None);
    }
}

/// The built-in canvases are where the built-in colours already clear: a
/// theme of one of them is that built-in theme, with nothing moved.
#[test]
fn on_a_built_in_canvas_nothing_is_moved() {
    for name in FULL_THEMES {
        let base = Palette::built_in(name).unwrap();
        let canvas = colour_text(rgb_of(base.canvas));
        let t = theme(&format!(r#"{{"canvas": "{canvas}"}}"#));
        for r in &t.report.roles {
            let built_in = colour(&base, r.role);
            match r.origin {
                Origin::BuiltIn => assert_eq!(r.color, built_in, "{name}: {}", r.role),
                // Derived from the canvas by the built-in role's own distance
                // from it: the same colour, to a rounding of its hue.
                Origin::Derived => assert!(
                    contrast_ratio(r.color, built_in) < 1.03,
                    "{name}: {} is {:?}, built in {built_in:?}",
                    r.role,
                    r.color
                ),
                other => panic!("{name}: {} is {other:?}", r.role),
            }
        }
    }
}

/// Fork D, the second half: a colour the file names is never altered — and
/// where it falls short of its floor that is said, not corrected.
#[test]
fn a_named_colour_is_drawn_as_written_and_reported() {
    let t = theme(
        r##"{"canvas": "#002b36", "text": "#93a1a1", "error": "#dc322f", "tool": "#ffffff"}"##,
    );
    for (name, written) in [
        ("text", rgb(0x93, 0xa1, 0xa1)),
        ("error", rgb(0xdc, 0x32, 0x2f)),
        ("tool", rgb(255, 255, 255)),
    ] {
        let r = role(&t.report, name);
        assert_eq!(r.origin, Origin::Named, "{name}");
        assert_eq!(r.color, written, "{name}");
        assert_eq!(colour(&t.palette, name), written, "{name}");
    }
    // Solarized's body text is 5.6:1 on its base, its red 3.3:1.
    assert!(!role(&t.report, "text").clears);
    assert!(!role(&t.report, "error").clears);
    assert!(role(&t.report, "tool").clears);
    let below: Vec<&str> = t.report.shortfalls().map(|r| r.role).collect();
    assert_eq!(below, ["text", "error"]);
    assert!(!t.report.is_clean());

    let line = t.report.log_line().expect("there is something to say");
    assert!(line.starts_with("theme test: "), "{line}");
    assert!(
        line.contains("`text` is 4.18:1 where the floor is 7"),
        "{line}"
    );
    assert!(line.contains("`error`"), "{line}");
    assert!(line.contains("drawn as written"), "{line}");
    assert!(
        !line.contains("`tool`"),
        "a colour that clears is not news: {line}"
    );
}

/// A missing text role starts as the built-in theme's colour and keeps its
/// hue: it is that colour, lighter — not another one.
#[test]
fn a_fitted_colour_is_the_built_in_one_moved_in_lightness_only() {
    let t = theme(r##"{"canvas": "#002b36"}"##);
    let base = Palette::built_in("dark").unwrap();
    let fitted: Vec<&RoleReport> = t
        .report
        .roles
        .iter()
        .filter(|r| r.origin == Origin::Fitted)
        .collect();
    assert!(fitted.len() >= 3, "the premise: some are moved: {fitted:?}");
    for r in fitted {
        let (was, now) = (
            oklab::Oklab::from_rgb(colour(&base, r.role)),
            oklab::Oklab::from_rgb(r.color),
        );
        assert!(now.l > was.l, "{}: lighter, on a dark canvas", r.role);
        assert!(now.l - was.l < 0.12, "{}: and not by much", r.role);
        assert!(
            (now.a - was.a).abs() < 0.02 && (now.b - was.b).abs() < 0.02,
            "{}: the hue moved — {was:?} to {now:?}",
            r.role
        );
    }
}

/// The two roles that are not text have no floor to be moved to, and the
/// built-in theme's own would be wrong on another canvas — the dark theme's
/// backdrop is invisible on Solarized's base. They keep the built-in theme's
/// distance from its canvas instead.
#[test]
fn the_backdrop_and_the_border_keep_their_distance_from_the_canvas() {
    let canvas = parse_colour(SOLARIZED).unwrap();
    let base = Palette::built_in("dark").unwrap();
    let base_canvas = rgb_of(base.canvas);
    assert!(
        contrast_ratio(colour(&base, "keycap_bg"), canvas) < 1.1,
        "the premise: the built-in backdrop cannot be told from this canvas"
    );

    let t = theme(r##"{"canvas": "#002b36"}"##);
    for name in ["keycap_bg", "border"] {
        let r = role(&t.report, name);
        assert_eq!(r.origin, Origin::Derived, "{name}");
        assert_eq!(r.floor, None, "{name}");
        assert!(r.clears, "{name}: nothing to fall short of");
        let (want, got) = (
            lightness_offset(colour(&base, name), base_canvas),
            lightness_offset(r.color, canvas),
        );
        assert!((want - got).abs() < 0.01, "{name}: {want} and {got}");
        assert!(
            r.color.b > r.color.r,
            "{name}: in the canvas's hue: {:?}",
            r.color
        );
    }
    assert!(role(&t.report, "keycap_bg").on_canvas > 1.2);

    // A light canvas: the backdrop is the darker of the two, as built in.
    let sepia = theme(r##"{"canvas": "#f4ecd8"}"##);
    let backdrop = role(&sepia.report, "keycap_bg").color;
    assert!(lightness_offset(backdrop, parse_colour("#f4ecd8").unwrap()) < 0.0);
}

/// Every text role is fitted against the backdrop the theme ends up with —
/// a named one included, which is why it is settled first.
#[test]
fn text_is_fitted_against_the_backdrop_the_file_names() {
    let plain = theme(r##"{"canvas": "#002b36"}"##);
    let raised = theme(r##"{"canvas": "#002b36", "keycap_bg": "#2a5a68"}"##);
    assert_eq!(role(&raised.report, "keycap_bg").origin, Origin::Named);
    let backdrop = parse_colour("#2a5a68").unwrap();
    for name in ["muted", "user", "code_comment"] {
        let (on_derived, on_named) = (role(&plain.report, name), role(&raised.report, name));
        assert!(on_named.clears, "{name}");
        assert!(
            contrast_ratio(on_named.color, backdrop) >= TEXT_FLOOR,
            "{name}"
        );
        assert!(
            oklab::Oklab::from_rgb(on_named.color).l > oklab::Oklab::from_rgb(on_derived.color).l,
            "{name}: a lighter backdrop asks for lighter text"
        );
    }
}

/// A backdrop with no room on it — mid grey, where white is 3.9:1 — is the
/// author's to choose. The roles fitted against it say they could not be.
#[test]
fn a_role_that_cannot_be_fitted_says_so() {
    let t = theme(r##"{"canvas": "#101010", "keycap_bg": "#808080"}"##);
    let text = role(&t.report, "text");
    assert_ne!(text.origin, Origin::Named);
    assert!(!text.clears, "7:1 on mid grey and on near-black at once");
    assert!(text.least() < BODY_FLOOR);
    let line = t.report.log_line().unwrap();
    assert!(line.contains("`text` could not be fitted"), "{line}");
    assert!(!line.contains("drawn as written"), "{line}");
}

#[test]
fn what_a_file_gets_wrong_is_said_and_the_theme_still_loads() {
    let t = theme(
        r##"{
            "_about": "the author's own note",
            "_credit": 7,
            "canvas": "#002b36",
            "accnt": "#ffffff",
            "muted": "grey",
            "user": 12,
            "accent": "#ffffff"
        }"##,
    );
    assert_eq!(
        t.report.notes,
        [
            Note::UnknownKey("accnt".into()),
            Note::NotAColour {
                role: "muted".into(),
                value: "\"grey\"".into()
            },
            Note::NotAColour {
                role: "user".into(),
                value: "12".into()
            },
        ],
        "a key that starts with _ is the author's and is not read"
    );
    // A value that is not a colour leaves nothing to keep: the role is fitted.
    assert_eq!(role(&t.report, "muted").origin, Origin::Fitted);
    assert!(role(&t.report, "muted").clears);
    assert_eq!(role(&t.report, "accent").origin, Origin::Named);
    assert!(!t.report.is_clean(), "notes are something to say");
    let line = t.report.log_line().unwrap();
    assert!(line.contains("`accnt` is not a role"), "{line}");
    assert!(line.contains("`muted` is \"grey\", not a colour"), "{line}");
}

#[test]
fn a_file_without_a_canvas_is_not_a_theme() {
    for (src, want) in [
        ("", "not JSON"),
        ("{", "not JSON"),
        ("[1, 2]", "not a JSON object"),
        ("\"dark\"", "not a JSON object"),
        ("{}", "no `canvas`"),
        (r##"{"text": "#ffffff"}"##, "no `canvas`"),
        (r#"{"canvas": "navy"}"#, "`canvas` is \"navy\""),
        (r#"{"canvas": 7}"#, "`canvas` is 7"),
        (r##"{"canvas": "#fff"}"##, "`canvas` is \"#fff\""),
    ] {
        let error = parse("test", src).expect_err(src);
        assert!(error.to_string().contains(want), "{src:?}: {error}");
    }
}

#[test]
fn a_theme_name_is_what_a_settings_file_and_a_command_line_can_hold() {
    for ok in ["solarized", "my-theme_2", "A", &"x".repeat(NAME_LIMIT)] {
        assert!(is_valid_name(ok), "{ok:?}");
    }
    for not in [
        "",
        "my theme",
        "тема",
        "a.b",
        "a/b",
        "../up",
        &"x".repeat(NAME_LIMIT + 1),
    ] {
        assert!(!is_valid_name(not), "{not:?}");
    }
}

/// An exported theme names every role, so it is read back as exactly the
/// palette it was written from — a built-in one and a fitted one alike — and
/// in the order a person reads it: the canvas first.
#[test]
fn an_exported_theme_reads_back_as_the_palette_it_was_written_from() {
    let fitted = theme(r##"{"canvas": "#002b36", "accent": "#b58900"}"##).palette;
    let mut palettes: Vec<Palette> = FULL_THEMES
        .iter()
        .map(|name| Palette::built_in(name).unwrap())
        .collect();
    palettes.push(fitted);
    for palette in palettes {
        let text = export(&palette);
        let back = theme(&text);
        assert_eq!(back.palette, palette);
        assert!(back.report.notes.is_empty(), "{:?}", back.report.notes);
        assert!(back.report.roles.iter().all(|r| r.origin == Origin::Named));

        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value.as_object().unwrap().len(), ROLES.len() + 2);
        let at = |key: &str| text.find(&format!("\"{key}\"")).unwrap_or(usize::MAX);
        assert!(at("_about") < at(CANVAS), "{text}");
        let mut last = at(CANVAS);
        for role in &ROLES {
            assert!(last < at(role.name), "{} out of order in {text}", role.name);
            last = at(role.name);
        }
        assert!(text.ends_with("}\n"));
    }
    // The built-in themes clear their floors (a test next to them holds them
    // to it), so their exports have nothing to report.
    for name in FULL_THEMES {
        let back = theme(&export(&Palette::built_in(name).unwrap()));
        assert!(
            back.report.is_clean(),
            "{name}: {:?}",
            back.report.log_line()
        );
    }
}

// ---- the registry ----

fn write(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn the_themes_of_a_directory_are_read_by_their_file_names() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "solarized.json", r##"{"canvas": "#002b36"}"##);
    write(dir.path(), "sepia.json", r##"{"canvas": "#f4ecd8"}"##);
    write(
        dir.path(),
        "scratch.txt",
        "not a theme, and not asked to be",
    );
    std::fs::create_dir(dir.path().join("nested.json")).unwrap();

    let (registry, warnings) = Registry::load(dir.path());
    assert_eq!(registry.names().collect::<Vec<_>>(), ["sepia", "solarized"]);
    assert!(is_dark(rgb_of(
        registry.get("solarized").unwrap().palette.canvas
    )));
    assert!(!registry.get("sepia").unwrap().palette.dark);
    assert_eq!(registry.get("scratch"), None);
    assert_eq!(registry.get("dark"), None, "a built-in theme is not a file");
    // The directory named like a theme is reported; nothing else is.
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("nested.json"), "{warnings:#?}");
}

#[test]
fn a_file_that_is_not_a_theme_is_reported_and_the_rest_still_load() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "good.json", r##"{"canvas": "#002b36"}"##);
    write(dir.path(), "broken.json", "{");
    write(dir.path(), "blank.json", "{}");
    write(dir.path(), "my theme.json", r##"{"canvas": "#002b36"}"##);
    // The built-in names are taken: a file cannot make `dark` mean
    // something else.
    write(dir.path(), "dark.json", r##"{"canvas": "#ffffff"}"##);
    write(dir.path(), "light.json", r##"{"canvas": "#000000"}"##);
    // A theme that loads, with something to say.
    write(
        dir.path(),
        "low.json",
        r##"{"canvas": "#002b36", "text": "#586e75", "colour": "#ffffff"}"##,
    );

    let (registry, warnings) = Registry::load(dir.path());
    assert_eq!(registry.names().collect::<Vec<_>>(), ["good", "low"]);
    let about = |file: &str| -> &String {
        let hits: Vec<&String> = warnings.iter().filter(|w| w.contains(file)).collect();
        assert_eq!(hits.len(), 1, "{file}: {warnings:#?}");
        hits[0]
    };
    assert!(about("broken.json").contains("not JSON"));
    assert!(about("blank.json").contains("no `canvas`"));
    assert!(about("my theme.json").contains("name is letters"));
    assert!(about("dark.json").contains("built-in theme's name"));
    assert!(about("light.json").contains("built-in theme's name"));
    for skipped in ["broken", "blank", "my theme", "dark.json", "light.json"] {
        assert!(about(skipped).ends_with(", skipping"), "{skipped}");
    }
    let low = about("theme low:");
    assert!(
        low.contains("`text` is") && low.contains("`colour` is not a role"),
        "{low}"
    );
    assert!(!low.contains("skipping"), "{low}");
    assert_eq!(warnings.len(), 6, "{warnings:#?}");
}

#[test]
fn no_directory_is_no_themes() {
    let dir = tempfile::tempdir().unwrap();
    let (registry, warnings) = Registry::load(&dir.path().join("themes"));
    assert_eq!(registry.names().count(), 0);
    assert!(warnings.is_empty());
    // …and an empty one.
    let (registry, warnings) = Registry::load(dir.path());
    assert_eq!(registry.names().count(), 0);
    assert!(warnings.is_empty());
}

/// Before `init` — and in a test binary, which never calls it — there are no
/// user themes; a test brings its own, and only that test sees them.
#[test]
fn a_test_sees_the_themes_it_brought_and_no_others() {
    assert_eq!(registry().names().count(), 0);
    let mine = Registry::of([parse("mine", r##"{"canvas": "#002b36"}"##).unwrap()]);
    let seen = with_registry(mine, || {
        let names: Vec<String> = registry().names().map(str::to_string).collect();
        // Another thread is another test.
        let elsewhere = std::thread::spawn(|| registry().names().count())
            .join()
            .unwrap();
        (names, elsewhere)
    });
    assert_eq!(seen, (vec!["mine".to_string()], 0));
    assert_eq!(registry().names().count(), 0, "and it is gone afterwards");

    // A test that fails inside still gives the registry back.
    let failed = std::panic::catch_unwind(|| {
        with_registry(Registry::of([]), || panic!("a failing test"));
    });
    assert!(failed.is_err());
    assert_eq!(registry().names().count(), 0);
}
