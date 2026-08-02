# MCP servers in the settings window — design plan

**Status:** the track is **complete** — stage 1 done and accepted 2026-08-02,
stage 2 (secrets + import, §9, forks S1–S8 accepted the same day) implemented and
live-run 2026-08-02. The file stayed in `docs/history/` throughout: every
reference to it (roadmap, ADR 0007, CLAUDE.md) already points here, and moving it
back and forth would churn those links for no gain.

Stage 1's acceptance: a real server was configured end to end from the settings
window with a platform-independent command, and the model called its tool. Forks
accepted — **F1–F8 confirmed by the user 2026-08-02,
all as recommended** (F1(b) a dedicated section, F2(a) a selector, F3(a)
space-separated with shell quoting, F4(a) the env map stays variable names,
F5(a) a new server starts disabled, F6 deferred, F7(a) reconnect, F8(a)
in-editor validation); **scope — stage 1 only**, stage 2 (secrets + JSON
import) stays a separate decision. Closes the groundwork
item "server editor UI (currently — edit `settings.json` by hand)"
([roadmap](../roadmap.md) §Tools, [ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md)
§Consequences). Behaviour — spec §9.6; the host itself is done and unchanged
by this track.

## 1. What exists today, and what the gap actually is

The MCP host is complete: servers spawn from `config.mcp`, tools become
`McpTool` wrappers in the registry, statuses and full tool descriptions are
shown in settings, TOFU pinning guards the catalog. The **only** missing piece
is authoring: `config.mcp.servers` is edited by hand in `settings.json`, and
decision point R6 (ADR 0007) made that deliberate at the time — the host was
new and a hand-edited file was the smaller surface.

Reading the code first changed how big this looks:

- **The orchestrator side is already done.** The settings screen owns a working
  `AppConfig` and emits `SettingsIntent::SaveConfig(config)` carrying the
  **whole** config; `handle_update_config` diffs `config.mcp`, calls
  `restarts.mark_mcp()`, and the 1.2 s debounce flushes into
  `apply_mcp_settings()` — killing and respawning servers. So editing
  `config.mcp.servers` from the screen needs **no new `AppCommand`, no new
  `AppEvent`, and no orchestrator change** for the edits themselves. This is
  exactly the impersonation-persona shape (`config.impersonation_profiles`,
  spec §11.8), which is the precedent this plan follows.
- **Undo comes free.** `Ctrl+Z` on the settings screen restores a whole older
  `AppConfig` snapshot, so undoing a field edit — or a server create/delete —
  works with no extra code (docs/history/settings-undo.md).
- **No-op restarts are already suppressed.** `McpManager::is_current` compares
  the settings against what the servers were actually launched with, so an edit
  plus its `Ctrl+Z` costs no `npx`/node respawn.
- **A hole this UI should close, found while reading.** A server that exhausts
  its restart budget (3 crashes / 5 min) sits `Disconnected` "until manual
  intervention (editing settings recreates the slot)" — but since `is_current`
  landed, toggling a field off and on again inside the debounce window produces
  an *identical* final config and therefore **no re-apply at all**. There is
  currently no reliable way to retry a dead server short of restarting the app.
  See F7.

So the work is ~90 % settings-screen, and the risky parts are the two fields
that are not scalars (`args: Vec<String>`, `env: BTreeMap<String,String>`) plus
the security question of where an MCP server's secrets live.

## 2. The data being edited

```rust
McpServerConfig {
    id: String,                      // slug [a-z0-9-] ≤32 — part of mcp__<id>__<tool>
    command: String,                 // resolved as a shell would (PATHEXT), see §8
    args: Vec<String>,
    env: BTreeMap<String, String>,   // child var → NAME of a source OS env var (R8)
    enabled: bool,
    tool_timeout_secs: u64,
    max_result_chars: usize,
    pinned_catalog: Option<String>,  // written by the app, never by the UI
}
```

## 3. Forks

### F1 — where the editor lives

- **(a)** Stay in "Tools" → the existing "Plugins (MCP)" group: master toggle,
  a server selector, the selected server's fields, then the read-only status
  rows. Cheapest, and where a user already found the master toggle.
