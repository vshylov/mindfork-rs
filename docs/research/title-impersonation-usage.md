# The title's and impersonation's usage for the budget — and the ratio they would erase

> **Status:** implemented (2026-09-09) — every fork at its recommendation
> (the user's decision, 2026-09-09); stage 0 is the measurement in §2.1
> (the two requests' ratios, and a turn whose ratio runs above 1.0, on the
> GPU stack), stage 1's live run is in §6.1. The item the
> roll-usage-calibration track recorded
> ([roll-usage-calibration.md](roll-usage-calibration.md) §7, fork F2b):
> the automatic title and impersonation price their reservations from
> the budget's calibrated estimate and never record their exact `usage`.
> Measured, the item turns on the budget's one rule for its ratio — "the
> latest wins, floored at 1.0" — which lets any over-counting request
> erase the correction an under-counting turn recorded; the two requests
> here are the most over-counting the app makes.

## 1. Why, precisely

Since the roll-usage-calibration track the prompt estimate counts what the
server counts, so a request's exact-to-estimate ratio is the tokenizer's
density on that request's text: the turns 0.9 (the compact JSON of the
schemas a little denser than four bytes a token), the prose requests —
the roll, the title, impersonation — about 0.6 (Russian at six bytes a
token). The budget keeps **one** ratio, written by the turn's rounds, the
loops' rounds and the roll, read by every reservation, and stores each
new value floored at 1.0: an over-count is a wait, an under-count the
failure (admission-by-budget R6).

Two things measured for this track (§2.1):

- **The title and impersonation are the app's most over-counting
  requests** — 0.65 and 0.59. Recorded under the rule as it stands, each
  stores 1.0.
- **A turn can under-count by a quarter.** A message carrying 25 KB of
  JSON — the shape of a tool result, a page read, a search — estimated
  11 058 tokens against 14 767 exact: **1.34**. Recorded, that ratio
  prices the next round a third over its raw estimate, which is the
  reservation the pool needs.

Put together: under "the latest wins" the next silent request after such
a turn — the roll at the landing, the title, an impersonation — records
0.6, stores 1.0, and the following turn's first round is priced a quarter
under its size. The hazard is the rule's, not this track's — the roll and
the loops already record — but the item as named would add the two
requests that trip it most surely, so it is decided here: **the ratio a
request prices with is the latest ratio of its own kind of request.** A
turn's correction is then erased by nothing but a turn.

**Requirements.**

- **R1. The title and impersonation record** their exact usage as every
  other request does, in their tasks, beside the estimate their
  reservation was priced from.
- **R2. No request erases another kind's correction** — each kind of
  request prices with the latest ratio of its own kind.
- **R3. No under-count introduced** — a kind's first request prices at its
  raw estimate, as the first request of a session does today.

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

`prompt_estimate_e2e_live` (Qwen3.6-27B, the LAN `llama-server`) with a
scratch impersonation step and a print at every stream's `Finished`
(reverted by reversing the edits):

| request | tools | estimate | exact | exact / estimate |
|---|---:|---:|---:|---:|
| turn 1 | 24 | 4791 | 4349 | 0.91 |
| turn 2 | 24 | 4931 | 4431 | 0.90 |
| turn 3 | 24 | 5098 | 4523 | 0.89 |
| the title | 0 | 294 | 192 | **0.65** |
| the roll | 0 | 687 | 423 | 0.62 |
| **impersonation** | 0 | 643 | 378 | **0.59** |

The same smoke with its second message carrying 25 KB of JSON (220
catalogue items):

| request | tools | message bytes | estimate | exact | exact / estimate |
|---|---:|---:|---:|---:|---:|
| turn 1 | 24 | 119 | 4791 | 4349 | 0.91 |
| **turn 2, the JSON** | 24 | 25 154 | 11 058 | 14 767 | **1.34** |

