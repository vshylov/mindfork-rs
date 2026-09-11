# Research: Python sandbox on Wasmer (WASIX) + extraction into a separate library

**Status:** research (2026-07-11) + **Phase 0 (spike) passed on Windows —
verdict GO** (see §9). **Delivery decision (2026-07-11): option B — bundle a
ready-made `wasmer` CLI as a sidecar** behind the `shared/sandbox.rs` contract
(embedding V8 in our own dll was rejected because of LLVM/libclang + static V8
in our build, §9.7); the full embed spike was shelved as no longer relevant.
Requirement: `python_exec` should have two modes — **local interpreter** (as
now) and an **isolated environment** on Wasmer with preinstalled packages
(numpy, requests, etc.) for relatively complex tasks; **Wasmer mode is the
default**. The sandbox environment's code must not live in the main
executable — a separate **artifact next to the binary** (per the decision,
`wasmer.exe`/`wasmer` itself, not a dll; the spirit of the requirement — "zero
Wasmer in the main exe, a separate file alongside it" — is preserved).

Related documents: [spec §9.3, §13.2](../../spec.md) (the current `python_exec`
and its security posture), [ADR 0004](../decisions/0004-engine-contract-multi-provider.md)
(the "new implementation behind an existing trait/contract" pattern),
[docs/install.md](../install.md).

---

## 1. Task and current state

Today `python_exec` ([features/tools/python.rs](../../src/features/tools/python.rs))
is a plain launch of the **system** Python as a separate process (`python -c <code>`,
stdin closed, stdout/stderr piped, 10 s timeout via `kill_on_drop`, UTF-8 via
`PYTHONIOENCODING`/`PYTHONUTF8`). There is no isolation whatsoever: the code
gets full access to the filesystem/network/processes as the user. That's
exactly why the global switch `tools.python_enabled` is **off by default**
(spec §13.2 — "no OS sandbox on Windows"), and the tool itself requires Python
to be installed on the machine.

What we want:

1. **Wasmer mode (default)** — Python inside a WebAssembly sandbox: no host
   filesystem access, network by explicit permission, preinstalled packages
   (numpy, pandas, requests, …), no Python required on the machine.
2. **Local mode** — the previous behavior (system interpreter, `python_path`).
3. **Sandbox in a separate dll/so** next to the binary: the heavy wasm runtime
   doesn't bloat the main exe and loads only when needed.

---

## 2. The "Python in WASM" landscape (checked against the web, July 2026)

### 2.1 Wasmer + WASIX — the only path that gives both numpy and networking

**WASIX** is Wasmer's extension of WASI preview1 (threads, full sockets, fork,
setjmp/longjmp, and as of 7.0 dynamic linking). Wasmer is the only runtime
that supports WASIX ([wasix.org](https://wasix.org/docs/)).

State as of July 2026:

- **Wasmer 7.0 (2026-01-30)** — a pivotal release: **dynamic linking
  (dlopen/dlsym) in WASIX** + libffi (ctypes). Before that only the pure
  interpreter worked; now native C extensions work too — numpy, pydantic,
  etc. Plus experimental **async API** (full asyncio; SQLAlchemy, greenlet),
  singlepass/cranelift/LLVM backends, faster LLVM compilation (python.wasm
  ~90 s → ~10 s). ([Wasmer 7 blog](https://wasmer.io/posts/wasmer-7),
  [heise](https://www.heise.de/en/news/WebAssembly-Wasmer-7-0-brings-experimental-async-support-for-Python-11163325.html),
  [InfoWorld](https://www.infoworld.com/article/4125985/wasmer-beefs-up-python-support.html))
- **CPython 3.13** (fork [wasix-org/cpython](https://github.com/wasix-org/cpython));
  3.12 — legacy "pure-python only" build. 3.14 on the roadmap. Distributed as
  the **`python/python`** package in the Wasmer registry (webc format):
  `wasmer run python/python --dir=. -- script.py`.
  ([Python on the Edge](https://wasmer.io/posts/python-on-the-edge-powered-by-webassembly))
- **Native WASIX wheel index** — <https://pythonindex.wasix.org/> (pip-compatible
  `/simple`): ~64 packages, including **numpy 2.4.0.dev0, pandas 2.3.2,
  cryptography 45.0.4, pillow 11.3.0, matplotlib 3.10.6, aiohttp 3.13.2**. The
  build environment includes OpenSSL 3.5.1, zlib, libffi, sqlite,
  libjpeg/png/webp, etc. ([wasix-org/build-scripts](https://github.com/wasix-org/build-scripts);
  the repo was archived on 2026-04-14 in favor of its successor
  [wasinix](https://github.com/wasix-org/wasinix) — the build infra is alive
  but in motion). **scipy is not there yet**; polars/PyTorch/curl_cffi are
  announced as "coming soon".
- **requests** is missing from the index because it's **pure Python** (as
  are urllib3, certifi, idna, charset-normalizer) — installed as a regular
  wheel from PyPI. It needs sockets (present in WASIX) and `ssl` (OpenSSL is
  in the build; aiohttp with TLS being in the index is a good sign). Confirm
  with a live run (§6, Phase 0).
- **Networking is explicit opt-in at the runtime level.** WASIX sockets are
  not forwarded to the host by default: the CLI has a `--net` flag, and when
  embedding you provide your own `virtual-net` implementation (we enable host
  passthrough ourselves). In other words, the sandbox is **network-free by
  construction** until we grant it. ([Wasmer sandbox post](https://wasmer.io/posts/edgejs-safe-nodejs-using-wasm-sandbox))
- **Filesystem is fully virtual**: the guest sees only what we mount (the
  package's webc filesystem + explicit preopen/mapdir). The historical
  vulnerability in CLI ≤4.2.3
  ([GHSA-4mq4-7rw3-vm5j](https://github.com/wasmerio/wasmer/security/advisories/GHSA-4mq4-7rw3-vm5j) —
  cwd was mounted by default) does not apply to embedding with explicit
  preopens; our policy is **zero host directories by default** (§5.5).

### 2.2 Alternatives (considered and rejected)

| Option | Why not |
|---|---|
| **Official CPython WASI** (tier 2, wasmtime) | WASI p1: no sockets, threads, dlopen → neither requests nor numpy. C extensions are only experimental ([wasi-wheels](https://github.com/dicej/wasi-wheels) unmaintained; [numpy doesn't start](https://github.com/dicej/wasi-wheels/issues/4) on plain wasmtime; [numpy#25859](https://github.com/numpy/numpy/issues/25859) — a WASI build of numpy is not a priority) |
| **Pyodide** | The most mature package set, but it's emscripten: [works only in the browser/Node](https://github.com/pyodide/pyodide/issues/558), [won't run in wasmtime/wasmer](https://github.com/pyodide/pyodide/discussions/5145). Dragging a JS engine into a TUI app — no |
| **py2wasm** (Nuitka→wasm) | An *application* compiler to wasm, not an interpreter for arbitrary tool code |
| **RustPython / MicroPython (wasm)** | No CPython C API → no numpy/pandas; incomplete stdlib |
| **wasmtime as the runtime** | Best-in-industry interruption (epoch/fuel), but it doesn't execute WASIX — and without WASIX there are neither sockets nor ready-made native wheels |
| **Containers (docker/podman/WSL)** | A heavy external dependency and administration burden; against the spirit of a portable TUI "drop it next to the binary and it works" |

**Conclusion:** for the combination "numpy + requests + isolation +
Windows/Linux + embeddable in Rust", Wasmer/WASIX has no alternative today.

### 2.3 Embedding from Rust

- Crates: `wasmer` 7.x + **`wasmer-wasix` 0.702.0** (2026-06-30) + `webc` 12
  ([crates.io](https://crates.io/crates/wasmer-wasix), [lib.rs](https://lib.rs/crates/wasmer-wasix)).
  Key features: `sys-thread` (the WASIX thread pool — python.wasm is
  multithreaded), `cranelift`/`llvm`/`singlepass` backends, `host-vnet`
  (network passthrough; enabled by a toggle), `host-fs` (we do **not** need it
  — filesystem is virtual only).
- API: `WasiEnvBuilder` / a runner for webc packages (`BinaryPackage`), output
  capture via `Pipe::channel()` on `stdout`/`stderr`, mounting via virtual-fs
  (the package's webc-fs + our directories), networking via a `virtual-net`
  implementation. A Windows host is claimed (feature `windows-sys`; the CLI
  officially builds for Windows), but the "python + dynamic linking on a
  Windows host" combo needs a live-run confirmation.
- **Compilation cache:** the first run compiles python.wasm (tens of MB;
  cranelift — seconds, LLVM — ~10 s) → `Module::serialize`/`deserialize`
  (or `wasmer-cache`) into `data/sandbox/cache/` is mandatory — after that
  loading is almost instant.
- **Interruption is Wasmer's weak spot.** There is no analogue of wasmtime's
  epoch-interruption in Wasmer's core ([old request #337](https://github.com/wasmerio/wasmer/issues/337),
  [discussion](https://users.rust-lang.org/t/interrupt-wasmer-wasmtime/60331));
  standard paths: (a) `terminate` the WASIX process — the signal is delivered
  at syscall boundaries, a tight CPU loop (`while True: pass`) may not be
  interrupted; (b) metering middleware (`wasmer-middlewares`) — a
  deterministic instruction limit, but with overhead and unverified
  compatibility with dynamic linking. Detailed in §4.1.

---

## 3. Proposed architecture

> **UPDATE AFTER PHASE 0 (decision 2026-07-11):** we chose **option B — the
> `wasmer` CLI sidecar** (§9.7), not an embedded V8 in a cdylib. So §3.1 below
> (**cdylib with an embedded runtime**) is the **rejected alternative A**, kept
> for the record. The current delivery is **§3.1-B**. Sections §3.2–§3.5
> (on-disk layout, config/tool/UI, assets, sandbox policy) **remain valid** —
> they don't depend on the execution method (the `shared/sandbox.rs` trait
> hides it).

### 3.1-B Sidecar: bundled `wasmer` behind the `shared/sandbox.rs` contract (CHOSEN)

A ready-made binary **`wasmer.exe` / `wasmer`** (v7.2+, with V8+WASIX built
in — verified, §9) is placed next to the application. The main exe **doesn't
contain a single byte of Wasmer**; the sandbox is a separate process, spawned
on demand (like the current Local mode shells out to `python`, only now it's
our own `wasmer run` with an isolation policy).

- **`shared/sandbox.rs` contract** (behind a trait, mocked in tests — the
  `EngineBackend` pattern): find the binary next to the exe (override
  `MINDFORK_SANDBOX_WASMER`), spawn via `tokio::process` with `--v8`, mount
  assets (`--volume HOST:/sp`, `PYTHONPATH=/site-packages`), network by
  toggle (`--net`), timeout with **kill on timeout** (`kill_on_drop` +
  `taskkill /T` on Windows — proven clean, §9.1). Tool code is passed as a
  **file** (written into a mounted temp directory and run as
  `python /w/job.py`) — this avoids escaping `-c` arguments (see §9.4).
- **Code wrapper** (generated by us): begins with a **`setsockopt` shim**
  (§9.3, otherwise requests/urllib fail) + `PYTHONPATH`/`sys.path` setup as
  needed.
- **Interruption via process kill** (the main advantage of B): clean,
  ~360 ms, no lingering processes (§9.1) — eliminates risk #1 (§4.1)
  entirely, no metering/worker escalation needed.
- **Fault isolation**: runtime panics/OOM live in the sidecar process — the
  TUI doesn't crash.
- **Delivery weight**: the `wasmer` binary (~50–100 MB) next to the exe — like
  the `dictionaries/` groundwork; fetched by the same `mindfork sandbox
  setup` (§3.4), not stored in the repo.
- **No ABI/worker needed** — the process *is* the boundary; the
  `libloading`/cdylib/`extern "C"` machinery from option A isn't introduced.

### 3.1 (A, REJECTED) A separate `mindfork-sandbox` library (cdylib)

The project is currently a single package ([Cargo.toml](../../Cargo.toml)).
We'd switch to a **workspace**:

```
Cargo.toml            # [workspace] members = [".", "crates/mindfork-sandbox"]
crates/mindfork-sandbox/
  Cargo.toml          # crate-type = ["cdylib"]; deps: wasmer, wasmer-wasix, webc, …
  src/lib.rs          # extern "C" ABI + sandbox runtime
```

The main package **doesn't depend** on the sandbox crate — not a byte of
Wasmer ends up in the exe; the dll is built as a separate target. The
workspace shares a common `target/`, so in dev `mindfork_sandbox.dll` shows
up **next to the exe automatically** (`target/debug/`); in the release it's
just a second file in the archive (like `dictionaries/`).

**Why a dll and not a worker exe:** this was the requirement; the upside is
one process, no IPC scheme, simple data exchange. Downsides, stated honestly:
a wasm-runtime panic/OOM happens inside the application process (mitigated
with `catch_unwind` + a Store memory limit), a non-terminating thread leaks
(§4.1). **The ABI is designed so the implementation can later be moved to a
worker process without changing the host code** — the dll would start
spawning a worker itself, if live tests show that `terminate` isn't enough.

**C ABI** (Rust ABI is unstable across compiler/versions; data exchange —
JSON strings, already a tool convention):

```c
uint32_t mfsb_abi_version(void);                     // = 1; a major mismatch → we don't load
void*    mfsb_init(const char* config_json);         // asset paths, limits; NULL on error
char*    mfsb_exec(void* ctx, const char* req_json); // blocking call; JSON response
void     mfsb_cancel(void* ctx, uint64_t job_id);    // from another thread
void     mfsb_free(char* ptr);                       // free response strings
void     mfsb_shutdown(void* ctx);
```

Request: `{job_id, code, timeout_ms, net: bool}`; response: `{stdout, stderr,
exit_code, timed_out, error?}`. Inside the dll: `catch_unwind` at every
extern boundary (a panic across FFI is UB), its own mini task runtime
(`wasmer-wasix` TaskManager; the app's tokio is not touched), logs to
`logs/sandbox.log` (stdout is taken by the TUI — project convention).

**Host side:** a new `shared/sandbox.rs` — find the library next to the
binary (`mindfork_sandbox.dll` / `libmindfork_sandbox.so`; override
`MINDFORK_SANDBOX_LIB`), load with `libloading`, check `mfsb_abi_version`, a
safe `SandboxClient` wrapper **behind a trait** (mocked in tests — the
`EngineBackend` pattern). Called from the tool: `spawn_blocking` +
`tokio::time::timeout` + `mfsb_cancel` on timeout. The only new dependency
of the main package is `libloading` (tiny).

### 3.2 On-disk layout

```
mindfork.exe
mindfork_sandbox.dll          # next to the binary (like dictionaries/)
data/sandbox/                 # under shared/paths.rs (portable/system/path)
  python.webc                 # CPython 3.13 WASIX (the python/python package)
  site-packages/               # preinstalled packages (§3.4), read-only mount
  cache/                       # serialized compiled modules
```

No dll or assets → the tool responds with a clear message ("sandbox not
installed: … run `mindfork sandbox setup`"), the application doesn't crash —
the `UnavailableEmbedder` pattern.

### 3.3 Config, tool, UI

- `shared/config.rs`: `PythonMode { Wasmer, Local }` (serde kebab-case,
  **default = Wasmer**); `ToolSettings` gains
  `python_mode: PythonMode`,
  `python_net_enabled: bool` (networking inside the sandbox),
  `python_wasm_timeout_secs: u64` (default **30** — wasm interpretation is
  ~2–5× slower than native; the previous 10 s stays with Local mode). All
  `#[serde(default)]` — old `settings.json` needs no migration. `python_path`
  stays (Local). `python_enabled` **remains the master switch** (see decision
  point §7.1).
- `features/tools/python.rs` → a dispatcher by mode (an enum backend inside
  `PythonExec`): the tool id **`python_exec` doesn't change** (stable wire
  protocol; the feed presenter already renders code+console and keeps
  working — the `format_output` format is kept). `description()` in Wasmer
  mode lists the preinstalled packages ("numpy, pandas, requests, … are
  available") — the model uses the tool more readily and accurately; it also
  mentions isolation ("no access to files on the machine").
- `ToolConfig` gains `python_mode`/`python_net`/`python_wasm_timeout`/asset
  paths; the registry already rebuilds on `config.tools` edits.
- **Settings UI** ("Tools" section, "Python" group): the existing toggle;
  mode as a Choice (`Wasmer sandbox` / `Local interpreter`); mode-driven
  field visibility (interpreter path — Local only; network and timeout —
  Wasmer only) — precedent `model_fields`; description tooltips. `/` search
  and section counters pick this up automatically (a single index).

### 3.4 Assets: python.webc + site-packages

- **Option A (recommended): `mindfork sandbox setup`** — a clap subcommand
  (like `backup`/`import-lamellama`): downloads `python.webc` from the Wasmer
  registry and wheels from a **fixed lock list** in the repo (name/version/
  URL/sha256) — WASIX wheels from `pythonindex.wasix.org`, pure ones from
  PyPI; a wheel = a zip → unpacked into `site-packages/` **without pip or a
  host Python**. Idempotent, hash-checked, ~100–200 MB on disk.
- Option B: a ready-made `sandbox-assets.zip` in GitHub Releases, setup just
  unpacks it. Simpler for the user, more expensive to maintain across
  releases.
- Option C (rejected): pip cross-install (`--platform wasix_wasm32 --target
  …`) — requires an installed Python, which defeats the goal of "works out
  of the box".
- Starting set: **numpy, pandas, requests (+urllib3/certifi/idna/
  charset-normalizer), cryptography, pillow, aiohttp**. matplotlib is
  questionable (heavy; a headless render-to-file inside the sandbox is of
  limited use since the file never leaves the sandbox); scipy isn't in the
  index yet.
- For the guest: `PYTHONPATH=/site-packages` (RO), working directory `/tmp`
  — an in-memory tmpfs.

### 3.5 Sandbox policy

| Resource | Policy |
|---|---|
| Filesystem | only Python's webc-fs + RO `site-packages` + RW tmpfs `/tmp`. **Not a single host directory.** (The extension "mount `tools.fs_root`" is deliberately not in the first version.) **Correction (2026-09-11):** the RO `site-packages` was never delivered — `--volume` mounts writable, and `wasmer` 7.2.0 has no read-only form; the fix, `site-packages` packed with CPython into one image, is in [ADR 0005](../decisions/0005-python-sandbox-wasmer.md) §5 (amended). |
| Network | `python_net_enabled=false` → no sockets at all (no passthrough); `true` → host passthrough. Groundwork: a domain allowlist via our own `virtual-net` wrapper — out of scope. |
| CPU/time | timeout `python_wasm_timeout_secs` → `mfsb_cancel` → terminates the WASIX process; residual risk of a CPU loop — §4.1 |
| Memory | wasm32 ≤ 4 GB address space; limit Store tunables (e.g. 512 MB–1 GB max memory pages) |
| Output | previous `MAX_OUTPUT_CHARS` (8000) |
| Concurrency | one active task per application (a gate in the dll) — both a defense against thread leaks and predictable load |

---

## 4. Risks and open questions

1. **Interrupting a CPU loop is the main technical risk.** Wasmer has no
   epoch-interruption. Layered plan: (a) terminate the WASIX process — covers
   I/O-bound loops and, probably, most CPU loops (CPython checks signals in
   the eval loop; the question is whether WASIX delivers the signal without a
   guest syscall; **verify in Phase 0** on `while True: pass`); (b) if not,
   metering middleware (measure the overhead and compatibility with dlopen);
   (c) escalation — a worker process behind the same ABI (kill is an
   absolute guarantee). Until then: gate "one task at a time", a
   non-terminating thread leaks but doesn't crash the application. Treated as
   a blocker for the feature's GA status, not a blocker for starting.
2. **Windows host.** Support is claimed, a Windows CLI exists, but the
   combination of "python.webc 3.13 + dynamic linking of native wheels on a
   Windows host" needs a live-run confirmation (Phase 0). The user always has
   a fallback — Local mode.
3. **Ecosystem is young.** Dynamic linking is half a year old (7.0 —
   January 2026); build-scripts was archived in favor of wasinix in April
   2026 — the index is alive, but wheel URLs/tags may change → a lock list
   with exact URLs+sha256 (in the repo), setup doesn't depend on "latest".
4. **Weight and build.** A dll with cranelift is tens of MB; assets ~150 MB;
   a clean build of the sandbox crate takes minutes. CI: a separate job/
   target, the main gate (fmt/clippy/test) isn't slowed down. In the main
   package's `cargo test`, the sandbox is only involved via a mock; live runs
   are `#[ignore]`.
5. **First start.** Compiling python.wasm takes seconds to tens of seconds →
   a module cache is mandatory + a "preparing sandbox…" banner (the
   `RagProgress` pattern) on first run.
6. **requests/TLS.** Expected to work (OpenSSL 3.5.1 in the build, WASIX
   sockets, aiohttp with TLS in the index), but this is a key promise of the
   feature — verify a GET to `https://…` in Phase 0.
7. **Cyrillic/UTF-8.** In the WASIX guest, the locale is UTF-8 by default
   (no `PYTHONUTF8`-style workaround needed), but a "print Cyrillic" smoke
   test carries over (precedent: the cp1252 bug in Local mode).
8. **Licenses.** Wasmer/wasmer-wasix are MIT; CPython is PSF; wheels have
   their own licenses. Assets aren't in the repo (fetched by setup, like the
   Hunspell dictionaries) — no conflicts.

---

## 5. Staged plan

- **Phase 0 — spike (go/no-go, no code in main). ✅ DONE** (§9, verdict GO):
  on Windows we verified Python 3.13, numpy (dynamic linking), requests
  (HTTPS 200), opt-in networking, process-kill interruption; surfaced
  "V8 only", the `setsockopt` shim, and the weight of embedding →
  **decision: sidecar (§9.7)**. The embedding spike wasn't finished (no
  longer relevant for a sidecar). *Remaining (optional): a control run on
  Linux — but the sidecar mechanics are identical and already verified on
  Windows.*
- **Phase 1 — scaffolding (sidecar). ✅ DONE** (2026-07-11).
  `shared/sandbox.rs` (`SandboxRunner` behind a trait + `MockSandbox`; the
  real `WasmerSandbox`): finding `wasmer` (env `MINDFORK_SANDBOX_WASMER` →
  `data/sandbox/`), launching via `tokio::process` (`--v8`, `--volume
  HOST:GUEST`, `--net` by flag, timeout+`kill_on_drop`), capturing
  stdout/stderr, the code wrapper (**`setsockopt` shim** + script written to
  a temp directory with auto-cleanup, no `-c` escaping); pure
  `build_wrapper`/`build_args`. `python.rs` — an enum dispatcher for
  **Wasmer/Local** (tool id `python_exec` unchanged, `description` per mode+
  network; `format_output_parts` shared between both paths → the feed
  presenter isn't touched; graceful "sandbox not installed"). Config
  `PythonMode{Wasmer(default)/Local}`, `ToolSettings +=
  python_mode/python_net_enabled(default true)/python_wasm_timeout_secs(default 30)`;
  `python_enabled` remains `false`. `ToolConfig`/`build_registry` thread
  through mode, network, timeout and `sandbox_dir` (from `Paths::sandbox_dir`,
  `data/sandbox/`). Settings UI "Tools"→"Python": mode Choice + toggle +
  mode-driven visibility (path — Local; network+timeout — Wasmer). **963
  unit tests green** (+16), clippy/fmt clean. Live `#[ignore]` smoke test
  `runs_real_python_in_sandbox` run against a real `wasmer 7.2.0` —
  `print('hello sandbox')` executed inside the sandbox. Per the user's
  decisions: `python_enabled=false`, network=`true` with a toggle.
- **Phase 2 — assets and network. ✅ DONE** (2026-07-12).
  `mindfork sandbox setup` (clap subcommand `Sandbox{Setup{--force}}`, its
  own tokio runtime + single-instance guard): downloads into
  `data/sandbox/` the **`wasmer`** binary (a platform tar.gz from GitHub,
  streamed + sha256, unpacked with `flate2`+`tar` into `wasmer-dist/`),
  **`python.webc`** (via `wasmer package download` itself, home/cache under
  the sandbox), **wheels** from a lock list (numpy from the wasix index,
  the requests stack from PyPI) → sha256 verified → unpacked into
  `site-packages/` **without pip/a host Python**. All driven by a **lock
  list with exact URLs+sha256** (`features/sandbox_setup.rs`, a pure core +
  a thin network layer), idempotent (existing files are skipped, `--force`
  re-downloads). **Compilation cache** — `WasmerSandbox` sets
  `WASMER_CACHE_DIR=<dir>/cache` (warm starts). A shared resolver
  `shared::sandbox::locate_wasmer` (direct `<dir>/wasmer[.exe]` →
  `wasmer-dist/bin/wasmer[.exe]`) — used by both the runtime and setup.
  Networking decision (Phase 1): `python_net_enabled` toggle. The first-run
  banner (python.wasm compilation) — **deferred to Phase 3** (needs a
  tool→UI progress channel; the setup command prints progress to stdout).
  **969 tests green, 39 `#[ignore]`** (+5 live smokes), clippy/fmt clean.
  **Run on a real setup (Windows, wasmer 7.2.0)**: `sandbox setup`
  downloaded/unpacked everything (wasmer 206 MB, python.webc 44 MB, 6
  wheels, sha256 matched); **8 live smokes green** — numpy 2.3.2 (dynamic
  linking), requests HTTPS 200 (with network), blocked without network,
  Cyrillic, timeout kill.
- **Phase 3 — robustness and polish.** Sidecar memory/resource limits; "one
  task" gate; first-run banner (tool→UI compilation progress); revisit the
  `python_enabled` default (§7.1, now that setup is a single command); docs
  (spec §9.3/§13.2) + ADR "Python sandbox: `wasmer`/WASIX sidecar behind
  `shared/sandbox.rs`" → `docs/decisions/0005`.

---

## 6. What this gives the user (summary)

| | Local (now) | Wasmer (plan, default) |
|---|---|---|
| Requires Python on the machine | yes | **no** |
| Code's access to machine files | full | **none** (virtual filesystem) |
| Network | full, always | **toggleable** (a toggle, physically no sockets) |
| numpy/pandas | if installed manually | **preinstalled** |
| requests | if installed manually | **preinstalled** (with network enabled) |
| Speed | native | ~2–5× slower (acceptable for a tool) |
| Startup | ~50 ms | first — seconds (compile+cache), then ~0.1–0.5 s |
| Disk space | 0 | ~200–350 MB (`wasmer` binary + python.webc + packages) |

---

## 7. Decision points (need user decisions before Phase 1)

1. **`python_enabled` default.** The sandbox removes the original reason for
   "off by default" (spec §13.2). Recommendation: keep **`false`** in the
   first PR (assets might not be present yet — "enabled but not ready" is
   worse than "disabled"), revisit as `true` after Phase 2 has been tried
   out (once setup and clear errors exist).
2. **Default network in the sandbox (`python_net_enabled`).** In favor of
   `true`: the main value is preinstalled requests; enabling Python is
   already a deliberate opt-in. In favor of `false`: the project's
   conservative privacy defaults (python/fs are off). Recommendation:
   **`true`** — but with a separate toggle and a clear tooltip in the UI.
3. **Asset delivery:** A (setup download from a lock list) vs B (an archive
   in the release). Recommendation: **A** (reproducible, doesn't bloat
   releases; B can be added later).
4. **matplotlib in the starting set:** recommendation is **no** (weight;
   the output file is locked inside the sandbox anyway), add on request.
5. **Interruption:** decided by the Phase 0 findings (terminate-only /
   +metering / a worker process behind the same ABI).

---

## 9. Phase 0 results (spike, 2026-07-11, Windows 11 x64)

Run on a real target machine (Windows 11 Pro, x86_64). **Wasmer 7.2.0**
installed (winget `Wasmer.Wasmer`; `runtimes: Singlepass, Cranelift, V8;
features: wasix`). All work was done outside the repo (scratchpad); no code
was added to `main`/a branch.

### 9.1 Go/no-go summary table

| Check | Result | Details |
|---|---|---|
| Launching `python/python` | ✅ | CPython **3.13.0rc2** (wasix-org WASIX build) |
| Backend on Windows | ⚠️→✅ | **cranelift and singlepass crash**, only **V8** works (see 9.2) |
| Cold start | ✅ | ~3.0–3.8 s (package download + first compilation) |
| Warm start | ✅ | **~630–660 ms** (V8, package cached) |
| stdlib (json/hashlib/socket) | ✅ | present |
| `ssl` / TLS | ✅ | **OpenSSL 3.5.1**; manual `wrap_socket` → **TLSv1.3** |
| numpy (native `.so`) | ✅ | **numpy 2.3.2**, matmul + `linalg.eigvals` (loads `lapack_lite.so`); **dynamic linking of 19 `.so` files works** (~4.9 s including module compilation) |
| requests (full stack) | ✅ | **requests 2.34.2 → HTTP 200** (real GitHub API, TLS) — with the shim (9.3) |
| Network opt-in | ✅ | without `--net` — **a clear explicit refusal**; with `--net` — DNS + TCP work |
| Interrupting a CPU loop | ✅ | `taskkill /T /F` kills `while True: pass` in **~360 ms, no lingering processes** |
| Embedding from Rust (wasmer+wasmer-wasix, `v8` feature) on Windows | ✅/⏳ | dependencies resolve (wasmer 7.2.0, wasmer-wasix 0.702.0); the cdylib build — see 9.5 |

### 9.2 Critical: on Windows only the V8 backend works

The **cranelift** compiler (default) **panics** while compiling
`python.wasm`: `unimplemented clobbers for exn abi of WindowsFastcall` +
`index out of bounds` in `cranelift-codegen` — the **exception-handling ABI**
isn't implemented (the exception-handling proposal that WASIX uses for
setjmp/longjmp/dynamic linking) under Windows fastcall. **singlepass** —
`exceptions proposal not enabled` (and it doesn't start up even with
`--enable-exceptions`). Only the **V8** runtime works (`--v8`): it has native
WASM exception support. This changes the embedding plan: the dll would need
to be built with the **`v8`** feature of the `wasmer` crate (docs confirm
`sys` and `v8` compose; V8 provides WASM exceptions+GC), **not** the default
cranelift. The cost is V8 as a static dependency (dll weight, build
time/complexity). On Linux cranelift probably works (check separately) — but
to **avoid** maintaining two code paths, it's reasonable to also take V8
there for uniformity (decision — §7, an additional decision point below).

### 9.3 Networking works, but needs a `setsockopt` shim

With `--net`: **DNS and raw TCP connect work**, a manual TLS handshake
yields TLSv1.3. But `urllib`/`http.client` (and hence `requests`) failed with
`[Errno 28] Invalid argument` — the culprit is exactly one thing:
`setsockopt(IPPROTO_TCP, TCP_NODELAY)` isn't implemented in WASIX/V8 and
throws `EINVAL`, and `http.client.HTTPConnection.connect` **always** sets it.
Since **we** generate the tool code wrapper, this is fixed with a tiny shim
mixed in ahead of the user's code:

```python
import socket
_o = socket.socket.setsockopt
socket.socket.setsockopt = lambda self, *a, **k: (None if <TCP_NODELAY> else _o(self,*a,**k))
```

After the shim, `urllib` → HTTP 200, full `requests` → HTTP 200. Conclusion:
**a `setsockopt` shim (swallow unsupported socket options) is a mandatory
part of the Wasmer-mode wrapper.**

### 9.4 Plan refinements

- **Python 3.13 assets**: numpy is pulled as the wheel
  `numpy-2.3.2-cp313-cp313-wasix_wasm32.whl` (tag `wasix_wasm32`, stable
  2.3.2 is preferred over dev-2.4.0); requests and its dependencies
  (urllib3/certifi/idna/charset_normalizer) are ordinary `py3-none-any`
  wheels from PyPI (not in the WASIX index, and shouldn't be — pure
  Python). A wheel = a zip → unpacking without pip/a host Python is
  confirmed. The lock list (§3.4, §7.3) should pull numpy from
  `pythonindex.wasix.org` and the requests stack from PyPI.
- **Mounting**: the CLI flag `--mapdir GUEST:HOST` is **deprecated** →
  `--volume HOST:GUEST` (reversed order!). Doesn't matter for embedding (the
  virtual-fs API), but keep it in mind in the setup docs/scripts.
- **UTF-8**: not checked separately (moved into Phase 2 smokes), but the
  guest locale in WASIX is UTF-8, so the `PYTHONUTF8` workaround from Local
  mode is probably unnecessary.

### 9.5 Embedding spike (cdylib on Windows) — surfaced a heavy build toolchain

A minimal project (`wasmer 7` + `wasmer-wasix 0.702`,
`features=["v8"]`/`["sys","v8"]`) **resolves** (wasmer 7.2.0, wasmer-wasix
0.702.0), but the **build fails**: the V8 backend, via `bindgen 0.72`,
requires **libclang** (`Unable to find libclang … set LIBCLANG_PATH`). No
LLVM/clang on the machine (installable with winget `LLVM.LLVM`, ~2.5 GB).
The upshot — two heavy requirements for the dll approach on Windows, both a
consequence of "V8 only" (9.2): **(1) build-time** — LLVM/libclang for V8
bindings; **(2) runtime** — static V8 in the dll (weight, build time). This
isn't a dead end, but it substantially changes the cost of "embed in a dll"
and raises **decision point 7.7** (below): embed V8 in the dll vs.
**bundling the ready-made `wasmer` CLI** as a sidecar (its V8+WASIX is
already built and working, and killing the process is clean, 9.1) behind
the same `shared/sandbox.rs` contract. The full embedding run ("python.wasm
via `WasiEnvBuilder` + `Pipe` + `terminate`") was deferred pending the 7.7
decision — no point installing LLVM and waiting on a V8 compile if the
sidecar is chosen.

### 9.7 Decision point 7.7: **embed V8 in the dll** vs. **bundle `wasmer` CLI as a sidecar**

Both options hide behind the same `shared/sandbox.rs` trait (the host code
doesn't distinguish between them).

| | A. Embed V8 in `mindfork_sandbox.dll` | B. Bundle `wasmer.exe`/`.so` — no dll, a sidecar process |
|---|---|---|
| As requested (dll) | yes | no (a process, not a dll) — but behind the same contract |
| Our build toolchain | +LLVM/libclang, a long V8 build | none (take the ready-made Wasmer binary) |
| Delivery weight | a dll with V8 (tens of MB) | `wasmer.exe` (~50–100 MB) alongside |
| CPU-loop interruption | in-process `terminate` of V8 (not verified on a tight loop) | **process-kill — proven clean (9.1)** |
| Runtime panic/OOM | inside the application process (needs `catch_unwind`+limits) | **isolated in the sidecar** (it crashes — doesn't take down the TUI) |
| Runtime updates | rebuild the dll | replace the binary |
| Filesystem/network control | fine-grained (virtual-fs/virtual-net API) | coarser (CLI flags `--volume`/`--net`), but sufficient |

**Recommendation:** given 9.2/9.5 (Windows hard-requires V8, and embedding it
pulls LLVM+V8 into *our* build) and 9.1 (process-kill is perfectly clean),
**option B (sidecar with a bundled `wasmer`)** is more pragmatic and more
reliable regarding interruption/isolation, while still preserving the spirit
of the requirement (a separate artifact next to the exe, a unified
`shared/sandbox.rs` contract, zero Wasmer in the main exe). Option A remains
possible if a dll is strictly required.

> **DECIDED (2026-07-11): option B.** The user chose the `wasmer`
> sidecar bundle. The current architecture is **§3.1-B**; §3.1-A (cdylib) is
> rejected. The full embedding spike (WasiEnvBuilder+Pipe+terminate) wasn't
> finished — it's no longer relevant for B. The sidecar mechanics have
> **effectively already been verified**: every run in §9 (python/numpy/
> requests/kill) went through the `wasmer` CLI — exactly what
> `shared/sandbox.rs` will do.

### 9.6 Impact on risks (§4) and decision points (§7)

- **Risk 1 (interruption)** reduced: process-kill of a CPU loop on Windows is
  clean and fast (~360 ms). This confirms the "worker process behind the
  same ABI" escalation as a guaranteed fallback, if in-process V8
  `terminate()` doesn't stop a tight CPU loop (verify in the embedding
  spike).
- **Risk 2 (Windows)** is largely **cleared** for the runtime (python/numpy/
  requests/network/kill all work), **but** with the 9.2 caveat: the V8
  backend (not cranelift) is mandatory.
- **Risk 6 (requests/TLS)** **cleared** — end-to-end with the shim (9.3).
- **New decision point (7.6): compilation backend.** Windows forces V8.
  Options: (a) **V8 everywhere** (unified code, but a heavier dll on Linux
  too); (b) V8 on Windows + cranelift on Linux (a lighter Linux build, but
  two paths and `#[cfg]`). Recommendation: **(a) V8 everywhere** in the first
  version for uniformity; defer the Linux cranelift optimization. Needs a
  user decision.
- **Decision point 7.1 (`python_enabled` default)**: the 9.1 results
  strengthen the case for keeping it `false` until `sandbox setup` is ready —
  the assets (python.webc + wheels, ~150–250 MB) are fetched separately,
  "enabled but not installed" is worse than "disabled".

---

## 8. Sources

- [Wasmer 7 (blog, 2026-01-30)](https://wasmer.io/posts/wasmer-7) · [release v7.0.0](https://github.com/wasmerio/wasmer/releases/tag/v7.0.0) · [Phoronix](https://www.phoronix.com/news/Wasmer-7.0-Released) · [heise](https://www.heise.de/en/news/WebAssembly-Wasmer-7-0-brings-experimental-async-support-for-Python-11163325.html) · [InfoWorld](https://www.infoworld.com/article/4125985/wasmer-beefs-up-python-support.html)
- [Python on the Edge (Wasmer blog, Sep 2025)](https://wasmer.io/posts/python-on-the-edge-powered-by-webassembly) · [Dynamic linking in WASIX](https://wasmer.io/posts/dynamic-linking-in-wasm-wasix) · [Announcing WASIX](https://wasmer.io/posts/announcing-wasix)
- [pythonindex.wasix.org](https://pythonindex.wasix.org/) (WASIX wheel index) · [wasix-org/build-scripts](https://github.com/wasix-org/build-scripts) (archived; successor [wasinix](https://github.com/wasix-org/wasinix)) · [wasix-org/cpython](https://github.com/wasix-org/cpython)
- [python/python package in the registry](https://wasmer.io/python/python) · [WASIX docs](https://wasix.org/docs/)
- [wasmer-wasix (crates.io)](https://crates.io/crates/wasmer-wasix) · [lib.rs](https://lib.rs/crates/wasmer-wasix) · [docs.rs](https://docs.rs/crate/wasmer-wasix/latest)
- Interruption: [wasmer#337](https://github.com/wasmerio/wasmer/issues/337) · [rust-lang forum](https://users.rust-lang.org/t/interrupt-wasmer-wasmtime/60331) · (for comparison: [wasmtime epochs](https://docs.wasmtime.dev/examples-interrupting-wasm.html))
- Filesystem sandboxing: [GHSA-4mq4-7rw3-vm5j](https://github.com/wasmerio/wasmer/security/advisories/GHSA-4mq4-7rw3-vm5j) · [wasmer#4267](https://github.com/wasmerio/wasmer/issues/4267)
- Alternatives: [Pyodide outside the browser — no](https://github.com/pyodide/pyodide/issues/558), [wasmtime/wasmer discussion](https://github.com/pyodide/pyodide/discussions/5145) · [wasi-wheels (unmaintained)](https://github.com/dicej/wasi-wheels) · [numpy WASI issue](https://github.com/numpy/numpy/issues/25859) · [state of WASI support in CPython](https://snarky.ca/wasi-support-for-cpython-june-2023/)
