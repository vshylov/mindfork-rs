# Research: running the live e2e smokes on a rented GPU

**Status:** **decided and shipped** — the track closed 2026-07-29; the plan it
produced is archived at [docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md).
Note that two of the conclusions below were **overturned by the stage-0 probe**
and are marked inline: embeddings are served by llama.cpp itself rather than TEI
(§3.1), and the endpoint type is `authenticated`, not `protected` (§5).
Forks R1–R8 (§9) resolved to the recommended option
throughout — *user's decision, 2026-07-28*: **R1a** HF Inference Endpoints ·
**R2a** `protected` + the `MINDFORK_ENGINE_KEY` enabling change · **R3**
reduced form (`delete` + sweeper) · **R4a** L40S 48 GB · **R5a**
`workflow_dispatch` only · **R6a** the llama.cpp smoke set · **R7** n/a ·
**R8** `HF_TOKEN` as a plain repository secret (the `workflow_dispatch`-only
trigger already restricts the live job to users with write access, so a gated
environment adds no protection).
The unpinned llama.cpp `master` build (§3.1) was accepted knowingly — the user
builds `master` regularly and sees breakage very rarely.
Implementation plan: **[docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md)**.
**Date:** 2026-07-28.

## 1. Why

`cargo test` (1484 unit tests) runs anywhere. The **69 `#[ignore]` smokes** do
not: the ones that matter most need a live `llama-server` with a real model, and
today they run only on the developer's machine against a fixed LAN address
(`run_all_tests.bat` → `http://192.168.1.20:8000/v1`). Consequences:

- the live gate is **not reproducible** by anyone else and cannot run in CI;
- AGENTS.md §3 makes a live run **mandatory** for anything touching engine /
  memory / tools, so the whole workflow depends on one machine being up;
- a regression in the llama.cpp path can only be caught by hand.

The question: can a GPU be rented per-run (RunPod or similar), driven from CI or
from a script, and — the part that actually decides the design — **can we
guarantee the GPU is released even when the run crashes**.

## 2. What the live suite actually needs (facts from this repo)

| Fact | Where | Consequence |
|---|---|---|
| Smokes read `MINDFORK_ENGINE_URL` and `MINDFORK_EMBED_URL` (`…/v1`) | `orchestrator/tests/mod.rs:173-185`, `shared/api/openai/client.rs` | The runner only needs two URLs; no code change to point them elsewhere |
| `live_backend()` builds `OpenAiClient::new(url)` — **no API key** | `orchestrator/tests/mod.rs:176` | **The harness cannot send `Authorization: Bearer`.** Protecting the endpoint with `llama-server --api-key` needs a code change (a `MINDFORK_ENGINE_KEY` env var), or an approach that needs no header at all — see §5 |
| Two servers, different flags: chat `--jinja`, embeddings `--embeddings -ub <ctx> -b <ctx>` | CLAUDE.md §Commands, "raised the physical batch" entry | One pod can host both; the embedding server's batch flags are load-bearing (without them large chunks fail) |
| Some smokes want a **second, different** embedding model (`MINDFORK_EMBED_URL_ALT`) | `embed_guard.rs`, `reembed.rs`, `embed_prefix.rs` | The model-change / calibration / prefix smokes need e5-large-instruct as well — a third server, ~1 GB more |
| ~55 of the 69 smokes are llama.cpp-relevant (44 in `tests/live.rs` + engine/supervisor/embed) | `grep -c '#\[ignore'` | The rest need cloud keys or the Python sandbox and are out of scope here |
| Measured wall clock: the orchestrator e2e set is **~500–720 s** | CLAUDE.md live-run entries | Budget ~15–20 min of test time |
| The smokes are **not** hermetic: `web_search`/`fetch_url` hit the real internet | `features/tools/web.rs`, `fetch.rs` | Expect occasional flakes independent of the GPU |

**The models** (both verified via the HF API on 2026-07-28 — **neither is
gated**, `"gated": false`):

| Repo | File | Size |
|---|---|---|
| `google/gemma-4-31B-it-qat-q4_0-gguf` | `gemma-4-31B_q4_0-it.gguf` | **17.65 GB** |
| (same repo) | `gemma-4-31B-it-mmproj.gguf` | 1.2 GB — vision projector, **not needed** |
| `ggml-org/bge-m3-Q8_0-GGUF` | `bge-m3-q8_0.gguf` | ~0.6 GB |

