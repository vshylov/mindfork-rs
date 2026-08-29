# Research: gpt-oss-120b (split Q8_0) on the live e2e gate

**Status:** research, **forks F1–F6 open** — nothing is built and nothing has
been deployed. Measured entirely from the Hugging Face APIs (model metadata,
the endpoints provider catalogue, the account's quotas and the endpoints
OpenAPI schema), 2026-08-29; **no endpoint was created, nothing was billed.**
**Date:** 2026-08-29.
**Extends:** [docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md) (the
gate itself, stages 0–3) and
[docs/history/e2e-second-chat-model.md](../history/e2e-second-chat-model.md)
(how a second chat model was added — the shape this one follows).
**Journal:** [docs/journal/ci.md](../journal/ci.md).

## 1. Why this model, and not just a third of the same kind

The gate today runs two dense multimodal models of the same size class
(`gemma-4-31B` q4_0, `Qwen3.6-27B` Q4_K_M), each a single 17–19 GB file from a
single-quantization repository. Three things it therefore never exercises,
all three of them shipped code:

- **A model whose weights are split across several files.**
  [`src/shared/gguf.rs`](../../src/shared/gguf.rs) exists for exactly that case —
  it parses the `-00001-of-00002` tail, rebuilds every part's path for the
  launcher's preflight, and strips the tail for display. Its own doc comment
  uses `gpt-oss-120b-Q8_0-00001-of-00003` as the example. It has **never met a
  live split model**: every assertion about it is a unit test over strings.
- **The name a split model reports.** `EngineBackend::model_id` asks
  `GET /v1/models`, then `/props`, and passes the answer through
  `gguf::display_id` (docs/research/external-model-name.md). Against a split
  model an un-aliased `llama-server` reports *the first part's path*, which is
  the one input that normalization exists for — and the one the feed caption,
  the message metadata snapshot, `get_llm_name` and the `llm_history` records
  all inherit (docs/research/language-model-history.md, spec §9.14).
- **An OpenAI open-weights model.** gpt-oss is the harmony chat template:
  three channels, `analysis` mapped to `reasoning_content` by llama.cpp, and a
  `reasoning_effort` knob in place of the `enable_thinking` /
  `reasoning_budget` pair that Gemma and Qwen answer to. The app sends all
  three fields (`shared/api/openai/wire.rs`); which of them this family
  actually honours is unmeasured.

That is three uncovered dimensions in one dispatch, which is what makes the
model worth the money rather than a fourth flavour of the same run.

## 2. What the model actually is (Hub API, 2026-08-29)

`unsloth/gpt-oss-120b-GGUF` — public, ungated, **31 GGUF files, 1010 GB in
total** across 13 quantizations. The set that matters:

| Path | Size |
|---|---|
| `Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` | 49.61 GB |
| `Q8_0/gpt-oss-120b-Q8_0-00002-of-00002.gguf` | 13.78 GB |
| | **63.39 GB** |

Two facts worth having before choosing anything:

- **Every quantization in that repository is 62–65 GB.** Q2_K is 62.6 GB and
  Q8_0 is 63.4 GB — a 1.3 % spread across the whole ladder, because gpt-oss
  ships its MoE experts in MXFP4 natively and the quantization only touches
  the dense tensors. So Q8_0 is the *highest-fidelity* member of the family at
  no size penalty, and "pick a smaller quant to fit a smaller card" is not a
  lever that exists here. The user's choice of Q8_0 is the right one, and it
  is right for a reason worth writing down.
- **`ggml-org/gpt-oss-120b-GGUF` ships the same 63.39 GB as one file**
  (`gpt-oss-120b-MXFP4.gguf`). That is the natural **control arm**: if a smoke
  goes red on the split model, the same model as a single file separates
  "sharding broke it" from "gpt-oss broke it" for one more deploy. It is not
  the primary, because a single file exercises nothing new.

The 1010 GB is not trivia — see U1.

## 3. Hardware: on this platform the H100 is the wrong ask

The provider catalogue and this account's quotas, read 2026-08-29
(`GET /v2/provider`, `GET /v2/provider/quotas/vshylov`). Everything that can
hold 63.4 GB of weights:

| Vendor / region | Instance | GPU RAM | $/hr | Account quota |
|---|---|---|---|---|
| aws us-east-1 | `nvidia-a100` x1 | 80 GB | **2.50** | 4 |
| aws us-east-2 | `nvidia-rtx-pro-6000` x1 | 96 GB | **2.75** | 4 |
| aws us-east-1 | `nvidia-l4` x4 | 96 GB | 3.80 | 16 |
| aws us-east-1 | `nvidia-a10g` x4 | 96 GB | 5.00 | 16 |
| aws us-east-1 | `nvidia-a100` x2 | 160 GB | 5.00 | 4 |
| **aws us-west-2** | **`nvidia-h200` x1** | **141 GB** | **5.00** | **2** |
| gcp us-east4 | `nvidia-h100` x1 | 80 GB | 10.00 | **0** |

