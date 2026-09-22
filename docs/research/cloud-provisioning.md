# One command from a bare GPU box to a chat — provisioning on RunPod and its kin

> **Status:** researched, decided and **implemented through stage 2**
> (2026-09-21) — stage 0 the Linux CUDA fix, stage 1 `mindfork setup`, stage 2
> `install.sh`, rehearsed GO on `v0.10.2-rc1` (§8). **Still owed: the probe on
> a rented pod** — nothing in this track has yet run on Linux under a GPU — and
> a model download as a later stage. **F1, F4
> and F5 were put to the user and chosen at their recommendation** (the user's
> decision, 2026-09-21); the rest of §6 stands at its recommendation, stated in
> the same exchange. The probe of §7 is the user's to run on a pod they rent —
> `tools/pod_probe.sh` is the checklist — and stage 0 proceeds beside it.
> Measured today: the
> llama.cpp release list (newest build `b11070`, and the 40 newest for §3.1's
> completeness count), `llama_setup::backends` fed that build's real Linux asset
> names, and a Linux `mindfork` binary in a bare `ubuntu:24.04` container.
> **Not measured: anything on a rented pod.** This session can rent nothing, so
> §3.2 is RunPod's documentation and the sources of its images, and §7 is the
> probe that turns it into measurements **before** any code is written.
>
> The gap: the app can already fetch its sandbox and its engine, each with one
> command — and then hands the user a settings screen for the four paths and
> the number that make a managed engine run. On a desktop that is done once. On
> a rented box it is done on every rental, against a meter, in a browser
> terminal.

## 1. Why, precisely

A rented GPU box is a machine that is *new every time*. On RunPod the
container's own disk is cleared whenever the pod stops (§3.2); only one
directory survives. Everything a desktop user sets up once is set up there per
rental: install the app, `mindfork sandbox setup --enable-python`,
`mindfork llama setup --backend … --set-binary`, then open the TUI and type the
model, the projector, the embedder's model and the context into *Model/server*
— each a place to get it wrong while the meter runs.

The user's sketch (2026-09-21): **one command** that installs the wasmer
sandbox, downloads llama.cpp, records the path to the main model, its `mmproj`
and the embedding model, sets the context window, switches managed mode on,
"and so on".

**Requirements.**

- **R1.** One non-interactive command line takes a machine that has only `curl`
  to a TUI that answers — with no settings screen in between.
- **R2.** **Idempotent.** The same line after a pod restart repairs what the
  container lost and downloads nothing that survived.
- **R3.** Everything that must survive lives under **one directory the user
  names** (on RunPod, the volume).
- **R4.** Settings are **written, not overridden**: what provisioning sets is
  what the settings screen shows, and what the user changes there afterwards
  sticks ([docker-jupyter-env.md](docker-jupyter-env.md) F4 — the lesson
  already paid for).
- **R5.** Every downloaded byte is verified against a digest published by its
  source ([SECURITY.md](../../SECURITY.md); R3 of
  [llama-cpp-download.md](llama-cpp-download.md)).
- **R6.** No credential is read from a variable the user did not name
  ([PRIVACY.md](../../PRIVACY.md) §3 — the reason `llama setup` stopped reading
  `GITHUB_TOKEN`).
- **R7.** Nothing RunPod-specific in the binary. The service-specific part is
  documentation and, at most, a template.
- **R8.** Nothing changes for a desktop user.

## 2. What exists (inventory)

### 2.1 The pieces that already provision

| Piece | Where | What it already guarantees |
|---|---|---|
| `mindfork sandbox setup [--enable-python]` | `main.rs:815`, `features/sandbox_setup.rs` | lock list + sha256; a re-run fetches only what is missing; the tool is switched on **after** success, never without its assets (ADR 0005 §5). ~206 MB down, ~1.7 GB on disk |
| `mindfork llama setup --backend <id> [--build] [--set-binary]` | `main.rs` `run_llama_setup`, `features/llama_setup.rs` | sha256 from the release's own `digest`; resumable; one directory per build (`data/llama/<backend>-<tag>/`); an existing install is probed, not re-fetched; `--version` and `--list-devices` run afterwards |
| an **empty** binary field | `llama_setup::resolve_binary` | resolves to the newest install under `data/llama/` — after `llama setup` no binary path needs writing at all |
| the CLI's settings write | `open_config_for_cli_write` (`main.rs:1036`) + `JsonStore::save_config` + `acquire_cli_guard` (`main.rs:449`) | ADR 0006 downgrade guard, language seeding on a fresh root, `.bak` → `.tmp` → rename at `0600`, single instance |
| a partial `settings.json` | `AppConfig` is `#[serde(default)]` throughout | naming three fields is a legal file |
| `defaults.json` next to the binary | `shared/paths.rs` | `portable` (the default) keeps `data/` **beside the binary** — on a pod that *is* R3 |
| `backup` / `restore` / `stats --compare` | spec §12.3–§12.4 | carrying the user's data to the box, and knowing afterwards which copy is the newer |

