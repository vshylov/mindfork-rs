# Pauses in the middle of a spoken reply

**Status:** decided and built — `fix/tts-pauses` (2026-10-24). The owner asked
for all three changes proposed after the diagnosis: look-ahead by seconds of
audio, the speech timings in the log, and short blocks packed together (a heading
with its paragraph, a list's items together), "the full cycle to a PR". The
sub-decisions below (F1–F9) are the agent's, taken on that instruction. The live
run found a provider stall no look-ahead hides; a second request for a stalled
chunk followed, at the owner's word, in `fix/tts-stalled-chunk` (§6, F10–F14).

Related: [tts.md](tts.md) (the speech research; §13 "grown pauses"), spec §11.9,
[docs/journal/tools.md](../journal/tools.md).

## 1. The complaint

With *Speak replies* on (`companion.speak_replies`), a reply is sometimes spoken
with long silences in the middle — most noticeably around lists and code blocks.
The owner's last session (2026-10-24, 18:30–18:56 UTC) had one long silence and a
couple of short ones.

## 2. What the log could say: nothing

The session's log holds the turn's rounds, the title and the embedder — and not
one line about speech. The speech task logged no timing at all: neither how long
a chunk took to synthesize, nor how long it played, nor that the sound ran dry
while the next chunk was still being made. So the diagnosis was made by
reproduction (§3), and the log gets the numbers that would have answered the
question directly (F7).

## 3. Diagnosis — measured

The reply from that session (5 183 characters of markdown: three headings, two
lists, eleven paragraphs) was put through the same road as the app —
`build_utterances` → `chunk_utterances` with OpenAI's 4096 limit — and every
chunk was synthesized by the owner's settings (`gpt-4o-mini-tts`, `onyx`, `pcm`),
timing the request:

- **38 chunks, one per block.** `speakable_text` ends every block — paragraph,
  heading, list item, the "(code block skipped)" note — with a line break, and
  `chunk_utterances` packed sentences **within a line only**, so every block went
  as a request of its own. The design said otherwise: "a short message goes as a
  single request" (spec §11.9; research §13: "the right fix is not to chunk").
  Its tests only ever chunked a single paragraph.
- **One chunk of look-ahead.** The pipeline synthesized chunk N+1 when the queue
  held fewer than two clips — that is, as chunk N *started* playing. So N+1's
  whole synthesis had to fit inside N's sound.
- **The clip is read whole before it plays** (`resp.bytes()`), so a chunk's
  synthesis time is the full generation, not the first byte (0.5–2 s).
- **Synthesis time varies widely.** For the long chunks: 3.2–26.7 s; throughput
  from 6.7× real time down to **0.9×** (a 309-character paragraph, 23.9 s of
  sound, took 26.7 s).

The silence before chunk N+1 is therefore `synth(N+1) − sound(N)`: a short block
(a heading, a list's last item, the code-block note — 1–5 s of sound) followed by
a long paragraph (10–27 s of synthesis). Simulating the pipeline on the measured
times:

| Silence before | after (the short block) | silence |
|---|---|---|
| a 309-char paragraph | the list's last item, 59 chars, 5.1 s of sound | **21.7 s** |
| a 225-char paragraph | the heading "What can be concluded…", 4.2 s | **11.0 s** |
| a 167-char paragraph | the heading "My conclusion", 1.4 s | **8.8 s** |
| a 288-char paragraph | a list item, 2.2 s | 1.3 s |
| two more | | 0.3–0.7 s |

Six silences, 43.8 s in all, in 397 s of speech — the owner's "one long and a
couple of short ones", in the places the complaint named. Lists and code blocks
are where short blocks are.

The same times with look-ahead counted in **seconds of audio** instead of clips:
15 s ahead leaves one silence (11.8 s), **30 s ahead leaves none**.

A side observation, not acted on: one list item (48 characters) came back as
0.6 s of sound — the model swallowed it. Seen once.

## 4. Forks and decisions

**F1 — what the look-ahead counts.** (a) seconds of audio queued; (b) more
clips; (c) nothing — synthesize everything as fast as possible. **(a)**: a clip
count treats a two-second heading like a thirty-second paragraph, which is the
defect; (c) pays for everything a stop throws away, the reason the bound existed.

**F2 — how far ahead.** **40 s.** The next chunk's synthesis starts when less
than 40 s of sound is queued, so it has at least that long to finish. A
400-character chunk is ~31 s of sound, and at the slowest throughput measured
(0.9× real time) it takes ~34 s — inside the margin. What a stop wastes is at
most the 40 s plus the chunk in flight, ~70 s of sound, about two cents at
`gpt-4o-mini-tts` prices.

**F3 — one request at a time, or two.** (a) sequential; (b) two in flight.
**(a)**: the simulation needs no more, and a second request is a second chance at
a provider's rate limit.

**F4 — the packing unit.** (a) sentences packed greedily across the message's
blocks, a line break kept where a block ends; (b) whole blocks only; (c) the whole
message up to the provider's limit. **(a)**. (b) leaves a heading alone whenever
its paragraph does not fit beside it — the very case. (c) is §13's "do not
chunk", but with the clip read whole the first sound would wait for all of it:
4096 characters is ~5 minutes of sound and, at the throughputs measured,
45 s–5 min of synthesis. A seam therefore always falls on a sentence boundary,
and a block boundary inside a chunk is still a line break, the pause the speech
extractor puts there for every engine.

**F5 — the sizes.** **Soft targets of 200, 300, then 400 characters**, capped by
the provider's hard limit. The first chunk is small because the first sound waits
for its whole synthesis (today it was the first block, of any length — 337
characters here, 4.2 s). The ramp to 400 gives each early chunk a predecessor
long enough to hide it in: chunk 2 must be made while chunk 1 plays, and a chunk
twice as long breaks even only at 2× real time. The targets are soft: a sentence
is never cut to meet one — a sentence longer than the target goes whole, and only
the provider's ceiling (4096 / 2000) cuts inside a sentence, as before. On the
session's reply: 38 requests become 15.

**F6 — playing a chunk while it downloads.** Not built. It would bring the first
sound down to the first byte (0.5–2 s), but needs a growing source in the player
and a policy for a stream slower than real time (measured: 0.9×), which plays as
stutter inside a word rather than a pause between sentences. Recorded here as the
next step if the first sound ever matters more than it does now.

**F7 — the log.** One `INFO` line per chunk — its number, characters, synthesis
time, sound length, the sound that was queued when its synthesis began, and the
**silence** it caused — and one summary line per run: chunks, sound, the first
sound's latency, the number and total of audible silences (≥ 250 ms), and whether
the run was stopped. The silence is read from the queue itself — polled every
80 ms while a chunk is synthesized, from the moment it is found empty — so a
`/tts pause` (the queue is not empty, only paused) is never counted as one.

**F8 — the bridge's voice notes** use the same chunker (`bridge_voice_reply`):
fewer requests per note and fewer seams in it; nothing else changes there.

**F9 — a clip's length.** Raw PCM's length is its byte count; a container (WAV,
MP3) is now **decoded whole before it is queued**, off the runtime's threads, so
its length is exact. A streamed MP3's header cannot be trusted for it (MiniMax's
counts no frames, Kokoro's half — `playback::decoder`), and a length of zero would
have made the look-ahead synthesize everything.

## 5. Live run

`gpt-4o-mini-tts`, `onyx`, 2026-10-24, the pipeline as built, the sound "played"
on the clock (`ClockQueue`) so that what is measured is when a chunk is ready
against when the listener would need it.

**The session's reply**, the one that fell silent: 38 blocks → **15 requests**
(186, 256, 375, … characters). First sound after **2.7 s** (it was 4.2 s, the
first block whole). 14 chunks of 15 played without a silence — among them one
made *slower* than it sounds (35.2 s for 28.5 s), hidden by the 40 s queued. One
request took **153.5 s** (31 s of sound, 31.9 s queued when it began): a silence
of **121.5 s**. That is the provider — the diagnosis's 38 requests took 0.7–26.7 s
— and no look-ahead hides a request that outlasts everything queued, since chunks
are made one at a time (F3). The log now names it in one line: `synth_ms=153470
… silence_ms=121537`.

**The smoke's reply** (`a_structured_reply_plays_without_a_silence_live`, five
headings-lists-code-paragraphs blocks in the failing order): 16 blocks → 5
requests, **no silence**, twice — once with OpenAI slow (the first chunk 18.3 s
for 12.3 s of sound, so the first sound came late, as it would have before), once
at its usual speed (first sound 2.7 s, 113 s of speech). The tightest margin was
the ramp's second chunk: made in 12.45 s with 12.75 s queued.

**Proposed: a second request for a stalled one.** When a chunk's synthesis
outlasts the sound queued when it began — the listener is about to hear nothing
— ask for the same chunk again and take whichever answer comes first, cancelling
the other. It would have turned the 121.5 s into a few seconds, at the price of a
second paid request for that chunk alone. It changes what is paid for, so it was
the owner's call, and a change of its own (§6).

## 6. A stalled chunk asked for again

The owner, 2026-10-24, after §5: "a second request when a chunk's synthesis
takes longer than the sound queued; take the first answer" — a pull request of
its own on top of `fix/tts-pauses`, the full cycle. Sub-decisions the agent's:

**F10 — when.** When the request has run longer than the sound that was queued
when it began — from then on the listener hears nothing — and **not before 10 s**
of it. Without the floor the first chunk of every run (nothing queued) and every
chunk after a silence would always go twice. 10 s is past what a chunk takes at
the provider's usual speed (2.7–15 s measured for up to 400 characters, the first
chunk's 200 at 2.7 s) and far short of a stall (153.5 s).

**F11 — how many.** One more, never a third: a provider that stalls twice for
one chunk is having a bad minute, and a third request pays for it again.

**F12 — which answer.** The first that is **not a refusal**; the other request is
dropped, which closes its connection, so the provider stops making it. A refusal
while the other request runs waits for the other; both refused — the later
refusal is told, as one refusal was before. Cancellation (a stop) ends both.

**F13 — the log.** A line when the second request goes (`speech: a chunk's
synthesis stalled; asking again`, how long the first had run), a line when one
answers (`speech: a chunk asked twice is answered`, `by` the first request or the
second — whether the second paid for itself), `hedged` on the chunk's line,
`hedges` in the run's summary — so a bill's extra request can be found in the log.

**Live** (2026-10-24, `gpt-4o-mini-tts`, `onyx`, the sound on the clock): the
smoke's reply — 5 requests, no silence, **no chunk asked twice** (one took 16.9 s
with 39.9 s queued: covered, so not asked again). The owner's reply — 15 chunks,
**no silence in 390 s of speech**; four chunks took 17–32 s, each covered by the
~40 s queued; the first chunk was asked twice — its request still running at the
10 s floor with nothing queued — and answered at 11.0 s (which request answered was
not logged yet; it is now). Through the real client against a server that never
answers the first request, that request's connection is closed the moment the
second answers (`a_stalled_request_is_dropped_on_the_wire_when_the_second_answers`).

**F14 — where.** Speech the listener waits for only. A Telegram voice note is sent
whole when it is ready; nobody hears a silence in it, so it keeps one request per
chunk.
