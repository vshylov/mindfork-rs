# Design plan: the live e2e gate on HF Inference Endpoints

**Status:** **complete** — stages 0–3 done, the gate runs and covers the whole
llama.cpp smoke set.
**Research and decision:** [docs/research/remote-e2e-gpu.md](research/remote-e2e-gpu.md)
(forks R1–R8, all resolved to the recommended option — *user's decision,
2026-07-28*).
**Date:** 2026-07-28.

## 1. Goal

Make the mandatory live gate (AGENTS.md §3) runnable **from anywhere**, instead
of only from one machine on one LAN address (`run_all_tests.bat` →
`http://192.168.1.20:8000/v1`).

Shape of the answer, from the research: an ephemeral **HF Inference Endpoint**
running HF's own **llama.cpp engine** (a real `llama-server`, so the
llama.cpp-specific paths are genuinely exercised) for chat, plus a **TEI**
endpoint with `BAAI/bge-m3` for embeddings; created, used and deleted by one
script; triggered by `workflow_dispatch`.

**Explicit non-goal:** replacing the local run. The dev-box path stays exactly as
it is (`MINDFORK_ENGINE_URL` unset of a key ⇒ today's behaviour, byte for byte),
and remains the authoritative one for the managed-mode smokes that cannot run
remotely at all (§4).

## 2. Stages

Four stages, three of them PRs. Each is a separate branch (AGENTS.md §2).

| # | Branch | What | Live run | Status |
|---|---|---|---|---|
| 0 | `spike/hf-endpoint-probe` | Settle the unknowns against one throwaway endpoint (`tools/hf_probe.py`) | it *is* the live run | **done — GO** |
| 1 | (same branch) | The enabling change: an optional Bearer key for the live-smoke helpers | local, regression only | **done** |
| 2 | `feat/e2e-hf-runner` | The runner script, the workflow, the sweeper, docs | the full remote gate | **done** |
| 3 | `feat/e2e-hf-alt-embedder` | A second embedding endpoint so the model-change smokes run too | those 4 smokes | **done** |

Stage 1 shipped on the stage-0 branch rather than its own: it is ~10 lines plus
tests, and the probe needed it to run the suite at all.

Stage 0 gates the rest: it is cheap (~$2, an hour of L40S) and answers questions
that would otherwise be guessed at inside a PR.

## 3. Stage 0 — probe (go/no-go)

Driven by **[`tools/hf_probe.py`](../tools/hf_probe.py)** (stdlib only, no
`pip install`; `HF_TOKEN` never leaves the machine it runs on). It creates a
throwaway endpoint, answers the questions below, and deletes it — verifying the
deletion, with a failure to delete reported louder (exit 3) than a failed check.
Nothing is committed except the answers, which land in the research doc.

```
python tools/hf_probe.py hardware        # U7
python tools/hf_probe.py run --dry-run   # review the payloads first
python tools/hf_probe.py run             # create, check, delete
python tools/hf_probe.py inspect <name>  # the U1 fallback: read what the UI built
```

It uses the raw REST API rather than `huggingface_hub` deliberately: full
control over the payload, and the server's 4xx bodies are printed verbatim, so a
rejected payload *teaches us the schema* (`--set a.b.c=value` iterates on it).

**Stage 0 verdict: GO** — every unknown resolved, 2026-07-28, against
`nvidia-l40s` x1 (chat) and `nvidia-t4` x1 (embeddings), aws us-east-1. Total
spend for the whole stage ≈ **$0.17**. Nothing was left running (`list` empty).

| | Answer |
|---|---|
| **U1** | `model.image.llamacpp.modelPath` — loaded `/repository/gemma-4-31B_q4_0-it.gguf`, the right file. `mmprojModelPath` is a separate optional field, so the repo's second file is a non-issue. |
| **U2** | `ctxSize` is an **explicit payload field** — asked for 16384, `/props` reports `n_ctx=16384`. The Max Tokens × Max Concurrent Requests story in the docs does not apply to the API path. |
| **U3** | `LLAMA_ARG_JINJA=1` works: `finish_reason=tool_calls`. |
| **U4** | SSE fine through the router: **ttfb 0.7 s, 495 events, 14.7 s** for a 600-token generation. No truncation. |
| **U6** | `authenticated` endpoints reject an unauthenticated request with **401**. |
| **U5** | Embeddings served by the **llama.cpp engine itself** (`mode: "embeddings"`): `/v1/embeddings` returns the OpenAI shape, 2 vectors, **dim 1024**. |
| **U5b** | `/health` **is** reachable (200) — better than feared, so the two supervisor smokes keep their meaning once `probe()` sends the header (§5). |
| **U7** | `aws` / `us-east-1` / `nvidia-l40s` / `x1`, 48 GB, $1.80/hr; `nvidia-t4` $0.50/hr. Quota (`/v2/provider/quotas/{ns}`): L40S 16, T4 30 — the zeros in `/v2/provider` are per-compute placeholders, not the account quota. |

Two findings that change the design, both for the better:

- **`LlamacppMode` has an `embeddings` value**, so bge-m3 is served by the
  llama.cpp engine itself — the *same GGUF and quantization* the similarity
  gates were calibrated on — instead of TEI. §3.1 of the research said the
  opposite; it reasoned from the reserved `LLAMA_ARG_EMBEDDINGS` env var, but
  the mode is a first-class payload field.
- **The container `url` is ours to supply** (no catalog route, no default), so
  the llama.cpp build can be **pinned to a tag** instead of tracking `master`.
  That retires the reproducibility risk in §10.

And one trap worth remembering: `EndpointType` is `public | authenticated |
private`, and the API **silently coerces** an unknown value rather than
rejecting it. Sending the older wording `protected` produced a `private`
(PrivateLink-only) endpoint that no CI runner could reach — with a 200 and a
healthy-looking response. The probe now refuses to continue when the echoed
type differs from the requested one.

Deploy time was **21 s** to `running` for the 17.65 GB model (84 s for the
embedder) — HF serves the weights from its own storage, so there is no
multi-minute model pull to budget for.

**The full suite, run against the endpoints — 2026-07-28.** `cargo test --
--ignored --test-threads=1`, 1016 s: **67 passed, 2 failed** of 69. Of those, 36
actually exercised the endpoints (**35 green**); 4 skipped for want of
`MINDFORK_EMBED_URL_ALT` (stage 3), 15 for want of cloud keys, 13 for want of a
provisioned Python sandbox. Cost ≈ $0.75, both endpoints deleted and verified.

Neither failure is infrastructural:

- `runs_real_python_in_sandbox` — no `wasmer` on this machine; it fails rather
  than skips by design.
- `attachment_read_e2e_live` — **a brittle assertion the remote run exposed**,
  confirmed by running the same test against the LAN stand:

  | | turn 1 | turn 2 | |
  |---|---|---|---|
  | LAN | `attachment_search` | `attachment_read` | pass |
  | HF | `attachment_search` + `attachment_read` ×5 | — (answered from history) | fail |

  Same model, quantization, context and flags. On HF the model was *more*
  thorough — it searched and then read five pages anyway — so page 1 was already
  in the conversation and turn 2 legitimately needed no tool. Turn 2's assertion
  ("asking for a specific page must go through `attachment_read`") therefore
  holds only when turn 1 happens not to have read that page, which the test does
  not control. This is the second time this test's assertion has been invalidated
  by the model finding a better route (the first was when `attachment_search`
  arrived). Proposed fix: cover the guaranteed path **deterministically** by
  running this smoke with no embedder configured, so paging is the only way in —
  search is already covered by `attachment_search_e2e_live`.

**A third finding, and a requirement on the runner.** The first run failed U5
with a `503`, and the cause was ours: HF's `running` state only means the
container is up, while `llama-server` still answers `503 Loading model` until
the weights are in — the very distinction `OpenAiClient::probe()` exists to
draw. The chat checks happened to begin with `/health` and survived; the
embedding checks fired straight at `/v1/embeddings`. So **the runner must gate
on `/health`, not on the endpoint state**, or the suite's first request fails.
Encoded as `wait_healthy()`; re-checked and green.

Unknowns, in the order that matters:

- **U1 — GGUF file selection via the API.** The UI asks which `.gguf` to serve;
  the corresponding API field is **not documented** anywhere reachable. The repo
  `google/gemma-4-31B-it-qat-q4_0-gguf` contains two files
  (`gemma-4-31B_q4_0-it.gguf` **17.65 GB** and an unwanted 1.2 GB mmproj), so
  "it picks one for you" is not good enough. *Method:* create it in the UI, then
  read `get_inference_endpoint(...).raw` and copy whatever field appears.
  *Fallback:* `custom_image` + `container_args` with an explicit
  `ghcr.io/ggml-org/llama.cpp:server-cuda` and our own flags.
- **U2 — effective context.** `LLAMA_ARG_CTX_SIZE` is reserved; context is set
  indirectly via *Max Tokens (per request)* × *Max Concurrent Requests*. Measure
  what the server reports (`/props` or the logs) and check it covers the suite's
  needs (attachments, reflection digests). The suite runs `--test-threads=1`, so
  concurrency 1–2 is enough and the whole budget can go to context.
- **U3 — `LLAMA_ARG_JINJA=1` actually takes effect.** Tool calling is load-bearing
  for most of the 44 orchestrator smokes. *Check:* one `/v1/chat/completions`
  with a `tools` array → expect `finish_reason: tool_calls`.
- **U4 — request timeout and SSE through the HF router.** Confirm a streamed
  completion arrives as SSE and that a long one is not cut. (The Cloudflare 100 s
  problem is RunPod-specific, but HF has its own router; do not assume.)
- **U5 — TEI + `BAAI/bge-m3`.** `POST /v1/embeddings` returns the OpenAI shape
  `OpenAiClient` expects, dimension **1024**. Also: does `/health` survive the
  proxy — it decides whether the two supervisor smokes stay meaningful (§4).
- **U6 — `protected` accepts `Authorization: Bearer hf_…`** on both endpoints.
- **U7 — the exact `vendor` / `region` / `instance_type` / `instance_size`
  strings** for an L40S 48 GB (from the vendor list, not from a guess).

**Go criterion:** U1–U6 answered, and a hand-run of the llama.cpp smoke set
against the probe endpoint is green. **No-go fallbacks:** U1 fails → `custom_image`;
U2 too small → a bigger instance or fewer concurrent requests; U3/U4 fail →
reconsider RunPod (R1b), which the research keeps costed and designed.

## 4. Which smokes run remotely

From R6a — the llama.cpp set, ~55 of the 69 `#[ignore]` smokes:

| Where | Count | Remote |
|---|---|---|
| `orchestrator/tests/live.rs` | 44 | yes (2 of them need `MINDFORK_OPENAI_KEY`/`GEMINI_KEY` — skipped) |
| `shared/api/openai/client.rs` | 7 | yes — the base engine smokes |
| `orchestrator/tests/mcp.rs`, `shared/mcp.rs` | 2 | yes (needs `npx` on the runner) |
| `app/supervisor.rs` | 2 of 3 | yes, but **weakened** — see below |
| `app/supervisor.rs` (`managed_child_death_…`) | 1 | **no** — needs `MINDFORK_LLAMA_BIN`, i.e. a local child process |
| `embed_guard.rs`, `reembed.rs`, `embed_prefix.rs` | 4 | **no** — need `MINDFORK_EMBED_URL_ALT` (stage 3) |
| Cloud (Anthropic/Gemini/OpenAI/TTS), Python sandbox | ~11 | no — different credentials/assets |

**The two supervisor smokes — caveat lifted by the probe.** `monitor_does_not_flap_…`
and `embed_probe_reaches_ready_…` drive `OpenAiClient::probe()`, which bails only
on `503`. The worry was that behind HF's router a `401`/`404` on `/health` would
read as "ready", so they would pass for the wrong reason. Measured: `/health`
**is** proxied through and returns a real `200`/`503` (that is exactly how the
loading state was caught, §3). Once stage 1 makes `probe()` send the header,
these two keep their meaning remotely.

## 5. Stage 1 — the enabling change

Small, self-contained, useful on its own: the live-smoke helpers learn an
optional Bearer key, so the smokes can run against **any** authenticated
OpenAI-compatible server.

- One shared helper (`#[cfg(test)] pub(crate)` in `shared/api`, visible
  crate-wide and FSD-clean since every layer may import `shared`):
  `live_client(url_var, key_var) -> Option<OpenAiClient>` — reads the URL, and
  `with_api_key` when the key var is set. Empty key ⇒ `None` ⇒ today's behaviour
  (`with_api_key` already filters empties).
- Call sites converted (6 files): `orchestrator/tests/mod.rs`
  (`live_backend`/`live_embedder`), `openai/client.rs` (`client_from_env`),
  `shared/mcp.rs`, `embed_guard.rs`, `reembed.rs`, `embed_prefix.rs`.
- Env vars: `MINDFORK_ENGINE_KEY`, `MINDFORK_EMBED_KEY` (+ `_ALT` in stage 3).
- **`OpenAiClient::probe()` gains `self.auth(...)`.** Today it GETs `/health`
  without the header; on a protected endpoint that is a 401 that reads as ready.
  This does not change any current behaviour (no key ⇒ no header) and is what the
  two supervisor smokes need to mean anything against an authenticated server.
- The app itself needs **nothing**: `ExternalSettings.api_key_env` already exists
  (the API-keys track, ADR 0008), so external mode with a Bearer key is already
  supported end to end.

**Tests:** unit — the helper returns `None` without a URL, no key ⇒ no header,
set key ⇒ header present, empty key ⇒ no header; and `probe()` sends the header
when a key is set. **Live run:** the local stack, to prove the unset-key path is
unchanged (the authenticated path is proven in stage 2).

## 6. Stage 2 — the runner

**`tools/e2e_hf.py`** — one entry point, identical locally and in CI (the
precedent is `packaging/linux/build-packages.sh`; `tools/` is where this repo
keeps its Python helpers). Grown from `tools/hf_probe.py`, whose lifecycle and
cleanup code it inherits.

```
create chat endpoint   (llamacpp, gemma-4-31B q4_0, L40S, authenticated,
                        ctxSize 16384, nParallel 1, LLAMA_ARG_JINJA=1)
create embed endpoint  (llamacpp mode=embeddings, bge-m3 Q8_0, T4,
                        authenticated, ctxSize 8192)
  → poll the endpoint state to `running`
  → then poll /health until 200 — `running` is not loaded (§3)
  → cargo test -- --ignored --test-threads=1   with the four env vars
finally: delete both, then verify they are gone; a failed delete fails the run
```

Both endpoints run **the same llama.cpp build we pin ourselves**
(`model.image.llamacpp.url`), and the embedder serves **the same GGUF and
quantization as the local stack** — which matters, because the memory gates'
similarity thresholds were calibrated against exactly that model
(docs/research/embedding-model-change-reindex.md §8.2).

**As built** (2026-07-28), with three departures from the sketch above, each
earning its keep:

- **`tools/hf_api.py`** — the client (HTTP, create payloads, wait/delete/verify,
  cleanup) was **extracted and shared** with `tools/hf_probe.py` rather than
  copied into the runner. Two copies of the create payload would drift the
  moment the schema moved, and two copies of the cleanup would mean two places
  where a bug leaks a billing GPU.
- **The tests are compiled before the GPU exists.** `cargo test --no-run` runs
  first (`--no-prebuild` opts out), so a cold CI runner does not spend minutes of
  billed L40S time linking, and a build error costs nothing at all.
- **Cleanup sends every DELETE before verifying any of them.** A cancelled CI job
  gives the handler ~7.5 s before SIGKILL; the calls that actually stop the meter
  must not queue behind a confirmation round-trip for the previous endpoint.

Design points that are decisions, not details:

- **`finally`, and a **verified** delete.** Reporting "deleted" without checking
  is how leaks start. If deletion fails the script exits non-zero **even if the
  tests passed** — a leaked endpoint is a failure.
- **Deterministic names** (`e2e-chat-<run-id>`, `e2e-embed-<run-id>`) written to
  the log *before* the create call, so an orphan is identifiable even if the
  create response is lost.
- **`--keep` flag** for debugging a red run, and `--reuse-chat` / `--reuse-embed`
  to attach to existing endpoints (iterating on a smoke without paying a deploy
  each time). A reused endpoint is never deleted — it is not this run's to free.
- **Named after the runner, not the platform**, so a RunPod backend could be
  added later without renaming anything.

**`.github/workflows/e2e-live.yml`** — `workflow_dispatch` only (R5a), with
inputs for the test filter, the GPU and whether to skip the embedder. Needs
`HF_TOKEN`, and `npx` for the MCP smokes (whose npm cache the job warms *before*
the GPU exists, since a cold `npx` can outlast the smoke's 120 s readiness
timeout). Its `timeout-minutes` (45) matters in one direction only: it must stay
**below the sweeper's threshold** (90 min), or the sweeper would delete the
endpoints of a run still using them.

**`.github/workflows/e2e-sweeper.yml`** — hourly `cron` + `workflow_dispatch`:
list endpoints, delete anything named `e2e-*` older than the max lifetime. This
is the reduced §6 of the research (R3): HF's auto scale-to-zero already bounds
the *money* at one idle window; the sweeper bounds the *time* and reclaims
endpoint quota, which scale-to-zero does not.

**Docs:** `docs/install.md` (running the live gate, the new env vars),
CLAUDE.md journal entry, `README.md` env table. **No CHANGELOG entry** — dev
infrastructure with no user-visible effect (AGENTS.md §4).

## 7. Stage 3 — the second embedder (done)

Four smokes (`embed_guard` ×2, `reembed`, `embed_prefix`) need a *second,
different* embedding model via `MINDFORK_EMBED_URL_ALT` — they are the guards for
the embedding-model-change track, i.e. exactly the memory-critical ones.

**As built**: a third llama.cpp endpoint (`mode: "embeddings"`) on a T4 serving
`Ralriki/multilingual-e5-large-instruct-GGUF` / `…-q8_0.gguf` — the same
quantization as the local stand, because the calibration figures
(`REFERENCE_UNRELATED` / `REFERENCE_PARAPHRASE`) were measured against that file.
`MINDFORK_EMBED_URL_ALT` + `MINDFORK_EMBED_KEY_ALT` are set from it; **no Rust
change was needed** — stage 1 had already routed those smokes through
`live_client`, key variable included.

Two decisions worth recording:

- **On by default** (`--no-alt-embed` opts out), against the plan's original
  "optional". Off by default would leave the four smokes skipping *while
  reporting ok*, which is the exact failure a gate exists to prevent; ~$0.13 of a
  ~$1 run is the wrong thing to optimise. It is bound to the primary embedder —
  a second model needs the first to be compared against.
- **The context is shared with the primary embedder** rather than clamped to
  e5's `n_ctx_train` of 514. llama.cpp accepts a larger context (the local stand
  runs this model on the default 4096), and what actually breaks embeddings is
  too *small* a physical batch, not too large a context — the "chunks: 0" bug
  the journal records.

