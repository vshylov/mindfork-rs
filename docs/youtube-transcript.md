# Design plan: the words, not just the description — a transcript as a chat attachment

> **Genre:** track design plan (AGENTS.md §1), **stage 2** of the YouTube track.
> Stage 1 is merged (PR #247, #248); research, measurements and forks R1–R9 —
> [docs/research/youtube-integration.md](research/youtube-integration.md), what
> stage 1 turned into — §8a there. Behaviour — spec §9.9.
>
> §3 lists the forks that need the user's answer **before** any code is written.
>
> **Task (user's framing, 2026-08-01):** stage 2 is fork **R3(c)** — an option to
> get not only the description but *the words themselves*, and, when they are
> large, to land them as a **chat attachment** (spec §9.7) rather than as a tool
> result. The reason is exactly that: attachments already have a token budget,
> page-by-page reading (`attachment_read`) and semantic search
> (`attachment_search`), while a tool result goes into the context whole and
> would destroy an 8k local model.

---

## 1. What this is not: a cheap path

Stage 1's cost model applies **unchanged**. A transcript is the *same request*
with a different prompt: the video is ingested either way, and on the 3.x models
audio is not even billed separately from video (research §3.3a). So

- `transcript: true` must not read, to the model or to the user, as "a cheap
  extra". The tool description has to say the cost is the same, or the model will
  reach for it by default;
- the `max_minutes` ceiling and the `start`/`end` segment arguments apply to a
  transcript request exactly as they do to a watch request. Nothing about the
  input side changes;
- what *does* change is the **output** side: a description is ~500 tokens, a
  transcript of a 25-minute talk is several thousand. That is the one new cost,
  and it is where truncation becomes a correctness problem (F5).

---

## 2. The mechanism today — the constraint that shapes the stage

Read out of the code rather than assumed, because it decides F1.

**Tools do not mutate `Chat`.** `Tool::invoke` returns a `ToolOutcome` carrying
`Vec<ChatEffect>`, and `ChatEffect` today has exactly two variants
(`SetSystemMessage`, `SetSamplingOverride`) — both scalar. The agentic loop
accumulates effects (`effects.extend(outcome.effects)`,
`app/orchestrator/generation.rs`) and the **orchestrator** — the sole owner of
`Chat` (spec §4.4.2) — applies them in `handle_done`, i.e. **when the whole turn
is over**, not after the round that produced them.

**`ToolContext.attachments` is a turn snapshot.** It is an
`Arc<[Attachment]>` built once, at turn start, from `Chat.attachments`.
`attachment_read` and `attachment_search` read it (search additionally filters
its hits by it, so a removed file can never surface).

Put together, the naive implementation has a hole with a name. If the tool
returns "the transcript is attached as *X*, 14 pages" and the effect is only
applied at end of turn, then the model's very next move — `attachment_read("X",
1)`, which is exactly what the tool result told it to do — hits a turn snapshot
that does not contain *X* and gets "no such attachment; attached are: …".

That is not a hypothetical: it is the **third instance of one defect class** in
this project, both previous ones found in live runs — the by-reference
attachment block that described the situation without saying what was possible
(the model improvised with `fs_read` and `web_search`), and stage 1's own
unconfigured path (eight tool calls rediscovering measured dead ends). Both were
fixed by making the message close the door. Here the door has to be genuinely
open instead, because the tool *can* deliver.

Two further facts from the same reading, both load-bearing later:

- **A cancelled turn still delivers its effects.** The loop breaks on
  cancellation and still sends `GenResult { effects, .. }`. So a transcript the
  user already paid for is not lost when they press `Esc` — which is the right
  behaviour and needs no work.
- **The attach path already exists and is reusable**:
  `handle_attach_result` decides inline-vs-by-reference against the budget,
  replaces a previous attachment with the same `source`, prunes the index, emits
  `FileProgress::Attached` + `emit_attachments()`, and spawns background
  indexing. Stage 2 should join that path, not build a second one.

---

## 3. Decision points

### F1 — how the attachment reaches `Chat` (the contract change)

This is the stage's real design work. All options add one `ChatEffect` variant;
they differ in **when the attachment becomes readable**.

- **(a) `ChatEffect::AddAttachment(Box<Attachment>)`, applied by the
  orchestrator at end of turn *and* mirrored into the loop's `ToolContext`
  snapshot at the end of the round** — *recommended*. The loop never touches
  `Chat` (the invariant holds); it only updates the turn snapshot it already
  owns, which is what `attachment_read`/`attachment_search` consult. The
  attachment is therefore readable from the **next round of the same turn**,
  which is when the model will ask for it. The effect carries the
  **already-built `Attachment`**, so the object the model was told about and the
  object that gets persisted are the same one — same `id`, which is also the key
  the background index is written under.
- (b) The same effect, no mirroring. Smaller diff; the attachment appears only
  on the next user turn, so the tool result has to say "you can read it from
  your next message onwards" — a dead end dressed up as an instruction, and the
  exact failure mode §2 describes.
- (c) Send the effect to the orchestrator mid-turn and wait for it to apply and
  answer, over a channel (the tool-confirmation shape, spec §9.8). Correct but
  heavy: a round-trip and a new channel so the loop can be told something it can
  compute itself, and it makes the loop wait on the orchestrator for a fact that
  does not depend on it.

Notes on (a) that are part of the recommendation:

- the snapshot is rebuilt **once per round**, after the call loop — within a
  round the model has already issued its calls, so per-call granularity would
  buy nothing and would fight the borrow of `ctx` inside the call loop;
- the variant is boxed for the same reason `SetSamplingOverride` is
  (`clippy::large_enum_variant`);
- the variant is deliberately **generic** (`AddAttachment`, not
  `AddTranscript`): it is the natural home for any later "this tool produced too
  much text to hand back inline" case.

### F2 — when does the transcript become an attachment, and when is it just text?

- **(a) The threshold is the existing `config.attachments.max_file_tokens`** —
  *recommended*, and no new setting. Below it, the transcript goes straight into
  the tool result: the model reads it immediately with no second call, and
  attaching would be strictly worse (the same text would then sit in the pinned
  block *and* in the history). At or above it, it becomes an attachment.
  This has a property worth stating as an invariant: an attached transcript is
  **by reference by construction**, for any value of the setting — because
  `decide inline` requires `est <= max_file_tokens`, which is precisely what the
  threshold excludes. So this path never adds an inline attachment and never
  competes for the chat's inline budget.
- (b) A dedicated `config.video.transcript_max_inline_tokens`. A second knob
  saying the same thing as an existing one, and a user who tuned the attachment
  budget would reasonably expect it to apply here.
- (c) Always attach. A 3-minute video's transcript is ~500 tokens; turning that
  into a pinned excerpt plus a mandatory second tool call is ceremony.

### F3 — indexing, name and source key

- **(a) No special case for indexing; name from the title; source is a synthetic
  `youtube:<id>` key that includes the segment** — *recommended*.
  - *Indexing* falls out of F2(a) for free: a by-reference attachment is already
    indexed in the background, so `attachment_search` works on a transcript with
    no new code, and degrades gracefully with no embedder (ADR 0002) — exactly
    like every other by-reference file.
  - *Name*: `<video title> (transcript)`, clipped to a readable length, falling
    back to `YouTube <id> (transcript)` when the title cannot be read; with a
    segment, the segment is in the name. It has to be meaningful in `/file list`
    and typeable in `/file remove` (`#N` also works).
  - *Source*: attachments dedupe by `source` — re-attaching the same file
    replaces the previous snapshot. A transcript has no path, so it needs a
    synthetic key: `youtube:<id>` plus the segment when one is given. Consequence
    (intended): transcribing the same video/segment twice **replaces** rather
    than duplicates, while two *different* segments of one video coexist.
- (b) Name it by video id. Stable, and unreadable in `/file list`.

### F4 — one provider call or two?

`transcript: true` should give the description *and* the words. Since the input
cost is paid per request, that is one request or double the bill.

- **(a) One request; the prompt asks for the description, then a marker line,
  then the timestamped transcript; the tool splits on the marker** —
  *recommended*. Matches the measurement (research §3.2 probe 2 returned a
  summary, a visual outline *and* a transcript in one answer). Fallback when the
  marker is absent: treat the whole answer as the description and say plainly
  that no transcript came back — guessing which half is which would be worse
  than reporting it.
- (b) Two requests (describe, then transcribe). Doubles the expensive half for a
  formatting convenience.
- (c) `transcript: true` means "the answer *is* the transcript", no description.
  Then a user who wants both pays twice anyway, via two tool calls.

### F5 — timestamps, `start`/`end`, and truncation

- **(a)** *recommended*, three parts:
  - **Timestamps are absolute**, measured from the start of the *video* even
    when a segment is requested; the prompt says so, and the attachment's own
    header records the segment, so an ambiguous answer is still interpretable.
    (Whether Gemini would otherwise number a clip from zero is **unmeasured** —
    stating it in the prompt costs nothing and removes the question.)
  - **The attachment text carries a header** — title, URL, segment, and a line
    saying the transcript is model-produced from audio, not official captions.
    It is read by the model, so it is axis A, and it makes the attachment
    self-describing when read page by page much later.
  - **Truncation must be reported.** Today `VideoUnderstanding::describe`
    returns `String`, and a `MAX_TOKENS` finish with partial text is returned
    silently — for a description that is cosmetic, for a transcript it is a
    correctness bug, because the model would believe it has the whole thing (the
    same guarantee `attachment_read`'s page walk exists to give). So `describe`
    returns a small struct (`text` + `truncated`), the attachment gets an
    explicit end marker when truncated, and the tool result says so and points
    at `start`/`end` for the rest. Output budget: a constant
    (`TRANSCRIPT_MAX_TOKENS`, ~8k — roughly 25 minutes of dense speech), not a
    setting; the honest truncation report is what makes a constant acceptable,
    and it mirrors how the length ceiling already behaves.
- (b) Leave `describe` returning `String` and ignore truncation. One less
  contract change, and a silently half-transcribed video.

---

## 4. The shape, assuming the recommendations

```
entities/attachment.rs      no change to the type; a `decide_mode(est, used, cfg)`
                            helper extracted so the orchestrator and the tool
                            agree on the mode by construction rather than by
                            comment.

features/tools/mod.rs       ChatEffect::AddAttachment(Box<Attachment>)
                            ToolParams += the attachment budget (the tool needs
                            `max_file_tokens` for the F2 threshold).

features/tools/youtube.rs   youtube_watch { …, transcript?: bool }
                            one provider call, split on the marker; small →
                            appended to the result, large → built into an
                            Attachment and returned as the effect, with the
                            result naming it, its page count and the two tools
                            that reach it.

shared/video/mod.rs         describe() -> VideoAnswer { text, truncated }
shared/video/gemini.rs      finish_reason MAX_TOKENS → truncated (today: dropped)

app/orchestrator/
  generation.rs             rebuild ctx.attachments from the round's
                            AddAttachment effects; handle_done applies them
                            through the existing attach path.
  attachments.rs            the apply path made reusable (dedupe by source,
                            prune, FileProgress::Attached, emit_attachments,
                            spawn indexing) — one path, two entry points.
```

No new config section, no schema bump: `Chat.attachments` is an existing field,
the threshold is an existing setting, and nothing about storage changes
(ADR 0006 F12).

**Localization.** The `transcript` parameter description, the prompt, the
attachment header and the tool's result text are **axis A** (`ctx.loc` — the
model reads them). The feed note and the status chip come free: they go through
the existing `FileProgress::Attached`, which is already axis B.

---

## 5. Tests

Unit, alongside the code:

- the marker split — both halves, and the fallback when the marker is missing;
- an empty transcript (a video with no speech) is reported, not attached;
- the F2 threshold in both directions, plus the invariant "an attached
  transcript is by reference";
- the source key: same video+segment replaces, a different segment coexists;
- truncation: a `MAX_TOKENS` finish reaches the tool as `truncated`, and the
  result and the attachment both say so;
- the loop mirrors an `AddAttachment` effect into the turn snapshot — asserted as
  the *behaviour* that matters: `attachment_read` in the **next round of the same
  turn** finds it (a mocked two-round turn), which is the F1 hole;
- the orchestrator persists it, dedupes by source, and emits the attachment
  events (integration, through the real `run` loop).

**Live smoke** (`#[ignore]`, `MINDFORK_GEMINI_KEY`): a real video with speech,
`transcript: true`, asserting the transcript carries words that were *said*
(stage 1's smoke deliberately asserts on something only *visible* — together they
cover both halves of the original question), that it landed as an attachment, and
that the timestamps are in the requested form. Per AGENTS.md §3 this touches
tools + a provider protocol, so a live run is mandatory.

---

## 6. Documentation to update (AGENTS.md §4)

CLAUDE.md journal + status header; CHANGELOG `[Unreleased]` (user-visible);
spec §9.9 (and a pointer from §9.7, since a transcript is now a way an
attachment appears); docs/architecture.md §8 (the tool table and the effect);
README (the new argument); docs/roadmap.md (close the R3c groundwork item);
research §8a (what stage 2 turned into); this plan → `docs/history/` on merge,
with its own relative links re-pointed (`python tools/link_check.py`).

---

## 7. Out of scope — do not start these here

Named because they are adjacent enough to drift into:

- **cross-chat caching** of an expensive watch (R8b) — a follow-up in the same
  chat is already free;
- **a transcript without a cloud key** (R7) — every free route is PoToken-gated,
  so this means a paid vendor or a `yt-dlp` sidecar;
- **default-model rot** — a separate concern of its own;
- **`fetch_url`/`python_exec` gaining `attach: true`.** The new effect makes it
  cheap, and that is exactly why it should be a deliberate decision later rather
  than a side effect of this stage.
