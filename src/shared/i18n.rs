//! Multilingualism of the **agent's service scaffold** (i18n, axis A): background-
//! task prompts, the "self-model" scaffold, tool results — text read by the
//! *model*. The language is a profile property (`Profile.language`, see [docs/history/i18n.md]).
//! **Interface** language (axis B, text for the human) uses the same mechanism via
//! `ui.*` keys and is stored globally (`config.interface.language`), independent of
//! profiles.
//!
//! Its own micro-solution with no crates (precedent `calc.rs`/ADR 0003): `rust-i18n`
//! is a process-global locale (we need per-profile), `fluent` is overkill
//! (pluralization/grammar are needed by axis B, not prompts). Bundles — JSON in the
//! repository's `locales/`, **baked into** the binary via `include_str!` (no "file not
//! found" failure mode; prompts are functionality, not decoration).
//!
//! **External locales (Tier 3, [docs/history/i18n-external-locales.md]).** On startup
//! [`init`] scans `data/locales/*.json`: a `<code>.json` file (a BCP-47-like code
//! — `de`, `pt-br`, `zh-tw`) merges **on top of** the built-in bundle of the same code
//! (a partial override — only present keys are overridden), while a file with a
//! new code adds a **new language** ([`Lang::Ext`]) without a rebuild. Missing
//! keys resolve via a chain: own bundle → the fallback declared by the `_fallback`
//! meta-key (e.g. `"_fallback":"en"` for a language translated from English) →
//! reference `en` → the key itself. A broken/unreadable file or an invalid name — a
//! warning in the log + skip (graceful degradation: the built-in stays intact); the
//! content is additionally validated against the reference (unknown keys, a
//! placeholder mismatch → warn, not rejection). Exhausting the chain (the key is
//! nowhere → a slug reaches the output) is logged once per
//! key — the only signal of this defect for external locales without gate tests.
//! Exporting a bundle template — [`export_bundle`] (CLI `mindfork locales export`).
//! Without `init` (tests) — only built-in bundles, behavior unchanged.
//!
//! **Bundle format.** JSON `{"key": value}`, where the value is a string **or an
//! array of strings**. An array is joined with **one space** (`join(" ")`): long
//! prompts in code are `\`-joined one-liners, and splitting into word fragments in
//! JSON gives the same text (readable, no `\n` escapes). An actual line break is an
//! explicit `\n` inside a fragment.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The agent-scaffold language (prompts / tools / the "self-model" scaffold).
/// A profile property (`Profile.language`) and the interface language (`config.interface.language`).
///
/// The built-in `Ru`/`En` are separate variants (used as `Lang::Ru`/`Lang::En`);
/// `Ext(code)` — a language from an external `data/locales/<code>.json` file. The code
/// is interned (`&'static str`, `Box::leak`), so the type stays `Copy`; `Eq`/`Hash`
/// compare by the code's content (not by pointer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    /// Russian — the reference language (a fallback when a key is missing from another bundle).
    #[default]
    Ru,
    /// English.
    En,
    /// An external language from `data/locales/<code>.json` (the code is an interned string).
    Ext(&'static str),
}

/// The reference language: if a key is missing from the selected bundle, it's taken
/// from here, then — the key itself (a prompt shouldn't panic over a bundle typo).
///
/// **English**, since the source language is English (CLAUDE.md §Conventions). It was
/// `Ru` from the days when the sources were Russian, and with `en`/`ru` at full key
/// parity under test that was invisible for the built-ins — it decided one thing only:
/// what an **external** `data/locales/<code>.json` shows for a key it is missing, and
/// a German locale falling back to Russian helps nobody. It also decides the language
/// of the translation template `export_bundle` writes
/// (docs/research/robustness-and-defaults.md D5).
const REFERENCE: Lang = Lang::En;

/// Interner for external-language codes: a runtime `String` → `&'static str` (`Box::leak`).
/// There are finitely many languages — a bounded one-time leak (a precedent — the
/// syntect-theme cache in `markdown/code.rs`). The built-in `ru`/`en` are literals, the interner doesn't touch them.
static INTERN: LazyLock<Mutex<HashSet<&str>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn intern(code: &str) -> &'static str {
    let mut set = INTERN.lock().expect("language-code interner poisoned");
    if let Some(s) = set.get(code) {
        return s;
    }
    let leaked: &'static str = Box::leak(code.to_string().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// Already-warned `(language, key)` pairs — so exhausting the fallback logs once per
