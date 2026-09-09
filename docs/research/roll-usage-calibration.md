# The roll's usage for the budget — and the estimate it would calibrate

> **Status:** proposed (2026-09-09) — stage 0 is the measurement in §2.1
> (every calibration of one live session beside the request's parts, on
> the GPU stack). The item the roll-timings track recorded
> ([roll-timings.md](roll-timings.md) §7, fork F1b's consumer): the
> compaction roll prices its reservation from the budget's calibrated
> estimate and never records its exact `usage`, as every loop's round
> does. Measured, the item turns on what that calibration is: the ratio
> the budget keeps is not the tokenizer's density but the tool schemas'
> overhead in disguise, because the estimator does not count the schemas
> — and a request without schemas, the roll, is priced nine times over
> by it, while its own ratio recorded would price the next turn six
> times under.

## 1. Why, precisely

The session budget (admission-by-budget §4.2–§4.3) reserves for every
stream `need = prompt + reply`, the prompt being `estimate × density`
floored by the loop's last exact size, where `density` is the latest
ratio of a server's exact `prompt_tokens` to the app's estimate of the
same request — "the tokenizer's density on this conversation's kind of
text", written by any loop's round, floored at 1.0 (R6: an over-count is
a wait, an under-count is the failure). The roll prices with it and never
writes it; the item asked whether it should.

Stage 0 answered a prior question. `estimate_prompt_tokens` sums the
system message, the message texts and the tool-call arguments at four
bytes a token — and **not the tool schemas** (`ChatRequest.tools`), which
the wire sends as `{type, function: {name, description, parameters}}`
and the chat template renders into the prompt. On the GPU stack a fresh
chat's first request estimated **85** tokens against **4358** exact: 24
schemas, 18 116 bytes of JSON, about 4270 tokens the estimate never saw.
Three consequences, all measured (§2.1):

- **The first request of an app session is priced at a fiftieth of its
  size.** No ratio exists yet, so the budget admits it beside anything —
  a background run, a dialogue — on the estimate alone.
- **The ratio is the overhead, not a density.** It fell from 51 to 6
  across one conversation as the messages grew under a constant 4270:
  it prices the next request *as if the schemas scaled with the text*,
  and works only because consecutive turns carry the same schemas.
- **A request without schemas is priced by a ratio about schemas.** The
  roll — the summarizer's prompt and a digest, no tools — estimated 1106,
  measured 710, and was priced `1106 × 6.03 + 2048 = 8722`: nine times
  its stream, a reservation that keeps a run or a dialogue out of a pool
  the roll leaves mostly empty. Had it recorded its own ratio (0.64,
  floored to 1.0), the next turn would have been priced `905 × 1.0`
  against 5204 exact — the under-count R6 exists to prevent.

So the item has a precondition. Once the estimate counts what the server
counts, the ratio means what §4.3 says it means, every request's ratio
is the same figure, and the roll's `usage` is one more the server
vouched for.

**Requirements.**

- **R1. The estimate counts the schemas** — the same bytes the wire sends,
  at the same bytes-per-token, so the first request of a session is
  priced within a fraction of its size, not a fiftieth.
- **R2. The roll records its usage** as every loop's round does — in its
  task, beside the estimate it was priced from.
- **R3. No under-count introduced** — with the schemas counted the
  estimate over-counts by 6–12 % on the turn (§2.1); the density's floor
  at 1.0 keeps it there, and a prose request over-counted more (the
  roll, 1.5×) waits a little where it could have run, which R6 accepts.

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

`compaction_preserves_a_planted_fact_e2e_live` (Qwen3.6-27B, the LAN
`llama-server`, a Russian conversation of seven turns, `/compact`, one
turn after), with a scratch print at every `record_usage` and of the
request's parts at every estimate (reverted, by reversing the edits):

| request | system bytes | message bytes | tools | schema bytes | estimate | exact | exact / estimate |
|---|---:|---:|---:|---:|---:|---:|---:|
| turn 1 | 136 | 171 | 24 | 18 116 | 85 | 4358 | **51.3** |
| turn 2 | 136 | 389 | 24 | 18 116 | 152 | 4492 | 29.6 |
| turn 4 | 136 | 689 | 24 | 18 116 | 244 | 4578 | 18.8 |
| turn 7 | 136 | 2706 | 24 | 18 116 | 774 | 4868 | **6.3** |
| the title (never records) | 538 | 271 | 0 | — | 211 | — | — |
| **the roll** (never records) | 1444 | 2766 | 0 | — | 1106 | **710** | **0.64** |
| turn 8, after the roll | 2856 | 779 | 26 | 19 763 | 925 | 5181 | 5.6 |

