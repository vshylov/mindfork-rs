# A prompt the server cut in silence — telling it from the `usage`

Status: **built** (branch `feat/prompt-cut-detection`); forks decided by the
owner on 2026-10-05 — F1 (a), F2 (a), each the recommendation (§5). The live
run is §8. Follows
[ollama-window.md](ollama-window.md) §5, which left this out of that track:
*Ollama's `usage` reports the cut length, so the compaction trigger reads a small
conversation and never fires.*

## 1. Why

A server that cannot hold a prompt should refuse it, and the ones this project
was built on do: llama-server answers `400 exceed_context_size_error` before the
stream starts, and the app's failure names `/compact` (spec §6.7, *when it is
already too late*). **Ollama answers `200` and cuts.** The window it cuts to is
its own setting, 4096 tokens by default under 24 GB of GPU memory, against a
first turn of mindfork's of about 3500. What the cut costs, and what the app
sees of it, was measured (§2) — and the measurement corrects what this project
has said so far, that Ollama cuts "from the start, the system message first"
(install.md §3, the window-too-small note): that is one of two behaviours, and
not the common one.

## 2. What Ollama does (measured, 0.35.1 in Docker on the 4090, `gemma4:e4b`)

`OLLAMA_DEBUG=1`, `OLLAMA_CONTEXT_LENGTH=2048`, `/v1/chat/completions`, a system
message holding a code word (`ZARYA-8823`) and an instruction to give it when
asked; the last user message asks for it.

| Request | `usage.prompt_tokens` | Ollama's log | Reply |
|---|---|---|---|
| 12 exchanges, 1797 tokens | 1797 | — | `ZARYA-8823` |
| 14 exchanges, ~2087 tokens | **2017** | `truncating input messages which exceed context length` (debug) | `ZARYA-8823` |
| the same with `"truncate": false` in the body | 2017 | the same | `ZARYA-8823` |
| the same through the native `/api/chat` with `"truncate": false` | — | — | **`400`** `exceed_context_size_error`, `n_prompt_tokens: 2087`, `n_ctx: 2048` |
| system + 2 exchanges + a last message of ~2656 tokens | **1027** | `truncating input prompt limit=1027 prompt=2656 keep=5 new=1027` (warn) | `lantern` |
| the same, last message ~3967 tokens | **1027** | `… prompt=3967 keep=5 new=1027` | `willow stone meadow engine` |

So Ollama has **two cuts**, one after the other:

- **The messages.** The oldest messages are dropped whole until the rest fits;
  the system message and the last message are always kept. The model keeps its
  instructions and loses the beginning of the conversation, and the `usage` lands
  **just under the window** (2017 of 2048).
- **The prompt.** When the system message and the last message alone do not fit,
  the rendered prompt is cut from the front to **half the window** — the first
  5 tokens kept, then the end (`limit=1027` for 2048, whatever the size). The
  instructions go first, and the model answers without them: the code word
  became filler words.
- **Nothing on the OpenAI-shaped surface can stop it**: `/v1` takes no
  `truncate` (the field is ignored). Only Ollama's own `/api/chat` refuses —
  a different wire format (NDJSON, its own tool and image shapes); moving the
  client to it is a client of its own, not this track (§6).

What follows for the app:

- **After the first cut the trigger works already, if the window is known.** A
  `usage` just under the window is over 75 % of it, so compaction folds the
  conversation on the next landing — the turn that was cut was answered without
  its beginning, but it says nothing new about the window.
- **After the second it does not.** Half the window is under the threshold: the
  trigger reads a half-empty window and folds nothing, the window-too-small note
  (ollama-window.md F3) waits for a turn over 75 %, and the reply was made
  without the instructions. This is the case the app must see.

## 3. What the app can know

The server's figure is exact and wrong; the app's own figure is right and
inexact. A cut is told only where a **lower bound** of what was sent — one the
app can defend — exceeds what the server says it processed. Two bounds:

### 3.1 The previous request's exact size

Within a turn a round's request only grows: the next round is the previous one
plus the assistant's calls and the tools' results (`tool_round`), with the same
system message and tools. So **a round that reports fewer tokens than the round
before it was cut** — certain, with the server's own numbers on both sides.

