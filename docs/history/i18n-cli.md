# CLI and remaining-surface multilingualism — "all text into bundles"

> **Status: design plan, decision points §4 confirmed by the user (2026-07-14).**
> Continuation of the i18n track: axis A (agent language, Tiers 1–3 —
> [docs/history/i18n.md](i18n.md)) and axis B (interface language —
> [docs/history/i18n-ui.md](i18n-ui.md)) are complete.
> User framing (2026-07-14): "so that the translation bundles can hold **absolutely
> all** text shown to the user, including what's printed via `println!`/
> `eprintln!`/`bail!` in `main.rs`."
>
> **User decisions (2026-07-14):** all decision points in §4 — per the recommendations,
> **with one amendment to decision point 3**: on **total uncertainty** about the language
> (neither `settings.json` nor `defaults.json`, or a corrupt `defaults.json`) we print in
> **English**, not the Russian reference. I.e. `cli_lang()`'s final fallback and the early
> error print (before the language is known) — `Lang::En`. Outcome of the confirmed
> decision points: (1) our own micro-parser (variant B); (2) language source
> `settings → defaults → **en**`; (3) total uncertainty → **en**; (4) a single-line
> `{prefix}: {err:#}`; (5) engine-tail scope — `managed.rs` + `shared/sandbox.rs`, HTTP
> client wrappers — a boundary; (6) `clap` removed from the dependencies.

## 1. Motivation and scope

After axes A+B were completed, text remained outside the bundles that a human sees **in
the console** (CLI subcommands `backup`/`restore`/`import-lamellama`/`sandbox`/`locales`,
the second-instance message, early TUI-startup errors) and **in error tails**
(anyhow chains, which surface both in the CLI and in the status-chip/feed). This is axis
B — text for the human, the language is the global `config.interface.language`; the
mechanism is the same (`shared/i18n`, keys in the bundles), no new crates needed.

Two architectural problems this surface didn't have before:

1. **Chicken and egg at startup.** The UI language lives in `settings.json`, the
   default — in `defaults.json`; errors reading these files must be printed *before*
   the language is known. Mitigation: the built-in bundles are available **without**
   `i18n::init` (a `LazyLock` over `include_str!`) — localized printing is possible from
   the first line of `main`; the only question is which language to pick.
2. **Text our code doesn't generate**: `clap`'s help/errors, the std `Error:` prefix
   from `main() -> Result`, anyhow's "Caused by:", `io::Error` strings from the OS.
   These can't be "moved into a bundle" — either replace them with our own output, or
   declare them a boundary.

## 2. Inventory (what's still outside the bundles)

| Category | Where | Surfaces in | Language |
|---|---|---|---|
| CLI messages (~25) | `main.rs`: `println!`/`eprintln!`/`bail!` in `run_backup`/`run_restore`/`run_import`/`run_locales`, second-instance, `acquire_cli_guard` | stdout/stderr | UI |
| Argument-parser help/errors | `clap` derive: `about`, subcommand/argument doc comments; boilerplate "Usage:"/"Options:"/"Commands:"/"Print help"; parse-error messages (`error: unexpected argument…`), the `-c` range error | stdout/stderr | UI |
| anyhow chains of CLI features (~60) | `features/backup.rs` (~23: "creating the archive…", "the archive is corrupted…", "unsafe entry name…"), `features/sandbox_setup.rs` (~30, including the **progress callback**: "Downloading…", "Unpacking…", "Done. Sandbox installed", "sha256 mismatch…"), `features/migration.rs` (~5, English already) | stdout/stderr | UI |
| shared low-level (~10) | `shared/paths.rs` (corrupt `defaults.json`, "creating the data directory…", English exe contexts), `shared/instance.rs` (thiserror `Display`), `shared/logging.rs` (1), English contexts in `main.rs` ("opening storage", "building tokio runtime") | stderr (part of it — **before** the language is known) | UI |
| Standard error scaffolding | `Error: …` (std `Termination` for `main() -> Result`) + a multi-line "Caused by:" (anyhow `Debug`) | stderr | — |
| Engine tail (~10) | `shared/api/managed.rs` (preflight/probe: "model file not found…", "server… exited before becoming ready…", "did not become ready within…") → **status chip** "no connection: why"; `shared/sandbox.rs` ("sandbox busy…", "wasmer binary not found", launch contexts) → **`python_exec` result** (read by the model, seen by the human) and **CLI warmup** | status bar / feed / stdout | chip — UI; tool result — **profile language** (axis A); warmup — UI |
| HTTP client wrappers | `shared/api/{openai,anthropic,gemini}` — contexts around network errors; the main content is the server's error body (untranslatable by nature) | feed (`AppEvent::Error`) | UI |

