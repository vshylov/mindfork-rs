# i18n Tier 3 — external locales and new languages without a rebuild

> **Status: IMPLEMENTED** (branch `feat/i18n-external-locales`). Continues
> [docs/history/i18n.md](i18n.md) §5 "Tier 3". The scope fork was confirmed by
> the user (2026-07-13): went with the **full** variant — external files not
> only *override* the built-in ru/en, they also let you **add new languages**
> without a rebuild. Axis A (agent language) and axis B (interface language)
> use one mechanism (`shared/i18n`), so an external file covers both
> automatically (in the bundle, both `prompt.*`/`tool.*` and `ui.*`).
> **Implementation summary — end of file (§7).**

## 1. Goal and scope

Today the `locales/{ru,en}.json` bundles are **built in** to the binary
(`include_str!`), and `Lang` is an enum `{Ru, En}`. Tier 3 provides:

1. **Override the built-ins** — `data/locales/{ru,en}.json` is merged **on
   top of** the built-in one, key by key. A partial file is the norm: only the
   keys present get overridden, the rest come from the built-in bundle. Lets
   you tune prompts / fix en wording without a rebuild (closes the deferred
   en-wording review by editing a file).
2. **New languages** — `data/locales/<code>.json` with a code that doesn't
   match a built-in one (`de`, `fr`, `uk`…) becomes a first-class `Lang`,
   selectable in the UI and stored in `Profile.language`/
   `config.interface.language`. Missing keys of a new language degrade to the
   reference (`ru`) via `t()`.

**Out of scope:** hot reload (a file edit applies on restart — precedent: the
`&'static` leak), auto-translation, unit coverage of user-supplied file
content.

## 2. Architecture

### 2.1 `Lang` — enum + `Ext` variant

Minimize ripple: **keep** the `Ru`/`En` variants (used as `Lang::Ru`/
`Lang::En` in ~80 places) and add one variant for dynamic languages.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Ru,                    // built-in reference
    En,                    // built-in
    Ext(&'static str),     // external language; string is the code (interned, Box::leak)
}
impl Default for Lang { fn default() -> Self { Lang::Ru } }
```

- `Copy` is preserved (`&'static str: Copy`); `Eq`/`Hash` are by code content
  (`&str` compares by value, not by pointer → interning isn't required for
  `Eq` correctness, only to get `&'static` out of a runtime `String`).
- `code(self) -> &'static str`: `Ru→"ru"`, `En→"en"`, `Ext(c)→c`.
- `from_code(&str) -> Lang`: `"ru"→Ru`, `"en"→En`, else `Ext(intern(code))`.
- `intern(&str) -> &'static str`: a global `LazyLock<Mutex<HashSet<&'static str>>>`;
  a new code → `Box::leak(code.to_string().into_boxed_str())`. There are only
  ever finitely many languages — a bounded one-time leak (precedent: the
  syntect-theme cache in `markdown/code.rs`).

### 2.2 serde — as a code string

Drop `#[serde(rename_all="lowercase")]`, write manual `Serialize`/
`Deserialize`: serialization = `code()`, deserialization =
`from_code(&String::deserialize()?)`. Existing `"ru"`/`"en"` data round-trips
byte-for-byte; `#[serde(default)]` (Default=Ru) is unchanged. A profile with
`"language":"de"` deserializes to `Ext("de")` even before the bundle is
registered (interning doesn't depend on the registry) → no ordering hazard; a
missing bundle just degrades to the reference + logs a warning.

### 2.3 Locale registry: built-in + external

```rust
static BUILTIN: LazyLock<HashMap<Lang, &'static Locale>>;   // ru/en, for tests and init's base
static REGISTRY: OnceLock<HashMap<Lang, &'static Locale>>;  // set by init() in production

pub fn init(dir: &Path);              // main.rs, once at startup
fn registry() -> &'static HashMap<..> // REGISTRY.get().unwrap_or(&*BUILTIN)
pub fn locale(lang) -> &'static Locale // registry lookup → fallback REFERENCE(ru)
```

- **`init(dir)`**: builds owned maps of the built-ins (`ru`/`en`; panics on a
  broken built-in — a gate test), `overlay_external(dir)` merges the
  external files, then each map gets `Box::leak`ed once into a `Locale` →
  `REGISTRY.set`. Idempotent (`set` ignores repeats).
- **Without `init` (tests)**: `registry()` returns `BUILTIN` — pure built-in
  behavior, the existing ~1000 tests are unaffected.
- **`overlay_external(maps, dir)`**: `read_dir(dir)` (no directory → no-op,
  built-ins only); for each `*.json`:
  - stem = the language code; validate `[a-z]+` (else `warn` + skip).
  - read+parse **without panicking** (`parse_external_map -> Result`); an
    error → `warn` (to the log, stdout is taken by the TUI) + skip the file
    (the built-in stays intact — the failure mode isn't "prompt broken" but
    "graceful degradation"; this is precisely the difference between external
    and built-in).
  - merge: `maps.entry(lang).or_default()` then `insert` each key (per-key
    override). For a built-in code — on top of the built-in map; for a new
    one — on top of an empty one (missing keys → fallback to the reference in
    `t()`). `info!` logs the key count.

### 2.4 `Lang::ALL` (built-in) vs `Lang::all()` (registry)

- `pub const ALL: &[Lang] = &[Lang::Ru, Lang::En]` — **kept**: "built-in
  languages". Used by completeness gate tests (`key_sets_match`/
  `placeholder_sets_match`/`en_bundle_has_no_cyrillic`) and per-locale
  structural tests — they check the **built-in** bundles (external ones are
  user content, not gated). Zero churn.