Across turns the next request mostly extends the previous turn's last one
(the final reply and the new message are appended), but not always: the system
message carries the time and the observations chosen for this turn (Tier 2,
`inject_self_model`), and an edit, a regeneration or a landed summary rewrite
the history. So the previous request is kept as a **fingerprint** — the
model's name, a hash of the tools, the system text, a hash and a byte length per
message — and the next request's bound is:

```text
exact(previous)
  − bytes of the previous messages after the longest common prefix (+16 a message)
  − bytes of the previous system text between the common head and tail
  + floor(what the new request adds in their place)
```

Bytes are a hard upper bound on tokens (a byte-level tokenizer spends at most
one token a byte), so what was removed is never under-counted. A different model
or a different tool set gives no bound at all; a removed message that carried an
image gives none either (its tokens have no byte bound). An edit near the start
removes nearly everything and the bound falls to nothing — silence, never a
false cut.

### 3.2 The request's own text

With nothing to extend — the first request of a session, a long chat reopened
against a small window — the bound is the request's own text. The status bar's
estimate, UTF-8 bytes / 4, cannot be it: measured against Ollama's count, it is
over the real figure on some text by more than three times:

| Text (12 KB samples) | estimate / actual | floor / actual |
|---|---|---|
| English prose (the README) | 0.89 | 0.44 |
| Russian prose (`ru.json`'s strings) | 1.43 | 0.71 |
| Russian prose (the Russian demo) | 1.16 | 0.57 |
| Rust code | 0.93 | 0.43 |
| JSON (`en.json`) | 0.92 | 0.45 |
| Chinese | 1.27 | 0.64 |
| columns aligned with spaces | **2.28** | 0.20 |
| rule lines of `=` and `-` | **3.36** | 0.04 |
| a table of contents with dot leaders | 1.34 | 0.16 |
| Python, deep indentation | 0.57 | 0.23 |
| a markdown table with long dashes, HTML, CSV, logs, base64, hex | 0.25–0.46 | 0.12–0.23 |

A tokenizer merges a run of one character, so the **floor** counts a run's first
two characters and drops the rest, then divides by 8 — half the estimate's four
bytes a token. On every sample it stays under 0.71 of the real figure; the
largest the estimate has been measured over elsewhere is Russian prose on
another tokenizer, 1.68 (history-compression.md §9a M9), which the floor's
factor of two still covers. **The tool schemas are left out**: the server renders
them in its own format, measured only on llama-server (roll-usage-calibration.md),
and they are most of a turn's prompt — so this bound is weak at a small window,
which is why §3.1 carries the common case.

### 3.3 The rule

A request was cut when `processed + 32 < max(bound 3.1, bound 3.2)` — 32 tokens
for what a template adds or drops around a message. The bound of a cut request
still holds for the next one (what it reported is under what it held), so the
fingerprint is kept with the reported figure either way. Detection runs for an
**external** server only: llama-server and vLLM refuse an overlong prompt, the
clouds refuse it, and Anthropic counts cached input apart from `input_tokens`,
which would read as a cut.

What it catches: the prompt cut lands at half the window, so any earlier request
of the chat over half of it — at 4096, mindfork's own first turn of about 3500 —
bounds the next one far above the cut figure, and a pasted page after it is
caught by §3.1. The message cut lands within a message of the window and is
caught only when it removed more than the bound's slack — it is the cut §2 says
the trigger already handles.

## 4. What the app does with a cut

- **The log**, every time: the processed figure and the bound.
- **The trigger**: a request the server cut was over its window, whatever the
  window is and whatever the figure says. So the turn counts as over the
  compaction threshold — a fold starts if there is anything to fold, **even with
  no window known** (the first turn against Ollama, before `/api/ps` has it).
  Spec §6.7's S2 stands: the trigger still reads no estimate as a size; a cut is
  a fact the bound establishes, and the fold's size comes from `tail_tokens`.
- **The session budget's calibration** does not record a cut round's ratio
  (`record_usage`): it would price the next request by a figure the server did
  not process.
- **A note in the feed** (F1, F2), naming the server's setting — and the
  window-too-small note of the same turn is not said, since its figure would be
  the cut one.
- **The correction** of what was said before: install.md §3 and the
  window-too-small note describe both cuts.

## 5. Forks

- **F1 — what the app does when it sees a cut.**
  (a) the note, and the turn counts as over the compaction threshold: a fold
  starts when there is something to fold, and the note then says to ask again
  (`/regen`) once it lands; (b) the note only; (c) (a), and the turn is asked
  again by itself once the fold lands — the user does not act, but every cut
  costs a second reply and a reply the user may already be reading is replaced.
  **Recommended: (a). Owner's decision 2026-10-05: (a).**
- **F2 — how often the note.** (a) once per server session, like the
  slow-prefill and window-too-small notes — a window too small for the
  conversation cuts every turn, and a note every turn is nagging; the log keeps
  every cut; (b) after every cut turn — each reply made without part of the
  conversation is marked. **Recommended: (a). Owner's decision 2026-10-05: (a).**

## 6. Not in this track

- **Asking Ollama to refuse** (`/api/chat` with `truncate: false`): a client for
  Ollama's own API. It would turn the silent cut into the refusal llama-server
  gives, and the app already answers that one.
- **The roll's own prompt**: a summary of a long range against a small window
  can be cut the same way, and its usage is not read for this.
- **OpenRouter's `middle-out`** compresses a prompt for a model with a small
  window; the `openrouter` mode is not covered.
- **A server that reports only the uncached part** as `prompt_tokens` would read
  as cut on every cached turn. Ollama, llama-server and vLLM report the whole
  prompt (measured on Ollama: `prompt_tokens` 1797 with 1491 cached).

## 7. Tests and the live run

- Unit: the floor (runs, scripts, the factor); the fingerprint's bound (an
  extension; the system's middle replaced; a regeneration; an edit; another
  model; other tools; an image in what was removed); the rule and its
  tolerance; in the loop, a round reporting less than the one before is a cut,
  the final round without tools is not, a managed or cloud turn detects nothing
  and a cut round records no calibration; across turns, the orchestrator keeps
  the fingerprint per chat and drops it with the engine; the note once per
  session, the fold without a known window, the window note not said in a cut
  turn.
