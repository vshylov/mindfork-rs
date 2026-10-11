# An empty answer after tools — the model's, and asked once more

> Status: **done** (2026-10-11). The fix is in `TurnLoop::run` (spec §6.3); the lead in
> §6 is on the roadmap. Journal: [engine.md](../journal/engine.md), "an empty answer
> after tools is asked once more".

## 1. What the gate saw

Two Qwen 3.6 27B dispatches in a row ended a code smoke on a round that said nothing:

- run 38076541948 — `code_edit_e2e_live`, after a tool error, the turn's last round
  empty (the smoke failed on a wrong edit first, so this went unread);
- run 38101601194 — `code_workspace_navigate_e2e_live`: three calls, then a round with
  no text, no call and no thoughts, `finish_reason: stop`. The user would have seen
  *"First I'll look at the project's structure…"* and nothing behind it.

Neither smoke had failed this way in the four Qwen dispatches before. The endpoints run
the `server-cuda:latest` image, so a llama.cpp change was the first suspect: between the
local stand's b11513 and b11556, `8b5436102 chat : refactor API (#30210)` (b11531)
reworked the chat layer, and its server logs `reasoning budget: … generation_prompt=''`
where b11513 logged `generation_prompt='<|im_start|>assistant\n<think>\n'`.

## 2. Smokes on both builds

The two builds alternated in blocks on one stand (Qwen3.6-27B Q4_K_M, RTX 4090,
`-c 16384 -np 1`), each block `code_workspace_navigate_e2e_live` ×4 and
`code_edit_e2e_live` ×3, three blocks a build:

| Build | Red | Of which an empty round |
|---|---|---|
| b11556 | 2 of 21 | 1 (`navigate`: one token, `'>'`, then end of generation) |
| b11513 | 0 of 21 (0 of 38 with the day's earlier runs) | — |

The other b11556 red was a different shape: *"the length must be cast to `f64`"* and a
stop, no `code_edit` call. Suggestive, and small; a smoke-level count cannot tell a build
from luck at one in twenty.

## 3. The request itself, replayed

A recording pass-through between the smokes and `llama-server` (scratch, not committed:
it saved every chat request's body and a summary of its streamed answer) caught one: the
fourth request of a `navigate` turn — system, the question, three tool rounds
(`code_list`, `code_grep`, `code_read`) — answered with a bare `stop`. That body was then
sent again and again, one at a time, to each build:

| Replay of the captured request | Empty |
|---|---|
| b11556, as sent | **3 of 40**, then 2 of 60 — **5 of 100** |
| b11513, as sent | **2 of 40** |
| b11556, thinking muted (`reasoning_budget: 0` + `enable_thinking: false`) | **0 of 40** |
| b11556, the turn's own thoughts carried in the history (§6) | **0 of 100** |

`/apply-template` renders the request **byte for byte the same** on both builds
(7043 bytes). So the build is acquitted: the same prompt draws an empty answer from the
model at a few percent on either side of the refactor, and the run of reds was the dice.
Every other answer opened on the right thing — 234 of the 273 on "7300" within their first
60 characters, the rest on `src/config.rs`; the empty ones are a clean end of generation
right after the opening `<think>`.

## 4. The fix: once more, muted

A round that follows a tool round and stops with no text and no call is sent again —
the same history, nothing added — with thinking muted, once per turn; the turn's own
`reasoning_budget` is put back for any round after it. A second empty answer stands.

- **Muted, not as sent**: as sent, the same request comes back empty 5 in 100, so the
  re-ask would itself fail now and then; muted, 0 in 40, every answer the right one. The
  turn's thinking already happened in the rounds before; what is missing is the words.
- **Only after a tool round**: that is where it was measured, and where the narration
  ("first I'll look…") makes an empty end read as a promise broken. A first round that
  says nothing is not this failure.
- **Only a clean stop**: a cut (`Length`) is the thinking-to-the-cap failure, which
  another re-ask does not cure (roadmap, the withheld-image notes on Qwen 3.6).
- **Not twice**: the woken-turn re-ask (spec §9.3.2) runs first; a round it already
  asked again is not asked a third time.
- No tool is exempt. Both control tools tell the model to write after them
  (`send_followup_message`: "after it you can write a second reply"; `rewrite`: "the
  next reply will replace it"), so silence after them is the same failure.

## 5. Live

The pass-through, in a mode that answers each turn's first round after its first tool
round with an empty `stop` itself, put a real model behind a guaranteed empty answer:

- the branch: `code_workspace_navigate_e2e_live` 6 of 6 green (12 injected empties),
  every injected empty followed by a muted re-ask with the same history — the re-ask
  answered or called the next tool, and the round after it thought again;
  `code_edit_e2e_live` 2 of 2 green;
- `main`, the control: `code_workspace_navigate_e2e_live` 3 of 3 red, each turn ending
  on its narration.

## 6. The lead: the turn's own thoughts

The rendered prompt shows the model its earlier rounds in this turn with an empty
`<think>\n\n</think>`: the client does not send a round's thoughts back with its
tool calls on the OpenAI-compatible wire (Anthropic's and Responses' reasoning do go
back, as `ThinkingBlock`s). In the captured turn the model had thought 151 and 111
characters in its first two rounds, then nothing in the third, then nothing at all.
Carrying those thoughts back as `reasoning_content` on the assistant messages took the
same request from **5 empty in 100 to 0 in 100** (one-sided Fisher p ≈ 0.03), with the
rendered prompt otherwise identical.

That is a change to every llama.cpp request a turn sends after its first tool round —
context spent, and every template's handling of `reasoning_content` (Gemma's, gpt-oss's
harmony channel) to measure — so it is a track of its own, not this fix: roadmap,
"the turn's own thoughts on the OpenAI-compatible wire".