## 8. Cost and the guarantee

Per run: L40S 48 GB $1.80/hr × ~30 min + T4 $0.50/hr ≈ **~$1.00**. The $20
balance is ~20 runs; at `workflow_dispatch`-only usage that is months.

The leak ceiling, restated for this design: a crashed run leaves the endpoints
**running but idle**; HF scales them to zero after the idle window (1 h default,
shorter if configured) and a scaled-to-zero endpoint costs nothing. Worst case
≈ one idle hour ≈ **$2.30**, with no code of ours involved. The sweeper cuts it
further. Nothing here can run away — which is the whole reason this platform was
chosen over a rented pod.

## 9. Definition of Done (per stage)

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  green; `python tools/cyrillic_scan.py` clean.
- Stage 1: unit tests as in §5; local live regression showing the unset-key path
  is unchanged.
- Stage 2 — **met, 2026-07-28**: **69 passed / 0 failed** (41 of them actually
  against the endpoints) on `gemma-4-31B_q4_0-it.gguf` (nvidia-l40s x1) +
  `bge-m3-q8_0.gguf` (nvidia-t4 x1), ready in 41 s, suite 929 s, total 970 s,
  ≈ $0.62, both endpoints deleted and verified. **Failure drill**: signalled
  58 s into a live suite → the handler deleted and verified both, exit 130, a
  separate process confirmed none remained. The drill also found a real leak
  first — see the journal entry.
- Docs updated per AGENTS.md §4; commits carry the model trailer; the PR body
  follows the template with a "Models" section.
- On completion of the track this plan moves to `docs/history/` and the
  references in CLAUDE.md are updated.

## 10. Risks

- ~~**U1 (GGUF selection) can force a redesign**~~ — **resolved in stage 0**:
  `modelPath` is an explicit field, no `custom_image` fallback needed.
- ~~**Unpinned llama.cpp `master`**~~ — **retired in stage 0**: the container
  `url` is ours, so the build is pinned to a tag. Keep the tag current
  deliberately (a bump is a one-line PR), and remember that a *newly pinned*
  build turning a smoke red may be upstream drift rather than our regression.
- **Non-hermetic smokes** (`web_search`, `fetch_url`, MCP's first `npx`) flake
  independently of the GPU; the MCP readiness timeout on a cold npm cache is
  already recorded in the journal.
- **`HF_TOKEN` is one credential doing two jobs** (managing endpoints and
  authenticating requests). Use a fine-grained token scoped to Inference
  Endpoints, separate from any interactive token, so revoking it costs nothing.
</content>
