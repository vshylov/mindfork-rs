# Cache hits in the token counter — `cached_tokens` from every wire

Status: **built; live GO on every wire** (§8); merged as #773 and **released in
0.19.0** (2026-10-18). The roadmap keeps it in its Closed list, and what was left
out (§6) under "The cache share — leftovers".
*After the release:* its first release check found the prefix missing on most
turns — the accident §1 relies on had stopped holding on 2026-08-08 — and the
stable observation selection §6 leaves out was then built, as F1 (e1) of
[prompt-caching.md](prompt-caching.md) (the finding in §2.2, the build in §5,
*F1 revisited*).
The script read 0% → 98–99% → 0% (turn 1, turn 2, the control) on four arms:
llama.cpp with Qwen 3.8 27B, llama.cpp with Gemma 4 31B, OpenAI and OpenRouter.
Gemini and Anthropic reported their figure on every request: a miss each time,
because Gemini's cache is best effort and nothing asks Anthropic to cache. *User's
decision, 2026-10-17: F1 (b), F2 (c), F3 (a), F4 (a), F5 (a) — each the
recommendation (§5).* Stage 0, the probe (§7), ran on 2026-10-17 on OpenAI,
Gemini and OpenRouter. Its results are in §3.1, and they settle the parser's
open question: Gemini omits a zero. The
roadmap item was *`cached_tokens` in the token counter* under §Context and
tokens. The groundwork is [prompt-caching.md](prompt-caching.md): its §3 measured
what a broken prefix costs on three providers, its fork F3 recommended this
counter, and its §6 lists it as one of the three things still available while
the prompt layout stays as it is. One stage, one pull request.

## 1. Why

Every provider we speak to reuses the computation of a prompt whose beginning
it has seen, and every one of them reports how much it reused. The app reads
none of it. Three things follow.

- **A cache that stops working is invisible.** prompt-caching.md §2.1 measured
  the prefix as stable today **by accident**: the self-model block's volatile
  part, the relevance-selected observations, is truncated away on the real
  profile. The day it renders, every turn's prefix changes at the end of
  `system`, and the whole conversation is reprocessed. llama.cpp went from 100%
  to 0% reuse on that edit, with 3.5× the prefill. On OpenAI 5.6+ and Anthropic a
  miss is worse than a lost discount: the rewrite is billed at 1.25×. Nothing on
  screen and nothing in the log would say so. *Since measured otherwise
  (2026-10-18):* the masking had stopped holding on 2026-08-08, when the section
  budgets made the top observation always render, and the counter showed it on
  its first release check — the second turn of a chat read nothing back
  ([prompt-caching.md](prompt-caching.md) §2.2).
- **The deferred caching work has no instrument.** Anthropic breakpoints,
  `prompt_cache_key` and the stable observation selection (prompt-caching.md §5
  F1(e), §6) can only be judged by the figure this track reads.
- **A latent defect in the Anthropic client.** With caching on, Anthropic's
  `input_tokens` is only the *uncached remainder*: 13 tokens for a 7.4k prompt
  (prompt-caching.md §3.3). The client sends no `cache_control` today, so the
  figure is whole. On the day a breakpoint lands, `prompt_tokens` collapses, and
  everything that reads it reads a 13-token conversation: the counter, the
  compaction trigger (spec §6.7) and the budget's calibration
  ([admission-by-budget.md](admission-by-budget.md) §4.3). The prompt-cut
  detection already keeps out of clouds partly for this reason
  ([prompt-cut-detection.md](prompt-cut-detection.md) §3.3).

## 2. What the code does today (read at `a2cdbe02`)

`TokenUsage` (`shared/api/contract.rs`) carries `prompt_tokens`,
`completion_tokens`, `reasoning_tokens` and llama.cpp's `prefill`. It has no
cache field. Four clients build it, in five places:

| Wire | Built in | `prompt_tokens` from | Cache fields on the wire | Read today |
|---|---|---|---|---|
| OpenAI-compatible: llama.cpp, vLLM, LM Studio, Ollama, xAI, OpenRouter, any external | `openai/client.rs` | `usage.prompt_tokens` | `usage.prompt_tokens_details.cached_tokens`; llama.cpp also `timings.cache_n` | no. `wire::Timings` parses `prompt_n` and `prompt_ms`; `cache_n` appears only in a test fixture |
| OpenAI Responses (the OpenAI cloud) | `openai/responses/client.rs`, on `Completed` and `Incomplete` | `input_tokens` | `input_tokens_details.{cached_tokens, cache_write_tokens}` | no |
| Anthropic | `anthropic/client.rs`: `message_start` keeps the input, `message_delta` builds the chunk | `input_tokens` | `cache_read_input_tokens`, `cache_creation_input_tokens` | no. `AntUsage` has input and output only |
| Gemini | `gemini/client.rs` | `promptTokenCount` | `cachedContentTokenCount` | no |

**Every consumer of `prompt_tokens` means the whole prompt**:

- the counter: `stream_round`'s `Usage` arm → `TokenReport` → `AppEvent::TokenUsage` → `ChatScreen::set_token_usage` → `StatusModel` → `token_counter_span`;
- the turn's `TurnUsage`, which feeds the compaction trigger and `next_prompt_estimate`;
- `SessionBudget::record_usage`, the estimate's calibration, called from the turn and from the tool loop;
- `PromptCut::judge`;
- the title, impersonation, page-summary and roll paths, each of which adds prompt and completion into a total.

The design writes this down as an invariant (§4.2) instead of leaving it
implied.

**The bar today** reads `tokens: {approx}{total}{reason}`, where `reason` is
` (reasoning N)`. The pill drops whole chips at the edge (spec §11.1.1). A longer
counter therefore costs hints sooner, and it never shows a number cut in half.