**The H100 is not available and would not be the cheap option if it were.** It
exists on this platform only in `gcp/us-east4`, at $10.00/hr — *twice* the
price of the 141 GB H200 — and this account's H100 quota is **0**, so a create
would be rejected. The H200 the user rented on RunPod as "excessive" is, here,
the cheapest card that removes every memory question at once.

Two things also cut against the RunPod sizing:

- **The gate runs at `ctxSize 16384`, not 128 k.** That is the context both
  existing models run at, and it is what the suite's longest smokes
  (attachments, reflection digests, compaction) were calibrated against. The
  KV cache at 16 k is a rounding error next to 63.4 GB of weights, so the
  <70 GB measured at 128 k is an *upper* bound with a lot of air in it.
- **`nvidia-a100` x1 (80 GB, $2.50/hr) is therefore a real candidate**, as is
  the 96 GB `nvidia-rtx-pro-6000` at $2.75/hr. Both have a caveat the H200 has
  not: the A100 is Ampere and dequantizes MXFP4 rather than running it
  natively, and the RTX PRO 6000 is Blackwell (sm_120), which the
  `ghcr.io/ggml-org/llama.cpp:server-cuda` image may or may not carry kernels
  for. Neither caveat is fatal; both are unmeasured, and each costs a failed
  deploy to find out.

Recommendation in F2: **H200 for the first run** (nothing about the hardware
can then be the reason a smoke is red), and treat the A100 as the cost
optimization to attempt *after* one green run exists to compare against.

Note the region: the H200 is `us-west-2`, while the gate's endpoints default
to `us-east-1`. `--region` is a single flag shared by all three endpoints
today, so either the model record carries its own region (F1) or the embedders
move west with it. Cross-region is only latency, not correctness, but the flag
has to be able to say it.

## 4. Unknowns to settle before any product code (stage 0)

The gate's own precedent is that a track of this shape starts with a probe
that answers the schema questions against the live API rather than guessing
them inside a PR (remote-e2e-hf.md §3). Six questions, in the order that can
kill the idea:

**U1 — does the download honour a file filter?** `LlamacppContainer` in the
endpoints OpenAPI schema has an undocumented field:

```
"variant": { "type": ["string","null"],
             "description": "Pattern of .gguf files to load", "example": "*" }
```

The public documentation page does not mention it at all; it does document the
failure this repository would otherwise produce — *"Workload evicted, storage
limit exceeded"*. With 1010 GB in the repository and no filter, that is the
likely outcome. **This is the blocking unknown**: if `variant` cannot restrict
the pull to `Q8_0/*`, the fallback is mirroring the two shards into a
single-quant repository under the user's namespace (a 63 GB upload, once), and
that changes the cost and the shape of the track.

**U2 — does llama.cpp pick up part 2 by itself** from
`/repository/Q8_0/…-00002-of-00002.gguf`, given only part 1 on `modelPath`?
It does locally; what is unmeasured is whether both parts are *present* in the
container, which is U1 wearing a different hat, and whether a `modelPath` with
a directory component is accepted at all (every payload sent so far has been a
bare file name).

**U3 — what does the server call itself?** The whole point of the run. The
prediction is that `/v1/models` reports
`/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` and `display_id`
turns it into `gpt-oss-120b-Q8_0`. It has to be *observed* before T2 (§6) can
be written, because a test that asserts the raw shape is worthless if the
image sets an alias of its own. **`LLAMA_ARG_ALIAS` is not on HF's reserved
list**, so we could name the model ourselves — and deliberately will not:
setting it would route around the exact normalization this run exists to
exercise.

**U4 — deploy time and the timeout coupling.** Stage 0 measured 21 s to
`running` for a 17.65 GB model, which implies a ~840 MB/s pull; 63.4 GB at that
rate is ~75 s plus the load. If it is much worse, three numbers move together
and must keep their order: the runner's `--timeout` (1500 s), the workflow's
`timeout-minutes` (45) and the sweeper's `--max-age-minutes` (90). The
invariant is that the sweeper's threshold stays above every job timeout, or the
backstop starts deleting endpoints that are still in use (F4).

