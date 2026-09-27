use super::*;
use crate::shared::i18n::{Lang, locale};

fn en() -> &'static Locale {
    locale(Lang::En)
}

/// A themes directory with a theme that is as the floors want it, one that
/// is not, and a file that is no theme at all.
fn themes_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, content: &str| std::fs::write(dir.path().join(name), content).unwrap();
    write("sepia.json", r##"{"canvas": "#f4ecd8"}"##);
    write(
        "solarized.json",
        r##"{"canvas": "#002b36", "text": "#93a1a1", "accnt": "#b58900", "muted": "grey"}"##,
    );
    write("broken.json", "{");
    write("readme.txt", "not asked to be a theme");
    dir
}

#[test]
fn a_built_in_theme_is_exported_as_a_template_and_a_users_as_fitted() {
    let dir = themes_dir();
    for name in FULL_THEMES {
        let text = export(name, dir.path(), en()).unwrap();
        let back = user_theme::parse("copy", &text).unwrap();
        assert_eq!(back.palette, Palette::built_in(name).unwrap(), "{name}");
    }
    // A theme of one colour comes out whole: what was fitted is written down,
    // and the file can be edited from there.
    let text = export("sepia", dir.path(), en()).unwrap();
    let (registry, _) = Registry::load(dir.path());
    let back = user_theme::parse("copy", &text).unwrap();
    assert_eq!(back.palette, registry.get("sepia").unwrap().palette);
    assert!(text.contains("\"canvas\": \"#f4ecd8\""), "{text}");
    assert!(text.contains("\"muted\": \"#"), "{text}");
}

#[test]
fn a_theme_there_is_none_of_names_the_ones_there_are() {
    let dir = themes_dir();
    for lang in [Lang::En, Lang::Ru] {
        let error = export("gruvbox", dir.path(), locale(lang)).unwrap_err();
        let ThemesError::Unknown(text) = &error else {
            panic!("{error:?}")
        };
        assert!(text.contains("gruvbox"), "{lang:?}: {text}");
        assert!(
            text.contains("dark, light, sepia, solarized"),
            "{lang:?}: the built-in ones first, then the user's: {text}"
        );
        assert!(!text.contains("broken"), "{lang:?}: not a theme: {text}");
        assert!(!text.contains('{'), "{lang:?}: {text}");
    }
    // The same answer from `check`, which looks for the name as a file.
    let error = check(Some("gruvbox"), dir.path(), en()).err().unwrap();
    assert!(matches!(error, ThemesError::Unknown(_)), "{error:?}");
}