/// key, not on every frame render (`t` is on the hot path).
static WARNED_MISSING: LazyLock<Mutex<HashSet<(Lang, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Logs "a key exhausted the fallback chain → a slug reaches the output" once per key.
/// Called only for an actually-missing key (a rare path), the lock doesn't get in the
/// way of the happy-path `t`.
fn warn_missing_key_once(lang: Lang, key: &str) {
    let mut warned = WARNED_MISSING.lock().expect("warned-keys set poisoned");
    if warned.insert((lang, key.to_string())) {
        tracing::warn!(
            lang = lang.code(),
            key,
            "i18n: key missing from every bundle in the chain — the key itself (a slug) reaches the output"
        );
    }
}

impl Lang {
    /// Built-in languages — for completeness gate tests and per-locale structural
    /// tests (they check the **built-in** bundles; external ones are user content, not gated).
    /// UI selectors and functional consumers use [`Lang::all`] (the registry).
    pub const ALL: &[Lang] = &[Lang::Ru, Lang::En];

    /// Stable language code (for serde and the locale file name).
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
            Lang::Ext(c) => c,
        }
    }

    /// Language by code: built-in `ru`/`en` → the matching variant, otherwise — an
    /// external [`Lang::Ext`] with an interned code. Interning doesn't depend on the
    /// registry, so a profile with an unknown code deserializes even before [`init`] runs.
    pub fn from_code(code: &str) -> Lang {
        match code {
            "ru" => Lang::Ru,
            "en" => Lang::En,
            other => Lang::Ext(intern(other)),
        }
    }

    /// All languages known to the app: built-in + discovered external ones
    /// (deterministic order: `Ru`, `En`, then `Ext` by code). In tests (without
    /// [`init`]) = `[Ru, En]`. For UI selectors and functional consumers
    /// (e.g. recognizing a console label across all languages).
    pub fn all() -> Vec<Lang> {
        all_from(registry())
    }

    /// Human-readable label for the UI language selector: the language's name in its
    /// own spelling from **its own** bundle (the `ui.lang.name` key). For an external
    /// language with no such key — its code; the built-in ru/en have a hard fallback in
    /// case the key gets overridden to empty. Reads exactly its own bundle
    /// ([`locale_exact`], no fallback to the reference) — otherwise an external
    /// language with no `ui.lang.name` would show ru's name.
    pub fn label(self) -> &'static str {
        let fallback = match self {
            Lang::Ru => "Русский", // the language's own endonym; cyrillic-ok
            Lang::En => "English",
            Lang::Ext(code) => code,
        };
        locale_exact(self)
            .and_then(|l| l.get("ui.lang.name"))
            .filter(|s| !s.is_empty()) // an empty override shouldn't produce an empty label
            .unwrap_or(fallback)
    }

    /// The raw JSON bundle baked into the binary (`include_str!` relative to this
    /// file → `locales/` at the repo root). Only for built-in variants.
    fn bundle_src(self) -> &'static str {
        match self {
            Lang::Ru => include_str!("../../locales/ru.json"),
            Lang::En => include_str!("../../locales/en.json"),
            Lang::Ext(_) => unreachable!("an external language has no baked-in source"),
        }
    }
}

impl Serialize for Lang {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let code = String::deserialize(d)?;
        Ok(Lang::from_code(&code))
    }
}

/// Registry languages in deterministic order (extracted for testing over a local
/// map, without the global registry).
fn all_from(reg: &HashMap<Lang, &'static Locale>) -> Vec<Lang> {
    let mut v: Vec<Lang> = reg.keys().copied().collect();
    sort_langs(&mut v);
    v
}

/// Deterministic language order in the UI: `Ru`, `En`, then external ones by code.
fn sort_langs(v: &mut [Lang]) {
    v.sort_by_key(|l| match l {
        Lang::Ru => (0u8, ""),
        Lang::En => (1u8, ""),
        Lang::Ext(c) => (2u8, *c),
    });
}

/// Language from an OS locale string (e.g. `"ru-RU"`, `"en_US.UTF-8"`): takes the
/// primary subtag (up to `-`/`_`/`.`), `ru` → [`Lang::Ru`], everything else (incl.
/// `None`) → [`Lang::En`] as the international default. Factored into a pure function
/// for testability (the OS's own `get_locale` call isn't reproducible in a test).
/// External languages (Tier 3) aren't recognized here yet — the registry isn't
/// initialized yet at call time ([`Paths::resolve`] in the peek phase); extending
/// this is future work (docs/roadmap.md, "Detect language from the OS locale").
pub fn lang_for_locale(locale: Option<&str>) -> Lang {
    let primary = locale.and_then(|l| l.split(['-', '_', '.']).next());
    match primary.map(str::to_ascii_lowercase).as_deref() {
        Some("ru") => Lang::Ru,
        _ => Lang::En,
    }
}

/// Interface language from the OS locale — for a fresh install with no
/// `defaults.json`/`settings.json` (a bare portable zip, a deb/rpm package, where
/// choosing a language at install time is impossible, §4.3 docs/history/installers.md).
/// Delegates to [`lang_for_locale`]; `En` when the locale is unavailable.
pub fn detect_os_language() -> Lang {
    lang_for_locale(sys_locale::get_locale().as_deref())
}

/// The loaded bundle for one language: a flat "key → text" table.
pub struct Locale {
    lang: Lang,
    map: HashMap<String, String>,
    /// The fallback language declared by the external file's `_fallback` meta-key
    /// (e.g. a German locale translated from English sets `"_fallback": "en"` —
    /// missing keys are taken from en explicitly rather than by default). `None` for
    /// built-ins. Resolution chain: own bundle → this fallback → reference (`en`) → the key itself.
    fallback: Option<Lang>,
}

impl Locale {
    /// Builds a locale from a raw map, extracting the `_fallback` meta-key (it isn't
    /// translated and doesn't participate in gates/output). The single `Locale`
    /// construction point — so `_fallback` is handled the same way for built-in and external.
    fn from_map(lang: Lang, mut map: HashMap<String, String>) -> Locale {
        let fallback = map.remove("_fallback").map(|c| Lang::from_code(c.trim()));
        Locale {
            lang,
            map,
            fallback,
        }
    }