### 2.2 What cannot be set without the settings screen

| The sketch says | Field | The only other route today |
|---|---|---|
| main model | `engine.managed.model_path` | `MINDFORK_MODEL` |
| `mmproj` | `engine.managed.mmproj` | `MINDFORK_MMPROJ` |
| embedding model | `embed.managed.model_path` **and** `embed.mode` | `MINDFORK_EMBED_MODEL` |
| context window | `engine.managed.context_size` (default 8192) | `MINDFORK_CTX` |
| managed mode on | `engine.mode` (already the default) | implied by `MINDFORK_LLAMA_BIN` |
| "and so on" | `gpu_layers`, `sessions`, `flash_attn`, `no_mmap`, `tools.*` … | `MINDFORK_NGL`, otherwise none |

The environment route fails R4 and is awkward besides:

1. **It overrides, on every launch, in memory** (`apply_env_overrides`,
   `main.rs:1183`) — and becomes permanent by accident: the orchestrator saves
   its whole in-memory config on any settings edit, so an overridden value is
   written back the first time the user changes anything. The lab's start-up
   hook says why it seeds a file instead (`docker/lab/before-notebook.d/10-mindfork`).
2. **The managed branch is keyed on `MINDFORK_LLAMA_BIN`**: `MINDFORK_MODEL`,
   `MINDFORK_MMPROJ`, `MINDFORK_CTX` are read only inside it
   (`main.rs:1193-1198`). That defeats §2.1's empty-binary resolution — to say
   "use this model" one must also spell
   `data/llama/cuda-12.8-b11070/llama-server`. The embedder has the same shape.

### 2.3 The container precedent — `docker/lab`

A test rig, not a deployment (it builds from source and configures *external*
mode), but every trap it documents applies here: seed `settings.json` **only
when absent**; `libasound2t64`, not `libasound2`; an empty `/etc/machine-id`
makes key storage refuse; and `docker/models/fetch.sh` is a finished resumable,
size-checked Hugging Face downloader that keeps the token off `argv`.

## 3. What was measured (2026-09-21)

### 3.1 Upstream now ships CUDA for Linux — and `llama setup` refuses it

[llama-cpp-download.md](llama-cpp-download.md) §3.7 recorded, on 2026-09-09,
that Linux "has no CUDA archive at all". **That stopped being true five days
later**: upstream PR #28186 (merged 2026-09-14) added them, first seen in
`b10969`. `b11070` (2026-09-21), Linux rows, every one with a `digest`:

```
llama-b11070-bin-ubuntu-x64.tar.gz                    16.9 MB   cpu
llama-b11070-bin-ubuntu-vulkan-x64.tar.gz             30.4 MB
llama-b11070-bin-ubuntu-cuda-12.8-x64.tar.gz         168.8 MB   ┐ new
llama-b11070-bin-ubuntu-cuda-13.3-x64.tar.gz         149.1 MB   │
cudart-llama-b11070-bin-ubuntu-cuda-12.8-x64.tar.gz  594.4 MB   │
cudart-llama-b11070-bin-ubuntu-cuda-13.3-x64.tar.gz  410.2 MB   ┘
cudart-llama-bin-win-cuda-12.4-x64.zip               391.4 MB   ← Windows, for contrast
```

**The Linux runtime archive is named differently from the Windows one**: it
carries the build tag and is a `.tar.gz`. `cudart_name`
(`llama_setup.rs:255`) formats the Windows shape for every OS —
`cudart-llama-bin-{os}-{backend}-{arch}.zip`. Fed the real `b11070` names
through `backends(&release, "linux", "x86_64")` (a scratch test, removed):

```
id=cuda-12.8 asset=llama-b11070-bin-ubuntu-cuda-12.8-x64.tar.gz cudart=None missing=true
             wanted=cudart-llama-bin-ubuntu-cuda-12.8-x64.zip
id=cuda-13.3 … cudart=None missing=true
```

