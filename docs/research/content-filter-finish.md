# Research: a reply stopped by the provider's content filter

**Status:** forks decided by the user on 2026-09-16 — all three at the
recommendation; implemented (§5).

**Related:** spec §6.3 (the agentic loop), §6.4 (the interruption notes and
`/continue`), [openrouter-external.md](openrouter-external.md) §4.2 (where this was
first written down, as "pre-existing and not specific to OpenRouter"),
[continue-generation.md](continue-generation.md) (`MessageFinish`),
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (the engine
contract), [docs/journal/engine.md](../journal/engine.md).

## 1. What is broken, precisely **[code]**

Every provider has a way to say "my moderation stopped this reply", and no client
of ours passed it on. The domain `FinishReason` had no value for it, so each client
fell back to whatever its wildcard arm said, and the four got it wrong four
different ways:

| wire | what the provider sends **[docs]** | what the client made of it |
|---|---|---|
| OpenAI-compatible — `managed`, `external` (OpenRouter), xAI | `finish_reason: "content_filter"` — OpenAI's own value, and one of the five OpenRouter normalises every route's reason into | `Stop` — the fragment reads as a complete answer |
| OpenAI Responses | `response.incomplete` with `incomplete_details.reason: "content_filter"` | `Length` — the note says the reply hit the length limit and offers `/continue`, which would meet the same filter; **worse than silence** |
| Anthropic | `stop_reason: "refusal"` (the streaming classifiers) | `Stop` — silent |
| Gemini | `finishReason` `SAFETY`, `RECITATION`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY`; `promptFeedback.blockReason` for a refused prompt | `Stop`, with an English sentence yielded **as reply text** — stored in the chat, sent back to the model next turn as its own words, never localised |

`[docs]` — these are the providers' published shapes and are unmeasured here. They
cannot be measured on purpose without prompts written to be refused, which this
project does not send (§4).

## 2. Forks

### C1. Scope — **(a), decided**

- **(a) all four wires in one PR.** One domain value, each client mapping its own
  spelling into it; the Responses case, which actively misleads, closes at once.
- (b) the OpenAI-compatible wire only, the rest as a follow-up.

### C2. What the chat file records — **(a), decided**

- **(a) `MessageFinish::Stop`.** No data format change. `/continue` refuses a
  `stop` reply, which is right here: resuming would meet the same filter. The note
  is shown when the reply ends; the file does not say why it ended.
- (b) a new `MessageFinish::Filtered`. An honest record, but an older build fails
  to parse a chat that carries it — a `CHAT_SCHEMA` bump, a migration step, a golden
  fixture and a Data entry (release-engineering.md F12) for a record nothing reads.

### C3. Gemini's in-text note — **(a), decided**

- **(a) the six filter reasons and a refused prompt move to the shared, localised
  note**, outside the reply text. `MALFORMED_FUNCTION_CALL` and `OTHER` are not
  filters and keep the path they have.
- (b) leave Gemini as it is.

## 3. Decided, with the reason

- **`FinishReason::Filtered`** in the engine contract. A client-side flag or a
  text marker would put the same fact on a second channel beside the one every
  consumer already reads.
- **Tool calls win over the filter**, as they already did for Gemini's note: a
  round that produced calls ran them, and a filter reason that arrives after them
  changes nothing about what the model asked for.
- **Not continuable, not retried.** The retry decorator retries only failures
  before content, and a filter stop is not a failure; `continuable` lists
  `Cancelled | Error | Length` and stays that way.
- **Where the fact is shown:**
  - **the chat turn** — a note in the feed;
  - **impersonation** — the same note; the partial draft still lands in the input
    box, as it does after `Length`;
  - **a subagent** — its outcome stays `Completed`, since `RunOutcome` is stored
    and C2 applies to it too, but the status line the parent model reads names
    the filter. Gemini's in-text note used to be the only way a parent learned
    this, and moving it (C3) must not take that away;
  - **the compaction roll** — a log line, as a truncated roll gets;
  - **the title and the dialogue** — nothing new: the title uses what arrived, and
    a dialogue line that came back empty already fails the run.

## 4. What a live run can show

The positive case needs a prompt the provider refuses, and writing one to order is
not something to put in a test suite. So the gate is:

- **unit tests** on each wire's documented shape — the stream a client parses, not
  just its mapping function;
- **the regression half live**: an ordinary reply still ends as `Stop`, through
  each client this changes.

## 5. What was implemented

- **`FinishReason::Filtered`** ([contract.rs](../../src/shared/api/contract.rs)),
  and each wire's spelling mapped into it: `from_wire("content_filter")`;
  `RespIncomplete::finish_reason` over the `incomplete_details` the Responses wire
  now keeps ([responses/wire.rs](../../src/shared/api/openai/responses/wire.rs));
  Anthropic's `refusal`; Gemini's `is_filter_reason`, split from `is_block_reason`,
  which keeps `MALFORMED_FUNCTION_CALL` and `OTHER` and their in-text note, while a
  refused prompt now ends as `Filtered` with no text of ours
  ([gemini/client.rs](../../src/shared/api/gemini/client.rs)).
- **The consumers** (§3): `finalize_message` stores `Stop`; `continuable` is
  unchanged, and the new value is simply not in it; the feed's note
  `ui.err.reply_filtered`, impersonation's `ui.err.impersonation_filtered`, the
  subagent's `tool.call_subagent.result.filtered` (en, ru); the compaction roll's
  log line.
- **One SSE stub for the clients' stream tests** —
  [sse_stub.rs](../../src/shared/api/sse_stub.rs), which is Anthropic's private
  helper moved rather than copied into Responses and Gemini, both of which had none.

**Tests** — 3263 green, 184 ignored (3251 / 184 before). A stream test per wire,
each through the real client and the real SSE framing: an OpenRouter-shaped
`content_filter` chunk with its `native_finish_reason`; a Responses `incomplete`
for the filter, and for `max_output_tokens` and no reason at all, both still
`Length`; an Anthropic `refusal`; Gemini's `SAFETY` finish, a refused prompt, and
the malformed call that keeps its note. The reason tables; the feed's note and
impersonation's; a whole turn on a mock engine (announced not continuable, stored
as `stop`, the fragment kept); a filtered subagent whose parent is told. **Eleven
mutations, all caught**, one per decision above, each by a named failing test.

**Live — the regression half** (§4), 2026-09-16: `simple_generation` through all
four changed clients, each asserting a non-empty reply ending in `Stop` or
`Length` — OpenRouter on `google/gemma-4-31b-it` (the OpenAI-compatible client),
OpenAI Responses, Anthropic and Gemini on their smoke defaults. **GO**, no skip.