    /// This locale's language — for cache keys that depend on the UI language (e.g. the feed cache).
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Whether the key exists in the bundle (no fallback) — for the gate test "code
    /// only references existing keys".
    #[cfg(test)]
    pub fn has_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// A key's value. Fallback: this language → the declared `_fallback` →
    /// reference (`en`) → the key itself. Never panics — a missing key degrades
    /// gracefully instead of crashing a prompt. The routine fallback of an incomplete
    /// bundle (the key exists in the reference) stays quiet; **exhausting** the chain
    /// (the key is nowhere → a slug reaches the output) is logged once
    /// per key — the only signal of this defect for external locales without gates.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(v) = self.map.get(key) {
            return v;
        }
        // The declared fallback language (meta `_fallback`), then the reference — via direct access to
        // their maps (no recursion through `t`): one hop to each, not a call chain.
        if let Some(fb) = self.fallback
            && fb != self.lang
            && let Some(v) = locale(fb).map.get(key)
        {
            return v;
        }
        if self.lang != REFERENCE
            && self.fallback != Some(REFERENCE)
            && let Some(v) = locale(REFERENCE).map.get(key)
        {
            return v;
        }
        warn_missing_key_once(self.lang, key);
        key
    }

    /// The value of an existing key as `&'static` (for dynamic keys like
    /// `ui.tool.label.{id}`). Requires `&'static self` — both ends are static. No
    /// fallback: `None` if the key doesn't exist (the caller substitutes a Russian
    /// `&'static` fallback).
    pub fn get(&'static self, key: &str) -> Option<&'static str> {
        self.map.get(key).map(|s| s.as_str())
    }

    /// The value with named placeholders `{name}` substituted. **Single-pass**
    /// substitution: an argument's value is substituted verbatim and is NOT
    /// re-scanned, so a `{placeholder}` inside the value isn't expanded (eliminates
    /// cascading re-substitution — critical when the value contains `{…}`: page
    /// content for `fetch_url`, the `policy_core` text in `{core}`). No plural rules —
    /// wordings are number-neutral ("×{n}", "notes: {n}").
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let (out, unused) = substitute(self.t(key), args);
        // Code↔bundle drift: a passed argument that's not in the template is almost
        // always a renamed/forgotten placeholder (the bundle has `{count}`, the code
        // sends `{n}`). Compiled away in release.
        debug_assert!(
            unused.is_empty(),
            "tf(\"{key}\"): arguments not found in the template: {unused:?} — was a placeholder renamed?"
        );
        out
    }
}

/// Single-pass substitution of `{name}` from `args`. Returns the result and the
/// names of arguments not found in the template (for the debug drift check in
/// [`Locale::tf`]). An unknown/broken `{…}` is copied verbatim (graceful degradation).
fn substitute<'a>(template: &str, args: &'a [(&'a str, &'a str)]) -> (String, Vec<&'a str>) {
    let mut used = vec![false; args.len()];
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        // Placeholder name — up to the nearest `}`, with no nested `{` (otherwise it's not one).
        match after.find('}') {
            Some(close) if !after[..close].contains('{') && !after[..close].is_empty() => {
                let name = &after[..close];
                match args.iter().position(|(k, _)| *k == name) {
                    Some(idx) => {
                        out.push_str(args[idx].1);
                        used[idx] = true;
                    }
                    // A placeholder with no argument — leave it verbatim as `{name}`.
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            // A lone `{` with no pair — copy and continue.
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let unused = args
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|((k, _), _)| *k)
        .collect();
    (out, unused)
}

/// The external file's meta-key that sets the fallback language (not a translatable
/// key; extracted in [`Locale::from_map`], excluded from validation/gates/export).
const FALLBACK_META_KEY: &str = "_fallback";

/// The set of `{name}` placeholder names in a string — for the gate test and runtime
/// validation of external files (an overridden key's placeholder set must match the
/// reference, otherwise `tf` will leave a hole/ignore an argument).
fn placeholders(s: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        if let Some(j) = rest[i..].find('}') {
            out.insert(rest[i + 1..i + j].to_string());
            rest = &rest[i + j + 1..];
        } else {
            break;
        }
    }
    out
}

/// Whether a language code is valid for an external-locale file name. BCP-47-like:
/// lowercase Latin letters, digits, and hyphen subtag separators; starts with a
/// letter, doesn't end in a hyphen, no double hyphens (`de`, `pt-br`, `zh-tw`, `sr-latn`).
fn is_valid_lang_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() || *bytes.last().unwrap() == b'-' {
        return false;
    }
    bytes.windows(2).all(|w| w != b"--")
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Parses a JSON bundle into a flat "key → text" table. An array value is joined
/// with one space (see the module doc). Returns `Err` (doesn't panic) — fits both
/// built-in (wrapped in a panic) and external files (wrapped in a warning).
fn json_to_map(src: &str) -> Result<HashMap<String, String>, String> {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(src).map_err(|e| format!("JSON parse error: {e}"))?;
    let mut map = HashMap::with_capacity(raw.len());
    for (k, v) in raw {
        let text = match v {
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(a) => {
                let mut parts = Vec::with_capacity(a.len());
                for x in &a {
                    match x.as_str() {
                        Some(s) => parts.push(s),
                        None => return Err(format!("key {k}: array element is not a string")),
                    }
                }
                parts.join(" ")
            }
            other => {
                return Err(format!("key {k}: expected a string or array, got {other}"));
            }
        };
        map.insert(k, text);
    }
    Ok(map)
}

/// A built-in bundle's map. Panics only on a broken built-in — covered by the
/// completeness gate test, so unreachable at runtime.
fn builtin_map(lang: Lang) -> HashMap<String, String> {
    json_to_map(lang.bundle_src()).unwrap_or_else(|e| panic!("built-in bundle {lang:?}: {e}"))
}

/// A registry built only from built-in bundles — used when [`init`] wasn't called
/// (tests), and as the base for [`init`].
fn build_builtin_registry() -> HashMap<Lang, &'static Locale> {
    Lang::ALL
        .iter()
        .map(|&lang| {
            let loc: &'static Locale =
                Box::leak(Box::new(Locale::from_map(lang, builtin_map(lang))));
            (lang, loc)
        })
        .collect()
}

static BUILTIN: LazyLock<HashMap<Lang, &Locale>> = LazyLock::new(build_builtin_registry);
/// The full registry (built-in + external), filled by [`init`] once at startup.
static REGISTRY: OnceLock<HashMap<Lang, &Locale>> = OnceLock::new();

