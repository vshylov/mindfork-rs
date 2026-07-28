# Research: running the live e2e smokes on a rented GPU

**Status:** research, pre-decision. Forks **R1–R8** in §9 need the user's answer
before any implementation (AGENTS.md §1).
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
> required** — neither repo is gated. A token is still worth setting
> (`HF_TOKEN`) because anonymous downloads are rate-limited and slower, but it
> is a convenience, not a gate.

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

| Option | llama.cpp fidelity | Cleanup guarantee | Cost per ~30 min run | Verdict |
|---|---|---|---|---|
| **RunPod Pods** | full (any container) | **none native** — must be built (§5) | ~$0.22 (A40) | **Recommended**, with the layered switch |
| **Modal** | full (official llama.cpp example) | **platform-enforced** — containers scale to zero, `timeout` is a hard cap | ~$1.00 (L40S) | Best answer to the cleanup question; 4–5× the price, and see the auth caveat |
| RunPod Serverless | ✗ — the stock worker is vLLM; llama.cpp needs a custom worker | platform-enforced | low | Loses the point: the smokes test **llama.cpp-specific** paths |
| Vast.ai | full | none native, plus interruptible hosts | ~$0.10–0.18 | Cheapest, least reliable; same switch needed |
| Lambda / Hyperstack / bare VMs | full | none native | higher | No advantage over RunPod here |
| Cloud APIs (OpenAI/Gemini/Claude) | ✗ | n/a | n/a | Already covered by separate key-gated smokes |
| **Self-hosted GitHub runner on the dev box** | full | **n/a — nothing is rented** | **$0** | Genuinely the simplest option; see §8 |

**Why llama.cpp fidelity is non-negotiable:** the smokes exercise llama.cpp
request-body extensions (`dynatemp_*`, `dry_*`, `xtc_*`, `mirostat`, `samplers`),
`--jinja` tool calling, `reasoning_format`/`reasoning_budget` "thoughts", the
`/health` readiness probe and its `503 Loading model`, and the `-ub/-b` embedding
batch behaviour. Any vLLM-based serverless endpoint would exercise none of it.

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

Three options, and the harness constraint from §2 decides:

| | Public exposure | Auth | 100 s limit | Code change |
|---|---|---|---|---|
| HTTP proxy | yes, guessable URL | none available (no header support) | **yes** | — |
| TCP direct | yes, plaintext | none available | no | — |
| **SSH tunnel** (`ssh -L 8000:127.0.0.1:8000 …`) | **none** | SSH key | no | **none** |

With `OpenAiClient::new(url)` sending no `Authorization` header, both public
options would leave an **unauthenticated LLM endpoint on the open internet** for
the duration of the run. The SSH tunnel is not just nicer — it is the only option
that is both secure and needs no code change: `llama-server` binds `127.0.0.1`
inside the pod, nothing is published, and the tests use
`MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1` exactly as they do today.

(If a public endpoint is ever wanted, the minimal change is to teach
`live_backend()`/`live_embedder()` an optional `MINDFORK_ENGINE_KEY` →
`OpenAiClient::with_api_key`, paired with `llama-server --api-key`. Not needed
for the tunnel design.)

## 6. **The cleanup guarantee** — the central question

Short answer: **no rented-pod provider gives you one; you build it, and the only
layer that truly guarantees it is inside the pod.**

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

**Modal removes L0–L3 entirely**, which is the honest argument for it: containers
are billed only while alive, scale to zero after `scaledown_window`, and
`timeout` is a hard platform-enforced cap. There is nothing to leak. Its caveat
here is auth: Modal's `requires_proxy_auth=True` uses `Modal-Key`/`Modal-Secret`
headers, which the harness cannot send (§2) — so it would be either an
unauthenticated `.modal.run` URL (protected only by obscurity, though unauthorized
requests are rejected before a container starts, so the *cost* risk is bounded)
or a small code change.

## 7. Cost

Per run, A40 48 GB @ ~$0.44/hr, assuming pod boot ~1 min + model pull ~2–4 min +
load ~1–2 min + tests ~15–20 min ≈ **25–30 min → ~$0.20–0.25**. Ten runs a month
is under $3. Modal on an L40S is ~4× that and still trivial. **Cost is not the
constraint here; the leak risk is** — one forgotten pod running for a weekend on
an A40 is ~$21, which is 100 runs' worth.

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
  (a) **RunPod pods + layered switch** — cheapest, full control, we own the
  cleanup logic. *(recommended)*
  (b) Modal — platform-guaranteed cleanup, ~4× cost, auth caveat.
  (c) Self-hosted runner on the dev box — $0, no leak risk, needs the box up.
  (d) Both (a) and (c): self-hosted for the routine gate, rented for portability.

- **R2 — transport.**
  (a) **SSH tunnel, nothing published** *(recommended — the only option needing
  no code change, see §5)*
  (b) RunPod HTTP proxy + a new `MINDFORK_ENGINE_KEY` and `--api-key`.
  (c) TCP direct + `--api-key` (plaintext key on the wire).

- **R3 — how much of the switch to build now.**
  (a) **L0 + L2 + L3 + L4** *(recommended — L0 is the guarantee, L3 covers the
  lost-id leak)*
  (b) all five including the heartbeat (L1).
  (c) L0 + L2 only (accept up to one wasted pod-hour in rare cases).

- **R4 — GPU / hardware.**
  (a) **A40 48 GB Community** *(recommended — cheaper and roomier than a 4090)*
  (b) RTX 4090 24 GB (as originally proposed).
  (c) Secure Cloud (stable IPs, ~2× price).
  Also: `interruptible` (spot) is **not** recommended — a preempted pod mid-suite
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

- **R7 — model caching.**
  (a) **none — pull from HF each run** *(recommended: ~2–4 min, no monthly cost,
  no datacenter pinning)*
  (b) a RunPod network volume (~$1.40/mo for 20 GB, pins the datacenter).

- **R8 — secrets.** `RUNPOD_API_KEY` (required) and `HF_TOKEN` (optional, §2) as
  GitHub **repository secrets** (`gh secret set`). Scope the RunPod key as
  narrowly as its permission model allows and keep it separate from any key used
  interactively. Confirm: repository secrets, or an environment with required
  reviewers for the live job?

## 10. Sketch of the implementation (if R1a + R2a are chosen)

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
  R4a (48 GB).
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
- [google/gemma-4-31B-it-qat-q4_0-gguf](https://huggingface.co/google/gemma-4-31B-it-qat-q4_0-gguf) ·
  [ggml-org/bge-m3-Q8_0-GGUF](https://huggingface.co/ggml-org/bge-m3-Q8_0-GGUF)
</content>
</invoke>
