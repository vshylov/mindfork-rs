# The context window an Ollama server runs — read from `/api/ps`

Status: **built** (branch `feat/ollama-window`); forks decided by the owner on
2026-10-05 — F1 (a), F2 (a), F3 (a), each the recommendation (§4). The live run
is §7. The promotion
plan's Ollama recipe ([promotion.md](promotion.md) §4, stage 1) asks the user
to type the window twice — once into Ollama, once into mindfork — because the
app cannot ask Ollama for it. This document is how it can.

## 1. Why

Automatic compaction (spec §6.7) measures a conversation against the window the
engine runs, and folds it into a summary at 75 % of it. Where the app has no
window it does nothing, and a long chat ends in whatever the server does with a
prompt that does not fit. Against Ollama that is the worst outcome there is,
measured on 0.35.1 (install.md §3, [the journal](../journal/website.md)):

- **Ollama cuts an overlong prompt in silence.** 2060 tokens in a window of
  2048 were cut to 1027 from the front, the system message first, and
  answered with a `200`; the `usage` reported the cut length.
- **Its default window is small where most GPUs are**: 4096 tokens under
  24 GB of memory, 32768 up to 48 GB, 262144 above — and mindfork's first turn
  is about 3500.
- **The app learns a window from three places** (`compaction.rs::context_budget`):
  a typed `compaction.context_tokens`, a managed server's `-c`, the engine's
  own answer — llama.cpp's `/props`, then the endpoint's catalogue. Ollama
  answers neither: `/props` is a `404`, and its `/v1/models` lists names only.

So today the window is the user's to type, and the recipe says so. One that is
typed and later drifts from Ollama's — the slider moved, the server restarted
with another `OLLAMA_CONTEXT_LENGTH` — is worse than none, because a typed
number wins over everything.

## 2. What Ollama says (measured, 0.35.1 in Docker on the 4090, `gemma4:e4b`)

`GET /api/ps` — the models loaded right now — at the server's root, outside
`/v1`, with no key:

```json
{"models":[{"name":"gemma4:e4b","model":"gemma4:e4b","size":…,
  "details":{…},"expires_at":"…","size_vram":…,"context_length":16384}]}
```

| Probe | Result |
|---|---|
| a cold server, nothing loaded | `{"models":[]}` |
| after a `/v1/chat/completions` of the model, `OLLAMA_CONTEXT_LENGTH=16384` | `context_length: 16384` |
| `OLLAMA_CONTEXT_LENGTH=2048` | `2048` — the window that cut the prompt above |
| loading it on purpose, `POST /api/generate {"model":…}` with no prompt | `done_reason: "load"`, **63.2 s** on a cold disk cache, then the entry |
| another client's `/api/chat` with `options.num_ctx: 8192` | the model reloaded at `8192` |
| …then a `/v1` request of mindfork's | reloaded again at `16384` |
| a request naming `gemma4` (only `gemma4:e4b` pulled) | `404` — Ollama reads a bare name as `:latest` |

What follows from the table:

- **`/api/ps` answers only for a loaded model**, and Ollama loads on the first
  request. At the moment the app asks today — when the engine is applied, i.e.
  at the start — the model is usually not loaded and the list is empty.
- **The window an OpenAI-shaped request gets is the server's setting**, not the
  last client's: a `/v1` request reloads the model at the server's default. So
  the figure read **right after one of mindfork's own requests** is the one its
  next request will run in.
- **The name to look for is the configured one**, with Ollama's own rule for a
  bare name (`:latest`); a name not in the list is silence, as the catalogue
  does with a model it does not list
  ([external-model-name.md](external-model-name.md) §3).

## 3. Design

- **The client.** `OpenAiClient::context_budget` keeps its order — `/props`
  first, which a llama-server answers — and when that says nothing, asks
  `GET <root>/api/ps` for the configured model's `context_length`. A non-Ollama
  server answers `/api/ps` with a `404`, which is silence, like `/props` from a
  vLLM. Not on a gateway (the same guard `/props` has), not with a blank model.
- **The orchestrator.** The question is asked when the engine is applied, as
  now, and — **F1** — once more after the session's first finished turn when
  nothing answered the first time: that turn is what loaded the model.
- **What the window then does**: the same as a window from `/props` — the
  compaction trigger, the attachments' budget. A typed number still wins.

## 4. Forks

- **F1 — when to ask, since `/api/ps` answers only for a loaded model.**
  (a) at the start, and once more after the first finished turn of the session
  if nothing answered — the turn loaded the model, with mindfork's own window;
  one extra look per session at a server that never answers (vLLM, LM Studio).
  (b) load the model at the start (`/api/generate` with no prompt), then ask —
  the answer is there before the first turn, but opening the app to read an old
  chat costs a model load: 63 s measured, and the GPU's memory for five minutes.
  (c) after every turn while nothing has answered — a turn's worth of requests
  more on every server that cannot say. **Recommended: (a). Owner's decision
  2026-10-05: (a).**
