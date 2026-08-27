# `/continue` — resuming an interrupted generation — research

> Status: **implemented — stage 1 (managed/external), 2026-08-28**; the §8
> stage-2 cloud widening is recorded in the roadmap ("Engine and
> reliability"). Journal: [engine.md](../journal/engine.md), "stage 1:
> resuming an interrupted reply in place". User's decision
> (2026-08-27): every fork in §6 at its recommended option — F1(a) through
> F9(a). The §7 probe ran the same day (spike PR `spike/continue-probe`,
> `src/shared/api/continue_probe.rs`): results in §7.1, one design amendment
> in §4(d) (the echoed prefill), and the Grok row in §2 is now a measured no.
> **Probe complete**: the llama-side arms re-ran the same day on Qwen 3.6
> (llama.cpp build b10659) — the #21889 thinking rejection is gone on current
> builds (prefill accepted, thinking silently skipped), and the explicit knob
> pair is unknown even to b10659, so it is vLLM-only in practice.
> Journal: [engine.md](../journal/engine.md), "stage 0: the live continuation
> probe".
>
> The ask: a `/continue` command that lets the model resume a generation that
> was interrupted — by the user (`Esc`/`/stop`), by a connection/stream
> failure, or by running out of context (after the window is enlarged) —
> continuing **exactly after the last token that arrived**, and behaving sanely
> in turns that use tools. Spec §6.4 already reserves the feature: *"Continuing
> a response: request a continuation of the assistant's last response
> (optional; useful when `finish_reason = Length`)"* (spec.md:481-485); it is
> not on the roadmap and has never been designed until now.

## 1. Problem

All three interruption kinds already end in the same, well-defined state: the
partial reply is kept as a first-class `Message` (text **and** thoughts), saved
to disk, and annotated in the feed — `(generation cancelled)` for `Esc`
(`screens/chat/feed.rs:514-516`), *"The reply was cut short — … What did
arrive is kept; press Ctrl+R to generate it again"* for a mid-stream failure
(`ui.err.generation_interrupted`, locales/en.json:1503). The only route
offered is **regeneration**, which throws the partial away and re-spends the
whole reply — painful exactly when the interruption came late in a long
answer, and doubly so after a context overflow, where the regenerated reply
would grow back into the same wall.

A third interruption kind is currently invisible: `FinishReason::Length`
(token/context limit hit mid-generation) is treated identically to `Stop` in
the main turn path (`generation.rs:1355-1372`) — the reply just stops
mid-sentence with no note at all. The only consumer of `Length` today is the
compaction roll (`orchestrator/compaction.rs:454`).

"Continue exactly after the last token" is achievable in **text space**: the
partial is re-sent as a trailing assistant message and the provider continues
it (assistant prefill / continue-final-message). Token boundaries at the seam
may re-tokenize differently than the interrupted stream had them, but the text
is continued byte-exactly; that is the same guarantee every provider's own
prefill feature gives. What is *not* recoverable anywhere is a partial
**reasoning** trace (§4e) or a partially-streamed **tool call** (already
dropped at the contract layer, `generation.rs:1355`, `contract.rs:377-383`).

## 2. What the providers actually support (verified 2026-08-27)

The app's six modes map to four clients (`supervisor.rs:499-508`). Continuation
support is per *protocol and model*, not per app:

| Mode / client | Exact continuation | Mechanism and caveats |
|---|---|---|
| **Managed** llama-server, **External** llama.cpp (`OpenAiClient`, `/chat/completions`) | **yes — native, on by default** | *"Prefilling of assistant messages similar to the Claude API"*; `--prefill-assistant … (default: prefill enabled)` — a trailing assistant message is simply continued. A newer explicit form exists in master (`continue_final_message`, with `add_generation_prompt` required false), but it is **not wired into `/v1/chat/completions` as of b10659** — the probe's wrong-typed value passed as 200 — so in practice the pair is vLLM-only and default prefill carries llama.cpp. Three documented sharp edges: (1) **thinking**: with `--reasoning-budget` non-zero (default **-1**) on a thinking-capable template, #21889-era builds reject prefill — HTTP 400 *"Assistant response prefill is incompatible with enable_thinking"*; per-request `chat_template_kwargs: {"enable_thinking": false}` (documented in the server README) lifts it — **and the rejection is build-dependent: gone on b10659** (measured 2026-08-27, Qwen 3.6 — prefill accepted, thinking silently skipped with empty reasoning), so the design sends the kwarg and accepts both behaviours; (2) on Qwen 3.5 with `enable_thinking:false`, a template-injected `<think>\n\n</think>` residue precedes the continued content (#21511) — cosmetic, our fallback thoughts parser (`shared/api/thoughts.rs`) strips exactly this shape, and it did **not** reproduce on b10659; (3) *"Cannot continue an assistant message that contains tool calls"* (server-common.cpp) — which our tail never does, see §3. Prefill requires string content (#14353); our assistant wire content is always a string (`openai/wire.rs:159-184`). |
| **External** vLLM | **yes — explicit opt-in** | `continue_final_message: true` + `add_generation_prompt: false` in the request body (vLLM OpenAI-compatible server docs); without them a trailing assistant message is treated as complete and a **new** message is started. |
| **External** Ollama / LM Studio / other | **unknown** | Not documented either way; the realistic default is "new message", which would corrupt a continuation (two replies glued with no seam). Needs a probe or an honest refusal (§6 F2). |
| **Anthropic** (`AnthropicClient`) | **model-dependent — removed on current models** | Prefill (trailing assistant message) is a long-standing documented feature, **but Anthropic removed it on Opus/Sonnet 4.6, 4.7, 4.8 and the whole 5 family (Opus 5, Sonnet 5, Fable/Mythos 5): such requests now return 400**. Models ≤ the 4.5 generation (incl. `claude-haiku-4-5`) still accept it — and this repo has **measured** it: the impersonation probe that ended history on an assistant turn got empty text + `Stop` from claude-haiku-4-5 — *"not a rejection but a prefill, since a trailing assistant turn is continued rather than answered"* ([engine journal](../journal/engine.md), "impersonation sends the compacted conversation"; the rule is pinned by `compacted_impersonation_starts_with_assistant_and_ends_with_user` and recorded in spec §17). Two extra constraints where it works: prefill is incompatible with extended thinking (the continuation request must go out without a `thinking` block), and a prefill ending in trailing whitespace is rejected (the tail must be right-trimmed before sending). |
| **Gemini** (`GeminiClient`) | **yes — measured, undocumented** | The same probe measured native Gemini (gemini-3.5-flash) continuing a trailing `model` turn (empty text + `Stop` on a complete-looking tail). Google's API reference documents neither support nor prohibition; AI Studio's own "run from an edited model turn" flow relies on the same behaviour. Treat as working-but-unwarranted: keep the live probe in the smoke set. |
| **OpenAI** (`ResponsesClient`, `/responses`) | **no** | Neither Chat Completions nor Responses continues a final assistant message — a trailing assistant item is prior context and the model starts a new message (long-standing; re-confirmed by OpenAI's own developer-community answers in 2025). `previous_response_id` continues a *conversation*, not a partial message, and our client deliberately sends `store:false` (ADR 0004). Only a lossy instruction-based imitation is possible. |
| **Grok** (`OpenAiClient` against xAI) | **no — measured** | xAI documents that roles may appear in any order, and documents nothing about continuing a trailing assistant message. The §7 probe (grok-4.5, 2026-08-27) settled it: a mid-answer assistant tail is treated as a complete turn and the model **restarts** — reasoning opens with "The user asked…" and the reply is a fresh sentence without the fixture's marker. No continuation semantics; `/continue` refuses on Grok. (xAI silently drops unknown request fields — measured in the Grok research — so the llama.cpp/vLLM knobs are ignored rather than erroring.) |

Sources: llama.cpp server README (prefill row, `--reasoning-budget` default,
`chat_template_kwargs`) — github.com/ggml-org/llama.cpp `tools/server/README.md`;
issues #21889 (thinking rejection, closed-not-planned), #21511 (Qwen think-tag
residue), #14353 (string content); vLLM OpenAI-compatible server docs
(`continue_final_message`); Anthropic API docs (prefill; removal on 4.6+/5-family
per the model-migration guidance); OpenAI developer community "Can the API
continue generation exactly after the last assistant message" (answer: no);
xAI chat guide (role order; no continuation contract); this repo's own probe —
[engine journal](../journal/engine.md) "impersonation sends the compacted
conversation".

## 3. Current state in the code

Everything below is measured against the current tree; file:line refs are the
evidence.

**What survives an interruption.** `finalize_message`
(`generation.rs:2457-2478`) files partial text and partial thoughts into a
normal `Message` with `MessageMetadata{sampling, mode, model}`;
`handle_done` pushes and saves it (`generation.rs:653-658`). A
partially-streamed tool call is dropped (the `ToolCalls` gate at
`generation.rs:1355` plus `ToolCallAccumulator::finish`,
`contract.rs:377-383`). A cancel **during** a tool call files the whole round —
the call record and a `tool_cancelled` result message — so a turn interrupted
between rounds always leaves either a text tail or a tool-result tail, never a
dangling `tool_calls` assistant tail. If nothing streamed at all, no message is
created (`generation.rs:2463-2465`, `620-622`). Pinned by
`cancel_stops_generation_and_saves_partial` and
`a_mid_stream_error_reaches_the_feed_and_keeps_the_partial_reply`.

**Nothing records *why* a reply ended.** `Message`
(`entities/message.rs:80-114`) has no finish reason and no interrupted flag;
`finalize_message` drops `out.reason` on the floor; `FinishReason::Length` is
invisible in the main path. `/continue` needs a persisted, additive end-state
field (§4a) — the `#[serde(default)]` pattern per ADR 0006.

**A trailing assistant message already goes out as-is.** `build_request_in`
(`request.rs:158-186`) emits whatever the chat holds; none of the four wire
builders validates role order or requires a trailing user message. Anthropic
and Gemini merge adjacent same-role messages and drop empty ones
(`anthropic/wire.rs:194-246`, `gemini/wire.rs:219-284`); Responses emits an
assistant item only when non-empty (`responses/wire.rs:245-249`). Past
assistant *thoughts* are stripped from requests by construction
(`request.rs:16-38`, rationale `contract.rs:50-62`) — a continuation request
carries the partial's visible text only, which is exactly what prefill wants.

**No affordance for an explicit continuation flag yet.** `ChatRequest`
(`contract.rs:207-216`) has no such knob; repo-wide grep for
`continue_final`/`add_generation_prompt` is zero hits.

**Compaction cannot hurt the tail.** A roll never edits `chat.messages`
(`orchestrator/compaction.rs:10-13`), the cut always lands on a `User`
boundary (`features/compaction.rs:297-300`), so a trailing partial assistant
message survives any number of rolls — and compaction is precisely what frees
prompt space for a post-overflow continuation. Context overflow itself arrives
as a pre-stream 400 (`exceed_context_size_error` et al.,
`OVERFLOW_MARKERS`, `features/compaction.rs:310-331`) whose note already
points at `/compact`.

**Seams to reuse.** `handle_regenerate` (`generation.rs:218-255`) is the
template for "command that prepares history, then `start_generation`" —
including the readiness-before-mutation ordering. `start_generation`
(`generation.rs:316`) is the single re-entry point for both existing callers.
The turn loop (`TurnLoop::run`, `generation.rs:1347-1374`) needs no structural
change: a continuation is round 1 with a prefilled tail; if the continued
round ends in tool calls, `tool_round` proceeds as usual. Impersonation
already ships a provider-agnostic "continue what's started" fallback — the
seed goes into the **system prompt** (`prompt.impersonation.continue`,
`impersonation.rs:217-221`) — the shape a degraded mode would reuse (§6 F3).

**The one UI trap: bubble merging inserts a paragraph break.** Consecutive
assistant messages are joined with `"\n\n"` (`message_feed.rs:213-219`,
`feed.rs:433-438`) — so a continuation pushed as a *new* `Message` would render
`…partial\n\ncontinuation…` and break mid-word resumes. The continuation must
**append to the existing `Message`** (same id), and the feed must re-mark the
last bubble as streaming (`ensure_streaming_bubble`, `feed.rs:355-371`,
currently reuses only an already-streaming bubble).

`RetryBackend` does not interfere: a continuation request is a fresh
`chat_stream`, and the commit rule (`retry.rs:249-256`) is per-attempt.
Cancelling a continuation cancels like any turn — `/continue` after a
cancelled continuation just continues later.

## 4. Design

**(a) Persist the end state.** `MessageMetadata` gains
`finish: Option<MessageFinish>` (`Stop | Length | Cancelled | Error`),
`#[serde(default)]`, written by `finalize_message` from `out.reason` (which it
currently discards). Old chats read as `None`. This is also what finally makes
`Length` visible: the same change adds the missing feed note for a
length-truncated reply (§4g).

**(b) The command.** `/continue`, a registry row (`UiCommand`, `Arity::none`)
— no new parser module (the registry earns one only with real syntax). Runner
arm refuses, each with a localized note naming a working route
(lessons §4): no chat → `ui.cmd.no_chat`; generating → `ui.cmd.generating`
(route: `/stop`); read-only sub-agent transcript → add to `BLOCKED`
(`input.rs:355-365`) next to `/regen`; last message not an interrupted
assistant reply → a new note ("nothing to continue; `/regen` regenerates the
last reply"); thoughts-only partial (empty text) → refuse — no provider can
continue a reasoning trace over a chat API, and three of four wires drop an
empty-content message outright (§3). Eligibility on the recorded end state is
fork F1; provider capability is fork F2, refused with an honest note where
absent (F3).

**(c) The continuation turn.** `AppCommand::ContinueLast` →
`handle_continue`, shaped like `handle_regenerate` minus the truncation:
idle-gate → `ready_backend()` → eligibility checks → `start_generation` with a
new `continuation: Option<ContinuationSeed>` — the seed carries the tail
message's id and its byte length at start. `build_request` is unchanged (the
tail already rides along); `ChatRequest` gains `continue_final: bool`. Per
wire:
- `OpenAiClient` (managed/external/Grok): when set, add
  `"continue_final_message": true, "add_generation_prompt": false` and
  `"chat_template_kwargs": {"enable_thinking": false}` to the body. On current
  llama.cpp the explicit pair is honoured; on older llama.cpp the unknown
  fields are ignored and default prefill continues anyway; on vLLM the pair is
  exactly its documented opt-in; the kwarg suppresses the thinking rejection on
  Qwen-family templates and is inert for templates without `enable_thinking`
  (and for the continuation request specifically, "don't re-open reasoning" is
  the wanted semantics anyway).
- `AnthropicClient`: allowed only for models still accepting prefill (F2);
  the continuation request goes out without a thinking block, and the tail's
  text is right-trimmed in the wire copy (Anthropic rejects trailing
  whitespace in a prefill). The persisted message is not modified.
- `GeminiClient`: nothing to add — the trailing `model` turn is the mechanism.
- `ResponsesClient`: unsupported; the capability table (below) never lets a
  continuation reach it.
Capability lives in one place, mirroring
`entities::sampling::supported_sampling_fields`: a
`continuation_support(mode, model) -> Exact | None` table consulted by the
command's eligibility check and by the note text (derive the message from the
state it describes, lessons §4).

