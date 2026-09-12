# Journal — Tool system

The `Tool` contract and the tools built on it: files, web search and page fetch, the Python sandbox, MCP servers, speech, YouTube, the control tools, and the confirmation gate in front of the dangerous ones.

**Reference documents for this area:** architecture.md §8, spec.md §9

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (70)

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
- Post-M9: `web_search` keyed providers — Tavily (done)
- Post-M9: the two-agent dialogue — research and the stage-0 live probe (done)
- Post-M9: `run_dialogue` — the directed dialogue, stage 1 (done)
- Post-M9: `run_dialogue` — the live transcript, stage 2, track complete (done)
- Post-M9: `get_llm_name`/`get_llm_history` — the language model, named and dated (done)
- Post-M9: the Python package pinned exactly — a selector is a range (done)
- Post-M9: the language-model history, seeded from the chats that predate it (done)
- Post-M9: the web tools become opt-in, and the one download outside the lock list (done)
- Post-M9: parallel sub-agents — stage 2, the round's parallel group (track complete)
- Post-M9: concurrent ordinary tools — the round's read-only calls run at once (done)
- Post-M9: admission by budget — the unified KV pool never overfilled by the app (done)
- Post-M9: the parked-set bound of the RAM prompt cache, measured (done)
- Post-M9: the parked-set bound on Gemma 4 31B, measured on a rented L40S (done)
- Post-M9: sub-agents in the background — a run that outlives its turn, stage 1 (done)
- Post-M9: sub-agents in the background — the stop key, the unread mark, and Gemma (stage 2, done)
- Post-M9: background dialogues — a directed scene that outlives its turn (done)
- Post-M9: `ToolOutcome.wrote` — a memory writer reports its write (done)
- Post-M9: `ToolOutcome.prefill` and the page summary's request — reasoning muted, usage recorded (done)
- Post-M9: the dialogue's director on Gemma's template — the checkpoint's history must alternate too (done)
- Post-M9: a fetched page is read in its own encoding (done)
- Post-M9: local files are read in their own encoding (done)
- Post-M9: a fetched page's attachment is named after the page (done)
- Post-M9: Python sandbox — the starter set grows: sympy, lxml, openpyxl, matplotlib and more (done)
- Post-M9: the sandbox's packages, packed read-only (done)
- Post-M9: sandbox file exchange — stage 2, files out of the sandbox (done)
- Post-M9: sandbox file exchange — stage 3, files into the sandbox (done)
- Post-M9: sandbox file exchange — stage 4, opening a file in the system (done)
- Post-M9: sandbox file exchange — stage 5, the local interpreter joins the contract (done)
- Post-M9: a withheld server image is stated, and stated in words that work (done)
- Post-M9: the rest of the withheld-image family gets the clause that works (done)
- Post-M9: a handle means one file for the whole turn (done)
- Post-M9: an image is installed only after it has started (done)
- Post-M9: the same bytes put a missing copy back (done)
- Post-M9: two letters are not a format, and an invisible mark is not a name (done)
- Post-M9: the `files` argument is read once, and the result stops listing (done)
- Post-M9: a process the script leaves behind stops deciding the call (done)
- Post-M9: the "show charts" switch is shown in local Python mode too (done)
- Post-M9: one name means one file — the fold, the empty cut, and the numbering (done)

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
the probe was never merged, and its branch is gone — the code is frozen under
the tag `probe/code-search-stage5`.

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
  JSON disabled. **One question this IP could not settle at the time**: with every
  provider serving a block page, a stale CSS selector and a blocked response are
  indistinguishable — both parse to zero, so all four selector sets were recorded
  as unverified rather than assumed good.
- **Re-measured once the IP partly recovered, and the gap is now mostly closed.**
  The user re-ran `live_search_returns_results` and it passed — which alone
  proves only that *someone* answered, so each provider was probed by name:
  **DuckDuckGo lite and html both served real result sets, 10 links and 10
  snippets each — their selectors are confirmed, and the earlier zeroes were the
  block, not rot.** Mojeek (`200` + `<title>Captcha`) and Ecosia (`403`,
  `<title>Ecosia Firewall`) were still blocking, so those two stay unverified for
  the same reason as before. The primary provider being sound is what the
  question was really about.
- **And it produced a stronger confirmation of this fix than the original run.**
  Mojeek was still serving `HTTP 200` with `<title>Captcha</title>` and **none**
  of the five old body markers, while DuckDuckGo beside it answered normally — so
  the title anchor was shown to *discriminate*, not merely to fire. The first
  live run could only show it firing when everything was blocked.
- **Recovery is per-operator, and the spread is hours.** DuckDuckGo came back
  while Mojeek and Ecosia did not. That is the argument for keying the cooldown
  by family and for reordering rather than skipping, made by measurement rather
  than by design intent: a chain that dropped a blocked family would have kept
  DuckDuckGo out long after it recovered.
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
  This PR is the defect and needs no key. (Brave did not survive that PR — see
  the next entry.)

### Post-M9: `web_search` keyed providers — Tavily (done)
- **Why, and why not more free ones.** The measurements in the previous entry
  closed the keyless option: from a residential IP the chain answers about twice
  before every provider blocks, the block outlives fifteen minutes of silence,
  and a real Chrome from the same IP is blocked too — a rate ceiling, not a
  fingerprint one. Every keyless alternative was surveyed and rejected (Yep 403,
  Marginalia a queue interstitial over a tiny index, Startpage and keyless Brave
  captchas, public SearXNG instances 403 or JSON disabled). The provider survey,
  the rejected vendors and the decided forks:
  [web-search-keyed-providers.md](../research/web-search-keyed-providers.md).
- **One provider: Tavily.** Its free tier is 1 000 searches a month with no card,
  its response is shaped for this use (title/url/snippet/score) and it returns
  cleaned page text on request. **Brave was implemented and then removed the
  same day** (2026-08-25): the user could not register for it at all — the
  service is not offered in every country. That is a stronger reason than one
  person's access, and it is why it came out rather than shipping disabled: a
  provider nobody here can obtain a key for is a provider the live gate can
  never cover, and an untestable second backend is a liability, not a fallback.
  The `Backend::Api` shape means re-adding it (or any other) is a variant and a
  parser, not a reshape — this is not a one-way door. **SearXNG was designed and
  not adopted** —
  §4.1 of the research stands, including the address-policy argument it settles
  (a user-typed LAN URL is the engine-address category, not the model-chosen
  one), but a self-hosted instance is a deployment this track will not ask for.
  Google CSE and Bing were rejected as dead: closed to new customers with a
  2027-01-01 end date, and retired 2025-08-11 respectively. The clouds' own
  server-side search tools were rejected too — a search costs a whole model round
  trip, the results come back without readable content (Anthropic's
  `encrypted_content` is only decryptable by Anthropic), and it would weld
  `web_search` to a cloud key.
- **The shape.** `PROVIDERS` was a table of *HTML-scraping* descriptions, so
  keyed providers could not be nullable columns on it: the loop now runs over
  `Backend::{Api, Scraped}` and each attempt returns `Attempt::{Results, Empty,
  Blocked}`. That third arm is the whole point of the previous entry, now made
  explicit in the type rather than reconstructed from an empty parse. The
  keyless path through it is unchanged.
- **The invariant, and the test that pins it**: with no key configured the
  backend order **is** the scraped chain, for every `web_provider` value. If that
  ever fails, a fresh install has quietly changed how it searches.
- **Keyed first (fork F3).** A dead round trip through a blocked scraper costs
  more seconds than a credit costs cents, and §1.1's fifteen-minute persistence
  means the scraper is likely to be blocked rather than merely slow. `auto` uses
  every keyed provider that has a key; `free_only` uses none, for someone holding
  a key for another purpose. It stays a *choice* rather than a toggle even with
  one provider, because the slot set is what a second one would extend and the
  stored `"auto"`/`"freeonly"` values keep round-tripping. The free chain is
  never removed.
- **Failure modes read apart from each other.** `429` is a rate limit and takes
  the same family cooldown as an anti-bot throttle. `401`/`403`/`402` mean the key
  is wrong, revoked or spent — that must not read as throttling, but must not fail
  the search either, or a stale key would take web search down with a working
  keyless chain sitting behind it. So it falls through *and* becomes `last_err`:
  the run continues, and if nothing answers, the message names the key instead of
  blaming the network. Same defect class as ever ([lessons §4](../lessons.md)) —
  the message has to close the right door.
- **A result that arrives with its page text is not fetched again** (Tavily's
  `raw_content`) — the latency the keyed backend was chosen to avoid, and one
  fewer automated request in front of the site's own anti-bot. **The result also
  names which backend answered**: the chain degrades silently by design, and
  without the name the model cannot tell a thin answer from a degraded one — the
  transcript that started this work has it reasoning aloud about the tool's
  health with no evidence to reason from.
- **Plumbing.** `SecretKey::Search(SearchSlot)` mirrors `SecretKey::External`
  exactly — a closed set, an unambiguous `search-<slot>` name, machine-bound
  (ADR 0008); its own variant rather than a `Provider` one because these are not
  inference providers and a `web_search` key must never resolve through the
  engine's key lookup. Keys resolve in `orchestrator::web_search_keys` (stored
  beats the named variable) and reach the tool as strings, so the secret store
  stays in one layer. A `SecretKey::Search` edit rebuilds the registry, for the
  same reason the Gemini key does for the video slot. In settings, the Web-search
  group gained the provider choice plus a key/env pair **only where the key can
  be spent** — `auto` shows both, a named provider only its own, `free_only`
  neither; an unused key row reading as a live one is how someone ends up
  believing a provider is configured when nothing will ever call it.
- **Clippy found a real defect, not a style nit.** `SearchSlot::ALL` came back
  unused — because the orchestrator's `secrets_present` list had not been
  extended, so a stored search key would have kept showing "not set" in settings
  over a key that was on disk and in use. Nothing was pinning it; there is a test
  now.
- **Tests**: 2599 green (+13), 109 `#[ignore]` (+2, both live); clippy
  `-D warnings`, fmt, the i18n gates, `cyrillic_scan`, `link_check`,
  `doc_index_check` clean, and the settings screenshot dumps regenerated.
  **Mutation-tested**: making `free_only` return keys, putting the keyed
  backends after the free chain, and letting the environment beat the stored key
  each fail exactly their own test.
- **One test was rewritten because the mutation run exposed it as worthless.**
  "A result that already has content is not re-fetched" originally asserted on
  the field *after* enrichment — and it passed with the skip removed, because an
  unfetchable URL leaves the field alone either way. The selection is now its own
  function (`needs_content`) and the assertion is on that; it fails under the
  mutation. A negative behaviour needs an assertion on the decision, not on the
  state afterwards — the same trap [lessons §2](../lessons.md) records for
  fixtures that measure nothing.
- **Live run — GO** (2026-08-25, the user's own Tavily key in `TAVILY_API_KEY`).
  The criterion (research §8) was ten searches in one run, all ten returning
  results and none falling through to the keyless chain — the load pattern that
  degrades the free chain to two. **Ten for ten in 10.5 seconds**, every one
  answered by Tavily, *on the same IP that every free provider was still
  blocking at the time*: the contrast is the measurement. The smoke deliberately
  has **no skip-on-throttle escape** — a keyed provider answering 429 inside ten
  searches would be a real finding about the free tier, not an infrastructure
  excuse.
- **A second live check, for a failure that would have looked like success.**
  `include_raw_content` is a vendor field name, and if it were wrong the tool
  would silently receive no page text and quietly fetch every page itself —
  slower, more requests in front of other sites' anti-bot, and indistinguishable
  from working. Only a live call can settle a field name, so there is a smoke
  that does (`live_tavily_returns_page_text_so_the_tool_need_not_fetch_it`):
  **GO** — results carry text, and `needs_content` selects fewer pages than there
  are results, which is the saving stated as an assertion rather than a hope.

### Post-M9: the two-agent dialogue — research and the stage-0 live probe (done)

- **What/why**: the feature the sub-agent track left open (research §3.14,
  roadmap) — `run_dialogue`: two personas with caller-written system messages
  talking to each other via the role swap, one call / one transcript on the
  record (`RunKind::Dialogue`), and a model-driven director deciding when the
  dialogue is over. Design doc with every fork:
  [docs/research/two-agent-dialogue.md](../research/two-agent-dialogue.md).
  Forks F1–F10 confirmed at the recommended options; **F6 amended by the
  user**: the director carries the parent chat's persona and a conversation
  brief built by the `/compact` summarizer's mechanism — the neutral built-in
  director was rejected because the stop/steer judgment should know *why* the
  user wanted the dialogue.
- **The probe** (`spike/dialogue-probe`, `src/shared/api/dialogue_probe.rs`):
  the §3.2 derivation (swap + user prologue + same-role merge) and §3.3
  checkpoint shape inlined over the app's own clients, wrapped in
  `RetryBackend` exactly as `live_backend()` wraps the e2e set — the first
  run's "failure" was a stale pooled connection (lessons §9's back-to-back
  flake), an instrument fault, not a finding. Three fixtures (a finite café
  scene; a haggle engineered near-impasse for steering; a poet verbose by
  construction to force edits), a raw-request arm reading `timings.cache_n`,
  and one cloud run each on the strict-alternation wires.
- **Results — GO everywhere** (details §5.1–§5.2 of the doc). Gemma 4 31B:
  11/12 director stops at real endings, 53/53 verdicts parsed, 0 role bleed,
  a finite dialogue in 42–59 s. Qwen 3.6 27B: 11/12 stops (one an *honest
  impasse* — the walk-away branch the direction allows), 42/43 verdicts,
  0 bleed, thinking-model costs (a line 22–27 s avg). haiku-4-5 and
  gemini-2.5-flash: 1/1 each, the swap survives both merging wires. Across
  all four backends: **104/105 checkpoint verdicts parsed**. The unprompted
  find: the intended escalation ladder emerged by itself — notes while
  steering preferences (15 on Gemma, obeyed), retry against a style, and
  rewrite in the character's voice when the persona won anyway.
- **Cache slots — the §3.9 fear did not materialize**: on the default
  4-slot server all three contexts keep their prefixes from the second visit
  (A `cache_n=146/161`, B 145, D 106; same shape on Qwen), so alternation
  costs the appended tail, not a full re-prefill.
- **Two executor rules found live, folded into the design**: (1) the
  all-thinking empty turn — an instruction conflict (a director note against
  a persona's format rule) spends the whole 1536-token cap in
  `reasoning_content`; on Qwen even unconflicted openers can spiral (~29% of
  generations under steering pressure). The muted re-ask
  (`reasoning_budget: 0`) recovered **28/28** across both models; the 4096
  ceiling alternative is recorded, not taken. (2) A muted director can still
  deliberate in plain text past the 512-token verdict cap — the
  no-call-means-continue fallback absorbed the one occurrence.
- **Tests**: 2653 unit tests green (unchanged), `#[ignore]` 120 → 125 (the
  five probe arms). Live: the full probe on both gate models on the LAN
  stack, plus the two cloud spot-checks.

### Post-M9: `run_dialogue` — the directed dialogue, stage 1 (done)

- **What**: the product code of the two-agent dialogue (spec §9.13,
  [ADR 0011](../decisions/0011-dialogue-directed-run.md), design
  [two-agent-dialogue.md](../research/two-agent-dialogue.md)) — the
  loop-executed `run_dialogue` beside `call_subagent`: two persona contexts
  and a persistent director context taking strictly sequential turns on the
  turn's one backend, the transcript landing on the call's record as a
  `SubagentRun` with `kind: Dialogue` and `participants`.
- **Key decisions, as confirmed in the research doc's forks**: the
  role-encoded transcript (a = `Assistant`, b = `User`, director
  interventions `System` → note rows) bought the entire viewing pipeline
  unchanged — the one projection change (`FeedMessage::from_message` maps
  `System` to `Note`) moved together with `entities::chat::
  visible_message_count`, and the existing every-prefix pinning test is what
  held them to each other. The executor is a scripted loop over direct
  `stream_round` calls — not a nested `TurnLoop` — with the derivation and
  the verdict vocabulary as pure functions in `features/tools/dialogue.rs`;
  the probe's two rules are in the loop (the muted re-ask on an empty line,
  no-call-means-continue on a checkpoint). The director carries the parent
  persona and a conversation brief — the compaction summary threaded through
  `TurnShared.compaction_summary` plus the request's own tail.
- **Not predicted by the plan**: the settings demo dumps rot-gate fired on
  the new "Dialogue: run time limit" row (regenerated, the committed frames
  and screenshots updated); the transcript's live growth had to be **per
  line** (`RoundSink.mute_steps`) because the streamed partial carries no
  speaker side and half the lines land on the `User` side — token-level
  streaming into the transcript is the track's stage 2; and the shared
  engine-script fixtures moved to `pub(super)` in the sub-agent test module
  so the dialogue suite would not re-copy them (the duplication-gate seam
  budgeted at design time, lessons §1).