- Live: `console_probe.py`, Ollama at `OLLAMA_CONTEXT_LENGTH=4096`: a short
  turn, then a pasted page — the note in the feed and the log's bound; and the
  existing `ollama` scenario at 16384 as the control, with no note and the
  bound under the processed figure on every round.

## 8. Built, and the live run (2026-10-05)

- **The parts**: `shared::tokens::floor_text` (§3.2); `orchestrator/prompt_cut.rs`
  — `RequestShape` (the model, a hash of the tools, the system text, a hash and
  the bounds of each message), `PromptAnchor` (a shape with the size the server
  reported), `PromptCut::judge` (§3.3); `TurnLoop::stream` judges each round of
  an external server and keeps a cut round out of `record_usage`;
  `TurnUsage::cut` carries the turn's first cut; `ContextDiscovery` keeps each
  chat's last request for the engine generation that measured it;
  `maybe_auto_compact` counts a cut turn as over the threshold and says whether
  a fold is under way; `note_prompt_cut`, claimed once per server session
  (`Engines::claim_cut_note`).
- **Tests**: three of the floor, ten of the bound and the rule, eight of the
  orchestrator — a round reported short of the one before (the cut, the
  calibration kept, the note), a managed server not judged, the round limit's
  final round without tools not a cut, the previous turn bounding the next
  (2051 after 3500 a cut, 3600 not), the fold with no window known and its
  note, the note once per session, the window note waiting out a cut turn, a
  kept request forgotten with the engine. **3936 unit tests, 257 `#[ignore]`**
  (+21). Eight mutants, one per part, each caught by its test.
- **Live**, `console_probe.py`, Ollama 0.35.1 in Docker, `gemma4:e4b`, the
  recipe with no window typed:

  | Run | Ollama's log | The app |
  |---|---|---|
  | `ollama-cut`, 4096: a question, then a pasted page of 6 KB | `truncating input prompt limit=2051 prompt=5210 keep=5 new=2051` | `processed=2051 held=4357`; the feed: *The server cut this request … of at least 4357 tokens it processed 2051 …* |
  | `ollama`, 16384 (the control) | no truncation | no round told as cut; 13 checks |
  | `ollama`, 4096 | no truncation (3718, 3583, 3645 processed) | the window-too-small note; no round told as cut |

  The bound held 84 % of what the request really carried (4357 of 5210), and
  the cut figure was half of what the app knew the chat's previous request
  to be. The reply to the cut request was still *Paris*: the question sat at
  the end of the page, which the cut keeps — the instructions were what went.
