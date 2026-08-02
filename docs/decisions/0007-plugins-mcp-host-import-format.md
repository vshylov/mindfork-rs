# ADR 0007 — Plugins: MCP host for tools + a neutral import exchange format

**Status:** accepted (2026-07-17). Fixes the architecture of the "plugin
system" track (stages 1–3). Research, decision points R1–R8, and probe results
— [docs/research/plugin-system.md](../research/plugin-system.md). Related to
[ADR 0002](0002-embeddings-dedicated-server.md)/[ADR 0005](0005-python-sandbox-wasmer.md)
(external process behind a trait/contract) and
[ADR 0004](0004-engine-contract-multi-provider.md) (engine boundaries).

## Context

The request: a "plugin system" — the non-public LameLLaMA importer out of the
monolith, user-defined model tools without rebuilding the app, pluggable cloud
providers. Research showed this is **three surfaces of different nature**, and
a single in-process plugin API (a dylib) is the worst option (Rust has no
stable ABI; `abi_stable` is dead; Bevy removed dynamic plugins as unsound).

## Decision

### 1. Tools — an MCP host (stdio subprocesses), our own micro-client

- **MCP** (Model Context Protocol) is the de facto standard for AI tools:
  thousands of ready-made servers, the genre convention among terminal chat
  clients (R1). A custom protocol would give zero ready-made tools.
- **Our own micro-client** (`shared/mcp.rs`), not `rmcp` (R2): the tools-only +
  stdio subset of the 2025-11-25 revision is wire-stable back to 2024-11-05,
  zero new dependencies, texts are localizable; `rmcp`'s churn (breaking
  changes in minor releases) doesn't pay off. The transport (`McpConnection`)
  is decoupled from the process (`McpClient`) — the protocol is tested over
  `tokio::io::duplex` with no processes. Any counterparty protocol version is
  accepted (confirmed by the probe against a live server).
- **Lifecycle** — `McpManager` in the orchestrator (mirroring `EngineManager`):
  background spawn tasks → `Ready`/`Failed`/`Exited` events with an `epoch`
  guard; a restart budget of 3 crashes per 5 minutes (the VS Code LSP
  pattern); settings-change debounce via `RestartQueue`. A server's tools are
  `McpTool: Tool` wrappers (`mcp__<server>__<tool>`, normalized ≤64 chars) in
  the shared registry; called by the standard agentic loop, results clipped
  (`max_result_chars`).
- **Cancellation as a contract**: `ToolContext.cancel` (a clone of the turn's
  token) + `select!` around `invoke` for any tool in the agentic loop — Esc
  isn't blocked by a long-running call; an MCP call sends the server
  `notifications/cancelled`.

### 2. Security: double opt-in + TOFU catalog pinning (R7, R8)

An MCP server is an arbitrary program with the user's privileges (equivalent
to installing software; there is no sandbox — same as goose/Zed/Claude Code).
Mitigations:

- master switch `config.mcp.enabled = false` by default; servers are
  configured only by the user — R6 said "by editing `settings.json`", and was
  revisited in 2026-08 once the host had proved itself: the same data is now
  authored in the settings screen's "Plugins" section
  ([mcp-server-editor.md](../history/mcp-server-editor.md)), which changes the
  authoring surface and nothing about the trust model; tools are
  **disabled in profiles by default** → a double opt-in;