**Two traps checked and absent.** `TokenUsage` travels as a stream chunk, not as
a trait method, so `RetryBackend` passes it through. The silent-`None`
delegation hole ([lessons](../lessons.md) §9, "a trait method with a default
plus a decorator") does not apply. The live harness goes through
`live_backend()`, which wraps the backend, so the live run exercises the
decorator too.

**A sub-agent's report drops `context`**, because the bar shows the parent's
conversation. The cache figure is about the same request as `context`, so it is
dropped with it.

## 3. What each provider says

| Provider | Caching | Field(s) | `prompt` includes the cached part? | Evidence |
|---|---|---|---|---|
| llama.cpp | automatic (`cache_prompt`) | `timings.cache_n` (since 2025-09, PR #15827); `prompt_tokens_details.cached_tokens` (since 2026-03, PR #19361), both sent and equal | **yes**: `prompt_tokens` = `cache_n` + `prompt_n` | **measured 2026-10-17** (§3.1): build 11715, Qwen 3.8 27B and Gemma 4 31B |
| Ollama | automatic | none known on `/v1` | **yes** (1797 reported with 1491 cached, prompt-cut-detection.md §6) | **to measure** whether any field comes |
| LM Studio | automatic | unknown | **yes** (prompt eval 42, `prompt_tokens` 1237, [local-servers.md](local-servers.md) §3) | **to measure** |
| vLLM | automatic prefix caching | `prompt_tokens_details.cached_tokens`, only with `--enable-prompt-tokens-details` | yes | documented |
| xAI | automatic | `prompt_tokens_details.cached_tokens` | yes | documented; no key in this environment |
| OpenRouter | depends on the provider it routes to | `prompt_tokens_details.{cached_tokens, cache_write_tokens}`, both always sent | **yes** | **measured 2026-10-17** (§3.1): OpenAI, Anthropic and Gemini routes |
| OpenAI Responses | automatic, 1024 tokens minimum | `input_tokens_details.{cached_tokens, cache_write_tokens}`, both always sent | **yes** | **measured 2026-10-17** (§3.1): `gpt-6.1-sol`, `gpt-5.6-sol`, `gpt-5.4-mini` |
| Anthropic | **explicit only** (`cache_control`) | `cache_read_input_tokens`, `cache_creation_input_tokens`, both always sent, on `message_start` and again on `message_delta` | **no**: `input_tokens` is the remainder | **measured 2026-10-17** (§3.1): zeros without `cache_control`; 13 + 7372 with it on 2026-08-08 (§3.3 there) |
| Gemini | implicit; 2048 tokens minimum on 2.5, 4096 on 3.x | `cachedContentTokenCount`, **absent on a miss** | **yes** | **measured 2026-10-17** (§3.1): `gemini-3.8-flash`, `3.5-flash`, `2.5-flash` |

A row marked "documented" is a claim about a provider: "a rule drawn from one
model is a guess about the provider" ([lessons](../lessons.md) §3). The rows
still marked so are measured by the live run (§7) before their parser ships.

### 3.1 The probe, 2026-10-17

Stage 0 (§7) ran on every wire with a key in this environment. The last three
rows were added the same day, once an Anthropic key and the local
`llama-server` on an RTX 4090 were at hand. Each request was
the same ~30 KB system message (8400 tokens on OpenAI's tokenizer, 9800 on
Gemini's) behind a fresh nonce, and was sent three times in a row through the
client's own request builder. Gemini 2.5 Flash and 3.5 Flash were run again in
a second probe, with the same outcome.

| Wire | Model | Request 1 (cold) | Requests 2–3 | A miss reads as |
|---|---|---|---|---|
| Responses | `gpt-6.1-sol` | cached 0, **written 8406** of 8409 | cached **8406** (99.96%), written 0 | `0`; both keys on every reply |
| Responses | `gpt-5.6-sol` | cached 0, **written 8403** of 8406 | cached 8403, written 0 | `0` |
| Responses | `gpt-5.4-mini` | cached 0, written 0 | cached **8192** of 8407 (97%) | `0`; the write key present at 0 |
| Gemini | `gemini-3.8-flash` | no cache key | cached **7782** of 9825 (79%) | **the key absent** |
| Gemini | `gemini-3.5-flash` | no cache key | cached 7777 of 9820 (79%) | the key absent |
| Gemini | `gemini-2.5-flash` | no cache key | **no cache key in 6 of 6 requests** across both probes | the key absent |
| OpenRouter | `openai/gpt-5.4-mini` | cached 0, written 0 | cached **8192** of 8404 (97%) | `0`; both keys on every reply |
| OpenRouter | `anthropic/claude-haiku-4.5` | cached 0, written 0 | cached 0, written 0 | `0` (nothing asks it to cache) |
| OpenRouter | `google/gemini-3.8-flash` | cached 0, written 0 | cached **4080** of 9825 (42%) | `0` |
| Anthropic | `claude-haiku-4-5` | read 0, written 0 of 9849 | read 0, written 0 | `0`; both keys on `message_start` and `message_delta` (nothing asks it to cache) |
| llama.cpp b11715 | Qwen 3.8 27B Q4_K_M | cached 0 of 9914 (`cache_n` 0) | cached **9910** of 9914 (`cache_n` 9910, `prompt_n` 4) | `0`; the details and `timings` agree |
| llama.cpp b11715 | Gemma 4 31B Q4_0 | cached 0 of 9840 | cached **9839** of 9840 | `0` |

What this changes in the design, and what it confirms:

- **Gemini leaves a zero out.** A miss has no `cachedContentTokenCount` at all,
  and the same proto3 omission dropped `thoughtsTokenCount` from a reply that
  did not reason. Gemini always knows its count, so the Gemini client reads an
  absent key as `Some(0)` (§4.3). Read as `None`, every miss would look like a
  server that does not report, which is the confusion §4.1 exists to prevent.
  Gemini has no write figure, so writes are `None` there.
- **Responses and OpenRouter send both keys on every reply**, zeros included.
  They parse as they come.
- **On 5.6+ the cold request writes the whole prompt.** 8406 of 8409 tokens,
  billed at 1.25×. That is the figure F1 (b) carries. The older generation
  writes nothing and caches in 128-token steps (8192); 5.6+ reuses all but the
  last three tokens.
- **The share has to round down.** 8406 of 8409 is 99.96%, and must not read
  as `100%` (§4.4).
- **The prompt count includes the cached part** on all three wires (8409 with
  8406 cached; 9825 with 7782). The §4.2 invariant holds there as it stands.
  Only the Anthropic client needs the sum.
- **Gemini 2.5 Flash's implicit cache never engaged** in six back-to-back
  requests of 9800 tokens. On that model `cache 0%` is the honest reading, not a
  defect of ours.
- **The gateway's Gemini route caches about half what Google does directly**
  (4080 against 7782 tokens). That is the route's own behaviour, and nothing for
  the app to build. The gateway's meter shows what a hit is worth: the OpenAI
  route cost $0.0063 cold and $0.0008 warm.

**Not measured here:**

- Ollama and LM Studio;
- xAI: no key in this environment;
- vLLM.

The parser for each follows §4.3 and is held to its row by the live run (§7).

**Cost:**

- the gateway's own meter: $0.054 for its nine requests;
- the direct OpenAI and Gemini requests: about 75k and 150k input tokens,
  mostly read from cache. The direct Gemini figure is about 118k.

## 4. Design

1. **The domain.** `TokenUsage` gains `cached_tokens: Option<u32>`, plus
   `cache_write_tokens: Option<u32>` under F1. `None` means the wire said
   nothing, i.e. a server that does not report. `Some(0)` means it said, and
   nothing was reused. That difference is the whole instrument. A `0` that reads
   like silence is a broken prefix nobody sees, the same shape as "the honest *I
   cannot say* and the bug *I forgot to ask* are the same value"
   ([lessons](../lessons.md) §9).
2. **The invariant**, written on `TokenUsage::prompt_tokens`: the whole prompt,
   cached part included, on every wire. The Anthropic client sums
   `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`. The
   other wires already report the whole prompt, measured where §3 says so and
   checked by the probe for the rest. No consumer of `prompt_tokens` changes.
3. **Parsing**, per wire. Every new field is `#[serde(default)]`, and absence
   maps to `None`. The exception is a wire measured to omit zeros (§3.1).
   - OpenAI-compatible: `usage.prompt_tokens_details.{cached_tokens,
     cache_write_tokens}`, which the gateway sends on every reply (§3.1). If
     the details are absent and `timings` came, `timings.cache_n` (a llama.cpp
     from before 2026-03). A server that sends neither reads `None`.
   - Responses: `input_tokens_details`, both keys, on both `Completed` and
     `Incomplete`.
   - Anthropic: the two cache fields ride `message_start`'s usage, so they are
     kept next to `input_tokens` until `message_delta` builds the chunk.
   - Gemini: `usageMetadata.cachedContentTokenCount`. An **absent key is
     `Some(0)`**, because Gemini omits a zero (§3.1). Writes are `None`, since
     Gemini has no write figure.
4. **The counter.** `TokenReport` and `AppEvent::TokenUsage` gain `cached:
   Option<u32>`. It is sent only with the exact `context`, from the `Usage` arm,
   and never with an estimate. The screen resets it when a turn starts, as it
   does `context` and `reasoning`. The share is computed in the widget against
   the `context` it is shown next to, rounded **down**, so a single missed
   token never reads as 100%. A child run drops it together with `context`.
   How it reads is F3.
5. **The log.** Each round of the turn and of a sub-agent writes one line with
   `prompt`, `cached` and `written`. That is where a developer looks for "the
   prefix broke on round one", which the bar may not show (F2).
6. **Nothing is stored** (F4).

## 5. Forks

### F1. What the domain carries

- **(a)** `cached_tokens` only: what was reused.
- **(b)** (a) plus `cache_write_tokens`: what was written.

**Recommendation: (b).** The Anthropic sum (§4.2) has to parse writes anyway. On
OpenAI 5.6+ and Anthropic a broken prefix shows up as *writes*, billed at 1.25×,
not merely as missing reads (prompt-caching.md §3.2–§3.3). Carrying it costs
one field. Whether it is shown is F3's business.

### F2. Which request the bar describes

A turn with tools is several requests, and the bar's `context` is the latest
one's.

- **(a)** The latest round's cache figure, paired with the `context` it is shown
  next to. Simple and consistent. On a tool turn the later rounds ride the cache
  the first round wrote, so it reads high even when the turn's first request
  missed.
- **(b)** The turn's first round: the one that says whether the prefix survived
  since the last turn. It pairs with a different `context`, so the share is no
  longer of the number beside it.
- **(c)** (a) on the bar, plus every round's figures in the log (§4.5).

**Recommendation: (c).** The bar keeps saying one true thing about one request.
A first-round miss, which is the defect this exists to catch, is in the log on
every turn, and in the live run's assertion (§7).

### F3. How it reads in the bar

- **(a)** The share, appended to the existing annotation: `tokens: 7385
  (reasoning 120 · cache 99%)`. It is shown whenever the wire reported the field,
  `0%` included.
- **(b)** The same, but hidden at `0%`.
- **(c)** The count, like reasoning: `(cached 7372)`.
- **(d)** Nothing in the bar: log only.

**Recommendation: (a).** The share is what tells the story (99% against 0%), and
it is shorter than the count, which matters in a pill that sheds hints for width
(spec §11.1.1). Hiding `0%` (b) makes a miss look exactly like a server that
does not report, which is the confusion §4.1 exists to prevent.

The honest cost: a chat below a provider's minimum reads `cache 0%` on every
turn. That means 1024 tokens on OpenAI and 4096 on Haiku 4.5 and Gemini 3.x.
Anthropic reads `cache 0%` always, because nothing asks it to cache. The manual
says why in one sentence. Writes are not shown, to keep the pill short.

### F4. Stored on the message?

- **(a)** No: the bar and the log only.
- **(b)** An additive `MessageMetadata` field, like `cost_nanos`.

**Recommendation: (a).** Nothing would read it. prompt-caching.md F3(c) reached
the same conclusion, and a per-round share has no screen to land on. It is
additive whenever a consumer appears (ADR 0006).

### F5. The request side

- **(a)** Reporting only: no request changes.
- **(b)** Plus `prompt_cache_key` on Responses: one field, keyed by the chat id
  (prompt-caching.md F2(c)).
- **(c)** Plus Anthropic breakpoints (F2(b) there).

**Recommendation: (a).** This track is what will tell whether (b) and (c) pay
off. Building them in the same pull request would measure them with an
instrument nobody has validated yet. They stay roadmap items. Under (a),
Anthropic honestly reads `cache 0%`.

## 6. Not in this track

- **The request side**, per F5: breakpoints, `prompt_cache_key`, and the stable
  observation selection (prompt-caching.md §5 F1(e)). The last has been built
  since, as F1 (e1) there (§5, *F1 revisited*): the prefix now holds on 8 of 8
  follow-ups, read through this track's figure. Breakpoints and `prompt_cache_key` remain open.
- **DeepSeek's dialect** (`prompt_cache_hit_tokens` / `_miss_tokens`). An
  external server that speaks it reads `None`. On demand.
- **Money.** The gateway's `cost_nanos` is the only meter the app has, and it is
  recorded but not shown ([openrouter-mode.md](openrouter-mode.md), F11).
- **Muted turns** (title, roll, impersonation, page summary). They do not drive
  the bar, and their usage paths stay as they are.

## 7. Measurement before code, and the live run

**Stage 0: the probe**, before any parser is written. It was run on
2026-10-17, and the results are in §3.1. The arms are kept as the instrument to
re-run when a new model lands ("a wire shape seen once per reply is not a
contract", [lessons](../lessons.md) §9):

- `shared/api/cache_probe_tests.rs`;
- a `cache_probe_tests.rs` beside each client;
- run them with `cargo test cache_probe -- --ignored --nocapture --test-threads=1`.

There is one `#[ignore]` probe per wire:

- It sends two identical requests through the client's own request builder,
  following "use the project's own client as the instrument"
  ([lessons](../lessons.md) §3).
- Each request is about 5000 tokens, above every documented minimum.
- A per-run nonce goes at the front, because OpenAI's cache survives between
  runs and contaminated the first probe of prompt-caching.md §3.2.
- It prints the raw `usage` object of each request. The unit tests' fixtures are
  that output.

| Row | Available here |
|---|---|
| OpenAI Responses | key in the environment |
| Gemini (one 3.x model) | key in the environment |
| OpenRouter: OpenAI-, Anthropic- and Gemini-routed models | key in the environment |
| llama.cpp, current build | a local or LAN `llama-server`, or the rented gate (`HF_TOKEN` is set) |
| Ollama, LM Studio | if installed locally |
| Anthropic, xAI | **no key in the environment**: the owner's call |

About 10k input tokens per row, about 100k in all: a few cents.

**Unit tests**, written next to the code:

- each wire's probe fixture produces the expected `TokenUsage`;
- the Anthropic sum;
- absent versus zero on every wire;
- the bar's label in `en` and `ru`: `Some(0)`, `Some(n)`, `None`, each with and without reasoning;
- the wiring: a `Usage` chunk with a cache figure arrives as `AppEvent::TokenUsage.cached`;
- a child run drops it;
- the small-window gate stays green with the longer counter.

**The live run, `cached_tokens_e2e_live`.**

- **llama.cpp.** Two turns in one chat. Turn 2's first round must report
  `cached > 0` and a `prompt_tokens` no smaller than turn 1's. A **control
  arm** changes the system prompt's first byte, and `cached` must drop to near
  zero. This proves the instrument can say "missed", because "a gate that
  passes is indistinguishable from a gate that does nothing"
  ([lessons](../lessons.md) §2).
- **The clouds.** Assert that the field was parsed (`is_some()`, or `Some(0)` on
  Anthropic) and log the value. A hit is the provider's best-effort routing, and
  "a provider's optional output cannot be an assertion"
  ([lessons](../lessons.md) §2).

**Docs at the end** (AGENTS.md §4):

- spec §11.1 (the counter) and §6.6;
- architecture §5 (the contract);
- the manual's status-bar paragraph;
- CHANGELOG *Added*;
- the engine journal;
- the roadmap item closed, with a pointer from prompt-caching.md §6.

## 8. The live run, 2026-10-17

`app/orchestrator/tests/cached_tokens_live.rs` runs one script per wire on the
**production** supervisor, so the turn goes through the client, the
`RetryBackend`, the orchestrator and the `AppEvent::TokenUsage` the bar reads.
The script is:

- a profile whose persona is about 10k tokens of stable text behind a per-run
  nonce, with no tools;
- two turns in one chat;
- the **control**: the persona's first byte changed, in a new chat.

Each row is the first round's exact counter. The cloud results were the same on
two consecutive runs. The llama.cpp arms ran against build 11715
(`9d1878a43`) on an RTX 4090 with `-ngl 99 -c 16384 -np 1 --jinja`:

| Arm | Model | Turn 1 | Turn 2 | Control |
|---|---|---|---|---|
| `cached_tokens_openai_e2e_live` | `gpt-5.4-mini` | 0 of 8689 (0%) | **8576 of 8708 (98%)** | 0 of 8689 (0%) |
| `cached_tokens_openrouter_e2e_live` | `openai/gpt-5.4-mini` | 0 of 8693 (0%) | **8576 of 8712 (98%)** | 0 of 8693 (0%) |
| `cached_tokens_gemini_e2e_live` | `gemini-3.8-flash` | `Some(0)` of 10888 | `Some(0)` of 10899 | `Some(0)` of 10888 |
| `cached_tokens_anthropic_e2e_live` | `claude-haiku-4-5` | `Some(0)` of 10251 | `Some(0)` of 10267 | `Some(0)` of 10251 |
| `cached_tokens_e2e_live` (llama.cpp) | Qwen 3.8 27B Q4_K_M | 0 of 10936 (0%) | **10932 of 10959 (99%)** | 0 of 10936 (0%) |
| `cached_tokens_e2e_live` (llama.cpp) | Gemma 4 31B Q4_0 | 0 of 10903 (0%) | **10903 of 10922 (99%)** | 0 of 10903 (0%) |

**Verdict: GO on every wire.**

- The figure reaches the bar on every wire.
- Where the cache served, the second turn reads its predecessor's prompt from
  it, and the control reads nothing.
- On llama.cpp this is asserted, not just logged: `cached > 0` on turn 2, a
  prompt still whole and larger than turn 1's, and a control under 5%. The two
  templates (Qwen's and Gemma's) gave the same picture.
- On Anthropic the prompt is the sum §4.2 asks for, though here it is the sum of
  `input_tokens` and two zeros. A non-zero cache field can only come from a
  request that asks for caching, which F5 leaves out. The sum is held by its
  unit tests on the 2026-08-08 figures.
- On Gemini the absent key arrives as the `Some(0)` §4.3 promised, not as
  `None`.
- The arms assert what is theirs to assert: a parsed figure, `0` included. The
  hit is logged, not asserted.

What the run found:

- **The smoke needs its own nonce.** The first runs had none, and a run's first
  request read 3840 of 8666 tokens (44%) from the run before it, its control as
  well. This is the contamination prompt-caching.md §3.2 recorded, met again. The
  persona now carries a session id after its first byte. That keeps runs apart
  and leaves the control's change at byte 0.
- **Gemini's implicit cache did not serve a continuation.** Through the app, no
  turn read anything, at 4.9k and at 10.9k tokens. Four raw requests isolated it
  (`gemini-3.8-flash`, 10.9k tokens):
  - a continuation sent right after its first request: missed, 2 of 2;
  - the first request repeated exactly: hit, 8830 tokens;
  - the next continuation: hit 1 time of 2.

  So the probe's hits (§3.1, identical requests) say nothing about the
  conversation's shape. On Gemini, `cache 0%` on a second turn is the provider
  being best effort, not a defect of ours. An arm asserting a hit there would
  measure Google's mood (lessons §2).
- **One OpenAI run went silent for five minutes** and failed on the wait's
  ceiling. That run had no nonce. It did not recur in the three runs since. The
  wait now names its step and the last eight events when it times out, so a
  recurrence says where it stopped.

**Cost:**

- OpenAI and OpenRouter: about 26k input tokens each per run, a third of them
  read from cache;
- Gemini: about 33k per run;
- the four raw Gemini requests: about 44k;
- Anthropic: about 30k for the smoke and 30k for the probe;
- llama.cpp: local, free.

**On the rented gate, 2026-10-18** (run 39143245277, `main` at `5f18fceb`, after
0.19.0 and prompt-caching's F1 (e1); Gemma 4 31B QAT Q4_0 on an L40S, an endpoint
of one slot). The gate lists every `#[ignore]` smoke by itself, so this was the
track's first run there, unchanged:

- `cached_tokens_e2e_live`: turn 1 `prompt 10904 cached 0` (0%), turn 2
  **10904 of 10923 (99%)**, the control `0` (0%) — every assertion held on a server
  the track had never seen.
- `cache_probe_external_live`: the same request three times read 3, then 9838 and
  9838 of 9839 tokens from the slot.
- The cloud arms and the cloud probes skipped there: the gate carries no keys for
  them.

The dispatch was 264 of 264 green across its four shards in 36 min.

