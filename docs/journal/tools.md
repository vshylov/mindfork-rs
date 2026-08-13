# Journal — Tool system

The `Tool` contract and the tools built on it: files, web search and page fetch, the Python sandbox, MCP servers, speech, YouTube, the control tools, and the confirmation gate in front of the dangerous ones.

**Reference documents for this area:** architecture.md §8, spec.md §9

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (23)

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
