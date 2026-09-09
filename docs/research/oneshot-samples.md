# The one-shot requests' samples — impersonation's prompt is the session's largest, and its own twice

> **Status:** implemented (2026-09-09) — every fork at its recommendation
> (the user's decision, 2026-09-09); stage 0 is the measurement in §2.1
> (every request of one session with the engine's timing of its prompt,
> on the GPU stack), stage 1's live run is in §6.1. The item the page-summary-usage track recorded
> ([page-summary-usage.md](page-summary-usage.md) §7, fork F2b's other
> half): the two one-shot requests whose streams still drop their `Usage`
> chunk's timing — the automatic title's and impersonation's — where the
> turn's rounds, the roll, the three silent loops and, since the last
> track, a tool's page summary all offer theirs to the slow-prefill note.
> Measured, impersonation's prompt is the **largest a session makes** —
> the whole conversation, re-processed cold — and the title's is under
> the rule's floor on an ordinary first exchange.

## 1. Why, precisely

The slow-prefill note ([slow-prefill-detection.md](slow-prefill-detection.md)
§3) reads the engine's own timing of a prompt — `prompt_n` net of the
prefix cache over `prompt_ms` — and says once per server session how
long a cancelled stream would hold its slot. The note wants a **cold**
sample of at least 256 tokens; a warm server's turns give none (tens of
tokens on a cached prefix), so the tracks since have added every cold
prompt the app makes: the roll's, the loops' first rounds', a page
summary's. Two requests still drop the chunk: the automatic title and
impersonation, both reading `Usage` for the budget's ratio and nothing
else.

Measured (§2.1), the two are not alike:

- **Impersonation's prompt is the session's largest, and cold every
  first time.** It sends the whole conversation with the roles swapped
  under its own system prompt, so nothing of it is in any slot's cache:
  10 642 tokens processed whole on a conversation the fresh chat's first
  turn had opened with 4349. On the 4090 that is 3.7 s; at the CPU build's
  38 tok/s it is 280 s — every `Ctrl+U` on a long chat pays the whole
  conversation's prefill, which is the very figure the note exists to
  tell. A second impersonation right after processes 4 tokens: the
  swapped prefix stays in a slot.
- **The title's prompt is small.** Its digest is capped at 4000
  characters ([`TITLE_CONTEXT_BUDGET`](../../src/features/rename_chat.rs)),
  about a thousand tokens at most; an ordinary first exchange gave 200 —
  under the floor, no sample. Cold every time (its own system), on the
  chat server always, and it fires once per conversation.

The one is worth the plumbing on its own; the other costs one field on a
result that already lands in the orchestrator. Both offers go where the
roll's and the loops' go: after the request's own landing, to the one
rule, which decides once per server session.

## 2. What is there

### 2.1 Measured (stage 0, the GPU stack)

The LAN stack (Qwen3.6-27B Q4_K_M, four slots over 16 384), the estimate
smoke `prompt_estimate_e2e_live` with a scratch print of every request's
`Usage` chunk — prompt size and the engine's timing — and a second
impersonation sent right after the first:

| request | prompt | processed | ms | tok/s |
|---|---:|---:|---:|---:|
| turn 1 — a fresh chat, 24 schemas | 4349 | 4349 | 1825 | 2383 |
| turn 2 | 4439 | 94 | 267 | under the floor |
| turn 3 — carrying 25 KB of JSON | 14 863 | 10 428 | 3817 | 2732 |
| the automatic title (a 450-character digest) | 200 | 200 | 192 | under the floor |
| the roll | 434 | 434 | 244 | 1779 |
| **impersonation** | 10 642 | **10 642** | 3743 | 2843 |
| impersonation again, at once | 10 642 | 4 | 111 | under the floor |

Every one-shot request is processed whole the first time — its system
prompt differs from the chat's, so the prefix cache misses — and
impersonation's is the largest prompt of the session, above the fresh
chat's first turn with all its schemas. The second impersonation is the
prefix cache's: the swapped conversation sat in a slot from the first.

Arithmetic for the CPU build (38 tok/s, [slow-prefill-detection.md](slow-prefill-detection.md)
§2.1): this session's impersonation 280 s — inside its 600 s limit, the
longest wait the app makes on such a host; a two-turn chat of 1500
characters, about 400 tokens swapped, 10 s of prefill and a 54 s hold at
the external default batch — the note.

### 2.2 The code