> Correction to the premise in the request: **a Hugging Face token is not
> required to download them** — neither repo is gated. For a rented pod a token
> is a convenience (anonymous downloads are rate-limited), not a gate. It becomes
> *required* only if HF Inference Endpoints is chosen (§3.1), where it is the
> management and auth credential rather than a download key.

**VRAM.** 17.65 GB of weights + KV cache for 16k ctx + compute buffers on a 24 GB
card is tight but probably fits; adding bge-m3 (~1 GB resident) and a second
embedder makes it tighter. This is worth noting because of a pricing accident:

| GPU | VRAM | RunPod Community $/hr |
|---|---|---|
| RTX 4090 | 24 GB | ~0.69 |
| **A40** | **48 GB** | **~0.44** |
| RTX A6000 | 48 GB | ~0.49–0.53 |
| L40S | 48 GB | ~0.99 |

A 48 GB A40 is both **roomier and cheaper** than a 24 GB 4090. There is no reason
to squeeze into 24 GB. (Prices move; treat as of 2026-07.)

## 3. Candidate platforms

| Option | llama.cpp fidelity | Cleanup guarantee | Infra we own | Cost per ~30 min run | Verdict |
|---|---|---|---|---|---|
| **HF Inference Endpoints** | **first-class** — HF's own llama.cpp engine | **platform-bounded** — auto scale-to-zero, `pause`/`delete` API | almost none | ~$0.90 (L40S 48 GB) | **Recommended** — see §3.1 |
| **RunPod Pods** | full (any container) | **none native** — must be built (§6) | pod bootstrap, SSH, downloads, switch | ~$0.22 (A40 48 GB) | Cheapest; you own the cleanup logic |
| Modal | full (official llama.cpp example) | platform-enforced (`scaledown_window`, `timeout`) | a Python app file | ~$1.00 (L40S) | Good guarantee, more code than HF |
| Google Cloud Run + GPU | full (any container) | platform-enforced (scale-to-zero, per-second) | GCP project, Artifact Registry, IAM, image with the model | ~$0.35 (L4 24 GB) | Works, but the most setup; L4 24 GB only |
| RunPod / other Serverless | ✗ — stock workers are vLLM; llama.cpp needs a custom worker | platform-enforced | a custom worker | low | Loses the point (see below) |
| Vast.ai | full | none native, plus interruptible hosts | same as RunPod | ~$0.10–0.18 | Cheapest, least reliable |
| Lambda / Hyperstack / bare VMs | full | none native | most | higher | No advantage over RunPod here |
| Cloud APIs (OpenAI/Gemini/Claude) | ✗ | n/a | none | n/a | Already covered by separate key-gated smokes |
| **Self-hosted GitHub runner on the dev box** | full | **n/a — nothing is rented** | a runner service | **$0** | Simplest of all; see §8 |

**Why llama.cpp fidelity is non-negotiable:** the smokes exercise llama.cpp
request-body extensions (`dynatemp_*`, `dry_*`, `xtc_*`, `mirostat`, `samplers`),
`--jinja` tool calling, `reasoning_format`/`reasoning_budget` "thoughts", the
`/health` readiness probe and its `503 Loading model`, and the `-ub/-b` embedding
batch behaviour. Any vLLM-based serverless endpoint would exercise none of it.

### 3.1 Hugging Face Inference Endpoints — llama.cpp as a managed service

This is the "more convenient service" the survey was looking for, and it changes
the recommendation. **HF Inference Endpoints has a first-class llama.cpp
engine**: point an endpoint at a GGUF repo and HF deploys the actual
`llama-server` (image built from llama.cpp `master`) with an OpenAI-compatible
API. No pod bootstrap, no SSH, no model download step — the model already lives
in HF's own storage.

What it gives us:

- **Real `llama-server`**, so the llama.cpp-specific paths above are genuinely
  exercised.
- **Lifecycle by API**: `huggingface_hub.create_inference_endpoint(...)` →
  `.wait(timeout=…)` → `.pause()` / `.delete()`; CLI equivalents
  `hf endpoints deploy|describe|pause|scale-to-zero|delete`. The whole
  create-run-destroy cycle is ~15 lines, not a bootstrap script.
- **Billing by the minute**; a **paused or scaled-to-zero endpoint costs
  nothing**.
