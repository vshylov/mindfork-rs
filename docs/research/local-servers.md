# A local Ollama or LM Studio, found and offered — and LM Studio known by name

Status: **built, both stages** — stage 1 in §8, stage 2 in §9; forks decided
by the owner on 2026-10-06 — F1 (a), F2 (a), F3 (b) with the command named
`/local`, F4 (a), F5 (b), F6 (a) (§5). Stage 1 of the promotion plan ([promotion.md](promotion.md) §4) has two open items, and
this document takes both: "the engineless chat offers a server it finds on
`localhost:11434` or `:1234`", and an LM Studio recipe, which waited for a live
run. The run is §2; it found that LM Studio works through the external mode
today, and three things that do not.

## 1. Why

The audience the one-shot channels reach — r/LocalLLaMA above all — already
runs Ollama or LM Studio. What a fresh install shows them today:

- **A chat that is not configured.** The default mode is `managed` with no
  model (`ServerMode::Managed` is `#[default]`, `shared/config.rs`), so the chat
  is `NotConfigured`: an amber *chat: not configured* chip, and an empty feed
  that lists three routes — a cloud through the settings, `mindfork llama setup`
  and a GGUF, `mindfork demo` (`ui.feed.no_engine.*`, spec §11.3). A message
  sent there is refused with "No model is connected — open the settings".
- **A server already running on the same machine is not seen.** Nothing in the
  application asks `localhost:11434` or `:1234`.
- **The Ollama recipe is a line of three `--set` keys** (README, install.md §3),
  typed by hand, model name included. LM Studio is named only in a parenthesis.

## 2. What LM Studio does (measured)

LM Studio as installed on 2026-10-06: the app reports 1.1.7 (its executable is
`Bionic.exe`), the CLI is `lms`, the runtime `llama.cpp-win-x86_64-nvidia-cuda-avx2@2.49.0`.
The model is `gemma-4-E4B-it` Q4_1 from `D:\LLM\GGUF`, imported with
`lms import --copy`, on the RTX 4090.

**The server and its endpoints.**

- The server is off until started: `lms server start`, or the app's Developer
  tab. Port `1234`.
- `GET /v1/models` lists every downloaded model, embedding models included,
  with nothing that tells one from the other.
- `GET /api/v0/models` adds `type` (`llm`, `embeddings`), `state` and, for a
  loaded model, `loaded_context_length` beside `max_context_length`.
- `GET /api/v1/models` lists `models[]` with `type`, `key`, `display_name`,
  `capabilities` and `loaded_instances[]`, each with
  `config.context_length`.
- **Every unknown path is answered `200`** with
  `{"error":"Unexpected endpoint or method. (GET /props)"}` — `/health`,
  `/props`, `/api/ps`, `/` alike. The client's `/props` reading parses that
  body into a `Props` with every field empty, so today it costs nothing: no
  window, no slot count (so no slow-prefill note), vision `Unknown`. Anything
  that identifies a server must read a field, never a status.

**Loading.**

- A request that names a downloaded model that is not loaded loads it, just in
  time: the first request took 7.6 s with the load.
- The load uses the model's default configuration. On the 4090 that was
  `context_length` **131072** — the model's maximum — with `parallel` 4 and a
  TTL of 3600 s. What a smaller GPU gets was not measured.
- A model loaded with `lms load <key> -c 4096` reports `4096` in both
  `/api/v1/models` (`loaded_instances[].config.context_length`) and
  `/api/v0/models` (`loaded_context_length`).
- A request naming a model that does not exist is **answered by the loaded
  one**, with no error.

**Streaming.** Gemma 4 reasons by default, in `reasoning_content`. The `usage`
arrives with `include_usage`, `reasoning_tokens` included. There are no
`timings`; `stats` is empty.

**A prompt over the window (loaded at 4096).** Two answers:

