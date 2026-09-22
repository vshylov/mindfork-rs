# Journal — Releases, packaging and branding

Getting the built product to a user: version and schema discipline, the release pipeline, Linux packages and the Windows installer, and the brand assets that ship with them.

**Reference documents for this area:** architecture.md §12, AGENTS.md §6

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (50)

- Post-M9: release engineering — stage 1 (CI pipeline + toolchain pin + license) (done)
- Post-M9: release engineering — stage 2 (version 0.9.0 + CHANGELOG + showing the version) (done)
- Post-M9: release engineering — stage 3 (schema versions + JSON migrations) (done)
- Post-M9: release engineering — stage 4 (SQLite schema migrations) (done)
- Post-M9: release engineering — stage 5 (release pipeline + backup manifest) (done)
- Post-M9: release engineering — stage 6 (cargo-deny) + dictionaries in the release (done)
- Post-M9: installers — research + stage 1 (code prerequisites) (done)
- Post-M9: installers — stage 2 (Linux packages deb/rpm/pkg.tar.zst) (done)
- Post-M9: installers — stage 3 (Windows installer, Inno Setup) (done)
- Release 0.9.1 (prepared)
- Post-M9: branding — logo and wordmark (stages 1–4) (done)
- Post-M9: branding — TUI wordmark, a lockup instead of a bare glyph (done)
- Post-M9: the mindfork.io site URL in project metadata (done)
- Post-M9: the Windows installer can provision the Python sandbox (done)
- Release 0.9.5 (prepared)
- Post-M9: demo screenshots — stage 1 (fixture, frame dumps, raster tool) (done)
- Post-M9: demo screenshots — stage 2 (the full set, the drift gate, README embeds) (done)
- Post-M9: demo screenshots — stage 3 (the interactive `mindfork demo`) (done)
- Post-M9: demo screenshots — stage 4 (SVG writer for mindfork.io) (done)
- Post-M9: demo screenshots — a uniform gallery and a richer hero (done)
- Post-M9: demo screenshots — the canvas padding measured in cells (done)
- Post-M9: the Windows installer shows the license and the disclaimer (done)
- Release 0.9.7 (prepared)
- Post-M9: the installer's legal pages speak Russian (done)
- Post-M9: the Windows installer moves to Inno Setup 7 (done)
- Post-M9: the binary/command renamed to `mindfork` (done)
- Release 0.9.8 (prepared)
- Post-M9: `artwork/` renamed to `assets/` (done)
- Post-M9: the dictionaries are copied more than once (done)
- Post-M9: a place for the user's own dictionaries (done)
- Post-M9: the release metadata — one product, one name, measured on both artifacts (done)
- Post-M9: the privacy policy in the wizard, the archive and the third translation (done)
- Post-M9: the dictionaries get a provenance record — and their licences (done)
- Post-M9: en_GB updated to V 4.0.9 — the licence stated, in the file itself (done)
- Release 0.9.9 (prepared)
- Post-M9: public release readiness — the audit and stage 1 (done)
- Post-M9: public release readiness — stage 3, the release pipeline (done)
- Post-M9: public release readiness — stage 4a, the ways a first run ends badly (done)
- Post-M9: public release readiness — stage 5a, the documents a stranger meets (done)
- Post-M9: public release readiness — stage 5c, the release pull request (done)
- Post-M9: the Cargo package becomes `mindfork` (done)
- Post-M9: public release readiness — stage 6, the flip (done)
- Post-M9: crates.io — the publish becomes a button the owner presses (done)
- Release 0.10.1 (prepared)
- Release 0.10.2 (prepared)
- Post-M9: the README's screenshots are the dark ones (done)
- Post-M9: `install.sh` — the release installs itself on a bare Linux box (done)
- Release 0.11.0 (prepared)
- Post-M9: `install.sh` failed on the first real pod — root without CAP_CHOWN (done)
- Release 0.11.1 (prepared)

### Post-M9: release engineering — stage 1 (CI pipeline + toolchain pin + license) (done)
- **The first stage of the "release engineering" track** (design plan
  [docs/history/release-engineering.md](../../docs/history/release-engineering.md), decision points confirmed by the
  user 2026-07-15; branch `feat/ci-pipeline`): versioning, changelog,
  CI, and data-schema versioning/migrations. Stage 1 closes out **CI** — before this,
  the `fmt`/`clippy -D warnings`/`test` gates relied on nothing but agent discipline, and
  **the Linux build wasn't checked at all** (development happens on Windows).
- **`.github/workflows/ci.yml`**: a `lint` job (ubuntu — `cargo fmt --check` +
  `cargo clippy --all-targets -- -D warnings`) and a `test` job (a matrix of
  `ubuntu-latest` + `windows-latest` — `cargo test`). `#[ignore]` smokes
  (engine/network/live model) are silently skipped without the env vars — CI needs no network or a live
  server. `Swatinem/rust-cache` caching, `concurrency` with
  `cancel-in-progress` by ref, triggers `pull_request` + `push:main`.
- **`rust-toolchain.toml`** — pinned to `1.96.0` + `rustfmt`/`clippy` (F4): the
  `-D warnings` gate would suddenly break on every new stable's new lints;
  toolchain upgrades are a deliberate separate PR.