- `spawn_impersonation` (`impersonation.rs:258`): the reservation on the
  silent lane when it has a budget — `sessions` is `Some` exactly on the
  **shared** engine (`handle_impersonate`, `:129`), the same server as
  the note's rule; a separate impersonation server (`Managed`/`External`)
  has its own pool, its own batch and its own status (`imp_status`), and
  the task gets `None`. The stream reads `Usage` to record
  `Shape::Impersonation` under that same condition and drops the chunk's
  `prefill`. The task lands on `imp_done_tx: (Uuid, FinishReason)`;
  `handle_imp_done` (`:153`) clears the state and emits
  `ImpersonationFinished`. A cancel (`Esc`) ends the stream before its
  usage; a timeout (`IMPERSONATION_TIMEOUT`, 600 s) aborts it.
- `spawn_title` (`title.rs:230`): always the chat engine
  (`backend_if_ready`), a digest of the conversation — or of a sub-agent
  transcript — under `title_system_message`, reasoning muted; the stream
  reads `Usage` for `Shape::Title` and drops the timing; a displaced
  attempt is made again (`SILENT_YIELDS_MAX`), its usage never arriving.
  It lands as `TitleResult { chat_id, text, origin }`;
  `handle_title_result` (`:176`) applies or reports.
- The offers today: `handle_done` for the turn (`generation.rs:1022`),
  `handle_bg_done` for every silent task and the roll
  (`background.rs:258`); `note_slow_prefill` (`generation.rs:861`) reads
  the chat server's mode for the batch — the managed launch line's, or
  2048 for an external server — and `claim_prefill_note` gives the one
  note per chat-server session.
- The run loop (`mod.rs:322`, `:361`): the two landings are their own
  arms, `title_rx` and `imp_done_rx`.

## 3. Design

### 3.1 Impersonation's sample rides its landing

`spawn_impersonation` keeps the `Usage` chunk's `prefill` **when it has a
budget** — the record's own condition, which is "the shared engine": the
sample is then the chat server's, the server whose batch the rule names
and whose session the claim counts. The task lands `ImpDone { id, reason,
prefill }` on `imp_done_tx`; `handle_imp_done` does what it did — clears
the state, emits `ImpersonationFinished` — and then offers the sample to
`note_slow_prefill`, the roll's shape: the request's own landing first,
the rule's one note per server session deciding. A stream that ended
before its usage (a cancel, a timeout) lands `None`; on a separate
impersonation server the task keeps nothing (§4).

### 3.2 The title's sample rides its result

`TitleResult.prefill`, kept across the title's attempts with
`Prefill::keep_larger` (a displaced attempt's usage never arrives, so in
practice the one that finished), and offered by `handle_title_result`
after its own landing **whatever the text** — an error result's prompt
was processed all the same, and a `Requested` title's error goes to the
list overlay while the note goes where every note goes. The title of a
sub-agent transcript is the same request on the same server: the same
offer.

### 3.3 What changes in the numbers

Nothing on the 4090: every sample above is a second or less at any batch.
On the CPU build, a warm server's day — the chat's prefix cached across
app sessions, every turn under the floor — the first `Ctrl+U` or the
first automatic title on a long enough opening is now a note where the
user would otherwise have waited for the first roll or reflection to
say it; the figure is the same, since one server has one throughput.

## 4. Difficult spots

- **A separate impersonation server is another server.** Its batch is
  its own launch line's (or its own default), its session is its own,
  and the note's text names the *chat* server's field and launch line —
  a sample from it would tell the user to change the wrong server. The
  task keeps the sample only under the condition it records under; a
  rule of the impersonation server's own is the own-engine track's
  question (§7).
- **The second impersonation is warm.** 4 tokens processed, under the
  floor: offered and ignored, like a warm turn's.
- **One note per session, and a turn claims it first.** On a fresh data
  root a chat's first turn processes its schemas cold (4349 tokens
  above) and the landing notes before any one-shot request could; the
  live smokes therefore seed a chat on disk and send the request with no
  turn before it (F4), the roll smoke's shape.
- **The title's attempts.** A displaced attempt returns `cancelled` with
  no usage; `keep_larger` over the attempts keeps whatever arrived, and a
  title that gave up (three displacements) lands nothing — as it lands no
  text.
- **The estimate smoke's conversation on the CPU build** would take
  280 s to impersonate: the CPU arm of the live run uses a small seeded
  chat, above the floor and inside a minute.

## 5. Forks

- **F1. Impersonation's sample and the engine.** (a) **The shared engine
  only — the sample kept under the record's condition, `sessions`**
  *(recommended — the one server the rule knows; nothing to decide at
  the landing)*. (b) Both engines, at the chat server's batch — wrong
  figures and the wrong field named. (c) A rule of the impersonation
  server's own — its batch, its claim — with the own-engine track (§7).
