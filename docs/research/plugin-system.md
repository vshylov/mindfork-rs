# Research: plugin system (importers, tools, cloud integrations)

**Status:** research done, track implemented (stages 1–3 of the plan §7);
decisions taken are recorded in
[ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md), behavior —
spec §9.6. Forks §8 accepted by the user 2026-07-17; probe results — §9.
**Date:** 2026-07-16. **Branch:** `docs/plugins-research`.
**Web facts** (crate versions, spec revisions, precedents) checked against
primary sources on 2026-07-16 by four parallel research passes; links in §10.

## 1. Task

User's request (2026-07-16): investigate the feasibility of a **plugin
system**. Named goals, in order of priority as stated:

1. **LameLLaMA importer** — the best candidate for the first plugin: LameLLaMA
   is a non-public program, and knowledge of its formats shouldn't be kept in
   the monolith (`features/migration.rs` today carries the wire types for its
   `Settings.json`/`Conversations`).
2. **Tools as plugins** — plug in user-supplied tools for the model without
   rebuilding the application.
3. **Cloud platform integrations** — pluggable inference providers.

**Main conclusion of the research:** a "plugin system" is not one technology
but **three surfaces with different natures**, and the best design for each is
different. A single in-process plugin API (dylib "like big programs do it") is
the worst option: Rust has no stable ABI, and the AI-application industry in
2025–2026 has already converged on other mechanisms (MCP for tools, "any
OpenAI-compatible endpoint" for providers, a documented exchange file for
importers). Below is the justification and a concrete plan.

## 2. Three extension surfaces (code analysis)

| Surface | Contract today | Nature | What a "plugin" needs |
|---|---|---|---|
| Importers | `features/migration.rs::import_dir(dir, loc) -> ImportResult{profiles, chats, sampling, interface}` → upsert into `Storage` (CLI `import-lamellama`) | **a one-off batch operation** with no TUI | read a foreign format, hand back profiles/chats |
| Tools | `Tool` trait (`id`/`description(loc)`/`parameters(loc)` JSON schema/`invoke(ctx) -> ToolOutcome{result, effects}`), `ToolRegistry`, static `CATALOG`, per-profile `enabled_tools` + `reconcile_tools`, gates `effective_tool_ids`; called by the client-side agentic loop between HTTP rounds | **long-lived, called in the hot loop of a turn**; `ToolContext` carries `Arc<Storage>`/`engine`/`embedder` — **not passable** across a process boundary | accept JSON args, return text; own schema and description |
| Engines | `EngineBackend` trait (`chat_stream` — SSE streaming with cancellation) + `Embedder`; 4 implementations (llama.cpp/OpenAI/Gemini/Anthropic); **external mode already accepts any OpenAI-compatible server** (url + `api_key_env` + model) | **streaming, latency-sensitive**, complex protocol (thoughts/signatures/tool-calls) | speak the inference protocol |

Consequences:

- A plugin tool **won't get** `ctx.storage`/`ctx.embedder` — and shouldn't:
  memory/RAG with per-profile isolation is core internal state. Plugins are
  "external" tools (APIs, files, devices), self-sufficient with their own
  dependencies.
- A plugin engine "beyond the process" must speak the streaming protocol —
  i.e. be an HTTP server. Such a protocol already exists and is a de-facto
  standard (OpenAI-compatible), and the app already speaks it (external mode).
- An importer doesn't need a long-lived process at all: the protocol is a
  **file**.

Project precedents directly support the out-of-process direction: managed
`llama-server` (ADR 0004), the `wasmer` sidecar (ADR 0005 — embedding in a dll
was considered and **rejected** in favor of a subprocess), a dedicated
embedding server (ADR 0002). All the needed tooling is already in the
dependency tree: `tokio::process` + monitor tasks with an `exited` token,
`kill_on_drop`, Job Object (windows-sys already in use), `serde_json`,
`async-trait`.

## 3. Landscape of plugin mechanisms (web-checked, July 2026)

### 3.1 In-process dylib — REJECTED