- **`LICENSE`** (MIT, the file was added — the license was declared in `Cargo.toml` but the file
  didn't exist); CI + License badges added to the README header.
- **A platform-portability scan of the tests** (subagent, a very thorough scan of
  `src/`): **0 tests are able to fail Linux CI** — the code was written with Linux in
  mind (sorting for determinism, stripping the verbatim `\\?\` prefix, paired
  `#[cfg(windows)]`/`#[cfg(not(windows))]` variants, CRLF tolerance; spawns
  in tests gated by `cfg!(windows)`, `.exe` suffixes cross-platform via
  constants). The residual risk — a latent Linux-only clippy `-D warnings` hit
  (only checkable by an actual CI run; the key `cfg(not(windows))` branches
  were checked manually — gated correctly). No live engine run required
  (CI infrastructure, engine/memory/tools unaffected).
- Local gates on Windows are green: **1049 unit tests passed, 48 `#[ignore]`**,
  clippy `-D warnings`/fmt clean. Docs: [docs/history/release-engineering.md](../../docs/history/release-engineering.md).
- **Next:** stage 2 (`feat/versioning-changelog` — bump to `0.9.0`, `CHANGELOG.md`,
  version in help/logs), stages 3–4 (JSON/SQLite migrations, ADR 0006), stage 5
  (release pipeline), optional stage 6 (`cargo-deny`).

### Post-M9: release engineering — stage 2 (version 0.9.0 + CHANGELOG + showing the version) (done)
- **Stage 2** of the "release engineering" track
  ([docs/history/release-engineering.md](../../docs/history/release-engineering.md), branch
  `feat/versioning-changelog`): app versioning and the changelog.
- **Bump `0.1.0` → `0.9.0`** (`Cargo.toml` + `Cargo.lock`): a signal of "almost 1.0".
  `1.0.0` — once the track is proven in production (CI + migrations + a release
  pipeline), at which point "data survives upgrades" becomes a contractual promise (F1).
- **`CHANGELOG.md`** (Keep a Changelog 1.1, Russian): rubrics Added/Changed/
  Fixed/Removed/**Data** (storage formats and migrations get their own
  rubric)/Security; there's always an `[Unreleased]` section; comparison links to GitHub.
  Initial fill-in — the `[0.9.0]` section = a condensed retrospective of features by
  track (with a link to the CLAUDE.md journal for detailed history; the whole journal
  isn't carried over).
- **Showing the version**: `env!("CARGO_PKG_VERSION")` (compile-time, no
  parameter threading) in the **`F1` help overlay's title** (`⌨ Hotkeys · mindfork-rs
  v0.9.0`; the popup width isn't narrower than the title) and in the **startup log** (`tracing::info!`
  `version = …` next to `root = …`). `--version` already printed `CARGO_PKG_VERSION`.
- **Changelog discipline** codified in the process: a row in **AGENTS.md
  §4** (a user-visible effect / a data-format change → an entry in
  `[Unreleased]`) + a checklist item in the **PR template**.
- Pure infrastructure/UI — **no live engine run required**. Gates green;
  no tests exist for the help title's content (verified). Docs:
  [docs/history/release-engineering.md](../../docs/history/release-engineering.md).
- **Next:** stage 3 (`feat/json-schema-migrations` — a JSON version/migration framework,
  a downgrade guard, a pre-migration backup, ADR 0006), stage 4 (SQLite `user_version`),
  stage 5 (the `release.yml` release pipeline).

### Post-M9: release engineering — stage 3 (schema versions + JSON migrations) (done)
- **Stage 3** of the "release engineering" track
  ([docs/history/release-engineering.md](../../docs/history/release-engineering.md) §3.4, F7–F12; branch
  `feat/json-schema-migrations`): versioning the schemas of saved data and a migration
  framework so a binary upgrade never loses data. **ADR 0006**.
- **Per-artifact versions** (F7): `SETTINGS_SCHEMA`/`PROFILES_SCHEMA`/`CHAT_SCHEMA`/
  `DB_SCHEMA` (all = 1) — artifacts change at different rates, one global number
  would force "migrating" untouched files. `SETTINGS_SCHEMA` is pinned to
  `config::SCHEMA_VERSION` by a test (won't drift apart).
- **The framework — `shared/storage/schema.rs`** (pure, Value-level): types `Step`/
  `JsonArtifact`/`Assessment`, version detection is **structural** (no version field → 1,
  so existing files aren't rewritten), `assess`/`apply_steps`. The registry —
  `settings/profiles/chat_artifact()` (all current=1, empty steps; the engine is "in production" with
  empty migrations). **Orchestration — `features/data_migration.rs`** (not in `shared`:
  the pre-migration backup is `features::backup`, and `shared` can't depend on
  `features`, FSD): reads files as `Value` → gates → backup → apply+control-parse+
  atomic write. `main.rs` calls `data_migration::run` before opening storage (TUI +
  the CLI `import`). A deviation from the design doc (there everything lived in `schema.rs` inside
  `Storage::open`) — for FSD and zero churn at `Storage::open`'s test sites; recorded
  in ADR 0006.
- **Downgrade guard** (F10): data from a newer app version (`detect > current`)
  → **the app refuses to start**, with a localized message (not "read it best-effort").
- **Hardened reads** (F11): a corrupt `settings.json`/`profiles.json` → the app refuses to start
  (previously — silent defaults that then overwrote the `.bak`); a corrupt `chats/<id>.json`
  → skipped with a `warn`, the file untouched (previously one corrupt file crashed the
  entire startup — hardcoded in `json.rs::load_chats`).
- **Pre-migration backup** (F9): a non-empty plan → one backup (reusing
  `backup::create_backup` → `backups/pre-migrate-<date>.zip`) before any write; a backup
  failure → the migration doesn't start. Dormant while every schema stays at 1.
- **Bump policy** codified (F12, `schema.rs` + AGENTS.md §4): additive — no
  bump (as before, `#[serde(default)]`); breaking — a bump + a step + a golden fixture +
  a CHANGELOG entry (the "Data" rubric).
- **Localization**: 5 `migrate.err.*` keys (ru+en); migration logs stay Russian (file
  logs, not localized, i18n §2.3). **Tests**: the framework against a synthetic artifact at current=2
  (assess/apply/downgrade/step-fail); the orchestrator against a tempdir (a no-op with no backup,
  downgrade→refusal, a corrupt settings→refusal, a corrupt chat→skip, a live migration+backup+write).
  **1060 unit tests green** (+11), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
  No live engine run required (storage infrastructure); **a manual run on a copy of
  real data (226+ chats) is a recommended pre-merge step**. Docs: ADR 0006,
  spec §5.2/§12, architecture §7.
- **Next:** stage 4 (`feat/db-schema-migrations` — `PRAGMA user_version` + steps in
  transactions, a shared pre-migrate moment with JSON), stage 5 (the release pipeline).

### Post-M9: release engineering — stage 4 (SQLite schema migrations) (done)
- **Stage 4** of the "release engineering" track
  ([docs/history/release-engineering.md](../../docs/history/release-engineering.md) §3.4; branch
  `feat/db-schema-migrations`): versioning and migrating the SQLite schema on top of
  stage 3's framework. **ADR 0006** (extended).
- **`PRAGMA user_version`** as the DB schema version (`DB_SCHEMA = 1`). `db/mod.rs::migrate`
  rewritten to be version-aware: (1) **additive DDL** (`CREATE … IF NOT EXISTS`, extracted into
  `baseline_ddl`) runs **every time** — this is the mechanism for adding tables/indexes to
  an existing DB without a bump (the additive policy from F12); (2) a fresh/existing DB
  (`user_version = 0`) gets stamped with baseline version **1** (not a data migration — no backup
  needed); (3) breaking **steps** (`DB_STEPS`, empty for now) are run by the runner
  `apply_db_steps` — **each in its own transaction along with the `user_version` update**,
  rolled back entirely on failure; (4) downgrade (`user_version > DB_SCHEMA`) — a protective `bail`.
- **A shared pre-migrate moment for JSON+SQLite** (no second backup): `data_migration::run`
  peeks at `db::peek_user_version` before opening storage (a downgrade guard with
  a localized message, the `data.db` file) and factors in `db::needs_step_migration` when
  deciding on a **shared** backup; the actual DB migration runs later in `Db::open`. **The SQLite backup
  API isn't needed**: at backup time the DB isn't open yet (quiescent), so zipping its files
  (`data.db`+`-wal`+`-shm`, already in `create_backup`) is consistent. A deviation from the design doc
  (which suggested `rusqlite::backup`) — justified by the quiescent state.
- **Existing DBs** (today at `user_version = 0`) get stamped with 1 on the first run
  (no backup/messages — data untouched). `DB_STEPS` is empty → no real DB migrations/
  backups yet (dormant, like JSON).
- **Tests**: baseline stamps a fresh DB with 1 + idempotency of a repeated `migrate`;
  downgrade → `bail`; `peek_user_version` (no file → 0); `needs_step_migration` (empty);
  `apply_db_steps` — a successful step commits (+a table, version 2) and a failing one **fully
  rolls back** (no table, version unchanged); `run` → refusal when the DB's `user_version`
  is from the future. **1067 unit tests green** (+7), 48 `#[ignore]`, clippy `-D warnings`/fmt
  clean. No live engine run required; **a manual run on a copy of a real `data.db`
  (stamping 0→1) is a recommended pre-merge step**. Docs: ADR 0006, spec §12.2,
  architecture §6/§7.
- **Next:** stage 5 (`feat/release-pipeline` — `release.yml` on a tag, archives+sha256,
  a schema-version manifest in the backup zip, AGENTS.md §6 "Release"), optional stage 6 (`cargo-deny`).

### Post-M9: release engineering — stage 5 (release pipeline + backup manifest) (done)
- **The final stage** of the "release engineering" track
  ([docs/history/release-engineering.md](../../docs/history/release-engineering.md) §3.5; branch
  `feat/release-pipeline`). The track is complete (stages 1–5; optional stage 6 — `cargo-deny`).
- **`.github/workflows/release.yml`** (trigger — a `v*` tag): the `build` job (a matrix of
  `windows-latest` + **`ubuntu-22.04`** — old glibc 2.35) builds `cargo build
  --release --locked` and uploads the binary as an artifact; the `release` job (ubuntu, `contents:
  write`) downloads both, **packs on Linux** (which has both `tar` and `zip` — avoiding
  Windows shell differences) into `mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}`
  (the binary + README/CHANGELOG/LICENSE/install), computes `sha256sums.txt`, extracts
  the notes = the `[X.Y.Z]` section of the CHANGELOG (`awk`), `gh release create --verify-tag`.
- **A schema-version manifest in the backup zip** (`features/backup.rs`): `BackupManifest`
  (`app_version` + `SchemaVersions{settings,profiles,chat,db}` + `created_at`) is written as the
  entry `manifest.json` in every archive (`create_backup`); on unpacking it's **not
  extracted** into the root (it's metadata, not data — `extract_archive` skips it).
  `read_manifest` reads it (None — an old backup). `mindfork restore` warns
  (localized, `backup.warn.newer_manifest`) when `is_newer_than_current` — a copy from
  a newer app version (data intact; the startup downgrade guard still
  protects it).
- **AGENTS.md §6 "Release"**: a release checklist (a release PR bump+CHANGELOG → merge → the
  user tags → `release.yml` → an artifact smoke) + the condition for promoting to `1.0.0`.
- **Tests**: the manifest in the archive + `read_manifest` round-trip; `is_newer_than_current`;
  None for an archive with no manifest; restore doesn't extract the manifest into the root. **1071
  unit tests green** (+4), 48 `#[ignore]`, clippy `-D warnings`/fmt clean. Both workflow
  YAMLs valid. `release.yml` is only checkable by an actual tag (doesn't trigger on a PR)
  — **the first `v0.9.0` tag after merging** will be its live run + an artifact
  smoke. Docs: CHANGELOG, AGENTS §6, install.md.
- **Next (groundwork, out of the track):** the first `v0.9.0` tag; promotion to `1.0.0`; optional
  `cargo-deny` (stage 6); self-update/installers/musl — see release-engineering.md §5.

### Post-M9: release engineering — stage 6 (cargo-deny) + dictionaries in the release (done)
- **Optional stage 6** of the "release engineering" track (F6) + a release
  pipeline tweak; branch `feat/cargo-deny-release-dicts`.
- **`cargo-deny` dependency audit**: `deny.toml` (advisories/licenses/bans/sources)
  + `.github/workflows/audit.yml` (**weekly `cron` + `workflow_dispatch`, NOT on
  `pull_request`** → doesn't block merges, advisory mode). `EmbarkStudios/cargo-deny-action@v2`.
  Run locally (`cargo-deny 0.20.2`): licenses/bans/sources — ok (the license
  allowlist checked against the dependency graph); of the advisories, **`anyhow` (RUSTSEC-2026-0190, an unsound
  `Error::downcast_mut`) — a direct dependency, fixed by bumping `1.0.102 → 1.0.103`**
  (the floor in Cargo.toml raised); 4 unfixable transitive ones via `syntect`
  (yaml-rust/bincode unmaintained, quick-xml ×2 DoS — RUSTSEC-2024-0320/2025-0141/
  2026-0194/2026-0195) added to `ignore` with a rationale (syntect parses **its own
  built-in** syntaxes/themes, not user input → the risk doesn't materialize; revisit on a
  syntect upgrade). Principle: fix what we control, document what we ignore among the
  unfixable transitive ones. `cargo deny check` green.
- **Spellcheck dictionaries in the release**: `release.yml` puts `dictionaries/*.aff`+`*.dic`
  (`en_US`/`en_GB`/`ru_RU`, ~5 MB) into the archive as **`data/dictionaries/`** — a portable
  layout the app reads next to its binary → **spellcheck out of the box** in
  the release too (not just in dev via `build.rs`). Kicks in from the **next**
  tag onward (the published `v0.9.0` doesn't include the dictionaries — re-release if desired).
- **Clarified stale comments**: the dictionaries have **been in the repo for a while**
  (6 files checked in, no `.gitignore` entry) — comments fixed in `build.rs`, CLAUDE.md (the entry
  below), AGENTS.md §6 (archive contents). The old "not part of the repo (.gitignore)"
  comments were stale info.
- Pure infrastructure/docs + a patch-level dependency bump — **no live engine run
  required**. **1071 unit tests green** (anyhow 1.0.103 API-compatible), 48
  `#[ignore]`, clippy `-D warnings`/fmt/`cargo deny check` clean. The `audit.yml` YAML
  valid (doesn't trigger on a PR — runs via `workflow_dispatch`/`cron` after a merge).
- **The "release engineering" track is fully closed** (stages 1–6 + the v0.9.0 release +
  the plan in `docs/history/`).

### Post-M9: installers — research + stage 1 (code prerequisites) (done)
- **A new "installers" track** (the order: Windows msi/exe + Linux deb/rpm/
  pkg.tar.zst; at install time — choosing the interface language and the data
  location, see `defaults.json`; code signing left open). **Research** —
  [docs/history/installers.md](../../docs/history/installers.md) (three parallel web surveys
  based on primary sources, July 2026): Windows — **Inno Setup 6.7.x (exe)**, not MSI (every
  "special" requirement is a stock `CreateInputOptionPage`/`CreateInputDirPage`/
  `SaveStringsToUTF8File`/the official `Russian.isl`; MSI would be days-to-weeks of workarounds);
  Linux — **nfpm** (one YAML → all three formats), a layout of `/usr/lib/mindfork-rs/` +
  a symlink at `/usr/bin` (on Linux `current_exe` resolves a symlink to the real path — the app's
  path resolution works **with no code changes**); deb/rpm/pacman are non-interactive → on
  Linux the app determines the language from the OS locale. **Code signing**: without signing,
  SmartScreen reputation resets with every release, and even EV no longer gives instant reputation;
  options are — SignPath Foundation (free, public OSS), Certum Open Source
  (~€69/€29, for individuals), Azure Artifact Signing ($9.99/mo, restricted geography).
  **Decision points R1–R9 confirmed by the user 2026-07-16 per the recommendations; signing (R8)
  deferred** — the repo is private, there's no site/logo/icon (revisit when
  preparing for a public launch). Research branch — `docs/installers-research`.
- **Stage 1 "code prerequisites"** (branch `feat/installed-mode-prereqs`) — three additive
  changes preparing the app for an installed (not portable) look; no migrations
  (`#[serde(default)]`), engine/memory unaffected.
  - **P1 — a dictionary fallback next to the binary.** `dict::load` gained a parameter
    `bundled_dir: Option<&Path>`: dictionaries are looked up first in the data root (`dict_dir`),
    then in the portable layout `exe_dir/data/dictionaries` (`Paths::
    bundled_dictionaries_dir`, from a new field `Paths.exe_dir`). A pair already loaded
    from the root **isn't** reloaded from bundled (`loaded: HashSet` by base name —
    a user dictionary of the same name wins); in portable mode bundled ==
    dict_dir → the second pass is a no-op. Before, with `mode=system`/`path`, there were no
    dictionaries in the data root → spellcheck silently disabled; now bundled picks them up
    (where the installer/package puts them). Shared logic factored into `load_dir(dir, selected, loaded,
    dicts)`. Threaded through `SpellLoader`/`runtime::run`/`run_loop` → `main.rs`
    (`paths.bundled_dictionaries_dir()`).
  - **P2 — auto-detecting the language from the OS locale.** `Defaults.default_language:
    Lang` → `Option<Lang>` (`#[serde(default)]` → None when the field is absent; a deb/rpm
    package writes only `{"mode":"system"}`). `Paths::resolve` resolves
    `default_language.unwrap_or_else(i18n::detect_os_language)`; `detect_os_language`
    delegates to the pure `lang_for_locale(Option<&str>)` (the primary subtag `ru*`→Ru, else
    En; testable) over `sys_locale::get_locale()` (a new dependency `sys-locale`,
    cross-platform, pure Rust). Resolved **before** CLI parsing (the peek phase), so the
    language is always concrete → `cli_lang` simplified to `settings.unwrap_or(default_language)`
    (`defaults_present`/`Defaults::marker_present` removed, `resolve() -> Self` with no
    bool). Only changes fresh installs' behavior (previously — always Ru when the field is
    absent). Closes the roadmap groundwork item "Detect language from the OS locale".
  - **P3 — `defaults.json` tolerates a UTF-8 BOM** (`strip_bom` before the whitespace check
    and parsing — precedents `rag_ingest::read_text`, the LameLLaMA importer). An installer/
    editor could write the file with a BOM → the app would crash on startup.
- **Tests**: paths (an Option language with the field present/absent/a legacy fallback; BOM;
  `bundled_dictionaries_dir` None in `with_root`); dict (bundled supplies
  missing ones; a root dictionary wins over bundled with no duplicate); i18n
  (`lang_for_locale` — subtags/None); main (`cli_lang` — settings ∨ resolved).
  **1077 unit tests green** (+6), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
  **Live run**: engine/memory unaffected; against the real binary confirmed the
  peek phase (`--version`/`--help` localized), **BOM tolerance** (`defaults.json` with a
  BOM + `mode:system` → starts fine), and refusal on a corrupt `defaults.json` (exit code 1).
  A full TUI dictionary smoke is interactive (needs a terminal); the logic is covered
  by the unit test `bundled_dir_supplies_missing_dictionaries`.
- **Next:** stage 2 `feat/linux-packages` (nfpm → deb/rpm/archlinux + a CI install
  smoke), stage 3 `feat/windows-installer` (Inno Setup), optional stage 4 "Signing"
  (deferred). Playbook/layout/DoD — `docs/history/installers.md` §8.

### Post-M9: installers — stage 2 (Linux packages deb/rpm/pkg.tar.zst) (done)
- **Stage 2** of the "installers" track (`docs/history/installers.md` §4, §8; branch
  `feat/linux-packages`, **stacked on `feat/installed-mode-prereqs`** — the packages put
  dictionaries at `/usr/lib/mindfork-rs/data/dictionaries`, found via P1's fallback from
  stage 1). Packaging/CI only — the app's source code untouched (zero increase in
  unit tests, engine/memory unaffected).
- **`packaging/nfpm.yaml`** — one nfpm config → **three formats** (`--packager
  deb|rpm|archlinux`; nfpm handles all three, cargo Arch tooling doesn't cover it).
  Layout §4.2: the real binary at `/usr/lib/mindfork-rs/mindfork-rs` + `defaults.json`
  (`{"mode":"system"}`, `type: config|noreplace` → an edit survives an upgrade) + dictionaries
  at `/usr/lib/mindfork-rs/data/dictionaries/` + a **symlink** at `/usr/bin/mindfork-rs`
  (`type: symlink`) + docs at `/usr/share/doc/mindfork-rs/`. `defaults.json` can't
  go in `/usr/bin` (FHS); the symlink layout works **with no code changes** —
  `current_exe()` on Linux resolves `/proc/self/exe` to the real path, the app
  finds the neighboring `defaults.json`/dictionaries. Dependencies set by hand (nfpm doesn't compute them):
  deb — `libc6 (>= 2.35)`, rpm/arch — nothing (glibc is in the base; the stack is rustls +
  bundled SQLite + x11rb).
- **`packaging/linux/build-packages.sh`** — stages the binary into `dist/stage/` and runs
  nfpm three times with the conventional names (`mindfork-rs_X.Y.Z-1_amd64.deb`,
  `-X.Y.Z-1.x86_64.rpm`, `-X.Y.Z-1-x86_64.pkg.tar.zst`). One shared script for CI and local use.
- **CI**: (1) a new **`packaging.yml`** (on `pull_request`/`push:main`, touching
  `packaging/**`, + `workflow_dispatch`): the `build` job (cargo build --release → nfpm
  via the goreleaser apt repo → 3 packages as an artifact) + the `smoke` job (a matrix of
  containers `ubuntu:24.04`/`fedora:latest`/`archlinux:latest`: installs the package via its own
  package manager, checks the layout/symlink/`defaults.json`, runs `mindfork-rs
  --version` **as a regular user** — the peek phase creates no directories). This is
  the CI install smoke (validated on a PR — the only way to check nfpm/
  containers since development happens on Windows). (2) **`release.yml`** gained the job
  `linux-packages` (from the ready `bin-linux` via the same script) + `release` now
  `needs: [build, linux-packages]` and puts the packages into `dist/` (they end up in
  `sha256sums.txt` and the GitHub Release).
- **The idiomatic Arch path — groundwork**: `.pkg.tar.zst` on Releases for `pacman -U` exists; an AUR
  `mindfork-rs-bin` (PKGBUILD + .SRCINFO from Releases) — a separate small step after
  the first package release.
- **Checks**: `cargo fmt/clippy/test` unaffected (no code changes; **1077 unit tests**
  same as stage 1); the nfpm/both workflow YAML valid (structure/types `config|noreplace`/
  `symlink`/paths verified). **Live run**: nfpm and the container installs **can't be
  reproduced locally on Windows** — `packaging.yml` runs them on a PR (building packages +
  an install smoke across three distros); that's the stage's DoD verification.
- **Next:** stage 3 `feat/windows-installer` (Inno Setup), optional stage 4 "Signing"
  (deferred by the user).

### Post-M9: installers — stage 3 (Windows installer, Inno Setup) (done)
- **Stage 3** of the "installers" track (`docs/history/installers.md` §3.3, §8; branch
  `feat/windows-installer`, **linear stack on `feat/linux-packages`** — both stages
  edit `release.yml`, the linear `1→2→3` chain avoids a file conflict; builds
  on P1/P3 from stage 1: writes `defaults.json` with a UTF-8 BOM and puts dictionaries next to the binary).
  Packaging/CI only — the app's source code untouched.
- **`packaging/windows/mindfork.iss`** — Inno Setup 6.7.x, exe format (not MSI: every
  requirement is a stock Inno feature). Two **custom wizard pages**:
  "Application language" (radio buttons Russian/English, `CreateInputOptionPage(Exclusive)`) and
  "Data location" (radio buttons: the standard OS folder / portable / a custom folder
  via `CreateInputDirPage`). The choice is written to `{app}\defaults.json` at `ssPostInstall`
  (`{"mode":…,"default_language":…}`, `SaveStringsToUTF8File` — with a BOM, dropped by P3;
  Cyrillic paths in the JSON are escaped). **Not overwritten on upgrade**
  (`if not FileExists` + `ShouldSkipPage` hides both pages if `defaults.json`
  already exists). A bilingual UI: `[Languages]` en+`Russian.isl` (the official one), page
  text via `[CustomMessages]` with `ru.`/`en.` + `CustomMessage()`. Per-user, no UAC
  (`PrivilegesRequired=lowest` + `…OverridesAllowed=dialog`, `{autopf}`→
  `%LOCALAPPDATA%\Programs`); the portable option is hidden when installing per-machine
  (in Program Files you can't write data next to the exe). Dictionaries — at `{app}\data\
  dictionaries` (reserved by P1). The uninstaller cleans up only `defaults.json` +
  the installed files; user data (`%APPDATA%`/portable) is untouched.
  **The file is saved as UTF-8 with a BOM** — otherwise Inno on an en-US runner corrupts the
  Cyrillic.
- **CI**: (1) `packaging.yml` gained a `windows-installer` job (a Windows runner):
  installs Inno via choco, **compiles the `.iss` with a stub binary** — validates the
  `.iss` syntax and the Pascal `[Code]` on a PR (a real build isn't done here; locating
  `ISCC.exe` is resilient to Inno 6/7's version/path). (2) `release.yml` gained a job
  `windows-installer` (compiles the real `setup.exe` from the ready `bin-windows`);
  `release` now `needs: [build, linux-packages, windows-installer]` and puts
  the installer into `dist/` (→ `sha256sums.txt` + the GitHub Release).
- **Fix for the `{app}` crash in `ShouldSkipPage` (found via a live GUI run).** The first version
  checked for an upgrade via `ExpandConstant('{app}\defaults.json')` — but `{app}` isn't
  initialized yet at the wizard-page-display stage, and expanding it crashed **any
  interactive install** into a "Runtime error: An attempt was made to expand the
  "app" constant before it was initialized" dialog. A silent install **didn't** catch this
  (`ShouldSkipPage` isn't called during it; `CurStepChanged`/`ssPostInstall` uses
  `{app}` correctly by then) — so the bug only showed up on the GUI path. Fix: the path via
  `AddBackslash(WizardDirValue) + 'defaults.json'` (the directory field's current value,
  valid during the wizard). Lesson: **a silent install doesn't substitute for a live GUI
  run** for Inno `[Code]` that depends on `{app}`/pages.
- **Checks**: no Rust code → **1077 unit tests** as in stages 1–2, fmt clean;
  both workflow YAMLs valid. **Live run — GO** (a real **Inno Setup 6.7.3** on
  the dev machine): (1) the `.iss` **compiles** (`ISCC.exe` — the Pascal `[Code]`, all
  `[Files]`, `SourcePath` paths, Cyrillic) → `setup.exe`; (2) **the GUI wizard launches
  with no crash** — the `TWizardForm` window "Setup - mindfork-rs version 0.9.0", no Runtime
  error dialog (before the fix, that's the only thing that appeared); (3) **a silent install**
  (`/VERYSILENT /CURRENTUSER /LANG=en`) writes `defaults.json` via the wizard code — exactly
  `{"mode":"system","default_language":"en"}` (BOM `EF BB BF`, dropped by P3);
  (4) **the "custom folder" mode (`mode:path`)** verified by exercising the same `[Code]`
  (`DataPage`=custom + a Cyrillic path) → `{"mode":"path","path":"C:\\Users\\…\\
  <cyrillic-folder>\\sub","default_language":"ru"}` — **the `\`→`\\` escape** (`JsonEscape`),
  **Cyrillic** (UTF-8), `/LANG=ru`→`"ru"`; (5) **the installed binary reads** both
  `defaults.json` variants (`--version`→`0.9.0`, exit 0 → the JSON is valid, the `system`/`path`
  modes resolve — a full installer↔app round trip); (6) **an upgrade doesn't
  overwrite** `defaults.json` (a repeat install with `/LANG=ru` on top → the file stayed
  `…"default_language":"en"`); (7) **a clean uninstall** (files, the HKCU
  Uninstall registry entry, the shortcut). All test artifacts removed. (8) **A live GUI wizard + screenshots**
  (`Graphics.CopyFromScreen` over the `TWizardForm` rect): both custom pages render
  correctly in **both locales** — "Application language" (radio buttons
  Russian/English) and "Data location" (system/portable/custom
  folder), the title "Setup - mindfork-rs version 0.9.0". **Clicking GUI controls wasn't
  automated**: Inno's controls (custom VCL) aren't in the UI Automation tree, and Win32
  `SendMessage` blocks synchronously — navigation went via the Enter key (the default
  button), the `mode:path` choice was verified by exercising `[Code]` via a silent install.
- **The "installers" track — complete** (stages 1–3: research + code prerequisites +
  Linux packages + the Windows installer; the plan moved to `docs/history/installers.md`).
  Optional stage 4 "Signing" deferred by the user (private repo, no site/icon). Groundwork:
  a winget manifest (portable zip until signing), an AUR `mindfork-rs-bin`, an MSI for GPO/Intune
  if demand arises.

### Release 0.9.1 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.9.1`): bumped `Cargo.toml` `0.9.0 → 0.9.1` (+
  `Cargo.lock`), `CHANGELOG.md` — `[Unreleased]` → `[0.9.1] — 2026-07-18`, a
  fresh empty `[Unreleased]` opened, comparison links updated. The `v0.9.1`
  tag is applied by the user after the merge (the agent doesn't push
  tags/`main`).
- **Packages and the installer were already wired into the release** (the
  "installers" track, stages 2–3): `release.yml` contains the
  `linux-packages` (nfpm → deb/rpm/pkg.tar.zst) and `windows-installer` (Inno
  Setup → `setup.exe`) jobs, both listed in the publishing job's `needs`,
  artifacts are copied into `dist/` and end up in `sha256sums.txt` and the
  GitHub Release. No pipeline code changes were needed — only the endpoints
  were cross-checked: the `.iss` writes to `dist/*.exe` (matching the upload
  path) and accepts `/DAppVersion`; `build-packages.sh` +
  `packaging/nfpm.yaml` are in place. **v0.9.1 is the first tag that actually
  carries the packages/installer**: they landed in the pipeline only after
  `v0.9.0` (the published 0.9.0 doesn't include them, nor the spellcheck
  dictionaries).
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D
  warnings` / `cargo test` green — **1157 unit tests, 53 `#[ignore]`**. No
  live run needed (version + docs, app code untouched). Along the way: within
  `[0.9.1]` two duplicate "Fixed" sections were merged and the section order
  brought in line with what's declared in the CHANGELOG header; the test
  count and the date in the "## Status" header were refreshed (previously
  1128/50 from 2026-07-17).

### Post-M9: branding — logo and wordmark (stages 1–4) (done)
- **A new "branding" track** (design doc [docs/branding.md](../../docs/branding.md)):
  bring the `assets/` directory to full-fledged branding — portable assets,
  a brand guide, a packaging icon, a TUI logo, a wordmark in the docs.
  Forks R1–R7 confirmed by the user on 2026-07-18: **R2** — the logo in the
  help overlay's header (`F1`), **R3** — leave the interface palette
  **untouched** (`accent` carries the meaning "activity"; the logo is drawn
  in the brand colors), **R4** — `.desktop` with `Terminal=true`; R5–R7 per
  the recommendation (no CLI banner, `assets/` is the single source,
  AppStream — future work).
- **Stage 1 `feat/brand-assets`** — assets committed to git + a wordmark
  repair. **Key finding**: all six `mindfork-wordmark*.svg` carried
  `<text class="wm">` **with no `<style>`, `font-family`, or `font-size`**
  (the classes were never defined anywhere — the stylesheet block was lost
  during export), i.e. they rendered in a default serif font. Adding
  `font-family` wouldn't have helped: a viewer (GitHub, someone else's
  browser, a Linux viewer) doesn't have the needed monospace font. **The
  text was converted to outlines** (`<path>`) — the SVGs are self-contained.
  The typeface was reconstructed from the reference `wordmark-example.png`
  by **fitting metrics** (letter boundaries → font metrics, least squares):
  **JetBrains Mono ExtraBold** (SIL OFL 1.1), size 277.7 px, tracking
  −0.035 em. The reconstruction matches the reference **pixel-for-pixel** —
  word width 1249 px and the "icon → text" gap 213 px in both, individual
  letter widths diverging ≤3 px out of ~140 (anti-aliasing); a confirming
  detail — the descender of `o`/`d` (−10 font units) predicts their bottom
  edge exactly at the measured 348 px. The font was found locally (bundled
  with PyCharm) — no download needed; the `.ttf` isn't checked into the repo
  (needed only for regeneration). Added `assets/build-wordmarks.py` (a
  generator; the palette, tracking, and lockup proportions are constants)
  and `assets/README.md` (a brand guide: palette, glyph geometry, metrics,
  usage rules). The `viewBox` values were tightened to the actual content
  (`219.9×48` instead of `340×80` etc.) — the previous values assumed a size
  twice the reference and never rendered correctly; the vertical lockup was
  redrawn (word width = 2× the icon — no reference exists for it, and at the
  horizontal proportions the composition was bottom-heavy). The wordmark was
  placed in the README header (`<picture>` + `prefers-color-scheme`,
  falling back to the light variant; wrapped in an `<h1>` with `alt`,
  keeping the heading accessible).
- **Stage 2 `feat/windows-icon`** — the `.exe` icon via the **`winresource`**
  crate (a fork of the abandoned `winres`) from a ready-made `mindfork.ico`.
  **A double gate in `build.rs` is required**: the crate is declared under
  `[target.'cfg(windows)'.build-dependencies]`, and for **build**
  dependencies `cfg` is evaluated by the **host** — on a Linux host the
  crate is absent, and referencing it wouldn't compile; hence
  `#[cfg(windows)]` gating by host (is the crate present) **plus**
  `CARGO_CFG_TARGET_OS` by target (is the icon needed). Consequence:
  cross-compiling Linux → Windows won't embed the icon — a `cargo:warning`
  is printed there (the normal path is unaffected: the release workflow
  builds Windows on a windows runner). A failure to embed doesn't break the
  build (`cargo:warning`) — the app must still build on a machine without
  the Windows SDK. `SetupIconFile` and `WizardSmallImageFile` were added to
  `.iss` (`assets/mindfork-wizard-small.png`, 138×140 — the size for the
  modern style, a white background under Inno's white header);
  `UninstallDisplayIcon` and the `[Icons]` shortcuts picked up the icon on
  their own.
- **Verified live**: `rc.exe` from the Windows SDK was found, the build
  produced no warnings; the icon was **extracted** from the built `.exe` and
  from `setup.exe` — both identical, carrying exactly the brand colors
  `#09090b`/`#5c6370`/`#c25a27`; the `.iss` compiled; the wizard was
  screenshotted (icon in the title bar + logo in the page header; the
  install was **not** actually run). **The worry "Inno 6 only accepts BMP"
  wasn't confirmed** — a test compile of both formats succeeded, PNG was
  chosen (35× lighter: 1.6 KB vs. 58 KB). `cargo deny check` —
  `advisories/bans/licenses/sources ok` (winresource pulls in 7 build-only
  crates: the toml stack + winnow). **1157 unit tests green** (count
  unchanged — the crate's own code untouched), 53 `#[ignore]`, clippy/fmt
  clean.
- **Stage 3 `feat/linux-desktop-entry`** — an application-menu entry and
  theme icons: `packaging/linux/mindfork-rs.desktop` (**`Terminal=true` is
  mandatory** — the app is a TUI, stdout is occupied by the interface;
  without it, launching from the menu would close the window instantly;
  localized `GenericName`/`Comment`/`Keywords` for ru) + `nfpm.yaml`:
  `.desktop` into `/usr/share/applications`, six raster icons into
  `hicolor/<N>x<N>/apps` and an SVG into `hicolor/scalable/apps` — all under
  the name `mindfork-rs` (must match the `Icon=` key). The `packaging.yml`
  smoke was extended with checks for the presence/content of `.desktop` and
  all icons; `desktop-file-utils` is installed on Ubuntu/Fedora so the stock
  `desktop-file-validate` **actually runs**, rather than being skipped by a
  `command -v` check. `Chat` is **deliberately absent** from the categories
  — per the spec it requires a primary `Network` category, otherwise the
  validator warns. No cache-refresh scriptlets were added: Debian (the
  `update-icon-caches` trigger) and Fedora (a file trigger on `hicolor`)
  handle it themselves. Checked as far as possible locally on Windows (the
  YAML is valid, every `src` path in `contents` exists, `.desktop` is UTF-8
  without a BOM, LF-only, a primary category is present); actual `nfpm`/
  installs in containers aren't reproducible on Windows — validated by
  `packaging.yml` on the PR.
- **Stage 4 `feat/tui-logo`** — a logo drawn in terminal cells. New
  `widgets/logo.rs`: a pair of vertical pixels is encoded as one cell (both
  halves the same color — `█`; different — `▀` with `fg`/`bg`; one — `▀`/`▄`
  **with no background**, so as not to drag in a solid backdrop). The size —
  bounded **by the ink** (x 3…13, y 2…14 → 10×12 pixels = **10 columns × 6
  rows**; the height is even, so half-blocks line up exactly), rather than
  the full 16×16 grid with empty margins. Colors — **fixed brand RGB, not
  from the palette** (R3): the logo isn't retinted by the theme. The glyphs
  `█`/`▀`/`▄` fall within WGL4 → conhost compatibility mode needs no
  separate substitution (like `█` for the scrollbar and `▌` for the rails).
  Placement — as the first content block of the help overlay (`F1`, whose
  title already carries the version → de facto an "About" screen, so no new
  key was needed), but **only when there's spare height**: the hotkey list
  is long (33 entries, a popup ~35 rows), so when there's no room the logo
  isn't drawn at all — the hotkeys aren't pushed around and no extra scroll
  appears (the same degradation as the scrollbar and Mermaid).
- **A "code ≡ asset" gate**: a test parses the `<rect>` elements from
  `assets/mindfork-icon-transparent.svg` and cross-checks them against the
  `GLYPH` table in the code — the asset and the code won't silently drift
  apart (the technique mirrors i18n key-parity and `LABEL_CAP`).
  **Mutation-tested**: editing one constant fails the test with a clear
  message. **1164 unit tests green** (+7: 5 for the widget — including "only
  WGL4 glyphs" and "every row has a stem" — and 2 for showing/hiding in the
  help screen), 53 `#[ignore]`, clippy `-D warnings`/fmt clean. No live run
  needed (pure UI, `TestBackend`).
- **The "branding" track (stages 1–4) is complete.** Future work: a large
  `WizardImageFile` for the installer's "Finished" page, AppStream metainfo
  (R7), a CLI ASCII banner (R5 — decided against).

### Post-M9: branding — TUI wordmark, a lockup instead of a bare glyph (done)
- **A refinement of stage 4 based on user feedback** (branch
  `feat/tui-lockup`): the help overlay's header (`F1`) had a **centered glyph
  with no word and no top breathing room** — it read as a standalone image
  glued to the frame. Now there's a **horizontal lockup** (glyph + the word
  `mindfork`), left-aligned, with breathing room above and below.
  Assets/packaging untouched; changes only in `widgets/logo.rs` and
  `screens/chat/popups.rs`.
- **The wordmark is a custom pixel font, not the SVG.** Reusing
  `assets/mindfork-wordmark*.svg` isn't possible: the text there **has been
  converted to outlines** (stage 1, §2 of branding.md) — there's nothing to
  rasterize them with in the terminal. So `logo.rs` gained a `#`/`.` matrix
  per letter (`WORDMARK`, 8 glyphs for the word), drawn with the same
  half-blocks as the icon. The lettering mirrors the brand (JetBrains Mono
  ExtraBold): lowercase, descenders on `d`/`f`/`k`, **2-pixel strokes** — the
  same weight as the icon's bars (its grid also uses 2-unit bars). The
  result — 52 columns × 4 rows; the lockup as a whole — **65×6**.
- **Proportions taken from the brand metrics, not eyeballed**
  (`assets/README.md`): word height = `0.5227 × S`, where `S` is the
  **icon size** (16) → ≈ 8 pixels = 4 rows vs. 6; the baseline
  (`0.7418 × S`) → the word's bottom sits **one row above** the icon's
  bottom (`WORDMARK_TOP_ROW = 1`); the gap `0.3608 × S` is measured from the
  icon's **bounding box**, but we draw only the ink (x 3…13), so in the
  terminal it comes out to `0.3608 × 16 − 3` ≈ **3 columns**. Letter
  spacing — 1 column.
- **A fix from feedback on the first version** (too much air on the right;
  `i` twice as heavy as the other letters) — **one root cause**: the
  fraction `0.5227` was taken from the icon's **ink** height (12), not the
  icon size (16), so the word came out 6 pixels tall instead of 8. At 6
  pixels, 2-pixel strokes don't fit (an x-height of 4 leaves no gap inside
  `o`/`d`), and the font ended up effectively 1-pixel wide — thin next to
  the chunky glyph; `i` was the only letter with a proper 2-pixel stem,
  hence its "double width". Fixing the height to 8 unlocked the correct
  weight: the word became ExtraBold like the brand, and along the way grew
  from 48 to 65 columns — the air on the right shrank from ~26 to ~7 columns
  (against a popup width of ~76). Both complaints closed with one fix.
- **A second fix from feedback — `f`**: its hook (ascender height) and
  crossbar (x-height) sit right next to each other, with no separating row
  left in the budget — the 2-pixel hook merged with the crossbar into a
  solid block. The hook was made **1-pixel**: it lands in the top half of
  the cell (`▀`), the bottom half is empty → a gap shows, and the crossbar
  stays at x-height, on the same row as the top bars of `n`/`o`/`r` (an
  alternative — dropping the crossbar to the middle of the x-height — would
  have preserved the hook's weight but broken the overall baseline). `f`
  was widened 5 → 6 columns so the hook could be two columns wide (`▀▀`) and
  read clearly.
- **`mind`'s color comes from the palette, and this doesn't violate R3.**
  `fork` stays the fixed brand accent (`#c25a27`), while in the brand `mind`
  is "text on dark"/"text on light" (exactly why separate `-dark`/`-light`
  variants were set up), i.e. a color derived from the background. We take
  `palette.text` → a single lockup is correct in both themes, while the
  signature colors stay on-brand. The glyphs `█`/`▀`/`▄` are WGL4, so
  conhost compatibility mode still needs no separate substitution.
- **Left alignment** on the same margin as the hotkey list (two spaces),
  plus a blank line above (breathing room from the frame) and below. The
  mark reads as a block header and shares a vertical axis with the hotkeys.
  The block grew from `LOGO_ROWS + 1` = 7 to `LOCKUP_ROWS + 2` = 8 rows.
- **The show gate was extended to width**: previously only height was
  checked. The popup width under the lockup is **deliberately not
  stretched** (the popup is sized by the hotkey list) — when `width <
  LOCKUP_COLS + margin + padding`, the mark isn't drawn at all. The same
  hard degradation as the scrollbar and Mermaid. The order of computations
  in `render_help` was reordered: width first, then the decision on the
  mark (it depends on width), then the popup height.
- **The invariant "the word fits within the glyph's height" is a `const`
  assertion**, not a test: editing the font or `WORDMARK_TOP_ROW` in a way
  that would make `lockup_lines` silently clip the word's bottom rows
  **fails the build**. The "code ≡ SVG" gate for the icon (stage 4) stays
  intact.
- **Tests**: font integrity (the word is exactly `mindfork`, matrices are
  rectangular, no empty letters, no stray characters); word size and
  separate coloring of `mind`/`fork`; the lockup doesn't touch the icon (the
  first `LOGO_COLS` columns match byte-for-byte) and places the word
  **only** in its own 3 rows; `uses_only_wgl4_block_glyphs` extended to the
  lockup; the help-screen test rewritten from "there's orange somewhere" to
  positional — the stem sits exactly on the hotkey list's margin (i.e. the
  mark is left-aligned, not centered), "fork" is to the right of the icon.
  Test trap: `Span::content.len()` is **bytes**, and block glyphs are 3
  bytes wide (measure width via `chars().count()`); and `╭` appears in the
  buffer for the chat's own panels at column 0 — the popup's corner is
  taken as the rightmost one. **1167 unit tests green** (+3), 53
  `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live run needed** (pure UI without an engine/memory; covered by
  `TestBackend`). The composition was checked against a dump of the
  rendered popup in both locales (ru/en): the word reads correctly, the
  lockup fits within the popup width with margin (65+6 vs. ~78).

### Post-M9: the mindfork.io site URL in project metadata (done)
- **The domain `mindfork.io` was registered** (2026-07-26) for a future project
  site. It was already in the `F1` "About" dialog (`credits::SITE_URL`); this
  change carries it into the places that have a genuine **"project homepage"**
  slot, where the repository URL had been standing in. Branch
  `chore/release-0.9.4` (started as `chore/site-url`).
- **Where it went**: the Windows installer's `AppPublisherURL`
  (`packaging/windows/mindfork.iss` — surfaces in "Apps & features"); nfpm's
  `homepage` (→ deb `Homepage:` / rpm `URL:` / pacman `url`); the standard
  `homepage` field in `Cargo.toml` (was absent entirely); a README badge in the
  brand accent `#c25a27`; a header line in `docs/install.md` (that file ships
  inside the packages and the Windows install directory); and a footer on every
  GitHub Release page (`release.yml`).
- **Every actionable link deliberately stayed on GitHub** — the site does not
  exist yet, so a dead link must never be the only way to get help or a
  download. The installer's other two ARP links were **split** for exactly this
  reason: `AppSupportURL` → `/issues`, and a new `AppUpdatesURL` → `/releases`
  (both previously pointed at the bare repo root).
- **An audit of the repo URL found nothing else to convert**: all remaining
  occurrences are legitimately GitHub-specific (CHANGELOG `/compare/` links, the
  CI badge, the Releases download link, `Cargo.toml`'s `repository`,
  `credits::REPO_URL`, and — in a research doc — a *different* repo, the
  crossterm fork). The one homepage slot left is **outside the codebase**:
  GitHub's own repo "Website" field (`gh repo edit --homepage`), which is the
  user's to set.
- **The crate URL was deliberately NOT added anywhere user-facing.** The name
  `mindfork` is still free (verified against the registry API — 404), but the
  package is `mindfork-rs`, so claiming it is a packaging decision, not a URL
  edit: either a full rename (which renames the binary and ripples into the
  installer, nfpm layout, `.desktop` `Exec=`, the `/usr/bin` symlink, docs and
  CI artifact names) or `name = "mindfork"` + `[[bin]] name = "mindfork-rs"`.
  Until it is published, a crates.io version badge or a `cargo install mindfork`
  line would render **broken**; recorded as a roadmap item instead.
- **Release-notes footer** (`release.yml`): the body was just the CHANGELOG
  section; it now ends with the site + the install guide **pinned to the tag**
  (so instructions travel with the release they describe). Appended after the
  empty-notes fallback, so it is present on both paths; the repo URL is built
  from `GITHUB_SERVER_URL`/`GITHUB_REPOSITORY` rather than hardcoded. The step
  also `cat`s the composed notes into the job log.
- **Verification**: the `.iss` was **compiled** with real `ISCC.exe` 6.7.3 (an
  unknown directive is a compile error — that is the meaningful check for
  `AppUpdatesURL`); the release step was simulated against the real
  `CHANGELOG.md` on both the normal and the fallback path. **Gate green**: 1294
  unit tests, 59 `#[ignore]`, clippy/fmt/`cyrillic_scan` clean. No live engine
  run needed (metadata/docs only; the sole Rust change is a doc comment).
- **A false alarm worth recording**: the first simulation appeared to show the
  CHANGELOG extraction failing and falling back to a bare `Release vX.Y.Z.`
  — a would-be significant pre-existing bug. It was an artifact of the
  reproduction: a quoted heredoc collapsed `\\[` to `\[`, turning the awk regex
  into a character class. `od -c` against the real file showed the workflow has
  the correct double backslash, and re-running with bytes extracted verbatim
  from `release.yml` produced the section correctly. **The release notes were
  never broken.**

### Post-M9: the Windows installer can provision the Python sandbox (done)

- **Asked for directly**: an optional deployment of the Wasmer sandbox during
  installation — a checkbox on Windows, and "check whether the Linux packages can
  take such a parameter; if not, leave it as is". Branch
  `feat/installer-sandbox-option`. A simple task by AGENTS.md §1 (packaging
  configuration, no cross-layer contract, no new dependency), so no design doc —
  the two genuine forks went to the user instead (**decided 2026-08-07**, both as
  recommended: the "Additional tasks" page, and a failed download must not fail
  the install).
- **Nothing in the app changed, and that is the point**: `sandbox setup` has been
  a self-contained CLI command since ADR 0005 §4, and the installed binary is a
  console app — so the installer only has to launch it. A `[Tasks]` checkbox
  (`installsandbox`, `unchecked`) gates one `[Run]` entry. **No `runhidden`**: the
  console window Windows opens for a console binary *is* the progress display for a
  multi-minute download. **`runasoriginaluser`** matters for a per-machine install
  — Setup is elevated then, while `mode: system` resolves to a per-user data
  directory, so without it the sandbox would land in the elevating admin's
  `%APPDATA%`. A non-zero exit code of a `[Run]` entry is ignored by Inno, which is
  exactly the agreed failure behaviour: the sandbox is optional and re-runnable.
- **Left off by default deliberately**, matching `tools.python_enabled=false`
  (ADR 0005 §5): "enabled but not provisioned" is worse than "not installed", and
  enabling the tool stays a deliberate step. One consequence worth knowing — Inno
  **remembers the previous install's task selection** per AppId in the registry, so
  on an upgrade the box is pre-ticked for someone who chose it before. Correct and
  cheap: `sandbox setup` is idempotent and skips assets already present.
- **The measurement overturned my assumption, and it was load-bearing.** I had
  written into the script's comment that `[Run]` runs *after*
  `CurStepChanged(ssPostInstall)` — "verified, not assumed" — and then verified it,
  with a stub `mindfork-rs.exe` that logs whether `defaults.json` exists at the
  moment it is launched. It logged **`defaults_exists=false`** while the file was
  in the installed directory afterwards: Inno processes `[Run]` entries **before**
  `ssPostInstall`. Left alone, the sandbox would have been downloaded into the
  *default* data directory rather than the one picked on the "Data location" page —
  silently, since `sandbox setup` would succeed either way. Fixed by moving the
  write to **`AfterInstall: WriteDefaults`** on the binary's `[Files]` entry (files
  are necessarily installed before `[Run]` can launch one), which is a single call
  site rather than two; the procedure's body is unchanged and its
  `if not FileExists` guard already made it idempotent.
- **Two smaller traps, both found by compiling rather than by reading**: a `{ }`
  Pascal comment containing an app constant in braces **closes early** at that
  constant's `}` (syntax error — the block comment is now `//` lines, with the
  reason recorded); and a test path containing `"` is rejected by Inno's own
  directory-field validation, so `JsonEscape`'s quote-escaping is unreachable
  defence in depth (Windows forbids `"` in a path at all).
- **Linux — genuinely infeasible, for three independent reasons**, so the packages
  are unchanged: deb/rpm/pacman install **non-interactively** (no checkbox and no
  prompt to offer — debconf has no rpm/pacman counterpart); maintainer scripts run
  **as root** while `sandbox_dir()` is `<data root>/sandbox` and the packages ship
  `mode: system`, i.e. `~/.local/share/mindfork-rs/sandbox` — root cannot provision
  it for the installing user, and there is no system-wide fallback for the sandbox
  the way there is for dictionaries (P1); and downloading ~300 MB from a postinst
  is against packaging norms and breaks offline installs. An env-var "parameter"
  (`MINDFORK_INSTALL_SANDBOX=1 apt install …`) would clear only the first
  obstacle. Documented in install.md so the question isn't re-litigated.
- **Verified against the real Inno Setup 6.7.3** on this machine (no live model
  run is required by AGENTS.md §3 — packaging configuration, no engine, memory,
  tool or provider path is touched; there is no Rust change at all, so the suite
  stands unchanged at **1835 unit tests**, 76 `#[ignore]`). Since the change moved
  the *proven* `defaults.json` path, every previously-verified behaviour was
  re-confirmed through the new call site, with the stub binary standing in for the
  real one: the task selected → `[Run]` fires with `sandbox setup` and now sees the
  finished `defaults.json`; **no task → it does not run at all** (opt-in honoured);
  an upgrade → the existing `defaults.json` is preserved (still `ru` after
  re-installing with `/LANG=en`) and the entry runs once; `/LANG=ru` → the language
  branch writes `"ru"`; the "Another folder…" branch → `mode: path` with
  backslashes escaped and a Cyrillic path intact, **and the sandbox run saw that
  custom path**, which is the whole point of the ordering fix; uninstall → clean.
  The first "no task" run was a **false failure worth recording**: it reported the
  sandbox running unasked, because an earlier test install of the same AppId was
  never uninstalled and Inno restored its remembered task selection — the retest
  from a genuinely clean registry state passed.
- **The checkbox itself was looked at, not just asserted**: the real wizard was
  driven to "Select Additional Tasks" and screenshotted in **both** locales — the
  new box sits under "Create a desktop shortcut", unchecked, reading
  *Install the Python sandbox (downloads ~300 MB)* and its Russian counterpart
  (`SandboxTask` in `[CustomMessages]`). Automating that turned up a detail worth keeping for the
  next time: Inno 6 **disables the Welcome page by default**, so the tasks page is
  three pages in, not four, and `/CURRENTUSER` is needed to skip the install-mode
  dialog that `PrivilegesRequiredOverridesAllowed=dialog` puts first.
- **Deliberately not done**: removing `<data>/sandbox/` on uninstall. It sits
  inside the data root the uninstaller promises not to touch (in portable mode,
  right next to the user's chats), and it is re-downloadable rather than ours to
  delete — noted in the script's comment so the omission reads as a decision.
- **Follow-up asked for after the first review: the checkbox now also enables the
  tool** (`tools.python_enabled`), so ticking it gives a working `python_exec`
  rather than a provisioned sandbox the user then has to go and switch on. The
  label says both, since it changes a security-relevant setting. **ADR 0005 §5
  amended** (2026-08-07) — the default and the invariant are unchanged; what moved
  is *where* the deliberate act can be taken.
- **The installer deliberately does not write it.** `python_enabled` lives in
  `settings.json` — user data, in a data root whose location only `Paths::resolve`
  knows (system/portable/path) — so writing it from Pascal would mean
  re-implementing that resolution *and* risking an existing config. Instead the CLI
  grew `sandbox setup --enable-python`, which the `[Run]` entry passes: it reuses
  the real path resolution, and because the flag is applied **past the `?`** on
  `setup(...)`, a failed download leaves the tool off — ADR 0005 §5's "enabled but
  not provisioned is worse than disabled", preserved by construction rather than by
  a comment. Long-form flag only: it changes a security-relevant setting, so it
  should be spelled out at the call site.
- **Reading the startup path first caught a trap worth recording.** `run_tui` seeds
  `interface.language` from `defaults.json` **only when `settings.json` is absent**
  (`main.rs`, axis B). So a CLI that *creates* `settings.json` during installation
  would have silently discarded the language the installer had just asked the user
  for — the wizard's own choice, lost by the step meant to help. `enable_python_tool`
  therefore mirrors that seeding, and carries the other precaution the app takes
  before writing user data: **`data_migration::run` first**, so a `settings.json`
  from a newer version is refused instead of being read leniently and saved back
  **without the fields this build cannot see** (ADR 0006 F10).
- **Mutation testing earned its keep three times over here**, and two of the three
  first attempts were *my tests being wrong*, not the code: (1) the language
  assertion was **vacuous**, because `Paths::with_root` defaults to `Ru` — the same
  value `AppConfig` deserializes to — so it passed with the seeding removed; fixed
  by a `#[cfg(test)] with_default_language` builder. (2) The "already enabled →
  don't rewrite" test compared bytes of a file this program had itself written, and
  a rewrite reproduces those bytes exactly; it only became able to fail once the
  fixture was a **hand-written minimal config** that a rewrite would expand. (3) The
  corrupt-config test never exercised the migration guard at all — `load_config`
  errors on corrupt JSON by itself; the guard's real job is the **downgrade** case,
  which is *valid* JSON, so it needed its own test. A fourth lesson, mechanical: the
  first two attempts at that mutation silently patched the **wrong call site**
  (`data_migration::run` appears three times with identical indentation), so the
  mutation must be anchored on the enclosing function signature.
- **Verified against the real binary and the real dev data root**, not only in
  tests: `--enable-python` took `python_enabled` **false → true** while leaving
  `interface.language` and `max_tool_rounds` untouched; a plain `sandbox setup` left
  it **false** (the opt-in default holds); and the run was idempotent against an
  already-provisioned sandbox. The dev config was backed up and restored.
- **Follow-up, spotted by the user while the installer was being tested**:
  English sat *second* on the "Application language" page, and now leads — matching
  the `[Languages]` order. A two-line change with a trap in it: `WriteDefaults`
  mapped `SelectedValueIndex = 0` to `'ru'`, so reordering the two `Add` calls
  alone would have **inverted the language written into defaults.json** while
  looking correct on screen. The indices are now named (`EnLangIndex`/`RuLangIndex`,
  the pattern the data-location page in the same file already used), so the order
  and the mapping can no longer drift apart. The **pre-selected** option is
  unchanged and still follows the language the wizard is running in, not the list
  order. No CHANGELOG entry — the order of two radio buttons is below what a
  release note serves.
- **Verified in both directions** (silent installs: `/LANG=ru` → `"ru"`,
  `/LANG=en` → `"en"` — the assertion that fails if the mapping inverts) and by
  looking at the page in both locales: English first, with Russian still
  pre-selected in the Russian wizard. Getting that picture needed two workarounds
  worth keeping: the session's desktop input had become unavailable, so the wizard
  was advanced with **`PostMessage(BM_CLICK)`** instead of `SendKeys` (it needs no
  focus, and unlike `SendMessage` it doesn't block), and captured with
  **`PrintWindow`** instead of `CopyFromScreen` (it renders the window straight
  into a DC, so it works with no access to the screen). Also learned the hard way:
  the options on a `CreateInputOptionPage` are **not** child radio-button windows —
  Inno owner-draws them inside one `TNewCheckListBox` — so enumerating controls
  finds nothing and a picture is the only way to read that list.

### Release 0.9.5 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.9.5`): bumped `Cargo.toml` `0.9.4 → 0.9.5` (+ `Cargo.lock`),
  `CHANGELOG.md` — `[Unreleased]` → `[0.9.5] — 2026-08-09`, a fresh empty
  `[Unreleased]` opened, comparison links updated. The `v0.9.5` tag is applied by
  the user after the merge (the agent doesn't push tags/`main`).
- **MINOR, not PATCH**: while at `0.x` a MINOR carries features (AGENTS.md §6),
  and the section is dominated by them — Grok (xAI) as a fourth cloud provider,
  history compression (`/compact`, the automatic trigger, the read-back tools),
  chat file attachments and their semantic index, chat content search
  (`Ctrl+F`/`Ctrl+G`) and in-feed search, the MCP server editor with
  machine-bound env secrets and config import, `youtube_watch`,
  password-protected backups, dangerous-tool confirmation, `/reindex`, and the
  `DISCLAIMER.md` shipped with every archive and package.
- **The `[Unreleased]` section was consolidated on the way in.** It had grown
  over ~40 merged PRs into **13 rubric blocks** (Added ×4, Changed ×3, Fixed ×5,
  Security ×1); they were merged into one rubric each, in the order the CHANGELOG
  header declares (Added / Changed / Fixed / Removed / Data / Security) — the same
  clean-up the `0.9.1` release did. This is not cosmetic: `release.yml` publishes
  that section **verbatim** as the GitHub Release body (the `awk` extractor between
  `## [ver]` and the next `## [`), so a reader of the release page would otherwise
  meet "Fixed" five times. One item was also in the wrong rubric — the
  `youtube_watch` transcript, an addition sitting under "Fixed" — and moved to
  "Added" next to the tool it extends.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **1962 unit tests, 84 `#[ignore]`**; the documentation gates
  (`cyrillic_scan`, `link_check`, `doc_index_check`) green too. No live run needed
  (version + docs, app code untouched). The version in CLAUDE.md's "## Status"
  header was refreshed; the test count and date there were already current.

### Post-M9: demo screenshots — stage 1 (fixture, frame dumps, raster tool) (done)
- **The screenshot problem for the public opening**: captures of a real session
  would expose private conversations, and hand-made screenshots rot as the app
  evolves — the standing answer to rot here is a gate, not discipline. Design
  plan [docs/history/demo-screenshots.md](../history/demo-screenshots.md)
  (in `docs/` while the track ran); forks confirmed by
  the user 2026-08-10: interactive `mindfork demo` adopted as stage 3, capture
  set = chat / chat list / settings twice (Model+**Tools** — the latter added
  by the user as the best single showcase) / self-model, **Dark+Light x EN**,
  rot control = a drift-gate unit test. Branch `feat/demo-screenshots-mvp`.
- **Everything stays test-side** — the release binary gains no capture surface:
  `features/demo` is a code-generated showcase conversation (GFM table, bash
  block, Mermaid flowchart, LaTeX, expanded thoughts, a `note_save` tool card)
  with fixed ids and timestamps; `shared/shot` captures a rendered `Buffer` to
  JSON (per-cell symbol/RGB/modifiers; wide glyphs emitted once with their
  span; named-ANSI mapped to xterm values; canvas constants live next to the
  palettes in `theme.rs`); `app/demo_shots` holds the recipe (`ChatScreen`,
  `set_settings(Dark, En)`, `Ready` server statuses, `TestBackend` 116x44) and
  the `#[ignore]` regenerator writing `assets/screenshots/dumps/`.
- **`tools/screenshots.py`** (Pillow + fontTools — the `build-wordmarks.py`
  third-party precedent): glyphs are placed by grid cell, so the dump's
  geometry is authoritative and font metrics cannot drift the layout;
  JetBrains Mono is probed the wordmark script's way (system fonts, JBR
  bundles), with cmap-routed symbol fallbacks (Segoe UI Symbol/Emoji, Segoe
  UI, DejaVu); 2x supersampling; a glyph no font covers is a **named WARNING
  and exit 2**, not silent tofu. First catch: `U+1D65` (subscript v from
  `$M_{kv}$`) — in Segoe UI, absent from JetBrains Mono and Segoe UI Symbol.
- **What the render-look-adjust loop caught** (three rounds): a Mermaid branch
  draws ~17 rows tall, so the first fixture pushed the table and the thoughts
  off-frame — the conversation was recomposed until the final exchange fits
  one viewport; the status bar wraps at 110 columns and fits at 116; and a
  needle phrase (`"from the hardware"`) word-wrapped across rows, failing
  `contains` over row-joined text while perfectly visible — the joined-rows
  trap (lessons §2), needles are single-line-safe now.
- **Guards on the recipe**: determinism (two captures serialize byte-equal),
  grid coverage (every row's widths sum to the frame width), and an on-screen
  assertion for the showcase content (title, thoughts, table verdict,
  flowchart node, tool card) — a fixture edit that scrolls the subject out of
  frame fails the build instead of shipping a screenshot of nothing. The
  drift gate proper (fresh render vs committed dumps) is stage 2.
- **Tests**: 1971 unit green (+9: 4 capture, 2 fixture, 3 recipe), 85
  `#[ignore]` (84 live smokes + the new dump regenerator, which is not a live
  test — it needs no server, only deliberate invocation). **No live run**:
  capture is render-only, touching no engine/memory/tool path (AGENTS.md §3).
- **What the Sonar gate caught on the PR** (new-code security rating C → gate
  red): two `pythonsecurity:S8707` — the new *agentic workflows* path-injection
  rule: `--dumps`/`--out` flowed from argparse into `read_text`/`mkdir`
  unvalidated. Fixed by canonicalize-and-confine to the repository
  (`under_repo()`: `Path.resolve()` + `is_relative_to(REPO)` — not a
  `startswith` prefix, the partial-traversal pitfall the rule documents; the
  refusal names the path and the base). Alongside it: S3776 (`render()`
  cognitive complexity 34 → split into `cell_colors`/`draw_cell`/`render`,
  verified byte-identical output) and S1172 (a genuinely dead `size`
  parameter). The rule class is LLM-era and will meet every future
  path-taking tool script — recorded in lessons §1.

### Post-M9: demo screenshots — stage 2 (the full set, the drift gate, README embeds) (done)
- **Go received on the stage-1 hero shot** (2026-08-11 user session); branch
  `feat/demo-screenshots-set`. This stage delivers the rest of the confirmed
  matrix: chat list, settings twice (Model/server + Tools), self-model — each
  in Dark and Light, ten dumps and ten PNGs total.
- **The fixture grew organs** (`features/demo`): nine chat summaries with
  fixed dates (the showcase chat active on top), a mid-life self-model
  (summary, three goals in two states, a user model, three dated
  observations), a managed-llama.cpp `AppConfig` consistent with the hero
  conversation (same model, same 16k context), and a "Gaia" profile with the
  base tools enabled.
- **Recipes at per-screen heights** (hero 116x44; panels hug content —
  list 22, settings 30, self-model 24: empty terminal makes a poor gallery).
  The Tools section is reached by two `Tab` key events (`SECTIONS` order),
  and the showcase test pins the destination by content ("Agentic loop",
  "Wasmer sandbox"), so a section-order change cannot silently capture the
  wrong pane. First needle guess ("note_save") was wrong and instructive:
  the Tools section is global switches and parameters; per-tool toggles live
  under Profiles.
- **The drift gate** (`committed_dumps_match_the_code`, an ordinary test):
  renders the whole matrix fresh and compares byte-for-byte with the
  committed dumps; a missing file is a failure, not a skip; `\r\n` from
  git eol translation is normalized; the failure message names the first
  differing line and the two regeneration commands instead of printing two
  hundred-kilobyte JSONs at each other.
- **Raster tool**: `.ttc` collections handled (explicit face index for
  Pillow and fontTools), Yu Gothic / Noto CJK appended to the fallbacks —
  the coverage warning caught `U+FF0B` (fullwidth plus, the F3 screen's
  "add goal" affordance) covered by no Latin fallback.
- **The hero dump survived the refactor byte-identical** (capture gained a
  height parameter; `git status` stayed silent on `chat-dark-en.json`) — the
  artifact the user approved is exactly the artifact shipped.
- **README**: a theme-aware hero `<picture>` (dark/light PNGs switched by
  `prefers-color-scheme`) plus a `<details>` gallery of the other four, and
  a stated promise next to them: screenshots are generated from code and a
  gate fails the build when they drift. `assets/screenshots/README.md`
  documents the two-command regeneration.
- **Tests**: 1972 unit green (+1: the drift gate; the determinism, grid and
  showcase guards now sweep the whole matrix), 85 `#[ignore]` unchanged.
  **No live run** — render-only, no engine/memory/tool path (AGENTS.md §3).
- **What the Sonar gate caught on the PR** (new-code duplication 13% > 3%,
  zero issues): `chat_summaries()`'s eight per-chat constructor calls,
  exploded by rustfmt into identical multi-line blocks, formed a **sliding
  self-duplicate** — the same file matched itself seven lines apart over a
  49-line window. The fix is shape, not content: the rows became a
  `#[rustfmt::skip]` data table (one row per line, the canonical use of the
  attribute) mapped through a single constructor call site, leaving CPD no
  ten-line window to match. Verified value-identical the cheap way: the
  drift gate stayed green with zero dump changes.

### Post-M9: demo screenshots — stage 3 (the interactive `mindfork demo`) (done)
- **The track''s last planned stage** (design plan moved to
  [docs/history/demo-screenshots.md](../history/demo-screenshots.md) with this
  PR — the track is complete; stage 4, an SVG writer for the site, is
  deliberately deferred and recorded in the roadmap). Branch `feat/demo-mode`.
  The ask: "try the app without downloading a model" — the real TUI on seeded
  data with a scripted engine, touching nothing outside a temp folder.
- **Un-gating, minimally**: `shared/api/mock` compiles into the release binary
  now, but only `cycling` — the demo''s constructor — is un-gated;
  `scripted`/`sequence`/`cancellable` stay `#[cfg(test)]` pointwise (clippy
  itself insisted: a constructor nothing outside tests calls is dead release
  code). `MockSupervisor` stays test-only; the demo gets its own ~50-line
  `DemoSupervisor` (chat/impersonation = the scripted backend, embed =
  `MockEmbedder`, everything `Ready` synchronously) instead of inheriting
  test machinery.
- **`MockBackend::cycling(scripts, delay_ms)`**: rotates scripts endlessly
  (`sequence` runs dry into empty turns — wrong shape for a conversation) and
  paces chunks (18 ms) so streaming looks like streaming. Replies are
  **self-contained by design**: a background call (impersonation, regenerate)
  may consume a script out of turn, so no reply depends on which question
  preceded it — each says something true about the app, and the first one
  names what it is (the close-the-door rule, lessons §4).
- **Provisioning is the fixture, promoted**: `demo::provision` seeds config
  (engine `external` with model name `demo (mock engine)` — the feed header
  caption becomes the honest demo marker for free; the status-bar chip the
  plan sketched was rejected as new UI surface for one word), the "Gaia"
  profile (fixed id now, plus a greeting so a fresh chat also says what the
  demo is), the showcase chat, eight filler chats with real two-message
  excerpts (titles/dates shared with the list capture via one `ROWS` table),
  and the seeded self-model for `F3`.
- **`main` boots it before the real root is touched**: the `demo` arm branches
  ahead of `ensure_dirs`/logging, gets its own temp root
  (`%TEMP%/mindfork-demo-<pid>`), skips the single-instance guard (a demo may
  run next to the real app), data migration (the root is born current) and
  `MINDFORK_*` env overrides (the environment belongs to the real app), and
  removes the root on clean exit. The shared launch core was extracted as
  `launch_tui(paths, supervisor, apply_env, loc)` — `run_tui` and `run_demo`
  are now two thin wrappers over it.
- **Verification**: the whole loop runs headlessly in
  `orchestrator/tests/demo.rs` — a provisioned root bootstraps onto the
  showcase chat, a user message streams the self-describing reply, a second
  message gets a *different* one (cycling, not repetition). Provisioning is
  pinned complete and idempotent; CLI parsing and both help pages are tested;
  the capture drift gate stayed green throughout — the new profile fields
  (id, greeting) provably moved no pixel of the committed set. **1977 unit
  tests green (+5), 85 `#[ignore]`.** No live-model run (the engine is the
  mock by definition); the one thing only a human can judge — the demo in a
  real terminal — is the review step: `cargo run -- demo`.

### Post-M9: demo screenshots — stage 4 (SVG writer for mindfork.io) (done)

- **The deferred vector half of the screenshot pipeline** (deferred by the
  demo-screenshots plan until the site could set the sizes and themes it
  must serve; shipped with the website track's S4, branch
  `feat/screenshots-svg`). `tools/screenshots.py` gained `--format
  png|svg|both` (default both) and a `render_svg`: merged background
  rects, one `<text>` per row with a `<tspan>` per same-style run,
  classes for bold/italic/underline/strikethrough, solo middle-anchored
  tspans for wide and fallback-font cells — the vector cousin of the raster
  path's centered fallback drawing. The `<style>` is scoped under the
  root id, so a copy inlined into a page cannot leak rules into it; the
  font arrives by `@font-face` reference (`--svg-font-base`, default
  `/fonts/` — the site serves its own woff2).
- **Two measured traps.** Pillow's px-hinted `getlength("0")` said 10.0
  where the font's true advance is 9.6 (0.6 em at 16 px) — browsers lay
  glyphs out unhinted, so a grid built on 10.0 would leave every long run
  landing short of its cells; the grid now comes straight from the font
  tables (`hmtx`/`hhea`/`head`), and every multi-cell run also
  carries `textLength` as insurance against a viewer substituting a
  font with a different advance. And glyphs outside JetBrains Mono's
  coverage (`✦`, `⚒`) render from the viewer's fallback fonts — exact
  on the PNG, viewer-dependent in SVG; accepted, since the PNG remains the
  byte-exact reference.
- **Outcome**: 8–17 KB per frame against the PNGs' 100–400 KB, crisp at
  any zoom. PNG stays the README format — GitHub cannot load fonts into an
  embedded SVG. The consuming side (inlining, theme pairing) is the
  website journal's S4 entry.

### Post-M9: demo screenshots — a uniform gallery and a richer hero (done)

- **The gallery panels now share one height** (branch
  `feat/screenshot-polish`). The site's 2×2 `shot-grid` was showing four
  windows of three different heights (22/30/30/24 rows) — visibly ragged.
  The per-screen "hug the content" heights are replaced by one
  `PANEL_H = 30`, and 30 is not arbitrary: the settings **Tools** section —
  the richest capture — fills its parameter area exactly at that height, so
  any shared height had to be at least 30, and anything more re-opens the
  empty-rows problem the old constants existed to avoid.
- **The fixture was stocked to meet the height instead of padding frames
  with void.** The chat list grew from 9 to 22 dialogs — 13 new filler rows,
  each with a real two-message excerpt (`ROWS`/`ROW_BODIES` stay one
  index-aligned table), so the interactive demo's list, search and export
  gained content too, and 22 fills the 30-row list area to the last row.
  The self-model grew to five goals (two completed), three traits, three
  interests and six observations. The Model/server capture now shows
  speculative decoding configured (`draft-simple`, a 1B draft at
  `-ngld 99`) — the four draft fields fill the section's trailing rows,
  and the demo world stays coherent: the chat list holds the
  "Speculative decoding: draft models" conversation that recommends
  exactly this setup.
- **The hero got a fuller diagram and a live input box.** The two-branch
  flowchart (six bare edge rows, asymmetric slashes) became a three-branch
  context tree — `8k → Q6_K / 16k → Q5_K_M / 32k → Q4_K_M` — the same
  13 rows, but symmetric and generalizing the table above it instead of
  restating it (the LaTeX cache-rule line is now its caption). The label
  wordings were **brute-forced against `mermaid-text`**: node centering
  depends on exact label widths, and one character more ("cache headroom"
  for "KV headroom") bends the middle leg into a `┌─┘` jog — a comment in
  the fixture pins the constraint. The input box now holds a
  typed-but-unsent follow-up about the 1B draft (`INPUT_DRAFT`), seeded
  into `showcase_chat().draft` too, so `mindfork demo` opens mid-thought
  exactly like the screenshot.
- **One trap found live: the screens render dates in local time.** A
  narrative segment stamped 21:55 UTC displayed `[2026-07-31]` on the
  regenerating machine (UTC+3) but would display `[2026-07-30]` on CI
  (UTC) — the drift gate red on one side or the other. Fixture timestamps
  that reach a rendered date now stay mid-day UTC (recorded in
  lessons §2). Related: the self-model screen lists the narrative
  newest-first **by insertion**, so the vec must append in chronological
  order or the visible dates scramble.
- **The quality gate then measured the fixture's shape, and won.** The PR's
  first analysis failed on **24.7% new-code duplication (bar ≤ 3%)** with
  every literal in the file different — because Sonar's Rust CPD compares
  **normalized** tokens (`jscpd`, which compares exact tokens, reports 0%
  on the same two files), so five `goal(…)` blocks, six `segment(…)`
  blocks and 21 same-shape tuple rows are all sliding self-duplicates;
  and because the bar is a *density*, the same shapes that sailed through
  the big stage-1–3 PRs blew past 3% on this small one. The fix moved the
  bulk data out of token space entirely: the chat rows, the Q/A excerpts,
  the goals and the narrative are now four flat raw-string tables (one
  string literal = one token) parsed by `ts`/`table_lines`/`rows`/`bodies`
  — ~30 unique lines replacing ~150 structurally repeated ones — and the
  showcase-needle arrays became one-line match arms. The **drift gate is
  what made the refactor safe**: the committed dumps never changed, so
  green meant the parsed tables reproduce the old values byte for byte.

### Post-M9: demo screenshots — the canvas padding measured in cells (done)

- **The renders carried too wide a mat** (branch `fix/screenshot-padding`).
  `tools/screenshots.py` framed every capture in a flat 24 px of canvas
  background, and it read as unnatural in both places the images actually
  live: on GitHub the README embeds the PNG at `width=900` on a page whose
  background is not the app's, so the mat is a visible border; on
  mindfork.io the SVG is scaled to the width of the site's terminal chrome
  (~1.1×), so the mat grew with it and left the app's own frame floating
  inside the window.
- **The default is now one character cell** (`PAD_CELLS = 1`, ~10 px at the
  default 16 px size) — computed from the font's true advance rather than
  written as a pixel constant, so it stays proportional at any `--size`.
  `--pad` still overrides it. One cell is what a terminal leaves around its
  grid, which is the thing these images are pretending to be. The hero PNG
  goes 1150×994 → 1122×966.
- **The dumps did not change, so the drift gate stayed green** — this is a
  rendering-side change only, and the images were regenerated from the
  committed dumps (`python tools/screenshots.py`, `python
  tools/site_sync_assets.py`, `zola build`).
- **A regeneration trap worth writing down: JetBrains Mono is not installed
  on every machine that has to re-render.** The TTFs are deliberately not
  vendored, and probing found nothing here. The woff2 faces the site serves
  (`site/static/fonts/`) are the *full* family, not a subset, so
  `fontTools` (plus `brotli`) decompresses them back into TTFs for
  `--font-dir`. Proof that this is faithful and not merely plausible:
  re-rendering at the old `--pad 24` from those faces reproduced all ten
  PNGs and all ten SVGs **byte for byte**. The set uses no bold-italic
  cell, so the missing fourth face costs nothing.
- **Verification.** The whole SVG delta is the shift: every `x`/`y` moved by
  exactly −14 and the root box by −28, with the text and structure
  otherwise identical. The site was rebuilt and both the hero and the 2×2
  gallery checked in a browser.

### Post-M9: the Windows installer shows the license and the disclaimer (done)
- **The task** (user, 2026-08-13; branch `feat/installer-legal-pages`): the
  Windows wizard had no legal pages at all. `LICENSE` and `DISCLAIMER.md` were
  *installed* next to the binary — copied there since the installer's first
  version and, for the disclaimer, since the notice was written — but nothing
  ever put them in front of the person clicking through the wizard. Packaging
  only; the app's source is untouched.
- **Two pages, one acceptance** (user's decision, 2026-08-13). The MIT text goes
  on Inno's own license page, where the accept/decline radio gates `Next`; the
  disclaimer goes on the "info before install" page, read-only, `Next`
  continues. The alternative — a custom `[Code]` page with an "I have read and
  accept" checkbox — was offered and declined: the disclaimer *supplements* the
  license (that is its first sentence, and the whole reason it is a separate
  file), so making it a second contract to sign would misstate what it is.
  A third option, a short bilingual summary in `[CustomMessages]`, was declined
  for the obvious reason: a summary of a legal notice is not the notice.
- **The license page needs no copy of its own.** `LicenseFile` points straight at
  the root `LICENSE`, extension-less file and all — ISCC reads it happily, and it
  is plain ASCII with no markdown precisely because a gate test holds it that way
  (`credits::license_file_carries_nothing_but_the_mit_text`). The property that
  keeps SPDX scanners honest is the same one that makes the file directly
  displayable.
- **The disclaimer needs one, and it is generated.** `DISCLAIMER.md` is markdown;
  Inno renders text or RTF and nothing else, so pointed at the source the wizard
  would show `# Disclaimer`, `**bold**` and `[accompanying file](LICENSE)`
  verbatim. Hand-maintaining a second copy is the worse answer — the text a user
  *accepts at install time* would drift from the one in the repository, the
  release archives and the app's `F1` tab, and nothing would notice. So
  `tools/wizard_rtf.py` renders `packaging/windows/disclaimer.rtf` from the
  source, the result is committed (the installer compiles on machines with no
  Python), and `--check` runs in CI's `lint` job. It lives there rather than in
  `packaging.yml` because the change that breaks it is an edit to `DISCLAIMER.md`
  — a path `packaging.yml` does not even trigger on.
- **The converter covers the subset the file uses** — ATX headings, hard-wrapped
  paragraphs (unwrapped, so the wizard's memo wraps to its own width), `-` lists
  with a hanging indent, `---` rules, and inline bold/italic/code/links (a link
  keeps its text and drops the target, unclickable in a wizard anyway). Anything
  outside the subset passes through as literal text rather than being dropped: a
  new construct should show up in the output, not disappear from a legal notice.
  Every non-ASCII character becomes a `\uNNNN?` escape, so the RTF is pure ASCII
  — no encoding negotiation with the compiler, and nothing for `cyrillic_scan.py`
  to trip over should the source ever gain any.
- **The page is named after what it holds.** The stock "info before install" page
  calls itself "Information" and asks the user to read "important information" —
  an understatement for a notice whose closing line says not to use the software
  if you disagree with it, and it would read as a stray readme rather than the
  second half of a license → disclaimer pair. `[Messages]` overrides
  `WizardInfoBefore`/`InfoBeforeLabel` in both languages; the `ru` caption is the
  borrowed «Дисклеймер» (cyrillic-ok: the ru caption itself), the same word the
  `F1` tab settled on. The **texts
  themselves stay English in the `ru` wizard**, matching the app
  (`shared/credits.rs`): only the chrome around them is localized.
- **Live run — GO** (Inno Setup 6 on the dev machine, no live model needed — this
  is packaging): the `.iss` **compiles** with the new pages; the wizard was
  launched in **both locales** (`/CURRENTUSER /LANG=en|ru`) and screenshotted on
  each page. The license page shows the MIT text with `Next` **disabled** until
  "I accept" is chosen; the disclaimer page renders the RTF with real headings,
  bold, italics and monospaced `llama.cpp` — em dashes included, which is what
  proves the `\uNNNN?` escapes work. Nothing was installed (the wizard was killed
  after the captures) and the test `setup.exe` was deleted.
- **A GUI-automation note for the next such run:** the accept radio's `Alt+A`
  accelerator is a *different letter* in the `ru` wizard, so a run driven by
  accelerators silently stays on the license page and screenshots the wrong one.
  `Tab` into the radio group + `Up` is locale-independent. Also, `GetWindowRect`
  returns physical pixels while a DPI-unaware PowerShell process draws in scaled
  ones — a per-window capture comes out clipped; capturing the whole screen does
  not.
- **Checks**: no Rust code → **2150 unit tests** green as before, 96 `#[ignore]`;
  `fmt`/`clippy -D warnings` clean, `cyrillic_scan`/`link_check`/`doc_index_check`
  /`wizard_rtf --check` clean.

### Release 0.9.7 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.9.7`): bumped `Cargo.toml` `0.9.6 → 0.9.7` (+ `Cargo.lock`),
  `CHANGELOG.md` — `[Unreleased]` → `[0.9.7] — 2026-08-17`, a fresh empty
  `[Unreleased]` opened, comparison links updated. The `v0.9.7` tag is applied by
  the user after the merge (the agent doesn't push tags/`main`).
- **MINOR, not PATCH**: while at `0.x` a MINOR carries features (AGENTS.md §6),
  and the section is seven additions against one fix — command-only control
  (nineteen typed routes plus `/profile` and `/self clear`), OSC 52 copying for
  remote sessions, `/export` to a file, the cross-chat `chat_search`/`chat_read`
  pair with navigable `chat://` references, automatic chat titling, the external
  server's API key in settings, and the model's name on the assistant's header.
  The single fix (help descriptions clipped mid-word) and the single change (the
  `F1` window's adaptive layout) are both inside the same help track.
- **The `[Unreleased]` rubrics were reordered on the way in.** The section had
  grown as `Changed` → `Added` → `Fixed`, because the help track landed last and
  opened its rubric at the top; they were put back into the order the CHANGELOG
  header declares (Added / Changed / Fixed / Removed / Data / Security). This is
  not cosmetic — `release.yml` publishes the `## [0.9.7]` section **verbatim** as
  the GitHub Release body, so the reader of the release page would otherwise meet
  a layout tweak before the seven features. No merging was needed this time: one
  rubric block each across 21 merged PRs — where `0.9.1` and `0.9.5` each had to
  collapse a dozen duplicate rubrics, the per-PR discipline held on its own.
- **No `Data` rubric**: the release adds config and entity fields
  (`interface.show_model_name`, `interface.auto_title`,
  `interface.clipboard_osc52`, `tools.chat_search`, `Chat.renamed_manually`, the
  `External` secret slots) but every one is `#[serde(default)]`, so no stored
  file changes shape and there is nothing for a user to be told about.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **2304 unit tests, 99 `#[ignore]`**; the documentation
  gates (`cyrillic_scan`, `link_check`, `doc_index_check`, `wizard_rtf --check`)
  green too. No live run needed (version + docs + site content, app code
  untouched). CLAUDE.md's "## Status" header carries the new version; its test
  count and date were already current.

### Post-M9: the installer's legal pages speak Russian (done)
- **The packaging half** of the track whose app side is in
  [ui-screens.md](ui-screens.md) ("the licence and the disclaimer in Russian");
  plan: [docs/history/legal-ru-translations.md](../history/legal-ru-translations.md).
  The `ru` wizard showed a page whose caption was already the borrowed Russian
  word and then handed the reader the English notice; both legal pages now show
  the translation.
- **Inno takes the pages per language.** `LicenseFile`/`InfoBeforeFile` are
  `[Languages]` parameters, not only `[Setup]` directives, so both moved out of
  `[Setup]` onto the language entries — one place answering "which text does this
  wizard show", rather than a default in one section and an override in another.
  The `en` wizard is byte-for-byte what it was.
- **The RTF converter grew from one pair to three.** `tools/wizard_rtf.py`'s
  `PAIRS` was written as a list because "the license page may one day want the
  same treatment" — this was the day. The Russian pages need generating for the
  markdown *and* for the encoding: every non-ASCII character becomes a `\uNNNN?`
  escape, so a page of Cyrillic reaches ISCC as pure ASCII and neither the
  compiler nor `cyrillic_scan.py` has to care. The English `LICENSE` still needs
  no copy — plain ASCII, pointed at directly. Each generated file is now stamped
  with the source it came from, since three of them share one "do not edit" note.
- **Live run — GO** (Inno Setup 6.7 on the dev machine; packaging, so no model is
  involved). The `.iss` compiles, and the wizard was launched in **both** locales
  (`/CURRENTUSER /LANG=en|ru`) and screenshotted on each legal page. The `ru`
  licence page renders the translation with the Next button disabled until the
  accept radio is chosen, and the `ru` disclaimer page renders real headings, bold and
  monospaced `llama.cpp` — which is what proves a whole document of `\uNNNN?`
  escapes survives RichEdit, not just the em dashes the English page tested.
  Nothing was installed (the wizard was killed after the captures) and the test
  `setup.exe` was deleted.
- **Two GUI-automation notes for the next such run**, on top of the earlier
  entry's (drive with `Tab`+arrows, not the accept radio's `Alt+A`, which is a
  different letter in `ru`; capture the whole screen, not the window):
  `Setup.exe` re-launches itself, so the PID `Start-Process` returns is not the
  window's — find the window, then **verify it is in the foreground before
  sending a single key**, or the keystrokes land in whatever the user has open.
  And there is no Welcome page (Inno 6 disables it under `WizardStyle=modern`),
  so the licence page is the *first* page, not the second.
- **Both translations ship**, next to the originals they translate: the release
  archives' doc set, the `.deb`/`.rpm`/`.pkg` doc directory and the installed
  folder (user's decision) — someone who read a Russian page in the wizard can
  find that text again afterwards.
- **Checks**: 2443 unit tests green, 106 `#[ignore]`; `fmt`/`clippy -D warnings`
  /`cyrillic_scan`/`link_check`/`doc_index_check`/`wizard_rtf --check` clean.

### Post-M9: the Windows installer moves to Inno Setup 7 (done)
- **The question was the compiler; the answer was the supply** (branch
  `feat/inno-setup-7`, research
  [docs/research/inno-setup-7.md](../research/inno-setup-7.md), five forks
  confirmed by the user). `packaging/windows/mindfork.iss` compiles on Inno Setup
  7.1.0 **unmodified**, zero warnings, and the installer it produces was verified
  identical to the 6.7.3 one down to the bytes of `defaults.json` — same 17-file
  tree, same 46 bytes with the BOM, same clean uninstall; the only differences
  are in the install log (extended-length paths, UTC time stamps). So nothing in
  the script forced the move.
- **What forced it was CI.** Both workflows ran `choco install innosetup`, and
  that package has been **frozen at 6.7.1 since 2026-02-17** while upstream
  shipped 6.7.2, 6.7.3, 7.0.2 and 7.1.0 — so every release so far was compiled
  two maintenance releases behind its own line, by a step that also reinstalls
  what the runner image already carries (the image installs the same chocolatey
  package). There is no 7.x on chocolatey at all. The install step had to be
  rebuilt whichever line we stayed on, which is what made 7 free.
- **The trap, found while designing and worth remembering**: chocolatey puts an
  `ISCC.exe` shim on `PATH`, and both jobs resolved the compiler with
  `Get-Command ISCC.exe`. Installing 7 without touching that would have kept
  compiling with 6.7.1 while the log said 7 was installed — a silent wrong-tool
  failure. `tools/install_inno.ps1` therefore **returns** the absolute path
  (`$env:ISCC` for later steps) and asserts `--version`, never searches for one.
- **One script, not two inline copies** (the repository's first `.ps1`): the pin
  — version, release URL, SHA-256 — lives once, because a gate compiling with a
  different Inno Setup than the release is a gate that has stopped testing the
  release. The download is the official `jrsoftware/issrc` release asset, hash
  verified before it is run; the pinned hash was checked against the real
  download. The script is idempotent, so it is also how a developer gets exactly
  the compiler CI uses.
- **The installer is now 64-bit** (`SetupArchitecture=x64`, user's decision
  against the recommendation, and for a reason the recommendation had missed).
  `ArchitecturesAllowed=x64compatible` already refused an unsupported OS — but
  from *inside* a wizard that had started, a page after the licence. A 64-bit
  setup does not start there at all: Windows declines to load the image. Cost,
  measured: 3.13 MB → 3.88 MB (+746 KB). The `Architectures*` pair stays explicit
  beside it, since it states the installation's rule rather than the compiler's.
- **Two visible consequences, both taken deliberately.** Inno 7 changed the
  `AppVerName` default, so the wizard caption lost the word "version" — accepted
  rather than pinned back with `{cm:NameAndVersion}`. And `SetupArchitecture` is
  a 7 directive, so the script is now 7-only: Inno 6 refuses it **by name**
  (*Unrecognized [Setup] section directive "SetupArchitecture"*), which is the
  right failure — a contributor on 6 is told what is missing instead of building
  a setup.exe that quietly differs from the released one.
- **Live run — GO** (Inno Setup 7.1.0 on the dev machine; packaging, so no model
  is involved). The x64 setup was silently installed and uninstalled (17 files,
  `defaults.json` byte-identical to the Inno 6 build, directory gone afterwards),
  and the **wizard was driven and screenshotted** with the technique the earlier
  entries left behind — `PostMessage(BM_CLICK)` to advance, `PrintWindow` to
  capture: the licence page in English (plain `LICENSE`) and Russian
  (`license-ru.rtf`, the escaped-Cyrillic RTF rendering intact), both custom
  Pascal pages, and the PNG header logo on a 64-bit setup binary. Nothing was
  installed by the GUI runs and their temp directories were removed.
- **Two more GUI-automation notes for the next run**: control text carries its
  **accelerator ampersand** (`I &accept the agreement`, `&Next`), so a pattern
  copied off a screenshot matches nothing; and the capturing process must call
  `SetProcessDPIAware` first, or `GetWindowRect` returns logical pixels while
  `PrintWindow` renders physical ones and the capture comes back cropped.
- **Checks**: 2443 unit tests green, 106 `#[ignore]` (no Rust code touched);
  `fmt`/`clippy -D warnings`/`cyrillic_scan`/`link_check`/`doc_index_check`
  /`wizard_rtf --check` clean, both workflow YAMLs valid.

### Post-M9: the binary/command renamed to `mindfork` (done)
- **The command is the brand now** (branch `feat/binary-rename`, research
  [docs/research/binary-rename.md](../research/binary-rename.md); user's
  decisions 2026-08-24: rename; the project/repository/package stays
  `mindfork-rs`; and — no public release having happened — a clean cut, with
  no transitional `/usr/bin/mindfork-rs` symlink and no `[InstallDelete]`
  archaeology in the installer). One Cargo stanza does the rename
  (`[[bin]] name = "mindfork"`, `path = "src/main.rs"` — an explicit `[[bin]]`
  disables autodiscovery); nothing in the repository invokes the built binary
  by name, so `cargo build/run/test` were untouched.
- **Everything else is surfaces**: the CLI usage/`--version`/guard strings
  (5 locale keys in each of en/ru), the empty-chat title fallback (now
  `credits::APP_NAME` instead of a second literal), the startup/exit log
  lines, MCP `clientInfo.name`, the sandbox download User-Agent; the Linux
  staged binary + `/usr/bin` symlink + the `.desktop` (file renamed,
  `Name`/`Exec`/`Icon`) + the hicolor icon names, with `packaging.yml`'s
  container-smoke assertions moved in step; the Inno installer
  (`AppName`/`UninstallDisplayName`/`DefaultGroupName`/shortcuts/exe);
  `release.yml`'s binary paths; and the command examples across install.md,
  import-format.md, CONTRIBUTING.md, AGENTS.md §6 and spec.md.
- **What deliberately kept the old string** — each now carries a source
  comment naming the research doc: `ProjectDirs("", "", "mindfork-rs")` (the
  existing per-user data folder), the `secrets.rs` v1 encryption constants
  (protocol inputs; "aligning" them would orphan every stored key), the
  single-instance lock id (a pre-rename build and a current one must still
  exclude each other), the Inno `AppId`/`DefaultDirName`/
  `OutputBaseFilename`, the package and artifact names, and every directory
  (`/usr/lib/mindfork-rs/` also being where the `config|noreplace`
  `defaults.json` lives). A new gate test in `credits.rs` pins
  `[[bin]] name` ≡ `APP_NAME` and the package name, so the brand constant and
  the typed command cannot drift apart.
- **Linux was the question that started the track**; the answer (research doc
  §4): no distribution packages a `mindfork` (nearest neighbour —
  MindForger's `mindforger`; checked 2026-08-24), package managers swap the
  renamed paths cleanly on upgrade, and `current_exe()` never depended on the
  file name — `/proc/self/exe` resolves the symlink to
  `/usr/lib/mindfork-rs/` whatever the binary is called. The would-be
  transition costs (user scripts spelling the old command, a stale desktop
  pin, a tarball unpacked over an old one) are all voided by the pre-release
  timing.
- **Verification**: no live engine run (engine/memory/tools untouched — the
  renamed `clientInfo`/`User-Agent` strings change no protocol);
  `target\debug\mindfork.exe --version` prints `mindfork 0.9.7`; the
  container install smokes and the `.iss` compile ride the PR's
  `packaging.yml`. **Gate green**: 2556 unit tests (one new — the name gate),
  107 `#[ignore]`; `fmt`/`clippy -D warnings`/`cyrillic_scan`/`link_check`/
  `doc_index_check` clean.

### Release 0.9.8 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.9.8`): bumped `Cargo.toml` `0.9.7 → 0.9.8` (+ `Cargo.lock`),
  `CHANGELOG.md` — `[Unreleased]` → `[0.9.8] — 2026-08-27`, a fresh empty
  `[Unreleased]` opened, comparison links updated. The `v0.9.8` tag is applied
  by the user after the merge (the agent doesn't push tags/`main`).
- **What the section carries**: the two big tracks — the code workspace
  (`/project attach` through the typed build/run/test command lines and the
  `F4` changes screen) and the sub-agent chats (the sub-agent with the agent's
  tools; transcripts as nested, searchable, auto-titled conversations, live
  while they run) — plus Tavily-backed web search, the `mindfork` binary
  rename, the `auto` theme's OSC 11 detection, per-screen `F1` help, the
  Russian legal texts, split-GGUF support, the stage-3 commands, and a wave of
  fixes.
- **The `[Unreleased]` rubrics were reordered on the way in**: the section had
  grown as Added → Changed → Data → Fixed, and `release.yml` publishes the
  `## [0.9.8]` section verbatim as the GitHub Release body — so Data was moved
  behind Fixed to match the order the CHANGELOG header declares (Added /
  Changed / Fixed / Removed / Data / Security). No rubric merging was needed:
  one block of each across all the merged PRs.
- **The first release whose `Data` rubric describes real migrations**: chat
  files 1 → 2 (transcripts reconstructed for old sub-agent calls) and
  `settings.json` 1 → 2 (the sub-agent run-timeout rename and the raised token
  default) — the first production work for the migration framework from
  release-engineering stages 3–4. 0.9.0's `Data` text described the storage
  format; every release since had no rubric at all.
- **Site refreshed with the release** (reasoning in [website.md](website.md)):
  the 0.9.8 release post, the landing grid grown 9 → 12, the hero line, the
  fetch panel's `tools` row, and the at-a-glance article's loop section.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings`
  / `cargo test` green — **2616 unit tests, 109 `#[ignore]`**; the
  documentation gates (`cyrillic_scan`, `link_check`, `doc_index_check`,
  `list_scroll_check`, `wizard_rtf --check`) green too. No live run needed
  (version + docs + site content, app code untouched). CLAUDE.md's "## Status"
  header carries the new version (its test count and date were already
  current); README's project-status paragraph moves `v0.9.7` / 2304 / 99 →
  `v0.9.8` / 2616 / 109.

### Post-M9: `artwork/` renamed to `assets/` (done)
- **Why**: the directory had outgrown its name. It started as logo sources — the
  icon, the wordmarks, `mindfork.ico`, the generator and the brand guide — and
  since the demo-screenshots track it also carries `screenshots/`: the rendered
  PNG/SVG and the JSON frame dumps behind them. A `fontTools` script and a set of
  serialized terminal frames are not artwork, and the repository's own prose had
  already stopped calling them that — `.gitignore` says "brand assets mirrored
  from …", the mirror script is `tools/site_sync_assets.py`, `.dockerignore`
  calls the directory "a Windows-only build input". `assets/` is the word the
  repo was using; the directory name just hadn't caught up.
- **`media/` was proposed and rejected**: it means images/audio/video, which
  excludes `build-wordmarks.py` and `screenshots/dumps/*.json`, and in *this*
  repository a top-level `media/` reads as runtime media — the app attaches
  images to messages (spec §9.10). A `brand/` was rejected for the opposite
  reason: product screenshots are not brand assets, and splitting them out would
  have made the rename a restructuring.
- **The rename is a build change, not a docs change.** The directory is an input
  to three build systems, so a prose-only sweep would have left `cargo test`
  green and the installer broken: `build.rs` (`rerun-if-changed` plus the
  `winresource` icon embed), `packaging/nfpm.yaml` (six hicolor PNGs and the
  scalable SVG), `packaging/windows/mindfork.iss` (`SetupIconFile`,
  `WizardSmallImageFile` — the two backslash paths), `site.yml`'s `paths:`
  trigger, `tools/site_sync_assets.py`, `tools/screenshots.py`, and the two
  `CARGO_MANIFEST_DIR` joins in `widgets/logo.rs` and `app/demo_shots.rs`.
- **`git mv` for the 50 files** so `git log --follow` keeps working, then one
  scripted rewrite of **119 references across 29 files**, with the English noun
  protected by an assertion in the script: the OFL clause in `assets/README.md`
  and `build-wordmarks.py` — permission "to create artwork and distribute the
  resulting curves" — is the word, not the path, and stays. The only identifier
  carrying the old name, `widgets::logo::tests::glyph_matches_artwork_svg`,
  became `glyph_matches_asset_svg`.
- **Journal and history prose was rewritten too**, not just the relative links
  `link_check.py` would have caught. A past entry naming a directory that no
  longer exists costs the next reader more than a rewritten record does; what the
  entries say happened is unchanged.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **2679 unit tests, 126 `#[ignore]`**, the count unmoved by
  the rename; the documentation gates (`cyrillic_scan`, `link_check`,
  `doc_index_check`, `list_scroll_check`, `wizard_rtf --check`) green, and
  `tools/site_sync_assets.py` re-run by hand mirrors its 25 files from the new
  path. No live run needed — nothing on an engine, memory or tool path is
  touched.

### Post-M9: the dictionaries are copied more than once (done)
- **The report**: `cargo run -r` does not copy `dictionaries/` into `data/`. It
  does — exactly once. `build.rs` copies the six checked-in Hunspell files into
  `target/<profile>/data/dictionaries/` so a development build has spellcheck with
  no manual step, and on a profile directory that has never been built that is
  what happens (measured: a first `cargo check --release` on a clean tree produced
  all six, ~5 MB). What it never did was copy them **again**.
- **Cause — the script's output was not one of its inputs.** Cargo re-runs a build
  script only for the paths it declares, and this one declared `dictionaries/`,
  `assets/mindfork.ico`, `syntaxes/` and `SOURCE_DATE_EPOCH`: four sources, no
  destination. Delete `target/<profile>/data` — the ordinary way to put the app
  back to a fresh install, since settings, chats, `data.db` and `logs/` all live
  there and are all disposable — and nothing brings the dictionaries back.
  Measured on 1.96.0, on this tree: `rm -rf target/release/data`, then
  `cargo check --release` — not restored. Measured on a minimal probe crate with
  the same declaration shape, because reproducing it here would have touched
  `build.rs` and re-run the script by that alone: a `src/main.rs` edit that
  recompiles the crate does not restore it, nor does a `Cargo.toml` touch; only
  touching a **declared** input does. The build then runs with spellcheck silently
  off while `dictionaries/` sits in the repository looking innocent, until one of
  those four inputs happens to move for a reason of its own.
- **The fix is the destination declared as `rerun-if-changed` too**: a declared
  path that does not exist counts as changed, so a missing `data/dictionaries` is
  itself the reason to re-run. Declared naively that is a re-run on *every* build,
  because the copy stamps "now" on the very files cargo then compares against the
  fingerprint (measured on the probe: three consecutive no-op builds, three runs).
  So the copy became conditional — a file already there with the same length and
  no older than its source is skipped, and one that is copied is given its
  source's modification time — which leaves the destination still and cargo quiet.
  After the fix, on this tree: `rm -rf target/release/data` + a no-op
  `cargo check --release` restores all six; the build after that re-runs the
  script once (the directory was recreated, 13.9 s), and the two after **that**
  finish in 0.26 s with the syntax dump's mtime unmoved — the script did not run.
- **Why one spurious re-run is worth avoiding at all**: each run of this script
  re-stamps `MINDFORK_BUILD_EPOCH`, and a changed `rustc-env` rebuilds the crate —
  in release that is LTO with a single codegen unit, minutes of work for a build
  with no source change.
- **The destination is declared only when there is something to copy** (a
  `dictionaries/` directory in the tree): a declared path that never gets created
  would re-run the script on every build for good, which is the failure the
  conditional copy exists to avoid.
- **No unit test**: `cargo test` does not run `#[cfg(test)]` inside a build script
  — a build script is not a test target — so what stands in for one is the
  measured before/after above, plus the probe crate that isolates cargo's
  freshness rules from this repository's build.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **2728 unit tests, 125 `#[ignore]`** on Linux (the 2735 /
  128 in CLAUDE.md's status header is the Windows count, which includes the
  `cfg(windows)` tests a Linux container cannot run); unmoved either way, since a
  build script has no test target. The documentation gates
  (`cyrillic_scan`, `link_check`, `doc_index_check`, `list_scroll_check`,
  `wizard_rtf --check`) green. No live run needed — nothing on an engine, memory
  or tool path is touched.

### Post-M9: a place for the user's own dictionaries (done)
- **The gap installers stage 1 left.** P1 (the entry above) made the *bundled*
  dictionaries load under a `system`/`path` install by falling back to
  `exe_dir/data/dictionaries`, and stated the rule that a dictionary of the same
  name in the data root wins. What it did not do was give the data root a
  `dictionaries/` at all. On Windows the installed layout puts the bundled ones
  under `…\AppData\Local\Programs\mindfork-rs` — the right place for files the
  installer put there and the wrong place for the user's own, since an update or
  an uninstall owns that folder — and the data
  folder simply had no such directory, so the "a user dictionary of the same name
  wins" rule had nowhere to be exercised and adding a language nobody ships meant
  guessing.
- **`Paths::ensure_dirs` now creates it and seeds it once.** The same
  discoverability argument the empty `locales/` has carried since external
  locales — an empty directory says "put files here" — plus the part a directory
  cannot say: a `README.txt` naming the `*.aff` + `*.dic` convention, pointing at
  the LibreOffice dictionary repository, and stating that a dictionary added under
  a bundled one's name replaces it.
- **Only when the directory is created**, never into one that already exists, and
  that single rule buys both halves: a portable install (where the directory
  arrives with the dictionaries already in it) is left untouched, and after the
  first launch the file is the user's — a deleted README stays deleted, an edited
  one is not overwritten on the next start. Both are tests, because "written
  once" is exactly the kind of property a later refactor turns into "written
  every time" without any visible symptom.
- **The language is the caller's locale, not `default_language` read again.**
  The requirement was the language from `defaults.json`, and on a fresh install
  that is precisely what arrives: `main` resolves the CLI language as *settings
  language → `default_language` → the OS locale*, and a fresh install has no
  `settings.json`. Taking the resolved `&Locale` instead of re-reading the field
  differs in exactly one case, and there it is the better answer: a user who has
  since switched the interface to English gets an English file rather than the
  language the installer once wrote. It also keeps `paths.rs` from reaching into
  the i18n registry on a startup path that runs before logging.
- **Platform line endings** (`\r\n` on Windows): unlike everything else the app
  writes, this file exists to be opened in whatever text editor the OS puts in
  front of the user. The bundles hold plain `\n`; the substitution is at the
  write.
- **The invitation is not a dictionary.** The loader takes `*.aff` files with a
  matching `.dic` and ignores everything else, so a `README.txt` beside real
  dictionaries is walked past — pinned by a test in `features/spellcheck/dict.rs`
  rather than left as a property of the current `load_entry`. It does ride along
  into backups, which pack `dictionaries/` whole; at ~700 bytes that is not worth
  an exclusion.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **2733 unit tests (+5), 125 `#[ignore]`** on Linux; the
  status header goes 2735 → **2740**, its Windows count carried forward by the
  same +5 (all five are platform-neutral) plus the `cfg(windows)` tests a Linux
  container cannot run. The documentation gates
  (`cyrillic_scan`, `link_check`, `doc_index_check`, `list_scroll_check`,
  `wizard_rtf --check`) green. No live run needed — nothing on an engine, memory
  or tool path is touched.

### Post-M9: the release metadata — one product, one name, measured on both artifacts (done)
- **Why now**: stage 3 of the code-signing track
  ([docs/research/code-signing.md](../research/code-signing.md) §6.2, fork F3 —
  the user chose the brand `mindfork` on 2026-09-01). SignPath pins product name
  and version through a **file metadata restriction**, so these strings stop
  being cosmetic the moment signing lands: a disagreement between the two
  artifacts becomes a rejected signing request during a release, found by the
  person waiting to approve it with a tag already pushed.
- **What the artifacts actually said** — read off the built files, not assumed:

  | Field | `mindfork.exe` before | `setup.exe` before | now |
  |---|---|---|---|
  | `ProductName` | `mindfork-rs` | `mindfork` (inherited from `AppName`) | `mindfork` |
  | `FileDescription` | `mindfork-rs` | `mindfork Setup` | `mindfork` / `mindfork Setup` |
  | `FileVersion` / `ProductVersion` | 0.9.8 | **0.0.0.0** | 0.9.8 |
  | `CompanyName` | *empty* | `Vladimir Shylov` | `Vladimir Shylov` |
  | `LegalCopyright` | *empty* | *empty* | from `LICENSE` |
  | `OriginalFilename` | *empty* | *empty* | set on both |

  Two defects, opposite in kind. `winresource` fills the resource from Cargo
  metadata unless told otherwise, so the binary announced the **package id**
  where every user-facing surface says the brand — and `FileDescription`, the
  field Windows shows as the program name in the UAC dialog and Task Manager,
  said `mindfork-rs` too. Inno, meanwhile, defaults `VersionInfoVersion` to
  `0.0.0.0` and only *inherits* the rest (`VersionInfoProductName` ← `AppName`,
  `VersionInfoCompany` ← `AppPublisher`), so the installer shipped a zero
  version and was otherwise right only by accident.
- **The copyright is read, not written twice.** `build.rs` takes the
  `Copyright (c) …` line out of `LICENSE` (with a `rerun-if-changed` on it), so
  the year cannot go stale in a signed binary. The `.iss` cannot read a file, so
  it keeps its own copy — and a gate test,
  `credits::the_installer_and_the_binary_declare_the_same_product`, holds the
  two together along with the company, the description and both version fields.
  The brand is checked three ways in the same test, because it is written in
  three places: `credits::APP_NAME` (what the app calls itself), `PRODUCT_NAME`
  in `build.rs` (what the binary declares) and `AppName`/`VersionInfoProductName`
  in the `.iss` (what the installer declares). The
  test was checked the only way a gate is worth checking: flipping the `.iss` to
  `mindfork-rs` fails it, `left: "mindfork-rs" / right: "mindfork"`.
- **Verified end to end on Windows**: the debug `.exe` re-read after the build,
  and the installer **actually compiled** with the local Inno Setup 7.1.0
  (`ISCC /DAppVersion=0.9.8 /DBinDir=…\target\debug`) and its version resource
  read back — which is also what confirms `VersionInfoOriginalFileName` is a
  directive this compiler accepts.
- **Ordering note for the signing stage**: Inno regenerates the signed
  uninstaller stub when `VersionInfo` directives change, so the stub must be
  produced *after* this stage, never before
  ([code-signing.md](../research/code-signing.md) §6.6).
- Also: `/dist/` is now git-ignored — it is the installer's `OutputDir`, and
  compiling the script by hand (which the Inno 7 doc invites) left an untracked
  4 MB executable in the tree. 2744 unit tests green, 129 `#[ignore]`.

### Post-M9: the privacy policy in the wizard, the archive and the third translation (done)
- **Stage 4 of the code-signing track** ([code-signing.md](../research/code-signing.md)
  §8.1, F7a). The policy written in stage 1 stops being a file on GitHub: the
  Windows wizard shows it after the disclaimer, the installer leaves a copy
  beside the program, and every release archive carries it. Note that after
  F8a — the web tools becoming opt-in — the page is a **choice, not a
  requirement**: with no transfer to systems the user did not specify, the
  condition that would have demanded an install-time notice no longer applies.
- **Three things the design could not see from outside Inno.**
  `CreateOutputMsgMemoPage` takes its text as an `AnsiString` and renders RTF,
  so the file has to travel *inside* the setup — a `[Files]` entry with
  `dontcopy`, pulled out at wizard time by `ExtractTemporaryFile`. Those two
  entries must sit at the **top** of `[Files]`: with `SolidCompression`,
  extracting a file means decompressing everything listed before it, so a
  temporary file near the bottom would stall the wizard behind the whole
  payload. And the per-language choice that `[Languages]` makes for the licence
  and the disclaimer has to be made in code here, because a page built in
  `[Code]` has no `[Languages]` entry of its own.
- **`tools/wizard_rtf.py` learned pipe tables** — one bullet per row, first cell
  bold, the rest as `label: value` pairs taken from the header (trailing
  punctuation stripped, so a column asking "Holds your conversations?" does not
  produce a `?:` pair). That rendering is only honest when a row is a *record*;
  `PRIVACY.md` §4 was two independent lists laid out as a two-column table, and
  a row-wise rendering would have implied a pairing that does not exist — so the
  source became two lists, which is what it always was. The rule is written down
  in the script's own docstring, next to the subset it supports.
- **Two errors in the English original, found by the translation.** §1 pointed
  at "§8 and §9" for the website and GitHub, which are §9 and §10; and §7 said
  the machine-bound encryption "protects a copied file", where what ADR 0008
  actually claims is protection *against* a settings file carried off to another
  machine. Both fixed in both languages. A translator reading for meaning is a
  better proofreader than another pass over one's own prose.
- **Verified without installing anything.** The script compiles on the local
  Inno Setup 7.1.0, which also compiles `[Code]`, so the page's Pascal is
  checked. For the rendering, both RTFs were loaded into a **real RichEdit** —
  the same control the wizard's memo page uses — off-screen through a WinForms
  probe, and captured: headings, bold/italic, monospace paths and the hanging
  indent all land, links keep their text, and the ru file's `\uNNNN` escapes
  decode to Cyrillic. The table rendering was checked the same way on a probe
  built from §2 alone.
- **Sizes**, since they end up in every download: `privacy.rtf` 23 KB,
  `privacy-ru.rtf` 114 KB (Cyrillic quadruples through `\uNNNN` escapes), both
  compressed in the setup.
- Fixed on the way: `docs/install.md` still said the licence and the disclaimer
  are "shown in English in either wizard language", which stopped being true
  when the ru translations landed; it now lists the three read-only pages in
  order. 2744 unit tests green, 129 `#[ignore]` — no Rust changed, and the
  gates that matter here are `wizard_rtf.py --check`, `link_check` and
  `cyrillic_scan`.

### Post-M9: the dictionaries get a provenance record — and their licences (done)
- **Why**: stage 5 of the code-signing track
  ([code-signing.md](../research/code-signing.md) §3.1). The vendored grammars
  next door are exemplary — `syntaxes/SOURCES.md` pins repository, commit,
  licence and vendored licence text per file — while `dictionaries/` held six
  files and no record at all. The commit that added them (`40d3b49`, 2026-06-29)
  said only "added ... dictionaries". For a signer whose condition is "no
  component that is not open source", that is the question a reviewer asks; for
  everyone else it is plain redistribution hygiene.
- **Established by matching bytes, not by memory.** Every claim in the new
  `dictionaries/SOURCES.md` was confirmed by downloading a candidate and
  comparing sha256:
  - `en_US.*` and `en_GB.*` are **byte-identical to `ropensci/hunspell`
    `inst/dict/`** — which is also what explains the stray comment
    `# Jeroen: removed numbers from WORDCHARS for R` sitting in `en_US.aff`
    (Jeroen Ooms maintains that R package);
  - that repo's own `readme.txt` names the origin — the LibreOffice **English
    dictionaries extension 2018-11.01** — and the origin confirms it: both
    `.dic` files are **byte-identical to `LibreOffice/dictionaries` `en/` at
    `605e1d1`** (2018-10-25), and `en_GB.aff`'s header says *"V 2.66,
    2018-11-01"*;
  - `ru_RU.*` are **byte-identical to `wooorm/dictionaries` `dictionaries/ru/`**
    (`index.aff`/`index.dic`), generated in turn from LibreOffice's.
- **Both `.aff` files are modified relative to the origin**, and the same way:
  `WORDCHARS 0123456789’` → `WORDCHARS ’`, plus stripped trailing whitespace on
  ~1000 lines of `en_GB.aff`. Worth recording rather than glossing: one of the
  two licences is LGPL, where stating modifications is the point.
- **Licences, which were missing entirely.** `licenses/en_US.txt` (SCOWL —
  Kevin Atkinson, with Ispell/WordNet/12dicts notices), `licenses/en_GB.txt`
  (the LGPL statement and the Bartlett/Kelk/Brown/Pinto attribution) and
  `licenses/ru_RU.txt` (BSD 3-clause style, Alexander I. Lebedev 1997–2008, plus
  a 2012 fix by László Németh). The two English ones are the upstream READMEs
  **at our vintage's commit**, not today's: master's `README_en_GB.txt` has
  grown a changelog for releases we do not ship.
- **And they now travel.** The licences were vendored *and* wired into every
  artifact that carries the dictionaries — `stage_data` in `release.yml`, an
  `nfpm.yaml` entry, a `[Files]` line in the installer — because both licences
  require the notice to accompany a redistribution, and until now three archives,
  three package formats and one installer shipped the words without them.
  `build.rs` needs no change and gets none: `copy_dir` skips subdirectories, and
  the loader only ever pairs an `.aff` with a `.dic`, so `licenses/` is invisible
  to the dev build either way.
- **One thing deliberately not done**: the `en_GB` licence is stated as "LGPL"
  with no version, because that is what the 2018 files say — upstream first
  bundled an explicit LGPL v3 text in its 2019-03-01 release. Moving to a current
  upstream would fix that *and* change the word list (upstream's `en_GB.dic` has
  grown from 996 KB to 1.2 MB), so it is a behaviour change and belongs in its
  own PR. Recorded in SOURCES.md so the next person does not have to re-derive it.
- Verified: the installer compiles on Inno 7.1.0 with the three licence files
  compressed into it, both YAML files parse, and the archive staging was
  dry-run locally. 2745 unit tests green, 129 `#[ignore]` — no Rust changed.

### Post-M9: en_GB updated to V 4.0.9 — the licence stated, in the file itself (done)
- **The follow-up `dictionaries/SOURCES.md` had written down** when the
  provenance record landed: the British dictionary's licence was *"LGPL"* with
  **no version**, because that is what the 2018-11.01 files said; upstream only
  began shipping an explicit LGPL v3 text a release later. Fixed by moving to a
  current upstream, which is a data change and therefore its own PR.
- **Taken from the author's own repository this time** —
  `marcoagpinto/aoo-mozilla-en-dict`, **V 4.0.9 (2026-09-01)**, commit
  `df5ce3e` — rather than from a packager, and **unmodified**. That is the whole
  point: the new `.aff` states its terms in its own first lines, *"Licensed
  under the GNU Lesser General Public License, version 3 or any later version"*,
  and the repository's `LICENSE` is the LGPL v3 text, vendored beside it. The
  ambiguity is gone from the file, not merely from our description of it.
- **The variant was the trap, and it is invisible until a user is told their
  spelling is wrong.** Upstream publishes three British word lists — `-ise`,
  `-ize` (Oxford), and `-ise -ize` which accepts both. Ours has always been the
  third. The obvious source for a bump, `LibreOffice/dictionaries` `en/`, has
  meanwhile moved to the **`-ise`-only** list: measured, it answers *no* to
  `organize`. Taking it would have quietly started flagging *organize* for every
  British user while looking like a routine update. Marco's `en_GB (-ise -ize)`
  is the matching list; measured before installing (`organise` yes, `organize`
  yes, `color` no).
- **`WORDCHARS` no longer needs the packager's edit.** The old file carried
  `WORDCHARS ’` instead of upstream's `WORDCHARS 0123456789’` — a change made
  for the R package. It changes nothing here: this app segments words itself
  (`spellcheck::segment`, on `char::is_alphabetic`) and hands the checker one
  word at a time, so Hunspell's own tokenization, which is what `WORDCHARS`
  governs, never runs. Which is why the upstream file could be taken verbatim —
  fewer modifications to state under a licence that asks for them.
- **A gate that would have caught all of this**:
  `the_bundled_dictionaries_load_and_answer` loads the **repository's own**
  `dictionaries/`, not a fixture, and pins one marker per plausible mistake — a
  pair that no longer loads, the wrong English variant (`organize` vs
  `organise`), a language mixed up (`colour` vs `color`), and a `.dic` that
  parsed to nothing. Every other test in that file writes its own fixture, so
  nothing noticed when the *data* changed; a swapped upstream, a wrong variant
  or an `.aff` the parser dislikes would have sailed through a green suite and
  surfaced as "spellcheck went quiet" on someone's machine. Note that the new
  `.aff` begins with a BOM, which `spellbook` turns out to accept — checked by
  running it, not by reading the parser.
- **Cost**: the word list grows from 87 455 stems to 101 294, the `.dic` from
  996 KB to 1.29 MB. It ships in every archive, package and installer, so that
  is ~300 KB before compression on each. 2746 unit tests green, 129 `#[ignore]`.

### Release 0.9.9 (prepared)
- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.9.9`): bumped `Cargo.toml` `0.9.8 → 0.9.9` (+ `Cargo.lock`),
  `CHANGELOG.md` — `[Unreleased]` → `[0.9.9] — 2026-09-13`, a fresh empty
  `[Unreleased]` opened, comparison links updated. The `v0.9.9` tag is applied
  by the user after the merge (the agent doesn't push tags/`main`).
- **What the section carries** — 147 PRs in the seventeen days since 0.9.8,
  96 items: the Python file exchange track (files into and out of the sandbox,
  the local interpreter on the same contract, `/file open`, the packed
  read-only package image, the starter packages), background runs (background
  sub-agents and dialogues, the tasks screen, `/tasks stop`, parallel runs,
  tool calls and sessions with the context-pool guard, the yielding background
  requests, the CPU batch and the slow-prompt note), the directed dialogue, the
  engine download and the resolved binary field, `/continue`, the encoding
  fixes, the security defaults (web tools off, the Tavily key, the verified
  Python download, Protected View), the privacy policy and the dictionaries'
  provenance, and a long wave of fixes.
- **The `[Unreleased]` rubrics had to be merged, not just reordered.** The
  section had grown as Fixed / Added / Changed / Security / Fixed / Data /
  Changed / Fixed — three Fixed blocks and two Changed blocks, because each
  PR added its item to a rubric block it found or opened, and a rubric opened
  at the end of the section collected the items of the PRs that followed it.
  Two items had landed under Data that way: `/continue` and the external
  server's model name (PRs #401–#404, merged right after the Data header was
  opened). `release.yml` publishes the `## [0.9.9]` section verbatim as the
  GitHub Release body, so the blocks were merged into the header's order
  (Added / Changed / Fixed / Data / Security; no Removed), newest block first
  within each rubric, and the two items moved to Added. The item texts are
  untouched: a script did the slicing, and the set of non-blank lines before
  and after was diffed and found identical.
- **The `Data` rubric**: the `files/` directory beside `chats/` (no migration),
  and chat files 2 → 3 (a stamp for dialogue transcripts) → 4 (the repair of
  titles the old 100-character limit had cut), both through the
  backup-then-migrate path.
- **Site refreshed with the release** (reasoning in [website.md](website.md)):
  the 0.9.9 release post, three landing cards extended in place, and a
  paragraph in the at-a-glance article. Zola was not run locally (not
  installed here); the site workflow's PR gate builds the site at the pinned
  0.22.1.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings`
  / `cargo test` green — **3217 unit tests, 176 `#[ignore]`**; the
  documentation gates (`cyrillic_scan`, `link_check`, `doc_index_check`,
  `list_scroll_check`, `wizard_rtf --check`, `site_legal_pages --check`) green
  too. No live run needed (version + docs + site content, app code untouched).
  CLAUDE.md's "## Status" header moves to 2026-09-13 / 0.9.9 / 3217 / 176;
  README's project-status paragraph moves `v0.9.8` / 2616 / 109 → `v0.9.9`
  / 3217 / 176.

### Post-M9: public release readiness — the audit and stage 1 (done)
- **A read-only audit before the repository goes public**, in six parallel halves
  (repository and history; public documents; CI, releases and supply chain; the
  security of the shipped defaults; first run and code robustness; website and
  branding) plus Sonar on `main`. Result, plan and forks:
  [public-release-readiness.md](../research/public-release-readiness.md). The
  headline: the code is not the problem (Sonar clean, no secret ever committed);
  the twelve blockers sit around the flip itself (the owner's settings, the order,
  old release assets shipped without the dictionaries' licences), a first minute
  that dead-ends, and a whole disk one settings switch away. Stages 2–6 are listed
  there, each to get its own forks.
- **Stage 1 — three blockers in one PR** (fork F4(a)).
  - **B8, a launch without a terminal (F1(a)).** Measured first: the debug binary
    with stdin from `/dev/null` and stdout into a file entered the alternate
    screen, wrote 4 677 bytes of escape codes, created `data/` next to itself and
    ran until `timeout` killed it at 20 s. `real_main` now refuses `Run` and `Demo`
    when stdout is not a terminal — before anything touches the disk — with one
    localized line and exit code 2. Only stdout is asked: crossterm reads keys from
    the console itself (`CONIN$` in crossterm_winapi 0.9.1, `/dev/tty` in crossterm
    0.29 when stdin is not a tty — both read in the vendored sources), so a piped
    stdin is a launch that works. Re-measured on the fixed binary: exit 2 in about a
    second, 0 bytes on stdout, no `data/`; `llama installed` redirected still works.
  - **B9, the first run (F2(a)).** While the chat server reports `NotConfigured`,
    the feed's empty placeholder lists the routes — a cloud provider through
    `Ctrl+P` or `/settings`, a local build through `mindfork llama setup` plus a
    GGUF, `mindfork demo` — and the send error names the settings instead of "LLM
    server is not configured". The screen starts at `Connecting`, so a configured
    install never flashes the routes; a screen test pins that. README: the demo in
    the first screen, a cloud model id stated as required, where a GGUF comes from.
  - **B11, PRIVACY.md (F3(b) — the user's decision, against the recommendation).**
    A line-by-line check against the code found 16 statements false, incomplete or
    stale, each re-read in the code before the text changed: "Cloud providers are
    never probed" (Grok runs on `OpenAiClient` and receives `/props` and
    `/models`), thirteen wheels (34), the llama.cpp download missing from the
    "complete list", `fetch_url`'s whole-page attachments, the environment an MCP
    server inherits, what a backup holds, the Python working folder, among others.
    `llama setup` stopped reading `GITHUB_TOKEN`, and its rate-limit message stopped
    suggesting it. Same seam, folded in: a failed chat search logged its query text
    against §6 — it now logs the length. English and Russian edited in step; the
    site page and both installer RTFs regenerated.
- **Left for a later stage, on purpose:** a managed engine that resolves a build
  but has no GGUF may start `llama-server` in its model-less router mode rather than
  fail, so what the user then sees is unmeasured — stage 4 measures it before its
  message is touched.
- **Gates**: `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` /
  `cargo test` green — **3276 unit tests, 186 `#[ignore]`** (+4: the launch
  decision, two feed placeholder tests, the screen threading the status);
  `cyrillic_scan`, `link_check`, `doc_index_check`, `wizard_rtf --check` and
  `site_legal_pages --check` green. No engine live run: startup, UI and documents
  only; the headless re-measurement stands in for B8, and the empty state in a real
  terminal is the owner's look before merge.

### Post-M9: public release readiness — stage 3, the release pipeline (done)

- **Stage 3 of the track** ([release-pipeline.md](../research/release-pipeline.md); forks
  R1–R5 decided by the user on 2026-09-17, each at its recommendation). Stages 1 and 2
  were about the first minute of a stranger's run and what an enabled tool may reach;
  this one is about **what a stranger downloads, and what produced it** — B5, B6 and B12
  of the audit plus the licence, supply-chain and dependency lines under "Should".
- **B12 — the Windows binary needed a runtime we do not ship.** Measured on the 0.9.9
  release binary: it imports `VCRUNTIME140.dll` (the Visual C++ redistributable's; a clean
  Windows does not carry it, and neither the archive nor the installer brings it) and
  eleven `api-ms-win-crt-*` stubs (the Universal CRT — a Windows component since 10, so
  those were never the problem). One DLL between a download and a machine that has never
  installed a C++ application. With `-C target-feature=+crt-static` it links, the graph's C
  dependencies (`onig`, `libsqlite3-sys`, `sqlite-vec`) pick the static runtime up through
  the `cc` crate, **neither name is imported any more**, and it costs 365 KB (+1.4 %). The
  flag lives in `.cargo/config.toml` for the msvc target rather than in the release
  workflow (fork R1), so the 3288 tests run on the linkage the artifact ships — measured
  green locally before the change was written down. The caveat is in the file: cargo does
  not merge this with a `RUSTFLAGS` set in the environment, which replaces it wholesale.
- **B6 — a tag published a live release with nothing checking it.** `gh release create`
  ran without `--draft`, and the version came from the tag with nothing comparing it to
  `Cargo.toml`; the notes came from an `awk` that silently wrote "Release vX.Y.Z." when it
  found no section. So a tag one digit off shipped binaries whose `--version` contradicted
  their own page, and a forgotten CHANGELOG rename shipped an empty one — both discovered
  by whoever downloaded it. `tools/release_guard.py` now refuses **before the first build**
  on the tag's shape, on a disagreement with `Cargo.toml`, and on a missing or empty
  CHANGELOG section; the release is created as a **draft**, so AGENTS.md §6's artifact
  smoke test happens before anyone else can see it, and publishing is the owner's click.
  A tag with a prerelease suffix (`v0.9.9-rc1`) is accepted, marked `--prerelease` and
  allowed to fall back to `[Unreleased]` — which is what makes a rehearsal of the whole
  workflow possible without inventing a version.
- **The guard's own refusals are exercised on every pull request** (`--self-test`, in
  ci.yml's `lint` job): a workflow's error paths are otherwise only ever reached by a
  release that fails, which is the worst place to find out (lessons §10).
- **`nfpm` came from an unsigned repository at an unnamed version.** Both workflows added
  `deb [trusted=yes] https://repo.goreleaser.com/apt/` — `trusted=yes` is apt being told
  not to check the signature — and installed whatever was newest, into the job that builds
  the `.deb`/`.rpm`/`.pkg.tar.zst` a stranger installs. `tools/install_nfpm.sh` pins 2.47.0
  and verifies its SHA-256, shared by `release.yml` and `packaging.yml` exactly as
  `install_inno.ps1` is, so the gate cannot validate packages built by a different tool.
  Measured in an `ubuntu:24.04` container, both runs: installs, verifies, and is idempotent
  — the first spelling looked for a `version:` line that nfpm does not print (it prints
  `GitVersion:`), and the script then rejected the copy it had just installed. A tool's
  `--version` output is a measurement, not a guess.
- **The licences travel now.** Three gaps, one shape: the spellcheck dictionaries' licences
  reached every artifact, the 22 vendored grammar licences reached none (the grammars are
  compiled into the binary, so they had no file to sit beside), no artifact carried the
  texts of the 558 locked packages, and `PRIVACY.md` was in the archives and the installer
  but **not** in the Linux packages. Now: `THIRD-PARTY-NOTICES.md` generated by `cargo
  about` from the release's own `Cargo.lock` (367 KB, 215 licence sections over 362
  packages — measured), `licenses/syntaxes/` beside it, and the policy in
  `/usr/share/doc/mindfork-rs/`. The notice file is **not committed** (`.gitignore`): it is
  derived from a lock file, and the copy that matters is the one built with the release.
  `packaging.yml` generates it too — that workflow already triggers on `Cargo.lock`, so a
  dependency whose licence the list cannot account for fails on the pull request rather
  than on the tag. `build-packages.sh` refuses to run without the file and prints the one
  command that makes it.
- **Verified in a container before the rehearsal**: the packages carry `PRIVACY.md`,
  `PRIVACY.ru.md`, `THIRD-PARTY-NOTICES.md` and all 22 grammar licences under
  `/usr/share/doc/mindfork-rs/`, and the missing-notices arm refuses with exit 1.
- **Dependencies**: `quick-xml` 0.39.4 → 0.42.0, which drops both RUSTSEC ignores from
  `deny.toml`. It turned out to be ours alone — syntect taken with `default-features =
  false` no longer pulls it through plist, so the comment pinning it "to the version
  syntect resolves" described a graph that no longer existed. The bump is a real API move
  (names and text are `str` now, `decode()` is gone in favour of `xml10_content()`), three
  call sites in `doc_extract.rs`, its six tests green. `chacha20` 0.10.1 was **yanked** and
  sat in the lock through pdf-extract → lopdf → rand; updated to 0.10.2, and `yanked` in
  `deny.toml` is now `deny` rather than `warn`.
- **The rehearsal earned its keep on the first try.** `v0.9.9-rc1` proved the parts that
  matter from the run's own artifacts — the CI-built `mindfork.exe` imports neither
  `VCRUNTIME140.dll` nor any `api-ms-win-crt-*`; the notices came out 359 KB / 215 licence
  sections and byte-identical inside the `.deb`; `/usr/share/doc/mindfork-rs/` carries the
  privacy policy and `licenses/syntaxes/`; nfpm installed as `2.47.0` with its hash
  verified — and then **the Windows installer job failed**: Inno Setup takes digits and
  dots in `VersionInfoVersion`, and the script handed it `AppVersion` = `0.9.9-rc1`. The
  prerelease mechanism meant to exercise this workflow could therefore never have reached
  the installer, which is the one artifact whose payload cannot be checked from outside.
  Fixed by deriving a numeric `NumericVersion` for the two `VersionInfo*` fields while
  `AppVersion` keeps the suffix for the user and the file name; the gate test pins both the
  fields and the derivation, and the compile was verified locally against the pinned Inno
  Setup 7.1.0 for `0.9.9-rc2` and `0.9.9` alike.
- **Rehearsal 2 — `v0.9.9-rc2`: GO.** Every job green and the page **draft and
  prerelease**; checked on the published assets: `sha256sums.txt` recomputed and matching,
  the Windows binary *inside the archive* importing neither `VCRUNTIME140.dll` nor any
  `api-ms-win-crt-*`, `THIRD-PARTY-NOTICES.md` and `PRIVACY.md` and 22 grammar licences in
  both archives and in the `.deb`, the installer compiled from a prerelease version, and
  the attestation step **skipped** because the repository is private — the guard behaving
  as designed. Not exercised, and said so rather than implied: the `[Unreleased]` fallback
  (a `## [0.9.9]` section exists, so the guard correctly preferred it) and the attestation
  itself. Full table in [release-pipeline.md](../research/release-pipeline.md) §8.
- **Gates**: fmt / clippy / test green — 3288 unit tests, 188 `#[ignore]` — plus the six
  documentation gates and the two new ones (`actions_pin_check`, `release_guard --self-test`).

### Post-M9: public release readiness — stage 4a, the ways a first run ends badly (done)

- **Stage 4a of the track** ([robustness-and-defaults.md](../research/robustness-and-defaults.md);
  forks decided by the user on 2026-09-18 — F1(a), F2(a), F5(a) at the recommendation,
  **F3(b)** and **F4(c)** against it, and the stage split into two pull requests). Stage 3
  was about what a stranger downloads; this one is about the ways the first ten minutes
  end badly that are nobody's fault but ours. The engine half — a server started without a
  model, and the reply budget — is in [engine.md](engine.md); here the rest.
- **D1 — a dead core left a live interface.** `runtime.spawn(orchestrator::run(..))`
  dropped the `JoinHandle` and the UI drained events with
  `while let Ok(event) = rx.try_recv()`, which reads a **closed** channel exactly as an
  empty one. So a panic in the orchestrator restored the terminal, killed the task, and
  left the interface running and repainting with nothing behind it: every command went
  into a channel with no reader, nothing answered, and the only way out was to quit. The
  drain is now `drain_events` → `Idle | Applied | BackendGone` — a named function because
  the loop itself needs a real terminal and this decision is the part worth a test (two:
  a queue drained in one pass, and a closed channel that still applies what was queued
  before the death). The session then ends with a localized line naming the log file and
  a non-zero exit; `main` keeps the handle, so the panic reaches the log rather than the
  alternate screen that ate it.
- **D3 — a fatal error vanished with its window.** Double-clicked from Explorer, a console
  application gets a console of its own that Windows destroys at exit, so every refusal
  printed before the interface opens flashed by unread — indistinguishable from "it does
  nothing when I click it". `GetConsoleProcessList` answers whether that is the case, and
  it was measured rather than assumed: **3** processes attached when started from a shell,
  **1** in its own console. `shared/console.rs` waits for Enter when the count is 1 **and**
  stdout is a terminal — the second condition is not a formality, since a redirected
  launch would otherwise hang a script. Verified both ways on the built binary: launched
  with a wrong flag in its own console it was still alive after four seconds; from a shell
  the same refusal printed and exited 2 immediately.
- **D4 — "already running" exited 0**, telling whatever started it that the app had run
  and finished cleanly. It is 2 now, the code the CLI already answers a wrong invocation
  and a launch without a terminal with. It is also the likeliest double-click of all — the
  user clicks the shortcut again because the first window is behind something — so it is
  one of the places D3 holds the window.
- **D5 — an external locale fell back to Russian.** `REFERENCE` was `Lang::Ru` from the
  days when the sources were Russian; with `en`/`ru` at full key parity under test it was
  invisible for the built-ins and decided exactly one thing: what a `data/locales/de.json`
  shows for a key it is missing. English now — and with it the translation template
  `mindfork locales export` writes, which is the other thing the reference decides. The
  two tests that pinned the old answer were rewritten to assert the new one, including
  that it is **not** the Russian string.
- **D6 — the installer never touched `PATH`** while the app's own guidance says to run
  `mindfork llama setup`. An optional task adds `{app}` to the user's environment (the
  machine's on an "all users" install), **unchecked by default** — the user's choice
  against the recommendation, and the cautious one: an installer that edits the
  environment unasked is a surprise. `NeedsAddPath` keeps an upgrade from appending the
  same directory twice, and an uninstall takes the entry back out by hand, because
  `uninsdeletevalue` would delete the whole `Path`. Compiled against the pinned Inno Setup
  7.1.0; the install itself is the owner's check.
- **Gates**: fmt / clippy / test green — **3294 unit tests, 188 `#[ignore]`** (+6 unit
  tests, +1 live smoke).

### Post-M9: public release readiness — stage 5a, the documents a stranger meets (done)

- **Stage 5a of the track** ([public-documents.md](../research/public-documents.md);
  forks decided by the user on 2026-09-18, all four at the recommendation: **0.10.0**
  for the first public release, a manual **in the repository**, a **deep** README trim,
  and everything on the website except a community channel). Stages 1–4 changed what the
  app does; this one changes what someone finds when they arrive.
- **There was no manual, and that was measured rather than assumed.** Three documents
  hold manual-shaped content and none of them is one: the `F1` help (About / Hotkeys /
  Commands / License / Legal / Components) is a *reference* that explains no screen; the
  README's three keymap tables are the same kind of thing; and `spec.md` §11 — 1,832
  lines — is the complete description of every screen written for implementers. The
  website's nine articles cover the *concepts*. What was missing was the task-oriented
  middle, so [manual.md](../manual.md) is that: the first five minutes, the screens, chats
  and profiles, the four different memories, files and images and the code workspace, the
  tools and what each switch opens, settings worth knowing, the keys and commands (moved
  out of the README), and what to do when something goes wrong. Its labels were taken
  from `locales/en.json` rather than from memory, so "Python execution" and "Background
  runs (subagent/dialogue)" are what the screen actually says.
- **The README is a front page again**: **43,386 → 12,278 bytes**, 691 → 256 lines. The
  keymap went to the manual, "How it's built" shrank to the three decisions that shape
  the code, Development to the gates and the two GPU-free routes, and the feature list
  from 229 lines to seven bullets. [docs/README.md](../README.md) is the human index
  `CLAUDE.md` deliberately is not — two tables, "using the app" and "working on the
  code".
- **`Cargo.toml` had six fields and needed twelve.** `description` (the one sentence,
  now also in `nfpm.yaml`, which had been omitting Grok), `keywords`, `categories`,
  `readme`, `documentation`, `rust-version` — and an `include` list, which is the part
  that was measured: `cargo package` was **700 files, 26.8 MiB, 9.0 MiB compressed**,
  carrying `docs/`, `assets/`, the website and `to_main.bat` into a crate nobody would
  build them from. With the list: **367 files, 15.4 MiB, 3.8 MiB compressed**.
- **The verification build earned its minutes twice.** `cargo package` (the real one,
  not `--no-verify`) first failed to find `docs/legal/*` — the Russian legal texts are
  `include_str!`-ed into the binary by `shared/credits.rs`, so an include list without
  them is a *build* failure, not a missing document. Then it warned
  `icon not found, .exe will ship without it`: `build.rs` embeds
  `assets/mindfork.ico`, and an `include` that drops `assets/` ships a Windows binary
  with no icon. Both are in the list now, and the lesson is in
  [lessons.md](../lessons.md) §1: an `include` is a build input list, and only a
  verification build knows what is on it.
- **The rest of the audit's §2.3 documents row**: `CODE_OF_CONDUCT.md` (the Contributor
  Covenant 2.1, downloaded rather than reconstructed, with the reporting address that
  PRIVACY.md and 1160 commits already carry); `.github/ISSUE_TEMPLATE/` — a bug form
  asking exactly what CONTRIBUTING says to include, a feature form, and a `config.yml`
  routing security to the private channel; CONTRIBUTING's *"your first build, with
  nothing installed"* — clone, `cargo test`, `cargo run -- demo`, and the statement that
  a live run is required only for engine, memory and tool work; and the README's status
  line, which had said **3217 unit tests / 176 smokes** since v0.9.9 and now says the
  true 3323 / 196.
- **Tracked leftovers gone**: `run_all_tests.bat` (it carried a LAN address),
  `to_main.bat`, and `docs/ui-design/` — 7 files and ~850 KB of a design-tool export
  with a generated third-party runtime and a Cyrillic-named HTML file. They were listed
  under stage 6, the owner's flip checklist, but removing tracked files is a pull
  request, not a checklist step.
- **No live run, stated rather than skipped** (AGENTS.md §3): prose, metadata and a
  package manifest, with no engine, memory or tool path touched. What stood in for one:
  the eight repository gates, and **`cargo package` with its verification build**, which
  is what caught both defects above.
- **Gates**: fmt / clippy / test green — **3323 unit tests, 196 `#[ignore]`** (unchanged;
  this stage adds no code).

### Post-M9: public release readiness — stage 5c, the release pull request (done)

- **The release pull request for 0.10.0** (AGENTS.md §6 step 1;
  [public-documents.md](../research/public-documents.md) fork F1(a)). The version
  question was the one fork of stage 5 that was not about prose: the project's own
  rule promotes to **1.0.0** *"once the track is proven in production (CI green on both
  OSes + migration scaffolding merged + release pipeline has shipped ≥1 release)"*, and
  the third condition is exactly the one this release satisfies rather than inherits —
  the pipeline has been rehearsed twice (`v0.9.9-rc1`, `rc2`, both drafts, both deleted)
  and has never published anything. So **0.10.0**, with 1.0.0 following once a release
  has actually shipped and the promise is earned in public rather than announced.
- **What the mechanical half changed**: `Cargo.toml` and `Cargo.lock`,
  `site/zola.toml`'s `app_version` (the version the site's overview says it describes),
  `CLAUDE.md`'s status header, `[Unreleased]` → `[0.10.0] — 2026-09-18` with a fresh
  empty `[Unreleased]`, and the comparison links.
- **The section opens with a highlights paragraph** (N8 of the design doc), because the
  release body *is* that section verbatim — `tools/release_guard.py` cuts it out of the
  file and hands it to `gh release create --notes-file`. Without one, the first public
  release page would open on `### Added`. Measured after writing it: the guard accepts
  `v0.10.0`, matches it against `Cargo.toml`, and produces **258 lines** of notes whose
  first paragraph is the highlights.
- **What the release page will say the release is**: the invisible half (a Windows binary
  that starts on a clean machine, file tools that cannot leave the folder they are given,
  a sandbox with no route to the LAN, every dependency's licence travelling with every
  download, a tag checked against this changelog before anything builds) and the visible
  half (the model picker, a thinking model's answer no longer cut in the middle, and a
  manual).
- **Gates**: fmt / clippy / test green — **3323 unit tests, 196 `#[ignore]`**; the tag
  guard run for real against the new version.

### Post-M9: the Cargo package becomes `mindfork` (done)

The crate is renamed for crates.io, where the About dialog has been pointing all
along (`credits::CRATE_URL` → `crates.io/crates/mindfork`), and the repository
keeps `mindfork-rs`. **This revises the decision of 2026-08-24**, which kept the
package name too; the user's reason for the split is that the `-rs` is a
repository name — it says the project is written in Rust and keeps it apart from
an unrelated company of the same name — while a registry has no such ambiguity to
resolve. Cargo offers no third way: it publishes strictly under `[package] name`,
so the linked URL and the package name are the same decision. Recorded in
[../research/binary-rename.md](../research/binary-rename.md) §10, where §1, §7 and
§9 are amended rather than left to contradict it.

- **Measured before deciding it was safe.** The three identifiers that name
  something already on a user's disk — the data directory
  (`ProjectDirs::from("", "", "mindfork-rs")`), the API-key crypto strings
  (`ENTROPY`, `HKDF_INFO`, `CHECK_PLAINTEXT`) and the single-instance lock id —
  are frozen literals, each with a comment saying why, and **none** is derived
  from `package.name`. So the rename moves no data and invalidates no stored key.
  The Linux package, `/usr/lib/mindfork-rs/`, the release assets and the Sonar
  project key keep the repository's name as well.
- **The one change that was not cosmetic**: `logging.rs` documents the
  `MINDFORK_LOG` filter as `mindfork_rs=debug`, and an `EnvFilter` target is the
  *crate* name — after the rename that spelling matches nothing. A stale comment
  that would have sent a user to an empty log, rather than one that merely looks
  old.
- **The gate now ties three names instead of two**:
  `assert_eq!(env!("CARGO_PKG_NAME"), APP_NAME)` in place of a literal, beside the
  existing `[[bin]] name` check. `[[bin]]` is redundant now and stays: it is what
  the gate reads, and it keeps the command from following a future package rename.
- **Verification**: `cargo publish --dry-run` — 368 files, 15.4 MiB (3.8 MiB
  compressed), the verification build of `mindfork v0.10.0` green, upload aborted
  by the dry run; fmt, clippy, **3326 unit tests**. No live run applies: a package
  name is read by Cargo and by nobody at runtime.
- **Deliberately not done here**: the publish itself (the owner's, AGENTS.md §5),
  and with it the CHANGELOG line, the README badge and `cargo install mindfork` in
  install.md. They belong to the next version bump — `v0.10.0` already tags a tree
  whose package was `mindfork-rs`, and until the crate is on the registry each of
  those lines is a promise the registry does not keep.

### Post-M9: public release readiness — stage 6, the flip (done)

The last stage of the track, and the only one that is a checklist rather than a
pull request: irreversible, outward-facing actions, taken in an order that put
every reversible check first. Full record —
[../research/public-release-readiness.md](../research/public-release-readiness.md) §5.

- **The audit came before the deletions, and both before the flip.** Every text
  blob in the object database (**9 229**, over 1 776 commits) and every piece of
  GitHub-side prose (**582** pull requests, 286 comments, 11 releases) was scanned
  for nine credential shapes: **zero hits**. The blocker's named worry — the
  Hugging Face namespace that `tools/hf_api.py` prints into every sweeper log —
  turned out to be `vshylov`, the owner's own GitHub login, so the flip disclosed
  nothing the repository URL did not. What the pass did surface was unasked for:
  222 of 582 pull requests are in Russian, and the notes of v0.9.0–v0.9.2 entirely
  so.
- **B2 was wider than B2 said.** The pre-0.9.9 release assets carried the
  spellcheck dictionaries without their licence texts — measured in the files, not
  inferred from the commit date: at v0.9.8 `en_GB.aff` had a one-line "Released
  under LGPL" and `en_US.aff`/`ru_RU.aff` had nothing. The same payload sat in
  **183 Actions artifacts** (2.8 GiB) that the blocker never mentioned and that a
  public repository serves to anyone until retention expires. Deleted: 59 assets
  (484 MiB, 8 downloads ever) and those 183 artifacts. The residue is written
  down rather than glossed: GitHub's generated source archives for those tags
  still contain the dictionaries, and removing them would mean deleting the tags.
- **Two measurements that are not in any document.** A draft release created
  *before* immutability is enabled **does** become immutable when published
  (v0.10.0: `draft=false, immutable=true`) — the docs are silent on it. And
  enabling `secret_scanning_validity_checks` or
  `secret_scanning_non_provider_patterns` returns 200 while leaving both
  `disabled`: paid features, refused without an error.
- **A new rule collided with the project's own procedure.** The `v*` tag ruleset
  forbids deleting a tag, while AGENTS.md §6 rehearses the release pipeline with a
  `v<version>-rc1` tag that is inspected and then deleted. Excluded `v*-rc*`,
  `v*-alpha*`, `v*-beta*`: release tags untouchable, rehearsals disposable.
- **B1 closed with a stranger's view**: no token, and the repository, `/releases`,
  `/releases/latest`, the API and the manual all answer; the asset downloads and
  `sha256sums.txt` names exactly the six binaries.
- **Not done here**: `cargo publish` (the owner's, at the next bump) and the
  announcement, which is its own pull request because merging it deploys the site.

### Post-M9: crates.io — the publish becomes a button the owner presses (done)

- **The registry is the one channel this project cannot take back.** A GitHub
  release is built as a draft, smoke-tested and published when it looks right,
  and a bad one can be deleted; a crate version that has been uploaded can only
  be **yanked** — the files stay downloadable, and the number never becomes free
  again. That asymmetry is the whole design of
  `.github/workflows/crates-io.yml`: it does **not** run on the `v*` tag push
  that starts `release.yml`, because between that tag and a release anyone
  should trust sits AGENTS.md §6 step 5, the artifact smoke test. It runs on
  `release: published` — the owner's own act, the one that ends that step — so
  what reaches the registry is the tree the published binaries were built from,
  and one action feeds both channels. A prerelease (the `v<version>-rc1`
  rehearsal of §6, whose tag is deleted afterwards) is excluded by the job's
  `if`.
- **And a rehearsal of its own.** `workflow_dispatch` defaults to
  `dry_run: true`: every step, including cargo's verification build of the
  packaged sources, stopping short of the upload. Turning it off publishes the
  checked-out ref — the way to put on the registry a version that has no release
  of its own to hang off. Both are the owner's press; the agent publishes
  nothing (AGENTS.md §5), which is why this stage delivers the button and not
  the crate.
- **Measured before any of it was written.** The name `mindfork` was still free
  on 2026-09-19 (`crates.io/api/v1/crates/mindfork` → 404), and
  `cargo publish --dry-run` is clean: **368 files, 15.4 MiB (3.8 MiB
  compressed)**, well inside the registry's 10 MiB limit, with the verification
  build finishing. What is in those 368 files matters more than their size:
  `dictionaries/licenses/*.txt` and `syntaxes/licenses/*.txt` are among them.
  Stage 6 deleted 59 release assets and 183 Actions artifacts precisely for
  carrying the dictionaries **without** those texts, and a crate is
  redistribution like any other — the `include` list written for the package
  rename already carried them, and this is the check that it did.
- **Three refusals before the build, not after it.** The same
  `tools/release_guard.py --tag` the release pipeline runs (on a release event
  with the release's own tag, so a tag disagreeing with `Cargo.toml` is caught
  here too; on a dispatch with a tag synthesized from the manifest, where what
  is left to check is that the CHANGELOG has a section for the version). Then a
  `404`-or-refuse question to crates.io about the exact version — placed
  *before* the toolchain and the build, so a re-run over an already-published
  version answers in seconds instead of spending the build and being refused by
  the upload. Then an empty-token check on the real path only, since the
  rehearsal needs no credential. The version itself is read by importing the
  guard's own `cargo_version`, rather than by a second parser that could drift.
- **Two small measurements behind the step bodies.** `release_guard.py --tag`
  always writes `notes.md` into the working directory — the file
  `gh release create --notes-file` is given — and this job wants the verdict,
  not the notes; it removes it. That turned out not to be load-bearing: with an
  untracked `notes.md` present, `cargo package` still packaged, because
  `Cargo.toml`'s `include` is an explicit list and `notes.md` is not in it. The
  second: the verification build needs `libasound2-dev` like every other Linux
  job here, since the package it compiles is the whole application, TTS
  playback included.
- **Every arm driven locally** before the workflow could ever run: the version
  step on a release event and on a dispatch; the guard on a matching tag and on
  `v0.9.1` (refused, naming both versions); the registry question against
  `mindfork 0.10.0` (404 → pass) and against `serde 1.0.0` (200 → refused); the
  publish step as a rehearsal, with a token, and with the token missing. The
  workflow parses, and `actions_pin_check.py` sees 41 references, all pinned.
- **Why the first publish is not 0.10.0 from `main`, and this is the
  measurement that says so.** `v0.10.0` tags a tree whose package was still
  called `mindfork-rs`; `main` is that tree plus the rename and a batch of
  Dependabot merges. Between the two locks **42 dependency versions differ**, 10
  packages appear and 3 disappear (`digest` 0.10 → 0.11, `base64` 0.22 → 0.23,
  `sha2`/`hkdf` a generation up). Publishing `0.10.0` from `main` would put on
  the registry a version that no tag matches and that no released binary was
  built from — and, unlike everything else in this pipeline, it could never be
  corrected afterwards. So the first published version rides the next release,
  and with it the things that are broken promises until the crate is really
  there: the CHANGELOG line, the README's version badge and
  `cargo install mindfork` in install.md.

### Release 0.10.1 (prepared)

- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.10.1`): bumped `Cargo.toml` `0.10.0 → 0.10.1` (+ `Cargo.lock`),
  `site/zola.toml` `app_version`, `CHANGELOG.md` — `[Unreleased]` → `[0.10.1] —
  2026-09-19`, a fresh empty `[Unreleased]` opened, comparison links updated. The
  `v0.10.1` tag is applied by the user after the merge (the agent doesn't push
  tags/`main`). Gates green: 3326 unit tests, `clippy -D warnings`, `fmt`, and
  `release_guard.py --tag v0.10.1` accepting the tag this release will carry.
- **PATCH, and the reason is worth stating**: while at `0.x` a MINOR carries
  features (§6), and this release adds none. What it adds is a **distribution
  channel** — the first tag whose publication also puts the crate on crates.io
  through `.github/workflows/crates-io.yml`. The order matters and is the whole
  point of that workflow's trigger: merge → tag → `release.yml` builds the draft
  → the artifact smoke test → the owner publishes → the crate goes up.
- **This is the release that makes three sentences true.** The README's
  `crates.io` badge, `docs/install.md` §1's `cargo install mindfork` section and
  the CHANGELOG line were deliberately withheld until the version that actually
  publishes (the `crates.io` pull request said so), because each of them is a
  promise a reader can test in one command.
- **And the install section says what `cargo install` does not give you**,
  measured rather than assumed: the dictionaries are copied *next to the binary*
  by `build.rs` at build time and `cargo install` keeps only the executable, so
  spellcheck starts off; and the default layout is portable, so the data
  directory lands in `~/.cargo/bin/data/`. Both are consequences of decisions
  taken long before the registry existed for this project, and a reader who
  meets them in the wild without warning would reasonably call them bugs.
- **The publish step's default was moved to the reversible side** before this
  release could use it. It read "rehearse when `dry_run` is `true`, otherwise
  upload", which makes an empty or malformed input fall towards the one action
  in this pipeline that cannot be taken back. It now reads "upload on a
  published release, or on a dispatch that *says* `false`" — everything else
  rehearses. Driven through all seven (event, input, token) combinations: the
  two that upload are the two that mean it. GitHub does always deliver a
  declared input's default, so this fixes no observed failure; it removes the
  need to depend on that when the cost of being wrong is a permanent version.
- **A detail confirmed from the other side.** The entry above recorded that an
  untracked `notes.md` does not make `cargo package` refuse, because
  `Cargo.toml`'s `include` is an explicit list and that file is not in it. This
  release showed the same rule the other way round: run with the version bump
  still uncommitted, `cargo publish --dry-run` stopped and named exactly
  `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and `README.md` — the four dirty
  files that *are* in `include`. The dry run belongs after the commit, which is
  where the workflow does it anyway (it publishes a checkout of a tag).
- **One rubric was measured and then not written.** `quinn-proto` 0.11.14 →
  0.11.18 arrived as a **security** release (three advisories), which is the kind
  of thing a CHANGELOG's `Security` section exists for. But `cargo tree -i
  quinn-proto` prints "nothing to print" — on every target and every edge kind:
  the package is an optional dependency of `reqwest` that this build never
  enables, so it sits in `Cargo.lock` and not in the binary. A `Security` line
  would have claimed a fix no user of this program ever needed. The lock entry is
  updated anyway, because a lock that lags is a lock nobody trusts.
- **What else landed since v0.10.0 and is deliberately absent from the notes**:
  the Dependabot batch as such, the `CI gate` job and the branch ruleset behind
  it, the stage-6 record and the launch post. A changelog is what a user's
  installation does differently, and none of those change it.
- **Published, and the numbers of it.** Publishing the GitHub release at
  00:12:01Z started `crates-io.yml`, and `mindfork 0.10.1` was on the registry at
  00:14:11Z: 3 941 402 bytes, MIT, one binary target, `rust-version` 1.96, not
  yanked, the declared keywords and categories intact. That run was the first
  execution of the branch no rehearsal can reach — `dry run false`, the upload
  taken, the URL printed by the last step. `cargo info mindfork` resolves it from
  the index, and the README's badge renders `v0.10.1`.
- **What the registry does not show, written down so it is not mistaken for a
  failure later**: docs.rs answers `doc_status: false`, because it builds
  documentation for a library target and this crate has none. The
  `documentation` field has always pointed at the manual, and that is the link
  the crates.io page carries.

### Release 0.10.2 (prepared)

- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.10.2`): bumped `Cargo.toml` `0.10.1 → 0.10.2` (+ `Cargo.lock`,
  one line), `site/zola.toml` `app_version`, `CHANGELOG.md` — `[Unreleased]` →
  `[0.10.2] — 2026-09-20` under a two-sentence lead, a fresh empty `[Unreleased]`
  opened, comparison links updated. The `v0.10.2` tag is applied by the user after
  the merge (the agent doesn't push tags/`main`). Gates green: 3379 unit tests
  (196 `#[ignore]`), `clippy -D warnings`, `fmt`, and `release_guard.py --tag
  v0.10.2` accepting the tag this release will carry — its extracted notes read
  through, since they are the text of the draft.
- **The number is the owner's, and it bends §6 knowingly.** While at `0.x` a MINOR
  carries features and a PATCH carries fixes, and `mindfork stats` is a feature;
  the request named `v0.10.2`. What argues for it: the command is read-only and
  lives entirely outside the TUI, no storage format moves (the only new format is
  the snapshot's own, `SNAPSHOT_FORMAT`, which nothing but `--compare` reads), and
  the one change on screen is a fix. Recorded so that the PATCH is not later read
  as an oversight, or as a precedent that a track is a PATCH.
- **What the release carries**: both stages of `mindfork stats`
  ([docs/history/data-stats.md](../history/data-stats.md)), `restore`'s password
  prompt on stderr with the saved-password refusal named as such, and the
  one-frame chat switch. No `Data` rubric, because there is nothing under it: no
  schema constant changed between `v0.10.1` and this commit (checked in the diff,
  not assumed), so 0.10.1 and 0.10.2 open the same files and no migration runs.
- **What else landed since v0.10.1 and is deliberately absent from the notes**:
  the site's structured data, its `Cache-Control`, IndexNow, `/llms.txt` and the
  Search Console record; the README badges; the crates.io item closing in the
  roadmap. A changelog is what a user's installation does differently, and none
  of those change it.
- **The release post ships in this pull request**, as its own commit — 0.9.9's
  shape (#548). It has a consequence worth writing down once: `site.yml` deploys
  on the merge, and the release is a draft until the artifact smoke test and the
  owner's publication, so for that window mindfork.io says "0.10.2 is out" about
  a release the releases page does not list yet — and `cargo install mindfork`,
  which the post names, lags by the few more minutes `crates-io.yml` takes after
  the publication. `app_version` has had the same window since §6 put it in the
  release PR. It is short, it is the owner's to close, and splitting the post
  into a second pull request merged after the publication is the alternative if
  it ever matters; it was not taken here.
- **`release_guard.py --tag` writes `notes.md` into the working directory** —
  right for the workflow, which reads it in the next step, and a stray untracked
  file when the guard is run by hand as the checklist's gate. Removed before the
  commit; nothing ignores it, so `git status` is what catches it.
- **This is the second release whose publication also uploads the crate.** Nothing
  about that path changed since 0.10.1 proved it: merge → tag → `release.yml`
  builds the draft → the artifact smoke test → the owner publishes →
  `crates-io.yml` uploads `mindfork 0.10.2` from the same tag.
- **The draft, checked before anyone else could see it** (§6 step 5). `v0.10.2` sat
  on the merge commit `4b30a64d`, which was `origin/main`; `release.yml` ran
  17:13:05Z → 17:28:37Z, every job green, and left a draft that was not a
  prerelease with the seven assets 0.10.1 has, each about 1 % larger. Its notes
  were the CHANGELOG's `[0.10.2]` section verbatim plus the footer. All six lines
  of `sha256sums.txt` equalled the `digest` GitHub computes per asset on upload,
  and the downloaded Windows archive hashed to the same value locally — three
  witnesses, one number. From the archive: `mindfork --version` says 0.10.2; the
  image names no `vcruntime`/`msvcp`/`api-ms-win-crt` DLL, so the static CRT of
  the release-pipeline stage holds; a launch with no terminal refuses with exit
  code 2; `stats` on the empty portable root says there is no data and creates
  nothing (the file set hashed equal before and after).
- **And the release's own feature, run by the shipped binary on real data.** A
  `defaults.json` beside the unpacked executable pointed it at the development
  data root (`mode: path`) — 233 chats, 1198 messages. Its `stats` output was
  byte-identical to the development build's, fingerprint included; names, sizes
  and modification times under the root were equal before and after both runs,
  which is "only reads" measured on the artifact rather than on the build that
  passed the tests; and a `--json` snapshot taken by the release binary (about
  300 KB, no message-text field in it) compared *identical* under both binaries.
  One trap for whoever repeats this: that root's interface language is `ru`, so
  the summary is in Russian, and a grep for "fingerprint" finding nothing is not a
  failure. The snapshot (it holds chat titles) and that `defaults.json` were
  deleted afterwards — left in place, the unpacked executable would open the real
  development data for anyone who launched it. Not done from the agent's shell:
  the interactive TUI run and the installer, which stay with the owner.
- **Published, and the numbers of it.** The owner published the release at
  17:35:51Z; it is `immutable` and the repository's latest. `crates-io.yml`
  started two seconds later, and `mindfork 0.10.2` was on the registry at
  17:38:28Z: 375 files packaged — the count the pull request's
  `cargo publish --dry-run` had predicted — 3 985 912 bytes (0.10.1 was
  3 941 402), MIT, one binary target and no library, edition 2024,
  `rust-version` 1.96, not yanked, the declared keywords and categories intact,
  both versions listed and 0.10.2 the default. The second execution of the upload
  branch, and nothing about it differed from the first.
- **`cargo info mindfork` must be asked from outside the repository.** Run in the
  checkout it answered `version: 0.10.2 (from .\)` *before* the crate had been
  uploaded — it resolves the local manifest first, so inside this repository it
  can never say anything about the registry. From another directory it resolves
  0.10.2 from the index. The 0.10.1 entry above cites the same command as
  evidence without saying where it was run; the registry API's version record is
  the witness that does not depend on the working directory.
- **The two badges do not move together.** A minute after the upload the README's
  release badge rendered `v0.10.2` and its crates.io badge still `v0.10.1`:
  shields.io's cache, not the registry, and not something to chase.
- **The window this entry predicted, measured.** The site's deploy finished at
  17:10:23Z and the release was published at 17:35:51Z, so mindfork.io said
  "0.10.2 is out" for **25 min 28 s** before the releases page agreed, and named
  `cargo install mindfork` for **28 min 05 s** before the registry had the
  version. `release.yml` itself is 15 min 32 s of it and the smoke test above most of
  the rest — so the window cannot be closed by hurrying, only by merging
  the post in a second pull request after the publication. At this project's
  traffic it has not been worth a second pull request; the number is here so the
  next release decides on a measurement rather than on a guess.

### Post-M9: the README's screenshots are the dark ones (done)

- **The shot should look like the thing it shows.** The five README captures
  were a `<picture>` pair switched by `prefers-color-scheme`, so a reader whose
  system is light saw mindfork rendered on a white terminal — a configuration
  almost nobody runs. A terminal screenshot is not a page element that should
  follow the page's theme: the console it is a picture *of* has a dark
  background in most setups, and the dark capture is the authentic one in either
  theme. The owner asked for the dark variants; the five `<picture>` blocks
  collapsed to a plain `<img>` of `*-dark-en.png` (branch
  `docs/readme-dark-screenshots`).
- **The same README is the crates.io page, and crates.io drops `srcset`.**
  Measured on the published rendering
  (`static.crates.io/readmes/mindfork/mindfork-0.10.2.html`): the renderer
  rewrites a relative `<img src>` to an absolute
  `github.com/vshylov/mindfork-rs/raw/HEAD/…?sanitize=true`, and leaves
  `<source srcset>` exactly as written. Confirmed in a dark-scheme browser on
  `crates.io/crates/mindfork`: all **six** `<picture>` images — the wordmark and
  all five screenshots — resolved to `https://crates.io/crates/assets/…` and
  reported `naturalWidth` **0**. That URL is a 404 (checked directly, both with
  and without the `/crates/` prefix), so every visitor arriving in dark mode has
  been reading a crate page with six blank images. The collapse fixes five of
  them for free: a plain `<img>` is the one element crates.io does rewrite.
- **The wordmark stays a `<picture>` and got the URL crates.io would have
  written itself.** A logo genuinely must contrast with the page under it, so
  the theme switch is right there — but its dark `<source>` now carries the
  absolute
  `https://github.com/vshylov/mindfork-rs/raw/HEAD/assets/mindfork-wordmark-dark.svg?sanitize=true`,
  the same form crates.io emits for the light `<img>`. Verified: `200`,
  `image/svg+xml`, **2606 bytes** — byte-for-byte the committed file. This is
  the one `<picture>` left in the README, and it is the one that needs to be.
- **Nothing changed in the capture pipeline.** Both themes are still generated
  and still committed: mindfork.io inlines the light **SVG**s under its own
  theme toggle, and the drift gate sweeps the whole matrix. Only which file the
  README embeds changed.
- **crates.io will keep showing the old README until the next publish.** The
  rendering is a snapshot taken at publish time — there is no re-render for a
  crate already on the registry — so the dark screenshots reach
  `crates.io/crates/mindfork` when 0.10.3 is published, and not before. The
  GitHub README changes on merge.
- **Tests**: unchanged (3379 unit green, 196 `#[ignore]`); no code was touched.
  Gates run: `link_check.py`, `doc_index_check.py`, `cyrillic_scan.py`. **No
  live run** — a documentation PR, no engine/memory/tool path (AGENTS.md §3).

### Post-M9: `install.sh` — the release installs itself on a bare Linux box (done)

- **Stage 2 of the provisioning track**
  ([cloud-provisioning.md](../research/cloud-provisioning.md) §4.4, forks F5 and
  F6). `mindfork setup` cannot put `mindfork` on a machine that does not have it,
  and on a rented pod that machine is new at every stop. `packaging/linux/install.sh`
  is that first step and nothing more: platform check, the archive, its checksum,
  the one library a bare image lacks, a link, and — after `--` — the rest of the
  line handed to `mindfork`, which is where the real work (`setup`) stays, in
  tested Rust.
- **A release asset, by the user's choice (F5)**: `releases/latest/download/install.sh`.
  `release.yml` copies it into `dist/` *before* the checksums, so it is listed in
  `sha256sums.txt` and covered by the build attestation like every other asset —
  no new root of trust: whoever can publish a release could already ship the
  binary. The site's bucket does not become a code-distribution channel.
- **What the shell is careful about.** POSIX `sh` — verified under `dash`, which
  is what a minimal image has — and one `main` called on the last line, so a
  download cut short runs nothing. The latest tag comes from the **redirect** of
  the releases page, not the API (60 requests an hour, shared by a datacenter's
  whole address). A tag is validated before it reaches a URL or a file name. The
  archive is refused unless `sha256sums.txt` lists it and agrees — the release
  writes `<hash>  ./<name>`, and the parser reads that spelling and the two
  `sha256sum` writes elsewhere. It is unpacked beside the target and the binary
  **moved** in, because `tar` over a running binary is "Text file busy" and a
  rename is not. Whether the app can start is decided by **running it**, not by
  asking a package database: the loader names `libasound.so.2`, the script
  installs the distribution's package (`libasound2t64` before `libasound2` — the
  time_t64 trap of `nfpm.yaml`, third appearance) as root or through
  passwordless `sudo`, and otherwise stops with exit code 3 and the exact
  command. A version already in place is recognised by a marker file, so the
  line a restarted pod runs downloads nothing; `DIR/data` is never touched.
- **`--from DIR` is a feature and the test seam at once**: install from files on
  disk, no network. It is what lets `packaging/linux/install_test.sh` run 25
  scenarios anywhere — every refusal as a control arm (a tampered archive, an
  unlisted one, a missing `sha256sums.txt`, two archives and no `--version`, a
  tag with shell in it), arguments after `--` arriving as five with the one that
  has a space in it whole, a re-run that unpacks nothing, an upgrade that moves
  the marker and not the data — and what lets the **release job install its own
  archive** with its own script from its own `sha256sums.txt` before the draft
  exists, requiring the tag's version back (a rehearsal tag's suffix stripped:
  the binary's version is `Cargo.toml`'s).
- **Verified locally** in a bare `ubuntu:24.04` with no network: 25/25 with a
  real Linux binary (`libasound` absent → exit 3, the `apt-get` line named),
  22/22 without one; the redirect resolves to `v0.10.2`, whose asset names are
  the ones the script builds. **In CI** (`packaging.yml`, new `install-script`
  job): five bare images — Ubuntu 24.04 and 22.04 (the `libasound2` fallback),
  Debian 12, Fedora, Arch — run the scenarios, a real install in which the
  script installs the distribution's ALSA package itself and the app answers
  `--version`, and the download of the latest published release end to end.
  `shellcheck --severity=warning` runs on both scripts.
- **In CI, as measured** (the pull request's own `packaging.yml` run): all five
  images green on the first push. Ubuntu 24.04, Ubuntu 22.04 and Fedora each
  print `passed=25 failed=0` and turn out to be bare of `libasound` — the real
  binary does not start there, the script answers exit 3 with the command. In
  the real install the script says `libasound.so.2 is missing … — installing`,
  brings in the distribution's package (`apt` on 22.04, where the `libasound2`
  fallback is the one that exists; `dnf` on Fedora), and `mindfork 0.10.2`
  answers, through the `/usr/local/bin` link too; the second run prints
  `already installed`. With no `--version` the redirect resolved to `v0.10.2`,
  the published archive was downloaded, `sha256 ok`, unpacked, and answered
  `mindfork 0.10.2`. `shellcheck` clean.
- **Rehearsal — `v0.10.2-rc1`: GO**, 2026-09-21, on the merge commit of the pull
  request (`95aa418d`, the user's tag). Every job green and the page a **draft
  prerelease**. `install.sh` (13 460 bytes) is among its eight assets and the
  first line of `sha256sums.txt`; the digest listed there, the digest of the
  downloaded asset and the digest of `packaging/linux/install.sh` at the tag are
  the same `701cecea…`. The new step printed `passed=24` — one fewer than in a
  bare image, because the runner *has* `libasound`, so the "names the library"
  arm has nothing to name — and `installed: mindfork 0.10.2 (expected: mindfork
  0.10.2)`: the rehearsal tag's suffix is stripped as designed. **The build
  attestation covers the script**: `gh attestation verify install.sh` exits 0
  with `install.sh` among the subjects, workflow `release.yml`, ref
  `refs/tags/v0.10.2-rc1` — and exits 1 on a file the release never produced,
  which is the arm that says the 0 means something (the command prints nothing
  on success; read `--format json`). Last, the draft's **own archive installed
  by the draft's own script** in a bare `ubuntu:24.04` with no network
  (`--from`): `sha256 ok`, unpacked, the marker `v0.10.2-rc1`, and the expected
  exit 3 for the missing library. The draft and the tag are the user's to delete.
- **Owed**: the pod probe, after which install.md §3.4's recipe — written from
  RunPod's documentation and from what was measured in containers — gets its
  measured numbers.
- **Gates**: no Rust changed — **3408 unit tests, 197 `#[ignore]`**, as before;
  link / source-language / index / action-pin gates green.

### Release 0.11.0 (prepared)

- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.11.0`): bumped `Cargo.toml` `0.10.2 → 0.11.0` (+ `Cargo.lock`,
  one line), `site/zola.toml` `app_version`, `CHANGELOG.md` — `[Unreleased]` →
  `[0.11.0] — 2026-09-21` under a lead paragraph, a fresh empty `[Unreleased]`
  opened, comparison links updated. The `v0.11.0` tag is applied by the user
  after the merge (the agent doesn't push tags/`main`). Gates green: 3408 unit
  tests (197 `#[ignore]`), `clippy -D warnings`, `fmt`, and `release_guard.py
  --tag v0.11.0` accepting the tag this release will carry — its 47 lines of
  notes read through, since they are the text of the draft (and the `notes.md`
  it drops in the working directory removed, as recorded under 0.10.2).
- **A MINOR, by §6's own rule**: while at `0.x` a MINOR carries features, and
  this one is a track — `mindfork setup`, the install script, backend families —
  with three fixes to `llama setup` beside it. The owner chose the number and
  the moment (2026-09-21).
- **Why now, before the pod probe**: `main`'s README has advertised
  `releases/latest/download/install.sh` since the install script merged, and
  that address is a **404** until a release carries the asset (measured after
  the merge — `v0.10.2` has seven assets and the script is not one of them). And
  the probe that is still owed cannot be run in full against 0.10.2 at all: that
  binary refuses CUDA on Linux, has no `setup`, and ships no script. After this
  release the probe *is* the line in the README, end to end, on a rented pod —
  the acceptance test of the whole track. Nothing in the release is worse than
  what it replaces; what the probe finds goes into 0.11.1.
- **What the release carries**: the provisioning track through stage 2
  ([cloud-provisioning.md](../research/cloud-provisioning.md)) — `mindfork
  setup` with `--set` and `--verify`, `install.sh` and install.md §1/§3.4,
  `--backend` families, and the three `llama setup` fixes (CUDA on Linux refused,
  the version read as a log line with the build check skipping in silence, a
  half-uploaded nightly picked). No `Data` rubric, because there is nothing under
  it: no schema constant and no config field changed between `v0.10.2` and this
  commit (checked in the diff, not assumed), so 0.10.2 and 0.11.0 open the same
  files and no migration runs.
- **Deliberately absent from the notes**: the site's release gate and its
  rehearsal, the README's dark screenshots, a CI action bump, the research
  documents. A changelog is what a user's installation does differently.
- **The first release with eight assets**, and the first whose `release` job
  installs its own archive with its own script before the draft exists
  (rehearsed GO on `v0.10.2-rc1`). The draft's smoke test (§6 step 5) gains the
  script's checks: one digest three ways, the attestation read with `--format
  json` and a control arm, the archive installed `--from` the downloaded assets
  in a bare container.
- **The release post ships in this pull request**, as its own commit — and for
  the first time its merge does **not** put it on the site: `site.yml` now holds
  the deploy while `Cargo.toml` names an unpublished version
  ([site-waits-for-release.md](../research/site-waits-for-release.md)). This is
  the first real hold; its timings belong in the entry that records the
  publication. The site's install page gains the one-liner in the same commit.
- **The draft, checked before anyone else could see it** (§6 step 5). `v0.11.0`
  sat on the merge commit `0c52a4bb`, which was `origin/main`; `release.yml` was
  green and left a draft that was not a prerelease, with **eight** assets — the
  seven 0.10.2 has, and `install.sh`. Its notes were the CHANGELOG's `[0.11.0]`
  section byte for byte (51 lines, compared against the guard's own extraction).
  All seven lines of `sha256sums.txt` equalled the `digest` GitHub computes per
  asset on upload, with nothing listed that is not an asset — checked without
  downloading a binary. The job's new step said `passed=24 failed=0` and
  `installed: mindfork 0.11.0 (expected: mindfork 0.11.0)`. For the script, one
  digest three ways (`701cecea…`: the list, the downloaded asset, the file at the
  tag), and `gh attestation verify` exit 0 with eight subjects, `install.sh`
  among them, `release.yml` at `refs/tags/v0.11.0` — against exit 1 for a file
  the release never produced. **Two** attestations answer for the script, not
  one: it is byte-identical to the rehearsal's, an attestation is bound to a
  digest, and the rehearsal's outlived the deletion of its draft and its tag.
  Then the draft's own archive installed by the draft's own script in a bare
  `ubuntu:24.04` with no network: `sha256 ok`, unpacked (the binary, the
  dictionaries, the licences, the documents), marker `v0.11.0`, and the expected
  exit 3 for the missing library.
- **The released Linux binary, run on Linux** — a first for this checklist, in
  the cached lab image, which has `libasound`: installed `--from` the draft's
  assets, it answers `mindfork 0.11.0`; a launch with no terminal refuses with
  exit 2; bare `setup` is help and exit 2; `setup --model … --ctx … --set …`
  writes its three settings; a mistyped `--set` key is refused by name. And
  **`llama backends` on Linux lists `cuda-12.8 727 MB (+ CUDA runtime)` and
  `cuda-13.4 562 MB (+ CUDA runtime)`** for build `b11081` — stage 0's fix,
  measured on the shipped artifact against the live release list, where 0.10.2
  printed "no CUDA runtime published". The same listing is the argument for
  families made by upstream on the day of the release: Linux CUDA 13 had been
  `cuda-13.3` that morning (`b11070`, the fixture) and was `cuda-13.4` by the
  evening, so `--llama cuda-13` kept working and `cuda-13.3` would not have;
  `--backend cuda` is refused with both candidates named. Not done: a CUDA
  *install* on Linux under a GPU (727 MB, two tarballs merged into one directory,
  `--list-devices`) — the pod probe's; and the Windows archive's headless smoke,
  the interactive TUI run and the installer, which stayed with the owner.
- **Published, and the numbers of it.** The owner published the release at
  21:44:52Z on 2026-09-21; it is the repository's latest. `crates-io.yml` started
  two seconds later and finished at 21:47:44Z; `mindfork 0.11.0` was on the
  registry at 21:47:30Z — 4 011 683 bytes (0.10.2 was 3 985 912), not yanked, the
  default version. The third execution of the upload branch, and nothing about it
  differed.
- **The address the README had been promising answered.**
  `releases/latest/download/install.sh` was a 404 from the merge of the install
  script until this publication, and a 200 after it. The README's line was then
  run as published, by an **unprivileged user** in a container — a path neither
  the scenarios nor CI had taken, since both run as root: the latest tag resolved
  from the redirect to `v0.11.0`, the archive downloaded, `sha256 ok`, unpacked,
  `not linked into /usr/local/bin (no permission). Run it as: /tmp/mf/mindfork`,
  `mindfork 0.11.0`, and the arguments after `--` handed over; the same line again
  said `already installed` and downloaded nothing.
- **The site followed by itself, for the first time with a real hold behind it** —
  3 min 37 s from the publication to the post being live, where 0.10.2's post had
  gone out 25 min 28 s *before* its release. The timings are in
  [website.md](website.md), "the site waits for the release".

### Post-M9: `install.sh` failed on the first real pod — root without CAP_CHOWN (done)

- **The first run of the README's line on a real RunPod pod, 2026-09-22, failed
  inside `tar`**: 51 lines of `Cannot change ownership to uid 1001, gid 1001:
  Operation not permitted`, then `Exiting with failure status`, and no binary.
  The download and the checksum had passed (`sha256 ok`); the archive was fine.
- **Cause, reproduced locally before the fix.** The release archive records the
  entries' owner as `runner/runner` — uid 1001, the CI runner that packed it
  (`tar -tzvf`). GNU tar run **as root** restores recorded owners by default
  (`--same-owner` is root's default; a non-root user gets `--no-same-owner`
  implicitly). A RunPod pod runs the command as root, but in a container
  **without `CAP_CHOWN`**, where every `chown` is `EPERM`. `docker run
  --cap-drop CHOWN` is the same shape: the released 0.11.0 script there prints
  49 refusals and leaves no `mindfork`; a plain `tar -x` fails the same way; and
  `tar --no-same-owner -x` succeeds with the files owned by root, which is what
  they should be. Every earlier check ran as root **with** `CAP_CHOWN` — the 25
  scenarios locally, the five CI images, the release job — or as an unprivileged
  user, for whom tar never chowns. Neither could see it; only a pod could.
- **The fix is one flag** — `tar --no-same-owner` in `unpack` — with the reason
  in a comment beside it. Nothing about the archive's owner was ever worth
  keeping: the files are the installer's to own. Verified on the **real 0.11.0
  archive** under `--cap-drop CHOWN`: `sha256 ok`, `unpacked`, the binary owned
  `0:0`, the expected exit 3 for the missing library.
- **The test now has the pod's shape, and proves it has it.** `install_test.sh`
  packs its fixtures with `--owner=1001 --group=1001 --numeric-owner`, so every
  scenario exercises a foreign owner whatever uid runs the test; and one new arm,
  under root with `capsh --drop=cap_chown`, has a **control**: a plain `tar -x`
  must be refused (exit 2) where the script must succeed — an environment that
  cannot show the difference cannot test the fix. Root without `capsh` is a
  **FAIL**, not a skip: this is the one arm that found a real defect on a real
  machine, so it does not get to be silently absent. A non-root run skips it and
  says why. `packaging.yml`'s five images gain `libcap2-bin`/`libcap`; the
  release job runs the scenarios under `sudo` so the arm runs there too.
  Measured: 28/28 on Ubuntu 24.04 and 22.04 as root with `capsh`; the FAIL arm
  fires without it; 22/22 with the skip as uid 1000.
- **Shellcheck caught four old lines while it was here** — `check … $?` after an
  `a && b` condition (SC2319), where `$?` is the condition's, not a command's.
  All four are explicit `if … then ok=0; else ok=1; fi` now. `shellcheck
  --severity=warning` is clean on both scripts.
- **What this says about the probe.** One line on one pod found what two days of
  containers could not, because the property that mattered — root without a
  capability — is one no container of ours had. The recipe in install.md §3.4 is
  unchanged: it was right, the script under it was not. **Until 0.11.1 ships,
  the README's line fails on RunPod**; the workaround is to install with a plain
  user's tar (`--no-same-owner` by hand) or to wait for the release.
- **Gates**: no Rust changed — **3408 unit tests, 197 `#[ignore]`**; 28 shell
  scenarios; link / source-language / index / action-pin gates green.
- **And the rest of the line ran on that pod.** With the archive unpacked by hand
  (`tar --no-same-owner`, `libasound2t64` installed), the owner reported that
  `./mindfork setup --sandbox --llama cuda-12 --model <Gemma 4 31B Q8_0>
  --ctx 131072 --verify` **went through without a problem** (2026-09-22). That is
  the first time any of this track has run on Linux under a GPU: the official
  llama.cpp CUDA build for Linux installed with its runtime archive (stage 0), the
  sandbox provisioned in a container, the settings written, and `--verify`
  bringing a 31B Q8_0 model up at a 131 072-token context on the card. The
  probe's remaining questions — the JIT on an 8.0/9.0 card, mmap off a network
  volume, `machine-id` across a stop — are still open, but the acceptance test of
  the track is passed everywhere except at the one flag this entry fixes.
- **And with 0.11.1 published, the line itself — nothing typed but the line.**
  The owner ran the README's command on the pod on 2026-09-22 and it went
  through end to end: `already installed` (the volume had survived), the
  sandbox provisioned (wasmer 7.2.0, Python 3.13.5, every package, the cache
  warmed, the image packed and started), llama.cpp `cuda-12.8` of `b11101`
  downloaded (161 MB) with its runtime (566 MB unpacked) — the family resolved
  to a build twenty newer than the one the docs were written against —
  `devices: CUDA0: NVIDIA RTX PRO 6000 Blackwell Server Edition (97251 MiB,
  96693 MiB free)`, three settings written, and `chat server: ready in 4 s —
  context 131072, text only, 4 slots`. That is the track's acceptance test,
  passed on the machine it was written for; install.md §3.4 now carries the
  numbers, and the research's probe is closed GO (§7).

### Release 0.11.1 (prepared)

- **A release PR** per the checklist in [AGENTS.md §6](../../AGENTS.md) (branch
  `chore/release-0.11.1`): bumped `Cargo.toml` `0.11.0 → 0.11.1` (+ `Cargo.lock`,
  one line), `site/zola.toml` `app_version`, `CHANGELOG.md` — `[Unreleased]` →
  `[0.11.1] — 2026-09-22` under a lead paragraph, a fresh empty `[Unreleased]`
  opened, comparison links updated. The `v0.11.1` tag is applied by the user
  after the merge. Gates green: 3408 unit tests (197 `#[ignore]`), `clippy -D
  warnings`, `fmt`, `release_guard.py --tag v0.11.1` accepting the tag.
- **A PATCH, by §6's rule, and a small one on purpose.** One change since
  `v0.11.0` and it is not in Rust: `install.sh`'s `--no-same-owner`, with the
  scenario that reproduces the pod's shape. No `Data` rubric — no schema
  constant and no config field moved (the diff of `src/` against `v0.11.0` is
  empty), so 0.11.0 and 0.11.1 open the same files.
- **Why a release rather than waiting**: `releases/latest/download/install.sh`
  is the README's first line, and until this ships that line fails on the
  machine it was written for. A published release is the only way the address
  serves the fixed script.
- **The release post is short** and says the one thing: the line failed, why,
  and that it works now — with the owner's own result that the rest of the line
  ran on the pod. `app_version` moves with it; `site.yml` holds the deploy as
  it did for 0.11.0.
- **The draft, checked before anyone else could see it** (§6 step 5). `v0.11.1`
  sat on the merge commit `f77e0069`, which was `origin/main`; `release.yml` was
  green and left a draft that was not a prerelease, with eight assets. Its notes
  were the CHANGELOG's `[0.11.1]` section byte for byte (16 lines). All seven
  lines of `sha256sums.txt` equalled GitHub's own digest per asset. The job's
  install step said `passed=27` — the CAP_CHOWN arm **ran on the runner** under
  `sudo`, its control firing (a plain `tar -x` refused, the script succeeding) —
  and `installed: mindfork 0.11.1 (expected: mindfork 0.11.1)`. `install.sh` one
  digest three ways (`d0c4a156…`), `tar --no-same-owner` in the asset, attested
  (one attestation this time: the script changed), the control arm exit 1. And
  the check 0.11.0 lacked: **the draft's own archive installed by the draft's
  own script under `--cap-drop CHOWN`** — `sha256 ok`, `unpacked`, **0 chown
  refusals** where 0.11.0's script had 49, the binary owned `0:0`, marker
  `v0.11.1`, the expected exit 3 for the missing library.
- **Published, and the numbers of it.** The owner published at 12:40:10Z on
  2026-09-22; `crates-io.yml` 12:40:12Z → 12:43:01Z, `mindfork 0.11.1` on the
  registry at 12:42:55Z (4 011 922 bytes, the default version); `Site` started
  by itself two seconds after `crates.io` completed and the post was live at
  12:43:35Z — **3 min 25 s after the publication** (0.11.0: 3 min 37 s). The
  hold on the merge had shown its notice as before. From outside: the post
  answers 200, the home page's structured data says `"softwareVersion":
  "0.11.1"`.
- **The README's line, as published, in the pod's shape.** Root without
  `CAP_CHOWN` in a bare `ubuntu:24.04` with only `curl` added — the container
  that reproduced the failure — ran `curl … | sh -s -- --dir /workspace/mindfork
  -- --version`: the latest tag resolved to `v0.11.1`, `sha256 ok`, `unpacked`,
  `libasound.so.2 is missing … — installing` / `installed`, linked, `mindfork
  0.11.1`, exit 0, **0 chown refusals**; the same line again said `already
  installed`. The one thing that failed on the first pod now passes in the
  same shape; what the owner ran past it by hand (the whole `setup … --verify`
  on a 31B at 131k) is recorded above.
