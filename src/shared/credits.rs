//! App metadata for the "About" dialog (`F1`): author, links, license text, and
//! the list of third-party components with their licenses. `shared` layer (FSD):
//! the data is language-neutral (names/URLs/SPDX) or a whole document
//! ([`license_text`], [`disclaimer_text`]), so none of it goes through the
//! locale bundles — the chat screen (`screens/chat/popups.rs`) reads it directly.
//!
//! The [`COMPONENTS`] list is kept in sync with `Cargo.toml`'s direct
//! dependencies by the gate test [`tests::components_cover_direct_dependencies`]
//! — mirroring how the logo glyph table is checked against the SVG asset
//! (`widgets/logo.rs`). Licenses are cross-checked against `cargo metadata`
//! (crates' SPDX identifiers).

use std::sync::LazyLock;

use crate::shared::i18n::Lang;

/// App brand name (as in the wordmark logo and on crates.io). The binary is
/// named after it (`[[bin]]` in `Cargo.toml`, held by the gate test
/// [`tests::the_binary_is_named_after_the_brand`]); the package keeps the
/// project name `mindfork-rs` (`CARGO_PKG_NAME`). See
/// docs/research/binary-rename.md.
pub const APP_NAME: &str = "mindfork";

/// Author (matches the copyright in `LICENSE`).
pub const AUTHOR: &str = "Vladimir Shylov";
/// Project site (the domain is registered; the site is not built yet).
pub const SITE_URL: &str = "https://mindfork.io";
/// Repository (the `repository` field in `Cargo.toml`).
pub const REPO_URL: &str = "https://github.com/vshylov/mindfork-rs";
/// Crate page (the name `mindfork` is free on crates.io).
pub const CRATE_URL: &str = "https://crates.io/crates/mindfork";

/// SPDX identifier of the app's own license, for the one-line "License" row on
/// the help dialog's "About" tab (the full text is a tab of its own,
/// [`LICENSE_TEXT`]). Read from `Cargo.toml`'s `license` field rather than
/// spelled out here, so the row and the manifest cannot drift apart.
pub const LICENSE_ID: &str = env!("CARGO_PKG_LICENSE");

/// Build target of the running binary as the "About" tab shows it —
/// `"windows x86_64"`. Language-neutral, like the version and the links: OS and
/// CPU architecture come from `std::env::consts`, which is what the binary was
/// **built** for, so a report ("it does X on my machine") names the actual
/// build rather than what the user believes they downloaded.
pub fn platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The date the running binary was built (`YYYY-MM-DD`, UTC) — or `None` in a
/// debug build, where the value would be a lie.
///
/// The timestamp is compiled in by `build.rs` (`MINDFORK_BUILD_EPOCH`, Unix
/// seconds; `SOURCE_DATE_EPOCH` overrides it for reproducible builds). Cargo
/// re-runs a build script only when one of its declared `rerun-if-changed`
/// paths moves — ours are `dictionaries/`, `assets/` and `syntaxes/` — so
/// editing `src/` rebuilds the binary **without** re-running the script: a
/// development build would show the date of some unrelated day and go on
/// showing it. `debug_assertions` is the honest line between the two cases: a
/// release build comes off CI from a fresh checkout, and there the stamp is the
/// build. A missing row says nothing; a wrong date says something false.
///
/// Formatting is UTC and language-neutral, like [`platform`] and the version:
/// ISO order sorts, and a date is what a bug report needs to name a build older
/// than the version number can (two releases share `0.9.7`, they do not share a
/// day).
pub fn build_date() -> Option<&'static str> {
    static DATE: LazyLock<Option<String>> = LazyLock::new(|| {
        if cfg!(debug_assertions) {
            return None;
        }
        let secs: i64 = env!("MINDFORK_BUILD_EPOCH").parse().ok()?;
        let stamp = chrono::DateTime::from_timestamp(secs, 0)?;
        Some(stamp.format("%Y-%m-%d").to_string())
    });
    DATE.as_deref()
}