#[test]
fn a_name_is_a_theme_and_a_path_is_a_file() {
    let dir = themes_dir();
    let inside = |name: &str| Target::File(dir.path().join(name));
    assert_eq!(target("dark", dir.path()), Target::BuiltIn("dark"));
    assert_eq!(target("light", dir.path()), Target::BuiltIn("light"));
    assert_eq!(target("sepia", dir.path()), inside("sepia.json"));
    assert_eq!(target("not-there", dir.path()), inside("not-there.json"));
    // Anything that looks like a path is that path, wherever it points.
    for path in [
        "sepia.json",
        "./sepia",
        "drafts/sepia",
        "drafts\\sepia.json",
    ] {
        assert_eq!(
            target(path, dir.path()),
            Target::File(PathBuf::from(path)),
            "{path}"
        );
    }
    // A theme being written somewhere else is checked where it is.
    let elsewhere = tempfile::tempdir().unwrap();
    let draft = elsewhere.path().join("draft.json");
    std::fs::write(&draft, r##"{"canvas": "#101010"}"##).unwrap();
    let checked = check(Some(draft.to_str().unwrap()), dir.path(), en()).unwrap();
    assert_eq!(checked.len(), 1);
    assert!(checked[0].clean, "{}", checked[0].text);
    assert!(
        checked[0].text.contains("Theme draft"),
        "{}",
        checked[0].text
    );
}

#[test]
fn a_report_is_a_table_of_every_role() {
    let dir = themes_dir();
    let checked = check(Some("solarized"), dir.path(), en()).unwrap();
    let (text, clean) = (&checked[0].text, checked[0].clean);
    assert!(!clean, "a named colour is below its floor");
    let rows: Vec<&str> = text.lines().collect();

    assert!(rows[0].starts_with("Theme solarized — "), "{text}");
    assert!(rows[0].ends_with("solarized.json"), "{text}");
    assert!(
        rows[1].contains("#002b36") && rows[1].contains("dark"),
        "{text}"
    );

    let header = rows.iter().find(|r| r.contains("on canvas")).unwrap();
    let row_of = |role: &str| -> Vec<&str> {
        rows.iter()
            .find(|r| r.split_whitespace().next() == Some(role))
            .unwrap_or_else(|| panic!("no row for {role} in\n{text}"))
            .split_whitespace()
            .collect()
    };
    for role in &user_theme::ROLES {
        assert!(!row_of(role.name).is_empty());
    }
    assert_eq!(
        row_of("text"),
        [
            "text", "#93a1a1", "the", "file", "5.61", "4.18", "7.0", "BELOW"
        ]
    );
    assert_eq!(row_of("muted")[2], "fitted");
    assert_eq!(*row_of("muted").last().unwrap(), "ok");
    assert_eq!(row_of("tool")[2], "built-in");
    // A role that is not text: no backdrop to be on, no floor, no verdict.
    assert_eq!(row_of("keycap_bg")[2..4], ["the", "canvas"]);
    assert_eq!(row_of("keycap_bg")[5..], ["—", "—"]);
    assert_eq!(row_of("border_focus")[4..], ["—", "3.0", "ok"]);

    // The columns are columns: a value starts where its header does.
    let column = |row: &str, word: &str| {
        let at = row.find(word).unwrap_or_else(|| panic!("{word} in {row}"));
        row[..at].chars().count()
    };
    let text_row = rows.iter().find(|r| r.contains("#93a1a1")).unwrap();
    assert_eq!(column(header, "colour"), column(text_row, "#93a1a1"));
    assert_eq!(column(header, "on canvas"), column(text_row, "5.61"));
    assert_eq!(column(header, "floor"), column(text_row, "7.0"));

    assert!(text.contains("Not used as written:"), "{text}");
    assert!(text.contains("`accnt` is not a role"), "{text}");
    assert!(text.contains("`muted` is \"grey\", not a colour"), "{text}");
    assert!(
        text.contains("Below the floor: 1 — text (the file)."),
        "{text}"
    );
}

#[test]
fn a_clean_theme_says_so_and_a_built_in_one_is_built_in() {
    let dir = themes_dir();
    let checked = check(Some("sepia"), dir.path(), en()).unwrap();
    assert!(checked[0].clean);
    assert!(
        checked[0].text.ends_with("Every colour clears its floor."),
        "{}",
        checked[0].text
    );
    assert!(!checked[0].text.contains("Not used as written"));

    for name in FULL_THEMES {
        let checked = check(Some(name), dir.path(), en()).unwrap();
        let text = &checked[0].text;
        assert!(checked[0].clean, "{text}");
        assert!(
            text.starts_with(&format!("Theme {name} — built in")),
            "{text}"
        );
        assert!(!text.contains("the file"), "it has no file: {text}");
    }
}

#[test]
fn with_no_argument_every_theme_is_checked_and_a_bad_file_is_one_of_the_answers() {
    let dir = themes_dir();
    let checked = check(None, dir.path(), en()).unwrap();
    let firsts: Vec<&str> = checked
        .iter()
        .map(|c| c.text.lines().next().unwrap())
        .collect();
    assert_eq!(checked.len(), 3, "the three .json files: {firsts:#?}");
    // In name order: broken, sepia, solarized.
    assert!(firsts[0].contains("broken.json") && firsts[0].contains("is not a theme"));
    assert!(firsts[0].contains("not JSON"), "{}", firsts[0]);
    assert!(firsts[1].starts_with("Theme sepia"));
    assert!(firsts[2].starts_with("Theme solarized"));
    assert_eq!(
        checked.iter().map(|c| c.clean).collect::<Vec<_>>(),
        [false, true, false]
    );

    // No themes is an answer too, and not a failure.
    let empty = tempfile::tempdir().unwrap();
    for dir in [empty.path().to_path_buf(), empty.path().join("themes")] {
        let checked = check(None, &dir, en()).unwrap();
        assert_eq!(checked.len(), 1);
        assert!(checked[0].clean);
        assert!(checked[0].text.starts_with("No themes in "));
    }
}

#[test]
fn a_file_that_is_not_a_theme_says_why_in_the_commands_language() {
    let dir = themes_dir();
    let write = |name: &str, content: &str| std::fs::write(dir.path().join(name), content).unwrap();
    write("blank.json", "{}");
    write("list.json", "[]");
    write("navy.json", r#"{"canvas": "navy"}"#);
    for lang in [Lang::En, Lang::Ru] {
        let loc = locale(lang);
        for (name, key, value) in [
            ("broken", "cli.themes.error.not_json", ""),
            ("blank", "cli.themes.error.no_canvas", ""),
            ("list", "cli.themes.error.not_an_object", ""),
            ("navy", "cli.themes.error.bad_canvas", "\"navy\""),
        ] {
            let error = check(Some(name), dir.path(), loc).err().unwrap();
            let ThemesError::NotATheme(text) = &error else {
                panic!("{name}: {error:?}")
            };
            assert!(text.contains(&format!("{name}.json")), "{lang:?}: {text}");
            // The reason is the locale's, up to where its argument goes.
            let reason = loc.t(key);
            let fixed = reason.split('{').next().unwrap();
            assert!(text.contains(fixed), "{lang:?} {name}: {text}");
            assert!(text.contains(value), "{lang:?} {name}: {text}");
            assert!(!text.contains('{') || name == "broken", "{lang:?}: {text}");
        }
    }
}

/// A header in another language is as wide as it looks, not as long as it is
/// in bytes — the columns hold in Russian too.
#[test]
fn the_columns_hold_in_another_language() {
    let dir = themes_dir();
    let ru = locale(Lang::Ru);
    let checked = check(Some("solarized"), dir.path(), ru).unwrap();
    let rows: Vec<&str> = checked[0].text.lines().collect();
    let header = rows
        .iter()
        .find(|r| r.contains(ru.t("cli.themes.col.on_canvas")))
        .unwrap();
    let text_row = rows.iter().find(|r| r.contains("#93a1a1")).unwrap();
    let column = |row: &str, word: &str| wrap::str_width(&row[..row.find(word).unwrap()]);
    assert_eq!(
        column(header, ru.t("cli.themes.col.colour")),
        column(text_row, "#93a1a1")
    );
    assert_eq!(
        column(header, ru.t("cli.themes.col.on_canvas")),
        column(text_row, "5.61")
    );
    assert_eq!(
        column(header, ru.t("cli.themes.col.floor")),
        column(text_row, "7.0")
    );
    assert!(text_row.ends_with(ru.t("cli.themes.below")), "{text_row}");
}
