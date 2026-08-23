# mindfork-rs Specification

Engineering specification for **mindfork-rs** — a console (TUI) AI chat application in Rust for live, meaningful conversation with local **Gemma** and **Qwen** models.

This is **attempt #2**. Attempt #1 (`lamellama-rs`, a `gpui` + `gpui-component` GUI, `mistral.rs` engine) ran into two roadblocks: the rawness of `gpui` (building a genuinely nice interface is very labor-intensive) and weak Gemma 4 support in `mistral.rs`. So that path was shelved, and instead a **console application** is built on `ratatui`, with inference through a local OpenAI-compatible server (**llama.cpp `llama-server`**; `xinfer` was the original plan — see the historical note above).

The document is derived from the `lamellama-rs` specification (`D:\Projects\lamellama-rs\next_version_spec.md`): the domain model, profiles, per-profile memory isolation, state-ownership discipline, EOS handling, the tool roster, and the testing strategy **carry over as concepts**. The UI layer (`ratatui` instead of `gpui`) and the inference layer (a local OpenAI-compatible server instead of the embedded `mistral.rs` SDK) change, which entails a number of architectural consequences fixed below.

The document's purpose is to serve as the implementation baseline; it records the decisions made and the open questions.

> ⚠️ **Historical note on the engine.** The specification was originally written for
> the **`xinfer`** library, but it turned out too raw (incoherent output on Gemma 4,
> builds poorly on Windows). The project switched to **llama.cpp `llama-server`**; in
> external mode **any OpenAI-compatible server** works (vLLM, LM Studio,
> Ollama, …). The transport is the same OpenAI protocol, so the entire design below
> (streaming, sampling, EOS, "thoughts", client-side agentic loop, tool contract)
> remains valid and has already been brought up to date with the current engine. Specifics: the managed server is
> `llama-server` (`-m`/`-ngl`/`-c`/`--jinja`/`--reasoning-format`), the client is
> `OpenAiClient`, the supervisor is `LlamaSupervisor`, the settings are `EngineSettings`.
> Startup — in [docs/install.md §3](docs/install.md); see decision #1 in [§16.1](#161-accepted-decisions).

---

## Table of Contents