So on Linux, as of 0.10.2, `mindfork llama backends` lists `cuda-12.8` with the
"no CUDA runtime" note, and `llama setup --backend cuda-12.8` **bails** with
`llamacpp.setup.cudart_missing`, naming a file upstream has never published.
`--no-cudart` gets past it and then depends on the host's own CUDA libraries.
This is a defect on its own, provisioning or not, and it is the same lesson as
§3.3 of that document: **derive, do not format** — the runtime is "the asset
that starts with `cudart-` and ends with `-bin-<os>-<backend>-<arch>.<ext>`",
which holds for both shapes.

Three more facts from the same listing:

- **Not every nightly is complete.** Of the 40 newest builds: 37 carry the full
  Linux CUDA set, **2 have no assets at all** (`b11027`, `b11028`) and **1 is
  partial** (`b11030`: 7 assets, runtimes without their builds, no CPU build).
  `fetch_release` already skips a build with nothing for the platform; a partial
  one that has *some* backend but not the one named would answer "unknown
  backend". With a backend named, the scan should look for **that backend**.
- **The CUDA minor in the name moves.** Windows went `cuda-13.3` → `cuda-13.4`
  between `b10883` and `b11070` — twelve days. A saved one-liner that says
  `cuda-12.8` has a shelf life (fork F9).
- **The build image is `nvidia/cuda:…-devel-ubuntu24.04`** (upstream's
  `release.yml`), so the CUDA archives most likely need **glibc 2.39** — an
  inference from the image, not a measurement. mindfork's own floor is 2.35.
  Upstream's default CUDA architecture list makes 8.6/8.9/12.0 native and
  leaves 8.0 (A100) and 9.0 (H100/H200) to PTX, i.e. a **JIT compile on first
  launch** — also an inference, and §7 measures both.

With CUDA published, the alternatives are moot and recorded only so they are
not re-derived: **Vulkan** in a container needs the `graphics` driver
capability plus `libglvnd`/ICD files and no report of it working on RunPod or
Vast was found; **building from source** works on the official images (they
are `-devel`, with `nvcc`, `cmake`, `git`) at an unmeasured 15–30 minutes;
**the upstream Docker image** (`ghcr.io/ggml-org/llama.cpp:server-cuda`,
binary in `/app`) is the base a template would use (F8).

### 3.2 The host — what a RunPod pod is (documentation, not measurement)

- **A template** is: container image, a start command that overrides `CMD`,
  environment variables, HTTP/TCP ports, container-disk size, volume size and
  its mount path.
- **Storage.** The container disk is **cleared when the pod stops** — not only
  when it is terminated. The volume, mounted at `/workspace`, survives
  stop/start and is deleted on terminate. A network volume is independent of
  any pod, mounts at `/workspace` too, and pins the pod to one datacenter.
- **The official images** (`runpod/pytorch:*`, `runpod/base:*`) are
  `nvidia/cuda:*-cudnn-devel-ubuntu{22.04,24.04}`, run as **root**, and carry
  `curl`, `tmux`, `unzip`, `git`, `cmake`, `build-essential`, `openssh-server`.
  Their `/start.sh` starts sshd when `PUBLIC_KEY` is set and then sleeps. They
  preset `HF_HOME=/workspace/.cache/huggingface`.