- **TOFU catalog pinning** (rug-pull detector, tool poisoning): a sha256 of
  the tools' names+descriptions+schemas is pinned on first startup
  (`pinned_catalog`); if the catalog changes, the tools are held back until
  reconfirmed in settings (Enter on the server's row);
- **full tool descriptions are visible** in the UI (the settings screen's
  bottom panel) — descriptions go into the system prompt on every turn;
- secrets — R8 originally said the `env` map holds only **names of environment
  variables** (the `api_key_env` precedent), and was **revisited in 2026-08**
  together with R6: a hosted server (GitHub, Slack) then needed its token set in
  the OS and the app restarted, which is the opposite of "configure it in the
  window". A declared variable's value can now also be entered in the settings
  screen and is stored **encrypted with this machine's key**
  ([ADR 0008](0008-api-key-storage.md)) under `mcp-<server>-<VARIABLE>` in the
  same per-machine entry as the cloud keys — so R8's actual invariant is
  unchanged: **no secret is written to `settings.json` in plaintext**, and the
  config stays portable. Resolution mirrors ADR 0008 §3 — a stored secret wins,
  the named OS variable remains the fallback for CI, scripted setups and
  machines with no encryption scheme. The same storage is what makes importing
  another client's `mcpServers` JSON lossless: those files carry **literal**
  secrets, so the import is parsed by the orchestrator (never by `screens`) and
  each value goes straight into the store. See
  [mcp-server-editor.md](../history/mcp-server-editor.md) §9;
- the server command is resolved as a shell would (`PATHEXT` completion on
  Windows), so one config works on every platform. **The `.bat`/`.cmd` ban was
  removed in 2026-08** after measuring the premise: CVE-2024-24576 ("BatBadBut")
  is fixed in `std` as of Rust 1.77.2, which escapes batch-file arguments and
  **refuses** the ones it cannot escape — so the ban added no protection while
  forcing users onto `cmd /c npx …`, where the arguments are re-parsed by
  `cmd.exe` *outside* that escaping. Spawning the resolved `.cmd` directly is
  both portable and strictly safer. `CREATE_NO_WINDOW`, Job Object kill-on-close (the
  `cmd /c npx → node` process tree does not survive the app exiting or
  crashing), clipped results and per-call timeouts, server stderr only goes
  to the file log.

### 3. Importers — a neutral documented format, not a plugin process (R3, R4)

Import is a one-off batch operation: the protocol *is* a **file**. The
`mindfork-import` v1 format
([docs/import-format.md](../import-format.md)): flat JSON with
profiles/chats/settings, idempotent via UUIDv5 derived from stable `key`s,
strict validation of structure while tolerating unknown fields. CLI `mindfork
import <file>`; `import-lamellama` **was removed** — knowledge of the
non-public LameLLaMA moved into a private external converter. The alternative
"domain entities as-is" was rejected: an external contract would have fixed
the internal schema wholesale.

### 4. Cloud providers — the external mode already is the plugin API (R5)

An OpenAI-compatible endpoint is the industry boundary for plugging in
providers; our `external` mode (url + `api_key_env` + model) already covers
it, and bridges like LiteLLM/OpenRouter give "any exotic option." Providers
with unique capabilities (thoughts/signatures/effort) are added as native
trait implementations (ADR 0004) — deliberately not plugins. An engine plugin
API was rejected.

## Consequences

- A new "Plugins (MCP)" tool group is dynamic: wrappers are not part of the
  static `CATALOG`, the registry is rebuilt on events (`rebuild_registry`),
  the catalog for the UI travels as an `McpSnapshot` inside
  `AppEvent::Settings`; the `effective_tool_ids` gate matches by the `mcp__`
  prefix.
- The context budget is the main systemic constraint (schemas for 5–6 servers
  ≈ 15k+ tokens): mitigated by per-profile opt-in for each tool; a per-server
  tool cap and deferred schemas are groundwork.
- i18n boundaries: tool descriptions/schemas/results — server text (not
  localized); manager status reasons — axis B; client wire errors — a
  technical layer (like the HTTP client wrappers).
- Groundwork (roadmap): HTTP transport, resources/prompts,
  `notifications/tools/list_changed`, deferred schemas ("tool search"), a WASM
  sandbox for untrusted tools, server `instructions` → system prompt.

## Verification

The probe (stage 2) and live e2e smokes (stage 3): Gemma 4 31B + a real `npx
@modelcontextprotocol/server-filesystem` — the model calls
`mcp__fs__read_text_file` on the first round and uses the result; 14 tools ≈ 7
KiB of schemas don't blow out a 16k context; TOFU/cancellation/budget are
covered by unit tests (duplex fakes, paused time).