- **F2 — the recipe.** (a) once a release carries this, the recipe drops
  `compaction.context_tokens`: the window is set in one place, Ollama's, and
  read from there — and a typed number that drifts from Ollama's can no longer
  win over Ollama's own; until that release the recipe keeps it, since the
  site describes the released version. (b) keep it. **Recommended: (a). Owner's
  decision 2026-10-05: (a)** — a step of the next release PR.
- **F3 — a window too small for the conversation.** Knowing the window does not
  save Ollama's default: at 4096, mindfork's 3500-token first turn is already
  over the 75 % at which compaction starts, with nothing earlier to fold — and
  the code says nothing then, on purpose ("saying it every turn would be
  nagging", `compaction.rs`). The third turn is then cut in silence.
  (a) on an **external** server, once per server session, a feed note when a
  turn ends over the threshold with nothing to fold: the window, the prompt,
  and the setting that raises it — naming Ollama's
  (`OLLAMA_CONTEXT_LENGTH`, the app's context slider) since that is the case it
  is for; (b) nothing beyond the documents. **Recommended: (a)**, in this
  branch — without it, learning the window helps exactly the users who already
  followed the recipe. **Owner's decision 2026-10-05: (a).**

## 5. Not in this track

- **A prompt Ollama already cut.** Its `usage` reports the cut length, so the
  compaction trigger reads a small conversation and never fires — a long chat
  reopened against a 4096 window stays cut. Seeing it needs the app's own
  estimate beside the server's figure (a "truncation detector"), which spec
  §6.7 S2 deliberately keeps out of the trigger. A track of its own.
- **LM Studio** reports its loaded window too (`/api/v0/models`,
  `loaded_context_length`); not run here, so not designed here.
- **The engineless chat offering a server it finds** on `localhost:11434` or
  `:1234` — the promotion plan's idea, a track of its own.

## 6. Tests and the live run

- Unit: the client reads `context_length` for the configured model from a stub
  `/api/ps` (`:latest` for a bare name; a list without the model, an empty
  list, a `404`, a body that is not JSON — all silence; `/props` answered first
  wins); the orchestrator asks once more after the first turn when nothing
  answered, and not when something did, and not twice; F3's note once per
  session, external only, and not when there was something to fold.
- Live: `console_probe.py --scenario ollama` with the recipe **without**
  `compaction.context_tokens`, against `OLLAMA_CONTEXT_LENGTH=4096` and
  `16384`: the window read (the log), and at 4096 the F3 note after the second
  turn; a llama-server's `/props` still wins (the CPU stand of the slow-prefill
  fix).

## 7. Built, and the live run (2026-10-05)

- **The client**: `OpenAiClient::context_budget` — `/props`, then
  `loaded_window` (`GET <root>/api/ps`, `LoadedModels::window_of`). **The
  orchestrator**: `ContextDiscovery::asked_after_turn`,
  `ask_window_after_turn` from `handle_done` after a turn that carried usage;
  `note_window_too_small` where the trigger finds nothing to fold, claimed once
  per chat-server session (`Engines::claim_window_note`, cleared at `Ready`
  like the slow-prefill note's).
- **Tests**: five of the client (the window read, `:latest`, five kinds of
  silence, a blank model asked nothing, `/props` first and alone), three of the
  orchestrator (asked again after the first turn; not without a reason — four
  controls, then "not before the start's answer" and "once per engine"; the
  note once per session and again after `Ready`). **3915 unit tests, 257
  `#[ignore]`** (+8).
- **Live**, `console_probe.py --scenario ollama` with `MINDFORK_OLLAMA_CONTEXT=`
  (empty — the recipe as F2 makes it, no window typed), each against a cold
  Ollama 0.35.1, `gemma4:e4b`:

  | `OLLAMA_CONTEXT_LENGTH` | the app's log | the feed |
  |---|---|---|
  | 16384 | `engine reported its context window context_budget=16384` | no note |
  | 4096 | `… context_budget=4096` | after the second turn: *The conversation already takes 3651 tokens of this server's 4096-token window …* |

  Both runs also passed the scenario's earlier checks (the reply, the tool,
  the picker, no launch line). At 4096 Ollama's log had no `truncating input
  prompt` by the end: the note came before anything was lost.
- **Owed to the next release (F2)**: the recipe — README, the site's install
  page, install.md §3 — drops `--set compaction.context_tokens=16384`.

