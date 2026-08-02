# MCP servers in the settings window — design plan

**Status:** stage 1 **done** (2026-08-02); forks accepted — **forks F1–F8 confirmed by the user 2026-08-02,
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
    command: String,                 // .bat/.cmd forbidden (BatBadBut)
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