- **Auto scale-to-zero after idle** (1 hour by default) — this is the important
  one for §6: it is a *platform-provided* upper bound on a leak. A crashed run
  that never deletes its endpoint wastes at most one idle hour and then goes to
  $0 by itself.
- Hardware: T4 14 GB $0.50 · L4 24 GB $0.80 (GCP $0.70) · A10G 24 GB $1.00 ·
  **L40S 48 GB $1.80** · A100 80 GB $2.50 · H200 $5.00 (AWS). For a 17.65 GB
  model the L40S is the comfortable choice; the 24 GB tiers are the same tight
  fit as elsewhere.

> **Corrections from the stage-0 probe, 2026-07-28** (measured, not read):
> the paragraph below reasons from the documented `LLAMA_ARG_*` env vars, and
> the API tells a different and better story. Via `POST /v2/endpoint/{ns}` the
> `llamacpp` container takes explicit fields — `modelPath`, **`ctxSize`**,
> `nParallel`, `threadsHttp`, `url`, plus optional `mmprojModelPath`, `mode`
> and `pooling` — so context size is set directly (asked 16384, got
> `n_ctx=16384`), the unwanted mmproj file is simply a field left unset, and
> **the container image is ours to choose, so the llama.cpp build can be
> pinned** instead of tracking `master`. `LLAMA_ARG_JINJA=1` does work as an
> env var (`finish_reason=tool_calls`). The endpoint type is
> `public|authenticated|private` — **not** `protected`, and an unknown value is
> silently coerced to `private`. Deploy took **21 s** for the 17.65 GB model.
> Details: [docs/remote-e2e-hf.md §3](../history/remote-e2e-hf.md).

Configuration, and its limits (verified against the docs):

- Settings are `LLAMA_ARG_*` environment variables. **`LLAMA_ARG_JINJA` is
  available** — tool calling works. So does `LLAMA_ARG_THINK`
  (`--reasoning-format`) and `LLAMA_ARG_BATCH`/`LLAMA_ARG_UBATCH`.
- **Reserved (cannot be set):** `LLAMA_ARG_MODEL`, `LLAMA_ARG_N_GPU_LAYERS`,
  `LLAMA_ARG_CTX_SIZE`, `LLAMA_ARG_N_PARALLEL`, `LLAMA_ARG_EMBEDDINGS`,
  `LLAMA_ARG_NO_MMAP`, host/port/threads/metrics. Context size is set indirectly
  through the endpoint's *Max Tokens* × *Max Concurrent Requests* settings.
- ~~Because `LLAMA_ARG_EMBEDDINGS` is reserved, do not try to serve bge-m3 GGUF
  through the llama.cpp engine; use TEI with `BAAI/bge-m3` instead.~~
  **Wrong — corrected by the probe.** `LlamacppMode` has an `embeddings` value,
  so `model.image.llamacpp.mode: "embeddings"` serves the GGUF directly
  (measured: `/v1/embeddings`, OpenAI shape, dim 1024, on a T4). That is
  strictly better than TEI here: it is the *same GGUF and quantization as the
  local stack*, and the memory gates' similarity thresholds were calibrated
  against exactly that model. `pooling` is best left unset, so llama.cpp reads
  it from the GGUF metadata as it does locally.
- If total control is required, `custom_image` + `container_command` /
  `container_args` accept an arbitrary image (e.g.
  `ghcr.io/ggml-org/llama.cpp:server-cuda`) with explicit flags — at the cost of
  handling the model yourself.

Costs of this choice: the llama.cpp build is **whatever `master` is that day**
(unpinned — good for catching upstream drift, bad for reproducibility), the GPU
menu is fixed (no cheap 48 GB), and an HF account with a payment method is
required. Note also that scaled-to-zero endpoints still consume endpoint
*quota* — to release quota you must `pause` or `delete`.

### 3.2 What was checked and rejected

- **HF Spaces (Docker + GPU)** can run `llama-server` and has a *sleep time*
  setting, but paid GPU Spaces "stay running until you pause them" and the sleep
  window is coarse; it is a demo-hosting product, not a job runner. Inference
  Endpoints is the same company's answer to this use case and strictly better
  here.
- **HF Inference Providers / serverless Inference API** routes to third-party
  providers (Together, Fireworks, …). Not llama.cpp, no control over flags.