- **The last message does not fit beside the system message: refused.** A `400`
  that wraps llama.cpp's error in a string — `Engine protocol predict request
  returned 400: {…"type":"exceed_context_size_error","n_prompt_tokens":9732,"n_ctx":4096}`.
  The app's overflow marker matches the wrapped type, so the user is told the
  conversation no longer fits (§2, the second run below).
- **The conversation does not fit: cut in silence, from the middle.** LM Studio
  gets the same refusal from its runtime, then **asks again with the middle of
  the conversation removed**. Its log shows `send_error … exceeds the available
  context size` and a second task right after. What is kept is the system
  message, the **first** exchange and the last message.
  Measured on a conversation shaped like mindfork's — a 2 600-word system
  message, then turns of ~175 tokens, each asking to remember a token `Tn`:

  | Turns | `prompt_tokens` | The model remembers |
  |---|---|---|
  | 8 | 4044 | T0 … T7 |
  | 9 | **2809** | **T0** |
  | 12 | 2809 | T0 |

  The answer is a `200`, and its `usage` is the cut size. The `usage` counts
  the cached tokens too: one request had a prompt eval of 42 tokens and
  `prompt_tokens` of 1237. So the rule of the prompt-cut detection
  (`processed + 32 < held`,
  [prompt-cut-detection.md](prompt-cut-detection.md) §3.3) applies as it is.
  Unlike Ollama's cut, which drops the oldest messages one by one and leaves
  the prompt near the window, this one is a cliff: everything between the
  first exchange and the last message goes at once, and the same again every
  turn after.

**Today's build against it** — `console_probe.py --scenario ollama` with
`MINDFORK_OLLAMA_URL=http://localhost:1234/v1` and
`MINDFORK_OLLAMA_MODEL=google_gemma-4-e4b-it`, at a window of 4096. 12 of 13
checks passed:

- the reply, and the model's thoughts;
- a `note_save` card from the agentic loop;
- the settings' model list naming LM Studio's models — the embedding model
  among them;
- no llama-server launch line, and no round told as cut.

The one failure is the window, which the app does not read. The first turn was
3446 tokens and the second 3725. The `ollama-cut` scenario on the same server
ends in the refusal above, shown as *"The conversation no longer fits the
model's context window. Run /compact …"* with the server's reply.

## 3. What Ollama lists (measured, 0.35.1 in Docker)