/// App license text (MIT) — from the `LICENSE` file at the repository root.
/// The authoritative text; the Russian rendering is [`LICENSE_TEXT_RU`], and
/// [`license_text`] is what picks between them.
///
/// The file is kept **byte-identical to the canonical MIT text**: the SPDX
/// identifier `MIT` in `Cargo.toml`, `packaging/nfpm.yaml` and the README badge
/// is only truthful while it is, and license scanners (GitHub's `licensee`,
/// distribution audits) match it by similarity — extra prose in this file makes
/// them report "Other". Everything the project wants to say *around* the
/// license lives in [`DISCLAIMER_TEXT`].
pub const LICENSE_TEXT: &str = include_str!("../../LICENSE");

/// The MIT license in Russian — `docs/legal/LICENSE.ru.txt`, an **unofficial
/// translation** that says so in its own first paragraph: the grant is the
/// English [`LICENSE_TEXT`], and this file is there so a `ru` user is not handed
/// a page of legal English (docs/history/legal-ru-translations.md §2).
///
/// It lives under `docs/legal/` rather than beside its original because the
/// root's `LICENSE*` namespace belongs to the scanners above — a
/// `LICENSE.ru.md` there is exactly what makes one report "Other". Plain text
/// with **no markdown**, like the file it translates: the "License" tab reflows
/// paragraphs and would print a `#` verbatim (gate test
/// [`tests::the_russian_license_is_paragraphs_only`]).
pub const LICENSE_TEXT_RU: &str = include_str!("../../docs/legal/LICENSE.ru.txt");

/// The disclaimer covering model output, third-party models/providers and the
/// software's automated actions — from `DISCLAIMER.md` at the repository root.
/// A supplement to the MIT license, deliberately a **separate file** (see
/// [`LICENSE_TEXT`]); markdown, rendered by our own renderer (ADR 0003) on the
/// help dialog's "Disclaimer" tab. The authoritative text; the Russian rendering
/// is [`DISCLAIMER_TEXT_RU`].
pub const DISCLAIMER_TEXT: &str = include_str!("../../DISCLAIMER.md");

/// The disclaimer in Russian — `docs/legal/DISCLAIMER.ru.md`, an unofficial
/// translation on the same terms as [`LICENSE_TEXT_RU`]. Markdown, written to
/// the same subset as its original, so the renderer and the installer's RTF
/// converter (`tools/wizard_rtf.py`) meet nothing new.
pub const DISCLAIMER_TEXT_RU: &str = include_str!("../../docs/legal/DISCLAIMER.ru.md");

/// The privacy policy — from `PRIVACY.md` at the repository root: what the app
/// keeps on the machine, what leaves it and under which setting, and what
/// reaches the author (nothing). Shown on the help dialog's "Legal" tab under
/// the disclaimer, and by the Windows wizard as a page of its own
/// (docs/research/code-signing.md §8.1). The authoritative text; the Russian
/// rendering is [`PRIVACY_TEXT_RU`].
pub const PRIVACY_TEXT: &str = include_str!("../../PRIVACY.md");

/// The privacy policy in Russian — `docs/legal/PRIVACY.ru.md`, an unofficial
/// translation on the same terms as [`LICENSE_TEXT_RU`] and
/// [`DISCLAIMER_TEXT_RU`].
pub const PRIVACY_TEXT_RU: &str = include_str!("../../docs/legal/PRIVACY.ru.md");

/// The license text for an interface language: Russian for [`Lang::Ru`],
/// English for everything else — an external `data/locales/<code>.json` bundle
/// included, since a user-supplied bundle cannot bring legal text we would then
/// be shipping as ours.
///
/// The mapping lives here rather than at the call site so the help dialog and
/// any later reader cannot disagree about which text is "the" one for a
/// language.
pub fn license_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => LICENSE_TEXT_RU,
        _ => LICENSE_TEXT,
    }
}

/// The disclaimer for an interface language — same rule as [`license_text`].
pub fn disclaimer_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => DISCLAIMER_TEXT_RU,
        _ => DISCLAIMER_TEXT,
    }
}

