# Journal — Storage, config and backup

Everything that reaches the disk: the JSON store (config/profiles/chats), SQLite, schema versioning and migrations, backup/restore, machine-bound secrets, and the per-chat state that rides in the chat file.

**Reference documents for this area:** architecture.md §7, spec.md §5, §12

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (11)

- Post-M9: persisting the input-box draft in the chat file (done)
- Post-M9: persisting deleted exchanges in the chat file (`Ctrl+E`/`Ctrl+R`) (done)
- Post-M9: data storage mode + backup/restore (done)
- Post-M9: remembering the last-open chat (done)
- Post-M9: API keys in settings — stage 1 (core: machine-bound storage) (done)
- Post-M9: database compaction on backup and restore (done)
- Post-M9: password-protected backups (done)
- Post-M9: `backup`/`restore` narrate their work, and give back the keyboard (done)
- Post-M9: the external server's API key, entered in settings (done)
- Post-M9: the first real settings step — `SETTINGS_SCHEMA` 1→2 (done)
- Post-M9: sub-agent chats, PR 3 — `CHAT_SCHEMA` 1→2, a transcript for every old call (done)

### Post-M9: persisting the input-box draft in the chat file (done)
- **Unsaved input-box text is stored on the chat and restored on
  switching**: a new `Chat.draft` field (`entities/chat.rs`, `#[serde(default)]` →
  old chat files load without migration). Empty on a new chat; cleared on send.
  Switching to/creating a chat loads its draft into the input box (a new one →
  empty).
