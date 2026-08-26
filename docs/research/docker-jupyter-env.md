# A containerised test environment: JupyterLab + mindfork + a CPU llama.cpp stack

Research and design for a one-command, disposable environment that runs the app
from the **current branch** inside a browser JupyterLab terminal, against a
local CPU-only inference stack (chat + embeddings). Pre-decision: the forks in
§5 need the user's answer before any of it is built (AGENTS.md §1).

Related: [docs/install.md](../install.md) §2–§3 (data layout, engine modes),
[docs/history/command-only-control.md](../history/command-only-control.md) (what
a JupyterLab terminal takes away),
[docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md) (the rented live
gate this does **not** replace), [ADR 0004](../decisions/0004-engine-contract-multi-provider.md)
(external mode is a first-class engine mode, not a test seam).

## 1. Why

Two needs, and the second one is the reason this is worth building rather than
scripting by hand:

1. **JupyterLab is a first-class target, and it is the one we cannot test on
   Windows.** Three tracks were opened by findings from a JupyterLab terminal —
   command-only control, `/export`, commands stage 3 — and every one of them was
   found by a manual pass on a borrowed host. `Ctrl+N`/`Ctrl+T` never arriving,
   OSC 52 being dropped, the file browser's idea of "current directory": none of
   this reproduces in Windows Terminal. Today verifying a fix means finding a
   JupyterLab. A `docker compose up` that ends in a browser tab makes that a
   30-second step.
2. **A local live stack.** The `#[ignore]` smokes are the mandatory gate for
   anything touching engine/memory/tools (AGENTS.md §3), and today they run
   either against a hand-started `llama-server` or against rented HF endpoints
   (~$1 and ~16 minutes per run). A CPU stack on ports 8000/8001 with the exact
   URLs the docs already print means `cargo test -- --ignored` works from the
   Windows host with two environment variables and no GPU. It will be slow and
   it will not replace the rented gate for model-behaviour smokes (§6.3) — but
   for protocol, streaming, tool-call and RAG plumbing it is a real gate that
   costs nothing.

Non-goal: shipping a container to users. This is developer tooling; the
supported installation paths stay the release archives, the Linux packages and
the Windows installer.

## 2. What was measured

Everything below was verified against the live registries/APIs on 2026-08-26,
not assumed.

| Fact | Value | Source |
|---|---|---|
| CPU llama.cpp server image | `ghcr.io/ggml-org/llama.cpp:server` exists (also `server-b<NNNN>` per build); `full`, `light` likewise; there is **no** `latest` | ghcr.io manifest HEAD → 200/404 |
| Chat model | `bartowski/google_gemma-4-E2B-it-GGUF` → `google_gemma-4-E2B-it-Q8_0.gguf`, **4 967 497 184 B (4.63 GiB)** | HF API + HEAD |
| Vision projector | `mmproj-google_gemma-4-E2B-it-f16.gguf`, **985 653 760 B (940 MiB)**, same repo | HF API + HEAD |
| Embedder | `ggml-org/bge-m3-Q8_0-GGUF` → `bge-m3-q8_0.gguf` (lowercase `q8_0`), **634 553 760 B (605 MiB)** | HF API + HEAD |
| JupyterLab base image | `quay.io/jupyter/minimal-notebook:lab-4.6.3` (Ubuntu 24.04, Python 3.13) | quay.io tag list |
| Status-bar RAM/CPU widget | `jupyterlab-system-monitor` **0.8.0 requires `jupyterlab ~=3.0`** — dead end on JupyterLab 4. The JupyterLab-4 path is **`jupyter-resource-usage` 1.3.0** (`jupyter-server>=2`) | PyPI metadata |

The embedder repo is not a free choice: `tools/hf_api.py:74` names
`ggml-org/bge-m3-Q8_0-GGUF` as *"the exact model the gates were calibrated on"*,
and the retrieval thresholds in `docs/install.md` §3 were tuned against it. The
container must serve the same file or the smokes measure a different thing.

## 3. Constraints the codebase imposes

Read out of the source rather than guessed; each one changes a line of the
Dockerfile or the compose file.