- **A terminal, three ways.** The web terminal (documented as "not recommended
  for long-running processes"); *basic* SSH through `ssh.runpod.io` — no
  SCP/SFTP, and by one field report PTY-only, which suits a TUI; *full* SSH on
  an exposed TCP port 22.
- **The meter** does not stop by itself: no idle auto-stop, and a container
  that exits keeps billing ([remote-e2e-gpu.md](remote-e2e-gpu.md) §4, verified
  then).
- **Serverless** runs queue-fed handlers with no terminal — no place for a TUI.

### 3.3 What a container takes away (measured locally)

A Linux `mindfork` binary, copied out of the lab image into a bare
`ubuntu:24.04`:

```
$ /probe/mindfork --version
error while loading shared libraries: libasound.so.2: cannot open shared object file
exit=127
$ wc -c < /etc/machine-id
0
```

- **`libasound.so.2` is a load-time dependency.** The `.deb` declares it; the
  portable `tar.gz` — the one that fits R3 — cannot, and on RunPod the package
  that satisfies it lives on the disk that is cleared at every stop. So R2 is
  not a nicety: *something* must put it back after each restart (F6).
- **`/etc/machine-id` is empty**, so `shared::secrets` refuses to store an API
  key ([docker-jupyter-env.md](docker-jupyter-env.md) §3). On a pod the durable
  route is the one that already exists: `api_key_env` naming a variable the
  platform injects. Seeding a machine id from the volume would put the key
  material next to the ciphertext and undo what
  [ADR 0008](../decisions/0008-api-key-storage.md) binds — not proposed. For
  the same reason the encrypted keys inside a backup restored from home do not
  decrypt there; that is the design working.

### 3.4 Getting the models there

The sketch says *record the path*, and that is route 1. The others, so the
choice is explicit (F4):

1. **The user brings paths** — a network volume that already holds the GGUFs,
   or their own download. No new network destination, no token.
2. **The bootstrap script downloads** — `fetch.sh`'s loop (`curl -fL -C -
   --http1.1 --retry-all-errors`, size-checked, `.part` + rename). Shell,
   untested by `cargo test`, and the token question lands in a script.
3. **The app downloads** — `--model hf:<repo>/<file>` into `data/models/`,
   reusing `llama_setup`'s resumable fetch. The Hub publishes a sha256 per LFS
   file, which would satisfy R5 — **unverified today**, to be measured by that
   stage. Costs: a new destination in PRIVACY.md, a token the user must *name*
   (R6), and `data/models/` joining `data/llama/` on the lists of what a backup
   skips and a restore leaves alone.
4. **`llama-server -hf`** — rejected. The robustness track refuses a model-less
   managed server precisely because it can fetch from Hugging Face by itself
   (`ManagedConfig::is_runnable`); the app needs the path for its preflight,
   the split-GGUF check and the model caption; and the cache layout is
   upstream's to change.

### 3.5 The neighbours

**Vast.ai**: Docker too, but in its SSH/Jupyter launch modes the image's
entrypoint is *not called* — start-up logic goes into an on-start script; SSH
lands in tmux by default. **Lambda**, **TensorDock**, **DigitalOcean GPU
droplets**: full VMs, cloud-init user data as root on first boot, nothing
cleared on reboot. All of them run "a shell line at start", which is why R7
costs nothing: one script and one subcommand serve every one.

## 4. Design

### 4.1 Three layers

```
install.sh        puts mindfork on the machine (the step no subcommand can do),
   │              repairs what the container lost, then hands over
   ▼
mindfork setup    sandbox + llama.cpp + settings, in Rust, tested, localized
   │
   ▼
a template        optional: a start command that runs the line above
```

The middle layer is the feature. The first is thin because it must exist
*before* the binary does; the third is documentation until someone wants more.

### 4.2 `mindfork setup`

The composite of the two `setup`s that exist, plus the settings half that does
not (the noun is F2):

```
mindfork setup [OPTIONS]
  --sandbox                 as `sandbox setup --enable-python`
  --llama <BACKEND>         as `llama setup --backend <BACKEND>`   (+ --llama-build <TAG>)
  --model <GGUF>            engine.managed.model_path, and engine.mode = managed
  --mmproj <GGUF>           engine.managed.mmproj
  --embed-model <GGUF>      embed.managed.model_path, and embed.mode = managed
  --ctx <N>                 engine.managed.context_size
  --ngl <N>                 engine.managed.gpu_layers
  --set <KEY>=<VALUE>       any other settings field; repeatable            (F3)
  --verify                  start what was configured, report, stop         (F7)
```

- **Every option is optional and each names one step**; a step not named is not
  run. `mindfork setup --ctx 65536` alone is a legitimate use.
- **Order: downloads first, settings last, one guard for the lot** — the
  existing "a failed install writes nothing" rule, per step.
- **Paths are checked before anything is written**, with the supervisor's own
  preflight (model stat, `check_split_model`, projector stat): a typo is an
  error now, not a status-bar line later.
- **Naming a model is asking for managed mode.** Unlike `--set-binary`, which
  leaves the mode alone, `--model` sets `engine.mode = managed` and says so —
  a data root restored from home may well arrive in `claude` mode.
- **No binary path is written**: the empty field already resolves to the build
  just installed (§2.1), and stays correct when a newer one is installed.
- **A failed step does not stop the rest** (F10): the settings of the steps
  that succeeded are written, a per-step summary is printed, the exit code is
  non-zero. At an hourly rate a user would rather chat without Python than not
  chat; the re-run (R2) repairs the rest.

`--set` is the sketch's "and so on". The key is a dotted path into the
serialized `AppConfig`; the value is a JSON literal when it parses as one,
otherwise a string. The merged document is deserialized and serialized again,
and **the key must survive the round trip with the value given** — that one
check catches a typo (a `#[serde(default)]` struct drops an unknown key in
silence) and a wrong type alike. `schema_version` and `api_keys` are refused.

