# Journal — Tool system

The `Tool` contract and the tools built on it: files, web search and page fetch, the Python sandbox, MCP servers, speech, YouTube, the control tools, and the confirmation gate in front of the dangerous ones.

**Reference documents for this area:** architecture.md §8, spec.md §9

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (32)

- Post-M9: new tools — files, fetch_url, calculator, date/time (done)
- Post-M9: conversation control tools (followup / rewrite) (done)
- Post-M9: Python sandbox on Wasmer/WASIX — Phase 1 (sidecar scaffolding) (done)
- Post-M9: Python sandbox on Wasmer/WASIX — Phase 2 (asset provisioning) (done)
- Post-M9: Python sandbox on Wasmer/WASIX — Phase 3 (hardening/polish) (done)
- Post-M9: Python sandbox — OS-level memory limit (Windows Job Object) (done)
- Post-M9: Python sandbox — pandas in the starter set (done)
- Post-M9: plugins — stage 1: generic import (the mindfork-import format) (done)
- Post-M9: plugins — stage 2: MCP client probe (GO) (done)
- Post-M9: plugins — stage 3a: MCP host core (done)
- Post-M9: plugins — stage 3b: MCP host (TOFU, UI, i18n, docs, ADR) (done)
- Post-M9: chat message speech (TTS) — OpenAI/Gemini/external (done)
- Post-M9: Python sandbox — beautifulsoup4 in the starter set (done)
- Post-M9: confirmation before dangerous tool calls (done)
- Post-M9: the assistant can watch a YouTube video (`youtube_watch`) (done)
- Post-M9: YouTube stage 2 — the words, as a chat attachment (done)
- Post-M9: `youtube_watch` — a degraded answer has to close the door (done)
- Post-M9: MCP servers in the settings window (done)
- Post-M9: MCP servers — secrets for the `env` map and JSON import (stage 2, done)
- Post-M9: page fidelity for `fetch_url` (+ two neighbouring traps) (done)
- Post-M9: images returned by MCP tools (done)
- Post-M9: an address policy for model-chosen URLs (done)
- Post-M9: cross-chat search for the assistant (`chat_search`/`chat_read`) (done)
- Post-M9: `chat://` as the one address of a conversation (done)
- Post-M9: the code workspace — stages 0 and 1 (done)
- Post-M9: the code workspace — stage 2, editing and the journal (done)
- Post-M9: the code workspace — stage 3, the build/run/test command slots (done)
- Post-M9: the code workspace — stage 5, the semantic index, measured and rejected (done)
- Post-M9: the change journal followed the chat, not the project (done)
- Post-M9: the code workspace's loose ends (done)
- Post-M9: sub-agent chats, PR 2 — the sub-agent with the agent's tools (done)
- Post-M9: `web_search` said "no results" while it was blocked, again (done)

### Post-M9: new tools — files, fetch_url, calculator, date/time (done)
- **Four new tools** (`features/tools/`), all following the existing `Tool`/
  `ToolContext`/`ToolOutcome` contract (no `Chat` mutation, a tool error → result
  text, not a panic):
  - **Files** (`fs.rs`): `fs_read`/`fs_write`/`fs_list` — read/write (with `append`)/
    listing of local files via `tokio::fs`. Under a **new global switch
    `tools.fs_enabled`** (off **by default**, like Python: the tool can
    read/overwrite any file). Optional **sandbox `tools.fs_root`**:
    if set, all paths are canonicalized and must lie inside the root (`FsRoot::
    resolve`: for existing paths — `canonicalize`, for a new file — `canonicalize`
    the parent + the name; `starts_with(root)` catches escape via
    `..`/an absolute path). Limits: reading ≤50k chars, listing ≤500 entries. UTF-8-lossy (there's nothing
    to read a binary with).
  - **`fetch_url`** (`fetch.rs`): page fetch via its own `reqwest` client
    (browser headers) → readable-text extraction (**reuses
    `web::extract_readable`** — made `pub(crate)`: `extract_readable`,
    `truncate_chars`, `USER_AGENT`, `ACCEPT_HTML`, `ACCEPT_LANGUAGE`) → summarization
    through `ctx.engine` as an independent single-turn request (like `call_subagent`: no
    history/tools, a token limit of 768, a 90s timeout). The `focus` argument
    focuses the summary; `summarize=false` → the extracted text without the model;
    a summarization failure → graceful degradation (returns the extracted text). Gated by
    **`web_enabled`** (network access, like `web_search`). Requires `http(s)://`.
  - **`calculate`** (`calc.rs`): its own **recursive-descent evaluator**
    (no new crates) — `eval(&str) -> Result<f64>` (pure, testable):
    a lexer + parser `expr/term/factor/unary/atom`, precedence, right-associative
    `^`, unary minus, `%`, parentheses, exponential notation, constants
    (`pi`/`e`/`tau`) and functions (`sqrt`/`cbrt`/`abs`/`exp`/`ln`/`log`(+base)/
    `log2`/trigonometry/`atan2`/hyperbolic/`floor`/`ceil`/`round`/`min`/`max`/
    `pow`). No I/O, **not gated**.
  - **`current_time`** (`datetime.rs`): local time + UTC (`chrono::Local`/`Utc`,
    default features already include `clock`); a `format` argument — a `strftime`
    string (an invalid spec is caught via `write!`+`DelayedFormat`, no panic).
    No I/O, **not gated**.
- **Reconciling tools for existing profiles** (`features/profiles.rs::
  reconcile_tools`, a new `Profile.known_tools` field, `#[serde(default)]`): profiles
  store a snapshot of `enabled_tools`, so tools added to the application
  **didn't show up** for already-saved profiles (the model only saw the old set and
  kept reaching for `python_exec` for everything — the symptom that surfaced this).
  `reconcile_tools` enables the profile's tools from `default_tool_ids` that it doesn't
  "know" yet (not in `known_tools`), and records them as known; tools **disabled by the user
  are not re-enabled** (they're in `known_tools`). For a profile without `known_tools` (created
  before the registry existed), the base of "known" is the current `enabled_tools`, so only
  truly new tools get added. Called during `bootstrap` (for each
  loaded profile, and on change — `upsert_profile`) and in `profiles::create`
  (a new profile gets the full current set right away). Idempotent.
- **Plumbing**: IDs added to `default_tool_ids` (meaning they automatically appear in
  the profile toggle catalog `screens/settings.rs::tool_catalog`); `effective_tool_ids`
  got a **4th parameter `fs_enabled`** and gates `web_search`+`fetch_url` via
  `web_enabled`, `fs_*` via `fs_enabled` (call sites updated in
  `orchestrator/generation.rs`); `ToolConfig` got `fs_root`, threaded into
  `standard_registry` → `Fs*::new` (from `build_registry`, `orchestrator/mod.rs`).
  `ToolSettings` extended with `fs_enabled`/`fs_root` (`#[serde(default)]` → without migration).
- **Settings UI** ("Tools" section): a "File access" toggle (`FieldId::TFs`)
  and "Files: sandbox directory" text field (`FieldId::TFsRoot`) with description tooltips
  (`field_description`). The safe `calculate`/`current_time` have no switches.
- **Tests**: pure (`calc::eval` — precedence/exponent/functions/errors; `datetime`
  rendering with injected time; `fs` sandbox/roundtrip/append/listing on a tempdir;
  `fetch` refusal on non-http and building a single-turn request with `focus` via a
  capturing backend). Network/real ones — `#[ignore]`. **472 tests green** (+37),
  clippy/fmt clean.

### Post-M9: conversation control tools (followup / rewrite) (done)
- **Two optional tools** (`features/tools/control.rs`, spec §9.3.3),
  letting the model control the structure of its own response:
  - **`send_followup_message`** ("write another message") — the assistant finishes
    the current message, calls the tool, and writes a **second reply** as a separate
    bubble right after the first.
  - **`rewrite_current_message`** ("rewrite my current message") — if partway through it realizes
    it answered incorrectly: the text accumulated in the current round is **discarded**, and
    the next round is written from scratch. The discarded content (assistant + tool) goes into
    `Chat.deleted` (like `Ctrl+E`/`Ctrl+R`) for manual recovery.
- **This is control flow, not an ordinary tool**: the client-side
  agentic loop itself recognizes it (`app/orchestrator/generation.rs`), not `Tool::invoke` (the
  `Tool` implementations only exist for the schema/registration/gating; `invoke` returns the same
  "permission" text that the loop synthesizes). Tool-calling protocol alternation
  isn't broken (`assistant(tool_call) → tool → assistant`), **no synthetic user turn is
  introduced** (by agreement — simpler and more reliable).
