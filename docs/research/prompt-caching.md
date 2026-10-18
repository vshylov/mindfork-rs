# Prompt caching — research and decision points

> Status: **F1 re-opened and decided** (2026-10-18). The 2026-08-08 deferral
> rested on §2.1 — that the price of the layout was mostly not being paid. It is
> paid on most turns, and has been since that evening: §2.2 names the block,
> measures how often and what it costs, and
> [§5 F1 revisited](#f1-revisited-2026-10-18) lays
> the options out again. User's decision: (e1), picking observations by the chat's
> opening, behind an MVP probe, with (e2) on no-go. Roadmap: §Context and tokens.
> Adjacent: [history-compression.md](history-compression.md) §2.3, spec
> [§6.6](../../spec.md) (KV cache and prefix caching),
> [ADR 0004](../decisions/0004-engine-contract-multi-provider.md),
> [cached-tokens.md](cached-tokens.md) (the instrument that caught it).
> Measurements taken 2026-08-08 and 2026-10-18 against live providers and a live
> `llama-server`.

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

**Superseded the same evening.** That adjacent defect was fixed within the hour
(per-section budgets, below), and the fix removed the truncation this section's
"stable by accident" rested on. §2.2 is what the prompt does since.

### 2.2 2026-10-18: the masking is gone — most turns miss

**Seen first by the new counter.** While the 0.19.0 release draft was checked on a
copy of the dev data (`openai` mode, `gpt-6.1-sol`, interface `ru`), the cache
share in the token counter ([cached-tokens.md](cached-tokens.md)) showed a new
chat's first turn at prompt 9217, cached 0, written 9214 (cold, as expected), and
its second turn two minutes later at prompt 9245, **cached 0, written 9242**. A
fresh run asking the same first question read 9214 (99%) from the cache. So the
first turn is deterministic across runs, and something early in the prompt changes
between turn 1 and turn 2 of one chat.

**The block, named.** The released binary was run on a copy of the dev data in
`external` mode behind a logging proxy to a local `llama-server` (Gemma 4 26B-A4B,
with the dev profile's own embedder, bge-m3), driven through two conversations with
`tools/console_probe.py`'s `Session`, and the request bodies of consecutive turns
were diffed. On every miss, `system` diverges at **character 8685** of about 10.7k
(token 2425 of about 3000, in Gemma's tokenizer) — at the first item under "Recent
observations"
(`selfmodel.render.observations_recent`). That is the observations section of the
self-model block, which `SelfModel::render_for_prompt` renders from
`injection_recent`'s pick for the **latest user message**
(`app/orchestrator/generation.rs`). Everything before it is byte-identical: the
persona, the block's header, the summary and the interlocutor model. The 42 tool
schemas are identical too, and so is every earlier message. The other candidates
are ruled out by the same diff. There is no attachment in these chats and no
other recall in `system`. No time-dependent text changed either: the age labels
are day buckets.

**Why it renders now.** Two things changed since §2.1, and the first is decisive.

- **The per-section budgets** ([self-model-injection-budget.md](../history/self-model-injection-budget.md),
  commit `15a903d1`, 2026-08-08 20:21). They landed under an hour after F1 was
  deferred, and §2.1's own measurement had found the defect they fix. They give the
  observations a floor of 20% of `prompt_cap` (`SHARE_OBSERVATIONS`), plus whatever
  the earlier sections leave. When no item fits whole, `fit_parts` cuts the first
  item rather than dropping the section. So **the top pick always renders**. That
  holds on every profile with the injection on (`get_self_model` enabled) and at
  least one observation, not only on this one.
- **On the dev profile, the room grew as well.** The summary went from 1349 to 733
  characters, no goal is active, and `prompt_cap` is 4000. The section rendered at
  479–964 characters, which holds one observation, twice two. Their median length
  is 482 characters, and a tenth of them run past 940.

**The pick is mostly noise.** `self_notes_relevant` has no similarity floor. It
takes the 20 notes nearest the latest user message, keeps the `@self` ones and
returns the first three, and the section shows what fits, from the first. The
profile has 106 observations, nearly all about the relationship and the
self-model. A question about the KV cache is "near" whichever of them happens to
score highest, and that one changes with the wording. Across six follow-ups on one topic, the top pick
changed four times, among four unrelated observations.

**How often.** Two conversations in one new chat each, on the released binary:

| conversation | follow-up turns | prefix kept | missed |
|---|---:|---:|---:|
| four unrelated questions | 3 | 0 | **3** |
| six questions on one topic (the KV cache) | 5 | 2¹ | **3** |

¹ One of the two was kept only because the pick flipped back to turn 1's
observation, while turn 1's prompt was still in a server slot. After turn 5 a
background reflection ran (`auto_reflect_every` = 5); it read the self-model and
wrote nothing, so turn 6's change is again the observation alone.

**What a miss costs.** The captured bodies were replayed with each pair on a freshly
started server, so no slot or RAM cache carried over. The second request went once
as sent, and once with the first request's `system` — the "stable block"
counterfactual.

| stack | turn | as sent: reused | prefill | stable block: reused | prefill |
|---|---|---:|---:|---:|---:|
| llama.cpp, Gemma 4 26B-A4B Q4_1, RTX 4090 | T2 | 0 of 11283 | 1760 ms | 10956 of 11261 (97%) | 236 ms |
| | T3 | 0 of 11279 | 1765 ms | 11283 of 11377 (99%) | 225 ms |
| | T4 | 0 of 11615 | 1776 ms | 11279 of 11638 (97%) | 245 ms |
| llama.cpp, Qwen 3.8 27B Q4_K_M, RTX 4090 | T3 | 0 of 12244 | 4881 ms | 12246 of 12351 (99%) | 344 ms |

The same bodies went to OpenAI `gpt-6.1-sol` in the app's Responses shape
(`instructions`, flat tools, `store: false`). Their history holds Gemma's replies;
what is under test is the layout, which is the app's. One nonce per run heads the
instructions, so no earlier run can answer (§3.2's methodology note).

| request | input | cached | written |
|---|---:|---:|---:|
| T1 (cold) | 9347 | 0 | 9344 |
| T2 as sent | 9690 | **0** | **9687** |
| T2 with T1's `system` | 9658 | **9344** | 311 |
| T3 as sent | 9677 | 0 | 9674 |

That is the release check's observation, reproduced, next to what a stable block
would have read. At 0.1× for a read and 1.25× for a write (§4), the missed turn
bills about 12.1k input-token equivalents and the kept one about 1.3k. **A miss
costs about 9× the prompt of a hit**, on every such turn. On llama.cpp it costs 7×
(Gemma) to 14× (Qwen) the prefill time. Both ratios hold as the conversation
grows, because the miss re-pays the whole of it.

**The shared head is worth nothing.** 2425 tokens of `system` precede the
divergence, and no stack reused any of them.

- **llama.cpp** found them (`selected slot by LCP similarity, f_sim_best = 0.215`)
  and still re-processed the whole prompt. Both models carry layers whose state
  cannot be rewound to an arbitrary position: Gemma 4's sliding-window attention
  and Qwen 3.8's recurrent layers. So the server resumes from a saved checkpoint,
  not from the point of divergence. The 0 is measured; the mechanism is
  llama.cpp's.
- **OpenAI** reused none of the 2425-token head of `instructions` either. §3.2's
  layout B, by contrast, was reused up to the message its block sat in. That fits
  reuse resuming at message boundaries, and a change inside the one `instructions`
  string leaves none before it.

So **on these stacks, a change anywhere in `system` costs the whole prompt**, and
ordering inside `system` buys nothing (F1 (b)).

**Adjacent, for the self-model track.** Of `narrative_in_prompt` = 3 observations,
the section fits one, at most two, on this profile. It is also headed "Recent
observations" while its content is a relevance pick.

**Method, in case it is repeated.** The proxy saved every request body and the
final `usage`/`timings`. The driver pasted each question and waited for the proxy
to go quiet. Replays sent the body with `max_tokens: 1` (the prompt is unchanged).
None of it is committed; each piece is a page of Python around `console_probe`.

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
(No longer true from the same evening: see §2.2 and F1 revisited below.)

If this is ever revisited, the promising direction is **(e)**, not (c): it
removes the volatility instead of relocating it, so the prompt the model reads
is unchanged and the objection above does not apply. What it costs is the
per-turn relevance selection, which §2.1 measured as inert on the real profile
anyway. It would need its own measurement of whether a stabler selection is
worth less to the model than a per-turn one.

### F1 revisited (2026-10-18)

The 2026-08-08 decision rested on §2.1's finding that the price "is mostly not
being paid". §2.2 measured the opposite: it is paid on most turns, and has been
since the evening of that decision. A paid turn costs about 9× the prompt of a kept
one on OpenAI 6.1, and 7–14× the prefill on the local stacks. The objection to (c)
and (d) is unchanged, because it was about the block's status and not its cost;
so are their numbers. What changed is the case for (a), (b) and (e). One option is
also considered here for the first time, as (f).

- **(a) Leave as is.** This now means a full miss on every turn whose top pick
  changes: 6 of 8 measured follow-ups. It applies on every profile with the
  injection on and at least one observation. On OpenAI 5.6+ each miss is a write of
  the whole prompt at 1.25×; on llama.cpp it is a full re-prefill.
- **(b) Reorder inside `system`.** Measured to buy nothing on all three stacks,
  because a change anywhere in `system` costs the whole prompt (§2.2, "the shared
  head"). Drop.
- **(c) The observations into the new user turn.** Numbers as in §3; the objection
  stands.
- **(d) The whole block after the conversation.** As (c), and larger.
- **(e) Stable content, same place.** The block keeps its position and its status,
  so the prompt the model reads is shaped as today, and the objection does not
  apply. The open question was what the per-turn pick is worth. §2.2 answers part
  of it: on the dev profile it is mostly noise, since there is no floor and the top
  observation flips among unrelated ones within one topic. Three ways to make it
  stable:
  - **(e1) Pick by the chat's opening, not its latest message.** The query becomes
    the chat's first user message, and after a compaction, the summary (the roll
    breaks the prefix anyway). That is deterministic from turn to turn with no
    stored state. The pick then changes only when the observations themselves
    change, through a new one written or a reflection, which changes the
    self-model block anyway. One input of `injection_recent` changes. Its weak
    spot is an opening like "Hi!", which picks something arbitrary, but at least
    the same thing every turn.
  - **(e2) Recency only.** The freshest `narrative_in_prompt` observations, with no
    embedding call per turn. This matches the section's own header, "Recent
    observations". The pick changes only when an observation is written. It gives
    up relevance entirely.
  - **(e3) Relevance above a floor.** A pick displaces the recency default only
    above a similarity threshold. Per-turn relevance survives where it is real, but
    the prefix still breaks on every topic shift, and the threshold must be tuned
    per embedder.
- **(f) A system message after the conversation.** This would move the
  observations out of `system` without making them the user's words. It is not
  portable. Measured: the chat template of Qwen 3.8, a gate model, refuses it
  ("System message must be at the beginning"); Gemma 4's accepts it. Anthropic and
  Gemini have no system role inside the conversation at all. Drop.

**Recommendation: (e1)**, built behind an MVP probe with a go/no-go criterion:

- **go**: §2.2's two conversations, re-run, keep the prefix on every follow-up turn
  except those right after a self-model write (here, 8 of 8). And the picks for
  real chat openings read as relevant, judged on a dozen of the owner's own chats.
- **no-go → (e2)**: if picks anchored on the opening are as unrelated as today's,
  relevance buys nothing for its per-turn embedding, and recency is the honest
  selection.

Why not (e2) outright: it removes the Tier-2 behaviour (relevance) where (e1) only
stabilises it, and the probe is cheap. Why not (e3): it keeps the miss on every
topic shift and adds a threshold to tune. None of these changes Anthropic, which
caches nothing until F2 (b). `prompt_cache_key` (F2 (c)) improves routing and does
not rescue a changed prefix.

**User's decision (2026-10-18): (e1), behind the MVP probe above, falling back to
(e2) on no-go.**

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
   some future change is invisible. **Built 2026-10-17**:
   [cached-tokens.md](cached-tokens.md). Its first release check found §2.2.
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