- **GitHub-hosted GPU runners**: still not a generally available product with a
  card big enough for a 17.65 GB model. Not an option today.
- **RunPod Serverless, Beam, Koyeb, Cerebrium, Baseten, Replicate**: all solve
  scale-to-zero well, but each expects you to write a worker/handler in their own
  framework. For "run an existing OpenAI-compatible server binary", that is more
  work than either HF or a plain pod, with no added fidelity.

## 4. RunPod mechanics (verified)

- **REST API** `https://rest.runpod.io/v1`, Bearer token. `POST /pods` (fields:
  `imageName`, `gpuTypeIds`, `gpuCount`, `ports` e.g. `"8000/http,22/tcp"`,
  `env`, `volumeInGb`, `containerDiskInGb`, `name`, `cloudType` SECURE/COMMUNITY,
  `interruptible`), `POST /pods/{id}/stop`, **`DELETE /pods/{id}`** = terminate.
- **There is no TTL field** in the REST create body — no `terminateAfter`,
  `stopAfter`, `idleTimeout`. (The legacy GraphQL
  `PodFindAndDeployOnDemandInput` does expose `stopAfter`/`terminateAfter`, but
  they are undocumented and at least one user reports pods kept running anyway —
  **do not rely on them**.)
- **RunPod does not auto-stop idle pods.** An idle pod bills the full rate.
- **A container that exits does not stop billing** — the pod stays allocated. So
  a self-destruct must actively call the API, not merely exit.
- **`runpodctl` is preinstalled in every pod with a pod-scoped API key** →
  `runpodctl remove pod $RUNPOD_POD_ID` works from inside the container with no
  secret injected. This is the foundation of the dead-man's switch.
  (`RUNPOD_API_KEY` inside the pod has been reported to 403 on the REST API —
  prefer `runpodctl`.)
- **Billing is per second**; charges settle every ~5 min. Account is **prepaid**
  — you cannot be billed past your balance. Default account cap **$80/hr**.
- **HTTP proxy** `https://<pod-id>-<port>.proxy.runpod.net` is public, TLS
  terminated — but sits behind Cloudflare with a **100 s cap on time-to-first-
  byte** (a `524` after that). Once a stream starts it stays open. Cold prompt
  processing on a 31B model could plausibly cross 100 s.
- **TCP** ports get a public `ip:port`, no TLS, no 100 s limit. Community-cloud
  IPs may change on migration; Secure-cloud IPs are stable.
- **SSH**: the proxy form (`ssh <pod-id>@ssh.runpod.io`) is limited — no
  SCP/SFTP. Full SSH (and therefore **port forwarding**) needs TCP port 22
  exposed and `openssh-server` in the image; official templates ship it, and the
  public key is injected via the `PUBLIC_KEY` env var.
- **Network volumes** ($0.07/GB/mo) would cache the 17.65 GB model between runs,
  but pin the pod to one datacenter and bill monthly whether or not you run. At
  datacenter download speeds the model pulls in a couple of minutes — **not worth
  it initially**.

## 5. Transport: how the runner reaches the servers

The harness constraint from §2 — `OpenAiClient::new(url)` sends no
`Authorization` header — shapes this, but it is worth naming the fix first:

> **The enabling change.** Teaching `live_backend()`/`live_embedder()` an
> optional `MINDFORK_ENGINE_KEY` / `MINDFORK_EMBED_KEY` that routes through the
> already-existing `OpenAiClient::with_api_key` is **~10 lines**. It unlocks
> every managed option and is a genuine improvement on its own (the smokes could
> then run against *any* authenticated OpenAI-compatible server). Treating the
> missing header as a hard blocker would be over-weighting it.

| | Public exposure | Auth | 100 s proxy limit | Needs the enabling change |
|---|---|---|---|---|
| **HF endpoint, `authenticated`**<br>(written `protected` below — the wrong value, see §3.1) | URL only; token required | `Authorization: Bearer hf_…` | no | **yes** |
| HF endpoint, `public` | **yes, unauthenticated** | none | no | no |
| **RunPod SSH tunnel** (`ssh -L 8000:127.0.0.1:8000 …`) | **none** | SSH key | no | no |
| RunPod HTTP proxy | yes, guessable URL | `llama-server --api-key` | **yes** | yes |
| RunPod TCP direct | yes, plaintext | `llama-server --api-key` (key in cleartext) | no | yes |

