<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="artwork/mindfork-wordmark-dark.svg">
    <img src="artwork/mindfork-wordmark-light.svg" alt="mindfork-rs" width="320">
  </picture>
</h1>

[![Website](https://img.shields.io/badge/web-mindfork.io-c25a27.svg)](https://mindfork.io)
[![CI](https://github.com/vshylov/mindfork-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/vshylov/mindfork-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**A console (TUI) AI chat app in Rust.** Built for **local** models
**Gemma 3/4** and **Qwen 3.5/3.6** (via **llama.cpp `llama-server`**), but through a single
engine contract it also supports **cloud APIs**: **OpenAI**, **Google Gemini**, and
**Anthropic (Claude)** — see [ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md).
UI — on [ratatui](https://ratatui.rs). Platforms: **Windows** and **Linux**.
Architecture — **Feature-Sliced Design (FSD)**.

> The project's idea is not just "yet another LLM client," but an attempt to make local
> Gemma/Qwen **more self-aware and interesting to talk to**, by giving the model tools for
> self-reflection: read and change its own system message and sampling
> parameters, keep notes about the user, spin up a short-lived sub-agent
> for a "second opinion." Details are in [spec.md](spec.md) (and in the original
> requirements doc [docs/history/request.md](docs/history/request.md)).

---

## Features

**Chat and interface**
- Full chat cycle in the terminal: response **streaming**, generation interruption,
  server state in the status bar.
- **Token counter** in the status bar (conversation + response as one number): grows
  as generation proceeds, visible right from the start. Before the server responds,
  the conversation is shown as an estimate (`~`), then replaced with the exact number
  from `usage` (`stream_options.include_usage`).
- **Chat list** (`Esc`, full-screen): search by title, two sort orders
  (by creation / modification date), rename (`F2`), new/clone/delete,
  model-generated **auto-title** (`Ctrl+R` in the list window).
- **Copy the conversation to the clipboard** (`F5`) — both in the main window (the
  active chat) and in the chat list (the selected one); cross-platform via `arboard`.
- **Message feed** with its own **Markdown** renderer (theme, code highlighting,
  **GFM tables** with smart column layout, **Mermaid diagrams** as text-mode
  graphics — flowchart/sequence, with a hard fallback to the source and a settings
  toggle) and **Unicode approximation of LaTeX**
  inside `$…$`/`\(…\)` (Greek letters, arrows, operators, fractions, roots,
  functions `\log`/`\sin`/…, subscripts/superscripts — no rasterization, friendly to
  a JupyterLab terminal), a collapsible **"thoughts" (CoT)** block (`Ctrl+T`), and
  **tool blocks** (tool call name/arguments/result).
- A custom **multiline input box** (`Shift+Enter` — line break, `Enter` — send) with
  word wrap, precise cursor positioning, **fast multiline paste from the clipboard**
  (`Ctrl+V` — line breaks are kept as text, not sent), **clear input with undo**
  (`Ctrl+K` — delete all text, repeat — restore it), and an **emoji picker popup**
  (`Ctrl+B` — inserts at the cursor position, remembers the last pick).
- **Regenerate** the last response (`Ctrl+R`) and **delete the last exchange**
  (`Ctrl+E`, the user's text is returned to the input box).
- **Impersonation** (`Ctrl+U`): the model writes the next message **on the user's
  behalf** — a streaming preview replaces the input box, and on completion the text
  is inserted into the input (`Esc` cancels). Non-empty input is used as a seed — the
  model continues what's already there. Separate server and sampling: `shared` (the
  same server as the assistant) / `managed` / `external` / cloud (`openai` /
  `gemini` / `claude`).
- **Live spellcheck** for Russian and English (Hunspell): error underlines,
  a suggestions popup (`Ctrl+G`), a personal dictionary.
- **Themes** auto / dark / light, a help/"About" dialog (`F1` / `?`) with tabs:
  about (author/version/links), hotkeys, commands, license, components.
- **Interface language** (Russian / English, the "Interface language" field in the
  "Interface" section): the whole UI chrome — status bar, feed, settings, help, the
  self-model screen, errors. **Independent of the agents' language** (each profile
  has its own scaffold language), applies live.
- **Old-terminal compatibility mode** (a toggle in settings, "Interface" section):
  for Windows 10 conhost and other emulators without emoji support — safe glyphs
  (WGL4/ASCII) instead of emoji and rare characters, straight borders instead of
  rounded ones, an ASCII spinner, popup background dimming by color instead of `DIM`.
- **Feed scrolling** with the mouse wheel or `PageUp`/`PageDown`. Mouse capture is a
  **toggle** (`Ctrl+W`): off by default (native mouse text selection works), turns on
  for wheel scrolling (selection then needs `Shift`).
- **Layout-independent** Ctrl shortcuts — on Windows they work under any installed
  keyboard layout (Cyrillic, Greek, Hebrew, …); elsewhere, under Cyrillic.

**AI companion profiles**
- Unique id, system message, an optional **greeting** (the model starts the
  conversation first), role names, tool set, sampling defaults.
- **Custom names for the user and the assistant** ("Persona" group): a set name
  replaces the feed's role header (in caps — `GAIA` instead of `YOU`) and the label
  in the `F5` conversation export (`Gaia:` instead of `User:`). Empty by default —
  then the labels follow the interface language. They apply to existing chats too
  (resolved from the profile at render time), so they can be changed at any time.
- **Agent-scaffold language** (the "Scaffold language" field in the Persona
  subsection): the language of background-task prompts, the self-model scaffold, and
  tool results — text that the *model* reads (not the language of its replies, which
  is set by the system message, and not the interface language). Chosen when the
  profile is created and **locked** once the profile has data (chats / self-model /
  notes). The default profile is Russian; for an agent in another language, create a
  new profile and choose the language before the first chat. Ru/En (Tier 1,
  [docs/history/i18n.md](docs/history/i18n.md)).
- **Notes and RAG isolation by `profile_id`** — different companions' memories don't
  mix.
- Three-tier sampling resolution: `Chat.sampling_override → Profile.default →
  global`.
- **Rich sampling** on top of the standard OpenAI fields — llama.cpp extensions right
  in the request body: `min_p`, `top_n_sigma`, `typical_p`, penalties
  (`repeat_penalty`, DRY, XTC), `mirostat`, `seed`. For **livelier and more
  unpredictable** replies: **dynamic temperature** (`dynatemp_range`/`_exponent` —
  temperature adapts to per-token entropy), **adaptive-p** (`adaptive_target`/
  `_decay`), **DRY breakers** (`dry_sequence_breakers`), and a configurable
  **sampler order** (`samplers`). Every field is optional and sent only when set.

**Tools (client-side agentic loop)**
- **Introspection:** `get/set_sampling`, `get/set_system_message`,
  `get_last_user_message_time` — the model can change its own behavior mid-conversation.
- **Memory:** `note_save` / `note_recall`, **RAG** `rag_add` / `rag_search`
  (chunking + embeddings + kNN via sqlite-vec). Chunking follows best practices:
  splitting on sentence/word boundaries with **overlap**, small paragraphs get
  grouped; markdown is split **semantically** (by headings, protecting code blocks).
  On search, neighboring chunks are **stitched** back together via the overlap —
  saves context and doesn't confuse the model with a repeat.
- **Attaching files to a chat:** `/file attach <path>` attaches a text file to the
  current chat, `/file remove <name|#N>` takes it away, `/file list` shows what is
  attached. The file's text is passed to the model with **every** message of that
  chat, so it can be asked about at any point — and removing it genuinely takes it
  out of what the model sees. Unlike `/rag add`, this is chat-scoped, needs no
  embedding server, and delivers the file **in full** rather than as
  search-matched fragments. Attachable: any text file (source code, configs,
  logs), plus `.html`/`.pdf`/`.docx`. The content is snapshotted when attached, so
  the conversation stays coherent even if the file later changes. A file above the
  budget isn't refused — it is attached "by reference" (name, size and the
  beginning). Budgets live in settings ("Memory" → "Attachments"); the status bar
  shows a `§ files: N (~tokens)` chip, since attachments cost tokens every turn.
- **Loading files into RAG from the input box:** `/rag add <path> [-r]` commands
  (indexes a file or directory, recursively with the flag; currently `*.txt`/`*.md`)
  and `/rag remove <path>` (removes a file/directory from the store). Indexing runs
  in the background, with a progress indicator and spinner; re-adding a file
  **replaces** its chunks (no duplicates). Command input is highlighted yellow and
  skipped by spellcheck. **`/rag list`** shows the store's sources (chunk count,
  date), **`/rag rebuild`** reindexes the store — after changing chunk size/overlap
  (configurable in the "Tools" section) or switching to an embedding model with a
  different dimensionality. Sources' original text is stored in the DB, so
  reindexing doesn't need the source files on disk.
- **Reading messages aloud (`/tts`):** `/tts` reads the last message, `/tts N` — the
  last N, `/tts all` — the whole conversation, `/tts stop` — stops it,
  `/tts pause`/`/tts resume` — pause and resume (handy for long text). Code,
  ` ```mermaid ` diagrams, tables, and formulas are **skipped with a short spoken
  note**, and "thoughts" and tool calls aren't read at all. The provider is
  configured separately from the chat (the "Text-to-speech" tab in the "Model"
  section): the OpenAI cloud (default, `gpt-4o-mini-tts`/`onyx`), the Gemini cloud,
  or any third-party OpenAI-compatible TTS server; the cloud key is shared with chat.
  You can set a **separate user voice** — then `/tts all` reads the user's and the
  assistant's lines in different voices. Playback runs as a pipeline (the next chunk
  is synthesized while the current one plays); no sound card — a clear note, not a
  crash.
- **`call_subagent(system_message, message)`** — an independent single-turn request
  with no history, tools, or recursion; a model-driven "second opinion."
- **Web search** `web_search` — an in-house implementation with multi-provider
  fallback (DuckDuckGo lite/html → Mojeek → Ecosia) and anti-bot throttling
  detection. By default it fetches result pages, extracts readable text, and
  re-ranks them by relevance to the query (via embeddings); turned off with the
  `fetch_content` argument or the `config.tools.web_fetch_content` setting.
- **`fetch_url(url, focus?, summarize?)`** — fetch a page and summarize it with the
  model (like `call_subagent`); `focus` steers the summary, `summarize=false` returns
  the extracted text without the model. Gated by the web-access switch.
- **`python_exec`** — runs Python in an **isolated Wasmer/WASIX sandbox** (by
  default: no access to the host's files, network gated by a toggle, preinstalled
  **numpy / pandas / requests** packages, no Python needed on the host) or in the
  **local interpreter** (the previous behavior). The tool itself is **off by
  default**. Interrupted on timeout (process kill), a "one task at a time" gate, an
  optional memory limit (Windows). The sandbox is installed with one command,
  `mindfork sandbox setup`; see
  [ADR 0005](docs/decisions/0005-python-sandbox-wasmer.md).
- **Files** `fs_read` / `fs_write` / `fs_list` — read/write/list local files (**off
  by default**, like Python). An optional `fs_root` "sandbox" restricts access to a
  given directory (escaping via `..` is blocked).
- **`calculate(expression)`** — a math-expression evaluator (its own parser:
  arithmetic, powers, parentheses, constants, and functions). No network/disk access.
- **`current_time(format?)`** — the current date/time (local zone + UTC; `format` is
  a `strftime` string). No network/disk access.
- **Response-structure control** (optional, off by default):
  **`send_followup_message`** — the assistant finishes the current message, calls the
  tool, and writes a **second reply** as a separate bubble right after the first;
  **`rewrite_current_message`** — if it realizes mid-way that the answer is wrong, it
  discards the reply so far and writes it again (the discarded text is kept for
  manual recovery). Enabled with toggles in profile settings. Verified against live
  Gemma 4 and Qwen 3.6.
- **Plugins: MCP-server tools** — plugs in any tools from the **Model Context
  Protocol** ecosystem (files, git, GitHub, databases, browser, …) as external stdio
  servers: servers are described in `settings.json` (the `mcp` section; command +
  args + env variable names — secrets aren't written to the file), and their tools
  show up in the profile toggles under a "Plugins (MCP)" group. **Double opt-in** (a
  master switch, off by default, plus a per-profile toggle), **TOFU catalog pinning**
  (a change to a server's tool set/descriptions requires re-confirmation —
  protection against tampering), full tool descriptions are visible in settings, as
  are server statuses; result clipping, per-call timeouts, cancel with Esc. See
  [docs/install.md §4.2](docs/install.md) and
  [ADR 0007](docs/decisions/0007-plugins-mcp-host-import-format.md).
- **Global switches**: the effective set = `profile ∩ globally enabled` (web gates
  `web_search`+`fetch_url`, Python gates `python_exec`, file access gates `fs_*`, MCP
  tools are gated by the `mcp.enabled` master switch); `calculate`/`current_time` are
  safe and always available.

**Other**
- **Settings screen** (`Ctrl+P`): model/server, inference, sampling, profiles,
  tools, interface. The "Model/server", "Sampling", and "Profiles" sections have
  **"Assistant"/"Impersonation"** subsections. Changing the model **restarts** the
  managed server on the fly. The managed server lets you configure
  **FlashAttention** (`--flash-attn`) and **speculative decoding** (`--spec-type`,
  including `draft-mtp` for MTP models — a multiplier speed boost).
- **Python sandbox**: `mindfork sandbox setup` — installs an isolated WASIX
  environment (downloads `wasmer` + `python.webc` + numpy/pandas/requests packages
  from a lock list with sha256 verification) for the sandbox mode of the
  `python_exec` tool.
- **Import** from other applications: `mindfork import <file>` — a documented,
  neutral [mindfork-import](docs/import-format.md) format (JSON); an external
  converter reads the source application's format and emits the file, and import is
  idempotent (re-running it doesn't create duplicates).
- **Backup/restore**: `mindfork backup [-o FILE] [-c 0..9]` and
  `mindfork restore <archive>` (a zip with configurable compression; restore is
  transactional — a pre-restore copy of the prior data and a rollback on failure).
- **Portability and install-time defaults**: by default all data lives in a `data/`
  subdirectory next to the binary (the folder/USB stick is self-contained); a
  `defaults.json` file can move it to the standard OS folder or an arbitrary
  directory **and** set the agent-scaffold language for new profiles
  (`default_language`, ru/en — the installer fills it in based on the user's choice
  during setup). Atomic writes + `.bak`, soft delete with cascade.

---

## Architecture

The inference engine sits behind the **`EngineBackend`** trait (`shared/api/contract`),
and the app is an HTTP client to it; embedding the model (an rlib with candle/CUDA) is
deliberately **not used**. Several providers are supported behind one contract
([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)):
- **a local OpenAI-compatible server** — managed `llama-server` (the app spawns the
  process itself) or any external one (vLLM / LM Studio / Ollama), `OpenAiClient`
  (Chat Completions, sampling is sent as-is);
- **the OpenAI cloud** — `ResponsesClient` (Responses API `/v1/responses`, a Bearer
  key): reasoning summaries ("thoughts"), `reasoning.effort`, `text.verbosity`;
  reasoning is resent along with a tool call (the reasoning item);
- **the Google Gemini cloud** — a native `GeminiClient` (`generateContent`,
  `x-goog-api-key`): "thoughts" summaries (`thinkingConfig.includeThoughts`), depth
  via `thinkingLevel` (3.x) / `thinkingBudget` (2.5), `top_k`; the thought signature
  (`thoughtSignature`) is resent on a tool call (required for Gemini 3);
- **the Anthropic (Claude) cloud** — `AnthropicClient` (Messages API
  `/v1/messages`, `x-api-key`) with **extended thinking (CoT)**: adaptive thinking +
  visible "thoughts", correct even with tool calls (resending the thinking block with
  its signature within the same turn).

The provider is chosen in settings with a single mode selector (`managed` /
`external` / `openai` / `gemini` / `claude`). **The API key is entered right in
settings** (the "API key" field): it's stored **encrypted and tied to this machine**
(Windows — DPAPI, Linux — a key derived from `machine-id`), so the settings file can
be moved between machines — on a new one the key is entered again, and back on the
original it's read again. A stored key is never shown or copied; input is masked,
`Del` deletes it. The key is shared across chat, impersonation, and embeddings for
one provider. An alternative for CI and advanced setups is the "API key (env)" field
with an **environment variable name** (used if no key is entered). The layers above
`EngineBackend` (orchestrator, agentic loop, tools, UI) don't depend on the provider.

The engine configuration is **nested**: each mode/provider has its own settings
sub-section (`managed` / `external` / `openai` / `gemini` / `claude`), so several can
stay configured at once and you can **switch without losing anything**. The settings
screen shows only the fields for the selected mode, and in the cloud it hides
sampling parameters the provider doesn't accept (their values are kept for local
models). This applies to the assistant engine, the impersonation server, and
embeddings alike.

> **Why not xinfer.** The project was originally designed around the lightweight
> `xinfer` library, but it turned out to be too raw for Gemma 4 (incoherent output,
> builds poorly on Windows). The working backend is **llama.cpp `llama-server`**
> (OpenAI protocol, `--jinja`). Launching and the protocol are in
> [docs/install.md §3](docs/install.md).

Key decisions:
- **The agentic loop is client-side** (in the orchestrator): stream → tool calls →
  execution → a new request, up to `max_tool_rounds`. Tools return a result +
  effects; the orchestrator — the sole owner of `Chat` — applies effects without
  locks.
- **A unidirectional UI↔orchestrator flow**: `AppCommand`s go up, `AppEvent`s go
  down; `generation_id` drops stale chunks; an `Idle / Generating / Cancelling` state
  machine.
- **EOS** by token id on the server (the `stop` field isn't sent — an
  anti-self-cutoff measure); "thoughts" from `delta.reasoning_content` with a
  fallback to parsing `<think>` (llama.cpp); for Claude — `thinking_delta` + a
  signature; for OpenAI Responses — `reasoning.summary` + a reasoning item; for
  Gemini — parts with `thought:true` + `thoughtSignature` on the call (all so the
  round trip stays correct with tool use).
- **Storage**: JSON (config/profiles/chats) + SQLite/sqlite-vec (notes/RAG),
  isolation by `profile_id`, soft delete everywhere.
- **Embeddings** — a dedicated embedding server
  ([ADR 0002](docs/decisions/0002-embeddings-dedicated-server.md)).

### Structure (FSD, dependencies strictly downward)

`app → screens → widgets → features → entities → shared`

| Layer | Contents |
|---|---|
| `src/app/` | orchestrator, events, TUI loop, server supervisor, tokio↔UI bridge |
| `src/screens/` | screens: `chat`, `settings` (emit `*Intent`, unaware of `app`) |
| `src/widgets/` | `message_feed`, `input_box`, `chat_list`, `profile_list`, `status_bar` |
| `src/features/` | `tools/*`, `spellcheck/*`, `profiles`, `chat_search_sort`, `rename_chat`, `migration` |
| `src/entities/` | domain types: `chat`, `message`, `profile`, `note`, `rag`, `sampling` |
| `src/shared/` | `api` (`contract` + implementations `openai` (Chat Completions + `responses/`), `gemini`, `anthropic`, `managed` behind the `EngineBackend` trait), `storage`, `config`, `paths`, `theme`, `markdown`, `wrap`, `keys`, `logging`, … |

UI crates were chosen for ratatui 0.30
([ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md)): `tui-scrollview`, input —
a **custom widget** (gives full control over `Shift+Enter` vs `Enter`, scrolling, and
spellcheck underlines). Markdown — a **custom renderer** on top of `pulldown-cmark`
(tables + delimiter-scoped LaTeX + theming,
[ADR 0003](docs/decisions/0003-own-markdown-renderer.md)).

---

## Build and run

Requires **Rust** (edition 2024, a recent stable toolchain).

```bash
cargo build --release          # binary in target/release/
cargo run                      # dev run (needs a REAL terminal)
```

> The TUI requires a real TTY. In a headless environment the app "hangs" — that's expected.

### Connecting a model (llama.cpp)

Example of external mode — start `llama-server` manually and point the app at the URL:

```bash
# terminal 1: inference server (--jinja is required for the Gemma format and tool-calling)
llama-server -m google_gemma-4-E4B-it-Q4_1.gguf \
  --host 0.0.0.0 --port 8000 -ngl 99 -c 8192 --jinja
```

```powershell
# terminal 2 (PowerShell): tell the app where to connect
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
cargo run
```

In **managed mode** the app launches `llama-server` itself (the binary path + GGUF
are set on the settings screen `Ctrl+P` or via env vars):

```powershell
$env:MINDFORK_LLAMA_BIN = "C:\path\to\llama-server.exe"
$env:MINDFORK_MODEL     = "C:\GGUF\google_gemma-4-E4B-it-Q4_1.gguf"
$env:MINDFORK_NGL       = "99"     # GPU layers (opt.)
$env:MINDFORK_CTX       = "8192"   # context (opt.)
$env:MINDFORK_PORT      = "8000"   # opt.
```

If the GGUF file isn't found or isn't accessible in managed mode, the app
**immediately** shows a clear error in the status bar (instead of hanging on "server:
connecting…" until the load timeout) — the model's presence is checked before
`llama-server` is even launched.

In managed mode the settings screen exposes **FlashAttention** (`--flash-attn`:
`auto`/`on`/`off`) and **speculative decoding** (`--spec-type`) — it speeds up
generation by drafting ahead. `draft-*` types need a separate draft model (`-md` +
`-ngld`/`--spec-draft-n-max`/`-n-min`); for **MTP models** (e.g.
`mtp-gemma-4-12B-it.gguf` as the draft for a regular `gemma-4-12B-it`) — `draft-mtp`;
`ngram-*` types don't need a separate model. The draft fields in the UI are visible
only for `draft-*`.

Embeddings for RAG use a **dedicated** server: `MINDFORK_EMBED_URL` (external) or
`MINDFORK_EMBED_BIN` / `_MODEL` / `_PORT` (managed). Not configured → RAG returns a
clear error, everything else keeps working.

Full instructions (modes, dictionaries, import, data paths) are in
**[docs/install.md](docs/install.md)**.

### Spellcheck dictionaries

Hunspell pairs `*.aff` + `*.dic` (`en_US`, `en_GB`, `ru_RU`) go into `dictionaries/`
at the project root; `build.rs` copies them next to the binary during the build. No
directory → spellcheck is simply off (doesn't crash). The dictionaries themselves
**aren't part of the repository** (`.gitignore`).

---

## Hotkeys

| Key | Action |
|---|---|
| `Enter` / `Shift+Enter` | send / line break (`Alt+Enter` — the same break, for terminals without the kitty protocol) |
| `Shift+←/→/↑/↓`, `Ctrl+A` | select text / select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | copy / cut selection · paste from clipboard |
| `Esc` | chat list (open/close) · cancel generation |
| `Ctrl+Q` / `F10` | quit (including from the chat list) |
| `F1` / `?` | help/"About" dialog (tabs: about · hotkeys · commands · license · components; `Tab`/`←→` — switch tabs) |
| `Ctrl+P` | settings screen |
| in the chat list (`Esc`) | search, sort orders, `F2`/`Ctrl+N`/`Ctrl+D`/`Del`, `Ctrl+R` auto-title, `F5` copy |
| `F5` | copy the chat conversation to the clipboard (active / selected in the list) |
| `Ctrl+N` | new chat (with a profile picker) |
| `Ctrl+R` | regenerate the last response |
| `Ctrl+E` | delete the last exchange (text → back into the input box) |
| `Ctrl+U` | write a message on the user's behalf (impersonation) |
| `Ctrl+K` | clear all input text (undo with `Ctrl+Z`) |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo an input-box edit |
| `Ctrl+T` | collapse/expand "thoughts" |
| `Ctrl+G` | spellcheck suggestions |
| `Ctrl+B` | emoji picker popup (inserts at the cursor position) |
| `Ctrl+W` | toggle: mouse wheel ↔ text selection |
| click / drag with the mouse in the input box | cursor / text selection (with `Ctrl+W` capture on) |
| `PageUp` / `PageDown` / mouse wheel | scroll the feed |
| `/file attach <path>` | attach a text file to the chat (an input-box command) |
| `/file remove <name\|#N>` · `/file list` | remove an attachment / show what is attached |
| `/rag add <path> [-r]` | index a file/directory into RAG (an input-box command) |
| `/rag remove <path>` | remove a file/directory from RAG (an input-box command) |
| `/tts` · `/tts N` · `/tts all` | read the last message aloud / the last N / the whole conversation |
| `/tts stop` · `pause` · `resume` | stop / pause / resume reading aloud |

Ctrl shortcuts are layout-independent: on Windows they are resolved through the
keyboard layout itself, so any installed layout works (Cyrillic, Greek, Hebrew, …);
on Linux — under Cyrillic, plus whatever the terminal itself falls back to (the VTE
family — GNOME Terminal & co. — handles every layout on its own).

> **The mouse wheel and text selection share one terminal mechanism** (mouse
> reporting), so capture is a toggle (`Ctrl+W`). Off (default): the mouse selects
> text as usual. On: the wheel scrolls the feed, and text can be selected by holding
> `Shift` (Windows Terminal and most terminals support this). The current mode is
> shown in the status bar.

---

## Development

```bash
cargo test                                 # unit tests (no server needed; 971 green)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Smoke tests against a live model are marked `#[ignore]` and run manually (they're
silently skipped without the required env var):

```powershell
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"   # chat server (URL includes /v1)
$env:MINDFORK_EMBED_URL  = "http://127.0.0.1:8001/v1"   # embedder (memory/RAG smokes)
cargo test -- --ignored --nocapture --test-threads=1
```

They cover streaming/finish, anti-self-cutoff on the EOS text (`<|im_end|>` for Qwen
and `<end_of_turn>` for Gemma), tool-calling, "thoughts", sampling extensions, and
control tools, plus end-to-end self-model/notes/narrative tests (gates, the graph,
reflection, consolidation, cross-organ links). **25 `#[ignore]` smokes ran green
against Gemma 4 31B + bge-m3** (`llama-server`, ~370s).

**Conventions:** Rust edition 2024; `anyhow` in application layers, `thiserror` in
`shared` library modules; logs go only to a file (`logs/`, stdout is taken by the
TUI); tests live next to the code (`#[cfg(test)]`); comments and documentation are in
English. Dependencies flow strictly downward through the FSD layers. Before
committing: `cargo fmt`, `cargo clippy`, `cargo test` — everything green.

---

## Documentation

- **[CLAUDE.md](CLAUDE.md)** — a quick project orientation and status (M0–M9 done).
- **[spec.md](spec.md)** — the full engineering spec (source of truth).
- **[docs/install.md](docs/install.md)** — install, run, the engine (llama.cpp), the
  OpenAI-compatible protocol, dictionaries, import.
- **[docs/decisions/](docs/decisions/)** — ADRs (UI crates, the embedding server, the
  markdown renderer, the engine contract and multi-provider inference, the Python
  sandbox on Wasmer/WASIX).
- **[docs/history/](docs/history/)** — archive: the original requirements doc
  ([request.md](docs/history/request.md)) and the completed M0–M9 plan
  ([plan.md](docs/history/plan.md)).

## Status

The entire **M0–M9** plan is implemented, plus extensive post-M9 work. The chat
cycle, isolated profiles, tools with a client-side agentic loop, a sub-agent, web
search and Python behind switches, the settings screen, import from LameLLaMA,
themes, live spellcheck, loading files into RAG with `/rag add|remove` commands
(smart chunking with overlap + semantic markdown + stitching on retrieval),
**user impersonation** (`Ctrl+U`, shared/managed/external), the **self-model /
user-model** screen (`F3`), and **notes connectivity** (semantic recall, the link
graph, superseding with a "scar," auto-"sleep," narrative as notes). An **isolated
Python sandbox**
([ADR 0005](docs/decisions/0005-python-sandbox-wasmer.md)): `python_exec` isolated
in WASIX via a `wasmer` sidecar (numpy/pandas/requests, network behind a toggle, a
memory limit on Windows), installed with `mindfork sandbox setup`.
**Multi-provider inference**
([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)): local llama.cpp
plus the **OpenAI (Responses API) / Google Gemini (native `generateContent`) /
Anthropic (Claude)** clouds behind a single contract, each with reasoning/"thoughts"
summaries and a correct round trip on tool use. **971 unit tests** green;
`#[ignore]` smokes ran against the live **Gemma 4 31B + bge-m3** stack
(`llama-server`), **Claude 4.x** against the live Anthropic API, **Gemini 3.1 Pro**
against the live Gemini API with a key, and the **Python sandbox** against a live
`wasmer` (Windows).
