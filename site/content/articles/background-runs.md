+++
title = "Background runs, and the pool they share"
description = "A subagent that outlives its round, the app's own quiet requests, and one local llama-server with one context pool under all of them: what the server does when the pool overflows, how the app keeps it from happening, and what parallel sessions actually buy on one GPU."
updated = "2026-09-13"
weight = 9

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-09-13"
+++

*The ninth in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/),
[the self-model](/articles/self-model/),
[vector search](/articles/vector-search/),
[local speed](/articles/local-speed/),
[the Python sandbox](/articles/python-sandbox/),
[the trust boundaries](/articles/trust-boundaries/) and
[the code workspace](/articles/code-workspace/).*

## The round used to be the unit

A delegation, until recently, held the reply. The subagent's transcript
streamed while it ran, and you could watch it, but the assistant could not
say anything to you until the run landed — a nested turn is still a turn.
`start_subagent` is the same delegation with the waiting taken out: the
call returns at once with the run's address, the assistant keeps answering,
and the result arrives later as a **task notification** in the chat, worded
for the assistant to read, with a preamble saying it is not your message
and grants nothing. When the chat is open and idle the app starts a turn on
it and the assistant reports; otherwise the note waits for your next
message, and the chat is marked unread until you come back.

Why a second tool rather than a flag on the first was measured, not
guessed. Four shapes of the ask were tried on the four clouds and on Qwen
3.6 locally — an optional `background` flag, the flag plus a sentence of
advice, a required `mode`, and a separate tool — against a scenario where
the user asks for a big review "start it and move on" and a small sum "right
now". Claude never passed the optional flag, three trials of three, and
*narrated* it every time — "I've started the critique in the background" —
then took the foreground result and relayed it as early feedback. A
well-formed call that means something else is the failure a harness cannot
detect. A required field or a second tool got 3/3 on every cloud and 5/5
locally; no model asked for the background when the answer was the point;
nobody invented the result before it came. So: a second tool, present in the
catalog only when you switch background runs on, and until then no request
differs by a byte.

## What a run owns

A background run is its own task with its own cancellation: `Esc` ends your
reply and not the run; `F6` on its transcript does, and so does
`/subagents stop`. Taking back or regenerating the exchange that started
it ends it too, and quitting lands every run as cancelled before the exit.
The record lands with the turn as a placeholder and is filled in by id when
the run ends — the route the automatic title already used — so a run that
finishes while another turn is streaming in its chat waits for that turn to
land, and its rows never come first. A staged dialogue can run the same
way, and the two share one cap, since what the cap protects is shared.

A background run **never asks you to confirm a tool call**. That was a
decision, not an omission: there may be nobody at the keyboard, the
"dangerous" mark on a tool is an author's guess rather than a
classification, and a confirmation reaching into a run's calls compounds
the guess. The control is the profile's tool set — switch off, for that
profile, whatever you would not let run unattended.

`F7` is the screen for all of it: every subagent and dialogue run of every
chat, the ones in flight first with their position (`round 3 · fs_read`,
`line 5`, *director*) and elapsed time, then the landed ones with how they
ended; and below them the app's own quiet work — reflection, the two
consolidations, history compaction — as running, *waiting* or idle. That
last word is the rest of this article.

## One pool, and what the server does when it is full

A `llama-server` launched without `-np` runs **four slots over one unified
context pool** — its default since December 2025 — and above one session
the app launches it with `-np N --kv-unified` on purpose: the pool costs no
more memory than one context, and what N sessions cost is sharing it. The
question is what happens when the sharing fails, and it was measured before
anything was designed, on a 2048-token pool with two slots.

Two conversations of 1244 prompt tokens each, sent at once, when the pool
cannot hold both prompts: HTTP 500 to **both** — in the streaming shape, one
stream got four chunks and the other none before the same error object.
The server's log tells the mechanism: slot 0 finished its prefill and began
decoding; slot 1's prefill found no room; the batch was halved from 1024
down to 1 in a hundred milliseconds, and `Context size has been exceeded`
went to both tasks. The slot that had been decoding perfectly well died
with the one that overflowed. The second collision is subtler: two prompts
that *fit*, 884 tokens each with 280 to spare, each allowed a 400-token
reply. Both prefilled, both decoded about 140 tokens, and at the next token
there was no cell — both ended, after a minute of visible progress, and
both had committed, so neither could be retried. That is the case a guard
that priced prompts alone would have admitted.

The server offers a per-slot cap, and it was tried: `--kv-unified-per-slot`
turns the collision into an instant, per-request refusal — and into a
silent cut, a reply stopped 260 tokens short with nothing but `length` to
say so, which for a subagent is a truncated report the parent reads as
complete; and it halves every conversation, the main chat included. That is
a server operator's tool for many independent clients, not this app's
guard.

## Admission by budget

The app keeps the sharing within the pool itself. Every stream a turn opens
**reserves** what it will occupy — its prompt plus its reply cap — and a
stream that would not fit beside the ones already open **waits for room**
instead of taking a slot. The prompt estimate is the weak part of that
sentence: the app's estimator is a byte count, and on adversarial text it
was off by a factor of 2.19 in the direction that admits a collision. So
the reservation is *calibrated* by the turn's own latest exact count from
the server, a ratio measured on this model and this conversation's kind of
text — and, since later work found the ratio was mostly the tool schemas in
disguise (a fresh chat's first request estimated 85 tokens where the server
counted 4358), the schemas are now counted as they are sent, each kind of
request keeps a correction of its own, and the estimate lands a tenth over
the exact count rather than a fiftieth under.