Two clean answers, one per platform:

- **HF → `type="authenticated"` + the enabling change.** Standard bearer auth, TLS,
  nothing bespoke.
- **RunPod → SSH tunnel, no change at all.** `llama-server` binds `127.0.0.1`
  inside the pod, nothing is published, and the tests keep using
  `MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1` exactly as they do today. Also
  sidesteps the Cloudflare 100 s cap.

Creating an HF endpoint as `public` would work with no code change, but leaves an
**unauthenticated LLM endpoint on the open internet** for the duration of the
run. Not recommended when the alternative costs ten lines.

## 6. **The cleanup guarantee** — the central question

Short answer: **no rented-pod provider gives you one; you build it, and the only
layer that truly guarantees it is inside the pod. A managed service moves the
guarantee into the platform — which is the strongest argument for HF over
RunPod.**

**On a managed endpoint (HF, Modal, Cloud Run) most of what follows is
unnecessary.** Idle auto-scale-to-zero is itself the dead-man's switch: the worst
case is not "a GPU runs until someone notices" but "one idle window is wasted,
then it costs nothing". On HF with the default 1-hour window that ceiling is
**~$1.80** on an L40S, with no code of ours involved — and a scheduled sweeper
calling `hf endpoints ls` + `delete` reduces it further and reclaims quota. The
rest of this section is what a **rented pod** requires to reach a comparable
position.

Every external watchdog shares one failure mode: it is a *different* machine that
can also fail. `if: always()` in GitHub Actions does **not** run when the runner
is killed, the job is force-cancelled, or the network partitions. So the design
principle is:

> The pod must be able to kill itself without anyone telling it to.

Five layers, cheapest and most reliable first:

**L0 — in-pod self-destruct (the actual guarantee).** The pod's start command
launches, before anything else:

```bash
( sleep "${MAX_LIFETIME_SECONDS:-3600}"; runpodctl remove pod "$RUNPOD_POD_ID" ) &
```

No secret to inject (pod-scoped key is preinstalled), no external actor, survives
runner death, cancelled jobs, expired tokens and network partitions. This alone
bounds the loss to `MAX_LIFETIME × rate` ≈ **$0.44** at one hour on an A40.

