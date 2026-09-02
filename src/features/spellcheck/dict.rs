//! Loads Hunspell dictionaries (`spellbook`) from the `dictionaries/` directory and
//! the personal dictionary. See spec §11.5.
//!
//! The directory is scanned for `*.aff` + `*.dic` pairs sharing a name (`en_US.aff` +
//! `en_US.dic`). Each successfully loaded pair is a separate dictionary; corrupt ones
//! are skipped with a log entry (spellcheck degrades, doesn't crash). Loading is
//! heavy (parsing `.dic`) — called from a background thread (see `main.rs`).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use spellbook::Dictionary;

use super::check::SpellChecker;

/// Loads dictionaries and the personal dictionary from `personal_path` per the interface
/// settings (spec §11.6):
/// - `enabled = false` → dictionaries aren't loaded (a disabled checker is returned);
/// - a non-empty `selected` → only pairs with a base name from the list are loaded
///   (`en_US`/`ru_RU`/…); an empty `selected` → all found ones.
///
/// Dictionaries are looked up first in `dict_dir` (the data root), then in `bundled_dir` —
/// the portable layout next to the binary, where the installer/package puts dictionaries in
/// non-portable storage mode (P1, §4.2 installers.md). A pair whose base name is already
/// loaded from `dict_dir` is **not** reloaded from `bundled_dir` (a user
/// dictionary of the same name wins). In portable mode `bundled_dir` matches
/// `dict_dir` — the second pass adds nothing (all names are already loaded).
///
/// A missing directory/files is not an error (a disabled checker). Loading is heavy
/// (parsing `.dic`) — called from a background thread (see `app/runtime.rs`).
pub fn load(
    dict_dir: &Path,
    bundled_dir: Option<&Path>,
    personal_path: &Path,
    enabled: bool,
    selected: &[String],
) -> SpellChecker {
    let mut dicts = Vec::new();
    if !enabled {
        tracing::info!("spellcheck disabled in settings — dictionaries aren't loaded");
    } else {
        // Already-loaded base names — so `bundled_dir` doesn't duplicate a dictionary
        // taken from `dict_dir` (and doesn't reload the same one in portable mode).
        let mut loaded = HashSet::new();
        load_dir(dict_dir, selected, &mut loaded, &mut dicts);
        if let Some(bundled) = bundled_dir
            && bundled != dict_dir
        {
            load_dir(bundled, selected, &mut loaded, &mut dicts);
        }
    }

    let personal = load_personal(personal_path);
    SpellChecker::new(dicts, personal, Some(personal_path.to_path_buf()))
}

/// Loads `*.aff`/`*.dic` pairs from one directory, skipping base names already
/// present in `loaded` (and adding loaded ones to it). Filters by `selected`.
fn load_dir(
    dir: &Path,
    selected: &[String],
    loaded: &mut HashSet<String>,
    dicts: &mut Vec<Dictionary>,
) {
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                load_entry(&entry.path(), selected, loaded, dicts);
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(dir = %dir.display(), "dictionary directory is missing");
        }
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "failed to read the dictionary directory");
        }
    }
}

/// Loads one directory entry when it is a selected, not-yet-loaded `.aff` with
/// a `.dic` next to it; anything else is skipped without an error.
fn load_entry(
    aff: &Path,
    selected: &[String],
    loaded: &mut HashSet<String>,
    dicts: &mut Vec<Dictionary>,
) {
    if aff.extension().and_then(OsStr::to_str) != Some("aff") {
        return;
    }
    // Filter by the selected dictionaries (by the file's base name).
    let stem = aff.file_stem().and_then(OsStr::to_str).unwrap_or_default();
    if !selected.is_empty() && !selected.iter().any(|s| s == stem) {
        return;
    }
    // The name is already loaded (from a higher-priority directory) — don't reload it.
    if loaded.contains(stem) {
        return;
    }
    let dic = aff.with_extension("dic");
    if !dic.exists() {
        return;
    }
    match load_pair(aff, &dic) {
        Ok(dict) => {
            tracing::info!(dict = %aff.display(), "dictionary loaded");
            loaded.insert(stem.to_string());
            dicts.push(dict);
        }
        Err(err) => {
            tracing::warn!(dict = %aff.display(), error = %err, "dictionary skipped");
        }
    }
}

/// Loads a single `.aff`/`.dic` pair.
fn load_pair(aff: &Path, dic: &Path) -> anyhow::Result<Dictionary> {
    let aff_text = fs::read_to_string(aff)?;
    let dic_text = fs::read_to_string(dic)?;
    Dictionary::new(&aff_text, &dic_text)
        .map_err(|e| anyhow::anyhow!("dictionary parse error: {e}"))
}

/// Reads the personal dictionary (one word per line; empty lines and `#`
/// comments are ignored). A missing file — an empty set.
pub fn load_personal(path: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            let word = line.trim();
            if !word.is_empty() && !word.starts_with('#') {
                set.insert(word.to_string());
            }
        }
    }
    set
}