The roll's reservation in that session: `1106 × 6.034 + 2048 = 8722`
tokens, for a stream of 710 + 284. The same figures with the schemas
counted at four bytes a token (18 116 → 4529; 19 763 → 4941):

| request | estimate with schemas | exact | exact / estimate |
|---|---:|---:|---:|
| turn 1 | 4614 | 4358 | 0.94 |
| turn 7 | 5303 | 4868 | 0.92 |
| turn 8 | 5866 | 5181 | 0.88 |
| the roll | 1061 (no schemas) | 710 | 0.67 |

The turn's estimate lands 6–12 % over the exact — the compact JSON is a
little denser than four bytes a token, and the template adds a few tokens
a message — which the density floor turns into a price a tenth over the
stream, on the safe side. The roll over-counts by half: Russian prose is
nearer six bytes a token than four. No under-count anywhere. The roll's
price would be `1061 + 2048 = 3109`.

### 2.2 The code

- `shared/tokens.rs`: `estimate_text` (bytes ÷ 4, rounded up) and
  `estimate_prompt(system, parts)` (each part plus a four-token template
  overhead) — written for the status bar's "~N" before the exact count
  arrives (spec §11.1).
- `orchestrator/generation.rs`: `estimate_prompt_tokens(&ChatRequest)`
  feeds `estimate_prompt` the system message, each message's content and
  each tool call's arguments; used by the turn's rounds, the loops'
  rounds (`tool_loop::run_rounds`), the roll, the title, impersonation,
  the sub-agent and dialogue runs, and the live indicator.
- `shared/session_budget.rs`: `record_usage(estimate, exact)` stores the
  ratio floored at 1.0; `price(estimate, floor, reply_cap)` applies it.
  Callers of `record_usage`: the turn's round (`TurnLoop`) and the loops'
  (`record_round_usage`). The roll, the title and impersonation price and
  do not record.
- `orchestrator/compaction.rs`: `spawn_compact` prices `need` from the
  estimate and `max_tokens`, `collect_roll` keeps the usage chunk's
  `prefill` on `Collected` and drops the rest.

## 3. Design

### 3.1 The estimate counts the schemas