Not inventoried (boundaries, §7): panics/`debug_assert`, `tracing` logs,
`build.rs`, OS `io::Error` strings, external server error bodies.

## 3. Solution architecture

### 3.1 CLI language — `cli_lang()`

Resolution order: `settings.json` → `interface.language` (the human already chose a
language in the app — the CLI is for the same human) → `defaults.json` **if the file is
present** → `default_language` (set by the installer) → **`En`** (total uncertainty: a
fresh binary with no configuration — an international default). No new config field/
marker. A corrupt `settings.json` → the next step (the same tolerance as
`load_config().unwrap_or_default()`). The existence of `defaults.json`/legacy
`location.json` is checked explicitly — otherwise the serde default `default_language=Ru`
would never let it reach the `En` tail (`default_language=Ru` is the default of the
**scaffold** language for new profiles, axis A, not a display-language signal).

### 3.2 A read-only "peek" phase before parsing arguments

Currently `Cli::parse()` is the first line of `main`, so `--help` doesn't touch the disk.
For help to be localized (and for external locales too), the language and the locale
registry are needed **before** parsing — but `--help` mustn't start creating directories.

- `Paths::discover()` splits into `Paths::resolve()` (a pure computation: exe_dir →
  `Defaults::read` → `root_dir`, **without** `create_dir_all`) and `paths.ensure_dirs()`
  (creates the root + `locales/`); `discover()` = `resolve()+ensure_dirs()` for call-site
  compatibility in tests.
- `main`'s order: `resolve()` → `cli_lang()` (a read-only read of `settings.json`) →
  `i18n::init(locales_dir)` (a read-only scan) → **argument parsing with localized
  text** → CLI branches / the TUI branch (`ensure_dirs()` → `logging::init` → …).
- **Warnings from loading external locales** currently go to `tracing` — during the
  early `init` the subscriber isn't set up yet. `init` starts **returning** a
  `Vec<String>` of warnings; the TUI branch logs them after `logging::init`, the
  early-exit branches (`--help`) silently discard them (best-effort, as before).