**U5 — harmony.** Does `LLAMA_ARG_JINJA=1` give `finish_reason=tool_calls` on
this template; does `analysis` arrive as `reasoning_content`; and — the one
with teeth — **does `reasoning_budget=0` mute anything here?** Two client
smokes (`control_tools_are_callable`, `tool_call_is_emitted_and_parsed`) were
made deterministic on Qwen by muting thinking that way
(e2e-second-chat-model.md §2). gpt-oss has no `enable_thinking`; it has
`reasoning_effort`, which the app already sends. If the mute is a no-op on this
family, those two smokes are the ones most likely to go red, and the fix is
already sketched — send `reasoning_effort: low` where the mute is meant.

**U6 — `nGpuLayers`.** The field is optional and the schema says llama.cpp
picks the value itself, with `--fit` sizing the context to free memory. On a
63.4 GB model an automatic partial offload would not fail — it would run, ten
times slower, and quietly turn a 20-minute suite into a timeout. Pin it.

**How to answer them cheaply.** Split stage 0 in two:

- **0a — cents, no GPU.** One `intel-spr` x1 CPU endpoint ($0.033/hr) against a
  *small* multi-quantization repository, with `variant` set to one quant. Read
  `GET /v2/endpoint/{ns}/{name}/logs` (the API has a logs route, which the
  existing tooling has never used) to see which files were pulled and what
  command line the image actually builds. That settles U1's semantics and most
  of U6 for the price of nothing.
- **0b — one H200 deploy, ~15 minutes, ≈$1.30.** The real model: `/health`,
  `/props` (`n_ctx`), `/v1/models` (U3, recorded verbatim), one tool call and
  one reasoning request (U5), then delete and verify. No suite yet.

## 5. What a full run would cover — and the three smokes it cannot

Of the 128 `#[ignore]` smokes, the llama.cpp set is ~60 and would run as-is.
Three cannot: `image_attachment_e2e_live`, `image_url_attachment_e2e_live` and
`tool_result_image_is_seen_live` require a vision projector and **fail loudly
rather than skip** against a text-only server, by design
([lessons.md](../lessons.md) §9). gpt-oss is text-only and there is no
projector to attach, so as things stand a dispatch on this model is red on
three smokes for a reason that has nothing to do with the model. F3 decides
what to do about it; note that "let them fail" is not free — a gate with three
permanent reds stops being read.

Expected model-sensitive results, from the Qwen precedent: the two thinking
ceilings of U5, and any smoke whose assertion encodes how *chatty* a model is.
The Qwen run found the product needed no change and only test-side assumptions
moved; that is the outcome to expect here too, and finding otherwise is the
point.

## 6. New tests worth adding

**T1 — a name must not be a part number** (free, applies to every stack). The
existing `the_engine_names_the_model_it_is_running_live` already refuses a
name ending in `.gguf` or containing a path separator. One more assertion —
that the displayed name carries no `-00001-of-00002` tail — costs a line, is
vacuously true on every single-file stack, and is the exact regression this
model would catch. It belongs on the existing smoke, not in a new one.

**T2 — the split model, declared.** A new `#[ignore]` smoke that asserts the
*raw* id from `/v1/models` carries a shard tail **and** that `model_id()`
strips it — i.e. that the live path from server to header really is normalized,
not just our fixtures. It must be gated on a declaration
(`MINDFORK_LIVE_SPLIT_MODEL=1`) and, when declared, **fail rather than skip** if
the stack turns out not to be split: the same discipline as the vision smokes,
for the same reason. Its exact assertion waits on U3.

**T3 — the recorded history row carries the normalized name.**
`llm_name_and_history_e2e_live` already drives `get_llm_name` and
`get_llm_history` end to end; against a split model it becomes the check that
what lands in `data.db` is `gpt-oss-120b-Q8_0` and not a container path. Worth
one added assertion on the record's text, not a new test.

Not proposed, deliberately: a live smoke for the launcher's split-model
preflight (all parts present next to part 1). That path is managed mode — a
local child process — and cannot run against a rented endpoint at all
(remote-e2e-hf.md §4).

## 7. Forks

**F1 — how the model is added.**
- **(a) A third `CHAT_MODELS` record in `tools/hf_api.py`, carrying its own
  defaults** (repository, `modelPath`, `variant`, no projector, instance,
  region, ctx, timeouts), with the workflow's `model` input gaining a third
  choice and `chat_instance` gaining an empty default meaning "whatever the
  model says" — *recommended*. It extends fork F1 of the second-model track
  ("a model is one decision, not three independent flags") to the two decisions
  this model adds, and no caller can dispatch a 63 GB model onto an L40S.
- (b) A separate `e2e-gpt-oss.yml` workflow. Duplicates the whole job — the
  toolchain, the ALSA dependency, the npm warm-up, the cleanup evidence step —
  to change four values.