FSD: the plan, the `--set` logic and the config mutation are a pure
`features/provision.rs`; the handler in `main.rs` composes it with
`sandbox_setup::setup` and `llama_setup::setup`; the parser grows in
`features/cli.rs`; every line from the locale bundles, `en` and `ru`.

### 4.3 `--verify`

Launch the managed chat server (and the embedder, if configured) through
`ServerHandle::launch` + `wait_until_ready`, print what `/props` reports —
context, vision, slots — and the time to ready, then stop them. A command that
says "done" and a TUI that then says "server failed" is the exact failure this
track exists to remove; on a pod it is also where an out-of-memory context or a
first-launch JIT shows up *before* the user is looking at a chat. It doubles as
the track's live smoke: the managed-server path is one of the two the rented
gate cannot cover ([install.md](../install.md) §7.2).

### 4.4 `install.sh`

POSIX `sh`, idempotent, no `jq`, no Python:

1. Refuse anything but Linux x86_64 with glibc ≥ 2.35, in words.
2. Resolve the version from the **redirect** of `releases/latest` (no API call,
   so no 60-per-hour limit on a datacenter's shared address), or `--version`.
3. Download the archive and `sha256sums.txt` into `--dir`, verify, unpack
   (the Linux archive is flat — `release.yml` packs `stage-linux/` with
   `tar -C … .` — so the directory is ours to name). Skipped when that version
   is already there.
4. `libasound.so.2` absent → as root with `apt-get`: install `libasound2t64`,
   falling back to `libasound2`; otherwise print the line to run (F6).
5. As root, link `/usr/local/bin/mindfork` (container disk — hence every run).
6. Arguments after `--` go to `mindfork` verbatim.

After a pod restart the same line costs seconds: steps 3 and the downloads of
`setup` find everything present; steps 4–5 put back what the container lost.

### 4.5 What the user types on RunPod

```bash
curl -fsSL https://github.com/vshylov/mindfork-rs/releases/latest/download/install.sh \
  | sh -s -- --dir /workspace/mindfork -- setup \
      --sandbox --llama cuda-12 \
      --model /workspace/models/gemma-4-31b-it-q4_0.gguf \
      --mmproj /workspace/models/mmproj-gemma-4-31b-it-f16.gguf \
      --embed-model /workspace/models/bge-m3-Q8_0.gguf \
      --ctx 32768 --verify
tmux new -A -s mindfork mindfork
```

**tmux is not decoration here.** The managed `llama-server` is a
`kill_on_drop` child of the TUI: a dropped SSH session takes the TUI down, the
TUI takes the server down, and reconnecting means loading twenty gigabytes
again. The same line as a template's start command (followed by `/start.sh`)
makes a restart need no typing at all.

## 5. Difficult spots

1. **The container disk is cleared at every stop** (§3.2–§3.3). Anything the
   bootstrap puts outside `--dir` is put back on every run, by construction.
2. **glibc 2.39 for the CUDA archives** (inferred) — a `*-ubuntu2404` image.
   The post-install `--version` run already turns a loader failure into a
   message; it should name the image family.
3. **First-launch JIT on A100/H100** against `MANAGED_READY_TIMEOUT` (600 s),
   with the JIT cache on the disk that is cleared. `CUDA_CACHE_PATH` on the
   volume is the likely answer; §7 measures.
4. **GitHub's 60 requests an hour, per address, in a datacenter.** `llama
   setup` costs one. If the probe shows it biting, the R6-compatible escape is
   an option that *names* the variable (`--token-env <NAME>`), never an
   implicit read.
5. **A network volume is not a local disk.** Mapping a 20 GB GGUF from it may
   crawl; `engine.managed.no_mmap` exists for exactly this. Measure, then
   document or default.
6. **Someone else's disk.** Chats sit unencrypted in `data/`; on a community
   host that is a stranger's machine, and terminate deletes the volume. The
   recipe: `mindfork backup -p` before terminate, `mindfork stats --compare`
   at home. Documentation, not code.
7. **The meter never stops by itself.** One sentence in the recipe, in bold.
8. **A third CLI path into user data**, after `--enable-python` and
   `--set-binary`: it takes both precautions or none.
9. **The browser terminal is xterm.js** — `Ctrl+N`/`Ctrl+T` never arrive, OSC
   52 is dropped ([command-only-control.md](../history/command-only-control.md)).
   Known, handled, worth a line.
10. **`curl | sh`.** The script is a release asset listed in `sha256sums.txt`,
    documented both ways: piped, and download–read–run.

## 6. Forks

> **Resolved 2026-09-21.** F1, F4 and F5 — the three that decide what gets
> built and published — were put to the user and **chosen at their
> recommendation**, as was running the probe (§7) on a pod of the user's own
> with stage 0 proceeding beside it, which settles F11. F2, F3 and F6–F10 were
> stated with their recommendations in the same exchange and stand at them
> unless contested.

- **F1 — the shape of "one command".** (a) a composite subcommand plus a thin
  bootstrap script; (b) a shell script that chains today's commands and edits
  `settings.json` itself; (c) a generic `mindfork config set` and a shell
  chain; (d) a Docker image / RunPod template only.
  → **Recommendation: (a).** (b) puts the delicate half — a migration-guarded,
  atomic write into user data — into `sed`; (c) is not one command; (d) serves
  one service, adds an image to build, scan and keep current, and still needs
  (a)'s logic inside it. *(Chosen by the user, 2026-09-21.)*
- **F2 — the noun.** `mindfork setup` · `mindfork provision` · `mindfork init`.
  → **Recommendation: `setup`** — it is the composite of `sandbox setup` and
  `llama setup`, and reads as such.
- **F3 — "and so on".** (a) typed flags for the five named settings plus
  `--set key=value`; (b) typed flags only; (c) also `--settings <partial.json>`
  merged over the config.
  → **Recommendation: (a).** Typed flags carry validation and help text where
  it matters; `--set` makes the rest reachable without a flag per field. (c)
  when a template first needs it.
- **F4 — models.** (a) paths only now, an in-app download as a later stage
  with its own forks; (b) the download in the first stage; (c) never — a
  documented `curl` loop.
  → **Recommendation: (a).** Paths are what was asked; the download brings a
  new network destination, a token-naming rule and backup/restore list changes
  (§3.4) — a stage, not a flag. *(Chosen by the user, 2026-09-21.)*
- **F5 — where `install.sh` lives.** (a) a release asset, in `sha256sums.txt`;
  (b) `mindfork.io/install.sh`; (c) no script — six documented lines.
  → **Recommendation: (a).** It adds **no new root of trust**: whoever can
  publish a release can already ship the binary. (b) makes the site's bucket a
  code-distribution channel. Cost of (a): `release.yml` changes, so an `-rc1`
  rehearsal (AGENTS.md §6). *(Chosen by the user, 2026-09-21.)*
- **F6 — `libasound`.** (a) the bootstrap installs it when root; (b) ship the
  `.so` beside the binary with an `$ORIGIN` rpath; (c) a second, audio-less
  Linux artifact.
  → **Recommendation: (a).** (b) and (c) each change what every Linux user
  downloads to fix what a container lacks.
- **F7 — `--verify`.** (a) in the first stage; (b) later; (c) never.
  → **Recommendation: (a)** — §4.3: it is the difference between "configured"
  and "works", and it is the live smoke.
- **F8 — a RunPod template / image of our own.** (a) a documented recipe
  (install.md) and the start-command line; (b) a published template; (c) a
  published image.
  → **Recommendation: (a)**, (b) once the recipe has survived real use.
- **F9 — backend families.** (a) `--llama cuda-12` resolves to the one
  `cuda-12.*` the build has, refusing an ambiguity; (b) exact ids only.
  → **Recommendation: (a)** — §3.1: the minor moved in twelve days, and a line
  saved in a template should outlive that. Applies to `llama setup` too.
- **F10 — a failed step.** (a) continue, write what succeeded, exit non-zero;
  (b) stop at the first failure and write nothing.
  → **Recommendation: (a)** — §4.2.
- **F11 — the Linux CUDA fix.** (a) its own `fix/` PR first — it is a defect
  today, for any Linux user with an NVIDIA card; (b) folded into stage 1.
  → **Recommendation: (a).** *(Settled by the user's choice of how the probe
  runs, 2026-09-21: stage 0 starts now.)*

## 7. The probe, tests and the live run

**The probe comes first** (AGENTS.md §1: an MVP probe with a go/no-go). It
needs one rented hour on a `*-ubuntu2404` RunPod image (an A40 or a 4090 is
under a dollar) and uses **only what is released today**. `tools/pod_probe.sh`
runs P1–P5, P7 and P8 and writes one report to paste back; P6 is a person
looking at `mindfork demo`, which needs no model. The JIT half of P4 exists
only on a compute-capability 8.0 or 9.0 card (A100, H100/H200), so a cheap
A40/4090 hour answers everything else and leaves that one open:

| # | Question | How |
|---|---|---|
| P1 | does the 0.10.2 archive start after one `apt-get` line? | unpack into `/workspace`, `mindfork --version` |
| P2 | does GitHub's API answer from the pod's address? | `mindfork llama backends`; the `X-RateLimit-Remaining` header |
| P3 | does the official CUDA build see the GPU? | `llama setup --backend cuda-12.8 --no-cudart` (the workaround for §3.1), then its `--list-devices`; then again with the runtime archive unpacked beside it |
| P4 | glibc, and the JIT | `ldd --version`; time to `/health` on a first and a second launch; repeat with `CUDA_CACHE_PATH` on the volume |
| P5 | the sandbox in this container | `sandbox setup --enable-python`, wall time, then one `python_exec` call |
| P6 | the TUI | web terminal, basic SSH, full SSH — each bare and under tmux |
| P7 | the volume | model load with and without `no_mmap`, local volume vs network volume |
| P8 | `/etc/machine-id` | `wc -c`; whether it changes across a stop/start |

**GO** when P1, P3, P5 hold and P6 has at least one usable terminal. **NO-GO
on P3** sends stage 0 back to §3.1's alternatives (source build, or the
upstream image as a template base) before anything else is designed.

**What the README's line answered, run on a RunPod pod (2026-09-22, an RTX
PRO 6000 Blackwell, 96 GB, a 31B Q8_0 at `--ctx 131072`):** P1 — the archive
starts once the script has installed `libasound2t64` itself (and, the day
before, does *not* unpack as root without `CAP_CHOWN`: the one defect the
track shipped with, fixed in 0.11.1). P2 — the API answered; `llama setup`
resolved `cuda-12` to `cuda-12.8` of `b11101`. **P3 — GO**: the official CUDA
build with its runtime archive, `devices: CUDA0: NVIDIA RTX PRO 6000 Blackwell
Server Edition (97251 MiB, 96693 MiB free)`. P4 — compute capability 12.0 is
in upstream's native list, so no JIT to measure on this card; the server was
`ready in 4 s — context 131072, text only, 4 slots` (a model in the page cache
from the previous run). **P5 — GO**: 245 MB of wasmer, Python and every
package, the cache warmed, the image packed and started, `tools.python_enabled
= true` written. P6 — the owner drove it from the pod's terminal; the TUI
itself was not the subject. P7 and P8 — not measured: the models were on the
pod's own volume, and `machine-id` was not read. **The probe's go/no-go is
GO**; what `pod_probe.sh` would still add is the JIT on an 8.0/9.0 card, a
network volume's read speed, and `machine-id` across a stop.

**Unit.** The `b11070` names as a fixture next to `B10883`: Linux CUDA pairing,
the tagged `.tar.gz` runtime, a partial release, family resolution and its
ambiguity refusal. For `setup`: parser cases in both option spellings; the
`--set` round trip (unknown key, wrong type, refused keys, JSON literal vs
string); the mode switch; per-step failure isolation; help rendering in every
language without placeholders.

**Live (mandatory — this touches the engine).** `--verify` against the local
stack on Windows, and the whole one-liner on a rented pod, recorded in
[journal/engine.md](../journal/engine.md) in the "Smoke — GO" shape.

## 8. Stages

0. **`fix/llama-linux-cudart`** — derive the runtime's name; look for the named
   backend when scanning; families (F9). Corrects
   [llama-cpp-download.md](llama-cpp-download.md) §3.7 and
   [install.md](../install.md) §3.1. **Done 2026-09-21**, and its live smoke
   found a second defect on the same seam: `b11070` logs a line ahead of its
   `--version`, which had made the build-number check skip itself in silence
   ([journal/engine.md](../journal/engine.md)). Until a release carries the fix,
   the *released* binary still refuses CUDA on Linux — which is what P3 of the
   probe expects to see, and why it goes on with `--no-cudart`.
1. **`feat/setup-command`** — `mindfork setup` with paths, `--set`, `--verify`.
   **Done 2026-09-21** ([journal/engine.md](../journal/engine.md)), live **GO**
   on Windows/CPU. Two decisions were made in the building and are recorded
   in spec §3.4: `--model` and `--embed-model` **switch the mode** to managed
   and say which one they replaced; and although no binary path is written, a
   managed binary path that names **no file** is cleared after a successful
   `--llama` — the path a data root restored from another machine brings.
   `--verify` also refuses a busy port instead of probing it: a readiness
   probe cannot tell our server from whatever already answers there.
2. **`feat/install-script`** — `install.sh`, the release workflow, the rehearsal;
   install.md gains "On a rented GPU box" with the RunPod recipe.
   **Implemented 2026-09-21** ([journal/release.md](../journal/release.md)):
   the script, its 25 scenarios, five bare images in `packaging.yml`, the
   release job installing its own archive, install.md §1 and §3.4. `--from
   DIR` was added in the building — an offline install, and the seam that
   makes the script testable without a network. **Rehearsed GO** on
   `v0.10.2-rc1` the same day: the script among the draft's assets, first in
   its `sha256sums.txt`, covered by the build attestation, and the draft's own
   archive installed by it. **The first real pod run (2026-09-22) failed inside
   `tar`** — root without `CAP_CHOWN` refusing to restore the archive's recorded
   owner — a shape no container of ours had; fixed with `--no-same-owner` and a
   `capsh --drop=cap_chown` scenario with its control arm
   ([journal/release.md](../journal/release.md)). With the archive unpacked by
   hand, **the rest of the line ran on that pod**: `setup --sandbox --llama
   cuda-12 --model <31B Q8_0> --ctx 131072 --verify` went through — the first
   run of any of this on Linux under a GPU, and the track's acceptance test
   passed. **0.11.1** (2026-09-22) ships the fix, and the same day **the
   README's line ran on the pod as published, end to end** — nothing typed
   but the line: `ready in 4 s — context 131072, text only, 4 slots` (§7).
   The track's acceptance test is passed. Left for `pod_probe.sh`, when a
   pod is up anyway: the JIT on an 8.0/9.0 card, a network volume's read
   speed, `machine-id` across a stop.
3. *(if F4 goes that way)* **model download** — its own research section first.

The probe runs before stage 0 is merged; stage 1 does not depend on it.

## 9. Not in this track (recorded so they are not re-derived)

- **GPU detection / choosing a backend for the user** — still F3 of
  [llama-cpp-download.md](llama-cpp-download.md): the choice is the user's.
- **A first-run wizard in the TUI.** `setup` is what one would call; the screen
  is a UI track.
- **An embedder context setting.** `supervisor.rs` hardcodes
  `DEFAULT_CONTEXT_SIZE`; nothing in the sketch asks for it.
- **`extra_args` in the settings.** The launcher has the field and every caller
  passes an empty list; tempting, separate, and a security review of its own
  (a free-form argv into a child process).
- **Adjacent finding: `MINDFORK_MODEL` is ignored without
  `MINDFORK_LLAMA_BIN`** (§2.2), although an empty binary has resolved by
  itself since the download track. A small fix in its own PR, or a line in
  install.md.
- **Exposing the pod's `llama-server` to the home machine.** The opposite
  topology — app at home, engine rented — already works as `external` over an
  SSH tunnel ([remote-e2e-gpu.md](remote-e2e-gpu.md) §5).
- **Vulkan in containers, source builds, a CUDA build of our own** — §3.1.

## 10. Documentation touch list (AGENTS.md §4)

- [install.md](../install.md): §3.1 (Linux CUDA exists; families), a new "On a
  rented GPU box" section, §4.1 (the composite), the env quick-start's caveat.
- [spec.md](../../spec.md) §12 (the CLI surface) and §3.4 (what `setup` writes).
- [architecture.md](../architecture.md) §3 (`features/provision.rs`), §12 (the
  release asset).
- README (commands), the CLI help in `en`/`ru`, CHANGELOG (Added; Fixed for
  stage 0).
- [PRIVACY.md](../../PRIVACY.md) — only if F4 brings a download.
- [journal/engine.md](../journal/engine.md) (stages 0–1),
  [journal/release.md](../journal/release.md) (stage 2);
  [lessons.md](../lessons.md): *an upstream fact measured once has a date on it*
  — §3.7 of the download research was true for five days.