- **Rust has no stable ABI and none is on the horizon**: RFC crABI
  (rfcs#3470) and RFC `#[export]` (rfcs#3435) have been open since 2023 and
  aren't accepted; the experimental nightly implementation `export_stable`
  (rust#134767, merged 2025-05-05) isn't in the 2025H1/H2 project goals.
- **`abi_stable` is dead** — last release 0.11.3 from 2023-10-12, zero commits
  for almost 3 years, community PRs (including C-unwind and the fix for
  RUSTSEC-2024-0014) unmerged. The live alternative is `stabby` (ZettaScale,
  72.1.8, June 2026), but that's a single-vendor solution for Zenoh's needs
  (license EPL-2.0 OR Apache-2.0).
- **A negative precedent of the highest caliber**: Bevy deprecated (0.14) and
  removed (0.15) `bevy_dynamic_plugin` as **unsound** ("likely unsound, or at
  the very least so dangerous…", bevy#11969).
- Practice for third-party authors requires either **toolchain lockstep**
  (same rustc + flags) or a frozen C-ABI with all its restrictions (no
  String/Vec across the boundary, panic = abort, allocations don't cross the
  boundary, don't unload the dll). Windows users won't rebuild plugins.
- Also against: project precedent — ADR 0005 rejected in-process embedding
  (dll) in favor of a sidecar, for failure-isolation and build-weight reasons.

### 3.2 In-process WASM — not for our surfaces (groundwork)

The state is mature: wasmtime 46 (Tier 1 on Windows x64; component model,
wasi-http, wasi-sockets — Tier 1; WASI 0.3 with async shipped 2026-06-11),
Rust has a native `wasm32-wasip2` target since 1.82; Zed is the reference for
plugins on a versioned WIT; Extism 1.30 is a turnkey wrapper (memory/fuel/
timeout limits, HTTP via a host function with `allowed_hosts`). But:

- **Runtime weight**: Zellij, in v0.44.0 (March 2026), moved off wasmtime to
  the `wasmi` interpreter specifically for binary size and to avoid
  compile+cache — a JIT runtime is heavy for a TUI. We'd have to embed
  wasmtime/Extism in the main exe — against the project's spirit (ADR 0005:
  "zero Wasmer in the main exe").
- **The sandbox value evaporates given the task**: user-supplied tools need
  network and arbitrary I/O — meaning we'd still have to hand out
  capabilities; isolation remains, but the main argument ("safely run
  untrusted code") weakens, while the cost (a custom SDK and toolchain for
  plugin authors, a custom ABI on top of bytes) remains.
- No ecosystem of ready-made tools exists for our own custom WASM ABI — every
  tool would have to be written specifically for us.

Filed as **groundwork**: if the need arises to run *untrusted* tools without
installing processes — Extism/wasmtime + WIT solves it; our `wasmer` sidecar
already gives an asset-provisioning precedent.

### 3.3 Embedded scripting (Rhai / mlua / Steel) — not taken

Alive (Rhai 1.25.1, mlua 0.12.0, Steel 0.8.2), but: tools need network/files —
we'd have to keep expanding bindings for everything; execution happens in our
process with no isolation; Helix hasn't been able to merge Steel plugins since
2023 (PR #8675 — still draft). For "private code outside the monolith" a
script would work, but LameLLaMA import is solved more simply (a file), and
tools more conventionally (MCP).

### 3.4 Subprocess/sidecar — CHOSEN

The industry converges exactly here, and all the patterns are documented:

- **MCP** (stdio, NDJSON JSON-RPC 2.0) — the de-facto standard for AI tools
  (Anthropic 2024-11; supported by Claude/OpenAI/Google, goose, Zed, oterm,
  gptme, and other hosts). Current spec revision **2025-11-25**.
- **LSP** (Content-Length framing) — the older sibling; `rust-analyzer` also
  provides a precedent of "subprocess for ABI isolation" (proc-macro-srv).
- **nushell** — subprocess plugins with msgpack/json, registration, and idle
  GC; **HashiCorp go-plugin** — the canonical long-lived sidecar with
  handshake and mTLS.
- Windows nuances are known and solvable: `CREATE_NO_WINDOW`; **banning
  `.bat`/`.cmd`** as plugin commands (CVE-2024-24576 "BatBadBut" — Rust
  ≥1.77.2 itself rejects unescapable arguments; document `npx` servers on
  Windows as `cmd /c npx …` or don't support a full path to a `.cmd`); **Job
  Object** for killing a process tree (our `windows-sys` already does this —
  the Python sandbox).
- Exactly one ready-made "go-plugin for Rust" framework exists — **MCP via the
  official SDK `rmcp`** (2.2.0, July 2026); outside MCP it's "build it
  yourself from tokio+serde_json," which the project has already done twice
  (llama-server, wasmer).

## 4. The "tools" surface: MCP host (recommendation)

### 4.1 Why MCP, not our own protocol

- **Ecosystem**: thousands of ready-made servers (files, git, GitHub,
  databases, browser, …) and author SDKs in every language — our own protocol
  would launch with zero ready-made tools. Negative precedent: `llm-functions`
  (aichat) — a decent custom format that never spread beyond its host app.
- **Terminal chat clients have already done this** (oterm, gptme,
  mcp-client-for-ollama) — "an MCP client from a config file" has become a
  genre convention; the project's roadmap already carries the groundwork item
  "MCP client — a natural extension of ToolRegistry."
- Our `Tool` trait maps onto an MCP tool **without loss**: `description` and
  the JSON schema come from the server, `invoke` → `tools/call`, text result →
  `ToolOutcome::text` (external tools have no effects).

### 4.2 Protocol scope (narrow and stable)

We take the **tools-only, stdio-only** subset of revision **2025-11-25** — it's
been wire-stable since 2024-11-05:

- transport: subprocess, newline-delimited JSON-RPC 2.0, UTF-8, no embedded
  newlines; **server stdout is protocol only**, stderr is logs (must be
  drained to our `logs/`, or the pipe fills up);
- `initialize` (we send `protocolVersion: "2025-11-25"`, `capabilities: {}`,
  `clientInfo`; accept whatever version they answer with, as we understand
  it, otherwise disconnect) → `notifications/initialized`;
- `tools/list` (+ pagination `cursor`/`nextCursor`), `tools/call`
  (`isError: true` → error text to the model, not a protocol error);
- `ping` (we answer with an empty result), `notifications/cancelled` (sent on
  timeout/cancellation), `notifications/tools/list_changed` (re-read the
  catalog);
- for unsupported **requests** from the server (`sampling/createMessage`,
  `elicitation/create`, `roots/list`) we answer `-32601` (otherwise a correct
  server would hang waiting); unknown **notifications** are silently ignored;
- shutdown: close stdin → bounded wait → SIGTERM (unix) → kill; on Windows —
  stdin-close → Job-Object/TerminateProcess.

**Not taken** (groundwork): resources, prompts, sampling, roots, elicitation,
HTTP transport, structuredContent validation. Revision 2026-07-28 (RC:
stateless core, extensions) has an stdio-compatible path and ≥12-month
deprecation windows — nothing needs pre-building; investing in
roots/sampling/logging isn't worth it (deprecated in the RC).

### 4.3 Client implementation: our own micro-client vs `rmcp` (fork R2)

| | Our own micro-client | `rmcp` 2.x |
|---|---|---|
| Scope | ~6 message kinds; estimated 600–900 lines with tests | `default-features=false, features=["client","transport-child-process"]` |
| Dependencies | **zero new ones** (tokio/serde_json/async-trait already present) | rmcp + rmcp-macros + process-wrap 9 + which 8 (+ transitive tail) |
| Spec conformance | we implement the pitfalls of §4.6 ourselves (a known list) | 2.2.0 passes the official conformance suite |
| API stability | our own | real churn: 1.8.0 → 2.0.0 → 2.1.0 → 2.2.0 in 3 weeks (June–July 2026), breaking changes in minor releases |
| i18n/logging | full control (errors go into bundles, axis B) | wrappers over English errors |
| Project precedent | our own CLI parser (instead of clap), markdown, calc, i18n, three cloud SSE parsers | — |

**Recommendation: our own micro-client.** The subset is small and has been
wire-stable since 2024-11-05, zero new dependencies, error text is
localizable, and this is exactly the case where the project has historically
chosen a micro-implementation. `rmcp` is the fallback path if the subset
starts growing (HTTP transport, sampling, tasks) — the switch is localized
behind a transport trait. Honest research caveat: it's a "genuine toss-up" —
rmcp's ready-made conformance is valuable; the decision is the user's.

### 4.4 Integration into the architecture

**Config** (`shared/config.rs`, all `#[serde(default)]` — no migration):

```jsonc
"mcp": {
  "enabled": false,              // master switch (OFF by default, like python)
  "servers": [{
    "id": "github",              // slug [a-z0-9-]{1,32} — part of the tool ids
    "command": "C:/tools/github-mcp.exe",
    "args": ["--stdio"],
    // env var for the child → NAME of the source variable in the app's environment
    // (the secret isn't written to settings.json — precedent api_key_env; see R8):
    "env": { "GITHUB_TOKEN": "MINDFORK_GITHUB_PAT" },
    "enabled": true,
    "tool_timeout_secs": 60,     // per-call; startup timeout is separate (default 30s)
    "max_result_chars": 20000    // result clip (precedent: Claude Code caps at 25k tokens)
  }]
}
```

**Lifecycle — `McpManager`** (mirrors `EngineManager`,
`app/orchestrator/mcp.rs`): spawns enabled servers on startup/settings change
(via the `RestartQueue` debounce, like engines); a monitor task with an
`exited` token (pattern from `managed.rs`); handshake + `tools/list` → a
**dynamic catalog**; per-server status (`Ready`/`Connecting`/`Disconnected(reason)`).
A restart budget like VS Code's LSP: N crashes within a window → `Disconnected`
until manual intervention (no eternal restart loop).

**Tool registration**: a `McpTool` wrapper implements `Tool`
(`invoke` → `tools/call` with a timeout; `description`/`parameters` — a
snapshot from `tools/list`, **not localized** — an i18n boundary, like engine
probe errors); `group()` → a new `ToolGroup::Plugins`;
`enabled_by_default() = false` (opt-in per profile, `reconcile_tools` won't
enable it automatically — it's not in `default_tool_ids`). The orchestrator's
registry is already rebuilt on settings changes — MCP tools get added to it
once the server is ready (another reason for the rebuild), and a turn picks
up the current snapshot.

**Naming**: `mcp__<server>__<tool>` (Claude Code convention; goose uses
`server__tool`). The spec (SEP-986) allows `A-Za-z0-9_-.` up to 128
characters — we normalize to provider function-name limits: `.` → `_`, result
≤ 64 chars (truncate + a short hex tail derived from the full name; check the
exact provider limits at implementation time — historically
`^[a-zA-Z0-9_-]{1,64}$` for OpenAI/Anthropic). The id is stored in
`Profile.enabled_tools` as a plain string — the profile contract doesn't
change.

**Integration points that need attention** (the main "ripples"):

1. **`CATALOG` is static** (a `LazyLock` from `standard_registry`) — dynamic
   tools don't land there. Profile toggles (`tool_catalog()` in
   `screens/settings`) and `effective_tool_ids` need a dynamic addendum: an
   MCP-catalog snapshot rides on `AppEvent::Settings` (the orchestrator knows
   it), the gate is by the `mcp__` prefix + `config.mcp.enabled` (a new branch
   in `effective_tool_ids`, not a `CATALOG` lookup).
2. **i18n gate tests unaffected**: `all_tool_descriptions_localized_to_en`
   iterates the static `CATALOG` — MCP tools aren't there by construction.
3. **Cancellation**: the agentic loop currently `await`s `registry.invoke`
   without `select!`-ing against generation cancellation; a slow `tools/call`
   would block Esc. In the MCP phase, a per-call timeout is mandatory;
   threading a `CancellationToken` through `ToolContext` (and `select!` in the
   loop) is a small, separate refactor, also useful for our native tools
   (`web_search` isn't cancellable today either). Include it in scope.
4. **Feed**: `present.rs` already has a generic branch (Markdown/Plain) — MCP
   tool results render through it with no changes; non-text result blocks
   (image/audio/resource) get a text placeholder `[image …]` in Phase 1.
5. **Settings UI**: a "Plugins" section (or a group inside "Tools"): server
   status chips (the `ServerStatuses` pattern), per-server enable toggles,
   **full tool description view** (tool-poisoning antidote, §4.5). Adding a
   server — by editing `settings.json` (R6).
6. **Context budget — the main systemic constraint.** Schemas from 5–6
   servers ≈ 15k+ tokens; hosts introduce caps (Cursor — 40 tools, VS Code —
   128). For our local 8–16k contexts, the existing mechanism already saves
   us: **per-profile opt-in for each tool** (enable the 3 you need, not 40).
   Plus a per-server tool cap (config, default ~25) with a warning.
   Deferred schemas ("tool search," as in Claude Code) — groundwork.

### 4.5 Security (per official best practices + Invariant Labs)

Threat model: an MCP server is an **arbitrary program running with the
user's privileges** (equivalent to installing software; no sandbox in Phase
1 — same as goose/Zed/Claude Code), plus **tool poisoning** — instruction
injection through tool descriptions (OWASP MCP03:2025), rug-pull (the
description changes after approval), cross-server shadowing, poisoning of any
schema field or result.

Mitigations within the MCP-phase scope:

- a master switch `mcp.enabled=false` + servers are configured only by the
  user (the full command is visible in config/UI); tools are **disabled in
  profiles by default** (double opt-in);
- **TOFU pinning**: a hash (name+description+schema) of a server's catalog at
  first approval; a change → the server is flagged "catalog changed, please
  re-confirm" (rug-pull detector; Invariant Labs / mcp-scan recommendation);
- showing the **full** tool description in settings UI (not truncated);
- clipping the result (`max_result_chars`) and timeouts (`tool_timeout_secs`)
  — bounded prompt input; server stderr goes only into the file log;
- banning `.bat`/`.cmd` commands (BatBadBut), `CREATE_NO_WINDOW`, Job Object
  (the process tree doesn't outlive the app's exit);
- annotations (`readOnlyHint`/`destructiveHint`) are shown in the UI but
  treated as **untrusted** (the spec's own stance). Per-call confirmation for
  destructive calls is deliberately NOT in the first phase (a new modality
  mid-generation; double opt-in + TOFU is enough to start) — groundwork, fits
  into the existing confirmation popup (`ConfirmAction`).

The stance agrees with spec.md §13.1 ("moderate requirements" for prompt
injection) — but tool descriptions go into the system prompt every turn, so
the minimum (double opt-in, description visibility, TOFU) is mandatory.

### 4.6 Known implementation pitfalls (checklist for the phase)

From hosts' field experience (VS Code/Codex/Claude Code issue trackers):
servers writing junk to stdout (banners/`print`) → **skip non-JSON lines with
a warn**, don't tear down the connection; undrained stderr blocks the server;
`npx`/`uvx` are `.cmd` shims, `ENOENT` on Windows without a shell; killing
only the direct child leaves orphans (need a Job Object/process group); some
servers don't exit on stdin-close (escalate to kill); request ids are unique
and non-null, don't reply to notifications; don't invent `_meta` keys with the
`modelcontextprotocol/mcp` prefix; startup latency of "heavy" servers needs
its own startup timeout.

### 4.7 Testing

Transport behind a mini-trait (`spawn` separated from read/write) → unit
tests for handshake/framing/pitfalls on `tokio::io::duplex` without
processes; functional ones — against a tiny test-server script (a ready-made
binary in tests, like the short-lived processes in `managed.rs` tests); a
live `#[ignore]` smoke — against a real third-party server (via an env
variable with the command), plus an e2e against a live model (§7, the
probe).

## 5. The "importers" surface: neutral format + external converters (recommendation)

### 5.1 Why not a "plugin process"

Import is a one-off batch operation: the protocol is a **file**. Exactly how
the mature precedents are built: beancount 3.x moved importers out of the
core (beangulp: private converters emit documented ledger text), KeePass
(documented XML/CSV + Generic CSV Importer, converted by third parties),
Netscape bookmarks HTML (30 years of a universal import boundary). Counter-
example: ChatGPT's `conversations.json` — an undocumented **tree** of nodes
that every consumer has to re-learn how to flatten. Conclusion: an exchange
format must be **flat and documented**.

### 5.2 Design

- **Documented format** `mindfork-import.json` (in `docs/import-format.md`):

```jsonc
{
  "format": "mindfork-import", "version": 1,
  "profiles": [{
    "key": "assistant-anna",          // stable external key (for UUIDv5 idempotency)
    "name": "Anna", "language": "ru",
    "system_message": "…", "greeting": null,
    "character_names": { "user": "…", "assistant": "…" },
    "sampling": { "temperature": 0.8 }   // supported subset; unknown fields dropped
  }],
  "chats": [{
    "key": "conv-123", "profile_key": "assistant-anna",
    "title": "…", "created_at": "…", "modified_at": "…",
    "system_message": "…",
    "messages": [{ "role": "user|assistant|system", "text": "…",
                    "thoughts": null, "timestamp": "…" }]
  }],
  "settings": { "sampling": {…}, "interface": {…} }   // optional
}
```

- **Generic CLI**: `mindfork import <file.json>` — validation (format version,
  a downgrade guard like the data-schema one), mapping into domain entities,
  **idempotency** via deterministic UUIDv5 from `key` (exactly today's
  `PROFILE_NAMESPACE` mechanism), upsert into `Storage`. Sanitization — as in
  `import-lamellama` (dropping unsupported sampling, BOM, etc.).
- **Private converter** `lamellama2mindfork` — a separate **private**
  repository (any language; the ready-made wire types can be carried over
  as-is from `features/migration.rs`): reads a LameLLaMA directory → emits
  `mindfork-import.json`. All knowledge of the non-public program leaves the
  monolith.
- The format also opens up importing **from anything** (ChatGPT export,
  SillyTavern JSONL, attempt #1) — converters are written without touching
  the monolith.

### 5.3 Alternative (rejected): exchange format = domain `Profile`/`Chat` as-is

For: serialization already exists, schema versions/migrations already exist
(ADR 0006). Against (decisive): an external contract would fix the
**internal** schema wholesale (`deleted`, `reflected_upto`, `tool_calls`,
metadata…) — every domain refactor would become a breaking change for
third-party converters. A dedicated format-v1 is small, stable, and promises
nothing extra.

### 5.4 Fate of `import-lamellama` (fork R4)

Recommendation: **remove** it from the monolith in the same phase (CHANGELOG:
Removed + Added `import` sections), the `import-lamellama` command points
users to the converter. Alternative — keep it deprecated for 1–2 releases
(dual code path). Idempotency survives the switch: the converter emits the
same stable keys (config names / conversation ids), the UUIDv5 namespace
moves into the format spec.

## 6. The "cloud integrations" surface: OpenAI-compatible endpoint = plugin API (recommendation)

- **Pulling `EngineBackend` out of the process = inventing an inference HTTP
  server.** It's already invented: the OpenAI-compatible protocol; our
  **external mode already exists** (url + `api_key_env` + `model_name`) — this
  is exactly the provider plug-in point.
- The industry does exactly this: aichat (`openai-compatible` client type),
  LibreChat ("custom endpoints"), Open WebUI Pipelines (the plugin boundary
  *is* the OpenAI format itself). Multiplexer bridges: **LiteLLM proxy**
  (self-hosted, MIT core, 100+ providers behind one endpoint) and
  **OpenRouter** (hosted, 400+ models). One external slot covers "any
  exotic thing" through them.
- Providers with their own protocol and unique capabilities (thoughts,
  reasoning signatures, effort) get added via a **native trait implementation
  in the monolith** — as already happened with Responses/Gemini/Anthropic
  (ADR 0004). This deliberately isn't plugins: integration quality (streaming
  "thoughts," tool-use round-trip) requires deep coupling with the contract.
- What to do within this track (small scope): **document the pattern** in
  install.md (LiteLLM/OpenRouter recipes for external mode). An option beyond
  that (fork R5): **managed-custom-command** — the supervisor can spin up an
  *arbitrary command* as an OpenAI-compatible sidecar (a generalization of
  `ManagedConfig`: command+args instead of llama-server specifics; the
  `/health` probe already tolerates 404). Gives "a local proxy starts
  itself." Recommendation: defer until real demand appears — external + a
  manually started proxy already covers the scenario today.

## 7. Phased plan

Track "plugins," stages = separate branches/PRs (AGENTS.md §1–2):

- **Stage 1 — `feat/generic-import`** (stands on its own merit, closes goal
  #1): format `mindfork-import.json` v1 + `docs/import-format.md`; CLI
  `mindfork import` (our own parser already handles subcommands; i18n text
  goes into bundles); removal of `import-lamellama` (per R4) — knowledge of
  LameLLaMA moves into a private converter (written outside this
  repository). Tests: mapping/idempotency/validation/format golden file. No
  live engine run required (a file operation); manual verification against a
  real export (6 profiles / 226 chats — M9 precedent).
- **Stage 2 — `spike/mcp-client`** (probe, go/no-go): a mini stdio client
  (initialize/tools list/call/ping/shutdown) + a temporary tool registration
  for one server; **GO criterion**: a live local model (Gemma 4) correctly
  calls an MCP tool (e.g. the `filesystem` server) and uses the result;
  schemas from 1–2 servers don't blow up a 16k context; Esc/timeout don't
  hang a turn. NO-GO path: stay on native tools, file MCP as groundwork (the
  probe is cheap).
- **Stage 3 — `feat/mcp-host`** (after GO; **done**, one PR in two parts):
  3a — client/`McpManager`/config/gates/cancellation-timeouts/tests;
  3b — UI (a "Plugins (MCP)" group under "Tools": master toggle + server
  statuses, per-profile toggles, full descriptions in the bottom panel,
  TOFU re-confirmation via Enter), i18n chrome, docs (spec §9.6,
  architecture §8, README, install §4.2), CHANGELOG. Outcome — ADR 0007
  "Plugins: MCP host + import exchange format."
- **Stage 4 (optional, per R5)** — install.md recipes for LiteLLM/OpenRouter;
  managed-custom-command, if demand is confirmed.
- **Groundwork** (in the roadmap): MCP HTTP transport, resources/prompts,
  per-call confirmation for destructive tools, deferred schemas ("tool
  search"), a server UI editor, a WASM sandbox for untrusted tools, server
  `instructions` → the system prompt.

## 8. Forks

> **User's decision (2026-07-17): all forks R1–R8 accepted per
> recommendations.** The track starts with stage 1 (`feat/generic-import`).

- **R1. Tool-plugin mechanism**: **MCP host over stdio (recommendation)** |
  a custom JSON-RPC protocol | in-process WASM (Extism). Recommendation —
  MCP: ecosystem + genre convention; a custom protocol = zero ready-made
  tools.
- **R2. MCP client implementation**: **our own micro-client
  (recommendation)** | `rmcp` (`default-features=false`). See the table in
  §4.3; honestly a close call.
- **R3. Import exchange format**: **a dedicated `mindfork-import.json` v1
  (recommendation)** | domain entities as-is (§5.3, rejected).
- **R4. Fate of `import-lamellama`**: **remove in stage 1
  (recommendation)** | deprecate for 1–2 releases. The private converter
  lives outside this repository either way.
- **R5. Clouds**: **document-only the external+LiteLLM/OpenRouter pattern
  (recommendation)** | + managed-custom-command (the supervisor spins up a
  sidecar proxy) | an engine plugin API (rejected, §6).
- **R6. Configuring MCP servers**: **a section in `settings.json`, edited
  by hand; in the UI — statuses/toggles/description viewing
  (recommendation)** | a separate `mcp.json` (not overwritten by the app,
  but a dual source) | a full UI editor right away (expensive, groundwork).
- **R7. Security strictness for the MCP phase**: **off-by-default master
  gate + double opt-in + catalog TOFU pinning + clips/timeouts
  (recommendation)** | a minimum without TOFU | a maximum with per-call
  confirmation for destructive calls (deferred to groundwork — a new
  modality mid-generation).
- **R8. Server secrets/environment**: **inherit the app's environment + an
  `env` map with *names* of source variables (precedent `api_key_env`; the
  secrets themselves aren't in `settings.json`) (recommendation)** | a clean
  environment with explicit passthrough (stricter, but breaks PATH-dependent
  servers; honestly: hiding env from a process running with the user's own
  privileges is security theater — it can read the files anyway).

## 9. Probe results (stage 2, `spike/mcp-client`) — GO

**Run of 2026-07-17** (live setup: Gemma 4 31B q4, external `llama-server`
`--jinja` on 192.168.1.20:8000; a real third-party MCP server
`npx @modelcontextprotocol/server-filesystem`). The mini-client `shared/mcp.rs`
(transport behind `McpConnection::over` over `AsyncRead`/`AsyncWrite` — protocol
unit tests on `tokio::io::duplex` without processes; `McpClient::spawn` — a
subprocess: `CREATE_NO_WINDOW`, stderr drained to the log, a shutdown ladder).
Smoke `gemma_reads_file_via_mcp_filesystem_server` — a manual mini agentic
loop (mirrors the orchestrator's mechanics: stream → tool_calls → execution →
next round):

- server: `secure-filesystem-server 0.2.0`, **confirmed protocol 2025-11-25**
  (our own version); `npx` on Windows — via `cmd /c` (pitfall §4.6 confirmed);
- **14 tools, schemas ≈ 7 KiB** — the 16k context doesn't blow up
  (criterion 2);
- **the model called `read_text_file` with correct JSON arguments on the
  very first round**, the result went out in the second round, the final
  answer uses the data from the file (criterion 1); 2 rounds, ~10s, a clean
  shutdown;
- timeout/cancellation: `notifications/cancelled` on timeout, a `ping`
  response, `-32601` for unsupported server requests, skipping junk stdout
  lines — covered by unit tests against a fake server (5 of them,
  criterion 3).

**Verdict: GO** — stage 3 (`feat/mcp-host`) unblocked. The probe's client is
the basis for stage 3 (in the probe it's a module under `#[cfg(test)]`, not
compiled into the binary). Refinements for stage 3's plan from the probe's
results: accept any counterpart protocol version (the tools subset is
wire-stable, the 2025-11-25 server replies with our own version); `cmd /c
npx` works, but config must also accept direct exe paths.

## 10. Sources

MCP: [spec 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
(transports / [lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) /
[tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) /
[security best practices](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices) /
[changelog](https://modelcontextprotocol.io/specification/2025-11-25/changelog));
[RC 2026-07-28](https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/);
[rmcp (crates.io)](https://crates.io/crates/rmcp) /
[rust-sdk releases](https://github.com/modelcontextprotocol/rust-sdk/releases);
[Invariant Labs: tool poisoning](https://invariantlabs.ai/blog/mcp-security-notification-tool-poisoning-attacks);
[OWASP MCP Top 10 — MCP03](https://owasp.org/www-project-mcp-top-10/2025/MCP03-2025%E2%80%93Tool-Poisoning);
[CyberArk: poison everywhere](https://www.cyberark.com/resources/threat-research-blog/poison-everywhere-no-output-from-your-mcp-server-is-safe).
Hosts: [goose extensions design](https://block.github.io/goose/docs/goose-architecture/extensions-design/) /
[config](https://deepwiki.com/block/goose/5.3-extension-types-and-configuration);
[Zed context servers](https://zed.dev/docs/assistant/model-context-protocol);
[oterm MCP](https://ggozad.github.io/oterm/mcp/);
[Claude Code MCP](https://code.claude.com/docs/en/mcp);
"too many tools": [Cursor 40](https://forum.cursor.com/t/tools-limited-to-40-total/67976),
[VS Code 128](https://github.com/microsoft/vscode/issues/290356),
[15k tokens of schemas](https://demiliani.com/2025/09/04/model-context-protocol-and-the-too-many-tools-problem/).
Mechanics: [libloading](https://crates.io/crates/libloading);
[abi_stable (dead)](https://github.com/rodrimati1992/abi_stable_crates);
[stabby](https://github.com/ZettaScaleLabs/stabby);
[RFC crABI #3470](https://github.com/rust-lang/rfcs/pull/3470) /
[RFC #[export] #3435](https://github.com/rust-lang/rfcs/pull/3435) /
[export_stable rust#134767](https://github.com/rust-lang/rust/pull/134767);
[Bevy: dynamic plugins unsound](https://github.com/bevyengine/bevy/issues/11969);
[wasmtime tiers](https://docs.wasmtime.dev/stability-tiers.html) /
[WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3);
[Extism](https://github.com/extism/extism);
[Zed extensions (WASM/WIT)](https://zed.dev/blog/zed-decoded-extensions);
[Zellij → wasmi v0.44.0](https://github.com/zellij-org/zellij/releases/tag/v0.44.0);
[LSP base protocol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/);
[nushell plugin protocol](https://www.nushell.sh/contributor-book/plugin_protocol_reference.html);
[go-plugin](https://github.com/hashicorp/go-plugin);
[BatBadBut / Rust 1.77.2](https://blog.rust-lang.org/2024/04/09/cve-2024-24576.html);
[CREATE_NO_WINDOW](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags);
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).
Precedents: [llm-functions](https://github.com/sigoden/llm-functions);
[llm CLI plugin hooks](https://llm.datasette.io/en/stable/plugins/plugin-hooks.html);
[LibreChat custom endpoints](https://www.librechat.ai/docs/quick_start/custom_endpoints);
[Open WebUI Pipelines](https://docs.openwebui.com/features/extensibility/pipelines/);
[LiteLLM](https://github.com/BerriAI/litellm); [OpenRouter](https://openrouter.ai/);
[beangulp (beancount importers)](https://github.com/beancount/beangulp);
[KeePass import/export](https://keepass.info/help/base/importexport.html);
[character card v2 spec](https://github.com/malfoyslastname/character-card-spec-v2);
[parsing ChatGPT conversations.json](https://community.openai.com/t/decoding-exported-data-by-parsing-conversations-json-and-or-chat-html/403144).