- (c) Keep the record minimal and pass the instance/region by hand at dispatch.
  Cheapest to write, and the first mis-dispatch costs a failed 63 GB deploy.

**F2 — hardware for the first run.**
- **(a) `nvidia-h200` x1, aws us-west-2, $5.00/hr** — *recommended*. 141 GB
  removes memory, MXFP4 support and kernel-availability from the list of things
  that could explain a red run.
- (b) `nvidia-a100` x1, 80 GB, $2.50/hr. Half the price, fits on paper at
  ctx 16 k, and adds "Ampere dequantizes MXFP4" as a variable to a run whose
  purpose is to find variables elsewhere.
- (c) `nvidia-rtx-pro-6000` x1, 96 GB, $2.75/hr. Same argument, plus an
  unverified sm_120 in the pinned image.
- Not an option: `nvidia-h100` — quota 0 on this account, and $10/hr.

**F3 — the three vision smokes on a text-only stack.**
- **(a) A declaration env var (`MINDFORK_LIVE_TEXT_ONLY=1`) that makes exactly
  those three skip, printed loudly by the runner and by each skip** —
  *recommended*. The hazard the current design guards against is a vision smoke
  passing on a blind model; a skip that the run has to *ask for* and that is
  named in the log is not that. Set only by the gpt-oss model record.
- (b) Run a filtered subset instead. `cargo test` has no exclusion filter, so
  this means either enumerating the ~60 wanted smokes or making two passes —
  and a hand-maintained list is a list that silently stops covering new tests.
- (c) Accept three permanent reds on this model. Honest, and it trains everyone
  to read a red gate as normal.

**F4 — the timeouts.** Only if U4 shows a slow deploy.
- **(a) Leave 45/90 as they are** — *recommended if the probe shows a deploy
  under ~5 minutes*.
- (b) Make the job timeout depend on the model
  (`timeout-minutes: ${{ inputs.model == 'gpt-oss-120b' && 75 || 45 }}`) and
  raise the sweeper to 120, preserving the "sweeper above every job timeout"
  invariant.
- (c) Raise both flat. Weakens the backstop for the runs that do not need it.

**F5 — the embedders.** Keep both (the primary and the alternate) on this run
too, at ~$0.5 extra — *recommended*, for the reason stage 3 gave: skipping them
leaves memory-critical smokes reporting ok while testing nothing. The
alternative is `--no-embed`, which turns a $2.6 run into a $2.1 one and drops
roughly half the coverage.

**F6 — scope.** Is gpt-oss-120b
- **(a) a third first-class gate model**, dispatched like the other two when a
  change warrants it — *recommended*; or
- (b) a compatibility run kept for split-model / open-weights questions
  specifically, dispatched a few times a year?
This decides only how the docs describe it — the implementation is identical —
but it is worth answering, because (a) implies keeping its assertions green and
(b) does not.

## 8. Cost

Per full run, at the recommended options: H200 $5.00/hr × ~30 min ≈ **$2.50**,
plus two T4 embedders (~$0.50/hr each × ~30 min) ≈ **$0.50** → **≈ $3.00**,
against ≈$1.00 for a Gemma/Qwen dispatch. Stage 0 adds ≈$1.35 once. On an A100
the same run is ≈$1.75.

The leak ceiling is unchanged and is the reason this stays on this platform: a
run that dies without cleaning up leaves endpoints idle, HF scales them to zero
after 15 minutes, and the sweeper reclaims the quota. Worst case on an H200 is
one idle quarter-hour, ≈$1.25.

## 9. If the forks are confirmed

One branch (`feat/e2e-gpt-oss-120b`), the probe and the implementation sharing
it as stages 0–1 shared `spike/hf-endpoint-probe`:

1. **Stage 0** — `tools/hf_probe.py` runs 0a and 0b (§4); the answers land in
   this document. Go/no-go on U1 and U2.
2. **Stage 1** — the `CHAT_MODELS` record and the `variant` / `nGpuLayers` /
   per-model instance-and-region plumbing in `tools/hf_api.py`, the workflow's
   third choice, T1–T3, and the docs (install.md §7.2, journal/ci.md,
   README env table if a new variable lands). One live dispatch, recorded.
3. **Docs** — this file stays in `docs/research/`; the track's outcome goes to
   the journal, and CLAUDE.md's Status list gains a line if F6 resolves to (a).

**Definition of done** — AGENTS.md §4 plus: `cargo fmt --check`, `clippy -D
warnings`, `cargo test`, `cyrillic_scan.py` green; one dispatched run whose
result is recorded in [docs/journal/ci.md](../journal/ci.md) with the model,
the instance, the timings and every smoke that moved.