**L1 — heartbeat (dead-man's switch).** The CI job touches a file over the tunnel
every 30 s; an in-pod loop terminates if the file is older than ~5 min. Turns "at
most an hour of waste" into "at most five minutes". Optional — L0 already caps
the money; L1 caps the *time*.

**L2 — CI cleanup step.** `if: always()` → `DELETE /v1/pods/{id}`. Handles the
normal path in seconds. **Not a guarantee**, for the reasons above.

**L3 — orphan sweeper.** A scheduled workflow (hourly `cron` +
`workflow_dispatch`) that lists pods, matches a name prefix (`e2e-<run_id>-…`)
and terminates anything older than the max lifetime. This catches the one leak
L0 cannot: **pod creation succeeded but the response was lost**, so no id was
ever recorded. Mitigate further by making the name deterministic *before* the
call, so the sweeper can find the pod without the id.

**L4 — money backstop.** RunPod is prepaid: keep a small working balance (e.g.
$20–30, not $500) — the balance is itself a hard ceiling, and RunPod stops pods
at $0. Optionally lower the account spend cap from the $80/hr default.

Ordering matters: **L0 first**. A design that only has L2 + L3 is the common
mistake — it works until the day the runner is OOM-killed mid-run.

**Modal and Cloud Run remove L0–L3 the same way**: containers are billed only
while alive, scale to zero when idle, and Modal's `timeout` is a hard cap.
Modal's caveat is auth — `requires_proxy_auth=True` uses `Modal-Key`/
`Modal-Secret` headers, which the harness cannot send even with the §5 enabling
change (that adds `Authorization: Bearer`, not arbitrary headers). Unauthorized
requests are rejected before a container starts, so the *cost* risk is bounded
either way.

## 7. Cost

| | GPU | $/hr | per ~30 min run | 10 runs/mo |
|---|---|---|---|---|
| HF Inference Endpoints | L40S 48 GB | 1.80 | **~$0.90** | ~$9 |
| HF, chat on L4 24 GB | L4 | 0.80 | ~$0.40 | ~$4 |
| HF embeddings (TEI, bge-m3) | T4 | 0.50 | ~$0.25 | ~$2.5 |
| RunPod pod | A40 48 GB | 0.44 | **~$0.22** | ~$2 |
| Modal | L40S | 1.95 | ~$1.00 | ~$10 |
| Self-hosted runner | own | — | **$0** | $0 |

A run is ~25–30 min: boot/deploy + model load + ~15–20 min of tests (on RunPod,
plus a 2–4 min model pull; on HF the model is already in HF storage).

**Cost is not the constraint at this volume — the leak risk is.** One forgotten
RunPod pod running over a weekend on an A40 is ~$21, i.e. a hundred runs' worth.
The HF/RunPod price gap (~$0.70 per run) buys away the entire class of problem in
§6; at ten runs a month that is ~$7, which is the wrong thing to optimise.

## 8. The alternative worth considering first

**A self-hosted GitHub Actions runner on the machine that already has the
stack.** Zero cost, zero cleanup risk, and it exercises the exact configuration
the journal has been recording for months. Trade-offs: the machine must be up
when the workflow runs (a `workflow_dispatch`/nightly trigger fits), and a
self-hosted runner on a **private** repo is acceptable — the well-known warning
against self-hosted runners applies to public repos accepting fork PRs.

This does not preclude the rented-GPU path; they answer different questions
("run the gate cheaply and often" vs "run the gate from anywhere, including from
a machine that has no GPU"). It is listed here because it is the cheapest correct
answer to the stated problem and should be rejected deliberately, not by
omission.

## 9. Forks for decision

- **R1 — platform.**
  (a) **HF Inference Endpoints** — llama.cpp as a managed service, cleanup
  bounded by the platform, least infrastructure of our own. *(recommended)*
  (b) RunPod pods + the layered switch — ~4× cheaper per run, but we own the
  bootstrap, the tunnel and the entire cleanup guarantee.
  (c) Modal — comparable guarantee to (a), more of our own code than (a).
  (d) Self-hosted runner on the dev box — $0, no leak risk, needs the box up.
  (e) (a) or (b) **plus** (d): self-hosted for the routine gate, rented for
  portability and for machines without a GPU.

- **R2 — transport / auth.** *(follows from R1)*
  (a) **HF `protected` + the ~10-line `MINDFORK_ENGINE_KEY` change**
  *(recommended with R1a — see §5)*
  (b) RunPod SSH tunnel, nothing published, no code change *(the answer if R1b)*
  (c) HF `public` — no code change, but an unauthenticated endpoint while the run
  lasts. Not recommended.

- **R3 — how much of the switch to build now.** *(only meaningful for R1b)*
  (a) **L0 + L2 + L3 + L4** *(recommended for a pod — L0 is the guarantee, L3
  covers the lost-id leak)*
  (b) all five including the heartbeat (L1).
  (c) L0 + L2 only (accept up to one wasted pod-hour in rare cases).
  With R1a this reduces to: a `finally`-style `delete` plus a scheduled sweeper.

- **R4 — GPU / hardware.**
  With R1a: (a) **L40S 48 GB $1.80/hr** *(recommended — no VRAM question)* ·
  (b) L4/A10G 24 GB (~$0.80–1.00, tight fit, needs measuring).
  With R1b: (a) **A40 48 GB Community $0.44/hr** *(recommended — cheaper **and**
  roomier than a 4090)* · (b) RTX 4090 24 GB · (c) Secure Cloud (stable IPs, ~2×).
  `interruptible`/spot is **not** recommended either way — a preemption mid-suite
  is a false failure.

- **R5 — trigger.**
  (a) **`workflow_dispatch` only** *(recommended to start — a live run is a
  deliberate act, per AGENTS.md §3)*
  (b) + nightly `cron` on `main`.
  (c) + a PR label (e.g. `live-e2e`) — never automatic on every PR (~$0.25 and
  25 min each, plus network-dependent flakes).

- **R6 — scope of the smokes to run remotely.**
  (a) **the llama.cpp set: `tests/live.rs` + engine/supervisor/embed smokes**
  *(recommended)*
  (b) + a second embedder (e5-large-instruct) so the model-change / calibration /
  prefix smokes run too — one more server, ~1 GB, a little more VRAM.
  (c) everything except the cloud-key and sandbox smokes.

- **R7 — model caching.** *(only meaningful for R1b — on HF the model is already
  in HF storage)*
  (a) **none — pull from HF each run** *(recommended: ~2–4 min, no monthly cost,
  no datacenter pinning)*
  (b) a RunPod network volume (~$1.40/mo for 20 GB, pins the datacenter).

- **R8 — secrets.** GitHub **repository secrets** (`gh secret set`):
  with **R1a** → `HF_TOKEN` only (one credential: it both manages the endpoints
  and authenticates the requests — issue a fine-grained token scoped to Inference
  Endpoints, separate from any interactive token);
  with **R1b** → `RUNPOD_API_KEY` (required) + `SSH_PRIVATE_KEY` + `HF_TOKEN`
  (optional — the models are public, §2; a token only lifts download rate limits).
  Confirm: plain repository secrets, or a GitHub *environment* with required
  reviewers gating the live job?

## 10. Sketch of the implementation

### 10.a If R1a (HF Inference Endpoints) is chosen

1. The ~10-line enabling change (§5): `MINDFORK_ENGINE_KEY` / `MINDFORK_EMBED_KEY`
   → `OpenAiClient::with_api_key`. Ships as its own small PR with a unit test.
2. `scripts/e2e-hf.py` (huggingface_hub) — one entry point for CI and local use:
   - `create_inference_endpoint(name=f"e2e-chat-{run_id}", repository=
     "google/gemma-4-31B-it-qat-q4_0-gguf", …, type="authenticated",
     instance_type="nvidia-l40s", env={"LLAMA_ARG_JINJA": "1"})`, plus a second
     endpoint for `BAAI/bge-m3` on the TEI engine;
   - `.wait(timeout=…)` on both, then `GET /health`;
   - `MINDFORK_ENGINE_URL=<url>/v1`, `MINDFORK_EMBED_URL=<url>/v1`,
     `MINDFORK_ENGINE_KEY=$HF_TOKEN` → `cargo test -- --ignored --test-threads=1`;
   - `finally:` `.delete()` on both — and assert they are gone.
3. `.github/workflows/e2e-live.yml` — `workflow_dispatch`, plus a sweeper
   (`hf endpoints ls` → delete anything named `e2e-*` older than the max
   lifetime). The sweeper also reclaims endpoint quota, which scale-to-zero
   alone does not.
4. Docs: `docs/install.md`, a CLAUDE.md journal entry.

### 10.b If R1b (RunPod) is chosen

1. `scripts/e2e-gpu.sh` (or a small workflow) — single entry point, so it works
   the same locally and in CI.
2. Create the pod: official CUDA image or `ghcr.io/ggml-org/llama.cpp:server-cuda`
   with `openssh-server`, `ports: "22/tcp"`, `env: PUBLIC_KEY`, `MAX_LIFETIME`,
   `HF_TOKEN`, name `e2e-${GITHUB_RUN_ID}-${ATTEMPT}`. **Write the intended name
   to disk before the call**, so the sweeper can find an orphan without the id.
3. The pod's start command: **L0 watchdog first**, then download the two GGUFs
   (`hf download` / `curl`), then start both `llama-server`s on `127.0.0.1`
   (chat `:8000 --jinja -ngl 99 -c 16384`, embeddings `:8001 --embeddings -ngl 99
   -c 8192` — the batch flags come from the app when managed, so pass them
   explicitly here).
4. Runner: wait for SSH, open `ssh -N -L 8000:127.0.0.1:8000 -L 8001:127.0.0.1:8001`,
   poll `/health` on both until ready.
5. `MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1
   MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1 cargo test -- --ignored
   --test-threads=1` (single-threaded is required — the journal's live runs all
   use it).
6. `if: always()` → `DELETE /v1/pods/{id}`; assert the pod is gone and fail the
   job loudly if it is not.
7. `.github/workflows/e2e-sweeper.yml` — hourly orphan sweep.
8. Docs: `docs/install.md` (how to run it), CLAUDE.md journal entry, and an ADR
   only if the design turns out to constrain the app itself (it should not).

## 11. Open questions / risks

- **Does 17.65 GB + 16k ctx actually fit in 24 GB?** Not measured. Sidestepped by
  R4a (48 GB) on either platform.
- **HF-specific, unverified:** that the llama.cpp engine accepts
  `google/gemma-4-31B-it-qat-q4_0-gguf` cleanly when the repo also contains an
  mmproj file (file selection is a deploy-time choice — should be fine, but
  untested); what context size the *Max Tokens* × *Max Concurrent Requests*
  settings actually produce; and whether the **unpinned `master`** llama.cpp
  build introduces day-to-day variance in the smokes. All three are cheap to
  settle with one throwaway endpoint before committing to R1a.
- **Community-cloud host quality varies** (download speed, disk, occasional bad
  hosts). A retry-once policy on pod creation may be warranted.
- **Non-hermetic smokes** (`web_search`, `fetch_url`, MCP `npx` download) will
  flake independently of the GPU; the MCP smoke's first `npx` run can exceed its
  120 s readiness timeout on a cold npm cache (already recorded in the journal).
- **`gemma-4-31B_q4_0-it.gguf`** — the file name is irregular for the repo; pin it
  exactly rather than globbing.
- Prices and the absence of a TTL field were checked on 2026-07-28 and should be
  re-checked before implementation.

## Sources

**Hugging Face**
- [Inference Endpoints: llama.cpp engine](https://huggingface.co/docs/inference-endpoints/en/engines/llama_cpp) ·
  [Autoscaling / scale to zero](https://huggingface.co/docs/inference-endpoints/en/guides/autoscaling) ·
  [Pricing](https://huggingface.co/docs/inference-endpoints/en/pricing) ·
  [Managing endpoints with `huggingface_hub`](https://huggingface.co/docs/huggingface_hub/en/guides/inference_endpoints)
- ["Inference Endpoints now supports GGUF out of the box"](https://github.com/ggml-org/llama.cpp/discussions/9669)
- [Text Embeddings Inference (TEI)](https://github.com/huggingface/text-embeddings-inference) ·
  [Using GPU Spaces](https://huggingface.co/docs/hub/en/spaces-gpus)
- [llama-server env vars (`LLAMA_ARG_*`)](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)

**RunPod and others**
- [RunPod REST API OpenAPI spec](https://rest.runpod.io/v1/openapi.json)
- [Manage Pods (docs)](https://docs.runpod.io/pods/manage-pods) ·
  [Expose ports](https://docs.runpod.io/pods/configuration/expose-ports) ·
  [Use SSH](https://docs.runpod.io/pods/configuration/use-ssh) ·
  [Pricing](https://docs.runpod.io/pods/pricing) ·
  [Billing](https://docs.runpod.io/accounts-billing/billing) ·
  [Network volumes](https://docs.runpod.io/storage/network-volumes)
- [When to use (or not use) RunPod's proxy](https://www.runpod.io/blog/runpod-proxy-guide)
- [AI on a schedule: using RunPod's API to run jobs only when needed](https://www.runpod.io/articles/guides/ai-on-a-schedule)
- [Terminating a pod from within the pod (community)](https://www.answeroverflow.com/m/1267200524181180427) ·
  [stopAfter/terminateAfter not honoured (community)](https://www.answeroverflow.com/m/1208014863406997534) ·
  [Auto-stop on idle is not supported (community)](https://www.answeroverflow.com/m/1285656228898541629)
- [runpodctl remove pod](https://github.com/runpod/runpodctl/blob/main/docs/runpodctl_remove_pod.md)
- [Modal pricing](https://modal.com/pricing) ·
  [modal.web_server](https://modal.com/docs/reference/modal.web_server) ·
  [Proxy tokens](https://modal.com/docs/guide/webhook-proxy-auth) ·
  [Scaling out](https://modal.com/docs/guide/scale) ·
  [llama.cpp on Modal](https://github.com/modal-labs/modal-examples/blob/main/06_gpu_and_ml/llm-serving/llama_cpp.py)
- [llama.cpp Docker images](https://github.com/ggml-org/llama.cpp/blob/master/docs/docker.md)
- [Cloud Run: GPU support for services](https://docs.cloud.google.com/run/docs/configuring/services/gpu) ·
  [Cloud Run GPUs generally available](https://cloud.google.com/blog/products/serverless/cloud-run-gpus-are-now-generally-available)
- [google/gemma-4-31B-it-qat-q4_0-gguf](https://huggingface.co/google/gemma-4-31B-it-qat-q4_0-gguf) ·
  [ggml-org/bge-m3-Q8_0-GGUF](https://huggingface.co/ggml-org/bge-m3-Q8_0-GGUF)
</content>
</invoke>