`estimate_prompt_tokens` adds, for each `ToolSchema`, the byte length of
the wire's own rendering — `serde_json::to_string` of `{type: "function",
function: {name, description, parameters}}`, the compact form — through
`estimate_text`, plus the per-message overhead once for the tool block.
No new constant: the same four bytes a token, with the calibration
absorbing the difference (§2.1: the compact JSON runs a little denser, so
the turn's estimate lands a tenth over, on the safe side). The status
bar's "~N" before the exact count reads the same estimate and becomes
honest with it: a fresh chat shows about 4600, not 85, until the server
says 4358.

### 3.2 The roll records

`Collected` carries the usage chunk whole (`usage: Option<TokenUsage>`;
`prefill` read off it where it was), and `spawn_compact` — which holds
`sessions` and priced `need` from the estimate a moment earlier — records
`(estimate, usage.prompt_tokens)` when an attempt reaches its usage, the
line `record_round_usage` has. A displaced attempt records nothing (no
usage); the retry does. The title and impersonation are left as they are
(F2): the title's request is too small for its ratio to mean anything
beyond noise, and impersonation's is the turn's own text, already
recorded by the turn before it.

### 3.3 What changes in the numbers

On the measured session: the first request priced 4614 against 4358
instead of 85; the roll 3109 instead of 8722; the ratio 0.9–1.0 across
the session instead of 51 falling to 6, so a request of any shape is
priced by a figure that is about its text. The compaction trigger is
untouched (it reads the exact usage, never the estimate — history
compression S2).

## 4. Difficult spots

- **The rendered schemas are not the compact JSON.** llama.cpp's Jinja
  templates render tools with whitespace and a preamble; Qwen's adds a
  `<tools>` block. Measured, the compact form over-counts the rendering
  by a few percent (4529 against about 4270), which is the safe
  direction; a template that renders far larger would show as a ratio
  above 1.0 and calibrate.
- **Prose over-counts.** Four bytes a token is the Latin figure; Cyrillic
  runs nearer six, and the roll's digest is the conversation's text. An
  over-count is a wait where the stream could have run (R6), and the
  floor at 1.0 means the calibration never lowers a price — the roll's
  ratio of 0.67 is stored as 1.0.
- **The first request still has no ratio.** It is priced at the raw
  estimate — now a tenth over its size rather than a fiftieth under. The
  clouds have no pool and are untouched.
- **The live indicator jumps.** A chat that read "~85" reads "~4614" until
  the exact count replaces it — the honest figure, but a visible change;
  the exact count still wins the moment it arrives.

## 5. Forks

- **F1. The estimator and the schemas.** (a) **Count the schemas' JSON —
  the wire's own rendering, compact — at the same bytes-per-token as
  text** *(recommended — R1; measured, the whole gap)*. (b) Leave the
  estimator and keep the roll out of the calibration — the first request
  of a session stays priced a fiftieth under, the roll nine times over.
  (c) An exact count from `/tokenize` — a round trip per request on the
  server the streams compete for, and no cloud has it (rejected already
  in admission-by-budget §4.3).
- **F2. Who records.** (a) **The roll, in its task, as the loops do**
  *(recommended — R2; the one silent request sized by the conversation,
  and a session that starts with `/compact` gets its ratio before the
  first turn is priced)*. (b) The roll, the title and impersonation — the
  title's ratio is noise at its size, impersonation's the turn's own text
  already recorded. (c) None — the estimator alone; the roll then still
  prices right, and the item is closed by its precondition.
- **F3. The roll's carrier.** (a) **`Collected.usage`, the chunk whole,
  recorded in `spawn_compact` where `need` was priced** *(recommended —
  one field replacing one; the spawn has the estimate and `sessions`)*.
  (b) At the landing through `CompactResult` — the landing has no
  estimate to record against.
- **F4. The schemas' bytes per token.** (a) **The text's four** *(recommended
  — measured 4.3; the calibration absorbs a 7 % difference)*. (b) A
  constant of their own — a second knob for one measured figure.
- **F5. Staging.** (a) **One PR, live-gated** *(recommended)*: a smoke sends
  the app's own turn request and a roll-shaped one through the live
  backend and reads the exact count beside the estimate — the turn's
  ratio within 0.75–1.25, the roll's at most 1.0 (over-counted, never
  under). (b) Not gated.

## 6. Tests and the live run

- `generation.rs` tests: a request with one schema of a known byte size
  estimates its bytes over four plus the text's; an empty tool list
  estimates as before.
- `tests/compaction.rs`: a manual roll whose stream carries a usage above
  its estimate leaves `density()` at exact over estimate (the recorder's
  request gives the estimate); a stream that ended short records nothing.
- `shared/session_budget.rs`: unchanged.
- `tests/live.rs`: `prompt_estimate_e2e_live` — the turn's request as the
  app builds it (through a bare orchestrator's `build_request`) and a
  roll-shaped one, each sent once through the live backend and read to
  its usage; the ratio exact/estimate printed and asserted within the
  bands of F5. The LAN regression pair after it:
  `compaction_preserves_a_planted_fact_e2e_live`,
  `roll_prefill_e2e_live`.

### 6.1 Runs

*(filled in at stage 1.)*

## 7. Not in this track

- **The title's and impersonation's usage** (F2b).
- **A per-script density** — six bytes a token for Cyrillic prose; the
  floor at 1.0 makes the over-count a wait, not a failure.
- **A figure on the settings screen** — beside the *Batch (-b)* field
  ([slow-prefill-detection.md](slow-prefill-detection.md) §7).

## 8. Documentation touch list (AGENTS.md §4)

- spec §6.3 (what the estimate counts; the roll records), §11.1 (the
  indicator's estimate counts the schemas).
- architecture §5 (`estimate_prompt_tokens`, the schemas), §6 (the
  budget's calibrators: the roll).
- [admission-by-budget.md](admission-by-budget.md) §7 (the gap measured
  here), [roll-timings.md](roll-timings.md) §7 (done here).
- CHANGELOG (Fixed: the estimate before the exact count; the roll's
  reservation), journal `engine.md`, lessons §3, CLAUDE.md's status line
  and count.
