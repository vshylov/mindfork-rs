//! App metadata for the "About" dialog (`F1`): author, links, license text, and
//! the list of third-party components with their licenses. `shared` layer (FSD):
//! the data is language-neutral (names/URLs/SPDX/legal text), so it does not go
//! through locales — the chat screen (`screens/chat/popups.rs`) reads it directly.
//!
//! The [`COMPONENTS`] list is kept in sync with `Cargo.toml`'s direct
//! dependencies by the gate test [`tests::components_cover_direct_dependencies`]
//! — mirroring how the logo glyph table is checked against the SVG asset
//! (`widgets/logo.rs`). Licenses are cross-checked against `cargo metadata`
//! (crates' SPDX identifiers).

/// App brand name (as in the wordmark logo and on crates.io). The package/binary
/// is `mindfork-rs` (`CARGO_PKG_NAME`), but the user sees the short "mindfork".
pub const APP_NAME: &str = "mindfork";

/// Author (matches the copyright in `LICENSE`).
pub const AUTHOR: &str = "Vladimir Shylov";
/// Project site (the domain is registered; the site is not built yet).
pub const SITE_URL: &str = "https://mindfork.io";
/// Repository (the `repository` field in `Cargo.toml`).
pub const REPO_URL: &str = "https://github.com/vshylov/mindfork-rs";
/// Crate page (the name `mindfork` is free on crates.io).
pub const CRATE_URL: &str = "https://crates.io/crates/mindfork";

/// App license text (MIT) — from the `LICENSE` file at the repository root.
/// Legal text in English, language-neutral — not localized.
///
/// The file is kept **byte-identical to the canonical MIT text**: the SPDX
/// identifier `MIT` in `Cargo.toml`, `packaging/nfpm.yaml` and the README badge
/// is only truthful while it is, and license scanners (GitHub's `licensee`,
/// distribution audits) match it by similarity — extra prose in this file makes
/// them report "Other". Everything the project wants to say *around* the
/// license lives in [`DISCLAIMER_TEXT`].
pub const LICENSE_TEXT: &str = include_str!("../../LICENSE");

/// The disclaimer covering model output, third-party models/providers and the
/// software's automated actions — from `DISCLAIMER.md` at the repository root.
/// A supplement to the MIT license, deliberately a **separate file** (see
/// [`LICENSE_TEXT`]); markdown, rendered by our own renderer (ADR 0003) on the
/// help dialog's "Disclaimer" tab. Legal text in English — not localized, same
/// as the license.
pub const DISCLAIMER_TEXT: &str = include_str!("../../DISCLAIMER.md");

/// Third-party components — **direct** runtime dependencies (`[dependencies]` +
/// `[target.'cfg(windows)'.dependencies]`): `(name, version, SPDX license)`. Dev/
/// build dependencies (`tempfile`, `winresource`) are excluded: they are not in
/// the shipped app. Sorted by name. Names are cross-checked against
/// `Cargo.toml`, versions against `Cargo.lock` (gate tests; version = the one
/// resolved for our direct dependency).
pub const COMPONENTS: &[(&str, &str, &str)] = &[
    ("ansi-to-tui", "8.0.1", "MIT"),
    ("anyhow", "1.0.103", "MIT OR Apache-2.0"),
    ("arboard", "3.6.1", "MIT OR Apache-2.0"),
    ("async-stream", "0.3.6", "MIT"),
    ("async-trait", "0.1.89", "MIT OR Apache-2.0"),
    ("base64", "0.22.1", "MIT OR Apache-2.0"),
    ("bytemuck", "1.25.0", "Zlib OR Apache-2.0 OR MIT"),
    ("chacha20poly1305", "0.10.1", "Apache-2.0 OR MIT"),
    ("chrono", "0.4.45", "MIT OR Apache-2.0"),
    ("crossterm", "0.29.0", "MIT"),
    ("directories", "6.0.0", "MIT OR Apache-2.0"),
    ("eventsource-stream", "0.2.3", "MIT OR Apache-2.0"),
    ("flate2", "1.1.9", "MIT OR Apache-2.0"),
    ("futures-util", "0.3.32", "MIT OR Apache-2.0"),
    ("hkdf", "0.12.4", "MIT OR Apache-2.0"),
    ("image", "0.25.10", "MIT OR Apache-2.0"),
    ("mermaid-text", "0.57.0", "MIT"),
    ("pdf-extract", "0.12.0", "MIT"),
    ("percent-encoding", "2.3.2", "MIT OR Apache-2.0"),
    ("pulldown-cmark", "0.13.4", "MIT"),
    ("quick-xml", "0.39.4", "MIT"),
    ("ratatui", "0.30.1", "MIT"),
    ("reqwest", "0.13.4", "MIT OR Apache-2.0"),
    ("rodio", "0.22.2", "MIT OR Apache-2.0"),
    ("rusqlite", "0.40.1", "MIT"),
    ("scraper", "0.27.0", "ISC"),
    ("serde", "1.0.228", "MIT OR Apache-2.0"),
    ("serde_json", "1.0.150", "MIT OR Apache-2.0"),
    ("sha2", "0.10.9", "MIT OR Apache-2.0"),
    ("single-instance", "0.3.3", "MIT"),
    ("spellbook", "0.4.2", "MPL-2.0"),
    ("sqlite-vec", "0.1.9", "MIT/Apache-2.0"),
    ("syntect", "5.3.0", "MIT"),
    ("sys-locale", "0.3.2", "MIT OR Apache-2.0"),
    ("tar", "0.4.46", "MIT OR Apache-2.0"),
    ("thiserror", "2.0.18", "MIT OR Apache-2.0"),
    ("tokio", "1.52.3", "MIT"),
    ("tokio-util", "0.7.18", "MIT"),
    ("tracing", "0.1.44", "MIT"),
    ("tracing-appender", "0.2.5", "MIT"),
    ("tracing-subscriber", "0.3.23", "MIT"),
    ("tui-scrollview", "0.6.5", "MIT OR Apache-2.0"),
    ("unicode-segmentation", "1.13.3", "MIT OR Apache-2.0"),
    ("unicode-width", "0.2.2", "MIT OR Apache-2.0"),
    ("uuid", "1.23.3", "Apache-2.0 OR MIT"),
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
}
