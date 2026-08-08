# Prompt caching — research and decision points

> Status: **research complete; implementation deferred** (user's decision,
> 2026-08-08 — see [§5 F1](#f1-prompt-layout--the-one-that-decides-whether-anything-else-is-worth-doing)).
> The measurements stand and the decision points are recorded; nothing is being
> built on them for now. Roadmap: §Context and tokens.
> Adjacent: [history-compression.md](history-compression.md) §2.3, spec
> [§6.6](../../spec.md) (KV cache and prefix caching),
> [ADR 0004](../decisions/0004-engine-contract-multi-provider.md).
> Measurements taken 2026-08-08 against live providers and a live `llama-server`.

## 1. What this is about

Every provider we speak to reuses the KV/prefix computation of a request whose
**beginning is byte-identical** to one it has seen. What that buys is real money
on the cloud (a 90% discount on the reused part) and real latency everywhere.
What it costs, when the prefix is not stable, is nothing — except on providers
that bill *cache writes*, where an unstable prefix is a **surcharge for nothing**.

The roadmap entry says caching is "unlocked by the stable daily self-model
injection". Measurement says the second half of that sentence is the whole
problem, and it is not about days.

## 2. What we send today

`build_request` assembles the system prompt in this order (spec §6.6, §6.7,
§9.7):

```
persona → rolling summary → attached files → self-model block → [conversation]
```

`generation.rs` appends the self-model **last**, deliberately: it is the most
volatile part, and the ordering rule the code follows is "most stable first"
(the same rule the compaction block's doc comment states). Within that block,
`injection_recent` picks observations **by relevance to the latest user
message** ("narrative as notes", Tier 2).

So the app's own ordering intent is right. **The container is wrong**: the
volatile block sits at the end of `system`, and `system` is the beginning of the
prefix — everything after it, i.e. *the entire conversation*, is invalidated
whenever it changes.

### 2.1 A measured surprise: today the block does not change — by accident

Measured against the real dev self-model (66 `@self` observations, a 1349-char
summary) and a live embedder, three consecutive questions on different topics:

| `prompt_cap` | rendered system | identical prefix across turns |
|---:|---:|---:|
| 1200 (default) | 2567 chars | **100%** |
| 2000 | 3367 chars | **100%** |
| 4000 | 5367 chars | **100%** |

The selected observation ids **did** differ per question — but the block never
reaches them: summary + goals + interlocutor traits exhaust the budget, and
`render_for_prompt` truncates. The prompt grows by exactly the budget delta
(2567 → 3367 → 5367), i.e. it is cut at the cap every time.

Two conclusions, pulling in opposite directions:

- **For caching:** the prefix is stable today, so caching works on this profile.
- **But by accident.** The volatility is not absent, it is *masked by
  truncation*. The moment the user shortens the summary — which the block
  actively asks them to do — the observations section renders, changes every
  turn, and the cache dies silently. A design that depends on a section not
  fitting is not a design.

**Adjacent defect, out of scope here, recorded for the self-model track:** on
this profile the relevance-based injection does nothing at all — the embedder is
queried every turn and the result is discarded by truncation, while the injected
text tells the model that observations "surface by relevance". Per-section
budgets (summary-as-snapshot, stage 3) reserve half the cap for "About you", but
goals and interlocutor traits then take the entire remainder.

## 3. Measurements

All three tables answer the same question — *what does changing the volatile
block cost?* — under two layouts:

- **A** — volatile block at the end of `system` (**what we do today**)
- **B** — volatile block moved **after** the conversation, into the new user turn

### 3.1 llama.cpp (`llama-server`, live, 4.7k-token conversation)

| layout | turn | prompt | reused | reprocessed | prefill |
|---|---|---:|---:|---:|---:|
| A | block unchanged | 4691 | 100% | 1 | 373 ms |
| A | **block changed** | 4687 | **0%** | **4686** | **2698 ms** |
| B | block unchanged | 4694 | 100% | 1 | 364 ms |
| B | **block changed** | 4690 | **99%** | **37** | **776 ms** |

The same edit costs the entire prefix in layout A and 37 tokens in layout B —
**3.5× the prefill latency**, growing linearly with the conversation.

### 3.2 OpenAI Responses (live, `gpt-5.6`, ~2.7k-token prompt)

| layout | turn | input | `cached_tokens` | `cache_write_tokens` |
|---|---|---:|---:|---:|
| A | cold | 2721 | 0 | 2718 |
| A | block unchanged | 2721 | 2718 | 0 |
| A | **block changed** | 2719 | **0** | **2716** |
| B | block unchanged | 2725 | 2722 | 0 |
| B | **block changed** | 2723 | **2197 (81%)** | 523 |

Caching is **automatic** — no breakpoints, no key, `store: false`. (This settles
a contradiction in OpenAI's own docs, one page saying caching needs explicit
breakpoints and another saying it is automatic above 1024 tokens.) Note the
right-hand column: on the 5.6 family a write costs **1.25×**, so layout A is not
merely "no discount" — it is a **standing surcharge**.

A methodology note worth keeping: the first run of this probe was contaminated —
OpenAI's cache survived between runs, so the "changed" case reported a hit from
the previous run's identical prompt. Every prompt in the final run carries a
unique session nonce.

### 3.3 Anthropic (live, `claude-haiku-4-5`, ~7.4k-token prompt)

Anthropic is the outlier: **nothing is cached unless we ask**. Measured, two
identical 6910-token requests with no `cache_control` — `write=0, read=0` both
times.

With a breakpoint after the stable head and another at the end of the prior
conversation:

| layout | turn | fresh | write | read |
|---|---|---:|---:|---:|
| A | block unchanged | 13 | 0 | 7372 |
| A | **block changed** | 13 | **2304** | 5067 |
| B | block unchanged | 27 | 0 | 7359 |
| B | **block changed** | 26 | **0** | **7359** |

In layout A the head survives (its own breakpoint) but **the conversation is
re-written on every turn the block changes**. In layout B nothing is rewritten.

Also visible in that table: `input_tokens` collapses to 13 — with caching on, it
is **only the uncached remainder**. The status bar must report
`input + cache_write + cache_read` or it will show 13 tokens for a 7.4k prompt.

**Two probe errors of mine, recorded because both would have produced a wrong
conclusion:** the first Anthropic run showed zeros everywhere and looked like
"breakpoints do not work" — the prompt was 3388 tokens against haiku 4.5's
**4096-token minimum**, i.e. exactly the silent no-op the docs warn about. The
second put the breakpoint *after* the volatile block, so layout B missed by
construction. The minimums are per model and non-monotonic (512 for Opus 5, 1024
for Sonnet 5, 4096 for Haiku 4.5 and Opus 4.5/4.6).

## 4. Provider contracts, condensed

| | opt-in? | minimum | read | write | reports |
|---|---|---|---|---|---|
| **llama.cpp** | automatic (`cache_prompt=true`) | none | free | free | `timings.cache_n` (since 2025-09), `usage.prompt_tokens_details.cached_tokens` (since 2026-03) |
| **OpenAI Responses** | automatic; `prompt_cache_key` improves routing; explicit breakpoints on 5.6+ | 1024 (strict on 5.6+) | 0.1× | **1.25× on 5.6+**, free before | `input_tokens_details.{cached_tokens,cache_write_tokens}` on `response.completed` |
| **Gemini (native)** | implicit, automatic for 2.5+ | 2048 (2.5) / 4096 (3.x) | 0.1× | not documented | `usageMetadata.cachedContentTokenCount` |
| **Anthropic** | **explicit only** (`cache_control`) | 512–4096 **per model** | 0.1× | 1.25× (5m) / 2× (1h) | `usage.cache_{creation,read}_input_tokens` on `message_start` |

Notes that matter for implementation:

- **Anthropic**: `system` must become an **array of text blocks** to carry a
  breakpoint (ours is a plain `String`); max 4 breakpoints; a read refreshes the
  5-minute TTL; the TTL is measured from the *start* of the request, so a slow
  turn eats into it. Changing the thinking configuration invalidates (it is
  rendered into the prompt) — our reasoning settings are user-editable.
- **OpenAI**: `instructions` is a plain string and **cannot** carry a
  breakpoint; the stable head would have to move into `input` as a
  `developer`-role message of `input_text` blocks. `prompt_cache_key` is free
  and we have the obvious key (chat id). Hits land in 128-token increments.
- **Gemini**: explicit `cachedContents` exists but bills **storage per hour**,
  is immutable except for expiry, and the generateContent caching page is now
  labelled *Legacy* — a poor fit for a conversation that changes every turn.
  Implicit caching needs nothing from us but a stable prefix.
- **llama.cpp**: `/props` reports **nothing** about cache configuration; infer
  from `timings.cache_n`. Default 4 slots with LRU eviction, so switching
  between chats evicts; `--cache-ram` (8192 MiB by default) parks evicted state
  in host RAM. `--cache-reuse` recovers *deletions*, not *replacements*, so it
  does not rescue a changed block at the front.

## 5. Decision points

### F1. Prompt layout — the one that decides whether anything else is worth doing

- **(a)** Leave as is. Caching keeps working only while the observations section
  is truncated away; it dies the day it is not, and on OpenAI 5.6+/Anthropic
  that failure costs a write surcharge rather than merely losing a discount.
- **(b)** Reorder inside `system` by stability: persona → attachments → summary
  → self-model. Cheap, but does not solve it: the volatile block still precedes
  the conversation.
- **(c)** Move the **volatile part only** — the relevance-selected observations
  — out of `system` and into the new user turn, keeping the stable self-model
  core (summary, goals, interlocutor model) in `system`. Measured: 0% → 99%
  (llama.cpp), 0% → 81% (OpenAI), and a 2304-token-per-turn write → 0
  (Anthropic).
- **(d)** Move the whole self-model block after the conversation.
- **(e)** Leave the block where it is and make its **content** stable instead:
  select observations by something that does not change per turn (the
  conversation as a whole, or a per-chat selection refreshed only on a
  compaction or every N turns) rather than by relevance to the latest user
  message. Untested — see below.

**Decision (user, 2026-08-08): none of (c)/(d) — implementation deferred.**

The recommendation here was (c) + (b), on the strength of §3. It was
**rejected, and the objection is sound**: many models react markedly worse when
extra data is appended to a *user* message. Moving the block out of `system`
does not merely change its position, it changes its **status** — a user turn is
read as part of what the human is asking, so the content gets echoed, argued
with, or attributed to the user, while `system` is trained to be authoritative.
That is precisely the risk the recommendation flagged as needing judgement
rather than measurement, and the asymmetry decides it: the saving is measurable
and bounded, the behavioural regression is neither, would land in the project's
flagship track (self-model), and would be noticed long after the change.

So the 2026-07-03 decision **stands**: the injection stays in `system`, and
losing prefix reuse when it changes remains the accepted price. What this
research adds is that the price is now known — §3 — rather than assumed, and
that today it is mostly **not being paid**, for the accidental reason in §2.1.

If this is ever revisited, the promising direction is **(e)**, not (c): it
removes the volatility instead of relocating it, so the prompt the model reads
is unchanged and the objection above does not apply. What it costs is the
per-turn relevance selection, which §2.1 measured as inert on the real profile
anyway. It would need its own measurement of whether a stabler selection is
worth less to the model than a per-turn one.

### F2. How far to go per provider

- **(a)** Layout only. Fixes llama.cpp, OpenAI and Gemini (all automatic); does
  **nothing** for Anthropic, which caches nothing without `cache_control`.
- **(b)** Layout + Anthropic breakpoints (`system` → array of blocks, a
  breakpoint after the stable head and one at the end of the prior
  conversation). Measured to work.
- **(c)** + `prompt_cache_key` on OpenAI (one line, chat id).
- **(d)** + OpenAI 5.6 explicit breakpoints (`instructions` → `input` blocks) and
  Gemini `cachedContents`.

**Recommendation: (b) + (c).** (d) is a large restructuring of two clients for a
marginal gain over automatic caching, and Gemini's explicit path bills storage.

### F3. Reporting

- **(a)** Nothing.
- **(b)** Show the cached share in the status-bar token counter.
- **(c)** (b) + record it in `Message.metadata`.

**Recommendation: (b).** All four providers report it; the counter already
exists; and it is the only way a user (or we) can tell that a cache silently
stopped working. (c) has no consumer yet.

### F4. Anthropic TTL

- **(a)** Default 5 minutes.
- **(b)** 1 hour (2× write) as a setting.

**Recommendation: (a).** A read refreshes the entry, so an active conversation
stays warm; 1h doubles the write cost for the case where the user walks away.

### F5. Is the layout switchable?

- **(a)** No — one layout, changed for everyone.
- **(b)** A setting.

**Recommendation: (a).** A setting here means two prompt layouts to reason about
forever, and the honest default is unknowable per user.

### F6. Anthropic's minimums

The threshold is per model and non-monotonic. Do we

- **(a)** always send the breakpoint and accept a silent no-op below the
  minimum, or
- **(b)** carry a per-model table?

**Recommendation: (a).** A stale table is worse than none, the no-op costs
nothing, and F3's reporting shows whether it engaged.

## 6. What remains available, if this is revisited

Deferring F1 does not block everything: two of the three stages never touch the
prompt's **content**, so the objection above does not reach them. Recorded here
rather than proposed — nothing is being built now.

1. **Anthropic breakpoints** (F2b) — the largest remaining gain, and it is
   layout-independent. Anthropic caches **nothing** today; with a breakpoint
   after the stable head, §3.3 measured the head being read back (5067 tokens)
   *even on the turn the volatile block changed*. The conversation is still
   rewritten each such turn, so this captures part of the win, not all of it.
   Cost: `system` becomes an array of text blocks in `anthropic/wire.rs`.
2. **Reporting** (F3) — all four providers report reuse; the status-bar counter
   already exists. This is also the instrument: without it, a prefix broken by
   some future change is invisible.
3. **`prompt_cache_key`** (F2c) — one field on OpenAI, keyed by chat id.

Each is independently verifiable with the probes in §3, which are cheap and
repeatable.

## 7. Open questions

- Gemini was **not** measured (docs only). Its implicit path should behave like
  OpenAI's; worth one probe before relying on it.
- Whether a compaction roll should try to keep the summary *after* the
  attachments so a roll invalidates less (§4, ordering) — cheap, untested.
- OpenAI docs contradict themselves on breakpoint lookback (50 vs 80); irrelevant
  unless we adopt F2(d).
- OpenAI now ships a native conversation-compaction endpoint. Out of scope, but
  worth knowing before investing further in our own summariser.

## 8. Sources

Anthropic: [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching),
[messages](https://platform.claude.com/docs/en/api/messages),
[streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
[thinking](https://platform.claude.com/docs/en/build-with-claude/thinking).
OpenAI: [prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching),
[responses reference](https://developers.openai.com/api/reference/resources/responses),
[pricing](https://developers.openai.com/api/docs/pricing).
Gemini: [context caching](https://ai.google.dev/gemini-api/docs/caching),
[caching API](https://ai.google.dev/api/caching),
[pricing](https://ai.google.dev/gemini-api/docs/pricing).
llama.cpp: [server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md),
PRs [#15827](https://github.com/ggml-org/llama.cpp/pull/15827) (`cache_n`),
[#19361](https://github.com/ggml-org/llama.cpp/pull/19361) (`cached_tokens`),
[#16391](https://github.com/ggml-org/llama.cpp/pull/16391) (`--cache-ram`).