Measured live in three arms, on the CPU build and on Qwen 3.6 27B with four
slots over 16 384: two children each sized at 55% of the pool took turns and
both completed, where the control arm — the guard disarmed by a lie about
the pool's size — lost one child; four quarter-pool children streamed
exactly two at a time.

## The app's own requests, in one lane

The guard covered everything the turn machinery opened. Seven request paths
did not go through it: the automatic title, reflection, the two
consolidations, the compaction roll, impersonation on the shared engine, and
a page summary made from inside a loop. They had always shared the server
with the turn, and nothing had gone wrong — until the pool was unified. The
compaction roll fires exactly when the conversation is at its largest: at
three quarters of the window, with a digest of everything but the last 2048
tokens. Beside the next turn, the two are far past the pool together; the
server halves its batch to one and ends both slots; the roll fails as a
debug line and the turn fails in front of you. Measured on the CPU build: a
chat seeded to 55% of the pool, a background run handed as much again, and
`/compact` the moment the parent landed — both arms failed, and the failure
was worded with the *title's* error string, which is how nobody had noticed.

The fix is a second lane on the same budget: one silent permit, over the
same pool sum as the turns. A silent stream reserves its digest plus its cap
like any other, waits for room under the same rule, and the app's own
requests take turns among themselves — a landing's five requests spread over
the minute after it instead of the second. The same two arms after the lane:
the roll waited for the run's stream to end, one live stream at a time,
both completed — 88 s end to end on the CPU build, 16.5 s on the GPU stack.

## Yielding

Waiting has a cost of its own. A turn that did not fit beside an open
silent round waited for that round: 45.9 s on the CPU build, 5.6 s on the
GPU. Now the silent stream is **cancelled** to make room for your message
and made again after your reply, at most three times before it is left to
finish — what remains is the server finishing the batch it is processing.
Measured: the CPU build's wait fell from 46 s to about 24 s, the GPU's from
5.6 s to under two; the roll pays its prefill twice. The impersonation
preview is never displaced this way, and the loops' own clocks stop while
they wait, so a task is not timed out for time it spent in the queue.

That remaining 24 s is the batch. llama.cpp honours a cancel *between*
batches of `-b` prompt tokens, and on a CPU-only host the default batch is
twenty-odd seconds of prefill. With no GPU layers the app now launches the
managed server with `-b 256 -ub 256`: a displaced stream holds its slot for
6.5 s instead of 23, at about a seventh slower prompt processing; the wait
is linear in the batch, and 256 is the knee that was measured. A GPU host
at its defaults is untouched. And because the batch is a launch flag that a
detection at runtime can only *advise* on, the app reads the server's own
timing of every prompt — the roll's and the loops' first rounds are the
coldest prompts a session makes — and once per server session says in the
feed how fast prompts are processed, how long such a hold would be, and the
one setting to change. On a GPU host, on the clouds, or where the batch is
already small, nothing is said.

## Sessions, honestly

*Parallel sessions* is a setting on every engine mode, 1 by default, and
its hint says what the server reports. What raising it buys was measured on
one RTX 4090, and the answer is per model. A small model gained 2.8× from
four streams at once. Gemma 4 31B at Q4_0 gained 1.14× at two streams and
1.22× at four — bandwidth-bound already at one, so a second stream costs the
first almost half its speed and each transcript reads at 12 tokens a second;
on that card, above two buys nothing a user would feel, and the gain of
parallel subagents is the overlap of their *tool work*. Qwen 3.6 27B on the
same card kept 77% of its per-stream speed at four streams and gained 2.9×.
The honest setting is per model, which is why the field is per mode and the
hint says what it measured rather than guessing.

Two more facts make the default of 1 comfortable. Interleaving two
conversations on one slot costs nothing measurable: the server's RAM prompt
cache parks an evicted context and restores it on the next visit — 970
cached tokens found again, a memory copy invisible next to the new tokens'
own prefill, on the 31B as on the small model, as long as the parked set
fits the cache. And past the slots the server queues rather than fails: six
requests on four slots all returned, two of them later. A client that opens
more streams than there are slots loses time, not requests — on a cloud
the same overflow is a 429, which is the other reason the budget exists.

The reads of one reply run together on the same principle. Models emit
several independent reads in one reply unprompted — 26 of 26 on both gate
models and four clouds — and *Parallel tool calls* runs them at once, only
the tools that change nothing and hold nothing, four at a time on a cloud
engine and one after another on a local one until you raise it, the results
recorded in the model's order.

## Stopping and quitting, exactly

A stop postpones. A reflection or a consolidation stopped before it wrote
anything gives its window back — the same replies are reflected on after
the next landing, as if it had never started; one stopped after it wrote
keeps its place, so nothing is written twice; one that had only read gives
the window back too. A quit follows the same rule, and where a task is in
the middle of a round of tools the quit waits for that round to report so
the decision is exact — as long as it takes by default, or at most the
seconds you set. The compaction roll lands on a channel of its own, and the
quit listens to it now; before, a quit during a roll waited the whole cap
for a landing that never arrived where it was looking.

Every one of these is a research note in the repository, with the forks,
the measurements and the live runs: admission by budget, the silent lane,
preemption, the CPU batch, slow-prefill detection, parallel subagents and
background subagents, under
[`docs/research/`](https://github.com/vshylov/mindfork-rs/tree/main/docs/research).