- A `resolve()` failure (a corrupt `defaults.json` — the only realistic case): the
  language is unknown → print in **English from the built-in bundle** (§4, decision
  point 3, the user's amendment).

### 3.3 `main() -> ExitCode`: error printing is our own

`fn main() -> std::process::ExitCode` + `fn real_main(...) -> anyhow::Result<()>`.
On `Err`, `main` prints itself: `eprintln!("{}: {err:#}", loc.t("cli.err.prefix"))`
(format `{:#}` — a single-line "msg: cause: cause" chain, already used in
`run_restore`) and returns exit code 1. This removes the English `Error:` (std) and
"Caused by:" (anyhow Debug). A clean helper `cli_error_line(loc, &err) -> String` —
testable without running the binary.

### 3.4 `cli.*` keys and automatic safety-net extension

New prefixes: `cli.help.*` (parser help), `cli.err.*` (prefix/scaffolding),
`cli.ctx.*` (startup contexts), `cli.guard.*` (second-instance + "close the app before
{action}"), `cli.backup.*`, `cli.restore.*`, `cli.sandbox.*`, `cli.import.*`,
`cli.locales.*`, `cli.paths.*`. ru values are **byte-for-byte** the current strings
(existing assertions don't change — accepted by all i18n PRs). The whole existing
safety net extends **automatically**: `bundle_prefixes()` is derived from the bundle →
the gates "a key from code exists in the bundle", "no dead keys", key/placeholder
parity, and `en_bundle_has_no_cyrillic` cover `cli.*` with no test edits;
`export_bundle` picks up the new keys on its own.

### 3.5 Threading the locale to depth — two established patterns

Selection rule (both already in the codebase):

- **A `&'static Locale` parameter** — for functions with parameterized deep chains
  (precedent from Tier 2c: `fs`/`python`/`calc` threaded `loc` into helpers):
  `backup::create_backup(..., loc)`, `backup::restore_backup(..., loc)`,
  `sandbox_setup::setup(..., loc)` (progress strings via `loc.tf`, the callback
  signature `FnMut(&str)` unchanged), `migration::import_dir(dir, loc)`,
  `Defaults::read(exe_dir, loc)` / `Paths::resolve(loc)`, `logging::init(paths, loc)`.
- **A structured error, formatted at the boundary** — for a 2–3-variant enum (precedent
  `ApiKeyError { NoName, Missing }` from axis B): `instance::InstanceError` stays a
  thiserror struct, but its `Display` stops being user-facing text — `main`/
  `acquire_cli_guard` build the message from the bundle by variant.

### 3.6 The argument parser — the central decision point (§4, decision point 1)

**Variant A — stay on clap, localize its surface.**
clap 4 derive accepts expressions in attributes (`#[command(about = expr)]`,
`#[arg(help = expr)]` — builder calls at runtime), i.e. subcommand/argument help can be
read from the bundle. Headers: "Usage:" — our own `help_template`, sections —
`next_help_heading`/`subcommand_help_heading`, `--help`/`--version` — our own `Arg`s
with `ArgAction::Help/Version` instead of the auto-generated ones. Cost: (a) derive
expressions don't accept parameters → a **global** `OnceLock` CLI locale is needed
(exactly what we rejected `rust-i18n` for); (b) **parse-error messages stay English** —
rendered inside clap; a custom renderer by `err.kind()`+`err.context()` could be
written (~100 lines, fragile to clap upgrades, still falls back to English); (c) the
`value_parser!(i64).range(0..=9)` range error — English.

**Variant B — our own micro-parser (`features/cli.rs`, ~200–250 lines + tests).**
The surface is small: 5 subcommands, 6 options, 3 positionals. Sketch:

```rust
pub enum CliCommand {
    Run,                                  // no subcommand → TUI
    Backup { output: Option<PathBuf>, compression: i64 },
    Restore { archive: PathBuf },
    ImportLamellama { dir: PathBuf },
    SandboxSetup { force: bool },
    LocalesExport { code: String, output: PathBuf },
    Help(Option<&'static str>),           // general / per-subcommand
    Version,
}
pub fn parse(args: &[String], loc: &Locale) -> Result<CliCommand, String>; // Err — ready-to-print text
pub fn render_help(topic: Option<&str>, loc: &Locale) -> String;
```

Parity of forms with clap (covered by tests): `--opt value`, `--opt=value`, `-o value`,
`-h`/`--help` (globally and after a subcommand), `-V`/`--version`; `-c 0..=9` validation
and "file already exists" — their own text from the bundle. **100% of the text from the
bundle**, the locale as a plain parameter (no globals), one dependency fewer (clap is
used **only** in `main.rs` — it leaves entirely along with the clap_builder/anstream
tree). Precedents of "a crate gets in the way of a requirement → our own micro-solution":
markdown (ADR 0003), i18n, calc, InputBox (ADR 0001). Cost: rewriting a working
interface; clap's "did you mean" hints go away (a simplest prefix-based one could be
added — not required).

**Not translated in either variant**: subcommand and flag names (`backup`,
`--output`) — this is protocol, like tool ids and `/rag` commands (i18n.md §2.3).

### 3.7 Engine tail: the managed probe and the sandbox

- `shared/api/managed.rs`: preflight/probe text is shown in the status chip ("no
  connection: why") — this is UI. `ManagedConfig` gains `loc: &'static Locale`
  (the supervisor already receives the UI locale from axis B: `apply_chat(..., loc)`),
  errors — through `ui.err.managed.*` keys. Closes a long-standing "leftover" of
  axis B.
- `shared/sandbox.rs`: dual audience — errors reach both the `python_exec` result
  (**profile** language, axis A) and the CLI warmup (UI language). The trait
  `SandboxRunner::run` receives `loc` as a parameter (the caller chooses: the tool —
  `ctx.loc`, warmup — the UI language); keys `sandbox.err.*`.
- HTTP client wrappers (`shared/api/{openai,anthropic,gemini}`) — a **boundary** (by
  default, decision point 5): their main content is the server's/OS's error body,
  untranslatable by nature; localizing three or four words of scaffolding doesn't pay
  for threading the locale into every client and its tests.

## 4. Decision points (to confirm before implementation)

1. **CLI parser.** (A) clap + localizing its surface, parse errors — a boundary
   (or a custom renderer by `ErrorKind`, still not 100%); (B) our own micro-parser —
   100% of the text from the bundle, the locale as a parameter, one dependency fewer.
   **Recommendation: B** — the only honest path to "absolutely all text"; the surface
   is small, precedents are in the project's DNA. A — if zero churn of a working CLI
   matters more.
2. **CLI language source.** ✅ `settings → defaults → **en**` (the recommendation +
   the user's amendment: the final fallback is en, not ru). No env override
   (`MINDFORK_UI_LANG`) is introduced — no request for it.
3. **Errors before the language is known** (a corrupt `defaults.json`). ✅ Print in
   **English** from the built-in bundle (the user's amendment: total uncertainty → en).
   Same keys, no hardcoding.
4. **Error-output form.** A single-line chain `{prefix}: {err:#}`
   (**recommendation**, already used in restore); or a multi-line render with a
   localized "Cause:".
5. **Engine-tail scope.** Stage 3 = `managed.rs` (status chip) + `shared/sandbox.rs`
   (**recommendation**); HTTP client wrappers stay a documented boundary.
   Alternative — threading the locale into every client (a lot of churn, small payoff).
6. **Fate of the clap dependency under B.** Remove it from `Cargo.toml`
   (**recommendation**) or keep it for the future (dead weight — against the project's
   conventions).

## 5. Stages (branches/PRs)

1. **`feat/cli-i18n-bootstrap`** ✅ **done** — the mechanism: `Paths::resolve()`/
   `ensure_dirs()`, `cli_lang()`, an early `i18n::init` returning warnings,
   `main() -> ExitCode` + `cli_error_line`, the parser per decision point 1, all text of
   `main.rs` + `paths`/`instance`/`logging` → `cli.*`. The most substantial stage.
2. **`feat/cli-i18n-features`** ✅ **done** — threaded `loc` into `backup` /
   `sandbox_setup` (including progress) / `migration`; their `bail!`/`context`/progress
   → keys (`backup.*` 17, `sandbox.setup.*` 37, `migration.*` 4). Mechanics per the
   Tier 2c playbook, ru byte-for-byte. +3 per-locale regression tests.
3. **`feat/i18n-engine-tail`** ✅ **done** (decision point 5) — `managed.rs` (5 keys
   `ui.err.managed.*`: preflight/probe → status chip, axis B; `loc` threaded through
   `launch`/`wait_until_ready` + the supervisor) + `shared/sandbox.rs` (7 keys
   `sandbox.err.*`: `SandboxRunner::availability`/`run` gained `loc` — dual audience:
   `python_exec` embeds their result in the profile's language (axis A), provisioning
   warmup — in the UI language). HTTP client wrappers stay a documented boundary (§7).
   +2 per-locale regression tests. i18n.md §2.3: engine probe errors removed from the
   leftovers list, the "HTTP client wrappers" boundary written in.

Scope estimate: ~120–150 new keys (ru+en) total.

## 6. Tests

- **Automatic**: all four bundle gates + parity + no-cyrillic cover `cli.*` with no
  edits (§3.4).
- **New units**: `cli_lang` ordering (tempdir: settings > defaults > ru; a corrupt
  settings → the next step down); `Paths::resolve` creates no directories /
  `ensure_dirs` creates them; `cli_error_line` (a localized prefix, format `{:#}`);
  under B — a parser-table test (all subcommands/option forms/errors/`-c` out of
  range/`--help` after a subcommand), `render_help` has no unsubstituted `{…}` across
  all `Lang::ALL`.
- **ru byte-for-byte**: the existing backup/sandbox_setup/migration/paths tests don't
  change assertions — call sites gain the `ru()` locale (accepted by every i18n PR).
- **No live run needed** (the engine is untouched — like axis B); a manual check
  against the real binary: `--help`/a parse error/`backup` in ru and en
  (`settings.json`), an external `de` locale + `interface.language=de` → CLI in de,
  a corrupt `defaults.json` → an en fallback.

## 7. Boundaries (deliberately NOT localized)

- **`io::Error` strings** — produced by the OS (already in the OS's language on
  Windows) and external server error bodies (HTTP) — arrive as ready-made text.
- **HTTP client wrappers** (under decision point 5 = recommendation) — see §3.7.
- **panics / `debug_assert` / `unreachable!`** — for the developer, not the user.
- **`tracing` logs** — the convention "file only" (i18n.md §2.3).
- **CLI subcommand/flag names** — protocol (§3.6).
- **clap error messages** — only if decision point 1A is chosen.

## 8. Out of scope

- Localizing/aliasing command names (`backup` → a translated alias) — like the `/rag`
  aliases, future work.
- Detecting the language from the OS locale.
- Translating existing user data.
