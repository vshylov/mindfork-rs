# Research: gpt-oss-120b (split Q8_0) on the live e2e gate

**Status:** **done, with three open questions.** Forks F1–F6 resolved
(*user's decision, 2026-08-29*); the runner, the workflow and the tests are built
on `feat/e2e-gpt-oss-120b`, and the full suite has been dispatched against the
model — **117 passed, 11 failed, none of them a product defect** (§10). Eight of
the eleven are fixed and re-verified live; three need a decision (§10.2).
**Date:** 2026-08-29.
**Extends:** [docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md) (the
gate itself, stages 0–3) and
[docs/history/e2e-second-chat-model.md](../history/e2e-second-chat-model.md)
(how a second chat model was added — the shape this one follows).
**Journal:** [docs/journal/ci.md](../journal/ci.md).

**Total spend: ≈$2.56**, across seven throwaway endpoints — the probes, one
full dispatch and one re-verification. Every endpoint deleted, every deletion
verified.

## 1. Why this model, and not just a third of the same kind

The gate today runs two dense multimodal models of the same size class
(`gemma-4-31B` q4_0, `Qwen3.6-27B` Q4_K_M), each a single 17–19 GB file from a
single-quantization repository. Three things it therefore never exercises,
all three of them shipped code:

- **A model whose weights are split across several files.**
  [`src/shared/gguf.rs`](../../src/shared/gguf.rs) exists for exactly that case —
  it parses the `-00001-of-00002` tail, rebuilds every part's path for the
  launcher's preflight, and strips the tail for display. Its own doc comment
  uses `gpt-oss-120b-Q8_0-00001-of-00003` as the example. It had **never met a
  live split model**: every assertion about it was a unit test over strings.
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
  actually honours was unmeasured — §4, U5, where it turns out to matter.

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
  the primary, because a single file exercises nothing new. (It also ships
  `eagle3-*` draft models, i.e. the `specModelPath` field in the endpoint
  schema has something to point at — an untried lever for a faster suite, noted
  and not taken.)

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

**Since an instance implies a region** (the H200 is only in `us-west-2`,
`rtx-pro-6000` only in `us-east-2`), that mapping is in `tools/hf_api.py` as
`INSTANCE_REGIONS`: naming a GPU is enough, and asking for an H200 in
`us-east-1` is a failed deploy rather than a choice.

## 4. Stage 0 — the unknowns, and what they measured

Four throwaway endpoints, ≈$0.30, every one deleted and the deletion verified.

**U1 — does the download honour a file filter? YES, and it is the whole
track.** `LlamacppContainer` in the endpoints OpenAPI schema carries an
undocumented field:

```
"variant": { "type": ["string","null"],
             "description": "Pattern of .gguf files to load", "example": "*" }
```

HF's own documentation page never mentions it; it does document the failure a
1010 GB repository would otherwise produce (*"Workload evicted, storage limit
exceeded"*). Measured decisively, on `unsloth/gemma-3-270m-it-GGUF` (24 quants,
6.4 GB, ~240 MB each) on a `intel-spr` CPU endpoint costing fractions of a cent:

| `variant` | `modelPath` | Result |
|---|---|---|
| `*Q4_K_M*` | `…-Q4_K_M.gguf` | healthy in **22 s** |
| `*Q2_K*` | `…-Q4_K_M.gguf` | **failed in 21 s**: `gguf_init_from_file: failed to open GGUF file '/repository/gemma-3-270m-it-Q4_K_M.gguf' (No such file or directory)` |

The mismatch is the proof: a file that does not match `variant` **is not on
disk**. So `variant` governs the pull, and `"Q8_0/*"` is what keeps a 1010 GB
repository down to 63.39 GB.

**U2 — a directory in `modelPath`, and part two found by itself. YES.** The
H200 deploy loaded `/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf`
and `/props` reported `n_ctx=16384`: `variant: "Q8_0/*"` pulled both parts,
a `modelPath` with a directory component is accepted (every payload before this
one was a bare file name), and llama.cpp read part two out of the same
directory on its own.

**U3 — what the server calls itself.** No alias is set by the image, on any of
the four probes: `/v1/models` reports the path as given.

| Deployed | `/v1/models` id | After `gguf::display_id` |
|---|---|---|
| gemma-3-270m (flat) | `/repository/gemma-3-270m-it-Q4_K_M.gguf` | `gemma-3-270m-it-Q4_K_M` |
| gpt-oss-20b (flat) | `/repository/gpt-oss-20b-Q8_0.gguf` | `gpt-oss-20b-Q8_0` |
| **gpt-oss-120b (split, subdir)** | `/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` | **`gpt-oss-120b-Q8_0`** |

That last row is the point of the track, and it now runs end to end: the live
smokes reported `live model name: Some("gpt-oss-120b-Q8_0")`, the stored
reply's metadata snapshot carried the same string, and `get_llm_history`
answered with one dated record — `2026-08-29 20:04 UTC — gpt-oss-120b-Q8_0
(external)`.

**`LLAMA_ARG_ALIAS` is not on HF's reserved list**, so the model could be given
a clean name at the container. Deliberately not done: an alias would route
around the exact normalization this run exists to exercise.

**U4 — deploy time. 63.39 GB reaches `running` in 42 s and `/health` in 93 s.**
Nothing about the timeouts has to move: the workflow's `timeout-minutes` (45)
and the sweeper's threshold (90) keep their order and their headroom.

**U5 — harmony, and the one finding with teeth.** `LLAMA_ARG_JINJA=1` gives
`finish_reason: tool_calls` on this template, and the `analysis` channel
arrives as `reasoning_content`. But **`reasoning_budget: 0` +
`chat_template_kwargs.enable_thinking: false` mutes nothing here** — the pair
the Qwen track used to make two client smokes deterministic
(e2e-second-chat-model.md §2). `reasoning_effort` is the lever on this family:

| Request | reasoning | text | 20b | 120b |
|---|---|---|---|---|
| plain | — | — | 160 ch | 201 ch |
| `reasoning_budget=0` + `enable_thinking=false` | — | — | **324 ch** | **103 ch** |
| `reasoning_effort: "low"` | — | — | **20 ch** | **25 ch** |

Measured first on `gpt-oss-20b` (the same template, 12 GB, on an `nvidia-l4` at
$0.80/hr) precisely so the expensive model would not be where this was
discovered. It did not become a red smoke — `control_tools_are_callable` and
`tool_call_is_emitted_and_parsed` are both green on the 120b, because gpt-oss
deliberates briefly where Qwen would not stop — so **nothing was patched**. It
is recorded because it is a live tripwire: if either goes flaky on this model,
the fix is `reasoning_effort: low`, not a bigger token ceiling.

**U6 — `nGpuLayers`.** The image runs llama.cpp's `--fit` when the field is
absent (visible in the failure log as `common_fit_params`), which on a 63 GB
model would not fail — it would quietly offload part of it to the CPU. Pinned
to 9999 **in the model's record only**, so the two models the gate was measured
on keep the payload they were measured with, byte for byte.

Two more things the probes handed over for free: the API resolves
`model.revision` to a commit SHA at create time (so a deploy is pinned to a
revision without asking), and `GET /v2/endpoint/{ns}/{name}/logs` returns the
container's log — which is how U1 was settled and is the fastest way to read a
failed deploy.

## 5. What a full run covers — and the three smokes it cannot

Of the 128 `#[ignore]` smokes, the llama.cpp set is ~60 and runs as-is. Three
cannot: `image_attachment_e2e_live`, `image_url_attachment_e2e_live` and
`tool_result_image_is_seen_live` require a vision projector and **fail loudly
rather than skip** against a text-only server, by design
([lessons.md](../lessons.md) §9). gpt-oss is text-only and no projector for it
exists — `/props` on the deployed endpoint reports
`modalities: {vision: false}` — so without F3 a dispatch on this model would be
red on three smokes for a reason that has nothing to do with the code.

Note that `/props` *could* be used to skip them automatically, and deliberately
is not: a stand accidentally started without `--mmproj` reports exactly the same
thing, and turning that into a silent skip is the failure the rule exists to
prevent. The declaration is a property of the model, not of the server's answer.

**Rehearsed, 2026-08-29** — 27 of those smokes were run against the probe's own
H200 endpoint before any of this was committed: the whole low-level client set
(23, of which 13 are honest skips for want of cloud keys) plus
`props_reports_the_context_window_live`,
`the_engine_names_the_model_it_is_running_live`,
`a_reply_records_the_discovered_model_live` and `llm_name_and_history_e2e_live`.
**All green**, 53 s of test time, 187 s of endpoint life, ≈$0.26.

**And then the built runner, end to end** —
`python tools/e2e_hf.py run --chat-model gpt-oss-120b --no-embed`: ready in
83 s, both declarations derived and printed
(`MINDFORK_LIVE_TEXT_ONLY=1`, `MINDFORK_LIVE_SPLIT_MODEL=1`), the new split
smoke green (`/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` in,
`gpt-oss-120b-Q8_0` out), the name smoke green, `image_attachment_e2e_live`
skipping and naming the variable, endpoint deleted and verified. ≈$0.12.

## 6. The three tests this adds

**T1 — a name must not be a part number** (free, applies to every stack).
`the_engine_names_the_model_it_is_running_live` already refused a name ending
in `.gguf` or containing a path separator; it now also refuses one that still
carries a `-00001-of-00002` tail. One assertion, vacuously true on every
single-file stack, and the exact regression this model would catch. It reuses
`gguf::parse_shard` (putting the extension back to ask the question) rather than
re-implementing the tail shape, because that module is deliberately the only
place the shape is known.

**T2 — the split model, declared** —
`a_split_model_is_named_by_the_model_not_the_part_live`, in the low-level client
smokes. It asserts *both* halves: that `GET /v1/models` really did report a
**part** (index 1 of >1), and that `model_id()` hands back a name with the
directory, the extension and the part number gone. Gated on
`MINDFORK_LIVE_SPLIT_MODEL=1` and, when declared, it **fails rather than skips**
if the server turns out to report a plain file — a declaration that is quietly
wrong is worse than none. Without the declaration a "no part number in the name"
check would pass on every single-file stack forever, which is precisely the
state this track found the gate in.

**T3 — the recorded history row carries the normalized name.** No new test:
`llm_name_and_history_e2e_live` already drives `get_llm_name` and
`get_llm_history` end to end, and against this model it *is* the check that what
lands in `data.db` is `gpt-oss-120b-Q8_0` and not a container path. Verified in
the rehearsal above.

Not added, deliberately: a live smoke for the launcher's split-model preflight
(all parts present next to part one). That path is managed mode — a local child
process — and cannot run against a rented endpoint at all
(remote-e2e-hf.md §4).

**Neither declaration is a workflow input.** Both `MINDFORK_LIVE_TEXT_ONLY` and
`MINDFORK_LIVE_SPLIT_MODEL` are derived by `tools/e2e_hf.py` from the model
record it just deployed — from `mmproj: None` and from the `-00001-of-00002`
tail on the weights file — so a declaration cannot disagree with the stack it
describes. `--no-mmproj` pointedly does **not** set the first one: that flag
exists to deploy a *sighted* model blind, and those three smokes failing is its
entire purpose.

## 7. Forks — all resolved (*user's decision, 2026-08-29*)

**F1 — how the model is added → (a), as built.**
- **(a) A third `CHAT_MODELS` record in `tools/hf_api.py`, carrying its own
  defaults** (repository, `modelPath`, `variant`, no projector, `nGpuLayers`,
  instance, region), with the workflow's `model` input gaining a third choice
  and `chat_instance` gaining a `model-default` option. It extends fork F1 of
  the second-model track ("a model is one decision, not three independent
  flags") to the four decisions this model adds, and no caller can dispatch a
  63 GB model onto an L40S.
- (b) A separate `e2e-gpt-oss.yml` workflow. Duplicates the whole job — the
  toolchain, the ALSA dependency, the npm warm-up, the cleanup evidence step —
  to change four values.
- (c) Keep the record minimal and pass the instance/region by hand at dispatch.
  Cheapest to write, and the first mis-dispatch costs a failed 63 GB deploy.

**F2 — hardware → (a) `nvidia-h200` x1, aws us-west-2, $5.00/hr.** 141 GB
removes memory, MXFP4 support and kernel availability from the list of things
that could explain a red run. (b) A100 80 GB at $2.50/hr and (c) RTX PRO 6000
96 GB at $2.75/hr stay on the table as the cost optimization to attempt *after*
a green run exists to compare against — each adds an unmeasured variable to a
run whose purpose is to find variables elsewhere. Not an option:
`nvidia-h100` — quota 0 on this account, and $10/hr.

**F3 — the three vision smokes on a text-only stack → (a) a declaration.**
`MINDFORK_LIVE_TEXT_ONLY=1` makes exactly those three skip, printed by the
runner and by each skip. The hazard the current design guards against is a
vision smoke *passing* on a blind model; a skip the run has to ask for, by name,
in the log, is not that. (b) A filtered subset — `cargo test` has no exclusion
filter, so it means enumerating ~60 smokes by hand, and a hand-maintained list
silently stops covering new tests. (c) Three permanent reds — honest, and it
trains everyone to read a red gate as normal.

**F4 — the timeouts → (a) leave 45/90 unchanged.** U4 measured the 63.39 GB
deploy at 93 s to healthy, so the conditional timeout of option (b) and the
flat raise of (c) would both weaken a backstop to buy headroom nothing needs.

**F5 — the embedders → keep both**, at ~$0.5 extra, for the reason stage 3
gave: skipping them leaves memory-critical smokes reporting ok while testing
nothing.

**F6 — scope → (a) a third first-class gate model**, dispatched like the other
two when a change warrants it, and expected to stay green.

## 8. Cost

Per full run, at the resolved options: H200 $5.00/hr × ~25 min ≈ **$2.10**, plus
two T4 embedders (~$0.50/hr each × ~25 min) ≈ **$0.40** → **≈ $2.50**, against
≈$1.00 for a Gemma/Qwen dispatch. Stage 0 and the verification run cost ≈$0.42 in total. On an A100 the
same run would be ≈$1.50.

The leak ceiling is unchanged and is the reason this stays on this platform: a
run that dies without cleaning up leaves endpoints idle, HF scales them to zero
after 15 minutes, and the sweeper reclaims the quota. Worst case on an H200 is
one idle quarter-hour, ≈$1.25.

## 9. State, and what is left

1. **Stage 0 — done, GO.** Four probe endpoints, U1–U6 answered above, ≈$0.30,
   nothing left running.
2. **Stage 1 — built** on `feat/e2e-gpt-oss-120b`: the `CHAT_MODELS` record and
   the `variant` / `nGpuLayers` / per-model instance-and-region plumbing in
   `tools/hf_api.py`, the two derived declarations and the plan line in
   `tools/e2e_hf.py`, the workflow's third choice and its `model-default`
   instance, T1 and T2, and the docs.
3. **Stage 2 — the dispatch, done** (§10), and its eight repairs re-verified
   live. Three questions left open, listed in §10.2.

**Definition of done** — AGENTS.md §4 plus: `cargo fmt --check`,
`clippy -D warnings`, `cargo test`, `cyrillic_scan.py` green; the dispatch
recorded in [docs/journal/ci.md](../journal/ci.md).

## 10. The first full dispatch — 117 passed, 11 failed

`python tools/e2e_hf.py run --chat-model gpt-oss-120b`, 2026-08-29: three
endpoints, ready in 106 s, suite 906 s, all three deleted and verified. ≈$1.69.

**Not one of the eleven is a product defect.** That is the result this model was
added for: every failure is an assumption the two existing models happened to
satisfy.

### 10.1. Fixed, and re-verified live

| Cause | Smokes | What it was |
|---|---|---|
| **U+2011 instead of `-`** | `attachment_read`, `attachment_search`, `cross_chat_search` | The model found the planted fact and printed `ZARYA‑8823` with a **non-breaking hyphen**; the assertion was a literal `contains("ZARYA-8823")`. 8 of 18 occurrences in one run, **the same model producing both glyphs** — so these were latent flakes on any model, not a property of this one. Fixed by `mentions_code`, which folds the Unicode dashes on both sides and stays strict about everything else. |
| **A probe module with no key** | `dialogue_probe` ×4 | `engine()` built its client with `OpenAiClient::new` and the `cache_slots` arm posted raw — neither carried `MINDFORK_ENGINE_KEY`, so every request was a `401`. Written against an unauthenticated LAN stand *after* stage 1 routed six other files through `live_client`, and never dispatched since: it would have failed identically on Gemma. Fixed via `live_client` and a new shared `live_bearer`, which also replaces `continue_probe`'s private copy of the same four lines. |
| **A 16-token ceiling** | `external_authenticated_server_takes_the_stored_key_live` | `"Say OK."` with `max_tokens: 16`: on a reasoning model the whole budget goes to the thinking channel, leaving empty text and a red smoke that says nothing about keys. The same shape as the Qwen ceiling (e2e-second-chat-model.md §2); raised to 512. Muting is not available here — §4, U5. |

Re-verified against a fresh H200, 2026-08-29: **all eight green** (5 min, ≈$0.45).
The supervisor smoke's control arm still gets its `401` without a key, so the
authenticated arm still proves what it claims.

### 10.2. Open — each changes what a test asserts *for every model*

- **`code_workspace_navigate_e2e_live`** — turn 2 asks how many lines
  `src/config.rs` has; the model answered **correctly** — six lines, and the first one quoted
  verbatim — without calling `code_read`, because turn 1's `code_search`
  had already put the file in the conversation. This is the *second* instance of
  a pattern already recorded for `attachment_read` in remote-e2e-hf.md §3: an
  assertion that a specific tool must be used holds only when the earlier turn
  happens not to have answered it. The fix proposed there — run the smoke in a
  configuration where the tool is the only route — applies here too, and is a
  test redesign rather than a patch.
- **`fetch_url_address_policy_e2e_live`** — "I'm unable to fetch that URL", with
  no tool call, so the address policy was never exercised. gpt-oss refuses
  markedly more readily than the other two (`file_attachment_e2e_live`'s baseline
  arm answered "I'm sorry, but I can't help with that" and still passed, because
  there a refusal *is* the control). Firming the prompt would change what the
  smoke asks of Gemma and Qwen as well.
- **`continue_probe::prefill_coexists_with_the_tool_grammar`** — after a prefill,
  the model emitted the tool call as JSON **text** rather than a parsed call. Its
  own message calls that "F5 no-go evidence": this is a stage-0 research probe
  recording a capability, and the capability is genuinely absent on this
  template. Its sibling in the same module already skips on a model it does not
  apply to (`thinking_model_rejects_prefill_and_the_kwarg_lifts_it` printed
  *"server model is …gpt-oss…, not a Qwen thinking model"*), which is the shape
  this one probably wants — but turning a red into a skip is exactly the move
  that needs a decision rather than an edit.