- **Contract**: an `AppCommand::SetDraft(String)` command (UI → orchestrator) and a
  `draft` field in the `AppEvent::ChatActivated` event. The orchestrator is the sole
  writer of `Chat`: `handle_set_draft` writes the active chat's draft and marks it dirty
  (`mark_dirty`, disk write with an 800ms debounce), **without touching `modified_at`**
  (editing a draft shouldn't bump the chat up the list); `handle_send` clears `chat.draft`
  along with appending the user message.
- **UI**: `ChatScreen::mark_input_changed` (called on any input edit — typing,
  pasting, restoring, suggestions) now also raises `draft_dirty`. The `app/runtime.rs`
  loop, every tick, picks up the changed draft (`take_dirty_draft`) and sends
  `SetDraft` (no repaint — the view doesn't change). `activate_chat` loads `draft` into
  the input box via `set_text`, but **deliberately doesn't set `draft_dirty`** (otherwise
  it would immediately send the same text right back via the same `SetDraft`) — it
  triggers a spellcheck recheck directly instead. See spec §11.7.

### Post-M9: persisting deleted exchanges in the chat file (`Ctrl+E`/`Ctrl+R`) (done)
- **Deleting an exchange (`Ctrl+E`) and regenerating (`Ctrl+R`) are irreversible in the
  UI, but what's deleted is saved to disk** for **manual** recovery by editing JSON in
  rare cases. A new `DeletedExchange { deleted_at, messages, draft }` type and a
  `Chat.deleted: Vec<DeletedExchange>` field (`entities/chat.rs`, `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]` → old files load without migration, an
  empty collection doesn't clutter the JSON). This is **not** a message but a
  container: the deleted messages + the input-box draft at the time of deletion + the
  date.
- **What goes in**: `Ctrl+E` (`handle_delete_last`) — the user message and the
  assistant's reply (`split_off(idx)`) + `chat.draft` **before** the user's text is
  restored into the input box; `Ctrl+R` (`handle_regenerate`) — the assistant's reply
  and the round's tool messages (`split_off(idx+1)`) + `chat.draft`. A pure
  `Chat::record_deleted(messages, draft)` method sets `deleted_at = Utc::now()`, ignores
  an empty set, and inserts the entry at the **front** of the collection (recent
  deletions are faster to find), without touching `modified_at` (the truncation
  operations themselves update that). There's no UI restore. See spec §11.7.

### Post-M9: data storage mode + backup/restore (done)
- **Data storage mode via a `location.json` marker** next to the binary
  (`shared/paths.rs`, type `DataLocation`: `portable`/`system`/`path`). By default
  (no marker/empty/`portable`) — data goes into a **`data/` subdirectory next to the binary**
  (`PORTABLE_DATA_SUBDIR`; the subdirectory separates data from build tooling files/caches —
  in dev, `target/debug/data/`, so it doesn't mix with build artifacts). `system` → a standard
  OS folder (the `directories` crate: Windows `%APPDATA%\mindfork-rs\data`, Linux
  `~/.local/share/mindfork-rs`); `path` → an arbitrary directory (no `data/` subfolder —
  the user specified an exact location). `Paths::discover()` reads the marker, resolves the root,
  and **creates it for every mode** (including the portable `data/`); the marker itself
  **always** sits next to the binary (outside `data/`) — it's about the installation, not
  user data. **A corrupted marker JSON is a startup error** (a typo in the
  path shouldn't silently drop you onto an empty dataset). Logs/instance-guard/storage follow
  the root automatically (`discover` → `logging::init`); `build.rs` copies dictionaries into
  `<profile>/data/dictionaries/`. A new accessor `paths.backups_dir()`. **Changing
  the layout**: previous portable data sat right in the binary's directory — after switching,
  it needs to be moved into `data/` once (auto-migration is deliberately not done).
- **Backup/restore** (`features/backup.rs`, a `clap`-based CLI):
  `mindfork backup [-o FILE] [-c 0..9]` and `mindfork restore <archive>` (no TUI,
  they end the process, like `import-lamellama`; they take the single-instance lock — protecting `data.db`
  from a race with a running application). `--import-lamellama` was switched to the same
  `import-lamellama` clap subcommand.
- **Zip contents** (paths relative to the root): `settings.json`, `profiles.json`,
  `data.db` (+ its sidecar `-wal`/`-shm`, if present), `personal_dictionary.txt`,
  the `chats/` and `dictionaries/` directories recursively (pulling in their own `*.bak`), all top-level
  `*.bak` files (`settings.bak`/`profiles.bak` — this project's `.bak` is
  `with_extension`, i.e. `settings.bak`, not `settings.json.bak`), and the
  "sandbox" directory `tools.fs_root` — **only if** its canonicalized path is inside
  the canonicalized root (otherwise skipped). **Excluded**: `backups/`, `logs/`,
  `location.json`. Entries in the archive are deduped by name. Compression level `0..=9` (0 → store,
  else deflate; clap validates the range, default 9). The default name —
  `backups/mindfork-backup-<date>.zip` (chrono Local).
- **Restore is transactional** (`restore_backup` → `RestoreOutcome`): (1)
  **archive validation before any destructive action** (opening the zip + checking
  `enclosed_name` for each entry — anti-zip-slip); an invalid/corrupted archive → `Err` without
  any cleanup. (2) If the root has data (`settings.json`|`profiles.json`|`data.db`|
  a non-empty `chats/`) → an **automatic pre-restore copy** of the prior data into `backups/`. (3)
  Clearing the whitelisted set (the same composition; **preserving**
  `backups/`/`logs/`/`location.json`; the cleanup is by whitelist, not "wipe everything" — stray
  files in the root are untouched) → unpacking the given archive into the root. (4)
  **If the unpacking fails partway through** and a pre-restore copy was made → an **automatic
  rollback**: another cleanup + unpacking the pre-restore copy (`RolledBack`); if the
  rollback itself fails → `Failed`, with the path to the pre-restore copy for manual recovery. `main.rs` reports
  every outcome to the console (stdout isn't occupied by the TUI). An `Err` from restore
  (before any destruction) and `Failed`/`RolledBack` give a nonzero exit
  code.
- **Dependencies**: `zip` (`default-features=false`, only `deflate` — no bzip2/zstd
  C dependencies), `clap` (derive), `directories`.
- **Tests** (`backup.rs`, on a tempdir): archive contents (expected files included, logs/
  backups/marker excluded); `fs_root` is only included when under the root; round-trip
  restore (replacing data + a pre-restore copy + removal of a stale chat); refusal on a
  corrupted archive without touching the data; restore into an empty root without a pre-restore copy;
  store-level (0) produces a valid archive; **rollback on an unpack failure** (a crafted archive with
  a file entry conflicting with a same-named directory surviving the cleanup →
  `RolledBack`). `paths.rs`: default-portable, a whitespace marker, round-trip of the three
  modes, path trimming, an error for an empty path/corrupted JSON, the system path contains the
  application name. **669 tests green** (+18 `#[ignore]`), clippy/fmt clean.

### Post-M9: remembering the last-open chat (done)
- **The app now restores the last-open chat on the next launch.** Previously
  `bootstrap` always activated the most recently modified chat (`chats.first()`
  after sorting by `modified_at`) — switching to an old, unedited chat was
  "forgotten" on restart. Now the active chat is remembered in settings.
- **New field `AppConfig.last_active_chat: Option<Uuid>`**
  (`shared/config.rs`, `#[serde(default)]` via the container's
  `#[serde(default)]` → old `settings.json` files without migration; **not
  editable on the settings screen** — an orchestrator property). Written by
  `Orchestrator::remember_active_chat` (called from `activate`) — **only on an
  actual switch** of the active chat (`activate` is also called to rebuild the
  feed of the same chat during regeneration/exchange deletion — no write
  there); an atomic write of `settings.json`, an error is not escalated
  (memory is a convenience). `bootstrap` activates `last_active_chat` if it's
  still visible, otherwise — the previous fallback (most recent).
- **Settings round-trip protection**: `handle_update_config` replaces the
  entire `self.config` with the UI snapshot, which may carry a stale
  `last_active_chat` (e.g. `None` from startup) — after the replacement the
  actual value is restored (`old.last_active_chat`), so editing settings
  doesn't erase the memory of the chat.
- **Tests**: config (default `None`); orchestrator
  (`remembers_and_restores_last_opened_chat` — a two-phase test: switching to
  the first chat → `settings.json` has `last_active_chat` = the first one; a
  second launch on the same data restores exactly that one, even though the
  second chat was modified later). **734 tests green**, 25 `#[ignore]`,
  clippy/fmt clean.

### Post-M9: API keys in settings — stage 1 (core: machine-bound storage) (done)
- **A new track** (at the user's request): cloud keys (OpenAI/Gemini/Claude)
  should be entered **in the settings window**, not via env variables
  ("ordinary users don't really understand environment variables"), and
  stored in the config **securely** — encrypted with a **machine key**,
  while keeping the config portable: scenario A/B/A (entered on A → moved
  the config to B → re-entered there → back on A → the keys are still
  readable). Research doc
  [docs/research/api-key-storage.md](../../docs/research/api-key-storage.md);
  forks D1–D6 **adopted by the user, per the recommendations, on
  2026-07-20**. Branch `feat/api-key-store` (stacked on
  `docs/api-keys-research`). Stage 1 is **the core** (a UI input field —
  stage 2), so there's no user-visible effect yet.
- **Format — a list of entries, one per machine** (`AppConfig.api_keys:
  Vec<ApiKeyEntry>`; `#[serde(default)]` + `skip_serializing_if` →
  additive, **no migration or schema bump**, per the ADR 0006 F12 policy).
  An entry carries `label` (a PC name + date, human-facing only),
  `scheme`, `check`, and `keys: provider → ciphertext`. **Recognizing
  "our" entry is done by decrypting the `check` probe**, rather than by
  storing a machine id: a machine identifier doesn't need to appear in a
  portable config, and it's not needed for Windows paths either. Foreign
  entries are left untouched (they'll come alive on their own machines);
  an entry with an **unfamiliar scheme** is read, kept, and simply
  treated as foreign — the format is extensible (future work: an OS
  keychain as another `scheme`).
- **Two schemes** (`shared/secrets.rs`, a new module): **`dpapi`**
  (Windows) — the system's `CryptProtectData`/`CryptUnprotectData` (a
  *user*'s key managed by the OS; decryption by another user/on another
  machine is impossible; `pOptionalEntropy` is an app-level constant).
  **`machine-key-v1`** (Linux) — HKDF-SHA256 over `/etc/machine-id` (the
  `sd_id128_get_machine_app_specific` pattern — systemd explicitly warns
  against using the raw machine-id; a fallback to
  `/var/lib/dbus/machine-id`) + ChaCha20-Poly1305 (AEAD, a random nonce as
  a prefix), the username in `info` → per-user binding, like DPAPI. No
  machine-id → the scheme is unavailable, leaving the env fallback in
  place. The ciphertext is encoded as **hex** (a hand-rolled codec — a
  precedent is `sandbox_setup::hex_lower`; a base64 dependency isn't
  needed, and the length difference doesn't matter for a config).
- **The "stored → env" resolution** (fork D3): `resolve_api_key(stored,
  api_key_env)` (`app/supervisor.rs`) — a stored key takes priority
  (entered by an explicit action, the target user never sees env), env
  stays a fallback (CI, power users, systems without machine-id).
  Decryption lives in `EngineManager` (`stored_key(api_keys, provider)`),
  while the supervisor accepts an already-decrypted `stored_key:
  Option<&str>` — **the server-launch layer knows nothing about the
  secret-storage format**, and its tests don't need encryption (a
  deliberate deviation from the design doc, noted there). Keys are
  **provider-centric**: one OpenAI key serves chat + impersonation +
  embeddings (removing the previous "specify the env name three times");
  an external proxy stays env-only (D4 — an arbitrary provider URL can't
  be bound to it).
- **Command `AppCommand::SetApiKey { provider, key }`** →
  `handle_set_api_key` (`orchestrator/settings.rs`): encrypts, places it
  into this machine's entry, persists it (`save_config`, rolled back on
  failure), flags a deferred (re)startup for **only the** slots whose
  active provider's key changed (the same `RestartQueue` debounce used for
  engine settings edits). An empty key means removal; an emptied entry
  gets dropped. The plaintext lives only in the argument and in the HTTP
  client.
- **Round-trip protection**: `handle_update_config` restores `api_keys`
  from the previous config — the UI snapshot never carries them, otherwise
  editing any setting would wipe the keys (precedents —
  `last_active_chat`, MCP TOFU-pin inheritance).
- **i18n**: `ui.err.server.no_api_key_env` → `ui.err.server.no_api_key`
  ("enter a key in settings or set an environment variable"); a new
  `ui.err.api_key_save_failed` (ru+en; the i18n parity/no-dead gates
  covered it automatically).
- **Dependencies**: `chacha20poly1305` + `hkdf` (RustCrypto, pure Rust,
  licenses already in `deny.toml`'s allowlist); `windows-sys` gained the
  `Win32_Security_Cryptography` feature (the crate was already present —
  the sandbox's Job Object). No new C dependencies.
- **The threat model is documented honestly** (§3 of the doc, module doc):
  we protect the **file** — moving/copying/backing up/syncing the config
  (outside its own machine it's a useless ciphertext), and on Windows
  also from other users of the machine. It doesn't protect against code
  running under the same user — it would call the same DPAPI; this is
  fundamental for any scheme where "the app decrypts on its own" (that's
  how Chrome and Git Credential Manager work too). The previous env-based
  path was no safer.
- **Tests**: `secrets` (7 — hex round-trip and rejecting garbage; AEAD
  round-trip; **rejecting a foreign key/a different user/a corrupted
  one**; nonce randomness; `derive_key` determinism and domain
  separation; a full write-then-read cycle on **the live platform
  scheme** — on this machine that's real DPAPI; a foreign entry and an
  entry with an unfamiliar scheme are ignored and **not overwritten**);
  supervisor (2 — priority of a stored key over env and working
  **without** `api_key_env`; a cloud mode reaches `Ready` on a single
  stored key); orchestrator (3 — the key persists as **ciphertext**
  (asserted: "no plaintext appears in `settings.json`") and reads back
  correctly; editing settings doesn't wipe keys; an empty key deletes the
  entry). **1192 unit tests green** (+12), 53 `#[ignore]`, clippy
  `-D warnings`/fmt clean.
- **No live engine run needed** (client protocols untouched — the same
  key goes into the same header; engine/memory unaffected). The DPAPI
  path was actually verified: the full-cycle unit test was run on a live
  Windows machine. The Linux path: the pure scheme core
  (`derive_key`/`encrypt_with_key`/`decrypt_with_key`) is covered by
  tests and run; the platform's ikm source (`/etc/machine-id`) will be
  verified by the `ubuntu` CI job (a Windows host only builds its own
  target).
- **Next — stage 2 (`feat/api-key-ui`)**: a masked `InputBox` mode, an
  "API key" field with a "configured (this computer)" status in the cloud
  subsections, "configured" flags in `AppEvent::Settings`,
  `SettingsIntent::SetApiKey`, README/spec/install.md/CHANGELOG + an
  **ADR** summarizing the track.

### Post-M9: database compaction on backup and restore (done)

- **`data.db` never shrinks on its own.** Deleted notes, `/rag remove`d chunks
  and an attachment index dropped with its chat all leave free pages that SQLite
  keeps in the file. So a backup was archiving the holes as well as the data, and
  a restore laid them back down. Now `mindfork backup` packs a **`VACUUM INTO`
  copy** of the database instead of the live file, and `mindfork restore`
  compacts what it unpacked — the second half matters because an archive made
  before this existed (or by another tool) is fragmented, and it is also what
  folds in a `-wal` an older archive may carry. Branch
  `feat/backup-db-compaction`. A simple task by AGENTS.md §1 (no cross-layer
  contract, no new dependency), so no design doc — but two things had to be
  measured rather than assumed, below.
- **The choice was `VACUUM INTO`, not an in-place `VACUUM` before copying.** The
  source is only read, so a backup cannot damage what it is backing up; the
  result is a single self-contained file, which is why the `-wal`/`-shm` entries
  are dropped from the archive when it succeeds (their content is folded in)
  rather than packed next to a copy they no longer describe. Restore is the one
  place an in-place `VACUUM` is right: the file is already ours, and rewriting it
  is the whole point.
- **Both paths are best effort, and that is the design, not a shortcut.** A
  `data.db` that cannot be read as a database is packed raw (sidecars included)
  and left alone on restore. A backup that *happens* for a corrupt database is
  worth more than a compact one, and the fallback is exactly the pre-change
  behaviour. Failures are logged (`tracing`), not surfaced — the CLI's own output
  is unchanged.
- **Measured hazard #1 — opening a database is not free.** The fallback test
  failed by finding that a stale `data.db-wal` had *disappeared* from the data
  root: SQLite deletes it next to a file it reads as **zero-page**, and that is a
  VFS-level delete which `SQLITE_OPEN_READ_ONLY` does **not** prevent. A separate
  probe showed a read-**write** open removes it for a valid database too (WAL
  recovery + checkpoint on close). A backup silently mutating the data root is
  not acceptable, so a non-database is now refused **by its header before being
  opened at all**, and a real one is opened read-only. Both guards are
  mutation-tested: dropping the header check fails the "empty" case, dropping
  read-only fails the "a real database" case.
- **Measured hazard #2 — `vec0` is addressed by rowid.** `rag_vectors` /
  `attachment_vectors` join their neighbours by `rowid`, so a renumbering would
  leave search returning the *wrong text* — silent, and invisible to any size
  assertion. `VACUUM` preserves `user_version` and explicit `INTEGER PRIMARY KEY`
  rowids, which is what makes this safe; `vacuum_into_preserves_the_vector_index`
  pins it by searching the compacted copy rather than by trusting the
  documentation.
- **Consequence worth knowing**: a genuinely hot WAL cannot be recovered
  read-only, so compaction is skipped and `data.db`+`-wal`+`-shm` are packed
  together — which is the consistent thing to do anyway. In practice the app
  never enables WAL mode, so this is a corner.
- **Tests**: `db/mod.rs::compact_tests` (the vector index survives; the schema
  version survives; a stale scratch destination is overwritten; in-place
  compaction reclaims pages and keeps the data; a non-database is refused; the
  source directory is untouched across placeholder/empty/real inputs) and
  `features/backup.rs` (a compacted database is packed **without** the sidecars
  and is still searchable, with no scratch file left behind; the raw file is
  packed byte-for-byte when it cannot be compacted; restore compacts a
  legacy-style raw archive; restore leaves an unreadable database alone).
  **1657 unit tests green** (+10), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **Verified against the real 17.2 MB dev `data.db`** — no live model needed
  (nothing touches the engine, and the memory *content* paths are unchanged), but
  a synthetic database cannot answer whether a real one with 1441 attachment
  chunks and real `vec0` indexes survives. The real CLI was run on an isolated
  copy: backup → the packed database has **freelist 3 → 0**, an **identical
  content hash** over every table, `user_version` preserved, and the
  `rag_documents`/`attachment_documents` rowid sets **matching** their `vec0`
  shadow tables; then a hand-built "legacy" archive (raw, uncompacted database)
  was restored into a fresh root and came back compacted with the same content
  hash. The size gain there is small (40 KB) precisely because that database is
  barely fragmented — the gain scales with how much has been deleted.

### Post-M9: password-protected backups (done)

- **Asked for directly**: a backup password, settable as a CLI argument *or* in
  the settings, stored machine-bound and encrypted the same way cloud API keys
  are (ADR 0008); restore must take both an archive encrypted with that password
  and an unencrypted one. Plan with forks F1–F8 —
  [docs/history/backup-password.md](../../docs/history/backup-password.md)
  (**user's decision, 2026-08-01**, all as recommended). Branches
  `docs/backup-password` → `feat/backup-password`.
- **Everything load-bearing was measured against a throwaway probe crate before
  any design was committed to**, and two of the results shaped the code directly:
  - **A password handed to an *unencrypted* archive is discarded by the zip
    layer** (`(Some(_), false) => password = None`). So the requirement's own
    wording — "restore either kind" — needs **no detection branch and no mode
    switch**; one code path does both, which is why the diff is small.
  - **The password is verified when an entry is *opened***, not after reading it
    (AES stores a 2-byte verifier in the entry header). That is what keeps the
    transactional restore intact: `validate_archive` already runs before anything
    destructive, so a wrong password becomes a clean refusal rather than a
    rollback. Had verification only happened at EOF, every entry would have had
    to be read up front.
  - Also measured: writing works at all (`with_aes_encryption`), the plaintext is
    absent from the archive bytes, entry **names are not** encrypted, the two
    failure modes are *distinct* errors (`UnsupportedArchive("Password
    required")` vs `InvalidPassword`), the manifest can stay unencrypted inside
    an encrypted archive, and the ciphertext **is authenticated** — 66 of 66
    single-byte corruptions in an entry's payload detected, **0** silently wrong.
    That last one corrected a first, sloppier probe of mine that flipped a byte
    at `len()/3` and reported no error: the byte had landed outside the entry.
- **F1 — WinZip AES-256 inside the zip, not our own container.** The stronger
  option (ChaCha20-Poly1305 + Argon2id, which would also hide the file names and
  give a KDF we control) was rejected for what a backup *is*: an artifact whose
  job is to be recoverable when the application is not available. Standard AES
  keeps it openable by 7-Zip/WinZip by hand; a private format makes the archive
  depend on this program continuing to exist and run.
- **The limits are written down rather than implied** (module doc, settings hint,
  install.md), in the house style of `shared/secrets.rs`: entry names and sizes
  stay visible (content-only encryption); the KDF is fixed by the format at
  PBKDF2-HMAC-SHA1/1000 and is weak against offline brute force of a short
  password — hence the hint asking for a passphrase; and **a machine-bound
  password plus a dead machine means unreadable archives**, which inverts the
  point of a backup, so the hint says to record it elsewhere. This last one is
  sharper than for an API key, where re-entering is merely an inconvenience.
- **F2 — the existing `AppConfig.api_keys` was reused** under a reserved entry
  key, so `put_key`/`stored_key`/`is_ours` work unchanged and one per-machine
  entry keeps holding everything that machine knows. Renaming the field to match
  its widened meaning is exactly what the additive-only rule (ADR 0006 F12)
  forbids, so the doc comment moved instead of the field. **The reserved name is
  `backup-password`, with a hyphen**: the first version used a dot, and the i18n
  gate correctly flagged it as a bundle key that isn't in the bundle — a real
  false positive, better removed at the source than taught to the gate as an
  exception.
- **F3/F4 — the copies the app makes on its own are covered too.** The
  pre-restore copy inherits the run's one effective password (argument, else the
  setting), and the pre-migration backup (ADR 0006) reads the stored password out
  of the raw `settings.json` `Value` — it runs before storage opens, so there is
  no typed `AppConfig` yet. The reasoning is the same in both places: a setting
  that says "my backups are encrypted" must not have an exception that quietly
  writes a plaintext copy of everything.
- **F5 — the manifest stays unencrypted** (it holds only version metadata), so
  `read_manifest` and the "this backup is from a newer version" warning keep
  working with no password, and the archive stays self-describing.
- **F6 — `restore` prompts for the password** when it is missing or wrong, up to
  three attempts, using `crossterm`'s raw mode (already a dependency — no
  `rpassword`). Deliberately **skipped when stdin is not a terminal**: prompting
  in a pipe or a CI job would hang forever instead of failing with a message.
  Restoring a foreign archive on a fresh machine is exactly the case where no
  stored password can apply, and the alternative is `--password` in the shell
  history.
- **UI — a new "Data" section.** None of Model/Sampling/Tools/Memory/Profiles/
  Interface is about the data root, and a security setting filed under an
  unrelated heading is a setting nobody finds; the section also gives the
  data-location and compaction groundwork items a home. The field itself is a
  mirror of the "API key" row (a *status*, never the value; an empty masked
  editor; `Del` clears), which is what the `is_api_key_field` → `is_secret_field`
  rename and the extracted `secret_row` are for.
- **Secrets still never reach the UI**: `emit_settings` sends a
  `backup_password_present: bool` beside `api_keys_present`, and the orchestrator
  restores `api_keys` on the way back — the round-trip protection that already
  existed is what makes the new flag safe. `handle_set_api_key`'s "encrypt →
  persist → roll back on failure" core was extracted as `store_secret` and shared.
- **Dependencies**: `zip` gained `aes-crypto` — exactly the three predicted new
  crates (`pbkdf2`, `sha1`, `constant_time_eq`, plus the `zeroize_derive` proc
  macro), all RustCrypto, still **no C dependency**. `cargo deny check` clean.
- **Tests**: the four-row read matrix; that an encrypted archive carries no
  readable data (asserted on the archive's **bytes** against a level-0 control —
  an API refusal would still pass if the content sat there in the clear); a
  round trip over both kinds of archive with the password held throughout (the
  requirement's own case); a bad password refused **without touching data or even
  writing a pre-restore copy**; the manifest readable without it; an empty
  password meaning plain; the pre-restore copy inheriting the password;
  corruption detected; the CLI parser (both spellings, on both commands, and the
  option before the positional); the help column that had to widen for
  `--password <PASSWORD>`; the settings field; the orchestrator persisting
  ciphertext and emitting the flag; and the precedence rule. **1702 unit tests
  green** (+12), 70 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check`/`cargo deny` clean.
- **Mutation-tested**: dropping the pre-flight password check, writing the
  pre-restore copy without the password, and making `write_zip` ignore the
  password each fail exactly the tests meant to catch them (1, 1 and 5
  respectively).
- **A live model run isn't required** (AGENTS.md §3) — no engine, memory, tool or
  provider path is touched. What stands in for it, as in the database-compaction
  work, is the **real CLI against a copy of the real data root** (311 chats,
  42 MB): a plain and an encrypted backup; **7-Zip reports `Method = AES-256
  Deflate` and `Encrypted = +`** on the data entries and *nothing* on the
  manifest, extracts `settings.json` with the password and refuses a wrong one —
  the interop claim behind F1, verified against a third-party tool rather than
  our own reader; restore refused (exit 1, data intact at 311 chats) with no
  password and with a wrong one; restore succeeded with the right one, and the
  **pre-restore copy came out encrypted**; the plain archive restored while a
  password was held; and with a password seeded into `settings.json` through the
  app's own encryption, `backup` and `restore` used it with **no argument at
  all** (314 encrypted entries).
- **A trap worth re-recording** (the journal already has it, and it bit again):
  `./mindfork-rs restore … | tail` reports **`tail`'s** exit code, so a refusal
  looked like `exit=0` until it was re-run without the pipe. Also, one probe of
  mine was a bad instrument rather than a finding: a stray file at the data root
  survives a restore **by design** (the cleanup is an allowlist), so proving a
  real replacement needs a stray file inside `chats/`.
- **Groundwork**: encrypting the archive's file names would need the outer
  container from F1(b); a stronger KDF is impossible without leaving the zip
  format; `MINDFORK_BACKUP_PASSWORD` (F7) was deliberately not added — trivial
  later, and a third source now would widen "where did this password come from"
  for no current need.

### Post-M9: `backup`/`restore` narrate their work, and give back the keyboard (done)

- **Reported from a real password-protected restore**: after typing the password
  and pressing `Enter` the program printed a newline and then **sat silent for
  ten seconds**, after which all three result lines appeared at once; and the
  `Enter`s pressed during that silence were replayed by `cmd` afterwards as
  three empty prompts. Branch `fix/restore-progress-and-typeahead` (a simple task
  by AGENTS.md §1: one feature module and the CLI, no cross-layer contract, no
  new dependency — no design doc).
- **Two defects with one symptom, and the second one made the first worse.**
  Every message was written *after* the work (`cli.restore.pre_saved`,
  `cli.restore.cleared`), so the whole restore ran mute; and a password prompt
  echoes nothing, so the one moment the user most needs a sign of life is the
  one where the program looked dead. Pressing `Enter` is then the natural thing
  to try — and those keystrokes sat in the console input buffer with nobody
  reading them, so on exit the shell inherited and replayed them.
- **Progress is a callback of already-localized lines**, the shape
  `sandbox_setup::setup` already uses: `features` has no TUI, so the CLI decides
  where the lines go (`println!`) and `data_migration` — which runs at startup,
  before the TUI — passes `|_| {}`. Each phase announces itself **before** it
  runs (checking → pre-restore copy → compacting → packing → clearing →
  unpacking → compacting), which is what makes it feedback rather than a log.
- **The entry loops count themselves out** (`EntryProgress`, `N of M`, at most
  once per 500 ms, counted from the loop's start so a small data root finishes
  in silence). Deliberately **not** a byte counter: the measured shape of a real
  root is ~300 small chat files plus one 26 MB `data.db`, so entry counting
  answers "is it moving?" for the bulk of the time, while an honest byte counter
  would have to reach inside the copy of a single file — recorded as groundwork
  rather than done.
- **The keystrokes are discarded on the way out**
  (`features/terminal_input.rs::discard_type_ahead`, the module renamed from
  `password_prompt.rs` — it is now terminal input for the CLI generally). Safe
  because there is no other consumer: the CLI has finished reading by then, and
  the keys were typed at us. Gated on stdin being a terminal, so a pipe or a CI
  job keeps its input; called on **both** commands and on every outcome
  (including a rollback), since the shell inherits the terminal either way.
- **One message survived the rewrite for a reason**: the path of the pre-restore
  copy is still printed at the end (`cli.restore.pre_saved`), because that is the
  path you undo a restore with and by then the progress lines have scrolled past.
  The phase label above it therefore names no path — it is announced before the
  copy exists. `cli.restore.cleared` became a progress line and its key was
  deleted (the i18n gate fails on a dead key).
- **Tests**: the phase order on a real restore (the contract is that each label
  precedes its step), a pre-restore copy **not** announced when the root is
  empty, the rollback announcing itself, `backup` announcing compaction before
  packing, and the ticker staying quiet under its interval / counting `N of M`
  with no unsubstituted placeholder in either language. **Mutation-tested**:
  dropping the clearing label, dropping the rollback label, or ignoring the
  throttle each fails exactly its own test. **1798 unit tests green** (+6), 75
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **No live model run is required** (AGENTS.md §3) — no engine, memory or tool
  path is touched. What stands in for it, as in the compaction and password work,
  is the **real CLI against a copy of the real data root** (321 chats, 26 MB
  database, 58 MB): `backup --password` narrated 331 entries and finished in
  15.7 s, `restore` narrated every phase of its 21.4 s and left the data intact
  (321 chats, the database restored), and a restore with no password still
  refuses before touching anything. The **type-ahead half cannot be tested from
  here** — it needs a real console, and `IsTerminal` is false in a piped
  harness — so it is left for the user's interactive check, the same boundary
  the MCP command resolver's wiring sits behind.


### Post-M9: the external server's API key, entered in settings (done)
- **What.** The `external` mode — connect to any already-running
  OpenAI-compatible server — could only take its Bearer key from an **environment
  variable named in settings**. It now takes the key itself, in the same field the
  cloud modes have, with the same machine-bound encryption
  (`AppConfig::api_keys`), for all four slots that have an `external` sub-section:
  assistant chat, impersonation, embeddings, speech. Plan:
  [docs/external-api-key.md](../history/external-api-key.md); this closes the groundwork
  item [ADR 0008](../decisions/0008-api-key-storage.md) wrote for itself.
- **Why the original decision no longer holds.** ADR 0008 left external env-only on
  one argument (research §F4, R4): *an arbitrary URL cannot be pinned to a
  provider*, plus "external is a power-user feature env already covers". The first
  half is about **addressing**, not about the user — and the MCP reuse
  ([mcp-server-editor.md](../history/mcp-server-editor.md) §9) had already answered
  it by addressing a secret as `(server, variable)`. The second half stopped being
  true: `external` is the mode for OpenRouter, LiteLLM, vLLM and every self-hosted
  gateway, and those require a key. So the settings screen was offering exactly the
  env barrier ADR 0008 existed to remove, in the one mode whose URL is typed by
  hand. A recorded rationale is a claim with a date on it (lessons.md §3).
- **The address is the slot, not the provider** — `SecretKey::External(ExternalSlot)`
  → `external-chat` / `external-impersonation` / `external-embed` / `external-tts`.
  This deliberately inverts ADR 0008 §3's provider-centric rule, and the reason is
  the shape of the data: one provider is one account, whereas the four `external`
  sub-sections hold four **independent URLs**. The ordinary configuration is a cloud
  gateway for chat beside a local `llama-server` for embeddings, so one shared
  "external key" would send the gateway's Bearer token to localhost and back. Fork
  F1, user's decision 2026-08-17. A hyphen, not a dot, in the storage name — a
  dotted literal reads as an i18n bundle key to the `cyrillic_scan.py` sibling gate
  (lessons.md §7), the same trap `backup-password` hit.
- **Reading the code shrank the task to plumbing** (lessons.md §3, again). Nothing
  new was needed in `shared/secrets.rs` (`put_key`/`stored_key` take a name),
  nothing in the `ServerSupervisor` trait (every method already takes one
  already-decrypted `stored_key: Option<&str>` — the orchestrator decides *which*
  secret that is), and **no new `FieldId`**: `XApiKey`/`IxApiKey`/`EApiKey`/
  `TtsApiKey` already exist with the masked editor, the empty seed, `Del`-deletes
  and the `SetSecret` intent behind them. What changed is which secret those ids
  resolve to.
- **One source for "which secret does this slot read".** Both `engines.rs` and the
  settings screen's `secret_field_key` used to spell out "mode → provider"
  independently; the mapping is now one contract — `trait SecretSlot`, whose
  `secret_key()` default method holds the whole decision while each of the four
  sections answers only "which provider" and "which slot, if external" — and both
  ask it. That is the load-bearing part rather than the line count: a row that
  addresses a different secret than the server resolves is invisible from outside —
  the supervisor would simply be handed the wrong string — which is why
  `MockSupervisor` grew a `chat_keys()` recorder so a test can see it. The trait
  arrived by way of the duplication gate, below; the *first* shape was an inherent
  `secret_key()` per struct, which is the same contract stated four times.
- **Resolution is unchanged, and that is the point** (fork F2): `resolve_api_key(stored,
  env)` was already "stored wins, env is the fallback", so the external paths went from
  passing `None` as the stored key to passing the real one. The external row has the
  cloud row's shape — a key plus an optional variable *name* — unlike MCP's `env` row,
  where the variable is *declared* and naming a source is an explicit instruction a
  stored value must not override. Choosing MCP's rule here would only have made the two
  engine paths disagree with each other.
- **No key stays legitimate** (fork F3). The cloud paths report `Disconnected` when a
  key cannot be resolved; the external paths keep `.ok()`, because a local
  `llama-server` needs none and a request with no key must stay byte-for-byte what
  the app sent before this feature. The unit test asserts that on the wire — **no
  `Authorization` header at all**, not merely an empty one.
- **Side effects are per slot** (`handle_set_secret`): `Chat`/`Impersonation`/`Embed`
  mark exactly their own server for the debounced re-raise, `Tts` marks nothing (the
  speech engine is built per utterance, like the backup password). No mode check is
  needed, unlike a provider key, which several slots may or may not be pointing at.
- **UI**: the `external` field list gains the key row above the existing env-name
  row, which stays (fork F4 — CI, scripts, and a machine where key storage is
  unavailable). Two bundle keys in both locales; the description has to close the
  door (lessons.md §4) — two adjacent fields that look like alternatives must say
  which one decides, so it states that a stored key is used *instead of* the
  variable named below, that the key is optional, and that each external server has
  its own.
- **Tests**: **2290 unit green** (+8), 98 `#[ignore]` (+1), clippy `-D warnings`/
  fmt/`cyrillic_scan`/`link_check`/`doc_index_check` clean. The wire-level trio
  (stored key sent, env fallback read, neither → no header) runs against a
  one-request TCP stub reading **to the end of the headers** rather than a fixed
  buffer — with a 2 KiB buffer the `PATH` value under comparison came back
  truncated, which looked like a resolution bug. Two things were **found by a test
  rather than by review**, both now in [lessons.md](../lessons.md):
  - the "a speech key must not restart the chat server" assertion first passed
    *with* `Tts => mark_chat()` applied, because `Quit` was handled before the
    debounce expired — the wrong restart was queued and simply never flushed. It
    only bites once the speech key is followed by a slot that *does* restart
    something and the count is read against **that** flush, the device
    `an_edit_and_its_undo_cost_no_restart` already used;
  - the `ru` hint was **clipped mid-sentence at 70 columns** — the panel caps at
    `HINT_MAX_ROWS`, and the external hint is the longer of the two key hints
    because it has to name the two fields' priority as well. A hint that closes the
    door only works if its last sentence is on screen, so both locales were trimmed
    to the load-bearing three claims (optional / nothing sent without a key /
    stored beats the variable below) and the existing "shown whole" gate now runs
    over **both** key rows. The identical trap is on record from the cloud hint
    (docs/history/settings-undo.md era) — the panel grew to fit the longest hint
    *then*; what is new is that a cap exists and a hint can still exceed it.
- **The duplication gate said no, and it was right** — 3.8% new-code duplication
  against a bar of ≤3% (PR #331's first run), a **sixth** recorded instance of the
  shape in [lessons.md](../lessons.md) §2 and the same *mechanism* as the help-table
  one: not a copied block, but **new lines inserted inside a range that was already
  flagged**. `config.rs` carries pre-existing triplicates — `cloud()`/`cloud_mut()`
  across the three engine sections, `active_model_name()` across two — so an
  inherent `secret_key()` added to those impl blocks landed *inside* them, and 18 of
  its lines counted as duplicated however little they resembled anything. The fix is
  the recorded one: structure it **from outside**. `trait SecretSlot` holds the
  decision once, outside every flagged impl, and each section contributes two
  one-liners; four adjacent 8-line impls cannot chain into a 10-line match because
  what differs between them is *identifiers* (type, mode enum, slot), which the
  detector does not normalize away — unlike the literals that made the earlier cases
  invisible. A **macro** would have been the smaller single-source answer and was
  rejected: there is not one `macro_rules!` in the codebase, and a first one for
  eight lines of boilerplate is a style break, not a simplification. The test side
  had the classic version of the same trap — the "type into a masked editor, expect a
  `SetSecret`" opening, now on its fourth copy in the settings tests — and got the
  recorded fix too: one `enter_secret` helper, which is where the "editor opens empty
  and masked" assertion belongs anyway, since every secret field owes it.
- **Live — GO, twice over.** The standard regression scope first: the orchestrator
  e2e set against the usual stack (gemma-4-31B + bge-m3 over
  `MINDFORK_ENGINE_URL`/`MINDFORK_EMBED_URL`) — **34 passed, 0 failed, 771 s**, one
  deliberate skip (`tts_speaks_chat_e2e_live`, no cloud TTS key set). Then the
  feature itself, which that set **cannot** reach: a local `llama-server --api-key sk-live-probe-42`
  (`gemma-3-4b-it-q8_0`, CPU) verified the whole chain through `LlamaSupervisor`:
  with the key stored the turn answered `"OK."`, and the **control arm** with no key
  anywhere got a real `401 Invalid API Key`. The control is what makes it mean
  anything — against a server that does not enforce a key, the first arm would pass
  regardless. Kept as `external_authenticated_server_takes_the_stored_key_live`
  (`MINDFORK_ENGINE_URL` + `MINDFORK_ENGINE_KEY`), and it *fails* rather than
  silently passes if the server turns out not to enforce the key. Note what this
  smoke could not have been: the orchestrator's live e2e set builds its backend
  through `MockSupervisor` (lessons.md §9), so it never runs `external_chat_setup`
  at all — the supervisor had to be driven directly.

### Post-M9: the first real settings step — `SETTINGS_SCHEMA` 1→2 (done)
- **What**: the dormant migration scaffold (ADR 0006) ran its first real step.
  The sub-agent track ([docs/research/subagent-chats.md](../research/subagent-chats.md)
  §3.12, PR 2) replaced `tools.subagent_timeout_secs` (one request, 60 s) with
  `tools.subagent_run_timeout_secs` (a whole run, 600 s) and lifted the
  default of `subagent_max_tokens` from 1024 to 4096. A rename is breaking by
  F12, so: `SETTINGS_SCHEMA = 2`, `config::SCHEMA_VERSION = 2` (the invariant
  test ties them), and `schema::settings_to_v2` — a pure `Value → Value` step.
- **The step's rule**: a value left at the old default is **dropped** so the
  new default applies on read (`ToolSettings` is `#[serde(default)]`); a value
  the user changed is carried over under the new name, because a number
  somebody typed is a decision; a `subagent_max_tokens` left at 1024 is dropped
  for the same reason; nothing else in the file is touched. The old defaults
  are **frozen in the step as literals**, not read from `config.rs` — a step
  describes the past and must not follow the constants when those move again.
- **What the bump touched besides the step**: `real_registry_is_all_current_v1`
  became `real_registry_versions_and_steps` (settings at 2 with one step, the
  other two dormant); `Step.summary` lost its `allow(dead_code)` — the runner
  now logs it; two tests whose premise was "every file is v1" had to say which
  version they meant (`data_migration`'s synthetic-step test pins its fixture
  at 1, `main.rs`'s "enable_python is a no-op" fixture at the current version —
  the rewrite it guards against is the *needless* one, and a migration is not
  that). Three golden-shape tests pin the step: old defaults dropped and the
  migrated value parsing into today's defaults, changed values carried, a file
  without a `tools` section.
- **Not done here, deliberately**: the chat-file step that synthesizes a
  sub-agent transcript for old `call_subagent` records (`CHAT_SCHEMA` 2) is the
  track's PR 3 — a separate bump, a separate golden fixture, its own journal
  entry.

### Post-M9: sub-agent chats, PR 3 — `CHAT_SCHEMA` 1→2, a transcript for every old call (done)

- **What**: PR 3 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §3.13, F10 —
  the user's decision to migrate rather than leave old calls as they were). The
  first chat-file step, `chat_steps::chat_to_v2`: every `call_subagent` record
  without a run — in `messages` **and** in `deleted[].messages` — gets one
  synthesized from what it already holds: `system_message`/`message` (and a
  `name`, if a post-PR-2 record somehow lacks a run) from `arguments`, the
  reply from `result`, `created_at` from the assistant message, `finished_at`
  from the `Tool` message that answered the call (else the call's own time), a
  title by the same rule the live run uses (`shared::title::sanitize_title` —
  the reason PR 1 moved it), `outcome: completed` when there is a result and
  **no outcome** when there is none (a call that never came back reads like an
  interrupted run today). Then `v = 2`.
- **Determinism and idempotence**: run ids are `Uuid::new_v5(SUBAGENT_NS,
  "<chat id>/<call id>")`, message ids derive from the run id — a restored
  backup migrates to the same `chat://` addresses, and a record that already
  has a run is skipped, so running the step twice is a no-op (pinned).
- **`Chat.v`**: the file now carries its schema version on every save
  (`#[serde(default = 1)]`, set to `CHAT_SCHEMA` by `from_profile` and the
  importer, kept as loaded otherwise — it states what the content conforms
  to, not which binary touched it). Without it a migrated file re-saved by the
  app would detect as 1 on the next start and be handed to the step again
  (harmlessly, but forever). `detect_chat`'s "not written while 1" note is
  history now.
- **Shape decisions**: the step lives in its own module, `chat_steps.rs`, with
  the golden fixture beside it (`storage/fixtures/chat_v1_call_subagent.json`
  — a v1 file as a Russian-locale install wrote it: a live call with its
  `Tool` answer, an archived call with none, an ordinary `current_time` call
  that must be left alone); the fixture is allowlisted in
  `tools/cyrillic_scan.py` because its Cyrillic *is* its content. The
  `call_subagent` name is a literal in the step: a step describes the past,
  and `shared` cannot reach `features` anyway.
- **Tests**: 2468 green (+8): the fixture parses as v1 today; the synthesized
  run's every field, the untouched neighbour record, the rest of the file
  intact; the deleted archive; control-parse into `Chat` and `final_reply`;
  determinism + idempotence; a run already present / arguments missing; and
  end to end through the real registry in `data_migration` — one pre-migrate
  backup, `v = 2` on disk, the second start finds the file current — plus "a
  freshly saved chat is current". No live run: a pure storage step, covered by
  the fixture.
- **Not done here**: the list, the read-only view and search over the
  synthesized runs — PRs 4–6; until then an old call's transcript is on the
  record and nowhere on screen but the card's result text.