- **Linux build needs ALSA.** `rodio`/`cpal` link against libasound: build with
  `libasound2-dev`, run with **`libasound2t64`** on Ubuntu 24.04. Not
  `libasound2` — after the time_t64 transition that name is virtual and apt
  satisfies it with an OSS shim missing `snd_device_name_get_hint`, i.e. the
  binary dies at startup. This is a trap the project already paid for once
  (`packaging/nfpm.yaml`, caught by `packaging.yml`'s ubuntu:24.04 smoke).
- **Nothing else is a system dependency.** `rusqlite` is `bundled`, `reqwest` is
  `rustls`, the clipboard is `x11rb` (pure Rust) — no sqlite, no OpenSSL, no
  pkg-config dance.
- **Toolchain is pinned** to 1.96.0 (`rust-toolchain.toml`), so the build stage
  pins the same image tag; a floating `rust:latest` would break the clippy gate
  the moment stable moves.
- **glibc direction.** Build on Debian bookworm (2.36), run on Ubuntu 24.04
  (2.39): older-on-newer is fine, the reverse is not.
- **`/etc/machine-id` must exist and be non-empty**, or `shared::secrets`
  reports "storing keys is not supported on this machine" and an API key typed
  into the settings screen cannot be saved (`src/shared/secrets.rs:384`). Docker
  images generally ship it empty. Baking a fixed id into the image is what makes
  a stored key survive `docker compose up` cycles on the same data volume.
- **Env overrides beat `settings.json`, permanently.** `apply_env_overrides`
  (`src/main.rs:751`) forces `mode: external` whenever `MINDFORK_ENGINE_URL` is
  set. Convenient for a smoke run, wrong for a test environment: with it set you
  cannot switch the engine to Claude/OpenAI on the settings screen and have it
  stick across a restart. Fork F4.
- **`AppConfig` is `#[serde(default)]`** (`src/shared/config.rs:1712`), so a
  *partial* `settings.json` naming only the two engine sections is a legal,
  forward-compatible seed file. No need to serialise the whole config.
- **The deb layout already solves "binary + adjacent data".** Real binary in a
  private directory next to `defaults.json` and `data/dictionaries/`, a symlink
  from `PATH`; `current_exe()` resolves the symlink via `/proc/self/exe`, so the
  app finds its neighbours. Mirroring that layout in the image means the
  dictionary fallback (install.md §2.1, P1) works unchanged.
- **The embedding server needs a raised physical batch.** `--embeddings` alone
  leaves `n_ubatch=512` and any chunk over ~512 tokens is rejected outright —
  files silently fail to index. `-ub 8192 -b 8192` (install.md §3).
- **`--jinja` is mandatory** for the chat server: Gemma's own chat template is
  what makes formatting and tool-calling correct.

## 4. Proposed shape

```
docker/
  compose.yaml            # models(one-shot) → chat, embed → lab
  .env.example            # ports, threads, context, RAM knobs, JUPYTER_TOKEN
  models/fetch.sh         # curl into the models volume, size-verified, idempotent
  lab/
    Dockerfile            # stage 1: rust:1.96-bookworm build; stage 2: minimal-notebook
    entrypoint.sh         # seed the data root on first run, then exec start-notebook.py
    defaults.json         # {"mode":"path","path":"/home/jovyan/mindfork/data", ...}
    settings.seed.json    # partial AppConfig: engine.external + embed.external
    jupyter_server_config.py
  README.md
.dockerignore             # target/, .git/, site/, lcov.info, dist/ …
```

Four services:

| Service | Image | Role |
|---|---|---|
| `models` | `curlimages/curl` (pinned) | one-shot; downloads the GGUFs into the `models` volume, exits 0. `restart: "no"`, the servers wait on `service_completed_successfully` |
| `chat` | `ghcr.io/ggml-org/llama.cpp:server-b<pin>` | `-m gemma…Q8_0 --host 0.0.0.0 --port 8000 -c ${CTX} --jinja` (+ `--mmproj` per F5); healthcheck `/health` |
| `embed` | same image | `-m bge-m3-q8_0 --embeddings --port 8001 -c 8192 -ub 8192 -b 8192` |
| `lab` | built from the repo | JupyterLab 4 on 8888 + the `mindfork` binary built from the working tree |

Ports 8000/8001 are published to the host on purpose: it makes the same stack
usable for `cargo test -- --ignored` from Windows, which is half the value (§1.2).

Why the app talks to the servers over the compose network in **external** mode,
not in managed mode: managed mode would put `llama-server` inside the lab
container, tie the model's lifetime to the TUI process, and reload 4.6 GiB every
time the app restarts. External mode is a first-class engine mode (ADR 0004),
the servers stay warm across app restarts, and `docker compose restart chat` is
a way to test the app's server-health and reconnect paths deliberately.

Data lives on a named volume mounted at `/home/jovyan/mindfork` with
`defaults.json` → `{"mode": "path", "path": "/home/jovyan/mindfork/data"}`, so
`chats/*.json`, `logs/` and anything `/export` writes are **visible in the
JupyterLab file browser** — which is exactly the workflow
[chat-export-file.md](../history/chat-export-file.md) §"the JupyterLab case"
describes as the way out of a terminal with no clipboard.

## 5. Forks — decided

**User's decision, 2026-08-26: F1a, F2a, F3a, F4a, F6a; the host's Docker VM has
16 GiB (measured: `docker info` → 16 531 472 384 B, 16 CPUs), so the vision
projector is affordable and stays on by default (F5).**

### F1. One container or a compose stack

- **F1a — compose, four services (recommended).** Independent restarts,
  healthchecks, per-service memory limits, and rebuilding the app after a code
  change touches only the `lab` image while the models stay loaded.
- F1b — a single all-in-one container with a supervisor. One `docker run`, but
  every app rebuild reloads both models, and the image carries a process
  manager we would have to maintain.

### F2. Where the model files live

- **F2a — a named volume filled by a one-shot `models` service (recommended).**
  Downloaded once (~5.2 GiB, or 6.2 with the projector), survives every image
  rebuild, and the download is re-runnable and size-verified.
- F2b — baked into the image at build time. Self-contained and offline-capable
  afterwards, but a 6 GiB image and a build that cannot be pruned.
- F2c — bind-mount a host directory of GGUFs the developer already has. Best
  when the files exist; worst as a default because it needs a host path.

*(F2a plus an `.env` override `MODELS_DIR=` that switches it to a bind mount
gives both; that is what "recommended" means here.)*

### F3. JupyterLab fidelity to the screenshot

The screenshot shows JupyterLab 4 with the Launcher, a file browser, Python 3
and **Java** kernels, extra sidebar extensions, and a RAM/CPU/Disk status bar.

- **F3a — Launcher + terminal + Python 3 kernel + `jupyter-resource-usage`
  status bar (recommended).** Reproduces the layout and the status bar; the
  RAM readout is genuinely useful when a CPU llama.cpp is chewing 6 GiB.
- F3b — F3a plus the Java kernel (IJava) and the other sidebar extensions, for a
  closer visual match. Costs a JDK (~200 MB) and buys nothing for testing
  mindfork.
- F3c — bare JupyterLab, no extras.

Whichever is chosen, the load-bearing part is the same: **the terminal is
xterm.js in a browser tab**, which is what makes the environment worth having.

### F4. How the app is pre-configured

- **F4a — seed a partial `data/settings.json` on first start, only if absent
  (recommended).** The settings screen shows real values, everything the user
  changes persists, and the environment variables stay free — so switching the
  engine to Claude/OpenAI to test a cloud path works and survives a restart.
- F4b — `MINDFORK_ENGINE_URL`/`MINDFORK_EMBED_URL` in the compose file. One
  line, already documented, but permanently pins the engine to external
  (`src/main.rs:751`) and quietly makes the cloud providers untestable in this
  environment.

### F5. Optional extras (each costs image size or RAM)

- **Vision projector** (`--mmproj`, +940 MiB download, +RAM): unlocks
  `/image attach` and the `/props` `modalities` path (spec §9.10) — a feature
  area otherwise untestable without a GPU box. Proposal: **on by default**,
  disabled with one `.env` flag.
- **Python sandbox**: `mindfork sandbox setup` is a ~300 MB download into the
  data volume. The container has a real Python 3.13 with numpy/pandas, so
  seeding `tools.python_mode = "local"` + `python_enabled = true` gives a
  working `python_exec` for free. Proposal: **local mode on, wasmer sandbox left
  to a documented one-liner** — which also mirrors what the Linux packages do.
- **Interface language**: the container's locale is `en_US`, so the app would
  come up in English. Proposal: `defaults.json` → `"default_language": "ru"`,
  overridable by an `.env` knob.
- **Node for the MCP host** *(added after the first review — user's request,
  2026-08-26)*. Nearly every server in the ecosystem is an `npx` one
  (install.md §4.2) and the base image ships no Node, which would leave a
  documented feature untestable here. Conda-forge (Node 26) rather than apt:
  Ubuntu 24.04 still packages the end-of-life Node 18, and conda's prefix is
  already owned by the notebook user, so `npm -g` needs no root. The reference
  filesystem server is installed at build time and seeded **scoped to the
  mounted work directory**, addressed by its binary rather than `npx` so a
  launch does not hit the registry — with the master switch left off, because
  the host's double opt-in exists precisely so that a convenience seed cannot
  be what enables an arbitrary user-privileged program.

### F6. Scope of the first PR

- **F6a — one PR for the whole stack (recommended)**, since none of the four
  services is useful without the others.
- F6b — stage 1 = models + the two llama.cpp services (usable immediately from
  the Windows host for `cargo test -- --ignored`), stage 2 = the lab image. Two
  smaller PRs; a memory note says the "one PR = one stage" rule bends for a
  small additive stage.

## 6. Risks and honest limits

**6.1. CPU speed.** Gemma 4 E2B at Q8_0 on a desktop CPU is roughly 5–15
tokens/s depending on cores and memory bandwidth, with a first-token latency
that grows with the prompt. Usable for exercising the UI, tool loops and RAG;
tedious for long generations. `Q4_K_M` (~2.7 GiB) would roughly halve the memory
and double the speed at some quality cost — worth offering as an `.env` knob
even though the ask named Q8_0.

**6.2. RAM.** Chat weights 4.63 GiB + KV cache at `-c 8192` + the embedder
0.6 GiB + JupyterLab ≈ **8–9 GiB** for the stack, before the projector's
~1 GiB. Docker Desktop on Windows defaults to a WSL2 VM sized at a fraction of
host RAM; if that limit is below ~10 GiB the chat server is OOM-killed at load
time and the symptom will look like a hang. This needs to be checked before
building, and stated in the README.

**6.3. This does not replace the rented gate.** Several live smokes assert on
model *behaviour* (tool selection, self-model consolidation, note connectivity)
and were calibrated on Gemma 4 31B / Qwen 3.6 27B. A 2B-effective model will
fail some of them for reasons that are not defects. The container gate is for
protocol and plumbing; `tools/e2e_hf.py` stays the gate of record, and any
journal entry claiming "Smoke — GO" must keep naming the stack it ran on.

**6.4. Build time.** A cold `cargo build --release` of this crate in Docker is
minutes, not seconds. BuildKit cache mounts for the cargo registry and the
target directory make the *second* build incremental; the pattern is
`RUN --mount=type=cache,… cargo build --release && cp target/release/mindfork /out/`,
because a cache mount is not part of the layer. Requires a `.dockerignore` that
excludes `target/` — otherwise the build context alone is several GiB.

**6.5. Windows line endings and the executable bit.** A shell script checked out
with CRLF fails inside the container with `exec format error`. Already covered:
the repository's `.gitattributes` is a blanket `* text=auto eol=lf`
(`git check-attr` confirms `eol: lf` on the new scripts), so nothing extra is
needed. The executable bit is *not* covered — `core.filemode` is `false` on
Windows, so git records mode 644 — which is why the Dockerfile `chmod`s the
start-up hook rather than relying on the checkout.

**6.6. The download is the least reliable part, and it failed the first way it
could.** A 4.63 GiB transfer from the Hugging Face CDN died at 4.34 GiB with
curl exit 92 — *"stream error in the HTTP/2 framing layer"*. Two consequences,
both now in `docker/models/fetch.sh`: the fetch runs `--http1.1` (HTTP/2
multiplexing has nothing to offer one enormous sequential body, and a measurable
failure rate to charge for it), and it uses `--retry-all-errors`, because plain
`--retry` covers transient HTTP statuses and connection failures but **not** a
mid-stream protocol error — i.e. without it, the one failure that actually
happens is the one not retried. Resume (`-C -`) plus a `.part` file that is only
renamed after the size check turns the remaining failures into a delay rather
than a corrupt GGUF.

## 7. Plan once the forks are settled

1. `.dockerignore`, `docker/models/fetch.sh`, `docker/compose.yaml` with the
   three non-lab services; verify both servers answer `/health` and that
   `MINDFORK_ENGINE_URL=http://localhost:8000/v1 cargo test -- --ignored` runs
   from the Windows host.
2. `docker/lab/Dockerfile` (build stage + notebook stage), entrypoint, seed
   files; `docker compose up` → browser → terminal → `mindfork`.
3. A live pass in that terminal against the checklist that
   [command-only-control.md](../history/command-only-control.md) §"tier-1"
   already defines, recording the result.
4. Docs: `docs/install.md` gets a section (§6.x, "Running"), `README.md` a
   pointer, `docs/journal/ci.md` the entry (this is developer infrastructure,
   the same file the rented gate lives in), `CHANGELOG.md` **nothing** — no
   user-visible effect.

## 8. Open questions for the user

- Available RAM for Docker Desktop's VM (§6.2) — determines the context size and
  whether the projector is affordable.
- How literally the screenshot should be reproduced (F3).
- Whether the Q4_K_M knob is wanted alongside Q8_0 (§6.1).