**(d) Append in place.** The turn loop's first round runs with the seed: its
`finalize_message` output is **appended to the seed message's `text`** (id
unchanged, `metadata.finish` updated, model snapshot per F7) instead of pushed;
later rounds of the same turn (after tool calls) file new messages as today.
*Amendment from the stage-0 probe (2026-08-27):* llama.cpp **echoes the
prefill** at the head of the response — the stream delivers
`partial + continuation` — while Anthropic and Gemini return the continuation
alone. The seed-append site therefore normalizes first: when the incoming
round's text starts with the seed's text, that prefix is stripped before
appending (and the feed must not re-render it as fresh deltas — the same
strip applies at the streaming consumer, keyed off the continuation marker on
`GenerationStarted`).
Feed side: on `GenerationStarted` with a continuation marker, the last
assistant bubble is re-marked streaming so `push_chunk` appends into it with no
separator; the in-flight mirror seeds `LivePartial` with the existing text so a
mid-turn chat switch rebuilds correctly. Saves ride the normal debounced path;
`cache.db` is derived data and reindexes from the saved chat.

**(e) Tools.** Three tail shapes, three behaviours:
- **Text tail** (the common case): prefill continuation as above, with the
  turn's tools **included** in the request — a continued round may finish its
  sentence and then legitimately call a tool; from there the agentic loop
  proceeds normally (fork F5; the probe verifies llama.cpp's tool grammar
  coexists with prefill).
