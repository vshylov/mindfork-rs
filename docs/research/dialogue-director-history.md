# The dialogue's director on Gemma's template — the checkpoint's history must alternate too

> **Status:** implemented (2026-09-09) — every fork at its recommendation
> (the user's decision, 2026-09-09); stage 0 is the measurement in §2.1
> (a participant's view and the director's first and second checkpoints
> against Gemma 4 31B on the LAN stack and Gemma 3 4B on the CPU build,
> then two repaired shapes of the second checkpoint on both), stage 1's
> live run is in §6.1. The
> question the gemma-impersonation track left
> ([gemma-impersonation.md](gemma-impersonation.md) §7): a scene's first
> line — does it go out as a system prompt with no turns, and does
> Gemma 3 drop the persona with it? Measured, it does not: the
> participants' views were built for the strict template from the start.
> What Gemma 3 refuses is the **director's** conversation from its second
> checkpoint on — its verdicts kept as tool-call turns with a `tool`
> result, which that template renders as a second user turn in a row.

## 1. Why, precisely

A directed dialogue (spec §9.13, [two-agent-dialogue.md](two-agent-dialogue.md))
derives three views from one transcript. The two participants' views
were designed for a strict template: a `user`-side prologue — the
`scene`, or a short localized marker — where the first derived turn would
be the speaker's own, then adjacent same-role lines merged, so "every
view is `system, user, assistant, user, …` — the strictest template
(Gemma's) is satisfied by construction" (§3.2 there). The opening line
comes from the call's arguments, so no view is ever a system prompt with
no turns. The hypothesis of the last track is answered: measured on
Gemma 3, a participant's view is accepted, the persona inside its first
user turn.

The director's view is the one that was not built for it. Its
conversation is **persistent and append-only** across checkpoints —
the script's increments as `user` turns, its verdicts as its own
tool-call turns with a short `tool` result ("noted") — for two reasons
the design names: memory of its own notes, and a cache-friendly prefix.
The dialogue track's Gemma arm passed that shape 53 checkpoints out of
53 — on **Gemma 4**, whose template takes any shape. Gemma 3's template
has no tool role: `llama-server` renders the `tool` result as a user
turn, the next script increment is another, and the template raises on
the pair — `Conversation roles must alternate`, a `400` before any
prefill — so the director fails at its **second** checkpoint
(`DialogueEnd::Failed { who: director }`), which with the default cadence
(a checkpoint after every exchange) and the default cap (16 lines) is
every scene longer than one exchange. The clouds never saw the pair:
their wires merge adjacent same-role messages — the Anthropic wire folds
the `tool_result` block and the next script into one user message
(`anthropic/wire.rs:248`) — which is exactly what a Jinja template
without a tool role cannot do for us.

The fix is the shape of the director's memory, not its content: the
verdict it gave is its own turn either way. As **text** — the calls
rendered as `name(arguments)` lines, no `tool` result — the conversation
alternates on every template, keeps the notes it gave, and keeps its
prefix. Measured on both Gemma templates, that shape and a stateless
full-script checkpoint are both accepted and both answer with the same
verdict; the text turn is the one that keeps what the design wanted
persistence for.

## 2. What is there

### 2.1 Measured (stage 0)

Through `/v1/chat/completions` directly, reasoning off, the three verdict
tools (`dialogue_continue`, `dialogue_stop`, `dialogue_note`) as the app
sends them, on a café scene:

| request | Gemma 4 31B (the LAN stack) | Gemma 3 4B (the CPU build) |
|---|---|---|
| a participant's view: `[u(begins), a(opening), u(B1)]` | a line, 65 tokens of prompt | a line, 57 tokens |
| the director's first checkpoint: `[u(script)]` + tools | `dialogue_continue`, 247 tokens | the text `dialogue_note`, no call, 91 tokens |
| the director's second checkpoint: `[u(script), assistant(tool_call), tool("Noted."), u(more)]` + tools | `dialogue_continue`, 342 tokens | **400**, roles must alternate |

The two repaired shapes of the second checkpoint:

| request | Gemma 4 | Gemma 3 |
|---|---|---|
| (b) the verdict as the director's own text turn: `[u(script), a("dialogue_continue()"), u(more)]` | `dialogue_continue`, 294 tokens | the text `dialogue_continue()`, 138 tokens |
| (a) stateless — the whole script in one user turn | `dialogue_continue`, 270 tokens | the text `dialogue_continue`, 114 tokens |

Two side facts. Gemma 3 4B answers a checkpoint with the verdict's name
as **text**, never as a call — a capability of the model and of a
template without tool support, not a shape the app sends; the app reads
no verdict as *continue*, so on that model the director never stops a
scene (§7). And Gemma 4's replies are byte-for-byte the same verdict in
all three shapes.

### 2.2 The code

- `dialogue_checkpoint` (`generation.rs:4204`): renders the script's new
  lines (`dialogue_script`, incremental through `DialogueState.rendered`,
  a `script_opening` header on the first ask and `script_more` after),
  pushes it as a `user` turn onto `director_msgs`, streams with the
  verdict schemas, then pushes `assistant_tool_calls(text, calls)` and one
  `tool("noted")` per call (`:4246`, `:4253`); the verdicts are applied
  (`Stop`, `Note`, `Retry`, `Rewrite`; none means continue).
- `DialogueState.director_msgs` (`:3628`): "the director's persistent
  conversation: script increments as `user` turns, its verdicts as its
  own tool-call turns (research §3.2) — which keeps its context
  append-only, the cache-friendly shape §5.1 measured".
- `participant_view` (`tools/dialogue.rs:179`): the prologue and the
  merge — the repair this track does not need to make.
- `dialogue_runs_alternating_and_lands_on_the_record` (`tests/dialogue.rs`)
  pins the second checkpoint at four messages: the first ask, the verdict
  turn, the tool acknowledgement, the new lines.
- spec §9.13: "its conversation is persistent across checkpoints: script
  increments as `user` turns, its verdicts as its own tool-call turns";
  [two-agent-dialogue.md](two-agent-dialogue.md) §3.2 the same, with the
  reasons; §5.1 the Gemma 4 arm's 53/53.
- The Anthropic wire (`anthropic/wire.rs:236–262`): a `Tool` message is
  a `user` turn with a `tool_result` block, and adjacent same-role
  messages are merged — the clouds' reason for never seeing the pair.

## 3. Design

### 3.1 The director's verdicts as its own text turns

`dialogue_checkpoint` keeps the conversation persistent and append-only,
and changes what it appends after a verdict: one `assistant` turn whose
text is the model's own text, if any, followed by each call rendered as
`name(arguments)` on its own line — and no `tool` messages. The next
checkpoint's script increment follows as a `user` turn, so the
conversation is `user, assistant, user, assistant, …` on every template:
Gemma 3 accepts it (§2.1), Gemma 4 gives the same verdict, and every
provider takes an assistant turn of text. The director's memory of its
notes is intact — the note's text is in the turn — and the prefix the
cache holds is the same prefix, one turn shorter per checkpoint.

### 3.2 Why not the stateless checkpoint

The whole script in one user turn is accepted the same way (§2.1) and
costs about the same tokens — the history re-sends everything anyway.
It loses the two things the design kept the history for: the director's
own notes ("I already told A to wrap up") would have to be re-listed in
the ask, and the prefix would change on every checkpoint. The text turn
keeps both and touches one function.

### 3.3 What changes in the numbers

On Gemma 3, a scene's second checkpoint: a verdict instead of a `400`
and a failed run. On Gemma 4, Qwen and the clouds: the same verdicts,
one `tool` turn fewer per checkpoint in the history. The go bar's
"verdict compliance" is unchanged where a model calls tools; where it
does not (Gemma 3 4B), the app's reading of no call as *continue* is
what it was.

## 4. Difficult spots

- **The rendering.** `name(arguments)` with the arguments as the call's
  JSON string, one line per call, after the model's text when there was
  one — the same fact the tool-call turn carried, readable to the model
  as its own words. A checkpoint that produced no call (a model that
  answers in text) stores the text alone, as `assistant_tool_calls(text,
  [])` already did.
- **Anthropic's pairing rule** — a `tool_use` block must be answered by a
  `tool_result` — is a rule about `tool_use` blocks, which the history no
  longer carries; the director's requests still send the verdict *schemas*
  and read the calls off the reply as before.
- **The `script_more` header** keys on `director_msgs.is_empty()` —
  unchanged, since the ask is still pushed first.
- **The pinned shape.** The unit test that counts four messages at the
  second checkpoint counts three and asserts no `Tool` role; the
  design's §3.2 and spec §9.13 say "tool-call turns" and are amended.
- **Gemma 3's first checkpoint at 91 tokens** for a 50-token system and
  a 60-token script with three schemas: whether the schemas reached the
  model is not this track's question (§7); the model named a verdict,
  so something did.

## 5. Forks

- **F1. The director's history.** (a) **The verdicts as the director's
  own text turns, no `tool` messages** *(recommended — alternates on
  every template, keeps the notes and the prefix, one function)*. (b)
  Stateless checkpoints — the whole script each time; accepted the same
  way, but the notes must be re-listed and the prefix changes every
  time. (c) Keep the tool exchange and resend as (a) on a `400` — a
  second path behind error-string matching. (d) Merge a `tool` message
  into the following user turn at the OpenAI-compatible wire, as the
  Anthropic wire does — wrong for every template that *has* a tool role
  (Qwen's, Gemma 4's), which would then lose the result's role.
- **F2. The verdict's rendering.** (a) **`name(arguments)` per call**
  *(recommended — the call as the model would write it)*. (b) The calls'
  raw JSON — longer, and JSON in an assistant turn invites JSON back.
- **F3. A reply with no call.** (a) **Read as *continue*, as today**
  *(recommended — a capability limit of the model, not a shape; the
  scene ends at its cap)*. (b) Parse a verdict's name out of the text —
  its own question (§7).
- **F4. The live run.** (a) **`dialogue_e2e_live` on Gemma 4** — the
  default cadence and cap put a second checkpoint in every scene — and,
  for Gemma 3, the direct measurement of §2.1's repaired shape, since a
  4B model does not call `run_dialogue` reliably enough to stage a scene
  from a turn *(recommended)*. (b) A Gemma 3 smoke through the turn —
  the model must choose to call the tool first.

## 6. Tests and the live run

- `tests/dialogue.rs`: the second checkpoint's messages are the first
  ask, the director's text turn — the verdict's name in it, a note's text
  when one was given — and the new lines: three, and no `Tool` role
  anywhere in the director's history; the `script_more` header on the
  second ask as before; a checkpoint whose reply carried no call stores
  the text alone.
- `features/tools/dialogue.rs`: `verdict_turn(text, calls) -> String`,
  the rendering, unit-tested on its own — a call with arguments, two
  calls, text plus a call, text alone.
- Live (F4a): `dialogue_e2e_live` on Gemma 4 (the LAN stack) — a scene
  with at least two checkpoints, the director's stop; Gemma 3's
  acceptance of the shape is §2.1's measurement.

### 6.1 Runs

`dialogue_e2e_live` and `background_dialogue_e2e_live` on Gemma 4 31B (the
LAN stack), the café scene, the default cadence and cap (F4a): both scenes
**Completed** — stopped by the director — at 5 spoken lines (1296 and 1256
tokens), so each passed its second checkpoint with the director's verdict
as its own text turn in the history and its stop at the third; the parent
turn's reply summarized the resolved mix-up. Gemma 3's acceptance of that
shape is §2.1's measurement (a 4B model does not stage a scene from a turn
reliably enough for a smoke). Unit: 2987 green, 153 ignored (2986 / 153
before: `verdict_turn`'s rendering; the second checkpoint's history pinned
at three turns and no `Tool` role).

## 7. Not in this track

- **A verdict answered in text** — Gemma 3 4B names the verdict without
  a call; reading a bare `dialogue_stop` out of text is a parser with
  its own failure modes.
- **The first checkpoint's prompt size on Gemma 3** — 91 tokens with
  three schemas: what `llama-server` injects for a template without tool
  support is its question, not the app's.
- **The opening-only impersonation on Gemma 3** and the rest of
  [gemma-impersonation.md](gemma-impersonation.md) §7.

## 8. Documentation touch list (AGENTS.md §4)

- spec §9.13 (the director's conversation: verdicts as its own text
  turns; why).
- [two-agent-dialogue.md](two-agent-dialogue.md) §3.2 (the same, a note
  on the Gemma 3 template), [gemma-impersonation.md](gemma-impersonation.md)
  §7 (the item done here).
- CHANGELOG (Fixed: a directed dialogue on Gemma 3 failed at its second
  checkpoint), the journal file the dialogue track wrote to (grep at
  stage 1), CLAUDE.md's status line and count.
