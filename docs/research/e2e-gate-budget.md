# The live gate outgrew its 45 minutes — and a cancelled run kept its GPUs

**Status:** decided 2026-10-09 (the owner's decision, §4). Stage 1 —
`fix/e2e-cancel-cleanup`; stage 2 — sharding; stage 3 — the dialogue loop
(`fix/run-dialogue-repeat`, §7).

The remote live gate ([docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md))
rents a llama.cpp server and two embedders as HF Inference Endpoints and runs
every `#[ignore]` smoke against them in one job capped at 45 minutes. On
2026-10-09 the owner dispatched it by hand, on `main` with the default model, and
the job hit that cap. This document records what the run measured, the two
defects it exposed, the forks and the decision, and the stages that follow.

## 1. The run, measured

CI run [37947267239](https://github.com/vshylov/mindfork-rs/actions/runs/37947267239):
`gemma-4-31b` on an `nvidia-l40s`, bge-m3 and multilingual-e5 on two `nvidia-t4`,
no filter. Read from the job log and from the chat endpoint's own server log
(`GET /v2/endpoint/{ns}/{name}/logs`, which still answers after the endpoint has
scaled to zero).

| Phase | Time |
|---|---|
| Job start → `cargo test --no-run` done | 2 min 44 s (cold cache) |
| Create → all three healthy | 1 min 39 s |
| Suite | 40 min, cut off by the job's timeout |
| Tests finished | **79 of 250** |

**Nothing hung; the suite outgrew the job.** The gate's history, by run:

| Date | Model, GPU | Tests | Suite |
|---|---|---|---|
| 2026-07-28 | Gemma 4 31B, L40S | 66 | 1049 s |
| 2026-08-15 | Qwen 3.6 27B, L40S | 94 | 1696 s |
| 2026-08-29 | gpt-oss-120b, H200 | 125 | 859 s |
| 2026-10-09 | Gemma 4 31B, L40S | 250 | > 2320 s for the first 79 |

On the same stack the tests themselves did not slow down: the 19 smokes this run
shares with the July one took **664 s against 679 s**. What grew is the count —
60 of the 79 finished here did not exist in July, and they alone took 1656 s.
Estimated from the finished part and the earlier runs' durations of the rest, the
whole suite is **50–60 minutes** on this stack, ≈ $2.80 a run (L40S $1.80/h, two
T4 at $0.50/h) — against the "~25 minutes, ~$1" the workflow's comment and
install.md §7.2 still say.

**The run is GPU-bound.** Over the 2320 s of the finished part the chat server
was busy for **1859 s (80 %)**: 1469 s decoding, 390 s in prefill, 288 requests,
33 tokens/s. A faster card would speed the suite up nearly in proportion; so
would running it in parallel.

**More than a quarter of that time was one test.** `dialogue_e2e_live` took
**643 s**: 63 requests and 17 548 generated tokens for a dialogue that landed
5 spoken lines and 1212 tokens. The server log shows why — a block of six
requests (the scene's speakers and director) repeated **nine times**, each
preceded by the parent turn with a prompt growing 907 → 1260 → … → 3711 tokens.
The parent model called `run_dialogue` again after every dialogue landed, and
its last reply was a raw `<|tool_call>call:run_dialogue{…` in the text. The test
passed — its first dialogue had landed — so nothing reported the loop. That is
a product defect, not a slow test (stage 3).

Two tests failed before the cutoff, both on the model's judgment rather than the
stack: `code_edit_e2e_live` (the fix compiles but prints `Mean: 2`, not
`mean=5`) and `fetched_page_is_searched_in_its_birth_turn_e2e_live` (the model
summarised the fetched page instead of searching it).

## 2. The defect the timeout exposed: a cancelled job kept its GPUs

After the timeout the workflow's last step, which only listed the endpoints,
showed all three still `running`. They idled out to `scaledToZero` 15 minutes
later and held their quota until they were deleted by hand. The job log has no
`[signal …]` and no `=== cleanup ===` line, and the job's own teardown reports
`Terminate orphan process: pid (2861) (python3)` — the runner, still alive
after its step had ended.

GitHub cancels a step by signalling **the step's own process** — SIGINT first,
harder signals seconds later — and nothing below it. That process was the step's
`bash`; the Python runner under it, which holds the SIGINT/SIGTERM handler that
deletes the endpoints, was never signalled. The July "failure drill" that proved
the handler (remote-e2e-hf.md §9) signalled the runner directly with SIGBREAK on
a dev box — it never went through the path a CI cancellation takes, and no
cancellation in CI had happened since. The leak cost ≈ $0.70 (one idle window on
the three cards); the sweeper would have reclaimed the quota at its next pass.

## 3. Forks

**F1 — the cleanup on a cancelled job.**
- (a) `exec` the runner, so the cancellation's signal is its own, **and** turn
  the last step into a backstop that deletes the run's endpoints by name, `if:
  always()`, needing nothing from the dead step but the run id. Two layers that
  fail independently. **Recommended.**
- (b) Only (a)'s `exec`. One layer; a runner killed outright (SIGKILL after the
  grace period, a lost machine) still leaks until the sweeper.
- (c) A `trap` in the step's bash that forwards the signal. Works, but it is
  shell plumbing in YAML that no check can see, and still one layer.

**F2 — fitting the suite into the job.**
- (a) **A faster card for Gemma**: A100 80 GB ($2.50/h, quota 4) — ~1.7× by
  memory bandwidth, unmeasured; H200 ($5.00/h) — ~2.4×, but its AWS region is
  now `deprecated` in HF's catalogue. Cost per run about the same; a one-off
  factor against a suite that grew 3.8× in ten weeks.
- (b) **Shards**: `e2e_hf.py run --shard i/N`, a matrix in the workflow, each
  shard with endpoints of its own. Billing is per minute, so N shards cost about
  one run plus N−1 deploys (~2 min each); wall time divides by N, and N grows
  with the suite. Costs: N times the endpoints at once (quota on 2026-10-09:
  L40S 16, T4 30 — three shards hold 3 + 6), N cold builds, a red run's log split
  across jobs. **Recommended.**
- (c) **Raise the ceilings**: job 90 min, sweeper 150. Two numbers; ~65 minutes
  a run, and the same wall again in a few weeks.
- (d) **Cut the waste**: the dialogue loop alone is ~9.5 minutes. Not enough by
  itself, and a defect worth fixing on its own account.

## 4. Decision

**User's decision, 2026-10-09:** F1 (a); F2 (b) on the same L40S, with (d) as a
separate fix of the product defect; (a) remains an optional measurement — one
dispatch with `chat_instance: nvidia-a100` — that combines with shards. The
three leftover endpoints of run 37947267239 were deleted the same day.

## 5. Stages

1. **The cleanup survives a cancelled job** (`fix/e2e-cancel-cleanup`) — §6.
2. **Shards** (`--shard i/N`, the workflow matrix) — designed in its own PR.
3. **The dialogue loop** (`fix/run-dialogue-repeat`) — why the parent calls
   `run_dialogue` again after a landed dialogue, and the raw tool call it ends
   with — §7.

## 6. Stage 1 — design and outcome

- **`exec python3 tools/e2e_hf.py …`** in the step: the runner is the step's
  process, so the cancellation's SIGINT reaches its handler, which sends every
  DELETE before verifying any — it has seconds before the harder signals.
- **`e2e_hf.py delete-run --run-id ID`**, run by the last step `if: always()`.
  It derives the names from the run id with the function the create uses
  (`run_names`, for every chat model's tag, since the step is told the run, not
  the model), keeps the listed endpoints whose names match **exactly** — run
  ids nest, `37947267239-1` is a prefix of `37947267239-11` and a suffix of
  `137947267239-1` — and hands them to the runner's own verified cleanup (exit 3
  on a failed delete). If the listing fails it deletes every candidate name
  blind; a 404 is proof enough. Then it lists the namespace, which is all the
  step did before.
- **`e2e_hf.py --self-test`**, in CI's `lint` job, needing neither a token nor
  the network: the names keep the run id within the 32-character limit for every
  model and both id forms; `delete-run`'s choice against nesting ids, another
  model's name for the same run, and look-alikes; the sweeper's verdicts,
  including the nine-digit fraction with an offset that once broke its parser;
  and two facts that live in the YAML — the runner `exec`ed, and the job's
  ceiling under the sweeper's 90 minutes. Red on `main` before the change (the
  `exec` and the backstop missing), green after; five mutants — suffix and
  substring matching, a 95-minute ceiling, the `exec` dropped, the backstop
  without `always()` — each caught.

**Outcome.** The backstop, run for real: three endpoints of a local run
(`running`, `running` and one that had `failed` to start) deleted and verified
by `delete-run`, the namespace empty; and again for one. **The drill, this time
through CI** (run [37963066790](https://github.com/vshylov/mindfork-rs/actions/runs/37963066790),
the branch, `orchestrator::tests::live`): cancelled by hand two minutes into
the suite. The runner logged `[signal 2] cleaning up before exit` within the
second, all three endpoints were deleted and verified 3 s later, the backstop
found none left, and `python3` was no longer among the orphans the job's
teardown killed. **GO.**

Seen on the way, for stage 2: on the evening of 2026-10-09 one L40S create
failed to start (`Endpoint failed to start`, 135 s) and the next took **604 s**
to be scheduled, its server then loading in 4 s — HF's capacity, not the image.
Shards multiply that exposure, so a shard whose endpoint fails must fail alone.

## 7. Stage 3 — the dialogue loop: two defects

Reproduced on the gate's own weights (`gemma-4-31B_q4_0-it.gguf` + its
projector, `llama-server -c 16384 -np 1 --jinja`, an RTX 4090):
`dialogue_e2e_live` made **eight** `run_dialogue` calls with identical
arguments in 528 s and ended on the same raw `<|tool_call>call:run_dialogue{…`.
Every call's scene had landed; every result said so.

**D1 — the result pointed at a door the turn had no key for.** It ends with
the transcript's `chat://` address, "read back with `chat_read`" (spec §9.13) —
and the smoke narrows the profile to `run_dialogue` alone, so no `chat_read`.
The control: the same smoke with `chat_read` added made **one** call in 64 s
and answered. The address is now named as a route only where the staging turn
offers `chat_read`; otherwise the result says the lines stay in the scene's
chat for the user, no tool of the caller's reads them, and staging the scene
again would not hand them over (lessons §4: only advertise what exists). That
took the smoke to 1 and 3 calls in two runs — better, not deterministic.

So **an identical scene is not staged twice in one turn**: a call whose
arguments equal those of a scene that already ran in the turn gets a result
saying so — where its ending is, what to change for a different scene — and no
scene. Only a scene that ran counts, so a malformed call repeated is refused as
itself. Three runs after: exactly one scene each, 1, 2 and 6 calls, 57–148 s —
the repeats are one round each now, not six requests and a minute of GPU.

**D2 — the round limit's final round wrote a call out as text.** When the
limit fires, the loop discards the round's calls and re-sends the request
that produced them, less the tool list. Nothing told the model the tools were
gone, and Gemma 4 answered the same request with the same call — as text,
which was the user's reply. A new smoke, `the_round_limit_ends_in_prose_e2e_live`
(one round allowed, an ask that needs two): **3 of 3** replies were
`<|tool_call>call:get_sampling{}<tool_call|>` without a note. The final round's
last tool result now carries one, on the wire only: no tools are left, do not
call or write one out, answer from what the rounds returned, in the language of
the user's message. On the last turn rather than as a `user` turn of its own,
because a template that enforces alternation refuses a user turn after the
tool results. **3 of 3** in prose after; the first version of the note, without
the language clause, was answered in the profile's language two times of three
(the harness's profile is Russian, the ask English), and the clause fixed that,
3 of 3.

`background_dialogue_e2e_live`, 137 s on the gate, took 79 s locally after.
Unit tests: the route named only with `chat_read`, the repeat answered without
a scene, a refused scene refused as itself, the final request's note on the
last tool result and nowhere in the chat, and where the note goes for each
shape of history — each red against a mutant of its fix.
