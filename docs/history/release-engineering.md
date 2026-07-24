# Release engineering: versioning, changelog, CI, and data migrations

Design plan for the track (genre per AGENTS.md §1). Status: **stages 1–5
implemented** (forks confirmed by the user 2026-07-15 per the recommendations
[rec.] in all blocks A–D). Remaining: the first tag `v0.9.0` (a live run of
`release.yml` + an artifact smoke), optional stage 6 (`cargo-deny`), then
moving the plan into `docs/history/`.

The project reached the point where it needs: proper application versioning,
a changelog, CI, and — most importantly — **versioning the schemas of all
saved data with a migration mechanism**, so a binary upgrade never loses
user data.

---

## 1. Audit of the current state

| Area | Now | Problem |
|---|---|---|
| App version | `Cargo.toml` = `0.1.0`, never bumped; no git tags; no releases | the version says nothing; no rollback/comparison points |
| Showing the version | `mindfork-rs --version` prints `CARGO_PKG_VERSION` (key `cli.version.line`) | no version in the TUI or the startup log — diagnosing "which binary does the user have" is hard |
| Changelog | none | the user has nothing to read on upgrade; the CLAUDE.md journal is for development, not for users |
| CI | none (`.github/` — just a PR template) | the `fmt`/`clippy -D warnings`/`test` gates rely only on agent discipline; the Linux build isn't checked at all (development happens on Windows) |
| License | `license = "MIT"` in Cargo.toml, no `LICENSE` file | the code isn't formally licensed; release archives have nothing to bundle |
| `settings.json` | `AppConfig.schema_version: u32 = 1` — the field **exists but is never checked**; on corrupt JSON `main.rs` does `unwrap_or_default()` | a decorative version; a corrupted file gets **silently overwritten with defaults** on the next save (the `.bak` then gets overwritten with the corrupted version too) |
| `profiles.json` | a bare `Vec<Profile>`, no version | a breaking format change has nothing to detect it |
| `chats/*.json` | a `Chat` object, no version; **one corrupt file crashes `load_chats()` entirely** | a breaking change has nothing to detect it; a corrupt chat blocks app startup |
| `data.db` (SQLite) | `migrate()` — only idempotent `CREATE TABLE IF NOT EXISTS`; `PRAGMA user_version` isn't used (always 0); `meta.rag_dim` for vec0's dimensionality | additive-only: renaming a column/table or changing semantics is impossible without loss |
| Change policy | "new fields — `#[serde(default)]`, schema — `CREATE IF NOT EXISTS`" (AGENTS.md §3) | covers **only** additive changes; the one breaking change in history (nested engine config) was resolved "without migration" — the user had to re-enter managed settings/keys |
| Backup | `mindfork backup/restore` — zip, a transactional restore with a pre-restore copy and rollback | the mechanism is excellent, but **not tied** to upgrades: no auto-backup before data migration; no version manifest in the archive |
| Downgrade | not detected at all | an old binary on newer data "reads it best-effort" — silent corruption |

Other artifacts and their status: `defaults.json` (the install marker,
flatten + tolerance + a fallback to the legacy `location.json` — already
version-resilient), `personal_dictionary.txt` (lines, a trivial schema),
`data/locales/*.json` (user content, already validated), `data/sandbox/`
(provisioning, recoverable via `sandbox setup`), logs (not data).

---

## 2. Forks

Format: options, **[rec.]** — recommendation. Confirmed by the user before
implementation (AGENTS.md §1).

### Block A — version and changelog

- **F1. Starting version and the path to 1.0.**
  - (a) **[rec.]** SemVer; the first release is **`v0.9.0`** (a signal of
    "almost 1.0"); **`1.0.0`** — once the track is proven in production: CI
    green on both platforms, the migration framework merged, the release
    pipeline has shipped ≥1 release. From 1.0 on, "data survives upgrades"
    is a contractual promise.
  - (b) `1.0.0` right away — M0–M9 have long been done; but the migration
    promise isn't backed yet.
  - (c) keep going with `0.1.x` — honest, but undersells the maturity.
