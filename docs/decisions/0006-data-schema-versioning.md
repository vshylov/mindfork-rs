# ADR 0006 — Data schema versioning and the JSON migration scaffold

**Status:** accepted (2026-07-15). Fixes stages 3–4 of the "release engineering"
track (versioning schemas of persisted data + JSON and SQLite migrations).
Design and decision points —
[docs/history/release-engineering.md](../history/release-engineering.md) §3.4
and §2 block B (F7–F12). Implementation:
[src/shared/storage/schema.rs](../../src/shared/storage/schema.rs) (clean
scaffold + version constants), [src/features/data_migration.rs](../../src/features/data_migration.rs)
(JSON orchestration + coordination with SQLite), and
[src/shared/storage/db/mod.rs](../../src/shared/storage/db/mod.rs) (version-aware
`migrate` via `PRAGMA user_version`). Related to [ADR 0005](0005-python-sandbox-wasmer.md)
in style (our own micro-solution instead of a heavy tool).

## Context

The app stores data in JSON (`settings.json`/`profiles.json`/`chats/<id>.json`)
and SQLite (`data.db`). Before this stage, versioning was **decorative**:

- `AppConfig.schema_version` was **never checked**; `main.rs` swallowed a
  corrupt `settings.json` via `unwrap_or_default()`, and the next save would
  **silently overwrite** the data with defaults (overwriting `.bak` too with
  the corrupt version);
- `profiles.json` and `chats/*.json` had no version — a breaking format change
  had no way to be detected; one corrupt chat file crashed `load_chats()`
  **entirely** (blocking startup);