- **Tool-result tail** (turn interrupted between rounds — the round was filed
  with `tool_cancelled` results): no prefill needed or sent; `/continue` simply
  re-enters the loop and the model writes the next round against the recorded
  results. This is the normal agentic request shape on every provider, so this
  case works even where prefill does not.
- **Dangling tool-calls tail**: cannot occur (§3) — and llama.cpp would refuse
  it, which the eligibility check mirrors by construction.
A partially-streamed tool call was never committed (no effect fired — effects
apply only after a completed round), so nothing can double-fire; committed
tool effects from earlier rounds stay committed, and their results are in the
history the continuation sees.

**(f) Context overflow, end to end.** After a `Length` stop or an
`exceed_context_size_error`, `/continue` is the second half of a two-step
recovery: free space first (raise the managed context in settings — the
supervisor restarts the server; or `/compact`, whose cut spares the tail by
construction; auto-compaction after the truncated turn may already have done
this), then `/continue`. If the continuation request itself overflows, the
existing overflow classification fires and its note already points at
`/compact` — no new path.

**(g) Notes close the door.** The two existing interruption notes and the new
`Length` note name `/continue` when the capability table says it would work,
and name `/regen` when it would not — the OSC 52 lesson: an honest description
of a dead end must still name the route that works.

**(h) Honesty about "exactly".** Text-exact, not token-exact: the provider
re-tokenizes the prompt, so the seam's token boundaries may differ from the
interrupted stream, sampling penalties see the re-tokenized prefix (same as an
uninterrupted run), and on Anthropic a whitespace-trimmed seam may drop
trailing spaces the partial ended with. The probe added a precise boundary to
this claim: a cut placed *inside* what the model would emit as a single token
(the probe's artificial "…is Par" of a one-token "Paris") is an
out-of-distribution state and the weld can go wrong (Gemma produced
"Parise."/"Pariz.", haiku-4-5 injected a U+00AD, Gemini a newline) — but a
real interruption can only ever cut at a **token boundary**, where the same
probe measured clean, exact seams on every continuing provider. One thing continuation cannot restore on a thinking model is the lost
reasoning trace: the model continues from the visible text without its
chain-of-thought, which can lower the tail's quality relative to an
uninterrupted run. The spec text should say exactly this much.

## 5. Hotkey

Full inventory (chat screen + global, from the key handlers and the F1
tables): every `Ctrl` letter is taken except `H I J M S` — of which
`Ctrl+H/I/J/M` are physically unreachable in terminals (delivered as
Backspace/Tab/LF/Enter; `keys::hotkey_char` matches `Char` only, and `Ctrl+M`
is the recorded reason mouse capture became `Ctrl+W`), leaving **`Ctrl+S`** as
the only free letter — with two caveats: XON/XOFF freezes the terminal on any
path that leaves `IXON` on (legacy conhost/ssh/tmux configs; crossterm strips
it in raw mode on the measured hosts, spec §11.7's reasoning for `Ctrl+Q`),
and VS Code binds it to Save. Function keys: F1–F5 and F10 are taken;
**F6/F7/F8 are free and unclaimed by both measured hosts** (F9/F11/F12
collide with VS Code debug / fullscreen / devtools). `Alt+<letter>` chords are
deliberately absent app-wide (unreliable on Windows emulators; journal
ui-input).

Recommendation: **command-only** (F6 fork in §6). The feature is explicitly
expected to be rare; the three safe function keys are a scarce resource better
kept for something frequent, and `Ctrl+S`'s failure mode (a frozen terminal)
is a bad trade for a rare command. If a chord is wanted anyway, `F6` — and per
the command-only-control convention the chord and `/continue` must share one
handler, with a `ctrl_pairs`/help-table row each.

## 6. Forks (settled)

> **User's decision (2026-08-27): every fork below goes to its recommended
> option — F1(a), F2(a), F3(a), F4(a), F5(a), F6(a), F7(a), F8(a), F9(a).**
> The stage-0 probe (§7.1) then confirmed F5(a) live and turned F2's cloud
> question concrete: Anthropic ≤4.5 and Gemini measured as continuing, Grok
> measured as restarting — so a later stage 2 has its rows ready.

- **F1 — what counts as continuable.**
  **(a) `metadata.finish ∈ {Cancelled, Error, Length}`, plus `None` (legacy
  messages, recorded before the field existed) — recommended.** A
  `Stop`-finished reply is refused with a note (continuing a completed reply
  is a different feature — the model would mostly emit an immediate EOS, which
  is what the repo's own probe measured on a complete-looking tail).
  (b) Any trailing assistant message — simpler, but makes `/continue` on a
  finished reply a silent no-op-shaped wart.
- **F2 — provider coverage in v1.**
  **(a) Managed + external (llama.cpp/vLLM semantics) only; clouds refuse with
  the honest note — recommended.** The motivating cases (local models,
  disconnects, small contexts) all live here; the llama.cpp path is the only
  one with a documented contract.
  (b) v1 also enables Anthropic (≤4.5 models, thinking off) and Gemini on the
  strength of the repo's own probe — more value, plus a per-model table to
  maintain and two more live smokes.
  (c) Everything incl. a Grok probe — max coverage, most surface.
  Stage 2 can lift (a)→(b) after the probe (§7) regardless.
- **F3 — degraded mode where exact continuation is impossible (OpenAI; clouds
  under F2a).**
  **(a) Refuse with a note naming `/regen` — recommended** (no silent
  quality cliff; the project's images precedent: capability is asked, never
  guessed).
  (b) Instruction-based imitation (the impersonation-seed shape: a system-line
  "continue exactly from where the draft stops", partial as trailing
  assistant) — works everywhere, visibly imperfect seams; if ever added it
  must be labeled in the note, not silent.
- **F4 — a partial that is thoughts-only (cut mid-reasoning).**
  **(a) Refuse ("the reply never started; `/regen`") — recommended.**
  (b) Continue by discarding the partial thoughts and regenerating — that *is*
  `/regen`; pointing at it is honest.
- **F5 — tools in the continuation round.**
  **(a) Include the turn's tools (probe pins that prefill + tool grammar
  coexist on llama-server) — recommended**; the continued turn stays a full
  citizen of the agentic loop.
  (b) Tool-less continuation (the `final_round` shape) — simpler, but a
  continued reply that needs one more lookup dead-ends.
- **F6 — hotkey.** **(a) None; command-only — recommended.** (b) `F6`.
  (c) `Ctrl+S` (accepting the IXON/VS-Code caveats).
- **F7 — engine/model changed since the partial.**
  **(a) Allow; the message's `metadata.model` is updated to name the model
  that wrote the larger share — practically: keep the original name unless the
  continuation outgrows the partial — recommended** as the pragmatic reading
  of "the header shows the message's own metadata snapshot" (spec §11.3).
  (b) Refuse when the engine differs — purist, and blocks the legitimate
  "continue on the bigger cloud model" move.
  (c) Allow and always overwrite with the continuing model.
- **F8 — where the continuation lands.**
  **(a) Append into the same `Message` (id kept) — recommended**; required
  for seamless mid-word resumes (§3's `"\n\n"` merge trap).
  (b) A new message with `new_bubble = false` — no orchestrator amend path,
  but the paragraph-break seam makes mid-word continuation visibly broken;
  rejected unless (a) turns out to fight the storage/FTS layer.
- **F9 — discoverability.**
  **(a) The cancelled/cut-short/length notes mention `/continue` when the
  capability table says it applies — recommended** (door-closing; text derived
  from state).
  (b) Notes unchanged; F1 help only.

## 7. MVP probe (go/no-go, before any product code)

One `#[ignore]` smoke file against the live stack (plus optional cloud keys),
the impersonation-probe pattern:

1. **Gemma (non-thinking), mid-sentence tail** — send history ending in an
   assistant message cut mid-word; assert the reply continues the word (no
   re-greeting, no restart). **Go/no-go for the whole feature.**
2. **Qwen 3.5/3.6 (thinking)** — same request: (a) plain → expect the 400
   thinking rejection (pins the failure mode); (b) with
   `chat_template_kwargs enable_thinking:false` → continues; assert any
   `<think>` residue is stripped by the thoughts parser. **Go/no-go for
   thinking-model support.**
3. **Tools + prefill** — same as 1 with the turn's tool schemas attached and a
   tail engineered so the continuation should end in a tool call; assert the
   call arrives as `tool_calls`, not prose. **Decides F5.**
4. **Explicit knobs** — repeat 1 with `continue_final_message`/
   `add_generation_prompt` set, and once with a deliberately wrong type
   (the xAI lesson: a `200` is not proof) to learn whether the server
   validates or ignores them.
5. *(If F2b/c)* **Anthropic haiku-4-5** (continues) vs **a 4.6+ model**
   (400 — pins the gate), **Gemini** mid-sentence quality, **Grok**
   trailing-assistant behaviour — one request each on the existing keys.

Cost: pennies; runs on the same env vars as the rest of the live gate.

### 7.1 Results (2026-08-27) — GO

Eight `#[ignore]` smokes in `src/shared/api/continue_probe.rs`
(branch `spike/continue-probe`), run against `llama-server` at
192.168.1.20:8000 (gemma-4-31B q4_0) plus the Anthropic, Gemini and Grok
keys — **8/8 green**; the llama-side arms re-ran the same day on **Qwen 3.6,
llama.cpp build b10659** — 8/8 again. The first run rebuilt the instrument (lessons §3): the
fixture gained an invented marker ("Per the Zorbville atlas…") because
llama.cpp's echo makes continuation and verbatim restart the same bytes
without one, and the asserted cut moved to a word boundary because a cut
inside a single token is a state no real interruption produces (§4h).

| Arm | Result |
|---|---|
| §7.1 llama.cpp, word-boundary tail | **continues exactly** — `" Paris."`, seam one space, `finish=Stop`; the prefill is **echoed** at the head of the stream (→ the §4d amendment). Re-run on Qwen 3.6/b10659: identical — with Qwen answering the fixture in-universe (`" Zorbville."`), which widened the probe's accepted completions (the marker is also a premise). Mid-word arm (recorded): welded `"Par"` into `"Pariz."`/`"Parapluie."` — the out-of-distribution state, as reasoned. |
| §7.2 Qwen 3.6 thinking (b10659) | the #21889 rejection is **gone**: a plain prefill is accepted and thinking is silently **skipped** (empty reasoning, no `<think>` residue — #21511 not reproduced), exactly the semantics a continuation wants; the `chat_template_kwargs` escape continues too, kept for older builds. |
| §7.3 prefill + tools | **GO for F5(a)** — the continued round finished its sentence and emitted a parsed `get_weather` call, `finish=ToolCalls`. Reproduced on Qwen 3.6/b10659. |
| §7.4 explicit knobs | **neither build** knows `continue_final_message` on `/v1/chat/completions` — a wrong-typed value passes as 200, knobs-off is ignored — including b10659, the day's build; default prefill alone carries the feature there, the fields ride along for vLLM. |
| §7.5 Anthropic haiku-4-5 | **continues** — `" Paris."`, no echo. |
| §7.5 Anthropic opus-4-8 | **rejects**, wording pinned: *"This model does not support assistant message prefill. The conversation must end with a user message."* |
| §7.5 Gemini 2.5-flash | **continues** — `" Paris."`, no echo. |
| §7.5 Grok 4.5 | **restarts** — no continuation semantics; the §2 row is settled as no. |

Journal entry (with the full raw outcomes): [engine.md](../journal/engine.md),
"`/continue` — stage 0: the live continuation probe".

## 8. Scope estimate

- **Stage 0 (probe)** — the §7 smokes on a `spike/` branch; results recorded
  here and in the engine journal. Go/no-go.
- **Stage 1 (`feat/continue-generation`)** — `MessageFinish` metadata +
  `Length` note; `ChatRequest.continue_final` + `OpenAiClient` body fields;
  `/continue` registry row, runner arm, eligibility + capability table;
  `handle_continue` + continuation seed through `start_generation` /
  `TurnLoop` / `finalize_message` amend path; feed re-marking + in-flight
  mirror seed; the three notes; i18n (en/ru); tests. Touches
  `entities/message.rs`, `shared/api/{contract,openai/wire}.rs`,
  `features/ui_command.rs`, `screens/chat/{commands,input,feed}.rs`,
  `app/orchestrator/{generation,mod}.rs`, `app/runtime/dispatch.rs`,
  `locales/*`. ~12 files.
- **Stage 2 (optional, after F2)** — cloud enablement per the probe:
  Anthropic model gate + trailing-whitespace trim, Gemini pass-through, Grok
  verdict; the capability table grows rows, nothing structural.
- Docs per AGENTS.md §4: spec §6.4 (from "optional" to specified behaviour) +
  §11.7 command row; architecture §5–§6 (request flow) and §10 (feed append);
  README command table; CHANGELOG (Added); journal entry in
  [engine.md](../journal/engine.md) (mechanics + smoke verdicts; the command
  row itself folds into the same entry).

## 9. Testing plan

- **Unit**: eligibility matrix (each refusal produces its note — the registry
  suites `no_command_is_a_silent_no_op` / `every_command_clears_the_box…`
  cover the plumbing for free); wire fixtures asserting the exact body the
  continuation flag adds per client (and adds *nothing* when off — byte
  compare, the images-feature discipline); the amend path (reload the chat
  file: one message, text concatenated with no separator, finish updated);
  feed (continuation streams into the existing bubble; a switch away and back
  mid-continuation rebuilds); `Length` note appears (and did not before —
  mutation-check the gate both ways, lessons §2). Fixture-driven orchestrator
  tests opt out of auto-title (`no_auto_cfg`) as the titling lesson requires.
- **Live** (mandatory, engine-touching): the §7 smokes become the permanent
  `#[ignore]` set; record model/stack + outcome in the journal entry.
- Watch the duplication gate: the new smokes share a prologue with the
  impersonation probes — budget the shared fixture at design time
  (lessons §2, "if the third test starts the same way as the first two…").

## 10. Open questions

- **External non-llama.cpp/vLLM servers**: is a probe worth it (llama.cpp is
  detectable via `/props`), or is "documented body fields + honest note" the
  right stopping point? Today's recommendation: the latter; revisit if a real
  LM Studio/Ollama user asks.
- **Gemini thinking models**: whether a continuation request should also
  suppress `thinkingConfig`, and what a 3.x thinking model does with a
  mid-sentence `model` tail — the §7.5 probe answers both.
- **TTS**: `/tts` on a continued message re-speaks the whole text — acceptable,
  or should continuation invalidate a cached narration? (Likely a non-issue;
  check while wiring.)
- **Auto-title**: a continued first reply may become "substantive" only after
  continuation; the trigger already runs per-landing and is idempotent per
  chat, so no interaction is expected — verify in the orchestrator tests.
- Whether the `Length` note should ship even if `/continue` itself is
  deferred — it closes a door that is open today and is a two-line change
  once `MessageFinish` exists.