- **Optional, off by default**: not in `default_tool_ids` (→ `reconcile_tools`
  doesn't enable them for existing/new profiles), but present in a new catalog
  `all_tool_ids` — `screens/settings.rs::tool_catalog` builds profile toggles from
  it. The loop recognizes a control call **only if it's actually enabled** in
  the profile (`allowed_has`), otherwise — a plain refusal. `effective_tool_ids` doesn't
  touch them (they pass through `_ => true`).
- **A separate bubble** — a `Message.new_bubble` flag (`#[serde(default,
  skip_serializing_if)]` → without migration): `from_messages` (`widgets/message_feed.rs`)
  doesn't merge a flagged message into the previous assistant block. Internal
  tool blocks for control tools in the feed are **hidden** (`from_message` filters
  by `control::is_control_tool`).
- **Live stream ↔ reload**: new events `AppEvent::AssistantContinue`
  (followup: finish the bubble, start a new one) and `AssistantRewrite` (discard
  the accumulated text of the current bubble) — `screens/chat.rs::continue_assistant`/
  `rewrite_assistant`. In the generation loop, `pending_new_bubble` sets `new_bubble` on
  the next domain assistant message; a rewrite round doesn't consume it and puts the
  discarded content into `GenResult.deleted` → `handle_done` calls `chat.record_deleted`.
- Every call = one round (`max_tool_rounds`, a backstop against endless extra messages/
  rewrites). Tests: pure (`control` names/text; `from_messages` two bubbles
  + hiding the control block + regular merging still works), live (`chat.rs`: followup
  two bubbles == reload, rewrite discards the partial), integration for the orchestrator
  (followup → 4 messages with `new_bubble`; rewrite → the discarded content in `deleted`).
  **511 tests green**, clippy/fmt clean.
- **Verified on live Gemma 4 12B and Qwen 3.6 27B** (`llama-server`, `--jinja`): three
  `#[ignore]` smokes green on both — the client-level one
  (`client.rs::control_tools_are_callable`: the model actually calls
  `send_followup_message`) and two end-to-end orchestrator ones through a real
  `OpenAiClient` (`tests.rs::followup_tool_e2e_live` → history `user →
  assistant(followup) → tool → assistant(new_bubble=true)`; `rewrite_tool_e2e_live`
  → the discarded round in `Chat.deleted`, the final history clean). Run:
  `MINDFORK_ENGINE_URL=…/v1 cargo test … -- --ignored --nocapture --test-threads=1`.
- **Observation**: both models readily call `send_followup_message`; a call to
  `rewrite_current_message` is more sensitive to the prompt (Qwen, given a soft "little
  scenario" phrasing, would simply write an incorrect answer without calling the tool — required
  explicitly marking the call as a mandatory step). This is about **model behavior**
  on the counter-natural request "answer incorrectly, then rewrite," not about the mechanism: once
  a model actually calls the tool, discarding/archiving work the same. In real
  use the tool triggers naturally (the model itself realizes it made a
  mistake), and its description is neutral.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 1 (sidecar scaffolding) (done)
- **`python_exec` gained two modes** ([docs/research/python-wasmer-sandbox.md](../../docs/research/python-wasmer-sandbox.md)):
  **Wasmer** (the default) — an isolated WASIX sandbox via a **`wasmer` sidecar**
  (a binary next to the app, not embedded in the exe — the Phase 0 §9.7 decision: embedding V8 in
  a dll would pull LLVM/libclang+a static V8 into our build, whereas killing a sidecar
  process is clean); **Local** — the previous system interpreter. Branch `spike/python-wasmer-sandbox`. User
  decisions: `python_enabled=false` (a master gate, as before), network in the sandbox
  `=true` with a toggle.
- **`shared/sandbox.rs`** — a `SandboxRunner` contract behind a trait (`availability`/`run`;
  `MockSandbox` for tests, the `EngineBackend` pattern) + a real `WasmerSandbox`:
  locates the binary (env `MINDFORK_SANDBOX_WASMER` → `data/sandbox/wasmer[.exe]`), a
  CPython source (env → `data/sandbox/python.webc` → the registry package `python/python`), launches via
  `tokio::process` (`wasmer run --v8 [--net] --volume HOST:GUEST --env … <python> --
  /w/job.py`), captures stdout/stderr, a timeout+`kill_on_drop` (python runs INSIDE
  the wasmer process — in-process V8, killing wasmer itself stops the code). Pure,
  testable `build_wrapper`/`build_args`. **The code wrapper carries a `setsockopt` shim**
  (a Phase 0 finding: WASIX doesn't implement `TCP_NODELAY` → `EINVAL`, while http.client/requests
  always sets it; the shim mutes it → requests/urllib work). The task script lives in a
  temp directory (`JobDir` with auto-cleanup via `Drop`, no `tempfile` runtime
  dependency). `site-packages/` is mounted at `/sp` (PYTHONPATH) when present.
- **`features/tools/python.rs`** — an enum dispatcher over `PythonMode`: the id `python_exec`
  **stays the same** (a stable wire protocol), `description()` varies by mode+network
  (in Wasmer it lists the packages — the model uses it more readily). A shared `format_output_parts`
  serves both paths → the feed's presenter (`present::parse_console`) untouched. A graceful
  "sandbox unavailable" when the binary is missing (the `UnavailableEmbedder` pattern).
- **Config** (`shared/config.rs`): `PythonMode{Wasmer(default)/Local}` (serde lowercase,
  `ALL`/`label`/`cycle` like `FlashAttn`); `ToolSettings += python_mode/
  python_net_enabled(default true)/python_wasm_timeout_secs(default 30)` (all `#[serde(default)]`
  → old `settings.json`s need no migration). `python_enabled` stays `false`,
  `python_path` — Local. `ToolConfig`/`build_registry` pass through the mode/network/timeout +
  `sandbox_dir` (from `Paths::sandbox_dir` = `data/sandbox/`; `JsonStore::sandbox_dir`
  delegates — no edits needed at every `OrchestratorDeps` call site).
- **Settings UI** (the "Tools" section → the "Python" group): a Choice mode field (`TPythonMode`) +
  the `python_enabled` toggle + **mode-driven visibility** (the interpreter path — only
  Local; network+timeout — only Wasmer), via field_spec/catalog (a precedent from managed/
  cloud); description hints. New `FieldId`s: `TPythonMode`/`TPythonNet`/
  `TPythonWasmTimeout`.
- **Tests**: sandbox (`build_wrapper` carries the shim; `build_args` ordering/net/mounting
  HOST:GUEST; `wasmer_in_dir`; availability Ready/Missing; the python fallback to a package);
  python (the Wasmer/Local dispatcher via `MockSandbox`; timeout/unavailability; the output
  format; `description` by mode); config (defaults + `PythonMode` serde/cycle);
  settings (mode-driven visibility of the Python group + cycling the mode). **963 unit tests
  green** (+16), clippy `-D warnings`/fmt clean. A live `#[ignore]` smoke,
  `runs_real_python_in_sandbox`, run against real `wasmer 7.2.0` (Windows):
  `print('hello sandbox')` executed inside the sandbox.
- **Next — Phase 2**: `mindfork sandbox setup` (download `wasmer` + `python.webc` +
  wheels from a lock list: numpy from the wasix index, the requests stack from PyPI), a
  cache of the compiled module, a first-run banner; `#[ignore]` smokes for numpy/requests/
  Cyrillic/timeout. **Phase 3** — resource limits, a "one task" gate, ADR 0005.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 2 (asset provisioning) (done)
- **`mindfork sandbox setup`** — a clap subcommand (`Sandbox{Setup{--force}}`, like
  `backup`/`restore`): installs the Python sandbox into `data/sandbox/` **out of the box**
  (user's decision — auto-download). Its own tokio runtime (network async outside
  the TUI) + a single-instance guard; progress printed to stdout. Idempotent: existing
  assets are skipped, `--force` re-downloads.
- **`features/sandbox_setup.rs`** — a pure core + a thin network layer. Provisioning via
  a **lock list with exact URL+sha256** (`ARCHIVES`/`WHEELS` — consts in the repo, resilient
  to "latest"): (1) the **`wasmer`** binary — a platform tar.gz from GitHub (by
  `std::env::consts::OS/ARCH`), a streamed download with incremental sha256 (the large
  archive isn't buffered), unpacked with `flate2` (pure Rust)+`tar` into `data/sandbox/wasmer-dist/`;
  (2) **`python.webc`** — via `wasmer` itself (`wasmer package download python/python -o … --wasmer-dir`)
  (home/cache under the sandbox, not `~/.wasmer`); (3) **wheels** — numpy from
  `pythonindex.wasix.org` (a native wasix wheel), the requests stack (requests/urllib3/
  certifi/idna/charset_normalizer) from PyPI (`py3-none-any`) → verify sha256 → unpack
  the zip into `site-packages/` **without pip/host Python** (protection against zip-slip via
  `enclosed_name`, as in `backup`). Pure, testable `archive_for`/`verify_sha256`/`hex_lower`/
  `unpack_wheel`/`extract_targz`.
- **Compilation cache** (`shared/sandbox.rs`): `WasmerSandbox::run` sets the env var
  `WASMER_CACHE_DIR=<dir>/cache` — the first run compiles python.wasm (seconds),
  afterward a warm start from the cache, self-contained.
- **A shared binary resolver** `shared::sandbox::locate_wasmer(dir)` (a direct
  `<dir>/wasmer[.exe]` for a manual install → `wasmer-dist/bin/wasmer[.exe]` after
  setup) — a single source of truth about the layout for the runtime (`WasmerSandbox`) and
  provisioning (`sandbox_setup`).
- **Dependencies**: `flate2` (already a transitive dep; miniz_oxide, no C), `tar`, `sha2` —
  all pure Rust.
- **Deferred to Phase 3** (by scope): a first-run banner (compilation progress
  tool→UI — needs an event pipe, the setup command prints progress to stdout); resource
  limits; a "single task" gate; revisiting the `python_enabled` default (setup is now one
  command).
- **Tests**: sandbox_setup (archive selection by platform; `verify_sha256`/`hex_lower`;
  `unpack_wheel` on a crafted zip; `extract_targz` round-trip on a synthetic tar.gz;
  the lock list covers numpy+the requests stack, all URLs https + sha256 64-hex);
  `locate_wasmer` (direct + wasmer-dist, direct takes priority). **969 unit tests green**
  (+6), **39 `#[ignore]`** (+5 live smokes), clippy `-D warnings`/fmt clean.
- **Run against a real rig** (Windows 11, wasmer 7.2.0): `mindfork sandbox setup`
  downloaded and unpacked everything (wasmer 206MB → `wasmer-dist/bin/wasmer.exe` 90MB, python.webc
  44MB, 6 wheels, all sha256 matched). **8 live smokes green** (`MINDFORK_SANDBOX_DIR`
  pointed at the provisioned dir): numpy 2.3.2 (matmul/sum via dynamic linking of native
  `.so`), requests HTTPS 200 (with network access), requests blocked with no network access, Cyrillic,
  timeout kill (`while True: pass` → "exceeded the time limit"), basic sandbox/local.

### Post-M9: Python sandbox on Wasmer/WASIX — Phase 3 (hardening/polish) (done)
- **Locked in by a decision**: [ADR 0005](../../docs/decisions/0005-python-sandbox-wasmer.md)
  (the `wasmer`/WASIX sidecar behind `shared/sandbox.rs`; the security/resource posture;
  defaults). Track outcome (Phases 0–3): the Python sandbox works out of the box.
- **A "one task at a time" gate** (`shared/sandbox.rs`): `WasmerSandbox` gained
  an `Arc<Semaphore>` (1 permit); `run` does a `try_acquire` **before** resolve/spawn
  — a concurrent call is rejected immediately ("the sandbox is busy with another task").
  Defense in depth against process/thread leaks and predictable load (in the normal
  agentic loop calls are already sequential). The permit is held for the duration of `run`,
  released on completion/timeout.
- **Warming the compilation cache at install time** (`features/sandbox_setup.rs::warmup`,
  best-effort at the end of `setup`): a single `import numpy` run through a real
  `WasmerSandbox` compiles `python.wasm` (+ numpy's native `.so`) into
  `<dir>/cache`, so **the first real tool call is warm** — no
  multi-second compilation in front of the user. This **replaces** the planned
  "first-run banner" (no tool→UI progress pipe needed): there's no cold start
  on the first call anymore. A warmup failure doesn't fail the install.
- **No RAM limit — deliberately not done**: the `wasmer` CLI has no memory/fuel/
  metering flag (only `--stack-size` and proposal toggles), and OS-level job objects/
  `setrlimit` are unsafe and platform-specific for a small payoff. The sandbox's resource
  posture: **timeout** (CPU) + **wasm32** (~4 GB address space) +
  the **single-task gate**; a hard RAM cap is groundwork (recorded in ADR 0005/spec §13.2).
- **`python_enabled` stays `false`** (a deliberate opt-in): the sandbox removes
  the original cause, but "enabled without assets" is worse than "disabled"; enabling is a step after
  `sandbox setup`. Recorded in ADR 0005.
- **Docs**: ADR 0005; spec §13.2 rewritten for the two modes + the resource posture, §9.3
  (the `python_exec` entry); install.md §4.1 (setup warms the cache); the ADR list in
  CLAUDE.md/architecture.md.
- **Tests**: the gate (`gate_rejects_second_concurrent_task` — holding the permit →
  rejection before spawn; `gate_permit_released_after_run` — the permit is returned).
  **971 unit tests green** (+2), **39 `#[ignore]`**, clippy `-D warnings`/fmt clean.
  Live run: `sandbox setup` warmed the cache (`import numpy`), smokes green.

### Post-M9: Python sandbox — OS-level memory limit (Windows Job Object) (done)
- **The "hard RAM cap" groundwork item from Phase 3 implemented** for Windows (on request). A new
  setting `tools.python_wasm_memory_mb: Option<u64>` (default **None** —
  opt-in): when set, the `wasmer` process is placed into a **Job Object** with
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY`; exceeding it kills the process — **protects the host
  from OOM** on a runaway script.
- **Spike investigation** (scratchpad, a real wasmer): the Job Object limit works as a
  hard backstop; the failure **isn't graceful** (V8 "Fatal out of memory" on stderr, or a
  Python `MemoryError`), but the host is protected. **Baseline ~768 MB** — V8+CPython need
  that much to start (below it, the sandbox won't start), so a meaningful limit is ≥~1024.
  **The job handle is closed right after `AssignProcessToJobObject`** (the limit holds
  while the process is a job member) → the raw HANDLE isn't held across `await`, the future
  stays `Send`. **Not done on Unix**: `RLIMIT_AS` is unreliable with V8 (it reserves
  a large virtual address space, a low limit breaks startup) — there it's timeout + wasm32.
- **Implementation**: `WasmerSandbox::with_memory_limit(Option<u64>)` + `apply_memory_limit`
  (`#[cfg(windows)]` winapi via the `windows-sys` target dep; `#[cfg(not(windows))]` —
  a no-op with a debug log), called right after spawn. Threaded through `ToolSettings` →
  `ToolConfig` → `build_registry`. A UI field "Memory limit (MB, 0=none)" in the Python group
  (Wasmer mode) with a hint (Windows only, minimum ~1024).
- **Tests**: config (default None); UI (the field shown in Wasmer, hidden in Local). Live
  `#[ignore]`+`#[cfg(windows)]` smokes on the provisioned sandbox: `memory_cap_
  stops_runaway` (a 1 GB limit + a 3 GB alloc → failure, host intact) and `memory_cap_allows_
  normal_work` (a 2 GB limit doesn't get in the way) — **both green live**. **971 unit tests
  green**, **41 `#[ignore]`**, clippy `-D warnings`/fmt clean. ADR 0005 / spec §13.2
  updated.

### Post-M9: Python sandbox — pandas in the starter set (done)
- **pandas added to the `sandbox setup` lock list** (groundwork item "more packages";
  rounds out the planned numpy/pandas/requests starter set and makes the tool
  description accurate — it used to promise pandas, which wasn't there). `pandas 2.3.2`
  (a native wasix wheel) + pure PyPI deps (python-dateutil/six/pytz/
  tzdata) added to `WHEELS` (URL+sha256). **Warmup** switched to `import pandas` (pulls in
  numpy too — the heaviest compile path). **Verified live**: pandas imports
  under WASIX (DataFrame + aggregates; the first import takes ~11s to compile the `.so`, then
  it's warm). Smoke `pandas_in_sandbox` (`#[ignore]`) green; the `lockfile_wheels_*` test
  extended. **971 unit tests**, **42 `#[ignore]`**, clippy/fmt clean. The setup
  download grew ~10 MB (pandas ~8.7 MB + pure dep wheels).
- **Deliberately NOT added**: matplotlib (headless output is stuck inside the sandbox —
  useless), scipy (not in the wasix index yet), polars/PyTorch (announced, not yet
  in the index). Minor groundwork items (validating the minimum memory limit in the UI, a run on
  Linux/macOS) — out of the current scope.

### Post-M9: plugins — stage 1: generic import (the mindfork-import format) (done)
- **The first stage of the "plugin system" track** (research
  [docs/research/plugin-system.md](../../docs/research/plugin-system.md), decision points R1–R8
  accepted by the user 2026-07-17 per the recommendations; branch `feat/generic-import`,
  stacked on `docs/plugins-research` — general roadmap edits, a precedent from the linear
  installer stack). Importing from third-party apps moved from an importer for
  LameLLaMA hardcoded into the monolith to a **neutral documented exchange
  format**: an external (possibly private) converter emits a single JSON file — the app imports
  it with the command `mindfork import <file>`. The pattern "converter → a documented
  file → import" — precedents beancount/beangulp, KeePass, Netscape bookmarks
  (research §5); knowledge of the non-public LameLLaMA leaves the monolith.
- **The `mindfork-import` v1 format** ([docs/import-format.md](../../docs/import-format.md) —
  an external contract): `format`/`version` + `profiles[]` (key/name/language/
  system_message/greeting/character_names/sampling) + `chats[]` (key/profile_key/
  title/dates/messages with user|assistant|system roles) + an optional `settings`
  (sampling/interface). **Deliberately NOT the domain entities as-is** (R3): a dedicated,
  small format doesn't lock the internal schema (`deleted`, `reflected_upto`, …)
  into an external contract. Properties: **idempotency** — id = UUIDv5 of a stable
  `key` (separate namespaces for profiles/chats — the ASCII `mindfork import` + a tag;
  an optional explicit `id` for continuity with a prior import); **strict about
  structure** (a wrong `format`, a version newer than supported → a downgrade guard,
  a duplicate/empty `key`, a reference to a profile not in the file, an unknown role → a clear,
  localized error with coordinates) while **tolerant of extension** (unknown
  fields are ignored — evolution without a bump, mirroring `#[serde(default)]`); a BOM
  is dropped. Sampling parses straight into `SamplingConfig` (unknown knobs
  dropped); interface settings apply **partially** (only fields that are set —
  not overwriting the user's settings; a refinement over the previous wholesale approach).
  A chat's system message: an explicit field takes priority, else the first system message in the
  history (system messages aren't written to history — in mindfork it's separate from the messages).
- **`features/import.rs`** (replaced `features/migration.rs`, which was removed):
  pure `parse_import(json, loc)`/`import_file(path, loc)` → `ImportResult
  {profiles, chats, sampling, interface}`; `main.rs::run_import` as before
  (data_migration before opening storage, upserts, partial settings application).
  **CLI**: the subcommand `import <file>` replaces `import-lamellama <dir>` (R4 —
  remove it right away); running the old command gives **a hint about the
  replacement** (`cli.import.lamellama_removed`), not an "unknown command" error. i18n: 11 new
  `import.*` keys + `cli.*` edits (ru+en; the parity/no-dead/Cyrillic gates
  covered it automatically; a nuance — the test file isn't named `import.json`: the
  gate scanner picks up dotted literals with the bundle prefix `import.`).
- **Tests**: a golden fixture of the format (mirroring the spec's example, with intentional
  unknown fields), idempotency/separate namespaces, an explicit `id`,
  an explicit `system_message` taking priority, all validation errors (+ en localization with no
  Cyrillic), BOM/a missing file; CLI (import/the import-lamellama hint/
  help/column alignment). **1093 unit tests green** (+3: −8 migration,
  +10 import, +1 cli), 48 `#[ignore]`, clippy `-D warnings`/fmt clean.
- **No live engine run required** (a file operation without the engine/memory);
  instead — **a manual smoke against the real binary** in an isolated directory
  (a portable `data/` next to the exe): importing 1 profile + 2 chats → all fields land
  correctly (language/greeting/profile sampling, system from history, thoughts, partial
  settings: `temp=0.7`, `theme=dark`, spellcheck untouched); **a repeat import is
  idempotent** (same UUIDs, overwrite + `.bak`, no duplicates);
  `import-lamellama` — a localized hint; `--help` — even columns.
  Importing real LameLLaMA data (226 chats) — pending a private
  converter (outside this repo; the ready wire types from the old `migration.rs`
  are portable into it as-is).
- **Next**: the `spike/mcp-client` probe (stage 2 of the track, go/no-go on a live
  Gemma 4) → `feat/mcp-host` (stage 3). See the plan in research §7.

### Post-M9: plugins — stage 2: MCP client probe (GO) (done)
- **A probe for the "plugins" track** (stage 2 of the plan, branch `spike/mcp-client` —
  stacked on `feat/generic-import`): a go/no-go check for "a live local model
  correctly calls tools of a real MCP server via our agentic-loop machinery".
  Verdict — **GO** (results in the research doc
  [docs/research/plugin-system.md §9](../../docs/research/plugin-system.md)), stage 3
  (`feat/mcp-host`) unblocked.
- **Mini-client `shared/mcp.rs`** (tools-only, stdio, target revision 2025-11-25;
  in the probe the module is under `#[cfg(test)]` — not compiled into the binary, it
  enters the binary at stage 3): transport is decoupled from the process —
  `McpConnection::over` works over any `AsyncRead`/`AsyncWrite` (protocol unit
  tests on `tokio::io::duplex` without processes), `McpClient::spawn` adds the
  subprocess (`CREATE_NO_WINDOW`, continuous stderr drain to the log — otherwise a
  filled pipe would block the server, a shutdown ladder "close stdin → wait → kill",
  `kill_on_drop`). Protocol: `initialize` (we send 2025-11-25, we accept any counterpart
  version — the tools subset has been wire-stable since 2024-11-05) →
  `notifications/initialized`; `tools/list` with `nextCursor` pagination; `tools/call`
  (text blocks concatenated, non-text → a placeholder, `isError` surfaced); we reply to
  `ping` with an empty result and `-32601` to any other server request (silence would
  hang a well-behaved server); `notifications/cancelled` on request timeout; **garbage
  lines on stdout are skipped with a warn** (field-host pitfall #1), unknown
  notifications are ignored.
- **Protocol unit tests** (5, against a scripted fake server over duplex):
  handshake + pagination + a call; banner garbage before JSON doesn't break the
  connection; ping→an empty result and a sampling request→`-32601` (checking
  client-side responses from the server's point of view); a timeout sends
  `notifications/cancelled` with reason=timeout; a JSON-RPC error surfaces as text.
- **Live smoke — GO** (`gemma_reads_file_via_mcp_filesystem_server`,
  `#[ignore]`; Gemma 4 31B q4, an external `llama-server --jinja` on a nearby machine
  + **a real third-party** `npx @modelcontextprotocol/server-filesystem`): the
  `secure-filesystem-server 0.2.0` server confirmed protocol **2025-11-25**;
  **14 tools, schemas ≈ 7 KiB** — the 16k context stays intact; the model called
  `read_text_file` with correct JSON arguments **on the very first round**, the result
  went out in the second round, the final answer uses the file's content;
  2 rounds, ~10 s, a clean shutdown. The smoke's manual mini-agentic-loop mirrors the
  orchestrator's mechanics (stream → `ToolCallAccumulator` → execution →
  `assistant_tool_calls`+`tool` messages → the next round). The "npx is a
  `.cmd` shim" pitfall was confirmed: on Windows, spawn via `cmd /c npx …`.
- **Refinements for stage 3 based on the probe's findings**: accept any counterpart
  protocol version (the 2025-11-25 server replies with our own version; a strict
  rejection isn't needed); the server config must accept both `cmd /c npx …`
  commands and direct exe paths. **1098 unit tests green** (+5), **49 `#[ignore]`**
  (+1), clippy `-D warnings`/fmt clean.

### Post-M9: plugins — stage 3a: MCP host core (done)
- **Stage 3 of the "plugins" track** (docs/research/plugin-system.md §4.4–§4.7, §7;
  part 3a — the core; branch `feat/mcp-host`, stacked on `spike/mcp-client`): MCP
  servers are launched by the app from config, their tools become full-fledged
  `Tool`s in the registry, called by the standard agentic-loop. UI polish
  (statuses/descriptions in settings, TOFU catalog pinning), i18n chrome, and docs
  (spec/architecture/README/install/CHANGELOG/ADR) — stage 3b.
- **`shared/mcp.rs` graduated from `#[cfg(test)]` into the binary** and grew into a
  host: a monitor task owns the `Child` (`kill`/`exited` tokens, mirroring
  `managed.rs`; the client's `Drop` arms `kill`, a grace period for a self-initiated
  exit after closing stdin); **Job Object kill-on-close** (Windows, borrowed from
  `sandbox.rs`) — a process tree (`cmd /c npx` → `node`) doesn't outlive app
  exit/crash; spawn accepts an **env map** (already-resolved pairs); **`.bat`/`.cmd`
  are forbidden** as the server command (`forbidden_batch_command`, BatBadBut
  CVE-2024-24576 — `cmd /c npx …` and direct exes are allowed);
  `request_cancellable` — cancelling via the token sends the server
  `notifications/cancelled` (reason=cancelled, mirroring the timeout);
  `list_tools`/`call_tool` moved onto `McpConnection` (testable on a duplex without
  processes); `shutdown(self)` simplified to "drop + wait for exited".
- **Config `config.mcp`** (`McpSettings`, `#[serde(default)]` → no migration): a
  master switch `enabled=false` (like Python) + `servers[]` (an `id` slug /
  `command` / `args` / an `env` map of the **names** of source env vars — secrets
  never enter `settings.json`, per R8 / `enabled` / `tool_timeout_secs`=60 /
  `max_result_chars`=20k).
- **Wrapper `McpTool: Tool`** (`features/tools/mcp.rs`): id
  `mcp__<server>__<tool>` (a sanitizer `[A-Za-z0-9_-]`, ≤64 — truncation + a hex
  tail derived from the full name against collisions); the description/schema —
  **a server snapshot, not localized** (an i18n boundary, like engine probe
  errors); `invoke` → `tools/call` with a per-call timeout and `ctx.cancel`; the
  result **is clipped** (`max_result_chars`, a localized marker
  `tool.mcp.result_truncated`); `isError` with empty text → a clear marker.
  Group `ToolGroup::Plugins` (ALL=9) + gate `ToolGate::Mcp`;
  `enabled_by_default=false` (**double opt-in**, per R7 — `reconcile_tools`
  doesn't auto-enable it); labels are interned as `&'static str`
  (`ToolInfo.label`; a precedent — interning i18n language codes).
- **`effective_tool_ids` += `mcp_enabled`**: `mcp__*` tools are gated by the
  master switch **by prefix** — they're dynamic, absent from the static `CATALOG`
  (a `CATALOG` lookup is impossible). A 6th parameter, all call sites updated.
- **Cancellation in `ToolContext`** (a small refactor, useful for native tools too):
  `TurnInfo`/`ToolContext` now carry `cancel: CancellationToken` (a clone of the
  generation task's / background loop's token); the agentic-loop wraps
  `registry.invoke` in a **`select!` with the token** — Esc isn't blocked by a
  long-running tool (MCP/network); a cancelled call yields the result
  `loop.tool_cancelled`, after the round the loop ends the turn as `Cancelled`
  (accumulated output preserved). The MCP call itself notifies the server
  (`notifications/cancelled`) in that case; for other tools the select is a
  safety net.
- **`McpManager`** (`app/orchestrator/mcp.rs`, mirroring `EngineManager`): spawns
  enabled servers as **background tasks** (command handlers stay synchronous) →
  events `Ready{conn,tools}`/`Failed{reason}`/`Exited` into the loop's internal
  channel `run`; **an epoch guard** — late events from shut-down servers (a
  "died before cancel" race) are dropped; config validation before spawn (slug
  id, non-empty command, batch ban) → `Disconnected(reason)`; a missing source
  env variable — warn+skip. **A restart budget** (the VS Code LSP pattern): a
  crash after readiness → restart, but ≤3 per 5 min, beyond that —
  `Disconnected` until settings are edited; a startup failure (`Failed`) isn't
  restarted (likely a config error). `rebuild_registry()` — the **only** path
  for rebuilding the registry (standard set + live MCP wrappers; previous
  `build_registry` call sites migrated); `config.mcp` edits go through the
  `RestartQueue.mark_mcp` debounce (a 4th flag); `Quit` shuts servers down
  (dropping slots → cancel → the shutdown ladder).
- **A dynamic catalog in the UI**: `AppEvent::Settings` += `mcp_tools:
  Vec<ToolInfo>` (a snapshot from the manager; `ChatScreen.settings_snapshot`
  became a 4-tuple `SettingsSnapshot`); `SettingsScreen::tool_catalog()` — an
  instance method (the static catalog + an MCP tail, `PTool` indices for the
  static part are stable; `default_fields` copies the MCP snapshot into the tmp
  screen). MCP tool toggles — grouped in the profile under "Plugins (MCP)" with
  an **honest gate** ("disabled globally: MCP servers"); the master toggle "MCP
  servers" — in the "Tools" section (`FieldId::TMcpEnabled`). Server statuses on
  screen — stage 3b.
- **Tests**: client (cancel → `notifications/cancelled`; `call_tool`
  concatenation/non-text placeholders; a batch-command ban, incl. in `spawn`);
  config (a partial server entry → defaults, round-trip); McpTool (the id
  sanitizer — short/long with a hex tail; character-based clipping +
  localization; invoke over a duplex fake; cancellation via `ctx.cancel`);
  manager (slug validation; the restart budget under paused time; Ready builds
  the catalog + a foreign epoch is dropped; Exited clears tools and respects the
  budget; a disabled/invalid server); orchestrator (Ready → a wrapper in the
  registry + `mcp_tools` in the Settings snapshot; Exited removes it from the
  registry); settings (MCP toggles in the profile with a gate; the master
  toggle). **1120 unit tests green** (+22), **50 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4, external `llama-server` + a real
  `npx @modelcontextprotocol/server-filesystem`): `mcp_filesystem_e2e_live` —
  **the full path through the orchestrator** (config with a server → `Ready` →
  profile toggles → a turn): 14 tools in the catalog, the model called
  `mcp__fs__read_text_file` and named the secret number from the file; the
  probe's client-level smoke (`gemma_reads_file_via_mcp_filesystem_server`) is
  green on the new API (protocol 2025-11-25, 2 rounds, ~5–8 s). A side lesson
  from the test: `wait_for` drains events — `ProfileList` (bootstrap) has to be
  pulled **before** waiting for the later Settings that carries the MCP catalog,
  otherwise the test hangs.
- **Deliberate boundaries of 3a** (→ 3b/future work): server statuses and
  viewing full tool descriptions in the UI, TOFU catalog pinning (a rug-pull
  detector), localization of status reasons (currently Russian, a technical
  layer), `notifications/tools/list_changed` (a catalog re-listing), non-text
  result blocks (placeholder), a per-server tool ceiling.

### Post-M9: plugins — stage 3b: MCP host (TOFU, UI, i18n, docs, ADR) (done)
- **Completion of the "plugins" track** (same branch `feat/mcp-host`, on top of
  3a): MCP host security and observability + a full docs package. The track
  (stages 1–3) is closed by **ADR 0007** "Plugins: MCP host + a neutral import
  exchange format"; behavior — spec **§9.6** (a new section).
- **TOFU catalog pinning** (a rug-pull detector / tool poisoning, per R7):
  `catalog_hash` (`features/tools/mcp.rs`) — sha256 over sorted (name+
  description+JSON schema) tools; determinism via sorting by name + ordered
  `serde_json` keys (BTreeMap). The pin — `McpServerConfig.pinned_catalog`
  (`#[serde(skip_serializing_if)]`; written by the **orchestrator**, not the UI;
  manually removing the field = resetting trust). Flow: first startup →
  auto-pin (`McpEventOutcome.pin` → `persist_mcp_pin` — a direct config write,
  NOT through `handle_update_config`, otherwise diffing `config.mcp` would loop
  restarts); a match → registration; **a mismatch** → the catalog is held in
  the slot (`PendingCatalog{conn,tools,hash}`), the tools are NOT handed to the
  model, status "catalog changed — please reconfirm". Confirmation: Enter on the
  server row → `SettingsIntent::ConfirmMcpCatalog` →
  `AppCommand::ConfirmMcpCatalog` → `McpManager::confirm` (registers + persists
  the new pin). A guard against a UI race: `handle_update_config` **inherits
  pins** by server id from the previous config when the UI snapshot doesn't
  carry them (a stale copy doesn't reset trust or trigger a false restart).
- **MCP host snapshot in the UI**: `McpSnapshot { tools: Vec<ToolInfo>, servers:
  Vec<McpServerSnapshot{id,status,tool_count,pending_catalog}> }`
  (`features/tools/mcp.rs` — FSD-clean for screens) replaced the bare
  `mcp_tools` in `AppEvent::Settings`; `handle_mcp_event` now emits settings on
  **any** event (live statuses), the registry only rebuilds when the catalog
  changes. `ToolInfo` gained `description: Option<String>` — the **full**
  description of an MCP tool (the server's own text; built-in tools have
  `None` — their descriptions live in the bundles).
- **Settings UI**: in the "Plugins (MCP)" group of the "Tools" section — server
  status rows (`ready · tools: N` / `connecting…` / a failure reason;
  read-only `FieldId::TMcpServer(idx)`); a server whose catalog has changed —
  warn + a hint "Enter: confirm". MCP tool toggles in the profile show the
  **full description** in the bottom panel on focus (a tool-poisoning antidote
  — descriptions go into the system prompt). For this, `FieldRow.description`
  was changed from `Option<&'static str>` to `Option<String>`
  (`describe(impl Into<String>)`; ~3 consumers adjusted) — dynamic server text
  isn't interned.
- **i18n of status reasons** (axis B): `McpManager::apply`/`handle_event`/the
  spawn task gained `loc` — config validation, the restart budget, "catalog
  changed", and `Failed` contexts — pulled from the bundles (`ui.err.mcp.*`,
  ru+en); server-row text — `ui.settings.mcp.*`. **Boundary**: the client's
  wire errors (`shared/mcp.rs`) — a technical layer (like HTTP client
  wrappers, i18n-cli §7), tool descriptions/schemas — server text (not
  localized).
- **Docs**: spec §9.6 (a full description of the subsystem), architecture §3 (a
  map: `shared/mcp.rs`, `orchestrator/mcp.rs`, `features/tools/mcp.rs`) + §8
  (the "Plugins (MCP)" group in the table, an implementation bullet), README (a
  feature block), install.md §4.2 (configuring servers with a jsonc example,
  `cmd /c npx`, TOFU), CHANGELOG (Added + Security), roadmap (MCP future work:
  HTTP transport, resources/prompts, list_changed, deferred schemas, a UI
  editor, …), ADR 0007 (+links from CLAUDE.md/architecture), status in the
  plugin-system.md research doc.
- **Tests**: manager (first startup auto-pins + returns the pin to persist; a
  changed catalog is held → confirm registers and returns the hash; a matching
  pin — no re-persisting; descriptions in `ToolInfo`); orchestrator (the pin
  persists to `settings.json`; a changed catalog doesn't reach the registry
  until `ConfirmMcpCatalog`; pin inheritance under a stale UI snapshot);
  settings (server rows + Enter confirmation only when pending; a full
  description in the toggle row). **1125 unit tests green** (+5 over 3a),
  **50 `#[ignore]`**, clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemma 4 31B q4 + a real `npx server-filesystem`): the e2e
  `mcp_filesystem_e2e_live` was rerun after 3b — TOFU auto-pin on the happy
  path is transparent (14 tools, `mcp__fs__read_text_file`, 7319 in the
  response, ~7 s); the TOFU negative path (a changed catalog) is covered by
  unit/integration tests (a live simulation of a server swap isn't needed —
  the mechanics are purely client-side).

### Post-M9: chat message speech (TTS) — OpenAI/Gemini/external (done)
- **The track was restored from a reverted stage 1** (`git show 19ead9f` —
  the `pulldown-cmark` extractor, the `/tts` command, the `rodio` player,
  stop points, the settings tab, clients) after **a re-investigation via a
  live spike** ([docs/research/tts.md §13](../../docs/research/tts.md)): the
  previously implemented engines didn't deliver acceptable quality on real
  text, so the track was reverted (PRs #197–199), then candidates were
  checked on live hardware. **DECISION (2026-07-23): the primary engine is
  OpenAI TTS** (`gpt-4o-mini-tts`, default voice **`onyx`** — verified by
  the spike: ~1 stress error per paragraph, better than every local
  option; loanwords handled natively; no drift). The key product argument:
  **not a new vendor** — the OpenAI key is already stored (ADR 0008), no
  new sign-up needed (ElevenLabs/Azure would require one). **The stage's
  scope — OpenAI + Gemini + external** (three `TtsMode` modes); **no
  local sidecar**. Forks D1–D9 (§10) remain valid behaviorally; only the
  engine choice was revisited.
- **A local managed sidecar — future work, not "stage 2"**: the spike
  showed local engines are either NO-GO for Russian (Qwen3-TTS, Supertonic
  3 — wrong stresses, an accent/drift), or need their own Rust frontend
  (vosk-tts — GO with caveats: 22 kHz, a custom Russian frontend needed
  for a Python-free distribution). For an offline/non-OpenAI audience —
  the `external` mode (any OpenAI-compatible TTS server:
  Kokoro-FastAPI/speaches/LocalAI). The managed mode wasn't added to the
  selector (precedent: `Claude` before `AnthropicClient` — we don't ship a
  non-functional entry).
- **Config `TtsSettings`** (`shared/config.rs`, field `AppConfig.tts`), all
  `#[serde(default)]` — no migrations: `mode: TtsMode
  {OpenAi(default)/Gemini/External}`; `openai`/`gemini: TtsCloudSettings`
  (model/voice/instructions/api_key_env/url); `external:
  TtsExternalSettings` (url/model_name/voice/api_key_env); `speed:
  f32=1.0`; `speak_roles=false`; `stop_on_chat_switch=true`;
  `stop_on_generation_start=false`. Defaults: OpenAI
  `gpt-4o-mini-tts`/`onyx`, Gemini `gemini-2.5-flash-preview-tts`/`Kore`.
  The cloud key is **shared with chat** (ADR 0008), the env-variable
  fallback preserved. **`speed` is effectively ignored by
  `gpt-4o-mini-tts`** (a known defect) → speed is requested via words in
  `instructions`.
- **Clients** (`shared/tts/`): `openai.rs` — `POST {base}/audio/speech`,
  serving **both** the OpenAI cloud (format `pcm`, s16le 24 kHz, bypassing
  a decoder) **and** a third-party server (format `wav`, the most
  portable); the `instructions` field only for the cloud; unset fields
  aren't sent (`skip_serializing_if`). `gemini.rs` — native
  `generateContent` with `responseModalities:["AUDIO"]` (the same
  transport as `GeminiClient`), PCM from `inlineData`; **a mandatory
  read-aloud directive** (default `Read this text aloud verbatim`) to
  avoid the `400 Model tried to generate text…` error. Known Gemini
  issues: 503 overload and voice drift across chunks (hence an opt-in, not
  the default). Input limits: OpenAI 4096, Gemini/External 2000
  characters.
- **Speech extractor** (`shared/markdown/speak.rs`,
  `speakable_text(markdown, loc)`) — a second walker over `pulldown-cmark`
  events (ADR 0003), `Writer` untouched: code blocks, ` ```mermaid `,
  tables, display math ($$…$$) **are skipped with a voice marker** in the
  profile's language (axis A); inline code is read as text, inline math →
  `latex_to_unicode`, links → text (the URL is dropped). Thoughts (CoT)
  and tool blocks aren't spoken (they're absent from `Message.text`).
  Markers/role prefixes are in the **profile's language** (axis A, speech
  content, not UI chrome).
- **Player** (`shared/tts/playback.rs`, `rodio`): the `Player::append`
  queue gives a pipeline of "synthesizing N+1 while N plays" for free;
  headless/CI (`open_default_sink` → `Result`) — graceful degradation
  "audio unavailable", not a panic (the `UnavailableEmbedder` pattern,
  test `open_degrades_gracefully_without_audio_device`).
- **Orchestration** (`app/orchestrator/tts.rs`, mirroring `rag.rs`): the
  orchestrator (the owner of `Chat`) takes a conversation snapshot →
  selection by scope (`build_utterances`, a snapshot at command time —
  works while generation is running too, no generation gates) → chunking
  (`chunk_utterances`/`pack_sentences`: sentences from
  `rag::split_sentences` are packed into a chunk up to the provider's
  limit, a short message → **one request** — no "growing pauses" as seen
  in the spike; packing follows speech blocks) → a cancellable background
  task (`tts_cancel: CancellationToken`, cancelled by a new command/
  `Quit`). **Stop points**: per setting — switching chats
  (`stop_on_chat_switch`) and starting generation
  (`stop_on_generation_start`); **unconditionally** — deleting an
  exchange (`Ctrl+E`), regeneration (`Ctrl+R`), deleting a chat (the text
  being spoken no longer exists). Contract: `AppCommand::Tts(TtsScope)`/
  `TtsStop`, `AppEvent::TtsActive(bool)`.
- **UI**: a command `/tts`/`/tts N`/`/tts all`/`/tts stop` (parser
  `features/tts_command.rs`, case-insensitive, `TtsScope
  {Last/Recent(n)/All}`; an error carries **the argument itself**, the
  hint text is built by the UI locally), command highlighting in the
  input box, the help overlay, a quiet chip **"♪ speaking"**
  (`ui.status.speaking`, the note U+266A is WGL4 → unaffected in
  compatibility mode), a **"Speech"** tab in the "Model" settings
  section: an "Engine" group (mode-driven fields
  engine/voice/speed/instructions; the key via the shared provider-status
  field from ADR 0008) + a "Behavior" group (toggles
  `speak_roles`/`stop_on_chat_switch`/`stop_on_generation_start`).
- **Dependencies**: `rodio` (+`cpal`/`symphonia`, MPL-2.0 already in
  `deny.toml`'s allowlist) and `base64`. A new **system** dependency —
  ALSA on Linux: build-time `libasound2-dev` (apt steps in
  `ci.yml`/`release.yml`/`packaging.yml`), runtime `libasound2t64 |
  libasound2` (deb; a time_t64 trap — a virtual dependency would
  otherwise pull in an OSS shim with no symbols) / `alsa-lib` (rpm/arch,
  installed explicitly in the Arch smoke — `pacman -U` doesn't pull
  dependencies).
- **Tests**: the command parser, the speech extractor (a golden test on a
  representative LLM response + a per-locale gate), clients (wire shapes,
  Gemini parsing, sample rate from mimeType, blocking), PCM→f32
  conversion + graceful player degradation, message
  selection/prefixes/chunking, orchestration (failures with an
  unconfigured engine, stop points, `/tts stop` idempotency, a late-`done`
  race), settings. **1258 unit tests green**, clippy
  `-D warnings`/fmt/`cargo deny` clean (checked at restoration time). Live
  `#[ignore]` smokes (OpenAI/Gemini synthesis, a third-party server, tone
  playback, an e2e run through the orchestrator) — carried over from the
  verified stage 1 (they were passing before the revert). **The live
  smoke `openai_synthesizes_russian_speech_live`** was rerun in this
  session against the real OpenAI API — GO (300 KB of PCM). OpenAI
  `onyx`'s quality was confirmed by the spike (§13.9, direct API calls).
  An interactive `/tts` TUI run wasn't performed in this session (it
  needs a real terminal) — the command/orchestration are covered by unit
  tests.
- **Pause/resume `/tts pause`/`/tts resume`** (at the user's request
  after a live run — for long text): `rodio::Player::pause`/`play`. A key
  shift — the device is now opened by **the handler** (`handle_tts`,
  still lazily, "on command"), wrapped in `Arc<Playback>` (`Send+Sync`)
  and **shared with the background task**; the orchestrator holds a
  handle (`tts_playback: Option<Arc<Playback>>`) → pause/resume apply
  **instantly**, without waiting for a poll. The task's loops **were
  unchanged**: while paused, the queue isn't drained, so the pipeline
  itself holds back synthesis, and `done` isn't emitted
  (`is_drained=false`) until resumed. `stop_tts`/`handle_tts_done` clear
  the handle (dropping closes the device); `stop_tts` clears the pause
  too (otherwise a cancelled task would never drain). `Playback::open`
  moved into the handler: no audio → no task (previously the task would
  start and then fail). Contract: `TtsCommand::Pause/Resume` →
  `ChatIntent`/`AppCommand::TtsPause/Resume` →
  `handle_tts_pause/resume` (no-op with no speech active). i18n: key
  `ui.help.tts_pause`, `ui.tts.bad_arg` updated. Tests: the parser
  (pause/resume + case), a no-op with no device (audio unavailable in CI
  → the `tts_playback=None` path). **1255 unit tests**, clippy/fmt/i18n
  gates clean.
- **Distinct user/assistant voices for `/tts all`** (at the user's
  request after a live run): a new field `voice.user_voice` in
  `TtsCloudSettings`/`TtsExternalSettings` (`#[serde(default)]` → no
  migration). When a separate "User voice" is set for the active mode and
  it **differs** from the assistant's voice, **a second engine** is built
  (`engines_from_config → (assistant, Option<user>)`, a type alias
  `TtsEnginePair`; reuses model/key/base-URL resolution via
  `build_engine(voice_override)`). The pipeline now **carries the role**:
  `build_utterances`/`chunk_utterances` work with
  `Vec<(MessageRole, String)>`, and the task picks the engine by the
  chunk's role (`user_engine.unwrap_or(engine)` for user turns). Voice is
  a property of the engine at creation time (not of the request), hence
  two instances rather than a `synthesize` parameter (minimizing
  trait/client changes). `user_voice` is **independent of
  `speak_roles`** (voices differ acoustically, prefixes are verbal); when
  `None`, everything uses one voice (the previous behavior). A config
  helper `TtsSettings::active_voices() -> (assistant, user)` reads the
  active mode's fields, empty = `None`. UI: field "User voice"
  (`FieldId::TtsUserVoice`) in the "Speech" tab (both the external and
  cloud branches) + i18n `ui.settings.{field,desc}.tts_user_voice`.
  `engine_from_config` was removed (orphaned). Tests: `active_voices` (per
  mode/empty), `engines_from_config` (a second engine only when set and
  different), `chunk_inherits_role`, pipeline tests updated to tuples.
  **1258 unit tests**, clippy/fmt/i18n clean. Live multi-voice testing is
  left to the user (it needs a terminal + audio).
- **Future work** (roadmap): a local sidecar (vosk-tts + a custom Russian
  frontend, `mindfork tts setup` modeled on ADR 0005) + an ADR summarizing
  it; stitching chunks across speech boundaries (even smoother
  multi-paragraph speech), SSE streaming within a chunk, an audio cache,
  auto-picking a voice by message language, reading tables row by row,
  speaking a selected message from the feed, multi-speaker (Gemini), OS
  TTS.

### Post-M9: Python sandbox — beautifulsoup4 in the starter set (done)
- **beautifulsoup4 added to the `sandbox setup` lock list** (branch
  `feat/sandbox-beautifulsoup`, user's request; the same mechanical change as the
  earlier pandas addition, ADR 0005 §4 — no architectural decision, so no design
  doc). HTML parsing is the natural companion to the already-present `requests`:
  fetch a page → parse it, without the model having to hand-roll regex over markup.
- **Three pure-Python wheels from PyPI** (`py3-none-any`, exact URL + sha256, as
  everything else in the lock list): `beautifulsoup4 4.15.0` + its runtime
  dependencies `soupsieve 2.9.1` (CSS selectors — `soup.select`) and
  `typing_extensions 4.16.0`. Nothing native → no wasix index involved, and
  **`warmup` was deliberately not touched** (it exists to compile native `.so`
  files; pure Python has nothing to compile — unlike the pandas addition, which
  switched warmup to `import pandas`).
- **The `dir` field is the name in `site-packages`, not the distribution name** —
  verified by actually listing the wheels rather than assuming: `bs4`,
  `soupsieve`, and `typing_extensions.py` (a single module file, like the
  existing `six.py` entry). Getting this wrong would silently break idempotency
  (the directory check would never hit, so every `setup` run would re-download).
  The sha256 of all three was verified against a real download.
- **The tool description** (`tool.python_exec.desc.wasmer`, ru+en) now names
  beautifulsoup4 — this is what the model reads, so an unlisted package is a
  package it won't reach for; "scientific packages" was also reworded to
  "preinstalled packages" (bs4 isn't scientific).
- **Tests**: the `lockfile_wheels_*` gate extended with `bs4`/`soupsieve`/
  `typing_extensions.py`; a live `#[ignore]` smoke `beautifulsoup_in_sandbox`
  (parses HTML offline — no network needed — and calls `soup.select`, which
  exercises soupsieve, the dependency most likely to be missing). **1294 unit
  tests green** (count unchanged — the new test is `#[ignore]`), **59
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan` clean.
- **Live run — GO** (real `wasmer` 7.2.0 + the provisioned dev sandbox):
  `sandbox setup` skipped everything already installed and downloaded/unpacked
  exactly the three new wheels (idempotency intact), warmup passed; the new smoke
  printed `bs4 4.15.0` / parsed text / one CSS-selector match. **No regression**:
  all 14 `python::tests` sandbox smokes green (numpy/pandas/requests with and
  without network/timeout/memory cap/en localization) — adding
  `typing_extensions` to a shared `site-packages` didn't shadow anything.
  (`runs_real_python_in_sandbox` needs `MINDFORK_SANDBOX_WASMER`/`_PYTHON` on top
  of `MINDFORK_SANDBOX_DIR` — a known, deliberate quirk, see the english-source
  migration entry.)
- **Setup download grew by ~190 KB.** Deliberately **not** added: `lxml` (needs a
  native wasix wheel — not in the index), `html5lib` (an extra parser on top of
  the stdlib `html.parser` bs4 already uses).

### Post-M9: confirmation before dangerous tool calls (done)

- **Human-in-the-loop before a tool call that changes something outside the app**
  (roadmap §Tools, one of the five "most valuable next"). Design plan with forks
  F1–F8 — [docs/history/tool-confirmation.md](../../docs/history/tool-confirmation.md),
  accepted by the user as recommended 2026-07-30 with one addition: the feature
  must be **fully** switchable off. Behaviour — spec §9.8. Branch
  `feat/tool-confirmation`, one PR.
- **Reading the code first moved the work.** The gate itself is trivial:
  `generation.rs` has exactly one invocation site, already wrapped in a `select!`
  with the turn's cancellation token and already carrying three "do not run it"
  branches (disabled / control tool / discarded rewrite round) whose results are
  localized text. What had to be *designed* is that **the agentic loop is a
  background task that only emits events** — every existing internal channel
  (`title_tx`, `imp_done`, `bg_done`) runs task → orchestrator, and nothing yet
  sends anything back *into* one. That is fork F8 and the only architectural
  decision here: the orchestrator holds the in-flight turn's confirmation sender
  and routes `AppCommand::ConfirmTool` into it.
- **Two staleness guards, both of which would otherwise run a tool nobody looked
  at**: a reply whose `generation_id` is not the turn in flight (the user answers
  at the exact moment a turn is cancelled and the next begins — the same guard
  `AppEvent::TokenUsage` already uses), and, within a turn, a reply matched to its
  `call_id` (the model made several calls in one round and they were answered out
  of order). The wait is a `select!` against the cancellation token, so `Esc`
  works with the popup open, and a closed channel reads as a refusal rather than
  as approval.
- **What counts as dangerous is declared by the tool** (`Tool::danger()`, default
  `false`), like `group`/`ui_label`/`gate` — the single-source-of-truth decision
  the project already took for tool metadata. `python_exec`, `fs_write` and
  **every** MCP tool; not reads, not writes to our own storage (notes,
  self-model, RAG, attachments — visible in the UI, profile-scoped, reversible).
  The default is `false` precisely *because* the switch is opt-in: a wrong `false`
  costs a confirmation someone wanted, while a wrong `true` on `current_time`
  would train them to press `Enter` without reading.
- **MCP annotations are deliberately not parsed.** `destructiveHint`/
  `readOnlyHint` are server-supplied, i.e. untrusted: they could only ever
  *relax* a decision, which is the attack. Treating every MCP tool as dangerous
  is both simpler and safer, and the noise is bounded by the feature being
  opt-in and by "allow for this turn".
- **Declining does not cancel the turn**: the model is told (axis A) and the loop
  carries on, so it can explain itself or take another route — ending the turn
  would throw away the text already streamed. `Esc` declines; a second `Esc`,
  with the popup gone, cancels as it always did.
- **An FSD correction caught mid-implementation**: `ToolDecision` was first put
  next to `AppCommand`, but the popup that produces it lives in `screens`, which
  may not import `app`. Moved to `features/tools/confirm.rs` — the same reason
  `RagProgress` lives in `features`.
- **The popup shows the call through `present.rs`** — the same formatting the feed
  will show for it afterwards, so `python_exec` reads as code rather than as a
  JSON blob; long arguments cut with "…" (a decision prompt, not a viewer). It is
  checked **before** the generation gate in the key routing, because unlike every
  other popup it is open precisely while the turn runs.
- **Sub-agents were used at the user's request to save context**: one took the
  setting end to end (config field, settings row, `field_spec` entry, locale keys,
  tests), one took the chat-screen tests. The delicate halves — the channel, the
  gate, the orchestrator integration tests — were kept in the main session. One
  concurrency lesson: an agent editing `screens/chat/**` silently reverted a
  section-number fix applied there mid-flight, so edits to a file an agent owns
  have to be re-applied after it finishes.
- **A section-number collision worth noting**: `§9.7` was already chat file
  attachments, so this became **§9.8** — the references written during
  implementation had to be renumbered, and a `grep` by keyword rather than by file
  was needed to avoid renumbering the attachment ones.
- **Tests**: 6 orchestrator integration tests through the real `run` loop
  (approve, decline, allow-for-turn, a safe tool never asked about, the setting
  fully off, and both staleness guards — each bogus reply says *deny*, so
  honouring either shows up as an unwritten file), plus chat-screen tests for the
  popup and settings tests for the switch. `fs_write` is the tool under test
  rather than `python_exec`: dangerous by the same rule, and whether it ran is a
  fact on disk rather than a sandbox that has to be provisioned.
- **1647 unit tests green** (+14), **70 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A defect the sub-agent found in its own test, by mutating rather than by it
  passing**: ratatui's `Buffer` `Debug` prints row content **without escaping
  quotes**, so an assertion containing `"` against `format!("{:?}", buffer)` can
  never match — a render test written that way passed even when the popup showed
  raw JSON. Rewritten to join rows by hand. Worth remembering for any future
  buffer assertion carrying a quote.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`,
  `--jinja`): `tool_confirmation_e2e_live` passed first time — the real model
  reached for `fs_write` on its own, the confirmation arrived carrying the actual
  arguments (`{"content":"ZARYA-5150","path":"note.txt"}`), `Allow` let it
  through and the file was written. That is the half the mocked tests cannot
  cover: that a real model calls the tool at all, so the confirmation appears on
  the path users take rather than only when a fixture forces the call.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (577 s) —
  memory/self-model/notes/graph/cross-organ links/RAG/attachments/control
  tools/i18n/TTS — plus the MCP smoke (`mcp_filesystem_e2e_live`, 14 tools,
  `mcp__fs__read_text_file`). The full set is the right scope: the tool-invocation
  path sits on **every** turn, and MCP tools are now all marked dangerous, so the
  default-off path had to be shown to leave them untouched.

### Post-M9: the assistant can watch a YouTube video (`youtube_watch`) (done)

- **Asked for as "research the possibility of integrating with YouTube, so the
  assistant can get a description of what is talked about **or shown** in a
  video"** — and that "or shown" turned out to be the whole story. Research with
  forks R1–R9 — [docs/research/youtube-integration.md](../../docs/research/youtube-integration.md)
  (**user's decision 2026-08-01, all as recommended**); behaviour — spec §9.9.
  Branch `docs/youtube-research` (research + stage 1).
- **Everything load-bearing was measured live from this machine before any
  design**, and two results decided the shape:
  - **Every free caption path is closed.** The watch page still hands out a fully
    **signed** `timedtext` URL, and that URL returns **HTTP 200 with a zero-byte
    body** — tried bare, `&c=WEB`, `&fmt=srv3`, `&fmt=vtt`, `&fmt=json3`, with
    browser `User-Agent` + `Referer`. That is YouTube's PoToken gate, and it is
    **not** a datacenter-IP problem: this was a residential IP, the sympathetic
    case. InnerTube `player` gives `UNPLAYABLE` on WEB/MWEB and `400
    FAILED_PRECONDITION` on ANDROID/IOS; `get_transcript` 400s;
    `captions.download` requires the **video owner's** OAuth by design. The Rust
    crates on that surface (`yt-transcript-rs`, `ytranscript`, `ytt`) inherit it —
    a dependency moves where the failure prints, not whether it happens.
  - **Gemini ingests a YouTube URL directly**, through the same
    `v1beta:generateContent` the project already speaks: a 20 s clip = **2098
    prompt tokens in 3.4 s**; the full 213 s video = **22 050 in 8.0 s**; ~103
    tok/s at low detail (audio 32 exactly, video ~71). Verified across
    `2.5-flash`, `3.1-flash-lite`, `3.6-flash`; `watch?v=`/`youtu.be`/`shorts`
    accepted verbatim. OpenAI's Responses API and Anthropic take **no** video —
    checked against primary sources, after a blog claiming a "Claude video API"
    turned up in search and proved to be invented.
- **So the capability is Gemini-only, which is exactly why it must not go through
  the chat engine.** Putting a media part into the provider-agnostic
  `ChatRequest` (ADR 0004's boundary) would hand video only to users whose *chat*
  engine happens to be Gemini — i.e. deny it to the local-model users most likely
  to want it. Instead: a tool with its own client under `shared/video/`, called
  out of band, returning text into the conversation — the shape **TTS already
  uses** (ADR 0009). The key needs no plumbing at all: `stored_key` is
  provider-centric (ADR 0008), so the Gemini key entered for chat or embeddings
  is the same one.
- **Returns the answer, not the material** (the `fetch_url` shape): a 10-minute
  video is ~62k tokens *at Google* and a few hundred *in the conversation*, which
  is what makes it usable from a local 8k-context model. A raw transcript is
  deliberately out of scope — its honest home is a chat attachment (§9.7), which
  is already paged and searchable.
- **Cost is bounded before it is spent.** The tool refuses past
  `config.video.max_minutes` (default 30 ≈ 186k tokens) and names `start`/`end`
  in the refusal, so the model can retry with a segment instead of giving up. The
  gate measures the **segment**, not the video — with bounds given only that span
  is charged, so gating on full duration would refuse requests that cost little.
  When the length cannot be read at all, the request is clipped to the ceiling
  **and the answer says so**: silently describing only the beginning is the
  failure mode fork R4 rejected.
- **Degradation is the contract, not an afterthought.** Title/channel/length/the
  author's description come free from the watch page (oEmbed as fallback) and
  still work with no key; with no provider the tool does **not** disappear — it
  returns that metadata plus a plain statement of what is missing, so the model
  can explain itself to the user. A provider failure or timeout degrades the same
  way with the reason included.
- **Four things the design sketch got wrong, found while building:**
  - `is_youtube_url` is **not** `video_id().is_some()`. A bare 11-character id is
    accepted as input (models pass one as often as a URL), but must not let
    `fetch_url` route arbitrary 11-character text into the YouTube branch.
  - The watch page is parsed with a **string-aware balanced-brace scanner**, not
    the probe's regex: the blob contains `}` inside strings, and the non-greedy
    match survived only by luck.
  - **Thinking has to be muted explicitly.** `maxOutputTokens` *includes* thought
    tokens, so on a 3.x model the budget can go entirely to thinking and the
    answer comes back blank (observed on `gemini-3.6-flash`). The mute is per
    generation, so rather than write that heuristic a second time,
    `is_gemini_3`/`is_gemini_3_pro` in the engine wire became `pub(crate)`.
  - `VideoConfig`'s `Debug` is **hand-written to redact the key** — the type is
    reachable from `ToolConfig`, which derives `Debug`, so a plaintext key must
    not be one stray `{:?}` from a log file. Pinned by a test.
- **`fetch_url` stopped dead-ending on YouTube links** (fork R6): measured, the
  watch page has **zero** paragraphs and **zero** list items, so readability
  extracted nothing and the answer was "failed to extract readable text" — a dead
  end the model cannot reason its way out of. It now returns the same metadata
  block and points at `youtube_watch`.
- **Registry wiring**: `config.video` feeds the **tool registry** (the client is
  built there), so it rebuilds on a `config.video` edit *and* on a **Gemini key
  change** — otherwise the tool would keep reporting itself unconfigured until
  some unrelated settings edit happened to rebuild it.
- **Tests**: URL forms (every shape plus a bare id, and the links that must
  *not* parse); the brace scanner against `}` inside a string; watch-page and
  oEmbed parsing; duration formatting; the wire body (file part, bounds only when
  set, per-generation thinking mute, model-prefix normalization); a blank answer
  naming its `finishReason` and a filter block reported rather than silently
  empty; the tool's degraded paths (no provider, non-YouTube URL, reversed range,
  over-ceiling segment, provider failure) each asserting the provider was **not**
  called where it must not be; key redaction; localization. Settings: four rows
  in a "Video" group, going through the `field_spec` access table (the settings
  plumbing was delegated to a subagent with that constraint spelled out; it
  followed the precedents and reused two existing label keys rather than adding
  near-duplicates). **1732 unit tests green** (+30), **72 `#[ignore]`** (+2),
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Live run — GO** (real Gemini `gemini-2.5-flash`, 2026-08-01):
  `watches_a_real_video_live` clips a video to its first 20 s — where a
  transcript would give almost nothing, the first sung line starting at ~0:18 —
  and got back a timestamped account of clothing, hair, three distinct settings
  and the cuts between them, under the live title and duration read from the
  watch page (7.2 s end to end). That is the assertion that matters: it is the
  half a transcript could not have produced.
  `live_youtube_link_returns_metadata_not_a_dead_end` confirms the `fetch_url`
  half against the real page (1.3 s).
- **Groundwork** (roadmap): `transcript: true` landing a long transcript as a
  chat attachment (R3c); cross-chat caching of an expensive watch (R8b) — within
  one chat a follow-up is already free; a transcript path needing no cloud key at
  all (R7) — the one story stage 1 does not serve; and default-model rot
  (`gemini-2.5-flash-lite` already 404s for new users).

### Post-M9: YouTube stage 2 — the words, as a chat attachment (done)

- **Stage 2 of the YouTube track**, fork **R3(c)**: plan with forks F1–F5 —
  [docs/history/youtube-transcript.md](../../docs/history/youtube-transcript.md)
  (**user's decision, 2026-08-01**, all as recommended); behaviour — spec §9.9.
  `youtube_watch(transcript: true)` brings back the spoken words as well as the
  description, and a large transcript lands as a **chat attachment** (spec §9.7)
  rather than as a tool result — attachments already have a budget, page-by-page
  reading and semantic search, while a tool result goes into the context whole
  and would destroy an 8k local model. Branch `feat/youtube-transcript`.
- **It is not a cheap path, and the parameter says so.** A transcript is the
  *same request* with a different prompt — the video is ingested either way, and
  on the 3.x models audio is not even billed apart from video (research §3.3a).
  So it is **one** provider call for both halves, split on a marker, and the
  `transcript` description states the cost; without that the model would reach
  for it by default. What genuinely changes is the **output** side, which is
  where truncation stops being cosmetic (below).
- **The stage's real work was the contract, and reading the code decided it.**
  Tools do not mutate `Chat`: they return `ChatEffect`, which had exactly two
  scalar variants, and the **orchestrator** applies effects in `handle_done` —
  i.e. when the whole **turn** is over. Meanwhile `ToolContext.attachments` is a
  snapshot taken at the turn's start. So the naive implementation has a hole with
  a name: the tool says "attached as *X*, 14 pages", the model does exactly what
  it was told — `attachment_read("X", 1)` — and gets "no such attachment". That
  is the **third instance of one defect class** here, both earlier ones found
  live: the by-reference attachment block that described the situation without
  saying what was possible, and stage 1's own unconfigured path (eight tool calls
  rediscovering measured dead ends). Both were closed by making the message close
  the door; here the door has to be genuinely open, because the tool can deliver.
- **Fix (fork F1a)**: a generic `ChatEffect::AddAttachment(Box<Attachment>)`
  carrying the **already-built** attachment, mirrored by the loop into its own
  turn snapshot at the end of the round (`generation::sync_attachments`, applying
  the same dedupe-by-source rule the orchestrator will) and persisted by the
  orchestrator through `insert_attachment` — the path `/file attach` takes, so
  the index, the feed note and the status chip all follow. The loop still never
  touches `Chat`; the snapshot is its own. Carrying the built object means what
  the model was told about and what gets stored are the same one, `id` included —
  that `id` is the key the background index is written under. Two facts fell out
  of the same reading: a **cancelled turn still delivers its effects**, so a
  transcript already paid for is not lost on `Esc`; and the attach path was
  already reusable, so stage 2 joined it instead of building a second one.
- **The threshold is an existing setting (F2), and it has an invariant.**
  `config.attachments.max_file_tokens` decides result-vs-attachment: below it the
  model reads the words at once with no second call, and attaching would put the
  same text in the pinned block *and* in the history. Above it — an attachment,
  which for **any** value of the setting is **by reference by construction**
  (inline requires `est <= max_file_tokens`, exactly the other side of the
  threshold). So this path never adds an inline attachment, never competes for
  the chat's inline budget, and is indexed for `attachment_search` with no
  special case (F3). The source key `youtube:<id>#transcript@<segment>` makes
  re-transcribing the same span replace it while two different spans coexist.
- **Truncation had to become part of the contract (F5).**
  `VideoUnderstanding::describe` returned `String`, and a `MAX_TOKENS` finish
  with partial text was returned silently. For a description that is cosmetic;
  for a transcript it is a correctness bug, because a complete-looking prefix
  would let the model believe it had read the video to the end — losing the very
  guarantee `attachment_read`'s page walk exists to give. It now returns
  `VideoAnswer { text, truncated }`, and the ceiling is reported in **both**
  places: the result (what the model reads now) and the file (what it reads
  later, when being a prefix is otherwise invisible).
- **The live run measured something the plan had flagged as unmeasured, and it
  came out "no".** The prompt asks for timestamps counted from the start of the
  video; asked for a 0:40–1:20 clip, Gemini numbered the transcript **from
  zero** — while the attachment header says they are absolute, so the file was
  making a false claim to anyone reading it weeks later (seek to 0:03 for a line
  that is really at 0:43). Fixed with arithmetic we control rather than an
  instruction we hope is followed. And the **second** live run justified the
  design of that fix: the same model, same request, emitted **absolute**
  timestamps on its own — a blind shift would have pushed them outside the
  segment entirely. So whether to shift is decided by the first timestamp, and
  the ambiguity is bounded by the offset itself (documented at the function).
- **Tests**: the marker split and its no-guessing fallback; the F2 threshold in
  both directions plus the by-reference invariant; the source key separating
  segments; truncation reported in result and file; the timestamp correction in
  both directions; and — the stage's real risk — **the loop letting the next
  round read what the previous one attached**, end to end through the real
  agentic loop with a fake attaching tool (round 1 attaches, round 2 reads it
  back), plus the persistence half through `handle_done`. **1749 unit tests
  green** (+14), **73 `#[ignore]`** (+1), clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **Mutation-tested**: dropping the
  mirroring, the persistence, or the truncation flag each fails its own test and
  nothing else.
- **Live run — GO** (real Gemini, `gemini-3.5-flash`): a 0:40–1:20 clip,
  `transcript: true` — the transcript came back as an attachment carrying the
  sung lines with timestamps inside the requested span (5.9 s), and the tool
  result named the attachment, its page count and the two tools that reach it.
  The assertion is deliberately the mirror of stage 1's, which asserts on
  something only *visible* on screen: this one asserts on something only *said*,
  so both halves of the question the track started from are covered live. Stage
  1's smokes (`watches_a_real_video_live`, the `fetch_url` one) re-run green.
- **Groundwork unchanged** (roadmap): cross-chat caching of an expensive watch
  (R8b), a transcript path needing no cloud key (R7), default-model rot. New:
  `fetch_url`/`python_exec` gaining `attach: true` — the effect makes it cheap,
  which is exactly why it should be a deliberate decision rather than a side
  effect of this stage.

### Post-M9: `youtube_watch` — a degraded answer has to close the door (done)

- **Found by the user in a live run**, the first real one after the merge: an
  `openai` chat (gpt-5.6) with **no Gemini key stored**, so `youtube_watch` took
  its unconfigured path and returned the metadata plus "video understanding is
  not configured". The model then spent **eight tool calls** rediscovering the
  dead ends this project had already measured — three `web_search`es (including
  one for "<video id> transcript"), scraping `captionTracks` off the watch page
  and hitting the signed `timedtext` URL (200, **0 bytes**, exactly as §3.1 of
  the research says), `pip install youtube-transcript-api` (**no pip** in the
  sandbox), looking for `yt-dlp`/`ffmpeg` (**absent**) — until the user cancelled
  the turn. Branch `fix/youtube-dead-end-wording`.
- **This is our defect, not the model's.** The stage-1 message said *what* was
  missing and how the **user** could fix it, but never said the video's content
  is unreachable by any other route — so an agentic model quite reasonably read
  it as a local limitation and went looking. **The same defect class**, and the
  same fix, as the by-reference attachment block: there too the entry described
  the situation without stating what was and wasn't possible, and there too the
  model improvised (`fs_list`, `fs_read`, four `web_search`es) and ended on an
  impossible suggestion.
- **Fix — the degraded answers now name the routes that don't work**
  (`not_configured`, `failed`, `timeout`, both bundles): captions come back empty
  without a token, neither `fetch_url` nor `python_exec` gets around that (no
  `yt-dlp`, no `ffmpeg`, no `pip` in the sandbox), and no transcript is in web
  search — followed by what to do instead (answer from what is there, say plainly
  that the video was not seen, and tell the user how to enable watching). The
  **tool description** says the same up front, so the choice is informed before
  the call rather than after it. `failed` additionally allows **one** retry, since
  a provider error can be transient — but not by another route.
- **The regression test asserts a property, not prose**: every degraded message,
  in every built-in locale, must name `python_exec` and `fetch_url` — tool ids,
  which are stable identifiers rather than wording. **Mutation-tested in both
  directions**: weakening the clause fails it with the whole message in the
  failure output. Nothing else changed — no code path, no config, no schema.
- **Worth recording separately**: the trace is also a clean field confirmation of
  §3.1 of the research, produced by a different agent on a different day from a
  different starting point. The `timedtext` fetch returned **200 with an empty
  body**, `pip`/`yt-dlp`/`ffmpeg` were absent, and web search had no transcript —
  every measured dead end, re-measured live.
- **Follow-up from the same live run: a Gemini key had nowhere to be
  entered.** The user's setup was chat on OpenAI with local embeddings — and the
  "Model" section offers a key row only for a slot whose **mode is that cloud**,
  so there was no field for a Gemini key anywhere in the UI, while `youtube_watch`
  needs one whatever the chat engine is. That is the gap that put the tool on its
  unconfigured path in the first place. Fixed by giving the "Video" group its own
  stored-key row (`FieldId::VideoApiKey`) — the same machine-bound storage as every
  other key (ADR 0008), addressing `CloudProvider::Gemini` **unconditionally**
  rather than deriving the provider from a mode, since this slot has no mode. All
  the behaviour came free from `is_secret_field` + `api_key_field_provider`: masked
  empty editor, commit as `SetApiKey`, `Del` deletes, and the key never reaches the
  screen's config. Tests pin the part that could regress silently — the row targets
  Gemini **with no engine set to Gemini** — plus the status/`Del` pair.
  **1735 unit tests green** (+2).
- **Default model moved to `gemini-3.5-flash`** (user's request after a working
  live run: 2.5 is old and will not stay around). Verified before switching, and
  the verification changed two documented figures. On the **same 20 s clip with an
  otherwise identical request**, `mediaResolution` turns out to be a **no-op on the
  3.x flash models** — 1822 prompt tokens at `LOW` *and* at `MEDIUM`, checked on
  3.1-flash-lite, 3.5-flash and 3.6-flash — while on 2.5 it behaves as documented
  (2062 → 5902). And 3.x reports **no `AUDIO` modality** at all, which looked like
  losing half the feature; it is a *reporting* difference, confirmed
  **behaviourally** rather than assumed: asked to quote the words in a segment,
  3.5 and 3.6 returned the sung lines with correct timestamps just as 2.5 did.
  Audio is folded into the video bucket. Net effect: the new default is **~12%
  cheaper** (91 vs 103 tok/s at low detail), so the 30-minute ceiling is ~164k
  tokens rather than ~186k. The resolution setting is **kept** — it is real on
  2.5-class models and the API documents it generally — but its hint now says
  where it does nothing instead of promising a 3× saving. Figures corrected in
  spec §9.9, install.md, both settings hints and the `MediaResolution` doc
  comment; the research doc gained §3.3a with the comparison table. **Live smokes
  re-run against the new default — GO.** A trap worth recording: the settings
  hints are stored as **arrays of strings** (the bundle allows either), so a
  single-line regex edit mangles them — caught by validating the JSON before
  writing, which is why the files were never damaged.
- **1735 unit tests green** (+3), 72 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run needed** — the change is the
  text of three localized strings and a tool description; the engine, memory and
  tool paths are untouched, and the behaviour it fixes is the model's reading of
  that text, which no automated smoke can assert. Whether the wording actually
  stops the flailing is the user's next live run to judge.

### Post-M9: MCP servers in the settings window (done)

- **Closes the ADR 0007 groundwork item "server editor UI"** — until now
  `config.mcp.servers` was hand-edited in `settings.json` (decision point R6,
  deliberate at the time: the host was new and a file was the smaller surface).
  Design plan with forks F1–F8 —
  [docs/history/mcp-server-editor.md](../../docs/history/mcp-server-editor.md)
  (**confirmed by the user 2026-08-02, all as recommended; scope — stage 1**).
  Behaviour — spec §9.6. Branches `docs/mcp-server-editor` → `feat/mcp-server-editor`.
- **Reading the code first shrank the task and reshaped it.** The settings screen
  already owns a working `AppConfig` and emits `SettingsIntent::SaveConfig` carrying
  the **whole** thing; `handle_update_config` diffs `config.mcp` → `mark_mcp()` → the
  1.2 s debounce → `apply_mcp_settings()`. So editing the inventory from the screen
  needed **no new `AppCommand`, no new `AppEvent` and no orchestrator change at all**
  for the edits themselves — the impersonation-persona shape
  (`config.impersonation_profiles`, spec §11.8), which is the precedent this follows
  throughout. `Ctrl+Z` undo came free for field edits *and* for create/delete (it
  restores a whole older config snapshot), and `is_current` already suppresses the
  no-op restart from an edit plus its undo.
- **The hole that reading turned up, and the one genuinely new action.** A server
  that exhausts its restart budget (3 crashes / 5 min) sits `Disconnected` "until
  manual intervention (editing settings recreates the slot)" — but since `is_current`
  landed, toggling a field off and on inside the debounce window yields an
  *identical* final config and therefore **no re-apply at all**. There was no
  reliable way to retry a dead server short of restarting the app. So `Enter` on a
  status row now does what that row needs: confirm a changed catalog when one is
  pending (today's meaning), otherwise **reconnect** — an action, not a config edit,
  so it gets an intent/command pair mirroring `ConfirmMcpCatalog`. The restart budget
  is cleared by it: an explicit reconnect *is* the manual intervention it waits for.
- **Reconnect forced the event epoch to become per slot** (`McpSlot.epoch`), replacing
  the comparison against the manager's global one. Bumping the global epoch to restart
  one server would drop **another** server's in-flight `Ready` and strand it on
  "connecting…" forever; keeping the epoch would let the cancelled task's late
  `Exited` restart the fresh one. Per-slot is strictly more precise than the global
  check it replaces (an event for a server that was removed, or whose slot has moved
  on, is dropped exactly as before) and is a ~10-line diff.
- **Where it lives (F1b): a dedicated "Plugins" section**, not a group in "Tools".
  The "Data" precedent — a section exists when nothing else is about that thing:
  "Tools" is a list of *gates for built-in tools* (one toggle each), while this is an
  *inventory of external programs* with per-server settings, statuses and a lifecycle.
  It also gives the remaining MCP groundwork (HTTP transport, resources/prompts, tool
  caps) somewhere to land. Layout: master switch → a `Choice` server selector plus the
  selected server's fields → the status rows that used to sit in "Tools" (kept as the
  at-a-glance list, so status is not duplicated).
- **The two non-scalar fields.** `args: Vec<String>` is edited as a **shell-quoted
  command line** (F3a) — how a person types one, and unlike a separator it is total:
  `join_args` quotes any argument containing whitespace or either quote character, so
  the round trip holds for *any* value (pinned by a test over paths with spaces,
  embedded quotes, apostrophes and an empty argument). `env` is `VARIABLE=SOURCE`
  pairs (F4a): flat parsing is unambiguous **because** the value is the name of a
  source environment variable rather than a secret (ADR 0007 R8) — a property worth
  stating, since stage 2 changing that would change the shape.
- **A new server starts disabled** (F5a) with a generated free id and no command:
  nothing is spawned while the command is half-typed, and flipping "Enabled" becomes
  the deliberate "start it" moment a per-field debounce cannot express.
- **Validation before the commit** (F8a) for the three rules the screen can check
  itself: slug shape, **uniqueness** among servers, and the batch-command ban. The
  status row remains the backstop for everything else, but an invalid **id** creates
  no slot at all — just a log warn — so without this a typo'd id would make the
  server *vanish* from the status list. The server-id rule moved from
  `orchestrator/mcp.rs` into `shared/mcp.rs` next to `forbidden_batch_command`: the
  screen may not import `app` (FSD), and one rule with two copies is how they drift.
  The validation is checked **before** taking the editor's `&mut` borrow — uniqueness
  needs the rest of the config, which a validator holding that borrow cannot see.
- **The TOFU pin is deliberately untouched by an edit**: changing `command`/`args`
  keeps it, so pointing a server at a different program trips the catalog check —
  precisely when it should fire. A **rename** drops the pin (it is inherited by id in
  `handle_update_config`) and the server re-pins silently, which is acceptable since a
  rename changes every tool name anyway; documented rather than special-cased.
- **Not in this stage** (F4b/F6, a separate decision): machine-bound secret **values**
  for the `env` map and import of the ecosystem's `mcpServers` JSON. Storage needs no
  change for the first — `secrets::put_key`/`store_secret` already take an arbitrary
  key name, as the backup password proved — but it amends R8, and the two are coupled
  (an imported `env` carries literal secrets), so they stay stage 2. The honest limit
  today: a hosted server still needs its token in an OS environment variable.
- **Tests**: create → edit → delete round trip (including that a new server is off and
  gets a free id); `args`/`env` round-tripping through the field, plus hand-typed
  forms; the id refused for shape, emptiness and collision while its *own* id is not a
  collision; the batch ban; the selector cycling and its popup; `Del`/the `•` marker
  skipping user data; undo restoring a deleted server; and at the manager level,
  reconnect respawning one server **without stranding another's in-flight event** and
  clearing an exhausted budget. **All seven load-bearing behaviours were
  mutation-tested** — each mutation (a server created enabled, validation dropped,
  Enter doing nothing on a healthy row, the global epoch restored, the budget kept,
  the fields treated as config data) fails its own test and only that one. Plus the
  seam the whole editor rests on — a `config.mcp` edit reaching the host through the
  existing `UpdateConfig` → diff → debounce path, and *not* costing a restart when
  nothing effective changed — which was the one untested claim behind "the
  orchestrator needed no changes" (mutation-tested too). **1766 unit tests green**
  (+10), **74 `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **Live run — GO** (`mcp_reconnect_live`, a real
  `npx @modelcontextprotocol/server-filesystem`): first bring-up **Ready, 14 tools**;
  reconnect → the tools leave with the connection, the server comes back **Ready with
  the same 14** in 6.1 s. That is the half unit tests cannot answer — they can prove
  the slot is reset and a task spawned with a fresh generation, but whether a real
  subprocess is actually torn down and a new one handshakes in its place is a property
  of the process handling. The smoke needs only `npx`, no model, so it runs without an
  engine. `mcp_filesystem_e2e_live` is green too (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`): 14 tools in the catalog, the model called
  `mcp__fs__read_text_file` and used the result.
- **Regression — clean** (same stack): **26 of 27** orchestrator e2e live smokes green
  on the first pass (669 s) — memory/self-model/notes/graph/cross-organ links/RAG/
  attachments/control tools/i18n/MCP. The one failure was `followup_tool_e2e_live`,
  where the model simply answered with a bare one-line greeting and never
  called `send_followup_message`; it passed on re-run (`saw_continue=true`, a second bubble).
  A model-behaviour flake of the class the control-tools entry already records, not a
  regression: the diff touches **zero** files on the agentic-loop/control-tool path.
- **The first manual run in a terminal found two things** (plan §8), both fixed on
  the same branch.
  - **A platform-dependent config.** A spike measured *why* `cmd /c npx …` is
    needed on Windows rather than assuming: `cmd.exe` completes a bare name from
    `PATHEXT` and **Rust does not** (`Command::new("npx")` → `NotFound`,
    `Command::new("npx.cmd")` → spawns). `shared::mcp::resolve_command` now does
    that completion, so one config works on every platform. The same spike
    **overturned the premise of the `.bat`/`.cmd` ban** (ADR 0007 §2):
    CVE-2024-24576 is fixed in `std` as of Rust 1.77.2 — measured, `a"b`, `%CD%`
    and `a&whoami` are escaped and an embedded newline is *refused* with
    `InvalidInput` — so the ban added nothing over `std` while pushing users onto
    `cmd /c`, where arguments are re-parsed by `cmd.exe` **outside** that
    escaping. Removed, with the reasoning recorded in the ADR; the live smokes now
    use a bare `npx` on every platform, which makes them the demonstration.
  - **The live run then caught a bug in the new resolver**, which is what a live
    run is for: npm ships an extensionless `npx` (a Unix script) *next to*
    `npx.cmd`, and preferring the exact name spawned the script — `os error 193:
    not a valid Win32 application`. `cmd.exe` only ever completes a **bare** name;
    a name that already carries an extension is tried as written and then still
    completed (so `my.tool` reaches `my.tool.exe`). The rule's core became a pure
    function over `PATH`/`PATHEXT`, so a test builds the npm layout in a temp
    directory instead of mutating the process environment. The *wiring* (that
    `spawn` calls the resolver) is covered by the **live smoke only** — a unit test
    would need a global `PATH` mutation (racy under a parallel suite) or would sit
    through the 30 s handshake timeout; recorded rather than faked.
  - **"Ready" could mean invisible.** The reported symptom was a server showing
    `ready · tools: 1` while the assistant said it had no such tool — the double
    opt-in (R7) working as designed, but the row never said so. Same defect class
    as the by-reference attachment block and the `youtube_watch` unconfigured path:
    the UI states a situation without saying what is possible next. The row now
    reads `ready · tools: N · in profile: K` and points at "Profiles" when `K` is
    zero; the double opt-in itself is unchanged.
  - **1769 unit tests green** (+3), 74 `#[ignore]`; the resolver's bare-name rule,
    its extension handling and both halves of the status row were mutation-tested.
    **Live — GO** with a bare `npx` on Windows: `mcp_filesystem_e2e_live` (14
    tools, the model read the file) and `mcp_reconnect_live` (Ready → reconnect →
    Ready, same 14).
- **Acceptance criterion — met** (manual run by the user, 2026-08-02): a real
  server configured **entirely from the settings window** — `mcp-echo-server` with
  the platform-independent `npx` + `-y mcp-echo-server` (no `cmd /c`), the status
  row reading `ready · tools: 1 · in profile: 1`, and the model calling
  `mcp__mcp-echo-server__echo` and getting its result back. Every part of the track
  that only a human at a terminal could exercise is confirmed: authoring, the
  resolver, the profile-count row, and the tool reaching the model.
- **A process trap, hit for the second time in this repo** (the journal already
  records it from the vendored-syntaxes work): `git checkout -- <file>` used to revert
  a scripted mutation reverts **all** uncommitted work in that file. It cost the whole
  of `apply.rs`, restored by re-running the patch script. The fix is procedural —
  commit before mutating, which is what the second run did.

### Post-M9: MCP servers — secrets for the `env` map and JSON import (stage 2, done)

- **Completes the "MCP servers in the settings window" track** (plan
  [mcp-server-editor.md](../../docs/history/mcp-server-editor.md) §9, forks **S1–S8
  confirmed by the user 2026-08-02, all as recommended**). Stage 1 made a server
  *authorable* in the window; it still could not be *given a token* — a hosted
  server (GitHub, Slack) needed an OS environment variable and an app restart,
  which is the opposite of the track's goal. Behaviour — spec §9.6; **ADR 0007
  R8 amended** (as stage 1 amended R6) and **ADR 0008** gains its third reuse.
- **Reading the code first settled four things, and one of them was the whole
  risk of the stage.** The storage needed no change (`put_key`/`store_secret`
  take an arbitrary key name — proved by the backup password); the resolution
  pattern already existed and was exactly right (`resolve_api_key(stored,
  api_key_env)` — stored wins, env is the fallback); decryption belongs in
  `McpManager`, mirroring `EngineManager`, which itself calls `stored_key` and
  hands the spawn layer plaintext. And the trap: **`McpManager::is_current`
  compares `McpSettings` only**, so changing a secret leaves the config identical,
  the debounce skips the re-apply, and the server keeps running with the old value
  with nothing to notice it. The engines had already solved this
  (`chat_is_current(&settings, keys)`); `is_current` now takes the key blob too.
  Recorded in the plan §9.1 **before** implementation — it would otherwise have
  shipped silently.
- **The comparison is against the *resolved* environment, not the key blob** (a
  deliberate departure from the engines): killing and respawning `npx`→node is the
  most expensive re-apply on the screen, so changing an unrelated OpenAI key must
  not touch it. `applied: Option<(McpSettings, ResolvedEnvs)>`; the slot carries
  its resolved env because a respawn after a crash or a `reconnect` builds the task
  from the slot.
- **S1 — the `env` row keeps its meaning and becomes the declaration.** An empty
  source (`TOKEN=`) is legal and already parsed; below the row sits **one secret
  row per declared variable** (status + empty masked editor + `Del` deletes) —
  the "API key" row's exact behaviour, reused wholesale (`secret_row`,
  `is_secret_field`, the masked editor). **No new config field and no schema
  change**: the presence of a secret is a fact about `config.api_keys`, which
  `handle_update_config` already restores on the way back, so secrets survive
  every settings edit with no new code.
- **S2+S3 — one typed `SecretKey`, not a third variant.** `SecretKey { Provider |
  BackupPassword | McpEnv{server,var} }` + `storage_name()` replaced
  `SetApiKey`/`SetBackupPassword` with one `SetSecret`, and
  `api_keys_present`+`backup_password_present` with one `secrets_present`. Typed
  rather than a raw name because the side effects after storing differ per kind
  (a provider key re-raises the slots that use it and rebuilds the registry for
  Gemini; an MCP one marks `mcp`; a backup password needs nothing) — dispatching
  those by parsing a string is how they drift.
- **S4 — orphaned secrets are never collected automatically.** A rename or delete
  orphans them, but the `env` row commits on `Enter`, so a half-typed edit would
  destroy a value the user then has to re-enter — and `Ctrl+Z` restores the
  *config* while secrets are deliberately absent from that snapshot, making a
  garbage collector's mistakes unrecoverable. An orphan is inert ciphertext;
  cleanup belongs with ADR 0008's "forget this computer" groundwork.
- **S5–S7 — the import, and why the orchestrator parses it.** A `mcpServers` file
  carries **literal** secrets, so the row takes a **file path** rather than pasted
  JSON (a pasted blob would leave live tokens on screen and in the editor's undo
  buffer) and `features/mcp_import.rs` only *plans* — the orchestrator reads,
  stores each literal value as a machine-bound secret, and only then extends the
  config. Imported servers arrive **disabled**; ids are sanitized to our slug with
  a numeric suffix for collisions *within* the file; an id that **already exists is
  skipped** (a re-import is a no-op, never an overwrite — a bug caught by its own
  test, since the first version uniquified before checking and quietly imported a
  duplicate); non-stdio entries are reported rather than dropped.
- **Along the way**: `valid_env_name` (POSIX-shaped `[A-Za-z0-9_]`) moved next to
  `valid_server_id` in `shared/mcp.rs` — it is what keeps the flat `VAR=SOURCE`
  row parseable *and* `mcp-<server>-<VAR>` unambiguous (only the server id may
  contain `-`); and the "Command" field's hint stopped claiming `.bat`/`.cmd` are
  banned — stage 1 removed that ban and the hint still told users to write
  `cmd /c npx …`.
- **A design conflict found by a failing test**: `McpEnvSecret` belongs in
  `is_profile_field` (user data — no `•` "modified" marker, nothing to reset to),
  but that predicate is also the first gate in `reset_field`, which would have made
  `Del` a no-op on a secret row. The secret branch now runs **before** it: the two
  meanings of "has no default" and "Del has a real meaning" are separate.
- **Tests**: the storage name; a stored secret winning over the source and the
  source still serving as fallback; `is_current` false when **only** a secret
  changed; an unrelated provider key leaving the servers current; per-variable rows
  following the declaration, showing status, never putting the secret into the
  screen's config; `Del` deleting; the row dropping unrepresentable names; the
  import row committing a path; and end-to-end through the orchestrator — secrets
  stored as ciphertext with **no plaintext in `settings.json`**, servers disabled,
  ids sanitized, a re-import a no-op. **1787 unit tests green** (+13), **75
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Mutation-tested, and it earned its keep**: nine mutations, eight caught by
  their own tests — the ninth **survived**, exposing that nothing asserted a stored
  MCP secret schedules the re-apply, without which the `is_current` fix above is
  never even consulted. `storing_an_env_secret_schedules_the_reapply` closes it
  (and fails under that mutation).
- **Live run — GO** (Gemma 4 31B q4_0, external `llama-server`, `--jinja`; real
  `npx`): the new smoke `stored_env_secret_reaches_the_child_process_live` spawns a
  minimal Node MCP server that reports what it sees in **its own environment** —
  it answered `ZARYA-7719`, the value stored only as a machine-bound secret. That
  is the one thing unit tests cannot show: that decryption, the spawn env and the
  child's `process.env` really line up. Regression: `mcp_filesystem_e2e_live` (14
  tools, the model called `mcp__fs__read_text_file` and used the result) and
  `mcp_reconnect_live` (Ready → reconnect → Ready, same 14) both green.
- **Follow-up from the manual acceptance run** (the user, 2026-08-02): the import
  worked — a real Cursor config imported cleanly — but the variable configuration
  "looks strange and inconvenient: why type `variable=value` pairs and then enter
  the value in a separate field as well". Fair, and the phrasing is the evidence:
  the value slot means the **name of a source variable**, and it *reads* as
  `variable=value`, so the row below asking for the value looks redundant.
  Checking `McpClient::spawn` settled how much of the mechanism was even
  load-bearing — it uses `Command::envs` with **no** `env_clear`, so the child
  already inherits the whole application environment, and `NAME=SOURCE` is only
  needed to take a value from a *differently named* variable. The row is now a
  **bare list of names** (`GITHUB_TOKEN, SLACK_TOKEN`), labelled "Variables";
  `NAME=SOURCE` is still parsed but no longer advertised. **No behaviour
  changed** — only what the field asks for. The claim the new hint makes is
  pinned live rather than asserted: the smoke now also sets a variable named
  **nowhere** in the server's config and the child reports it
  (`ZARYA-7719|INHERITED-4417`), so an `env_clear` appearing later would fail the
  test instead of quietly making the hint false.

- **A second follow-up from the same run**, and it was a defect rather than a
  preference: with a source named (`API_KEY=CLAUDE_API_KEY`) the screen still
  offered a value row, and S8's "stored wins" meant a secret could silently
  override the source the row named. Hiding the row alone would have made it
  worse — the override would remain, minus the only thing that could reveal it.
  The rule is now **one origin per variable, decided by the row**: a bare name
  takes the stored value (or is left to inheritance), `NAME=SOURCE` takes it from
  that OS variable and consults no stored value, and gets no value row. This
  narrows S8 instead of reversing it — the fallback existed for a shared config
  on a machine where the variable is set in the OS, and that case uses the
  variable's own name, which inheritance already covers. The row index stays the
  position in the whole map, so skipping a row never shifts `secret_field_key`'s
  addressing (pinned by the test). Two tests had their premise inverted and were
  rewritten rather than patched: `each_variable_has_exactly_one_origin` replaced
  `stored_secret_wins_over_the_env_source`.

- **A third follow-up, and the objection was about the logic rather than the
  screen** ("the fallback is good, but values are machine-bound, so you end up
  checking whether the variable exists *and* whether a key was entered on this
  particular machine"). Half of that had already gone with the change above —
  per variable the row decides which route applies, so the two are never both in
  play. What was left is an **asymmetry of visibility**: the stored route
  reported itself, a named source reported nothing, and a missing OS variable
  surfaced only as the server failing to work. A sourced variable now gets a
  **read-only status row** in place of the value field it deliberately lacks
  (`from CLAUDE_API_KEY: found / not found`, flagged when missing), and its hint
  states what was invisible before: the app sees the environment it was
  **started** with, so a variable set after launch needs a restart.
  Mutation-tested. Rejected: leaving it silent (the diagnosis stays indirect) and
  dropping the source form from the UI (it is the only way to rename a source).

- **The live check of that row found two defects, one of them older than this
  track.** Focusing a variable whose source is missing printed "the tool is
  enabled in the profile but disabled by a global switch — enable it in the
  «Tools» section", which is about neither that row nor that section. Root cause:
  `FieldRow.warn` meant **two** things — "colour this row" and "append the gate
  explanation" — so the sentence appeared wherever the flag was raised, including
  (since stage 1, unnoticed) on an MCP server whose catalog had changed. The
  explanation now travels in its own `warn_note`, set where the gate is actually
  detected; `warn` is back to meaning only "this row needs attention". And the
  text now **names the section that holds the switch**: the MCP master toggle
  moved to "Plugins" when the editor got its own section, so the hardcoded
  "Tools" had been wrong for it ever since — the user caught that too. Third,
  smaller: the new row's description showed a literal `{src}`, resolved with `t`
  where it needed `tf`. One test pins all three (mutation-tested).

- **Groundwork** (roadmap): the external proxy's key via the same mechanism; an
  explicit cleanup of orphaned MCP secrets, with "forget this computer"; HTTP
  transport, resources/prompts, `list_changed`, deferred schemas.

### Post-M9: page fidelity for `fetch_url` (+ two neighbouring traps) (done)

- **Specified by one real chat, not by a roadmap item.** The user pasted
  `https://docs.vlang.io/memory-management.html` and asked the assistant to read
  it and compare V's memory-management modes. One `fetch_url` call turned into 20
  messages — 3 fruitless `web_search`es and **6** `python_exec` rounds, including
  downloading the same 16 MB tarball of the V repository **twice**. Reading the
  transcript against the code found four causes, and they chain. Plan with forks
  F1–F2 — [docs/history/fetch-url-fidelity.md](../../docs/history/fetch-url-fidelity.md)
  (**user's decision 2026-08-03: all four, F1(a) `fetch_url` only, F2(b) attach**);
  behaviour — spec §9.3.1. Branch `feat/fetch-url-fidelity`.
- **P1, the root cause — extraction threw away the code.** `extract_readable`
  selects `p, li`, and the page was measured rather than assumed: **0 `<pre>`**
  (VitePress wraps code in a bare `<div class="language-v">`), section titles in
  `<h2>` — both dropped whole. The result the model received has visible holes
  ("…define a `free()` method on custom data types:" straight into the next
  paragraph). Documentation whose every "here is an example:" leads nowhere reads
  as **arriving damaged**, so the model went looking for the source elsewhere —
  and every later detour follows from that one inference.
- **`extract_rich` is `fetch_url`'s alone** (F1a): `web_search` budgets 1500
  characters per result page for **ranking**, where headings and code spend the
  budget without helping. Pinned by a test asserting the prose path is unchanged.
  Rich extraction keeps document order (one selector run), renders Markdown-ish
  (`## Heading`, fenced code with the language from a `language-*` class on the
  element or its inner `<code>`), preserves whitespace inside code (`collapse_ws`
  is right for prose and destroys code), exempts code and headings from
  `MIN_FRAGMENT_CHARS` (that floor exists to drop navigation chrome from
  snippets; a two-word heading is content), and **dedupes by ancestry** — a
  `div.language-*` wrapping a `<pre>` matches two selectors and must not emit its
  content twice.
- **P2 — the text was cut at 12 000 characters, silently, mid-word.** The
  recorded result is exactly 12 000 characters and ends on `…final out`. Nothing
  marked it partial and nothing could fetch the rest, so a long page was
  indistinguishable from a complete one. Now a page over the budget **is attached
  to the chat** (F2b): `ChatEffect::AddAttachment` already existed (built for
  `youtube_watch(transcript:)`), and an attachment is already paged
  (`attachment_read`) and searchable (`attachment_search`) — this is the
  mechanism's second consumer, not a new one.
- **The threshold is `attachments.max_file_tokens`, exactly as `youtube_watch`
  uses it**, and one consequence carries over for free: an attachment made this
  way is **always by reference**, because inline requires `est <=
  max_file_tokens` — the other side of the same comparison. The mode still goes
  through `decide_mode`, so it cannot drift from what `/file attach` decides.
  `summarize=true` still summarizes (from the head — the summarizer is a
  single-turn subagent) **and** attaches: a summary alone would repeat P2 in a
  politer form, since the model would have no way to reach what the summary
  skipped. A far higher hard ceiling stays (400 000 characters) and is now
  **announced** when reached, closing P2 at both ends.
- **P3 — `python_exec` gives a fresh sandbox per call and never said so.**
  `WasmerSandbox::run` creates a `JobDir` per run and drops it; only `/w` and
  `/sp` are mounted and the guest's `/tmp` dies with the process. The description
  said "no access to the machine's files" and nothing about state. Same defect
  class the journal already records twice (the by-reference attachment block,
  `youtube_watch`'s unconfigured path): the message describes a situation without
  saying what is possible next. Fixed in the description, with a test pinning
  that it names **`/tmp`** — the concrete path a model reaches for — in every
  built-in locale.
- **P4 — `web_search` said "no results" three times while it was blocked.**
  Measured live from this machine on the transcript's own queries: DDG lite 202 +
  `anomaly` (detected), Ecosia 403 (detected), **Mojeek HTTP 200 with
  "Verification required. Please complete the challenge"** — invisible to
  `is_throttled`, so `got_clean_page` was set and the honest "all providers are
  throttled" error was suppressed. The model was told the web knows nothing about
  V's memory management and, quite reasonably, went to `python_exec` +
  `requests`. The check (`is_challenge_page`) runs **only on a page that parsed to
  zero results**, so a genuine result set can never be mistaken for a challenge —
  strictly narrower than adding the markers to `is_throttled` itself.
- **Tests**: rich extraction against the transcript's real markup shape (heading,
  fenced code with language, whitespace preserved, nav dropped — plus the prose
  path asserted **unchanged**, which is what fork F1a means); no double emission
  of a wrapped block; language from an inner `<code>`; short code/headings
  surviving the floor; the attachment path (whole text, by reference, named by
  `<title>` with the URL as fallback, the result naming `attachment_read`/
  `attachment_search` and the page count); summarizing from the head while still
  attaching; the ceiling announced on both branches; the captcha classifier
  against the real interstitial text and against two results-page fixtures.
  **1811 unit tests green** (+13), **76 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check`/i18n gates clean.
  **Mutation-tested**: dropping ancestry dedup, neutering the challenge
  classifier, dropping the attachment effect, removing the `/tmp` sentence, and
  reverting `fetch_url` to prose extraction each fail exactly their own test.
  One gap recorded rather than faked — the provider loop's three-line branch has
  no unit test, since the providers are hardcoded URLs and a captcha cannot be
  summoned on demand; the classifier it calls is what the test pins.
- **Live run — GO** (network only, no model needed):
  `live_documentation_page_keeps_its_code` against the same page the transcript
  used — **18 907 characters** extracted (against the 12 000 truncated before),
  carrying `## Control`, a fenced ```v block and `fn (data &MyType) free()`, and
  landing as a **4-page by-reference attachment** whose result points at
  `attachment_read`. That is the pair of claims only a live fetch can settle,
  since both depend on the real markup. The other three network smokes
  (`live_fetch_without_summarize`, the YouTube dead-end one,
  `live_search_returns_results`) are green — the last one also confirming the
  provider fallback still answers from this IP.
- **The user's own live re-run confirmed all four in the app** (chat
  `28cdf212-…`, gpt-5.6): 15 messages instead of 20 — `fetch_url` → **all four**
  attachment pages read in a single round (the "walk `1..M` and know you read
  everything" guarantee, doing its job) → one `python_exec` call instead of six,
  with no `/tmp` write-then-lose and no second download. P4 fired for real: all
  three searches came back with the "every provider is throttling us" error
  instead of "no results", so the model got the truth rather than a false
  negative about the web.
- **And it exposed one more silent-wrong-answer path, fixed on the same branch.**
  The attachment was named **"V Documentation"** — the site-wide `<title>`;
  measured, `docs.vlang.io` gives every page that same title while `<h1>` names
  the page. Two pages of one site would have shared a name, and
  `attachment_read` resolves a name with `.find()` — **the first match**,
  silently reading a different file than the one asked for. Three guards, cheap
  and layered: the name now comes from **`<h1>` first** (better names, and it
  removes the collision at its source for the common docs layout); a name
  already taken by a *different* page gets the URL's last segment appended
  (deterministic, so re-fetching the same page keeps its name and replaces its
  own attachment rather than piling up copies); and `attachment_read` **reports
  an ambiguous name** with each candidate's source instead of guessing —
  addressing by source keeps working. The first two make a collision unlikely,
  the third makes it loud when it happens anyway, including for attachments that
  arrived some other way. Live on the same page: the attachment is now named
  **"Memory management"** rather than "V Documentation". **1814 unit tests
  green** (+3), 76 `#[ignore]`; the three new guards mutation-tested
  (preferring `<title>` over `<h1>`, dropping the disambiguation, dropping the
  ambiguity check each fail their own test).
- **Model-level regression — GO** (Gemma 4 31B q4_0 + bge-m3, external
  `llama-server`, `--jinja`), run once the user brought the LAN stack back up:
  **26 of 26** orchestrator e2e live smokes green in 556 s — memory/self-model/
  notes/graph/cross-organ links/RAG/attachments/control tools/i18n/MCP/tool
  confirmation. The right scope even though no orchestrator code changed: a tool
  now emits an effect on a path that previously never did, and the tool registry
  sits on every turn.

### Post-M9: images returned by MCP tools (done)

**What.** An image block in an MCP tool result stops being `[image content omitted]` and
is shown to the model. It rides on the tool-result `Message` and is delivered inside the
tool result itself on four of five engines; Gemini gets a fallback. Research and the live
matrix: [mcp-tool-images.md](../research/mcp-tool-images.md); behaviour — spec §9.10,
§13.4.

**The measurement that mattered was the one that showed the first measurement was
wrong.** Round 1 sent a blue-square fixture inside a tool result and every arm answered
"blue background, white square" — an apparently clean pass. Adding a **control with no
image at all** produced "light blue background, white five-pointed star": the model was
answering the *question*, confidently, from nothing. Re-run with a green field and a white
**circle**, the control's invention diverged and the signal became readable. Both live
smokes shipped here carry that control arm, and the fixture is deliberately a different
shape and colour from the one the user-image smokes use.

Reading the chat template had also predicted the wrong answer: Gemma's template extracts
only `text` parts from a tool result, so an image looked doomed. It is not — llama.cpp
lifts media out of *any* message's content parts before templating. The template was the
wrong instrument, the same shape as "use the project's own client, not curl and awk".

**Live matrix** (2026-08-13, each arm a matched pair with a control):

| Engine | Image inside the tool result |
|---|---|
| llama.cpp `gemma-4-31B` + mmproj | works (+51 prompt tokens) |
| Anthropic `claude-haiku-4-5` | works (`tool_result.content` blocks) |
| OpenAI Responses `gpt-5-nano` | works (`function_call_output.output` parts) |
| xAI `grok-4.5` | works (content parts) |
| Gemini `gemini-2.5-flash` | **HTTP 400** — "Multimodal function responses are not supported for this model" |

**Key decisions** (all five forks confirmed by the user on 2026-08-13):

- **Gemini gets a per-provider fallback** (F1-A): the `functionResponse` stays text-only
  and the images follow it as user parts in the same turn. Chosen **statically**, never by
  sending and catching the 400 — a mid-turn retry after a hard failure is exactly what the
  retry decorator refuses once a turn is committed.
- **The domain needed nothing new.** A tool result is a `Message` with role `Tool`, and
  `Message.images` has existed since the first multimodality stage — so no storage, no
  migration, no new persistence path. `ToolCallRecord` gained an image **count** (not the
  pixels) so a reloaded conversation renders the same block as a live one without the feed
  walking to another message to find out.
- **A cap of 4 per result, stated out loud** (F2). An MCP server is third-party code and
  every image it returns rides every later turn. The dropped ones are named in the result
  text: a silent cap reads as "the tool returned four images" when it returned fifty.
- **`tools.mcp_images`, default on** (F3). The MCP double opt-in gates the *server*; this
  gates its *pixels*, because an image carries a hazard text does not — instructions can be
  painted into it, the DATA fencing that protects text has no analogue, and the feed shows
  a chip rather than the picture. Recorded in spec §13.4.
- **The seam is on the tool contract, the producer is MCP only** (F5): `ToolOutcome.images`
  exists for a future built-in screenshot tool, but nothing invents a consumer for a path
  with no producer today.
- **A chip, not pixels** (F4) — `# N image(s) returned` on the tool block, in both the
  collapsed and expanded forms, since it is part of what the call did rather than a detail
  `Ctrl+O` reveals.

**A fixture trap worth recording.** The first version of both smokes ended with a trailing
user question after the tool result. On Gemini that is not merely unrealistic: a tool
result lives in a `user` content and adjacent same-role contents merge, so the question
ended up sharing one content with the `functionResponse` — and Gemini returned an empty
candidate (`STOP`, zero completion tokens) **while still billing the image** (prompt
341 vs 83 tokens). The diagnosis came from the token counts, not the text. Both smokes now
use the shape a real turn has: the question up front, the tool result last.

**Tests.** Unit: the MCP client (an image block is carried, audio and a payload-less image
keep their placeholder, the cap drops extras and says how many), the switch in both
directions, and twelve wire tests — three per backend — pinning that a tool result with no
images serializes byte-identically to before, that the image lands in that provider's own
shape after the result text, and (Gemini) that the `functionResponse` carries no image at
all. Live: `tool_result_image_is_seen_live` (llama.cpp) and
`tool_result_image_takes_the_user_part_fallback` (Gemini) — **both GO**, each with its
control arm; the Anthropic and Responses shapes are covered by the probes recorded in the
research doc plus their wire tests.


### Post-M9: an address policy for model-chosen URLs (done)

**What.** `fetch_url` and the page fetches `web_search` makes now refuse local and private
addresses. Before this, both validated the **scheme and nothing else**: `http://127.0.0.1:8000/`,
the LAN engine on `192.168.1.20`, and the cloud metadata endpoint were ordinary fetches whose
bodies came back to the model as page text. Design and the seven forks, confirmed before
implementation: [fetch-url-address-policy.md](../research/fetch-url-address-policy.md).

**Why it is a different question from `/image attach <url>`**, which was deliberately left
unfiltered three days earlier: there the *user* types the address, in the same box as
`/image attach <path>`, which reads any file on the machine. Here a model picks it, and its
URLs routinely come from a page it just read, a search result, or an attached document —
content this project already treats as untrusted for *instructions*, while it was steering
where the app connects. The trust levels differ, so the answers differ, and both are now
written down where the next reader will ask.

**The guard is bound to the connection, not to a check before it.** The address test lives
inside a `reqwest::dns::Resolve` implementation, so the connector uses exactly the addresses
the policy returned; a "resolve, check, then connect" shape would leave the second lookup
free to answer differently (DNS rebinding). Every address a name resolves to is judged, not
the first — a name with one public and one loopback record is refused rather than raced.

**A guarded resolver turned out not to be a guard, and the wire test is what said so.**
`hyper` parses an IP-literal host itself and never calls the resolver, so `http://127.0.0.1/`
went through a fully "guarded" client and returned `200`. The design had predicted the
mechanism and still shipped the fix as a separate function the *call site* had to remember —
which is a habit, not an invariant. It became `GuardedClient`: the type owns the resolver and
the literal check and hands out `get`/`post`, so the unchecked path is unreachable. The one
exception, `unchecked_inner()`, is for URLs the code itself writes (YouTube metadata) and
says so in its name.

**The refusal names every closed route** (fork F5, lessons §4): not just "cannot reach that",
but that no other tool reaches it either, that retrying by IP, by hostname or through a
redirector is pointless, and that only the *user* can open it — with the new
`tools.web_allow_private` toggle, off by default. Without those clauses the predictable
outcome is a model spending three more round trips on requests that must all fail.

**Tests.** +7 unit (2150 green, 96 `#[ignore]`). The classifier is asserted in **both**
directions — every refused range and its IPv4-mapped spelling, and the public addresses just
outside each boundary — because a classifier that always says "no" would pass a one-sided
table. On the wire, a stub on `127.0.0.1` **counts connections**: with the policy on the
fetch is refused and the count stays **zero**, which is the difference between refusing
before the connection and after it; with `allow_private` the same fetch succeeds and the
count is one. A dead port through the permissive policy proves `was_blocked` does not
mistake an ordinary transport failure for a refusal — without that, every network blip would
reach the model wearing this message. The existing `live_fetch_without_summarize` smoke is
now the positive arm: it fetches a public URL through the guarded default.

**Smoke — GO** (2026-08-13, reference stack: `llama-server` at 192.168.1.20:8000,
gemma-4-31B q4_0). One `fetch_url` call, refused, and the model stopped and explained back
to the user — in the scaffold language — that the address is on a local network its tools
cannot reach, with the stub's connection counter at zero.

**The full `#[ignore]` set was run twice** (this change touches `build_registry`, i.e. how
*every* tool is constructed), and **no single run was 96/96**: run 1 was 95/1 (947 s) with
`live_search_returns_results` returning zero results, run 2 was 94/2 (857 s) with two
`openai::client` smokes failing on `connection closed before message completed` against the
LAN server — while the search smoke passed. Every failure was re-run in isolation and
passed (3/3 and 8/8 respectively), no failure repeated, and the union of the two runs covers
the whole set. Both areas are ones this change cannot reach: the engine clients are outside
the guard by construction (fork F1), and a search result that *parsed to zero results* is
proof the request went out and came back — a blocked provider would have taken the "all
providers unavailable" path instead. The search failure is anti-bot throttling, which the
suite provokes by design: it drives several web smokes from one IP within a few minutes.

**The first version of that smoke measured nothing, and the fixture was the reason.**
Pointed at `169.254.169.254`, the model refused **on its own** without ever calling the tool
("I am not permitted to access internal network addresses or cloud metadata endpoints"), so
the guard was never exercised; only the "the model never called the tool, so nothing was
tested" assertion caught it. Retargeted at a loopback stub the user might plausibly ask
about, it measures the guard rather than the model's own reflexes. Third instance of the
same trap in this project (lessons §2, §9).

### Post-M9: cross-chat search for the assistant (`chat_search`/`chat_read`) (done)
- **The ask, and the constraint that shaped it**: let the assistant find
  information in the profile's other chats, the way the chat-list message
  search works — with the user's explicit expectation that some models will
  abuse such a tool, so it must be toggleable and **off by default**. The
  containment turned out to cost nothing new: `Tool::enabled_by_default =
  false` (the self-model/control precedent) puts the pair in the per-profile
  Tools catalog without enabling it anywhere, `reconcile_tools` never
  auto-adds optional tools, and a disabled tool is not advertised to the model
  at all — no schema in the prompt, no temptation (the S12 rationale). A
  global config gate was considered and rejected (fork F1): two switches for
  one tool invite the "enabled in profile, still off" confusion this journal
  already records. Design and all six decided forks:
  [cross-chat-search-tool.md](../research/cross-chat-search-tool.md), spec §9.11.
- **What the code survey found** (and the design had to add): the full-text
  index (`cache.db`) is **profile-blind** — no `profile_id` column, and the
  whole UI search stack above it is cross-profile *by design* (the chat list
  shows every profile's chats), with the current chat included. Both
  boundaries the tools need existed nowhere. They now live in **one turn
  snapshot**: `ToolContext::other_chats` (the `attachments`/`history`
  precedent), built by `snapshot_other_chats` — current profile only, current
  chat excluded (its visible half is the model's own context; its folded half
  belongs to `history_search`), hidden dropped — and built only when the pair
  is in the turn's tool set. The snapshot doubles as the address book:
  whatever a result names, the companion tool can read.
- **Scoping is in the SQL, not a post-filter** (fork F4):
  `CacheDb::search_messages_in`/`count_matching_messages_in` take the escaped
  query plus the chat-id list (`WHERE chat_id IN`, one placeholder each) —
  a global `LIMIT` filtered afterwards could be starved entirely by another
  profile's rows, which the cache tests pin with a 20-row foreign chat
  against a 2-row cap. `CacheDb` keeps taking an already-escaped query (FSD),
  so the single `to_fts_query` escaper serves its third caller unchanged.
- **The pair composes like the history pair**: hits are grouped by
  conversation (the UI's "group, don't rank" — trigram `bm25` stays a weak
  proxy), conversations by recency, hits by timestamp, and every hit names
  the **page** of the conversation's transcript — computed by rendering the
  hit chat through the same `HistoryView` + `compaction.page_tokens` that
  `chat_read` pages with, so a search address is exactly what a read returns
  (one chat JSON load + render per shown conversation, bounded by `top_k`).
  Conversations are addressed by title plus an 8-hex uuid prefix; `chat_read`
  resolves id-prefix → exact title → substring, reports ambiguity with the
  candidates' addresses (the `attachment_read` ladder), and re-checks the
  loaded file against the snapshot's boundary — a stale entry whose file now
  says "different profile" is refused, not read, pinned by a test that plants
  a secret in the foreign chat. One deliberate deviation from
  `history_search`: `top_k` is capped at 20 — one conversation bounds the
  history pair naturally, a whole profile does not.
- **Tests**: 12 unit tests on the pair (profile isolation through the
  snapshot, the stale-reference belt, addressing rungs, page-address ==
  read-page equality, honest totals, "nothing here" vs "no hits", the
  punctuation-reaches-the-index probe from the history pair's lesson,
  catalog/default-set membership, en/ru descriptions) + 2 on the scoped cache
  queries; suite at **2181 unit / 97 `#[ignore]`**. The demo settings frames
  drifted by one counter (the tool catalog grew 6→8) — dumps and renders
  regenerated, the deterministic pipeline changed exactly the four affected
  frames.
- **Smoke — GO** (gemma-4-31B q4_0 + bge-m3 via llama-server, the user's live
  stack): profile narrowed to exactly the pair *before* seeding — the
  read-back smoke's lesson, since with `note_save` in reach the model files
  the fact into profile memory and never crosses a conversation — a nonsense
  code planted in chat A, the question asked in a fresh chat B. The model
  called `chat_search` once and answered with the exact code; the
  "tool was actually called" assertion guards against a refusal reading as a
  pass (the address-policy lesson). Passed twice (solo, then inside the full
  set). Full orchestrator e2e regression — the change sits on every turn's
  `TurnInfo`/catalog path — **34/34 in 832 s**, no repeats needed.
- **Sonar round** (the PR's first analysis): new-code duplication **4.7%**
  against the 3% bar — the fourth recorded instance of the duplication-gate
  trap (lessons §2), this time all code, no fixtures: `search_messages_in`
  was a scoped twin of `search_messages` (13 lines), `chat_read`'s parameter
  schema mirrored `attachment_read`'s (18), the smoke's bootstrap prologue
  mirrored the read-back smoke's (21), and the one line added to three
  identical background `TurnInfo` builds landed inside already-duplicated
  blocks. Fixed with shared seams, not suppression: one private
  `search_messages_where` with an optional `IN` under both public faces; a
  `paged_read_parameters` helper next to `search_parameters` (the read half
  of each pair now shares its contract the way the search half always did);
  `narrow_profile_to` in the live harness (bootstrap + "only the tools under
  test" in one place); and `Orchestrator::background_tool_ctx` replacing the
  three drift-prone background build sites — the seam a future `TurnInfo`
  field will thank. Unit suite unchanged at 2181; the cross-chat smoke re-run
  green after the cache-path refactor.

### Post-M9: `chat://` as the one address of a conversation (done)
- **The tools' half of the reference track** — the feed half, the design and
  every fork are in [ui-feed.md](ui-feed.md) and
  [chat-uri-links.md](../research/chat-uri-links.md); recorded here because the
  pair is where an address is minted and where the model is taught.
- **The scheme replaced the brackets** (fork F7). `chat_search`'s conversation
  header, `chat_read`'s page header and the ambiguity candidate list now print
  `chat://a1b2c3d4` instead of `[a1b2c3d4]`, and `short_id` moved into the
  shared `features/chat_links.rs` so the string has one producer. One form
  everywhere means the model never has to translate between what a result
  handed it and what it should write back.
- **Teaching is where the address already lives.** Both `desc` strings and
  `tool.chat_search.hint` ask the model to cite `chat://<id>` **when it mentions
  a conversation to the user**, and say why (the interface turns it into a link
  they can open). Deliberately not a new prompt block next to
  `inject_compaction`: while the pair is off — the default — a disabled tool is
  not advertised at all, so this teaching costs exactly zero tokens for every
  profile that never enabled it, and cannot exist without the addresses it talks
  about.
- **The defect the teaching would otherwise have created.** `resolve` stripped
  only `-` before its hex test, so `chat_read("chat://a1b2c3d4")` — the address
  the model had just been told to write — failed the id rung, fell through to
  title matching and answered "unknown". It now reads the reference through
  `chat_links::hex_needle`, which strips the scheme case-insensitively and
  bounds the hex at 4–32 characters; the ladder above it is unchanged, so a
  hex-shaped title still resolves as a title. Pinned by a test that hands
  `chat_read` all four forms of the same reference (scheme, upper-case scheme,
  bare short id, full uuid).
- **Live smoke — GO** (gemma-4-31B q4_0 + bge-m3, the user's stack): the §9.11
  go/no-go gained the citation assertion — the answer must carry a `chat://`
  address resolving to the seeded chat — and passed on the first run, the model
  naming the conversation as `chat://cd1d3e13` off the tool descriptions alone.
  `narrow_profile_to` now returns the bootstrap chat id so the assertion is
  exact rather than "not the current one". Details and the full-set regression
  (34/34, 827 s) in [ui-feed.md](ui-feed.md).

### Post-M9: the code workspace — stages 0 and 1 (done)

A project directory attached to a chat, and the tools that read it: `/project
attach|detach|status` plus `code_list`/`code_read`/`code_grep`. Stage 1 of the
track planned in
[docs/history/code-workspace.md](../../docs/history/code-workspace.md); behaviour — spec §9.12.
Branches `docs/code-workspace` → `spike/code-workspace-probe` →
`feat/code-workspace-core`.

- **Stage 0 was a probe, and it decided the shape of everything after it.** The
  track's one genuinely risky hypothesis was whether a local model honours an
  exact-substring edit contract at all; everything else it plans is composition
  of machinery this repository already has. Measured on a throwaway branch:
  gemma-4-31b **5/5** and qwen-3.6-27b **5/5**, on two arms, against a bar of 3
  of 5. Full record and limits — docs/history/code-workspace.md §7.
- **The second arm is the one that measured anything.** Arm A pastes the `cargo
  build` error, which is how a user actually arrives — and puts the failing line
  *in the prompt*, so a model can assemble `old_string` from the message rather
  than from the file. Arm B quotes no code and puts the obvious fragment in the
  file **twice**, so the fragment can only come from a read and has to be widened
  to be unique. Both arms compile **and run** the fixture with `rustc` as ground
  truth, so a "fix" that deletes the arithmetic cannot pass.
- **What the model did with the read format**: reproduced a five-line fragment
  byte-for-byte from a numbered read — prefixes stripped, indentation intact —
  and widened past the duplicate on its own. That is why the `   12→` shape is
  written down in spec §9.12 as a contract rather than a formatting choice.
- **Attaching is the gate, and it buys the safety property.** The `code_*` tools
  are offered only while `Chat.workspace` is set (`effective_tool_ids` through
  `code::is_workspace_tool`), so a chat without a project sends a request
  byte-identical to what it sent before the feature. No global switch: a second
  toggle would recreate the "enabled but still off" confusion the cross-chat pair
  deliberately avoided (fork F1 there), and the project's presence is a more
  honest permission than a checkbox.
- **The family is deliberately not gated by `tools.fs_enabled`.** `fs_read` is a
  global capability narrowed by an optional `fs_root`; this is the opposite
  shape — scoped to one chat and one directory, refusing when there is none
  rather than widening to the disk. Pairing them would mean attaching a project
  is not enough, which is exactly what the design set out to avoid.
- **`.gitignore` is honoured outside a git checkout too, and that is a
  measurement.** `ignore`'s walker consults it **only inside a repository** by
  default; the fixture that caught this is a plain temp directory with a
  `.gitignore`, where `target/` and `secret.txt` came back in every search until
  `require_git(false)`. On a real Rust checkout that difference is most of the
  bytes on disk.
- **`ToolGates` came out of clippy, not taste.** The workspace flag made
  `effective_tool_ids` an eight-argument function, six of them `bool`, and
  `clippy::too_many_arguments` refused it. The booleans became a named struct
  with `Default`, which also removed the wrong-position hazard at thirteen call
  sites — a test now says `history: true, workspace: true` instead of counting
  commas.
- **The prompt block only describes what the turn actually has.** A project can
  be attached to a profile with the tools switched off; then the block says the
  project is out of reach rather than naming `code_read`. Same rule the summary
  block follows for `history_read`, and the same defect class §9.7 recorded — a
  block promising an absent tool costs the model a turn of improvising.
- **Live smoke — GO** (gemma-4-31B q4_0, the user's stack). Two turns, and the
  first run rewrote the test: asked for a timeout that only exists in the
  project, the model answered correctly from `code_list` + `code_grep` and never
  opened the file — the hit line carries the whole constant, so a read would have
  been a wasted round. The narrow "must call `code_read`" assertion was *wrong*,
  the same way `attachment_search` superseded `attachment_read`'s assertion once
  attachments gained an index (lessons §9). Turn 1 now asserts the outcome; turn
  2 asks for the file's line count and first line, which only a read can answer,
  and the model obliged. A second smoke covers the gate: with no project
  attached, the model made **zero** tool calls and said it has no access.
  Full-set regression on the same stack (gemma-4-31B q4_0 + bge-m3):
  **37 passed / 0 failed in 725 s** — worth running rather than reasoning about,
  since `effective_tool_ids` and `build_request` sit on every turn, not only on
  a turn with a project (docs/lessons.md §9).
- **Scope call:** the settings section the plan listed for this stage moved to
  stage 3. All four planned fields (command timeout, output truncation, the round
  backstop, the semantic index) belong to stages that do not exist yet; a section
  with no field is not a deliverable, and adding one now would ship UI describing
  behaviour the binary does not have.
- **Two findings from CI, both of the classes lessons.md already names.** The
  Linux job went red where Windows was green: `Path::file_name()` knows only the
  host's separators, so a root canonicalized on Windows and read back on Linux —
  which the portable data directory makes a real case — returned the whole path
  as the project's name (now split by hand on both separators, lessons §6). And
  the duplication gate scored **4.3%** against a 3% bar on `project_command.rs`
  alone: the parser is a sliding self-duplicate of `/file`, `/image` and `/rag`,
  the same tokens with different literals — the fourth-recorded shape of that
  trap, and the seam (`features/slash.rs`) is what the rule says to build. The
  three older parsers are deliberately left on their own copies: a mechanical
  refactor does not share a PR with a feature, and they are the seam's obvious
  next callers. Sonar also flagged `tools/probe_runs.py` (`pythonsecurity:S8701`,
  the agentic-workflows family): a test name from `argv` interpolated into a
  `shell=True` command line is an injection sink — now an argv list with
  `shell=False`, plus a name pattern checked before use.
- **Tests**: 2347 unit (+36), 101 `#[ignore]` (+2 stage-1 live smokes; the two
  stage-0 probes stay on the spike branch, which is where the editing contract
  they measure lives until stage 2). New dependencies `ignore` (gitignore
  semantics) and `regex` (already in the graph transitively, pinned to the lock
  version); `grep-searcher`/`grep-regex` were considered and skipped — matching
  line by line over files the walker already opened is a dozen lines, and the
  throughput they buy has no consumer here.

### Post-M9: the code workspace — stage 2, editing and the journal (done)

`code_edit` / `code_write`, the change journal every edit is recorded in, and the
round-limit exemption. Stage 2 of
[docs/history/code-workspace.md](../../docs/history/code-workspace.md); behaviour — spec §9.12.
Branch `feat/code-workspace-edit`.

- **The contract is the one stage 0 measured**, unchanged: an exact fragment that
  must occur once, with the two refusals — missing, and ambiguous — as its
  working half. What is new is that a refusal now provably writes nothing, which
  is a test rather than a hope.
- **The journal is the load-bearing addition.** Before a file is changed for the
  first time in a chat, its bytes go to `data/workspace/<chat-id>/`. Three
  decisions inside it: only the **first** touch is recorded (the baseline is "as
  it was before the assistant started", so a second edit must not move it); the
  baseline file is named by a **hash** of the relative path (a path is not a file
  name — it carries separators, and on Windows characters a name cannot hold);
  and a change that cannot be journaled is **refused rather than applied**,
  because the user's control over what the assistant did is the changes screen
  and its revert, and both rest on bytes that exist nowhere else once the file is
  overwritten. It is not git, deliberately: "what did the assistant do" differs
  from "what differs from HEAD" in both directions — an attached directory need
  not be a repository, and a repository routinely carries the user's own
  uncommitted work.
- **"Exempt" and "unbounded" are not the same promise.** The user's requirement
  was that the tool-round limit must not reach the development tools, and it does
  not. But writing the test for it surfaced the other half: a model that repeats
  one exempt call leaves a turn that never ends, burning a cloud provider's
  tokens with only `Esc` to stop it — and a repeated tool call is a *measured*
  failure mode here, not a hypothetical (5 in 20 on one family, in the
  second-chat-model track). So the exemption is bounded by
  `WORKSPACE_ROUND_CEILING = 200`, an order of magnitude above any real fix,
  ending the turn exactly the way the ordinary limit does. The configurable
  backstop stays in stage 3 with the other numbers.
- **A round counts if *any* call in it counts**, so mixing a `web_search` into a
  round of reads still spends one. Otherwise the exemption would be a way round
  the limit rather than an exception to it.
- **Two defects the tests found, both real.** `code_write` could not create a
  file in a directory that did not exist yet — its own description promised
  otherwise — because the path resolver canonicalized only the *immediate*
  parent; it now walks to the deepest existing ancestor and re-attaches the tail,
  with a test that a `..` in that tail cannot escape (it has no `file_name`, so
  the walk ends in an error). And the round-limit exemption had been declared on
  the two new tools only, leaving stage 1's readers still spending the budget —
  caught by a test asserting the property for the whole family rather than for
  the tools the change happened to touch.
- **Live smoke — GO** (gemma-4-31B q4_0, the user's stack). The stage-0 scenario
  now runs on the real mechanism: a project that does not compile, fixed through
  `code_edit`, with `rustc` compiling **and running** the fixture as ground truth
  so a "fix" that deletes the arithmetic cannot pass.
  Full-set regression on the same stack: **40 passed / 0 failed in 774 s** — run
  because the round-counting change sits on *every* turn, not only on one with a
  project attached (docs/lessons.md §9).
- **The commitment stage 0 left open could not be met, and that is the finding.**
  Stage 0 recorded that the refusal paths had never fired live and asked stage 2
  for a smoke that provokes a miss. Three fixtures were built to force one: a
  user quoting the target line with a space missing, a file with the obvious
  fragment twice, and the compile-error scenario. **Zero misses in seven live
  runs.** The model normalizes an approximate quote to what the file says, and
  includes the constant's name so its fragment is unique — because it reads
  first, and a read tells it the truth. The refusal messages are therefore
  insurance, not a hot path: they keep their unit coverage (including that
  nothing is written), and the live smokes assert the **outcome** — an
  approximate quote must not cost the user their change — while *reporting* the
  miss count, so the rate stays visible instead of assumed.
- **The `git checkout --` trap, for the fourth time**, in the session that had
  the lesson in context: undoing a one-line mutation experiment took the whole
  uncommitted file with it. It cost nothing only because every edit had been
  applied by scripts kept outside the tree. docs/lessons.md §1 now says to commit
  *before* mutating and treats "I will revert it after" as the moment to commit.
- **The duplication gate caught the family, and the fix was structural.** Five
  `impl Tool` blocks differing only by a label, a bundle key and a schema are the
  same tokens with different literals — 5.1% against a 3% bar, the sixth recorded
  instance of that shape (docs/lessons.md §2). Neither escape the lesson names was
  available: this codebase has no `macro_rules!` anywhere, and a blanket
  `impl<T: WorkspaceTool> Tool for T` is refused by coherence. So the repetition
  was removed rather than disguised: **one** `CodeTool` enum with one `impl Tool`
  dispatching to five free functions. The registry now loops `code::ALL`, so a
  new tool cannot be registered without joining the family's list.
- **A doc/code mismatch the same pass uncovered.** Stage 1's spec text said the
  family is "off by default in the catalog"; the code has never overridden
  `enabled_by_default`, so it is **on**. The code is right — the project's
  presence is the permission, and requiring a directory *and* five toggles would
  contradict it — so the sentence was corrected rather than the behaviour.
- **Tests**: 2366 unit (+12), 104 `#[ignore]` (+3). Demo dumps regenerated (two
  more catalog tools move the displayed count), and the screenshots with them.

### Post-M9: the code workspace — stage 3, the build/run/test command slots (done)

`code_build`/`code_run`/`code_test`, the `/project *-cmd` commands that fill
their slots, and the process-tree kill underneath them. Stage 3 of
[docs/history/code-workspace.md](../../docs/history/code-workspace.md); behaviour — spec §9.12.
Branch `feat/code-workspace-commands`.

- **The load-bearing decision was made before this stage and this stage is where
  it pays**: the model **cannot compose a command**. The three tools take no
  arguments at all — `{"type": "object", "properties": {}}` — so there is nothing
  to inject into, no flag to append and no second command to chain. What the
  model *can* do is read the line, which the system block quotes verbatim; that
  is what lets it tell the user their own command is the thing that is wrong, and
  it is the whole of its say over one. A test-filter argument is the first thing
  to want and the first injection vector, so the test slot itself is the answer.
- **Both halves of the runner already existed in the repository, in places
  `features` cannot reach.** The Windows Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` was `shared/mcp.rs::JobGuard`, written for
  `cmd /c npx → node`; the shell-style argv splitter was
  `screens/settings/helpers.rs::parse_args`, and FSD forbids importing `screens`
  from `features`. Both were hoisted rather than copied (`shared/proc.rs`,
  `shared/cmdline.rs`) — the stage-1 `FsRoot` precedent, and the rule
  docs/lessons.md §2 states about a parser written twice. MCP's behaviour is
  byte-for-byte unchanged: it still calls `TreeGuard::assign`, which is the
  Windows job and nothing else.
- **The unix half is the genuinely new code, and it is opt-in for a reason.**
  `process_group(0)` at spawn plus `killpg(SIGKILL)` reaches the grandchildren;
  `TreeGuard::assign` (MCP's constructor) deliberately does **not** take a group,
  because giving a server its own process group changes how signals reach it —
  a behaviour change unrelated to running a build, and not one to smuggle into a
  feature PR. The two constructors also encode a safety property in the type: a
  guard with no group remembers no pgid, so `killpg` cannot be reached with the
  application's *own* group id.
- **Dropping the guard kills, and that is not an accident.** Cancellation
  (`Esc`) drops the tool's future, so no cleanup code of ours runs at all —
  `Drop` is the only hook left. The counterpart is `disarm()` after the child has
  been reaped: from that moment its pid is free for the OS to reuse, and a late
  `killpg` would reach a stranger.
- **The tree kill is a measured property, not a hope.** A fixture spawns a parent
  that spawns a grandchild writing to a file forever; the test waits until the
  file is actually growing (otherwise it could pass by killing a tree that never
  grew one), kills, and asserts the file stops growing. Mutation-checked in both
  directions: removing `guard.kill()` and leaving `child.start_kill()` — exactly
  what `kill_on_drop` would give — turns it red. That is `cargo build` leaving
  `rustc` behind, reproduced in a unit test.
- **A pipeline is refused when the line is *set*, not when a model first runs
  it.** `cargo build 2>&1 | tee log.txt` cannot work in an application that
  spawns the program itself, and without a check it would surface three turns
  later as "cargo: program not found" — which reads as a broken toolchain. The
  refusal names the character it found and the route that works (put the steps in
  a script, name the script). The same check runs again inside the tool, because
  a chat file is JSON on disk and can be edited by hand, and a guard living at
  one end of a path is one refactor from being gone (docs/lessons.md §2). Quoted
  metacharacters stay legal: `grep "a|b" src` is a correct command line, and
  refusing it would be the check becoming a nuisance.
- **Partial output is kept on a timeout** — the deliberate inverse of the Python
  sandbox, which discards it. A script's value is its final answer; a build's
  first errors arrived in its first second, and throwing them away because the
  build was slow wastes the whole wait. Truncation keeps head **and** tail for
  the same reason (fork F12): a compiler puts its first errors at the top and its
  summary at the bottom, so a tail-only cut keeps the *count* of errors and drops
  the errors.
- **`workspace.max_rounds` supersedes stage 2's constant, and the user chose the
  shape.** Both options the design offered read badly — a number that cannot be
  raised is a wall with no door, and a `0` meaning "the built-in default" is a
  field lying about its own value. So `WORKSPACE_ROUND_CEILING = 200` became the
  setting's default of **500**, and `0` genuinely means no limit: a legitimate
  choice for a long refactor, with `Esc`, the per-command timeout and the
  one-at-a-time gate still underneath. Writing it also exposed a message defect
  of exactly the class docs/lessons.md §4 names — the exhaustion note always
  quoted `max_tool_rounds`, even when the *workspace* ceiling was what fired,
  sending the user to a setting that was not the problem. It now names which of
  the two ended the turn.
- **Two more messages fixed in passing, same class.** The system block still
  named only `code_list`/`code_read`/`code_grep`, a stage behind: stage 2 added
  the editors and never updated it. Rather than adding two more names to a
  sentence that will drift again, the block now lists the tools **this turn
  actually offers**, one gloss each, derived from the turn's own tool set — so a
  profile with the editors switched off cannot be told it can edit, and a slot
  with no line contributes nothing.
- **The console shape has one owner now.** `python_exec::format_output_parts`
  built the `stdout:`/`stderr:`/exit-code text and `present::parse_console` read
  it back, in two files. The command tools need the same shape plus a `command:`
  section, so the assembly moved next to the parser (`present::format_console`)
  and `python_exec` delegates to it — the two halves of one format cannot drift
  when they are ten lines apart. Truncation stayed with each caller, because that
  is the one part they genuinely disagree about.
- **The tests' own gate had to be dealt with honestly.** `COMMAND_GATE` is
  process-wide by design, so two `#[tokio::test]`s spawning commands in parallel
  make each other fail with the "busy" refusal, and which one loses depends on
  the scheduler. A `SERIAL` mutex makes them queue, and the gate's behaviour is
  asserted deliberately by its own test rather than observed as flake.
- **Reviewing the stage's own diff found five things, one of them a real
  defect.** A command we kill has **no exit code**, and the result was reporting
  a fabricated `-1` that the model would have passed on to the user as the
  command's own — the header already says it timed out, so the failure line is
  now suppressed instead. The other four are the shapes this project keeps
  recording: `/project <slot>-cmd` spelled by two identical helpers (now
  `CommandSlot::setter_command`), the `python()` test probe written twice minutes
  apart (hoisted to `shared::proc::test_python` — docs/lessons.md §2's own rule
  about the second test starting like the first), a note built before the
  mutation and patched afterwards, and the round-budget test still asserting
  `calls > 1` where it could assert the ceiling **came from the setting**
  (`max_rounds: 3`, and the count is a number only the setting can produce).
- **Adding two rows to the help overlay broke two render tests, and one of them
  was right to break.** A 44-character command label (`/project
  build-cmd|run-cmd|test-cmd [line]`) widened the aligned label column and
  re-wrapped every description on the tab — so the labels shrank to the width of
  their `/project attach` neighbour. The second failure was real: the commands
  tab has outgrown one screen, and an assertion reading it at scroll 0 was an
  "everything fits" assumption that any new command falsifies. Reading it at both
  scroll extremes was not enough either — five rows in the middle were invisible
  in *both* frames, an absence that reads as a missing command; the frame is now
  tall enough that the two overlap.
- **Live smoke — GO** (gemma-4-31B q4_0, the user's stack). The stage-3 loop ran
  as designed on the first attempt: `code_build` → the real `E0277` from `cargo`
  → `code_read` → `code_edit` → `code_build`, green. `cargo build --offline` on a
  dependency-free fixture crate rather than a bare `rustc` (the stage-0 and -2
  choice), because only a real build system puts a `cargo → rustc` tree behind
  the kill this stage exists for, and only a real compiler's diagnostics are what
  the model has to read. Ground truth on both sides: the fixture must fail to
  build before, and `cargo run` must print the right number after, so a "fix"
  that deletes the arithmetic cannot pass. A second smoke covers the finer gate —
  with no slot configured, the model asked for tests, found none, and called none
  of the three tools, because they do not exist that turn.
  Full-set regression on the same stack: **41 passed / 1 failed in 834 s**, and
  the one failure — a history read-back smoke — **passed on isolated re-run**.
  Read as docs/lessons.md §9 says to read it: the union of the runs is green and
  no failure repeated. The diff supports that reading rather than only the
  re-run doing so — round counting for *counting* tools is byte-for-byte what it
  was (`self.round >= self.max_rounds`, unchanged), the history gate is
  untouched, and the compaction block is built by the same call as before. Worth
  running at all because `effective_tool_ids` and `build_request` sit on every
  turn, not only on one with a project.
- **Tests**: 2388 unit (+22), 106 `#[ignore]` (+2). Demo dumps regenerated (three
  more catalog tools move the displayed count), and the screenshots with them.
  New dependency `libc`, unix-only and one call wide (`killpg`); the Windows half
  of the same job is the `windows-sys` Job Object already in the graph.

### Post-M9: the code workspace — stage 5, the semantic index, measured and rejected (done)

The track's last planned stage does not ship. Fork F4 made it conditional on a
live measurement against `code_grep`; the measurement came back no, and the
rejection is the deliverable. Full record —
[docs/history/code-workspace.md](../../docs/history/code-workspace.md) §7.9;
the probe lives on `spike/code-search-probe`, unmerged.

- **The verdict.** Over 48 turns per arm on gemma-4-31b, with the shipped tools
  as the control and the same plus `code_search` as the treatment: correct in
  16/22 of the control's turns that looked at the project against 19/29 of the
  treatment's — and 16/48 against 19/48 of *all* turns. **The two denominators
  point in opposite directions**, and choosing between them is a judgement about
  what an unusable turn means. An effect that changes sign with a definition is
  smaller than the instrument measuring it. Calls: 111 against 127. The
  criterion was "measurably improves answers or reduces rounds"; it does
  neither.
- **The instrument took more work than the thing it measured, and needed six
  revisions — every one found by reading the raw turns, never by reading the
  summary table, which looked plausible each time.** In order: the tool was
  never called at all (the workspace block enumerates the turn's tools in words,
  is built from `code::ALL`, and the model believed it over its own schema
  list); one pass is not a rate (the control scored 5/5 then 3/5 on the same
  questions); two questions were padding (zero calls in both arms); "wrong"
  conflated a retrieval miss with an empty turn and with a turn that never
  looked; the grader had false positives, then — once tightened — false
  negatives; and the harness was starving both arms at the default
  `max_tokens`.
- **The first defect is the one worth carrying elsewhere.** The workspace block
  was written across stages 1–3 against the opposite fear — that it must never
  promise a tool the turn does not have (§9.7's lesson). This measured the
  converse and it is just as strong: **a tool the block does not name does not
  get used**, even though its schema is right there in the request. The block is
  not documentation, it is the model's working inventory.
- **The fifth is a limit, not a bug.** A keyword grader cannot resolve an
  open-ended prose answer. Loose markers graded a general-knowledge guess correct
  on a turn with no tool calls; markers tightened to source-only literals graded
  "machine-bound encryption … ADR 0008" wrong for missing `dpapi`. Every marker
  set trades one error class for the other, and the residual error is the size of
  the effect being chased. Resolving this needs a judge, not a better list.
- **The sixth was already written down in this repository.** The probe asked
  open-ended questions at the default `max_tokens` of 2048 with thinking on,
  while docs/journal/ci.md had measured that exact ceiling on that exact class of
  prompt (1024: 0/3, 2048: 2/3, 4096: 3/4). Raising it to 4096 moved the per-arm
  numbers and did **not** reduce the empty turns — half of every arm's turns are
  unusable either way, which is the real ceiling on what a probe of this shape
  can resolve.
- **What the no-go buys.** No `cache.db` schema and migration, no background
  indexing task with progress reporting, no incremental reindex on every edit, no
  settings toggle, no `/project reindex`, and no hard dependency on the embedding
  server for a feature that otherwise does not need one. Permanent cost, for an
  effect that could not be detected.
- **What it does not settle.** Not that a semantic code index is a bad idea —
  that one could not be shown to help *here*: a corpus commented in unusually
  discursive English prose, which is exactly what makes `code_grep` strong, read
  by a 31B local model whose turns are unusable about half the time. The one
  effect that survived every revision is not the one F4 asked about: with a
  search tool available the model answers **without looking at the project** far
  less often (8 turns against 15). Shipping on that would be shipping on an
  unmeasured basis, which is what the fork exists to prevent.
- **Tests**: none added to the application — nothing shipped. The probe carries
  four unit tests of its chunker on its own branch.

### Post-M9: the change journal followed the chat, not the project (done)

- **Found by pulling on a loose thread, not by a report.** A dead `Journal`
  binding in `journal_before_write` (built, never used, explicitly dropped —
  born that way in the code-workspace stage-2 commit) prompted the question
  "what was that meant to do". It was meant to do nothing — `record` does its
  own `load` and its own first-touch check. But the *journal operation that is
  missing* turned out to be one call site away, on attach. Branch
  `fix/workspace-journal-reattach`.
- **Design fork F13 was specified and half-built.** The plan says it plainly
  (docs/history/code-workspace.md §3.5: "attaching a different root starts a
  fresh manifest") and the code does it — inside `Journal::record`, so the
  reset fires on the assistant's **next write** in the new project. Nothing
  fired on attach, and neither reader (`workspace_diff::build`, `::revert`)
  ever compared the manifest's recorded root with the one it was handed:
  `workspace_paths` takes the root from the chat's *current* workspace, while
  the journal is keyed by *chat id*. So between `/project attach B` and B's
  first edit, `F4` listed A's journaled paths resolved against B.
- **Measured before fixing, because "confusing" and "destructive" are
  different bugs.** Most stale rows come back `Gone`, which is only confusing.
  A path both projects have does not: sibling repositories share `Cargo.toml`,
  `README.md`, `src/main.rs`. The reproduction (A edits `Cargo.toml`, attach B,
  press `r`) ended with `B/Cargo.toml` holding **A's** bytes and A's own file
  still unrepaired. That is the hazard the writer's own comment already named
  — "a stale entry would make the changes screen offer a revert into an
  unrelated directory" — with the comment sitting in the one place that could
  not prevent it.
- **Fixed at the attach *and* in both readers**, deliberately not just the
  one-line attach fix. The bug was not a missing line; it was an invariant that
  lived only in the writer, so any new caller could undo it. One predicate now
  answers "whose project is this" — `Journal::describes(root)` over a free
  `same_root`, shared with `record` so writer and readers cannot disagree.
  `build` answers with an empty set (truthful: nothing changed in *this*
  project), `revert` refuses with a reason (an action must not silently
  no-op, docs/lessons.md §4), and `handle_project_attach` clears the journal
  best-effort, mirroring detach.
- **`same_root` is fail-safe, not fail-open, and the asymmetry is reasoned.**
  Plain string equality answers every normal case — both spellings come from
  canonicalizing helpers that also strip the Windows `\\?\` prefix
  (`canonical_dir`, `workspace_root`), and the pre-existing end-to-end test
  `the_change_set_round_trips_through_the_orchestrator` is what proves they
  agree, since a mismatch there would have emptied a real change list. A
  differing spelling gets a second chance through `canonicalize`; anything
  else counts as a different project. Guessing "same" wrongly puts bytes in
  the wrong file, while guessing "different" wrongly hides a list whose files
  a revert could not have resolved anyway (`workspace_diff::resolve`
  canonicalizes the root and refuses when it is gone).
- **Re-attaching the same directory keeps the journal** — typing `/project
  attach .` twice must not throw away the record of what the assistant did.
  Its own test, and it passes with the fix reverted, which is what makes it a
  guard rather than a restatement.
- **Five tests, and the three that assert the fix were run against the
  unfixed code first**: `another_projects_journal_is_neither_shown_nor_
  reverted` (the reproduction, now asserting B's file is untouched),
  `re_attaching_a_different_project_drops_the_old_journal` (orchestrator
  level, through `AppCommand::ProjectAttach`),
  `a_journal_says_which_project_it_describes`,
  `one_directory_spelled_two_ways_is_still_the_same_project`, and
  `re_attaching_the_same_project_keeps_the_journal`. With the guard neutered
  the first three fail and the last two pass — a regression test that passes
  either way is decoration.
- **Verification**: **2423 unit tests green** (0 failed, 106 `#[ignore]` — the
  2418 baseline plus five), clippy `-D warnings`/fmt and every repository gate
  clean. **Live run — GO** (AGENTS.md §3): the change is on the code-workspace
  tool path, so the whole orchestrator e2e set ran against the LAN stack
  (Gemma 4 31B q4_0 on `192.168.1.20:8000`, bge-m3 on `:8001`) — **42 passed /
  0 failed** in 862 s, the seven `code_*` smokes among them, which is what
  exercises journaling through real tool calls rather than through a fixture.
- **Left open, deliberately**: `JournalEntry::first_touched_at` is written and
  serialized but read by nothing (only the `Serialize` derive keeps rustc
  quiet); the plan specifies it in the manifest row, and `entries()`'s
  "oldest touch first" comes from `Vec::push` order rather than from the
  field. Sort by it, surface it, or drop it — not decided here. Separately,
  docs/history/code-workspace.md has drifted from the code in five further
  places (`code_list`'s `glob`, `code_grep`'s `max_results`,
  `ToolGroup::Workspace`, `/project status`'s change count, detach keeping
  the journal) where **spec §9.12 matches the code** — plan drift to reconcile
  in a docs pass, not defects.

### Post-M9: the code workspace's loose ends (done)

- **The three things the F13 fix deliberately left open**, closed together
  because they are one feature's tail and none is big enough for a PR of its
  own. Branch `fix/workspace-loose-ends`.
- **`JournalEntry::first_touched_at` was written, serialized and read by
  nothing** — only the `Serialize` derive kept rustc quiet about it. Of the
  three options (sort by it, surface it, drop it) the choice is **sort**, and
  it is the same argument the previous entry's lesson makes: `entries()`'s doc
  comment promises "oldest touch first" while what kept that promise was
  `Vec::push` order in a *different* function. Push order gives the same answer
  today, which is exactly why it was not enough — a manifest is a JSON file on
  the user's disk. `entries()` now sorts by the field (stably, so two edits
  inside one second keep their written order), and the field is load-bearing
  instead of decorative.
- **The same field was missing `#[serde(default)]`, and that was a real
  failure mode**, not a convention nit — measured before deciding: a manifest
  row without the key fails the whole `Manifest`, `load` swallows the error by
  design ("a corrupt manifest must not make the workspace unusable"), and the
  user's entire change list disappears while its baselines stay on disk with
  nothing referencing them. The default is the Unix epoch, which sorts first on
  purpose: a row whose time was never recorded *is* the oldest thing known
  about that file, and saying so beats inventing "now". No released build ever
  wrote such a manifest, so no CHANGELOG entry — this is tolerance for a
  hand-edited file, not a bug anyone hit.
- **The dead `Journal` binding in `journal_before_write` is gone.** Established
  first that it was inert rather than assuming it: `Journal` is a one-field
  `PathBuf` wrapper, `new` does no I/O, and there is no `impl Drop` for it
  anywhere in the crate (all eight were listed). It was born dead in the
  stage-2 commit — the shape of a first draft where `journal.record(…)` was
  inline before moving into `spawn_blocking` — and the explicit `drop(journal)`
  is why clippy never mentioned it. Kept out of the F13 PR on AGENTS.md §2's
  rule and merged here instead.
- **Plan drift recorded rather than papered over** — a new
  docs/history/code-workspace.md §7.10, following the §7.5–§7.8 convention of
  writing deltas down instead of editing the plan's body, which would erase
  what was actually planned. Five places where the finished code differs from
  §3.1/§3.2 (`code_list`'s `glob`, `code_grep`'s `max_results`,
  `ToolGroup::Workspace`, `/project status`'s index state and change count,
  detach keeping the journal), each verified against the source, and in every
  one of them **spec §9.12 already matched the code** — so the spec needed no
  change and the plan is the stale document. One of the five, the index state,
  is not drift at all but §7.9's no-go.
- **Two tests, both run against the unfixed code first**:
  `entries_come_back_oldest_touch_first` (a manifest reordered by hand, plus an
  epoch-stamped row appended last) and
  `a_row_without_a_timestamp_does_not_blank_the_journal` — both FAILED without
  their fix and pass with it. Deleting the dead binding needs no test: the
  existing `code_edit`/`code_write` journaling tests already cover the function,
  and a deletion that changed behaviour would have broken them.
- **Verification**: **2425 unit tests green** (0 failed, 106 `#[ignore]` — the
  2423 baseline plus two), clippy `-D warnings`/fmt and every repository gate
  clean. **Live run — GO** (AGENTS.md §3): the deletion sits on the tool write
  path, so the whole orchestrator e2e set ran against the LAN stack (Gemma 4
  31B q4_0 on `192.168.1.20:8000`, bge-m3 on `:8001`) — **42 passed / 0
  failed** in 846 s, the seven `code_*` smokes among them. No CHANGELOG entry:
  the sort is invisible (push order already gave it), the serde tolerance
  guards a file no release ever wrote, and the deletion is inert.

### Post-M9: sub-agent chats, PR 2 — the sub-agent with the agent's tools (done)
- **What**: PR 2 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §7,
  [ADR 0010](../decisions/0010-subagent-nested-turn.md)). `call_subagent` grew
  from one tool-less request into a **nested turn**: a child `TurnLoop` over the
  turn's `TurnShared` (PR 1's seam), with the turn's effective tools minus
  `call_subagent`, `history_read`/`history_search` and the self-model family
  (`subagent::withheld_from_subagent`), the parent's environment (a cloned
  `ToolContext` under the sub-agent's persona and sampling, `history: None`, a
  child cancellation token; a request from `build_request_in` over the parent's
  `RequestEnv`), and a **muted** event sink — only the token counter passes,
  re-based on the parent's total. The transcript — `User(message)` and the
  run's rounds exactly as any chat stores them, plus persona, title, `name`,
  outcome, tokens, an id for `chat://` — lands on the call's `ToolCallRecord`
  (`entities/subagent.rs::SubagentRun`, additive) and reaches `Chat` with the
  turn. The result text is the final reply plus a `chat://` trailer, and why
  when the run did not complete (cancel / timeout / engine failure / round
  budget). New optional `name` argument (the persona's display name → the
  initial title, else the first line of the message).
- **Key decisions** (the research's forks, all decided 2026-08-23): a
  loop-executed tool (the control-tool precedent) rather than a runner trait
  or a second `start_generation` — a `Tool` cannot reach the registry, the
  confirmation channel or the UI sender, and the main loop's behaviours
  (confirmation, signatures, control tools, effects, images) are exactly what
  a tool-using sub-agent needs; the transcript **on the record, inside the
  parent's file** (the user's clarification: inseparable, removed only by
  `Ctrl+E`/`Ctrl+R` of the spawning exchange — which the record's placement
  gives by construction, no cascade code); the child lands with the turn, no
  mid-turn channel (stage 1); effects by kind — identity to the run,
  `AddAttachment` to the parent; the self-model excluded whole; one time knob
  for the whole run.
- **Two things the implementation found that the plan did not say.** The
  async chain `run → tool_round → execute_call → resolve_call_result →
  run_subagent → run` is recursive and needs one `Box::pin` (at the child's
  `run()`); and `stream_round` had to stop taking `evt_tx` directly — every
  event the loop emits now goes through `RoundSink`, which is what makes
  muting one line rather than a flag threaded through nine sends. The
  `Tool` impl's `invoke` stays reachable by the background loops in principle;
  it validates the arguments and answers "loop only" instead of running a
  tool-less request that would now be a second, different behaviour.
- **Settings**: `tools.subagent_timeout_secs` (one request) → `subagent_run_timeout_secs`
  (600 s, the whole run); `subagent_max_tokens` 1024 → 4096 by default;
  `ToolConfig` lost its two sub-agent fields (the loop reads the limits from
  `config.tools` through `GenSpawn`). `SETTINGS_SCHEMA` 1→2 is the scaffold's
  **first real step** — see the storage journal.
- **Live run — GO.** `subagent_with_tools_e2e_live` (external Gemma 4 31B
  Q4_0, `llama-server` at the LAN host): the parent is told to delegate
  reading a sandboxed file with a planted nonsense token; the sub-agent must
  use `fs_read`. **5/5**: every run the parent delegated once, the sub-agent
  (which named itself «File Reader» every time) made exactly one `fs_read`
  call and answered, and the token reached the parent's reply; 120–188 tokens
  per run. Qwen 3.6 27B was not run then — the only GPU was serving the Gemma
  instance. **Run after the track closed (2026-08-23, Qwen3.6-27B Q4_K_M on
  the same `llama-server`, the code of #363): GO** — one delegation, one
  `fs_read`, the token in the parent's reply, 271 tokens, 25 s; the
  cross-chat smoke (`cross_chat_search_answers_from_another_chat_live`) GO
  too, 45 s. One behavioural difference from Gemma: Qwen passed **no `name`**,
  so the transcript's initial title fell back to the instruction's first line
  (F13) — which is exactly the case the landing auto-title (PR 6) exists
  for; the smoke runs with titling off, so the long title is what it shows.
- **Tests**: 2460 green (+13: the entity's additive round trip and
  `final_reply`; the argument parser, the initial title and the loop-only
  refusal; six orchestrator tests over a recording, scripted engine — the run
  on the record with the child's request inspected, no nesting, identity
  effects on the run, a 1-second run timeout landing a partial run,
  `Esc` landing `Cancelled`, an empty `message` refused without a run; the
  three settings-step shapes), 107 `#[ignore]` (+1, the live smoke). The
  settings screenshots were regenerated for the renamed field.

### Post-M9: `web_search` said "no results" while it was blocked, again (done)
- **The report**: search works "in 50–70 % of calls", and on the rest the model
  announces that anti-bot measures are cutting it off and goes to the primary
  sources it already knows. The ask was whether keyed providers would help.
  Measuring it first turned up two problems wearing one coat, and only the
  second is about capacity. Full survey, provider comparison and the decided
  forks: [web-search-keyed-providers.md](../research/web-search-keyed-providers.md).
- **The defect: the body-phrase captcha check expired.** `is_challenge_page` was
  added the last time this bit (P4, above) and matched five whole phrases in the
  body. Measured 2026-08-25 from the reporting machine: **Mojeek's block page now
  carries none of them** — `HTTP 200`, `<title>Captcha</title>`, body "Please
  prove you are human". So the page parsed to zero results, was not recognised as
  a challenge, `got_clean_page` was set, and `no_results_outcome` took the
  honest-emptiness branch. The app's own live smoke reproduced it exactly:
  `live_search_returns_results` **panicked with "no results"** rather than taking
  the throttled path it is written to skip on. Third instance of
  [lessons §4](../lessons.md)'s recurring class in this one function — a message
  that closes the wrong door — and the second time on this exact line.
- **The anchor moved to the `<title>` element.** A title names what a page *is*
  and survives a rewrite of its prose; `CHALLENGE_TITLES` is therefore allowed to
  be short and generic (`captcha`, `just a moment`, `attention required`, …)
  where the phrase list could not be, because two guards stand in front of it:
  the check still runs **only on an empty parse**, and a title **containing the
  query** is declared a results page whatever else it says — which is precisely
  the "somebody searched for captchas and found nothing" case the phrase list was
  contorted to avoid (`captcha bypass - Mojeek Search` reads as results; the same
  page under an unrelated query reads as a block, and that trade errs toward
  "retry later" rather than toward "the web knows nothing"). The body phrases stay
  as a second signal for generically-titled pages, so this is strictly additive
  in coverage.
- **The chain also stopped re-paying dead round trips.** A family that answered
  with a block is remembered (`mark_blocked`) and moved to the back of the order
  for `PROVIDER_COOLDOWN`; `provider_order` reorders, it never skips — skipping
  would let a stale cooldown report "everything is throttled" with no request
  having gone out, which is the same lie in the opposite direction.
  `Provider::family` is what the cooldown is keyed by, because **`lite.` and
  `html.duckduckgo.com` share one per-IP throttle** (measured: with lite already
  blocked, html answered `202` on its first request) — so the chain of four
  entries was always a chain of **three** independent providers, and every call
  restarted at the blocked one. Five minutes is a judgement call, not a
  measurement, and says so at the constant.
- **What was measured, and what could not be.** From a residential IP, six
  back-to-back requests per provider: DDG lite answers ~2 then `202`+`anomaly`,
  Mojeek ~2 then the `200` captcha, Ecosia `403` at once. **A block outlives
  fifteen minutes of complete silence** — `web.rs` called throttling
  "short-lived", and it is not. It is also **not fingerprinting**: with the IP in
  the penalty box, a real Chrome driving the same pages got the same block
  (`anomaly` present; `<title>Captcha`). That is why the browser-like headers
  added to the provider request (`Accept`/`Accept-Language` — `fetch_content`
  already sent them, `fetch` sent a bare `User-Agent`) are recorded as hygiene
  with no promises attached. The keyless alternatives were surveyed and are
  exhausted: Yep `403`, Marginalia a queue interstitial over a tiny index,
  Startpage and Brave-without-a-key captchas, public SearXNG instances `403` or
  JSON disabled. **One question this IP could not settle**: with every provider
  serving a block page, a stale CSS selector and a blocked response are
  indistinguishable — both parse to zero. The four selector sets are therefore
  unverified against the live sites and must be checked from a clean IP.
- **Tests**: 2586 green (+7), 107 `#[ignore]`; clippy `-D warnings`, fmt,
  `cyrillic_scan`, `link_check`, `doc_index_check` clean. **Mutation-tested**:
  removing the title anchor fails both title tests, removing the
  title-contains-query guard fails the fruitless-search test, and giving the two
  DDG entries separate families fails both ordering tests — each exactly its own.
- **Live run — GO** (network only, no model needed). The criterion was that the
  conditions which produced "no results" must now produce the honest error, and
  they do: the same smoke on the same blocked IP now prints *"skip: every search
  provider is throttling this IP"* where before it panicked on the false
  negative. The positive arm (a search returning results) could not be re-run —
  the IP was still blocked from the measurements above.
- **The keyed providers are the next PR, not this one** (user's decision,
  2026-08-25): Tavily and Brave, tried before the free chain when a key is
  present, with the free chain kept as the last fallback and the no-key default.
  This PR is the defect and needs no key.