- `pub fn all() -> Vec<Lang>` — a registry-aware list (built-ins + found
  external ones), deterministic order (`Ru`, `En`, then `Ext` by code). In
  tests (without `init`) = `[Ru, En]`. Used by:
  - **UI language selectors** (axis B `ILanguage`: `spec.rs`/`cycle_lang`;
    axis A `PLanguage`: `apply.rs`/`choice.rs`) — so new languages are
    selectable;
  - **`present.rs::exit_labels`** — recognizing the `python.console.exit`
    label across **all** languages (a profile on `de` writes it in German →
    the console parser must know it).

### 2.5 Language label in the UI

`Lang::label()`: `Ru→"Русский"`, `En→"English"` (as now — the built-ins stay
untouched), `Ext(code) → locale(self).get("ui.lang.name").unwrap_or(code)` — a
new language names itself via the reserved key `ui.lang.name` (otherwise its
code is shown).

### 2.6 Paths and plumbing

- `Paths::locales_dir()` = `root/locales`; the directory is created in
  `Paths::discover` (for discoverability — an empty directory signals "put
  files here").
- `main.rs`: `i18n::init(paths.locales_dir())` **right after** `Paths::discover`
  + `logging::init`, **before** `Storage::open`/loading the config (a profile
  with an `Ext` code deserializes fine either way, but the registry must be
  ready for the first `locale()` call).
- `backup.rs`: the `locales/` directory is user content (their override
  prompts) → **included** in the backup archive (add it to the
  include-whitelist next to `chats/`/`dictionaries/`).

## 3. Ripple (edit points)

| File | Change |
|---|---|
| `shared/i18n.rs` | `Lang` enum+`Ext`, manual serde, `code`/`from_code`/`intern`, registry (`BUILTIN`/`REGISTRY`/`init`/`overlay_external`/`parse_external_map`), `all()`, `label()` for `Ext` |
| `shared/paths.rs` | `locales_dir()` + directory creation in `discover` |
| `main.rs` | `i18n::init(paths.locales_dir())` at startup |
| `features/backup.rs` | `locales/` in the include-whitelist |
| `screens/settings/{spec,apply,choice}.rs`, `helpers.rs::cycle_lang` | `Lang::ALL` → `Lang::all()` in 4 UI language selectors |
| `features/tools/present.rs::exit_labels` | `Lang::ALL` → `Lang::all()` |

Gate tests and per-locale structural tests (`ALL`) — **left untouched**.

## 4. Tests

**Mechanism** (tempdir, built-ins not needed — no new crates needed):
- `overlay_external` merges a built-in override (a partial `en.json` changes
  one key, the rest — built-in).
- a new language (`xx.json`) → `Lang::Ext("xx")` in the registry, keys
  present come from the file, missing ones → fall back to `ru`.
- a broken JSON external file → warn+skip, the built-in stays intact
  (registry is standard).
- an invalid file name (`EN.json`/`e n.json`) → skip.
- `all()` includes the found `Ext`, order is deterministic.
- serde `Lang`: round-trip `ru`/`en`/`de`; unknown code → `Ext`.
- `Paths::locales_dir` under the root.

**Registry isolation in tests**: `init` sets the global `OnceLock`; mechanism
tests must not fire against the shared registry. Solution: test the **pure**
`overlay_external`/`parse_external_map`/`from_code`/`all_from(reg)` over a
local map (an argument), not touching the global `REGISTRY`. Cover `init`
with one idempotency smoke test through a separate process-independent path
(or test its components rather than the global).

Existing gates (`key_sets_match`, `placeholder_sets_match`,
`en_bundle_has_no_cyrillic`, `all_ui_keys_referenced_in_code_exist_in_bundle`)
— green, unchanged.

## 5. Live run

Unit tests cover the mechanism fully (filesystem — tempdir). Additionally, a
manual check on a real install: drop `data/locales/de.json` with a few keys
→ "Deutsch"/"de" appears in settings, profile/UI language selection works,
missing keys fall back to Russian, a broken file — a warning in the log +
the app stays alive. A live run against a model is **not required** (this is
a loading mechanism, not model behavior); ru/en regression is covered by the
existing unit suite.

## 6. Risks

1. **Init ordering vs. deserialization.** Resolved: `from_code` interns
   independently of the registry; `init` runs before `Storage::open`. A
   profile with an unknown code degrades to the reference instead of
   crashing.
2. **Global `OnceLock` in tests.** The mechanism is tested with pure functions
   over a local map; the global isn't touched (see §4).
3. **Stale external file vs. new built-in keys.** Per-key overlay: an
   external file overrides **only its own** keys, new built-in keys flow
   through → prompt improvements aren't lost. So we **don't** copy the
   templates into `data/locales/` at build time (a full copy would silently
   revert future built-in edits); we document "put only the keys you want to
   override".
4. **Incomplete new language.** Deliberate degradation to `ru` + an `info`
   log of coverage; it's the user's responsibility (their file).

## 7. Implementation summary

Implemented per plan, no deviations from the forks.

- **`Lang`** (`shared/i18n.rs`): enum `Ru`/`En`/`Ext(&'static str)` (`Ru`/`En`
  kept → zero churn at ~80 call sites), manual `Serialize`/`Deserialize` as a
  code string, `code`/`from_code`/`intern` (global `Mutex<HashSet>` +
  `Box::leak`). `Default` via `#[derive(Default)]`+`#[default]` (clippy).
  `ALL` (built-in) stayed for the gates; added `all()` (registry-aware,
  `all_from(reg)` for testing without the global).
- **Registry**: `BUILTIN` (`LazyLock`, tests/base) + `REGISTRY` (`OnceLock`,
  set by `init`); `overlay_external` merges `data/locales/*.json` (valid name
  `[a-z]+`, broken/unreadable → `warn`+skip, per-key override / new
  language), `json_to_map` (lenient, `Err` instead of panicking),
  `locale`/`locale_exact` (with/without fallback to the reference — `label`
  for `Ext` reads its own bundle without leaking `ru`). The `ui.lang.name` key
  was added to both built-in bundles (self-naming).
- **Plumbing**: `Paths::locales_dir()` + directory creation in `discover`;
  `i18n::init` in `main.rs` (before `Storage::open`); `locales/` in
  `backup.rs`'s include-whitelist; the 4 UI language selectors +
  `present.rs::exit_labels` switched from `Lang::ALL` to `Lang::all()`.
- **Tests**: 12 new units in `i18n.rs` (json_to_map, overlay override/new
  language/skip on broken or bad name/no directory, sort_langs,
  code/from_code round-trip, serde, label fallback, full compose over a local
  map) + `Paths::locales_dir`. The global `REGISTRY` isn't touched in tests
  (pure functions over a local map). **1015 unit tests green** (+12), clippy
  `-D warnings`/fmt clean.
- **Live run** (not against a model — a file-loading mechanism):
  `data/locales/de.json` (3 keys) at real-binary startup → log `INFO …
  external locale: added new language … lang="de" keys=3`. The
  `init`→registry integration is confirmed on a real path; interactive
  language selection in the TUI follows the pattern of existing selectors
  (needs a live terminal), covered by a selector-cycle unit test.
- **Out of scope (future work)**: hot reload (an edit applies on restart —
  precedent: the `&'static` leak), auto-translating existing data, a CLI
  export of the full bundle.

**The i18n track (axis A tiers 1–3 + axis B) — complete.**