- **(b) [recommended]** A dedicated **"Plugins"** section in the left menu.
  Precedent: "Data" earned its own section because none of the others is about
  stored data — the same argument holds here. "Tools" is a list of *gates for
  built-in tools* (one toggle each); MCP is an *inventory of external programs*
  with per-server settings, statuses and lifecycle. It also gives the remaining
  MCP groundwork (HTTP transport, resources/prompts, per-server tool caps) a
  place to land instead of further crowding "Tools" (already 6 groups, ~20
  rows). Cost: the menu goes 7 → 8 sections; discoverability is covered by `/`
  search and by the section counter.
- **(c)** A separate full-screen editor (the `F3` self-model shape). Rejected:
  a new screen, a new intent family and a new event, for data that fits the
  existing row model.

### F2 — selecting a server

- **(a) [recommended]** A `Choice` selector row (the `IpSelect` shape: `←/→`
  cycles, `Enter` opens the option-list popup) + the selected server's fields
  below it. The read-only status rows stay as their own group listing *all*
  servers.
- **(b)** Select by standing on a status row. Rejected: `Enter` on a status row
  already means "confirm the changed catalog", and one key with two meanings on
  the same row is what the settings-navigation track just finished removing.

### F3 — editing `args: Vec<String>`

- **(a) [recommended]** One text row, **space-separated with shell-style
  quoting**: `-y "@modelcontextprotocol/server-filesystem" "D:/my work"`. It is
  how a person types a command line, it round-trips (quote on join when an arg
  contains a space or a quote), and the parser is one small pure function.
  Precedent for list-as-text: `samplers` (`;`) and `dry_sequence_breakers`
  (`,` + escapes).
- **(b)** A rarer separator (`;`) with no quoting rules. Unnatural for a command
  line, and silently wrong for any arg containing the separator.
- **(c)** A JSON array (`["-y","@mcp/server","D:/work"]`). Exactly what users
  copy out of `claude_desktop_config.json` — but that argues for the JSON
  import (F6), not for making every hand edit look like this.
- **(d)** Indexed rows `Arg 1`, `Arg 2`, … with their own create/delete keys.
  Most "correct", but `Ctrl+N`/`Ctrl+D` are the server create/delete in this
  section, so it would need a second convention for a nested list.

### F4 — editing `env` (and where MCP secrets live)

Today the map is child variable → **name of a source OS env variable**, so no
secret is ever written to `settings.json` (R8, the `api_key_env` precedent).
That is a good property and a real usability wall at the same time: to use a
hosted server (GitHub, Slack, …) the user must set an OS env var and restart
the app — which is the opposite of "configure it in the window".

- **(a) [recommended for stage 1]** Keep R8 exactly as is; edit the map as one
  text row: `GITHUB_TOKEN=MINDFORK_GITHUB_PAT, OTHER=SRC`. No security change,
  no new contract.