- **F2. The title's sample.** (a) **Offered, one field on `TitleResult`**
  *(recommended — cold every time, on the chat server, above the floor on
  a long opening; the cost is the field)*. (b) Skipped — under the floor
  on an ordinary opening, as measured.
- **F3. Where "shared" is decided.** (a) **In the task, by `sessions`**
  *(recommended — the condition the record already uses, one place)*.
  (b) At the landing, by the config's impersonation mode — a mode
  changed mid-stream would misfile one sample; a second reading of one
  fact.
- **F4. The live run.** (a) **Two seeded smokes** *(recommended — a
  chat seeded on disk, no turn before the request so the session's one
  note is not claimed by the turn's cold schemas; `Impersonate` in one,
  `AutoRenameChat` in the other; the note expected on the CPU build under
  `MINDFORK_EXPECT_SLOW_PREFILL=1`, none on the GPU — the roll smoke's
  shape)*. (b) The estimate smoke extended — it runs on the GPU stack,
  where no note comes, so the samples would be a log line and no
  assertion.

## 6. Tests and the live run

- `tests/impersonation.rs`: a scripted stream carrying a slow usage on a
  shared engine — `ImpersonationFinished` lands first, the note after it;
  the same stream with the impersonation mode set to a separate server
  (`sessions` none) lands no sample and no note; a stream cut before its
  usage lands `None`.
- `tests/title.rs`: `handle_title_result` with a slow sample — the
  rename first, then the note; an error result still offers; the second
  result on the same server says nothing.
- `tests/live.rs`: `impersonation_prefill_e2e_live` and
  `title_prefill_e2e_live` (F4a) — a seeded chat of a few long
  exchanges, the request, the note within a few seconds of the landing
  on the CPU build (`MINDFORK_EXPECT_SLOW_PREFILL=1`), no note on the
  GPU stack.

### 6.1 Runs

`impersonation_prefill_e2e_live` and `title_prefill_e2e_live` (F4a): a chat
seeded on disk — an assistant opener and four long exchanges — the request
with no turn before it, the note read within 3 s of the landing:

| host | request | time | the engine's timing | note |
|---|---|---:|---|---|
| the LAN stack (Qwen3.6-27B, 4090) | impersonation | 8.2 s | a second or less | none |
| the LAN stack | the title | 1.2 s | a second or less | none |
| the CPU build (gemma-3-4b, `-ngl 0`) | impersonation | 66.7 s | 1246 tokens in 34.3 s — 36 tok/s | **36 tok/s, a 56 s hold at the default batch** |
| the CPU build | the title | 32.2 s | 1119 tokens in 31.7 s — 35 tok/s | **35 tok/s, a 58 s hold** |

On the CPU build each request's landing is the note — from `ImpersonationFinished`
and from `ChatRenamed` alike, one per server session — with the figures the
server's own log shows for the prompt; the 4090 says nothing, as the rule
wants. Found on the way: the CPU arm's first run answered `400` before any
prefill — Gemma's template refuses the swapped conversation of a chat that
opens with the user's message (§7); the seed gained an assistant opener.
Unit: 2982 green, 153 ignored (2978 / 151 before: the two landings' offers,
the shared-engine condition and a cut stream, the two live smokes).

## 7. Not in this track

- **Impersonation on its own engine** — a rule of that server's own
  (its batch, its one note), with the rest of the own-engine question
  ([silent-tasks-budget.md](silent-tasks-budget.md) §8).
- **Impersonation's limit against a slow host** — 600 s against a
  280 s prefill on the CPU build for a JSON-carrying conversation; the
  loop's and the summary's limits' sibling.
- **A child run's sample**, **the first request of a kind**
  ([page-summary-usage.md](page-summary-usage.md) §7).
- **Impersonation on Gemma's template** — found by the CPU arm of the
  live run: the swapped conversation of a chat that opens with the
  user's message begins with an assistant turn, and the Gemma 3
  template refuses it (`Conversation roles must alternate`, a `400`
  before any prefill). Done: [gemma-impersonation.md](gemma-impersonation.md)
  — the opening folded into the persona, the list alternating.

## 8. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the samples the rule reads: the title's and impersonation's
  on the shared engine), §11.2 (the automatic title's sample), §11.8
  (impersonation's).
- architecture §6 (`TitleResult.prefill`, `ImpDone`; the offers).
- [slow-prefill-detection.md](slow-prefill-detection.md) §7,
  [page-summary-usage.md](page-summary-usage.md) §7 (the item done here).
- CHANGELOG (the Added entry of the note, amended), journal `engine.md`,
  CLAUDE.md's status line and count.