/// The privacy policy for an interface language — same rule as [`license_text`].
pub fn privacy_text(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => PRIVACY_TEXT_RU,
        _ => PRIVACY_TEXT,
    }
}

/// Third-party components — **direct** runtime dependencies (`[dependencies]` +
/// `[target.'cfg(windows)'.dependencies]`): `(name, version, SPDX license)`. Dev/
/// build dependencies (`tempfile`, `winresource`) are excluded: they are not in
/// the shipped app. Sorted by name. Names are cross-checked against
/// `Cargo.toml`, versions against `Cargo.lock` (gate tests; version = the one
/// resolved for our direct dependency).
pub const COMPONENTS: &[(&str, &str, &str)] = &[
    ("ansi-to-tui", "8.0.1", "MIT"),
    ("anyhow", "1.0.104", "MIT OR Apache-2.0"),
    ("arboard", "3.6.1", "MIT OR Apache-2.0"),
    ("async-stream", "0.3.6", "MIT"),
    ("async-trait", "0.1.92", "MIT OR Apache-2.0"),
    ("base64", "0.23.1", "MIT OR Apache-2.0"),
    ("bytemuck", "1.25.2", "Zlib OR Apache-2.0 OR MIT"),
    ("chacha20poly1305", "0.10.1", "Apache-2.0 OR MIT"),
    ("chardetng", "1.0.0", "Apache-2.0 OR MIT"),
    ("chrono", "0.4.45", "MIT OR Apache-2.0"),
    ("crossterm", "0.29.0", "MIT"),
    ("directories", "6.0.0", "MIT OR Apache-2.0"),
    (
        "encoding_rs",
        "0.8.41",
        "(Apache-2.0 OR MIT) AND BSD-3-Clause",
    ),
    ("eventsource-stream", "0.2.3", "MIT OR Apache-2.0"),
    ("flate2", "1.1.10", "MIT OR Apache-2.0"),
    ("futures-util", "0.3.34", "MIT OR Apache-2.0"),
    ("hkdf", "0.13.0", "MIT OR Apache-2.0"),
    ("ignore", "0.4.33", "MIT OR Unlicense"),
    ("image", "0.25.10", "MIT OR Apache-2.0"),
    ("mermaid-text", "0.57.0", "MIT"),
    ("pdf-extract", "0.12.0", "MIT"),
    ("percent-encoding", "2.3.2", "MIT OR Apache-2.0"),
    ("pulldown-cmark", "0.13.4", "MIT"),
    ("quick-xml", "0.42.0", "MIT"),
    ("ratatui", "0.30.2", "MIT"),
    ("regex", "1.13.1", "MIT OR Apache-2.0"),
    ("reqwest", "0.13.5", "MIT OR Apache-2.0"),
    ("rodio", "0.22.2", "MIT OR Apache-2.0"),
    ("rusqlite", "0.40.2", "MIT"),
    ("scraper", "0.27.0", "ISC"),
    ("serde", "1.0.229", "MIT OR Apache-2.0"),
    ("serde_json", "1.0.151", "MIT OR Apache-2.0"),
    ("sha2", "0.11.0", "MIT OR Apache-2.0"),
    ("similar", "3.2.0", "Apache-2.0"),
    ("single-instance", "0.3.3", "MIT"),
    ("spellbook", "0.4.2", "MPL-2.0"),
    ("sqlite-vec", "0.1.9", "MIT/Apache-2.0"),
    ("syntect", "5.3.0", "MIT"),
    ("sys-locale", "0.3.2", "MIT OR Apache-2.0"),
    ("tar", "0.4.46", "MIT OR Apache-2.0"),
    ("thiserror", "2.0.20", "MIT OR Apache-2.0"),
    ("tokio", "1.53.1", "MIT"),
    ("tokio-util", "0.7.19", "MIT"),
    ("tracing", "0.1.44", "MIT"),
    ("tracing-appender", "0.2.5", "MIT"),
    ("tracing-subscriber", "0.3.23", "MIT"),
    ("tui-scrollview", "0.6.7", "MIT OR Apache-2.0"),
    ("unicode-segmentation", "1.13.3", "MIT OR Apache-2.0"),
    ("unicode-width", "0.2.2", "MIT OR Apache-2.0"),
    ("uuid", "1.26.1", "Apache-2.0 OR MIT"),
    ("windows-sys", "0.61.2", "MIT OR Apache-2.0"),
    ("zip", "2.4.2", "MIT"),
];

