# ADR 0005 — Python sandbox: `wasmer`/WASIX sidecar behind `shared/sandbox.rs`

**Status:** accepted (2026-07-12). Fixes the architecture for isolated execution of
`python_exec` (Phases 0–3). Research and log —
[docs/research/python-wasmer-sandbox.md](../research/python-wasmer-sandbox.md).
Related to [ADR 0002](0002-embeddings-dedicated-server.md) (dedicated external
process behind a trait) and the security posture of [spec §13.2](../../spec.md).

## Context

`python_exec` originally launched the **system** Python as a separate process —
with no isolation (full access to the user's FS/network/processes), so it was
disabled by default and required Python to be installed. The task: give the tool
an **isolated** environment with preinstalled packages (numpy, requests, …) for
moderately complex tasks, while **keeping the isolated code out of the main
binary** (a separate artifact alongside it).

## Decision

### 1. Sandbox engine — Wasmer/WASIX, the only viable path

Under the combined requirement "numpy + requests + isolation + Windows/Linux +
don't pull an ML/JS stack into the app," the alternatives fell away (Phase 0,
research §2.2): official CPython-WASI (wasmtime) — no sockets/threads/dynamic
linking → neither requests nor numpy; Pyodide — browser/Node only;
RustPython/MicroPython — no CPython C API. **Wasmer 7 (Jan 2026) + WASIX**
delivers dynamic linking (native numpy `.so`), sockets (requests), CPython 3.13.
Confirmed live (numpy 2.3.2, requests HTTPS 200, process-kill interruption).

### 2. `wasmer` sidecar process, not runtime embed in a dll

The request assumed a separate dll with an embedded runtime. Phase 0 revealed
that on **Windows only the V8 backend works** (cranelift/singlepass fail on the
exception ABI), and **embedding it pulls LLVM/libclang + static V8 into our
build**. So instead of embedding — a **bundled `wasmer` binary as a sidecar
process** behind the `shared/sandbox.rs` contract:

- the main exe has **zero Wasmer** (the spirit of "a separate artifact
  alongside it" is preserved — that role goes to `wasmer[.exe]` itself in
  `data/sandbox/`, not a dll);
- **interruption = process kill** (clean and fast, ~360 ms; python runs
  in-process inside wasmer/V8, there is no separate child process);
- **failure isolation**: runtime panics/OOM live in the sidecar, they don't
  bring down the TUI;
- runtime updates = replacing the binary; the app build doesn't grow.

### 3. `shared/sandbox.rs` contract behind a trait (like `EngineBackend`)

`SandboxRunner` (`availability`/`run`) with a real `WasmerSandbox` and
`MockSandbox` in tests. Clean, testable `build_wrapper`/`build_args`; a single
binary resolver `locate_wasmer` (shared by both the runtime and provisioning).
`python_exec` (`features/tools/python.rs`) — a thin dispatcher by mode; **the
tool id and its schema (`{ code }`) are mode-independent** — the model sees one
tool, the implementation can change without changing the protocol.

**Amended (2026-09-12, sandbox file exchange stage 3):** the schema gains an
optional `files` — the chat's files a call copies into `/w/in`
([docs/history/sandbox-file-exchange.md](../history/sandbox-file-exchange.md) §12 T10) — and it
is offered **in the Wasmer mode only**. Local runs no job directory until that
track's stage 5 brings parity, so until then the schema does depend on the mode.
That is the lesser of the two breaks: a model offered an argument its mode cannot
honour would name files that never arrive, and find out only from the refusal.
The id, and the meaning of `code`, are unchanged.

