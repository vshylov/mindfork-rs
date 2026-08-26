# The containerised test environment

JupyterLab in a browser tab, a terminal inside it, and `mindfork` built from the
current working tree talking to a CPU-only llama.cpp stack. One command up, one
command down, nothing installed on the host but Docker.

Why it exists, what was measured and which alternatives were rejected:
[docs/research/docker-jupyter-env.md](../docs/research/docker-jupyter-env.md).

## 1. Start

```bash
cd docker && cp .env.example .env && docker compose up --build
```

Then open **<http://localhost:8888/lab?token=mindfork>** → *Other → Terminal* →
type `mindfork`.

The first run downloads ~6.2 GiB of GGUF weights and compiles the crate, so it
takes a while; both are cached afterwards. Later starts are `docker compose up`
and are up in seconds, plus however long llama.cpp needs to load the weights.

Stop with `docker compose down`. The models and the app's data survive it — they
live in named volumes. `docker compose down -v` is the reset button that removes
both.

## 2. What runs where

| Service | What it is | Reachable at |
|---|---|---|
| `models` | one-shot download into the `models` volume, then exits | — |
| `chat` | `llama-server` with Gemma 4 E2B-it Q8_0 (+ the vision projector) | `chat:8000` inside, `localhost:8000` outside |
| `embed` | `llama-server --embeddings` with bge-m3 Q8_0 | `embed:8001` inside, `localhost:8001` outside |
| `lab` | JupyterLab 4 + the `mindfork` binary | `localhost:8888` |

Inside the lab container:

- `~/mindfork/data` — the app's data root (`settings.json`, `chats/*.json`,
  `data.db`, `logs/`). A named volume; it survives image rebuilds.
- `~/work` — the host's `docker/work/`, so dropping a file there from Windows
  makes it reachable by `/file attach` or `/rag add`.
- `mindfork` is on `PATH`; the real binary and the spellcheck dictionaries sit in
  `/opt/mindfork`, mirroring the layout of the Linux packages.

The app is pre-configured through a seeded `settings.json` — engine and
embeddings in **external** mode pointing at the two servers. Everything stays
editable in the settings screen and every change persists, including switching
the engine to a cloud provider; that is why the environment does *not* use
`MINDFORK_ENGINE_URL`, which would pin the mode to external on every launch.

## 3. After a code change

```bash
docker compose up -d --build lab
```

Only the app is rebuilt; the models stay loaded in the two server containers.
The cargo registry and the `target/` directory are BuildKit cache mounts, so this
is an incremental compile, not a fresh one.

## 4. Running the live smokes from the host

The two server ports are published, so the mandatory `#[ignore]` gate
(AGENTS.md §3) can run from Windows against this stack with no GPU:

```powershell
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
$env:MINDFORK_EMBED_URL  = "http://127.0.0.1:8001/v1"
cargo test -- --ignored --nocapture --test-threads=1
```

Measured on the first run (2026-08-26), the protocol group
`cargo test ignored_smoke -- --ignored` gives **22 passed, 1 failed in 243 s**:
streaming, anti-self-termination on EOS text, tool-call parsing, thoughts, the
sampling extensions, the typed non-transient 400 on an oversized prompt, and the
vision path all pass. The failure is `control_tools_are_callable` — the
2B-effective model did not call `rewrite_current_message`.

**Which is exactly why this is not the gate of record.** Several smokes assert on
model *behaviour* and were calibrated on Gemma 4 31B / Qwen 3.6 27B; a small
model fails some of them for reasons that are not defects. Use this for protocol,
streaming, tool-call and RAG plumbing, and keep `python tools/e2e_hf.py run` for
a verdict worth writing "Smoke — GO" about. The full `cargo test -- --ignored`
also runs here, but at CPU speed it is an hours-long affair.

## 5. Knobs

Everything is in `.env` (copied from `.env.example`, which documents each one).
The ones that come up most:

- `CHAT_GGUF` — swap the quant. `Q4_K_M` (~2.7 GiB) roughly halves the memory and
  doubles the speed; the download follows the file name automatically.
