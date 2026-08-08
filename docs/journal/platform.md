# Journal — Platform: packaging, CI and dev tooling

Everything outside the running application: installers and Linux packages, releases, CI workflows and their cost, static analysis, the remote live-test gate, branding assets and the repository's own tooling.

**Reference documents for this area:** architecture.md §12, AGENTS.md §6

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (32)

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
- Post-M9: cutting GitHub Actions minutes (done)
- Post-M9: skipping the test job for docs-only pull requests (done)
- Post-M9: the remote live e2e gate on HF Inference Endpoints (stages 0–3, done)
- Post-M9: live-smoke diagnostic log in English (done)
- Post-M9: what the first real CI run of the remote gate found (done)
- Post-M9: broken documentation links, and a gate that stops them recurring (done)
- Post-M9: SonarQube Cloud analysis in CI (done)
- Post-M9: one Actions cache per job instead of one per branch (done)
- Post-M9: the SonarQube quality gate became blocking (done)
- Post-M9: cutting the Windows CI job from 19 minutes (done)
- Post-M9: tool tests run against in-memory storage (done)
- Post-M9: orchestrator fixtures — two convertible, the rest not (done)
- Post-M9: SonarQube backlog — triage + stage 1 (Python tools) (done)
- Post-M9: SonarQube backlog — stage 2 (UI screens and widgets) (done)
- Post-M9: SonarQube backlog — stage 3 (runtime and shared) (done)
- Post-M9: SonarQube backlog — stage 4 (orchestrator and tools) (done)
- Post-M9: the quality gate stopped judging new-code coverage (done)
- Post-M9: the Windows installer can provision the Python sandbox (done)