/// Merges external `data/locales/*.json` files into owned bundle maps: `<code>.json`
/// on top of the built-in bundle of the same code (a key-level override) or as a
/// new language. An unreadable/broken file, an invalid name → a warning + skip (the
/// built-in stays intact). No directory — a no-op (built-in only). Returns warnings
/// (problem files/keys): [`init`] is called **before** the log subscriber is set up,
/// so warnings aren't written to `tracing` here — they're returned to the caller,
/// who logs them after `logging::init` (early-exit branches — `--help` — silently
/// discard them). Informational events (a language added/keys overridden) stay as
/// `tracing::info!` (not critical if lost on an early exit).
fn overlay_external(maps: &mut HashMap<Lang, HashMap<String, String>>, dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return warnings,
    };
    // A snapshot of the reference (en) for substantive validation of external files:
    // the key set + each one's placeholders. Taken before the loop (`maps` is mutated inside it).
    let ref_placeholders: HashMap<String, std::collections::BTreeSet<String>> = maps
        .get(&REFERENCE)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), placeholders(v)))
                .collect()
        })
        .unwrap_or_default();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        // The language code is BCP-47-like (lowercase letters/digits/hyphen, starts
        // with a letter): robust to random names (`EN.json`/`readme.json`), allows `pt-br`.
        if !is_valid_lang_code(stem) {
            warnings.push(format!(
                "external locale {}: invalid name (a language code is lowercase Latin letters/digits/hyphens, starting with a letter), skipping",
                path.display()
            ));
            continue;
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!(
                    "external locale {}: read failed ({e}), skipping",
                    path.display()
                ));
                continue;
            }
        };
        let ext_map = match json_to_map(&src) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!(
                    "external locale {}: parsing failed ({e}), skipping",
                    path.display()
                ));
                continue;
            }
        };
        // Substantive validation against the reference (mirrors the gate-test parity
        // check — the only bundle category without test coverage): warn, but don't
        // discard (the user might be editing experimentally).
        warnings.extend(validate_external_map(&path, &ext_map, &ref_placeholders));
        let lang = Lang::from_code(stem);
        let count = ext_map.len();
        let is_new = !maps.contains_key(&lang);
        let target = maps.entry(lang).or_default();
        for (k, v) in ext_map {
            target.insert(k, v); // key-level override
        }
        if is_new {
            tracing::info!(
                lang = stem,
                keys = count,
                "external locale: added a new language (missing keys fall back to the ru reference)"
            );
        } else {
            tracing::info!(
                lang = stem,
                keys = count,
                "external locale: overrode keys of the built-in bundle"
            );
        }
    }
    warnings
}

/// Checks an external bundle against the reference and returns warnings (doesn't
/// discard): (1) a key absent from the reference — a likely typo: it overrides
/// nothing and is dead (for a new language too: there should be no missing keys,
/// let alone extra ones); (2) an overridden key with a `{placeholder}` set different
/// from the reference's — will break `tf` (a hole/ignored argument). The `_fallback`
/// meta-key is excluded from the check. Returns strings for the caller to log
/// (see [`overlay_external`] — warnings aren't written here, since `init` runs
/// before the log subscriber is set up).
fn validate_external_map(
    path: &Path,
    ext_map: &HashMap<String, String>,
    ref_placeholders: &HashMap<String, std::collections::BTreeSet<String>>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for (k, v) in ext_map {
        if k == FALLBACK_META_KEY {
            continue;
        }
        match ref_placeholders.get(k) {
            None => warnings.push(format!(
                "external locale {} key {k}: not in the reference (a typo? the key overrides nothing)",
                path.display()
            )),
            Some(want) => {
                let got = placeholders(v);
                if &got != want {
                    warnings.push(format!(
                        "external locale {} key {k}: placeholder set diverges from the reference (want {want:?}, got {got:?}) — will break tf substitution",
                        path.display()
                    ));
                }
            }
        }
    }
    warnings
}

/// Loads external locales from a directory and fixes the full registry. Called
/// once at startup (`main.rs`) **before** the first access to [`locale`]. A repeat
/// call is ignored. No directory/files — the registry = the built-in bundles.
///
/// Returns warnings about problem external files/keys: `init` runs **before** the
/// log subscriber is set up (`logging::init`), so a `tracing::warn!` here would be
/// lost — the caller logs them itself after log init (early-exit branches like
/// `--help` discard them). A repeat call returns an empty list.
#[must_use]
pub fn init(dir: &Path) -> Vec<String> {
    let mut maps: HashMap<Lang, HashMap<String, String>> =
        Lang::ALL.iter().map(|&l| (l, builtin_map(l))).collect();
    let warnings = overlay_external(&mut maps, dir);
    let registry: HashMap<Lang, &'static Locale> = maps
        .into_iter()
        .map(|(lang, map)| {
            let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(lang, map)));
            (lang, loc)
        })
        .collect();
    if REGISTRY.set(registry).is_err() {
        return Vec::new();
    }
    warnings
}

/// The active registry: full (after [`init`]) or built-in only (tests/before init).
fn registry() -> &'static HashMap<Lang, &'static Locale> {
    match REGISTRY.get() {
        Some(r) => r,
        None => &BUILTIN,
    }
}

/// Returns a language's bundle (`&'static` — convenient to put into task/round
/// snapshots). An unknown language (an external code with no file) → the reference
/// locale (`en`) — graceful degradation.
pub fn locale(lang: Lang) -> &'static Locale {
    let reg = registry();
    reg.get(&lang)
        .or_else(|| reg.get(&REFERENCE))
        .copied()
        .expect("the reference locale (en) is always present in the registry")
}

/// Exactly this language's bundle **without** falling back to the reference: `None`
/// if the language isn't registered (an external code with no file). Needed for
/// `Lang::label` — a fallback to ru would show a Russian name for a foreign language.
fn locale_exact(lang: Lang) -> Option<&'static Locale> {
    registry().get(&lang).copied()
}