- **F2. How the changelog is maintained.**
  - (a) **[rec.]** a manual `CHANGELOG.md` (Keep a Changelog 1.1, in
    Russian), the `[Unreleased]` section is filled in **on every PR with a
    user-visible effect** (a checklist item in the PR template and
    AGENTS.md §4). The project's commits are conventional but written for
    the developer (Russian, with implementation details) — generating from
    them would give a noisy log, not a user changelog.
  - (b) generating with `git-cliff` from conventional commits — less
    discipline, worse text.
  - (c) a hybrid (a generated draft, manually edited) — double the work.
- **F3. Changelog rubrics** (minor): Russian analogues of Keep a Changelog —
  "Added / Changed / Fixed / Removed" + **its own "Data" rubric** (storage
  format changes and migrations — the thing users care about most) [rec.].

### Block B — CI

- **F4. Toolchain: pinned or floating stable.**
  - (a) **[rec.]** pin a minor version in `rust-toolchain.toml` (+
    `rustfmt`, `clippy` as components). Reason: the `clippy --all-targets
    -- -D warnings` gate — every new stable brings new lints and **would
    break CI out of nowhere**. With a pin, upgrading the toolchain is a
    deliberate, separate PR ("bump to 1.NN", fixing the new lints), like
    upgrading any dependency.
  - (b) floating `stable` — always-fresh lints, but red CI on a random
    Tuesday through nobody's fault.
- **F5. Test matrix.**
  - (a) **[rec.]** `ubuntu-latest` + `windows-latest` (both declared
    platforms; Linux currently isn't checked at all — the first run will
    likely surface platform-specific test issues, fixed on the spot in
    this stage). One lint job (fmt+clippy) — on ubuntu (faster and
    cheaper, lints are platform-independent, `#[cfg(windows)]` code being
    checked by clippy on the windows job isn't needed — `cargo test`
    itself compiles it on windows).
  - (b) windows only (the current dev platform) — Linux stays a blind spot.
  - (c) ubuntu only — the primary platform isn't checked.
- **F6. Dependency audit** (`cargo audit`/`cargo deny`).
  - (a) **[rec.]** a separate optional final stage: `cargo-deny`
    (advisories + licenses + duplicates) on a schedule (weekly) and
    manually, **doesn't block** merges (an advisory job).
  - (b) don't introduce it.

### Block C — schema versions and migrations

- **F7. Schema-version granularity.**
  - (a) **[rec.]** **per artifact**: `SETTINGS_SCHEMA` / `PROFILES_SCHEMA` /
    `CHAT_SCHEMA` / `DB_SCHEMA` (u32, starting at 1). Artifacts change at
    different rates; a single global number would force "migrating"
    untouched files.
  - (b) one global number for all data — simpler to think about, cruder to
    work with.
- **F8. JSON migration mechanics.**
  - (a) **[rec.]** transformations over `serde_json::Value`: a step is a
    pure `fn(Value) -> Result<Value>` "version N → N+1", followed by a
    control-parse into a typed struct and an atomic write. Old format
    versions live **only as golden fixtures in tests** (raw JSON strings),
    not as a zoo of `SettingsV1/V2/…` types.
  - (b) per-version snapshot types + a `From` chain — type-safe, but drags
    in and freezes every historical struct into the code forever.
- **F9. The migration moment.**
  - (a) **[rec.]** **eager at startup**, before the first typed-data read:
    `Storage::open` builds a plan (which files are stale, whether a DB
    migration is needed); a non-empty plan → **one pre-migration backup**
    (reusing `features/backup::create_backup` → `backups/pre-migrate-
    <date>.zip`; a failed backup → the migration doesn't start, data is
    untouched) → migrate all files → a summary in the log. 226+ chats —
    instant; one point in time, one mental model, one backup.
  - (b) lazy, on reading each file — smears the moment out, mixed versions
    on disk, a backup "eventually, piecemeal".
