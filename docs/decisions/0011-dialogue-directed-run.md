# ADR 0011 — The directed dialogue as a scripted multi-context run on the subagent's record

**Status:** accepted (2026-08-29). Design, the confirmed forks and the
stage-0 probe's measurements —
[docs/research/two-agent-dialogue.md](../research/two-agent-dialogue.md).
Implemented in `feat/dialogue-run`:
[src/app/orchestrator/generation.rs](../../src/app/orchestrator/generation.rs)
(`TurnLoop::run_dialogue`, `DialogueState`, `conversation_brief`),
[src/features/tools/dialogue.rs](../../src/features/tools/dialogue.rs) (the
schema-only tool, the derivation, the verdict vocabulary),
[src/entities/subagent.rs](../../src/entities/subagent.rs)
(`RunKind::Dialogue`, `Participant`). Related:
[ADR 0010](0010-subagent-nested-turn.md) (the record shape and the
loop-executed pattern this extends), [ADR 0006](0006-data-schema-versioning.md)
(the `CHAT_SCHEMA` 3 bump).

## Context

The subagent track left the two-agent dialogue as its named next feature
(research §3.14): two personas with caller-written system messages talking to
each other — each seeing the other as the user, the shape a chat model plays
a role best in — with the transcript viewable as an ordinary child chat. The
user's requirements sharpened it (2026-08-28): the **main agent directs** —
it carries the chat's persona and knows the conversation with the user, and
it decides when the dialogue is over, with the power to steer and edit lines;
the same model writes everything; and the feature must cost **no extra VRAM**
— one engine session, three conversations taking turns.

A stage-0 probe (spike `dialogue-probe`, research §5.1–§5.2) measured the
shape live before any product code: GO on both gate models and both
strict-alternation clouds, 104/105 checkpoint verdicts parsed, zero role
bleed, cache slots held — and two rules discovered: the all-thinking empty
turn (recovered 28/28 by a muted re-ask) and the note→retry→rewrite
escalation ladder emerging unprompted.

## Decision

1. **The record is the subagent's.** A dialogue lands as `SubagentRun` with
   `kind: Dialogue` and an additive `participants: Vec<Participant>`, on the
   call's `ToolCallRecord` — one call, one transcript. Every transcript
   surface (list nesting, the read-only screen, search, titles, `chat://`,
   deletion with the exchange) keys off "the record has a run" and needed no
   change; what keys off the kind is only the role headers and the composed
   system bubble. The new serde variant is why `CHAT_SCHEMA` moves 2→3 with
   a stamp-only step: an older binary refuses the file politely instead of
   failing to parse it.
2. **The transcript is role-encoded** (fork F2): participant `a` is stored
   `Assistant`, `b` is `User`, director interventions are `System` entries
   drawn as note rows. Requests are **derived** from it fresh per line — the
   role swap, a `user` prologue, adjacent same-role merge — which keeps every
   view strictly alternating (the strictest template's requirement, measured)
   and makes editing trivially correct. `Message.author` was deliberately not
   added; it remains the growth path past two participants.
3. **The executor is a scripted sequential loop, not a nested `TurnLoop`**
   (fork F1). Participants have no tools in v1 (fork F4 — a tool round under
   the role swap is the orphaned-call shape a strict cloud rejects), so a
   participant line is one `stream_round` call; the director acts through
   five verdict tools the loop itself interprets (the control-tool
   precedent), at a fixed cadence, with thinking muted. Deterministic control
   flow, a fixed request budget (`lines + lines/cadence`), no round-budget
   entanglement — and at most **one request in flight ever**, which is the
   VRAM contract.
4. **The director is the main agent directing** (fork F6, the user's
   decision): parent persona + a conversation brief (the rolling summary the
   compaction keeps, threaded through `TurnShared`, plus the request's own
   tail) + a localized appendix carrying the caller's `direction`. The
   self-model injection stays top-turn-only (ADR 0010's rule, unchanged).
5. **The probe's two rules are load-bearing**: an empty participant line is
   re-asked once with `reasoning_budget: 0` (a second empty fails the run
   honestly), and a checkpoint reply with no verdict call counts as
   `continue`. Both generations of a recovery count against `max_messages`.
6. **Budgets**: `max_messages` (argument, default 16) is the cap and the
   retry meter; the per-line cap is the subagent's `subagent_max_tokens`;
   the whole run is bounded by its own `tools.dialogue_run_timeout_secs`
   (1800 s — an order past the subagent's, from the probe's measured
   costs), with the state owned outside the timed future so a timeout lands
   the partial transcript. The five existing `RunOutcome`s cover every
   ending; no new variant was needed.

## Consequences

- A chat that never stages a dialogue is unchanged on the wire and on disk;
  the only new stored shapes are a variant and two additive fields. The
  `CHAT_SCHEMA` 3 stamp re-versions every chat file at startup — the
  designed downgrade cost, paid once.
- The open transcript grows **per line** in this stage: the streamed partial
  carries no speaker side, so token-level streaming into the transcript (and
  a retry/rewrite that edits the live view in place) is the track's stage 2,
  on the `ChildStep` seam `mute_steps` currently closes.
- The feed projection and the list counter moved together (`System` → a
  counted note row), pinned by the existing every-prefix equality test.
- The dialogue is deliberately expensive in time on local models and in
  tokens on clouds (the user's accepted trade); the sequential-requests
  invariant is what keeps it free in VRAM, and the append-only contexts are
  what keeps the local cache warm (measured in the probe).