1. [Overview, goals, and differences from attempt #1](#1-overview-goals-and-differences-from-attempt-1)
2. [Technology stack and dependencies](#2-technology-stack-and-dependencies)
3. [Supported models and inference engine](#3-supported-models-and-inference-engine)
4. [Solution architecture (FSD-inspired)](#4-solution-architecture-fsd-inspired)
5. [Domain model and data storage](#5-domain-model-and-data-storage)
6. [Inference engine and the client-side agentic loop](#6-inference-engine-and-the-client-side-agentic-loop)
7. [EOS and stop-token handling](#7-eos-and-stop-token-handling)
8. [Sampling parameters](#8-sampling-parameters)
9. [Tool system](#9-tool-system)
10. [AI-companion profiles](#10-ai-companion-profiles)
11. [TUI: interface and interaction](#11-tui-interface-and-interaction)
12. [Configuration, portability, migration](#12-configuration-portability-migration)
13. [Security](#13-security)
14. [Testing strategy](#14-testing-strategy)
15. [Implementation stages and readiness criteria](#15-implementation-stages-and-readiness-criteria)
16. [Accepted decisions and open questions](#16-accepted-decisions-and-open-questions)
17. [Self-model (SelfModel)](#17-self-model-selfmodel)

---

## 1. Overview, goals, and differences from attempt #1

### 1.1. Vision

`mindfork-rs` is a cross-platform (**Windows, Linux**) console application for talking to local LLMs from the **Gemma** (3, 4) and **Qwen** (3.5, 3.6) families. The key idea carries over from attempt #1: instead of broad, shallow model support — **narrow but high-quality** support for two families, plus a set of **tools** that make the assistant smarter, more self-aware, and "more alive": RAG, notes, Python execution, web search (DuckDuckGo), introspection (reading/changing its own system message and sampling parameters, a sense of time) and — most importantly — `call_subagent` (spinning up a temporary sub-agent for a second opinion).

Why TUI: the interface ends up fast, predictable, and portable; the application can run in an ordinary terminal and in a **JupyterLab** terminal. That's why markdown rendering is **text-only** (no rendering to images), with **unicode approximation of LaTeX** (arrows and simple formulas stay readable). Mermaid diagrams aren't required yet.

### 1.2. Goals

1. **Quality support for Gemma 3/4 and Qwen 3.5/3.6** — correct chat templates, tokenization, reliable generation stopping, resistance to "self-truncation" on EOS-token text.
2. **Console UI on `ratatui`** — chat list, message feed with markdown, multiline input/editing, ru/en spellcheck.
3. **Inference through a local OpenAI-compatible server** (llama.cpp `llama-server`), which the application connects to over HTTP.
4. **Extensible tool system**, executed by a **client-side agentic loop** (RAG, notes, Python, web, introspection, `call_subagent`).
5. **AI-companion profiles** with a unique identifier; notes and RAG data are stored **separately per profile**.
6. **Increasing response "self-awareness"** through `call_subagent` and the assistant independently changing its own system message.

### 1.3. Differences from attempt #1 (`lamellama-rs`)

| Aspect | lamellama-rs (attempt #1) | mindfork-rs (attempt #2) |
|---|---|---|
| UI framework | `gpui` + `gpui-component` (GUI) | `ratatui` + `crossterm` (TUI) |
| Input/editing | `Input`/`TextInput` gpui-component | **own** multiline widget ([ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md)) |
| Markdown | gpui-component markdown rendering (incl. graphics) | **own** renderer on `pulldown-cmark` (text only) + unicode-LaTeX ([ADR 0003](docs/decisions/0003-own-markdown-renderer.md)) |
| Spellcheck | `spellbook` + gpui-component `diagnostics_mut` | `spellbook` + own TUI rendering/popup |
| Inference engine | `mistralrs` (embedded Rust SDK) | llama.cpp `llama-server` (external OpenAI-compatible **server**) |
| Tool-calling | mistral.rs **server-side** agentic loop (Rust callbacks in-process) | **client-side** agentic loop in the orchestrator |
| Web search / Python | built-in mistral.rs tools | **our own** implementations (reqwest+DDG, subprocess) |
| Embeddings (RAG) | mistral.rs `EmbeddingModelBuilder` (in-process) | `/v1/embeddings` on a **dedicated** embedding server (ADR 0002) |
| Architectural style | Cargo workspace, Clean Architecture (`llama-*`) | **FSD-inspired** modular structure (see [section 4](#4-solution-architecture-fsd-inspired)) |
| Platforms | desktop (gpui) | Windows + Linux (terminal, incl. JupyterLab) |

### 1.4. What carries over unchanged from attempt #1 (as concepts)

- Domain concepts: a chat as a list of messages; `System`/`User`/`Assistant`/`Tool` roles; a "thoughts" (CoT) field; a per-message Markdown mode; a snapshot of generation parameters in `Message.metadata`.
- The `Profile` entity with a `Uuid`, its own system message, display role names, a greeting, and a set of enabled tools.
- **Notes/RAG isolation by `profile_id`** (a storage invariant).
- **State-ownership discipline**: the orchestrator is the sole owner of `Chat`; tools read an immutable snapshot and never mutate the chat directly; a unidirectional UI ↔ backend data flow; `generation_id` guards against stale streaming events; the `Idle/Generating/Cancelling` state machine.
- EOS handling **by token id**, not by substring; no string stop sequences by default.
- The tool roster and `call_subagent` semantics (second opinion, no nesting allowed).
- Chat operations: create, rename, clone, (soft-)delete, regenerate the last response, delete the last message (with the text returned to the input box), edit a message in place.
- Per-profile sampling override priority (`Chat.sampling_override` → `Profile.default_sampling` → global default).
- Soft delete (`is_hidden`), portable storage next to the binary, a backup on save, single-instance enforcement, spellcheck (en_US, en_GB, ru_RU).

### 1.5. What is dropped / not carried over

- GUI (`gpui`/`gpui-component`) and graphical markdown rendering (LaTeX/mermaid to images).
- ~~User impersonation~~ — absent in attempt #1 and not originally planned here either, but **later implemented** in mindfork-rs (`Ctrl+U`, see [§11.8](#118-impersonation-writing-a-message-as-the-user)).
- An on-disk KV cache (we rely on the inference server's prefix caching).
- LoRA in the first version.
- The engine's built-in web/Python tools (we implement our own, since the inference server is external).

---

## 2. Technology stack and dependencies

### 2.1. Base stack

- **Language**: Rust (edition 2024). The skeleton already exists (`cargo new`, `mindfork-rs`).
- **Async runtime**: `tokio` (HTTP client to the inference server, background inference/tool tasks, managing the inference server child process).
- **TUI**: `ratatui` + `crossterm` (terminal backend; cross-platform, Windows + Linux).
- **Inference**: a **local OpenAI-compatible server** (`llama.cpp llama-server`; `/v1/...`); the application talks to it over HTTP. See [section 3](#3-supported-models-and-inference-engine).

### 2.2. Supporting crates (preliminary list)

| Purpose | Crate | Note |
|---|---|---|
| Multiline input/editor | **own widget** (`widgets/input_box.rs`) | The input box and in-place message editing. Off-the-shelf `tui-textarea`/`ratatui-textarea` are incompatible with ratatui 0.30 and don't support highlighting arbitrary ranges → [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md). |
| Markdown in the TUI | **own renderer** (`pulldown-cmark` + `syntect`) | Markdown with tables, code highlighting, and theming; `ratatui-markdown`/`tui-markdown` didn't fit (no tables/math, ignores the theme) → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md). Feed scrolling — `tui-scrollview`. |
| Word segmentation | `unicode-segmentation` | Splitting into words for spellcheck (Unicode word boundaries) and for unicode-LaTeX. |
| Spellchecker | `spellbook` | Hunspell-compatible, pure Rust. |
| HTTP client to the LLM | `async-openai` / `reqwest` | OpenAI-compatible client; extended for the server's non-standard fields (`reasoning_content`/`reasoning_effort`) via `serde`. In practice `reqwest` + our own types (`OpenAiClient`) were chosen. |
| HTTP (web tool) | `reqwest` | DuckDuckGo web search and page fetching. |
| Page content extraction | `scraper` / a `readability` crate | "Readable" web page text for the web tool (exact crate — at implementation time). |
| Serialization | `serde`, `serde_json` | Config and chats in JSON; tool schemas; request bodies. |
| Identifiers | `uuid` (v4) | `Profile.id`, `Chat.id`, `Message.id`, `generation_id`. |
| Date/time | `chrono` | Timestamps; the time tool. |
| DB (notes/RAG) | `rusqlite` + `sqlite-vec` | Embedded vector storage. |
| Errors | `anyhow` (application), `thiserror` (internal library modules) | |
| Logging | `tracing` + `tracing-subscriber` | Logging to a file next to the binary (stdout is occupied by the TUI). |
| Async streams | `futures` / `tokio-stream` | Streaming chunks from the inference server into the UI. |
| Cancellation | `tokio_util::sync::CancellationToken` | Stopping generation. |
| Single instance | `single-instance` (or a lock file/named mutex) | Enforcing a single running instance. |

> Exact versions are pinned via `cargo add` at implementation time. LaTeX→unicode (see [11.4](#114-markdown-cot-and-tool-blocks-latex)) is implemented via a custom substitution table or a lightweight crate — to be decided during implementation.

### 2.3. Optional features

- **GPU/CUDA** — pertains to building/running the **inference server** (`llama-server`), not our application: our application is an HTTP client and has no dependency on the ML stack. This is a significant simplification compared to attempt #1 (where `mistralrs` was pulled into the process with a CUDA build). Building/installing the server itself with CUDA/Metal is the user's/installer's responsibility.

---

## 3. Supported models and inference engine

### 3.1. Scope of model support

Target families (per requirements — **mandatory**):

- **Qwen** — 3.5, 3.6 (dense and MoE variants).
- **Gemma** — 3, 4 (including multimodal checkpoints — only text chat is used).

llama.cpp natively supports both families (as well as LLaMa, Mistral, GLM4, DeepSeek, Phi, and others — anything available as GGUF). Other models are "bonus" support, only if they work without extra effort on our application's part (it's model-agnostic at the OpenAI-API level). Narrowing to Gemma/Qwen concerns **verification quality** (templates, EOS, tool-calling, "thoughts"), not a hard ban on other models.

Formats and quantization are **entirely the inference server's responsibility**: GGUF (all quantization types), for llama.cpp — the model's built-in chat template (`--jinja` flag). Our application doesn't implement any of that; it only passes the user the server-launch parameters (see [3.4](#34-managing-the-server-lifecycle)).

### 3.2. The engine as a local server, not an embedded SDK

**Key architectural decision.** The engine is a **standalone HTTP server** (`llama.cpp llama-server`; default port `8000`) that serves an **OpenAI-compatible API** (`/v1/chat/completions`, `/v1/embeddings`, tokenizer endpoints). Embedding the model into our process (an rlib with candle/CUDA) is deliberately **not used** — that would bring back exactly the Windows build complexity the project is escaping. The supported path is precisely a server (CLI `llama-server -m <model.gguf> -ngl <n> --jinja`).

So `mindfork-rs`:

- **launches/uses the inference server locally** and talks to it over HTTP (`http://127.0.0.1:<port>/v1`);
- **runs the tool agentic loop on the client side** (in our orchestrator), not on the server. This differs from attempt #1, where `mistral.rs` executed Rust tool callbacks inside its own process.

Advantages of the decision:

- **Clean separation**: the application doesn't pull the ML stack (candle/CUDA) into its binary; a fast build, simple cross-platform support, GPU-free CI.
- **Full control over the agentic loop**: needed for stateful tools with profile/chat context (`profile_id`, a chat snapshot, effects sent back to the orchestrator), for `call_subagent`, and for memory isolation — all of these are awkward to implement as external MCP servers.
- **Model-source flexibility**: it's possible to connect to an already-running server (e.g. on another machine/port), and to any OpenAI-compatible one (vLLM/LM Studio/Ollama).

Drawbacks and how they're offset:

- HTTP/local streaming overhead — negligible on localhost.
- The inference server has to be installed separately (or shipped alongside) — addressed by configuring the binary path and a "connect to a running server" mode.
- `mistral.rs`'s built-in tools (web/Python) aren't available — **we implement our own** (see [section 9](#9-tool-system)).

> **Note:** the engine layer (`shared/api`) is hidden behind an `EngineBackend` trait, so the transport can be swapped (a different server, theoretically an in-process API) without touching the orchestrator — our agentic loop is already client-side.

### 3.3. Chat templates, "thoughts", tool-calling

- **Chat templates** are applied by the inference server (for GGUF — the model file's built-in chat template, `--jinja` flag). We pass messages structurally (`messages` with `system`/`user`/`assistant`/`tool` roles); formatting happens on the server.
- **"Thoughts" (CoT)**: for Qwen3/Gemma these are extracted either from a dedicated streaming field (`reasoning_content`; enabled in `llama-server` via `--reasoning-format`), or by parsing `<think>…</think>` markup out of the main stream. The "thoughts" parser lives in the engine layer during streaming (it can stitch tags split across a chunk boundary). See [6.5](#65-parsing-thoughts-cot).
- **Tool-calling**: the server supports OpenAI-compatible tool-calling (structured `tool_calls` in the response) and constrained/structured outputs (JSON Schema). We pass `tools` (OpenAI schemas) and `tool_choice`, get back `tool_calls`, **execute them ourselves**, and continue the dialogue (see [6.3](#63-client-side-agentic-loop)).

### 3.4. Managing the server lifecycle

Two modes (chosen in settings):

1. **Managed (default)** — the application launches a `llama-server` child process itself with the necessary arguments (`-m` GGUF, `-ngl`, `-c`, `--jinja`, `--port`, `--no-mmap`, etc. from the model settings), waits for readiness (polling `/health`), connects. On application exit — cleanly stops the child process. Changing the model = restarting the server. Model-load progress is shown in the UI (parsing the server's stdout/stderr into a log panel — an analog of attempt #1's "View Init Logs"). **Preflight:** before launching, the model file's presence is checked — `spawn` of the child process succeeds even without the GGUF (it only fails if the binary itself is missing), and the server would then die during loading while the readiness probe waited pointlessly until timeout; so a missing/inaccessible file immediately yields `Disconnected` with a clear message instead of hanging in "connecting…". **Early exit:** if the file is valid but the process dies *during* loading (a corrupt GGUF, out of memory), the child-process monitor task raises an `exited` signal that the readiness probe watches — it stops polling immediately with a clear error, instead of waiting for the timeout.
2. **External** — connect to an already-running server (URL/port in settings; any OpenAI-compatible one); the application doesn't manage its lifecycle.

Parameters:

- Path to the `llama-server` binary (managed mode) — in settings; by default it's looked up in `PATH` and next to the application binary.
- One loaded model instance, **one generation context at a time** (the target hardware is a consumer PC; parallel generations aren't needed).

---

## 4. Solution architecture (FSD-inspired)

### 4.1. Choice of style

Per requirements, **Feature-Sliced Design (FSD)** is preferred — it's convenient for development by AI agents thanks to explicit, narrow "slices" with one-directional dependencies. FSD is a frontend methodology, so here it's applied **in adapted form**: the layers organize **modules inside a single binary crate** (a simple build matters for iterative agent-driven development), and heavy/independent parts can later be split out into workspace crates without changing the boundaries.

### 4.2. Layers (top to bottom; dependencies point only downward)

```
src/
├─ main.rs                  # thin entry point: terminal init, tokio, single-instance, app::run()
├─ app/                     # composition: the TUI event loop, screen routing, DI, the session orchestrator
│  ├─ orchestrator.rs       #   owner of the domain state (SessionState), the generation state machine, the agentic loop
│  ├─ events.rs             #   AppCommand (UI→orchestrator), AppEvent (orchestrator→UI)
│  └─ runtime.rs            #   the tokio ↔ TUI-loop bridge (channels), inference-server management
├─ screens/                 # (FSD "pages") whole screens
│  ├─ chat.rs               #   the chat screen
│  └─ settings.rs           #   the settings screen
├─ widgets/                 # (FSD "widgets") composite UI blocks
│  ├─ chat_list.rs          #   the chat list: search, sorting, renaming
│  ├─ message_feed.rs       #   the message feed: markdown, thoughts, tool blocks, scrolling, word wrap
│  ├─ input_box.rs          #   our own multiline input (ADR 0001): word wrap, cursor, spellcheck indication
│  ├─ status_bar.rs         #   model/tokens/profile/server state
│  └─ dialogs.rs            #   modals: confirmations, profile picker, spellcheck-suggestion popup
├─ features/                # (FSD "features") user scenarios (one action each)
│  ├─ send_message.rs       │  regenerate.rs        │  delete_last.rs
│  ├─ edit_message.rs       │  chat_search_sort.rs  │  rename_chat.rs
│  ├─ spellcheck/           #   segmentation, checking, suggestions, personal dictionary
│  └─ tools/                #   the registry and tool implementations (client-side)
│     ├─ registry.rs        │  rag.rs   │  notes.rs   │  python.rs   │  web.rs
│     ├─ introspection.rs   │  subagent.rs
├─ entities/                # (FSD "entities") domain types (no I/O)
│  ├─ profile.rs  │ chat.rs │ message.rs │ note.rs │ rag.rs │ sampling.rs
└─ shared/                  # (FSD "shared") infrastructure and utilities
   ├─ api/                  #   the engine: the OpenAI client (the EngineBackend trait + HTTP implementation)
   ├─ storage/               #   repositories: JSON (config/chats/profiles) + SQLite (notes/RAG)
   ├─ config.rs             #   settings.json, schema versioning
   ├─ markdown.rs            #   markdown rendering + unicode-LaTeX approximation
   ├─ wrap.rs                #   column-based word wrap (unicode-width) for the feed/input
   ├─ keymap.rs  │ theme.rs │ paths.rs │ error.rs
```

FSD dependency rule: `app → screens → widgets → features → entities → shared`. A layer never imports "sideways" or "upward". Cross-cutting infrastructure (the engine, storage) lives in `shared` and is exposed to the orchestrator/features via traits.

> Note: the "engine" and "storage" are backend cores living in `shared/api` and `shared/storage`. Pure FSD doesn't have such layers; this is a deliberate adaptation here. If desired, they can easily be split out into separate workspace crates (`mindfork-engine`, `mindfork-storage`, `mindfork-spell`) — the boundaries allow it.

### 4.3. Mapping to attempt #1's layers

| lamellama-rs (crate) | mindfork-rs (layer/module) |
|---|---|
| `llama-core` | `entities/` |
| `llama-app` (orchestration) | `app/orchestrator.rs` + `features/` |
| `llama-engine` (`mistralrs`) | `shared/api/` (`OpenAiClient`) |
| `llama-tools` | `features/tools/` |
| `llama-storage` | `shared/storage/` |
| `llama-spell` | `features/spellcheck/` |
| `llama-ui` (`gpui`) | `screens/` + `widgets/` |

### 4.4. Execution flows and UI ↔ backend state

The discipline from attempt #1 carries over (it's UI-agnostic), but **the bridge is simpler**: `ratatui` has no executor of its own — there's one rendering thread and an ordinary `tokio` runtime.

1. **Single source of truth** — the orchestrator (`app/orchestrator.rs`): it owns `SessionState` (chats, profiles, generation statuses), and is the sole writer to `shared/storage`. The UI holds a read-only projection, updated only by `AppEvent`s.
2. **Unidirectional flow**: UI → orchestrator sends `AppCommand` (SendMessage, Cancel, RegenerateLast, DeleteLast, EditMessage, SwitchChat, RenameChat, DeleteChat, …); orchestrator → UI sends `AppEvent` (ChatUpdated, GenerationStarted/Chunk/Thoughts/ToolProgress/Finished/Cancelled, Error, ServerStatus, …).
3. **Command serialization**: the orchestrator is a single `tokio` task with an `mpsc` queue; commands are processed strictly sequentially.
4. **`generation_id`**: each generation run gets a `Uuid`; every streaming event carries `(chat_id, generation_id)`; events with a stale id are dropped (the classic "Stop → immediately Regenerate → the old stream's tail arrives late" race is closed). Per-chat state machine: `Idle → Generating{id} → Cancelling → Idle`.
5. **State-based command validation** (defense in depth): the UI blocks disallowed actions based on status, and the orchestrator additionally rejects inapplicable commands.

#### 4.4.1. The TUI loop and asynchrony

```
            ┌──────────────── main thread (render loop) ─────────────────┐
crossterm   │  poll input → AppCommand (mpsc → orchestrator)             │
  events ──▶│  drain AppEvent (mpsc ← orchestrator) → update view-model  │
            │  ratatui draw(view-model)                                  │
            └────────────────────────────────────────────────────────────┘
                         ▲ AppEvent                     │ AppCommand
                         │                              ▼
            ┌──────────────────── tokio runtime ────────────────────────┐
            │  orchestrator task: state machine, agentic-loop            │
            │   ├─ EngineBackend (HTTP stream to the server /v1/chat/...) │
            │   ├─ tools (rag/notes/python/web/introspection/subagent)   │
            │   └─ storage (JSON + SQLite)                                │
            │  llama-server child-process supervisor (managed mode)      │
            └────────────────────────────────────────────────────────────┘
```

- The render loop **is never blocked**: input is polled with a timeout (`crossterm::event::poll`), events are drained non-blockingly (`try_recv`). Rendering is **change-driven** (a `dirty` flag): a frame is produced only when an orchestrator event was applied, a terminal event arrived (input/mouse/resize), a dictionary reloaded, or spellcheck highlighting was recomputed. Nothing is redrawn while idle — otherwise `ratatui` would reposition the cursor (`frame.set_cursor_position`) on every tick, and terminals (especially Windows Terminal) reset the blink phase on every move → the cursor would blink more often and unevenly. The loop body still runs every tick regardless (the `poll` timeout), which is what wakes up debounced deferred actions (the spellcheck recheck): they run inside the loop, and `dirty` is only raised when the result actually changed. There are no timer-driven animations in the rendering.
- A frame is applied **atomically** — synchronized output (DEC private mode 2026): `run_loop` wraps `terminal.draw` in `BeginSynchronizedUpdate`/`EndSynchronizedUpdate` (CSI `?2026h`/`?2026l`); the terminal buffers everything in between and shows the frame as a whole. Without this the hardware cursor would "jump" during streaming/animations: ratatui writes the frame diff while the cursor is visible (the terminal cursor is the write position) and only afterward, in separate writes, moves it back to the input box (the backend's `show_cursor`/`set_cursor_position` are `execute!` calls with an immediate flush; a large diff is also chunked by stdout's small buffer), so the terminal's asynchronous rendering could show the cursor on the diff's last-written cell — the token counter during generation (the status bar at the bottom is written last) or the RAG banner's spinner. Support: Windows Terminal ≥ 1.18 and all modern emulators; terminals without support (conhost compat mode) ignore the unknown private mode — a graceful degradation (the jump remains, as before). Draw errors are propagated after the mode is lifted; `?2026l` is duplicated in the panic hook and on exit (a panic inside `draw` won't leave a frame frozen). Residual behavior: `ratatui` unconditionally sends `show`+`MoveTo` every frame, and WT resets the blink phase on every move — during an active stream the cursor in the input box looks "solid" (not blinking); this is prior behavior, with the jumps gone.
- Inference and tools run in `tokio` tasks; only owned values (clones/snapshots) reach the UI — there are no memory races by construction (`Send` bounds on the channels).
- Cancellation — a `CancellationToken` that aborts the HTTP stream; the partial response is captured.

#### 4.4.2. `Chat` ownership and tools (a simplification vs. attempt #1)

Because the **agentic loop is client-side** and is run by the orchestrator itself (sequentially, between HTTP rounds), the elaborate deadlock protection from attempt #1 (the `ChatEffect` channel, the ban on reentrant locks during server callbacks) **is simplified**:

- Tools have no access to `Chat`. They receive an **immutable snapshot** `ToolContext` (see [9.2](#92-the-tool-contract)) and return `ToolOutcome { result: String, effects: Vec<ChatEffect> }`.
- The orchestrator invokes the tool with `await`, gets the result, **applies the effects to `Chat` itself** outside any locks (it's the sole owner), and continues the loop. The `ChatEffect` channel isn't needed — effects are returned by value.
- The deadlock-freedom property is trivially preserved: no shared mutable locks on `Chat` exist; the cycle "engine → tool → chat lock" is impossible.

---

## 5. Domain model and data storage

### 5.1. Core entities (`entities/`)

Conceptual definitions (exact field names — at implementation time); types are `serde`-serializable. Identical to attempt #1 (section 5 of its specification):

```rust
pub enum MessageRole { System, User, Assistant, Tool }

pub struct Message {
    pub id: Uuid,
    pub role: MessageRole,
    pub text: String,
    pub thoughts: Option<String>,           // the reasoning (CoT) block
    pub tool_calls: Vec<ToolCallRecord>,    // tool calls in this message (for collapsible blocks)
    pub timestamp: DateTime<Utc>,
    pub is_markdown: bool,
    pub metadata: Option<MessageMetadata>,  // a snapshot of the applied generation parameters
    pub tool_call_id: Option<String>,       // for the Tool role
    pub tool_name: Option<String>,
}

pub struct ToolCallRecord {                 // for rendering collapsible tool blocks
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
    pub subagent: Option<Box<SubagentRun>>, // a `call_subagent` call's transcript (§9.3.2); additive
}

// A sub-agent run — a chat-shaped transcript that belongs to the call that made it
// (§9.3.2, ADR 0010). `messages` holds the run's rounds exactly as a chat stores them.
pub struct SubagentRun {
    pub id: Uuid,                           // for chat:// references, the list, search
    pub kind: RunKind,                      // Subagent (a director-led dialogue is the planned second kind)
    pub title: String, pub renamed_manually: bool,
    pub name: Option<String>,               // the persona's display name (the call's `name`)
    pub created_at: DateTime<Utc>, pub finished_at: Option<DateTime<Utc>>,
    pub system_message: String, pub sampling_override: Option<SamplingConfig>,
    pub messages: Vec<Message>,
    pub outcome: Option<RunOutcome>,        // Completed | Cancelled | TimedOut | Failed | RoundLimit
    pub tokens: u64,
}

pub struct Chat {
    pub v: u32,                             // the file's schema version (§12.2); absent in pre-2 files → 1
    pub id: Uuid,
    pub profile_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub system_message: String,             // the chat's active system message (the assistant can change it)
    pub character_names: CharacterNames,
    pub messages: Vec<Message>,
    pub sampling_override: Option<SamplingConfig>,
    pub is_hidden: bool,                     // soft delete
}

pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub default_system_message: String,
    pub character_names: CharacterNames,
    pub greeting: Option<String>,           // the assistant's greeting (the first message)
    pub enabled_tools: Vec<ToolId>,
    pub default_sampling: Option<SamplingConfig>,
    pub is_hidden: bool,
}

// Role names shown to the user (feed headers, `F5` export). Empty = not set:
// the surface falls back to the interface language's label (§11.3).
pub struct CharacterNames { pub user: String, pub assistant: String, pub system: String }

pub struct Note { pub id: Uuid, pub profile_id: Uuid, pub content: String,
                  pub tags: Vec<String>, pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc> }

pub struct RagDocument { pub id: Uuid, pub profile_id: Uuid, pub source: String,
                         pub chunk_text: String, pub embedding: Vec<f32>, pub created_at: DateTime<Utc> }
```

As in attempt #1, **the system message is held in an explicit `Chat.system_message` field** (rather than "the first System message in history"), because the assistant can change it via a tool.

### 5.2. Data storage

Two-tier (as in attempt #1):

1. **JSON files** (human-readable, easy to back up):
   - `settings.json` — global configuration (model/server launch, inference, default sampling, UI theme, spellcheck, tools).
   - `profiles.json` — the list of profiles.
   - `chats/{chat_id}.json` — one file per chat.
2. **SQLite** (`data.db`) — notes and RAG (need queries and vector search):
   - `notes(id, profile_id, content, tags, created_at, updated_at)`.
   - `rag_documents(id, profile_id, source, chunk_text, created_at)` + a `sqlite-vec` virtual table for embeddings (linked by `id`/`rowid`).
   - **Per-profile isolation** — a mandatory `WHERE profile_id = ?` on every notes/RAG query (a repository invariant).

**Plus a disposable third file, `cache.db`** — the full-text index behind searching chats by content (`Ctrl+F` in the chat list, [§11.2](#112-the-chat-list-an-overlay)). It is deliberately **not** a third tier of user data: everything in it is derived from `chats/*.json`, and that changes the rules. It carries none of the schema-versioning machinery of [§12.2](#122-schema-versioning-and-migration), because a version mismatch or a corrupt file is answered by *deleting the file and rebuilding*, never by a migration step and never by refusing to start — a disposable index must not be able to block startup. So deleting it is a **supported repair** rather than data loss, and it is **excluded from backups** ([§12.3](#123-backup-and-deletion) — the archive's include list is an allowlist of user data), so a restore lands without an index and rebuilds it. It is kept in step by the sole writer of chats: the orchestrator indexes a chat right after saving it, and a startup pass reconciles whatever changed outside the app (import, restore, a hand-edited file, a deleted cache).

**Soft delete**: `is_hidden` on chats and profiles; hiding a profile cascades to hide its chats and excludes its notes/RAG from result sets (`WHERE is_hidden = 0`). There's no hard delete.

**Data location**: by default, in a `data/` subdirectory **next to the executable** (portable); a `defaults.json` file can move it to an OS folder/custom directory and set the scaffold/interface language (see [§12.1](#121-configuration)). The `default_language` field is optional: when absent (a fresh install, a deb/rpm package), the language is detected from the OS locale (`ru*` → Russian, otherwise English). The file tolerates a UTF-8 BOM. In non-portable mode, read-only resources (spellcheck dictionaries) are also looked up in the portable layout next to the binary — an installer/package puts them there.

**Race-free writes**: the orchestrator is the sole writer; a chat is saved after mutations are applied and on a debounce timer for intermediate states; writes are atomic (write-rename), and a backup of the chat file is kept.

**Schema versioning**: every artifact has its own schema version (`SETTINGS_SCHEMA`/`PROFILES_SCHEMA`/`CHAT_SCHEMA`/`DB_SCHEMA`, all = 1) — they change at different rates; version detection is structural (see [§12.2](#122-schema-versioning-and-migration)), so existing files aren't rewritten. **Read hardening**: a corrupt `settings.json`/`profiles.json` → startup refuses (it used to be silently overwritten with defaults); a corrupt `chats/<id>.json` → skipped with a `warn`, the file on disk is left untouched (previously one corrupt file would take down the whole startup).

---

## 6. Inference engine and the client-side agentic loop

### 6.1. The engine layer (`shared/api`)

Everything that depends on the specific server/HTTP is hidden behind an `EngineBackend` trait — this gives testability (mock/replay) and the option to swap the transport later without touching the orchestrator.

```rust
pub trait EngineBackend: Send + Sync {
    /// A single-turn streaming request. Returns a stream of chunks.
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken)
        -> Result<BoxStream<'static, ChatChunk>>;

    /// Embeddings (RAG). May spin up a secondary server on demand (see 6.6).
    /// `role` says what the text *is* — a search query or a stored passage; some
    /// model families expect their input marked accordingly (see 9.3.5).
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>>;
}

pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<ApiMessage>,          // user/assistant/tool
    pub tools: Vec<ToolSchema>,             // OpenAI schemas of the enabled tools
    pub tool_choice: ToolChoice,            // Auto by default
    pub sampling: SamplingParams,           // see section 8
}

pub enum ChatChunk {
    Text(String),                           // a delta of the main text
    Thoughts(String),                       // a delta of "thoughts" (reasoning_content or <think>)
    ThoughtsSignature(ThinkingRef),         // reasoning to resend on tool-use (Anthropic/OpenAI)
    ToolCallDelta(ToolCallDelta),           // accumulating tool_calls
    Usage(TokenUsage),                      // the server's exact token counts
    Error { message, transient },           // a failure after the stream opened (see 6.8)
    Finished { reason: FinishReason },       // Stop | ToolCalls | Length | Cancelled | Error
}
```

The default implementation is HTTP to an OpenAI-compatible server (`OpenAiClient` on `reqwest` + our own types), streaming over SSE.

### 6.2. Building the request

1. The system message = `Chat.system_message` (as of the moment of the request).
2. The dialogue = `Chat.messages` → `messages` with `system`/`user`/`assistant`/`tool` roles (including tool-result entries).
3. The profile's tools (`Profile.enabled_tools` ∩ globally enabled) → `tools` (OpenAI schemas), `tool_choice = auto`.
4. Sampling — by priority `Chat.sampling_override` → `Profile.default_sampling` → the global default (see [section 8](#8-sampling-parameters)).
5. **No string `stop`** for EOS text (see [section 7](#7-eos-and-stop-token-handling)).

History is append-only — the server reuses the prefix cache (see [6.6](#66-kv-cache-and-prefix-caching)).

### 6.3. Client-side agentic loop

Since the server doesn't execute our tools, the orchestrator runs the loop:

```
round = 0
loop:
    chunks = engine.chat_stream(req, cancel)            # stream into the UI: Text/Thoughts/ToolProgress
    accumulate(assistant_text, thoughts, tool_calls)
    match finish_reason:
        Stop | Length        -> record the response, exit
        Cancelled            -> record the partial response, exit
        ToolCalls:
            round += 1
            if round > max_tool_rounds: append a system note "limit reached", exit
            append assistant-message(tool_calls) to req.messages
            for tc in tool_calls:                        # the client can execute every call of a round
                outcome = registry.invoke(tc.name, tc.arguments, &tool_ctx).await
                apply(outcome.effects)                   # the orchestrator mutates Chat (it's the owner)
                append tool-message(tc.id, outcome.result) to req.messages
            continue the loop (a new request with the extended history)
```

- `max_tool_rounds` — a safeguard against infinite loops; **8** by default, configurable. Unlike `mistral.rs` (which executed only the first tool call per turn), the client-side loop can execute every call in a round; tool descriptions are still designed for "one logical call per round" for predictability.
- Each tool call is shown in the UI as a **collapsible block** (name, arguments, result) + an "assistant is using a tool…" indicator.
- `tool_ctx` — a snapshot taken at the start of the turn (see [9.2](#92-the-tool-contract)).

### 6.4. Cancellation, regeneration, continuation, deletion

- **Cancellation**: a `CancellationToken` aborts the current HTTP stream; the partial response is kept; the chat goes `Cancelling → Idle`.
- **Regenerating the last response**: delete the last assistant message (and any tool messages from that turn) and repeat the request with the same context (a new seed, if a random seed is enabled).
- **Continuing a response**: request a continuation of the assistant's last response (optional; useful when `finish_reason = Length`).
- **Deleting the last message**: on request — the **last Assistant message** is deleted along with the **last User message**, and its text is **returned to the input box**; if the box isn't empty, the deleted message's text is **prepended** to what's already there. (If the last message is a user message with no reply, only it is deleted, with its text returned.)

### 6.5. Parsing "thoughts" (CoT)

> **Confirmed:** the server sends reasoning in a **dedicated `delta.reasoning_content` field** (in `llama-server` — when started with `--reasoning-format`, e.g. `auto`). Otherwise reasoning arrives inline in `content`, and the fallback `<think>` parsing kicks in (below).

- **Primary path**: reasoning arrives in `delta.reasoning_content` → directly into `ChatChunk::Thoughts`. Enabled via `thinking: true` (+ `reasoning_effort`) in the request.
- **Fallback path** (external mode without the environment variable): a streaming parser extracts `<think>…</think>` from `content`, correctly handling tags **split across a chunk boundary** (an unfinished-tag buffer). Covered by unit tests in both modes.
- "Thoughts" are stored in `Message.thoughts` and shown in a **collapsible block** in the feed.

### 6.6. KV cache and prefix caching

- No on-disk state cache is used. We rely on **the server's automatic prefix caching** (reusing KV blocks on a matching request prefix) — between turns and between agentic-loop rounds the history is append-only, so prefill only happens for the new tokens.
- **Within a turn**, adding tool results only appends tokens — there's no re-processing of the prefix.
- **Changing the system message** (by the user or by the `set_system_message` tool) changes the start of the prefix → full reprocessing on the next turn (expected). Within the current turn, a `system_message` change is **not applied** (the old prefix keeps acting) — this preserves consistency and the current generation's cache.
- KV compression and other optimizations are configured on the inference-server side and are transparent to the application.
- **History is append-only *between compactions*.** A compaction ([6.7](#67-history-compression-rolling-summary)) rewrites the start of the prompt once, so the next turn is a full re-prefill — the same deliberate, justified invalidation as a `set_system_message`, and the ongoing win is a shorter prefix. Compactions are rare threshold events, so a provider-level cache breakpoint would survive between them.

### 6.7. History compression (rolling summary)

The whole conversation is sent on every request, so a long chat eventually stops fitting the model's context window. Measured against the reference stack: llama-server answers **HTTP 400 before the SSE stream even starts** (`exceed_context_size_error`, with `n_prompt_tokens` and `n_ctx` in the body), and `--context-shift` does not rescue an oversized prompt — it only evicts during generation, discarding the **system prompt first**. Compression exists to keep the conversation below that ceiling. Research and measurements: [docs/research/history-compression.md](docs/research/history-compression.md).

**The mechanism changes what a request carries, never what the chat holds.** `Chat.messages` is untouched, so the feed, full-text search, the `F5` export, TTS and the reflection watermark all keep seeing the whole conversation — this is what makes the feature cheap and reversible. A request becomes `system + [summary block] + messages[upto..]`. **Impersonation** (`Ctrl+U`, [§11.8](#118-impersonation-writing-a-message-as-the-user)) builds its own request and reads the same view, so it is subject to the same ceiling and the same relief.

- **Storage** — `Chat.compaction: Option<Compaction>` (`summary`, `upto`, `boundary_id`, `compacted_at`, `rolls`). Additive, no migration (ADR 0006 F12). `upto` is a fast path only: the boundary is re-found by **`boundary_id`** on every read, so an edit that shifts indices cannot leave the summary silently covering the wrong span; if that message is gone the summary is ignored and the whole history is sent until the next compaction rebuilds it.
- **The cut is always a `User` message**, so an assistant turn is never separated from its tool results — which would break Anthropic's strict alternation and Gemini's per-call thought-signature replay. Everything older than the verbatim tail (`compaction.tail_tokens`) is folded in.
- **The roll** is one independent single-turn request (no history, no tools, reasoning muted — the `title.rs` shape), fed a digest of the newly-covered range **including tool activity**: tool results are persisted in history, replayed on every turn and invisible in the feed, so they are the largest hidden cost in a long chat. Rolling (`summarize(previous + next chunk)`) is not merely cheaper than re-summarizing everything — it is the only shape that always fits, since the summarizer has the same context limit as the chat that overflowed.
- **The length limit lives in the prompt, in words**, with `max_tokens` only as a safety net far above it. Measured: a bare token cap does not shorten a summary, it truncates one mid-sentence, and given room the summary **grows with every roll** — a stated limit plus an instruction to drop what later parts superseded fixes both.
- **The block** is prepended to `system` right after the persona, ordered by volatility (persona → summary → attachments → self-model). Its header is in the **profile** language (axis A), marks the content as DATA rather than instructions, and **names the two read-back tools below** — a block that describes a situation without saying what is possible is what sends a model improvising.
- **What the summary dropped is still reachable.** A summary is lossy by construction, so two tools page back into the folded range: **`history_read`** walks it page by page (`compaction.page_tokens`), numbered from 1 with the total stated, so the model can walk `1..M` and *know* it read everything; **`history_search`** finds where to look and returns fragments with their page numbers, so the pair composes — search says *where*, reading guarantees *everything*. Search runs over the full-text index the application already keeps (`cache.db`, [§5.2](#52-data-storage)), not over embeddings: an embedding server is separate and routinely unconfigured (ADR 0002), while the local user with a small window is exactly who compression exists for — and a trigram index matches substrings, so `8823` finds `ZARYA-8823`, identifiers being the first thing a summary loses. Both reach **only this conversation's own older messages**, which the user is looking at in the feed, so neither is gated; they are offered to the model **only while a folded range exists**, which is the same condition that puts the block in the prompt. The block names them only when the turn really offers them — a profile can switch the two tools off, and naming a tool the model does not have is a dead end rather than a hint.
- **The trigger** — a compaction starts by itself once the next turn's prompt would reach `compaction.threshold_pct` (default 75%) of the resolved context window; `/compact` runs one on demand at any time. The remaining quarter is the headroom that lets the user keep typing while the background roll works, and it doubles as the reply reserve — there is deliberately no second knob for that. `0%` means "manual only".
- **Which token number** — the **exact** `usage.prompt_tokens` the server reported for the turn's last round, plus what was generated on top of it, and nothing else. The client-side byte estimate is not a fallback here: its error **changes sign** by content type (it overestimates Russian prose by 68% while *under*estimating code by 20% and JSON tool results by 7%), i.e. it is unsafe exactly on the tool-heavy chats that overflow first. A provider that reports no usage therefore leaves the automatic trigger inactive rather than acting on a guess.
- **The context window** is resolved: an explicit setting → a managed server's own `-c` → what the engine reports about itself (`EngineBackend::context_budget`, llama.cpp's `/props`; the figure is used **as given** — with no `-np` flag the server reports several slots over an *undivided* context, so dividing by `total_slots` would be wrong by 4x). The engine is asked once per applied engine and re-asked when its readiness flips, so a server that came up late is not left unmeasured; an answer about an engine that has since been replaced is dropped. With no source at all the automatic trigger stays inactive — `/compact` still works, it needs no budget.
- **When it is already too late**, the failure says what to do instead of handing back the provider's raw JSON in a generic wrapper — and *which* advice depends on the switch: with compression on it names `/compact`, with it off it names the setting. Pointing at a command that would refuse is a dead end, not a hint.
- **UI** — `/compact` runs it on demand. The feed draws a muted divider at the boundary, carrying the summary as a foldable block that expands together with the "thoughts" blocks (`Ctrl+T`, [11.3](#113-chat-window)) — no separate hotkey and no extra per-chat state. A quiet status-bar chip shows a roll in progress. A background roll that fails stays silent and spends a strike in the background-task failure streak (three consecutive failures alert once); a roll the user asked for reports its failure immediately, since a command just typed is owed an answer.
- **Off by default? No — on**, where it can act at all: what it prevents is strictly worse than what it does, and the divider keeps it visible. But it is **fully switchable off** (`compaction.enabled`, settings → "Memory" → "Context"), and off means *inert*: no splice, the full history is sent exactly as before the feature existed, `/compact` refuses with a pointer at the setting, and the stored summary is kept rather than discarded — so off → on → off is lossless in both directions.

### 6.8. Transient engine failures and what the user is told

Cloud providers shed load as a matter of course (`429`, `500`/`503`, Anthropic's
dedicated `529 overloaded_error`), and a connection can die at any point. The
contract splits such a failure by **when** it happens, because that decides both
what can be said and what could be done about it.

- **Before the stream opens** — a transport failure or a non-2xx status — is an
  `Err` from `chat_stream`, carrying `EngineError { kind, status, retry_after,
  message }`. The status and any `Retry-After` are kept as fields rather than
  buried in prose (the header in seconds for OpenAI/Anthropic, OpenAI's
  `retry-after-ms`, and for Gemini — which sends no header — the
  `google.rpc.RetryInfo` hint inside the 429 body). The provider's own body is
  never swallowed: its first 500 characters are part of the message, which is what
  lets [§6.7](#67-history-compression-rolling-summary) recognize an overflow and
  answer with `/compact` instead of a generic wrapper.
- **After the stream opens** the failure cannot be an `Err` — the caller already
  holds the stream — so it arrives as `ChatChunk::Error { message, transient }`
  immediately before `Finished(Error)`. This channel exists because such failures
  were previously invisible: the partial reply was kept and nothing said it was a
  fragment, and for Anthropic — whose documented in-stream `error` event is how a
  `529` arrives once a stream is accepted — a truncation was indistinguishable
  from a completed answer.
- **What the user sees** is one of four things, chosen in one place so the two
  paths above cannot describe the same condition differently: the two overflow
  messages of §6.7; *"the reply was cut short"* plus a pointer to `Ctrl+R`, when
  text had already reached the screen (a fragment is a different situation from a
  failure to answer, and a message that does not say which leaves the user
  guessing); or the generic failure. Overflow outranks the fragment wording —
  naming `Ctrl+R` there would invite a retry that overflows again. Background
  turns (auto-title, reflection) log the reason instead, since the failure streak
  already reports them; a **compaction roll** is the exception and reports it,
  because `/compact` was just typed and is owed an answer.
- **`transient`** says whether another attempt could succeed. It is derived from
  the status (`408`/`429`/`500`/`502`/`503`/`504`/`529` — the set converges across
  every provider) or, with no status line left to read, from the provider's error
  *name*. A `400` is never transient: llama.cpp's `exceed_context_size_error` is
  compression's job, not a retry's.
- **A transient failure is retried automatically**, by a decorator around the
  engine rather than by any of the clients, and only on the **cloud and external**
  backends: a managed child that died reloads for minutes, and there the
  supervisor's health monitor and relaunch budget are the recovery mechanism
  ([§3.4](#34-managing-the-server-lifecycle)). A cloud backend, by contrast, is
  never monitored or relaunched, so this is the only recovery it has.
  - **Only before the first content chunk.** Once text, "thoughts" or a tool call
    has been streamed the turn is *committed* — the user has seen it start, no
    provider can resume a broken stream, and splicing a regenerated answer under a
    half-rendered one would be a lie. After that the failure is surfaced and the
    partial reply kept, as above. Because a tool call counts as content, a round
    that produced calls is never replayed **by construction**, so no tool effect
    can fire twice. Retrying happens at this one layer: the orchestrator and the
    agentic loop re-issue nothing.
  - **Three attempts** (the default of both first-party SDKs), waiting ~1 s then
    ~2 s — exponential, jittered *downward* by up to a quarter so many clients do
    not retry on the same beat. A `Retry-After` is honoured as given (providers
    state that an earlier retry fails) up to **30 s**; beyond that the request is
    treated as not retryable and the failure is shown at once, because a provider
    asking for minutes is describing a quota rather than a blip and a frozen
    spinner is worse than an error one can read. Quota-flavoured `429`s are not
    told apart from rate limits — two short waits cost seconds, and a rejected
    request is not billed.
  - **The wait is visible and interruptible**: a quiet status-bar chip says which
    attempt is starting and in how long, and `Esc` during a backoff ends the turn
    immediately rather than after it.
  - The policy is a set of constants, not settings — the same choice the health
    monitor's cadence is kept at.
- **Timeouts.** Engine clients bound only the **connect** phase (10 s). A stream
  legitimately stays open for minutes and a local prefill can be silent for tens
  of seconds, so a total or idle timeout would cut healthy generations; what is
  bounded is the case with no healthy reading — nobody answering at all. The
  initial request is sent inside the cancellation token's reach, so `Esc` works
  while connecting rather than only once bytes are flowing.

---

## 7. EOS and stop-token handling

### 7.1. Requirement

Generation **must not stop** when the model *mentions* the EOS-token text in its response (printing the string `<|im_end|>` or `<end_of_turn>` as part of the text). Elaborate prompt-injection protection isn't needed — it's enough for generation to keep going.

### 7.2. Solution

- Stopping is done **strictly by the token id of the model's actual special tokens** (EOG/EOS from the vocabulary), which the chat template/tokenizer on the server side takes care of. If the model samples the literal text `<|im_end|>` as ordinary tokens — that's not the end of the turn, the text just ends up in the response.
- **We don't send string `stop` sequences** matching the EOS text (otherwise the truncation bug returns). By default `stop` in the request is empty.
- If string stop sequences are ever needed (e.g. the companion's name as a separator) — that's configured separately and deliberately, with a UI warning.

### 7.3. Nuances

- Real EOG/EOS tokens: Qwen — `<|im_end|>`, `<|endoftext|>`; Gemma — `<end_of_turn>`, `<eos>`. Handling them as special tokens is the server's job.
- The server's default EOS logic (stopping by token id from the tokenizer/template) is already correct and satisfies the anti-truncation requirement; sending string `stop` for EOS text isn't needed (and isn't done).
- Client invariant: the `stop` field in the request is empty by default. Anti-self-truncation is verified by an `#[ignore]` smoke test against a live model (Qwen `<|im_end|>` + Gemma `<end_of_turn>`).

---

## 8. Sampling parameters

### 8.1. Mapping onto the OpenAI-compatible API

`SamplingConfig` serializes into the `POST /v1/chat/completions` body. Besides the standard OpenAI fields (`temperature`, `top_k`, `top_p`, `frequency_penalty`, `presence_penalty`, `max_tokens`, `thinking`/`reasoning_effort`/`reasoning_budget`), the model carries **llama.cpp `llama-server` extensions**: `dynatemp_range`/`dynatemp_exponent` (dynamic temperature), `min_p`, `top_n_sigma`, `typical_p`, `adaptive_target`/`adaptive_decay` (adaptive-p, experimental), `repeat_penalty`, `repeat_last_n`, `dry_multiplier`/`dry_base`/`dry_allowed_length`/`dry_penalty_last_n`/`dry_sequence_breakers`, `xtc_probability`/`xtc_threshold`, `mirostat`/`mirostat_tau`/`mirostat_eta`, `seed`, `samplers` (sampler order). `llama-server` accepts these fields directly in the request body (not only as CLI flags), so they work in both managed and external llama.cpp setups without restarting the server, and are resolved through the levels in 8.3. Every field is an `Option`, **sent only when set** (`skip_serializing_if`): unset extensions never make it into the JSON, so a strict third-party OpenAI server is unaffected by default, and an llama.cpp-incompatible field would either be ignored or rejected by it (a risk only if the user explicitly sets an extension against a strict server). List fields (`dry_sequence_breakers`, `samplers`) are serialized as JSON arrays and **are never sent empty** (an empty `samplers` would be read by the server as "disable all samplers").

| Parameter | HTTP field | Status |
|---|---|---|
| `Temperature` | `temperature` | ✅ carried over |
| `TopK` | `top_k` | ✅ carried over |
| `TopP` | `top_p` | ✅ carried over |
| `AlphaFrequency` | `frequency_penalty` | ✅ carried over |
| `AlphaPresence` | `presence_penalty` | ✅ carried over |
| `MaxTokens` | `max_tokens` | ✅ carried over |
| reasoning | `thinking` + `reasoning_effort` + `reasoning_budget` | ✅ reasoning on/level/budget |
| `MinP` | `min_p` | ✅ **llama.cpp extension** |
| `RepeatPenalty` / `RepeatLastN` | `repeat_penalty` / `repeat_last_n` | ✅ **llama.cpp extension** |
| `Seed` | `seed` | ✅ **per request** (`-1` = random) |
| `TypicalP`, `TopNSigma` | `typical_p`, `top_n_sigma` | ✅ **llama.cpp extension** |
| Mirostat* | `mirostat`/`mirostat_tau`/`mirostat_eta` | ✅ **llama.cpp extension** |
| DRY*, XTC* | `dry_*` (incl. `dry_sequence_breakers`), `xtc_*` | ✅ **llama.cpp extension** |
| Dynatemp* | `dynatemp_range`/`dynatemp_exponent` | ✅ **llama.cpp extension** (dynamic temperature) |
| Adaptive-p | `adaptive_target`/`adaptive_decay` | ✅ **llama.cpp extension** (experimental) |
| Sampler order | `samplers` (array of names) | ✅ **llama.cpp extension** |
| `MaxRetriesToRegenerateInvalidOutput` | — | not needed (structured outputs reduce "broken" output) |

### 8.2. Consequences

- The `SamplingConfig` model and the sampling settings UI (the "Sampling" section, "Assistant"/"Impersonation" subsections) include all the fields listed above. The llama.cpp extensions default to `None` — for old `settings.json` files and third-party servers behavior doesn't change until the user sets them.
- **A per-request random seed** is now available through the `seed` field (`-1` = random); regeneration variability can still also come from `temperature > 0`.
- A snapshot of the actually applied parameters is saved into `Message.metadata`.
- **Cloud dialects** ([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)) accept only a subset of fields (`supported_sampling_fields`): OpenAI/Gemini strip llama.cpp extensions and reasoning signals; **Claude** supports `max_tokens` + extended thinking (`thinking`/`reasoning_effort` → `{type:"adaptive", display:"summarized"}` + `output_config.effort`), but **rejects** `temperature`/`top_p`/`top_k` and `budget_tokens`/`reasoning_budget` (the 4.x models have "locked in" sampling). During tool use, Claude requires returning the thinking block with its signature in the same turn — this is handled by the agentic loop in memory, without persisting it. The settings UI and `get/set_sampling` show only the fields available for the current mode.

### 8.3. Override levels

Priority (highest → lowest): `Chat.sampling_override` (incl. changes made by the assistant via `set_sampling`) → `Profile.default_sampling` → the global `settings.json`.

---

## 9. Tool system

### 9.1. General model

- Tools are executed by the **client-side agentic loop** ([6.3](#63-client-side-agentic-loop)). Each is described by an OpenAI schema (`name`, `description`, `parameters` (JSON Schema), `strict`).
- The list of schemas of the enabled tools is sent to the server; the server (with constrained generation) returns syntactically valid `tool_calls`.
- A tool's result is a string (or a JSON string), added to the history as a `tool` message.
- Unlike attempt #1, web search and Python are **implemented by us** (the engine doesn't provide them).

### 9.2. The tool contract

```rust
/// An immutable snapshot of state at the start of the turn. No shared locks.
pub struct ToolContext {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,             // a snapshot of Chat.system_message
    pub effective_sampling: SamplingConfig, // the effective sampling (after the 8.3 priorities)
    pub last_user_message_at: DateTime<Utc>,
    pub storage: Arc<Storage>,              // notes/rag (internal synchronization, isolation by profile_id)
    pub engine: Arc<dyn EngineBackend>,     // for call_subagent
}

pub enum ChatEffect {                       // returned by a tool, applied by the orchestrator
    SetSystemMessage(String),
    SetSamplingOverride(PartialSamplingConfig),
}

pub struct ToolOutcome { pub result: String, pub effects: Vec<ChatEffect> }

pub trait Tool: Send + Sync {
    fn id(&self) -> ToolId;
    fn schema(&self) -> ToolSchema;
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome>;
}
```

`ToolRegistry` holds the enabled tools, hands schemas to the engine, and maps names to implementations. Mutating tools return `effects` (rather than touching `Chat`); the orchestrator applies them itself (see [4.4.2](#442-chat-ownership-and-tools-a-simplification-vs-attempt-1)).

### 9.3. Tool roster

Semantics and signatures — as in attempt #1 (section 9.3 of its specification). In brief:

| Tool | Arguments | Behavior |
|---|---|---|
| `rag_search` | `{ query, top_k? }` | Embed the query → kNN in `rag_documents` (sqlite-vec, filtered by `profile_id`) → top-N fragments with sources. |
| `rag_add` | `{ text, source? }` | Chunking → embedding → written to `rag_documents` (`profile_id`). Returns the number of chunks. |
| `note_save` | `{ content, tags? }` | Written to `notes` (`profile_id`). Returns the id. |
| `note_recall` | `{ query?, tags?, limit? }` | Search notes (text/tags; optionally semantic), filtered by `profile_id`. |
| `python_exec` | `{ code }` | Executes Python in an isolated Wasmer/WASIX sandbox (by default) or a local interpreter — see [13.2](#132-python-execution). Returns stdout/stderr/error. Behind a global switch (disabled by default). |
| `web_search` | `{ query, max_results? }` | Searches DuckDuckGo (our own `reqwest` client) + extracts readable page content. Behind a global switch. See [9.3.1](#931-the-web-tool-our-own-implementation). |
| `fetch_url` | `{ url, focus?, summarize? }` | Fetch a page (`reqwest` + **rich** extraction `web::extract_rich` — prose plus headings and code blocks) and summarize it via `ctx.engine` (a one-turn request, like `call_subagent`). `summarize=false` → the extracted text with no model involved. A page over the attachment budget arrives as a **chat attachment** rather than cut — see [9.3.1](#931-the-web-tool-our-own-implementation). Behind the `web_enabled` global switch (network access). |
| `calculate` | `{ expression }` | Evaluates a math expression with our own recursive-descent evaluator (arithmetic, `^`, parens, `pi`/`e`/`tau` constants, `sqrt`/`sin`/`log`/`min`/`max`/… functions). No I/O, not gated. |
| `current_time` | `{ format? }` | Current date/time (local zone + UTC via `chrono`); `format` — a `strftime` string. No I/O, not gated. |
| `fs_read` / `fs_write` / `fs_list` | `{ path, … }` | Read/write/list local files. Behind the `fs_enabled` global switch (disabled by default, like Python); an optional `fs_root` sandbox confines access to a directory. |
| `get_sampling` | `{}` | Returns `ctx.effective_sampling` (JSON), restricted to the fields available in the current engine mode (`supported_sampling_fields`). |
| `set_sampling` | a partial `SamplingConfig` | Returns `ChatEffect::SetSamplingOverride`; applied starting from the next turn. The schema and the applicable fields are restricted to those available in the current mode (a cloud provider would reject the rest); unsupported keys are dropped, with the model notified. If no parameter is available in the current mode, `get_sampling`/`set_sampling` aren't offered to the model at all. |
| `get_system_message` | `{}` | Returns `ctx.system_message`. |
| `set_system_message` | `{ system_message }` | Returns `ChatEffect::SetSystemMessage`; applied starting from the next request build. The only tool that invalidates the chat's prefix cache (justified). |
| `get_last_user_message_time` | `{}` | Returns the last user message's timestamp (ISO 8601) + elapsed time. |
| `call_subagent` | `{ name?, system_message, message }` | **The key feature** — a sub-agent with this turn's tools, run as a nested turn; its transcript stays on the call's record. See [9.3.2](#932-call_subagent). |
| `send_followup_message` | `{}` | **Control** (opt., off by default) — write one more message as a separate reply. See [9.3.3](#933-conversation-control-tools). |
| `rewrite_current_message` | `{}` | **Control** (opt., off by default) — discard the current (in-progress) message and write it again. See [9.3.3](#933-conversation-control-tools). |
| `chat_search` | `{ query, top_k? }` | **Optional, off by default** — full-text search across the *other* chats of the profile, grouped by conversation with page addresses. See [9.11](#911-cross-chat-search-chat_search-and-chat_read). |
| `chat_read` | `{ chat, page? }` | **Optional, off by default** — read another conversation of the profile as a paged transcript. See [9.11](#911-cross-chat-search-chat_search-and-chat_read). |

> **Embedding lifecycle**: a **dedicated** embedding server is used (ADR 0002) — a separate process on its own port, started at launch and kept alive; if not configured, RAG returns a clear error. See [decision #4](#161-accepted-decisions).

#### 9.3.1. The web tool (our own implementation)

Since the inference server doesn't provide built-in web search, we implement it ourselves:

- **Search**: a multi-provider fallback chain (DuckDuckGo lite/html → Mojeek → Ecosia) over `reqwest`; result parsing (`scraper`); anti-bot throttling detection. Throttling is not always a status code: Mojeek serves its captcha with **HTTP 200**, so a page that parsed to **zero** results is additionally checked for interstitial markers (`is_challenge_page`) — otherwise a blocked provider counts as "answered, found nothing", which suppresses the honest "all providers are throttled" error and tells the model the web has nothing on the subject. The check runs **only** on an empty parse, so a real result set can never be mistaken for a challenge.
- **Content extraction**: result pages are fetched in parallel, and "readable" text is extracted from the HTML (`scraper`: paragraphs/lists from `<article>`/`<main>`, boilerplate `nav`/`header`/`footer`/`aside` discarded). "Best effort" — one page's fetch failure doesn't fail the whole search. This prose-only extraction is what **ranking** needs and stays deliberately narrow (1500 characters per page); `fetch_url`, which is read by the model rather than ranked, uses the **rich** extraction below.
- **Rich extraction** (`fetch_url` only, `web::extract_rich`): the same readability plus **section headings** (`## …`) and **code blocks** (fenced, with the language taken from a `language-*` class on the element or its inner `<code>`), in document order, whitespace inside code preserved and short code/headings exempt from the fragment floor. Prose-only extraction turns a documentation page into prose whose every "here is an example:" leads nowhere — measured on `docs.vlang.io`, which has no `<pre>` at all and wraps code in a bare `div.language-v`. A container that already emitted its content does not emit it twice (ancestry dedup).
- **A page over the budget becomes a chat attachment** (§9.7) instead of being cut: the threshold is `attachments.max_file_tokens`, exactly as `youtube_watch(transcript:)` uses it, so such an attachment is always *by reference* and is immediately paged (`attachment_read`) and searchable (`attachment_search`). Below the threshold the text goes straight into the result. `summarize=true` still summarizes — from the head of the text, since the summarizer is a single-turn subagent — and attaches as well, so a partial summary is a starting point rather than the only access. A hard ceiling on one page's extracted text remains (400 000 characters) and, when reached, is **stated in the result**: silent truncation made a long page indistinguishable from a complete one.
- **Reranking**: results are reordered by embeddings (`ctx.embedder`, ADR 0002) by cosine similarity of the content to the query; if the embedder isn't available, the provider's original order is kept.
- Behind a global switch (privacy). Content fetching + reranking are gated by the call's `fetch_content` argument (default from `config.tools.web_fetch_content`, `true` by default); `false` → the fast "titles/snippets only" path.
- **Address policy** (`shared::net`, [docs/research/fetch-url-address-policy.md](docs/research/fetch-url-address-policy.md)). Both tools follow addresses the *model* chose — `fetch_url` directly, `web_search` through whatever the provider ranked — and such a URL routinely comes from a page just read, a search result or an attached document. So both refuse anything not publicly routable: loopback, the private ranges, link-local (where the cloud metadata endpoint lives), CGNAT, multicast and reserved space, each also in its **IPv4-mapped IPv6** spelling. Three mechanisms, because one is not enough: the check lives inside the client's **DNS resolver**, so the address that was approved is the address connected to (no rebinding window) and *every* address a name resolves to is judged, not the first; an **IP literal** is checked before the request, since `hyper` parses a literal host itself and never calls the resolver; and every **redirect hop** is checked again. The refusal names every closed route — no other tool reaches it either, and retrying by IP, by hostname or through a redirector is pointless — because a message that only says "cannot" spends the model's next three turns. `tools.web_allow_private` (**off** by default) opens the private ranges for someone whose model should read an internal service; it does not touch `/image attach <url>`, which the user types (§9.10), nor the engine addresses in settings, which are LAN by design.

#### 9.3.2. `call_subagent`

- **Purpose**: delegate a task — or get a **second opinion** — to a **sub-agent**: the same model under a system message the main agent composes, with an optional display `name` for the persona. The response differs from the agent's own point of view, which (by hypothesis) increases self-awareness and quality; with tools, the sub-agent can also *do* the work it is asked. Design and the decided forks: [docs/research/subagent-chats.md](docs/research/subagent-chats.md), [ADR 0010](docs/decisions/0010-subagent-nested-turn.md).
- **A loop-executed tool.** Like the conversation-control pair ([9.3.3](#933-conversation-control-tools-send_followup_message--rewrite_current_message)), the `Tool` impl exists for the schema, the catalog and the profile toggle; the agentic loop ([6.3](#63-client-side-agentic-loop)) recognises the name and runs the sub-agent itself — after the disabled gate and the confirmation gate ([9.8](#98-confirmation-for-dangerous-tool-calls)), inside the turn's cancellation, with a tool card and a record like any call. A tool sees only `ToolContext`; the registry, the confirmation channel and the UI sender a tool-using sub-agent needs live in the loop.
- **A nested turn.** The sub-agent is a child agentic loop of the same type as the turn's own, sharing the turn's engine, registry, confirmation round trip (a dangerous call inside the run asks the user exactly as outside; "allow for this turn" covers both), generation id and limits. It has its own request — `system = system_message`, one user message = `message` — its own context, round counters and a child cancellation token, and a **muted** event stream: nothing of it reaches the parent's bubble except the token counter, which continues the parent's — and a quiet status-bar chip, *"sub-agent «name» · round N · tool"*, reported around the muted sink at each round's start and each tool's entry and cleared when the run ends, so a parent turn parked inside a delegation for minutes never reads as a stuck "generating" (docs/lessons.md §4). The chip is worded by the screen in the interface language, guarded by the generation id, and goes with the turn. **The transcript is visible while it runs** ([docs/history/subagent-live.md](docs/history/subagent-live.md)): the generation task reports its progress to the orchestrator on the same channel the result travels (so progress and result arrive in order) — each filed round of the parent and of the sub-agent, the run's start (its id minted up front and kept by the landed record) and its end — and the orchestrator mirrors the turn in an in-flight table that every transcript path resolves through. So a running run is a row of the list under its parent, marked *running*, with its count growing as rounds file; it opens read-only with the rounds so far and grows as more file; it can be renamed (the title lands on the record as a manual one); and moving between the parent and that transcript — either way — **does not cancel the turn**, the one exception to "a switch cancels" ([11.2](#112-the-chat-list-an-overlay)). Nothing of this is persisted: the mirror is dropped at landing, and a crash loses the turn as before. **The sub-agent's text streams into its open transcript** (history plan §8): its loop's stream — text, thoughts, tool cards, its own token count — travels to the orchestrator as progress on the same channel as its filed rounds (so it can never arrive out of order with them), is kept as the round in progress, and is forwarded to the screen under the run's own stream id while the transcript is the open conversation; a transcript opened mid-round starts with what has already streamed.
- **What it gets.** The turn's effective tool set **minus** `call_subagent` (no nesting — a loop below the top refuses the name regardless of the set), `history_read`/`history_search` (the *parent's* folded history) and the self-model family with its injection (the profile persona's identity, not the sub-agent's). The turn's environment otherwise: the same attached files (`attachment_read`/`attachment_search`, under the parent's index), the same project and change journal (`code_*` edits show in the parent's `F4`), notes, RAG, web, Python, `fs_*`, MCP, the control tools; its system prompt carries the parent's attachment and workspace blocks. **Never the parent's messages** — the main agent decides what to pass.
- **Effects** go to the chat they describe: `set_system_message`/`set_sampling` to the run, an attachment a tool produced to the parent (and into both loops' snapshots).
- **The transcript** — the persona, `User(message)` and the run's rounds exactly as any chat stores them — lives **on the call's `ToolCallRecord`** (`subagent: SubagentRun`, [5.1](#51-core-entities-entities)), inside the parent's file: it is part of the exchange that made it, travels with it into `Chat.deleted` on `Ctrl+E`/`Ctrl+R`, is hidden with the parent, and is ignored by request replay (a request with a stored run is byte-identical to one without). It carries a title (the `name`, else the first line of `message`, until a person or the model names it — **the automatic titling fires at landing** for every run with a substantive reply, under *either* `interface.auto_title` point, since a transcript's question and reply arrive together; `Off` is quiet, and a hand-named run is left alone, [11.2](#112-the-chat-list-an-overlay)), an outcome (`completed`/`cancelled`/`timed_out`/`failed`/`round_limit`), the tokens it cost, and an id for a `chat://` reference ([11.3](#113-the-message-feed)).
- **The result** the main agent gets: the sub-agent's final reply, then one line naming the transcript's `chat://` address and — when the run did not complete — why. The run lands on the record together with the turn; until the list snapshot carries it, the address in the card is plain text (the "resolves or it is not a reference" rule).
- **Limits**: `max_tool_rounds` and `workspace.max_rounds` apply to the run as to the turn (each run spends one round of the parent's budget, as before); `tools.subagent_max_tokens` caps each of its replies (min'ed with the effective `max_tokens`); `tools.subagent_run_timeout_secs` bounds the whole run. `Esc` on the turn cancels the run with it; the partial transcript lands as `cancelled`.

#### 9.3.3. Conversation control tools (`send_followup_message` / `rewrite_current_message`)

Unlike ordinary tools (which return a text result without touching the conversation's structure), these are **control-flow** tools: they're recognized by the orchestrator's client-side agentic loop itself (`app/orchestrator/generation.rs`), not by `Tool::invoke`. The `Tool` implementations (`features/tools/control.rs`) exist only for the schema/description/registration/gating. Both are **optional** and **disabled by default**: they're not in `default_tool_ids` (so `reconcile_tools` doesn't enable them), but they are in the `all_tool_ids` catalog — toggles in profile settings. The loop only recognizes them when they're actually enabled in the profile (otherwise it's an ordinary refusal).

- **`send_followup_message`** ("write another message"): the assistant can finish writing its current message, then call the tool — after which it writes a **second reply**, shown as a **separate bubble** right after the first one. Implementation: the loop finalizes the current round's text as a message, adds a tool-result "permission granted", and continues the loop; the next round is the second message. Alternation and inference are standard (`assistant(tool_call) → tool → assistant`), no synthetic user turn is inserted. The "separate bubble" is enforced by the `Message.new_bubble` flag (honored by `from_messages`, which doesn't merge it into the previous assistant block); the control tool's internal tool block is hidden in the feed.
- **`rewrite_current_message`** ("rewrite my current message"): if, partway through writing, the assistant realizes it answered incorrectly, it calls the tool — the current round's accumulated text is **discarded** (the UI clears the bubble to be rewritten), and the next round writes the message from scratch. During the rewrite round, the request **includes** the prior `assistant(partial + tool_call)` + the tool result (the model sees "you were asked to rewrite"); once done, the discarded assistant message and its tool message **move into `Chat.deleted`** (like `Ctrl+E`/`Ctrl+R`, §11.7) — no longer part of inference/the feed, but preserved for manual recovery via JSON editing.

Both are subject to `max_tool_rounds` (each call = one round) — a backstop against endless "extra messages"/"rewrites". UI signals — the `AppEvent::AssistantContinue`/`AssistantRewrite` events (the live feed matches a reload from history).

#### 9.3.4. Changing the embedding model

Stored vectors are only comparable to a query embedded by the **same** model, so
swapping the embedding model silently invalidates everything already indexed.
Dimensionality cannot establish identity — `bge-m3` and
`multilingual-e5-large-instruct` are both 1024-d, yet the same text embedded by
both scores a cosine of only ~0.37, and every existing guard waves the swap
through. See
[docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md).

- **Detection is behavioural, not config-based.** A fixed canary string is
  embedded and stored (`meta`, together with the model's display name); on the
  next run it is embedded again and compared. This also catches what a config
  fingerprint cannot: the same GGUF path re-pointed at another file, a
  requantization, or a server restarted with different pooling flags. The check
  runs **lazily, on the first real embedding call** after launch or after an
  embedding-settings change — embeddings have no readiness probe (ADR 0002), so
  there is no startup moment when a managed server is known to be up. A failed
  check never blocks the actual work; it simply retries.
- **Nothing is deleted — a whole *generation* is retired.** Vectors are stamped
  with the **embedding generation** they were produced under (a monotonic
  counter in `meta`, mirrored by an `embed_gen` column on the rows those vectors
  belong to); a detected change bumps the counter, so one increment retires
  the entire database while every row stays where it is. Keeping the rows is
  what makes re-embedding possible at all — it works from the text they already
  hold — and it makes switching *back* to a previous model cost nothing. Rows
  written before the marker existed carry no generation and read as foreign,
  which is the correct default: every row in an existing installation is one.
- **Each store then follows the cheapest correct route**, all of which already
  existed:
  - **notes and `@self` observations** — a foreign vector is invisible to
    semantic search and its note is listed as needing one, so the existing lazy
    backfill re-embeds it on the next semantic path: self-healing, and it costs
    the user nothing. This is the most valuable half of the fix — nothing ever
    refreshed note vectors, so on a dimension change semantic recall silently
    returned arbitrary notes and every duplicate gate stopped firing.
  - **chat attachment indexes** — a foreign index reads as absent (§9.7), so
    `attachment_search` degrades to its "nothing indexed" answer and
    `attachment_read`, the guaranteed page-by-page path, is unaffected.
  - **the knowledge base** — too large to heal on a read path, and it is the
    user's own data, so the affected profiles are additionally recorded as stale
    and `rag_search` **refuses** over them, naming `/reindex` as the fix.
    Refusing rather than warning: the vectors are in a different space, so
    results would be noise dressed up as answers.
- **Staleness is per profile**, because `/rag rebuild` is. The mark is lifted by
  `/reindex` once that base holds no old-model vectors at all, by a rebuild (as
  soon as the old chunks are deleted, so it stays correct even if the rebuild is
  cancelled or some sources fail — nothing old survives either way), and by
  `/rag remove` once the base is empty, which closes the dead end "removed
  everything, re-added under the new model, still refused".
- **Honesty over noise.** The fingerprint is recorded **after** the generation is
  retired, so an interrupted run redoes it and a healthy launch never
  re-invalidates. A first run with nothing recorded is **silent** — with no prior
  fingerprint there is no evidence anything is stale. The user is told only when
  something was actually affected, and only the knowledge base asks anything of
  them.

**Re-embedding: the `/reindex` command.** Retiring a generation defines a work
queue — *every row whose generation is not the current one* — and `/reindex`
drains it: for each such row, embed the text it already stores, replace the
vector, stamp the generation. It runs in the background with the same progress
banner as `/rag add`, and can be cancelled.

- **Re-embedding is not re-chunking**, which is why this is a command of its own
  rather than a wider `/rag rebuild`. It needs no source text and no chunker, so
  it repairs legacy knowledge-base rows whose stored text is absent and whose
  file is gone (a rebuild counts those as errors and drops them); it covers
  **every** profile in one run instead of one switch per profile; it brings
  attachment indexes back without the user re-attaching each file; and it keeps
  chunk ids stable, so nothing downstream is invalidated. `/rag rebuild` keeps
  its own meaning — re-chunk one profile after a chunking-parameter change.
- **Top-level, not a `/rag` subcommand**, because it spans notes, chat
  attachments and every profile's knowledge base: filing it under the
  knowledge-base family would misdescribe its scope. That scope is not a breach
  of the `profile_id` isolation invariant (§9.5) — the job serves no query, it
  rewrites a row's vector under the partition key the row already carries.
- **Resumable, and safe to interrupt.** Stamping a row removes it from the
  queue, so an interrupted run leaves a consistent partial state and a rerun
  continues exactly where it stopped. The vector is written before the stamp,
  never the other way round, so a crash in between simply makes the row look
  foreign and it is redone. Knowledge-base search stays refused meanwhile: the
  stale marks are lifted only when the queue is genuinely empty, so a cancelled
  or partly failed run correctly leaves them in place.
- **A dimensionality change needs no special case.** The vector tables are
  fixed-width, so they are dropped up front (keeping the document rows, the
  fingerprint and the stale marks), after which every row simply reads as
  foreign and takes the same path. They are recreated at the new width on the
  first write.

**Thresholds follow the model.** Detecting a change and completing it are not
enough on their own, because the gates that decide when two pieces of memory
mean the same thing — the duplicate-pair threshold of the consolidation
overviews (`0.85`), the related-trait gate of `update_user_model` (`0.72`,
[§17.3](#173-tools)) and the summary↔observation overlap (`0.62`) — are absolute
cosines derived from live runs against bge-m3, i.e. positions inside *that*
model's distribution. Cosine distributions differ sharply: measured on a fixed
probe corpus, `multilingual-e5-large-instruct`'s usable range between
"unrelated" and "paraphrase" is **2.6× narrower** than bge-m3's, so used raw its
gates would call unrelated traits duplicates. A correct reindex alone would trade
a silent failure ("the gates never fire") for a loud wrong one ("the gates fire
on everything").

- **The constants keep their values and their meaning**; what changes is that
  they are *read* as positions in a reference scale and mapped into the range
  the active model actually has. The map is affine, anchored on the two
  measured means of the reference model — so bge-m3 maps to itself.
- **Calibration is automatic, not a table of known models.** A table would only
  help models someone has already measured; an arbitrary local GGUF would still
  be handed the reference numbers. Instead a fixed probe corpus — pairs of short
  statements of the kind the gates actually judge, half meaning the same thing
  and half unrelated — is embedded **once**, on the same once-per-model path
  that records the fingerprint, and the two means are stored beside it in
  `meta`. Cost: one extra request of 32 short strings per model change.
- **The absence of a calibration is the identity**, and every failure degrades
  to it: a probe that cannot be embedded or read back, a batch that does not
  describe the corpus, or a measurement that is degenerate (non-finite, out of
  range, or with a zero/inverted span) all leave the thresholds passing through
  **unchanged**. So an installation that has not changed its embedding model is
  unaffected, and a failed calibration can only leave the gates as they were —
  never make them wilder. Calibration failure is logged, never fatal.
- **Where a threshold is shown to the model** (the consolidation overviews print
  the cut-off they selected pairs with), the **effective** value is shown, so
  the number never contradicts the selection beside it.

The probe corpus is a measurement fixture, not prose: it is deliberately
bilingual and phrased in the register the gates operate in, and editing it
invalidates the reference constants and every threshold derived from them. See
[docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)
§6 and §8.2.

#### 9.3.5. Input prefixes (per-model input convention)

Some embedding families expect each input marked with its **role**: base e5 wants
`query: ` / `passage: `, the `-instruct` variants want an instruction-shaped
query and a **bare** passage, and bge-m3 — the model the project is calibrated
against — wants no marker at all. So a convention is a per-model choice of three,
not a switch. Setting: "Input prefixes" in the Embeddings tab, default `none`.
See
[docs/research/embedding-input-prefixes.md](docs/research/embedding-input-prefixes.md).

- **What it buys, measured.** On a 40-document / 14-query corpus the prefixes
  changed **no ranking at all** on e5 (12/14 top-1 under every convention); what
  improves is separation — the mean margin by 15% and the **smallest** margin
  25×, from an arbitrary 0.0002 tie to 0.0056. Real robustness, not a correctness
  fix, and the feature is documented as such rather than oversold.
- **A wrong convention is worse than none**, which is why `none` is the default
  and nothing is ever selected automatically: prefixing bge-m3 costs it a rank
  (11/14 → 10/14) and 31% of its margin. When a model change is detected and the
  new model's *name* suggests a convention, the notice says so — a hint, never an
  action.
- **The role is stated at every call site and has no default.** Only a search
  query against a stored index is `Query`; everything stored, and everything
  compared against something stored, is `Passage` — including the sites that read
  like queries but feed the similarity gates (the `note_save` and `add_insight`
  duplicate gates, the related-trait gate, the summary↔observation overlap). All
  of those are symmetric comparisons, so both sides must be marked the same way;
  a mismatched role on one side costs up to 17% of a compressed model's usable
  range. Web-search reranking is the only genuinely mixed site and issues two
  requests.
- **Turning prefixes on is a change of vector space**, and is treated as exactly
  that: the marker is applied by a decorator sitting *inside* the model-change
  guard, so the canary and the calibration probes go through it. Switching the
  convention therefore bumps the embedding generation and offers `/reindex`, just
  like swapping the model (§9.3.4), and a model's similarity range is always
  measured in the same dressing its real text gets. The canary carries the
  passage role: stored vectors are all passage-role, so changing only the *query*
  marker alters retrieval without invalidating anything and correctly forces no
  reindex.

### 9.4. Enabling tools

- `Profile.enabled_tools` — which tools are available in a given profile's chats (toggles in profile settings).
- Global "master switches" for external tools (web — gates `web_search` and `fetch_url`; Python; file access `fs_enabled` — gates `fs_read`/`fs_write`/`fs_list`) — in the tools settings (privacy/security). The effective set = `enabled_tools ∩ globally enabled`. Safe tools (`calculate`, `current_time`) have no switches.

### 9.5. Per-profile isolation

Any memory/knowledge tool (`rag_*`, `note_*`) gets `profile_id` from `ToolContext` and **must** filter by it — a repository invariant (`shared/storage`), covered by negative tests.

### 9.6. MCP-server tools (plugins)

User-defined tools are connected via **external MCP servers** (Model Context
Protocol, stdio subprocesses) — the "plugins" track, see the research doc
[docs/research/plugin-system.md](docs/research/plugin-system.md) §4 and ADR 0007.
A server's tools become full-fledged `Tool`s in the registry (the `McpTool` wrapper,
`features/tools/mcp.rs`) and are invoked by the standard agentic loop.

- **Protocol**: the tools-only subset of revision 2025-11-25 (wire-stable since
  2024-11-05; any counterpart version reported by the server is accepted) — a
  self-written mini-client `shared/mcp.rs` (newline-delimited JSON-RPC 2.0,
  `initialize` → `tools/list` with pagination → `tools/call`; replying to `ping`,
  `-32601` to other server-initiated requests, `notifications/cancelled` on timeout/
  cancellation, skipping garbage stdout lines).
- **Configuration** — the "Plugins" section of the settings screen, or the `mcp`
  section in `settings.json` by hand (the two are the same data; decision point R6 was
  revisited once the host had proved itself — see docs/history/mcp-server-editor.md):
  a master switch `enabled` (**disabled by default**) + `servers[]`
  (an `id` slug, `command`+`args` — the command is resolved as a shell would
  (`shared::mcp::resolve_command`), so `"command": "npx"` works on every platform:
  on Windows `PATHEXT` completion finds `npx.cmd`, which Rust's own `Command` does
  not do. A bare name is never taken as-is — npm ships an extensionless `npx` next
  to the shim, and it is a Unix script Windows cannot execute. `env` — a map "the
  child's variable → the **name** of the source environment variable"; the source
  may be left empty, which **declares** the variable and leaves its value to a
  stored secret (below). Either way no secret is written into `settings.json` (R8);
  `tool_timeout_secs` — a per-call timeout; `max_result_chars` — result clipping).
- **Secret values for `env`**: a declared variable's value can also be entered in
  the window and is then stored **encrypted with this machine's key**
  ([ADR 0008](docs/decisions/0008-api-key-storage.md)) under the name
  `mcp-<server>-<VARIABLE>` in `config.api_keys` — the same per-machine entry the
  cloud keys and the backup password use, so no new config field and no schema
  change. Each declared variable has exactly **one** origin and the row says which:
  a bare name takes the stored value (and with none stored is left to inheritance),
  while `VARIABLE=SOURCE` takes it from that OS variable and a stored value is not
  consulted for it — the alternative is a row saying "take it from X" while a secret
  silently overrides it. CI and scripted setups are unaffected: they set the variable
  under its own name, which the child inherits. A child
  variable name is restricted to `[A-Za-z0-9_]`: that is what keeps the flat
  variable-list row parseable and the storage name unambiguous. The secret never
  reaches the UI — the settings snapshot carries presence flags only. Orphaned
  secrets (a server renamed or deleted) are **not** collected automatically: the row
  commits on Enter, so a half-typed edit would destroy a value, and `Ctrl+Z` restores
  the config but cannot restore a secret — undoing a server delete brings the server
  back, not its secrets.
- **Import of the ecosystem's `mcpServers` JSON** (`claude_desktop_config.json` and
  the clients that copied its shape; VS Code's `servers` key is accepted too): the
  field takes a **file path**, not pasted JSON — a pasted blob would leave live
  tokens visible on screen and in the editor's undo buffer. The **orchestrator**
  reads and parses it, mirroring `ConfirmMcpCatalog`: such a file carries literal
  secrets, and the orchestrator is the sole writer of `settings.json` and the only
  layer that may touch plaintext. Semantics: imported servers arrive **disabled**
  (nothing spawns until the user says so, and an imported command may not even exist
  on this machine); every literal `env` value is stored as a machine-bound secret and
  its variable declared with an empty source, so nothing lands in `settings.json` in
  the clear; ids are sanitized to our slug (`[a-z0-9-]`, ≤32, a numeric suffix on
  collision within the file); a server whose id **already exists is skipped** and
  reported, so a re-import is a no-op and never overwrites a hand-tuned server;
  non-stdio entries (`"type": "sse"/"http"`, or a `url` and no `command`) are skipped
  and reported — HTTP transport isn't implemented. The row shows a one-line outcome:
  imported / skipped / secrets stored.
- **Lifecycle** — `McpManager` (`app/orchestrator/mcp.rs`, mirroring
  `EngineManager`): spawns enabled servers as background tasks on startup/settings
  changes (through the `RestartQueue` debounce); a process monitor (kill/exited
  tokens, a Job Object kill-on-close on Windows — the process tree doesn't outlive
  the app's exit); a restart budget — 3 crashes within 5 minutes, beyond that →
  `Disconnected` until settings are fixed; generation (`epoch`) events guard against
  races when settings are reapplied.
- **Identifiers**: `mcp__<server>__<tool>` (normalized to provider limits:
  `[A-Za-z0-9_-]`, ≤64, truncated + a hex tail). The id is stored in
  `Profile.enabled_tools` as a plain string; gated in `effective_tool_ids` by the
  `mcp__` prefix + `config.mcp.enabled`.
- **Double opt-in** (R7): a master switch **and** a per-profile toggle
  (`enabled_by_default = false`, `reconcile_tools` doesn't enable it automatically).
- **TOFU catalog pinning** (a rug-pull detector): the first time a server comes up,
  a sha256 hash of the catalog (tool names+descriptions+schemas) is automatically
  pinned to `config.mcp.servers[].pinned_catalog`; if the catalog **changes**, the
  tools aren't registered, and the server row in settings is marked "catalog
  changed" — Enter reconfirms it (the new pin is persisted). Manually removing the
  field from `settings.json` = resetting trust.
- **UI** (the "Plugins" section): a master toggle; a **server editor** — a selector
  plus the selected server's fields (id/command/arguments/environment/enabled/timeout/
  result limit), `Ctrl+N` adds a server and `Ctrl+D` deletes it; and status rows per
  server (`ready · tools: N · in profile: K` / `connecting…` / a failure reason) —
  `K` is how many of them the selected profile has enabled, because a server can be
  up while the model sees nothing (double opt-in), and a row that said only "ready"
  is how a user adds a server and finds it does not work. Arguments are edited
  as a shell-quoted command line and the environment as a comma-separated **list of
  variable names** (`GITHUB_TOKEN, SLACK_TOKEN`), both round-tripping through the
  field; the `VARIABLE=SOURCE` form remains for the rare case of a differently named
  source. The list is only *overrides*: a variable already set in the application's
  own environment reaches the child by inheritance without being listed
  (`Command::envs` adds, it does not replace). Below the variables row sits **one row per
  declared variable**, saying where its value comes from and whether it is there: a
  variable declared by name alone shows its secret's status (`configured (this
  computer)` / `not set`), opens an **empty masked editor** on Enter and deletes the
  stored value on `Del` — exactly the "API key" row's behaviour; one that names a
  source shows a **read-only** `from SRC: found / not found` instead (flagged when
  missing), because it has no value to enter and leaving it silent would make one of
  the two routes undiagnosable. The app sees the environment it was *started* with, so
  a variable set after launch reads as missing until a restart. Plus an "import from a
  file" row taking the path of an `mcpServers` JSON.
  A server created in the UI starts **disabled**, so
  nothing is spawned while its command is still half-typed; the id is validated before
  it commits (slug shape and uniqueness) — an invalid id creates no slot at all, so the
  server would otherwise vanish from the status list. **Enter on a status row** does
  what the row needs: confirms a changed catalog when one is pending, otherwise
  **reconnects** the server — the only way back for one that exhausted its restart
  budget, since an identical config is no longer re-applied (`is_current`).
  Tool toggles live in the profile under a "Plugins (MCP)" group, with the tool's
  **full** description (server-supplied text) shown in the bottom panel on focus —
  mandatory description visibility as an antidote to tool-poisoning (descriptions
  go straight into the system prompt).
- **i18n boundaries**: tool descriptions/schemas are server-supplied text and aren't
  localized; the manager's status reasons are localized (axis B); the client's
  wire-level errors are a technical layer (like the HTTP-client wrappers,
  docs/history/i18n-cli.md §7).
- **Invocation**: a per-call timeout + cancellation via `ToolContext.cancel` (Esc
  sends the server `notifications/cancelled`); the agentic loop additionally wraps
  every tool's `invoke` call in a `select!` with the cancellation token. The result
  is clipped (`max_result_chars`); `isError:true` → the error text is returned to
  the model as the result. Non-text result blocks (image/audio/resource) become a
  text placeholder.
- **Groundwork**: an HTTP transport, resources/prompts,
  `notifications/tools/list_changed`, deferred schemas — see docs/roadmap.md.

### 9.7. Chat file attachments (`/file attach`)

The user attaches a text file to a chat, and the model sees it for the whole
conversation. Deliberately **not** the same thing as `/rag add` (§9.3): that
indexes into the **profile's** knowledge base, needs a configured embedding
server, and retrieval only ever returns the fragments matching a query.
Attachments are **chat-scoped**, need **no embedder**, and are delivered
**in full** — see [docs/file-attachments.md](docs/file-attachments.md).

A file is not the only way one appears: a **tool** can produce an attachment
too, by returning `ChatEffect::AddAttachment` (a video transcript, §9.9). It
then travels the same path in every respect — budget, index, feed note, status
chip, `/file list`, `/file remove`.

- **Commands** (input box, like `/rag`/`/tts`): `/file attach <path>`,
  `/file remove <name|#N>`, `/file list`. As with RAG, the removal subcommand is
  only `remove` — never `delete` (it takes nothing off disk).
- **Storage**: `Chat.attachments` — the **extracted text snapshot** plus the
  name, source path, size and estimated token count. The snapshot means the
  conversation stays coherent if the file later changes or disappears, building a
  request does no I/O, and the chat file stays self-contained for backup/export.
  An additive field — old chat files read without migration.
- **Delivery**: `request::inject_attachments` appends a block of attachments to
  the request's `system` on every turn. Placing it at the front of the prefix
  keeps the rest of the conversation prefix-cached (§6.6); it is re-prefilled
  only when the attachment set changes — the same trade-off already accepted for
  the self-model injection (§17.4). The block header is in the **profile**
  language (axis A) and marks the content as **data, not instructions** (a
  prompt-injection mitigation, §13.4); section fences widen so a file cannot
  close its own section.
- **Two modes.** Within the budget (`config.attachments`, in estimated tokens) a
  file is **inline** — its full text is in the block. Above it, the file switches
  to **by reference**: the block carries its metadata and the head excerpt.
  Attaching therefore **never fails because of size**; a refusal only happens for
  a missing file or content that doesn't decode.
- **Reading a by-reference file — `attachment_read(name, page)`.** Pages
  (`page_tokens`) rather than character offsets: they are discrete and
  enumerable, so the model can walk `1..M` and *know* it has read everything —
  the guarantee retrieval cannot give. The tool reads the stored snapshot, so it
  **narrows** access compared with `fs_read` (only what the user attached, never
  the filesystem) — hence no gate and enabled by default. The by-reference entry
  in the block **states the page range and names the tool**, and says the file is
  unreachable by other means: without that the model improvises with the wrong
  tools (observed live: `fs_read` into the sandbox, then `web_search`) and ends
  up asking the user for the impossible.
- **The displayed cost is what is actually re-sent**: an inline file counts in
  full, a by-reference one counts its excerpt (not its whole size, and not zero).
  The **budget**, separately, is measured against inline text only — that is what
  `max_total_tokens` governs.
- **Formats**: any valid UTF-8 (source code, configs, logs) plus html/pdf/docx
  through the same extractors RAG uses. Undecodable content is refused with a
  clear message.
- **Finding a place by meaning — `attachment_search(query)`.** A by-reference
  file is additionally indexed into a **chat-scoped** semantic index in the
  background (chunking, embedding and vec0 as in RAG). The two tools are
  complementary, not redundant: search answers *where* to look in a file of
  hundreds of pages, `attachment_read` guarantees *everything* can be read. Only
  by-reference files are indexed — an inline one is already in the prompt in
  full, so search would return duplicates of what the model can see. Search
  results are filtered by the turn's attachment snapshot, so a file the user
  removed can never surface.
- **The index is a separate store, not the RAG base**: its own
  `attachment_documents` plus a vec0 table partitioned by `chat_id` (the `k`
  constraint applies **inside** a vec0 partition, so filtering a profile-wide
  search by chat afterwards would silently return fewer than `k` hits). The
  user's curated knowledge base stays clean of per-chat data. The **vector
  dimensionality is shared** with RAG (`meta.rag_dim`, one per DB): a `/rag
  rebuild` that changes the embedding model drops the attachment index too (it is
  derived data — re-attaching the file rebuilds it, and `attachment_read` is
  unaffected).
- **Indexing is best-effort** (the ADR 0002 pattern): with no embedder
  configured it is skipped with a clear note, and the block, `attachment_read`
  and everything else keep working in full. The feature never *depends* on RAG
  being set up. Accordingly, the block only offers `attachment_search` for files
  that actually have an index, and the tool distinguishes "nothing indexed here"
  from "no hits", pointing at page reading in both cases.
- **UI**: a feed note per command, a background-indexing banner with a spinner
  (the same slot RAG indexing uses; one row, **fitted to the window** rather
  than clipped — the folder is left out first, then the name is shortened in
  the middle, and the progress counter always stays), a `§ files: N (~tokens)` status-bar chip
  (attachments cost tokens on every turn — the standing cost has to be visible),
  and budget fields in the settings "Memory" section.


### 9.8. Confirmation for dangerous tool calls

Human-in-the-loop before a tool call that changes something **outside** the
application. Design record: [docs/history/tool-confirmation.md](docs/history/tool-confirmation.md).

- **Off by default**, a single switch: `tools.confirm_dangerous` (a toggle in the
  settings "Tools" section, "Agentic loop" group). When off, nothing is asked and
  no tool is gated — the loop behaves exactly as it did before the feature; the
  registry is not even consulted. The dangerous tools all sit behind master
  switches that are off by default too (`python_enabled`, `fs_enabled`,
  `mcp.enabled`), so this is a **second** layer for users who want to consent per
  call rather than once.
- **What counts as dangerous** is declared by the tool itself (`Tool::danger()`,
  default `false`), like its group and gate: `python_exec` (arbitrary code),
  `fs_write` (overwrites a path), and **every** MCP tool (third-party, effects
  unknown). Reads (`fs_read`/`fs_list`, `web_search`/`fetch_url`) and writes to
  *our own* storage (notes, self-model, RAG, attachments — visible in the UI,
  profile-scoped, reversible) are not. A server's own
  `destructiveHint`/`readOnlyHint` annotations are **untrusted input** and are
  deliberately not consulted: a server can claim anything, so they could only ever
  relax the decision, which is the attack.
- **The popup** shows the call formatted the way the feed will show it afterwards
  (`features::tools::present`), so `python_exec` reads as code rather than as a
  JSON blob; long arguments are cut with "…" — it is a decision prompt, not a
  viewer. `Enter` runs the call, `A` runs it and stops asking about **that tool**
  for the rest of the turn, `Esc` declines. Any other key is ignored and the popup
  stays. `Ctrl+Q`/`F10` punch through to quit, as in every other popup.
- **Declining does not cancel the turn**: the model is told (in the agent-scaffold
  language) and the loop continues, so it can explain itself or take another
  route — ending the turn would throw away the text already streamed. `Esc` with
  the popup gone then cancels the turn as it always does.
- **The allowance is turn-scoped** — the scope of one user request. It ends by
  itself, so no standing permission accumulates that the user would later have to
  remember granting.
- **Waiting is not timed out.** `Esc` and `Quit` both cancel the turn, so a popup
  left open cannot wedge the task; a silent auto-decline would be a surprising way
  to lose work.
- **Implementation note.** The agentic loop runs in a background task that
  otherwise only *emits* events, so this is the one place where something is sent
  back **into** a running task: the orchestrator holds the in-flight turn's
  confirmation sender and routes `AppCommand::ConfirmTool` into it. A reply whose
  `generation_id` is not the turn in flight — the user answered just as the turn
  was cancelled — is dropped, as is one whose `call_id` belongs to another call of
  the same round; either would run a tool nobody looked at.

### 9.9. Watching a YouTube video (`youtube_watch`)

Answers *what a video says **and shows***. Research and the decision points —
[docs/research/youtube-integration.md](docs/research/youtube-integration.md).

**Why it is a tool with its own provider and not part of the engine.** Measured
2026-08-01: Gemini is the only provider that ingests video at all (OpenAI's
Responses API and Anthropic take text and images only), and every free caption
route is closed — a signed `timedtext` URL taken off the watch page returns HTTP
200 with an **empty body** (YouTube's PoToken gate), and `captions.download`
requires the video owner's OAuth. Putting video into the provider-agnostic
`ChatRequest` would therefore give the capability only to users whose *chat*
engine happens to be Gemini. Instead the tool calls Gemini out of band and
returns text into the conversation — the shape TTS already uses (ADR 0009), so
it works on a local `llama-server` or on Claude just the same. The slot is
`config.video` (model / frame-sampling detail / length ceiling / env-var
fallback); the API key is the **shared Gemini provider key** (ADR 0008), not a
value of its own. It can be entered in this group as well as in the "Model"
section — that section offers a key field only for a slot whose mode *is* that
cloud, so a local or OpenAI setup would otherwise have nowhere to put a Gemini
key, while this tool needs one whatever the chat engine is.

**What it returns.** The answer, not the material: a description with
timestamps, narrowed by `focus`. A 10-minute video costs ~62k tokens at the
provider and a few hundred in the conversation — which is what makes it usable
from a local model with an 8k window.

**The words themselves — `transcript: true`** (stage 2,
[docs/history/youtube-transcript.md](docs/history/youtube-transcript.md)). It
costs **exactly the same** as watching: the video is ingested either way, and on
the 3.x models audio is not billed apart from video — so it is **one** provider
call for both halves, split on a marker, and the parameter's own description
says so, or the model would treat the words as a cheap extra. If the marker
never comes the whole answer is the description and the miss is reported: filing
half a description as a transcript would be worse than saying none arrived.

- **Where the words go** is decided by the existing attachment budget
  (`config.attachments.max_file_tokens`) — no setting of its own. Below it, the
  transcript is in the result and the model reads it at once. Above it, it
  becomes a **chat attachment** (§9.7), which is already paged and searchable,
  where a tool result would go into the context whole. An attached transcript is
  therefore **by reference by construction** — inline requires `est <=
  max_file_tokens`, which is exactly the other side of the threshold — so this
  path never competes for the chat's inline budget, and it is indexed for
  `attachment_search` with no special case.
- **The attachment is self-describing**: video, URL, segment, and a line saying
  it was produced by a model from the audio rather than taken from official
  captions. Its source key carries the segment, so transcribing the same span
  twice replaces it while two different spans coexist.
- **Timestamps are absolute** — measured from the start of the video, as the
  header claims. The prompt asks for that, and the provider *sometimes* obeys:
  measured live, the same model numbered a 0:40–1:20 clip from zero on one run
  and absolutely on the next. So the tool corrects the timestamps itself,
  deciding by the first one whether the clip was numbered from zero — a blind
  shift would push an already-absolute transcript outside the segment.
- **Truncation is reported**, in the result and in the file. An answer stopped at
  the output ceiling still looks whole, so a model told "here is the transcript"
  would believe it had read the video to the end — losing the guarantee
  `attachment_read`'s page walk exists to give.

**How the attachment reaches the chat.** Tools do not mutate `Chat`: the tool
returns `ChatEffect::AddAttachment` and the orchestrator applies it through the
same path `/file attach` takes (§9.7), so the index, the feed note and the
status chip all follow, and the object described to the model is the object
stored — `id` included, since that is the key the index is written under. But
effects are applied when the **turn** ends, while `ToolContext.attachments` is a
turn snapshot, so the agentic loop also mirrors the effect into that snapshot at
the end of the round: otherwise "attached as X, read it with `attachment_read`"
would be an instruction the turn itself could not carry out. A turn cancelled
after the call still attaches — the transcript was already paid for.

**Cost is bounded before it is spent.** Measured ≈91 prompt tokens per second of
video on the default (Gemini 3.x) model — where the frame-detail setting turns out
to change nothing — and ≈103/≈295 at low/medium on a 2.5-class one. So the tool
refuses a video longer than
`config.video.max_minutes` (default 30) and says so in terms the model can act
on — the `start`/`end` arguments clip to a segment, which is the only way to look
at a long video without paying for all of it. When the length cannot be read at
all, the request is clipped to the ceiling **and the answer says so**, rather
than silently describing only the beginning.

**Degradation is part of the contract.** Title, channel, length and the author's
description come from the watch page (oEmbed as a fallback) — free, no key, and
they still work with no provider configured. With no Gemini key the tool does
**not** disappear: it returns that metadata plus a plain statement of what is
missing, so the model can explain itself to the user instead of silently lacking
a capability. A provider failure or timeout degrades the same way, with the
reason included. Gated by `tools.web_enabled`, like `web_search`/`fetch_url`.

**A degraded answer closes the door, it does not merely describe the lock.** Each
one states that the video's content is unreachable by any other route available
to the model, and names them: YouTube returns captions empty without a token
(neither `fetch_url` nor `python_exec` gets around that — the sandbox has no
`yt-dlp`, no `ffmpeg` and no `pip`), and no transcript is in web search. Without
that, an agentic model reads "video understanding is not configured" as a local
limitation and spends its rounds rediscovering exactly the dead ends this
project already measured — observed in a live run. The tool description says the
same thing up front, so the choice is informed before the call rather than after
it.

`fetch_url` no longer dead-ends on a YouTube link. The watch page is a
JavaScript shell — measured, zero paragraphs and zero list items — so
readability extracted nothing and the answer was "failed to extract readable
text". It now returns the same metadata block and points at `youtube_watch`.

### 9.10. Images in a message (`/image attach`)

The user attaches an image and a vision-capable model sees it. Research, live
measurements and the fork decisions:
[docs/research/multimodal-images.md](docs/research/multimodal-images.md).

Deliberately **not** shaped like a file attachment ([§9.7](#97-chat-file-attachments-file-attach)),
despite the matching commands. An attachment is chat-scoped and re-injected into
the **system prompt** every turn; an image is **message-scoped** — it belongs to
the turn that introduced it and is replayed afterwards as ordinary history. That
is not a preference: every provider takes images as *message content parts*
only, and a set that could change at the head of the prompt would re-prefill the
whole conversation on every mutation. Measured on the reference stack, the
append-only shape keeps the prefix cache intact across an image turn
(`cache_n = 73` of 78; an appended turn prefills 23 tokens).

- **Commands**: `/image attach <path|url>`, `/image paste`,
  `/image remove <name|#N>`, `/image list` — the same surface and the same `#N`
  addressing as `/file`, so what `/image list` numbers is what `remove` accepts.
  As everywhere, the removal verb is only `remove`, never `delete`.
- **Attaching by address.** `attach` takes a web address as well as a path; the
  two are told apart by the scheme, since no path begins with `http://` or
  `https://`. The bytes are **always downloaded here** and never handed to the
  provider as a URL, even where the provider would take one: that keeps one code
  path across all five engines (Gemini accepts no remote URL at all), keeps the
  format normalization below, and — the deciding reason — puts the pixels in the
  chat, so a link that dies later cannot break a *stored* conversation. The
  address becomes the image's source, so attaching it twice replaces rather than
  duplicates, and its last path segment becomes the display name.
  What the download enforces, and what it deliberately does not: `http`/`https`
  only, **re-checked after every redirect**, at most five hops, connect and
  request timeouts, and the `images.max_bytes` ceiling applied **to the stream**
  as well as to `Content-Length`, which may be absent or untrue. The address
  itself is *not* filtered — this URL is typed by the user, in the same input box
  as `/image attach <path>`, which reads any file on the machine, and a private-
  range filter would break the case these users actually have (an image served by
  a NAS or a local dashboard) to defend them against themselves. The reasoning
  inverts wherever a *model* picks the URL, whose address policy is a separate
  question and a separate roadmap item. For the same reason
  `tools.web_enabled` does not gate this: that switch governs what the tools do
  while the model drives, not what the user types.
  A URL that answers with a page rather than an image is refused **by name** —
  the bytes are judged by the decoder, but the served `Content-Type` goes into
  the message, because linking the page instead of the picture on it is the
  likeliest mistake here and "unsupported format" would send the user looking in
  the wrong place — a message has to close the door.
  Forks and the live check: [docs/research/image-url-attach.md](docs/research/image-url-attach.md).
- **Pasting from the clipboard** has **two** routes, and the command is the
  reliable one. Whether `Ctrl+V` ever reaches the application is the *terminal's*
  decision: Windows Terminal binds it to its own paste, and since an image on the
  clipboard produces no text to inject, the app would see nothing at all — the
  same shape as the supplementary-plane paste that produces no key events
  ([§11.5](#115-input-and-editing-spellcheck)). So `/image paste` exists, works
  in every terminal, and is what the help overlay names; `Ctrl+V` is a
  convenience where the terminal forwards it. When it does and the clipboard
  holds no image, it pastes **text**, which is what the overlay has always
  promised that key does.
  The clipboard is read by `runtime`, not by the orchestrator — a UI-layer side
  effect, like `Ctrl+C` copying — so the orchestrator is handed pixels rather
  than a request to go and look. arboard normalizes each platform's storage
  (`CF_DIB`, `image/png`, `NSImage`) to raw RGBA, so there is no container to
  sniff; the encode happens on the blocking pool with everything else.
  A pasted image has no file, so it gets the first free `clipboard*.png` name and
  a source that cannot collide — pasting twice stages two images rather than the
  second replacing the first. It is **always encoded as png**, unlike a file,
  whose photographic formats become jpeg: the dominant clipboard image is a
  screenshot, and jpeg artifacts on small text are the expensive failure.
- **Staging.** `attach` does not create a message; it stages the image for the
  **next** one. Staging lives in the orchestrator, is keyed by chat, and is
  **session-only** — what was staged and never sent is a half-finished thought,
  not conversation state, and persisting it would resurrect images into a
  message written days later. The send consumes the staged set; from that moment
  the image is history and `remove` says so rather than pretending otherwise.
- **A message with only an image is a complete request** ("look at this"), so an
  empty send is refused only when nothing is staged either.
- **Storage**: the payload is base64 **inside the chat file**, next to the
  message (`Message.images`, additive — old chats read unchanged, ADR 0006 F12).
  The chat stays self-contained for backup/export and building a request does no
  I/O — the same doctrine as an attachment's text snapshot. What makes it
  affordable is the preparation below.
- **Preparation at attach time** (`features::image_prepare`): decode, downscale
  the long edge to `images.downscale_px` (1568 by default — Anthropic's
  standard-resolution ceiling, comfortably above the ~256-token encoder budgets
  measured on llama.cpp, Gemini and xAI), and normalize the format to **png or
  jpeg**. Both halves earn their keep: xAI accepts nothing else, so an
  un-normalized webp would be attachable on three providers out of four and fail
  at send; and an unscaled phone photo would ride *every* turn, billed and
  uploaded whole each time. An image that is already png/jpeg and already within
  the ceiling is passed through **byte for byte** — a screenshot keeps its exact
  pixels, since recompression is what makes small text unreadable. png stays
  png, transparency forces png, everything else becomes jpeg q85.
- **Delivery**: `request::message_to_api` turns `Message.images` into content
  parts, and each backend emits its own shape — `image_url` with a `data:` URI
  (llama.cpp Chat Completions **and** xAI, byte-identical), `image`/`source` for
  Anthropic, `inline_data` for Gemini, `input_image` for Responses. Each image
  is preceded by a short label part (`Image #1 — "chart.png":`) in the
  **profile** language (axis A), which is what lets the model refer to a file by
  the name the user sees. Images come before text, the ordering Anthropic
  documents.
- **A request with no images is byte-identical to what the app sent before the
  feature existed** — content stays a bare JSON string rather than a
  one-element parts array. Pinned by a test, because llama.cpp reuses its prefix
  cache on a matching rendered prompt and Gemma's template routes the two shapes
  through different branches: a blanket switch would silently re-prefill every
  existing conversation.
- **Capability, asked rather than guessed** (`EngineBackend::vision`): llama.cpp
  answers from `/props` (`modalities.vision`) — the same fetch `context_budget`
  already makes; the clouds answer statically. A model-name allowlist is
  deliberately not used: it goes stale and then lies. Three outcomes, three
  behaviours — **no** refuses the attach and names what would fix it (the
  `--mmproj` setting for a managed server, the model/provider choice otherwise);
  **yes** attaches silently; **cannot say** (any server without `/props`)
  attaches with a neutral note, because refusing there would break every vLLM or
  LM Studio user running a vision model.
- **The managed server** gains `--mmproj` (settings, or `MINDFORK_MMPROJ`), with
  the same missing-file preflight the draft model has: a typo'd projector must
  fail loudly rather than start a server that is silently blind.
- **Limits** (`config.images`, spec §12.1): `max_count` per message (8),
  `max_bytes` per file (10 MB — the strictest provider, checked before decoding
  so an enormous file never reaches the decoder), `downscale_px`. The count cap
  is checked *before* the work, so a user at the limit is told immediately
  rather than after a multi-second decode.
- **Token accounting**: the displayed estimate is Anthropic's patch formula
  (`⌈w/28⌉ × ⌈h/28⌉`), which over-estimates the local stack and roughly matches
  Gemini/xAI — the safe direction for a cost shown to the user. The
  auto-compaction trigger is unaffected: it reads the server's exact
  `usage.prompt_tokens`, which **includes** image tokens (measured).
- **A tool can return an image too** (spec §9.6, an MCP screenshot tool is the
  first producer; research: [docs/research/mcp-tool-images.md](docs/research/mcp-tool-images.md)).
  It lands on the tool-result message and is delivered *inside* the tool result —
  measured, llama.cpp, Anthropic, OpenAI Responses and xAI all accept it there.
  **Gemini is the exception**: a multimodal `functionResponse` is a hard `400`
  ("Multimodal function responses are not supported for this model"), so its
  builder emits the image as user parts immediately after the response. The
  choice is made **per provider, statically** — never by sending and catching the
  error, since a mid-turn retry after a hard failure is exactly what the retry
  decorator refuses (§6.8). Limits are the ones above plus a **cap of 4 images
  per tool result**, and the extras are named in the result text rather than
  dropped in silence. The switch `tools.mcp_images` (**on** by default, settings
  → "Plugins") decides whether a server's pixels reach the model at all: the MCP
  double opt-in already gates the *server*, but an image carries a hazard text
  does not — see §13.4.
- **UI**: a feed note per command and a status-bar chip for what is staged
  (images are a standing cost once sent, so the pending one has to be visible).
  Rendering the pixels in the terminal is out of scope — see
  [docs/roadmap.md](docs/roadmap.md).

### 9.11. Cross-chat search: `chat_search` and `chat_read`

The model-facing counterpart of the chat-list content search (§11.2.1): the
assistant can find and read what the *other* conversations of the current
profile said, for when the user points across conversations ("we discussed
this in another chat"). Design and the decided forks —
[docs/research/cross-chat-search-tool.md](docs/research/cross-chat-search-tool.md).

- **`chat_search { query, top_k? }`** runs the escaped query (the single
  `to_fts_query` escaper, §11.2.1) over the full-text index the application
  already keeps (`cache.db`, §5.2), scoped **in SQL** to the profile's other
  chats (`CacheDb::search_messages_in`) — the index itself is profile-blind,
  and a post-filtered global `LIMIT` could be starved by another profile's
  rows. Hits come back grouped by conversation (the UI's "group, don't rank"
  decision), newest conversation first and hits in timestamp order; each hit
  carries a 480-character snippet, its role and date, and the **page** of
  that conversation's transcript, and each conversation is addressed by its
  title plus a short id (8 hex of the uuid). `top_k` defaults to 5 and is
  capped at 20; if the 200-hit scan cap bites, the honest total is counted
  and stated.
- **One address, `chat://<short-id>`.** Both tools print a conversation's
  address in that form, `chat_read` accepts it back (id prefix with the
  scheme stripped, then the title ladder), and the descriptions tell the
  model to **cite it when it mentions a conversation to the user** — the
  feed then draws it as a link ([§11.3](#113-the-message-feed)). The scheme
  is minted in one place (`features::chat_links`), so what a result hands
  the model is exactly what it should write back and exactly what the
  interface can resolve; teaching it costs nothing while the pair is off,
  because a disabled tool is not advertised at all. Observed before it was
  designed: a model invented `chat://` out of the bracketed address this
  section used to print (grok-4.6, 2026-08-14) —
  [docs/research/chat-uri-links.md](docs/research/chat-uri-links.md).
- **`chat_read { chat, page? }`** reads one conversation as a paged
  transcript — the same renderer (`HistoryView`) and page size
  (`compaction.page_tokens`) as `history_read`, which is what makes the page
  a hit names the page a read returns. `chat` resolves by short-id prefix,
  then exact title, then title substring; several matches report the
  ambiguity with the candidates' addresses, and an unknown reference points
  back at `chat_search`.
- **Scope** is a turn snapshot (`ToolContext::other_chats`), built in one
  place (`snapshot_other_chats`): the current profile's chats only (§9.5),
  the **current chat excluded** (its visible half is the model's own context;
  its folded half belongs to `history_search`, §6.7), hidden chats dropped.
  **Sub-agent transcripts are in** ([§9.3.2](#932-call_subagent);
  docs/research/subagent-chats.md F4): every transcript of those chats *and
  of the current one* — its transcripts are not in the model's context — is
  a conversation of its own in the snapshot, with its own `chat://` address
  and a label naming it as a sub-agent transcript of its parent; the index
  scopes a transcript by its own id (§11.2.1), and `chat_read` reads it out
  of the parent's file. `chat_read` re-checks the loaded file against the
  same boundary, so a stale snapshot cannot leak across profiles — and a
  transcript whose exchange was taken back reads as unavailable. Only
  `message.text` is searchable — thoughts and tool results are not in the
  index (roadmap: "widening what chat search indexes").
- **Off by default.** Both tools are optional (`enabled_by_default = false`):
  present in the per-profile Tools catalog, but no profile gets them without
  the user's hand — `reconcile_tools` never auto-enables optional tools — and
  while off they are not advertised to the model at all. That is the abuse
  containment the feature was designed around: a deliberate per-profile
  opt-in plus bounded output; no new per-turn call limit (`max_tool_rounds`
  bounds the loop, as everywhere else).
- **Degradation** follows the read-back pair: a profile with no other chats
  answers "nothing here" (as distinct from "no hits"); an unavailable index
  degrades to a normal answer naming the route that still works; a
  sub-trigram query, a bad page and an ambiguous reference each state what to
  do next.

---

### 9.12. The code workspace: `/project` and the `code_*` tools

The user attaches a **project directory** to a chat, and the assistant can list,
read, search and change it, and run the build/run/test commands the user
configured for it — and the user can see everything it changed, as a diff, and
put any of it back. The finished track is
[docs/history/code-workspace.md](docs/history/code-workspace.md). Its last
planned stage, an optional **semantic index** over the project, was built as a
probe, measured against `code_grep` and **rejected** — it could not be shown to
improve answers or reduce rounds on this codebase, and §7.9 there records the
measurement and what would change the answer. `code_grep` is the search this
feature has.

Deliberately **not** the same thing as `fs_read`/`fs_write`/`fs_list` (§9.3),
which stay exactly as they are. Those are a global capability behind
`tools.fs_enabled`, reaching the whole file system unless `tools.fs_root` narrows
them. The workspace family is the opposite shape: it is scoped to **one chat**
and **one directory the user pointed at**, and with nothing attached it does not
exist at all.

- **Commands** (input box, like `/file`): `/project attach <directory>`,
  `/project detach`, `/project status`. The path may contain spaces and may be
  quoted; it is canonicalized when attached, and a path that is missing or is not
  a directory is refused with the reason. `/project attach` on a chat that
  already has a project replaces it. The command slots are set the same way —
  `/project build-cmd|run-cmd|test-cmd [line]` (with a line: set it; without:
  show it) and `/project clear build|run|test`. Clearing is its own subcommand
  rather than "set to nothing", because a command line can legitimately be any
  string, so an empty argument would be ambiguous where a word is not.
  `/project status` lists all three slots, the empty ones included: the question
  it answers is what the assistant can run here, and an answer that hides the
  empty slots cannot say "none of them".
- **Storage**: `Chat.workspace` — the canonical root and the three command
  lines, the root stored in the **readable** form (Windows canonicalization yields `\\?\C:\…`, which would otherwise travel
  into the system prompt and every tool result). An additive field: old chat
  files read without migration, and a chat with no project writes no new key
  (ADR 0006 §8). A project is a path on *this* machine, so an imported chat never
  carries one.
- **Attaching is the consent.** There is no second switch: the `code_*` tools are
  offered to the model only while a project is attached (`effective_tool_ids`),
  so a chat without one sends **byte-identical** requests to what the app sent
  before the feature existed. The profile's own per-tool toggles still apply on
  top, and a project attached to a profile with the tools switched off is told to
  the model as such rather than advertised (see the block below).
- **The tools** (group "Files", no global gate, and **on** in a profile's tool
  set by default — the project's presence is the permission, and making the user
  attach a directory *and* tick five toggles would contradict that):
  - `code_list(path?, depth?)` — the shape of the project or of one directory,
    `.gitignore` honoured, hidden entries and `.git/` skipped, depth 2 by
    default;
  - `code_read(path, offset?, limit?)` — the file with **line numbers** in the
    form `   12→text`, a header stating the total, and a window over a long file;
  - `code_grep(pattern, path?, glob?)` — a regular-expression search returning
    `path:line: text`, smart-case (a lowercase pattern matches any case), with
    `glob` narrowing the files;
  - `code_edit(path, old_string, new_string, replace_all?)` — replaces an exact
    fragment. `old_string` must occur **exactly once**; a fragment that is
    missing, or occurs more than once without `replace_all`, changes nothing and
    the answer says which of the two it was. The result echoes the changed lines,
    numbered, so a second read is not needed to verify;
  - `code_write(path, content)` — creates a file (with any missing directories)
    or replaces one whole. Its description sends the model to `code_edit` for a
    change inside an existing file, since a whole-file write can silently drop
    what it did not mention;
  - `code_build()`, `code_run()`, `code_test()` — run the corresponding slot's
    line. **No arguments at all**, by schema: the line is the user's, and the
    model can neither change it, add a flag to it, nor point it somewhere else.
    A slot with no line means its tool is not offered.
- **Running a command** (spec-level behaviour, `features/tools/code.rs`):
  - **No shell.** The line is split into `argv` the way a shell would split it
    (quotes honoured) and the program is spawned directly, completed from
    `PATHEXT` on Windows so one line works on both platforms. A pipeline or a
    redirect therefore cannot run, and the refusal says so **when the line is
    set** — naming the character it found and the route that works (put the
    steps in a script, name the script) — rather than surfacing three turns
    later as a program that could not be found. The same check runs again at
    execution time, because a chat file is JSON on disk and can be edited by
    hand.
  - **The whole process tree is killed** on the timeout and on `Esc`: a Job
    Object on Windows, a process group and `killpg` on unix
    (`shared/proc.rs`). Killing only the process we spawned would leave
    `cargo`'s `rustc` children compiling — the reason this is not `kill_on_drop`
    like the Python sandbox, which runs its interpreter *inside* the process it
    kills.
  - **Partial output is kept** when a command is stopped, and the answer says it
    timed out. That is the deliberate inverse of the sandbox, which discards it:
    a build's first errors arrive in its first second, and throwing them away
    because the build was slow wastes the whole wait.
  - The command runs with the project root as its working directory, with
    `NO_COLOR=1`/`CLICOLOR=0` and any surviving ANSI escapes stripped. Output is
    capped per stream and cut from the **middle**, keeping head and tail: a
    compiler puts its first errors at the top and its summary at the bottom.
  - **One at a time**, across the application: a second command is refused with
    a message saying to wait, rather than queued — two builds of one project
    fight over the same output directory.
  - The result carries the command line, the duration and the exit code in the
    same shape `python_exec` uses, so the feed renders it as a console with no
    new widget code.
- **The read format is a contract.** The line-number prefix is a reading aid the
  model must strip when it quotes a fragment back; stage 0 measured both live
  model families doing exactly that, byte-for-byte, including indentation
  (docs/history/code-workspace.md §7). It is what the editing stage rests on, so it is
  not changed casually.
- **Confinement.** Every path argument is canonicalized and must lie inside the
  root, which settles `..`, an absolute path elsewhere and a symlink pointing out
  in one check; the walker does not follow links, for the same reason. Reads
  refuse binaries and files over 2 MB, and every listing, search and read states
  when it was truncated.
- **`.gitignore` is honoured even outside a git checkout** — measured, the
  walker's default consults it only inside a repository, and an attached
  directory that is not one would have its ignore file silently disregarded (on a
  Rust checkout, that is `target/`: most of the bytes on disk).
- **The system prompt** carries a block naming the project and its root, marked
  as **data, not instruction** (§13.4), and naming the tools that reach it. When
  a project is attached but the profile has the tools off, the block says the
  project is out of reach instead — a block promising an absent tool is how a
  model spends a turn improvising with the wrong ones (§9.7 learned the same).
- **Editing keeps the file's own shape.** Matching runs on text normalized to
  `\n` (a model writes `\n`; a Windows checkout is CRLF), and the file is written
  back with its original line endings and BOM. Without that, one edit reads as a
  whole-file rewrite in the diff and in the user's own version control.
- **Every change is journaled first.** Before a file is changed for the first
  time in a chat, its bytes are copied to `data/workspace/<chat-id>/` together
  with a manifest row (`features/workspace_journal.rs`). Only the **first** touch
  is recorded — the baseline is "as it was before the assistant started" — and a
  change that cannot be journaled is **refused rather than applied**: the user's
  control over what the assistant did is the changes screen and its revert, and
  both rest on those bytes, which exist nowhere else once the file is
  overwritten. The journal lives under the app's data root, never inside the
  user's project, and detaching the project drops it.
- **The journal belongs to one project, and says so.** Its manifest records
  the root it describes, and attaching a *different* directory drops it at
  that moment. It has to be that moment rather than the assistant's next
  write: in between, the changes screen would diff the previous project's
  baselines against the new root, and reverting a file the two projects both
  have (`Cargo.toml`, `README.md` between sibling repositories) would write
  the previous project's bytes into this one. Both readers check the recorded
  root as well, so the guarantee does not rest on the attach path alone — a
  journal that describes another project is shown as no changes and refuses
  to revert. Re-attaching the **same** directory keeps the journal.
- **The editing and command tools declare themselves dangerous**, so
  `tools.confirm_dangerous` (§9.8) parks them for confirmation when the user
  wants that. The reading tools do not, exactly as `fs_read` does not. A command
  tool is dangerous for a different reason than an edit: it runs the project's
  own code (`build.rs`, an npm script, the test suite). That is inherent to the
  feature and already consented to twice — the user typed the line and attached
  the directory — but it is exactly what someone who turns the setting on wants
  to be asked about.
- **The `code_*` tools are excluded from the tool-round limit.** The limit exists
  to stop a model looping on *external* work, where each round is a request and
  possibly money; a code fix is read → change → check, and a budget of eight
  rounds ends it halfway. A round is counted when **any** call in it counts, so
  mixing a `web_search` into a round of reads still spends one — the exemption is
  not a way round the limit. Exempt is not unbounded: a turn that spends
  `workspace.max_rounds` consecutive rounds inside the project ends the way the
  ordinary limit ends one (a final round without tools), because a repeated tool
  call is a measured failure mode of local models rather than a hypothetical one.
  That number is a setting, defaulting to **500**, and **0 switches it off** —
  turning it off is a legitimate choice for a long refactor, and `Esc`, the
  per-command timeout and the one-at-a-time gate remain underneath it either way.
  The message that ends such a turn names **which** of the two limits fired and
  the setting that raises it.
- **Settings** (§11.6, "Tools" → "Workspace"): `workspace.command_timeout_secs`
  (300), `workspace.output_limit_chars` (10 000 per stream) and
  `workspace.max_rounds` (500; 0 — off). A group rather than a section of its
  own: the capability's gate is a *project*, attached from the chat, so there is
  nothing to switch on here.
- **The system prompt** names the tools **this turn actually offers**, one by
  one, and quotes each configured command line verbatim. The model reading the
  line is deliberate: it is what lets the assistant tell the user that their own
  command is the thing that is wrong, and it is the whole of its say over one.
- **The changes screen** (`F4`, or `/changes`) is the other half of what the
  editing tools promise. They apply a change without asking — there is no
  per-edit popup by design — and what makes that safe is that every change is
  visible afterwards and any of it can be put back. Two panes: the files the
  assistant touched, and the selected file's unified diff against its journaled
  pre-image. `↑↓` picks a file, `Tab` hands the arrows to the diff, `PgUp/PgDn`
  scrolls it, `r` puts one file back behind a confirmation naming it, `Esc`
  returns to the chat.
  - The diff is **against the journal, not against git**, for the reason the
    journal exists: an attached directory need not be a repository, and a
    repository routinely carries the user's own uncommitted work, which is not
    the assistant's doing.
  - Every way a file can fail to have a diff is a **state that says which it
    is** — created, gone from disk, binary or too large to show, or touched and
    left exactly as it was. An empty pane for all five would be
    indistinguishable from a defect.
  - Reverting restores the bytes **and** drops the journal row in one step: a
    restored file still listed would offer a second revert that does nothing,
    and a dropped row whose file was not restored loses the pre-image for good.
    A file the assistant *created* is deleted instead — the only deletion in the
    feature, and it is the user who asks for it.
  - Line endings alone are not a change (matching is on `\n`-normalized text, as
    in the editing tools), and a path that would leave the root is neither
    diffed nor written — re-checked here because a manifest is a file on disk.
- **UI**: a feed note per command. The attach note names the root and what the
  assistant can now do; `/project status` with nothing attached names the command
  that attaches something; setting a slot echoes the line, and clearing one that
  was already empty says so rather than answering with silence.

## 10. AI-companion profiles

A concept carried over in full from attempt #1 (section 10 of its specification):

- **Unique `Profile.id: Uuid`**; a chat is tied to a profile (`Chat.profile_id`).
- **Data isolation**: notes and RAG are separate per profile (`WHERE profile_id = ?`).
- **Creating a chat from a profile**: `default_system_message → Chat.system_message`, `character_names`, `greeting` (as the assistant's first message), and `default_sampling` are copied over.
- **Editing a profile** doesn't affect already-created chats (they have their own copy of `system_message`). **Exception — role names** (`character_names`): they're display-only, so they're resolved from the profile at render time (the chat's copy is kept but not used) and a rename applies to existing chats immediately.
- **Deleting a profile** is soft (`is_hidden`), cascading to hide its chats and exclude its notes/RAG.

A profile holds: a unique identifier, a system message, and an **optional greeting message** (some models are more interesting when the assistant speaks first) — a direct requirement from the task.

**Role names** (`Profile.character_names`, settings → "Profiles" → the "Persona" group): what the interlocutors are called in this profile's chats — the feed's headers (in caps: `GAIA` instead of `YOU`) and the labels of the `F5` conversation export (`Gaia:` instead of `User:`). Both fields are **empty by default** — then each surface uses the interface language's own label, so the chat follows axis B until the user overrides it. The names are resolved at render time from the profile (see the exception above), so they can be changed at any point, including for old chats. Whitespace-only counts as unset. Legacy seed values (the pre-i18n Russian placeholders, and the `You`/`Assistant` they were migrated to) were never displayed and are cleared once at startup so the labels don't get stuck in a foreign language.

**Scaffold language** (`Profile.language: Lang`, i18n Tier 1, [docs/history/i18n.md](docs/history/i18n.md)): the language of background-task prompts, the "self-model" scaffold, and tool results — text that the **model** reads (axis A). This is **not** the language the model answers in (that's set by the system message) and **not** the interface language (axis B, separate). Chosen when the profile is created and then **locked** as soon as the profile has data (visible chats / a non-empty self-model / notes) — so that the entire profile's memory stays in one language; the gate is authoritatively checked by the orchestrator (`profile_has_data`), and the UI renders the "Scaffold language" field as locked. The default (bootstrap) profile is created together with a default chat → immediately locked to `Ru`; for a different language, a new profile is created instead. Resolution — `profile.language` at request time; scaffold text is fetched through `shared/i18n` from the built-in `locales/<lang>.json` bundle, with external `data/locales/*.json` layered on top at startup (Tier 3 — overriding built-in text and adding new languages with no rebuild, [docs/history/i18n-external-locales.md](docs/history/i18n-external-locales.md)). Tiers 1–2 translated the hot core and all 35 tools; `Lang` is `Ru`/`En`/`Ext(code)`.

---

## 11. TUI: interface and interaction

### 11.1. Screens and navigation

Two main screens + overlays (modals):

```
┌──────────────────────────── Chat screen ─────────────────────────────┐
│ ▸ chat list (overlay, Esc)                Title: <chat> · <profile>   │
├──────────────────────────────────────────────────────────────────────┤
│  Message feed (markdown, scroll)                                      │
│   • role/name; ▸ collapsed "thoughts" block; ▸ collapsed tool-call block │
│   • "assistant is using a tool…" indicator                            │
├──────────────────────────────────────────────────────────────────────┤
│  Input box (our own multiline widget, spellcheck)                     │
├──────────────────────────────────────────────────────────────────────┤
│ status: model · tokens/context · profile · server state               │
└──────────────────────────────────────────────────────────────────────┘
```

- **Settings screen** — a separate screen (via `Ctrl+P`/a button) with sections (see [11.6](#116-the-settings-screen)).
- **The message-level search screen** — a separate screen opened from the chat list's content mode (`Ctrl+G`): the messages matching the query, with `Enter` jumping the feed onto one and highlighting the query inside it; `Esc` from that chat comes back to the results (see [11.2.1](#1121-the-message-level-search-screen)).
- **The "self-model" screen** — a separate screen (`F3`) for viewing and editing the agent's self-model (see [17.7](#177-ui--the-self-model-screen-f3)).
- **Overlays**: the chat list, profile picker when creating a chat, confirmations, the spellcheck-suggestion popup, key-binding help (`?`), the server startup log.
- **Token counter** in the status bar: the total "conversation (prompt) + response" count,
  updates live during generation and is visible right from the start (not "starting from one").
  There's a dual source: a live approximation from the number of streamed deltas + an exact
  number from the server's `usage` block (requested via `stream_options.include_usage`; arrives
  at the end of the turn). Until the exact number arrives, the conversation is shown as a client
  **estimate** (the "UTF-8 bytes / 4" heuristic), marked with `~`; the exact `prompt_tokens`
  replaces it. The response counter accumulates across agentic-loop rounds. An external server
  without `usage` support keeps the estimate + the approximation (graceful degradation).
  A protocol detail this depends on: llama.cpp sends the `usage` chunk **after** the one
  carrying `finish_reason` (`choices` empty, then `[DONE]`), so the client holds the reason
  until the stream's own terminator instead of ending on it — otherwise the exact numbers are
  discarded on every turn and the `~` estimate never goes away. History compression ([6.7](#67-history-compression-rolling-summary))
  reads the same exact figure, and reads nothing else.
- **Server state** in the status bar — compact **chips** (a glyph + a label) per
  server, since there can be several (chat / embeddings / impersonation). The glyph
  encodes the status: `●` ready (green), `◐` connecting (amber), `✕` no connection /
  not configured (red/amber). **The chat server is always shown** and, on a
  disconnect, carries the reason text (`✕ chat: no connection: <why>`) — it blocks
  generation. **Embeddings and impersonation** are separate `emb`/`imp` chips shown
  **only when configured** (`NotConfigured`, incl. impersonation in `shared` mode →
  the chip is hidden), with no reason text (compact; details are in the logs/settings).
  A snapshot of all statuses (`ServerStatuses`) is emitted by the orchestrator
  (`AppEvent::ServerStatus`) on any change to any of them. **All three servers are
  monitored alike**: a configured one starts at `Connecting`, and a background
  `/health` probe resolves it to `Ready`/`Disconnected` (the cloud has nothing to
  load → `Ready` at once, no monitor). The embeddings probe doesn't make RAG eager —
  it's a `/health` GET, the embedder itself is still touched only on a real call
  (ADR 0002) — and it gates nothing: it exists so a configured-but-unreachable
  embedding server can't report itself ready (before it, the status came from the
  settings alone and the failure surfaced only on the first `rag_search`).
- **The status keeps up to date**, rather than describing the moment the server was
  configured: the monitor re-checks every 60 s while healthy and every 5 s while
  down, flips to unavailable only after 3 consecutive failures (one success
  restores it), and publishes only on a change. A **managed** child that dies is
  reported at once from its exit signal and relaunched under a crash-loop budget
  (≤3 per 5 min); external/cloud servers are never relaunched by us — their
  monitor simply picks the recovery up. This also means a server started *after*
  the app now becomes usable on its own: previously the chat gate kept generation
  blocked until a restart or a settings edit. See
  docs/server-health-monitoring.md.
- **Hotkey hints** in the status bar are fixed except one: **`Esc` means "go back"**,
  and where back is depends on how the chat was reached, so its label follows the key
  — the chat list normally, the search results when the chat was opened from a hit
  ([§11.2.1](#1121-the-message-level-search-screen)). The bar is the only place on
  screen that answers this, so a label still saying "chats" would be the old answer in
  exactly the flow that needs the new one. It is **derived** from the navigation state
  once per frame rather than mirrored into a flag, which makes the hint true of every
  frame by construction. The wording is constrained by the grid: the hints are laid out
  in right-aligned columns whose width comes from the longest cell, so a label a few
  characters longer costs a whole extra row at common widths — the alternative label is
  a short **directional** one ("to search"), which also can't be misread as
  "`Esc` opens search".

### 11.2. The chat list (an overlay)

A direct requirement from the task:

- Invoked with the **`Esc`** key (the same key closes the overlay — toggling "list ↔ chat"). Quitting the application — `Ctrl+Q`/`F10` (including from this overlay).
- **A search field with two scopes**, toggled by **`Ctrl+F`** (the active one is shown as
  `search: titles` / `search: content`). **Titles** is the default and the historical
  behaviour — a substring match on the title, filtered locally. **Content** is a full-text
  search over the **text of the chats' messages** ("thoughts" and tool calls are
  deliberately out of scope: they would match on words the user never wrote). Content mode
  **filters** — the list keeps the chats with at least one matching message, and the sort
  mode below still orders them; stage 1 deliberately does *not* rank, because the trigram
  index's `bm25` is weak, and ranking becomes a real question only once hits are individual
  messages. The title substring filter is **not** additionally applied in this mode: the
  query has already been answered by the index, and re-applying it to the title would hide
  the very chats the search just found.
- **How a content query matches**: by **substring**, like the title filter users are
  already trained on — which also sidesteps Russian morphology, for which FTS5 has no
  stemmer. The query is turned into a **literal** match by the orchestrator, so ordinary
  text searches for itself instead of erroring (`C++`, `cost-benefit` and `AND` are all
  invalid FTS5 syntax when passed raw); several words mean "all of them, in the same
  message"; and tokens shorter than 3 characters cannot match under a trigram index, so
  they are dropped — a query with nothing left shows the unfiltered list, exactly like an
  empty one. The index itself is a separate, disposable `cache.db`
  ([§5.2](#52-data-storage)). See
  [docs/research/chat-content-search.md](docs/research/chat-content-search.md)
  and [docs/history/chat-search-stage2.md](docs/history/chat-search-stage2.md).
- **From "which chats" to "where exactly"** (content mode only, since in title mode
  the query is a title substring and there is nothing to search message text for):
  **`Ctrl+G`** opens the message-level results screen ([§11.2.1](#1121-the-message-level-search-screen)),
  and **`Enter` opens the selected chat at its first matching message** instead of at
  the tail — the user asked where this text is, so landing on it beats landing at the
  end of the conversation. Which message that is can only be answered by the
  orchestrator (it owns both the index and the chat, so only it can order the matches
  by real chat position), so the list hands over the chat and the raw query
  (`AppCommand::OpenChatAtFirstMatch`); a chat whose match can't be resolved simply
  opens at the tail, exactly like a plain switch. Title mode keeps the historical
  plain switch, and the `Ctrl+G` hint is shown only in content mode — an advertised
  key that is a no-op is worse than a missing hint.
- **Two sort modes**: by creation date and by last-modified date (toggled with a key; the active mode is indicated).
- **List navigation**: `↑`/`↓` — one row, `PageUp`/`PageDown` — one page (a fixed step, since the list height is only known at render time), `Home`/`End` — to the first/last chat.
- **Renaming** a chat in place (`F2`) — in a **single-line `InputBox`**
  (`set_single_line`, see [11.5](#115-input-and-editing-spellcheck)/[11.6](#116-the-settings-screen)),
  which gives spellcheck (error underlining), word-wise navigation/deletion
  (`Ctrl+←/→`, `Ctrl+Backspace/Delete`), `Ctrl+Home/End`, clear/restore (`Ctrl+K`),
  clipboard paste, and horizontal scrolling of a long title. The spellchecker lives in
  the chat screen; `app` lends it to the list screen for highlighting.
- **A model-written title** (`Ctrl+R` in the list): the model reads a digest of the
  conversation (roles tagged, start+end when long — `features/rename_chat.rs`) and
  answers with a short title in the conversation's language: one single-turn background
  request with no history/tools and reasoning muted (`orchestrator/title.rs`), cleaned
  (`clean_generated_title`) and applied via `ChatRenamed`; failures report into the
  list overlay. **The same task also runs by itself** — `interface.auto_title`
  (the "Interface" section's Behavior group, a tri-state) fires it once per
  conversation: *after the assistant's first reply* (the **default** — the reply is
  what disambiguates a terse opening, so the name is measurably better, and the
  engine is idle by then), *after the user's first message* (the cloud-chat-UI
  timing — the title appears while the reply streams; its request is deliberately
  fired after the reply's own, since on a single-slot `llama-server` it would
  otherwise queue ahead of the answer), or *off*. "First reply" means the first
  **substantive** one: a cancelled or failed first turn defers the title to
  whichever turn actually answers, an existing conversation (which already has
  replies) is never retitled on upgrade, and regenerating the first reply
  re-titles — the name follows what the exchange became. A chat the user renamed
  (`F2` or `/rename <title>`) sets `Chat.renamed_manually` and is never touched;
  the flag is checked again when a result lands, so a rename made while the task
  runs wins. Automatic runs are **quiet** — failures go to the log, the rule for
  background turns — where the requested action reports into the overlay the user is looking
  at ([§6.8](#68-transient-engine-failures-and-what-the-user-is-told)). See
  [docs/history/auto-chat-title.md](docs/history/auto-chat-title.md).
- Contextual actions: create (with a profile picker), clone, delete (soft).
- **Sub-agent transcripts are rows of the list, nested under the chat whose
  call made them** ([§9.3.2](#932-call_subagent); docs/research/subagent-chats.md
  §3.7): indented with a `└` in place of the dot, in **call order** under their
  parent (they are the steps of its work, so "newest first" would read them
  backwards), with a muted mark beside the count when the run did not complete
  (*cancelled* / *timed out* / *failed* / *round limit* / *interrupted*). The
  filter rule, in both search scopes: **a row is shown iff it matches; a chat
  is additionally shown — dimmed — when any of its transcripts matches.** So a
  matched transcript never appears without its parent, an unmatched one never
  pads a matched parent, and text found only in the parent shows the parent
  alone. Content mode names transcripts from the index by their own ids
  ([§11.2.1](#1121-the-message-level-search-screen)), so the rule is one
  membership test there too — and `Enter` on a transcript row opens it on
  *its* first match, on a chat row on the chat's own (a match inside a
  transcript never drags the parent to a message it does not have). On a
  transcript `Enter` opens it, `F2` renames it,
  `Ctrl+R` has the model title it, `F5` copies it; **`Del` and `Ctrl+D` refuse**
  with a note in the status area naming the way a transcript does go away —
  `Ctrl+E`/`Ctrl+R` of the spawning exchange in the parent, or the parent
  itself — and the hotkey grid does not advertise the two on such a row. The
  orchestrator refuses `DeleteChat`/`CloneChat` for a transcript's id as well,
  whatever route sent them; cloning a **parent** copies its transcripts along
  under fresh ids (two chats answering to one `chat://` prefix would make the
  reference ambiguous). **A sub-agent that is running right now is a row too**
  ([§9.3.2](#932-call_subagent)): under its parent, marked *running* beside a
  count that grows as its rounds file, openable, renameable. **Switching chats
  cancels a running turn** — except between the running turn's chat and that
  transcript, in either direction: looking at the run is not leaving it. Back
  on the parent, its feed holds the rounds the turn has filed so far and keeps
  generating; the text of the round in progress reappears when the turn lands,
  with one whole refresh of the feed.
- **Copying the entire conversation** of the selected chat to the system clipboard (`F5`):
  the overlay only sees a summary, so the text is built by the orchestrator (the owner of `Chat`),
  while writing to the clipboard is a UI-layer concern (`arboard`). Confirmation/error is shown in the
  overlay's status area (success in green, an error in red); the overlay doesn't close.
  **The copy's contents are configurable** (`config.copy`, the "Interface" section): by default —
  only the message text; optionally includes "thoughts" (CoT), tool-call parameters
  (name + arguments), and their results (taken from `Message.tool_calls`).
  Roles are labeled with the chat profile's **custom names** when set (`Gaia:` instead
  of `User:`, §10); otherwise with the interface language's labels.
- Virtualization of a long list (only visible rows are rendered).

#### 11.2.1. The message-level search screen

A separate full screen (`screens/search.rs`, `ActiveScreen::Search`), opened from the
chat list's content mode with **`Ctrl+G`** — or straight from the chat by typing
**`/search <text>`** (§11.7), which collapses the three-key journey (`Esc`,
`Ctrl+F`, `Ctrl+G`) into one line and is the only route in a host that keeps
those keys; the sort order is then the chat list's default, there being no open
list to inherit one from. Stage 1 answers *"which chats mention
this?"*; this screen answers *"where exactly, and take me there."*

- **Grouped by chat**, not ranked: a chat header (title + its number of hits), then
  that chat's matching messages **in chat order** (their real position in the
  conversation, which only the orchestrator can resolve); chats **in whatever order
  the chat list is currently showing them**, so its `Tab` sort toggle carries over
  rather than the results quietly using a different one. This sidesteps ranking entirely — the
  trigram index's `bm25` is measurably usable but a weak proxy for relevance —
  and it reads well when one chat holds many hits,
  which is the common case (a common word matched 163 messages across 50 chats on a
  real 171-chat corpus).
- **Sub-agent transcripts are groups of their own, under their parent's**
  ([§9.3.2](#932-call_subagent); docs/research/subagent-chats.md §3.9). The
  index (`cache.db`, §5.2) marks a transcript's messages with the run's id
  (`messages.sub_id`, `CACHE_SCHEMA` 2 — the file rebuilds itself), indexed
  under the parent's file so the bookkeeping never sees a second level; every
  query that names a conversation then treats a transcript as one —
  `COALESCE(sub_id, chat_id)` is the conversation id in the `Ctrl+F` id set,
  in the tools' scope (§9.11) and in the grouping here. Results are grouped
  by `(chat, transcript)`: the chat's own hits first, then one group per
  matched transcript **in call order** (the list's tree, in the results), its
  header indented with a `└` and no blank line before it. A chat none of
  whose *own* messages match still heads its transcripts' groups — as a
  "0 matches" header navigation steps over — so a transcript is never shown
  orphaned. `Enter` on a transcript's hit opens the **transcript** (read-only,
  §11.3) on that message.
- **One hit** = a muted `role · date` prefix plus a **snippet** — an excerpt of the
  message centred on the first match, with the matched spans drawn in the accent
  color. Snippets are built in Rust from the text the index already stores, not by
  SQLite's `snippet()`: we need byte offsets to highlight, and under a trigram
  tokenizer the budget counts 3-grams rather than words (64 "tokens" yields ~70
  characters, against a documented ceiling this build silently exceeds). A message
  FTS5 matched whose text does not literally contain the token — possible under
  trigram — yields the head of the text with no highlights rather than an empty
  snippet.
- **A cap of 200 hits** with an honest *"showing N of M"* line; the true total comes
  from a separate count, since the rows only equal the total below the cap.
- **Navigation** is over hits, so chat headers are skipped by construction rather
  than by a filter: `↑`/`↓`, `PageUp`/`PageDown` (a fixed step, as in the chat list),
  `Home`/`End`. **`Enter`** opens the chat with the feed on that message
  (`AppCommand::OpenChatAt`, [§11.3](#113-the-message-feed)); **`Esc`** returns to
  the chat list **still searching for the same query**, so the search that got you
  there isn't thrown away; `Ctrl+Q`/`F10` quit.
- **`Esc` in a chat opened from a hit comes back to these results**, not to the chat
  list — you drilled down from them and are most likely working through the hits, so
  going back retraces the step you took. It is dropped once you *work* in that
  chat rather than read it (§11.3) — the same rule a followed `chat://`
  reference obeys, because it is the same back-stack. The results come back **whole** (the same
  selection and scroll position), because the screen itself is stashed rather than the
  query: re-running the search would lose exactly what you want back. It is one step
  deep and consumed on use — the next `Esc`, now from the results, goes on to the chat
  list as it always did — and it is forgotten as soon as a *different* chat is opened
  by any ordinary route (picking one in the list, `Ctrl+N`, a clone, restoring the last
  chat at startup). Re-activating the *same* chat (a regeneration, `Ctrl+E`, a repeat
  jump) is not leaving it and keeps the way back. The status bar's `Esc` hint says
  which of the three it currently means ([§11.7](#117-keybindings-preliminary)) —
  the third being the conversation a `chat://` reference was followed from
  ([§11.3](#113-the-message-feed)).
- **The query is highlighted inside the message you land on** (`chat_search::match_ranges`),
  so the feed and the results list agree on what matched — they share one matcher, hence
  one notion of a match. The highlight is applied **post-render**, by matching the text
  that was actually drawn and re-splitting its spans; mapping *source* byte offsets
  through the renderer is what is infeasible, and this never asks that question.
  Source offsets do not survive because `normalize_delimiters` rewrites the string before
  parsing, and LaTeX substitution, mermaid replacement, table re-layout, syntect→ANSI, two
  wrapping passes and rail prepending each independently destroy the correspondence; an
  assistant bubble is also rendered as several disjoint markdown fragments split by
  tool-call offsets. See
  [docs/history/chat-search-stage2.md](docs/history/chat-search-stage2.md) §1.4 and
  fork S3(b).
  Three properties follow from matching what is on screen, and they are the honest
  reading of the feature:
  - **Approximate by construction** — content the renderer *transformed* no longer
    contains the query as text and is not found: text pulled into a LaTeX formula and
    substituted into unicode, or a mermaid diagram drawn in place of its source.
  - **It over-highlights relative to the index** — "thoughts" and tool cards are
    highlighted although only `message.text` is indexed (§11.2, fork F3 of stage 1). So
    *highlighted* does not mean *this is what matched*; the word is genuinely on screen,
    which is why it is left in. The **role header is excluded** — otherwise a plain
    search for "assistant" would light up every assistant bubble it marked.
  - **Only the jumped-to message** is highlighted, never the rest of the feed:
    the query travels with the focus as one value, so a stale query cannot outlive its
    target, and highlighting every occurrence chat-wide would be noise nobody asked for.
- Like the other screens it knows nothing about `app` (FSD) and answers with a
  `SearchIntent`; the orchestrator owns the index and the chats and hands over a
  finished snapshot (`AppEvent::MessageSearchResults`). Where `Esc` goes *back* to is
  app-layer knowledge too — the chat screen only signals "go back" and never learns
  that a search exists (see [docs/architecture.md](docs/architecture.md) §10).

### 11.3. The message feed

- Incremental streaming of the assistant's latest response (the text and the "thoughts" stream separately).
- **A tool call's card opens the moment the call starts**, marked *running…*
  where its result will go, and is completed in place when the result arrives
  (`AppEvent::ToolCallStarted` → `ToolCall`, matched by the call's id). A long
  call — a sub-agent run, a project build, a `python_exec` — is therefore
  visible where it happens, not only as a status-bar chip, and a turn inside
  one never looks idle. A control call and a call discarded by a rewrite open
  no card, as before; a sub-agent's own calls stay in its transcript; a turn
  that ends mid-call (cancelled, timed out) clears the mark.
- **Collapsible blocks**: "thoughts" (CoT, `Ctrl+T`) and tool calls (`Ctrl+O`).
  Both are **collapsed by default** — the feed is scanned for the reply, and
  reasoning/tool plumbing is detail you ask for. Collapsing a tool call keeps its
  header (`⚒ name(args)` — *what* ran, plus a short scalar argument as the
  one-line summary the presenter puts there) and folds away the argument and
  result blocks; the header then carries the same pill the collapsed "thoughts"
  block uses — marker, label and the key that opens it. A call with nothing to
  hide (no arguments, no result yet) gets no pill. The pill is **one piece**:
  it follows the header's last row when it fits there and otherwise takes the
  next row whole, aligned under the name — it is never split at its spaces
  with the keycap alone on a row. The "images returned" chip (§9.10) is placed
  by the same rule.
  **Expanded, the card is a different presentation, not a longer one**: the
  header carries the tool's **name alone**, every argument is enumerated below as
  one `key: value` line each (untruncated; arrays/objects as compact JSON — the
  header can carry neither), then a gap row, then the result. The gap **keeps the
  `│` gutter** rather than being blank — a blank row would cut the card in two. A value that
  cannot share a line with its key — code, a large or multiline string — goes
  under a `key:` label as its own block, so `python_exec`'s code keeps its
  highlighting and still says which argument it is. Field order is the **tool's
  own** — its schema order, from a table in the presenter (`FIELD_ORDER`), for
  the tools where that differs from the alphabetical order a `serde_json::Map`
  iterates in; the wire format does not preserve the model's own order, and
  alphabetical put `call_subagent`'s request above the persona it was addressed
  to. An argument the table does not name (a schema that grew, an MCP tool) is
  listed after the ones it does, alphabetically. The same order applies to the
  compact header. The detail level is [`ArgDetail`] on the presenter — the
  dangerous-tool confirmation popup (§9.8) stays `Compact`, being a decision
  prompt rather than a viewer.
  **The state is stored per chat** (`Chat.feed_view`, the [`Chat::draft`
  playbook](#117-input-box)): the toggle returns the new state as an intent, the
  orchestrator writes it to the active chat with the save debounce and **without
  touching `modified_at`** (folding a block away isn't a change to the
  conversation), and hands it back in `AppEvent::ChatActivated`. So one chat can
  be read with everything expanded while another stays compact, and the choice
  survives a restart. Per-*message* collapse (a key on the selected block) —
  deferred; both flags are part of the feed's render-cache key.
- **Tool cards** (`⚒ name(…)`) show arguments/results meaningfully rather than as raw JSON: a presenter (`features/tools/present.rs`) produces a compact header (`name(value)`/`name(k=v, …)`) and blocks — `python_exec` draws a highlighted Python code block and a console (stdout/stderr/exit code in separate colors), `fs_read`/`fs_write` — content highlighted by the path's extension, prose tools (`web_search`/`fetch_url`/`rag_search`/`note_recall`) — markdown, short arguments — inline. Knowledge about tools lives in the tools layer; `message_feed` stays generic, reusing `markdown::highlight_code`. A card gets a **rail extension underneath it** (the colored `▌` gutter continues under the result) regardless of whether assistant text follows — so the rail doesn't cut off at the result when the call ends the turn.
- A per-message Markdown toggle.
- **Role headers** (`✦ ASSISTANT` / `❯ YOU`) show the profile's **custom names** when set
  (`Profile.character_names`, §10): the name is uppercased to match the header's style
  (Unicode-aware), an unset field keeps the interface language's label. The names are
  resolved from the profile and pushed to the screen by the orchestrator
  (`AppEvent::CharacterNames`) on chat activation and after a profile edit, so a rename
  applies to the open chat at once; they take part in the feed's render-cache key.
- **The model's name next to the assistant's header** — off by default, the
  `interface.show_model_name` toggle (§11.6). It is read from the **message's
  own** metadata snapshot (`MessageMetadata.model`, §5.1), not from the current
  settings: a conversation reopened after switching providers still says which
  model wrote each answer, which is the only reason the line is worth a row at
  all. A message stored without the snapshot — or in a mode that names no model
  (a managed server with no GGUF path yet) — shows nothing, and its header is
  byte-for-byte the one drawn before the setting existed. The name rides the
  header rather than a line of its own (it is an attribute of the bubble, and a
  second row would be spent on every assistant message) and is drawn **muted and
  unbolded**, like the "thoughts" pill: it answers an occasional question and
  must not compete with the role. A **stitched** bubble (§9.3 rounds merged into
  one block) keeps the first round's answer — one bubble, one header, one name.
  The **live** bubble carries it from the turn's start:
  `AppEvent::GenerationStarted` names the model, resolved from the same single
  read that the finished message's metadata gets, so the header cannot change
  under the reader when the turn ends, and a follow-up bubble opened mid-turn
  (§9.3) gets the same name. The flag is part of the feed's render-cache key —
  the name is baked into the cached header line.
- Contextual message actions: copy thoughts/message/the whole chat; **edit in place** (both user and assistant — a direct requirement); regenerate; delete last.
- **Scrolling**: `PageUp`/`PageDown` (by `PAGE_SCROLL` lines) and the **mouse wheel** (by `WHEEL_SCROLL` lines), with automatic "tail-following" when scrolled to the bottom. Terminal mouse capture is a **toggle**, `Ctrl+W` (off by default, so native text selection with the mouse works; when captured, the wheel goes to the application, and selection stays available with `Shift`). The wheel and selection share one terminal mouse-reporting mechanism, so "wheel only" can't be enabled separately. The current mode is shown in the status bar.
- **Who may scroll to the tail** — split by *who asked*. **User-initiated** actions go
  to the bottom unconditionally: activating a chat (unless a jump was requested),
  sending a message, starting a generation. You did it; you want to see the result.
  **Content arriving on its own** — a tool card, a service note, the next round's
  bubble after a follow-up or rewrite — only scrolls **if the view was already
  following the tail**: a reader who has scrolled away (or jumped to a message) must
  not be yanked back. Scrolling to the last line resumes following.
- **Jumping to a message** (`AppCommand::OpenChatAt` from the search screen, §11.2.1):
  the chat is activated and the feed is positioned on that message, whose rail is
  drawn in the accent color — "this is where you landed" — and **the searched query is
  highlighted inside that message** (the accent color, the same one the results list
  uses; approximate, and only that message — see [§11.2.1](#1121-the-message-level-search-screen)).
  The **mark survives
  scrolling** (that is the point: you read around the hit and can still see it) and is
  dropped only when the chat changes or another jump replaces it; the **view position**
  is released as soon as you scroll manually. The highlight moves as one with the mark
  (both set by the jump, both dropped with it), so a query left over from a previous
  jump cannot light up the wrong message. The position is anchored to the *message*
  (a feed index), not to a row number, so a rewrap — a resize, a theme change, `Ctrl+T`
  — returns the view to the message rather than to a stale row. A message the feed
  doesn't show (a `Tool`/`System` one) is a no-op and the chat simply opens at the tail.
- **A sub-agent transcript opened from the list is read-only** ([§9.3.2](#932-call_subagent),
  docs/research/subagent-chats.md §3.8). It looks like a chat with a system
  message: the persona the parent composed is drawn **first, as a system bubble**
  (headed by the profile's system name, else `SYSTEM`; muted rail) — the one
  thing a reader of a transcript wants first, and a chat never shows; the
  instruction's header is the **parent persona** (the profile's assistant name,
  else the assistant label — it wrote the instruction), the replies' header the
  sub-agent's `name`, else `SUB-AGENT`; both resolved at activation, so a profile
  rename shows at once. The input box is titled *read-only · commands only* and a
  quiet status-bar chip says *transcript*. **Sending refuses with a note** that
  names the parent and the way a transcript goes away (and leaves the line in
  the box); so do `Ctrl+R`/`/regen`, `Ctrl+E`/`/takeback`, `Ctrl+U`/`/impersonate`,
  `/compact`, `/file`, `/image`, `/project` and `/clone`. What works is what
  makes sense in a transcript: `/copy` and `F5`, `/rename`, `/export`, `/search`,
  `/find`, `/links` and `Ctrl+L`, `/tts`, `/chats` and `Esc`, `Ctrl+T`/`Ctrl+O`
  (the collapse state is the **parent's** — the transcript is part of it),
  `/new`, `/settings`, `/help`. The orchestrator fails closed by construction
  on everything else: the active id is the transcript's, and no chat of the
  list has it. The transcript is remembered as the last open one and restored
  at the next start like a chat. **A running transcript** opens the same way
  with the rounds filed so far and grows as the next ones file (the feed is
  rebuilt from every message so far, so rounds stitch exactly as they will once
  landed; the scroll follows the tail only if it already did) **and streams
  between them**: the sub-agent's text and thoughts arrive token by token into
  a bubble seeded with the round so far, its own tool calls open running cards,
  and the counter shows the run's own tokens; the sub-agent chip stays in the
  status bar, the parent's stream never reaches this feed, the run's end
  closes the bubble, and `Esc` is navigation here — cancelling the turn is the
  parent's `Esc`.
- **`chat://` references** ([§9.11](#911-cross-chat-search-chat_search-and-chat_read)).
  An address for a conversation of the **current profile** — a chat or one of
  its sub-agent transcripts — is drawn in the link style wherever it appears in the feed — an assistant's answer, a user's
  message, a tool card, "thoughts" — and **`Ctrl+L`** opens a picker of the
  ones this chat holds (newest first, deduplicated, title + date), `Enter`
  follows the chosen one. **With mouse capture on (`Ctrl+W`), a left click on
  the address follows it too** — the first thing in the feed that is clickable
  at all. The key is the route, the click is the convenience: capture is **off
  by default** because turning it on costs native terminal selection, so a
  click-only design would be unreachable for most users. Following an address
  is an ordinary chat switch — an address names a conversation, not a message —
  and **`Esc` retraces it**, back to the conversation the reference was followed
  from. That is the same one-deep, consumed-on-use back-stack the search screen
  uses (§11.2.1), and it is dropped by either of two things: **leaving** for a
  different chat by an ordinary route (picking one in the list, `Ctrl+N`, a
  clone), or **arriving** — working in the chat you drilled into. Sending a
  message, regenerating, taking back an exchange, `/compact` and attaching or
  removing a file all mean you are no longer just looking, and an `Esc` that
  then teleported you out would be a trap of its own; reading, folding blocks,
  typing without sending and staging an image for the next message do not. A
  chain of followed references steps back one conversation and no further, and
  the status bar says where `Esc` currently goes.
  Three properties are decided rather than incidental:
  - **Only a reference that resolves is drawn as one.** The address book is
    the current profile's non-hidden chats (the open one included), derived
    from the chat-list snapshot the screen already keeps; an unknown id, or
    one belonging to **another profile** (§9.5), stays plain text. So the user
    is never offered a door onto nothing, and the profile boundary needs no
    separate enforcement in the UI.
  - **Recognition runs in the block builder, before the wrap**, and its result
    is part of the render cache (the address book's fingerprint rides
    `CacheKey`, like `role_names`). An address is 15 columns, so a narrow panel
    splits it across rows, where the post-render matching used by in-feed
    search would no longer see it whole. Both forms are recognised: a bare
    `chat://<id>` and the markdown `[Title](chat://<id>)` the descriptions ask
    for, which the renderer draws as `Title (chat://<id>)`.
  - **The two degenerate cases are told apart** — a chat with no references at
    all says what a reference looks like instead of opening an empty list, and
    a reference back to the open conversation says so instead of quietly doing
    nothing. Clicking one behaves the same way, with the same words.
  - **The click map is rebuilt from the rows actually drawn**, each frame, in
    absolute terminal cells, and covers only the viewport: that is the one
    point where the wrap, the scroll and the panel's origin have all been
    applied, so no stored assumption about them can go stale, and the cost is
    bounded by the screen rather than by the conversation. A feed that has not
    been drawn has no map and reports no links. One consequence is stated
    rather than hidden: an address the wrap **split across two rows** is styled
    but not clickable — half an address is not an address — and `Ctrl+L`
    remains the route that always works.
- **Mouse in the input box** (with `Ctrl+W` capture on): a left click places the cursor, a drag selects text (the cursor snaps to a grapheme-cluster boundary). A click outside the box (in the feed) is a no-op (feed selection is a separate track). See §11.5.
- **Full redraw for emoji in the feed**: on legacy terminals, lines with emoji leave "hanging" artifacts, so a full per-cell redraw is requested by **two** events — scrolling the feed and **the feed's content changing** (a response streaming, a note added, `Ctrl+T`; flagged by the feed's own mutators — **before** rendering, so the artifact doesn't flash for even one frame: a bad frame can't be hidden behind synchronized output, since conhost ignores mode 2026). The gate is glyphs in the risk group (`is_risky_glyph`: VS16, ZWJ clusters, skin tone, supplementary pictographs, BMP emoji of width 2); CJK is deliberately excluded (terminals render ideographs consistently). **Known limitation**: a wide glyph's background can end up painted only halfway — `ratatui` resets the trailing cell to the default style and doesn't send it in the diff, and the terminal doesn't set the second half's attribute itself; this can only be fixed upstream. The redraw mechanism is `shared/ui.rs::prime_full_redraw` (a marker written into the buffer + `swap_buffers` with no screen output; `terminal.clear()` won't do — its `ESC[2J` causes flicker). The marker is a **space + the `HIDDEN` modifier**, not a placeholder character: a space matches the content of a wide glyph's trailing cell, so `ratatui` won't send it to the terminal. Otherwise their workaround for VS16 kicks in ("send the trailing cell too"), and the `crossterm` backend tracks position by cell number without accounting for glyph width (`x == last.x + 1` → no `MoveTo`) — the trailing cell would print one column to the right and shift the rest of the row (an adjacent wide glyph goes dark, the right border drifts). `HIDDEN` isn't otherwise used in the interface, which is pinned by a test.

#### 11.3.1. In-feed search (`Ctrl+F`)

Search inside the **open chat** — the browser's "find on this page", as opposed to
`Ctrl+G`'s cross-chat search ([§11.2.1](#1121-the-message-level-search-screen)).
Design record: [docs/history/in-feed-search.md](docs/history/in-feed-search.md).

- Opened with **`Ctrl+F`**, or by typing **`/find [text]`** — the route for hosts
  that keep `Ctrl+F` (VS Code binds it to the terminal's own find, §11.7). With
  text, the query is seeded before the field opens, so one line both opens the
  search and lands on the first match.
- The **key** is deliberately not `/`: the chat's input box is
  always focused, and `/` in an empty box is exactly how a command starts
  (`/rag`, `/file`, `/tts`, `/reindex`), so a `/` trigger would make command entry
  impossible — and gating it on an empty box does not help, because that *is* the
  command-entry gesture. `/find` is that same reasoning arriving at the other
  end: the whole word is a command, where the bare `/` could not be.
- The query field **stands in for the input box** while open, rather than taking a
  layout row of its own: a fifth constraint would shrink the feed, change the wrap
  width, and rewrap the whole chat on open *and* close. The message being written
  is untouched, and the query is remembered for a repeat `Ctrl+F` in the same chat.
- **Every** match in the chat is highlighted (unlike a jump from `Ctrl+G`, which
  marks one message), with a `match n of total` counter — a common word matches
  200–300 times in a single chat, so the count is not decoration.
- `Enter`/`↓` next, `Shift+Enter`/`↑` previous, wrapping around at the ends; `Esc`
  closes and clears the highlight. Stepping lands on the **line** the match is on,
  not the start of its message: the largest real message is 38,782 characters, so
  message-granular stepping would leave the viewport unmoved. That row is
  **re-derived every render, never stored** — which is what lets it survive a
  rewrap, and why [§11.2.1](#1121-the-message-level-search-screen)'s anchor can
  stay message-granular without contradiction.
- Matching runs over **what is drawn**, which is what lets the counter equal the
  highlights. Two honest consequences: it also finds words inside "thoughts" and
  tool cards, which the full-text index deliberately does not cover, so
  *highlighted* does not mean *this is what the index matched*; and it cannot find
  text the renderer reshaped (a LaTeX formula turned into unicode, a mermaid
  diagram replacing its source) or deliberately does not show (HTML attribute
  names and values, §11.4) — the same boundary as the jump highlight. **Measured**
  on the real corpus rather than assumed: 65 of 186 805 searchable words (0.035%),
  across 20 messages of 1213 — and they are LaTeX command names (`rightarrow`),
  mermaid syntax and hex colours, and markup tokens, none of which anyone searches
  for. That measurement is why threading source ranges through the renderer to
  close the remainder was **rejected**, not deferred.
- The search closes when the chat is re-activated (`Ctrl+E`, `Ctrl+R`, a rewrite
  round, a jump from the results screen all rebuild the feed and renumber it).

### 11.4. Markdown, CoT and tool blocks, LaTeX

> Implementation: **our own renderer on `pulldown-cmark`** (`shared/markdown.rs`),
> see [ADR 0003](docs/decisions/0003-own-markdown-renderer.md). `tui-markdown`
> (ADR 0001) was dropped: it doesn't support tables and math, and ignores the theme.

- Markdown rendering is our own walker over `pulldown-cmark` events (`render(input,
  width, palette)`). Text only (no graphics) — important for JupyterLab terminal
  compatibility. Colors come from the theme (`Palette`); code-block highlighting is
  `syntect`, whose theme **is itself built from the same `Palette`**
  (`build_code_theme`: scopes → semantic roles, text/comments — gray, scaled by
  background lightness `Palette.dark`), so highlighting stays consistent with
  dark/light/auto.
- **Which languages highlight.** syntect's bundled set is a snapshot of Sublime
  Text's defaults — 75 syntaxes, missing most of what a model actually tags a block
  with. So real `.sublime-syntax` grammars for 22 more languages (Zig, TypeScript,
  TOML, Dockerfile, PowerShell, Swift, Kotlin, SCSS/Sass, GraphQL, Terraform, Elixir,
  Solidity, Julia, Nix, Dart, Protobuf, CMake, nginx, Vue, Svelte, Nim) are
  **vendored** in `syntaxes/`
  and compiled into the binary's syntax dump at build time (`build.rs`); each is
  pinned to an upstream commit with its licence in `syntaxes/SOURCES.md`, refreshed
  by `python tools/fetch_syntaxes.py`. A label the set still doesn't know is mapped
  to a close relative where one exists (`canonical_lang`: `jsx → js`, `tsx → ts`,
  `hcl → tf`, `v → go`, `jsonc → json`, …) — partial highlighting beats grey text — and otherwise falls back
  to the unhighlighted rectangle below. See docs/history/vendored-syntaxes.md.
- The full typical LLM markdown output is supported: headings, lists, blockquotes,
  inline/block code with highlighting, links (autolinks `<url>` don't duplicate the
  URL), images (alt text + URL), **tables** (native box-drawing rendering with
  "water-fill" smart column layout for the panel width; cell content wraps by word,
  and if space is tight — a horizontal clip with "…"; `<br>` inside a cell — a line
  break; optional **horizontal separators between body rows** `├─┼─┤` —
  the `interface.table_row_separators` setting, a toggle in the "Interface" section,
  off by default (a compact look, with a separator only under the header); enabling
  it gives a "grid" look). A code block's info string (` ```rust,no_run `)
  resolves the language from the first token; `---` stretches across the panel width.
- **A code block with no highlighting is a rectangle.** Such a block (no language, or
  one syntect doesn't know — ` ```text `) is drawn on a reverse-video background, and
  that background used to follow the ragged right edge of each line. Its rows —
  fences included, they are the rectangle's top and bottom edge — are now padded to
  one width: the **block's own** (its widest line **plus one blank column** on the
  right, so the text doesn't run into the background's hard edge), capped by the
  panel and not stretched to it, the same rule tables follow. There is deliberately
  no matching column on the left: the code's own indentation stays aligned with the
  fence markers and the surrounding prose. A line longer than the panel is
  wrapped **by the renderer**, so the block stays square instead of leaving a ragged
  tail row for the feed's re-wrap to produce (and that re-wrap becomes a no-op, as it
  already is for tables). The fences' `DIM` sits on their span rather than on the
  line, so the padding takes the background without the dimming and the edges match
  the body. A **highlighted** block is deliberately left alone: the syntect pipeline
  only carries foreground color (`build_code_theme`), so it has no background — there
  is no rectangle to square off.
- **Raw HTML**: *inline* HTML (`<strong>x</strong>` inside a paragraph) has always
  rendered its text, because pulldown-cmark delivers that text as ordinary `Text`
  events. A **block** of raw HTML does not — it arrives as opaque chunks — and used
  to be dropped whole, so a pasted `<table>` rendered as *nothing at all*, prose
  included. Now its **text** is shown (`shared/markdown/html.rs`): tags stripped,
  `<script>`/`<style>` content dropped (it is not prose), entities decoded,
  whitespace collapsed, block elements ending the line and table cells separating
  words, `<img>` printing alt + URL exactly like a markdown image. Deliberately not
  the markup itself (a 30-row table would become a wall of tags) and not a
  reconstructed table (a much larger feature that would still need this fallback);
  attribute names and values are markup and stay off screen. Not an HTML parser —
  no tree is built, so malformed markup degrades into text rather than an error.
- **Mermaid diagrams** (` ```mermaid ` blocks) are rendered as text graphics
  (the `mermaid-text` crate ≥ 0.56.1 — our own upstream multi-byte fix,
  [research doc](docs/research/mermaid-ascii-rendering.md)) — the
  `interface.render_mermaid` setting, a toggle in the "Interface" section, **on by
  default**. The rule is a **hard fallback rather than clipping** (a diagram with
  cut-off arrows is unreadable, unlike a clipped table): only a whitelist of
  flowchart/sequence diagrams is rendered, and only when it fits entirely within the
  panel width (the crate's `max_width` is only a post-check hint); any failure
  (garbage from the model, a truncated stream, another type, overflow) prints the
  source as a code block **byte-for-byte identical to the toggle being off** — the
  worst case equals prior behavior. In compat mode (§11.6) the diagram is drawn with
  ASCII glyphs. **Streaming without flicker**: a block with an unclosed fence (the
  server is still writing the diagram) is shown as source — fence closure is checked
  against the source itself (CommonMark extends an unclosed fence to the end of the
  document, so at the parser-event level truncation is indistinguishable from a
  complete block, and a valid-looking partial fragment would otherwise render as a
  partial diagram); once the closing fence arrives, the feed cache recomputes the
  message and swaps the source for the diagram — a single "source → diagram"
  transition per block.
- **Unicode approximation of LaTeX — only inside math delimiters**
  (`$…$`/`$$…$$`; the `\(…\)`/`\[…\]` forms are normalized to these; code spans and
  fences `` ``` ``/`~~~` are skipped). The parser strips the delimiters. Substitutions: arrows,
  relations/operators, Greek letters (incl. `\var…`), fractions (`\frac`/`\dfrac`/
  `\tfrac`→`a/b`), `\binom{n}{k}→C(n, k)`, roots (`\sqrt{x}→√(x)`, `\sqrt[3]{x}→∛(x)`),
  fonts (`\mathbb{R}→ℝ`, `\mathcal`/`\mathfrak`; BMP only), text wrappers
  and accents (`\text`/`\vec`/`\overset`/…→content/base argument), operator
  names (`\log`/`\sin`/…→as a word), `\left`/`\right` (stripped, an "empty" delimiter
  `.` is swallowed), `\pmod{n}→(mod n)`, letter and numeric super/subscripts
  (`x^2→x²`, `x_i→xᵢ`, `x^T→xᵀ`; an unmappable group → `^(…)`/`_(…)`).
  **Environments** `\begin{aligned}…\end{aligned}` (and `cases`/`pmatrix`/…) are stripped:
  `\\` — a line break (a real one in block `$$…$$`, "; " in inline math), `&`
  (alignment) is removed, boilerplate `\label{…}`/`\hline`/… is dropped. An unrecognized
  brace command **keeps its braces** (`\boxed{x}` stays readable, isn't run together).
  "Bare" commands outside the delimiters are **left untouched**; price ranges
  `$5-$10` aren't swallowed by the math extension. Full LaTeX and image rendering are
  **not implemented**.
- **`highlight_code(code, lang, palette)`** — highlights a code block **without the
  surrounding ` ``` `** and without wrapping, using the same theme-consistent palette
  (`build_code_theme`) as fenced blocks. Used by the feed's tool cards (§11.3) for
  tool arguments/results (`python_exec`, `fs_read`/`fs_write`); an unrecognized
  language → plain text with no highlighting.
- **The renderer knows nothing about the search highlight** (§11.2.1): a jump's query
  is applied by the feed *on top of* the rendered lines, matching their text and
  re-splitting their spans — no `highlight_ranges` threaded through `writer.rs`/
  `latex.rs`/`table.rs`/the mermaid path, which is what keeps it cheap. It is applied
  **before** the feed's wrap, since `wrap::wrap_line` carries per-character styles
  through. The flip side is stated in §11.2.1: everything this section *transforms* —
  LaTeX substituted into unicode, a mermaid diagram drawn in place of its source — no
  longer contains the query as text, so the highlight will not find it.

### 11.5. Input and editing, spellcheck

- **Input/editing**: a **custom multiline widget** (`widgets/input_box.rs`, [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md)) — for a new message and for editing existing messages in place. Off-the-shelf crates (`tui-textarea`/`ratatui-textarea`) are incompatible with ratatui 0.30 and don't support highlighting arbitrary ranges — hence a custom widget.
  - The `spellbook` engine (Hunspell dictionaries from the `dictionaries/` directory at the data root; in non-portable mode — with a fallback lookup in the portable layout `data/dictionaries/` next to the binary, where an installer/package puts them).
  - A word is correct if accepted by **at least one** active dictionary (support for mixed ru/en text).
  - Segmentation — our own letter-run scanner (accounting for apostrophes/hyphens); checking runs in the background (`tokio`) with a ~300 ms debounce.
  - **URLs and email addresses are skipped whole** (`features/spellcheck/segment.rs`): an address isn't prose, and its host and path fragments would otherwise be underlined word by word — noise on exactly the text a user pastes rather than types. Recognized by a heuristic over four shapes: an explicit scheme (`https://…`, matched anywhere in the token, so `(https://x)` counts), a `www.` prefix, a bare domain (`example.com/path?q=1`, `example.com:8080`) and an address (`user@example.com`, `Name.Surname+tag@mail.example.co.uk`, with an optional `mailto:`) — with surrounding punctuation trimmed. A bare domain additionally has to be **lowercase ASCII**: that restriction is what keeps a run-on sentence (`end.Next` and its Cyrillic equivalent — a missing space after a period, a common typo) from reading as a domain and silently switching the check off for it. After an `@` the shape is already unambiguous, so there the host may be written in any case. A bare `@` isn't enough: a mention (`@username`) has no domain and stays checkable. The skip is in segmentation, so it covers both the underlining and the suggestions popup (`Ctrl+G`); file paths are not covered.
  - **Rendering errors in the TUI**: incorrect words are highlighted with a style (`UNDERLINED`/color) in the input area; a **suggestion popup** (a hotkey on the word under the cursor) offers options from `Suggester` + an "Add to dictionary" item.
  - A personal dictionary — `personal_dictionary.txt` (global), grown at runtime (`Dictionary::add`).
  - **Rendering underlines** — solved by our own input widget ([ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md)): errors are drawn per-span with `UNDERLINED`/color over our own `InputBox`, with no dependency on a third-party diagnostics API.
- **Cursor navigation over a wrapped line**: `↑/↓` move by **visual row** (a
  wrapped line's parts are separate rows), keeping the column across short rows
  (goal-column). `Home`/`End` act on the visual row first, and **pressing them
  again** — when the cursor is already at that stop — goes on to the whole
  **logical** line (the rule of large editors: the row is what you see, the line
  is what you edited). `Home` has one more stop ahead of those, the one editors
  call "smart home": **the first non-whitespace character**, so an indented line
  is entered at its text rather than at its indentation. The full ladder, nearest
  first: the row's text → the row's start → the line's text (only when that lies
  *before* the row's start, i.e. from a later row of a wrapped line — otherwise
  it is already the row's own text stop) → the line's start; stops that coincide
  collapse, so an unindented line keeps exactly the two steps. `End` has two
  stops (the row's end, then the line's) — trailing whitespace is not a thing
  worth a stop. The step is chosen by the cursor's **position** rather than by
  counting presses: no state to reset in the many other places that move the
  cursor, and it also does the useful thing when the cursor reached a stop by
  typing. The last stop stays put (the whole text is `Ctrl+Home`/`Ctrl+End`); a
  row that is entirely whitespace has no text stop. In **single-line** mode
  (settings fields, chat rename) `Home`/`End` always span the whole value — a
  path or URL that starts with an accidental space is easier to fix from column
  0. Before the first render the wrapping isn't known (the width comes from the
  last render), so everything falls back to the logical line.
- **Emoji picker popup** (`Ctrl+B`): a grid of popular emoji, arrow-key navigation, `Enter` inserts the selected one into the input box at the cursor (safe for multi-scalar clusters like `❤️`/`👍🏽`), `Esc` closes it; the popup remembers the last choice. Any action in the popup (moving the selection or closing it) requests a **full redraw** from the loop: a wide emoji occupies two cells, and when the glyph leaves its spot, `ratatui`'s per-cell diff doesn't repaint its **trailing** cell (in both buffers it's a default space), and the terminal doesn't clear the second half of a wide glyph itself — a "hanging" fragment was left on screen (visible via the selection background). The full redraw uses the same "sentinel buffer" technique as scrolling the feed with VS16 emoji (§11.3) — it rewrites every cell without `ESC[2J`, i.e. without flicker. **The popup's own emoji set is kept free of VS16 clusters** (`❤️`/`✌️` were replaced with `💖`/`🤞`; the "exactly 2 columns, no U+FE0F" invariant is pinned by a gate test): for VS16, `ratatui` additionally sends the glyph's trailing cell to the terminal, and the `crossterm` backend tracks position by cell number without accounting for its width — that trailing write happens without a `MoveTo`, lands one column to the right, and shifts the rest of the row (an adjacent wide emoji goes dark, the popup's border drifts). Under a full redraw, where "changed" cells are all of them, this shows up on every frame.
- **Line breaks on unix terminals**: the legacy encoding sends the same CR for both `Shift+Enter` and `Enter`, so on a "bare" terminal a line break was unavailable. On unix, `runtime` enables the **kitty keyboard protocol** at the `DISAMBIGUATE_ESCAPE_CODES` level (`crossterm::event::PushKeyboardEnhancementFlags`) if the terminal supports it (`supports_keyboard_enhancement()`) — then modifiers on special keys (`Enter`/arrows/…) are reported, `Shift+Enter` is distinguishable from `Enter`, and `Shift`+arrows from plain arrows (bringing keyboard selection to life). Flags are cleared on exit and in the panic hook. Printable input and a lone `Shift`+character aren't touched by the protocol (text comes through as-is) → the layout-independent Ctrl-shortcut parsing and typing `?`/emoji don't regress. Not needed on Windows (the Console API reports modifiers). For terminals **without** the protocol — **`Alt+Enter`** gives the same line break (it arrives as `Enter`+`ALT` via the meta-prefix `ESC`, recognized even on legacy terminals); accepted in every multiline field (chat, the system message/greeting in settings, the self-model editor).

### 11.6. The settings screen

Sections (a left-hand menu with a field count). Within a section, fields are laid out into
**semantic groups** (a group header `Group ────`, not navigable); values are
aligned in **one shared column across the whole section** — sized off the section's
longest label, with a floor and a cap: every group's values sit on one vertical line,
and an overly long label doesn't drag the column away (its value sits right after the
label; current labels stay shorter than the cap — extra context moves into the group
header). Below the list — a **hint panel**:
the selected field's full value (paths/URLs, truncated with `…` in the list) + a description hint.
Its height is that of the **longest hint across the whole catalog** — every section and
subsection, not just the one on screen (bounded above — an MCP tool's description is
arbitrary server text — and below by the three rows it has always had): a hint clipped
mid-sentence is unreadable, a height following the *selected* field would shift the list
on every step, and a per-section height resized the panel on every section switch — one
terminal size and locale, one panel height. The full value shares the panel and gives way
to the hint (it is also visible in the list row; the hint is only here); whatever the
panel still cannot show whole — the preview of a huge value, a hint in a window smaller
than even the reserved rows — ends with a visible `…` rather than stopping as if the
text ended there.
Sections with **subsections** (Model/Sampling/Profiles) show them as a **tab strip** above the
fields (a pinned row `Assistant │ Impersonation │ …`, `←/→` switches the active
tab) — not as a row-field in the list. The hotkey footer is **contextual** (in "Profiles"
it adds `Ctrl+N`/`Ctrl+D`). **Field search `/`** — an overlay with a flat listing across
every section/subsection (matching against label/group/description; a breadcrumb "Section ·
Subsection › Group › Field  value"), `↑/↓` to select, `Enter` — jump to the field (switching
section and subsection), `Esc` — cancel.

- **Model/server** — a tab strip **Assistant │ Impersonation │ Embeddings** (three
  of the app's servers, mirroring the status-bar chips); on the right of the section
  header — a **status chip** for the active subsection's server (`● ready` / `◐ connecting…`
  / `✕ not configured` / no connection with a reason): tweak the engine and see the effect
  without leaving to the chat. Mode (managed/external/openai/gemini/claude); for managed —
  the *Server* group (the `llama-server` binary, host, port), *Model* (GGUF `-m`,
  context `-c`, `--jinja`), *Performance* (`-ngl`, FlashAttn, `--no-mmap`),
  *Speculative decoding* (`--spec-type` + draft-model fields); for cloud —
  *Provider* (model, API-key-env, base URL). The **Embeddings** tab is the dedicated
  embedding server for RAG/memory (mode + parameters). **Changing the model = restarting
  the server** — with a debounce (~1.2 s of quiet): a burst of quick edits to engine
  fields is coalesced into a single restart with the final values; the config saves
  right away. The restart is decided against what the server is **actually
  running**, not against the previous edit — so an edit and its undo (`Ctrl+Z`)
  cost nothing, while a genuine change still restarts.
- **Sampling** (a tab strip Assistant/Impersonation): parameters by group —
  *Basics* (temperature, top-k/p, max_tokens, seed), *Dynamic temperature*,
  *Diversity* (min-p, top-n-sigma, typical-p, adaptive-p, XTC), *Repeat penalties*,
  *DRY (anti-repeat)*, *Mirostat*, *Sampler order*, *Reasoning* (thinking,
  reasoning_effort). In a cloud mode, parameters unsupported by the provider are hidden.
- **Tools**: only gates/parameters for tools — *Agentic loop*
  (`max_tool_rounds`, the sub-agent's per-reply token cap and whole-run time
  limit — `subagent_max_tokens`, `subagent_run_timeout_secs`), *Web search*,
  *Python* (a switch + the path), *Files* (access + a sandbox directory).
- **Memory**: *Knowledge base (RAG)* (chunk/overlap/cap sizes), *Notes*
  (auto-consolidation, "about self" observations in `note_recall`), *Self-model*
  (insight storage/injection, description target size, auto-reflection, the maintenance
  protocol).
- **Profiles** (a tab strip Assistant/Impersonation): CRUD (`Ctrl+N`/`Ctrl+D`;
  from the chat, **`/profile new|delete`** reaches the same two commands for a
  host that claims those keys, and always confirms a deletion — §11.7);
  profile picker, name,
  the *Persona* group (system message, greeting) and **tool toggles grouped by
  meaning** (`features/tools/meta.rs`: Introspection / Memory and Knowledge /
  Outside World / Files / Utilities / Sub-agent / Conversation Control /
  Self-Model) with a short inline description and an "on/total" count in the group
  header. A tool that's **disabled by a global gate** (web/python/files) but
  enabled in the profile is marked in the warning color with a "disabled
  globally" hint — honestly showing that it's unavailable to the model.
- **Interface**: the *Appearance* group (theme, **legacy-terminal compatibility** —
  see below, table row separators, Mermaid diagrams, **the model's name next to the
  assistant's header** — `show_model_name`, off by default, §11.3, OSC 52 clipboard),
  *Spelling* (spellcheck on/off, dictionary selection), *Behavior*
  (confirming `Ctrl+R`/`Ctrl+E`; **automatic chat titling** — after the user's
  message / after the assistant's reply (default) / off, §11.2), *Conversation
  copy (F5)* (what's included).

**Navigation** (a two-level focus: the section menu ↔ the field pane). In one
sentence: *the arrows change, `Enter` goes in, `Esc` goes out, `Tab` switches section.*
`Enter` is the **only** way into the field pane (`→` deliberately doesn't enter);
`Esc` is the only way out of it and always means "one level up" — from the field
pane back to the sections, from the sections out of the screen (the field editor,
the `Choice` popup and the `/` overlay already close into the pane the same way).
Inside the pane `←/→` mean exactly one thing — change the value or switch the
subsection tab — and never move focus. `Tab`/`BackTab` switch the section and
**preserve** the current focus (the field index resets). Rationale: while `→`
entered the pane, users built the model "`←` leaves it", but `←` has to cycle a
`Choice` value — and the first fields of most sections are `Choice` (the server
mode, the theme), so the intended return keystroke silently changed a setting that
is saved at once and restarts the server. The hotkey footer is **focus-contextual**
— that's where the model is stated. Which pane holds the focus is shown by a marker
in each title (`▸` on the sections, `◆` on the parameters): the glyph is always
drawn and only its **colour** moves — green for the pane that has the focus, muted
for the other. See docs/history/settings-navigation.md.

**Field editing.** For `Choice` toggle fields, `←/→` quickly cycles the value, while
`Enter` opens a **list popup** of every option (important for `--spec-type`,
modes, themes, the profile picker). Text/numeric fields are edited in an editor
(`Enter` commits) with **validation**: an invalid number (letters) doesn't close the
editor — the label turns red with a hint; a fix or `Esc` close it. An accent-colored
**`•` marker** on the left flags a field whose value differs from the default;
**`Del`** resets a field to its default (profile fields aren't affected).

**Undoing an edit** (`Ctrl+Z`, redo `Ctrl+Y`). Every commit is applied and saved
at once, so the screen keeps a stack of the edits made during **this visit** (it
is rebuilt on each `Ctrl+P`) and `Ctrl+Z` puts the previous value back, re-emitting
the same intent the edit produced. A run of edits to the *same* field is one step
(cycling past the value you wanted comes back in one press), and the cursor
**jumps to the field it reverted**, switching section and subsection if needed —
otherwise reverting a change made elsewhere would be invisible. Inside an open
field editor the same two keys stay the editor's own text undo. Not undoable, by
nature: an **API key** (the screen never holds it), **creating/deleting an
assistant profile** (owned by the orchestrator; deletion cascades over the
profile's chats) and **confirming an MCP catalog** (a trust decision, §9.6) — all
of them deliberate actions behind their own keys. Impersonation personas live in
the config, so their creation/deletion *is* undoable. See
docs/history/settings-undo.md.

**Legacy-terminal compatibility mode** (`interface.terminal_compat`, off by
default). Older emulators (Windows 10's conhost, etc.) can't do emoji or some
Unicode characters — "tofu" squares are drawn instead of icons, and the `DIM`
modifier is ignored. When enabled, the interface switches to a safe glyph set
(`shared/theme.rs::GlyphSet`, targeting WGL4/ASCII: the standard console font
repertoire of Consolas/Lucida Console): `✦ ASSISTANT` →
`* ASSISTANT`, `❯` → `>`, the tool card's `⚒` → `#`, "thoughts" `▸/▾` → `►/▼`, statuses
`◐/✕` → `○/×`, `✓/✗/⚠` → `√/×/!`, a Braille spinner → ASCII `|/-\`, rounded borders
(`╭╮╰╯`) → straight ones, popup background dimming — a muted color rather than `DIM`.
Glyphs from WGL4 (`●`, `▌` rails, table box-drawing, the `█` scrollbar, arrows, `…`)
aren't replaced. Applies live (the palette is rebuilt from a `Settings` event);
emoji in message **content** aren't touched (that's data, not styling).

**Interface language** (`interface.language`, the "Interface language" field in the
"Interface" section, a Choice ru/en; `Ru` by default, from `defaults.json` on a fresh
install). Axis B of the multilingual support (docs/history/i18n-ui.md): text for
**people** (the status bar, feed — role headers/pills, settings, the chat list,
popups/help, the self-model screen, UI errors, the `F5` export) is read from the
selected language's `ui.*` bundle. **Independent of the agent language**
(`Profile.language`, axis A): a Russian UI + English-speaking agents is a legitimate
combination; there's no inheritance between the axes. Applied live (screens/widgets
get an `&'static Locale` together with the palette from the `Settings` event; every
language in the selector is shown in its own name). Tool results and the self-model
rendering in the feed/`F3` follow the **agent's** language (axis A) — they're
primarily for the model. Tool labels in the profile toggles
(`ui.tool.label.*`/`ui.tool.group.*`, resolved in the settings layer) and displayed
server-unavailability reasons (`ui.err.server.*`) are also localized; only the
technical engine probe errors (`shared/api/managed.rs` — the provider layer) remain
in Russian.

**Field editing**: text/numeric fields are edited in a popup editor
(`Enter` opens it, `Enter` commits, `Esc` cancels). A field's value is logically
**a single line** (a URL, a path, a number), so the editor works in
**single-line mode** `InputBox` (`set_single_line`): no word wrap, with
**horizontal scrolling** (a long value "slides off" to the left, the cursor is always
visible), `↑/↓` disabled, `Home/End` — to the start/end of the whole value,
line breaks on paste collapse into a space. **Exception — the profile's system
message**: it's multiline by nature, so it's edited in a **large multiline popup**
with long-line wrapping, where `Shift+Enter` inserts a line break and `Enter`
commits (like in the chat input box, see [11.5](#115-input-and-editing-spellcheck)).

**The "API Key" field** (the Model/Impersonation/Embeddings/Speech subsections, in
every mode that can need a key — the clouds and `external`; a managed server is a
local process and shows none) — a special case of a text field: its **value is a
status** ("configured (this computer)" / "not set"), not a secret. `Enter` opens an
**empty** editor in **masked mode** `InputBox` (`set_mask`: characters are drawn as
`•`, selection isn't sent to the clipboard, the field is single-line) — a saved key
can't be shown, and editing enters a new one; `Del` removes it. Committing sends a
`SetSecret` intent (not into the working config copy): the orchestrator encrypts the
key with a machine-bound key and stores it in `AppConfig::api_keys` as a record for
**this** machine, so the settings file stays portable — on another machine the key is
entered again, and it's read back when returning here.

Which key a row addresses follows the subsection's **mode**. A **cloud** key is
**shared** across chat, impersonation, embeddings and speech for one provider — enter
it once. An **external** key belongs to that **one slot**: the four `external`
subsections hold four independent URLs (the common setup is a cloud gateway for chat
beside a local `llama-server` for embeddings), so one shared key would send a
gateway's token to localhost. In external mode the key is also **optional** — a local
server needs none, and then no authorization is sent at all. Either way the adjacent
"API key (env)" field remains a fallback (an environment variable name; used if no key
is entered). Keys never appear in the `Settings` snapshot — only "configured" flags
(`secrets_present`). If the machine doesn't support encryption (Linux with no
`machine-id`), the field shows "unavailable on this system" and the env path
remains. See `shared::secrets`, docs/research/api-key-storage.md,
docs/history/external-api-key.md.

### 11.7. Keybindings (preliminary)

| Key | Action |
|---|---|
| `Enter` | send the message (configurable: `Enter`/`Ctrl+Enter`) |
| `Shift+Enter` / `Alt+Enter` | line break in the input box (`Alt+Enter` — a fallback for terminals without the kitty protocol) |
| `Shift+←/→/↑/↓`, `Shift+Home/End` | select text (`Ctrl+Shift+←/→` — by word) |
| `Ctrl+A` | select all text in the input box |
| `Ctrl+C` | copy the selection to the clipboard (no-op without a selection) |
| `Ctrl+X` | cut the selection to the clipboard |
| `Ctrl+V` | paste from the clipboard (as one chunk, multiline, without sending) |
| `Esc` | go back: the chat-list overlay (the same key closes it) — or **the search results**, if the chat was opened from a hit (§11.2.1), or **the conversation a `chat://` reference was followed from** (§11.3); the status bar's hint says which · cancel generation |
| `Ctrl+Q` / `F10` | quit the application (also from the chat-list overlay) |
| `/exit` / `/quit` | quit the application by typed command — the route no terminal can intercept (see below) |
| `Ctrl+N` | new chat (profile picker) |
| `Ctrl+P` | the settings screen |
| `Ctrl+F` | in a chat: in-feed search (§11.3.1); in the chat list: switch the search between titles and message content (§11.2) |
| `Ctrl+G` | in the chat list (content mode): the message-level search screen — `Enter` opens the chat at the message (§11.2.1). In the input box — spellcheck suggestions |
| `F2` | rename the chat |
| `F5` | copy the entire chat conversation to the clipboard (the active chat / the one selected in the list) |
| `Ctrl+R` | in a chat: regenerate the last response; in the chat list: ask the model to title the selected chat (§11.2) |
| `Ctrl+U` | write a message as the user (impersonation, §11.8) |
| `Ctrl+E` | delete the last exchange (the text is returned to the input box) |
| `/tts [N\|all\|stop\|pause\|resume]` | speak the chat's messages / stop / pause / resume (§11.9) |
| `Ctrl+K` | clear all text in the input box (undo it — `Ctrl+Z`) |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo an input-box edit (coalesced snapshots) |
| `Ctrl+←`/`Ctrl+→` | move the cursor by word (across line boundaries) |
| `Ctrl+Backspace`/`Ctrl+Delete` | delete the word left/right of the cursor |
| `Home`/`End` | a ladder of stops (§11.5): `Home` — the row's text, its start, then the whole line's; `End` — the row's end, then the line's |
| `Ctrl+Home`/`Ctrl+End` | move the cursor to the start/end of the input box's text |
| `Ctrl+T` | collapse/expand "thoughts" in the feed (per chat, §11.3) |
| `Ctrl+O` | collapse/expand tool calls in the feed — the header stays, the arguments/result fold away (per chat, §11.3) |
| `Ctrl+W` | toggle mouse capture: the wheel scrolls the feed ↔ native text selection |
| click/drag with the mouse in the box | place the cursor / select text (with `Ctrl+W` capture on) |
| click on a `chat://` reference in the feed | follow it (with `Ctrl+W` capture on; `Ctrl+L` is the route that needs no mouse) |
| `Ctrl+B` | the emoji picker popup (inserted into the input box at the cursor; remembers the last choice) |
| `Ctrl+L` | follow a `chat://` reference the assistant wrote: a picker of the conversations this chat links to, `Enter` opens (§11.3) |
| `PageUp`/`PageDown` / mouse wheel | scroll the feed |
| `e` (on a message) | edit the message |
| `Space`/`Tab` (on a block) | collapse/expand a **single** block — deferred; today `Ctrl+T`/`Ctrl+O` act on the whole feed |
| `F1` / `?` | the help/"about" dialog (tabbed, see below) |

**The help/"about" dialog (`F1`/`?`)** — a modal popup in the KDE/Qt style:
a logo lockup in the header, a tab strip, and scrollable content for the active
tab with a scrollbar. Tabs (in the order shown): **"About"** (the brand name and
description, then the facts — version, license (the SPDX id from `Cargo.toml`; the full
text is its own tab), build target (`std::env::consts` — the OS and architecture the
binary was built for, so a report names the actual build), the links — the website
`mindfork.io`, the crate, the repository — and the author; laid out as the same
**leader table** the "Components" tab uses, one value column anchored against the
mirrored right margin, because a value column left-aligned one step past the widest
label left the right ~30 columns of a wide dialog empty),
**"Hotkeys"** (this table), **"Commands"** (input-box commands `/rag …`/
`/tts …` — kept out of the keybindings list so it doesn't clutter it), **"License"** (the MIT text),
**"Disclaimer"** (`DISCLAIMER.md` — what the author does not answer for when the model that
writes every word on screen was chosen and downloaded by the user: generated output,
third-party models and providers, the tools a model can invoke, cloud egress),
**"Components"** (third-party dependencies — **name, version, license**; the list is checked
against `Cargo.toml` (names) and `Cargo.lock` (versions) by `shared::credits` gate tests;
the tab is laid out as **leader tables** (the geometry shared with "About"): the name on the left margin, the version and
license columns aligned under each other against the mirrored right margin, and the run
between bridged by a dotted leader in the dialog's dimmest color — the tab's natural width
is well under the dialog's, and a left-hugging table left the right half empty, while
centering it was rejected because nothing else in the app centers text).
Opens on the "Hotkeys" tab (`F1`/`?` — the familiar help key), and on
reopening — on the **last-selected** tab (remembered). Navigation:
`Tab`/`←→` — switch tabs, `↑↓`/`PgUp`/`PgDn`/`Home` — scroll the active tab, `Esc`
(or `F1`/`?` again) — close, `Ctrl+Q`/`F10` — quit. The dialog **follows the
terminal between bounds** — 76–96 columns × 34–44 rows of content — one size for
every tab, so the window doesn't "jump" on switching: the maximum caps line length
for readability on a wide screen, and below the minimum the dialog is clamped to
the screen instead of shrinking further (a hard degradation). The logo is drawn only
when there's enough height/width (a hard degradation, docs/branding.md §5); the license
text wraps by word to the dialog's width, and the disclaimer — markdown at the source —
goes through our own renderer (ADR 0003), with wrapped list items hung under their marker.
The "License"/"Disclaimer"/"Components" tabs' data comes straight from
`shared/credits.rs`, bypassing the locale bundles — "Components" because it is language-neutral
(names/versions/SPDX), the two legal tabs because they are whole **documents**: `ru` shows the
translations under `docs/legal/` and every other language (an external bundle included) the
authoritative English original, chosen by `credits::license_text`/`disclaimer_text`. The
translations are unofficial and say so in their own first paragraph — the English text governs, and
a locale bundle cannot bring legal text of its own. A separate pair of tabs for them was never an
option: the `ru` strip already measures exactly the dialog's minimum width (below).
The **disclaimer is a tab of its own, not a tail on "License"**: the `LICENSE` file must stay
byte-identical to the canonical MIT text or the `MIT` SPDX identifier we publish stops being
truthful and license scanners start reporting "Other" (a `shared::credits` gate test holds
that line). The strip's labels are width-budgeted — a `screens::chat` gate test checks that
it fits the dialog's **minimum** width in **every** bundled locale, since the tab that
overflows is the rightmost one and would be silently truncated for one language only.
The **rows of the "Hotkeys"/"Commands" tabs are width-budgeted the same way**, and for the
same reason: those tabs are plain `Paragraph`s with no wrapping of their own, so a
description longer than the dialog simply lost its tail mid-word — in `ru` while looking
fine in `en`, or the other way round. Each tab is laid out as one table: related entries
sit in **groups separated by a blank line** (the groups carry no headers, so they need no
locale keys), and every description starts in the **same column** — one past the tab's
widest label, measured per tab and per locale in display columns, since the labels differ
in width between languages and a fixed constant would be wrong for one of them. A long
description **wraps, hung under that shared column**, and a gate test asserts every row of
both tables fits at both bounds of the dialog's width range in every bundled locale, with
a second test pinning the one-column alignment itself.
Wrapping rather than shortening, because the alternative is writing the help twice: once
short enough for `en` and once for whichever locale the label is widest in.
The popup's title is `mindfork v<version>` (the brand name `credits::APP_NAME`, not the
package `mindfork-rs`); the same text is set as the **terminal window's title**
at startup (`SetTitle`, Windows).

Ctrl-shortcuts are layout-independent: a character is normalized to the "physical"
Latin key (`shared/keys.rs::hotkey_char`), so `Ctrl+Q` (quit) works even under an
active Cyrillic layout (where the physical Q key delivers a Cyrillic `Ctrl` combo).
On **Windows** the physical key is resolved through the keyboard layout itself
(`VkKeyScanExW` → `MapVirtualKeyExW` → scan code → the letter at that position on
QWERTY) — the exact inverse of the lookup the terminal layer used to produce the
character, so **any installed layout** works (Greek, Hebrew, Georgian, Bulgarian
BDS/phonetic, …) without per-language data. A static JCUKEN table stays as the
fallback: on unix (where a terminal application cannot query the layout) and on
Windows when the active layout is undeterminable (conhost). ASCII characters are
never remapped by position — on Latin layouts (AZERTY/QWERTZ/Dvorak) a shortcut
belongs to the key *labeled* with that letter. Details and the platform matrix —
[docs/research/layout-independent-hotkeys.md](docs/research/layout-independent-hotkeys.md).

**Selection/copy/undo/quit** (see [docs/history/input-selection-undo-mouse.md](docs/history/input-selection-undo-mouse.md)).
Selection and **undo/redo** live inside the `InputBox` widget itself — available in every
input field (chat, chat renaming, settings fields, the self-model editor). Clipboard
copy/cut (`Ctrl+C`/`Ctrl+X`) is a UI-layer side effect (`runtime` via
`arboard`, the text is already at the UI, the orchestrator isn't involved); so far it's only wired up in the chat input box.
**Undo/redo** (`Ctrl+Z`/`Ctrl+Y`) — a stack of `(lines, cursor)` snapshots with coalescing:
consecutive edits of the same class (typing/deleting) merge into one undo unit
(typing breaks at a space — word-granular), navigation/selection/a structural edit
(a line break, a paste) start a new one; a programmatic text replacement (`set_text`,
sending) clears the history. **`Ctrl+K`** clears the box (one of the undo units — undone by
`Ctrl+Z`; the previous toggle semantics were removed). **Quit moved from `Ctrl+C` to
`Ctrl+Q` + `F10`** (freeing `Ctrl+C` for copying): `Ctrl+Q` is reliable in raw mode (crossterm
strips XON/XOFF flow control), `F10` is a second option in case the terminal/DE
intercepts `Ctrl+Q` (`F10` itself opens the emulator's menu on some Linux DEs, but
can be disabled — the two keys back each other up).

**`/exit` and `/quit` — the third route out.** The two keys back each other up only
until a host claims *both*, and VS Code's integrated terminal does exactly that (its
own `Ctrl+Q` chord, `F10` as the debugger's "step over"): the app then has no
advertised way out at all, and the pair of keys is what the help overlay teaches. A
slash command is ordinary typed text and reaches the app whatever the host binds, so
it is the route that cannot be taken away — the same argument that put `/image paste`
next to `Ctrl+V` ([§9.10](#910-images-in-a-message-image-attach)). **Two spellings, not one**:
`/exit` and `/quit` are the words every REPL, shell and database client answers to,
and someone looking for the way out types whichever they already know instead of
opening the help they are trying to leave. Both behave exactly like the keys: they
quit during generation too, and a stray argument (`/exit now`) leaves a note naming
both the bare command and the keys rather than going out to the model — a mistyped
quit must not read as a refusal to quit. The command clears the input box before
quitting, and the loop flushes that final draft, so the box does not come back next
launch holding the command used to leave it.

**Every action has a typed route as well as a chord** (`features/ui_command.rs`,
`screens/chat/commands.rs`; [docs/history/command-only-control.md](docs/history/command-only-control.md)).
`/exit` generalized: a terminal embedded in a host loses whole chords before
crossterm sees them — VS Code's integrated terminal claims `Ctrl+P`, `Ctrl+E`,
`Ctrl+F`, `Ctrl+K`, `F1`, `F3`, `F5` and both quit keys through its default
`commandsToSkipShell`, and a browser tab reserves `Ctrl+N`/`Ctrl+T`/`Ctrl+W`,
the last of which **closes the tab the session runs in**. Typing survives every
host, so the interface is fully operable by commands plus the safe key subset
(printable characters, `Enter`, `Esc`, `Backspace`/`Delete`, `Tab`, the arrows,
`Home`/`End`, `PageUp`/`PageDown`, `Shift`+arrows). Nineteen commands:
`/settings` `/self` `/chats` `/help` · `/new [profile]` `/rename [title]`
`/clone` `/copy` `/regen`·`/retry` `/takeback` `/impersonate [text]` `/stop` ·
`/find [text]` `/search <text>` `/links` · `/thoughts` `/toolcalls` `/mouse`
`/emoji`. Load-bearing properties:

- **A command is its key.** Each one reaches the action through the *same*
  handler the chord uses (`handle_ctrl_shortcut`, or the chord's own intent), so
  the two cannot grow two semantics — the `confirm_destructive_keys` popup
  applies to a typed `/regen` exactly as to `Ctrl+R`.
- **A command answers where a key may stay silent.** A chord that does nothing
  costs a keypress; a typed command that vanishes reads as a refusal, so every
  precondition (nothing generating for `/stop`, a turn running for `/regen`, no
  chat open for `/copy`, the settings snapshot not yet arrived) leaves a
  localized note naming a route that works (§4 of docs/lessons.md).
- **`Esc`'s three meanings split in two**: `/stop` always cancels the turn,
  `/chats` always opens the list.
- **Bare `/rename` hands the current title back as an editable command line**
  rather than opening a popup: the box is where a command is typed and is empty
  by definition at that moment, and it teaches the syntax by example.
- **One registry, not nineteen parser modules** — each command is an exact word
  plus at most one free-text argument, so copied parsers would be the sliding
  self-duplication the gate keeps catching; commands with real syntax
  (`/file`, `/image`, `/rag`, `/tts`) keep their own modules. The **help tab's
  rows are derived from the registry**, so a command cannot exist undiscoverable.
- **Not commands: text editing.** A command is typed *in* the box, so it cannot
  operate on the box's contents; the safe keys cover editing in every host, and
  `Ctrl+Z`/`Ctrl+K`/`Ctrl+C` stay conveniences.

**Stage 2 — the two actions inside other screens.** Profile CRUD lived only
behind `Ctrl+N`/`Ctrl+D` in the settings screen's "Profiles" section
([§11.6](#116-the-settings-screen)) and clearing the self-model only behind
`Ctrl+K` twice in its screen ([§17.7](#177-ui--the-self-model-screen-f3)) —
both unreachable from a host that claims those keys, and a physical-key
alternate (`Insert`) fails on a Mac client keyboard, which is what a browser
terminal is often driven from. Hence `/profile list · /profile new [name] ·
/profile delete <name>` and `/self clear`:

- `/profile` keeps a **parser of its own** (`features/profile_command.rs`),
  being the one typed route with a subcommand *and* an argument — the line the
  registry draws. `/self` instead gained a closed-set argument
  (`Arity::Subcommand`), so an unknown word is reported by the parser rather
  than by whoever runs the command.
- **Both destructive routes always confirm**, where the keys they mirror do not
  (`Ctrl+D` deletes the selected row outright). Deliberate: the screen shows you
  the profile you are deleting, while a typed prefix can resolve to one you did
  not picture, so the popup is what puts the target — and the number of
  conversations hidden with it — back in front of you. This is independent of
  `interface.confirm_destructive_keys`, which is about the two chat-level keys,
  and the commands therefore set the popup directly instead of going through
  `trigger_destructive`.
- **The outcome is reported by the orchestrator**, not by the command: from the
  settings screen the new (or vanished) row is the answer, but a chat has no
  profile list on screen, and a command that appears to do nothing reads as a
  refusal. Emitting it where the write happens means the claim is only made on
  success — and both routes answer alike.
- `/profile delete` refuses the **last** profile before asking, rather than
  posing a question the orchestrator would then decline; `/self clear` refuses
  with no chat open, the model belonging to the active profile.

**`/export [md|json] [path]` writes the conversation to a file**
(`features/export_command.rs`, `chat_export::to_import_json`;
[docs/history/chat-export-file.md](docs/history/chat-export-file.md)). The route
out that needs no clipboard at all — which is the only route in JupyterLab's
terminal, where the escape below is dropped and the pty is server-side anyway.

- **Markdown is byte-for-byte what `F5` copies.** One formatter, so the file and
  the clipboard cannot drift; the extension is honest because the *content* is
  Markdown already — that is how models write, and what the feed renders. It
  honours `config.copy`, so "what a copy includes" means one thing.
- **JSON is a `mindfork-import` v1 document** ([import-format.md](docs/import-format.md))
  carrying explicit ids, so `mindfork-rs import` puts it back onto the *same*
  chat — export and import are a round trip. The format has nowhere to put tool
  calls, and every JSON export's note says so rather than leaving it to be
  discovered.
- **Where it lands**: a path as given, or `<date>-<slug>.<ext>` generated from
  the title — both relative to the **current working directory**, and the note
  answers with the absolute path. An **existing file is refused**, never
  overwritten. Empty conversations are refused too, as `F5` already does.
- Attachments and images are not included; the chat list's own selection has no
  export route yet (it has `F5`).
- **A chat's copy or export does not carry its sub-agent transcripts**
  ([§9.3.2](#932-call_subagent)): Markdown includes a `call_subagent` card's
  *result* text under `copy_tool_results` — the sub-agent's final reply and the
  transcript's address — and the JSON document has nowhere for tool calls at
  all. A transcript is copied or exported from its own row (`F5`) or its own
  read-only view (`/copy`, `/export`), where it is a conversation like any
  other.

**A copy has two halves: the local clipboard and the terminal's** (OSC 52,
`shared/osc52.rs`; [docs/history/osc52-clipboard.md](docs/history/osc52-clipboard.md)).
`arboard` writes the clipboard of the machine the *process* runs on — over SSH
the wrong one, and on a headless box none at all (its constructor fails, and the
copy used to report only an error). OSC 52 hands the text to the terminal the
user is sitting at, travelling the same pipe the drawing does. Both copy routes
(`Ctrl+C`/`Ctrl+X` on a selection, `F5`/`/copy` on a conversation) funnel through
one writer, so they cannot disagree. Governed by
**`interface.clipboard_osc52`** ("Interface" section): `auto` (the default — only
when the session looks remote, `SSH_TTY`/`SSH_CONNECTION`, or the local clipboard
failed), `always` (for a remote session the environment does not advertise — a
container, a web terminal), `off` (a terminal that renders an unknown OSC as
text). What the protocol forces:

- **Nothing comes back.** There is no acknowledgement and no way to ask whether
  the terminal supports the sequence — the query form of OSC 52 *is* the
  clipboard-read path terminals disable as a leak vector. So the note says the
  text was **sent**, never that it arrived, and names the possibility that the
  terminal ignored it.
- **A ceiling of 74 994 bytes** (the 100 000-byte sequence limit, less the header
  and base64). Past it nothing is sent and the note says so: a silently truncated
  conversation looks complete, which is the worse failure. The local clipboard
  still has the text when it worked, and the note says which.
- **tmux** gets DCS passthrough with the inner escapes doubled; **screen** is
  deliberately unsupported (a different wrapper plus 768-byte chunking, for a
  shrinking audience — it degrades to the previous behaviour).
- Support is uneven and undetectable: yes on alacritty, kitty, konsole, mintty,
  Windows Terminal, wezterm, foot, iTerm2 and VS Code's terminal (1.93+); **no**
  on GNOME Terminal, Terminal.app, URxvt, Termux; opt-in on xterm and st. Note
  that **JupyterLab does not support it** — it embeds xterm.js without the
  clipboard addon — so the browser-terminal case that first raised this is
  exactly the one it cannot serve.

**Mouse in the input box** (stage D). With mouse capture on (`Ctrl+W`, §11.3), a left
click places the cursor in the chat input box, and a drag grows the selection (the cursor
snaps to a grapheme-cluster boundary, so it doesn't land in the middle of an emoji). The widget
remembers the area of its last render and inverts the wrap layout
(screen coordinates → a position in the text); a click below the last row → the end of the text,
a click right of a row's end → the end of that row. Click/drag never change the content (don't wake
the spellcheck debounce/draft save) and don't bump the wrap cache's revision. A click outside the box
(in the feed) is a no-op. Double-click (word selection) is groundwork (crossterm doesn't
give it to us directly).

**Preserving deleted exchanges (`Ctrl+E`/`Ctrl+R`).** Deleting an exchange (`Ctrl+E`) and
regenerating the last response (`Ctrl+R`) are irreversible in the UI, but nothing deleted this
way is lost on disk: each such deletion appends an entry to the
`Chat.deleted` collection (an array of `DeletedExchange { deleted_at, messages, draft }` in the chat
file). An entry captures the moment of deletion (`deleted_at`), the deleted messages
(`Ctrl+E` — the user message and the assistant's response; `Ctrl+R` — the assistant's
response and the round's tool messages), and `draft` — the input box's contents at the
moment of deletion (before the user's text is returned, for `Ctrl+E`). New entries are prepended
to the collection (recent deletions are faster to find). There's no restoration via the UI:
the collection exists only for **manual** JSON editing in rare cases where
something important was deleted. The field is only serialized when non-empty
(`skip_serializing_if`).

**Confirming `Ctrl+E`/`Ctrl+R` (configurable).** Since both operations are
irreversible in the UI, they can be protected with a confirmation: the
`interface.confirm_destructive_keys` setting (the "Interface" section, off by
default). With the setting on, `Ctrl+R`/`Ctrl+E` don't fire immediately but
open a modal "Confirm" popup (`Enter` — yes, `Esc` — no; `Ctrl+Q`/`F10` still
quit the application through the popup; other keys are ignored, and the popup stays
open). With the setting off — the previous instant behavior. Both operations are still
ignored while generation is running. The popup's state (`ConfirmAction`) lives in
the chat screen; the intent (`RegenerateLast`/`DeleteLastExchange`) is only issued on `Enter`.

### 11.8. Impersonation (writing a message as the user)

`Ctrl+U` makes the model write the next message **as the user** into the input
box. The request-building algorithm:

- the chat's system message (the assistant's persona) is replaced with the
  **impersonation profile's** system message — the user persona the chat's assistant
  profile points at (`Profile.impersonation_profile_id` →
  `AppConfig.impersonation_profiles`); no reference, a dangling id (the persona was
  deleted), or an empty message → a shared default;
- in the history, **user ↔ assistant roles are swapped**, and system/tool messages and
  empty ones are dropped (there are no tools in this mode);
- the history is the **compacted** one (§6.7): a folded prefix is replaced by the
  rolling summary block, so impersonation cannot hit the context ceiling a
  regular turn is already protected from. The block is built with the "no
  read-back tools" wording, since this mode carries none. Note the shape it
  produces: a cut always lands on a `User` message, which the swap turns into a
  **leading assistant turn** (accepted by Anthropic, native Gemini and OpenAI
  Responses — measured 2026-08-08); the tail still ends on `user`, which matters
  because a *trailing* assistant turn reads as a prefill and the model would
  continue it instead of writing the next message;
- sampling comes from the "Impersonation" subsection (`AppConfig.impersonation_sampling`),
  but reasoning is **forced off** within it (`reasoning_budget=0`): the mode
  discards "thoughts", and for models with thinking "baked" into the chat template, otherwise
  the entire token budget goes into `reasoning_content` and the reply comes back empty
  (cf. §8).

**The impersonation server** (`AppConfig.impersonation_engine`) has three modes
(`ImpersonationMode`): `shared` — the same server as the assistant's (managed or
external), but with impersonation sampling; `managed` — a separate `llama-server`
child process; `external` — a separate remote OpenAI-compatible server.

**UI:** while it's being written, the input box is hidden, and in its place is a
non-editable **streaming preview** of the reply with a spinner (`widgets/impersonation_preview`).
Once finished (unless cancelled with `Esc`), the text is placed into the input box; on
cancellation the box keeps its original text. If the input box wasn't empty, its text is passed as a
**seed** — the model is asked to continue what's there (outputting only the continuation), and
the preview shows the seed + the generated continuation.

**Impersonation profiles** (the user personas) are a list of their own —
`AppConfig.impersonation_profiles` (`{id, name, system_message}`), stored in
`settings.json` next to the rest of the globally-configured impersonation
(`impersonation_engine`, `impersonation_sampling`) rather than in `profiles.json`.
An assistant profile references one by id, so "who the assistant is" and "who I am
in this conversation" travel together; several assistant profiles may share one
persona. Deleting a persona doesn't touch the referencing profiles — a dangling
reference simply reads as "not set". The legacy per-profile field
`Profile.impersonation_system_message` is migrated once at startup into a named
persona ("«profile name» (impersonation)") and linked; the field itself stays on
disk (nothing reads it any more).

**Settings** (§11.6): the "Model/server", "Sampling", and "Profiles" sections get
an "Assistant"/"Impersonation" subsection selector. The two "Profiles" subsections
edit **different lists**: "Assistant" — the AI-interlocutor profiles (plus the
"Impersonation profile" field choosing the persona), "Impersonation" — the personas
themselves (selector, name, system message; no tools). `Ctrl+N`/`Ctrl+D` create/delete
in whichever list the active subsection shows.

---

### 11.9. Speaking messages aloud (TTS)

Chat messages can be **spoken aloud** with an input-box command
([docs/research/tts.md](docs/research/tts.md); the main engine is OpenAI TTS,
decision 2026-07-23):

```
/tts            speak the last message
/tts N          speak the last N messages (user and model)
/tts all        speak the entire chat conversation
/tts stop       stop playback (drop the queue)
/tts pause      pause (keep the queue)
/tts resume     resume a paused playback
```

**Different voices per role.** If the active mode has a separate "User voice"
(`user_voice`) set and it differs from the assistant's voice, a multi-message
speak-out (`/tts all`, `/tts N`) reads the user's lines in that voice and the assistant's
in the main voice. Implemented with **two engines** of the same provider (the voice is baked into
the client at construction); the role travels through the pipeline (`build_utterances`/`chunk_utterances`
carry `(MessageRole, String)`), and the task picks an engine per chunk's role. With an empty `user_voice`,
everything is read in one voice. Independent of the "Speak roles" toggle
(that one adds spoken prefixes; the voices differ acoustically).

**Pause/resume.** `/tts pause` pauses playback, **keeping the
queue** (unlike `stop`, which drops it), `/tts resume` — resumes it.
Implemented with `rodio::Player::pause`/`play`: the device is opened by the handler and
shared with the background task via an `Arc<Playback>` (`Send+Sync`), so pausing
applies **instantly**. While paused, the queue isn't drained → synthesizing ahead
naturally holds itself back, and the task doesn't finish until playback resumes. Useful for long
text (`/tts all`).

**What gets spoken.** A message = a `user`/`assistant` one with non-empty text (system and
tool messages are skipped — the same rules as for the `F5` conversation copy),
in chronological order. "Thoughts" (CoT) are never spoken: they live
in the separate `Message.thoughts` field. "Speakable" text is extracted from markdown
(`shared/markdown/speak.rs::speakable_text` — a second consumer of the
`pulldown-cmark` events, ADR 0003): code blocks, ` ```mermaid ` diagrams, tables, and
block formulas ($$…$$) are **skipped with a brief spoken note** ("code block
skipped"), inline code is read as text, inline math is converted to unicode
(`latex_to_unicode`), and a link is read as its text only (the URL is omitted). Notes and
optional role prefixes ("User." / "Assistant.") are in **the profile's language**
(axis A, [docs/history/i18n.md](docs/history/i18n.md)): this is spoken content, not UI chrome.

**The provider** (`AppConfig.tts`, the "Speech" tab in the "Model" section, §11.6) —
an independent "server slot", like embeddings (ADR 0002): Anthropic has
no TTS at all, so speech is configured separately from the chat engine. Modes
(`TtsMode`): `openai` (default, `POST {base}/audio/speech`, model
`gpt-4o-mini-tts`, voice `onyx` — both verified with a live spike), `gemini` (native
`generateContent` with `responseModalities:["AUDIO"]`, model
`gemini-2.5-flash-preview-tts`, voice `Kore`), and `external` — any third-party
OpenAI-compatible TTS server (for offline/non-OpenAI users; a local managed
sidecar is groundwork). The cloud API key is **shared** with chat (ADR 0008) — no need to
re-enter it; if unconfigured → the command replies with a clear hint. For `gpt-4o-mini-tts`
speed is set **in words** in the "Instructions" field (this model effectively
ignores the `speed` parameter). Behavior settings: "Speak roles" (applies to
**every** command variant), "Interrupt on chat switch" (on), and "Interrupt on
generation" (off).

**Execution.** The orchestrator (the owner of `Chat`) takes a **snapshot** of the conversation
at the moment of the command — so the command also works during generation — and starts
a cancellable background task. The text is cut into chunks along sentence boundaries (per the
provider's limit: OpenAI 4096, Gemini/External 2000 characters) — a short message goes as a
**single request**; the task runs as a **pipeline**: while chunk N is playing, N+1 is
synthesized (the first sound arrives quickly). Playback — a `rodio` queue inside the
process; when no audio device is available (headless/CI/no sound card) → a "audio unavailable"
note, not a panic. Contract: `AppCommand::Tts(TtsScope)`/`TtsStop`,
`AppEvent::TtsActive(bool)`.

**Stopping** is centralized in one orchestrator helper (`tts_cancel`): a new `/tts`
command interrupts the previous one; `/tts stop` stops manually; **per setting** —
switching chats and starting generation; **unconditionally** (an invariant, not a
setting) — deleting an exchange (`Ctrl+E`), regenerating (`Ctrl+R`), and deleting a chat:
the text being spoken no longer exists. While synthesis/playback is in progress,
a quiet "♪ speaking" chip is shown in the status bar.

---

## 12. Configuration, portability, migration

### 12.1. Configuration

- `settings.json` (serde) with a `schema_version` field for future migrations.
- Profiles — `profiles.json`; chats — `chats/{id}.json`; notes/RAG — `data.db`; the personal dictionary — `personal_dictionary.txt`; dictionaries — `dictionaries/`; backups — `backups/`. All in the **data directory**, portable by default — the `data/` subdirectory next to the binary (keeping data separate from build/service files/caches). Logs — `logs/` there as well.
- **Installation defaults** are set by a `defaults.json` file next to the binary (`Defaults` = `DataLocation` + `default_language`): the storage mode (`mode`/`path`: `portable` → `data/` next to the binary; `system` → the standard OS folder via the `directories` crate; `path` → a custom directory) **and** the scaffold language for new profiles (`default_language`: `ru`/`en`, axis A — [docs/history/i18n.md](docs/history/i18n.md); an installer will fill this in based on the user's choice). The file always sits next to the binary (outside the data directory) — it's about the installation. No file / an empty one → portable mode + language `ru`; for backward compatibility the old `location.json` (storage mode only) is read; a corrupt JSON file → startup fails.

### 12.2. Schema versioning and migration

**The JSON migration framework** ([ADR 0006](docs/decisions/0006-data-schema-versioning.md), release-engineering.md §3.4). A clean Value-level framework — `shared/storage/schema.rs` (version constants, `JsonArtifact` = `current` + `detect` + a `Step` chain, an `Assessment` verdict); file I/O, gates, the pre-migration backup, and control-parsing — `features/data_migration.rs::run`, called from `main.rs` before opening storage (both the TUI and the CLI `import` path). The orchestration lives in `features` (not `shared`), since the pre-migration backup is `features::backup`, and `shared` can't depend on `features` (FSD).

- **Version detection is structural** (the format doesn't change today): `settings.json` — by the `schema_version` field (absent → 1); `profiles.json` — a bare array → 1, otherwise the `schema_version` field; `chats/<id>.json` — by the `v` field (absent → 1; not written while the schema is 1) — zero churn on existing files.
- **Timing — eager on startup**: `run` builds a plan (files with a version `< current`); if the plan is non-empty → **one** pre-migration backup (`backups/pre-migrate-<date>.zip`) before any write (failure → the migration is aborted, data untouched) → the `Step` chain → **control-parsing** into a typed struct (if the migrated data doesn't parse → refuse, the file isn't rewritten) → an atomic write of the migrated `Value` (temp+rename+`.bak`). While every schema is 1, the plan is always empty (the path is dormant, covered by a test on a synthetic artifact).
- **Downgrade guard**: a file version newer than the application → startup refuses with a localized message (silent corruption is worse than a refusal).
- **Bump policy**: an additive change (a field with `#[serde(default)]`, a table/column with a default) — no bump; a breaking change (a rename/move/semantic change/removal) — bump the constant + a migration step + a golden fixture of the old format + a CHANGELOG entry (the "Data" section).
- **SQLite** (`data.db`): the schema version is `PRAGMA user_version` (`DB_SCHEMA = 1`), a version-aware runner in `db/mod.rs::migrate`. The idempotent `baseline_ddl` (`CREATE … IF NOT EXISTS`) runs **every time** — this way new tables/indexes are added to existing databases **without** bumping the version (additive DDL and `user_version` are independent; `user_version` only tracks breaking steps). A fresh/existing database with `user_version = 0` is stamped with version 1 (not a data migration — no backup needed). Breaking steps (`DB_STEPS`, currently empty) run **each in its own transaction together with the `user_version` update** (a full rollback on error). A downgrade (`user_version` newer than the application) — startup refuses. The downgrade guard and the decision about a **shared** pre-migration backup (whether JSON **or** the DB needs migrating) are coordinated with JSON in a single `data_migration::run` step before storage is opened; the actual DB migration happens later, in `Db::open`. At that moment the DB isn't open yet (quiescent) → its files in the shared zip backup are consistent without a separate SQLite backup API.

**Import from external applications** (a one-time operation, not to be confused with schema migration):

- **The neutral exchange format `mindfork-import`** ([docs/import-format.md](docs/import-format.md), stage 1 of the "plugins" track — [docs/research/plugin-system.md §5](docs/research/plugin-system.md)): an external (possibly private) **converter** reads the source application's format and emits one JSON file (profiles + chats + optional global sampling/interface settings); the application imports it via the `mindfork import <file>` command (`features/import.rs`). Knowledge of non-public source applications (LameLLaMA) lives in the converters, not in the monolith. Properties: **idempotency** (deterministic UUIDv5s derived from stable `key`s; an optional explicit `id` — continuity with what was previously imported), strictness about structure (a wrong `format`/duplicate keys/a reference to a missing profile/an unknown role → a clear error) with tolerance for extension (unknown fields are ignored), a downgrade guard on `version`, tolerance for a BOM. Source settings are applied partially (only the fields that are set). The former `import-lamellama` command (an importer baked into the monolith) has been **removed** — its role is now played by the "converter → `import`" pair.

**The first real step** (2026-08-23): `SETTINGS_SCHEMA` 1→2 — when the sub-agent gained tools ([§9.3.2](#932-call_subagent)) its one-request timeout `tools.subagent_timeout_secs` became the whole-run `subagent_run_timeout_secs` (600 s) and the per-reply cap's default rose to 4096; the step renames the key, drops a value left at the old default (so the new one applies) and carries a changed value over. **`CHAT_SCHEMA` 1→2** (the same track, next stage): every old `call_subagent` record — in `messages` and in the `deleted` archive alike — gets a transcript synthesized from what it already holds (the persona and the message from `arguments`, the reply from `result`, the reply's time from the `Tool` message that answered the call; a record with no result becomes an instruction with no reply and no outcome), under a deterministic id (`Uuid::new_v5` over chat id + call id, so a restored backup migrates to the same `chat://` addresses), and the file is stamped `v = 2` — which `Chat` now **writes on every save**, so a migrated file is never handed to the step again. Nothing existing is removed or rewritten: the migrated file is a superset of the old one. `profiles.json` stays at 1.

### 12.3. Backup and deletion

- **Soft delete is mandatory** (`is_hidden`; cascading profile→chats→notes/RAG).
- A chat-file backup on save (atomic write-rename).
- **Backing up/restoring all data** — a TUI-free CLI (`features/backup.rs`): `mindfork backup [-o FILE] [-c 0..9] [-p PASSWORD]` (zips the data directory: `chats/`, `dictionaries/`, `data.db`, `profiles.json`, `settings.json`, `personal_dictionary.txt`, every `*.bak`, and `tools.fs_root` if it's inside the data directory; excluding `backups/`/`logs/`/`defaults.json`/`location.json`; compression level 0–9) and `mindfork restore <archive> [-p PASSWORD]`. The commands take the single-instance lock (protecting `data.db` from a race). **Restore is transactional**: archive validation (anti-zip-slip) before any destructive action → if data exists, an automatic pre-restore copy into `backups/` → clearing → unpacking; if unpacking fails — roll back to the pre-restore copy. The outcome is reported to the console.
- **The archive can be password-protected.** The password comes either from `--password` on `backup`/`restore` or from the settings ("Data" section), where it is stored **encrypted and bound to this machine**, exactly like a cloud API key ([ADR 0008](docs/decisions/0008-api-key-storage.md)); the argument wins over the setting, an empty value means "no password". With one set, every data entry is encrypted with **WinZip AES-256** (`manifest.json` deliberately stays readable — it holds no user data, so the "backup from a newer version" warning still works without the password), and the archive remains openable by 7-Zip/WinZip. Restore accepts an encrypted **and** an unencrypted archive with no mode switch: the zip layer discards a password an entry doesn't need. A missing or wrong password is refused **before** anything is deleted — the password is verified when an entry is opened, so the pre-flight validation above covers it; on an interactive terminal `restore` prompts for it (up to three attempts) instead of failing, which is the case when restoring an archive from another machine. The copies the app makes on its own — the pre-restore copy and the pre-migration backup ([§12.2](#122-schema-versioning-and-migration)) — are encrypted too, so the setting has no exception that quietly writes a plaintext copy of everything. **What it does not protect** ([docs/history/backup-password.md](docs/history/backup-password.md) §2): entry names, sizes and the directory structure stay visible (ZIP AES encrypts content only); the key derivation is fixed by the format at PBKDF2-HMAC-SHA1/1000, which is weak against offline brute force of a short password — hence the hint asking for a passphrase; and a password known only to a dead machine makes its archives unreadable, so it has to be recorded elsewhere.
- **Both commands narrate what they are doing.** Packing or unpacking a real data root takes seconds (compaction, then a few hundred entries), and a command that prints nothing until it is finished is indistinguishable from one that has hung — most of all right after the `restore` password prompt, where the echo-less input leaves the user unsure it was taken at all. So each phase announces itself **before** it runs (checking the archive → the pre-restore copy → compacting → packing → clearing → unpacking), and the two entry loops count themselves out (`N of M`, no more often than twice a second, so a small data root still finishes in silence). Keystrokes typed while the command was working are **discarded** on the way out (`features/terminal_input.rs`): they were typed at us, and without that the shell inherits them on exit and replays them as its own command line — which is what an impatient `Enter` at the password prompt used to do.
- **The database is compacted on both paths.** `data.db` accumulates free pages (deleted notes, `/rag remove`d chunks, an attachment index dropped with its chat) that SQLite never returns to the file system on its own. A backup packs a `VACUUM INTO` copy instead of the live file — smaller, and self-contained, so the `-wal`/`-shm` sidecars are folded in rather than packed; a restore compacts what it unpacked, which is what an archive made before this existed (or by another tool) needs. Both are **best effort**: a file that isn't a readable database is packed / left raw, because a backup that happens is worth more than a compact one. The backup path never modifies the source — it is refused before SQLite ever opens it if the header isn't a database's, and opened read-only otherwise; both guards are there because opening a database can make SQLite delete a stale sidecar next to it.

---

## 13. Security

### 13.1. Generation robustness

- Deliberately modest requirements: don't build elaborate prompt-injection protection. It's enough that: generation doesn't stop on EOS text ([section 7](#7-eos-and-stop-token-handling)); tool calls are syntactically valid (constrained generation on the server).
- RAG/notes/web content is fed in as data; web/Python are behind global switches.

### 13.2. Python execution

`python_exec` — **our own implementation** with two modes ([ADR 0005](docs/decisions/0005-python-sandbox-wasmer.md)); the `tools.python_enabled` master switch (**off** by default — a deliberate opt-in) gates the tool entirely regardless of mode:

- **The Wasmer sandbox** (default) — code runs isolated in **WASIX** via a `wasmer` sidecar process (the binary sits next to the app, in `data/sandbox/`, installed by `mindfork sandbox setup`). The guest **can't see the host filesystem** (only a mounted tmp directory with the script + a read-only `site-packages`), **network access is a toggle** (`python_net_enabled`; without it there are physically no sockets). Packages are pre-installed (numpy, pandas, requests, beautifulsoup4, …). Interruption is a **process kill** (Python runs in-process inside wasmer/V8); a **timeout** + a "one task at a time" gate. Doesn't require Python on the machine. Cross-platform (Windows/Linux/macOS). **Every call is a fresh sandbox** — the job directory is created per run and dropped afterwards, and the guest's `/tmp` dies with the process, so neither variables nor files survive between calls. The tool description says so explicitly: it is the model that pays for the assumption otherwise (measured — a 16 MB download written to `/tmp` in one call, gone by the next, then fetched again).
- **The local interpreter** — running code in a **separate process** of the system Python (the path is in settings; a venv is recommended), capturing stdout/stderr, a **timeout**, output truncation. **No OS-level sandbox** (the code runs on the user's own machine) — hence the sandbox being the default mode.
- **Sandbox resource limits:** a timeout (CPU) + wasm32 (~4 GB of address space) + a "one task" gate + an **optional hard RAM cap** (`tools.python_wasm_memory_mb`, off by default). The RAM cap is **Windows-only** (a Job Object; exceeding it kills the process, protecting the host from OOM; a minimum of ~1024 MB); not applied on Unix (rlimit is unreliable with V8). The tool's contract (`python_exec`, `{ code }`) doesn't depend on the mode/implementation.

### 13.3. Sub-agent and recursion

- `call_subagent` runs as a nested turn with the turn's tools **minus itself** — a loop below the top refuses the name whatever the set says, so there is no recursion; every dangerous call inside the run goes through the same confirmation as outside ([9.8](#98-confirmation-for-dangerous-tool-calls)); the run is bounded by `max_tool_rounds`, a per-reply token cap and a whole-run time limit ([9.3.2](#932-call_subagent)).

### 13.4. Privacy

- All data is local (JSON + SQLite). External channels — only the web tool and (in managed mode) the inference server's child process on localhost. Web is behind a switch. Notes/RAG isolation by profile is an invariant.
- **Untrusted content reaching the model.** Text from outside the conversation
  (an attached file, a fetched page, a tool result) is fenced and framed as DATA
  rather than instructions ([§9.7](#97-chat-file-attachments-file-attach)). An
  **image has no such analogue**: instructions can be painted into pixels, the
  fence idea does not apply, and the feed shows a chip rather than the picture —
  so the user cannot see what the model was shown. For an image the *user*
  attached that is their own doing; for one an **MCP server** returns it is a
  third party's, which is why `tools.mcp_images` exists as a switch separate from
  enabling the server ([§9.10](#910-images-in-a-message-image-attach)). The
  mitigation otherwise is the existing one: MCP tools are off by default, gated
  twice, and count as dangerous for the confirmation prompt
  ([§9.8](#98-confirmation-for-dangerous-tool-calls)).

---

## 14. Testing strategy

Idiomatic for Rust: `cargo test`, unit tests next to the code (`#[cfg(test)]`), integration tests in `tests/`. The architecture is designed to be testable: the orchestrator — with no UI and no model; tools — with no server; storage — on temp files and in-memory SQLite.

### 14.1. Principles

- **Unit tests require no inference server, GPU, or network.** Everything that depends on the engine is hidden behind the `EngineBackend` trait; tests use a mock/replay implementation.
- Async tests — `#[tokio::test]`; time (debounces/timeouts) — `tokio::time::pause`.
- Tests against a real inference server are marked `#[ignore]`, run manually (a URL/path via an environment variable; verified on Gemma 4 E4B-it via `llama-server`).
- Tests are written along the way, stage by stage, not as one final phase.

### 14.2. Coverage by layer

| Layer | What's covered |
|---|---|
| `entities` | Serde round-trip of every entity; `schema_version`; domain-model invariants. |
| `shared/api` (the engine) | Parsing "thoughts" (`<think>` and the reasoning field) from a stream of chunks, including tags split across a chunk boundary; mapping `SamplingConfig` → request parameters (dropping unsupported ones); no string stops by default ([section 7](#7-eos-and-stop-token-handling)). |
| `features/tools` | Every tool against a mock `Storage`/mock engine: result correctness, returning `effects` (rather than mutating state), JSON schema validity; `call_subagent` — no tools/history, limits. |
| `shared/storage` | Repositories on `tempfile` and in-memory SQLite; **isolation by `profile_id`** (negative tests); soft delete and cascading; write-rename atomicity; sqlite-vec kNN on small vectors. |
| `app/orchestrator` | **The most important block**: the `Idle/Generating/Cancelling` state machine; race scenarios (spamming Send, Stop→Send, dropping chunks by stale `generation_id`, deleting a chat mid-generation); the client-side agentic loop and applying `effects`; `max_tool_rounds`; sampling priorities; building the `ToolContext` snapshot; the "delete last" logic (returning text to the input box, prepending). |
| `features/spellcheck` | `check`/`suggest` on en_US and ru_RU; segmentation (apostrophes/hyphens, mixed text); multiple dictionaries; the personal dictionary. |
| `screens`/`widgets` | Pure view-model logic: applying a sequence of `AppEvent`s to a projection (including a "stale" `generation_id`); markdown/LaTeX-substitution rendering; collapsed/expanded blocks. Optional — `ratatui` snapshot tests via `TestBackend`/a buffer. |

### 14.3. Integration tests

- **Replay without a model**: the `EngineBackend` trait lets us record a real server's event stream into a JSON fixture and deterministically replay it in orchestrator/UI tests (end-to-end scenarios: message → stream → tool call → effect → final → save; regeneration; cancellation).
- **With a real server** (`#[ignore]`): loading the model, streaming, cancelling mid-generation; **anti-self-truncation** (a prompt provoking `<|im_end|>`/`<end_of_turn>` output — generation doesn't stop); a full agentic cycle with a test tool respecting `max_tool_rounds`; `call_subagent` with no recursion; a prefix-caching smoke test (the second turn is faster). Run on Gemma and Qwen.
- **Storage/migration**: a full lifecycle on a temp directory (create a profile→chat→messages→notes/RAG→hide→reopen); the importer (fixtures of old formats; idempotency).
- **TUI** (optional): `ratatui::TestBackend` to verify screen rendering against fixture projections.

### 14.4. CI

- On every push: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace` (unit + integration, no server). The primary runner is **Windows**; Linux is secondary.
- Tests against a real server aren't run in CI — manually before closing stages M1, M5, M6, M8.

---

## 15. Implementation stages and readiness criteria

| Stage | Content | Readiness criterion |
|---|---|---|
| **M0. Scaffold** | The FSD module structure, `tokio`, the `ratatui` loop, the entry point, single-instance, file logging | `cargo build`; an empty TUI launches; shuts down cleanly (terminal restored) |
| **M1. Inference** | `shared/api`: managed launch of `llama-server`, the OpenAI client, streaming, cancellation, parsing "thoughts" | Chatting with Qwen with no tools works in a minimal TUI; Stop interrupts generation |
| **M2. Domain + storage** | `entities`, `shared/storage` (JSON chats/profiles, SQLite) | CRUD for chats/profiles; data saves/loads; soft delete |
| **M3. Chat UI** | Screens/widgets: the chat list (search, 2 sort modes, renaming), the feed (markdown, thoughts, tool blocks, scrolling), our own input widget, spellcheck | The full chat loop works in the TUI; chat operations; spelling errors are visible, suggestions work (en+ru) |
| **M4. Profiles + isolation** | `Profile` with an id, chat linkage, notes/rag isolation, a greeting | A chat is created from a profile; data is isolated by `profile_id` |
| **M5. Tools (basic)** | Registry, the client-side agentic loop, notes + RAG (+ embeddings on demand) + introspection (sampling/system_message/time) | The assistant saves/retrieves notes and knowledge, changes the system message and sampling; tool blocks in the UI |
| **M6. call_subagent** | A sub-agent with no nesting and with limits | The model calls the sub-agent, gets a string result; no recursion |
| **M7. Web + Python** | Our own web search (DuckDuckGo + content extraction) + Python (subprocess) | Search and code execution are behind switches; Python is off by default on Windows |
| **M8. Settings screen** | All sections, profile CRUD, server/model management | Settings and `llama-server` launch parameters are editable; changing the model restarts the server |
| **M9. Gemma + polish** | Verifying Gemma 3/4 (templates/EOS/tool-calling/"thoughts"), themes, migration, backups, a release build | Chat and tools work on Gemma the same as on Qwen; old data imports; a release for Windows and Linux |

### 15.1. Cross-cutting quality criteria

- Correct stopping on Gemma and Qwen; no self-truncation on EOS text.
- Notes/RAG isolation by profile is confirmed by tests.
- Cancelling generation never leaves the application/terminal in an inconsistent state.
- `call_subagent` never causes recursion and stays within its limits.
- The ownership invariant: `Chat` has one owner (the orchestrator); tools read a snapshot and return effects — a deadlock is impossible by construction.
- Fast user actions (spamming Send/Stop/Regenerate, switching/deleting chats mid-stream) don't cause races — covered by orchestrator tests (no UI, no model).

---

## 16. Accepted decisions and open questions

### 16.1. Accepted decisions

| # | Question | Decision | Sections |
|---|---|---|---|
| 1 | Engine integration form | **A local OpenAI-compatible server** (a managed subprocess by default + an external mode); HTTP communication; **a client-side agentic loop**. **Server: llama.cpp `llama-server`** (`xinfer` was the original plan — rejected as too raw on Gemma 4; external works with any OpenAI server). | [3.2](#32-the-engine-as-a-local-server-not-an-embedded-sdk), [6.3](#63-client-side-agentic-loop), [install.md §3](docs/install.md) |
| 2 | UI | A TUI on `ratatui` 0.30 + `crossterm`; input/editing — a **custom widget** (ADR 0001); markdown — a **custom renderer** on `pulldown-cmark` (tables + theming, text only) + unicode-LaTeX (ADR 0003); scrolling — `tui-scrollview`. | [11](#11-tui-interface-and-interaction), [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md), [ADR 0003](docs/decisions/0003-own-markdown-renderer.md) |
| 3 | Architectural style | An **FSD-inspired** modular structure in a single binary crate; the backend core in `shared/api` and `shared/storage` behind traits; a split into workspace crates is possible later. | [4](#4-solution-architecture-fsd-inspired) |
| 4 | Embeddings (RAG) | A **dedicated** embedding server (a separate port), the `/v1/embeddings` endpoint; in managed mode — the same `llama-server` with `--embeddings`. No second ML stack in our own process. (ADR 0002.) | [6.1](#61-the-engine-layer-sharedapi), [9.3](#93-tool-roster) |
| 5 | The web tool | **Our own** implementation: DuckDuckGo (`reqwest`) + readable-content extraction. (The engine provides no built-in search.) | [9.3.1](#931-the-web-tool-our-own-implementation) |
| 6 | The Python tool | **Our own** subprocess executor; off by default on Windows (no OS sandbox). | [13.2](#132-python-execution) |
| 7 | EOS/stop | Stopping by token id (server-side); **no string stops for EOS text**. | [7](#7-eos-and-stop-token-handling) |
| 8 | `Chat`/tool ownership | One owner — the orchestrator; tools read a snapshot and return `ToolOutcome.effects` (no `ChatEffect` channel needed — a simplification, since the loop is client-side). | [4.4.2](#442-chat-ownership-and-tools-a-simplification-vs-attempt-1) |
| 9 | UI ↔ backend state | A unidirectional flow; the orchestrator is the source of truth and the sole writer; `generation_id` guards against stale events; a state machine. | [4.4](#44-execution-flows-and-ui--backend-state) |
| 10 | Vector storage | SQLite + sqlite-vec. | [5.2](#52-data-storage) |
| 11 | `max_tool_rounds` | **8** by default, configurable. | [6.3](#63-client-side-agentic-loop) |
| 12 | Data location | `data/` next to the binary by default (portable); `defaults.json` → an OS folder/directory + the profiles' scaffold language; a CLI backup/restore. | [5.2](#52-data-storage) |
| 13 | Soft delete | Mandatory (`is_hidden`, cascading). | [12.3](#123-backup-and-deletion) |
| 14 | Spellcheck | `spellbook` (en_US/en_GB/ru_RU) + our own TUI indication/popup; a personal dictionary. | [11.5](#115-input-and-editing-spellcheck) |
| 15 | LoRA | Deferred. | [1.5](#15-what-is-dropped--not-carried-over) |
| 16 | The self-model (SelfModel) | A per-profile "self-model" for the agent (a description/goals/model of the companion/narrative) in SQLite; optional tools (off by default), a compact injection into the system prompt, background auto-reflection, the `F3` edit screen. Mutators write **directly** to `Storage` (not through `ChatEffect`). No numeric "strengths"/"severity" — contradictions are recorded as prose in the narrative. | [17](#17-self-model-selfmodel), [docs/self-model-mvp.md](docs/history/self-model-mvp.md) |

### 16.2. Open questions (to be settled during implementation)

- ✅ **The engine protocol** — **closed**: the standard OpenAI-compatible one (`/v1/chat/completions` SSE, `/v1/embeddings`), which llama.cpp `llama-server` speaks. Sampling fields — a reduced set ([8.1](#81-mapping-onto-the-openai-compatible-api)); reasoning via `delta.reasoning_content` (`--reasoning-format`); tool calls are parsed by the server (`finish_reason="tool_calls"`); readiness — polling `/health`. Startup — [install.md §3](docs/install.md).
- ✅ **Embeddings** — **closed**: a **dedicated** embedding server on a separate port ([ADR 0002](docs/decisions/0002-embeddings-dedicated-server.md)); the dimensionality is fixed from the first response and stored in the sqlite-vec schema. If not configured, RAG returns a clear error.
- ✅ **Spellcheck rendering / choosing the input widget** — **closed** ([ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md)): `tui-textarea`/`ratatui-textarea` are incompatible with ratatui 0.30 → a custom input widget, we draw underlines ourselves (per-span `UNDERLINED`).
- ✅ **The markdown crate** — **closed** ([ADR 0003](docs/decisions/0003-own-markdown-renderer.md)): `ratatui-markdown`/`tui-markdown` don't give tables/math and ignore the theme → a custom renderer on `pulldown-cmark`.
- **`spellbook` quality on ru_RU** (complex affix rules) — verified in M3 (works).
- **Distributing the inference server** (`llama-server`): document installation/the binary path; decide whether to ship it alongside the application. M8.
- **The quality of "self-awareness"**: empirically test the `call_subagent` and `set_system_message` hypothesis on Gemma/Qwen. M6/M9. ✅ **Developed post-M9** — the self-model ([§17](#17-self-model-selfmodel)): verified on live Gemma 4 12B/31B (the model maintains its own view of itself/the user, cross-chat recall, background auto-reflection).

---

## 17. Self-model (SelfModel)

An extension of the "self-awareness" thread ([§1.2](#12-goals) item 6, `call_subagent`, introspection):
a per-profile **representation the agent holds of itself**, maintained by the model itself and
influencing its subsequent responses. Implemented post-M9 in stages (a probe → Tier 2/3);
the detailed plan and decisions — [docs/self-model-mvp.md](docs/history/self-model-mvp.md), the log —
[docs/journal/self-model.md](docs/journal/self-model.md). **Optional, and off by default.**

**Why this way:** small local models fill richly-typed
structures poorly (numeric "belief strengths", contradiction `severity`, contradiction
detect/resolve machinery all produce meaningless numbers and noise). So the model is deliberately
**minimal and human-readable**: free text + simple lists; contradictions
are recorded as prose in the narrative rather than as a separate type.

### 17.1. Composition and location

The `SelfModel` entity (`entities/self_model.rs`), one instance per profile:

- `summary` — free-form "about me" text (who I am, what I value, how I behave);
- `goals: Vec<Goal>` — goals (`id`, `description`, `status`: `Active`/`Completed`/
  `Abandoned`); only active ones are rendered/counted as "informative";
- `user_model` — a representation of the companion (`perceived_traits`,
  `current_interests` — lists; `relationship_dynamic` — text);
- `narrative: Vec<NarrativeSegment>` — short dated insights/observations (including
  noticed contradictions — as prose), append-only with a cap.

**Isolation by `profile_id`** — same as notes/RAG ([§9.5](#95-per-profile-isolation)).

### 17.2. Storage

A `self_models(profile_id PK, data JSON, version, updated_at)` table in SQLite
([§5.2](#52-data-storage)). Created with `CREATE TABLE IF NOT EXISTS` — no migration
needed, old DBs read fine. Storage maintains `version` (incremented on every upsert).

### 17.3. Tools

The SelfModel tool group (optional: **not** in `default_tool_ids`, present in
`all_tool_ids` → profile toggles, off by default):

| Tool | Purpose |
|---|---|
| `get_self_model` | Read the current state of the self-model |
| `reflect` | Return the current model + a self-reflection rubric (no write; an "entry point") |
| `update_self_model` | The self-description + goals (add/complete/abandon) |
| `update_user_model` | The companion's traits/interests/relationship dynamic |
| `add_insight` | Record an observation/insight (including a contradiction as prose) into the narrative |

**A departure from the tool contract ([§9.2](#92-the-tool-contract)):** SelfModel
is per-profile data in SQLite, so the mutator tools **write directly**
through `ctx.storage` (like `note_save`), rather than returning `ToolOutcome.effects`.
`ChatEffect` exists only for mutating `Chat`; `SelfModel` isn't `Chat`, so the
"sole owner of `Chat`" invariant ([§4.4.2](#442-chat-ownership-and-tools-a-simplification-vs-attempt-1))
isn't affected.

### 17.4. Injection into the system prompt

At the start of a turn, the orchestrator reads the profile's model snapshot and **compactly**
injects it into the `system` prompt (rendering active goals, the description, the model of the
companion, and a handful of recent insights). Gate: the profile enabled `get_self_model` (opt-in). This way
the model "remembers" itself and the user without an explicit tool call — this is the
main mechanism of value (verified through cross-chat recall).

**Every section has its own budget** — a share of `prompt_cap` (description 40%,
goals 20%, interlocutor 20%, observations 20%) plus whatever earlier sections did
not use. A share is a *ceiling*, so a bloated description or a long trait list
cannot starve what comes after it; unused room flows forward, so a short section
makes the next one richer. Lists lose **whole items** rather than being cut
mid-item and report how many were dropped ("… +N more"), because a trait cut in
half reads as a different trait and the count tells the model it is seeing a
part. The full picture is always one `get_self_model` away — that read
(`render_full`) is deliberately untruncated. Before this, only the description
was bounded and the rest was a queue: measured on a real profile, the description
and goals consumed the entire budget and **neither the interlocutor model nor the
observations were injected at all**, while the injected text told the model that
observations "surface by relevance". See
[docs/history/self-model-injection-budget.md](docs/history/self-model-injection-budget.md).

### 17.5. Parameters (settings)

`config.self_model: SelfModelSettings` (all under `#[serde(default)]` — no migration needed):

- `max_narrative` — the cap on stored insights (older ones are evicted);
- `narrative_in_prompt` — how many recent insights go into the prompt;
- `prompt_cap` — the character cap for the compact injection (protecting the context window),
  split between the sections as described in §17.4; default **4000** (raised from 1200, which
  measurement showed too tight for a mature self-model — an existing `settings.json` keeps its
  own stored value);
- `auto_reflect_every` — the auto-reflection period (0 — off).

`SelfModelParams::from_settings` sanitizes it (at least 1 insight; no more in the prompt
than stored; a readable minimum character count). Edited in the "Memory" section of the settings
screen ([§11.6](#116-the-settings-screen)).

### 17.6. Auto-reflection (background)

When `auto_reflect_every > 0`, every N assistant replies a background task
(`orchestrator/reflection.rs`) asks the model to review the recent conversation and **update its own**
"self-model" itself. Unlike auto-titling ([§11.2](#112-the-chat-list-an-overlay))
this is a **mini agentic loop**: the reflection is given the SelfModel tools, and the loop executes
their calls (writing to `Storage`); up to 6 rounds, with a timeout. The chat/feed **aren't mutated**,
nothing is streamed to the UI — the reflection is silent. Gates: the feature is enabled, the profile enabled
the tools, a reflection isn't already running (one at a time), the server is `Ready`, there's enough conversation.

### 17.7. UI — the self-model screen (`F3`)

A full-screen screen (`screens/self_model.rs`), opened from the chat with **`F3`**
or by typing **`/self`** (§11.7 — VS Code binds `F3` to its terminal's find-next) —
for viewing and **manual editing**:

- the self-description (a multiline editor), goals (add / rename / `Space`
  to change the status / `Del` to delete), the model of the companion (traits/interests as a comma-
  separated list, the relationship dynamic), deleting insights (`Del`), clearing the whole model
  (`Ctrl+K` twice — with confirmation; **`/self clear`** from the chat is the
  same wipe behind the same confirmation, §11.7);
- navigation `↑↓`/`Home`/`End`, `Enter` — edit, `Esc` — close, `Ctrl+Q`/`F10` — quit.

The list is drawn manually by visual rows (not with the `List` widget): a long
multiline value wraps by word, and an item that doesn't fully fit in the
remaining height is shown **partially** (its top part is visible) rather than skipped
(scrolling keeps the selected row visible, pinning a long item to the top).

The orchestrator owns the data: `F3` → `RequestSelfModel` → the reply `SelfModelView`
(a snapshot) opens the screen; edits → `SelfModelIntent::Edit` → `AppCommand::
UpdateSelfModel` → the orchestrator applies it (`apply_edit`), saves it, and **re-emits**
`SelfModelView` — the open screen updates in place (the selection is preserved). FSD
is respected: the screen doesn't know about `app`/`Storage`.

### 17.8. Status

Implemented and verified on live **Gemma 4 12B (Q8_0)** and **31B (QAT q4)**: the model
calls `update_*`/`add_insight` on its own, builds up a representation of itself and
the companion, recalls it in a new chat under the same profile (via the injection), while
background auto-reflection updates the model with no explicit request. Possible further directions —
ranking/configurability of the narrative and deeper metacognitive tools —
are deliberately deferred (see "out of MVP scope" in [docs/self-model-mvp.md](docs/history/self-model-mvp.md)).

---

*The document is the engineering specification for `mindfork-rs` (the source of truth for "what" and "why"). The engine — llama.cpp `llama-server` (the OpenAI protocol); the UI crates and the markdown renderer are detailed in ADR 0001/0003. The up-to-date implementation status is in [CLAUDE.md](CLAUDE.md).*