/// Appends a word to the personal dictionary file (creating it if needed).
pub fn append_personal(path: &Path, word: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny English dictionary for tests (Hunspell format).
    const AFF: &str = "SET UTF-8\n";
    const DIC: &str = "3\nhello\nworld\ncat\n";

    fn write(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn load_all(dir: &Path) -> SpellChecker {
        load(dir, None, &dir.join("personal.txt"), true, &[])
    }

    #[test]
    fn loads_aff_dic_pair() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF);
        write(dir.path(), "en.dic", DIC);
        let checker = load_all(dir.path());
        assert!(checker.is_enabled());
        assert!(checker.check_word("hello"));
        assert!(!checker.check_word("zxcvb"));
    }

    #[test]
    fn missing_dir_disables_checker() {
        let dir = tempfile::tempdir().unwrap();
        let checker = load_all(&dir.path().join("nope"));
        assert!(!checker.is_enabled());
    }

    #[test]
    fn the_invitation_file_is_not_a_dictionary() {
        // `Paths::ensure_dirs` seeds a freshly created `dictionaries/` with a
        // README inviting the user to add their own pairs. It shares the
        // directory with real dictionaries, so the loader has to walk past it.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            crate::shared::paths::DICTIONARIES_README,
            "put your dictionaries here\n",
        );
        assert!(!load_all(dir.path()).is_enabled());

        write(dir.path(), "en.aff", AFF);
        write(dir.path(), "en.dic", DIC);
        let checker = load_all(dir.path());
        assert!(checker.is_enabled());
        assert!(checker.check_word("hello"));
    }

    #[test]
    fn aff_without_dic_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF); // no en.dic
        let checker = load_all(dir.path());
        assert!(!checker.is_enabled());
    }

    #[test]
    fn disabled_skips_loading() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF);
        write(dir.path(), "en.dic", DIC);
        // enabled = false → dictionaries aren't loaded, the checker is off.
        let checker = load(
            dir.path(),
            None,
            &dir.path().join("personal.txt"),
            false,
            &[],
        );
        assert!(!checker.is_enabled());
    }

    #[test]
    fn selection_filters_dictionaries() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en_US.aff", AFF);
        write(dir.path(), "en_US.dic", DIC);
        write(dir.path(), "ru_RU.aff", AFF);
        write(dir.path(), "ru_RU.dic", "1\nпривет\n");
        // Only en_US is selected → ru_RU isn't loaded.
        let checker = load(
            dir.path(),
            None,
            &dir.path().join("personal.txt"),
            true,
            &["en_US".to_string()],
        );
        assert!(checker.check_word("hello")); // from en_US
        assert!(!checker.check_word("привет")); // ru_RU isn't loaded
    }

    #[test]
    fn bundled_dir_supplies_missing_dictionaries() {
        // The data root (system mode) is empty, dictionaries sit next to the binary — take
        // them from the bundled directory (P1).
        let data = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(bundled.path(), "en.aff", AFF);
        write(bundled.path(), "en.dic", DIC);
        let checker = load(
            data.path(),
            Some(bundled.path()),
            &data.path().join("personal.txt"),
            true,
            &[],
        );
        assert!(checker.is_enabled());
        assert!(checker.check_word("hello"));
    }

    #[test]
    fn data_dir_dictionary_wins_over_bundled() {
        // A same-named dictionary exists both in the data root and next to the binary — the
        // data-root one is used (a user dictionary outranks a bundled one); no duplicate.
        let data = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(data.path(), "en.aff", AFF);
        write(data.path(), "en.dic", "1\nhello\n"); // only hello
        write(bundled.path(), "en.aff", AFF);
        write(bundled.path(), "en.dic", DIC); // hello/world/cat
        let checker = load(
            data.path(),
            Some(bundled.path()),
            &data.path().join("personal.txt"),
            true,
            &[],
        );
        assert!(checker.check_word("hello"));
        // "world" only exists in the bundled version — since the root's dictionary won, it's absent.
        assert!(!checker.check_word("world"));
    }

    /// The **shipped** dictionaries load and answer, read from the repository's
    /// own `dictionaries/` rather than from a fixture.
    ///
    /// Everything else here tests the loader against files this test wrote, so
    /// nothing notices when the *data* changes: a dictionary swapped for a
    /// different upstream, a variant, or a file with a BOM the parser dislikes
    /// would sail through a green suite and surface as "spellcheck went quiet"
    /// on someone's machine. That is not hypothetical — `en_GB` was replaced
    /// wholesale (dictionaries/SOURCES.md), and the British word list comes in
    /// three variants of which only one accepts both `-ise` and `-ize`.
    ///
    /// The markers are chosen to fail loudly on the plausible mistakes: a
    /// missing pair (nothing loads), the wrong English variant (`organize`
    /// against `organise`), a language mixed up (`colour` vs `color`), and a
    /// `.dic` that parsed but produced nothing.
    #[test]
    fn the_bundled_dictionaries_load_and_answer() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dictionaries");
        let personal = dir.join("does-not-exist.txt");

        for (name, present, absent) in [
            (
                "en_US",
                ["color", "organize", "neighbor"],
                ["colour", "neighbour"],
            ),
            (
                "en_GB",
                ["colour", "organise", "organize"],
                ["color", "neighbor"],
            ),
            (
                "ru_RU",
                ["словарь", "проверка", "терминал"],
                ["словарьь", "проверкаа"],
            ),
        ] {
            let checker = load(&dir, None, &personal, true, &[name.to_string()]);
            for word in present {
                assert!(
                    checker.check_word(word),
                    "{name}: {word:?} should be a word"
                );
            }
            for word in absent {
                assert!(
                    !checker.check_word(word),
                    "{name}: {word:?} should not be a word"
                );
            }
        }
    }

    #[test]
    fn personal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.txt");
        fs::write(&path, "# комментарий\nмойтермин\n\n").unwrap();
        let set = load_personal(&path);
        assert!(set.contains("мойтермин"));
        assert_eq!(set.len(), 1);

        append_personal(&path, "ещёслово").unwrap();
        let set2 = load_personal(&path);
        assert!(set2.contains("ещёслово"));
        assert!(set2.contains("мойтермин"));
    }
}