- Post-M9: the documentation refactor — CLAUDE.md became a router (done)
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
  bring the `artwork/` directory to full-fledged branding — portable assets,
  a brand guide, a packaging icon, a TUI logo, a wordmark in the docs.
  Forks R1–R7 confirmed by the user on 2026-07-18: **R2** — the logo in the
  help overlay's header (`F1`), **R3** — leave the interface palette
  **untouched** (`accent` carries the meaning "activity"; the logo is drawn
  in the brand colors), **R4** — `.desktop` with `Terminal=true`; R5–R7 per
  the recommendation (no CLI banner, `artwork/` is the single source,
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
  (needed only for regeneration). Added `artwork/build-wordmarks.py` (a
  generator; the palette, tracking, and lockup proportions are constants)
  and `artwork/README.md` (a brand guide: palette, glyph geometry, metrics,
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
  `.iss` (`artwork/mindfork-wizard-small.png`, 138×140 — the size for the
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
  `artwork/mindfork-icon-transparent.svg` and cross-checks them against the
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
  `artwork/mindfork-wordmark*.svg` isn't possible: the text there **has been
  converted to outlines** (stage 1, §2 of branding.md) — there's nothing to
  rasterize them with in the terminal. So `logo.rs` gained a `#`/`.` matrix
  per letter (`WORDMARK`, 8 glyphs for the word), drawn with the same
  half-blocks as the icon. The lettering mirrors the brand (JetBrains Mono
  ExtraBold): lowercase, descenders on `d`/`f`/`k`, **2-pixel strokes** — the
  same weight as the icon's bars (its grid also uses 2-unit bars). The
  result — 52 columns × 4 rows; the lockup as a whole — **65×6**.
- **Proportions taken from the brand metrics, not eyeballed**
  (`artwork/README.md`): word height = `0.5227 × S`, where `S` is the
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

### Post-M9: cutting GitHub Actions minutes (done)
- **Trigger**: the `v0.9.4` release run was refused by GitHub with *"The job was
  not started because recent account payments have failed or your spending limit
  needs to be increased"* — the 2000 included minutes of the Free plan ran out.
  Diagnosis note: the run showed up as `cancelled`, but the real cause was one
  matrix job failing at **scheduling** (no steps, 1 s) with `fail-fast: true`
  cancelling the rest; the reason is only visible in the job's **annotation**,
  not in the run/job status. `gh run rerun` is futile while the block is in
  place (attempts 2–4 all failed identically).
- **Where the minutes went** (measured, not estimated — GitHub rounds each job
  up to a whole minute and bills `windows-latest` at **2x**): CI on a PR =
  ubuntu 105 s + windows 467 s + lints 69 s → **20 billable min**; CI on the
  push to `main` = the same thing again → **20 min**; Packaging on `main` →
  **9 min**. The Windows test job alone is 16 of the 20.
- **The redundancy**: GitHub runs `pull_request` against the **merge result**
  (`refs/pull/N/merge`), so the push-to-`main` run re-tests code that has
  already passed on that exact tree — visible as a pair in the run list on every
  single merge.
- **Fix** (two workflow edits, no Rust): (1) `ci.yml` — the `test` matrix is now
  an expression, both OSes on `pull_request` and **Linux only** on `push`
  (`github.event_name == 'push' && fromJSON(...) || fromJSON(...)`); (2)
  `packaging.yml` — the `push: [main]` trigger removed entirely (`workflow_dispatch`
  kept). Per merge: CI 40 → 24 min, Packaging 18 → 9 min, ≈ **43 % less**.
- **Why `main` keeps a Linux CI run** rather than dropping the trigger outright:
  the README CI badge tracks the default branch and would go stale without one,
  and a Linux run still catches a semantic conflict between two separately-green
  PRs. It costs 4 min against the 16 saved by dropping Windows there.
- **Not done** (offered as follow-up): skipping the `test` job for docs-only
  changes. It is the biggest remaining win for this repo (the CLAUDE.md journal
  makes many PRs almost entirely `.md`, and PR #212 burned 20 min on one), but
  job-level path filtering needs either a third-party action or a hand-rolled
  `git diff` gate, and a wrong verdict silently skips the test gate. The `lint`
  job must keep running regardless — `cyrillic_scan` guards the docs themselves.
- Branch protection could not be consulted (403: unavailable for private repos on
  Free), so there are no required status checks to strand.

### Post-M9: skipping the test job for docs-only pull requests (done)
- The follow-up left open above (stacked on `ci/reduce-actions-minutes`). This
  repo's PRs are frequently near-pure documentation — the CLAUDE.md journal alone
  makes that the norm — and each one paid the full **20 billable minutes**
  (PR #212, a docs/packaging change, is the example). A new `changes` job
  classifies the PR's files and the `test` job gains
  `if: needs.changes.outputs.docs_only != 'true'`. `lint` is deliberately
  **untouched and always runs**: `cyrillic_scan` is a guard on the docs
  themselves, so skipping it exactly when only docs change would remove the gate
  where it matters most.
- **The allowlist is the whole design, and it is narrow**: `docs/**` plus
  top-level `*.md`. It is an allowlist rather than a denylist, so anything new or
  unrecognized falls through to running the tests. Files that *look* like docs
  but are read by the build or a gate test — established by grepping
  `include_str!`/`include_bytes!`/`CARGO_MANIFEST_DIR`, not by assumption — and
  therefore must never be in it: **`LICENSE`** (`include_str!` in
  `shared/credits.rs`, asserted by a test), **`artwork/*.svg`** (parsed by the
  `widgets/logo.rs` "code ≡ asset" gate), **`locales/*.json`** (`include_str!` +
  the i18n parity gates), **`Cargo.toml`/`Cargo.lock`** (the credits gates), plus
  `tools/`, `packaging/`, `dictionaries/`, `tests/fixtures/` and `.github/`
  itself. Note `.github/pull_request_template.md` is a `.md` that correctly does
  **not** match (the pattern anchors a top-level name, no slash).
- **Fail-safe by construction**: `docs_only=true` requires a `pull_request`
  event **and** a successful API call **and** the returned file count matching
  the PR's `changed_files` **and** every path inside the allowlist. Any other
  outcome runs the tests. The count check exists because the files endpoint
  truncates on very large PRs, and a partial page could otherwise hide a source
  file and look docs-only. The gate is `!= 'true'`, not `== 'false'`, so an
  empty output still runs the tests.
- **Corrected right after merging**: that value guard was *not* sufficient on its
  own. A failed `needs` dependency skips the dependent job **regardless of
  `if`**, so an infrastructure failure in `changes` silently skipped the test
  gate — observed for real in run 30217314709, where the billing block stopped
  `changes` from starting and `Tests` came out "skipped". Fixed by adding
  `!cancelled()` to the condition (`always()` would be wrong — cancelling the
  workflow must still cancel the job). Lesson: in a `needs` + `if` gate, the
  value and the upstream job's *status* are two separate failure modes.
- **Verified by executing the script**, not by reading it: the `run:` block was
  extracted from the YAML and run against a stubbed `gh` for six scenarios —
  docs-only → skip; code present → run; **truncated listing** → run; empty list
  → run; API failure → run; non-PR event → run. The path pattern was separately
  checked against PR #212's real file list and against each trap above.
- **Cost**: the `changes` job bills 1 minute per PR run (GitHub's per-job
  minimum) and saves 20 on a docs-only PR. Kept as its own job rather than an
  output of `lint` on purpose — reusing `lint` would serialize `test` behind it
  and couple a lint failure to the test gate.
- Applies to `pull_request` only. On `push` the trimmed Linux-only run from the
  previous commit is already just 4 minutes, and the `before`-SHA edge cases
  (branch creation, force push) would add real risk for very little gain.

### Post-M9: the remote live e2e gate on HF Inference Endpoints (stages 0–3, done)
- **The mandatory live gate stopped depending on one machine.** AGENTS.md §3
  requires a live run for anything touching engine / memory / tools, and until
  now that meant `run_all_tests.bat` → `http://192.168.1.20:8000/v1`: not
  reproducible by anyone else, not runnable in CI, and a llama.cpp regression
  catchable only by hand. Research
  [docs/research/remote-e2e-gpu.md](../../docs/research/remote-e2e-gpu.md) (forks
  **R1–R8 accepted by the user as recommended, 2026-07-28**), plan
  [docs/history/remote-e2e-hf.md](../../docs/history/remote-e2e-hf.md). Branches
  `spike/hf-endpoint-probe` (stages 0–1) and `feat/e2e-hf-runner` (stage 2).
- **Why HF Inference Endpoints and not a rented pod** (R1a): the survey's real
  question was not price but *"can we guarantee the GPU is released when the run
  crashes"*. No rented-pod provider gives one — you build it, and every external
  watchdog is another machine that can also fail. A managed endpoint moves the
  guarantee into the platform: idle auto-scale-to-zero **is** the dead-man's
  switch, so the worst case is one wasted idle window rather than a GPU running
  until someone notices. RunPod is ~4× cheaper per run; at ten runs a month that
  gap buys away an entire class of problem for ~$7. HF also runs **its own
  llama.cpp engine** — a real `llama-server`, so the llama.cpp-specific paths
  (request-body extensions, `--jinja` tool calling, `/health` + `503 Loading
  model`, embedding batch behaviour) are genuinely exercised, which no
  vLLM-backed serverless option would do.
- **Stage 0 — the probe (`tools/hf_probe.py`), verdict GO.** Seven unknowns, all
  settled against real throwaway endpoints for ≈ $0.17, mostly by reading the
  API's own 422 bodies (the script uses raw REST rather than `huggingface_hub`
  precisely so a rejected payload *teaches* the schema). Two findings improved
  the design: **`LlamacppMode` has an `embeddings` value**, so bge-m3 is served
  by the llama.cpp engine itself — the *same GGUF and quantization the
  similarity gates were calibrated on* — instead of TEI as §3.1 of the research
  had reasoned; and **the container `url` is ours to supply**, so the build can
  be pinned to a tag instead of tracking `master`, retiring the reproducibility
  caveat. Also: `ctxSize` is an explicit field (asked 16384, got `n_ctx=16384`),
  not the indirect Max Tokens × Max Concurrent Requests story in the docs, and
  deploy took **21 s** for a 17.65 GB model, because HF serves the weights from
  its own storage.
- **The trap worth remembering:** `EndpointType` is `public | authenticated |
  private`, and the API **silently coerces** an unknown value instead of
  rejecting it. The older wording `protected` produced a `private`
  (PrivateLink-only) endpoint no CI runner could reach — with a 200 and a
  healthy-looking response. The client now refuses to continue when the echoed
  type differs from the requested one.
- **Stage 1 — the enabling change** (~10 lines, useful on its own):
  `shared/api::live_client(url_var, key_var)` replaced six hand-rolled
  `OpenAiClient::new(env)` sites, and `probe()` now sends the key too. An unset
  or empty key sends no header — byte-for-byte the previous behaviour against a
  local `llama-server` — so this only *adds* the ability to point the same
  smokes at any authenticated OpenAI-compatible server. The `probe()` half is
  load-bearing rather than cosmetic: without it an authenticated `/health`
  answers 401, and 401 is not 503, so the probe reported "ready" whatever the
  key was and the two supervisor smokes would have passed for the wrong reason.
- **Stage 2 — the runner.** `tools/e2e_hf.py`: create both endpoints → wait for
  `running` → **wait for `/health`** → run the suite → delete and verify. Three
  departures from the plan's sketch, each earning its keep:
  - **`tools/hf_api.py` — the client was extracted and shared** with the probe
    rather than copied. Two copies of the create payload would drift the moment
    the schema moved, and two copies of the cleanup would mean two places where
    a bug leaks a billing GPU.
  - **The tests are compiled before the GPU exists** (`cargo test --no-run`), so
    a cold runner does not spend minutes of billed L40S time linking, and a
    build error costs nothing at all.
  - **Cleanup sends every DELETE before verifying any of them.** A cancelled CI
    job gives the handler ~7.5 s before SIGKILL; the calls that stop the meter
    must not queue behind a confirmation round-trip for the previous endpoint.
- **`running` is not loaded** — encoded as `wait_healthy()`, and it is the same
  distinction `OpenAiClient::probe()` exists to draw: HF's `running` means the
  container is up while `llama-server` still answers `503 Loading model`.
  Gating on the endpoint state alone fails the suite's first request.
- **The failure drill found a real leak — not the one it was designed to find.**
  The drill script itself crashed on a cp1252 encode error while echoing the
  runner's output; that broke the runner's stdout pipe, and **both endpoints
  leaked**. Root cause: `cleanup()` emptied the name list *before* its first
  `print`, that `print` raised `BrokenPipeError`, and the `atexit` re-entry then
  found nothing to do. Two fixes, both about not depending on being able to
  talk: prints inside cleanup go through a `say()` that swallows I/O errors, and
  **a name leaves the list only once its endpoint is proven gone**, so a
  cleanup that dies partway is retryable instead of amnesiac. Verified against a
  stubbed HTTP layer (deletes with a dead stdout; a crash mid-cleanup leaves the
  rest retryable; DELETEs all precede the verifications).
- **The sweeper fails safe towards keeping.** `e2e-*` endpoints older than 90
  minutes are deleted hourly, but an endpoint whose `createdAt` cannot be parsed
  is **kept and reported loudly**: deleting one could kill a run still using it,
  destroying real work for a false red, whereas a leak is already money-bounded
  by scale-to-zero. The 90-minute threshold must stay above the live job's
  45-minute timeout, or the backstop becomes a saboteur.
- **A checked assumption that was wrong.** The sweeper's decision logic was
  exercised against a fabricated listing before spending anything, and that
  caught a genuine bug: the fractional-second truncation in the timestamp parser
  also ate the digits of the timezone offset. The live API emits exactly
  `"2026-07-28T17:01:14.686Z"`, so this was on the main path, not a corner —
  every endpoint would have read as "age unknown" and never been swept.
- **Workflows:** `e2e-live.yml` (`workflow_dispatch` only — R5a: a live run is a
  considered act, ~$1 and ~25 min, and the non-hermetic smokes would flake
  unattended) and `e2e-sweeper.yml` (hourly `cron`, active only once on the
  default branch). Inputs reach the shell through the environment, never
  interpolated into a `run:` script. The job warms the npm cache before the GPU
  exists, since a cold `npx` can outlast the MCP smoke's 120 s readiness
  timeout.
- **No Rust changed in stage 2** — **1486 unit tests green** (the +2 over the
  previous entry are stage 1's), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. **No CHANGELOG entry**: dev infrastructure with no
  user-visible effect (AGENTS.md §4).
- **Smoke — GO** (2026-07-28, `gemma-4-31B_q4_0-it.gguf` on **nvidia-l40s** x1 +
  `bge-m3-q8_0.gguf` on **nvidia-t4** x1, aws us-east-1, llama.cpp
  `server-cuda`, `authenticated`, ctx 16384): **69 passed, 0 failed** —
  `cargo test -- --ignored --nocapture --test-threads=1`, both endpoints ready
  in **41 s**, suite **929 s**, total **970 s**, ≈ **$0.62**. Of the 69, **41
  actually exercised the endpoints**; 28 skipped for want of local assets or
  other credentials (11 Python sandbox, 12 cloud keys, 4 `MINDFORK_EMBED_URL_ALT`
  — stage 3, 1 managed `MINDFORK_LLAMA_BIN`). Both endpoints deleted and
  verified gone; `list` empty. This is the first fully green remote run — stage
  0's was 67/2, and both failures were fixed on that branch beforehand.
- **Failure drill — GO**: the runner was signalled **58 s into a live suite**;
  the handler deleted and verified both endpoints, exited 130, and a *separate*
  process confirmed none remained. The only difference from a cancelled CI job
  is which signal arrives (SIGBREAK on Windows, SIGINT there) — the handler and
  cleanup path are the same, and SIGBREAK is registered precisely so the drill
  can be run on a dev box.
- **Stage 3 — the second embedder (done).** Four smokes (`embed_guard` ×2,
  `reembed`, `embed_prefix`) guard the embedding-model-change track and need a
  *second, different* model via `MINDFORK_EMBED_URL_ALT`; without it they skipped
  **while reporting ok**, which is the exact failure a gate exists to prevent. A
  third llama.cpp endpoint on a T4 now serves
  `Ralriki/multilingual-e5-large-instruct-GGUF` / `…-q8_0.gguf` — the same
  quantization as the local stand, since the calibration constants were measured
  against that file, and deliberately a model that is **also 1024-d**, because
  the whole point of `same_dimension_model_swap_detected_live` is that no
  dimensionality check can tell the two apart. **No Rust change was needed**:
  stage 1 had already routed those smokes through `live_client`, key variable
  (`MINDFORK_EMBED_KEY_ALT`) included. **On by default** (`--no-alt-embed` opts
  out) against the plan's "optional" — ~$0.13 of a ~$1 run is the wrong thing to
  optimise when the alternative is four memory-critical smokes silently not
  running.
- **Smoke — GO (stage 3)**, and the interesting part is *how* green: the rented
  endpoints **reproduce the LAN stand's measurements to 3–5 decimals**, which is
  the evidence that matters when swapping the infrastructure under
  calibration-sensitive tests. bge-m3 calibrated to **0.41317 / 0.81843**
  against the reference constants 0.4128 / 0.8176; e5 to **0.78967 / 0.94557**
  against the journal's earlier live figure of 0.78968 / 0.94561 — identical to
  four decimals — mapping 0.72 → 0.9080 and 0.85 → 0.9580 exactly as designed;
  the prefix margins came out `e5 0.1223 → 0.1791` and `bge 0.4417 → 0.3330`
  against 0.1213 → 0.1789 and 0.4406 → 0.3317 measured locally. **5 passed, 0
  failed** (the 4 model-change smokes + the embedding readiness probe), ready in
  108 s, suite 78 s, total 187 s, ≈ $0.15; all three endpoints deleted and
  verified.
- **Still deliberately not done**: a *scheduled* live run; the managed-server
  smoke, which needs a child process of our own and so cannot run remotely at
  all; and the cloud-provider smokes, which need their own keys.
- **Merged in on the way**: `refactor/live-test-log-english` (the live-smoke log
  translation and the print-macro lint that closes the gap by position rather
  than by discipline). Not scope creep — the branches collide by construction:
  both edit `orchestrator/tests/live.rs`, and the stricter `cyrillic_scan.py`
  flagged 47 lines here without the translation, so CI's lint would have gone
  red whichever side landed first.

### Post-M9: live-smoke diagnostic log in English (done)
- **The developer-facing log of the live smokes was half-Russian** — 31 lines of
  `eprintln!` labels across `orchestrator/tests/live.rs` (29) and `tests/mcp.rs` (2)
  ("session 1: tools=…", "self-notes (observations) in DB: …", "gate showed a similar
  observation: …" were all Russian). Now English, matching the convention flipped by
  the english-source migration — and the precedent set there for provisioning progress:
  "it is a developer-facing test log". Branch `refactor/live-test-log-english`.
- **Why it survived the migration**: `tools/cyrillic_scan.py` allowlists test files
  **wholesale** (`path.endswith("tests.rs") or "/tests/" in path`) — a deliberate
  allowance, since these files legitimately hold Cyrillic fixture data and `ru`-locale
  assertions, and the scanner cannot tell a label from a fixture. So the gate was never
  going to catch it; it only became visible once the live suite started running in CI.
- **The scope line is what matters here** — only label text inside print macros moved.
  Untouched: the prompts sent to the model, the `ru`-locale substrings the gate
  assertions match (the `r.contains(…)` checks in `self_model_gate_e2e_live`,
  `summary_gate_e2e_live`, `trait_gate_e2e_live`,
  `self_consolidation_overview_e2e_live` and `recall_includes_self_e2e_live`),
  note/document fixture bodies, the A2 calibration probe pairs, and the TTS
  config/prompt strings. Translating any of those would have quietly stopped the tests
  testing what they test.
- **The one line that named a Russian value in its label** — how many notes cite the
  RAG source — was **not** solved with an opt-out marker but by binding that (Russian)
  source name once to a `const SOURCE` and interpolating it (`notes citing
  {SOURCE:?}`). That deduplicates a literal that had been repeated three times in code
  positions (two DB lookups + the log, where a typo in one would have failed
  confusingly), keeps the log naming the actual source, and leaves the **label** pure
  English. Values are data; labels are prose — interpolation is what separates them,
  and it is now the documented escape hatch.
- **A mislabel fixed in the same log** (the A2 calibration smoke, spotted while
  translating): both loops printed `MATCH?`, but the second is the *non*-match loop —
  so the log was already wrong in English. Now `MATCH?`/`NON-MATCH?`, padded to a
  common width, since the point of that smoke is eyeballing the two groups' cosines to
  place a threshold between them, and misreading which group a row belongs to is
  exactly the error it invites.
- **The gap is now closed by the gate, not by discipline** (`tools/cyrillic_scan.py`):
  test files stay allowlisted wholesale — the scanner cannot tell a fixture from a
  label **by file** — but it now can **by position**. A new `print_fmt_spans` tracks
  the format string of `print!`/`println!`/`eprint!`/`eprintln!` across lines (the
  string routinely opens on the `eprintln!(` line and carries its text on
  `\`-continuation lines), and Cyrillic inside that span translates even in a test.
  Deliberately narrow: **only the first literal**, so a value argument like
  `eprintln!("gate: {}", r.contains("<ru string>"))` is untouched; and
  **`assert!`/`panic!` are excluded**, because their *condition* sits in the same macro
  call as the message and flagging them would hit precisely the ru-locale assertion
  data that must stay. `write!`/`writeln!` too — in tests they usually build an
  expected-value buffer.
- **Mutation-tested in both directions** (a throwaway probe file, since the rule is
  cross-line state that a single-line reading can't confirm): it flags a single-line
  Russian label, a label on a **continuation line**, and one whose format string opens
  on the line *after* the macro; it does not flag ru-locale assertion data, fixture
  prompts, a Cyrillic literal in a value argument, an interpolated value, or a line
  carrying the `cyrillic-ok` opt-out. Repo-wide the rule has a **clean baseline** — a
  survey before writing it found exactly one candidate line, the one now interpolated.
- **No live run needed** (AGENTS.md §3): these are log strings and a lint — no behavior
  change, no engine/memory/tool path touched, and the changed lines only execute under
  `--ignored`. `--all-targets` compiles the ignored tests, so the `const`/interpolation
  change is still compile-verified. **1484 unit tests green** (count unchanged —
  string-only), 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan` clean. No
  CHANGELOG entry (§4: purely internal tests/tooling). Convention recorded in
  **AGENTS.md §3**.

### Post-M9: what the first real CI run of the remote gate found (done)
- The gate's first `workflow_dispatch` on GitHub (run 30396557877) is the run
  that mattered: the pipeline worked end to end — build → three endpoints →
  health gating → suite → cleanup, ~24 min inside a 45-min timeout, **62
  passed** — and everything it got wrong was invisible from a developer machine.
  Branch `fix/e2e-gate-ci-findings`.
- **A false `CLEANUP FAILED`, and it is the serious one.** The DELETE returned
  200 and the verification GET **~400 ms** later still saw the endpoint: HF
  acknowledges the delete and the endpoint disappears a moment afterwards. Only
  the *last* endpoint hits it — cleanup sends every DELETE before verifying any
  (the ~7.5 s SIGKILL window), so the earlier ones get incidental delay for free.
  Two consequences, the second worse than the first: the loudest signal in the
  system cried wolf over an endpoint it had genuinely deleted, and **exit 3
  masked the four real test failures**. The fix rests on a distinction worth
  keeping: **the DELETE is the action, the GET is only the proof**, so only the
  proof may wait — `verify_gone` now polls for ~10 s, which is safe on every path
  precisely because the meter is already stopped. A real leak still exits 3
  (pinned by a test).
- **The alternate embedder scaled to zero mid-suite** → the smoke that woke it
  got a `503`. Systematic, not a flake: `embed_guard` uses it early and
  `embed_prefix` some twenty minutes later, past the 15-minute idle window. The
  local stage-3 run could not have caught this — `--filter embed` packed all four
  smokes into 78 s. The obvious fix (widen the window) was the wrong one:
  **that window *is* the leak ceiling**, the thing that bounds a crashed run and
  the reason this platform beat a rented pod. One knob was doing two jobs. A
  keep-alive daemon thread now issues a real inference request every 5 minutes
  while the suite runs, keeping `lastUsedAt` fresh and leaving the guarantee
  intact (user's decision, 2026-07-29); pings are best-effort and can never fail
  the run.
- **Two smokes cannot run on a GitHub runner at all**, and were failing for
  environment facts rather than regressions: `plays_generated_tone_live` needs a
  sound card (ALSA finds none on a headless runner), and
  `live_search_returns_results` needs an IP that search engines do not throttle —
  a datacenter one is throttled far harder than a home one. Both now **skip** on
  the condition, as the sandbox and cloud-key smokes already do; a gate that is
  permanently red for an environment fact stops being read. The web one matches
  the **bundle key** rather than the prose, so it survives the locale, and leans
  on a distinction the tool already drew between "throttled" and "broken".
- **Left alone**: `simple_generation` returned an empty response once. The same
  model and stack passed it forty minutes earlier locally, so it is a flake until
  it recurs — worth naming rather than silently hardening around.
- **Smoke — GO, in the environment that produced the failures** (run
  30399806549, a second dispatch on the fix branch — three of the four are
  invisible anywhere else): **66 passed, 0 failed**, suite 1120 s, endpoints
  ready in 257 s. Each fix left its own evidence: `keepalive: 3 round(s) of
  pings` with the alternate embedder never leaving `running`, so
  `conventions_behave_as_measured_live` passed; `skip: no audio device
  available`; `skip: every search provider is throttling this IP`; all three
  deletes verified `gone` and exit 0. `simple_generation` passed, which is what
  makes calling it a flake honest rather than convenient.
- Gates green: **1486 unit tests**, 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. No CHANGELOG entry — dev infrastructure and tests
  (AGENTS.md §4).

### Post-M9: broken documentation links, and a gate that stops them recurring (done)
- **28 relative links in the docs pointed at nothing**, and had for a while.
  Nobody noticed because a stale link is *valid Markdown*: `cargo`, `clippy` and
  `cyrillic_scan` all pass, and it only 404s when someone clicks it on GitHub.
  Branch `fix/markdown-links`.
- **The cause is procedural, not careless.** AGENTS.md §4 says a finished track's
  plan moves to `docs/history/` — the file gains a directory level, and every
  relative link *inside* it silently breaks (`../src/foo.rs` starts resolving to
  `docs/src/foo.rs`). Links **to** the plan are the obvious half and do get
  updated; links **inside** it are the half that gets missed.
  `refactoring-solid.md` alone carried 23, because it cites source files
  heavily. One more was a different cause: CLAUDE.md still pointed at
  `src/shared/api/client.rs`, which moved to `openai/client.rs` in the ADR 0004
  Phase 2 split.
- **`tools/link_check.py`** (stdlib, the `cyrillic_scan.py` pattern: tracked
  files, `--list`, non-zero exit) now runs in CI's `lint` job — before the
  toolchain setup, since it needs neither Rust nor ALSA. It is **deliberately
  narrow**, so a green run means something: relative links only (checking the
  network would be slow and flaky), the file part only (heading slugs are a
  rendering detail, and chasing them would fail on every heading edit), and
  **code is skipped**.
- **Skipping code is what removes the need for an exception list.** A first,
  naive scan reported 32 hits, three of which were `[t](u)`, `√[n](x)` and
  `[text](url)` — all inside backticks, i.e. *examples* of links rather than
  links. Blanking fenced blocks and inline spans drops them by construction; the
  real count was **28**, not the 29 I had estimated by eye. Worth the note: the
  tool corrected its author before it corrected the docs.
- **Mutation-tested in both directions**, because a link checker that passes is
  indistinguishable from one that does nothing: re-breaking a single link the way
  a history move does turns it red; a file containing *only* code-span links
  stays green; adding one real link to that same file turns it red again.
- **The gate is placed where the bug is.** The `lint` job runs on every PR
  including a docs-only one (which skips the `test` job), which is exactly when
  links break. AGENTS.md §1 and the §4 table now name the trap and the tool.
- **No live run needed** (AGENTS.md §3): documentation, one CI step and a
  standalone script — no engine, memory or tool path touched. **1486 unit tests
  green**, 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean. No CHANGELOG entry — internal docs and tooling (§4).

### Post-M9: SonarQube Cloud analysis in CI (done)

- **Asked for directly**: the project side on sonarcloud.io was already set up
  (`SONAR_TOKEN` present in the repository secrets), only CI was missing. Branch
  `ci/sonarcloud` (the `ci/` prefix has precedent —
  `ci/reduce-actions-minutes`). A simple task by AGENTS.md §1 — CI configuration,
  no cross-layer contract and no code — so no design doc; the three genuine
  forks were put to the user instead (**decided 2026-08-05**).
- **Rust is a first-party Sonar analyzer now**, not the old community plugin, and
  that decided the shape: it **runs `cargo clippy` itself** (hence the toolchain
  and ALSA in the job) and imports coverage as LCOV or Cobertura
  (`sonar.rust.lcov.reportPaths`). So the job is deliberately *not* wired to our
  own `cargo clippy --all-targets -- -D warnings` gate: the analyzer's own run
  carries the 85 Clippy rules mapped to first-party Sonar rules, and mixing the
  two report paths is documented to **duplicate** issues.
- **Forks, all as recommended.** (1) **Coverage on** — `cargo llvm-cov` produces
  `lcov.info` in the same job; it is the metric every quality-gate condition is
  built around, and without it "Coverage on New Code" is simply `n/a`. (2) The
  job is **advisory** — it does not wait for the verdict
  (`sonar.qualitygate.wait` is left off), the same posture as `audit.yml`: a new
  analyzer on a 1833-test codebase will surface a batch of findings at once, and
  a gate that reddens every PR on day one stops being read. (3) Project key
  `vshylov_mindfork-rs` / organization `vshylov` — the standard GitHub-import
  scheme.
- **The minutes policy is respected rather than worked around**: the job takes
  `needs: changes`, so the docs-only PRs this repo produces constantly skip it
  (there is no new code to analyze), and it carries the same two guards as
  `test` — `!= 'true'` for the *value* and `!cancelled()` for the *job*, since a
  failed `needs` skips a dependent job whatever `if` says. It runs on `push:
  main` as well, deliberately: main is the baseline the "new code" comparison is
  made against.
- **Two builds in one job, and that is not an oversight**: the instrumented
  coverage build and the analyzer's plain clippy build have different
  fingerprints, so no arrangement makes them share one. Feeding the analyzer an
  external clippy report instead would still cost a second build (different
  RUSTFLAGS again) *and* add the duplicate-issue risk above — so the simpler
  option wins. `cargo llvm-cov` keeps its own `target/llvm-cov-target`, so it
  cannot poison the normal target dir (the journal already records what a
  poisoned `target/` looks like: seven tests "failing" on paths baked in from
  another tree).
- **`llvm-tools-preview` is added in the job, not in `rust-toolchain.toml`** — a
  developer checkout should not download the LLVM tools for a component only CI
  uses.
- **Two things the user has to keep true on the Sonar side** (recorded because
  neither is visible from this repo): **Automatic Analysis must stay off** for
  the project — CI-based analysis refuses to run while it is enabled — and the
  repository is **private**, which on SonarQube Cloud is a paid plan rather than
  the free public tier.
- **The coverage step was run locally rather than left to CI to discover** — it
  is the step most likely to fail, and a failure there costs runner minutes to
  learn: `cargo llvm-cov --workspace --lcov` ran the suite under instrumentation
  (**1833 passed, 76 ignored**, 41 s) and produced a 2 MB `lcov.info` over 166
  files, **87.0% region / 86.3% line** coverage. One detail confirmed rather than
  assumed: the report's `SF:` paths are **absolute**, which resolves correctly
  because they sit inside the scanner's base directory — the scan runs in the
  same workspace. Instrumented artifacts go to `target/llvm-cov-target`, so the
  ordinary `target/` is untouched.
- **Verification of the rest**: the workflow YAML parses and the `sonar` job's
  steps and guards are as intended; `cyrillic_scan`/`link_check` clean; **1833
  unit tests** unchanged (no Rust code touched). **No live model run is
  required** (AGENTS.md §3) — no engine, memory, tool or provider path exists in
  this change. What only the first CI run can settle is the analyzer's own clippy
  invocation and the upload. No CHANGELOG entry — dev infrastructure with no
  user-visible effect (§4).
- **Groundwork**: turning the gate blocking once the first analysis is triaged;
  a quality-gate badge in the README (a private project's badge does not render
  for anonymous readers without a token); and folding the coverage run into the
  Linux `test` job if the first measurements say the duplicated test run is the
  expensive half.

**What the first live runs found** (2026-08-05, recorded because two of the
three findings are invisible from the workflow's own status):

- **The job was green while the analysis had failed.** On the PR everything
  passed — Quality Gate, 0 new issues — but the push-to-`main` analysis was
  **rejected server-side**, and `ci.yml` reported success anyway. Not a bug:
  `ANALYSIS SUCCESSFUL` in the scanner log means *the report was uploaded*, and
  processing is asynchronous (the next log line says so). That is the exact blind
  spot of the advisory posture. What surfaces it is the Sonar app's **own** check
  on the commit, `SonarCloud Code Analysis` — it read "❌ The last analysis has
  failed" while ours read green. So the pair is: our job answers "did the scan
  run", that check answers "was the report accepted". Enabling
  `sonar.qualitygate.wait` would collapse the two — the scanner then waits for the
  CE task and fails on it — which is a second argument for the groundwork item
  above, beyond gate enforcement.
- **The cause was the organization's LOC quota, and two wrong hypotheses were
  discarded on evidence before it.** `Administration → Background Tasks → Show
  error details` gave it verbatim: the free plan allows **50 000 lines per
  organization**, 14 143 were already used, and this analysis brought **81 411**
  (`rust=80 174, py=1 237`). The first guess — that the project's Main Branch was
  named something other than `main` — was refuted by the Branches page (`main` is
  the main branch, simply never analyzed). The second — that exclusions could fit
  the project into the remaining 35 857 — was refuted by **measuring**: `src` is
  80 059 ncloc, of which `**/tests.rs` is 6 692 and inline `#[cfg(test)]` modules
  are **24 729**, and the latter cannot be excluded at all because `sonar.exclusions`
  works per *file*, not per region. Even production-only (48 638) does not fit.
  So it was a plan decision, not a configuration one: **Team, $34/month for up to
  100k LOC** (user's decision) — 81 411 used, ~18 600 of headroom.
- **Exclusions deliberately not added** despite fitting the option as offered:
  trimming `**/tests.rs` + `tools/` buys 7 900 lines while 24 729 lines of inline
  test modules keep counting, i.e. it makes the analysis *inconsistent* (some test
  code counted, some not) for less than half the headroom already available. The
  lever stays documented for whenever 100k is approached.
- **Re-running the analysis needed no commit**: `gh run rerun <id> --job <sonar>`
  re-ran only that job against the same commit, so lint and the tests were not
  paid for twice. Afterwards the check became **"Quality Gate not computed"**
  (`neutral`) — the normal state of a *first* main-branch analysis, since there is
  no new-code baseline to compare against yet.
- **The open question is closed: coverage really is imported.** `main` reports
  **85.2%**, against 86.3% line coverage measured locally — the gap is the ~1 240
  Python lines in `tools/`, which have no coverage report and therefore land in
  the denominator as uncovered. That distinguishes a working import from the
  silent failure mode it could not otherwise be told apart from ("0.0% Coverage on
  New Code" on a PR that touches no Rust looks identical either way).
- **Timing**: 7 min 51 s cold, **4 min 06 s** warm (analysis itself ~1 min; the
  analyzer's own Clippy run is the bulk of the rest).

### Post-M9: one Actions cache per job instead of one per branch (done)

- **Found while reading the logs of the SonarQube PR**, not by looking for it:
  the repository's Actions cache stood at **10.37 GB in 21 caches against a
  10 GB cap**, i.e. GitHub was already evicting by age. The listing showed the
  cause — five and six near-identical copies of the same key
  (`v0-rust-test-Windows_NT-x64-2afb1257-…`). A cache belongs to **the branch
  that wrote it**, so with `save-if` at its default every PR branch stores its
  own copy of the same build. Branch `ci/cache-save-on-main`.
- **Why it costs minutes rather than just space**: eviction is by age and
  repository-wide, so a stream of per-branch copies pushes out the caches that
  every run depends on, and the next `lint`/`test`/`sonar` builds from scratch.
  That is the same currency the trimmed matrix and the docs-only skip were
  bought with.
- **`save-if: ${{ github.ref == 'refs/heads/main' }}`** on the three `ci.yml`
  jobs: one copy per job, written by the push-to-`main` run, and a pull request
  still *restores* it — GitHub lets a branch read its base branch's caches. Per
  PR this drops what gets written from ~1.9 GB (lint 317 MB + Linux test 515 MB
  + sonar 634 MB + Windows test 445 MB) to just the Windows one.
- **Windows is the deliberate exception** (`|| runner.os == 'Windows'`), and
  getting it wrong would have been worse than the bug: the `main` matrix is
  Linux-only *by design* (the minutes work — `pull_request` already runs against
  the merge result, and Windows bills at 2x), so nothing would ever write a
  Windows cache and **every** PR would rebuild it from scratch on the expensive
  runner. Seeding it by adding Windows to the `main` matrix would cost ~16
  billable minutes per merge — precisely what was removed.
- **Scoped to `ci.yml`**: `packaging.yml`/`release.yml`/`e2e-live.yml` also use
  the action, but run on a tag, a dispatch, or a `packaging/**` PR, so they are
  not what churns — and two of them are rare *and* expensive, where a cold build
  hurts most.
- **Along the way**: `sonar.python.version=3.10, 3.11, 3.12` in
  `sonar-project.properties` — the only WARN the scan emits is the Python
  analyzer saying it assumes "all of Python 3" for `tools/*.py`. The list is what
  actually runs them (3.10 on the development machine, 3.12 on the ubuntu-24.04
  runner), not a guess. It rides this PR because that one already pays for a full
  test run and is the same CI plumbing; a docs-only PR would not have carried it,
  since `sonar-project.properties` is deliberately outside the docs allowlist.
- **Verification had to wait for the merge** — the property is "a PR branch stops
  writing Linux caches", which cannot be observed while the change is still on a
  branch. **Confirmed the same day** (2026-08-05), and earlier than expected,
  because a stacked PR already carried the rule: `gh cache list` by ref shows
  `refs/pull/259/merge` and `refs/pull/260/merge` holding **only** the Windows
  test cache, while `refs/pull/258/merge` — from before the change — still has a
  `sonar` copy, and the Linux trio (`lint` 317 MB, `sonar` 634 MB, `test-Linux`
  515 MB) now exists once, under `refs/heads/main`. Usage went **10.37 GB / 21
  caches → 9.65 GB / 19** and keeps falling as older per-branch copies age out.
  The YAML parses and all three `save-if` expressions render as intended; **1833
  unit tests** unchanged (no Rust code touched); no live run required (AGENTS.md
  §3) and no CHANGELOG entry — dev infrastructure with no user-visible effect (§4).
- **A property of the docs-only classifier, learned the same day**: it reads the
  **whole PR file list**, not the last push. Merging a CI branch into a docs PR
  therefore makes that PR non-docs-only, and it correctly runs the full matrix —
  which is what happened when this stack was merged child-first.

### Post-M9: the SonarQube quality gate became blocking (done)

- **Closes the groundwork item opened when the analysis was set up**: the job was
  advisory on purpose (a new analyzer on a 1833-test codebase can surface a batch
  of findings at once, and a gate that reddens every PR on day one stops being
  read), to be flipped once the first analyses were triaged. They came back clean
  — PR #258 `Quality Gate passed` with 0 new issues, `main` likewise once it had
  a baseline — so the reason to wait was gone. Branch `ci/sonar-blocking-gate`.
- **`sonar.qualitygate.wait=true`** in `sonar-project.properties` (not an
  argument in the workflow: analysis parameters live in one place). The scanner
  now polls the compute-engine task and fails the job on a failed gate.
- **The bigger half is not enforcement, it is honesty about the upload.** The
  first live run showed the failure mode this closes: `ANALYSIS SUCCESSFUL` in
  the scanner log means *the report was uploaded*, processing is asynchronous,
  and the report was then **rejected** server-side (over the organization's LOC
  quota) while `ci.yml` still reported success. Waiting makes the job answer "was
  this analysis accepted and did it pass" instead of "did the scan run" — the
  distinction that previously only the Sonar app's own commit check could make.
- **What can and cannot redden a PR**: the gate judges **new** code, so the
  existing backlog never blocks; what blocks is new uncovered code (Coverage on
  New Code, which the `cargo llvm-cov` step feeds), new issues, or an analysis the
  server refuses. Worth knowing the corollary: the project's **New Code
  definition** now has teeth, since it decides what "new" means.
- **Cost**: the job waits for processing, which measured ~2–5 s on this project
  (`sonar.qualitygate.timeout` defaults to 300 s and is deliberately left alone).
- **Verification**: the YAML and properties parse; **1833 unit tests** unchanged
  (no Rust code touched); no live run required (AGENTS.md §3) — no engine, memory,
  tool or provider path exists here. The change is self-verifying in a way the
  previous ones were not: if the gate did not actually block, this PR's own
  `sonar` job would not be reporting a verdict at all. No CHANGELOG entry — dev
  infrastructure with no user-visible effect (§4).

### Post-M9: cutting the Windows CI job from 19 minutes (done)

- **Asked directly**: CI was taking up to 20 minutes, most of it Windows tests —
  and whether to move to `cargo-nextest`. Branch `ci/windows-speedup`. A simple
  task by AGENTS.md §1 (CI configuration plus a test-only fixture change, no
  cross-layer contract), so no design doc; the one genuine fork — billable
  minutes versus wall clock — was put to the user instead (**decided
  2026-08-05**, warm the cache on `main`).
- **The Windows job was the entire CI wall clock.** Everything else finishes
  inside 4 minutes, so 19 minutes *was* the Windows job. Measured on run
  31016421114 rather than estimated: **7m31s cold compile + 9m09s test run +
  1m51s cache save**.
- **The cache was never warm, and the cause was precise.** The `rust-cache` step
  completed in **4 seconds** — a miss, not a restore. The `main` matrix was
  Linux-only, so a Windows cache was never written to a ref a pull request can
  read: GitHub scopes a cache to the branch that wrote it and only lets a branch
  read its **base**. So every PR paid a full cold build and then spent 1m51s
  saving 445 MB to its own `refs/pull/N/merge`, which nothing could ever restore
  and which dies with the PR — the cache list showed eight such copies. Checked
  the obvious escape hatch too: a *second* run on the same branch was also cold.
  Now `main` builds Windows without running the suite (it already passed on the
  PR; building is what fills the cache), **kept in the same job** because
  rust-cache embeds the job id in the key — visible in the key names themselves
  (`v0-rust-test-…`, `v0-rust-lint-…`, `v0-rust-sonar-…`). A separate warming job
  would have silently written a key the test job could never restore, i.e.
  warmed nothing while looking correct.
- **~800 fsyncs in two seeding fixtures.** `fragmented_db` and backup's
  `seed_fragmented_db` inserted 200 rows through `rag_insert`, which issues two
  statements in autocommit — and `delete_matching` loops the same way, so
  batching only the inserts would have fixed half of it. Now one transaction per
  phase, and deliberately **two** phases: the file has to grow and only *then*
  have pages freed, or no freelist is left behind and the tests stop testing
  anything. Assertions untouched and verified to still hold with margin — 48% of
  the file is free pages before the vacuum, and the vec0 index still joins by
  rowid after it. `Db::batch` is `#[cfg(test)]`, so it does not exist in a
  non-test build. Locally at the runner's four threads: the compaction tests
  **19.35s → 0.49s**, the full suite **68.67s → 45.36s**.
- **Measured result**, still with a cold cache since `main` had not yet saved
  one: job **19m00s → 15m00s / 16m22s / 15m29s** over three runs, test run
  **549.1s → 359.1s / 469.0s / 376.5s**, cache save **1m51s → 2s**. The third
  run is the one with the Defender step already removed, and it lands between
  the other two — confirming the removal cost nothing, as
  `RealTimeProtectionEnabled = False` predicts. The remaining ~6 minutes of cold
  compile go on the *next* pull request, once this merge leaves a warm Windows
  cache behind.
- **Confirmed after the merge** (2026-08-05): the warming run left
  `v0-rust-test-Windows_NT-x64-2afb1257-dc2e291a`, 445 MB, on
  `refs/heads/main` — the **same key** the pull-request job looks for, which was
  the whole point. It cost 9m31s of build plus 1m06s to save, i.e. **~19
  billable minutes** per merge rather than the ~15 estimated beforehand; the
  estimate in the workflow header was corrected to the measured figure.
- **The warm pull request, measured** (run 31033936114 — this very docs PR,
  which touches `.github/workflows/ci.yml` and is therefore deliberately outside
  the docs-only allowlist, so it ran the full matrix and served as the
  measurement without anyone having to manufacture a throwaway PR). The Windows
  job went **19m00s → 9m11s**, and the breakdown is where the shape of it shows:
  cache restore 4s (a miss) → **32s** (a real 445 MB restore), compile **7m31s →
  1m45s**, cache save 1m51s → 1s, test run 549.1s → 386.1s. The compile figure
  is the one that matters — it is exactly the predicted behaviour, dependencies
  coming from the cache and only our own crate rebuilding, which is also what
  the Linux `lint` job had been demonstrating on every pull request all along.
  **The compile half of the win is structural and reproducible; see the next
  entry for why the total is not.**
- **The second warm run corrected that headline, and it is worth keeping the
  correction rather than the claim.** Run 31034769688, the very next push to the
  same branch, came in at **18m26s** — the cache behaved exactly as designed
  (48s restore, compile **1m36s**, confirming the structural fix twice over),
  but the **test phase took 926.8s against 386.1s** on identical code. Two
  pieces of evidence in that same run rule out "a uniformly slow machine": the
  Linux job ran the suite in its usual **44.6s**, and the Windows *compile* in
  the very same job was fast. So it is the Windows test phase specifically.
  Collected figures for it, post-fixture-fix: **359.1 / 469.0 / 376.5 / 386.1 /
  926.8s** — four clustered around ~390s and one 2.4x outlier. The honest
  summary is therefore: compile reliably drops ~7m31s → ~1m40s, the cache save
  reliably drops 1m51s → ~1s, and the total lands anywhere between **~9 and ~18
  minutes** depending on how the test phase happens to run. The earlier "~9
  minutes" was one lucky sample stated as a result.
- **The lead was chased and settled: it is disk I/O, not a timeout-sensitive
  test.** A throwaway workflow on its own branch (`push`-triggered, so it needed
  no pull request and could not touch `ci.yml`) ran the suite once under nextest
  purely for its per-test timings — the reason nextest was borrowed rather than
  adopted, since stable libtest has no `--report-time`. Run 31043133962, a
  *normal* instance (438.2s wall against 52.3s local, the usual ~8x), joined
  against local per-test times for all 1833 tests:
  - **The timeout hypothesis is dead.** The genuinely timeout-bound tests are
    **1.0x** — `embed_external_url_is_available` 2.068s on CI against 2.065s
    locally, `wait_until_ready_bails_on_early_exit` 2.074s against 2.068s. A
    fixed 2s timeout is a wall-clock constant and does not stretch on a slow
    machine, so those tests cost the same everywhere and cannot produce a tail.
  - **The slowdown is broad but *not* uniform, and it tracks I/O.** Median
    per-test ratio **3.9x** (p10 2.2, p90 6.0), while the worst offenders run
    **8–19x**: `consolidation_overview_uses_the_calibrated_threshold` 18.75s
    against 1.34s, `note_cite_source_links_and_recall_shows_it` 14.52s against
    0.90s, and so on — every one of them a notes/rag/search test that goes
    through `ctx_with_storage`, which builds a **tempdir plus a file-backed
    SQLite** (`Storage::open(Paths::with_root(…))`). CPU-bound tests sit at the
    3.9x median; disk-bound ones are penalised several times over.
  - **Parallel efficiency corroborates it**: the per-test times sum to 876.1s
    against a 438.2s wall, i.e. only **2.0x** from four cores, where locally the
    same suite gets **4.4x** (231.3s over 52.3s). Losing more than half the
    parallelism is the signature of contention on one shared resource — four
    concurrent fsync-heavy tests serialising on the disk — not of a slow CPU.
  - **Shape**: a long flat tail rather than a few fat tests — the top 10 are
    12.6% of the total and it takes **100 tests to reach 47%**. So there is no
    single test to fix; the tail is the aggregate.
- **Groundwork that follows from it** — **done**, see the next entry:
  `ctx_with_storage` handed every tool test a file-backed SQLite that almost
  none of them need.
- **A hypothesis of mine that the measurement killed — twice over.** I
  attributed the runner being ~8x slower than a local Windows box at the same
  four-thread parallelism (37s local vs 549s) mostly to Defender scanning every
  file rustc writes, and wrote that into the workflow as the exclusion step's
  rationale. First the arithmetic undercut it: the test run improved 34.6% while
  the fixture fix alone had predicted 33.9% locally, leaving nothing for
  Defender to explain. Then the direct check settled it — the step was made to
  log `Get-MpComputerStatus`, and the runner answered
  **`RealTimeProtectionEnabled = False`**. Defender is already off on the image,
  so excluding paths from a scanner that is not running buys nothing; the step
  was removed and the comment now warns against re-adding it without checking
  that flag. The residual gap is the runner's CPU and disk.
- **Runner variance is large enough to matter when reading these numbers.**
  Three post-fix runs gave test runs of 359.1s, 469.0s and 376.5s — a 30%
  spread, with the middle one an outlier. So the honest attribution is that the
  controlled local measurement (68.67s → 45.36s at four threads) is the
  trustworthy one, and single-run CI comparisons here should not be read to two
  significant figures. Worth knowing before anyone tunes this workflow against
  one green run.
- **cargo-nextest — evaluated and rejected, measured rather than reasoned.**
  Slower here at both parallelism levels: **46.3s vs 37.4s** at full parallelism
  and **52.3s vs 45.4s** at the runner's four threads. The reason is structural:
  this is a single binary crate whose 1833 tests live in one executable, and
  nextest runs each test in its own process, which Windows charges for. It also
  does nothing about compilation, which was the larger half of the job. Its one
  attractive feature here, `--partition` sharding, multiplies the compile cost
  across shards — exactly what the cache warming just bought back. It did earn
  its keep once, though: its per-test timings are what found the five slow tests,
  since `--report-time` needs nightly on stable libtest.
- **A bug caught by validating the YAML rather than by eye**: the matrix was
  first written `os: '["ubuntu-latest","windows-latest"]'`, which GitHub expands
  only inside a `${{ }}` expression — as a plain string it would have produced a
  single matrix entry with that literal name.
- **1833 unit tests green** (count unchanged — a fixture optimization), 76
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No
  live run required** (AGENTS.md §3): CI configuration and a `#[cfg(test)]`
  fixture — no engine, memory, tool or provider path is touched. No CHANGELOG
  entry — dev infrastructure with no user-visible effect (§4).

### Post-M9: tool tests run against in-memory storage (done)

- **The follow-up the CI diagnostic pointed at**, asked for as its own PR
  (branch `refactor/tool-tests-in-memory-storage`). The nextest probe had shown
  that the Windows runner's penalty is **disk**, not CPU: a 3.9x median per-test
  ratio against local, but **8–19x** for every one of the worst offenders — all
  of them notes/rag/search tests reaching storage through the tools testkit,
  which built a tempdir plus a **file-backed** SQLite. Almost none of them need
  a file.
- **The change is three lines of substance.** `Db::open_in_memory` and
  `CacheDb::open_in_memory` already existed; the facade was the only gap, so
  `Storage::open_in_memory(paths)` joins them and keeps `JsonStore` on the real
  `paths` — chats/profiles/config behave exactly as before, only `data.db` and
  `cache.db` stop being files. The two testkit constructors
  (`ctx_with_storage_lang`, `ctx_with_backends`) and one test that shares a
  single `Arc<Storage>` between two profiles now call it.
- **No call site changed.** Both helpers keep returning the `TempDir` — the JSON
  half still needs a root, and callers keep it alive — so all ~126 uses across
  16 files are untouched. Checked rather than assumed that nothing depended on
  the files: only two tests bind the directory handle at all, and both merely
  pass it through; no tool test references `data.db`/`cache.db` or reopens
  storage.
- **Measured, locally at the runner's four threads**: full suite **40.68s →
  24.09s** (−41%), and at full parallelism **37.4s → 16.41s** (−56%). Per-test,
  the sum over `features::tools::*` goes **103.3s → 16.4s (−84%)**, and the
  individual tests CI measured worst collapse from ~1s to **0.03s** —
  `consolidation_overview_uses_the_calibrated_threshold` (18.75s on CI) 1.34s →
  0.03s, `note_cite_source_links_and_recall_shows_it` (14.52s on CI) 0.90s →
  0.03s. Those are exactly the tests the runner's disk was punishing 8–19x, so
  the CI effect should be larger than the local one.
- **Measured on CI, and it closed the diagnosis** (run 31045675223): the Windows
  job went **9m11s → 5m59s** and its test phase **386.1s → 189.9s (−51%)**,
  against the original cold-cache **19m00s / 549.1s**. Two details make this
  more than a speedup. The CI gain (−51%) **exceeded the local one** (−41%),
  which is what the disk hypothesis predicted, since the runner charges more for
  exactly the I/O that was removed. And in the same run **Linux barely moved** —
  38.7s against 41.0s before — because a Linux runner was never paying that
  penalty. A change that helps one OS 2x and the other not at all is the
  signature of the cause being the disk, not the code, so this run is both the
  fix and the confirmation of the earlier diagnosis.
- **Where CI now stands overall**: from ~19–20 minutes at the start of this work
  to **~6**, via three independent fixes — the warm Windows cache (compile 7m31s
  → ~1m45s), the fsync fix in the seeding fixtures, and this one. The 927s tail
  observed earlier should also shrink, since it was the disk-bound tests that
  were most exposed to a slow instance, but that is a claim about variance and
  needs more than one run to assert.
- **Two guards, because the win is invisible to every other test.** Switching
  the testkit back to `Storage::open` would hand all the fsyncs back and nothing
  would fail. So `tool_context_storage_touches_no_disk` (`features::tools`)
  writes a real note through the testkit's storage and asserts neither database
  file appears — **mutation-tested**: reverting the testkit fails it with the
  message naming the cause — and `in_memory_storage_works_but_writes_no_database_files`
  (`shared::storage`) pins the facade itself, both that the SQLite halves are
  real (schema applied, sqlite-vec registered, note and vector round-trip) and
  that no file is created. The first test's doc comment originally claimed the
  second one's coverage; corrected, since a test cannot see which constructor
  its caller picked.
- **Deliberately unchanged**: backup, migration and compaction tests keep
  `Storage::open` — they assert on the files themselves, and an in-memory
  database dies with its connection. The orchestrator fixtures also keep it:
  they genuinely exercise persistence (`flush_saves`, restart scenarios, the
  search-cache reconciliation that reads chat files), and two of the CI-worst
  tests are theirs, so that is a separate question rather than an oversight.
- **1835 unit tests green** (+2 guards), 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No live run required**
  (AGENTS.md §3): a test-only storage path — `Storage::open_in_memory` is
  `#[cfg(test)]` and does not exist in a non-test build, and no engine, memory,
  tool or provider behaviour changes. No CHANGELOG entry — internal tests
  (§4).

### Post-M9: orchestrator fixtures — two convertible, the rest not (done)

- **Asked as a follow-up to the tool-test change**: with `features::tools::*`
  moved off disk, `app::orchestrator::*` became the dominant remaining cost —
  **384.5s of the 876.1s** CI per-test sum in the nextest probe, against tools'
  419.1s. The question was whether the same trick applies.
- **Mostly it does not, and finding out *why* is the useful part.** The
  orchestrator fixtures split three ways:
  - `spawn_orch_at` — **two-phase** tests that restart the app on one data root.
    An in-memory database dies with its connection, so phase two would start
    from an empty `cache.db` and exercise the index *rebuild* path instead of
    the *restore* one: the test keeps passing while covering something else.
  - `spawn_orch_cfg`/`spawn_orch` — single-phase, so they *look* convertible.
    They are not: roughly thirty tests built on them reopen storage afterwards
    (`let reopened = Storage::open(Paths::with_root(&root))`) to assert what
    actually reached disk. That idiom is how this suite checks persistence at
    all.
  - `bare_orch_rx` and `orchestrator::rag::test_deps` — no test on either
    reopens storage, verified by scanning every test file rather than assumed.
    **Converted.**
- **The measurement that settled it, and the near-miss worth recording.**
  Converting `spawn_orch_cfg` failed **6 tests** outright — and, far more
  interesting, **2 more kept passing for the wrong reason**:
  `removing_an_attachment_drops_its_index` and `an_inline_file_is_not_indexed`
  assert an *absence*, and against an always-empty reopened store they pass
  whether or not the code works. A green suite would have hidden that, which is
  exactly why the conversion was reverted rather than patched test-by-test: the
  hazard is not the six that shout, it is the two that do not. The reasoning
  sits in `spawn_orch_cfg`'s doc comment so the next person does not rediscover
  it by breaking something.
- **Measured** (locally, four threads, per-test sums): `app::orchestrator::*`
  **78.9s → 54.0s (−32%)**, whole-suite sum **140.2s → 113.4s (−19%)** on top of
  the tool-test change. The two heaviest converted tests —
  `reflection::reflect_failures_alert_once_then_reset` (7.80s on CI) and
  `rag::index_source_reports_chunk_progress_in_subbatches` (9.07s on CI) — both
  drop to **0.01s**. Cumulatively with the previous entry the per-test sum goes
  231.3s → 113.4s (−51%).
- **Confirmed on CI, and it closes the whole CI track.** The Windows test phase
  measured **258.2s** on the tool-test PR's run and **129.2s** on this one — the
  two safe fixtures halved it again, because they were exactly where the
  runner's disk charged most. End to end the Windows job went **19m00s →
  4m56s**, and its test phase **549.1s → 129.2s** (4.3x). Post-merge `main` is
  green in under five minutes, with its Windows job at 2m42s since it only
  builds.
- **A cache detail worth knowing** (it looks like a bug and is not): the Windows
  cache on `main` keeps its original timestamp and is *not* re-saved after these
  merges. Its key is a fingerprint of the **dependencies**, which source-only
  changes do not touch, and GitHub cache keys are immutable — so rust-cache
  correctly skips saving and the existing entry keeps serving. It only rotates
  when `Cargo.lock` or the toolchain moves.
- **Where the CI work ended up overall**: ~19–20 minutes → ~5, from four
  independent fixes — the warm Windows cache (compile 7m31s → ~1m45s), the
  ~800-fsync seeding fixtures, in-memory storage for the tool tests, and the two
  safe orchestrator fixtures — plus the removed useless cache save. The runner's
  variance (359 / 469 / 376 / 386 / 927s on identical code, back when the base
  was 4x higher) has not gone away; the base is simply much lower now, and
  whether the tail shrank proportionally needs several more runs to claim.
- **1835 unit tests green**, 76 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3):
  test fixtures only; `Storage::open_in_memory` is `#[cfg(test)]`. No CHANGELOG
  entry — internal tests (§4).

### Post-M9: SonarQube backlog — triage + stage 1 (Python tools) (done)

- **The 114-issue initial-analysis backlog was triaged** (user decisions
  2026-08-06): the ~49 `rust:S2208` wildcard-import issues are the **documented
  `use super::*` convention** from the god-object split playbook
  (docs/history/refactoring-god-objects.md, architecture.md §3) — **Accepted in
  Sonar**, not "fixed"; the ~61 `S3776` cognitive-complexity issues are
  **triaged**: refactor the genuinely tangled functions, Accept the deliberate
  table/parser shapes. Staged as one bookkeeping pass + four fix PRs
  (Python tools → UI screens/widgets → runtime/shared → orchestrator/tools).
- **Stage 0 — Sonar bookkeeping, no code** (57 status changes via the MCP):
  49 × S2208 Accepted; `python:S5332` (link_check.py:49) **False Positive** —
  the flagged `http://` is the `SKIP_SCHEME` tuple used to *skip* external
  links, not a request; 7 deliberate-shape S3776 Accepted with rationale —
  `settings/spec.rs::field_spec` (147 — the single-source-of-truth access
  table, SOLID stage 3.2; splitting it recreates the four scattered matches it
  replaced), `markdown/latex.rs` ×4 (63/37/25/25 — converter tables),
  `calc.rs::tokenize` (31 — the recursive-descent evaluator),
  `wrap.rs::wrap_ranges` (29 — the hot-path width algorithm). **57 open
  issues remain** (48 Rust S3776 + 9 Python), which is what stages 1–4 burn
  down.
- **Stage 1 — `fix/sonar-python-tools`** (this entry's branch). The one real
  defect: `link_check.py`'s `CODE_SPAN` regex `` (`+)[^`]*?\1 `` is
  super-linear (S8786 — the greedy opener + backreference retries every
  opener length at every position). Replaced with a **linear manual scanner**
  (`blank_code_spans` + `_close_run`: an opening run of k backticks closes at
  the next run of ≥ k, an unclosed run stays literal). **Parity measured, not
  assumed**: over the whole 40k-line markdown corpus the 328 line-level
  diffs are all bare ``` fence lines (handled by the fence branch *before*
  spans, so unreachable) or inline mentions of triple-backtick fences, where
  the new behaviour is closer to CommonMark — and the tool's verdict on the
  repo is unchanged (clean).
- **Eight Python complexity refactors, all pure extractions** (dev scripts;
  behaviour pinned by offline probes rather than eyeballing):
  `cyrillic_scan.py::print_fmt_spans` (36) → per-state steps
  (`_find_macro`/`_open_quote`/`_close_literal` — a step returning `None` for
  the position ends the line, the returned state carries over);
  `cyrillic_scan.py::main` (40) → `scan_file`/`_line_flagged`/`report`;
  `e2e_hf.py::cmd_run` (41) → `Plan`/`print_plan`/`dry_run`/
  `create_endpoints`/`bring_up_all`; `cmd_sweep` (16) → `sweep_verdict` (the
  fail-safe "unknown createdAt → keep" reasoning moved into its docstring);
  `hf_api.py::cleanup` (20) → `_keep_endpoints`/`_verify_deletions` (the
  "a name leaves CREATED only once proven gone" invariant untouched — the
  verifier mutates, never rebinds); `hf_probe.py::cmd_doctor` (35) →
  `_granted_permissions`/`_report_fine_grained`/`_print_403_advice`;
  `probe_chat` (20) → `_check_tool_calling`/`_check_streaming`; `cmd_run`
  (30) → `_run_embed_only`/`_create_and_probe_embed`.
- **Verification** (the HF scripts have no offline entry beyond argparse, so
  the extracted pure parts were probed directly): `py_compile` on all five
  files; every `--help` path; `scan_file` against 11 cases including the
  cross-line print-macro tracker, the value-argument exemption and the
  `cyrillic-ok` markers; `blank_code_spans` against 7 cases plus the
  pathological 5000-backtick input the old regex choked on; `sweep_verdict`
  against 6 cases including both fail-safe unknowns and the
  fractional-second timestamp; full repo runs of `link_check` and
  `cyrillic_scan` — both still clean. **1835 unit tests green** (no Rust
  touched), clippy `-D warnings`/fmt clean. **No live run required**
  (AGENTS.md §3): dev tooling — no engine, memory or tool path. No CHANGELOG
  entry — internal tooling (§4).

### Post-M9: SonarQube backlog — stage 2 (UI screens and widgets) (done)

- **Stage 2 of the backlog burn-down** (`refactor/sonar-ui`, stacked on stage 1
  — every stage adds a journal entry here, so independent branches would
  conflict; the linear-stack precedent from the installers track). Seventeen
  `rust:S3776` functions across the UI layer reduced below the threshold by
  **mechanical, behavior-preserving helper extraction** — and, notably, **none
  needed an Accept**: unlike `field_spec` or the LaTeX tables, every one of
  these had natural seams (a popup branch, a render section, a per-arm body).
  Delegated to three parallel subagents over disjoint file clusters, each with
  the constraints spelled out (extraction only, comments travel with the code,
  tests must pass unedited, no tree copies — the poisoned-`target/` lesson).
- **Chat screens + self-model** (4): `chat/input.rs::handle_key` (**62**, the
  worst offender of the whole backlog) split into a modal router, the
  Ctrl-shortcut ladder, the plain-key match and per-slash-command helpers —
  routing order preserved (the tool-confirmation popup stays checked before
  the generation gate); `chat/render.rs::render` (24) → input-area/overlay
  helpers; `chat/rag.rs::set_rag_progress` (21) → per-arm helpers;
  `self_model.rs::render` (20) → row-expansion/row-drawing/editor-popup.
- **Settings screens** (6): `render_fields` (**47**) → `desc_panel_height`/
  `render_fields_header`/`build_field_items`/`render_desc_panel`/
  `group_toggle_counts`; `render` (18) → `footer_hints`/`render_editor_popup`;
  `handle_key_inner` (24) → `is_quit_key` + `plugins_list_key`/
  `profiles_list_key` (returning `Option<Option<_>>` — "consumed?" × result,
  because the originals' `_ => {}` arms fall through to the rest of the
  dispatcher) + `navigation_key`; `handle_fields_key` (18) → `field_enter`;
  `helpers::render_field_line` (25) → `field_value_and_style`/`row_marker`;
  `helpers::parse_args` (26) → `read_double_quoted`/`read_single_quoted`
  (taking `&mut impl Iterator<Item = char>`, so the tokenizer state machine is
  untouched).
- **Widgets** (7): `message_feed::from_messages` (21) → `merge_round`;
  `build_lines` (20) → `refresh_cached_block`/`highlight_block_tail` (the
  `CachedBlock` invariants moved verbatim); `highlight_line` (19) →
  `span_cut_points`; `status_bar::lines` (26) → `state_spans`/
  `token_counter_span`/`hotkey_list`; `status_bar::right_grid` (25) —
  triaged rather than assumed: the *algorithm* (`grid_layout`) was already its
  own under-threshold function, so the renderer split cleanly along
  row-lead/cell seams (`push_row_lead`/`push_hotkey_cell`) and no Accept was
  needed; `chat_list::on_key_search` (28) → `on_ctrl_search`/`open_selected`/
  `select_down`/`start_rename`; `input_box::on_key` (17) →
  `on_ctrl_shortcut`/`on_edit_key` (mutator calls moved untouched, so the
  `touch()`/`record_undo` sequencing is byte-identical).
- **No test was edited anywhere** — the 1835-test suite is the safety net the
  whole stage leans on, and it stayed green as-is: **1835 passed / 0 failed**,
  76 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
  **No live run required** (AGENTS.md §3): pure UI refactor — no engine,
  memory, tool or provider path is touched (the precedent set by the whole
  settings-redesign track). No CHANGELOG entry — internal refactor (§4).

### Post-M9: SonarQube backlog — stage 3 (runtime and shared) (done)

- **Stage 3** (`refactor/sonar-runtime-shared`, stacked on stage 2). Twelve
  `rust:S3776` functions across `app/runtime`, `shared`, `entities`,
  `features/spellcheck` and `main.rs` — again **all closed by extraction, none
  by Accept**: the three flagged going in as "may be inherent" (the raw-HTML
  state machine, both cloud wire translations) each turned out to have a seam
  the existing structure had already drawn.
- **Runtime** (2 agents in parallel over disjoint files, as in stage 2):
  `run_loop` (**46**) → `spellcheck_upkeep`/`spinner_frame_needed`/
  `draw_frame`/`handle_input_tick` — tick ordering and the `dirty` semantics
  untouched, the load-bearing comments (synchronized output, full-redraw
  triggers, paste batching) traveling with their code; `apply_event` (29) →
  seven arm-body helpers, the match itself staying the dispatcher;
  `process_input_batch` (17) → `handle_key_event`.
- **Shared**: `markdown/html.rs::html_block_to_lines` (28) → `apply_tag` — the
  split follows the seam the `DROPPED_ELEMENTS`/`LINE_BREAKING`/
  `WORD_SEPARATING` tables already drew, so the scanner itself (indices,
  quotes, comments, the unterminated-`<` fallback) stays one piece;
  `i18n.rs::dotted_literals_in_src` (22 — a *test-module* gate scanner, the
  runtime fallback chain untouched) → nested fns lifted + `key_run`;
  `theme.rs::hotkey_grid` (20) → `grid_col_widths`/`grid_cols`/`grid_keycap`;
  `mcp.rs::read_loop` (18) → `deliver_reply`/`answer_server_request`;
  `gemini/wire.rs::thinking_config` (22) → `gemini3_level`/`gemini25_budget` —
  extracted rather than Accepted because the per-generation split is exactly
  the seam and all three thinking-config tests pin it (both 2.5-Pro clamps
  included); `responses/wire.rs::build_input` (17) → `push_assistant_items`,
  pinned by the reasoning-item-ordering tests.
- **The rest**: `dict.rs::load_dir` (19) → `load_entry`;
  `entities/self_model.rs::apply_edit` (19) → `assign_trimmed`/`assign_list`/
  `set_goal_text`/`clear_all` (now essentially the dispatch);
  `main.rs::apply_env_overrides` (16) → `apply_engine_env`/`apply_embed_env`.
- **No test edited**: **1835 passed / 0 failed**, 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No live run
  required** (AGENTS.md §3): the two wire-file extractions are
  byte-identical request-shape moves pinned by their own wire unit tests, and
  the cloud protocols additionally ride the `MINDFORK_GEMINI_KEY`/
  `MINDFORK_OPENAI_KEY` smokes outside CI; nothing else touches an engine,
  memory or tool path. No CHANGELOG entry — internal refactor (§4).

### Post-M9: SonarQube backlog — stage 4 (orchestrator and tools) (done)

- **The final code stage** (`refactor/sonar-orchestrator-tools`, stacked on
  stage 3): nineteen `rust:S3776` functions on the engine/memory/tool paths —
  the most concurrency-critical part of the backlog, so the brief to both
  subagents was "fewer, larger, verbatim-moved helpers" with the invariants
  named up front. Again **all closed by extraction, none by Accept** — across
  the whole triage, the only functions whose complexity proved genuinely
  inherent were the seven identified up front in stage 0.
- **Orchestrator** (8): `generation.rs::spawn_generation` (**50**, the
  agentic loop) — the per-turn state moved into a private `TurnLoop` struct
  whose methods carry the loop verbatim (`run` / `tool_round` /
  `execute_call` / `resolve_call_result`, following the module's existing
  parameter-struct pattern — `GenSpawn`, `ConfirmGate`); channel order,
  effect timing, the confirmation round-trip, control-tool recognition,
  `sync_attachments` mirroring and the thinking-signature accumulation all
  moved unchanged. `tool_loop.rs::run_rounds` (19) → `read_round`/
  `invoke_allowed`; `tts.rs` ×3 (18/18/22) → `append_pieces`/
  `split_giant_word`/`synth_chunk` (the synthesize-ahead pipeline and
  pause/cancel semantics verbatim); `rag.rs::spawn_rag_rebuild` (26) — split
  along its own numbered phases into `gather_rebuild_sources`/
  `prepare_rebuild`; `reembed.rs::drain_store` (16) → `write_batch`, with
  both loop guards and the vector-then-stamp ordering staying where they
  were; `search.rs::reconcile` (17) → `reconcile_file` (the hidden-chat
  rationale comment traveling whole).
- **Tools/features** (11): `web.rs` ×2 (20/21 — provider order, throttling
  detection and best-effort degradation verbatim), `youtube.rs` (16),
  `self_model.rs` ×2 (**40**/22 — the atomic `self_model_update` closures now
  call helpers from *inside* the closure, so the atomicity boundary is
  untouched), `rag.rs` ×3 (20/23/22 — the chunk/stitch algorithms had natural
  seams after all), `notes/recall.rs` (21), `notes/self_notes.rs` (29),
  `mcp_import.rs` (18).
- **No test edited**: **1835 passed / 0 failed**, 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean — the exact
  pre-change baseline. **Live run — GO** (AGENTS.md §3): this stage touches
  the agentic loop and every tool path, and the LAN stack was unreachable at
  implementation time (both `192.168.1.20` servers timing out) — so the gate
  ran through the **remote HF runner** (`tools/e2e_hf.py run`, the exact
  scenario it was built for): Gemma 4 31B q4_0 on nvidia-l40s + bge-m3 and
  the alternate e5 embedder on T4s, llama.cpp `server-cuda`. **76 passed /
  0 failed** — the *entire* `#[ignore]` suite, not just the orchestrator
  e2e set: memory/self-model/notes/graph/cross-organ links/RAG/attachments/
  control tools/tool confirmation/MCP (a real `npx server-filesystem`,
  14 tools)/i18n/web/fetch, plus the four model-change smokes on the
  alternate embedder. Ready in 139 s, suite 1151 s, total 1290 s; all three
  endpoints deleted and verified gone, `list` empty.
- **With this stage the backlog burn-down is code-complete**: 57 open issues
  at the end of stage 0 → 0 expected after the four stage merges re-analyze
  on `main` (48 Rust + 9 Python fixed across stages 1–4).

### Post-M9: the quality gate stopped judging new-code coverage (done)

- **Found by the four backlog PRs meeting the blocking gate** — the first time
  the decision "make the gate blocking" (2026-08-05) met a pure refactor of
  legacy code. Three of the four failed, and the shape of the failure is the
  whole story: **zero new issues on all four**, ratings A, duplication 0%, and
  the *only* failing condition `new_coverage ≥ 80` — #269 at 0.0%, #271 at
  66.2%, #272 at 72.5%, with #270 passing at 96.1%. Analysis read through the
  SonarQube MCP server (registered in the user-scope Claude Code config this
  session) plus the REST API for the per-file breakdown the MCP tools do not
  expose.
- **Not a coverage regression, and that was measured rather than argued.**
  Overall project coverage: `main` 85.2%, the four PRs 85.2 / 85.3 / 85.2 /
  85.2. What changed is the *ledger*: extraction rewrites lines, so branches
  that had been uncovered for a long time stop being "old code" and start
  counting against the gate. Cross-checked against the repo's own `lcov.info`
  baseline, taken from `main` **before** these branches, and it matches file by
  file — `app/runtime/mod.rs` 29.6% on main → 63 of 63 new lines uncovered,
  `main.rs` 8.4% → 6 of 6, `orchestrator/tool_loop.rs` 26.7% → 27 of 27,
  `runtime/input.rs` 52.9% → 19 of 19.
- **The concentration is not a coincidence**: high cognitive complexity lives
  exactly where unit tests cannot reach — the TUI loop (needs a real terminal),
  the network/audio/cloud paths, the background tasks. Refactoring the most
  complex function in a file preferentially rewrites its *uncovered* half. And
  the percentage understates reality for those files: `web.rs`, `tts.rs`,
  `youtube.rs`, `tool_loop.rs` **are** tested — by the 76 `#[ignore]` live
  smokes (stage 4 ran 76/76 on the remote HF runner), which `cargo llvm-cov`
  does not execute. "Uncovered" there means "covered only by the live gate".
- **The fork that decided the design: `sonar.qualitygate.wait=true` does two
  jobs**, and the journal records the *second* as the reason it was enabled —
  it waits for server-side processing, which is what caught the report the
  server later rejected over the LOC quota while the job stayed green. So the
  obvious response ("make the gate non-blocking") would have thrown that away
  as collateral, since one flag carries both meanings. Four options were
  weighed: `wait=false` (loses the upload check), `continue-on-error` on the
  job (loses it too, and leaves a verdict that is visible but unread), coverage
  exclusions on `src` (inflates the number and hides real gaps in Rust code
  that *is* testable), and a custom gate.
- **Decision (user, 2026-08-06): a custom gate, blocking kept.** The built-in
  `Sonar way` cannot be edited (`isBuiltIn=true`; the organization has only the
  two built-ins), so the applied gate is now **"Sonar way without new-code
  coverage"** — its five other conditions, verified read-back to be identical
  to the source. The reasoning is marginal value: `clippy -D warnings`, 1835
  tests, `cyrillic_scan`, `link_check` and `cargo deny` already block, and what
  Sonar adds on top is cognitive complexity, security hotspots and duplication
  — which is what produced the 114-issue backlog and still blocks. Coverage is
  a metric the project already measures, stable at 85.2% and printed on every
  PR; blocking on it in a codebase with a deliberately untestable core was
  blocking on an accounting artifact.
- **The cost is stated rather than glossed** (in `sonar-project.properties` and
  the roadmap): a genuinely untested new feature can now pass the gate. What
  remains against that is the reported number and AGENTS.md §3.
- **`sonar.coverage.exclusions=tools/**` — done for measurement honesty, not
  for the gate** (which no longer judges coverage). The Rust-only lcov report
  cannot cover Python, so ~1240 lines of dev scripts sat in the denominator as
  100% uncovered, dragging the reported figure down for files that have no test
  harness by design. Coverage-only: they stay in `sonar.sources` and keep being
  analyzed for issues, so stage 1's nine Python findings are unaffected.
- **No CHANGELOG entry** — dev infrastructure with no user-visible effect
  (AGENTS.md §4), consistent with every other Sonar/CI entry. No Rust code
  touched, so the suite is unchanged at **1835 unit tests**, 76 `#[ignore]`.
  **No live run required** (§3): no engine, memory, tool or provider path
  exists in this change.

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

### Post-M9: the documentation refactor — CLAUDE.md became a router (done)

- **Trigger**: a session began with ~450K of its 1M-token window already spent,
  before any work started. Measured rather than estimated, and the cause was one
  file: `CLAUDE.md` is the only document loaded **in full, automatically**, and it
  had grown to **994 562 bytes (~249K tokens)** — of which the guide a session
  actually needs every time was **8 273 bytes** and the rest was a 243-entry
  engineering journal appended chronologically since M9. The remaining ~200K came
  from the standing instruction to also read `architecture.md` (144 KB) and the
  ADRs, both read whole though both are chaptered documents where a task needs two
  or three sections. Design plan and the measurements —
  [documentation-refactor.md](../history/documentation-refactor.md); forks F1–F7 all
  adopted as recommended (user's decision, 2026-08-08).
- **The diagnosis that shaped the split: the unit of loading was the file, the unit
  of need is the entry.** The average entry is ~4 KB and a typical task needs two or
  three; we were loading 243. And `docs/` was organized by **lifecycle**
  (`research/` before a decision, `history/` after a track) rather than by
  **subject**, so nothing answered "what must I read to work on the settings
  screen?".
- **What the journal is actually for** (established by reading across it before
  touching it, because the answer decides what may be moved): it is not a changelog
  — `CHANGELOG.md` is. Its unique content is **measurements** (85 entries) and
  **traps** (128 entries carry "worth recording"/"caught by"/"mutation-tested"
  language). The entries know it: ten cite an earlier entry's trap, and the
  `git checkout <file>` revert is recorded **three separate times** because it bit
  three times. The rest — what/why, forks, test counts — is largely duplicated into
  CHANGELOG, spec/architecture, and the design docs.
- **Split by subsystem along `architecture.md`'s chapters** (F1a), so the map is
  self-evident and mechanically checkable rather than ad-hoc: `docs/journal/`
  {`engine`, `storage`, `tools`, `self-model`, `notes`, `rag`, `ui-feed`,
  `ui-input`, `ui-screens`, `i18n`, `platform`, `refactors`, `milestones`}`.md`, each
  with a reference header naming its architecture/spec sections and an index of its
  entries. Chronological **within** a file; the M3–M9 milestone log moved to
  `milestones.md` rather than being deleted. Memory started as one `memory.md` and
  was split again in the same PR, at the user's request: at 180 KB and 45 entries it
  was the one file an agent would grep rather than read, and the project's own
  vocabulary already names three organs. Attachments and the embedding stack went
  with `rag.md` — they are retrieval infrastructure, and the alternative was a
  fourth file for machinery that serves all three.
- **Moved verbatim (F3a), and that is what made it verifiable**: 243 entries,
  **961 820 bytes byte-identical**, zero missing, zero extra, matching SHA-256. The
  alternative on the table — compressing older entries while moving — was rejected
  because it is editorial judgement applied 243 times, unverifiable, and destroys
  exactly the measurements and reasoning that are the journal's unique content.
- **`docs/lessons.md` is the piece that makes the split safe** (F2a): 65 lessons in
  10 sections, deduplicated, each a rule plus the incident that produced it plus a
  pointer to its journal entry. While the journal was one file an agent working on
  *anything* incidentally knew every trap the project had hit; this file is that
  serendipity, made deliberate. Recurrence counts are kept as evidence — a trap
  recorded three times is a trap that is easy to hit.
- **CLAUDE.md is now a router**: **994 562 → 11 253 bytes** (−98.9%). Orientation,
  the standing decisions, FSD structure, conventions, commands, a short Status, and
  a **document map with trigger conditions** ("working on X → architecture §N ·
  spec §M · `docs/journal/<area>.md`") rather than a list of documents. Two standing
  rules stated there: *read the section, not the file*, and *the journal is per
  area*. `spec.md` and `architecture.md` were deliberately **not** split (F7a) —
  their stable numbering is cited from dozens of places (`spec §9.6`), and reading
  by section already gets the saving.
- **A gate, because the structure rots silently** (F5a, the house answer since
  `link_check.py`/`cyrillic_scan.py`): `tools/doc_index_check.py` (331 lines,
  stdlib) checks each file's index against its headings **as an ordered sequence**,
  duplicate titles within and across files, two-way reachability between the router
  and the journal files, that `lessons.md` exists and is linked, and that CLAUDE.md
  stays under `CLAUDE_MD_MAX_BYTES = 30_000`. A missing or empty `docs/journal/` is
  a **failure**, not a pass. Runs in the `lint` job — the job that runs on the
  docs-only PRs where documentation structure actually breaks.
- **Six traps hit while doing it**, all worth the record:
  (1) the first verification script compared the journal files against `CLAUDE.md`
  — which had already been rewritten, so it would have passed **vacuously**; the
  real check reads the original from `git show HEAD:CLAUDE.md`.
  (2) The classifier's `^M3 ` patterns were uppercase and matched against a
  lowercased title, so every milestone heading fell through to a keyword rule.
  (3) Relative links from `docs/journal/*.md` need one more `../` than the source
  they were copied from — the same class of miss the "plan moves to
  `docs/history/`" rule warns about, and **106 entry bodies carried it**. The
  interesting part is why it stayed hidden: `link_check.py` walks **git-tracked**
  files, `docs/journal/` was untracked, and the gate therefore reported *clean*
  three times in a row while never once looking at its subject. It only spoke up
  after `git add`. So the entries are verbatim with exactly one mechanical
  exception — root-relative link targets gained a `../../` prefix — and the
  byte-identity proof normalizes that prefix away before hashing, which is what
  keeps it a proof rather than an assertion.
  (4) The gate's own footer hardcoded `docs/documentation-refactor.md`, which moved
  to `docs/history/` mid-session — a stale path inside the tool that exists to
  prevent stale paths.
  (5) **12 entry titles contain `→` (U+2192)**, which cp1252 cannot encode, so the
  gate's console guard is load-bearing rather than ceremonial.
  (6) A scripted multi-edit pass that computes every replacement from the
  **original** text and writes once at the end keeps only the last edit per file;
  `link_check` cannot see it, because every surviving path still resolves. Re-run
  sequentially and re-read the diff.
- **Classification was scripted, then corrected by hand** — keyword rules over 244
  headings, then **38 manual overrides**, because the rules failed predictably:
  "status bar" entries landed in `ui-input`, memory-track refactors in `refactors`,
  and `settings-screen redesign — stage 2` in `engine` because its title contains
  the word "server". A heading is a weak classifier; what froze the result was the
  byte-identity check, not the rules.
- **Inbound references**: 91 across 41 files. **59 re-pointed** to the matching
  journal file (pointers to "the log", and process instructions telling an agent to
  write a journal entry); **32 left alone** because they genuinely still mean
  CLAUDE.md (orientation, `§Conventions`, `§Commands`, `§Status`). Deliberately
  untouched: the six mentions in `docs/history/english-source-migration.md`, which
  are statements of historical fact about translating the *file* — re-pointing them
  would falsify the record. Seven stale pointers in `src/**/*.rs` doc comments were
  fixed too, which is why this PR compiles code it does not change.
- **Result**: automatic load **~249K → ~2.8K tokens**; a typical task ("work on the
  settings screen" — router + architecture §10 + the relevant spec §§ +
  `journal/ui-screens.md` + lessons) is **~40–60K instead of ~450K**, with nothing
  deleted and everything reachable from the map.
- **Verification**: 243/243 entries byte-identical (hash-compared against the
  pre-change file in git); **1952 unit tests green, 81 `#[ignore]`** (unchanged — no
  behaviour touched); `link_check.py`, `cyrillic_scan.py` and the new
  `doc_index_check.py` clean; the gate mutation-tested in nine directions (appended
  entry without its bullet, stale bullet, swapped index order, duplicate title
  across files, unreferenced journal file, dangling reference, missing
  `lessons.md`, emptied journal directory, oversized router) — each fails on its
  own and only its own. **No live model run is required** (AGENTS.md §3):
  documentation, doc comments and one dev script; no engine, memory, tool or
  provider path is touched. **No CHANGELOG entry** — no user-visible effect (§4).
- **Process rules updated**: AGENTS.md §1 now sends a task through the map instead
  of through the whole of CLAUDE.md and states the read-the-section rule; §4's
  "journal entry" row names the matching `docs/journal/<area>.md` **and its index**,
  and a new row sends a recurring trap to `docs/lessons.md`.
