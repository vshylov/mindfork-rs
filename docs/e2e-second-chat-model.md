# Design plan: a second chat model on the live e2e gate (Qwen 3.6 27B)

**Status:** open — stages 1–2 implemented and green on both local stands; forks
F1–F4 all resolved to the recommended option (*user's decision, 2026-08-15*).
Outstanding: one live CI dispatch per model (§7).
**Date:** 2026-08-15.
**Extends:** [docs/history/remote-e2e-hf.md](history/remote-e2e-hf.md) (the gate
itself, stages 0–3) and its research
[docs/research/remote-e2e-gpu.md](research/remote-e2e-gpu.md).
**Journal:** [docs/journal/ci.md](journal/ci.md).

## 1. Goal

The live gate runs on exactly one chat model — `gemma-4-31B` q4_0. Everything it
asserts about a model is therefore asserted about *that* model, and an assumption
shaped by it passes unnoticed. Run the same suite on a second family — Qwen 3.6
27B — so that a Gemma-shaped assumption is caught by the gate instead of by a
user on a different model.

**Non-goal:** replacing Gemma. The memory gates' similarity thresholds and most
of the suite's calibration were measured against that stack; the second model
adds a dimension, it does not move the baseline.

## 2. What the local run established (2026-08-15)

Stand: `llama-server` on the LAN box — `Qwen3.6-27B-Q4_K_M.gguf` with
`mmproj-Qwen3.6-27B-Q8_0.gguf`, `-c 16384 --jinja` (`/props`:
`modalities.vision=true`), plus `bge-m3-Q8_0` as the embedder. Both files come
from `ggml-org/Qwen3.6-27B-GGUF`, so the endpoint in §4 can serve the *same
files* the local stand does.

**First run: 95 passed, 2 failed** of 97 (40 of the passes are honest skips —
no cloud keys, no sandbox dir, no alternate embedder, no `MINDFORK_LLAMA_BIN`),
so 57 smokes actually met the stack, 55 green.

**The product needed no change.** All 44 orchestrator smokes were green on the
first run — memory, self-model, notes, attachments, compaction, MCP, images,
retry. The app runs on Qwen 3.6 as it stands. Both failures were in the
low-level `shared::api::openai::client::ignored_smoke` set, and both were the
same thing: a token ceiling sized for how much Gemma thinks.

| Smoke | Measured on Qwen 3.6 | Resolution |
|---|---|---|
| `simple_generation` | `max_tokens=64` went entirely to `reasoning_content`; empty text, `finish=Length`. Thinking on this trivial prompt costs 357–949 characters | ceiling → 1024, thinking left **on** (~4x margin over the widest run; both streams still exercised) |
| `control_tools_are_callable` | the ceiling does not converge — 1024: 0/3 runs called the tool, 2048: 2/3, 4096: 3/4, one failing run burning all 4096 tokens on thinking over 129 s | `reasoning_budget=0` at the original 512 → 3/3 in ~1.5 s |
| `tool_call_is_emitted_and_parsed` | *already flaky before any edit*: the model emits the correct call, then repeats it (15x observed) to the ceiling → `Length`. 2/11 failures at the server's default temperature, **5/20** at the orchestrator's 0.1, **0/20** with thinking muted | `reasoning_budget=0` |

Two of those measurements are worth keeping, because each killed an option that
looked obvious:

- **A bigger ceiling does not fix an open-ended prompt.** `control_tools` asks
  for a space fact *and then* a tool call; a reasoning model deliberates without
  bound, so every ceiling is a coin flip and each red run costs 2+ minutes of
  billed GPU.
- **Temperature is not the lever for the repetition.** Matching the
  orchestrator's 0.1 made `tool_call_is_emitted_and_parsed` *worse* (5/20 vs
  2/11). The repetition rides on the thinking loop.

Muting thinking in those two costs no coverage: tool calls *with* thinking on
are exercised by the orchestrator smokes, which run the real app path with tool
results fed back and `thinking` left at the server's default — the stronger test,
and green on both families.

**After the fix: 97 passed, 0 failed**, 1304 s (down from 1460 s — the muted
thinking gave back ~2.5 minutes). Unit tests 2282 green, 97 `#[ignore]`.

**No regression on Gemma** (*user's decision, 2026-08-15*: verify on a local
stand rather than pay for an HF dispatch). Against `gemma-4-31B_q4_0-it.gguf`
with its projector, `-c 16384`: the whole client smoke set — the three edited
ones plus five neighbours — **5 runs, 8/8 each**. The edits are worth nothing to
Gemma and cost it nothing, which is what "model-agnostic" has to mean.

## 3. The finding that widens the scope: the gate is blind

Three `#[ignore]` smokes require a projector — `tool_result_image_is_seen_live`,
`image_attachment_e2e_live`, `image_url_attachment_e2e_live` — and against a
text-only server they **fail loudly rather than skip**, deliberately: a vision
smoke that quietly passes on a blind model is worse than none
([docs/lessons.md](lessons.md) §9).

`hf_api.chat_payload` omits `mmprojModelPath` just as deliberately — "the repo's
second file is a vision projector we do not want". That was written in stage 0,
**before the images track existed**, and nothing has reconciled the two since;
the gate has not been dispatched against the image smokes at all. As it stands
today it would go red on those three, with or without Qwen.

The fix is one payload field, and both repositories already ship the file:

| Model | Weights | Projector |
|---|---|---|
| `google/gemma-4-31B-it-qat-q4_0-gguf` | `gemma-4-31B_q4_0-it.gguf` 17.65 GB | `gemma-4-31B-it-mmproj.gguf` 1.20 GB |
| `ggml-org/Qwen3.6-27B-GGUF` | `Qwen3.6-27B-Q4_K_M.gguf` 19.10 GB | `mmproj-Qwen3.6-27B-Q8_0.gguf` 0.63 GB |