- **Tests**: 2672 unit green (+19: the derivation/verdict suite in
  `dialogue.rs`, six orchestrator tests over the scripted engine — landing,
  cap, the steering ladder, the muted recovery, the timeout, the opened
  transcript's names and composed bubble), 126 `#[ignore]` (+1:
  `dialogue_e2e_live`, the probe's café fixture graduated). Live run —
  **GO, Qwen 3.6 27B Q4_K_M** on the LAN stack: one delegation, a 4-line
  strictly-alternating scene, the director stopped it at the resolution
  (`Completed`), 4597 tokens in 167 s, and the parent's reply cited the
  transcript's `chat://` address unprompted. One familiar behaviour
  resurfaced: the model passed **no persona names** (the same optional-field
  habit the sub-agent's F13 recorded on this family), so the localized
  fallback labels — in the run's agent language, ru — carried the headers
  and the title, which is exactly what the fallbacks exist for. The Gemma
  arm of the same scene ran shape-identical in the stage-0 probe; the pair
  stands.

### Post-M9: `run_dialogue` — the live transcript, stage 2, track complete (done)

- **What**: the stage the research doc's §3.7 named — the open dialogue
  transcript is live. A participant's line streams into it token by token
  **on its speaker's side**: `TurnProgress::ChildLineStarted { role }` before
  each line (the muted re-ask included), the mirror's `child_line_role` +
  `LiveTurn.role` for a transcript opened mid-line, and the screen's
  `stream_role` retargeting the streaming bubble
  (`AppEvent::TranscriptLine`). A director's retry/rewrite **replaces the
  open view in place** — `ChildTranscript` → `AppEvent::TranscriptReset`,
  because appending cannot express an edit — and the parent's status bar
  carries a scene chip (`RunProgressKind::DialogueLine`/`DialogueDirector`,
  the lessons-§4 rule: a turn parked in a long dialogue must not read as a
  stuck "generating"). Director checkpoints stay muted: deliberation is not
  a line.
- **Key decisions**: the side travels **per line**, not per token —
  `ChildLineStarted` resets the round partial and sets the side once, so the
  existing `ChildStep`/`child_partial` machinery needed no per-event field;
  the streaming bubble is retargeted only while **empty** (a bubble
  mid-content keeps its side; the next chunk opens a new one); and the chip
  gained a `kind` on the shared `SubagentProgress` payload rather than a
  second event — one chip, three wordings, worded by the screen (axis B).
- **Not predicted**: the first smoke run of the stage failed with the
  parent's turn spending itself entirely in `reasoning_content` — zero text,
  zero calls — which is the documented Qwen empty-turn mode (lessons §9),
  not a regression: the stage's diff never touches the parent request path.
  Re-run per the union-of-runs rule; recorded rather than hidden.
- **Tests**: 2676 unit green (+4: the speaker-side streaming, the in-place
  reset, the chip wordings, and an orchestrator test that opens a running
  dialogue mid-line and asserts the activation carries the line's side and
  streamed partial), 126 `#[ignore]` (unchanged). Live: `dialogue_e2e_live`
  **GO, Qwen 3.6 27B Q4_K_M** — a 10-line scene to the cap (`RoundLimit`,
  the polite loop the cap exists for), 14 370 tokens in 840 s, and the
  steering ladder fired **in the wild** this run: a `dialogue_retry` with a
  note to one participant and a `dialogue_rewrite` of a line that had
  drifted into the other character's voice — both visible in the landed
  transcript as the intervention rows stage 1 built for them (the rewrite's
  own wording is the model's judgment, not the mechanism's). This run the
  model also passed both persona names, so the title and headers carried
  them.

### Post-M9: `get_llm_name`/`get_llm_history` — the language model, named and dated (done)
- **The ask** (docs/research/language-model-history.md, forks confirmed
  2026-08-29): the assistant can learn the name of the **LLM** running it, and
  each profile keeps a dated history of model changes the assistant can read —
  with tool names that cannot be confused with the self-model. The survey
  reshaped the task the usual way (lessons §3): the name is already resolved
  once per turn (`effective_model_name`, the external-model-name track) and
  frozen into every reply's `MessageMetadata`; `handle_done` is the single
  point where a completed exchange lands; what was missing was exposure to the
  model and the per-profile aggregate.
- **Names, by the user's choice (F1)**: `get_llm_name`/`get_llm_history`, in a
  `features/tools/llm.rs` that mirrors `self_model.rs` — "model" alone was
  already claimed three ways (self_model, user_model, `*_model_name`), so the
  pair's prefix is `llm_` and neither family's names contain a bare "model"
  the other could be mistaken for. Both in `Introspection`, both **on by
  default** (F7) — read-only, no abuse surface, unlike the off-by-default
  self-model precedent.
- **Key decisions**: the name tool answers from **new turn-snapshot fields**
  (`ToolContext.model_name`/`engine_mode` — the single `effective_model_name`
  read moved above the context build, so header, stored metadata and tool
  answer cannot disagree), never by asking `ctx.engine` mid-turn; the history
  lives in a **`data.db` table** (F3 — the user's call: not precious enough
  for `profiles.json`; additive `CREATE TABLE IF NOT EXISTS`, explicit
  `rowid INTEGER PRIMARY KEY` so VACUUM keeps insertion order, no `DB_SCHEMA`
  bump) as `{changed_at, model, mode}` (F4 — mode included, stored as
  `ServerMode`'s serde key with `key()`/`from_key()` pinned to serde by test);
  the recorder in `handle_done` reads the turn's newest metadata **before**
  the messages move into the chat (the `is_first_reply` position — a
  `/continue` tail is folded away by `land_continuation` right after) and
  `Db::llm_history_note` appends under one mutex acquisition only when the
  **pair (name, mode)** differs from the newest record — dedup by name alone
  would let the stored mode go stale. A record from any landed reply,
  cancelled/errored included (F5a); baseline record on first exchange (F6);
  a turn whose engine named no model records nothing, and the tool's unknown
  text closes the door (lessons §4: names the mode, says no other route
  exists this turn, points at the engine settings).
- **Not predicted by the plan**: the first live run failed on the *fixture*,
  not the feature (lessons §9) — the live harness injects the backend through
  `MockSupervisor` and leaves `config.engine.mode` at the default `Managed`,
  so a server reached by URL recorded `(managed)`; the smoke now sets the
  mode to what the setup actually is. The settings demo dumps rot-gate fired
  on the two new toggle rows (`settings-model`/`settings-tools` frames —
  regenerated, PNGs re-rendered).
- **Tests**: suite at **2696 unit / 127 `#[ignore]`** (+16 / +1): the
  `ServerMode` key↔serde pin, the storage round trip/isolation/dedup, the
  tools' catalog+defaults membership, ru≠en descriptions, snapshot/degraded/
  empty/ordering renders, and seven orchestrator recorder tests (baseline,
  dedup, mode-change record, unknown-name silence, cancelled-partial counts,
  newest-metadata extraction, profile isolation, vanished-chat best-effort).
- **Smoke — GO, Gemma 4 31B Q4_0 (llama.cpp, external)**:
  `llm_name_and_history_e2e_live` — both tools called on the first ask (the
  "tool was actually called" assertion guards against a refusal reading as a
  pass), `get_llm_name` answered with the discovered name and mode, and the
  second turn's `get_llm_history` read back the baseline record the first
  exchange's recorder had just written. Full `orchestrator::tests::live` set
  re-run for the turn-path change (the single-read move sits on every turn):
  **48/48 green in one sweep**, 22 min on the same stack.

### Post-M9: the Python package pinned exactly — a selector is a range (done)
- **Trigger**: the live e2e gate's CI dispatch
  ([run 33276655992](https://github.com/vshylov/mindfork-rs/actions/runs/33276655992))
  came back 124 passed / 1 failed, and the one red was
  `runs_real_python_in_sandbox` — a smoke that never speaks to the engine, so it
  had nothing to do with the model under test. It looked like a runner quirk.
  It was not.
- **What it actually was**: `mindfork sandbox setup` downloaded
  `python/python` — **no version** — while the runtime beside it
  (`WASMER_VERSION`) had been pinned exactly all along. The registry published
  3.13.15/16/17 on 18–21 August 2026, whose payload is **155,874,883 bytes**
  against 3.13.5's **44,680,028**, and `wasmer` 7.2.0 cannot compile the new
  build: `Validate("Failed to create V8 module: null module reference returned
  from V8")`. So this was never a test defect — **every fresh
  `sandbox setup` since 18 August produced a sandbox that could not run
  Python**, and the gate's red was the only thing saying so.
- **Two hypotheses killed by measurement before the right one was reached.**
  *Not the CI runner*: it reproduced on Windows with a byte-identical error.
  *Not the compilation cache*: hiding the working sandbox's 180 MB `cache` left
  it passing anyway (2.33 s instead of 0.52 s — it recompiled fine). The
  differentiator was the payload itself, found by comparing the two
  `python.webc` files: 44.7 MB against 155.9 MB, different sha256, **identical
  `wasmer` binaries**.
- **The fix is one character, and it is the interesting part.** A wasmer package
  selector is a semver **range**, not a pin. Measured against the live registry:

  | Selector | Resolves to |
  |---|---|
  | `python/python` | 3.13.17 |
  | `python/python@3.13.5` | **3.13.17** |
  | `python/python@=3.13.5` | 3.13.5 |

  A "pinned" `@3.13.5` would have fixed nothing while looking fixed. `@=3.13.5`
  downloads 44,680,028 bytes, sha256 `c03ebe09…bdcab` — **byte for byte** the
  payload every working sandbox in this project already held.
- **Verified where it actually breaks.** An existing sandbox passes across a bad
  publish, so only a *freshly provisioned* one can tell you anything: the smoke
  was run against an empty `MINDFORK_SANDBOX_DIR` before the change (red) and
  after (green, 46 s including the ~250 MB provision). The setup's own warm-up
  went from `Warm-up completed partially (not critical)` to `Cache warmed up.` —
  the earliest signal, and it had been swallowed as non-critical.
- **Guarded**: `the_python_package_is_pinned_exactly` fails if the `=` is ever
  tidied away, because without it the constant silently becomes a range again.
  2698 unit tests green, 128 `#[ignore]`.

### Post-M9: the language-model history, seeded from the chats that predate it (done)
- **The trigger**, from the user, the day the history tools landed: the tools
  work and the history is **empty** — the recorder starts recording the day it
  ships, so every profile that existed before it answers "nothing recorded yet"
  about a past that is written down in full. The parked bullet at the end of
  docs/research/language-model-history.md §8 ("a one-time backfill is possible
  later") was exactly this; it is now that document's §9.
- **The whole design is one sentence**: *the seed is what the recorder would
  have written, had it existed when those replies were generated.* Every
  question the backfill raises answers itself from it, which is why there were
  no forks to confirm: records ordered by the replies' own timestamps **across
  the profile's chats** (the recorder fired per exchange, in time order — two
  chats are not each other's future), `changed_at` taken from the reply and not
  from the seeding moment (a history dated "now" is a list of names), and
  consecutive runs of the same (name, mode) collapsed exactly as
  `llm_history_note`'s dedup collapses them — A→B→A stays three records. Same
  for scope: only chats the app can see (a soft-deleted chat is gone from every
  other read path — this is not the place for an exception), and only
  `Chat::messages`, so a sub-agent transcript — which runs on the parent turn's
  engine and lives on a tool-call record — is skipped by construction.
- **One-time without a marker.** The condition is the data: a history with **no
  records at all** is seeded, and the emptiness check sits *inside* the
  transaction that writes (`Db::llm_history_seed`), so the first record ever
  written — seeded or recorded — closes the door, and a failed seed leaves
  nothing rather than a half-history that looks non-empty and can never be
  completed. Deliberately **no** "already seeded" flag: a profile whose replies
  name no model (the pre-`MessageMetadata` corpus, or an engine that never said
  a name) writes nothing and stays seedable, so a later launch that does find
  something still fills it. That re-scan walks chats `bootstrap` has just loaded
  into memory — cheaper than the row that would record having done it.
- **Where it runs**: `Orchestrator::bootstrap`, after profiles and chats are
  loaded and before anything can record a record of its own; best-effort per
  profile like the recorder itself (a failed read or write is logged and
  skipped — no part of startup hangs on bookkeeping). Noticed while writing it:
  this is also the one reconstructible part of the `data.db`-lost-next-to-chats
  scenario (`Storage::chats_without_db`) — the history refills itself there.
- **Structure**: the derivation and the startup hook are a new
  `app/orchestrator/llm_history.rs`; the forward recorder stays in
  `generation.rs` (a move would have mixed a refactor into a behaviour change).
- **Tests**: suite at **2711 unit / 128 `#[ignore]`** (+10): the storage seed
  (fills an empty history in order and dates it from the records, refuses a
  history that has any record, seeds nothing from nothing and stays seedable),
  the pure derivation (the interleaved two-chat sequence with the dedup and the
  A→B→A tail, replies naming no model and messages that are not replies,
  per-profile isolation, a sub-agent transcript adding nothing), and bootstrap
  end to end (seeds from stored chat files, skips a hidden chat, is idempotent
  across two launches, and — the join that matters — the recorder then *continues*
  the seeded history: the model the seed ends on writes no record, a new one
  appends).
- **Run on the real binary** (an engine smoke is not what this needed — no
  engine, provider or tool path changed; `features/tools/llm.rs` is untouched
  and reads the same table, and `llm_name_and_history_e2e_live` still covers
  the tool end). Three headless launches of `mindfork.exe` on a portable data
  root: the first created the data directory, then two dated replies for one
  model and one for another were written into the chat file by hand
  (`llm_history` empty), the second launch left
  `2026-01-01 … gemma-4-31b (managed)` + `2026-02-01 … qwen-3.6-27b (external)`
  in `data.db` — the middle duplicate collapsed, both dates the replies' own —
  and the third added nothing and logged nothing (one `seeded the
  language-model history … records=2` line in the whole log).

### Post-M9: the web tools become opt-in, and the one download outside the lock list (done)
- **Trigger**: writing `PRIVACY.md` for the code-signing track
  ([docs/research/code-signing.md](../research/code-signing.md) §3.2) meant
  inventorying every `reqwest` client construction in the binary instead of
  trusting the summary in SECURITY.md. Nine production sites; the summary was
  wrong about two of them.
- **What was wrong (1): `web_enabled` defaulted to `true`.** Every other
  outbound address in the app is one the user typed or picked — engine,
  embedder, TTS, MCP. The `web_search` chain is not: `lite.duckduckgo.com`,
  `html.duckduckgo.com`, `www.mojeek.com`, `www.ecosia.org` are picked by *us*,
  and the query is derived from the conversation. So the app's own promise
  ("only the endpoints you configure") had exactly one exception and it shipped
  on, while SECURITY.md said "when you explicitly enable the web tools".
- **(2): `web_tavily_key_env` defaulted to `TAVILY_API_KEY`**, and keyed
  backends are tried *first*. A key exported for some unrelated tool therefore
  routed the model's searches to `api.tavily.com` and spent that key's credits
  with no decision made anywhere in this app. The asymmetry was accidental —
  cloud `api_key_env` has always defaulted to `None`.
- **(3): `python.webc` was never hash-checked.** It is fetched by the `wasmer`
  binary rather than by our client, so no lock-list row can cover it (there is
  no URL to pin), and the only post-condition was `webc.is_file()`. The
  known-good digest had been sitting in the doc comment above `PYTHON_PACKAGE`
  since the August pinning incident, quoted and never compared.
- **The fix, in one PR because they are one claim**: `web_enabled: false`,
  `web_tavily_key_env: None`, and `PYTHON_PACKAGE_SHA256` checked by a new
  `verify_file_sha256` that streams the finished file (45 MB — no reason to hold
  it). A file that is *present* but mismatching is replaced rather than refused,
  since this command is provisioning and a stale asset is what it exists to fix;
  a *freshly downloaded* mismatch is fatal, because at that point the registry
  is serving something other than what we pinned.
- **Why the default rather than the sentence.** SignPath Foundation's conditions
  require software that transfers user data to systems the user did not specify
  to carry a privacy policy shown *during installation*, with an off switch
  there. Honest defaults satisfy that by design; editing SECURITY.md to promise
  less would have satisfied nothing (F8a, decided by the user 2026-09-01).
- **Not a migration.** The container's `#[serde(default)]` only fills a *missing*
  field, and settings are written in full — so an existing `settings.json` keeps
  whatever it says and only new installations see the change. The migration test
  that pins `web_enabled: true` through the v1→v2 step is exactly this property
  and was left alone.
- **Measured**: the demo-shot gate caught the change immediately —
  `settings-tools-{dark,light}-en` drift by precisely the toggle, the section
  counter (`2` → `1`) and the Tavily row (`TAVILY_API_KEY` → `—`); dumps
  regenerated and the four assets re-rendered.
- **Live — GO.** The pinned digest was verified twice: against the
  44,680,028-byte `python.webc` this machine has been running since August
  (identical), and by a **fresh** registry download into an empty directory —
  the new `#[ignore]` smoke `live_the_registry_still_serves_the_pinned_python_build`,
  which exists because a pin that has drifted turns every `sandbox setup` into a
  refusal, a worse failure than the one it guards. 2743 unit tests green,
  129 `#[ignore]`.


### Post-M9: parallel sub-agents — stage 2, the round's parallel group (track complete)
- **What**: the second half of
  [docs/research/parallel-subagents.md](../research/parallel-subagents.md)
  (forks F1–F10 at their recommended options; ADR 0010 amended). The model's
  several `call_subagent` calls in one reply run **at once**. `tool_round`
  hands its calls to `execute_round`, three phases (`resolve_round`,
  `run_group`, the records — split out after Sonar measured the round at a
  cognitive complexity of 18): the ordinary calls resolve in the model's order
  (`resolve_call`), the round's sub-agent calls (`is_group_call`: offered,
  depth 0, not a rewrite) become a `ChildSpec` each (`child_spec`) and run
  through `buffer_unordered(tools.subagent_parallel)` over `run_child` — a
  free function over `&TurnShared` owning nothing of the parent — with each
  card closing as its run lands, and `record_call` writes the tool messages
  and records in the model's order afterwards. `TurnShared` is borrowed
  immutably by every loop: the confirmation reply receiver and the
  "approved for this turn" set moved behind one `tokio::sync::Mutex`
  (`ConfirmState`) held for the whole ask-and-wait, so two runs reaching a
  dangerous call ask one popup at a time; the token totals moved to
  `TurnCounters` atomics every loop adds to (`stream_round` corrects the
  delta count to the server's `usage` at the round's end), and `RoundSink`
  reports the turn's total to the bar and the loop's own count as
  `ChildTokens`. Every `TurnProgress::Child*` step names its run; the
  orchestrator's mirror is `children: Vec<InflightChild>` (run, stream,
  partial, line role) keyed by id, `forward_child(run, …)` forwards a step
  only to that run's open transcript, `switch_within_turn` covers the parent
  and every running child, the list marks every running row, and a title
  given to any running transcript is carried onto its landed run. The chip
  (`AppEvent::SubagentProgress { run, .. }`) is a set on the screen: one
  line, or *"N sub-agents · latest"*. `CallSubagent { parallel }` — the
  registry is rebuilt on every settings edit — appends the measured
  sentence to its description above 1, with the number. The settings row
  `TSubParallel` ("Subagent: parallel runs") joins the Agentic loop group.
  `run_dialogue` stays outside the group (ADR 0011).
- **Key decisions.** Futures in the generation task rather than spawned
  tasks: the children borrow `&TurnShared` and need no `'static`, the
  turn's cancellation token reaches them unchanged, and `buffer_unordered`
  is both the width and the completion order. The ordinary calls run before
  the group, not among it: a sub-agent clones its context when it starts
  and the round's attachment effects are mirrored once at the round's end,
  so nothing a sibling tool did in the same round was ever visible to a
  child. `is_group_call` keeps a nested loop's `call_subagent` on the
  ordinary path, where the depth check still refuses it — two locks on the
  same door, as before. A session permit is per stream, so with fewer
  sessions than runs the children interleave round by round (fork F3);
  the RAM prompt cache made that free on both gate models (research §3.5).
- **Live run — GO on Qwen 3.6 27B** (`Qwen3.6-27B-Q4_K_M`, llama.cpp
  b10791, `-np 4 --kv-unified -c 16384`, `subagent_parallel = 2`, two
  sessions): `parallel_subagents_e2e_live` — the parent delegated both
  files in one reply, the two runs started **1 ms apart** and overlapped
  (the second finished before the first), each used `fs_read`, both
  planted codes in the parent's reply. `subagent_with_tools_e2e_live` and
  `dialogue_e2e_live` — the two loop-executed tools the refactor touched —
  GO on the same stack. **The full e2e set on stage 2's code, Qwen 3.6:
  50 passed, 0 failed**, 25 min (the two image smokes and TTS skipping in
  writing) — the group of one changes the loop's shape at the default too,
  which is why the whole set ran again rather than the three smokes alone.
  **And on Gemma 4 31B** (`gemma-4-31B_q4_0-it` + mmproj, the same unified
  line, the image smokes running for real): **50 passed, 0 failed**, 69
  min — both gate models green on the final code.
- **Tests**: 2769 green (+10: a persona-keyed, stream-counting engine in
  `tests/parallel.rs` — the scripted recorder plays by call order, which two
  concurrent children make nondeterministic — pins two siblings streaming
  at once and landing in the model's order with both cards open before
  either closes and the tool results in call order on the next request;
  `subagent_parallel = 1` running them one after the other; two alive on
  one session with one stream open at a time; one sibling's timeout landing
  `TimedOut` while the other completes; the mirror forwarding two running
  transcripts apart and every parent↔child switch staying inside the turn;
  the chip counting runs and naming the latest; the description carrying
  the number only above 1; the settings row's three), 132 `#[ignore]` (+1).

### Post-M9: concurrent ordinary tools — the round's read-only calls run at once (done)
- **What**: the item the parallel sub-agent track left as F1c, its own track
  ([docs/research/concurrent-tools.md](../research/concurrent-tools.md),
  forks F1–F9 at the user's decisions — F3 *4 on the four clouds, 1 on
  managed and external*; [ADR 0012](../decisions/0012-concurrent-tool-calls.md)).
  The model-behaviour probe ran before any design: with the app's own
  `fs_read`/`fetch_url` descriptions and no prompting, Gemma 4 31B on the
  LAN stack, gpt-5.6, gemini-3.1-pro-preview, grok-4.6 and claude-sonnet-5
  answered two independent reads with two calls and three pages with three
  — **26/26**, never one call, never one too many. So nothing is told to the
  model; the harness runs what may run together, together. `Tool::concurrent()`
  on the trait (default `false`; the catalog bit `ToolInfo.concurrent`,
  `ToolRegistry::is_concurrent`) is the author's claim that a call has no
  effect a sibling could observe, changes nothing outside the application,
  holds no exclusive resource, needs no cleanup if dropped and is never
  `danger()`; marked: `fs_read`/`fs_list`, `code_read`/`code_grep`/`code_list`,
  `attachment_read`/`attachment_search`, `chat_search`/`chat_read`,
  `history_read`/`history_search`, `get_self_model`/`get_sampling`/
  `get_llm_name`/`get_llm_history`, `fetch_url`. The loop's unit is the
  **segment**: `resolve_round` walks the round by maximal runs of
  consecutive marked calls (`is_concurrent_call`, `segment_end`); a segment
  of one takes `resolve_call` unchanged, a longer one `run_segment` — every
  card opened first, `buffer_unordered(concurrent_calls)` over
  `invoke_member` (the sequential path's `select!` and outcome mapping),
  each card closed as its result lands, results and effects handed back in
  the model's order; `CallDone` (renamed from `ChildDone`) is the shape a
  member and a group child both return, and the sub-agent group still runs
  after the ordinary phase. `concurrent_calls: u32` on `ManagedSettings`,
  `ExternalSettings` and `CloudSettings` (`DEFAULT_CONCURRENT_CALLS_LOCAL = 1`,
  `DEFAULT_CONCURRENT_CALLS_CLOUD = 4`, `EngineSettings::active_concurrent_calls`
  with a floor of 1, `#[serde(default)]`, no schema step), read into
  `TurnShared` at the turn's start; the settings row "Parallel tool calls"
  (`FieldId::XConcurrent`) beside "Sessions (parallel streams)" in the
  Parallel sessions group, routed to the active mode's section like
  `XSessions`, its hint naming the marked tools from the catalog.
  `ToolContext.sessions: Option<Arc<Semaphore>>` shares the turn's session
  semaphore with the tools (`None` for background tasks), and `fetch_url`'s
  `summarize_text` takes a permit around its summary stream — the one engine
  request a tool makes on its own now counts against `sessions` like every
  stream of the turn. `OrchestratorDeps.extra_tools` registers instrumented
  tools on top of the standard set, re-registered on every rebuild (empty in
  production).
- **Key decisions.** The segment rather than "every marked call as one
  group": the sub-agent group may run after everything because a child
  observes none of the round's effects, but an ordinary read can observe a
  write of the same round — `code_read(f)`, `code_write(f)`, `code_read(f)`
  returns what the model asked for only in the model's order — so the rule
  never moves a read across a write and the result equals a sequential
  round's by construction, not in practice. The mark on the trait beside
  `danger()`, default off: a wrong `false` costs seconds, a wrong `true`
  could interleave a read with the write it was meant to follow; a registry
  test pins the marked set to the documented list and that none of it is
  dangerous, so no confirmation popup can be part of a segment. The width a
  per-section field like `sessions`, and 1 on a local engine: at 1
  `segment_end` never forms a segment, so the default local round is the
  old path bit for bit, while a cloud gets the overlap out of the box. The
  effects applied in the model's order after the segment, not as they
  complete, so a sequential and a concurrent round leave the same `Chat`.
  Out by name, each with its reason: `web_search` (three concurrent
  searches from one address is the throttling case lessons §9 recorded, and
  unmeasured), `note_recall` (it embeds and upserts inside a read),
  `youtube_watch`, `python_exec`, the command tools, every MCP tool
  (`readOnlyHint` is untrusted input), the loop-executed pair and the
  control pair. Futures in the generation task, not spawned tasks — the
  sibling's reason: `&self.ctx` and `&self.shared` borrowed for the
  segment, the cancellation token reaching every member unchanged.
- **Live run — GO on Gemma 4 31B** (`gemma-4-31B_q4_0-it` + mmproj, llama.cpp
  b10807, `-np 4 --kv-unified -c 16384`, the LAN stack): `concurrent_tools_e2e_live`
  at `concurrent_calls = 2` — the clerk read both planted files in one reply,
  the second card opened before the first closed (one segment), each record
  held its own file's codename, both codenames in the answer, 31 s. **The
  full e2e set on the final code, Gemma 4 31B: 51 passed, 0 failed**, 116 min
  (the image smokes running for real on the projector stack; the set runs at
  the local default of 1, so it is the regression proof that the default
  round is the old path — the segment itself is the smoke above). **Qwen 3.6 27B** (`Qwen3.6-27B-Q4_K_M`, b10807, `-np 1 -c 16384`, its own
  projector): the smoke GO, and the set **49 passed, 2 failed** in 17 min —
  both failures on the sequential path of width 1, i.e. before any code of
  this track: `dialogue_e2e_live` (an empty reply, no `run_dialogue` call)
  and `code_workspace_navigate_e2e_live` (turn 2 answered without the tool
  once, then called `code_read` and returned an empty text once). Rerun on
  their own: the dialogue green at the first attempt, the navigation at the
  second — **union green, no repeat** (docs/lessons.md §9), recorded as the
  model's flakiness on a thinking template, not as a regression. Both had
  passed on Qwen in the sibling track's stage-2 run.
- **Tests**: 2769 → 2781 unit tests green (+12: `tests/concurrent.rs` — a
  counting, delaying probe tool registered through `extra_tools` pins a
  segment running its reads at once and recording them in the model's order
  with every card opened before any closed, width 1 taking the sequential
  path, the width bounding how many run at once, an unmarked call and a
  disabled tool each breaking the segment, effects landing in the model's
  order, and `Esc` mid-segment cancelling every member; the registry
  invariant — the marked set equals the documented one and none is
  dangerous; `active_concurrent_calls` following the mode with a default per
  kind; the summary taking and releasing a session permit around its
  stream; the settings row's render, edit and hint), 132 → 133 `#[ignore]`
  (+1: `concurrent_tools_e2e_live` — two planted-token files read in one
  reply, both tokens in the answer, two `fs_read` records on one message,
  both cards opened before either closed).
### Post-M9: admission by budget — the unified KV pool never overfilled by the app (done)
- **What**: the item the parallel sub-agent track left as F7, its own track
  ([docs/research/admission-by-budget.md](../research/admission-by-budget.md),
  forks F1–F8 at the recommended options, the user's decision 2026-09-04).
  Above one session a managed `llama-server` runs `-np N --kv-unified`: N
  slots over the one pool `-c` sizes, and when the processing sequences
  outgrow it *together* the server halves its batch down to one and then
  answers "Context size has been exceeded." to **every** processing slot,
  clearing their prompts (`server-context.cpp`, the `decode` retry branch —
  a `TODO` upstream to end only the largest sequence). The session semaphore
  of stage 1 was a *count*; it could not know that two permits over a 16k
  pool admit two 9k conversations. Now `shared::session_budget::SessionBudget`
  is the turn's budget: the permits of `sessions` and, over a pool the app
  can know, a token **reservation** per stream — the request's prompt
  estimate scaled by the latest exact-to-estimate ratio any loop of the
  turn has recorded (floored at 1.0), floored by the loop's last exact
  `usage.prompt_tokens + completion_tokens`, plus the reply cap; a stream
  with no cap (only the turn's own, which overlaps nothing) reserves the
  whole pool. `acquire` takes a permit, then waits — cancellably, with a
  `Notify` registered before the check so no release is missed — until the
  reservation fits next to the open ones *or no stream is open*: a stream
  alone is admitted whatever its size, so the server keeps deciding whether
  one conversation fits, and a waiter is always behind a stream that ends.
  `orchestrator/pool.rs` says what the pool is: managed `-c` (through
  `context_budget`, so the explicit override wins) above one session; an
  external llama.cpp's `n_ctx` when it reports more than one slot (`/props`
  says nothing about the pool's shape, so *shared* is assumed — never wrong,
  pessimistic on a split server, fork F4); none on the clouds or at one
  session. Both streams of a turn price their requests: `TurnLoop::stream`
  and `fetch_url`'s summary (`ToolContext.sessions` is the budget now).
  With no pool the type is the bare semaphore it replaced, bit for bit.
- **Measured before designing** (the local CPU build, `llama-server` b10807,
  Gemma 3 4B Q8_0, `-c 2048 -np 2 --kv-unified`, `tools/kv_pool_probe.py`):
  two 1244-token prompts at once — the batch halved 1024 → 1 in 100 ms,
  then both slots ended, the one already decoding included (plain: two
  HTTP 500 `server_error`; streaming: 200 and the error object after 4 and
  0 chunks); two 884-token prompts that fit, whose 400-token replies grew
  into each other — both ended after ~140 tokens, 69 s in; the parent's
  parked context restored intact after the collision (`cache_n` 1239 of
  1244), and a parked context restored while the other slot is full falls
  back to a full prefill rather than failing; `--kv-unified-per-slot 1024`
  made the refusal per request (a 400 in 20 ms) at the price of the split
  shape's window and a *silent* `length` cut at the cap; the app's
  estimator under-counted the probe's filler by **2.19×** — the reason the
  reservation is calibrated by the turn's own exact usage rather than
  trusted raw.
- **What the live control arm found.** `admission_control_e2e_live` — the
  same run with the app told the pool is 65536 — was written to prove the
  guarded arm did something, and it did more: both children landed
  **`Completed`**, one with a reply cut mid-word (`“KELVAR`), one empty.
  The OpenAI-compatible client parsed each `data:` payload as a
  `ChatCompletionChunk` first and asked `parse_stream_error` only when that
  parse failed — which `{"error":{…}}` never makes it do, since every field
  of the chunk has a `#[serde(default)]`; the envelope was an empty chunk,
  skipped, and the closed connection read as a finished turn. The
  in-stream error path built in the cloud-retry track had never fired on
  llama.cpp's shape; its parser's unit tests were green because they test
  the parser alone. Fixed: the client asks for the envelope *before* the
  chunk parse, with an `sse_server` test (a delta, then llama.cpp's error
  object → `Text`, `Error { transient: true }`, `Finished(Error)`, nothing
  after). Recorded in docs/lessons.md §2.
- **Live runs — GO** on the CPU build launched exactly as the managed
  launcher would at `sessions = 2`, `-c 2048`: the parent scripted (a
  hybrid backend — the persona's requests go to the real server and are
  counted, the parent's play a script — so the 4B model's willingness to
  delegate twice is not the variable), the planted text in each child's
  *message* (~1100 tokens) rather than a file, because Gemma 3's template
  drops tools (`supports_tools: false`, measured: `prompt_n` 5 with a tool
  schema against 62 without). `admission_e2e_live` (the app's pool belief
  2048): the two children **took turns** — most streams open at once 1 —
  and both completed with their codenames; twice, before and after the
  client fix. `admission_control_e2e_live` (belief 65536): both streamed at
  once, the server ended both at 1237 + 1233 tokens, and with the fix the
  committed child lands **`Failed`** (`“KEL`) while the uncommitted one is
  retried by the decorator and completes alone — the recovery §2.3 of the
  research had described as the status quo, live for the first time.
  **And on the LAN stack** (Qwen 3.6 27B Q4_K_M, b10807, relaunched
  `-np 4 --kv-unified -c 16384` — four slots over one 16384 pool; the
  smoke sizes itself from `/props`, so the same arms ran: 300 paragraphs
  per child, cap 2048): `parallel_subagents_e2e_live` GO as the
  regression (26 s); `admission_e2e_live` — two ~9k-token children took
  turns (most open at once **1**), both codes, 20 s; the control arm
  (belief 65536) — both at once, the second child **`Failed`** with no
  reply, the first completed, 14 s; `admission_four_e2e_live` — four
  children at `sessions = 4`, a quarter of the pool plus the cap each: the
  budget kept **exactly two** streaming at any time (two fit, three do
  not) and all four completed with their codenames, 30 s. A one-slot
  server (`-np 1`, how the stack was first found) makes the arms skip by
  design: `pool.rs` guards nothing below two reported slots, and one slot
  queues. **The whole `--ignored` set on that stack: 133 passed, 3
  failed, 65 min** — the three failures are the vision smokes
  (`image_attachment_e2e_live`, `image_url_attachment_e2e_live`,
  `tool_result_image_is_seen_live`) on a relaunch without `--mmproj`
  (`/props`: `vision: false`; the server's own "image input is not
  supported - hint: … provide the mmproj"), failing honestly for want of
  `MINDFORK_LIVE_TEXT_ONLY=1` rather than skipping — not this track's, and
  every turn-path smoke green.
- **Tests**: 2781 → 2798 unit tests green (+17: `shared/session_budget.rs`
  — fit together, a third waits and is admitted on a release, larger than
  the pool alone/not alone, a cancelled waiter leaves no reservation, no
  pool never waits, the count still bounds, pricing with the density
  floored at 1.0, the floor and the no-cap pool; `orchestrator/pool.rs` —
  one session, managed, external by reported slots, the clouds;
  `tests/parallel.rs` over the keyed recorder now reporting `usage` and
  the open-stream count at each arrival — two children that do not fit a
  4000-token pool take turns and both complete, the parent's exact usage
  scaling the children's reservations from two-at-once to one, a child's
  second round floored by its first round's exact size waiting for its
  sibling's long reply to end; the summary reserving under a pool
  (`fetch.rs`); the client's in-stream envelope; the empty cancelled
  round), 133 → 136 `#[ignore]` (+3: the three live arms above). The
  group's existing tests set a roomy `context_size`, since at the default
  8192 two children capped at the profile's 2048 would now take turns.
- **Docs**: spec §3.4, §6.3, §9.3.1–§9.3.2, §11.6; architecture §3 (the
  two modules), §5, §8; install.md §3; the `sessions` hint (en/ru);
  CHANGELOG Changed + Fixed; roadmap; parallel-subagents.md §4.7/§8;
  lessons §2.
### Post-M9: the parked-set bound of the RAM prompt cache, measured (done)
- **What**: the last open measurement of the parallel sub-agent track's §8
  ([docs/research/parallel-subagents.md](../research/parallel-subagents.md)
  §3.6, `tools/cache_ram_probe.py`): how many contexts llama.cpp's RAM
  prompt cache (`--cache-ram`, 8192 MiB by default) parks before a
  one-slot rotation of runs pays the full prefill again. On the LAN stack
  (Qwen 3.6 27B Q4_K_M, b10807, `-np 4 --kv-unified -c 16384`), every
  request pinned to one slot, conversations of 13 926 tokens added one at
  a time and every earlier one revisited after each addition: **four**
  come back (`cache_n` ~13.9k, `prompt_n` 26, 220 ms), the fifth evicts
  one, and from six on **none** come back — a round-robin over an LRU
  cache one entry too small evicts at every visit the entry the next
  visit needs, so the cost is a cliff (every round of every run at the
  full 5 s prefill), not one extra prefill. About 150 KB of KV state per
  token on this model; a 16k context ~2.3 GB. Written into the research
  (§3.6, §5, §8) and install.md next to `-c`: raising `subagent_parallel`
  on a long-context profile is a reason to raise `--cache-ram` with it.
  Not measured then: the 31B's own bound — the next entry, on a rented
  L40S (2026-09-05).

### Post-M9: the parked-set bound on Gemma 4 31B, measured on a rented L40S (done)
- **What**: the number the previous entry left open — how many contexts
  of the 31B the RAM prompt cache parks — measured on a Hugging Face
  Inference Endpoint instead of the LAN stack, whose single RTX 4090 does
  not carry four sessions comfortably
  ([docs/research/parallel-subagents.md](../research/parallel-subagents.md)
  §3.7): `tools/e2e_hf.py run --no-embed --command "python
  tools/cache_ram_probe.py - 8 14000"`, the probe taught to read the
  endpoint and the bearer from the runner's environment, to refuse a split
  pool (an `n_ctx` a conversation does not fit) and to label a series so
  two on one server share no prefix; `gemma-4-31B_q4_0-it` without the
  projector on an L40S, `server-cuda-b10795` pinned, `-np 4 --kv-unified
  -c 16384` through `nParallel` + `LLAMA_ARG_KV_UNIFIED`,
  `LLAMA_ARG_CACHE_RAM` at 8192 and at 20480. Three deploys, 31 minutes,
  about $0.93.
- **Result**: the default 8 GiB parks **one** 16k conversation of the 31B
  (two fresh ones would fit, but the first exchange after a park adds
  0.8–1.6 GiB); 20 GiB parks three. A fresh 16k entry is 3 663 MiB, read
  off the server's own log (the endpoints API serves the container log,
  and the eviction line prints every entry's size at WARN): 1.25 GiB of KV
  — 80 KiB per token, the 10 global-attention layers — plus three ~805 MiB
  copies of the 50 sliding-window layers' 1 024-cell window, the live one
  and the two context checkpoints the server keeps with the prompt, one
  more per exchange. §3.6's "150 KB per token" for the 27B is the same
  mechanism over its recurrent layers; the honest unit is per parked
  conversation. Restore 1.7 s against 8 s cold. The cold prefill ran at
  ~2 100 tok/s on this stack, so the LAN's ~50 tok/s on this model is the
  projector or the Windows build, not the model's path.
- **Found on the way**: a series label costs a token per paragraph on
  Gemma's tokenizer (466 of them pushed 14 000 nominal tokens to 16 704,
  over the pool — one wasted series, the probe's usage now says so); GHCR
  carries `server-cuda-b<build>` tags for a few builds only (b10795 alone
  between b10780 and b10840); HF's `/logs` route stays out of `hf_api.py`
  because nothing in the gate reads it — the probe's need was one-off,
  served by a scratch poller.
- Written into research §3.7 (and §3.6, §5, §8), install.md §3, and the
  roadmap (the prefill item narrowed).

### Post-M9: sub-agents in the background — a run that outlives its turn, stage 1 (done)
- **What**: the last open item of the parallel track's §8, designed in
  [docs/research/background-subagents.md](../research/background-subagents.md)
  (forks F1–F10 decided by the user 2026-09-05; F3 against every
  confirmation) and built as stage 1: `start_subagent` — a second tool, not
  a flag on `call_subagent`, because the probe found Claude omitting an
  optional boolean 0/3 while a tool of its own is used 3/3 on every cloud
  and 5/5 on Qwen 3.6 27B — behind `ToolGate::Background`
  (`tools.subagent_background`, off), so the default catalog's effective set
  and every request stay byte-identical. The loop resolves the call at its
  position (`start_background`): the `ChildSpec` a group child would get,
  over a fresh token, goes to the orchestrator as
  `TurnProgress::BackgroundStart` with the cloneable half of `TurnShared`;
  the call's result is the *started* line with the transcript's address,
  and a placeholder run (`background: true`, no outcome) lands on the record
  with the turn. `background_runs.rs` spawns the run as a task of its own —
  `spawn_background_run` builds a `TurnShared` with no confirmation round
  trip and the **app-wide** session budget (`session_budget()`, memoized per
  mode/count/pool, the one `Arc` every turn and run streams under) and runs
  the very same `run_child`; its progress is forwarded under a generation id
  of its own, its end after the channel closes. The landing re-finds the
  record by id (`Chat::child_mut_including_deleted`), deferred to
  `handle_done` when a turn is still running in that chat — the first
  attempt landed nothing because the placeholder was not in `Chat` yet, the
  second lost a race between the forwarded `ChildEnded` and the `Done` on a
  second sender, hence the oneshot behind the forwarder — then appends the
  notification row (`Message::notification`, a `System` row the feed draws
  as a note, sent as user text merged in front of the next user message by
  `request::api_messages`) and wakes the assistant when the chat is open and
  idle. `Esc` never touches a run; `/subagents stop [n]` (the subcommand
  parser now keeps a word's tail), take-back/regenerate (the notification
  is a user-side boundary for both), a deleted chat and `Quit` end it —
  `Quit` landing every run *cancelled* from its mirror before the exit flush.
  The cap (`BackgroundSlots`, `subagent_background_max`) refuses with the
  setting's name. Three settings rows, the `unfinished` label, the
  *"in background: n"* indicator, `en`/`ru`.
- **Tests**: nine over the keyed engine (`tests/background.rs`) — delivery
  at landing with the wake, one session taking turns (`max_in_flight` 1),
  stop after the turn, `Esc` leaving the run out, the wake off and the
  merged message, the cap, `Quit`, the archive on take-back, the gate — the
  wire mapping, the parser, the tool's twin shape; the whole suite green.
  The gallery height rose from 30 to 33 for the three rows (the dumps and
  images regenerated). Live: `background_subagent_e2e_live` on the LAN stack
  (Qwen 3.6 27B) — see the PR.
- **Not in stage 1** (research §7–§8): a key on the open transcript that
  stops the run, the unread mark on the list, background dialogues.

### Post-M9: sub-agents in the background — the stop key, the unread mark, and Gemma (stage 2, done)
- **What**: the track's second stage, the two items
  [docs/research/background-subagents.md](../research/background-subagents.md)
  §7 left for "what the live run argues for", plus that live run itself. The
  stop key first, because it is the reason the footer rule of spec §11.2 had
  an exception: a background run's open transcript is the one feed in the app
  that **streams under no turn**, and `generating` — read by the status bar's
  `Esc` hint, the input box's title and the key handler alike — meant "your
  turn is running" everywhere else. So the fact travels from where it is
  known: `LiveTurn` gained `background`, set by `activate_focused` for a seat
  of `background_runs` **whose run has not ended** (an ended one waiting for
  its chat's turn to land streams nothing, and a stop key there would stop
  nothing), carried into `ChatScreen::background_run` and
  `StatusModel::background_run`. On that screen `Esc` goes back instead of
  cancelling a turn that does not exist, and `F6` stops the run through
  `typed_subagents_stop` — the `/subagents stop` route itself, made
  `pub(super)`, so the key and the command cannot grow two behaviours. The
  hint appears in the corner block only while the run streams (`hotkey_list`
  appends it, `keep_order` puts it right behind `F1`), and `finish_generation`
  clears the flag whatever the outcome. Second, the **unread** mark:
  `deliver_notification` sets `Chat.unread` (additive, `#[serde(default)]`,
  no schema step) when the result lands in a chat that is not the open one,
  and `activate_focused` clears it — opening the chat *is* the read, and the
  mark survives a restart, so a result nobody came back to is still announced
  after `Quit`. The mirror of a run that ended while its chat's turn was
  still running now also carries the run's **messages**, so the transcript
  shows the whole run during that wait rather than the rounds filed so far.
  The settings hints gained what the research §3.3 measured — a run out is
  one more conversation the server parks, so `--cache-ram` goes up with it —
  and now name both stop routes and the unread mark.
- **Tests**: 2824 green (+26 over stage 1), 137 `#[ignore]`. Four over the
  keyed engine (`tests/background.rs`): the transcript activates as a
  *background* live turn where the parent mid-turn activates as a turn's, and
  neither switch cancels; an ended run waiting to land opens with **no** live
  turn; a result landing in a closed chat marks it unread, starts no turn,
  and the mark is in the file after `Quit`; opening the chat clears it. Five
  on the chat screen (`Esc`/`F6` on a streaming background transcript, the
  same two after the run ends, a turn child's transcript keeping `Esc` as
  cancel, `F6` doing nothing on a chat, and the **rendered** footer offering
  `F6` only while the run streams), one on the status bar (the shedding order
  with the stop key, the `Esc` label mid-stream), one on the list widget (the
  unread row) and one on the entity (the additive round trip).
- **Live — GO on Gemma 4 31B**, the model §7 left pending because the LAN
  stack (one RTX 4090) cannot host the 31B at several sessions: rented as an
  L40S through `tools/e2e_hf.py run --chat-model gemma-4-31b --no-embed
  --command …` (the user's route, 2026-09-05). `background_subagent_e2e_live`
  green in **31.6 s** — the parent delegated with `start_subagent`, answered
  the arithmetic in the same reply, the run read the planted file with
  `fs_read`, and the woken turn reported the codename — and the §3.1
  behaviour probe scored **5/5** background on S1, **5/5** foreground on S2,
  **5/5** notification use with no re-call and 5/5 on the objection, i.e. the
  top of the bar (≥ 4/5, 0/5, ≥ 4/5), matching Qwen 3.6 27B. Two things cost
  a rental each and are recorded for the next one: the first deployment sat
  1048 s *waiting to be scheduled* and then failed while the L40S catalogue
  read `available` (capacity, not configuration; the retry deployed in ~510
  s), and the probe died mid-run on a `cp1252` console the first time Gemma
  answered with an emoji — a traceback that reads exactly like a model
  failure until you read it (lessons §3). Three endpoints, each deleted with
  the deletion verified.
- **Not in the track** (research §8): background dialogues, a tasks screen
  across chats, the silent background tasks under the app-wide budget.

### Post-M9: background dialogues — a directed scene that outlives its turn (done)
- **What**: the last of the background sub-agent track's leftovers
  ([docs/research/background-dialogues.md](../research/background-dialogues.md),
  forks F1–F11 decided by the user 2026-09-05;
  [ADR 0011](../decisions/0011-dialogue-directed-run.md) amended).
  `start_dialogue` is to `run_dialogue` what `start_subagent` is to
  `call_subagent`: the same scene behind the same switch, answering at once
  with the transcript's `chat://` address while the scene plays on past the
  turn. The inventory is why the track was small — the seat, the mirror, the
  landing by id, the notification and the wake, the stop key, the unread mark
  are all keyed by **run id** and were already kind-agnostic — so what this
  PR adds is the start, the spec and the words: `BackgroundStart` carries a
  two-variant `RunSpec`, a `DialogueSpec` holding the parsed scene plus the
  director's inputs **snapshotted at the call** (fork F3: the parent's
  persona and the folded conversation brief, because a scene ending twenty
  minutes later has no turn to read them from), and `spawn_background_run`
  branches once at the end into the very same `DialogueCtx::run_parsed` the
  turn's own `run_dialogue` enters — the seam the preceding refactor built.
  `finish_landing` picks the notification's wording by `run.kind` (F8); the
  cap, the gate, `/subagents stop [n]` and `F6` are shared with sub-agents
  (F2, F7, F9) and their texts now say "background run" where they said
  "sub-agent".
- **Two fixes the track carried**, both measured rather than guessed:
  - **The scene was streaming outside the session budget.** `dialogue_stream`
    called `stream_round` directly while `TurnLoop::stream` priced and
    acquired a permit — invisible while a scene was the only thing running,
    and live since background sub-agents shipped: a *foreground* scene's ~25
    requests could overlap a background run on a one-session engine, the
    collective failure admission-by-budget exists to prevent. Every dialogue
    request now takes a permit and a pool reservation (F4), which is also
    what carries ADR 0011's one-request contract now that a scene can outlive
    its turn. The test that pins it fails at `max_in_flight = 2` with the
    permit removed.
  - **The wake turn could land empty.** The probe found the turn the app
    starts on a notification spending its whole reply cap in
    `reasoning_content` — 2 of 5 on the gate model, `finish_reason: length`,
    no text and no calls — with the *next* turn answering correctly from the
    same notification, which is what proved the model had read it. A muted
    re-ask recovered both, 2/2, so a woken turn now gets the one-shot muted
    re-ask a dialogue's line has (F11). This was a defect of the **shipped**
    background sub-agent feature, found by this track's probe.
- **Tests**: 2830 green (+6), 138 `#[ignore]`. The six: the scene lands
  by id with a `kind: Dialogue` placeholder and a notification in the
  dialogue's wording (asserted against both keys rendered in both languages,
  so it pins *which* text the landing chose); `Esc` leaves the scene out and
  the stop lands it *cancelled*; at one session the scene and the turns take
  turns (the mutation above); the shared cap refuses a scene when a sub-agent
  is already out; the gate adds exactly the two twins; and the woken turn's
  muted re-ask. Two traps on the way, both already in
  [lessons.md](../lessons.md): the keyed engine mock routes by the first
  matching key and the director's system quotes the call's arguments, so the
  director's queue has to come first; and `wait_for` drains, so the turn's
  end must be pulled before the scene's.
- **Live — GO on Qwen 3.6 27B** (LAN stack, b10807):
  `background_dialogue_e2e_live` — asked for a scene to read later *and* for
  something answerable now, the model staged the scene with `start_dialogue`
  and answered the arithmetic in the same reply; the scene completed by the
  director's decision in 5 spoken lines and 2748 tokens, the notification
  carried its closing reason and summary in the dialogue's wording, and the
  app's own woken turn reported it. 233 s.
  The first two runs failed with the parent turn producing **nothing** — no
  text, no calls — and the honest diagnosis was not "a flake": at
  `AppConfig::default()`'s **2048**-token cap this smoke's two-part ask spends
  the whole reply in `reasoning_content`, the measured ceiling
  [ci.md](ci.md) already records for open-ended asks on a thinking model. The
  smoke sets 4096 and says so, and it now names an empty parent turn for what
  it is instead of reporting it as a refusal to use the tool.

### Post-M9: `ToolOutcome.wrote` — a memory writer reports its write (done)

**What.** One field on the tool contract: `ToolOutcome.wrote: bool` — *this
call changed the profile's stored memory*, the self-model or a note —
`false` from every constructor, set builder-style (`.wrote()`,
`.wrote_if(created)`) on the success path of each of the nine memory
writers on the line that returns after the storage call, never by a
refusal. The consumer is the silent loops' refund rule
([docs/research/acted-by-effect.md](../research/acted-by-effect.md); the
engine journal has the track): a stopped or quit reflection or
consolidation gives its window back unless a call wrote. The turn's loop
ignores the field. `Tool::concurrent()` was not reused: it is a read-only
claim for concurrency, and `note_recall` (a cache write inside a read) and
`note_neighbors` are unmarked. Tests pin each writer's success path to
`true` and its refusal to `false`, and the reader to `false`.

### Post-M9: `ToolOutcome.prefill` and the page summary's request — reasoning muted, usage recorded (done)

**What.** A second fact on the tool contract beside `wrote`:
`ToolOutcome.prefill: Option<Prefill>` — *the engine's timing of a request
this call made on its own* — `None` from the constructors, set
builder-style (`.with_prefill(sample)`) by the one producer, `fetch_url`'s
page summary, on both of its paths (a page under the attachment budget
and one over it, whatever the text made of the summary). The consumers
are the two loops that call tools: the turn folds it into the largest
prefill sample it keeps for the slow-prefill note, the silent loop onto
its landing ([docs/research/page-summary-usage.md](../research/page-summary-usage.md)
§3.2; the engine journal has the track). Two more changes to the
summary's request in the same track: it records its exact count under
the budget's `Summary` kind at its `Usage` chunk — measured, a page's
text is the one kind of request that under-counts as a rule, 1.04–1.28
on four pages of five — and it mutes reasoning (`reasoning_budget = 0`,
the title's shape), since a thinking model spent the whole reply cap on
thoughts and answered with nothing on every page tried, the inline path
then handing over the raw text under "summary unavailable" and the
attached path saying nothing. Tests pin the record, the sample on both
paths (and `None` without a summary or without a usage chunk), and the
muted request; the live smoke `summary_usage_e2e_live` runs the JSON
page under a budget on the LAN stack and the CPU build.

### Post-M9: the dialogue's director on Gemma's template — the checkpoint's history must alternate too (done)

**What.** The question the gemma-impersonation track left
([docs/research/gemma-impersonation.md](../research/gemma-impersonation.md)
§7): does a directed dialogue's first line go out as a system prompt with
no turns, which Gemma 3's template would drop the persona with? The
design [docs/research/dialogue-director-history.md](../research/dialogue-director-history.md),
every fork at its recommendation (the user's decision, 2026-09-09).

**Measured (stage 0).** Through `/v1/chat/completions` on Gemma 4 31B and
Gemma 3 4B: a participant's view — the user-side prologue and the merge
`participant_view` does — is accepted by both; the director's first
checkpoint too; the director's **second** checkpoint — `[user,
assistant(tool_call), tool("Noted."), user]` — is a `400` on Gemma 3:
its template has no tool role, `llama-server` renders the `tool` result
as a user turn, and the pair of user turns is refused. The clouds never
saw the pair (their wires merge adjacent same-role turns); the dialogue
track's Gemma arm was Gemma 4, whose template takes any shape — 53/53
checkpoints there. Two repaired shapes — the verdict as the director's
own text turn, and a stateless full-script checkpoint — are accepted by
both templates and answer with the same verdict. A side fact: Gemma 3 4B
names the verdict as text and never calls; the app reads no call as
*continue*.

**How.** `verdict_turn(text, calls)` in `features/tools/dialogue.rs`:
the model's text, then each call as `name(arguments)` on its own line,
empty arguments left out; `dialogue_checkpoint` pushes it as one
`assistant` turn and no `tool` messages (F1a, F2a). The conversation
stays persistent and append-only — the notes it gave are in the turns,
the cached prefix is the same prefix — and alternates on every
template. `prompt.dialogue.noted` left both locales with its only use.
The unit test that pinned the second checkpoint at four messages pins
three, the verdict's text, and no `Tool` role in the director's history.

**Live** (F4a). `dialogue_e2e_live` and `background_dialogue_e2e_live` on
Gemma 4: both scenes Completed at 5 spoken lines (1296 and 1256 tokens),
stopped by the director at the third checkpoint — the second passed with
the text-turn history. Gemma 3's acceptance of the shape is the stage-0
measurement. Unit: 2987 green, 153 ignored (2986 / 153 before).

**Not in this track** (§7): a verdict answered in text on a model that
cannot call tools; the first checkpoint's prompt size on Gemma 3 with
the schemas; the opening-only impersonation.

### Post-M9: a fetched page is read in its own encoding (done)

**What.** Reported from a chat: the assistant read a 2002 article on
`sector.biz.ua` with `fetch_url`, and the page — over the attachment budget —
became an attachment whose name and nearly every letter were `U+FFFD` (9 289 of
12 549 characters). Four `attachment_read` calls returned the noise, and the model
fetched the page again through `python_exec` and `cp1251` before it could answer.
Research and design: [docs/research/page-charset.md](../research/page-charset.md) —
the user's decision (2026-09-11): "the most effective variant" (F1b), with F2–F7
taken under it and listed there. Branch `fix/page-charset`.

**Why.** The page declared windows-1251 in its header and in `<meta>`, and the
client read neither. `reqwest` has had `default-features = false` since M1, and
without its `charset` feature `Response::text()` is `String::from_utf8_lossy`. The
proof was exact: the page's `<h1>` bytes decoded lossily equal the stored name. The
corpus measurement then found a second cause of the same symptom — `www.163.com`
gzips a response nobody asked to be compressed, and `reqwest` is built without
decompression either.

**Measured** (research §2). Fourteen pages in five legacy encodings plus UTF-8
controls: four declare only in the document, so `charset`'s header-only reading
would have fixed the chat's page and not a Shift_JIS or an ISO-8859-1 one;
habr.com's `<meta>` sits at byte 1698, past a browser's 1024-byte prescan; legacy
text reads as UTF-8 at worst 59 decoded against 218 broken (GBK), while every UTF-8
page has 0 broken — the margin behind the majority rule. Through the app's client
the detector guessed every legacy page, naming KOI8-U for KOI8-R; without the TLD,
fourmilab.ch's two accented bytes read as windows-1257.

**How.** `shared::http_text::read(resp)`, one seam for `fetch_url` and
`web_search`'s result fetch: `Content-Encoding` undone first (`gzip`/`x-gzip`/
`deflate` through `flate2`, last applied first, at most 32 MiB inflated), then the
order BOM → UTF-8 by majority of the bytes → pure ASCII as declared → a declaration
the bytes do not refute (the header, or `<meta>`/an XML declaration up to `<body>`;
a disagreement settled by whether the detector's guess reads like one of them) →
`chardetng` hinted by the TLD. A coding it cannot undo reaches the model as
`tool.fetch_url.err.compressed`, which says a retry fails the same way. Two direct
dependencies, both within `deny.toml` and credited: `encoding_rs` (already in the
graph via `pdf-extract`) and `chardetng`.

**Found on the way.** Comparing the detector's guess with a declaration by identity
was a real defect, and the table caught it: KOI8-U is not KOI8-R, so a KOI8-R page
with a wrong `windows-1251` header and a right `<meta>` would have taken the header.
`reads_alike` compares what the two decode to. And a `U+FFFD` count is a vacuous
smoke for a single-byte decode, which never produces one: the corpus test asserts
agreement with the detector for a legacy decision instead.

**Tests.** A 28-row decision table in one raw-string literal (case, the prose's
script, the bytes' shape, `Content-Type`, the head, the TLD, the expected encoding
and step); GBK prose checked to really form some valid UTF-8 pairs; the
content-coding undo, its three errors, and the TLD sanitizer (`chardetng` panics on
upper case or a period); a wire test each through `fetch_url`'s and `web_search`'s
real request path, against a stub serving a meta-only, gzip-encoded windows-1251
page. **Mutation-tested** — sixteen mutations, each killed by the case written for
it: the majority rule, the pure-ASCII branch, a refuted UTF-8 obeyed, a disagreement
sent always to the header and always to the document, `reads_alike` as identity, the
`replacement` label, UTF-16 in a document, comments, the stop at `<body>`, the markup
gate, `http-equiv`, the XML declaration, the TLD hint, and — through both callers —
`read` decoding lossily again and gzip left packed. The TLD hint first survived, pinned
only by the live corpus, and got a row of its own: fourmilab.ch's one accented letter
on an undeclared page. Unit: 3050 green, 158 ignored (3043 / 156 before).

**Live — GO** (network only). `live_windows_1251_page_is_readable`: the chat's page
attaches under the archive's `<h1>`, 12 632 characters, the article's title in it,
no `U+FFFD`. `live_charset_corpus`: fourteen pages, every decision in agreement with
the evidence, `163.com` unpacked from gzip. Not done, recorded in research §5: a
lone wrong legacy declaration, undeclared ISO-2022-JP, the `<h1>` naming the
archive's banner rather than the article, and local files.

### Post-M9: local files are read in their own encoding (done)

**What.** The follow-up the page-charset entry left: every path that read a user's file
assumed UTF-8. Research, inventory and forks:
[docs/research/local-file-encoding.md](../research/local-file-encoding.md) — the user's
decisions of 2026-09-11, every fork as recommended (F1a every reading path, F2c the
interface language as the hint, F3b edits written back in the file's own encoding, F4b
the encoding named, F5b the order moved to `shared::text_decode` first in a refactor PR
of its own, F6a reading and writing in one PR). Branch `fix/local-file-encoding`.

**Measured first** (research §1–§2). Twelve paths turn a user's file into text; driven
through the tools' own code with files in windows-1251, KOI8-R, IBM866, UTF-16LE and
UTF-8 with and without a BOM, `fs_read` and `code_read` showed `U+FFFD`, `code_grep`
could not find a word, `/file attach` and `/rag add` refused, UTF-16 was "not a text
file" — and `code_edit` rewrote a windows-1251 source with `EF BF BD` for all 68 letters
of its comments while the changes screen showed `+1 −1`. The page decoder read all 24
fixtures right; short text needed a language hint (21 of 32 short Russian strings
without, 27 with `ru`), and the OS locale would have been the wrong hint on this very
machine (`en-US`, code page 1252). Every wrong guess on 113 short samples still
round-tripped byte for byte — the property F3b rests on. IBM866 turned out the majority
rule's worst case: two single words taken for UTF-8.

**How.** `text_decode::decode_file` (the order with no transport; a NUL is binary unless
the file is whole BOM'd UTF-16; `<meta>` only for markup), `encode` (the first character
an encoding cannot store; UTF-16 by hand, since `encoding_rs` has no encoder for it),
`round_trips`, `bom_of`, `tld_hint`. `TextFile` carries the encoding: `code_read` names
one that is not UTF-8, `code_grep` searches decoded text, and `code_edit`/`code_write`
write back in it behind the round-trip check, refusing — nothing written, nothing
journaled — a character the encoding lacks or a lossy read. `fs_read` refuses a binary
and notes a non-UTF-8 encoding; `/file attach`, `/rag add` and `/rag rebuild` read through
`read_source_text(path, hint)`, and the attach note names the encoding. The changes
screen decodes both sides in the current file's encoding and, over equal texts, says
*unchanged* only when both read losslessly or the bytes are equal. The hint rides
`ToolParams::from_config` into `ToolContext.file_hint`; the orchestrator's attach, RAG
and changes paths take it from `config.interface.language`.

**Found on the way.** The screen that exists to show what the assistant changed was the
one that hid the damage, by comparing two lossy renderings — lessons §3, *an equality over
a lossy view is not an equality*. And `FileText` carries no decision step: clippy's
dead-code check found it read by tests alone.

**Tests.** The decision helpers (a whole UTF-16 file against a blob opening `FF FE`, a
`<meta>` read only in markup, the character an encoding lacks, lossless round trips, the
hint's labels); each reading path through its real entry point — `CodeTool::{Read, Grep,
Edit, Write}`, `FsRead`, `read_text`, `extract_file`, `workspace_diff::build` — with
windows-1251 and UTF-16 fixtures; both edit refusals writing and journaling nothing; the
byte-exact edit that used to corrupt; the hint taken from the interface language.
**Mutation-tested** — twelve mutations, each killed by the test written for it: the
round-trip check, the refusal of a character the encoding lacks, the byte comparison on
the changes screen, the UTF-16 branch, the NUL rule, the hint taken from the interface
language, the baseline decoded in the current file's encoding, `code_write` keeping the
encoding, `fs_read` and `code_read` naming it, the attachment carrying it, and `<meta>`
read only in markup. Pinned by no unit test: the hint reaching `/file attach`, `/rag`
and the changes screen from the orchestrator — every fixture there is long enough to
decode right without it. Unit: 3065 green, 159 ignored (3050 / 158 before).

**Live — GO** (`code_edit_legacy_encoding_e2e_live`; the LAN stack was down, so a local
CPU build of llama.cpp served Gemma 4 12B QAT Q4_0 at `-c 16384 --jinja`). The model
listed the project and read `src/discount.rs` — the header said `windows-1251`, the
comments came back as Russian — then made one `code_edit` changing both the divisor and
the comment about it. On disk the file is windows-1251: no `EF BF BD`, the untouched
first line byte for byte the same, and the new Russian word the model wrote stored in
windows-1251. The old path would have written `U+FFFD` over every letter of both
comments.

### Post-M9: a fetched page's attachment is named after the page (done)

**What.** The follow-up the page-charset entry recorded as not done: with sector.biz.ua's
article readable, its attachment was named after the archive — the banner every article
of the site carries in its `<h1>` — because `page_name` took `<h1>` first, the order
docs.vlang.io's repeated `<title>` had called for. Research, corpus and forks:
[docs/research/page-attachment-name.md](../research/page-attachment-name.md) — the
user's decisions of 2026-09-11, every fork as recommended (F1c the agreement rule, F2a
everything before the title's last separator, F3a the name cleaned). Branch
`fix/page-attachment-name`.

**Measured first** (research §2–§3). 43 sites and 87 pages — documentation, reference,
news, archives, blogs, forums; 11 of them in windows-1251 or KOI8-R — fetched with the
tool's own headers and saved; a probe read the fields, and every candidate rule ran again
inside the test binary on the saved bytes. The `<h1>`-first rule named every page of six
sites after the site (sector.biz.ua, the Rust Book's mdBook banner, jvns.ca,
simonwillison.net, astralcodexten.com, the Arch Linux forums); a repeated `<title>` was
docs.vlang.io's alone; `og:title`, on 55 pages, repeated on none, but three of the six
sites do not set it. Each simple reordering left three sites colliding; agreement between
two fields left none. The probe and the app disagreed on one site, and the app is the one
that counts: Discourse's `<h1>` sits inside `<noscript>`, which `scraper`'s `html5ever`
reads as text.

**The premise corrected.** The task said `attachment_read` resolves a name to its first
match; since the fidelity track it reports an ambiguous name with each candidate's
source, and the comments on `page_name` saying otherwise were stale. What was wrong was
the name's meaning, not a silent misread.

**How.** `NameFields::read` collects the `<title>`, every `<h1>`, `og:title` and
`og:site_name`, whitespace collapsed and a heading's permalink mark (`¶`, a zero-width
space) trimmed. `NameFields::name`, first match wins: `og:title` unless it is the site
(`og:site_name`, or the title's last segment), without a site suffix it shares with the
title; an `<h1>` the title begins with at a word's end, spelled as the title spells it;
the title without its last segment (`split_last_segment` over ten spaced separators);
the first `<h1>`; the title. `unique_name` is unchanged — the backstop for a page whose
every field names its site.

**Found on the way.** The two previous entries in this file (page charset, local files)
had each been inserted above the closing *Not in this track* paragraph of the dialogue
director's entry, leaving that paragraph under the wrong entry; it is back in its own.

**Tests.** A table of fourteen fixtures through `body_to_text`, one per measured shape,
each pinning the step that must name it; the title's split, one row per separator, with
unspaced ones left alone; and through the tool, two windows-1251 articles under the
archive's banner `<h1>`, served locally, attached as their articles — the second with no
URL segment. On the saved corpus the implemented function gave the prototype's name on
every page, and no site two pages under one name. **Mutation-tested** — eighteen
mutations, every one killed: each step dropped, each site check, the suffix kept, only
the first `<h1>` allowed to agree, the `<h1>`'s own spelling, a case-sensitive
comparison, no word boundary, the first segment kept instead of everything before the
last, the `<h1>`-first order restored, either permalink mark kept, two separators
dropped, and `og:title` not read. Unit: 3067 green, 159 ignored (3065 / 159 before).

**Live — GO** (network only; the name is not the model's). sector.biz.ua's article
attached under the article's own title, not the archive's banner (12 657 characters);
docs.vlang.io's memory-management page is still "Memory management", four pages by
reference.

**Not in this track** (research §5, §7): a bare-titled page under a banner `<h1>` with no
`og:title`; a site-first title with no `og:title`; `/file remove <name>` acting on the
first match.

### Post-M9: Python sandbox — the starter set grows: sympy, lxml, openpyxl, matplotlib and more (done)

**What.** The user asked which packages the sandbox holds and which would widen what the
assistant can do; after a review against today's WASIX index they chose the whole first
tier — sympy (with mpmath), tabulate, lxml, openpyxl, pypdf, pyyaml, regex, networkx —
plus feedparser, pillow and matplotlib (2026-09-11). Twenty wheels with their
dependencies; branch `feat/sandbox-packages`. A mechanical lock-list change by ADR 0005
§4, so no design doc — but matplotlib did not work as installed, and one finding reached
past the track.

**Measured first.** Every wheel downloaded, hash-verified and unpacked into a scratch copy
of the dev `site-packages`, then exercised in the real sandbox (wasmer 7.2.0,
`python/python@=3.13.5`) on a fresh compilation cache.
- **The index moved.** The beautifulsoup4 entry left `lxml` out as "not in the index";
  it is there now (`cp313-wasix_wasm32`), as are pyyaml, regex, pillow, matplotlib,
  contourpy and kiwisolver — 63 projects, scipy still absent.
- **Two pins the latest releases would have broken.** sympy 1.14.0 requires
  `mpmath<1.4` and the latest is 1.4.1, so mpmath stays at 1.3.0; feedparser 6.0.14 no
  longer depends on the sdist-only `sgmllib3k` but on `feedparser-sgmllib`, which ships
  a wheel.
- **Everything but matplotlib worked as installed**: sympy solving and integrating,
  `DataFrame.to_markdown()`, lxml's XPath and bs4's `lxml` parser, `pd.read_excel` over
  openpyxl, pypdf, pyyaml with libyaml, `\p{Cyrillic}` in regex, networkx, feedparser,
  pillow's PNG/JPEG/WebP/TIFF (its build has no FreeType).
- **matplotlib failed twice.** At import: the guest has no `HOME`, and matplotlib raises
  "Could not determine home directory". With `MPLCONFIGDIR` set it imported, then
  trapped the moment a chart had text — `wasm-c-api trap: null function or function
  signature mismatch`. Bisected: every native module imports, a figure without text
  saves, SVG with text saves, and `FT2Font.set_text` traps under
  `LoadFlags.FORCE_AUTOHINT` only — `DEFAULT`, `NO_AUTOHINT` and `NO_HINTING` render.
  matplotlib's own default `text.hinting` is `force_autohint`, so every raster chart with
  a label reached FreeType's autohinter, which this build cannot call; `default` renders
  the same chart, a Cyrillic title and legend included (looked at, not only asserted).
  One bisect run "failed" every mode because the probe script was named `bisect.py` and
  shadowed the standard library's `bisect` — an instrument fault, caught by reading the
  traceback rather than the verdict.
- **The cost is bytecode, not native code.** Cold, the new packages took ~17 s to import
  (sympy 6.6 s, networkx 3.2 s, openpyxl 2.0 s) and 3.9 s warm.

**How.**
- The lock list is one string, `WHEEL_LOCK` — a row per wheel, `<dir> <sha256> <url>`,
  `#` comments — parsed by `wheels()`. Thirty-four struct literals of one shape is what
  the duplication gate reads as sliding self-duplication (lessons §2). A script checked
  the fourteen existing rows against `HEAD` byte for byte and the twenty new rows'
  hashes and `dir` names against the verified wheels.
- A second WASIX shim in `build_wrapper`, beside `setsockopt`: `MPLCONFIGDIR=/tmp/matplotlib`
  and a `matplotlibrc` there with `backend: Agg` and `text.hinting: default`. An
  environment variable and one file — matplotlib is not imported unless the code does.
- The warmup runs `compileall` over `site-packages` (test directories skipped), imports
  every native wheel and draws one figure with text. Measured on a fresh cache: 40 s,
  28.6 s of it `compileall`; the bytecode adds 47 MB. Its time limit went from 300 s to
  600 s for slower machines. Precompiled bytecode is also what a read-only
  `site-packages` needs, since nothing could be written back at import (below).
- The tool description names the packages and drops "etc." — **except matplotlib**,
  whose chart has no way out of the sandbox yet: the model is not told about something
  whose result it cannot deliver (lessons §4, "only advertise what exists"). The settings
  hint no longer says "numpy, requests"; install.md's "~300 MB on disk" was stale and is
  now measured.

**Found on the way — `site-packages` is writable from the guest.** ADR 0005 §5 and spec
§13.2 say read-only; `build_args` mounts it with a plain `--volume`, and wasmer 7.2.0 has
no read-only form (`HOST:GUEST:ro` is rejected as a path, and neither the 7.2.0 nor the
`main` source defines one). Proven: one call wrote `/sp/sitecustomize.py`, and the next,
clean call ran it — code the model ran once, under a prompt injection from a fetched page
say, would run inside every later call in every chat. A fix was probed: `site-packages`
packed into a `.webc` (a `wasmer.toml` with an `[fs]` volume and `python/python` as its
dependency) and run over `python.webc` with `--include-webc`. Writes land in an in-memory
layer — the injected file did not survive, the package's hash was unchanged — and the
starter set imported in 3.2 s. A clean call also ran against a dead proxy, which proved less
than it seemed: an earlier online call in the same cache had resolved the `python/python`
dependency, and on a fresh cache the same run fails offline with "Unable to find
python/python@=3.13.5 in the registry" (measured after) — `--include-webc` does not spare
the registry query. The fix is its own PR, and has to work offline first.

**Tests.** `every_lock_row_parses` (each non-comment row splits into three fields —
`wheels()` skips one that does not — with an https wheel URL, a lowercase 64-hex sha256
and a directory no other row claims), `lockfile_covers_the_starter_set`,
`wrapper_prepares_matplotlib_for_wasix`; live smokes `starter_set_packages_work_in_sandbox`
(one computed result per package, not a version) and `matplotlib_renders_text_in_sandbox`.
**Mutation-tested** — three mutations, each killed by the tests that claim it: a lock row
with its sha256 dropped (`every_lock_row_parses` — a row did not parse;
`lockfile_covers_the_starter_set` — tabulate missing); `text.hinting` back to
`force_autohint` (`wrapper_prepares_matplotlib_for_wasix`, and the live smoke on the trap
itself); `MPLCONFIGDIR` renamed (the live smoke on "Could not determine home directory").
Unit: 3074 green, 161 ignored (3072 / 159 before).

**Live — GO.** `mindfork sandbox setup` over the provisioned dev sandbox skipped the
fourteen installed wheels, fetched and unpacked the twenty new ones and warmed up, 25 s in
all; `site-packages` went from 73 to 212 MB and the compilation cache from 358 to 421 MB.
All sixteen `python::tests` smokes green against it — the two new ones, numpy, pandas,
bs4, requests with and without network, the timeout, both memory-cap smokes, the
localized output.

**Not in this track.** Files in and out of the sandbox — the user's decisions of
2026-09-11 go with that track's design doc; the read-only `site-packages` fix above; scipy,
polars and PyTorch, still not in the index; the website's sandbox article, which names the
old set.

### Post-M9: the sandbox's packages, packed read-only (done)

**What.** The defect the starter-set entry found: `site-packages` was mounted into the
guest with a plain `--volume`, `wasmer` 7.2.0 has no read-only volume, and a
`sitecustomize.py` one call wrote there ran inside the next — in every chat, with the
network on by default. ADR 0005 §5, spec §13.2 and the research doc had said read-only
since July. Branch `fix/sandbox-readonly-site-packages`, stacked on `feat/sandbox-packages`.
A defect fix, so no design doc; the one choice the user had not made is recorded below.

**Measured first.**
- **A second package over `python.webc` is not offline.** The probe the starter-set entry
  recorded — `site-packages` in a package of its own depending on `python/python`, run with
  `--include-webc python.webc` — failed against a dead proxy on a fresh cache: "Unable to
  find python/python@=3.13.5 in the registry". Its earlier offline pass had ridden an online
  resolution cached in the same compilation cache (lessons §3).
- **One self-contained package is.** `wasmer package unpack --format package` restores
  `python.webc` as a directory whose manifest has two volumes, one module, one command and no
  dependencies; with `"/sp" = "../site-packages"` added to its `[fs]`, `wasmer package build`
  makes a 257 MB image in about 2 s. Against a dead proxy on a fresh cache it ran — 9.4 s
  cold, 4.2 s warm — a `sitecustomize.py` written to `/sp` did not survive to the next call,
  and nothing reached the host directory.
- **The warmup's cache carries over.** Imports through the image on a cache the warmup had
  filled through the directory were warm at once (3.3 s for the heavy set against the
  directory's own 3.7 s), so the warmup stays on the directory, where its bytecode can be
  written, and the image is packed after it.

**How.**
- `WasmerSandbox` carries a `SiteSource`: `new` is the image, `for_provisioning` the
  directory — writable, and used only by the warmup's own script. `plan()` decides a launch:
  the image wherever it exists (it carries CPython, so `MINDFORK_SANDBOX_PYTHON` has nothing
  to add); a `site-packages` directory without one is `None`; neither is plain CPython.
  `availability` turns `None` into `sandbox.err.needs_repack`, which names
  `mindfork sandbox setup` and says nothing is downloaded again, and `run` refuses the same
  way before anything is spawned.
- `sandbox setup` gained `pack_image` after the warmup — unpack, extend the manifest
  (`with_site_packages`: the volume in `[fs]`, a `name`/`version` only where `[package]` has
  none, `None` for a manifest without `[fs]` rather than a guess), build to
  `packed-sandbox.webc.partial` and rename it over the image, drop the staging directory —
  and `verify_image`, which starts the image once and imports numpy, pandas, lxml and
  matplotlib. An image that does not start fails `setup`: the runtime has nothing else to
  run.
- The image's name has a hyphen: `sandbox.webc` would read to the i18n scanner as a key
  under the `sandbox.` prefix (lessons §7).

**Decided without the user**, recorded for review: an install from before the image is
refused — not mounted, and not repacked on the fly. Mounting keeps the defect; packing
inside a tool call would spend its 30 s timeout on what `setup` does in 13 s. The refusal
closes the door (lessons §4): what is wrong, the one command, and that it downloads nothing.

**Tests.** `the_packed_image_runs_and_nothing_is_mounted`,
`unpacked_site_packages_is_refused_with_the_way_out` (through `availability` and through
`run`), `provisioning_mounts_the_directory`, `without_packages_plain_python_runs`;
`the_image_manifest_adds_site_packages_and_a_name`, on the manifest the pinned
`python.webc` unpacks to, verbatim, and
`the_image_manifest_keeps_a_name_and_refuses_a_manifest_without_fs`. Live smoke
`site_packages_writes_do_not_survive_a_call`: the first call writes `/sp/sitecustomize.py`,
the host file is checked and removed before anything is asserted, the second call must not
print the injection. **Mutation-tested** — `WasmerSandbox::new` back on the directory:
the smoke failed on "the write reached the host's site-packages" (and left nothing behind),
`the_packed_image_runs_and_nothing_is_mounted` failed; the no-image branch mounting
instead of refusing: `unpacked_site_packages_is_refused_with_the_way_out` failed. Unit: 3080
green, 162 ignored (3074 / 161 before).

**Live — GO.** On the dev sandbox as the starter-set track left it — the directory, no
image — `numpy_in_sandbox` failed with the tool's answer: the sandbox is unavailable, run
`mindfork sandbox setup` again. `mindfork sandbox setup` then skipped all 34 wheels, warmed
up, packed the image (256.6 MB) and started it, 12.8 s in all, leaving no staging directory
or partial file. All seventeen `python::tests` smokes green against the image — the new
one, requests with the network, both memory caps, the timeout.

**Not in this track.** Deleting `site-packages` after packing — it is the pack's input and
the wheels' idempotency check, ~210 MB kept on disk; a network allowlist; the website's
article, whose "read-only package library" is now true.

### Post-M9: sandbox file exchange — stage 2, files out of the sandbox (done)

**What.** `python_exec` in its Wasmer mode was text in, text out: whatever the code
wrote died with the call, so the matplotlib the starter set added could draw a chart
nobody would ever see. Now the job directory holds an empty `out/` beside the script;
once the process exits, the regular files directly in `/w/out` are collected, stored in
the chat's folder `data/files/<chat-id>/`, listed in `Chat.files`, and a PNG or JPEG
among them goes back to the model. Track plan and every decision:
[docs/history/sandbox-file-exchange.md](../history/sandbox-file-exchange.md) — the user's forks in §3
and §5, the stage-1 probe in §10, this stage's sub-decisions in §11. Branch
`feat/sandbox-files-out`, stacked on the plan's `docs/sandbox-file-exchange`.

**Measured first** (stage 1, §10). On Qwen 3.6 27B and Gemma 4 31B, each with its vision
projector: a PNG was stored and the input file named unaided in all 30 runs. The drafted
"the model sees the chart" criterion measured nothing — the model printed the answer and
the title is its own code — so the probe set a plotting-area colour no code or output
names: 4/5 and 5/5 with the image, 0/5 blind on both. Blind, both families described a
chart they had not seen, which is why every image withheld now says so in words.

**How.**
- `shared/sandbox.rs`: `SandboxOutput` gains `files` and `skipped`; the run keeps its
  signature (inputs change it in stage 3). `collect_outputs` walks `out/` in name order by
  `symlink_metadata`, reads a file at most one byte past its cap (10 files, 25 MB each,
  50 MB a call), names a folder, a link and anything past a cap as skipped, and runs
  whatever the exit code; after a timeout nothing is read and what `out/` held is named.
- `entities/chat_file.rs`: `ChatFile` (name, origin, MIME, size, SHA-256) and the pure
  rules — `sanitize_name` (last component on both separators, Windows-refused and control
  characters, device names, trailing dots, 120 characters), `versioned`, `same_name`
  (case-insensitive), `sniff_image` by bytes, `mime_for`.
- `features/chat_files.rs`: `store` versions against the listing and the disk, writes with
  `create_new` and syncs; the same bytes under a listed name of the family are
  `Unchanged`. `unlisted` finds what a folder holds that a chat does not list; `remove`
  refuses a name that is not one plain component.
- `python.rs`: the `files:` section after the console — the folder, then one entry per
  output (renamed, unchanged, shown, not shown and why, the head of a text file) — one
  `ChatEffect::AddChatFile` per new file, images behind `tools.python_images` and the
  shared `MAX_TOOL_RESULT_IMAGES`, now in `shared/config.rs`. The description names
  matplotlib and says what `/w/out` does, the caps and the images sentence built from the
  values the run uses.
- The orchestrator: a turn's landing and a background run's both go through
  `list_stored_files` (idempotent by name, one feed note when the chat is the open one); a
  sub-agent routes the effect to the parent; `sync_files` mirrors the turn's snapshot.
  `record_call` withholds a result's images from an engine that reports no vision and says
  so, and says how many `prepare_tool_images` dropped — both were silent, for MCP too.
- `/file list` numbers stored files after the attachments and marks one missing from the
  folder; `/file remove` deletes our copy first and drops the listing second. `files/`
  joins the backup's directories; the card draws the `files:` section under the console;
  a test-only `Paths::with_sandbox_dir` lets a live smoke run the real registry over a
  temporary data root.

**Decided without the user**, recorded in §11 for review:
- **S6 — adopt, never sweep.** F10's text said opening a chat sweeps files it does not
  list. Chat saves are debounced, so a crash after a call wrote its chart and before the
  save leaves a file the user may already have seen on the card, and a background run's
  files are on disk before it lands. At startup no run is in flight: an unlisted file is
  listed as `Recovered`, and nothing is deleted — the user's "nothing may be lost".
- **S3 — one name.** F1's separate `stored` field is gone: the disk name is the handle,
  so two versions never share a name `/file remove` would have to refuse.

**A link in the guest never reaches the host** (measured 2026-09-12, wasmer 7.2.0 on
Windows). The smoke's first version assumed a guest `os.symlink` into `/w/out` would show
up on the host and be named as skipped; it printed "link made" and then nothing. Run
directly against a mounted directory, `os.symlink` and `os.link` both succeed in the guest
— it lists both and reads the target through the symlink — while the host directory stays
empty: guest links live in wasmer's own filesystem layer. The host-side `symlink_metadata`
check is defence in depth, for another host or version, and the smoke now asserts the
property — nothing a link names is kept — rather than the mechanism.

**Found on the way.** `parse_console` refused a result with anything after an exit code,
so a failed run that saved files would have lost its card to flat text — caught by the unit
test written for the section. And `prepare_tool_images` had dropped an oversize or
undecodable MCP image without a word.

**Tests.** Unit: `entities::chat_file` (the sanitizer's table — both separators, a Windows
path read on any host, device names in any case, trailing dots, the cap counted in
characters — versioning, images sniffed by bytes, the listing's round trip);
`features::chat_files` (a file on disk never overwritten, a listed name versioned
case-insensitively, nothing written for the same bytes, a name that is not one component
refused; unlisted files found, nothing deleted); `shared::sandbox::collect_tests` (name
order, a folder named, the three caps in one directory, a timed-out call's names, a link);
`tools::python` (stored, listed and shown; images off; the cap; a quiet run; versioned and
unchanged; a timeout; no folder; a device name and an SVG; the description in both locales;
the text head); `present` (the section after an exit code, and alone); `file_command`, the
`/file list` note, the backup's `files/`; and `orchestrator::tests::files` — a landing once,
the open chat's note, list and remove, a copy that cannot be deleted staying listed, a name
the two lists share, adoption and the bootstrap's call, the mirror, and four turns through
a scripted engine: no vision, a seeing model, an undecodable image, the same chart in two
rounds. **Mutation-tested**, each line broken and put back by a script: the vision gate,
the dropped-image note, the mirror's call site, the bootstrap's adoption, the order of
remove's two writes and the images switch were all caught by their tests; collection
following links survived here only because Windows refused the test its symlink — the unix
variant runs in CI. Unit: 3134 green, 165 ignored (3080 / 162 before).

**Live — GO** (Gemma 4 31B q4_0 with its projector, llama.cpp b10807, one slot; the dev
sandbox's packed image). The nineteen `python::tests` smokes green, among them
`outputs_are_kept_from_a_real_sandbox` — a matplotlib chart and a CSV kept although the
script then exited with 3, the folder named, the file written beside `/w/out` not
collected, the guest's link never on the host — and
`a_timed_out_call_keeps_none_of_its_outputs`. Through the orchestrator,
`sandbox_outputs_e2e_live`: the model saved `sales_chart.png`, `/file list` found it in the
folder, and one image went back; with `python_images` off the same file landed, none went
back, and the result told the model in the profile's Russian that it had not seen it — its
reply still summarised the chart, from the numbers the request gave it.
`image_attachment_e2e_live`, `file_attachment_e2e_live` and
`tool_result_image_is_seen_live` green beside it.

**Not in this track.** Inputs — `/w/in`, `files`, a binary `/file attach` — are stage 3;
`/file open` is stage 4; Local-mode parity stage 5. `workspace/` is still missing from the
backup's directories (a separate task).

### Post-M9: sandbox file exchange — stage 3, files into the sandbox (done)

**What.** The other direction: a call names the chat's files in an optional `files`
argument and each is copied into `/w/in` before the code runs, so a workbook the user
attached reaches pandas and what one call saved a later one reads. The handles are **one
numbered list** — the chat's attachments, then the stored files no attachment links, then
the images its messages carry — and that same list is what `/file list` shows, what
`/file remove` takes, what the pinned block states and what the confirmation popup
resolves, so `#3` means one thing to the user and to the model. `/file attach` stops
refusing what its text is not: a binary is kept with the chat (no attachment made), a
pdf/docx/html keeps its original beside the extracted text, and the pair is one item
everywhere. Track plan and every decision:
[docs/history/sandbox-file-exchange.md](../history/sandbox-file-exchange.md) — the user's forks in §3 and
§5, this stage's sub-decisions in §12. Branch `feat/sandbox-files-in`.

**The survey found two things the plan rested on and that did not exist.** The
confirmation popup presents a call's arguments in its compact view, which drops **arrays
outright** (`present::scalar_str` returns `None` for one) — so `files`, the argument that
decides what leaves the chat, would never have appeared there at all; F13(b)'s "resolved
line" stopped being a nicety and became the only way the popup can say what goes in. And
an image already **sent** has no handle: `/image list` numbers only what is staged for the
*next* message, and no snapshot of a chat's images reached a tool. D3 had named images as
inputs, so that was the user's call rather than ours — decided as T4: they join the one
numbering, `/file list` grows an images tail, and the snapshot (base64 payload included) is
built **only** for a turn that offers `python_exec` in its Wasmer mode, so a chat's pixels
are not copied into every context that has no way to reach them.

**How.**
- `shared/sandbox.rs`: `SandboxRunner::run` takes a `SandboxJob` (code, inputs, net,
  timeout) — the signature stage 2 deliberately left alone, changed once. An input is
  bytes the caller holds or a path copied without being read into memory; `in/` is created
  for every call, so code that looks there finds a folder rather than an error. The
  collection limits stayed a constant: nothing would set them per call, and the
  description is built from the same value.
- `features/chat_inputs.rs` (new, pure): the one list, the resolution, the popup's
  resolved view and the `files` parse. Each item's **guest name is decided with the list,
  not while copying** — the model writes `/w/in/<name>` into code before any result exists
  — sanitized and made unique across the whole list, with an attachment's text gaining
  `.txt` exactly where the text is not the file.
- `python.rs`: `stage` resolves before anything runs. An unknown handle, a name two items
  share, or a listed file whose copy is gone **refuses the call with nothing staged and
  nothing executed**, naming the valid handles — a script that asked for four files and
  got three would answer confidently from three (lessons §4). A handle named twice is one
  copy. The `files` argument exists in Wasmer mode only: Local has no job directory until
  stage 5, and ADR 0005 §3 records the divergence rather than letting a model name files
  that would never arrive.
- `request.rs`: `inject_files` after the attachments — names, sizes, staged names, and the
  two sentences that close the door (copies; `/w/out` is the way back). The caller passes
  an empty list unless the turn offers the tool in Wasmer mode, so the block cannot name a
  tool the turn does not have.
- The popup: `ToolConfirmRequest` carries the resolved inputs and the sandbox's network
  state, rendered by the screen in the interface language; `ToolContext.python_net` is
  where that flag reaches it, beside `mcp_images`.
- `/file attach`: the blocking read now reads the bytes **once** and ends in one of three
  outcomes — plain text (attachment as before), an extractor's text (attachment plus the
  original, linked by the additive `Attachment.file_id`), or bytes that decode as nothing
  (kept, no attachment). Removing a pair deletes our copy first and both listings second;
  re-attaching stores the new original, swaps the listing and drops the old copy last, so
  no state exists in which neither is there.

**Live — GO** (Gemma 4 31B q4_0 with its projector, llama.cpp b10807, one slot; the dev
sandbox's packed image), both smokes written so the answer cannot arrive through another
channel — §10's lesson from this same track. `sandbox_inputs_e2e_live`: turn 1 built an
Excel workbook with openpyxl from numbers it was told not to print and saved it to
`/w/out` (4.9 KB, stored as `sales.xlsx`); turn 2 named that workbook in `files`, read the
copy in `/w/in` with pandas and printed **4706** — Σ(i²·7+13) over twelve months — and the
reply carried that number. A workbook rather than a CSV on purpose: its bytes have to
survive storing and staging unchanged or openpyxl cannot open them.
`attached_binary_reaches_the_sandbox_live`: a generated 512×512 PNG attached from disk was
**kept** rather than refused, and a call that named it opened the copy with pillow and
printed `(512, 512)`, which the reply repeated. The nineteen `python::tests` sandbox smokes
are green after the contract change.

**Tests.** Unit: 3155 green, 167 ignored (3134 / 165 before). New: the item list and its
staged names (three kinds, a pair as one item, an extractor's `.txt`, an image's prepared
extension, a shared name versioned once, the fallback name); staging of each kind through
`MockSandbox`, which now records what reached `/w/in`; the four refusals with nothing run;
a handle named twice; `files` in the Wasmer schema only; the block's handles and staged
names, and its absence on a turn that stages nothing; `/file list` across the three kinds
with a pair as one item; an image handle refused by `/file remove` with the way out; a pair
removed in both halves; a binary attach that stores and makes no attachment; a document
attach that links the two; the popup's line with a resolved file, an unknown handle and the
network state.

**Two traps, both already in lessons.** A `cargo clippy … | tail` in a `&&` chain reported
`tail`'s status and let a commit land with the lint red (lessons §1, now three times); and
a Rust raw string `r#"…"#` cannot hold the handle `"#9"`, which closes it early.

### Post-M9: sandbox file exchange — stage 4, opening a file in the system (done)

Stage 4 of [sandbox-file-exchange.md](../history/sandbox-file-exchange.md) (fork F9, sub-decisions
§13): the files a chat holds became reachable outside the app. `/file open <name|#N>` hands
one to the desktop's handler and `/file folder` opens the chat's files folder — the last
half of D1, which asked for the outputs to be openable without hunting for the directory.

**One resolver, no second numbering.** The handle is the list of §12 T2 — attachments, the
stored files no attachment links, the chat's images — resolved through the same
`chat_inputs::resolve` that `/file remove` and the tool's `files` take, and the two refusals
they share (nothing of that name; a name several items carry, listed with each candidate's
`#N` and source) moved into one `resolve_file_handle` rather than being written twice. What
a handle *means on disk* is a pure function beside the list (`open_path`): the chat's own
copy for a stored file and for an attached document that kept its original — ours is the
half guaranteed to be there, the user's path may have moved since — and the item's `source`
for everything else. The caller then checks that path once, and a source that is not a file
opens nothing and is named: a pasted image's is `clipboard:<uuid>`, a fetched page's
attachment carries a URL, and a listed copy can be gone from the folder. Nothing is written
to make an open work — a command read as "show me this" must not put a new file on the
user's disk.

**The allowlist is the point of the module.** The folder it opens from is written by
`python_exec`, so a name in it is a name the *model* chose: handing it to the shell is
handing it to a default handler, and a `run.bat`, a `.lnk` or a scripted `.html`/`.svg`
would **run**. `os_open::is_document` therefore names what may be launched — png/jpg/jpeg/
gif/bmp/webp/tif/tiff, pdf, csv/tsv, txt, md, json, xlsx, docx — the last extension
deciding, so `report.pdf.bat` is a batch file; `decide` sends everything else to the folder
it sits in, which runs nothing and is still one double-click from the file, with the reason
in the note. Macro-enabled office shapes (`docm`, `xlsm`) are out for the same reason, and
`svg`/`html` are out although a call writes both.

**The launch is three calls of ours** (`shared/os_open.rs`, ~60 lines): `ShellExecuteW`
with the `open` verb on Windows (windows-sys gains `Win32_UI_Shell`; a return at or below
32 is an error code, and `SE_ERR_NOASSOC` becomes "no application is associated"),
`xdg-open` on Linux, `open` on macOS — one argument, no shell. Not `cmd /c start`, which
re-parses its argument outside Rust's escaping (lessons §6), and not a crate for three
calls. It runs on the blocking pool: the shell returns only once the handler has started.
The path is printed on success *and* on failure, so a desktop with no handler leaves the
user one copy-paste from the file rather than one error message from nothing.

**Testability was designed in, because the last step opens a window.** The orchestrator's
decision is a `plan_open` that returns the path and the note it will leave — every refusal
reported, nothing launched — so the wiring is tested without anything appearing on the
machine running the tests; and the unix launch takes its launcher as an argument, so a stub
script on Linux records what it was given. That is the half CI covers: the argument arrives
whole (`my chart (1).png`, spaces and brackets included) and a launcher that is not
installed comes back as an error.

**Gate — manual, no model run** (§8): on Windows 11, `MINDFORK_OPEN_LIVE=1 cargo test --
--ignored opens_a_document_and_a_folder_live` opened a viewer on `mindfork open gate.txt`
(a name with spaces, passed whole) and a file manager on the folder standing in for
`run.bat` — both `ShellExecuteW` calls above 32. The Linux half runs in CI through the stub
launcher; a GUI launch on a Linux desktop is the one thing not verified here, no Linux
desktop being available on the development machine.

**Tests.** Unit: 3165 green, 168 ignored (3155 / 167 before) — plus one more on Linux,
the stub-launcher test being `cfg(unix)`. New: the platform→launcher
mapping; the allowlist, including what it must refuse; `decide` falling back to the folder;
`open_path` across the four kinds; the plan for a document and for a script a call wrote;
the three shapes of "not a file on this machine"; a shared name refused with both
candidates; `/file folder` on a chat that saved nothing, which must also not create the
directory it reports on; the two feed notes; the unix launch's single whole argument and its
missing-launcher error.

### Post-M9: sandbox file exchange — stage 5, the local interpreter joins the contract (done)

The last stage of [sandbox-file-exchange.md](../history/sandbox-file-exchange.md) (fork
F11 (b), sub-decisions §14), and the one that ends a divergence stage 3 had to introduce:
`python_exec` in **Local** mode ran the code and nothing else — no files in, none out, and
a schema without `files`, because a model must never be offered an argument its mode cannot
honour. Local now runs the same exchange, and ADR 0005 §3's "the schema is mode-independent"
holds again unqualified.

**One runner interface, not a second code path.** `LocalSandbox` answers the same
`SandboxRunner` contract as `WasmerSandbox`: a job directory per call holding `job.py`
beside `in/` and `out/`, the interpreter started with that directory as its **working
directory**, the same `collect_outputs` under the same `OutputLimits::DEFAULT`, the same
rule that any exit code collects while a timeout collects nothing. One `prepare_job` lays
the directory out for both — the guest's shims are added by the Wasmer side and nothing on
the host wants them — and the registry picks the runner from the mode. `python_exec` lost
its `run_local` entirely: the tool no longer branches by mode at all, and what the mode
still decides is the wording and which of the two timeouts a launch gets.

**The folders are one source, two spellings.** F11 asked for relative `in/`/`out/` to work
in both modes, and the working directory gives exactly that; what the model *reads* keeps
each mode's own form through `PythonMode::dirs` — `/w/in`, `/w/out` in the guest, `in`,
`out` on the host — substituted into the same locale strings by the description, the
pinned block and the `files` argument's own description. The Wasmer rendering is byte for
byte the text stages 2 and 3 measured, which a test asserts by reconstructing it: spending
a live GO to make two strings look alike would have bought nothing.

**Three smaller decisions.** The code is written to a file rather than passed with `-c`, so
a long script cannot overflow a command line (Windows caps it) and a traceback names a
file. Local's availability is a **path check**, not a probe: an interpreter named with a
separator is checked as a file, a bare name is left to `PATH` — a `--version` run per turn
would answer a question the call answers anyway. And the network: Local has no switch to
honour, so `ToolParams` reports `python_net = true` there whatever the setting says,
because the confirmation popup states that value and the code really does reach the network.

**Live — GO (2026-09-12)**, Gemma 4 31B q4_0 on llama.cpp b10807, one slot of 16384.
`local_mode_files_round_trip_e2e_live` needs no sandbox at all, which is the point: a
twelve-row CSV attached to the chat, the model asked to name it in `files`, read it from
the input folder with the standard library, print the sum and write it into the output
folder. It printed **4706** — Σ(i²·7+13) over twelve months, a number only the file
carries — the reply repeated it, and `summary.txt` was stored with the chat and listed by
`/file list`, in 10.6 s. `runs_real_python_local` does the same round trip without a model.
The refactor's other half was re-measured rather than assumed: the nineteen `python::tests`
sandbox smokes, `sandbox_inputs_e2e_live` (24.6 s) and `attached_binary_reaches_the_sandbox_live`
(7.6 s) are green on the same stack after the job-preparation extraction.

**Tests.** Unit: 3171 green, 169 ignored (3165 / 168 before). New: the tool's path in Local
mode over a mock runner (a named file staged, an unknown handle refused with nothing run),
the job directory both runners lay out (the script, the copies under their own names, an
`out/` that exists before the run, the source untouched), a staged name that is not one
component refused, the interpreter path checked without a probe, the pinned block rendered
in the mode's own folders, both modes offering `files` with each naming its own folder, and
the network Local reports. The track's plan moves to `docs/history/` with this entry.

### Post-M9: a withheld server image is stated, and stated in words that work (done)

Found by a review pass over the merged file-exchange track — §11 S8's own rule, applied
back to the neighbour it had named. With `tools.mcp_images` **off**, the adapter dropped a
server's images and said **nothing**: the parser's `[image content omitted]`
(`shared/mcp.rs`) marks only a *malformed* block and the "N more image(s)" line only the
over-cap ones, so a well-formed image under the cap left no trace at all. The model then
read a complete-looking result and described a screenshot it had never received — exactly
the failure §10 measured, where a plot colour only the image carried was named 4/5 and 5/5
with it and 0/5 blind, and blind, **both** families described a chart they had not seen.

Two things had been asserting the opposite, which is how it survived: the comment on the
switch claimed "the placeholder the text already carries still says an image existed", and
`ui.settings.desc.mcp_images` promised the user that very note in this case. And **a test
pinned the defect** — `the_switch_decides_whether_a_server_image_reaches_the_model`
asserted `out.result == "Screenshot taken."` in *both* directions, so the gate was green
over it.

**The fix** is one statement in the adapter, where the switch is consulted and `ctx.loc` is
in hand (`tool.mcp.images_off`), appended **after** `clip_result`: the one line that says
what is missing must not be the line the truncation eats.

**The wording is the measured part, and it is not the family's.** The first attempt copied
the shape `loop.images_no_vision` / `loop.images_dropped` / `tool.python_exec.files.
not_shown_off` all use — name the withholding, end with "You have not seen them." Measured
on **Gemma 4 31B q4_0** (llama.cpp b10807, 2026-09-12, five runs an arm, the fixture's own
2048-token budget, no image ever sent):

| the tool result carries | described a screenshot it never got |
|---|---|
| nothing (control) | **5/5** |
| "You have not seen them." (the family's shape) | **5/5** |
| …" — so say so rather than describing what they might show" | 1/5 |
| …" — do not describe what they show; say that you cannot see them" | **0/5** |

So the descriptive form buys **nothing**: it is indistinguishable from silence on this
model, which had not been measured when the family was written. What ships is the
directive form, and `a_withheld_tool_image_is_not_described_live` is the arm that says so —
two-sided (no colour claimed *and* the model says outright it cannot see), because "does
not say green" passes on a wrong guess, which is precisely what the control produces.

**A false GO on the way, worth recording.** The first run of that smoke passed while
proving nothing: the criterion was the neighbouring `assert_sees_green_circle(.., false)`,
and the answer — *"The background colour is blue, and there is a white circle"* — satisfied
it by hallucinating the wrong colour. The control arm produced a byte-identical answer. A
second trap sat behind it: probing at `max_tokens: 256` returned empty content for the
directive arms and read as a clean 0/3, when the model had simply run out of budget inside
its thoughts (`finish=length`). Both are the same lesson §9 already carries — a vision
criterion must ask for what neither the code nor the output names — and neither was caught
by a gate.

**Scope, deliberately.** The three sibling strings carry the same descriptive shape and are
very likely just as inert, but they are a different defect (they *do* say something) and a
separate measurement; this PR does not touch them. Flagged for a follow-up.

**Tests.** The off arm of the switch test now asserts the server's own text survives *and*
the statement is there; a second test drives `max_result_chars` low enough to truncate and
asserts the statement still ends the result. Both fail with the guard reverted — checked,
not assumed. Unit: 3172 green, 170 ignored.

### Post-M9: the rest of the withheld-image family gets the clause that works (done)

The follow-up the entry above flagged. Having measured that a *descriptive* note is
indistinguishable from silence, the obvious next question was whether the five siblings —
`loop.images_no_vision`, `loop.images_dropped`, `tool.python_exec.files.not_shown_off` /
`_cap` / `.svg`, plus the parser's hardcoded over-cap line — are inert for the same
reason. Measured on **Gemma 4 31B q4_0** (b10807, 2026-09-12, five runs an arm, nothing
ever sent as an image), the answer is "two of them are, and the rest fail differently":

| the result carries | invented an answer |
|---|---|
| nothing (control, bracketed shape) | 5/5 |
| `loop.images_no_vision`, as shipped | 3/5 |
| `loop.images_no_vision` + the clause | **0/5** |
| `loop.images_dropped`, as shipped | 5/5 |
| `loop.images_dropped` + the clause | 1/5 |
| nothing (control, `python_exec` shape) | 4/5 |
| `files.not_shown_off`, as shipped | 1/9 — and **7/9 called the tool again** |
| `files.not_shown_off` + the clause | **0/5**, a plain refusal 4/5 |
| `files.svg`, as shipped | 1/5 — 4/5 called the tool again |
| `files.svg` + the clause | **0/5**, still 5/5 calling the tool again |

**The family is not one shape, and the verdict is not one verdict.** `loop.images_*` are a
bracketed line of their own and behave like the MCP string did — the descriptive form buys
little or nothing. The `python_exec` notes are a *suffix on one file's line* inside the
`files:` section, and there the shipped form was **not** inert: the model mostly went back
and called the tool again. That is not a hallucination, and it was nearly recorded as one —
the first classifier read `finish_reason: tool_calls` (empty content) as a truncation, and
before that a 2048-token budget produced real truncations that read as clean refusals. Both
outcomes look like "the model said nothing"; neither is. A re-call is still worth removing
here, because the second call's image is withheld for the same reason as the first, and the
user's round budget pays for it.

**`.svg` is the exception that keeps its shape.** There a re-call is the *right* move — the
note's whole job is to steer to "save a PNG" — so it gains "you have not seen it, so do not
describe it" while keeping the steer, and the measurement confirms both halves: the
invention goes to 0/5 and the re-call stays 5/5.

**A gate, because nothing else holds this.** The wording change broke no test — the strings
were pinned by nothing — so `every_withheld_image_string_tells_the_model_not_to_describe_it`
now asserts every member of the family, in both bundles, carries the clause — one needle per
language, since it is a sentence and not a token. Mutation-checked: reverting one string to its previous
text fails it by name. The parser's over-cap line stays hardcoded English, since the
protocol layer has no `Locale` and its sibling markers are in the same position.

**Live — GO (2026-09-12)** on the same stack: `a_withheld_chart_is_not_described_live` is
the `python_exec` shape end to end (the model returned `tool_calls`, inventing nothing), and
`a_withheld_tool_image_is_not_described_live` re-run beside it (control described a
screenshot it never got; with the note, "I cannot see the screenshot"). Unit: 3173 green,
170 ignored.

### Post-M9: a handle means one file for the whole turn (done)

The second finding of the review pass over the merged file-exchange track, and the one three
reviewers reached independently. §12 T2 promised that a pure `chat_inputs` **builds the list
once** and its consumers read it. The code built it **three** times: at turn start for the
pinned block, live in the confirmation popup, and live again in `PythonExec::stage`. The last
two agreed with each other and both could disagree with the first.

That matters because the list grows *during* a turn. `sync_attachments` and `sync_files`
mirror each round's effects into the tool context, and `items()` numbers positionally —
attachments, then stored files, then images — so one `fetch_url` landing a page pushes every
stored file and every image down by one, and one `python_exec` output pushes every image
down by one. The model, still reading a block written before it had produced a line of code,
names `#2` and is handed whatever slid into that slot. **No refusal**: `#N` was a plain index,
and an index cannot notice that it has changed meaning. `sync_attachments` also does
`retain(|x| x.source != a.source)` then `push`, so re-fetching one source **reorders** the
attachments without changing the length — even `#1` can move.

The same shift renames what lands in `/w/in`: `number()` versions staged names in list order,
so the `notes (2).md` the block promised becomes `notes (3).md` — and T3 exists precisely
because the model writes `/w/in/<name>` into its code *before* any result exists.

**Decision (user's, 2026-09-12): freeze the numbering for the turn**, over refusing on drift
and over rebuilding the block each round — the latter rewrites `request.system` mid-turn and
throws away the conversation's prefill, which on this stack was measured at ~50 tok/s cold.

**Carried, not frozen.** The list is re-derived every round and only the two fields the model
was *told* are carried over: the `#N` and the staged name. `ChatInput` gained the item's own
id to match on, and `reconcile` restores those two from the previous list, numbering only
what is new, after everything already promised. Freezing the whole item would have gone stale
the moment `sync_attachments` reordered, because `attachment` and `image` are positions into
the live context. A number is never reused — it counts on from the highest the turn issued —
so a handle for an item that has left misses rather than landing on a newcomer, and `resolve`
now matches `#N` against the handle instead of indexing with it. The turn's list lives on
`ToolContext.inputs`, refreshed by `ToolContext::sync_inputs` after the two mirrors; the tool,
the popup and the sub-agent's own block all read it, so the three derivations are one again.

**Tests.** Four pure ones over `reconcile` (a file landing mid-turn moves nothing; the staged
name survives a newcomer sorting ahead of it; a reordered attachment keeps its number; a
number is never reused), and one through the tool over the real context. All five fail with
the carry-over disabled — checked, not assumed. Two existing tests were repaired rather than
adapted: one had been replacing the context's snapshots without renumbering, and
`a_listed_file_whose_copy_is_gone_runs_nothing` turned out to assert only that *some* refusal
mentioned the name, which the "unknown handle" refusal satisfies too — it now asserts the
exact one. Unit: 3178 green, 172 ignored.

**Live — GO (2026-09-12)** on Gemma 4 31B q4_0, llama.cpp b10807, sandbox provisioned.
`a_handle_survives_a_round_that_adds_a_file_live`: a chat holding one image, one turn, two
rounds — the first call saves a file, the second names `#1` and prints the first bytes of
what arrived in `/w/in`. It printed the PNG signature. The three existing sandbox smokes were
re-measured after the refactor rather than assumed (`sandbox_inputs_e2e_live` summing to 4706
again, `sandbox_outputs_e2e_live`, `attached_binary_reaches_the_sandbox_live`).

**Two dead ends in that smoke, both worth the warning.** Its first version asserted over the
two calls' results **joined**, and failed on `ROUND-ONE` appearing in the *first* call's own
result — a saved text file is listed with the head of its content, so the test was failing on
the half that worked. Its second version passed under mutation: the prompt asked for two
calls without making the second depend on the first, so the model issued both **in one
round** — and the list is reconciled between rounds, so nothing it did could reach the defect.
The fix is to make call 2 need a value only call 1 can produce (a digest), which forces the
round boundary. Mutation-checked afterwards, the smoke fails exactly as a user would see it:
`#1` staged the wrong file, the code raised `FileNotFoundError`, the model retried `#1` twice
and then fell back to naming the file — three rounds spent on a number that had quietly
changed meaning. `python_turn` now reports each call's **arguments** alongside its result,
because a smoke about which handle was named cannot read that from the result text — which is
how the one-round version looked green.

### Post-M9: an image is installed only after it has started (done)

The third finding of the review pass over the merged file-exchange track, and the one that
cost the most if it ever fired. `pack_image` renamed the freshly built image over the
installed one and `verify_image` started it **afterwards**. So a build that did not run —
the August 2026 `python.webc` class of failure, an OOM during `import numpy`, a wasmer that
cannot compile what it was handed — left the broken image installed, destroyed the one it
replaced, and failed the command. And `WasmerSandbox::plan` asks only whether the file
**exists** (`image.is_file()`), so `availability()` went on answering `Ready`: the tool kept
advertising itself in every chat while every call failed at launch. `verify_image`'s own doc
comment described the opposite intent — "one that does not start … has to fail `setup`, not
the first call in a chat" — which is how it survived review. Packing is unconditional, so a
plain re-run to pick up a new wheel was enough to trigger it.

**Pack, start, install** — three steps because the order *is* the guarantee. `pack_image`
now builds only `packed-sandbox.webc.partial` and stops; `verify_image` starts **that file**
through a new `WasmerSandbox::for_candidate`, which runs a named image out of the sandbox
directory instead of the installed one; `install_image` renames it into place and is the
only thing that ever writes the file the runtime picks up. Until the candidate has run, the
image on disk is still the one that worked yesterday.

Two smaller things fixed on the way. A rejected candidate is **deleted** rather than left as
a few hundred megabytes nobody reads. And a verification that runs out of its 600 s is
reported as a timeout: a timeout carries no exit code (`SandboxOutput::default`) and no
stderr, so it took the `exit_code != Some(0)` branch and produced
`sandbox.setup.verify.failed` with an **empty** detail — the message was blank in precisely
the case whose cause is hardest to guess. `stdout` is the fallback detail when stderr is
empty for any other reason.

**Tests.** `for_candidate` starts the file it is named and does **not** fall through to the
installed image when the candidate is missing. The order itself is pinned by a `cfg(unix)`
test over a stub `wasmer` (the same device the file launcher uses): packing leaves a
sentinel installed image byte-identical and writes only the candidate, and installing is the
step that replaces it and consumes the candidate. The stub's shell was exercised in a real
shell before trusting CI with it. Unit: 3179 green on Windows and 3180 on Linux — the stub
test is `cfg(unix)`, so CI's ubuntu job is the one that runs it — 173 ignored.

**Live.** Two runs, both on this machine's provisioned sandbox. A junk candidate against the
**real** `wasmer` (`a_candidate_that_does_not_start_leaves_the_installed_image_alone_live`):
rejected with `Unable to determine how to execute …packed-sandbox.webc.partial` — naming the
candidate, which is itself the evidence that verification looks at the right file — the
sentinel image untouched, the junk cleaned up. And a real `mindfork sandbox setup` end to
end over the restructured order, followed by the sandbox's own smokes, because a fix to
provisioning that has not provisioned anything is a fix that has not been run.
### Post-M9: the same bytes put a missing copy back (done)

The fourth finding of the review pass, reached by two reviewers independently, and the one
where the app was giving advice it could not honour. `store_as` matched a listed file by
name **and** SHA-256 and returned `Stored::Unchanged` without ever looking at the disk. But
listed-but-missing is a state this code supports and reports — `StoredInfo.missing`,
rendered by `/file list` — and `tool.python_exec.err.files_missing` tells the model, in so
many words, to *ask the user to attach it again*. Doing that wrote nothing: the name and the
digest agreed, so the entry stayed missing and the next call refused identically. The only
way out was `/file remove` first, which nothing says anywhere.

Both halves were dead, for the same reason. A call re-rendering an identical chart got
"unchanged: already stored", and `/file attach` of the very same document returned the
listing's own id, so `handle_attach_result`'s `previous.filter(|old| stored.id != *old)`
skipped the drop too — nothing changed at all.

**The fix costs nothing**: `on_disk` is already read at the top of `store_as` for the
collision check, so the presence of the copy is a lookup in a list already in hand. Present
→ `Unchanged`, as before. Absent → write the bytes and return the new `Stored::Restored`,
which carries the **same listing**: the id, the name and the digest were right all along,
only the bytes were gone, so nothing is added to the chat and no effect is emitted. The
write goes through an extracted `write_new` shared with the ordinary path — `create_new`, so
a race keeps its file, `sync_all` before the listing is trusted, and a half-written file
removed rather than left. `AlreadyExists` on the restore path means someone wrote it between
the listing and now, and the digest already agreed, so it answers `Unchanged`.

The model is told: `tool.python_exec.files.restored` says the copy was gone and these bytes
put it back, rather than "unchanged … neither saved nor shown again", which would have been
a lie about a write that did happen. For `/file attach` the feedback is the `missing` marker
disappearing from `/file list`.

**Tests.** A pure one for each direction — the copy present is still a genuine no-op
(the old test asserted "nothing was written" against an **empty folder**, which is the
defect, and now seeds the copies it claims are there), and the copy missing is written back,
with a second offer then being the no-op it claims to be. And one through the orchestrator:
attach a binary, delete the copy, attach the same file again, and the bytes are back with
**one** listing rather than a second beside it. Both fail with the disk check removed.
Unit: 3180 green, 172 ignored.

**Live.** The file-exchange smokes re-measured on Gemma 4 31B q4_0 (b10807) with the
provisioned sandbox, because every `python_exec` output goes through `store` and this change
adds a branch to it — a spurious `Restored` would stop outputs being listed and shown at
all, which no unit test of the new path would notice.

### Post-M9: two letters are not a format, and an invisible mark is not a name (done)

Two findings of the review pass that share a seam: what a call **produced** and what a call
**named** it, both read from `entities/chat_file.rs`, and both able to mislead the person
reading the listing.

**`BM` and a length were enough to be an image.** `sniff_image` accepted any content of 26
bytes or more starting with those two letters — and BMP is the only one of the five
signatures that is ordinary text. A spreadsheet export whose first column is `BMI` was
therefore listed as `image/bmp`, and the damage ran through three places at once: the
listing lied, `is_text_like` was false so `keep_one` withheld the text head the model would
otherwise read, and the bytes were base64'd into the call's images and announced as
**shown** — after which `prepare_tool_images` failed to decode them and appended "1 image
dropped", contradicting the line above it. The existing test asserted only that `BM short`
is not an image, which is the length guard; nothing asked whether the content was one.

The fix reads the DIB header's own size, at offset 14, against the closed set BMP defines.
Only the first 18 bytes are touched, because adoption sniffs a 64-byte head
(`chat_files::unlisted`) and a check against the whole file's length is not available there.
The set is **measured, not assumed**: pillow — which a `savefig('.bmp')` in the sandbox goes
through — writes 40 for every mode it can save (1, L, P, RGB, RGBA), and the spec's other
values are carried for the encoders this project has not met. The old fixture turned out to
be a fake BMP (`BM` plus zeros), so it was replaced with a real opening rather than relaxed.

**A right-to-left override survived into the name.** `sanitize_name` replaced
`char::is_control`, which is category `Cc` only; `U+202E` is `Cf` and went through. A call
that writes `report\u{202E}cod.exe` gets a file every listing here — and the file manager
`/file folder` opens — renders as `reportexe.doc`. The launch allowlist does its job and
refuses to hand it to a handler (§13 U3), opening the folder instead; the user then reads a
document name and double-clicks an executable. The allowlist held and the name did not, and
the name is what the next decision is made from. Now the bidi controls and the zero-width
marks beside them (`U+061C`, `U+200B–200F`, `U+202A–202E`, `U+2066–2069`, `U+FEFF`) are
replaced like any other character Windows refuses. Ordinary non-ASCII is untouched — a test
pins a Cyrillic name with a diaeresis through unchanged.

**Tests.** The sniffing test gained the CSV that used to pass as an image and a `BM` prefix
with junk after it, and its BMP fixture became a real header; a new test covers the marks,
including the isolate/zero-width family and the negative case. Both fail with their guard
reverted — checked, not assumed. Unit: 3183 green, 173 ignored.

**Live.** `sniff_image` sits on the path of every stored file, so the stored-file smokes
were re-measured on Gemma 4 31B q4_0 (b10807) with the provisioned sandbox rather than
assumed: a stricter signature that rejected a real image would show up nowhere else.
### Post-M9: the `files` argument is read once, and the result stops listing (done)

Two more of tier 2, both about `python_exec`'s contract with the model — what it accepts,
and what it says back.

**One name where a list was asked for named nothing.** `args.get("files").and_then(as_array)`
answers `None` for `"files": "sales.csv"` — the shape models emit most often — and
`unwrap_or_default()` then turned that into *no files*: nothing staged, **nothing refused**,
the code run against an empty `in/`, and the model left to interpret a `FileNotFoundError`
with no hint that its argument was the problem. So it tried the same shape again. Every
other unusable `files` — an unknown handle, a shared name, a copy gone from the folder — is
a refusal before the run (§12 T7); this one was silence.

Worse, the parse existed **twice**: the tool's and the confirmation popup's, character for
character. Two readings of one argument is what §12 T6 exists to prevent, and they would
have drifted the first time either was touched. There is one now, `chat_inputs::named_files`,
returning `NamedFiles::Named | Malformed` so the caller can tell "names nothing" from
"cannot be read" — a distinction `unwrap_or_default` had collapsed.

The two are treated differently on purpose. A bare string is **taken**: the intent is not in
doubt and a round spent teaching JSON is a round the user pays for. A non-string *element*
is a **refusal**: dropping it would stage three of the four files a call asked for, which is
the exact failure T7 was written against.

**Nothing bounded what the result said about skipped outputs.** `OutputLimits` caps what a
call may *collect* — ten files, 25 MB each, 50 MB in all — and everything past a cap is
named in the result, one line per entry. A script that wrote twenty thousand files therefore
put twenty thousand lines into the tool message: into the conversation, into every later
turn's prompt and onto the bill, while its own stdout was clipped at 8000 characters two
fields away. The timeout path listed the whole directory the same way. The result now names
the first twenty and counts the rest.

**Tests.** The parser's own table (absent, null, empty, a list, a bare string, blanks
dropped, a number in the list, a number, an object), the tool taking a bare string end to
end, the tool refusing a malformed one with nothing run, and a call with five hundred
skipped entries reporting twenty lines and the number 480. Each fails with its own guard
reverted — checked, not assumed. Unit: 3186 green, 173 ignored.

**Live.** `sandbox_inputs_e2e_live` and `local_mode_files_round_trip_e2e_live` on Gemma 4
31B q4_0 (b10807): both are a real model choosing the `files` argument's shape for itself,
which is the half a table of JSON values cannot check.

### Post-M9: a process the script leaves behind stops deciding the call (done)

Tier 2's Local-mode finding, which turned out to be three things wearing one coat. Two are
fixed here; the third is named rather than half-fixed.

**The verdict was wrong.** `Child::wait_with_output` waits for the **pipes** to reach EOF,
not for the process to exit — and everything a script spawns inherits those pipes. So a
Local call whose script finished in a second was reported as having *exceeded its time
limit*, ten seconds later, and everything it had printed was thrown away with the verdict,
because a background process it left behind still held the write end. The call now waits on
the process (`child.wait()`) with the pipes read beside it, and the readers are given a
short grace before being stopped: a pipe a grandchild still holds must not hold the turn as
well. The buffers are capped at 1 MB per stream while they are at it — `MAX_OUTPUT_CHARS`
truncates at format time, which is after the bytes are already resident.

**The leftovers were the chat's files.** `JobDir::drop` cannot always run: a crash or a kill
never reaches it, and on Windows a directory that is some process's working directory cannot
be removed at all — which is exactly the state that same orphan leaves behind. What stayed
in `%TEMP%\mindfork-sbx-*` was not scratch: `in/` holds **copies of the chat's files**, put
there for the call. Leftovers older than a day are now swept once per process, by **age**
rather than by cause, because nothing here can tell a live call's directory from a dead one
and guessing wrong would delete a running call's inputs.

**The orphan itself is Local mode.** A process the script spawns outlives the call, with the
user's permissions. Containing that means process groups and Job Objects — and it would be
containing the wrong thing: ADR 0005 §3 has Local as the mode with *no* isolation, where the
code can already reach the whole machine by design and the Wasmer mode is what a user picks
when that matters. So it is documented, not partly prevented.

**Measured on the way, and worth knowing.** The blocking pipe read outlives the call however
this is arranged — tokio dispatches it to the blocking pool, and aborting the task does not
cancel a read already in flight. The application bounds that at exit with
`Runtime::shutdown_timeout(2 s)`; a test binary does not, which is why the new smoke's
sleeper is twenty seconds rather than the five minutes the first draft used, and why that
number is in a comment.

**Tests.** An `#[ignore]` smoke with a real interpreter — a script that leaves a sleeper
behind and prints — asserting the printed line survives, the call is not reported as timed
out, and it returns in under six seconds against the ten it used to take. Deliberately not a
mock: the defect is in how a real child's pipes behave, which is the one thing a mock cannot
have. Plus a pure test of the sweep in both directions: a fresh directory is left alone, a
stale one goes with the copies in it, and another program's temp directory is never touched.
Both fail with their guard reverted. Unit: 3188 green, 174 ignored.

### Post-M9: the "show charts" switch is shown in local Python mode too (done)

The last item of the review's tier 2, and the smallest. `tools.python_images` decides
whether an image the code saved goes back to the model; stage 5 (§14) gave `LocalSandbox`
the same `SandboxRunner` contract as the sandbox — one job directory with `in/` and `out/`,
one collector, one set of caps — so the flag has meant exactly the same thing in both modes
since. The settings row did not follow: it was pushed inside the `PythonMode::Wasmer` arm of
the catalog, beside the network toggle, with a comment saying neither means anything in
local mode. That was true when it was written.

What it cost is the ordinary shape of a hidden setting rather than anything exotic: the flag
is **on** by default, so most users saw the charts anyway — but someone who turned it off in
sandbox mode and then switched to the local interpreter kept the "off", with the row gone and
`config.json` the only way back, while every `python_exec` result said the model had not seen
an image it could have been shown. The row now sits above the mode split, beside the enable
toggle, with the switches that mean the same in both; it also stops appearing and vanishing
as the mode is cycled. The description needed nothing — it already named both folders
(`/w/out` in the sandbox, `out/` in local mode), which is the half that would have been
wrong to leave.

The visibility test already existed and already listed this row — in the **sandbox** arm. Its
local arm asserted only what is hidden there, which is why the defect survived a test whose
whole subject is which rows a mode shows. The lesson is narrower than "test the other
branch": a test that enumerates the presences of one arm and the absences of the other cannot
fail on a row that should be in both. Both arms now name it, and the local one carries why.

No live run: a settings row, no engine path touched.
### Post-M9: one name means one file — the fold, the empty cut, and the numbering (done)

Tier 3's first group. Three findings that look unrelated in a list and are one thing in the
code: a name has to reach exactly the item it names, and three different places could break
that.

**The fold was ASCII.** `ChatInput::matches` — what `/file open`, `/file remove` and the
tool's `files` argument all resolve through — compared with `eq_ignore_ascii_case`, while
`chat_file::same_name`, which the chat's folder uses to decide whether two stored names are
one file, compared with Unicode `to_lowercase`. So the folder treated a Cyrillic name typed
in lower case as the same file and the handle did not: the listing showed the name, the user
typed it back, and the answer was "no such file". `ru` is a fully supported locale; this is
not an exotic input. Two siblings had the same spelling (`Attachment::matches`,
`MessageImage::matches`) and so did `name_is_shared`, which is the worse half — it decides
whether a listing prints the source beside a name, so the listing could show two names as
distinct while the removal refused them as one, leaving the user nothing to act on. All four
now go through `same_name`, which is the one definition of the comparison.

**The emptiness check did not outlive the cut.** `sanitize_name` returned `None` for a name
that trimmed to nothing, then shortened a long one — and `shorten` takes the first 120
characters and trims trailing dots and spaces off *those*, so 121 spaces and a letter passed
the check and came back `Some("")`. Reachable from both callers (a model-chosen `/w/out`
name, a real file name). Downstream it was caught only by luck: `confined` refuses an empty
name, `unique_staged` would have taken it. The check moved after the cut.

**The image numbering was per call.** `prepare_tool_images` named with the index inside one
result, so every round's first image was `tool-image-1.png`. Two rounds each returning a
chart therefore put two items with one name in the chat, and `resolve` refuses a shared name
(`Resolved::Shared`) — the model naming its own chart in the next call's `files` got a
refusal, and the round was spent finding out. `ToolImageNames` now hands out the first free
number, seeded from the names the chat's messages already carry and carried through the turn
(`GenSpawn` → `TurnLoop`), so the count survives rounds rather than restarting. Two details
worth their lines: the number is taken whatever the extension, because `tool-image-1.png`
and `tool-image-1.jpg` are two names to `resolve` and one name one digit apart to a reader;
and naming now happens **after** the drop rather than before it, so a result whose first
image failed to decode no longer hands the second a number that contradicts the label the
model reads beside it — an inconsistency the old code had and nothing had noticed.

**On the tests.** All three defects were live in a suite of 3189 green tests, and the reason
is the same in each: the existing tests asserted the cases the author had in mind. The name
tests used ASCII names; the sanitizer tests used long names that were long *in letters*; the
chart test stored the same bytes twice, so the second round deduplicated and never produced
a second name at all. Each new test was mutation-checked against the guard it covers, and
the fifth (`name_is_shared`) needed a test of its own — the handle test did not reach it,
which the mutation run is what showed.

No live run: the three changes are in-process name transformations and reach no engine. The
chart's naming is covered through the real tool loop with a scripted engine
(`each_round_s_chart_gets_a_name_of_its_own`).