/// One vendored syntax grammar, for the "Components" tab: `(language,
/// upstream repository, SPDX license)`.
pub type Grammar = (&'static str, &'static str, &'static str);

/// The manifest of vendored grammars, embedded so the dialog cannot disagree
/// with what the build actually used (`syntaxes/SOURCES.md` — the same file
/// `tools/fetch_syntaxes.py` fetches from and `build.rs` compiles).
const SYNTAX_MANIFEST: &str = include_str!("../../syntaxes/SOURCES.md");

/// Vendored syntax grammars — third-party data, not crates, so they are listed
/// separately from [`COMPONENTS`] (the Cargo gate tests are about
/// dependencies).
///
/// Parsed from the manifest rather than duplicated into a static list: the
/// table is ours, so **drift is impossible by construction** — better than a
/// gate test that merely detects it. Rows are borrowed from the embedded
/// manifest, so nothing is allocated beyond the vector.
pub static GRAMMARS: std::sync::LazyLock<Vec<Grammar>> = std::sync::LazyLock::new(|| {
    SYNTAX_MANIFEST
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|'))
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            // 7 columns; skip the header and its `---` separator row.
            let [grammar, _file, repo, _path, _commit, licence, _lic_path] = cells[..] else {
                return None;
            };
            let head_or_rule =
                grammar == "Grammar" || grammar.chars().all(|c| c == '-' || c == ':');
            (!head_or_rule).then_some((grammar, repo, licence))
        })
        .collect()
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Direct runtime dependencies from `Cargo.toml`: the `[dependencies]` and
    /// `[target.'cfg(windows)'.dependencies]` sections. Line-based parsing (in
    /// this manifest each dependency is one line), like the SVG parser in
    /// `widgets/logo.rs`.
    fn cargo_runtime_deps() -> BTreeSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let toml = std::fs::read_to_string(path).expect("Cargo.toml is present");
        const WANTED: [&str; 2] = ["[dependencies]", "[target.'cfg(windows)'.dependencies]"];
        let mut section = "";
        let mut deps = BTreeSet::new();
        for line in toml.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                section = trimmed;
                continue;
            }
            if !WANTED.contains(&section) || trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((name, _)) = trimmed.split_once('=') {
                let name = name.trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
                {
                    deps.insert(name.to_string());
                }
            }
        }
        deps
    }

    /// All versions of a crate from `Cargo.lock`: `name → {version, …}` (a crate
    /// can have several versions — e.g. `thiserror` 1/2, `windows-sys`
    /// transitively).
    fn cargo_lock_versions() -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock");
        let lock = std::fs::read_to_string(path).expect("Cargo.lock is present");
        let mut map: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
        let mut name: Option<String> = None;
        for line in lock.lines() {
            let t = line.trim();
            if t == "[[package]]" {
                name = None;
            } else if let Some(v) = t.strip_prefix("name = \"") {
                name = v.strip_suffix('"').map(str::to_string);
            } else if let Some(v) = t.strip_prefix("version = \"")
                && let Some(v) = v.strip_suffix('"')
                && let Some(n) = &name
            {
                map.entry(n.clone()).or_default().insert(v.to_string());
            }
        }
        map
    }

    /// Gate: the component list has not drifted from the manifest's dependencies.
    /// Added/removed a dependency — update [`COMPONENTS`] (and cross-check the
    /// license via `cargo metadata`), otherwise the "About" dialog would lie.
    #[test]
    fn components_cover_direct_dependencies() {
        let manifest = cargo_runtime_deps();
        let listed: BTreeSet<String> = COMPONENTS.iter().map(|(n, ..)| n.to_string()).collect();
        let missing: Vec<_> = manifest.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&manifest).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "COMPONENTS has drifted from Cargo.toml — missing from the list: {missing:?}; extra: {extra:?}"
        );
    }

    /// Gate: the binary target carries the brand name and the package keeps
    /// the project name (docs/research/binary-rename.md). `[[bin]] name` is
    /// read from the manifest the way [`cargo_runtime_deps`] reads it, so
    /// [`APP_NAME`] and the command the user actually types cannot drift
    /// apart.
    #[test]
    fn the_binary_is_named_after_the_brand() {
        assert_eq!(env!("CARGO_PKG_NAME"), "mindfork-rs");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let toml = std::fs::read_to_string(path).expect("Cargo.toml is present");
        let mut in_bin = false;
        let mut bin_name = None;
        for line in toml.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_bin = t == "[[bin]]";
                continue;
            }
            if in_bin && let Some(v) = t.strip_prefix("name = \"") {
                bin_name = v.strip_suffix('"');
                break;
            }
        }
        assert_eq!(bin_name, Some(APP_NAME), "[[bin]] name ≠ credits::APP_NAME");
    }

    /// Gate: the manifest and the vendored files agree — every row names a
    /// `.sublime-syntax` that exists and a licence text that ships with it, and
    /// every file on disk has a row. A file without a row would be compiled
    /// into the binary with **no recorded provenance or licence**, which is the
    /// one thing vendoring third-party data must never do.
    #[test]
    fn grammar_manifest_matches_the_vendored_files() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/syntaxes"));
        let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
            .expect("the syntaxes/ directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "sublime-syntax"))
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(!on_disk.is_empty(), "no vendored grammars found in {dir:?}");

        // Re-parse with the file column, which `GRAMMARS` drops.
        let listed: BTreeSet<String> = SYNTAX_MANIFEST
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('|'))
            .filter_map(|l| l.trim_matches('|').split('|').nth(1).map(str::trim))
            .filter(|f| f.ends_with(".sublime-syntax"))
            .map(str::to_string)
            .collect();
        assert_eq!(
            listed, on_disk,
            "syntaxes/SOURCES.md has drifted from the files in syntaxes/"
        );

        for (grammar, repo, licence) in GRAMMARS.iter() {
            assert!(
                !repo.is_empty() && !licence.is_empty(),
                "{grammar}: the manifest row must carry a repository and a licence"
            );
            let text = dir.join("licenses").join(format!("{grammar}.txt"));
            assert!(
                text.is_file(),
                "{grammar}: the licence text is not vendored ({text:?}) — \
                 run `python tools/fetch_syntaxes.py`"
            );
        }
    }

    /// Gate: every component's version is present in `Cargo.lock` (robust to
    /// crate duplicates: we check membership in the version set). A version bump
    /// in `Cargo.lock` fails this test — update the version (and cross-check the
    /// license) in [`COMPONENTS`].
    #[test]
    fn component_versions_match_cargo_lock() {
        let locked = cargo_lock_versions();
        for (name, version, _) in COMPONENTS {
            let versions = locked
                .get(*name)
                .unwrap_or_else(|| panic!("{name} is missing from Cargo.lock"));
            assert!(
                versions.contains(*version),
                "version {name} {version} not found in Cargo.lock: {versions:?}"
            );
        }
    }

    /// Gate: for a crate the lock file holds twice, [`COMPONENTS`] names the
    /// version *this* crate uses.
    ///
    /// The test above asks only whether the listed version is somewhere in the
    /// lock, and a transitive copy answers yes: base64 stayed listed at 0.22.1,
    /// green, for as long as anything else still pulled 0.22.1 — while this
    /// crate had moved to 0.23.1 and the dialog was naming a version the binary
    /// does not contain. Cargo writes the version into a dependency line exactly
    /// when the name is ambiguous, so that line is the authority.
    #[test]
    fn a_duplicated_crate_is_listed_at_the_version_this_crate_uses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock");
        let lock = std::fs::read_to_string(path).expect("Cargo.lock is present");
        let ours = lock
            .split("[[package]]")
            .find(|block| block.contains(concat!("name = \"", env!("CARGO_PKG_NAME"), "\"")))
            .expect("this crate is a package in its own lock file");
        let listed: std::collections::BTreeMap<&str, &str> =
            COMPONENTS.iter().map(|(n, v, _)| (*n, *v)).collect();
        for line in ours.lines() {
            // `"name version",` — a dependency Cargo had to disambiguate.
            let Some((name, version)) = line
                .trim()
                .trim_matches(|c| c == '"' || c == ',')
                .split_once(' ')
            else {
                continue;
            };
            if let Some(shown) = listed.get(name) {
                assert_eq!(
                    *shown, version,
                    "{name} is in Cargo.lock more than once and this crate uses \
                     {version}, but the About dialog names {shown}"
                );
            }
        }
    }

    /// Every entry carries a non-empty license/version, and the list is sorted
    /// by name (a deterministic order in the dialog).
    #[test]
    fn components_are_sorted_and_licensed() {
        for (name, version, license) in COMPONENTS {
            assert!(!license.is_empty(), "{name} has no license");
            assert!(!version.is_empty(), "{name} has no version");
        }
        let names: Vec<_> = COMPONENTS.iter().map(|(n, ..)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "COMPONENTS is not sorted by name");
    }

    /// The build stamp is compiled in, parses, and renders as an ISO date —
    /// checked through the raw environment value, because [`build_date`] itself
    /// is deliberately `None` under `cargo test` (a debug build, where the
    /// stamp may be older than the source; see its doc comment). Without this
    /// the whole path would be untested in the only profile the test suite
    /// runs in, and a `build.rs` that stopped emitting the variable would fail
    /// the **release** build, far from here.
    #[test]
    fn the_build_stamp_is_a_valid_iso_date() {
        let secs: i64 = env!("MINDFORK_BUILD_EPOCH")
            .parse()
            .expect("MINDFORK_BUILD_EPOCH is not a number");
        let rendered = chrono::DateTime::from_timestamp(secs, 0)
            .expect("the build stamp is not a timestamp")
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(rendered.len(), 10, "not an ISO date: {rendered}");
        assert!(
            rendered.starts_with("20") && rendered.matches('-').count() == 2,
            "not an ISO date: {rendered}"
        );
        // The profile rule itself: the row appears in a release build and is
        // absent in a debug one, and nothing else decides it.
        assert_eq!(build_date().is_some(), !cfg!(debug_assertions));
    }

    /// The license text is embedded and it is MIT (not an empty include).
    #[test]
    fn license_text_is_embedded_mit() {
        assert!(LICENSE_TEXT.contains("MIT License"));
        assert!(LICENSE_TEXT.contains(AUTHOR), "LICENSE copyright ≠ AUTHOR");
    }

    /// `LICENSE` carries the MIT text and **nothing else**. The SPDX identifier
    /// we publish (`Cargo.toml`, `packaging/nfpm.yaml`, the README badge) is
    /// only truthful while that holds, and scanners that match the file by
    /// similarity start reporting "Other" once it drifts. This catches the
    /// tempting "just append a paragraph here" edit — the paragraph belongs in
    /// `DISCLAIMER.md`.
    #[test]
    fn license_file_carries_nothing_but_the_mit_text() {
        let last = LICENSE_TEXT.trim_end().lines().last().unwrap_or("").trim();
        assert_eq!(last, "SOFTWARE.", "LICENSE has content after the MIT text");
        assert!(
            !LICENSE_TEXT.contains('#'),
            "LICENSE has markdown headings — an addendum crept in"
        );
    }

    /// The two Russian files are embedded, and each one says in its own text
    /// what it legally is: a translation, with the English original governing
    /// (docs/history/legal-ru-translations.md §2). Dropping that sentence would turn a
    /// convenience into a second, unintended contract — which is the one part
    /// of this that cannot be walked back after a release.
    #[test]
    fn the_russian_texts_declare_themselves_unofficial_translations() {
        for (name, text) in [
            ("LICENSE.ru.txt", LICENSE_TEXT_RU),
            ("DISCLAIMER.ru.md", DISCLAIMER_TEXT_RU),
        ] {
            assert!(text.len() > 500, "{name} looks empty");
            // The status line each translation carries, in its own language.
            assert!(
                text.contains("неофициальный перевод") || text.contains("Неофициальный перевод"),
                "{name} does not call itself an unofficial translation"
            );
            assert!(
                text.contains("английский"),
                "{name} does not name the English original as the governing text"
            );
        }
    }

    /// The accessors map a language to a text: `ru` gets the translation,
    /// everything else — including an external bundle, which cannot bring legal
    /// text of its own — gets the authoritative English.
    #[test]
    fn the_legal_texts_follow_the_interface_language() {
        assert_eq!(license_text(Lang::Ru), LICENSE_TEXT_RU);
        assert_eq!(disclaimer_text(Lang::Ru), DISCLAIMER_TEXT_RU);
        for lang in [Lang::En, Lang::from_code("de")] {
            assert_eq!(license_text(lang), LICENSE_TEXT);
            assert_eq!(disclaimer_text(lang), DISCLAIMER_TEXT);
        }
    }

    /// The translation follows the original **section for section**: the same
    /// sequence of heading levels and the same number of list items. A section
    /// silently missing from one language is exactly the failure a translated
    /// legal notice must not have, and it is invisible to anyone reading only
    /// the other language.
    #[test]
    fn the_russian_disclaimer_mirrors_the_originals_structure() {
        let shape = |text: &str| {
            let levels: Vec<usize> = text
                .lines()
                .filter(|l| l.starts_with('#'))
                .map(|l| l.chars().take_while(|c| *c == '#').count())
                .collect();
            let bullets = text.lines().filter(|l| l.starts_with("- ")).count();
            let rules = text.lines().filter(|l| l.trim() == "---").count();
            (levels, bullets, rules)
        };
        assert_eq!(
            shape(DISCLAIMER_TEXT),
            shape(DISCLAIMER_TEXT_RU),
            "the ru disclaimer has drifted from DISCLAIMER.md (headings/bullets/rules)"
        );
    }

    /// The Russian license file is **plain paragraphs**, like the English one:
    /// the "License" tab reflows paragraphs and renders no markdown, so a
    /// heading or a link added here would reach the screen — and the installer's
    /// wizard page — verbatim. The extension says `.txt` for this reason; this
    /// test is what holds it.
    #[test]
    fn the_russian_license_is_paragraphs_only() {
        for (i, line) in LICENSE_TEXT_RU.lines().enumerate() {
            let l = line.trim_start();
            assert!(
                !l.starts_with('#') && !l.starts_with("- ") && !l.starts_with('>'),
                "docs/legal/LICENSE.ru.txt:{}: markdown in a plain-text file",
                i + 1
            );
            assert!(
                !l.contains("]("),
                "docs/legal/LICENSE.ru.txt:{}: a markdown link would print verbatim",
                i + 1
            );
        }
        // The grant itself, and the copyright line the English file carries.
        assert!(
            LICENSE_TEXT_RU.contains(AUTHOR),
            "the ru license lost the copyright holder"
        );
        assert!(LICENSE_TEXT_RU.contains("MIT"));
    }

    /// The disclaimer is embedded, names itself a supplement to the license,
    /// and still covers the three things it exists for: generated output, the
    /// third-party models behind it, and the liability limit.
    #[test]
    fn disclaimer_text_is_embedded_and_supplements_the_license() {
        assert!(DISCLAIMER_TEXT.starts_with("# Disclaimer"));
        assert!(
            DISCLAIMER_TEXT.contains("(LICENSE)"),
            "the disclaimer does not link back to LICENSE"
        );
        for topic in ["no warranty", "model", "Limitation of liability"] {
            assert!(
                DISCLAIMER_TEXT.contains(topic),
                "the disclaimer no longer mentions {topic:?}"
            );
        }
    }

    /// Reads a repository file the way [`cargo_runtime_deps`] reads the manifest.
    fn repo_file(relative: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{relative} is present: {e}"))
    }

    /// The value of an Inno Setup `[Setup]` directive, by exact name.
    fn directive(script: &str, name: &str) -> String {
        let prefix = format!("{name}=");
        script
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("mindfork.iss has no {name} directive"))[prefix.len()..]
            .trim()
            .to_string()
    }

    /// **One product, one name**, across the two files that stamp it.
    ///
    /// `build.rs` writes the binary's VERSIONINFO — the product name from its own
    /// constant, the copyright read straight out of `LICENSE` — while
    /// `mindfork.iss` writes the same six strings for `setup.exe` and cannot read
    /// a file, so its copy of the copyright line is the one that can drift.
    ///
    /// This is a signing gate, not tidiness: SignPath enforces product name and
    /// version through a file metadata restriction, so a disagreement between the
    /// two artifacts is a *rejected signing request* during a release — found by a
    /// person who is waiting to approve it, with a tag already pushed
    /// (docs/research/code-signing.md §6.2). The Inno defaults that would fill
    /// these in are worse than absent: `VersionInfoVersion` is `0.0.0.0` unless
    /// set, and `VersionInfoProductName` inherits from `AppName`, so it is right
    /// only until a neighbouring directive moves.
    #[test]
    fn the_installer_and_the_binary_declare_the_same_product() {
        let iss = repo_file("packaging/windows/mindfork.iss");
        let build_rs = repo_file("build.rs");

        // The brand, as build.rs stamps it into the .exe.
        let product = build_rs
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("const PRODUCT_NAME: &str = \"")?
                    .split_once('"')
                    .map(|(name, _)| name.to_string())
            })
            .expect("build.rs declares PRODUCT_NAME");

        // Three files, one brand: the app's own constant, what build.rs stamps
        // into the binary, and what the installer declares.
        assert_eq!(
            product, APP_NAME,
            "build.rs and credits disagree on the brand"
        );
        assert_eq!(directive(&iss, "AppName"), product);
        assert_eq!(directive(&iss, "VersionInfoProductName"), product);
        assert_eq!(
            directive(&iss, "VersionInfoCompany"),
            directive(&iss, "AppPublisher"),
        );
        assert_eq!(
            directive(&iss, "VersionInfoDescription"),
            format!("{product} Setup"),
        );

        // The copyright line build.rs takes from LICENSE, spelled out in the .iss.
        let expected = LICENSE_TEXT
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("Copyright (c)"))
            .expect("LICENSE carries a copyright line");
        assert_eq!(
            directive(&iss, "VersionInfoCopyright"),
            expected,
            "the installer's copyright drifted from LICENSE — build.rs reads that file, \
             this one cannot"
        );

        // Both version fields come from the tag, never from Inno's 0.0.0.0 default.
        // They take `NumericVersion` rather than `AppVersion` because Windows file
        // metadata accepts digits and dots only, and a rehearsal tag carries a
        // prerelease suffix — with `{#AppVersion}` there, the first rehearsal
        // aborted the compile (docs/research/release-pipeline.md §8). The define
        // is asserted too, so the pair cannot quietly become a literal.
        for name in ["VersionInfoVersion", "VersionInfoProductVersion"] {
            assert_eq!(directive(&iss, name), "{#NumericVersion}", "{name}");
        }
        assert!(
            iss.contains("#define NumericVersion Copy(AppVersion, 1, Dash - 1)")
                && iss.contains("#define NumericVersion AppVersion"),
            "mindfork.iss must derive NumericVersion from AppVersion, both ways",
        );
    }
}