A turn of Rust code (a 1 KB snippet) stayed at 0.91: the schemas dominate
a request of that size. The JSON turn is the real shape — a tool result
folded into the history runs at about 2.4 bytes a token on this
tokenizer, and the estimate's four under-counts it by a quarter. Two
more readings from the prints: the title's stream was **displaced** twice
by the next turn (`Finished(Cancelled)`, no usage — a stream cut short
records nothing, as the roll's does), and impersonation streams to its
usage like any other request (`Stop`, the chunk seen).

### 2.2 The code

- `shared/session_budget.rs`: one `density: AtomicU64`; `record_usage
  (estimate, exact)` stores `max(exact / estimate, 1.0)`; `price(estimate,
  floor, reply_cap)` multiplies by it; `density()` reads it.
- Seven pricing sites: the turn's round (`generation.rs`, `TurnLoop`), a
  child run's round (the same `TurnLoop`), a loop's round
  (`tool_loop::stream_round`), the roll (`spawn_compact`), the title
  (`spawn_title`), impersonation (`spawn_impersonation`), and the page
  summary inside a tool (`features/tools/fetch.rs`).
- Three recorders: `TurnLoop` (the parent's and the children's rounds
  alike), `record_round_usage` (the loops), `spawn_compact` (the roll).
  The title and impersonation match `ChatChunk::Usage(_) => {}`; the page
  summary reads no usage.

## 3. Design

### 3.1 One ratio per kind of request

`SessionBudget` keeps a ratio per **shape** — `Shape::{Turn, Run, Loop,
Roll, Title, Impersonation, Summary}`, one atomic each — and its three
calls take the shape: `price(shape, estimate, floor, reply_cap)`,
`record_usage(shape, estimate, exact)`, `density(shape)`. The rule inside
a shape is the rule today: the latest wins, floored at 1.0. A shape's
first request prices at its raw estimate (R3) — the turn's 0.9 over, the
title's 0.65 over, the JSON turn's 1.34 under until its own round
records, as today. Nothing crosses shapes: the roll at the landing, the
title behind the first exchange, an impersonation before the next turn
each store into their own slot, and the turn's 1.34 stands for the
turn's next round (R2).

The shapes follow the request populations, not the code paths: a child
run's rounds (`Run`) carry their own persona and the turn's tools; a
loop's (`Loop`) its instructions, its schemas and a digest; the roll
(`Roll`) the summarizer's prompt and a digest; the title (`Title`) the
first exchange; impersonation (`Impersonation`) the conversation with the
roles swapped and no tools; the page summary (`Summary`) a page of prose,
priced and never recorded — it reads no usage today and its ratio stays
1.0.

### 3.2 The title and impersonation record

The title's collect keeps the usage chunk and `spawn_title` records
`(Shape::Title, estimate, prompt_tokens)` when an attempt reaches it — the
roll's line; a displaced attempt has no usage and records nothing, the
retry does. Impersonation's `run` records `(Shape::Impersonation, …)` at
the usage chunk, when it has a budget (the shared engine; a separate
impersonation server has no pool and no budget to record into).

### 3.3 What changes in the numbers

On the measured JSON session: turn 2 records 1.34 into `Turn`; the title
that follows records 0.65 into `Title` and stores 1.0 there; turn 3's
first round is priced at `estimate × 1.34` — where the rule as it stands
would have priced it at `estimate × 1.0`, a quarter under. On the prose
session nothing changes: every shape's ratio is 1.0 either way.

## 4. Difficult spots

- **A shape's first request is raw.** The JSON turn itself was priced at
  11 058 against 14 767 — the first-request hazard the previous track
  left, unchanged here; only its *next* round is corrected.
- **The budget is app-wide.** A JSON-heavy chat's `Turn` ratio prices a
  prose chat's first turn after a switch 34 % over — one round, until
  that turn's own round records; a wait at worst (R6).
- **The probe smoke's band.** `prompt_estimate_e2e_live` asserts a turn's
  ratio within 0.75–1.25; the JSON turn is 1.34 and would fail it. The
  smoke keeps its prose turns in the band and asserts the JSON turn
  **above 1.0** — the premise this design rests on, measured on every
  run.
- **The API's shape.** Seven pricing sites and three recorders take one
  more argument; the budget's tests and the orchestrator's few
  `density()` readers name a shape. Mechanical.

## 5. Forks

- **F1. Record the two.** (a) **Yes, in their tasks, at the usage chunk**
  *(recommended — R1; every request the server vouched for records)*.
  (b) No — the title's and impersonation's ratios never exceed 1.0 on any
  measured text, so their record is 1.0 either way; the item closes by
  the ratio's population (F2) alone.
- **F2. The ratio's population.** (a) **One per kind of request — `Turn`,
  `Run`, `Loop`, `Roll`, `Title`, `Impersonation`, `Summary` — the latest
  of its kind wins** *(recommended — R2; measured, a turn at 1.34 and a
  title at 0.65 in one session, and under one ratio the second prices the
  turn after it a quarter under)*. (b) One ratio, the latest wins — as
  today, the hazard accepted (it exists already with the roll and the
  loops). (c) One ratio, an over-count never overwrites — the correction
  sticks for the session, a JSON chat's 1.34 pricing every later prose
  chat a third over until restart. (d) Two populations, with and without
  schemas — a loop's first round (schemas, a prose digest) would still
  erase the turn's.
- **F3. The key.** (a) **A `Shape` enum on the budget's API** *(recommended
  — seven names, one atomic each, no allocation on the pricing path)*.
  (b) The silent lane's label (`"title"`, `"compaction"`, …) as a
  string key — the turn and the runs have no label, and a map under a
  mutex on every price.