- `GET /api/tags` lists the pulled models, each with `capabilities` —
  `completion` for a chat model, `embedding` for an embedder — and
  `details.context_length` (the model's maximum, not the loaded window).
- `GET /api/ps` lists the loaded ones with the window they run (already read,
  [ollama-window.md](ollama-window.md)).
- `GET /` answers `200` with `Ollama is running`.
- LM Studio's `/api/v1/models` is a `404` on Ollama.

So each server is told by a field only it sends: `models[].capabilities` on
Ollama's `/api/tags`, `models[].type` on LM Studio's `/api/v1/models`.

## 4. Design

Two stages, each its own branch and live run.

### Stage 1 — LM Studio, a server the app knows

1. **Its window.** The window chain becomes llama.cpp's `/props` → Ollama's
   `/api/ps` → LM Studio's `/api/v1/models`. The last one reads the instance of
   the configured model: an instance whose `id` is the configured name, or else
   the model's only instance. Asked when Ollama's is — at the start, and once
   more after the first turn, since a just-in-time load happens on that turn
   ([ollama-window.md](ollama-window.md) F1). Compaction then folds before LM
   Studio cuts, as it does on Ollama.
2. **The server's name in what the app says.** The window-too-small note and
   the cut note say how to raise the window and what a cut loses, and both
   name Ollama's settings today. The client learns which server it is from the
   endpoint that answered the window (llama-server, Ollama, LM Studio, or none),
   and each note gets a text per server:
   - **Ollama:** as now.
   - **LM Studio:** the *Context Length* in the model's load settings, or
     `lms load <model> -c 16384`. What a cut loses: everything between your
     first exchange and the last message.
   - **Other:** the server's own context setting, with no product named.
3. **The recipe.** An "Already running LM Studio?" paragraph beside Ollama's
   (README, the site's install page, install.md §3): start its server, and the
   same `mindfork setup` line with `http://localhost:1234/v1` and the model's
   key. Its window is LM Studio's own setting, read by the app. Like 0.16.0's
   recipe change, the README and the site change in the release PR, because
   they describe the released version.

### Stage 2 — the engineless chat offers what it finds

1. **When.**
   - **At the start,** when the chat's engine is `NotConfigured` — a managed
     mode with no model, or an external one with no URL. A configured engine
     that does not answer is `Disconnected`, and is not second-guessed. The
     environment variables that configure an engine do so before this check.
   - **By `/local`,** whatever is configured (F3): the user who started a
     server after mindfork, or who wants to switch to one. Nothing found is
     then a feed note naming what was asked — Ollama on `11434`, LM Studio on
     `1234` — and that neither answered.
2. **What is asked.** Both servers at once, in the background, each with a
   short timeout (1 s to connect, 3 s in all), at `127.0.0.1`:
   - **Ollama:** `:11434/api/tags`, models whose `capabilities` hold
     `completion`, plus `/api/ps` for which one is loaded.
   - **LM Studio:** `:1234/api/v1/models`, models of `type` `llm`, with which
     ones have `loaded_instances`.

   Each listing also yields the server's embedding models: `embedding` in
   Ollama's `capabilities`, `type` `embedding` on LM Studio's `/api/v1/models`. At the start,
   nothing found is silence, and the empty feed stays as it is.
3. **What is offered.** A list on the chat screen, titled *Local servers*, one
   row per server and chat model — *Ollama · gemma4:e4b*, *LM Studio ·
   google_gemma-4-e4b-it* — with a loaded model first.
   - **Enter** on a row writes `engine.mode = external`, `engine.external.url`
     (`http://127.0.0.1:<port>/v1`) and `engine.external.model_name` through
     the same `UpdateConfig` path the settings use. The file is saved
     atomically, and the engine comes up as for any external server.
   - **Esc** changes nothing.
4. **The embedder, from the same server (F5).** When the embedder is
   `NotConfigured` — the default — and the picked row's server has an
   embedding model, the same pick writes `embed.mode = external`, the same URL,
   and that model. The row says so (*… · embeddings: nomic-embed-text-v1.5*).
   - **Which one:** a loaded embedding model first, then `bge-m3` by name —
     the model the app is calibrated against — then the first listed.
   - **An embedder already configured is never replaced:** a different
     embedder is a different vector space, and a reindex.
   - **`embed.convention` stays `none`.** A convention is never chosen
     silently (`shared/embed_prefix.rs`), and nomic's own task prefixes
     (`search_query:` / `search_document:`) are not one the app has. The live
     run measures recall with it as it is; a convention for it is a follow-up.
5. **The recipe gets shorter.** Start Ollama's or LM Studio's server, start
   mindfork, press Enter. The `mindfork setup` line stays for a headless
   machine and for a script. The empty feed's routes gain one: a local
   Ollama or LM Studio is offered when its server runs, and `/local` looks
   again.

## 5. Forks

- **F1 — the notes' texts.** (a) a text per server — Ollama, LM Studio, other —
  chosen by the endpoint that answered the window; (b) one text that names
  both products. (b) is shorter, but it tells an LM Studio user about
  `OLLAMA_CONTEXT_LENGTH`, and describes a cut that LM Studio does not make.
  **Recommended: (a). Owner's decision 2026-10-06: (a).**
- **F2 — how the find is offered.** (a) a list, even for one candidate: one
  shape, and the row says what will be written; (b) a yes/no question for one
  candidate and a list for several. **Recommended: (a). Owner's decision
  2026-10-06: (a).**
- **F3 — when servers are looked for.** (a) at the start only. A user who starts
  LM Studio's server later restarts mindfork, and the empty feed says so.
  (b) at the start, and again by a command; (c) every few seconds while nothing
  is configured — a list could open while the user is typing in the settings.
  **Recommended: (a). Owner's decision 2026-10-06: (b)**, the command named
  `/local` after the list's title, *Local servers*. It works whatever is
  configured (§4, stage 2, item 1).
- **F4 — a refused offer.** (a) asked again at the next start while nothing is
  configured — it stops by itself once anything is; (b) a stored "do not ask
  again", one more setting. **Recommended: (a). Owner's decision 2026-10-06:
  (a).**
- **F5 — the embedder.** LM Studio ships `nomic-embed-text-v1.5` with the app,
  and Ollama serves any embedder that has been pulled. (a) the offer sets the
  chat alone; (b) also the embedder, where the same server has one. A
  different embedder later means a reindex, and nomic's English-first model is
  not the multilingual one the app defaults to. **Recommended: (a). Owner's
  decision 2026-10-06: (b)** — notes and the knowledge base search by meaning
  from the first run. Bounded as §4, stage 2, item 4 says: an embedder already
  configured is never replaced, and no convention is chosen.
- **F6 — which servers.** (a) Ollama on `11434` and LM Studio on `1234`;
  (b) also a llama-server on its default `8080`, told by `/props`. Many
  unrelated development servers sit on `8080`, and the user who starts
  llama-server by hand knows the line already. **Recommended: (a). Owner's
  decision 2026-10-06: (a).**

## 6. Not in this track

- **The settings' model list hides LM Studio's embedding models** from the chat
  slot. `/v1/models` cannot tell them apart; `/api/v1/models` could. A small
  follow-up.
- **A model name LM Studio does not know** is answered by whatever is loaded,
  with no error. The app cannot see that from the reply.
- **The overflow message's advice** — "Run /compact" — is wrong when the last
  message alone does not fit beside the system message, where folding the
  rest cannot help. This is so on every server, not just LM Studio.
- **LM Studio's default window on a smaller GPU** was not measured; the
  application reads whatever it is.

## 7. Tests and the live run

- Unit: the LM Studio window read from a stub `/api/v1/models` — the named
  instance, the only instance, several with none named, nothing loaded, the
  `200 {"error":…}` body; the server kind from the endpoint that answered; the
  notes' text per kind. Stage 2: the probe's parse of both listings
  (`completion` vs `embedding`, `llm` vs `embeddings`, loaded first), a refused
  or silent port, the config written by a pick, nothing written by `Esc`, no
  probe when the engine is configured.
- Live, by `console_probe.py`, against LM Studio and the Ollama container:
  - **`lmstudio`:** the recipe; the window read; no launch line; at 4096, the
    window-too-small note naming LM Studio's setting.
  - **`lmstudio-cut`:** a conversation grown past 4096, the cut told in LM
    Studio's words.
  - **`first-run`:** a fresh data directory with both servers up. The list
    names both; a pick writes the external section and the chat answers. A
    second run with neither up shows the empty feed unchanged.

## 8. Built: stage 1, and the live run (2026-10-06)

Branch `feat/local-servers`.

- **The window.** `OpenAiClient::context_budget` asks LM Studio's
  `/api/v1/models` after `/props` and `/api/ps`
  (`local_servers::LmStudioModels::window_of`: the instance with the
  configured name, or else the model's only one).
- **The server.** `EngineBackend::server_kind` (`Ollama`, `LmStudio`,
  `Other`) is asked by the window's background task, in `external` mode only,
  and kept in `ContextDiscovery` beside the window; `RetryBackend` delegates it.
  Ollama is told by `/api/version`, LM Studio by `/api/v1/models`, each shape
  with a required field — the `200 {"error":…}` body parses as neither.
- **The notes.** The window note and both cut notes have a text per server
  (`ui.notice.*.ollama|lm_studio|other`), chosen by `Orchestrator::for_server`.
  LM Studio's names the configured model in `lms unload <model>` then
  `lms load <model> -c 16384`. Measured while writing it: a second
  `lms load` of a loaded model starts `<model>:2`, and requests naming the
  model keep going to the first instance — so the line unloads first.
- **Tests.** 3946 unit tests, 10 new:
  - the instance chosen, and silence four ways;
  - the error body parses as neither server;
  - the window read with `/props` and `/api/ps` answering `200` with nothing;
  - each server told by its field;
  - the delegation;
  - each note in each server's words;
  - the server landing with the window and going with the engine.

**The live run.** Two new `console_probe.py` scenarios, LM Studio 1.1.7,
`gemma-4-E4B` Q4_1, the dev build:

- **`lmstudio`, at 4096: 14/14.**
  - The log: `engine reported its context window context_budget=4096
    server=LmStudio`.
  - The window-too-small note after the first turn (3446 tokens) names
    Context Length and `lms unload`, and not Ollama.
  - A `note_save` card, and no round told as cut.
- **`lmstudio-cut`, at 8192: 11/11.** A question; a page of 150 notes, which
  fits; a page of 175, which does not. LM Studio dropped the first page and
  answered "Madrid". The log says `processed=6332 held=7368`. The feed carried
  LM Studio's cut note, which names the model in the `lms` line, and folded 4
  earlier messages.
- **Ollama 0.35.1 as the control.**
  - `ollama` at 16384 and at 4096: 13/13 each, the log naming `server=Ollama`,
    and at 4096 the note naming `OLLAMA_CONTEXT_LENGTH`.
  - `ollama-cut`: 7/7, with 0.16.1's figures to the token
    (`processed=2051 held=4357`).

## 9. Built: stage 2, and the live run (2026-10-06)

Branch `feat/local-offer`, stacked on stage 1.

- **The look.** `shared::api::local_servers::find` asks both servers at once,
  at `127.0.0.1`, with no proxy, a 1 s connect and a 3 s total timeout:
  - Ollama: `/api/tags`, then `/api/ps` for what is loaded. `capabilities`
    decides chat or embedder, and the name decides on an Ollama old enough to
    send none.
  - LM Studio: `/api/v1/models`, read for `type` and loaded instances.

  `offers` makes the rows — a loaded model first, each with the embedder the
  pick brings when the embedder is free (a loaded one, then `bge-m3`, then the
  first).
- **The seam.** `ServerSupervisor::find_local_servers`, with no default:
  - the real supervisor asks the machine;
  - the demo's finds nothing;
  - the mock answers what a test gives it, and counts its looks.

  Found while wiring it: `spawn_orch(None)` starts an engineless orchestrator,
  so without the seam every such unit test would have asked this machine's
  Ollama and LM Studio — both running here.
- **The orchestrator** (`orchestrator/local_servers.rs`):
  - looks at the start when the chat is `NotConfigured`, after `bootstrap`;
  - looks for `AppCommand::FindLocalServers`, the `/local` command;
  - writes `AppCommand::UseLocalServer` through `handle_update_config`, and
    asks again at that moment whether the embedder is still free;
  - when nothing is found, the command is answered with a note — nothing
    answered, or no chat model — and the start stays silent.
- **The screen.** `widgets/local_server_picker.rs`, the `ListScroll` list:
  - `Enter` picks, `Esc` closes, and `Ctrl+Q`/`F10` punch through, since the
    list opens unasked.
  - Its key legend is its minimum width, and the small-window gate sweeps it.
    That is 21 states, and `WIDTHS` gains 36 and 37 around the English legend's
    37, so 29 × 17 = 493 sizes.
  - The empty feed names a fourth route.
- **Tests.** 3962 unit tests, 16 new:
  - the two listings, the wrong server's answer, a closed port, an Ollama
    without `capabilities`;
  - the rows and the embedder's choice;
  - the list's keys, row and legend;
  - the screen's keys and `/local`;
  - the orchestrator's five: offered at the start; asked only by the command
    when configured; the command answered; the pick written and saved; a
    configured embedder kept.

**The live run.** `console_probe.py`, the dev build, Ollama 0.35.1 in Docker
and LM Studio 1.1.7:

- **`first-run`: 15/15.** (Counted 14 when this was first written; the run printed fifteen.)
  - A fresh copy opened the list by itself. The log says `servers=2 offers=2
    asked=false`.
  - `Enter` on Ollama's row: Paris, and `settings.json` holds the Ollama URL
    and no embedder — that Ollama has none pulled.
  - `/local`, then LM Studio's row. The note named the model and nomic, and
    `settings.json` holds both external sections.
  - Rome, then a `note_save` that the new embedder indexed. The `emb` chip came
    up, and the log has no embedder error.
- **`first-run-none`: 7/7**, with both servers stopped.
  - The empty feed names `/local` and no list opens.
  - `/local` says neither 11434 nor 1234 answered.
- **The control:** `lmstudio` and `ollama` with an engine set up by the
  recipe: all checks passed, and no list opened.