- **(b) [recommended as stage 2]** Allow a **stored secret value** as well: the
  value is entered in a masked field, encrypted machine-bound and kept in
  `config.api_keys` — which needs **no storage change at all**, since
  `secrets::put_key`/`store_secret` already take an arbitrary key name (the
  backup password proved it, `BACKUP_PASSWORD_KEY`). It needs: a key naming
  scheme (`mcp-<server>-<VAR>`), a generalized set-secret command (today
  `SetApiKey`/`SetBackupPassword` are two variants of the same thing), and a
  resolution branch in `McpManager::resolve_env`. Already on the roadmap as
  groundwork ("keys for the external proxy and MCP-server env maps via the same
  mechanism"). ADR 0008's threat model applies unchanged, and ADR 0007's R8
  would be amended rather than broken: still no plaintext on disk.
- **(c)** Leave `env` hand-edited in `settings.json` for now. Honest, but it
  means the feature does not cover the servers people most want.

### F5 — a newly created server starts disabled?

- **(a) [recommended]** `Ctrl+N` creates a server with `enabled: false`, a
  generated unique id (`server-1`) and an empty command; the user fills the
  fields in and flips the toggle. Nothing is spawned while the config is
  half-typed, and flipping `enabled` becomes the deliberate "start it" moment
  that a per-field debounce cannot express.
- **(b)** Created enabled (matching `McpServerConfig::default()`), relying on
  the debounce and on the status row to report the failure.

### F6 — importing the ecosystem's JSON

Every MCP client shares one config shape (`{"mcpServers": {"fs": {"command":
…, "args": […], "env": {…}}}}`), and users have one already. A "paste JSON" row
(a multiline editor with validation-without-closing, which the screen already
supports via `Editor.error`) would add a server without typing `args` at all,
and makes F3 far less load-bearing.

The catch is `env`: in the ecosystem's format those values are **literal
secrets**, and ours are variable *names* — an import must not silently write a
token into `settings.json`.

- **(a) [recommended]** Import in **stage 2, together with F4(b)**: once a
  secret has somewhere safe to live, an imported `env` value can be stored
  encrypted and the import is lossless.
- **(b)** Import in stage 1 with `env` **dropped and reported** ("2 environment
  variables were not imported — set them in …"). Useful earlier, but the note
  is the kind of dead end this project has had to fix twice (the by-reference
  attachment block, the `youtube_watch` unconfigured path).
- **(c)** No import.

### F7 — a per-server "Reconnect" action

- **(a) [recommended]** `Enter` on a server's status row does what that row
  needs: confirm the catalog when one is pending (today's meaning), otherwise
  **reconnect** — re-apply that server's slot. This closes the hole in §1
  (a server past its restart budget, or one whose external dependency was fixed,
  currently cannot be retried without restarting the app). It is an *action*,
  not a config edit, so it needs a small intent/command pair mirroring
  `ConfirmMcpCatalog`.
- **(b)** Leave it; the user restarts the app.

### F8 — validation

`validate_server_config` already rejects an invalid slug id, an empty command
and `.bat`/`.cmd`, surfacing the reason in the status row. One detail matters:
with an **invalid id** the manager creates no slot at all (just a log warn), so
today a typo'd id makes the server vanish from the UI. Under this plan the
selector is built from `config.mcp.servers` rather than from the manager
snapshot, so it stays visible and editable either way.

- **(a) [recommended]** Additionally validate in the editor (refuse to commit,
  showing the reason inline — stage-5 machinery of the settings redesign) for
  the three rules the screen can check for free: slug shape, **uniqueness of
  the id** among servers, and the batch-command ban. The status row stays the
  backstop for everything else.
- **(b)** Commit anything, let the status row explain.

## 4. Proposed staging

**Stage 1 — the editor** (`feat/mcp-server-editor`): F1(b), F2(a), F3(a),
F4(a), F5(a), F7(a), F8(a). Literally closes the roadmap item. **Adopted.**

**Stage 2 — secrets and import** (`feat/mcp-env-secrets`): F4(b) + F6(a).
Independently valuable, and it is what makes hosted MCP servers configurable
without touching the OS environment. **Not adopted yet** — a separate decision
once stage 1 has been used against a real server; until then `env` stays a map
of OS variable names and stays hand-editable in `settings.json` too.

## 5. Implementation notes (stage 1)

- New `FieldId`s: `McpServerSelect`, `McpId`, `McpCommand`, `McpArgs`,
  `McpEnv`, `McpServerEnabled`, `McpTimeout`, `McpMaxResult` (+ the existing
  `TMcpEnabled`, `TMcpServer(idx)` move to the new section).
- These are **indexed user data**, not config scalars with a default, so they
  do not go through the `field_spec` access table (which takes
  `fn(&mut AppConfig)` with no index) — they get their own branches in
  `apply_text`/`toggle_field`/`cycle_field`, exactly like `IpName`/`IpSystem`/
  `PTool(idx)`, and they join `is_profile_field` so `Del`-reset and the `•`
  "differs from default" marker correctly skip them.
- `SettingsScreen` gains `mcp_server_idx` plus a `clamp_mcp_server_idx` called
  from `refresh` (the list can shrink under an undo) — the
  `clamp_imp_profile_idx` shape.
- `refresh` replaces the working config wholesale, and the orchestrator
  re-emits `Settings` on **every** MCP event (statuses are live). That is safe —
  an open `InputBox` editor holds its own text — but the selection index must
  survive it, hence the clamp above.
- The TOFU pin is inherited by server **id** in `handle_update_config`, so
  renaming an id drops its pin and the server re-pins silently on next start.
  Acceptable (a rename changes every tool name anyway) but worth a doc line.
  Changing `command`/`args` deliberately **keeps** the pin: pointing a server at
  a different program is precisely when the catalog check should fire.
- New labels must fit `LABEL_CAP = 28` — pinned by
  `all_labels_fit_alignment_cap`.
- i18n: new `ui.settings.*` keys in **both** bundles (the parity / no-dead-key /
  no-Cyrillic gates cover them automatically).
- Rejected alternative: "open `settings.json` in `$EDITOR`". Poor fit — portable
  data root, no editor assumption, and the app would have to detect the
  external write and reload.

## 6. Testing

Unit (settings screen, `TestBackend` + intent assertions): create → edit →
delete round trip; a created server is disabled and selected; `args` quoting
round-trips (including an arg with a space and one with a quote); `env` map
round-trips; an invalid/duplicate id is refused in the editor with the reason
shown; `Del`/`•` skip these fields; `Enter` on a status row confirms when
pending and reconnects otherwise; `Ctrl+Z` restores a deleted server.
Orchestrator: an edit marks `mcp` and the debounce applies it once; an edit and
its undo cost no restart (the existing `is_current` test shape).

**A live run is required** (AGENTS.md §3): this touches the tool path — the
existing `mcp_filesystem_e2e_live` smoke plus a manual run configuring a real
`npx @modelcontextprotocol/server-filesystem` **entirely from the settings
window**, which is the actual acceptance criterion for this track.

## 7. Documentation to update on completion

CLAUDE.md journal; CHANGELOG (`Added`); spec §9.6 (servers are configured in
the UI, `settings.json` remains the equivalent path); README (settings);
docs/install.md §4.2 (the hand-edited JSON becomes the alternative, not the
only way); docs/roadmap.md (the groundwork item closes); ADR 0007 gets a note
that R6 was revisited — and, if stage 2 lands, that R8 is extended by ADR
0008's storage.

## 8. Follow-up from the first manual run (2026-08-02)

The first real run in a terminal turned up two things the design had not
covered. Both were fixed on the same branch.

**A platform-dependent config.** Writing `cmd /c npx …` on Windows is the single
biggest papercut in authoring a server, and it makes a config unportable. A
spike measured why, rather than assuming: `cmd.exe` completes a bare name from
`PATHEXT` and **Rust does not** — `Command::new("npx")` is `NotFound` while
`Command::new("npx.cmd")` spawns fine. So `shared::mcp::resolve_command` now does
that completion, and one config works everywhere.

The same spike overturned the premise of the `.bat`/`.cmd` ban (ADR 0007 §2):
CVE-2024-24576 is fixed in `std` as of Rust 1.77.2 — measured, `a"b`, `%CD%` and
`a&whoami` are escaped and an embedded newline is **refused** with
`InvalidInput`. The ban therefore added nothing over `std` while pushing users
onto `cmd /c`, where the arguments are re-parsed by `cmd.exe` outside that
escaping. It was removed, with the reasoning recorded in the ADR.

**The live run then caught a bug in the resolver itself** — which is exactly what
a live run is for. npm ships an extensionless `npx` (a Unix shell script) *next
to* `npx.cmd`, and the first implementation preferred the exact name, so it
spawned the script: `os error 193: not a valid Win32 application`. `cmd.exe` only
ever completes a bare name; a name that already carries an extension is tried as
written and then still completed (so `my.tool` can reach `my.tool.exe`). The
rule's core is now a pure function taking the `PATH`/`PATHEXT` as parameters, so
a test can build the npm layout in a temp directory instead of mutating the
process environment. The *wiring* (that `spawn` calls the resolver) is covered by
the live smoke only: a unit test would need either a global `PATH` mutation —
racy under a parallel suite — or would sit through the 30 s handshake timeout.

**A server can be "ready" and invisible.** The reported symptom was a server
showing `ready · tools: 1` while the assistant said it had no such tool. That is
the double opt-in (R7) working as designed — MCP tools are enabled per profile —
but the row said nothing about it, which is the same defect class this project
has fixed twice before (the by-reference attachment block, the `youtube_watch`
unconfigured path): the UI states a situation without saying what is possible
next. The row now reads `ready · tools: N · in profile: K` and, when `K` is zero,
points at the "Profiles" section. The double opt-in itself is unchanged.

Verified after the fixes: the same manual run, redone — `mcp-echo-server`
authored in the window as `npx` + `-y mcp-echo-server`, the status row reading
`ready · tools: 1 · in profile: 1`, and the assistant calling
`mcp__mcp-echo-server__echo` successfully. That closes §6's acceptance criterion,
the one part of this track no automated test could stand in for.

## 9. Stage 2 — secrets for the `env` map and JSON import

**Forks S1–S8 confirmed by the user 2026-08-02, all as recommended** — S1(a) the
`env` row declares the variables and a per-variable secret row holds the value,
S2(a)+S3(a) one typed `SetSecret`/`secrets_present`, S4(b) orphaned secrets are
never collected automatically, S5(a) an imported value becomes a stored secret,
S6(b) the import takes a file path and the orchestrator parses it, S7 the import
package, S8(a) the env-name path survives as the fallback. Scope: F4(b) + F6(a)
above, branch `feat/mcp-env-secrets`. Amends **ADR 0007 R8** (as stage 1 amended R6):
the spirit is kept — no plaintext secret on disk — but the storage becomes
[ADR 0008](../decisions/0008-api-key-storage.md)'s machine-bound encryption
instead of "the value lives in an OS environment variable".

### 9.1 The gap, and what reading the code settled

Today `env` is `child variable → **name** of an OS environment variable of the
app`. It keeps `settings.json` free of secrets, and it is a wall: to use a
hosted server (GitHub, Slack, …) the user must set an OS variable **and restart
the app**, which is the opposite of "configure it in the window". The import
(F6) is coupled to it: in the ecosystem's `mcpServers` format those values are
**literal secrets**, so an import without secret storage either loses them or
writes a token in the clear.

Four things were established by reading rather than assumed:

- **The storage needs no change.** `secrets::put_key`/`stored_key` and
  `Orchestrator::store_secret` take an arbitrary key name — proved by the backup
  password (`BACKUP_PASSWORD_KEY`), which reused the same per-machine entry with
  no new list, no new scheme and no migration.
- **The resolution pattern already exists and is per-provider:**
  `resolve_api_key(stored, api_key_env)` — the stored key wins, the env name
  stays as the fallback for CI and power users (ADR 0008 §3). One `api_key` row
  and one `api_key_env` row sit next to each other today; the same pair is
  exactly what an MCP variable needs.
- **Decryption belongs in the manager, not below it.** `EngineManager` itself
  calls `secrets::stored_key` and hands the *supervisor* an already-decrypted
  `stored_key: Option<&str>`. `McpManager` is the same layer, so it decrypting
  is the mirror of that precedent, not a new pattern.
- **`McpManager::is_current` would swallow a secret change.** It compares
  `McpSettings` only, so changing a secret without touching any server field
  leaves the final config identical to the applied one and `flush_restarts`
  skips the re-apply — the server would keep running with the old value and no
  way to notice. The engines already solved this: `chat_is_current(&settings,
  keys)` compares the pair. `is_current` must take the key blob too. **This is
  the one trap that would otherwise ship silently.**

### 9.2 Forks

#### S1 — how a secret value is declared and edited

The flat `VAR=SOURCE, VAR2=SOURCE2` text row is unambiguous *because* the value
is a variable name. A secret cannot go in that slot: not only would parsing stop
being unambiguous, the screen must never hold a secret at all (ADR 0008 §4).

- **(a) [recommended]** Keep `env` exactly as it is — it becomes the
  **declaration** of the child variables plus an *optional* OS source name (an
  empty source is legal and already parses) — and add **one indexed secret row
  per declared variable** below it, showing a status (`configured (this
  computer)` / `not set`) and opening an empty masked editor, exactly like the
  "API key" row. Resolution mirrors ADR 0008 §3: **a stored secret wins, the OS
  variable named in `env` is the fallback.** Cost: nothing. **No new config
  field**, no schema change — the presence of a secret is a fact about
  `config.api_keys`, and `handle_update_config` already restores that field on
  the way back, so secrets survive every settings edit with no new code. Reuses
  `secret_row`, `is_secret_field`, the masked editor and `Del`-deletes.
- **(b)** A second config field (`env_secret: Vec<String>` — the variables whose
  value is stored) edited as its own text row of names, plus the same per-name
  secret rows. Adds a field that duplicates information already derivable from
  the stored key set, and the two can drift.
- **(c)** A sigil in the value (`GITHUB_TOKEN=@secret`). One row, but the secret
  still cannot be *entered* there, so it needs the per-variable rows anyway —
  and it makes the flat parse ambiguous again for no gain.

#### S2 — the set-secret command

Today `AppCommand::SetApiKey { provider, key }` and
`AppCommand::SetBackupPassword(String)` are two variants of one thing; a third
kind would make three.

- **(a) [recommended]** One `AppCommand::SetSecret { key: SecretKey, value:
  String }` with a small typed enum `SecretKey { Provider(CloudProvider),
  BackupPassword, McpEnv { server: String, var: String } }` and one
  `SecretKey::storage_name() -> String`. Typed rather than a raw string: the
  post-store side effects differ per kind (a provider key marks the slots that
  use it for a deferred re-raise and rebuilds the registry for Gemini; an MCP
  secret marks `mcp`; a backup password marks nothing), and dispatching those by
  parsing a string name is how they drift. The two existing commands fold into
  it — the same intent/dispatch path, one handler.
- **(b)** Add `SetMcpSecret { server, var, value }` as a third variant. Smaller
  diff, keeps the duplication the task set out to remove.

#### S3 — presence flags in the settings snapshot

`AppEvent::Settings` carries `api_keys_present: Vec<CloudProvider>` and
`backup_password_present: bool`; the UI never sees a secret, only these.

- **(a) [recommended, if S2(a)]** Replace both with `secrets_present:
  Vec<SecretKey>` — one type shared by the command and the snapshot, so a fourth
  secret kind later costs nothing. ~10 mechanical call sites.
- **(b)** Add a third field (`mcp_secrets_present: Vec<String>` of storage
  names). Least churn, three parallel mechanisms for one fact.

#### S4 — orphaned secrets (a server renamed or deleted)

The storage name embeds the server id and the variable name, so renaming either
orphans the secret, and deleting a server orphans all of them.

- **(a)** Garbage-collect in `handle_update_config`: drop every `mcp-…` secret
  whose (server, variable) no longer exists. **Rejected** — the `env` row is
  committed on `Enter`, so a half-typed edit would destroy a secret the user
  then has to re-enter, and `Ctrl+Z` restores the *config* but cannot resurrect
  a secret (secrets are deliberately not part of the snapshot undo restores).
- **(b) [recommended]** Never collect automatically. An orphan is inert
  ciphertext in this machine's entry; re-entering under the new name is the fix.
  ADR 0008 already lists UI management of stored entries ("forget this
  computer") as groundwork — that is where a cleanup belongs, as an explicit
  act.
- **(c)** Collect only on an explicit server delete (`Ctrl+D`). Narrower than
  (a), but it still makes `Ctrl+Z` of that delete lossy.

#### S5 — what an imported `env` value becomes

In `claude_desktop_config.json` and its relatives, `env` values are literals —
usually a token, sometimes a benign setting (`NODE_ENV=production`).

- **(a) [recommended]** Every imported value is stored as a machine-bound
  secret, and the variable is declared in `env` with an empty source. Lossless,
  and nothing is ever written to `settings.json` in the clear. Cost: a benign
  literal also becomes machine-bound and invisible — re-entered on another
  machine like any other secret.
- **(b)** Classify by variable name (`*TOKEN*`/`*KEY*`/`*SECRET*` → secret, the
  rest → a literal). Needs a third value mode ("literal value stored in
  `settings.json`") — i.e. plaintext on disk, which is what R8 exists to
  prevent — and a name heuristic that is wrong for whatever it does not cover.
- **(c)** Drop `env` on import and report it. That is F6(b), already rejected in
  §3: it is the dead-end-note pattern this project has had to fix twice.

#### S6 — the import's input, and who parses it

- **(a)** A "paste JSON" row (a multiline editor with validation-without-closing).
  **Rejected**: the pasted blob carries live tokens, which would then sit
  **visible on screen** and in the editor's undo buffer.
- **(b) [recommended]** A **file path** row ("Import servers from a file"), with
  the platform's usual location named in the field description. The user has the
  file already; the tokens never appear on screen.
- **(c)** Both, sniffing a leading `{` to tell a blob from a path. Cheap and
  unambiguous, but keeps (a)'s exposure for the blob case.

**Where the parsing lives is not a fork:** the *orchestrator* parses and applies
it (`SettingsIntent::ImportMcpServers(path)` → `AppCommand` → parse → store
secrets → update `config.mcp` → `emit_settings`), mirroring `ConfirmMcpCatalog`.
The orchestrator is the sole writer of `settings.json` and the only layer that
may touch plaintext; a screen-side parse would put every imported token through
`screens`.

#### S7 — import semantics

Recommended as one package:

- imported servers arrive **disabled** — consistent with F5(a): nothing spawns
  until the user says so, and an imported command may not even exist here;
- an **existing id is skipped** and reported, so a re-import is safe and never
  silently overwrites a hand-tuned server (the user deletes first to refresh);
- ids are **sanitized to our slug** (`[a-z0-9-]`, ≤32) with a numeric suffix on
  collision — the ecosystem's keys are free-form (`My_Server`) and ours are part
  of every tool name;
- non-stdio entries (`"type": "sse"/"http"`, a `url` and no `command`) are
  **skipped and reported** — HTTP transport is groundwork, not a silent drop;
- the result is one summary line: imported / skipped / secrets stored.

Alternatives: overwrite an existing id (destroys hand-tuning), or import
enabled (spawns processes the user has not reviewed).

#### S8 — does the env-name path survive?

- **(a) [recommended]** Yes, both coexist, stored wins — the exact
  `api_key`/`api_key_env` shape. The env path is what CI, scripted setups and
  machines without an encryption scheme (Linux with no machine-id, where
  `scheme_available()` is false and the field says so) rely on. Removing it would
  make MCP unusable in precisely those environments.
- **(b)** Replace it: `env` values become secret-only. Simpler UI, breaks the
  above.

### 9.3 Staging

One PR (`feat/mcp-env-secrets`), two commits: **secrets** (S1–S4, S8) then
**import** (S5–S7), which depends on them. Out of scope, and staying on the
roadmap: the external proxy's key via the same mechanism (ADR 0008 groundwork),
HTTP transport, "forget this computer".

### 9.4 Implementation notes

- **`is_current` must take the key blob** — see §9.1. Mirror
  `chat_is_current(&settings, keys)`; a test that changes only a secret and
  asserts a re-apply is the guard.
- **The slot needs the resolved env**, not just `cfg`: a respawn after `Exited`
  and a `reconnect` both build the task from `slot.cfg`. Plaintext therefore
  lives in the manager's memory for as long as the server runs — unavoidable, it
  is what gets handed to the child process.
- **Restrict a child variable name to `[A-Za-z0-9_]`** in `parse_env_map`
  (a `,` or `=` in a name already breaks the flat row today). It also makes
  `mcp-<server>-<VAR>` unambiguous: only the server id can contain `-`, so the
  name splits from the right if it ever needs to.
- **`resolve_env`'s warning changes**: an empty source is no longer "variable not
  found" — it means "the value comes from a stored secret". Warn only when
  neither is present.
- Row **labels for the per-variable secret rows are user data** and can exceed
  `LABEL_CAP = 28`; the value column simply does not grow past the cap (the
  existing safety net), and `all_labels_fit_alignment_cap` covers static labels
  only — as with the server status rows.
- `Ctrl+Z` restores a config snapshot, which by design carries **no** secrets:
  undoing a server delete brings the server back but not its secrets. Same as
  API keys; worth one line in the docs.
- New `FieldId`s: `McpEnvSecret(usize)` (indexed into the selected server's
  declared variables) and `McpImport`. Both are indexed/user data → their own
  branches, not the `field_spec` table, and they join `is_profile_field` so
  `Del`-reset and the `•` marker skip them (`McpEnvSecret`'s `Del` deletes the
  secret instead, via `is_secret_field`).
- i18n: new `ui.settings.*` keys in **both** bundles; the import's summary is
  axis B (a human reads it).

### 9.5 Testing

Unit: the storage name is built and looked up consistently; a secret wins over
an env name and an env name is still the fallback; `is_current` says "not
current" when only a secret changed (**the §9.1 trap**, mutation-tested); the
per-variable rows appear for declared variables only, show status, and never put
the secret into the screen's config; import — a golden `mcp-servers.json`
fixture (a stdio server with `env`, an existing id, an `sse` entry, a free-form
key needing sanitization) asserting servers arrive disabled, secrets are stored
as ciphertext, **no plaintext appears in `settings.json`**, ids are sanitized,
and the summary counts match; a re-import is a no-op.

**A live run is required** (AGENTS.md §3 — this touches the tool path):
`mcp_filesystem_e2e_live` and `mcp_reconnect_live` (they need only `npx`), plus
a new smoke that spawns a server whose behaviour depends on an env value
delivered from a stored secret — the one thing unit tests cannot show is that
the value actually reaches the child process. Acceptance, as in stage 1, is a
**manual run**: import a real `claude_desktop_config.json`, enter a token in the
window, and have the assistant call that server's tool — with no OS environment
variable set.

### 9.5a Revised after the first manual run (S1's UI form)

The user imported a real config (a Cursor one) successfully, and then said the
variable configuration "looks strange and inconvenient — why type
`variable=value` pairs and then enter the value in a separate field as well".

Fair, and the phrasing is the evidence: the row's value slot means the **name of
a source variable**, but it *reads* as `variable=value`, so the field asking for
the value underneath looks redundant. Checking `McpClient::spawn` settled how
much of the mechanism was even load-bearing: it uses `Command::envs` with **no**
`env_clear`, so the child already inherits the whole application environment.
The `NAME=SOURCE` form is therefore only needed to take a value from a
*differently named* variable — the same-name case works by inheritance, with no
configuration at all.

So the row became a **bare list of names** (`GITHUB_TOKEN, SLACK_TOKEN`), with
`NAME=SOURCE` still accepted and no longer advertised, the label changed from
"Environment" to "Variables", and the hints were rewritten around "the value goes
in the row below; a variable already in the app's environment is inherited
anyway". **No behaviour changed** — only what the field asks for. The
inheritance claim the new hint makes is now pinned by the live smoke: a variable
set in the test process and named nowhere in the server's config reaches the
child (it would go silently untrue if `env_clear` ever appeared).

### 9.5b One origin per variable (the second follow-up)

Looking at the result, the user said that when a source variable *is* named,
offering to enter a value as well is unnecessary. It is worse than unnecessary:
S8 made a stored value **win** over the named source, so the row could say "take
it from `CLAUDE_API_KEY`" while a secret silently overrode it — a hidden state
nothing on screen could explain. Hiding the value row alone would have made that
worse, not better: the override would still happen, with the only thing that
could reveal it now gone.

So the rule became **one origin per variable, decided by the row**: a bare name
takes the stored value (and with none stored is left to inheritance);
`NAME=SOURCE` takes the value from that OS variable and consults no stored value
— and gets no value row. This narrows S8 rather than reversing it: the fallback
existed so a shared config could work on a machine with the variable set in the
OS, and that case uses the variable's own name, which inheritance already covers.
Only "renamed source **and** a stored value" changes behaviour, and that
combination is precisely the contradiction.

### 9.5c Both routes report themselves (the third follow-up)

With the UI accepted, the user's remaining objection was about the *logic*: a
fallback plus machine-bound storage means "checking whether the variable exists
and whether a value was entered on this particular machine".

Half of that had already gone with §9.5b — for a single variable the row now
decides which route applies, so the two are never both in play. What was left is
an **asymmetry of visibility**: the stored route reports itself ("configured
(this computer)" / "not set") while a named source reported nothing, so a missing
OS variable surfaced only as the server failing to work. That, rather than the
existence of two mechanisms, is what makes it feel like something has to be
checked by hand.

So a variable that names a source now gets a **read-only status row** in place of
the value field it deliberately does not have: `PRESENT — from
MINDFORK_SRC: found` / `ABSENT — from …: not found`, flagged when missing. The
description also states what was invisible before: the application sees the
environment it was **started with**, so a variable set after launch needs a
restart. Rejected alternatives: leaving it silent (the diagnosis stays indirect)
and dropping the source form from the UI entirely (it is the only way to rename a
source, and the user had called the fallback worth keeping).

### 9.6 Documentation to update on completion

CLAUDE.md journal; CHANGELOG (`Added` + `Security`); **ADR 0007** — R8 rewritten
(env names *or* machine-bound stored values, both without plaintext on disk),
with a pointer to ADR 0008; **ADR 0008** — its "MCP server env maps" groundwork
item closes; spec §9.6 (secret values, import); README; docs/install.md §4.2 (a
token no longer needs an OS variable); docs/roadmap.md (the stage-2 item closes,
HTTP transport and "forget this computer" stay).