- `CHAT_EXTRA_ARGS` / `EMBED_EXTRA_ARGS` — appended verbatim to the server
  command lines. Where `--mmproj` lives, and where `-t <threads>` goes.
- `MODELS_DIR` — a bare name is the named volume, a path is a bind mount of a
  host directory of GGUFs you already have.
- `LAB_LANG` — `ru` or `en`, applied while the data volume is still empty.
- `OPENAI_API_KEY` / `GEMINI_API_KEY` / `ANTHROPIC_API_KEY` / `XAI_API_KEY` —
  passed through to the app, and the seeded settings already name them, so
  switching the engine to a cloud provider in the settings screen just works.

## 6. Things worth knowing

- **RAM.** Weights 4.63 GiB + KV cache + the projector ~0.94 GiB + the embedder
  0.6 GiB + JupyterLab: budget ~9–10 GiB for the Docker VM. Below that the chat
  server is OOM-killed while loading and it looks like a hang —
  `docker compose logs chat` says so plainly. On Windows the limit is
  `%UserProfile%\.wslconfig` (`[wsl2] memory=…`).
- **Speed.** CPU-only inference of a Q8_0 model is single-digit-to-low-teens
  tokens per second. Fine for exercising the UI, tool loops and RAG; slow for
  long generations.
- **Stored cloud keys do not survive an image rebuild.** The Linux key scheme
  derives from `/etc/machine-id`, which is generated per build. Name the
  environment variable instead (§5) — that is the documented alternative and it
  is stable here.
- **TERM is left exactly as JupyterLab sets it.** Reproducing that terminal is
  the point; normalising it would hide the very differences this environment
  exists to expose.
- **`docker compose exec lab bash`** gives the same binary in a normal terminal —
  useful for telling "broken" apart from "broken *in a browser terminal*".
- **One instance per machine — and a closed browser tab does not close the app.**
  JupyterLab's terminal is a server-side pty: closing the tab leaves `mindfork`
  running, and the next launch is refused with *"mindfork is already running on
  this machine"* (spec §1.4). Reattach from the *Running Terminals* panel in the
  left sidebar, or `docker compose exec lab pkill mindfork`.

## 7. MCP plugin tools

The image carries **Node 26** and npm, so any `npx` server from the Model
Context Protocol ecosystem runs here (docs/install.md §4.2). The reference
filesystem server is installed at build time and pre-configured, scoped to the
mounted `~/work` directory:

```jsonc
"mcp": {
  "enabled": false,                          // ← the master switch, deliberately off
  "servers": [{ "id": "fs",
                "command": "mcp-server-filesystem",
                "args": ["/home/jovyan/work"],
                "enabled": true }]
}
```

To use it: `Ctrl+P` → **Plugins** → turn the master switch on, then enable the
tools on the profile (the host is double opt-in by design — an MCP server is an
arbitrary user-privileged program, so a convenience seed is not allowed to be
what turns it on). Verified in the container: `secure-filesystem-server 0.2.0`,
protocol `2025-06-18`, 14 tools.

Addressed by its binary rather than `npx @modelcontextprotocol/server-filesystem`
on purpose — `npx` would reach the registry on every launch. For any other
server, the `npx` spelling in the install docs works as written.

An environment created before this existed keeps its own `settings.json` (the
start-up hook never overwrites one), so its Plugins section is empty. Either
`docker compose down -v` for a clean slate, or add just the server entry by hand
in `Ctrl+P` → Plugins → `Ctrl+N`.

## 8. What does not work in here, and why that is fine

- **The system clipboard.** There is no X server, so `F5` cannot reach one — and
  OSC 52 does not survive JupyterLab either. That is not a container defect, it
  is the exact situation `/export` exists for
  ([docs/history/chat-export-file.md](../docs/history/chat-export-file.md)); the
  file lands in the folder the file browser is already showing.
- **Speech (`/tts`).** No audio device. The app says "audio unavailable" and
  carries on, which is the designed behaviour.
- **The Wasmer Python sandbox** is not provisioned — `python_exec` is seeded in
  *local* mode instead, against the container's own Python 3.13. To test the
  sandbox itself: `docker compose exec lab mindfork sandbox setup` (~300 MB into
  the data volume).