- **F10. Data newer than the app (downgrade).**
  - (a) **[rec.]** refuse to start with a clear, localized message ("this
    data was created by a newer mindfork version; upgrade the app or
    restore a backup"). Silent corruption is worse than a refusal. Same
    rule in the CLI paths (`import`, TUI operation); `restore`ing an
    archive with a newer manifest — a warning (after restoring, the old
    binary will honestly refuse to start, data is intact, the new binary
    will open it).
  - (b) best-effort reading — a source of hard-to-spot corruption.
- **F11. Hardening reads of corrupted files** (a companion, same stage).
  - `settings.json`/`profiles.json` corrupt → **refuse to start** [rec.]
    (currently — silent defaults followed by an overwrite; a precedent of
    strictness — a corrupt `defaults.json` is already a startup error).
  - A corrupt `chats/<id>.json` → **skip with a `warn` in the log, leave
    the file untouched** [rec.] (currently one corrupt file crashes the
    whole startup; but silently losing a chat isn't acceptable either —
    the file stays on disk for manual repair).
- **F12. Policy on "what counts as a bump"** (locked in in AGENTS.md and
  an ADR).
  - **Additive** (a new field with a default, a new table/index/column
    with a default) — **no bump**, as now: `#[serde(default)]` /
    `CREATE IF NOT EXISTS` / `ALTER TABLE ADD COLUMN` in the baseline
    aren't required. Nothing is redone.
  - **Breaking** (renaming/moving/changing semantics/removing a field,
    changing a value's format, restructuring tables) — **a constant bump +
    a migration step + a golden fixture of the old format + a changelog
    entry ("Data")**. The past precedent (nested engine config) would have
    become a migration under this framework, rather than losing settings.

### Block D — releases

- **F13. Building release artifacts.**
  - (a) **[rec.]** its own workflow (`release.yml` on tag `v*`, ~80 lines):
    `cargo build --release` on `windows-latest` and **`ubuntu-22.04`**
    (old glibc 2.35 — the binary runs on most live distros; a musl-static
    build — groundwork), packaging `mindfork-rs(.exe)` + `README.md` +
    `docs/install.md` + `CHANGELOG.md` + `LICENSE` into
    `mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}` + a
    `sha256sums.txt` file, publishing via `gh release create` with notes =
    the version's section from the CHANGELOG. A project precedent — its own
    micro-solutions instead of heavy tooling.
  - (b) `cargo-dist` — powerful (installers, an updater), but its own
    config/upgrade ecosystem for just two artifacts.
- **F14. Where the release checklist lives.**
  - (a) **[rec.]** a new section, **AGENTS.md §6 "Release"** — the whole
    task process lives there, a release is its continuation.
  - (b) a separate `docs/releasing.md`.

---

## 3. Design

### 3.1 Application versioning

- **SemVer**, source of truth — `Cargo.toml` (`env!("CARGO_PKG_VERSION")` is
  already used in `--version`). A release = a git tag `vX.Y.Z` on the merge
  commit into `main`.
- While `0.x`: MINOR — features/tracks, PATCH — fixes. From `1.0.0` on —
  full SemVer, where MAJOR is reserved for incompatibilities **not covered
  by a migration** (the goal — that there are none at all).
- Version ↔ data schemas are **independent**: a schema bump can happen in
  any MINOR (the migration makes it safe), the schema number isn't
  embedded in the app version.
- Showing the version: `--version` (exists) + the help overlay's title
  `F1` (`mindfork-rs vX.Y.Z`) + `tracing::info!` at startup (`version = …`
  next to `root = …`) — so every log starts with the binary's version.

### 3.2 CHANGELOG.md

- Format Keep a Changelog 1.1, in Russian, rubrics: Added / Changed /
  Fixed / Removed / **Data** (storage formats, migrations) / Security.
- There's always an `[Unreleased]` section; GitHub comparison links
  (`…/compare/v0.9.0...HEAD`).
- **Discipline**: a PR with a user-visible effect adds an item to
  `[Unreleased]` — a new checklist item in the PR template and a row in
  AGENTS.md §4's table. One item = one or two lines in user-facing
  language (not the CLAUDE.md journal).
- Initial fill-in: the `[0.9.0]` section — a condensed retrospective of
  "what the app can do" (15–25 lines, by track), linking to the CLAUDE.md
  journal for detailed history. The whole journal isn't carried over.

### 3.3 CI (GitHub Actions)

`.github/workflows/ci.yml`:

- Triggers: `pull_request` + `push` to `main`; `concurrency` with
  `cancel-in-progress` by ref.
- Jobs:
  - **lint** (ubuntu): `cargo fmt --check`, `cargo clippy --all-targets
    -- -D warnings`;
  - **test** (matrix: `ubuntu-latest`, `windows-latest`): `cargo test`
    (`#[ignore]` smokes are silently skipped without env variables —
    already set up that way; CI needs no network/live server; spawns in
    tests are cross-platform — `cmd /C exit` / `sh -c "exit 0"`).
- Cache — `Swatinem/rust-cache`; toolchain — from `rust-toolchain.toml`
  (F4).
- A CI badge in the README.
- Toolchain upgrades — a separate periodic PR (the pin bumps, new lints
  get fixed in the same PR).

Expected "first pitfalls" of the Linux run (fix them in this same stage):
platform assumptions in path/process tests; that's exactly the value of the
matrix.

### 3.4 Data-schema versioning and migrations

A new module — **`shared/storage/schema.rs`** — the sole home for version
constants and the migration registry (ADR 0006 will lock in the decision as
a whole).

**Constants:** `SETTINGS_SCHEMA = 1`, `PROFILES_SCHEMA = 1`,
`CHAT_SCHEMA = 1`, `DB_SCHEMA = 1`. The existing
`shared/config.rs::SCHEMA_VERSION` moves here (the field
`AppConfig.schema_version` stays — it's already in the files).

**Detecting a file's version — structural, the format doesn't change today:**

| Artifact | How the version is determined |
|---|---|
| `settings.json` | the `schema_version` field (absent → 1) |
| `profiles.json` | an array → 1; an object → its `schema_version` (an envelope shape will appear at the first breaking change) |
| `chats/*.json` | the `v` field (absent → 1) — the field is **not** written while the schema = 1 (zero churn in existing files) |
| `data.db` | `PRAGMA user_version` (0 → legacy/fresh DB, baseline sets it to 1) |

**JSON framework:**

```rust
struct Step { to: u32, summary: &'static str, apply: fn(Value) -> Result<Value> }
struct JsonArtifact { current: u32, detect: fn(&Value) -> u32, steps: &'static [Step] }
```

`Storage::open` flow (every storage-opening point — the TUI and the CLI
`import` — is a single path): read each file into a `Value` → `detect` →

- `v == current` → as now;
- `v > current` → **a startup error** (F10), localized text (bundle keys);
- `v < current` → the file goes into the migration plan.

A non-empty plan → a pre-migration backup (F9) → for each file: a `steps`
chain `v..current` → **a control-parse into a typed struct** (a migration
after which the file doesn't parse — an error, the file isn't overwritten)
→ an atomic write via the existing `write_json` (temp + rename + `.bak`) →
a summary in the log (`migrated settings.json 1→2; chats: 214 files 1→2`).

**SQLite framework (`db/mod.rs::migrate`):**

```rust
let v = user_version(conn)?;                 // PRAGMA user_version
if v > DB_SCHEMA { bail!(downgrade) }
if v == 0 { baseline(conn)?; set_user_version(conn, 1)?; }  // the existing idempotent DDL
for step in DB_STEPS.iter().filter(|s| s.to > v) {
    // each step — in a transaction; user_version updates inside it
}
```

- A `data.db` backup before steps ≥2 — via the SQLite backup API
  (`rusqlite::backup`, correct under any journal mode) into `backups/`;
  together with the JSON plan this is one shared "pre-migrate" moment
  (order in `Storage::open`: assess the JSON plan + peek at `user_version`
  → a shared backup → JSON migrations → `Db::open` with the steps).
- The vec0 virtual table isn't "migrated" via ALTER — on an incompatible
  change a step recreates it via the existing path
  (`rag_reset_vectors` + reindexing, precedent `/rag rebuild`).
- `meta.rag_dim` stays as is (data, not schema).

**Hardened reads (F11):** `main.rs` stops swallowing a corrupt config
(`load_config().unwrap_or_default()` → a startup error with text); a
corrupt chat file is skipped with a `warn`, without blocking startup and
without being overwritten.

**Tests (the framework's safety net):**

- golden fixtures: raw JSON of every historical version of every artifact
  (for now — one "v1" fixture each; every future migration must add a
  fixture of its old version) → after migration it parses and is
  semantically correct;
- negative: a newer version → an error, data untouched; a corrupt file →
  the F11 policy;
- a migration with no plan → no backup is created; with a plan → created
  before the write;
- DB: `user_version` 0→1 on an existing DB (baseline idempotency), a step
  fully rolls back in a transaction on error.

**Policy in the process:** AGENTS.md §3 — the "new fields —
`#[serde(default)]`" item is extended with the F12 policy (additive with
no bump / breaking = a step + a bump + a fixture + a "Data" changelog
entry); the PR template — a checklist item "the data format hasn't changed
/ the change is covered by a migration and a fixture."

**Backup manifest (in the release stage):** `backup.rs` puts into the zip
`manifest.json { app_version, schemas: {settings, profiles, chat, db},
created_at }`; `restore`, when the manifest is newer than the current
schemas — a warning (data is intact, the startup downgrade guard still
protects it either way).

### 3.5 The release process

Checklist (AGENTS.md §6, F14):

1. A release PR: bump `Cargo.toml` (+`Cargo.lock`), `[Unreleased]` →
   `[X.Y.Z] — date`, a fresh comparison link.
2. Merge → the user applies and pushes tag `vX.Y.Z` (the agent doesn't push
   to `main` or push tags — AGENTS.md §5).
3. The tag triggers `release.yml`: build → package → sha256 →
   `gh release create` with notes from the CHANGELOG section.
4. An artifact smoke: download, `--version`, run the TUI on a copy of the
   data.

---

## 4. Implementation plan (stages = separate PRs)

Order: CI first — it becomes the safety net for everything else.

| Stage | Branch | Contents | DoD |
|---|---|---|---|
| 1. CI | `feat/ci-pipeline` | `rust-toolchain.toml` (pin), `.github/workflows/ci.yml` (lint on ubuntu + a win/linux test matrix), a `LICENSE` file (MIT), a README badge; fix Linux platform tests as found on the first run | CI is green on a PR on both OSes |
| 2. Version + changelog | `feat/versioning-changelog` | bump to `0.9.0`; `CHANGELOG.md` (format, seed `[0.9.0]`, `[Unreleased]`); the version in the `F1` help title + the startup log; AGENTS.md §4 + the PR template (a changelog checklist item) | `--version`/`F1`/the log show 0.9.0; the discipline is locked into the process |
| 3. JSON migrations | `feat/json-schema-migrations` | `shared/storage/schema.rs` (constants, `JsonArtifact`, the runner), a downgrade guard, an eager migration with a pre-migration backup, the F11 hardening, golden fixtures, localized errors (bundle keys), **ADR 0006** (JSON+DB as a whole), spec §5.2/§12 + architecture §7 | the framework works in production on "empty" migrations (all schemas = 1); fixtures and negative tests are green; **a manual run on a copy of real data** (226+ chats) |
| 4. SQLite migrations | `feat/db-schema-migrations` | `PRAGMA user_version` + a step runner in transactions, baseline 0→1, a backup via the SQLite backup API, integration into the shared pre-migrate plan in `Storage::open` | an existing DB opens with `user_version=1`; transactionality tests; a manual run on a copy of a real `data.db` |
| 5. Release pipeline | `feat/release-pipeline` | `release.yml` (tag `v*` → build win/linux-22.04 → archives + sha256 → a gh release with notes from the CHANGELOG), AGENTS.md §6 "Release", a version manifest in the backup zip + a restore warning | tag `v0.9.0` (applied by the user) builds a public release with artifacts; the artifact smoke passes |
| 6. (opt.) Supply chain | `feat/supply-chain-audit` | `cargo-deny` (advisories/licenses/dupes), weekly + manual workflow, advisory mode | the job works, doesn't block merges |

The track needs no live engine runs (the engine/memory/tools aren't
touched) — instead, mandatory **manual migration runs on copies of real
data** (stages 3–4) and a release-artifact smoke (stage 5).

## 5. Out of scope (groundwork)

- App self-update and installers — a separate track (`cargo-dist`/
  winget/deb — once there's demand).
- A musl-static Linux binary; arm64 builds.
- A TUI notification "a new version is available" (checking GitHub
  Releases).
- Generating the changelog from commits (git-cliff) — if manual discipline
  becomes a burden.
- Automatic cleanup of old pre-migration backups (`backups/` rotation).