**Amended again (2026-09-12, the same track's stage 5):** that divergence is
over, and the sentence above holds unqualified — `{ code, files }` in both modes.
`LocalSandbox` answers the same `SandboxRunner` contract as `WasmerSandbox`: one
job directory per call with the script beside `in/` and `out/`, the interpreter
started with that directory as its working directory, one `prepare_job` laying
both out and one `collect_outputs` reading them back. `python_exec` no longer
branches by mode at all — the mode picks which runner the registry builds and
which folders the prompt names (`PythonMode::dirs`: `/w/in`/`/w/out` in the
guest, `in`/`out` on the host, where the working directory makes the relative
form mean the same thing). A second staging path written beside the first is
exactly what this section exists to prevent.

### 4. Provisioning — `mindfork sandbox setup` from a lock list (auto-download)

A separate clap subcommand (like `backup`) downloads into `data/sandbox/`: the
`wasmer` binary (platform tar.gz from GitHub), `python.webc` (via `wasmer
package download` itself), wheels (numpy from the wasix index, the requests
stack from PyPI) — from a **lock list with exact URLs + sha256** checked into
the repo (resistant to "latest"). Assets are **not** checked into the repo
(like the Hunspell dictionaries). Provisioning is idempotent; the compilation
cache is warmed on setup (`warmup`), so the first real call is warm.

### 5. Security posture

- **FS**: the guest sees only the mounted tmp directory with the script + the
  preinstalled packages; **no host directories** by default.
  **Amended (2026-09-11):** the packages were never read-only. `site-packages`
  was mounted with a plain `--volume`, `wasmer` 7.2.0 has no read-only volume,
  and a `sitecustomize.py` one call wrote there ran inside the next — in every
  chat. `sandbox setup` now packs CPython and `site-packages` into one
  self-contained image (`packed-sandbox.webc`: the unpacked `python.webc` with
  `site-packages` added as a volume), and what the guest writes to a package
  volume lands in memory and dies with the call. One package rather than a
  second one depending on `python/python`, because resolving a dependency
  queries the registry even with `--include-webc`, and on a fresh cache offline
  that run cannot start. A `site-packages` directory with no image — an install
  from before — is refused, never mounted, with the command that packs it; the
  one writable mount left is provisioning's own warmup, which fills the bytecode
  the image is packed with.
  **Amended (2026-09-11, sandbox file exchange):** "no host directories" now reads
  "one host directory per call". The job directory, created for the call and dropped
  with it, holds the script and an empty `out/`, whose regular files are collected
  after the process exits — never through a link, within per-call caps — and stored
  with the chat by the tool ([docs/history/sandbox-file-exchange.md](../history/sandbox-file-exchange.md)).
  **Amended again (2026-09-12, stage 3):** the same job directory now also holds
  `in/`, where the chat's files a call names are **copied** — never linked —
  before the run. They are the guest's own copies: `wasmer` 7.2.0 has no
  read-only volume, so one can be overwritten inside the guest and nothing
  follows from that — the chat's store is untouched, and only `out/` is
  collected. A name is one plain component, re-checked here whatever produced
  it, so a bug upstream cannot write outside the directory.
- **Network**: `--net` is not passed until the user enables
  `python_net_enabled` (a toggle, **on** by default — for requests; but
  enabling the tool itself is a separate opt-in). Without the flag there are
  physically no sockets.
- **CPU/hangs**: timeout → kill; **"one task at a time" gate** (defense in
  depth against process leaks; in the normal agentic loop calls are already
  sequential).
- **RAM**: a hard cap — **optional, Windows only** (Job Object
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY`; `tools.python_wasm_memory_mb`, off by
  default). The `wasmer` process is placed into a job right after spawning;
  exceeding the cap **kills the process** (protects the host from OOM) — the
  failure is not graceful (V8 dies with "Fatal out of memory", the text lands
  in the result's stderr), but the host is protected. Minimum ~1024 MB
  (V8+CPython needs ~768 MB to start; below that the sandbox won't start). The
  `wasmer` CLI has no memory flag, so the cap is set via winapi (`windows-sys`,
  target-dep). **Not applied on Unix**: `rlimit`/`RLIMIT_AS` is unreliable with
  V8 — it reserves a large virtual address space, and a low limit breaks the
  start itself (cgroups need root/systemd — out of scope). There the posture
  is timeout + wasm32 (~4 GB). The job handle is closed right after
  `AssignProcessToJobObject` (the limit holds as long as the process is a
  member of the job), so the raw HANDLE isn't held across an `await` (the
  future stays `Send`). Confirmed live (research §9.6).
- **`tools.python_enabled` stays `false` by default.** The sandbox removes the
  original reason (no OS sandbox), but "enabled but assets not installed" is
  worse than "disabled"; enabling is a deliberate step after `sandbox setup`.
  Revisit as it gets more field use.
  **Amended (2026-08-07):** the default is unchanged, and so is the invariant —
  what changed is *where* the deliberate step can be taken.
  `sandbox setup --enable-python` turns the setting on **after** a successful
  provisioning (never on failure — the flag is applied past the `?`), and the
  Windows installer's opt-in checkbox passes it, so ticking a box that names both
  effects is the deliberate act. The flip is done by the **app**, not the
  installer: `settings.json` is user data in a data root whose location only
  `Paths::resolve` knows, and it carries the same two precautions the TUI takes —
  the ADR 0006 downgrade guard before writing, and seeding `interface.language`
  from `defaults.json` when the file is created here (that seeding is gated on the
  file's absence, so creating one without it would lose the installer's choice).

### 6. Compatibility shim

The code wrapper carries a `setsockopt` shim: WASIX doesn't implement
`TCP_NODELAY` (`EINVAL`), and `http.client`/requests always set it — without
the shim, network access in the sandbox wouldn't work (a Phase 0 finding).
Wrapped in a function so it doesn't litter the user's namespace.

## Consequences

- Layers above `shared/sandbox.rs` (orchestrator, agentic loop, UI) — not
  affected.
- New dependencies — provisioning only: `flate2`/`tar` (unpacking), `sha2`
  (verification) — all pure Rust, no C.
- Cross-platform; on a platform not covered by auto-download — a clear
  instruction + `MINDFORK_SANDBOX_WASMER`.
- Testability: the clean core (args/wrapper/lock list/unpacking) — unit tests;
  real runs — `#[ignore]` smokes (numpy/requests/Cyrillic/timeout).

## Alternatives (rejected)

- **Embedding V8 in a cdylib** — LLVM/libclang + static V8 in our build;
  sidecar process-kill is more reliable than in-process terminate (Phase 0,
  §9.7).
- **Official CPython-WASI / Pyodide / RustPython** — no numpy+requests+isolation
  together (research §2.2).
- **Containers (docker/WSL)** — a heavy external dependency, against the
  spirit of a portable TUI.
- **Metering/fuel for interruption** — Wasmer has none; process-kill is
  simpler and proven clean.