Both fit an L40S 48 GB at `ctxSize 16384` with room to spare.

**Checked rather than assumed** (2026-08-15): all three smokes pass on a local
stand *with* a projector, on both families — Qwen in the full run of §2, Gemma
alongside the regression check. The control arm did its job in the process: asked
about a fixture it could not see, the blind model answered "dark purple" with
confidence, while the seeing one said green. That is the failure mode these
smokes exist to catch, and it is the reason attaching the projector is worth more
than marking them local-only.

## 4. Forks

All four were resolved to the recommended option (*user's decision, 2026-08-15*)
and are **as built** in `tools/hf_api.py`, `tools/e2e_hf.py` and
`.github/workflows/e2e-live.yml`. The rejected options are kept because each says
what the built one is buying.

**F1 — how the runner names a model.**

- **(a) One `--chat-model {gemma-4-31b, qwen-3.6-27b}` choice**, resolving to a
  `(repository, modelPath, mmprojModelPath)` triple held in `hf_api.py` —
  *recommended*. A model is one decision, so no caller can compose a repo/file
  pair that does not exist and learn about it twenty minutes later; `--gguf`
  survives as the per-file override it already is.
- (b) Three independent flags (`--chat-repo`, `--gguf`, `--mmproj`). Maximal
  freedom, and the freedom to deploy something impossible.

**F2 — what one CI dispatch runs.**

- **(a) One model per dispatch**, a `model` workflow input — *recommended*. Run
  time and cost are unchanged (~25 min, ~$1). The model goes into the endpoint
  name (`e2e-chat-qwen-<run-id>`), so the sweeper's log and an orphan hunt stay
  legible.
- (b) Both models in one dispatch, sequentially. One dispatch proves both, but
  the suite runs twice — ~50 min, past the workflow's `timeout-minutes: 45`,
  which cannot simply be raised: it is bounded above by the sweeper's 90-minute
  threshold, and that gap is what stops the sweeper deleting a live run's
  endpoints. Cost ~$1.8.
- (c) Both in parallel. Two chat endpoints alive at once, and the suite is
  `--test-threads=1` against one checkout — rejected.

**F3 — the gate's default model.** Recommend **Gemma stays the default**: the
memory thresholds are calibrated against that stack, and Qwen has exactly one
measured behaviour that is currently *muted rather than understood* (§2). Qwen
on demand, and as part of the release checklist.

**F4 — the projector.**

- **(a) Attach the mmproj for both models** — *recommended*. Closes §3: three
  smokes that are red today start running. Costs 0.63–1.20 GB of extra weights,
  no extra instance, seconds of deploy time.
- (b) Leave the gate blind and mark the three smokes local-only. Cheaper, and it
  makes the gate report ok while three real smokes never ran — the exact failure
  the alternate-embedder decision refused to accept
  ([remote-e2e-hf.md](history/remote-e2e-hf.md) §7).

## 5. Stages

| # | Branch | What | Live run |
|---|---|---|---|
| 1 | `feat/e2e-qwen36` | Make the three smokes model-agnostic (§2) | done — Qwen 97/97, Gemma 5x8/8 |
| 2 | same | `--chat-model` + the projector field in `hf_api.py`, the `model` input in `e2e-live.yml`, docs | **a dispatch per model — outstanding** |

**As built in stage 2.** `CHAT_MODELS` in `tools/hf_api.py` holds each model as a
`(tag, repo, gguf, mmproj)` record; `--chat-model` selects one and `--gguf` /
`--mmproj` stay as per-file overrides. `--no-mmproj` names the old behaviour for
debugging and says in the plan printout that the three vision smokes will fail
under it. The `tag` goes into the endpoint name (`e2e-chat-qwen-<run-id>`), which
is why it is short — `safe_name` truncates at 32 characters and a CI run id
spends ~13 of them. `tools/hf_probe.py` no longer checks the loaded file against
a constant: U1 takes the expected weights name from the selected model, since a
constant no longer describes every run.

## 6. Cost

Under F2a, unchanged: L40S 48 GB $1.80/hr x ~25 min + two T4 embedders ≈ **~$1**
per dispatch, one model at a time. F4a adds weights, not instances. F2b would
make it ~$1.8 and needs the timeout question answered first.

## 7. Definition of Done

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  green; `python tools/cyrillic_scan.py`, `link_check.py`, `doc_index_check.py`
  clean.
- Stage 1 — **met, 2026-08-15**: the three edited smokes green on both stands —
  Qwen (full suite 97/97) and Gemma (client set 5 runs, 8/8 each).
- Stage 2: a green dispatch on each model, with the three vision smokes actually
  running (not skipping) on both.
- Journal entry in [docs/journal/ci.md](journal/ci.md) naming the model, the
  stack and the outcome; `docs/install.md` and the README env table updated if a
  new flag or variable is user-facing. No CHANGELOG entry — dev infrastructure,
  no user-visible effect (AGENTS.md §4).
- On completion this plan moves to `docs/history/` with its relative links
  re-pointed (`python tools/link_check.py`).

## 8. Risks

- **Qwen's tool-call repetition is muted, not explained.** If it reaches the real
  app path it will surface in the orchestrator smokes rather than here; it is
  recorded as a known model behaviour, not as a fixed defect.
- **A second model doubles the surface for model-shaped assumptions.** This track
  found three in one run; expect the first Qwen dispatch on a CI runner to find
  more, and budget a red first run.
- **The two families' thinking budgets differ enough to matter.** Ceilings that
  are generous for Gemma are not for Qwen; when a future smoke sets `max_tokens`,
  the question "and how much does the other family think first?" now has a place
  to be asked.