- **F4. The child runs' shape.** (a) **Their own, `Run`** *(recommended —
  a persona's prompt and the turn's tools, a population of its own)*.
  (b) The turn's — a run's ratio then prices the parent's next round.
- **F5. Staging.** (a) **One PR, live-gated** *(recommended)*: the probe
  smoke gains an impersonation and a JSON-dense turn, asserting per
  shape — the prose turns within 0.75–1.25, the JSON turn above 1.0, the
  roll, the title and impersonation at most 1.0. (b) Not gated.

## 6. Tests and the live run

- `shared/session_budget.rs`: a turn's 1.34 survives a title's 0.65 —
  `density(Turn)` stays, `density(Title)` reads 1.0; each shape prices by
  its own; the existing pricing test names its shape.
- `tests/compaction.rs`: the roll's record lands in `Shape::Roll` and
  leaves `Shape::Turn` where it was.
- `tests/title.rs`: a title stream carrying a usage above its estimate
  records into `Shape::Title`; a displaced attempt records nothing.
- `tests/impersonation.rs`: an impersonation stream on the shared engine
  records into `Shape::Impersonation`; on a separate engine (no budget)
  nothing is recorded and nothing panics.
- `tests/live.rs`: `prompt_estimate_e2e_live` extended — an impersonation
  after the roll, the second turn a 25 KB JSON catalogue, the assertions
  of F5a. The LAN regression pair: `compaction_preserves_a_planted_fact_e2e_live`,
  `roll_prefill_e2e_live`.

### 6.1 Runs

`prompt_estimate_e2e_live` on the GPU stack (Qwen3.6-27B) — two prose
turns, a third carrying the 25 KB catalogue, `/compact`, an impersonation
— every request kept beside the exact count the server reported for it:

| request | tools | carries the JSON | estimate | exact | exact / estimate |
|---|---:|:---:|---:|---:|---:|
| turn 1 | 24 | — | 4791 | 4349 | 0.91 |
| turn 2 | 24 | — | 4966 | 4448 | 0.90 |
| **turn 3** | 24 | yes | 11 267 | 14 886 | **1.32** |
| the title | 0 | — | 328 | 209 | 0.64 |
| the roll | 0 | — | 762 | 457 | 0.60 |
| **impersonation** | 0 | yes | 6630 | 10 643 | **1.61** |

The prose turns in the band, the JSON turn under-counting by a third,
the title and the roll over-counting by four tenths — and impersonation,
which sends the whole conversation with no schemas to dilute the JSON,
under-counting by six tenths: the ratio its own kind now keeps, and the
one that would have priced the next turn under the rule as it stood.
The LAN regression pair after it —
`compaction_preserves_a_planted_fact_e2e_live` (the planted code
answered), `roll_prefill_e2e_live` (2.5 s, no note) — 2/2.

One test was rewritten with the rule it asserted:
`the_parents_exact_usage_calibrates_the_childrens_reservations` had the
parent's first round report an exact size far above its estimate and
expected the two children to take turns; under one ratio per kind the
parent's record is the parent's, the children still fit (F4), and a
child's own round reporting the same is what makes its second round
wait — `a_childs_ratio_is_the_childrens_not_the_parents` asserts both
halves. One unit run was lost to a fixture: a bare orchestrator's chat is
not active until the test says so, and `handle_impersonate` on no active
chat sends an error and returns.

Unit: 2972 green, 150 ignored (three tests added, one rewritten).

## 7. Not in this track

- **The first request of a shape** — priced at its raw estimate; only the
  exact count could do better, at a round trip per request.
- **A per-script density** — six bytes a token for Cyrillic prose, 2.4
  for JSON; the shapes carry it implicitly, per population.
- **The page summary's usage** (`Summary`) — done:
  [page-summary-usage.md](page-summary-usage.md), which measured the
  reading here wrong: a page's text under-counts on four pages of five.

## 8. Documentation touch list (AGENTS.md §4)

- spec §6.3 (the ratio per kind of request; the title and impersonation
  record).
- architecture §3 (`session_budget.rs`: the shapes).
- [admission-by-budget.md](admission-by-budget.md) §8 (the ratio's
  population, found later), [roll-usage-calibration.md](roll-usage-calibration.md)
  §7 (F2b done here).
- CHANGELOG (Fixed: a roll or a title no longer resets the correction a
  turn recorded), journal `engine.md`, CLAUDE.md's status line and count.
