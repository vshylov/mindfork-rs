# The page summary's usage — the one kind that under-counts, and a summary that came back empty

> **Status:** implemented (2026-09-09) — every fork at its recommendation
> (the user's decision, 2026-09-09); stage 0 is the measurement in §2.1
> (five pages' ratios, the summary's cold prompt, and the reply's shape on
> a thinking model, on the GPU stack), stage 1's live run is in §6.1. The item the
> title-impersonation-usage track recorded
> ([title-impersonation-usage.md](title-impersonation-usage.md) §7): the
> page summary inside `fetch_url` prices its reservation under its own
> kind (`Shape::Summary`) and never records its exact `usage` — "a
> request of prose, over-counted, priced at 1.0". Measured, the reading
> is wrong on four pages of five, and the measurement found two more
> things beside it: on a thinking model the summary comes back **empty**,
> its whole reply cap spent on thoughts; and its prompt is the coldest
> the turn makes, unread by the slow-prefill note.

## 1. Why, precisely

Since the roll-usage-calibration track the prompt estimate counts what the
server counts, and since the last track the budget keeps a ratio per
**kind** of request — the tokenizer's density on that kind's text, the
latest of the kind winning, floored at 1.0 — read by every reservation of
the kind. Six kinds record; the seventh, the page summary a tool makes on
its own, prices with 1.0 forever. The last track left it so on a reading:
a summary is a request of prose, and prose over-counts (the title 0.65,
impersonation 0.59, the roll 0.60), so recording it would store 1.0.

Measured (§2.1), the reading holds for one page in five. The summary's
text is a **page** — a documentation page with its code, an API's JSON, a
source file, an English article with its citation marks — and on four of
the five the exact count ran **above** the estimate: 1.04, 1.16, 1.17,
**1.28**. Only Cyrillic prose over-counted (0.76). The summary is the
app's one kind whose ratio runs above 1.0 as a rule; the turn's does so
only with a tool result's JSON in it (1.34, the last track). Priced at
1.0, a JSON page's summary reserves 28 % under its size — and under a
shared pool the reservation is the one thing keeping the streams from
overfilling it (admission-by-budget R6: an over-count is a wait,
under-counting the failure). Three pages fetched in one segment summarize
at once under that pool; a summary inside a silent loop streams beside
the turn.

Two more things the same probe showed:

- **On a thinking model the summary is empty.** With the sampling as the
  profile has it — `reasoning_budget` unset, the model's template
  deciding — Qwen3.6-27B spent the whole reply cap (768 tokens) on
  thoughts and produced **no text** on both pages, `finish = Length`. The
  inline path then hands the model the page's text under "summary
  unavailable"; the attached path attaches the page and says nothing of a
  summary. The prefill and the cap are paid every time for nothing. Every
  other one-shot request of the app — the title, the roll,
  impersonation, a dialogue's director — mutes reasoning up front
  (`reasoning_budget = Some(0)`), and the turn's loop retries an empty
  reply muted; the summary does neither. Muted, the same two pages
  summarized in 587 and 161 tokens.
- **The summary's prompt is a cold one, and large.** The summarizer's
  system and a page that never repeats: processed whole — 3236 tokens for
  a 12 000-character head, larger than any silent task's first round
  (a reflection's 2798, the roll's 1466) and second only to a fresh chat's
  first turn (4358 with the schemas). The stream's `timings` arrive on
  the `Usage` chunk the collect drops, so the note of
  [slow-prefill-detection.md](slow-prefill-detection.md) never sees it —
  where a warm server's turns process tens of tokens and the first cold
  prompt of a session may well be a page the user asked about.

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

The LAN stack: Qwen3.6-27B Q4_K_M, four slots over 16 384, a scratch
probe printing the summary request's estimate, the server's exact
`prompt_tokens` and the stream's `timings` at the `Usage` chunk, with
`fetch_url` invoked directly under a budget of four sessions over the
pool. The pages, in order; the two heads are the summarizer's
12 000-character cut of a page over the attachment budget:

| page | text | chars | bytes | estimate | exact | exact / estimate | prefill |
|---|---|---:|---:|---:|---:|---:|---|
| docs.vlang.io, memory management (head) | prose with code | 12 000 | 12 010 | 3115 | 3236 | **1.04** | 3236 tokens, 1615 ms — 2004 tok/s |
| ru.wikipedia, lexical analysis (whole) | Cyrillic prose | 7056 | 10 491 | 2748 | 2100 | 0.76 | 2100, 814 ms — 2580 |
| en.wikipedia, byte pair encoding (whole) | English prose, citations | 10 016 | 10 059 | 2628 | 3043 | **1.16** | 3043, 1143 ms — 2662 |
| api.github.com, one repository (whole) | JSON | 6814 | 6814 | 1816 | 2332 | **1.28** | 2332, 910 ms — 2563 |
| serde_json `src/value/mod.rs` (head) | Rust source | 12 000 | 12 008 | 3121 | 3646 | **1.17** | 3646, 1375 ms — 2652 |

Every prefill is the whole prompt: nothing of it is in the cache. On the
4090 at 2000–2700 tokens a second the hold a cancel would wait for is
under a second — no note; on the CPU build's 38 tok/s
([slow-prefill-detection.md](slow-prefill-detection.md) §2.1) the same
3236 tokens are **85 s**, against the summary's stream limit of 90 s
(§4).

The second run, the first and the fourth page again, with the sampling as
the test context has it and then with reasoning muted:

| page | reasoning | finish | reply text | thoughts | completion tokens | what the tool returned |
|---|---|---|---:|---:|---:|---|
| vlang (head) | the template's default | `Length` | 0 bytes | 3439 bytes | 768 | the attachment line alone — no summary |
| GitHub JSON | the template's default | `Length` | 0 | 3061 | 768 | "summary unavailable", then the JSON itself |
| vlang (head) | `reasoning_budget = 0` | `Stop` | 3656 | 0 | 587 | a summary |
| GitHub JSON | `reasoning_budget = 0` | `Stop` | 722 | 0 | 161 | a summary |

Two side facts of the second run: the server's `usage` reported
`reasoning_tokens = 0` on the streams that were nothing but thoughts (the
counter is not separated on this build; the fact is on the chunks); and
the prefill of a page fetched again minutes later was **4–6 tokens** —
the summarizer's prompt sat in a slot's prefix cache from the first run,
so the sample is cold the first time a page is asked about and under the
rule's floor after.

### 2.2 The code

- `summarize_text` (`fetch.rs:411`): the estimate of the request
  (`estimate_prompt(system, [task])`), the reservation priced under
  `Shape::Summary` with the reply cap, held around the stream; the
  collect reads `Text`, breaks at `Finished`, and matches
  `ChatChunk::Usage(_) => {}`. Its sampling is the turn's effective
  sampling with `max_tokens` capped — nothing about reasoning. Its
  callers: `inline_result` (a page under the attachment budget — the
  summary, or the text under "summary unavailable") and `attached_result`
  (over it — the head's summary, if any, then the attachment line); both
  discard an empty summary quietly.
- `ToolOutcome` (`tools/mod.rs:421`): `result`, `effects`, `images`, and
  `wrote` — a fact the tool reports for the loop that called it, set on
  one path and read by the silent loops
  ([acted-by-effect.md](acted-by-effect.md) §3.1). Every producer builds
  it through `text`/`with_effects` and sets `wrote` through `wrote_if`;
  the two constructors are the only literals.
- The turn's folds: a concurrent segment's `invoke_member`
  (`generation.rs:2491`) maps the outcome to `CallDone { result, effects }`
  and the round writes the `CallDone`s back in the model's order; the
  sequential path (`:2706`) maps the same outcome inline. The turn keeps
  its largest prefill sample at the end of each round
  (`last_usage.prefill`, `:2005`, `max_by_key(tokens)`) and offers it
  once at the landing (`note_slow_prefill`, `:1022`).
- The silent loop's fold: `invoke_allowed` (`tool_loop.rs:512`) returns
  `(String, bool)` — the result and `wrote`; `run_rounds` keeps its
  largest sample on an out-parameter from its own streams and lands it as
  `BgDone.prefill` ([loop-timings.md](loop-timings.md) §3).
- Reasoning muted up front: `title.rs:152`, `compaction.rs:324`,
  `impersonation.rs:204`, the dialogue's director (`generation.rs:3813`);
  the turn's muted retry of an empty reply (`:2041`). The wire sends
  `reasoning_budget` and `chat_template_kwargs.enable_thinking = false`
  together (`wire.rs:307`), since the built-in formats read the one and
  the models' templates the other; each cloud dialect strips what it
  does not take.

## 3. Design

### 3.1 The summary records its usage

At the `Usage` chunk of its stream, under its own kind:
`budget.record_usage(Shape::Summary, estimate, u.prompt_tokens)` when the
context carries a budget — the title's shape, beside the estimate its
reservation was priced from. The rule inside the kind is the rule as it
is: the latest page wins, floored at 1.0. A Cyrillic page's 0.76 stores
1.0; a JSON page's 1.28 prices the next summary — of whatever page — 28 %
over its raw estimate until a page at or under 1.0 resets it. Where that
over-counts, a wait (R6); the under-count of §1 is gone for every page
after the first of its kind. A stream that ends at the limit never
reaches the chunk: nothing recorded, the reservation spent as priced.

### 3.2 The summary's sample rides the outcome

`ToolOutcome.prefill: Option<Prefill>` — the engine's own timing of the
request a tool made on its own, the `wrote` shape: a fact the tool reports
for the loop that called it. `summarize_text` returns the stream's
`prefill` beside the text; both callers set it on the outcome. The turn's
loop folds a call's sample into its largest — `CallDone.prefill`, then
`max_by_key(tokens)` into `last_usage.prefill` where the round's results
are written back; the sequential path the same inline — and the landing
reads what it always read. The silent loop folds it into its
out-parameter at `invoke_allowed`, and `BgDone.prefill` carries it to the
one offer in `handle_bg_done`. The two constructors default the field to
`None` and the one producer sets it — a `with_prefill` beside `wrote_if`.
No new offer site: two folds, one field.

### 3.3 The summary mutes reasoning

`reasoning_budget: Some(0)` in the summary's sampling, the title's shape:
a one-shot request for a page's retelling, with a reply cap that the
thoughts of a 27B model ate whole. The alternative — the turn's muted
retry — pays the 3000-token prefill and the cap twice for what one muted
request gives.

### 3.4 What changes in the numbers

Under §3.1 the JSON page's summary of §2.1 prices the next one at
1816 × 1.28 + 768 = 3092 instead of 2584 — the size the server counted
plus the cap. Under §3.3 the same page's summary is 722 bytes of text in
161 tokens instead of 3061 bytes of thoughts in 768. Under §3.2 the CPU
build, an external server on its ordinary warm day, notes the 85 s hold
at the first page fetched rather than at the first roll or reflection —
whichever comes first still decides; the rule's one note per server
session is untouched.

## 4. Difficult spots

- **`Usage` before `Finished`.** llama.cpp sends the usage on the last
  content chunk and the collect breaks at `Finished` — the record and the
  sample are taken at the `Usage` chunk, as the title's are; nothing after
  the loop would run.
- **The fold needs `&mut self`; the segment runs on `&self`.** A
  concurrent segment's members are futures inside one task over a shared
  borrow (`invoke_member(&self, …)`), so the sample rides `CallDone` out
  of the segment and folds where the round consumes the segment's results
  in the model's order — one `max_by_key` over the calls, then into
  `last_usage.prefill`. The sequential path folds inline.
- **The CPU build's limit.** 3236 tokens at 38 tok/s is 85 s of the
  summary's 90 s stream limit, before a token of the reply: a
  12 000-character head cannot end inside the limit there, and the tool
  degrades as it did (the text, or the attachment alone). The JSON page —
  2332 tokens, 61 s, then a 161-token reply — can, barely. The stage-1
  smoke's page is the JSON one; the limit itself is §7's question, the
  sibling of the loop's ([loop-timings.md](loop-timings.md) §7).
- **A page fetched twice.** The second summary's prompt rides the prefix
  cache (4–6 tokens processed): a sample under the rule's 256-token floor,
  ignored; the ratio the same as the first time.
- **The clouds.** `prefill` is `None` from every provider but llama.cpp —
  the outcome carries none. The record happens under whatever budget the
  mode has (a permit count and no pool on a cloud): a ratio stored and
  never priced against a pool. The muting is what the title already sends
  on every provider; the dialects' field lists decide what leaves.
- **`reasoning_tokens` reads 0** on a stream that was nothing but
  thoughts (§2.1): the muting is decided by design, not by the counter,
  and nothing in this track reads it.
- **A test engine's stream** must carry `Usage` with a `prefill` for the
  fold tests — the `timed` streams of the loop-timings tests already do;
  `fetch.rs`'s scripted engine yields the chunk itself.

## 5. Forks

- **F1. The record.** (a) **At the `Usage` chunk, under `Summary`, when
  the context has a budget** *(recommended — the item as recorded; the
  one kind measured above 1.0 as a rule, and the under-count is the
  failure the budget exists for)*. (b) Leave the kind unrecorded — the
  summary is rare (behind `web_enabled`), its population mixed, the ratio
  flipping between 1.0 and 1.3 by page. (c) Price the summary under
  `Turn` — the last track separated the kinds precisely because their
  populations differ; the summary's differs most.
- **F2. The sample.** (a) **`ToolOutcome.prefill`, folded by both loops
  into the sample they already keep** *(recommended — a stream of the
  turn by spec §9.3.1's own words, so the turn's largest sample should
  include it; the `wrote` shape, one field and two folds, no new offer
  site; the one cold prompt of a warm session's first page)*. (b) Not this
  track — one question over the one-shot requests' samples, since
  impersonation's (cold, the whole conversation, 10 643 tokens in the last
  track's run) and the title's are equally unread. (c) A slot on
  `ToolContext` the tool writes into — a shared cell in a per-turn
  snapshot bundle, against its grain, and a second route beside the
  outcome's.
- **F3. The empty summary.** (a) **Reasoning muted up front,
  `reasoning_budget: Some(0)`** *(recommended — the shape of every other
  one-shot request; found by this track's probe on the gate model, one
  line and one assertion; without it the track records the usage of a
  request that produces nothing)*. (b) The turn's muted retry — the
  prefill and the cap paid twice. (c) A separate track — the inline path
  degrades to the text; but the attached path loses its summary silently
  and the cap burns as thoughts on every fetch.
- **F4. The live smoke's shape.** (a) **`fetch_url` invoked directly
  against the live engine under a budget, in `fetch.rs`'s own live
  module** *(recommended — no model decision to make the call; asserts
  the record's ratio, the outcome's sample and the summary's text in one
  run; the fold into the turn and the loop is the unit tests')*. (b) A
  turn through the orchestrator, `prompt_estimate_e2e_live` extended —
  the model must choose to call the tool, and on the CPU build the turn's
  own first round samples 4300 cold tokens, so the note's source could
  not be told apart.

## 6. Tests and the live run

- `fetch.rs`: the summary records under `Summary` and leaves `Turn` at
  1.0 — a scripted engine whose stream carries a `Usage` twice the
  estimate, `density(Shape::Summary)` read after; no budget, no record.
  The outcome carries the stream's `prefill` on both paths (inline and
  attached); a stream without one carries `None`. The request mutes
  reasoning — `summarize_builds_single_turn_request_with_focus` asserts
  `reasoning_budget == Some(0)`. The permit and reservation tests
  unchanged.
- `tests/generation.rs` (or `tests/tools.rs`): a turn whose tool returns
  a slow sample on a fast engine — a test tool registered through
  `extra_tools`, `Prefill { tokens: 4096, ms: 100_000 }` — lands with the
  slow-prefill note; the same tool returning `None` lands without; two
  calls in one round keep the larger.
- `tests/silent.rs`: a reflection allowed a test tool that returns a
  sample lands `BgDone { prefill: Some(…) }` with the larger of the
  tool's and its own stream's, and the landing notes.
- `tests/live.rs` untouched; **live** (F4a): `summary_usage_e2e_live` in
  `fetch.rs` — the JSON page under a budget of four over 16 384: the
  result carries a summary (not "unavailable", not the attachment line
  alone), `density(Shape::Summary) > 1.0`, the outcome's `prefill` is
  `Some`. Run on the GPU stack; on the CPU build as far as the 90 s limit
  allows (§4).

### 6.1 Runs

`summary_usage_e2e_live`, the JSON page (6814 bytes; estimate 1816 with
the instruction), `fetch_url` invoked directly under a budget of four
sessions over 16 384:

| host | time | exact | exact / estimate | prefill | the summary |
|---|---:|---:|---:|---|---|
| the LAN stack (Qwen3.6-27B, 4090) | 8.7 s | 2286 | **1.29** | 2286 tokens, 1203 ms — 1900 tok/s | seventeen lines, the star count among them |
| the CPU build (gemma-3-4b, `-ngl 0`) | 86.2 s | 2735 | **1.51** | 2735 tokens, 76 139 ms — 36 tok/s | four sentences |

Both a real summary — reasoning muted, the "summary unavailable" fallback
with the JSON behind it on neither — both above 1.0 in the `Summary` slot
with `Turn` at 1.0, both with the sample on the outcome. The two exact
counts differ because the tokenizers do (Gemma's is denser on JSON). The
CPU build ended **4 s inside** the summary's 90 s limit, as §4 counted:
76 s of prefill at 36 tok/s and a ten-second reply — a page a few hundred
tokens larger would not have (§7). Its sample is the note's on such a
host — 2735 tokens at 36 tok/s, a 57 s hold at the external default batch
— had the fetch gone through a turn; the fold is the unit tests'. Unit:
2978 green, 151 ignored (2973 / 150 before: `keep_larger`, the record, the
outcome's sample on both paths, the turn's and the loop's folds through a
`SampledTool`, the live smoke).

## 7. Not in this track

- **The one-shot requests' samples** — done:
  [oneshot-samples.md](oneshot-samples.md), which measured impersonation's
  prompt as the largest a session makes.
- **A child run's sample** — its loop keeps its own; `run_child`'s
  `CallDone` carries none up, and a background run's landing offers none.
- **The summary's limit against a slow host** — 90 s against an 85 s
  prefill on the CPU build; the loop's limit's sibling
  ([loop-timings.md](loop-timings.md) §7).
- **The first request of a kind**, **a per-script density**
  ([title-impersonation-usage.md](title-impersonation-usage.md) §7).
- **The reasoning counter** — `reasoning_tokens` 0 on a stream of
  thoughts (§2.1); whoever reads it next should know.

## 8. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the samples the rule reads: a page summary's), §6.3 (the
  `Summary` kind records; a tool's outcome carries its sample), §9.2
  (`ToolOutcome.prefill`), §9.3.1 (the summary: reasoning muted, its
  usage recorded).
- architecture §3 (`session_budget.rs`), §8 (`ToolContext.sessions`'s
  paragraph; the outcome's fields).
- [admission-by-budget.md](admission-by-budget.md) §8,
  [title-impersonation-usage.md](title-impersonation-usage.md) §7 (the
  item done here); [slow-prefill-detection.md](slow-prefill-detection.md)
  §7, [loop-timings.md](loop-timings.md) §7 (the sample's fourth source).
- CHANGELOG (Fixed: the page summary on a thinking model came back empty;
  Added: the summary's usage), journal `engine.md` and `tools.md`,
  CLAUDE.md's status line and count.