- the change policy only covered **additive** changes ("a new field —
  `#[serde(default)]`, a table — `CREATE IF NOT EXISTS`"); the single breaking
  change in the project's history (nesting the engine config) was resolved
  "without migration" — the user re-entered the data;
- data from a newer app version (a downgrade) was read "as best it could" —
  silent corruption.

Goal: give every artifact a schema version and a **migration scaffold**, so a
binary update never loses data, and a downgrade/corruption is detected
explicitly.

## Decision

### 1. Schema versions — per artifact (F7)

Constants in `schema.rs`: `SETTINGS_SCHEMA` / `PROFILES_SCHEMA` / `CHAT_SCHEMA`
/ `DB_SCHEMA`, all = 1. Per artifact (not a single global number), because
artifacts change at different rates — a shared number would force "migrating"
untouched files. `SETTINGS_SCHEMA` is tied to the existing
`config::SCHEMA_VERSION` (the default of the `AppConfig.schema_version` field)
— the invariant is checked by a test. `DB_SCHEMA` (`PRAGMA user_version`, stage
4) — see §10.

### 2. Version detection — structural (zero churn on existing files)

The file format is **unchanged** today (all schemas = 1); the version is
detected from the shape the file already has:

| Artifact | How the version is detected |
|---|---|
| `settings.json` | the `schema_version` field (missing → 1) |
| `profiles.json` | a bare array → 1; an object → its `schema_version` (an envelope shape will appear at the first breaking change) |
| `chats/<id>.json` | the `v` field (missing → 1) — the field is **not** written while the schema is 1 |
| `data.db` | `PRAGMA user_version` (0 → legacy/fresh DB, baseline stamps 1; see §10) |

### 3. FSD split: a clean scaffold in `shared`, orchestration in `features`

- **`shared/storage/schema.rs`** — Value-level logic only (no I/O): `Step` (a
  pure `fn(Value) -> Result<Value>` "version `< to` → `to`"), `JsonArtifact`
  (`current` + `detect` + `steps` chain), `Assessment` (`UpToDate` /
  `Migrate{from}` / `Downgrade{from}`), the `assess`/`apply_steps` methods, and
  a registry (`settings_artifact` etc.). The single home for version constants
  and the registry — the design doc's "single home" intent is honored.
- **`features/data_migration.rs`** — file I/O, gates (downgrade / corruption),
  the pre-migration backup, and control-parsing into typed structs
  (`AppConfig` / `Vec<Profile>` / `Chat`).

Orchestration is **not in `shared`**, because the pre-migration backup is
`features::backup`, and `shared` cannot depend on `features` (FSD
"dependencies go strictly downward").

**Deviation from the design doc** (there, migration is invoked inside
`Storage::open`): instead, `data_migration::run(paths, loc)` is called from
`main.rs` **before** the storage layer is opened (on both the TUI and the CLI
`import` path). Rationale:

- (a) threading `loc` into `Storage::open` would churn ~30 test sites calling
  `Storage::open(Paths::with_root(...))`;
- (b) migration is a startup-level, application-boundary concern (`main.rs`),
  where `loc` is already available; the scaffold/registry still live in
  `shared/storage/schema.rs`, so the "single home for constants" intent is not
  broken.

### 4. Downgrade guard (F10)

`detect > current` (data from a newer app version) → **refuse to start**, with
a localized message ("this data was created by a newer version of mindfork;
update the app or restore from a backup"). Silent corruption is worse than
refusing to start. The rule applies on CLI paths too.

### 5. Hardening reads of corrupt files (F11)

- a corrupt `settings.json`/`profiles.json` → **refuse to start** (previously —
  silent defaults, followed by overwriting `.bak`);
- a corrupt `chats/<id>.json` → **skip with `tracing::warn`**, the on-disk file
  is left untouched (previously one corrupt file crashed the whole app).
  Fixed directly in `json.rs::load_chats`, so the hardening is durable
  regardless of the migration path.

`read_json`/`write_json` (`json.rs`) became `pub(crate)` — migration reads
files as `serde_json::Value` (distinguishing "no file" from "corrupt") and
writes the migrated value through the same atomic path.

### 6. When migration runs — eager at startup + one pre-migration backup (F9)

`run` builds a plan (files with version `< current`) **and** peeks at the DB's
`user_version` (`db::peek_user_version` + `db::needs_step_migration`, see
§10) — so **one** backup covers both JSON and SQLite. If the plan/DB need
migration → **one** pre-migration backup **before any write**
(`backup::create_backup` named `backups/pre-migrate-<date>.zip` via
`default_backup_path(paths, "pre-migrate")`); if the backup fails, migration
does not start, data stays untouched. The sandbox's `fs_root` is **not**
included in this backup (the config itself may require migration — reading it
just for `fs_root` would be premature; the critical `settings`/`profiles`/
`chats`/`db` are already captured by the backup).

While all schemas = 1, the plan is always empty → the path is dormant, but
covered by a test on a synthetic artifact (`current = 2` with a `1→2` step)
that verifies the backup + write over real I/O.

### 7. Applying: step chain → control-parse → atomic write

`apply_steps` (a chain of pure `fn(Value)->Result<Value>`) → **control-parse**
(`serde_json::from_value` into a typed struct: validating that the migrated
data parses; on failure — refuse, the file is **not** overwritten) → an atomic
write of the migrated `Value` (the `Value` itself, not the re-parsed struct —
preserves the exact migrated shape) via `json::write_json` (temp + rename +
`.bak` of the previous version).

### 8. Bump policy (F12, fixed in AGENTS.md §4)

- **additive** (a new field with `#[serde(default)]`, a new table/column with a
  default) — **no bump**, as before;
- **breaking** (rename/move/semantic change/field removal) — bump the constant
  + a migration step + a golden fixture of the old format + a CHANGELOG entry
  (the "Data" section).

### 9. Localization

5 keys, `migrate.err.{settings_corrupt, profiles_corrupt, downgrade,
backup_failed, control_parse}` (the `ru`+`en` bundles). Migration-progress logs
stay Russian (file logs, not localized).

### 10. SQLite migrations (stage 4)

`data.db` is versioned by `PRAGMA user_version`; `DB_SCHEMA = 1`. The runner —
a version-aware `db/mod.rs::migrate` (order matters):

- **`baseline_ddl` (all the `CREATE … IF NOT EXISTS`) runs EVERY time** — this
  is the mechanism for adding new tables/indexes to an existing DB **without**
  a version bump (the additive policy, F12). Additive DDL and `user_version`
  are **independent**: `user_version` only tracks breaking migrations (steps),
  not additive DDL.
- a fresh/existing DB with `user_version = 0` gets stamped with the baseline
  `DB_SCHEMA` version (`set_user_version`). This is **not** a data migration
  (the DDL is idempotent) → no pre-migration backup, no message. All of
  today's databases (`user_version = 0`) get silently stamped 1 on the first
  run of stage 4.
- breaking steps in the `DB_STEPS` registry (currently **empty** — all schemas
  = 1) are run by `apply_db_steps`: `DbStep { to, summary, apply: fn(&Connection)
  -> Result<()> }`, filtered by `s.to > from.max(1)` (baseline = 1, real steps
  start at 2). **Each step runs in its own transaction TOGETHER WITH the
  `user_version` update**; on error — a full rollback (neither the schema nor
  the version changes). Dormant while `DB_STEPS` is empty.
- downgrade (`user_version > DB_SCHEMA`) — a defensive `bail` (for direct opens
  / tests); the user-facing localized refusal is issued by `data_migration`
  (§6).

**A single pre-migrate moment for JSON+SQLite** (`data_migration::run_with`):
before the storage layer is opened, `db::peek_user_version(data.db)` (0 if
there is no file) → the downgrade guard (`> DB_SCHEMA` → a localized refusal,
for the `data.db` file) → `db::needs_step_migration(uv)` factors into the
decision about **one** shared backup (the JSON plan **or** the DB needing
migration → a backup). The actual DB migration (baseline/steps) is performed
**later** by `Db::open` — here we only peek at `user_version`.

**The SQLite backup API is NOT used** (a deviation from the design doc §3.4,
which proposed `rusqlite::backup`): at the moment of the shared backup, the DB
is **not yet open** (`Db::open` happens afterward), i.e. it is quiescent, and
the `data.db`+`-wal`+`-shm` files are already included in
`backup::create_backup` — the zip gives a consistent snapshot without a
separate backup API. Rationale: quiescent state + the single-instance guard.

## Consequences

- Layers above the startup boundary (orchestrator, UI, tools) — not affected;
  migration is a one-time startup step before the storage layer is opened.
- The scaffold is "in production" on empty migrations: the engine is covered
  by tests on a synthetic artifact, and the first real breaking change will
  only add a step + a golden fixture, without touching the runner.
- A corrupt config/profiles is no longer overwritten with defaults; a corrupt
  chat no longer blocks startup.
- A downgrade is now detected with an explicit refusal instead of silent
  corruption.
- Downside: control-parse duplicates parsing (the value is parsed both for
  validation and again on regular load) — the cost is small (once at startup,
  only for files being migrated).

### 11. Backup manifest (stage 5)

`create_backup` puts a `manifest.json` in the archive (`BackupManifest`: app
version + schema versions `SchemaVersions{settings,profiles,chat,db}` + a
timestamp). On restore it is **not** unpacked into the root (metadata, not
data); `read_manifest` reads it separately, and `mindfork restore` warns
(`is_newer_than_current`) if the copy was made by a newer app version — the
data is intact, and the downgrade guard at startup still protects it. An old
backup without a manifest → `None` (no warning).

**Deferred** (groundwork):

- **vec0 recreation**: the `rag_vectors` virtual table and `meta.rag_dim` are
  not "migrated" via ALTER — on an incompatible change, a future breaking step
  will recreate them via the existing path (`/rag rebuild`).