/// Bundle content for export to a template file (CLI `mindfork locales export`).
/// Built-in `ru`/`en` — the source JSON **verbatim** (preserves arrays/formatting,
/// convenient to edit). Otherwise — the full set of reference keys with values
/// resolved for the language (JSON, keys sorted): for a registered external language
/// that's its values + ru gaps, for a new code — entirely ru (a translation template).
pub fn export_bundle(lang: Lang) -> String {
    if let Lang::Ru | Lang::En = lang {
        return lang.bundle_src().to_string();
    }
    let reference = locale(REFERENCE);
    let target = locale(lang);
    let mut keys: Vec<&String> = reference.map.keys().collect();
    keys.sort();
    let obj: serde_json::Map<String, serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            (
                k.clone(),
                serde_json::Value::String(target.t(k).to_string()),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bundles_parse() {
        // Parses every built-in bundle (catches broken JSON/wrong value types).
        for &lang in Lang::ALL {
            let _ = locale(lang);
        }
    }

    #[test]
    fn lang_for_locale_maps_primary_subtag() {
        // A Russian locale in various forms → Ru; everything else and None → En (the international default).
        assert_eq!(lang_for_locale(Some("ru")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("ru-RU")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("ru_RU.UTF-8")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("RU")), Lang::Ru);
        assert_eq!(lang_for_locale(Some("en-US")), Lang::En);
        assert_eq!(lang_for_locale(Some("de-DE")), Lang::En);
        assert_eq!(lang_for_locale(Some("")), Lang::En);
        assert_eq!(lang_for_locale(None), Lang::En);
    }

    #[test]
    fn key_sets_match_across_languages() {
        // The key sets of all bundles match — the translation neither lags nor gets ahead.
        let ref_keys: std::collections::BTreeSet<&String> = locale(REFERENCE).map.keys().collect();
        for &lang in Lang::ALL {
            if lang == REFERENCE {
                continue;
            }
            let keys: std::collections::BTreeSet<&String> = locale(lang).map.keys().collect();
            let missing: Vec<_> = ref_keys.difference(&keys).collect();
            let extra: Vec<_> = keys.difference(&ref_keys).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{lang:?}: missing {missing:?}, extra {extra:?}"
            );
        }
    }

    #[test]
    fn placeholder_sets_match_across_languages() {
        // The set of {…} placeholders for each key is identical across all languages
        // — otherwise the `tf` substitution will leave a hole or ignore an argument.
        // Uses the module's `placeholders` (the same logic backs runtime validation of external ones).
        for (key, val) in &locale(REFERENCE).map {
            let want = placeholders(val);
            for &lang in Lang::ALL {
                if lang == REFERENCE {
                    continue;
                }
                let got = placeholders(locale(lang).t(key));
                assert_eq!(want, got, "key {key}: {lang:?} placeholders diverge");
            }
        }
    }

    #[test]
    fn en_bundle_has_no_cyrillic() {
        // A direct proxy for the go criterion (docs/history/i18n.md, Tier 1): the
        // en scaffold contains no Cyrillic. Catches accidentally left-over Russian text
        // in a translation. tool names and keys are ASCII, so a clean en bundle is
        // all Latin/punctuation.
        for (key, val) in &locale(Lang::En).map {
            let cyr = val.chars().find(|&c| {
                ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё'
            });
            assert!(
                cyr.is_none(),
                "key {key}: Cyrillic in the en translation: {val:?}"
            );
        }
    }

    #[test]
    fn fallback_to_reference_then_key() {
        let en = locale(Lang::En);
        // A nonexistent key → the key itself (no panic).
        assert_eq!(en.t("no.such.key.exists"), "no.such.key.exists");
    }

    #[test]
    fn tf_substitutes_named_placeholders() {
        // On a real key with a placeholder (age "N days").
        for &lang in Lang::ALL {
            let s = locale(lang).tf("selfmodel.age.days", &[("n", "3")]);
            assert!(s.contains('3') && !s.contains("{n}"), "{lang:?}: {s}");
        }
    }

    /// Collects all string literals of the form `"<seg>.<seg>…"` (2+ segments of
    /// `[a-z0-9_]` separated by dots, entirely between quotes) from every `.rs` under
    /// `src/`. A shared scanner for the direct and reverse key gates.
    fn dotted_literals_in_src() -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        visit(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    /// The scanner half of [`dotted_literals_in_src`]: one file's text → the
    /// dotted literals in it.
    fn scan(s: &str, out: &mut std::collections::BTreeSet<String>) {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let Some(p) = s[i..].find('"') else { break };
            let start = i + p + 1; // past the opening quote; always > i (progress)
            let (j, dots) = key_run(bytes, start);
            // The literal in full (a quote follows), ≥2 segments, doesn't end in a dot.
            if j < bytes.len()
                && bytes[j] as char == '"'
                && dots >= 1
                && bytes[j - 1] as char != '.'
            {
                out.insert(s[start..j].to_string());
            }
            i = j; // j ≥ start > the previous i — the loop always makes progress
        }
    }

    /// Reads a run of key characters (`[a-z0-9_.]`) starting at `start`;
    /// returns the index just past the run and the number of dots in it.
    fn key_run(bytes: &[u8], start: usize) -> (usize, usize) {
        let mut j = start;
        let mut dots = 0usize;
        while j < bytes.len() {
            let c = bytes[j] as char;
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                j += 1;
            } else if c == '.' {
                dots += 1;
                j += 1;
            } else {
                break;
            }
        }
        (j, dots)
    }

    /// Recursively feeds every `.rs` under `dir` through [`scan`].
    fn visit(dir: &Path, out: &mut std::collections::BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                scan(&std::fs::read_to_string(&path).unwrap_or_default(), out);
            }
        }
    }

    /// Top-level prefixes (`ui`, `tool`, …) actually present in the bundle — used to
    /// tell locale keys apart from other dotted literals (paths, etc.).
    fn bundle_prefixes() -> std::collections::BTreeSet<String> {
        locale(REFERENCE)
            .map
            .keys()
            .filter_map(|k| k.split('.').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn all_bundle_key_references_in_code_exist() {
        // A gate against the bug class "code calls loc.t("tool.…"/"ui.…"), but the
        // key is missing from the bundle" (then `t` silently returns the key itself —
        // a slug in the model's prompt / the UI). Previously only `ui.*` was covered;
        // now — the WHOLE of axis A (`tool.`/`selfmodel.`/`notes.`/`prompt.`/…).
        // Dynamic keys (`format!`) aren't literals, covered separately (meta.rs).
        // File names sharing a bundle prefix are whitelisted.
        // `notes.md` — a sample attachment file name in tests, colliding with the
        // `notes.` prefix of the notes tools' keys; `setup.msi` — likewise, against
        // the `setup.` prefix of `mindfork setup`'s messages.
        const NON_KEY: &[&str] = &["defaults.json", "python.webc", "notes.md", "setup.msi"];
        let prefixes = bundle_prefixes();
        let ru = locale(Lang::Ru);
        let missing: Vec<String> = dotted_literals_in_src()
            .into_iter()
            .filter(|k| {
                prefixes.contains(k.split('.').next().unwrap()) && !NON_KEY.contains(&k.as_str())
            })
            .filter(|k| !ru.has_key(k))
            .collect();
        assert!(
            missing.is_empty(),
            "keys exist in code but are missing from the bundle: {missing:?}"
        );
    }

    #[test]
    fn bundle_keys_are_not_dead() {
        // The reverse gate: every bundle key is actually referenced in code (as a
        // literal), otherwise it's a dead key (a typo/refactor leftover). Dynamic
        // families (`ui.tool.label.<id>` — assembled via `format!`) are excluded by a whitelist prefix.
        const DYNAMIC_PREFIX: &[&str] = &["ui.tool.label."];
        let literals = dotted_literals_in_src();
        let dead: Vec<&String> = locale(REFERENCE)
            .map
            .keys()
            .filter(|k| !literals.contains(*k))
            .filter(|k| !DYNAMIC_PREFIX.iter().any(|p| k.starts_with(p)))
            .collect();
        assert!(
            dead.is_empty(),
            "bundle keys aren't used anywhere in code (dead?): {dead:?}"
        );
    }

    #[test]
    fn every_withheld_image_string_tells_the_model_not_to_describe_it() {
        // Measured, not stylistic (docs/journal/tools.md, spec §9.10): naming a
        // withholding is indistinguishable from saying nothing — on Gemma 4 31B the
        // descriptive form left the model describing a screenshot it never received
        // 5/5, exactly as silence did, and only the directive clause moved it to 0/5.
        // So the clause is the working part of these strings, and an editor tidying
        // them back into a plain statement has to fail here rather than in production.
        const FAMILY: &[&str] = &[
            "loop.images_no_vision",
            "loop.images_dropped",
            "loop.image_not_shown_no_vision",
            "loop.image_not_shown_dropped",
            "tool.mcp.images_off",
            "tool.python_exec.files.not_shown_off",
            "tool.python_exec.files.not_shown_cap",
            "tool.python_exec.files.svg",
        ];
        // One per language, because the clause is a sentence and not a token.
        for (lang, needle) in [(Lang::En, "do not describe"), (Lang::Ru, "не описывай")] {
            let loc = locale(lang);
            for key in FAMILY {
                let text = loc.t(key);
                assert!(
                    text.contains(needle),
                    "{lang:?} {key} must tell the model not to describe what it \
                     has not seen, got: {text:?}"
                );
            }
        }
    }

    // ------- External locales (Tier 3): pure tests over a local map -------
    // We do NOT touch the global registry (`init`/`REGISTRY`) in tests — otherwise
    // one test would swap the bundles for the whole test binary. We check
    // `overlay_external`/`json_to_map`/`from_code`/`sort_langs` over passed-in data.

    /// Builds owned maps of the built-in bundles (as `init` does before the overlay).
    fn builtin_maps() -> HashMap<Lang, HashMap<String, String>> {
        Lang::ALL.iter().map(|&l| (l, builtin_map(l))).collect()
    }

    #[test]
    fn json_to_map_parses_string_and_array() {
        let m = json_to_map(r#"{"a":"one","b":["two","three"]}"#).unwrap();
        assert_eq!(m["a"], "one");
        assert_eq!(m["b"], "two three"); // an array is joined with a space
    }

    #[test]
    fn json_to_map_rejects_bad_value() {
        assert!(json_to_map(r#"{"a":42}"#).is_err());
        assert!(json_to_map("{ битый").is_err());
    }

    /// Top-level keys that appear more than once in a bundle's JSON source.
    /// serde's map deserialization keeps the LAST duplicate with no error
    /// anywhere, so a key accidentally added twice silently overrides the
    /// earlier text — only the raw source still shows the collision.
    fn duplicate_keys(src: &str) -> Vec<String> {
        struct Dups;
        impl<'de> serde::de::Visitor<'de> for Dups {
            type Value = Vec<String>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut seen = HashSet::new();
                let mut dups = Vec::new();
                while let Some((k, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                    if !seen.insert(k.clone()) {
                        dups.push(k);
                    }
                }
                Ok(dups)
            }
        }
        let mut de = serde_json::Deserializer::from_str(src);
        serde::de::Deserializer::deserialize_map(&mut de, Dups).expect("bundle JSON parses")
    }

    /// Gate: no built-in bundle declares the same key twice. Found live: both
    /// occurrences of a duplicated `ui.settings.desc.model_name` were real
    /// fields' texts, and the cloud "Model" row showed the show-model-name
    /// toggle's description — no key gate could see it, because the key both
    /// exists and is used; only the duplicate itself is the defect.
    #[test]
    fn builtin_bundles_have_no_duplicate_keys() {
        // The detector must see a planted duplicate — a gate that cannot go
        // red is indistinguishable from no gate.
        assert_eq!(duplicate_keys(r#"{"a":"1","b":"2","a":"3"}"#), ["a"]);
        for &lang in Lang::ALL.iter() {
            let dups = duplicate_keys(lang.bundle_src());
            assert!(
                dups.is_empty(),
                "{lang:?}: duplicate bundle keys — the later value silently wins: {dups:?}"
            );
        }
    }

    #[test]
    fn overlay_external_overrides_builtin_per_key() {
        // A partial en.json overrides one existing key; the rest stays built-in.
        let dir = tempfile::tempdir().unwrap();
        let key = "ui.feed.role.user"; // exists in both bundles
        std::fs::write(
            dir.path().join("en.json"),
            format!("{{\"{key}\": \"OVERRIDDEN\"}}"),
        )
        .unwrap();
        let mut maps = builtin_maps();
        let before_other = maps[&Lang::En].len();
        overlay_external(&mut maps, dir.path());
        assert_eq!(maps[&Lang::En][key], "OVERRIDDEN");
        // The rest of en's keys stayed (the count didn't shrink — an override, not a replacement).
        assert_eq!(maps[&Lang::En].len(), before_other);
        // ru is untouched.
        assert_ne!(maps[&Lang::Ru][key], "OVERRIDDEN");
    }

    #[test]
    fn overlay_external_adds_new_language() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"ui.lang.name": "Deutsch", "ui.feed.role.user": "DU"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let de = Lang::Ext("de");
        assert!(maps.contains_key(&de));
        assert_eq!(maps[&de]["ui.feed.role.user"], "DU");
        // A key not included in the file is absent from the new language's map
        // (the fallback to ru happens at the `Locale::t` level, covered by
        // `fallback_to_reference_then_key`).
        assert!(!maps[&de].contains_key("ui.feed.role.assistant"));
    }

    #[test]
    fn overlay_external_skips_malformed_and_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("en.json"), "{ битый json").unwrap(); // broken
        std::fs::write(dir.path().join("DE.json"), r#"{"x":"y"}"#).unwrap(); // name not lowercase
        std::fs::write(dir.path().join("readme.txt"), "not json").unwrap(); // not .json
        let mut maps = builtin_maps();
        let en_before = maps[&Lang::En].clone();
        overlay_external(&mut maps, dir.path());
        // The broken en.json is skipped — the built-in stays intact; bad names didn't add languages.
        assert_eq!(maps[&Lang::En], en_before);
        assert_eq!(maps.len(), Lang::ALL.len());
    }

    #[test]
    fn overlay_external_missing_dir_is_noop() {
        let mut maps = builtin_maps();
        let before = maps.clone();
        overlay_external(&mut maps, Path::new("no/such/locales/dir"));
        assert_eq!(maps.len(), before.len());
    }

    #[test]
    fn sort_langs_orders_ru_en_then_ext_by_code() {
        let mut v = vec![Lang::Ext("uk"), Lang::En, Lang::Ext("de"), Lang::Ru];
        sort_langs(&mut v);
        assert_eq!(
            v,
            vec![Lang::Ru, Lang::En, Lang::Ext("de"), Lang::Ext("uk")]
        );
    }

    // ------- Single-pass substitution `tf` / `substitute` -------

    #[test]
    fn substitute_is_single_pass_no_cascade() {
        // Argument `a`'s value contains the placeholder `{b}` — it should NOT expand
        // (otherwise page content/`policy_core` containing `{…}` would trigger a cascade).
        let (out, unused) = substitute("{a}{b}", &[("a", "{b}"), ("b", "X")]);
        assert_eq!(out, "{b}X");
        assert!(unused.is_empty());
    }

    #[test]
    fn substitute_leaves_unknown_placeholder_verbatim() {
        let (out, unused) = substitute("{x} {a} {", &[("a", "1")]);
        assert_eq!(out, "{x} 1 {"); // an unknown `{x}` and a lone `{` — verbatim
        assert!(unused.is_empty());
    }

    #[test]
    fn substitute_reports_unused_args() {
        // A passed argument that's not in the template (code↔bundle drift) — in `unused`.
        let (_out, unused) = substitute("{a}", &[("a", "1"), ("z", "2")]);
        assert_eq!(unused, vec!["z"]);
    }

    #[test]
    fn tf_does_not_cascade_on_real_key() {
        // `selfmodel.maintenance_wrapper` = "(… {core})"; we substitute a value with
        // `{n}` — it shouldn't expand (no cascade), `core` is used (no unused).
        let ru = locale(Lang::Ru);
        let s = ru.tf("selfmodel.maintenance_wrapper", &[("core", "A {n} B")]);
        assert!(s.contains("A {n} B"), "{s}");
    }

    // ------- Language code / external locales -------

    #[test]
    fn is_valid_lang_code_accepts_bcp47_and_rejects_junk() {
        for ok in ["de", "en", "pt-br", "zh-tw", "sr-latn", "x9"] {
            assert!(is_valid_lang_code(ok), "should accept: {ok}");
        }
        for bad in ["", "EN", "e n", "-de", "de-", "d--e", "1de", "de.json"] {
            assert!(!is_valid_lang_code(bad), "should reject: {bad}");
        }
    }

    #[test]
    fn from_map_extracts_fallback_meta() {
        let mut m = HashMap::new();
        m.insert("_fallback".to_string(), "en".to_string());
        m.insert("k".to_string(), "v".to_string());
        let loc = Locale::from_map(Lang::Ext("de"), m);
        assert_eq!(loc.fallback, Some(Lang::En));
        assert!(!loc.map.contains_key("_fallback")); // the meta-key isn't among the translatable ones
        assert_eq!(loc.map.get("k").map(String::as_str), Some("v"));
    }

    #[test]
    fn external_fallback_meta_drives_t_before_reference() {
        // A `de` locale with `_fallback: en` and no translations: a key missing from
        // it should be taken from EN (the global BUILTIN), NOT the Russian reference.
        let mut m = HashMap::new();
        m.insert("_fallback".to_string(), "en".to_string());
        let de = Locale::from_map(Lang::Ext("de"), m);
        let key = "ui.feed.role.user"; // ru's and en's values for this key differ
        assert_eq!(de.t(key), locale(Lang::En).t(key));
        assert_ne!(de.t(key), locale(Lang::Ru).t(key));
    }

    #[test]
    fn label_filters_empty_override() {
        // An external override `"ui.lang.name": ""` shouldn't produce an empty label —
        // the filter discards the empty value (the mechanism `label` relies on).
        let mut m = HashMap::new();
        m.insert("ui.lang.name".to_string(), String::new());
        let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(Lang::Ext("de"), m)));
        assert_eq!(loc.get("ui.lang.name").filter(|s| !s.is_empty()), None);
    }

    #[test]
    fn overlay_validation_is_non_fatal() {
        // An external file with an unknown key (absent from the reference) and
        // diverging placeholders: `validate_external_map` warns (a log), but the
        // overlay still merges the content (validation is advice, not rejection).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"totally.unknown.key":"x","selfmodel.age.days":"vor {tagen} Tagen"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let de = &maps[&Lang::Ext("de")];
        assert_eq!(de.get("totally.unknown.key").map(String::as_str), Some("x"));
        assert!(de.contains_key("selfmodel.age.days")); // merged despite the warn
    }

    #[test]
    fn overlay_accepts_bcp47_named_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pt-br.json"),
            r#"{"ui.lang.name":"Português (BR)"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        assert!(maps.contains_key(&Lang::Ext("pt-br")));
    }

    // ------- Bundle export (CLI locales export) -------

    #[test]
    fn export_builtin_is_raw_source() {
        // A built-in language is exported as verbatim source (preserves arrays).
        assert_eq!(export_bundle(Lang::Ru), Lang::Ru.bundle_src());
        assert_eq!(export_bundle(Lang::En), Lang::En.bundle_src());
    }

    #[test]
    fn export_unknown_code_yields_reference_template() {
        // An unregistered code → a template: the reference's full key set with its
        // values (a valid JSON object, all keys present). The reference is English
        // (the source language), so a translator starts from the text the code was
        // written in rather than from its Russian translation.
        let json = export_bundle(Lang::Ext("zz"));
        let obj: HashMap<String, String> = serde_json::from_str(&json).unwrap();
        let en = locale(Lang::En);
        assert_eq!(obj.len(), en.map.len());
        assert_eq!(obj.get("ui.lang.name"), en.map.get("ui.lang.name"));
    }

    #[test]
    fn code_and_from_code_round_trip() {
        assert_eq!(Lang::from_code("ru"), Lang::Ru);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("de"), Lang::Ext("de"));
        for l in [Lang::Ru, Lang::En, Lang::Ext("de")] {
            assert_eq!(Lang::from_code(l.code()), l);
        }
    }

    #[test]
    fn lang_serde_round_trips_codes() {
        for (lang, json) in [(Lang::Ru, "\"ru\""), (Lang::En, "\"en\"")] {
            assert_eq!(serde_json::to_string(&lang).unwrap(), json);
            assert_eq!(serde_json::from_str::<Lang>(json).unwrap(), lang);
        }
        let de: Lang = serde_json::from_str("\"de\"").unwrap();
        assert_eq!(de, Lang::Ext("de"));
        assert_eq!(serde_json::to_string(&de).unwrap(), "\"de\"");
    }

    #[test]
    fn label_ext_falls_back_to_code_without_bundle() {
        // An external language with no loaded bundle (the registry in tests — built-in
        // only) → falls back to the code: `locale_exact` doesn't find `de`, the
        // reference isn't substituted.
        assert_eq!(Lang::Ext("de").label(), "de");
    }

    #[test]
    fn loaded_external_language_registers_and_self_names() {
        // The full `init` path without the global registry: overlay → leak into a
        // local map → `all_from`/`get`/`t`. Covers the combination pure tests don't touch.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("de.json"),
            r#"{"ui.lang.name":"Deutsch","ui.feed.role.user":"DU"}"#,
        )
        .unwrap();
        let mut maps = builtin_maps();
        overlay_external(&mut maps, dir.path());
        let reg: HashMap<Lang, &'static Locale> = maps
            .into_iter()
            .map(|(lang, map)| {
                let loc: &'static Locale = Box::leak(Box::new(Locale::from_map(lang, map)));
                (lang, loc)
            })
            .collect();
        let de = Lang::Ext("de");
        // The new language is registered in the order ru, en, de.
        assert_eq!(all_from(&reg), vec![Lang::Ru, Lang::En, de]);
        // Self-naming from its own bundle (the data source for `Lang::label` on Ext).
        assert_eq!(reg[&de].get("ui.lang.name"), Some("Deutsch"));
        // Its own key from the file.
        assert_eq!(reg[&de].t("ui.feed.role.user"), "DU");
        // A key outside the file → falls back to the **en** reference (not empty, not
        // the key itself, and not Russian — a German locale missing a key used to show
        // Russian, which is what D5 of robustness-and-defaults.md fixed).
        assert_eq!(
            reg[&de].t("ui.feed.role.assistant"),
            reg[&Lang::En].t("ui.feed.role.assistant")
        );
        assert_ne!(
            reg[&de].t("ui.feed.role.assistant"),
            reg[&Lang::Ru].t("ui.feed.role.assistant")
        );
    }
}
