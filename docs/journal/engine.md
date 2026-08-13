# Journal — Engine, generation and providers

Inference engine and the client-side agentic loop: the `EngineBackend` contract and its four implementations (llama.cpp/OpenAI-compatible — which also serves the xAI Grok cloud, OpenAI Responses, Anthropic, native Gemini), the managed `llama-server` launcher, readiness probing and health monitoring, sampling, streaming and token accounting, impersonation, history compaction.

**Reference documents for this area:** architecture.md §5–§6, spec.md §3, §6-§8

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (32)

- Post-M9: managed — preflight model-file check (done)
- Post-M9: `--no-mmap` flag + field hints in settings (done)
- Post-M9: managed embedding server — raised the physical batch (fixes "chunks: 0") (done)
- Post-M9: impersonation — writing a message on the user's behalf (`Ctrl+U`) (done)
- Post-M9: sampling "for variety" — dynatemp / adaptive-p / DRY breakers / sampler order (done)
- Post-M9: monitoring early `Child` exit during the readiness probe (done)
- Post-M9: multi-provider inference — Phase 0 (foundation) (done)
- Post-M9: multi-provider inference — Phase 2 (Anthropic / Claude) (done)
- Post-M9: nested engine config by mode/provider (done)
- Post-M9: get_sampling/set_sampling limited to fields available in the mode (done)
- Post-M9: FlashAttention + speculative decoding in managed mode (done)
- Post-M9: CoT (extended thinking) for Claude mode (done)
- Post-M9: message metadata — mode/model + sampling limited to available fields (done)
- Post-M9: engine restart debounce on settings edits (done)
- Post-M9: OpenAI no longer accepts `temperature`/`top_p` (done)
- Post-M9: OpenAI → Responses API ("thoughts", reasoning_effort, verbosity) (done)
- Post-M9: native Gemini client (generateContent) — Phase A (done)
- Post-M9: native Gemini client — Phase B (thought signatures at tool-use) (done)
- Post-M9: native Gemini — Phase C (partial: force-off for 2.5 Pro + surfacing blocks) (done)
- Post-M9: readiness probe for the embedding server (done)
- Post-M9: periodic server health monitoring (done)
- Post-M9: no server restart when nothing effectively changed (done)
- Post-M9: history compression — stage 1 (core, `/compact`) (done)
- Post-M9: history compression — stage 2 (the automatic trigger) (done)
- Post-M9: history compression — stage 3 (the read-back tools) (done)
- Post-M9: impersonation sends the compacted conversation (done)
- Post-M9: Grok (xAI) as a cloud provider (done)
- Post-M9: engine failures stop being silent (stage 1 of retry/backoff) (done)
- Post-M9: retry with backoff on transient cloud failures (stage 2, track complete) (done)
- Post-M9: images in a message — stage 1 (local + Grok) (done)
- Post-M9: images in a message — stage 2 (the three cloud formats, track complete) (done)
- Post-M9: images in a message — attach by URL (done)

### Post-M9: managed — preflight model-file check (done)
- **Symptom**: in managed mode, with a missing/inaccessible GGUF, the app would hang for
  a long time (up to the `MANAGED_READY_TIMEOUT=600s` timeout) in "server:
  connecting…" status. Cause: `ServerHandle::launch` (`shared/api/server.rs`) does a
  `spawn` of the child `llama-server`, and `spawn` **succeeds even without a model
  file** (it only fails if the binary itself is missing). `llama-server` then dies while
  loading and exits, but the background `wait_until_ready` probe doesn't notice and
  keeps polling the port pointlessly until the deadline.
- **Fix**: a preflight check in `ServerHandle::launch` — if `model_path` is set
  but `Path::is_file()` is false, immediately `bail!("model file not found or
  inaccessible: …")` **before** `spawn`. It fits neatly into the existing architecture:
  the supervisor (`app/supervisor.rs`) already maps a `launch` error → `ServerStatus::Disconnected(msg)`,
  and the status bar already shows it — no extra wiring needed. Covered by tests
  `launch_missing_model_file_errors_before_spawn` (no tokio runtime, checked before
  spawn) and `managed_with_missing_model_is_disconnected` (a real path → `Disconnected`).
- **Closed** (see below, "Post-M9: monitoring early `Child` exit in the probe"):
  a process that dies *while* loading (a corrupt GGUF, OOM) is now detected
  immediately by the probe, not by timeout.

### Post-M9: `--no-mmap` flag + field hints in settings (done)
- **A `--no-mmap` setting for the managed server**: a new `EngineSettings.no_mmap`
  field (`shared/config.rs`, `#[serde(default)]` → old `settings.json` loads without
  migration) → `ManagedConfig.no_mmap` (`shared/api/server.rs`); `build_args` adds
  `--no-mmap` only when it's on. Threaded through the supervisor (`app/supervisor.rs`,
  `managed_config`); a toggle in the "Model/server" section of the settings screen. Loads
  model weights entirely into RAM instead of mmap — helps on network/slow disks, needs
  more memory (off by default).
- **Field description hints in settings**: `screens/settings.rs::field_description(id)`
  returns a human-readable text for the field; `render_fields` shows it as a line
  at the bottom of the section (a top divider line, `dim`, word-wrapped) when a field is
  focused — the space is reserved **only when the field has a description** (other sections
  as before). Currently described: `-ngl` (GPU layers), `--jinja`, `--no-mmap`; extended by
  adding one branch in `field_description`.

### Post-M9: managed embedding server — raised the physical batch (fixes "chunks: 0") (done)
- **Symptom**: indexing a large file (e.g. `spec.md`) finished with "chunks:
  0, with errors: 1"; in the logs — `RAG: failed to index file … error=
  embeddings request returned an error status`.
- **Cause**: embedding models are **non-causal** — the whole input must fit into ONE
  physical batch (`n_ubatch`). By default `n_ubatch=512`, and llama-server
  equates `n_batch` to it (in the logs: *"setting n_batch = n_ubatch = 512"*).
  So **any chunk longer than ~512 tokens got the whole request rejected** ("input is too
  large to process. increase the physical batch size"). For Cyrillic/code, 512 tokens
  is only a few hundred characters, so `chunk_text`/`chunk_markdown` chunks (~800–1200
  chars) routinely didn't fit. The entire file's batch went as one request → one big error.
- **Fix**: the managed embedding server now launches with `-ub <ctx> -b <ctx>`
  (`server.rs::build_args` when `embeddings=true`, size = `context_size`, the embedder's
  `DEFAULT_CONTEXT_SIZE`=8192) — the physical and logical batch are raised to the
  context size, so an input chunk fits entirely in one ubatch. These flags aren't added
  for the chat server. Memory on embedding models (bge-m3 ~1.3 GB) grows negligibly.
  **Requires a managed-server restart** (happens on startup/when embedder settings change).
- **Diagnostics**: `OpenAiClient::embed` no longer **swallows the error body** (like
  `chat_stream` before it) — the status + JSON reason are logged and included in the
  error text (truncated to 500 chars). It was exactly this (and the llama-server log)
  that pointed to the root cause.
- **External embedder** (`MINDFORK_EMBED_URL`): we don't control the batch flags — if a
  third-party server is running with a small `n_ubatch`, a large chunk will still be
  rejected; now it's visible from the error text (rather than "chunks: 0"). Chunks are
  bounded by `CHUNK_MAX_CHARS` (≤ ~1200 tokens worst case), so they never exceed the
  embedder's context (8192).

### Post-M9: impersonation — writing a message on the user's behalf (`Ctrl+U`) (done)
- **Overview** (spec §11.8): on `Ctrl+U` the model writes the next message **on behalf
  of the user** into the input box. The request is built as follows: the chat's system
  message (the assistant's persona) is replaced with the **impersonation** one from the
  profile (`Profile.impersonation_system_message`; empty → `DEFAULT_IMPERSONATION_SYSTEM_
  MESSAGE`), the user↔assistant roles in history are swapped (`swap_role_message`;
  system/tool/empty messages are dropped — there are no tools in this mode), sampling is
  `AppConfig.impersonation_sampling`. Pure `build_impersonation_request`/
  `swap_role_message` functions in `app/orchestrator/impersonation.rs`.
- **Reasoning is forcibly disabled** (important): `build_impersonation_request`
  forces `thinking=false`, `reasoning_effort=None`, and critically `reasoning_budget=0`
  on top of the user's `impersonation_sampling`. Impersonation discards "thoughts"
  (`Thoughts` are ignored in `spawn_impersonation`), so it doesn't need reasoning.
  For models with thinking "baked into" the template (Gemma `peg-gemma4`, Qwen), the
  server ignores the `thinking`/`reasoning_effort` fields — only `reasoning_budget=0`
  (+ `chat_template_kwargs.enable_thinking=false` in `wire.rs`) actually suppresses
  "thoughts". Without this the model would spend its whole token budget on
  `reasoning_content`, and the reply's `content` would come back empty → the preview
  stayed empty (the same bug class as auto-title, see `title.rs`).
- **Three server modes** (`ImpersonationMode` in `shared/config.rs`):
  `shared` (default) — the same chat server as the assistant's, but with the
  impersonation sampling (no separate server is raised); `managed` — a separate child
  `llama-server`; `external` — a separate remote server. Settings —
  `AppConfig.impersonation_engine: ImpersonationEngineSettings` (fields like
  `EngineSettings`, but a three-way mode). Supervisor: `ServerSupervisor::
  apply_impersonation` (the real one raises managed/external + probes; not
  called for `shared` — the orchestrator uses the assistant's `backend`). The
  orchestrator holds `imp_backend`/`imp_handle`/`imp_status` and re-raises it on a
  change to `impersonation_engine` (`apply_impersonation_settings`).
- **Flow** (a background task, like auto-title): `AppCommand::Impersonate { seed }`
  → `handle_impersonate` (gated by `State::Idle` + no active impersonation + server
  readiness via `impersonation_backend_if_ready`) → `spawn_impersonation` streams
  `AppEvent::ImpersonationChunk`, and on end/timeout/cancel sends `(id, reason)` into
  an internal `imp_done` channel → `handle_imp_done` emits `ImpersonationFinished`.
  `AppCommand::CancelImpersonation` (`Esc`) cancels the token; so does `Quit`.
  `IMPERSONATION_TIMEOUT=600s` (a safety net against a stuck task; generous — on a slow
  local `llama-server` just prompt processing can take ~a minute, CPU generation is
  ~5 tok/s). "Thoughts"/tool calls in the input aren't shown.
- **Timeout ≠ user cancellation** (important, fixes "the reply cuts off and
  disappears"): a cancel via `Esc` arrives as `Ok(Ok(Cancelled))` (the stream inside
  `run` itself catches `cancel.cancelled()` and returns `Finished(Cancelled)`) and
  discards the text; a timeout is the `Err(_)` branch: we abort the server task
  (`cancel.cancel()`), but **keep** the accumulated text, returning it as `Length`. The
  old unconditional `if cancel.is_cancelled() { Cancelled }` collapsed both cases into
  discarding — which is why a timeout-truncated reply used to vanish.
- **UI** (`screens/chat.rs` + `widgets/impersonation_preview.rs`): while it's being
  written, the input box is **hidden**, replaced with a non-editable **streaming
  preview** with a spinner (`ImpersonationState`: text starts with whatever seed text
  was already typed). The accumulated text is inserted into the input box (`set_text`)
  on any completion **except an explicit cancel** (`Cancelled` — `Esc`): then the field
  keeps just the seed. A reply truncated by timeout (`Length`) or interrupted by a
  stream error (`Error`) **isn't lost** — partial text is more useful than an empty
  field. During impersonation only `Esc` (cancel) and `Ctrl+C` (quit) are handled. The
  `app/runtime.rs` loop repaints every tick while `is_impersonating()` (spinner
  animation). `Ctrl+U` is layout-independent (`shared/keys`), added to the help
  overlay (`F1`/`?`).
- **Continuing something already started**: if the input box isn't empty, the text
  goes in as `seed` — `build_impersonation_request` adds a request to the system
  message to continue what's started (output only the continuation), and the preview
  shows the seed + what's generated (final result = seed + continuation).
- **Settings** (`screens/settings.rs`): the "Model/server", "Sampling", and
  "Profiles" sections got a **subsection selector** for "Assistant"/"Impersonation"
  (`Subsection`, fields `model_sub`/`sampling_sub`/`profile_sub`; a new `FieldId::*Sub`).
  The Model impersonation subsection — the same fields + a three-way mode
  (`IxMode`, `cycle_imp_mode`); Sampling — the same fields over
  `impersonation_sampling`; Profile — **only the system message**
  (`PImpSystem`, a multiline editor), no tools. `ProfileEdit.
  impersonation_system_message` carries the edit.

### Post-M9: sampling "for variety" — dynatemp / adaptive-p / DRY breakers / sampler order (done)
- **Four new `SamplingConfig` fields** (llama.cpp request-body extensions, for
  livelier and less predictable replies): **dynamic temperature**
  `dynatemp_range`/`dynatemp_exponent` (temperature adapts to the distribution's entropy
  at each token), **adaptive-p** `adaptive_target`/`adaptive_decay`
  (a new sampler, llama.cpp PR #17927), **DRY breakers** `dry_sequence_breakers`
  (`Option<Vec<String>>`), and a configurable **sampler order** `samplers`
  (`Option<Vec<String>>`). Every field is `Option`, sent only when set.
- **List fields are JSON arrays, never sent empty** (`wire.rs::build_chat_request`,
  the `non_empty` helper): an empty `samplers` would be interpreted by the server as
  "disable all samplers", and an empty `dry_sequence_breakers` is **flatly rejected by
  llama-server** (`server-schema.cpp:238` throws `"must be a non-empty array of strings"`)
  — the filter prevents this.
- **Keys were verified against the llama.cpp source** (`tools/server/server-schema.cpp`,
  where body parsing moved to in recent versions from `server.cpp`): `dynatemp_range`/`_exponent`
  (:113), `adaptive_target` (:160, a flat key, `≤1.0`, negative=off),
  `adaptive_decay` (:164, hard range `0.0–0.99`), `dry_sequence_breakers` (:234),
  `samplers` (:474, an array of names or a string). Valid sampler names
  (`sampling.cpp`): `penalties`, `dry`, `top_k`, `top_p`, `top_n_sigma`, `typ_p`,
  `min_p`, `xtc`, `temperature`, `infill` (mirostat is a separate mode, not in the list).
- **UI** (`screens/settings.rs`, "Sampling" section): number fields as usual;
  `samplers` is edited as text via `;`, `dry_sequence_breakers` — via `,` with
  `\n`/`\t`/`\r` escapes (a single-line editor can't take them literally;
  `parse_breakers`/`join_breakers`/`decode_escapes`/`encode_escapes`). Also editable
  via the `set_sampling` tool (a schema + merging all the fields).
- **Tests**: wire serialization of the new fields + omitting empty arrays
  (`list_fields_sent_as_arrays_and_empty_omitted`), round-trip of lists and escapes
  (`samplers_list_round_trip`, `dry_breakers_decode_and_encode_escapes`), merging in
  `set_sampling`. **A live `#[ignore]` smoke** `accepts_creative_sampling_extensions`
  (`client.rs`) — all four fields in one request; checks the **combined** stream of
  `text`+`thoughts` (Gemma, a reasoning model, puts the answer in `reasoning_content` —
  the reasoning-budget trap). Verified on a live `llama-server` (Gemma 4 12B).

### Post-M9: monitoring early `Child` exit during the readiness probe (done)
- **Symptom**: on a corrupt GGUF / out of memory, the managed `llama-server` binds the port
  but **dies during model loading** (the preflight file check
  `launch_missing_model_file_errors_before_spawn` only catches a missing file, not
  "valid path but the process crashed"). The background probe (`wait_until_ready`) only held
  `OpenAiClient` and kept polling a dead port **until the timeout**
  `MANAGED_READY_TIMEOUT=600s`, leaving the UI at "server: connecting…".
- **Root-cause constraint**: `Child` was owned by `ServerHandle` (for `kill_on_drop`), but
  detecting exit is only possible via `child.wait()` — that needs `&mut Child` + ownership, which
  conflicts with the orchestrator holding the handle for kill-on-drop.
- **Fix** (`shared/api/server.rs`): `Child` moves into a **monitor task**
  (`spawn_monitor`), which `select!`s between `child.wait()` (process died → sets
  `exited: CancellationToken`) and `kill: CancellationToken` (set in `Drop for
  ServerHandle` → `start_kill` + `wait`). `ServerHandle` holds both tokens and hands
  `exited()` to the probe. `wait_until_ready(client, timeout, exited: Option<Cancellation
  Token>)`, after a failed probe, checks `exited.is_cancelled()` → an early `bail!`
  with a clear message ("…exited before becoming ready (corrupt GGUF or out of memory?)"),
  and during the pause between probes it `select!`s with the sleep, to wake instantly on process
  death. `kill_on_drop(true)` is kept as a fallback (the monitor owns `Child`).
- **Plumbing**: `app/supervisor.rs::spawn_probe` takes `Option<CancellationToken>`;
  managed modes (chat + impersonation) pass `Some(handle.exited())`, external —
  `None` (no process). Error → `ServerStatus::Disconnected` mapping already existed
  (the status bar shows the reason). **435 tests green** (+2:
  `wait_until_ready_bails_on_early_exit`, `monitor_cancels_exited_when_child_dies` —
  a real short-lived process, cross-platform), clippy/fmt clean.

### Post-M9: multi-provider inference — Phase 0 (foundation) (done)
- **Cloud providers through the same `EngineBackend` trait** ([ADR 0004](../../docs/decisions/0004-engine-contract-multi-provider.md)):
  Phase 0 unlocks **OpenAI** (`platform.openai.com`) and **Gemini** (via an
  OpenAI-compatible endpoint). Claude (a separate `/v1/messages` protocol) — Phase 2.
  Layers above the engine (orchestrator, agentic loop, tools, UI) are **untouched**.
- **A flat mode taxonomy** (`shared/config.rs`): `ServerMode` extended with
  `OpenAi`/`Gemini` (alongside `Managed`/`External`), `ImpersonationMode` — with them too
  (plus `Shared`). A new `CloudProvider { OpenAi, Gemini }` with `base_url()`;
  `ServerMode::cloud_provider()`/`ImpersonationMode::cloud_provider()` → `Option`.
  `EngineSettings`/`ImpersonationEngineSettings`/`EmbedSettings` got
  `model_name`/`api_key_env` (all `#[serde(default)]` → old `settings.json` without
  migration).
- **The model is a property of the backend, not the request** (minimal change): `ChatRequest`
  **doesn't carry** a model — it's injected into the body (`model`) by `OpenAiClient`. The client got
  `api_key`/`model`/`dialect` + builders `with_api_key`/`with_model`/`with_dialect`,
  sends `Authorization: Bearer` (method `auth`), and for embeddings sets `model` in
  `EmbeddingRequest`. `new(url)` = local/external (no key, lenient dialect).
- **Request-body dialect** (`shared/api/wire.rs`, `WireDialect { LlamaCpp, OpenAi,
  Gemini }`): `build_chat_request(req, stream, model, dialect)`. Strict cloud
  dialects (`OpenAi`/`Gemini`, `is_strict`) **strip** llama.cpp extensions and
  reasoning signals (`top_k`/`min_p`/`dynatemp_*`/`dry_*`/`mirostat*`/`samplers`/
  `thinking`/`reasoning_*`/`chat_template_kwargs`) — otherwise the cloud would return `400`
  even for the default `thinking:true` (`restrict_to_strict`). What remains: `temperature`/
  `top_p`/`frequency_penalty`/`presence_penalty`/`seed` + `tools`/`stream*`. **The token
  limit field differs:** OpenAI requires `max_completion_tokens` (newer models
  reject `max_tokens` with `400` — a real error hit during testing), Gemini-compat uses
  the classic `max_tokens`; `LlamaCpp` sends everything as before. Provider→dialect —
  `dialect_for` in the supervisor. `WireDialect` exported.
- **Supervisor** (`app/supervisor.rs`): `apply_chat`/`apply_impersonation`/`apply_embed`
  got cloud branches (`cloud_chat_setup`/`cloud_embed_setup`). The key is resolved from
  an **env variable, by name** (`resolve_api_key`; the secret isn't written to disk, ADR 0004);
  the base URL comes from the provider (override via `url`); dialect `OpenAi`. The cloud doesn't "load
  a model" → the status is immediately `Ready` (no probe). No model/key → `Disconnected` with
  a clear message (chat) / `UnavailableEmbedder` (RAG).
- **Settings UI — field visibility by mode** (`screens/settings.rs`): `model_fields`/
  the embed part of `tool_fields` show **only mode-relevant** fields (managed →
  llama-server parameters; external → URL+model (opt.); openai/gemini → model+API key
  (env)+base URL (opt.)) — this de-clutters the screen. The `ServerMode` cycle (`cycle_mode`,
  now directional) walks all 4 variants; `cycle_imp_mode` — 5. New
  `FieldId`s (`X/Ix/E` × `ModelName`/`ApiKeyEnv`) with `apply_text` and tooltips
  (`field_description`: modes, API-key-env, model name). The API key in the UI is the
  env-variable **name**, not a secret.
- **Marking unsupported sampling in the cloud** (`screens/settings.rs`): in the "Sampling"
  section, when the cloud provider is the subsection, parameters outside the strict
  dialect (llama.cpp extensions + `thinking`/`reasoning_effort`) are marked via the value's
  **color** — a set value is colored `palette.warning` (amber/brown —
  attention, but not alarm), visible without focus (`render_field_line(..., inactive)`).
  Only **set** values are flagged (`set`): unset ones (`—`) have nothing to flag.
  When focused, a detailed tooltip below reads `CLOUD_UNSUPPORTED_NOTE`
  ("…the value will be kept for local models"). **The value itself is untouched** (stored in the
  shared `default_sampling`/`impersonation_sampling`, will work with a local model).
  The supported subset is `cloud_supported_param`
  (temperature/top_p/frequency_penalty/presence_penalty/seed/max_tokens). The subsection's
  provider — `sampling_is_cloud` (for impersonation in `shared` mode, the assistant's engine is
  used). Fields aren't hidden (values survive switching back to a local model).
  Evolution: a focus-only tooltip → not noticeable → a text note → replaced on request
  with value coloring (more visible, no repeated text).
- **Tests**: config (cloud modes serialize/map to a provider, roundtrip,
  old JSON → defaults); wire (the OpenAi dialect strips extensions and sends `model`;
  LlamaCpp omits `model=None`); supervisor (`resolve_api_key` env/missing; cloud
  without model/key → `Disconnected`; with model+key → `Ready`+backend; cloud-embed
  without config → unavailable); settings (cloud mode shows model/api-key and
  hides llama-server fields). **543 tests green**, clippy/fmt clean.
- **Not included in Phase 0** (per the ADR 0004 plan): splitting `shared/api` into
  submodules (`contract/openai/anthropic/managed`) — deferred to Phase 2, when
  `AnthropicClient` shows up (until then the `anthropic/` submodule would be empty, and splitting for one
  OpenAI family alone would be premature). Live smokes against real OpenAI/Gemini — by
  key, outside CI.

### Post-M9: multi-provider inference — Phase 2 (Anthropic / Claude) (done)
- **Claude through its own Messages API protocol** ([ADR 0004](../../docs/decisions/0004-engine-contract-multi-provider.md)),
  as a new `EngineBackend` trait implementation — layers above the engine untouched.
- **New module `shared/api/anthropic/`** (`client.rs` + `wire.rs`): `AnthropicClient`
  hits `/v1/messages` (headers `x-api-key` + `anthropic-version`), parses
  event-based SSE (`message_start`→usage.input_tokens; `content_block_start` tool_use→
  `ToolCall`; `content_block_delta`: `text_delta`→`Text`, `thinking_delta`→`Thoughts`,
  `input_json_delta`→`ToolCall` args; `message_delta`→`Usage`+`Finished` by
  `stop_reason`). `wire::build_request` translates the domain `ChatRequest`: system →
  a **top-level** `system`; only user/assistant roles; tool results → `tool_result`
  blocks **inside a user turn** (there's no `tool` role); tool calls → `tool_use`
  blocks; adjacent messages of the same role **are merged** (Anthropic requires
  strict alternation); `max_tokens` is required (default 4096); a tool's schema is
  `input_schema`. **Sampling: only `max_tokens`** — the newest Claude 4.x models
  (opus-4-8/haiku-4-5) have "locked in" sampling and reject `temperature`/`top_p`/
  `top_k` as deprecated (HTTP 400 during a live test run), so they aren't sent at all (the UI
  marks them as unsupported for Claude via `cloud_supported_param`). The error body
  isn't swallowed (as with `OpenAiClient`).
  Anthropic has no embeddings — `Embedder` isn't implemented (RAG uses a separate one, ADR 0002).
- **Config**: `ServerMode`/`ImpersonationMode` += `Claude`; `CloudProvider` += `Claude`
  (`base_url`=`https://api.anthropic.com`, the client itself appends `/v1/messages`).
- **Supervisor**: `cloud_chat_setup` branches by the provider's protocol — OpenAI/Gemini
  → `OpenAiClient` (+dialect), Claude → `AnthropicClient`. Embeddings in Claude mode →
  `UnavailableEmbedder` (Anthropic doesn't do embeddings) with a clear log message.
- **Settings**: `Claude` in the mode cycles (5 values for the engine, 6 for
  impersonation), the same cloud fields (model/API-key-env/base URL). **Sampling
  marking is provider-dependent**: `cloud_supported_param(provider, p)` — for Claude
  **`top_k` is supported** (unlike OpenAI/Gemini), while penalties/seed aren't;
  `sampling_cloud_provider` returns the subsection's provider (shared impersonation inherits the
  assistant's).
- **Claude only appeared in the selector now** (with a real client) — in Phase 0 it was
  deliberately absent, so as not to ship a nonfunctional entry.
- **Tests**: wire translation (system top-level, tool_use/tool_result+merging, input_schema,
  the max_tokens default, parsing all SSE events), stop_reason mapping, supervisor (Claude
  chat → Ready+backend; Claude embed → unavailable), settings (Claude marks penalties/
  seed, but not top_k). **555 tests green** (+a live `#[ignore]` smoke
  `MINDFORK_ANTHROPIC_KEY`), clippy/fmt clean.
- **Splitting `shared/api` into submodules (ADR 0004 §2) — done** (as a separate step
  after Anthropic, for symmetry): `backend.rs`→`contract.rs`, `client.rs`+`wire.rs`→
  `openai/` (a private `wire`, re-exporting `OpenAiClient`/`WireDialect`), `server.rs`→
  `managed.rs`, `anthropic/` already existed. Moves via `git mv` (history preserved); the public
  surface is the same (re-exported from `mod.rs`); external `shared::api::backend::*` →
  `::contract::*`. Final structure: `contract` (traits/types), `openai`/`anthropic`/
  `managed` (implementations/launch), `thoughts`, `mock`. **555 tests**, clippy/fmt clean.

### Post-M9: nested engine config by mode/provider (done)
- **The engine config became nested** (`shared/config.rs`): previously `EngineSettings`/
  `ImpersonationEngineSettings`/`EmbedSettings` shared a **flat** field set
  (`model_name`/`api_key_env`/`url`/`binary`/…), common to all modes — switching
  the provider overwrote someone else's values. Now every mode/provider has its **own
  subsection**: reusable `ManagedSettings` (llama-server: binary/model/ngl/ctx/
  jinja/reasoning_format/no_mmap/host/port), `ExternalSettings` (url + opt. model_name),
  `CloudSettings` (model_name/api_key_env/url-override) — one instance per
  `openai`/`gemini`/`claude`. Embeddings have their own `ManagedEmbedSettings` (fewer
  fields — host/jinja/no_mmap/ctx are fixed by the supervisor). You can keep
  managed + OpenAI + Gemini + Claude all configured at once and switch quickly between them.
- **Mode-aware accessors** `EngineSettings::cloud()`/`cloud_mut()` (and for impersonation/
  embed) return the **active** cloud substructure per `mode` (via
  `cloud_provider()`); shared helpers `cloud_ref`/`cloud_mut` in config.rs. This preserved the
  existing settings `FieldId`s — reading/writing routes by the current mode
  (only one group is visible): `XUrl`/`XModelName` write to `external.*` in external mode and
  `cloud_mut().*` in the cloud; managed fields → `managed.*`.
- **No migration** (user's decision): an old flat `settings.json` is read via
  `#[serde(default)]` — unrecognized flat engine fields are silently ignored, subsections
  come out default. The managed binary/model/key need to be entered again once. Chats/profiles
  aren't affected. `schema_version` stayed at 1.
- **Settings UI** (`screens/settings.rs`): `model_fields`/the embed part of `tool_fields`
  build fields from the substructures (helpers `managed_rows`/`cloud_rows`); mode-driven
  visibility (as before). **Cloud-unsupported sampling parameters are now
  hidden** (filtered by `SAMPLING_PARAMS` via `cloud_supported_param`), rather than colored
  yellow — the warning path was removed (`render_field_line(inactive)`, `CLOUD_UNSUPPORTED_NOTE`,
  `sampling_unsupported_in_cloud`); hidden-parameter values are preserved (they'll work
  with a local model). Impersonation in `shared` mode inherits the assistant's provider filter.
- **Chat-header cosmetics** (`screens/chat.rs::model_meta`): now driven by `engine.mode` —
  managed shows `name.gguf · Nk ctx`, external — `external.model_name`, cloud — the
  active cloud model's name (no ctx). Previously in cloud modes the local
  GGUF's name and its context were shown stuck there.
- Other consumers updated: `supervisor.rs` (building clients/`ManagedConfig` from
  substructures; a shared `managed_config(&ManagedSettings)`, helpers
  `external_chat_setup`/`managed_chat_setup`), `main.rs::apply_env_overrides`
  (`config.engine.managed.*`/`external.url`). **556 tests green**, clippy/fmt clean.

### Post-M9: get_sampling/set_sampling limited to fields available in the mode (done)
- **The `get_sampling`/`set_sampling` tools now show/change only the sampling
  fields that the current mode's engine actually accepts** — previously the `set_sampling`
  schema always carried the full set (including llama.cpp extensions), and in the cloud
  the model would try to change `top_k`/`min_p`/… which the provider rejects (`400`). Now
  the model sees exactly what's applicable.
- **A single source of truth** — `entities/sampling.rs::supported_sampling_fields(provider:
  Option<CloudProvider>) -> &[&str]` (mirrors the wire dialect's `restrict_to_strict` /
  `anthropic::wire`): `None` (local/external llama.cpp) — the whole
  `SETTABLE_SAMPLING_FIELDS` set; OpenAI/Gemini — `temperature`/`top_p`/`frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens`; Claude — only `max_tokens`. The settings
  UI now feeds off it too: `screens/settings.rs::cloud_supported_param` delegates there
  (via a new `SamplingParam::field_name()`), removing the duplication of "what's supported."
- **Tools are mode-aware via `provider`** (`features/tools/introspection.rs`):
  `GetSampling::new(provider)`/`SetSampling::new(provider)` store the chat engine's provider.
  `SetSampling::parameters` builds a JSON schema only from the available fields (`field_schemas`
  — name→schema, covers the whole `SETTABLE_SAMPLING_FIELDS`); `invoke` **drops**
  unavailable keys from the arguments and tells the model about it (rather than silently applying);
  `GetSampling::invoke` filters output by the same set; **the `set_sampling` result
  text is also filtered** (`filter_to_supported`) — otherwise the feed and the model
  would get the full config with dozens of `null` fields (confusing for both the model and
  the user); tool descriptions carry the list of available fields (`scope_note`).
  The provider is taken from `config.engine.mode.
  cloud_provider()` in `build_registry` (a `ToolConfig.sampling_provider` field); the registry
  **is also rebuilt on an engine mode change** (`orchestrator/settings.rs`), not only on a
  `config.tools` change.
- **If the mode has no supported parameters at all — the tools are unavailable to the model**:
  `effective_tool_ids` got a 5th argument `sampling_provider` and filters out
  `get_sampling`/`set_sampling` when `supported_sampling_fields` is empty (a safety
  net — every current provider has at least `max_tokens`). Constants
  `GET_SAMPLING_ID`/`SET_SAMPLING_ID` (re-exported from `introspection`).
- **Tests**: `supported_sampling_fields` (mirrors the dialect/subsets); tools
  (the cloud hides `top_k` in `get_sampling`'s output; Claude drops `temperature`/`top_k`
  from `set_sampling` and notifies about it; the `set_sampling` schema per mode; `field_schemas` covers
  the whole set); `effective_tool_ids` keeps the tools when parameters exist; the settings UI filter
  (the previous `cloud_hides_unsupported_sampling_params` — now green on the delegation);
  the `set_sampling` result in the cloud doesn't carry a full config dump.
  **567 tests green**, clippy/fmt clean.

### Post-M9: FlashAttention + speculative decoding in managed mode (done)
- **The managed `llama-server` got FlashAttention (`--flash-attn`) and speculative
  decoding (`--spec-*`)** — the latter lets MTP models be attached (e.g.
  `mtp-gemma-4-12B-it.gguf`) via `--spec-type draft-mtp`. Only for local managed
  (the assistant's chat server and the impersonation server); external/cloud are unaffected,
  and it's inapplicable to the embedding server (it doesn't generate tokens).
- **Config** (`shared/config.rs`): two enums — `FlashAttn { Auto, On, Off }`
  (`Auto` = the flag isn't passed, llama.cpp's default) and `SpecType` (`None`/`draft-simple`/
  `draft-eagle3`/`draft-mtp`/`ngram-simple`/`ngram-map-k`/`ngram-map-k4v`/`ngram-mod`/
  `ngram-cache`; serde kebab-case matches the CLI value; `as_arg()`/`label()`/
  `cycle(dir)`/`needs_draft_model()`). `ManagedSettings` extended with `flash_attn`,
  `spec_type` and draft fields `draft_model` (`-md`), `draft_gpu_layers` (`-ngld`),
  `draft_n_max` (`--spec-draft-n-max`), `draft_n_min` (`--spec-draft-n-min`). All via
  `#[serde(default)]` — old `settings.json` without migration.
- **Argument construction** (`shared/api/managed.rs`): `ManagedConfig` carries primitives
  (`flash_attn: Option<String>`, `spec_type: Option<String>`, `draft_*`) — like
  `reasoning_format`; the supervisor converts the enums to strings (`managed_config`). `build_args`
  adds `--flash-attn`/`--spec-type`/`-md`/`-ngld`/`--spec-draft-n-max`/`-n-min`
  **only when set** (unset → llama.cpp's default). The preflight file check
  extended to the draft model (`-md`): a nonexistent path → a clear error before `spawn`
  (otherwise `llama-server` silently fails while loading it, and the probe waits until the timeout).
- **Settings UI** (`screens/settings.rs`, "Model/server" section, both
  Assistant/Impersonation subsections): FlashAttention and `--spec-type` — Choice fields (←/→ cycle);
  draft fields (`-md`/`-ngld`/n-max/n-min) shown **only for draft-* types**
  (`needs_draft_model`), so as not to clutter the section for ngram/none. All fields have
  description tooltips (`field_description`). `managed_rows` was refactored from a long
  list of FieldId arguments into a `ManagedFieldIds` struct (+ two const instances
  `ASSISTANT_MANAGED_IDS`/`IMP_MANAGED_IDS`). Optional numeric fields are parsed via
  `parse_opt_num` (empty → `None`/clear, non-numeric → keep the previous).
- **Tests**: config (as_arg/serde/cycle/needs_draft_model, roundtrip of the managed section with
  spec fields); managed (the flash-attn/spec flags in `build_args`, defaulting without them,
  the preflight error on a missing `-md`); settings (flash/spec visibility in managed,
  draft fields revealing when cycling to draft-mtp, cycling flash-attn while persisting).
  **579 tests green**, clippy/fmt clean. A live run against a real
  `mtp-gemma-4-12B-it.gguf` — a manual check (needs a GPU/model).

### Post-M9: CoT (extended thinking) for Claude mode (done)
- **Claude (Anthropic) can now do "thoughts" (CoT)** — previously `anthropic/wire::build_request`
  only sent `max_tokens`, so reasoning was never requested. The receiving side was already
  ready (the client maps `thinking_delta`→`ChatChunk::Thoughts`, the feed renders the "thoughts"
  block). Two phases: **A** — CoT in chat without tools; **B** — CoT with tool use
  (critical — tools are central to the application). See CLAUDE.md (the engine section).
- **Phase A — enabling thinking** (`anthropic/wire.rs`): the request carries
  `thinking:{type:"adaptive", display:"summarized"}` when `sampling.thinking==Some(true)`
  (+ `output_config.effort` from `reasoning_effort`, except `none`). **`budget_tokens`/
  `reasoning_budget` are not sent** — Claude 4.x models reject them (`400`); only adaptive,
  depth is set via `effort`. `display:"summarized"` is needed so the "thoughts" text arrives
  non-empty (default `omitted`). `supported_sampling_fields(Claude)` extended with
  `thinking`/`reasoning_effort` (mirroring the dialect) → the settings screen and `get/set_sampling`
  now show/edit them for Claude; `top_k`/penalties remain hidden.
- **Phase B — thinking-block signature with tool use** (the main protocol nuance): Anthropic
  requires an assistant turn with `tool_use` to carry **its own thinking block with a `signature`**
  within the same turn — otherwise the next request in the round gets `400`. Implementation:
  - Parse `signature_delta` (`anthropic/wire::AntDelta::SignatureDelta`) → a new chunk
    `ChatChunk::ThoughtsSignature(String)` (the client emits it; other backends don't).
  - `RoundOutput.thoughts_signature` accumulates the round's signature; the agentic loop
    (`orchestrator/generation.rs`), when a signature is present, attaches a thinking block to
    `ApiMessage::assistant_tool_calls(...).with_thinking(...)` at the point it's pushed into the request
    history. The type `ApiMessage.thinking: Option<ThinkingBlock>` (text+signature) is generic,
    used only by the Anthropic wire; `anthropic/wire::build_messages` puts
    `AntBlock::Thinking` **first** in the assistant turn.
  - **The signature lives only in the memory of one `spawn_generation` call** — it is NOT persisted in
    the domain `Message`/JSON: Anthropic only requires thinking on the most recent
    assistant turn, and between turns (reload/new request/regeneration) old
    thinking blocks are auto-discarded by the server. `message_to_api` (history) doesn't carry thinking —
    which is correct for Anthropic. Unfinished tool turns from history aren't rebuilt
    (regeneration truncates up to the last user message), so persistence isn't needed.
- **A generic contract, backends don't break**: the new `ChatChunk` variant and the
  `ApiMessage.thinking` field are ignored by llama.cpp/OpenAI; the llama.cpp thinking path is unchanged
  (`reasoning_budget`/`<think>`). Exhaustive `ChatChunk` matches were updated across all
  consumers (title/impersonation/reflection/subagent/fetch/openai-client/generation).
- **Tests**: wire (thinking off by default; adaptive+summarized+effort when enabled;
  `budget_tokens` isn't sent; the thinking block comes before tool_use; no signature → no block;
  parsing `signature_delta`); `supported_sampling_fields`/the `set_sampling` schema updated.
  **622 tests green**, clippy/fmt clean. Live `#[ignore]` smokes
  (`anthropic/client.rs`, `MINDFORK_ANTHROPIC_KEY`) **verified against the real Anthropic
  API**: `extended_thinking_streams_thoughts_and_signature` (Phase A — "thoughts"
  and a signature arrive) and `thinking_with_tool_use_round_trips_signature` (Phase B — the second round with
  the signature resent goes through without `400`). A Phase B smoke nuance: guaranteeing a
  thinking block requires a prompt with an explicit reasoning step + `reasoning_effort: High` —
  on a trivial request, adaptive thinking skips the reasoning step (no signature, which is
  correct in itself — no block is needed without "thoughts").
- **Known limitation**: `redacted_thinking` blocks (a rare protective classifier
  response) aren't handled yet — if such a block arrives before tool use, it
  won't be resent (a possible `400`). Very rare in normal use; groundwork noted.

### Post-M9: message metadata — mode/model + sampling limited to available fields (done)
- **The `Message.metadata` snapshot now carries the engine mode and model
  name, and sampling is pared down to the fields available in that mode.**
  Previously `finalize_message` (`app/orchestrator/generation.rs`) wrote
  `MessageMetadata { sampling: the full effective_sampling, model: None }` —
  the mode wasn't recorded, the model was always `None`, and the "what was
  applied" snapshot included llama.cpp extensions that a strict cloud dialect
  (OpenAI/Gemini/Claude) wouldn't even have accepted.
- **`MessageMetadata`** (`entities/message.rs`) gained a `mode: ServerMode`
  field (`#[serde(default)]` → old messages are read as `Managed`; `model:
  Option<String>` already existed). `sampling` is now filtered.
- **`SamplingConfig::retain_supported(provider)`** (`entities/sampling.rs`): a
  copy of the config with fields cleared to `None` when they're unavailable
  in the provider's mode — a **mirror of the wire dialect**, via the existing
  `supported_sampling_fields` (the same source of truth used by the settings
  UI and `get/set_sampling`; a serialization round-trip that retains keys,
  like `filter_to_supported` in introspection). Local mode (`None`) keeps the
  full configurable set; cloud gets strict subsets.
- **`EngineSettings::active_model_name()`** (`shared/config.rs`): the active
  model's name by mode (managed — the GGUF's base name without the path/
  `.gguf`; external/cloud — `model_name`). `screens/chat.rs::model_meta` was
  refactored onto it (removing duplicate model-name resolution — the feed
  caption and the metadata snapshot now source the name from one place).
- **Wiring**: `GenSpawn` gained `engine_mode`/`model_name` (a snapshot from
  `config.engine` at the start of the turn), `finalize_message` accepts them
  and builds the metadata with `retain_supported(mode.cloud_provider())`.
- **Tests**: entity (`retain_supported` drops `top_k`/`thinking` for OpenAI,
  keeps them for local; Claude keeps `thinking`, drops `temperature`); config
  (`active_model_name` per mode, an empty name = None); orchestrator
  (`assistant_metadata_records_mode_model_and_filtered_sampling` — cloud mode
  → metadata carries `mode=openai`, `model`, and `top_k` in sampling is
  cleared). **755 tests green** (+3), clippy/fmt clean.

### Post-M9: engine restart debounce on settings edits (done)
- **Problem**: the settings screen applies an edit **on every field commit**
  (`SettingsIntent::SaveConfig` → `AppCommand::UpdateConfig` →
  `handle_update_config` → an immediate `apply_chat_settings()`), so a series
  of edits like "binary → model → -ngl" produced three heavy managed
  `llama-server` restarts back to back. Deferred groundwork from stage 6 of
  the settings redesign.
- **Solution**: **the server restart** is debounced, not saving the config —
  the config is written to disk and re-emitted to the UI right away (the
  settings screen lives off the re-emit), only the expensive `apply_*`
  (re)launch is delayed. New module `app/orchestrator/restart_queue.rs` —
  `RestartQueue { chat, embed, impersonation: bool, deadline }` (mirroring
  `SaveQueue`): `mark_chat/embed/impersonation()` set a flag and extend the
  deadline (`RESTART_DEBOUNCE = 1.2s` from the **latest** edit), `take()`
  returns the flags and resets. In the `run()` loop — a second branch
  `sleep_until_opt(restart_deadline) => flush_restarts()`; `flush_restarts`
  (in `settings.rs`) calls `engines.apply_*` only for the flagged servers +
  **one** `emit_server_status()`. It reads the final `self.config` (already
  replaced during the edit) → a series of edits = one restart with the final
  values. The "sole owner of `Chat`" invariant is untouched, no new channels.
- **Stayed immediate**: the initial server bring-up (before the loop, in
  `mod.rs` — the queue doesn't apply there), persisting + `emit_settings()`,
  and rebuilding the tool registry (switching `tools`/the provider is cheap,
  and the get/set_sampling schema needs to be current from the next turn).
  On `Quit`, deferred restarts are **deliberately not flushed** (servers are
  torn down via Drop/kill_on_drop — there's no point bringing up a process
  right before dropping it). Within the debounce window the old server stays
  alive (`Ready`), then the chip shows `Connecting → Ready` — exactly the
  observability added by stage 6.
- **Tests**: unit `RestartQueue` (coalescing flags + a `take` reset; the
  deadline is extended by each mark — under paused virtual time); an
  integration test `model_change_restarts_chat_server_debounced` (formerly
  `model_change_restarts_chat_server`): two engine edits in a row → `Settings`
  is re-emitted immediately, `chat_call_count()` is still 1 (the restart is
  deferred), after the deadline — exactly 2 (one restart for the whole
  series; the flush marker is `AppEvent::ServerStatus`). Deterministic timing
  — `#[tokio::test(start_paused = true)]`: `tokio` with the **`test-util`**
  feature was added to dev-dependencies (test builds only; virtual time is
  advanced to the deadline once all tasks are idle — no races).
  `MockSupervisor.chat_call_count()` already existed (from M8's DoD).
  **794 unit tests green** (+2), clippy/fmt clean. Docs: architecture.md
  §3/§6, spec §11.6.

### Post-M9: OpenAI no longer accepts `temperature`/`top_p` (done)
- **`openai` mode stopped sending and showing `temperature`/`top_p`**: only the
  **GPT 5.4** family accepted them (soon to be retired); **GPT 5.5/5.6** no longer
  have these parameters (a request with them → `400`). Previously both fields were
  part of the strict OpenAI/Gemini shared subset (multi-provider Phase 0).
- **Split the OpenAI and Gemini subsets** (`entities/sampling.rs::
  supported_sampling_fields` — the single source of truth for settings UI, `get_sampling`/
  `set_sampling`, and the `Message.metadata` snapshot): OpenAI = `frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens`; **Gemini** (the OpenAI-compatible endpoint) —
  the same **plus** `temperature`/`top_p` (the compat layer accepts them, as before);
  Claude — unchanged.
- **Mirrored on the wire** (`shared/api/openai/wire.rs`): the shared `restrict_to_strict`
  (llama.cpp extensions + reasoning) is untouched — `temperature`/`top_p` are stripped in
  the existing `dialect == WireDialect::OpenAi` branch alongside swapping
  `max_tokens → max_completion_tokens`, so the Gemini dialect keeps them.
- **Consequences with no code changes** (all derived from the single source): the
  "Sampling" section of the settings screen in `openai` mode no longer shows both
  fields (`cloud_supported_param`); the `set_sampling` schema no longer offers them, and `get_sampling`
  doesn't output them; `retain_supported` zeroes them out in the message metadata snapshot.
  Values remain saved in `default_sampling`/`impersonation_sampling` and will work on
  a local model/Gemini (like other hidden parameters).
- **Tests**: sampling (the OpenAI/Gemini subsets diverged; `retain_supported`
  drops `temperature` for OpenAI and keeps it for Gemini); wire (the OpenAI dialect strips
  `temperature`/`top_p`; Gemini sends them); introspection (`get_sampling` hides them for
  OpenAI, shows them for Gemini); settings (the "Sampling" section's field filter by
  mode); orchestrator (message metadata in `openai` mode without `temperature`). **827
  tests green** (count unchanged), clippy/fmt clean. Docs: architecture.md §9.

### Post-M9: OpenAI → Responses API ("thoughts", reasoning_effort, verbosity) (done)
- **`openai` mode moved from Chat Completions to the Responses API** (`POST /v1/responses`)
  — research and the decision are in [docs/research/openai-responses-client.md](../../docs/research/openai-responses-client.md),
  ADR 0004. Motive: reasoning summaries ("thoughts") and `reasoning.effort` for OpenAI
  live **only** in Responses; Chat Completions is legacy for reasoning models. Responses is
  a separate protocol of the same vendor → **a new `EngineBackend` implementation**
  alongside `AnthropicClient` (layers above the engine untouched). Chose **variant A**
  (a transport swap, not a second mode/toggle) — the config doesn't grow, `CloudProvider::OpenAi`
  remains the key for `supported_sampling_fields`. Gemini and External stay on Chat
  Completions (`OpenAiClient`); embeddings too (`/v1/embeddings`, absent in Responses).
- **New module `shared/api/openai/responses/`** (`ResponsesClient` + `wire`): hits
  `{base}/responses` (Bearer key), `wire::build_request` translates `ChatRequest` →
  Responses: system → top-level `instructions`; history → an `input` array of **items**
  (`{type:message}` with a string `content`, `reasoning`, `function_call`,
  `function_call_output` — no `tool` role); `max_tokens`→`max_output_tokens`; `store:false`;
  the function tool is flat (`{type,name,description,parameters,strict:false}` — our schemas aren't
  strict). SSE — event-based (the tag is the `type` field inside `data`, as with Anthropic):
  `response.output_text.delta`→`Text`, `response.reasoning_summary_text.delta`→`Thoughts`,
  `response.output_item.added`(function_call)+`response.function_call_arguments.delta`→
  `ToolCall`, `response.output_item.done`(reasoning with `encrypted_content`)→
  `ThoughtsSignature`, `response.completed`/`incomplete`→`Usage`+`Finished`. The finish
  reason is derived by the client (`saw_tool_call`→`ToolCalls`; `incomplete`→`Length`) —
  Responses doesn't send `finish_reason`. The error body isn't swallowed (like the other
  clients).
- **Reasoning + tool-use round-trip** (stateless `store:false`): with `include:
  ["reasoning.encrypted_content"]`, the reasoning item (`id`+`encrypted_content`) must be
  **resent before its `function_call`** — otherwise a quality regression (OpenAI
  measured ~3% on SWE-bench). This is **the same mechanism** as Anthropic's thinking
  signature (Phase B): `ChatChunk::ThoughtsSignature(String)` → `ThoughtsSignature(ThinkingRef{id,
  signature})` (only OpenAI carries `id`, Anthropic uses `None`), `ThinkingBlock` gained `id`;
  `RoundOutput.thinking_ref` accumulates the id+signature; `build_input` places the reasoning item
  before the `function_call`. Places that ignore the variant (`subagent`/`fetch`/`title`/
  `impersonation`/`tool_loop`) still match `ThoughtsSignature(_)` — untouched.
- **Sampling**: `ReasoningEffort` extended with `Minimal`/`XHigh` (gpt-5.x; the Anthropic
  wire maps them to `low`/`high`); a new field `SamplingConfig.verbosity: Option<Verbosity>`
  (`text.verbosity`, Responses-specific); `reasoning_budget==Some(0)` (impersonation/
  auto-title) → `reasoning.effort:"none"` with no summary (analogous to muting "thoughts").
  `supported_sampling_fields(OpenAi)` = `max_tokens`+`thinking`+`reasoning_effort`+
  `verbosity` (no `temperature`/`top_p`/`seed`/penalties — Responses lacks them). The UI's
  "Sampling" section shows reasoning/verbosity for OpenAI, `set_sampling`/`get_sampling`
  and the `Message.metadata` snapshot draw from the same source. A new `SamplingParam::Verbosity`
  (a Choice field, like Reasoning).
- **`WireDialect::OpenAi` removed** (its only consumer — cloud OpenAI — is now on
  Responses): along with the `max_completion_tokens` field and the OpenAI branch of `restrict_to_strict`.
  `LlamaCpp`+`Gemini` remain (Gemini is strict and keeps temperature/top_p).
- **The "proxy with a key" pattern is closed** (`ExternalSettings.api_key_env`, `#[serde(default)]` → no
  migration): the previous pattern "`openai` mode + a url override as an OpenAI-compatible proxy
  with a key" wouldn't have worked once `openai` moved to Responses — now there's
  External with an optional Bearer key from an env var for this (External previously had no
  key field at all — a standalone gap). Wired into the supervisor (chat/imp/embed external) and the UI
  (an "API key (env, optional)" field in the External group; the setter in `spec.rs` routes
  by mode, like url/model).
- **Tests**: responses-wire (system→instructions, store:false, reasoning/summary/effort/
  verbosity, budget=0→effort:none, tools strict:false, tool-call/result→items,
  the reasoning item before a function_call, thinking with no id → no reasoning item, parsing
  every SSE event); sampling (the OpenAi subset, `retain_supported`); settings (OpenAi
  shows reasoning/verbosity, Gemini/Claude don't; verbosity only for OpenAi);
  introspection (the `set_sampling` schema with minimal/xhigh + verbosity). **835 unit tests
  green** (+8), clippy `-D warnings`/fmt clean. Live `#[ignore]` smokes
  (`responses/client.rs`, `MINDFORK_OPENAI_KEY`): a simple generation, a `Thoughts`
  stream with thinking, a reasoning-item tool-use round trip — run against real OpenAI
  outside CI.
- **Live run (gpt-5.5)**: generation/tool-calling work; but **reasoning summaries
  ("thoughts") didn't arrive**. The cause is server-side, not the client: OpenAI only
  returns `reasoning.summary` **to verified organizations**
  (`platform.openai.com/settings/organization/general`) — for an unverified one the
  summary comes back empty (or a `400` on the very presence of `reasoning.summary`).
  Raw CoT is never returned — only the summary. Two fixes as a result: (1) `summary:"auto"` → **`"detailed"`**
  (some models return an empty summary at `auto` but text at `detailed`; gpt-5.x
  supports it); (2) parsing added for the `response.reasoning_text.delta` event in addition to
  `response.reasoning_summary_text.delta` — both → `ChatChunk::Thoughts` (robustness against
  models that stream reasoning under a different event name). Summaries will appear once
  the organization is verified.
- **Reasoning tokens in the status bar (done)**: `TokenUsage` extended with the
  `reasoning_tokens` field (OpenAI Responses `output_tokens_details.reasoning_tokens`;
  OpenAI-compat/llama.cpp `completion_tokens_details.reasoning_tokens`; Anthropic doesn't
  separate them → `0`, "thoughts" are already in `completion_tokens`). Accumulates across
  agentic-loop rounds (`stream_round` takes `base_reasoning`, `RoundOutput.reasoning_tokens`);
  the `AppEvent::TokenUsage.reasoning: Option<u32>` event (`None` — leave it be, it's only
  known from `usage`); `StatusModel.reasoning` → a "(reasoning N)" annotation next to the
  counter when `>0`. Lets you see the model's "thinking" activity even when the summary
  text is gated by organization verification. **836 tests** (+1: `token_counter_shows_reasoning_when_present`).
- **Groundwork** (docs/research/openai-responses-client.md §5): `cached_tokens` in the
  counter, `prompt_cache_key`, OpenAI's built-in server-side tools (`web_search`/
  `code_interpreter` — conflict with the client-side agentic loop), native Gemini via
  its own protocol (next track).

### Post-M9: native Gemini client (generateContent) — Phase A (done)
- **`gemini` mode moved from OpenAI-compatible Chat Completions to native
  `generateContent`/`streamGenerateContent`** — research and the plan are in
  [docs/research/gemini-native-client.md](../../docs/research/gemini-native-client.md), ADR 0004.
  Motive: the compat path (`OpenAiClient`+`WireDialect::Gemini`) stripped reasoning under
  the strict dialect — there was no "thoughts" or depth control at all (even `top_k`,
  which Gemini accepts natively, was cut). The native API is **a new `EngineBackend`
  implementation** alongside `AnthropicClient`/`ResponsesClient` (layers above the
  engine untouched). **Phase A** — the core (thoughts + reasoning, no signatures); **Phase B**
  (Gemini 3 thought signatures at tool-use) — the next step.
- **New module `shared/api/gemini/`** (`GeminiClient` + `wire`): hits
  `…/v1beta/models/{model}:streamGenerateContent?alt=sse` (the `x-goog-api-key` header),
  `wire::build_request` translates `ChatRequest` → Gemini: system → top-level
  `systemInstruction:{parts:[{text}]}`; history → `contents:[{role:"user"|"model",
  parts}]` (no `system`/`tool` roles — system is top-level, a tool result → a
  `functionResponse:{name,response:{result}}` part with `role:"user"`; adjacent items of the
  same role are merged); a call → a `{functionCall:{name,args}}` part (`args` is an **object**, not a
  string; Gemini has no `call_id` → the client synthesizes a stable id `"{name}-{index}"`,
  matching the functionResponse by it); `max_tokens`→`generationConfig.maxOutputTokens`;
  reasoning → `thinkingConfig`. SSE parts: `text`→`Text`, `text`+`thought:true`→`Thoughts`,
  `functionCall`→`ToolCall` (args as a whole chunk), `usageMetadata`→`Usage`
  (`thoughtsTokenCount`→`reasoning_tokens`), `finishReason`→`Finished`
  (`MAX_TOKENS`→`Length`; the presence of calls→`ToolCalls` even at `STOP`). The error body
  isn't swallowed (like the other clients). The client has no embeddings — RAG in Gemini mode gets
  them via the OpenAI-compatible `…/v1beta/openai/embeddings` (`OpenAiClient`), as with Anthropic.
- **thinkingConfig + generation inference** (added already in Phase A): `includeThoughts:true`
  when `thinking==Some(true)`; the depth — Gemini 3.x via `thinkingLevel`
  (`minimal/low/medium/high`), Gemini 2.5 via `thinkingBudget` (tokens; mirroring the compat
  table: low→1024/medium→8192/high→24576). The generation is inferred crudely from the model name
  (`gemini-3*`). `reasoning_budget==Some(0)` (impersonation/auto-title) mutes it:
  `thinkingBudget:0` (2.5 — off) / `thinkingLevel:"minimal"` (3.x — can't be fully
  turned off, like OpenAI's `effort:none` isn't available everywhere). Gemini has no `verbosity`.
  Tool schemas are sanitized to an OpenAPI subset (dropping `$schema`/
  `additionalProperties`; Phase C will refine this against live schemas).
- **Sampling**: `supported_sampling_fields(Gemini)` = `temperature`/`top_p`/**`top_k`**/
  `max_tokens`/`seed`/`frequency_penalty`/`presence_penalty` + **reasoning**
  (`thinking`/`reasoning_effort`), no `verbosity`. Differences from the previous compat
  path: **+`top_k`** (accepted natively) **+reasoning**. The single source of truth → the UI's
  "Sampling" section, `get/set_sampling`, and the `Message.metadata` snapshot pick this up automatically.
- **`WireDialect` removed entirely** (orphaned after Gemini's move — it was the only
  remaining strict consumer; `OpenAi` had already moved to Responses): along with
  `is_strict`/`restrict_to_strict` and the `dialect` parameter of `build_chat_request`/
  `OpenAiClient` (`openai/wire.rs` shrank by ~160 lines). `OpenAiClient` remains for
  external/proxy and embeddings, sending sampling as-is (llama.cpp ignores unknown fields).
- **Supervisor**: `cloud_chat_setup`'s `Gemini` branch → `GeminiClient`; a new
  `CloudProvider::chat_base_url()` gives the native `…/v1beta` for chat, while `base_url()`
  (`…/v1beta/openai`) remains for embeddings. Impersonation in Gemini mode automatically
  gets the native client (the same `cloud_chat_setup`).
- **Tests**: wire (system top-level; generationConfig; thinkingBudget for 2.5 /
  thinkingLevel for 3.x / force_off; schema sanitization; functionCall/functionResponse +
  merging; coercion of non-object args; parsing SSE parts — text/thought/call/usage);
  client (finishReason mapping, stripping the `models/` prefix). **848 unit tests green**
  (+13), 3 `#[ignore]` smokes (`MINDFORK_GEMINI_KEY`: generation, a thought stream, one
  tool round), clippy `-D warnings`/fmt clean. A live run against a real key is outside CI.

### Post-M9: native Gemini client — Phase B (thought signatures at tool-use) (done)
- **Gemini 3 thought signatures (`thoughtSignature`) for the tool-use round trip** — a
  unique difference from Anthropic/OpenAI: at Gemini a signature is tied to a
  **specific part** (`functionCall`), not to a whole turn, and it's **mandatory for
  Gemini 3** (otherwise a `400`: "missing thought_signature" on a historical call).
  Hence the existing `ThinkingRef`/`ThinkingBlock` (one per turn) **isn't reused** —
  the signature rides on the call itself. See docs/research/gemini-native-client.md §2.3.
- **Contract**: `ApiToolCall.thought_signature: Option<String>` and
  `ToolCallDelta.thought_signature` (`ApiToolCall` gained `Default`); `ToolCallAccumulator::
  push` accumulates the signature into the right call by `index` (for parallel calls
  Gemini attaches the signature only to the first — the accumulator survives this, the
  rest get `None`). Other backends leave the field unset.
- **Domain (persistence)**: `ToolCallRecord.thought_signature: Option<String>` (`#[serde(default,
  skip_serializing_if=Option::is_none)]` → old chats need no migration; empty doesn't
  clutter JSON). Persistence is **mandatory**: `message_to_api`/`record_to_api` rebuild the
  history on every generation, and without the signature on a historical `functionCall`
  Gemini 3 returns `400`. The signature is opaque/encrypted — safe to store.
- **Wiring**: the Gemini client puts `part.thought_signature` into `ToolCallDelta`; the wire
  `build_contents` resends it as a `functionCall` neighbor (`thoughtSignature`) when
  present; `generation.rs` persists `call.thought_signature` into `ToolCallRecord`;
  `record_to_api` carries it back on replay. Within a single generation the signature travels via
  `out.calls` (the accumulator → `assistant_tool_calls`), between generations — via persistence.
  Round-level `thinking_ref`/`.with_thinking` (Anthropic/OpenAI) is **untouched** — Gemini doesn't
  use it.
- **Decision point "persist vs. current-turn-only signature"** (§7-1): whether Gemini 3
  requires the signature on **all** historical `functionCall`s or just the most recent
  turn — pending a live key. By default we design **with persistence** (safer); if a live
  run shows an in-memory signature (like Anthropic) is enough, persistence can be dropped.
- **Known limitation**: signatures on **text parts** (a pure-reasoning turn with no call)
  aren't persisted — our assistant-text replay doesn't carry them; Gemini 3's hard
  requirement only concerns `functionCall` parts.
- **Tests**: wire (emitting `thoughtSignature` as a `functionCall` neighbor when present; no
  key → no signature); contract (carrying the signature through the accumulator); message (serde:
  `None` default for an old record, round trip, `skip` when empty). **851 unit tests green**
  (+3), + an `#[ignore]` smoke `tool_use_round_trips_signature` (Gemini 3: round 2
  resending the signature with no `400`). clippy `-D warnings`/fmt clean.
- **Live run — GO** (Gemini 3.1 Pro Preview, `MINDFORK_GEMINI_MODEL=gemini-3.1-pro-preview`):
  all 4 gemini smokes green (generation, a "thoughts" stream via `includeThoughts`, one
  tool round, a signature round trip) — `tool_use_round_trips_signature`'s signature
  arrived (`thought_signature present: true`) and the resend went through **without
  a `400`**. The signature mechanism and both phases are confirmed on a live model; decision
  point §7-1 closed by design choice (always resend → the failure mode is unreachable),
  persistence kept.

### Post-M9: native Gemini — Phase C (partial: force-off for 2.5 Pro + surfacing blocks) (done)
- Two targeted correctness fixes following Phases A+B (docs/research/gemini-native-client.md
  §8 Phase C). Full tool-schema sanitization and an explicit UI choice of `thinkingLevel`/
  `thinkingBudget` — deferred (a scan of the tool schemas found them clean: the only
  "exotic" bit is `enum`, which Gemini accepts; no `$ref`/`oneOf`/`nullable`/… present).
- **Force-off clamp on Gemini 2.5 Pro** (`wire::thinking_config`): 2.5 Pro **can't
  disable thoughts** (`thinkingBudget` has a minimum of 128) — the previous `reasoning_budget==0`
  (impersonation/auto-title) sent `thinkingBudget:0` → `400`. Now for 2.5 Pro
  (`is_gemini_25_pro` = the name contains `gemini-2.5-pro`) force-off sends `128` (+
  `includeThoughts:false`); Flash/Flash-Lite still get `0` (there `0` does disable it).
  A narrow bug (only 2.5 Pro + muting), but a concrete one.
- **Surfacing blocks** (`client.rs`): previously a `finishReason` like `SAFETY`/`RECITATION`
  and a blocked prompt reduced to a silent, empty `Stop` — the user saw a silently
  empty turn. Now: (1) `promptFeedback.blockReason` (the request rejected by the filter
  before generation, a new `PromptFeedback` type in wire) and (2) blocking `finishReason`
  values (`is_block_reason`: SAFETY/RECITATION/BLOCKLIST/PROHIBITED_CONTENT/SPII/IMAGE_SAFETY/MALFORMED_FUNCTION_CALL/
  OTHER, except when a tool call is present) → emit a `ChatChunk::Text` note
  ("⚠ Gemini did not produce a response (reason: …)") + a `warn` log, so an empty turn is
  explainable. `Finished(Stop)` (not Error) — a normal turn completion with an explanation
  in the feed.
- **Tests**: wire (force-off for 2.5 Pro → 128; parsing `promptFeedback.blockReason`); client
  (`is_block_reason`/`block_note`). **853 unit tests green** (+2), clippy `-D warnings`/
  fmt clean. **Groundwork (not Phase C)**: thought signatures on text parts (not
  persisted — Gemini 3's hard requirement is only for functionCall); checking the full
  tool registry against Gemini (on a live run — only fix real `400`s).

### Post-M9: readiness probe for the embedding server (done)
- **Symptom** (user report, with a screenshot): the machine hosting the embedding
  server was off (`ping 192.168.1.20` — "Destination host unreachable"), yet the
  chip read **`● embeddings: ready`** (green). **Cause**: the status was derived from the
  configuration, never from the network — in `external` mode the entire "check"
  was a non-empty URL → `ServerStatus::Ready` ([supervisor.rs](../../src/app/supervisor.rs)),
  and `embed_status` was written exactly once, in `EngineManager::apply_embed`,
  with no channel to update it later. Deliberate at the time (ADR 0002 — RAG is
  lazy) and documented on `EmbedSetup`, with "a real probe is groundwork" recorded
  in the journal; this closes that item. Branch `fix/embed-server-probe`.
- **Fix — the embedding server is now probed exactly like the chat server**:
  `ServerSupervisor::apply_embed` gained `cancel`/`status_tx`/`loc` and returns an
  **immediate** `Connecting`, while a background `/health` probe (the existing
  shared `spawn_probe`) posts the real status. Managed passes `handle.exited()`, so
  a process that dies *while loading* (corrupt GGUF/OOM) is reported at once
  instead of after `MANAGED_READY_TIMEOUT`; external gets `EXTERNAL_READY_TIMEOUT`.
  `EngineManager` gained `embed_status_tx` + `embed_probe_cancel` +
  `set_embed_status` (a stale probe can't overwrite a newer server's status — the
  invariant the chat/impersonation servers already had), and `run` gained a fifth
  `select!` arm → `emit_server_status`.
- **The cloud deliberately keeps `Ready` with no probe** (mirroring
  `cloud_chat_setup`): there's nothing to load and no `/health`. Pinned by a test
  that asserts the channel stays *silent* — otherwise a future refactor could
  quietly start probing an endpoint that doesn't exist.
- **A launch failure is no longer disguised as "not configured"**: managed
  `ServerHandle::launch` failing (e.g. a missing model file) now yields
  `Disconnected(reason)` instead of `unavailable_embed()`. The settings-window chip
  shows the reason (the status-bar chip stays compact, by design). This also let
  the "`apply_embed` has no `loc`, pass the reference locale, the text is log-only"
  workaround go — the reason is now displayed, so it's localized properly (axis B).
- **Nothing gates on the new status** — RAG stays lazy and degrades through the
  error from the call itself; the status is informational. Two doc comments that
  had justified themselves with "there is no probe" were corrected rather than
  deleted: `EmbedGuard`'s reasoning survives intact but for a sharper reason — a
  probe says the *server* answers, never *which model* does, which is precisely
  what the canary exists to establish (and the cloud has no probe at all).
- **A testing trap worth recording**: the natural negative test (probe a dead port,
  expect `Disconnected`) ran **63 s**. Measured rather than guessed — a single
  `probe()` against a closed local port costs **~2.0 s** on Windows (SYN retry),
  and the probe retries until its timeout: 30 × 2 s. `start_paused` was already
  working (the sleeps were virtual); the connect was the whole cost. Fixed by
  making the stub *listen*: a throwaway `TcpListener` that accepts and either
  answers `200` or hangs up — failure is then immediate and the retry sleeps stay
  virtual. 63 s → **2 s**, and the test now covers the happy path end to end as
  well. Second trap inside it: writing the response without first draining the
  request makes the close an RST that discards the response — the healthy stub has
  to `read` before it writes.
- **Tests**: `Connecting` (not `Ready`) for a configured external server — the
  direct regression; the probe posting `Ready` against a live-ish stub and
  `Disconnected` against a silent one; a stale probe sending nothing; a managed
  launch failure surfacing as `Disconnected`; the cloud `Ready` **without** a probe.
  **1472 unit tests green** (+6), **67 `#[ignore]`** (+1), clippy `-D warnings`/fmt/
  `cyrillic_scan` clean.
- **Live run — GO** (bge-m3 on a real external `llama-server --embeddings`,
  `MINDFORK_EMBED_URL`): `embed_probe_reaches_ready_on_live_server` — the one
  question unit tests can't settle is whether a real `--embeddings` server serves
  the `/health` the probe relies on, since a probe that misjudges a *working*
  embedder would be worse than the bug it fixes. It does: `Connecting` →
  **`Ready`**. (The risk was bounded anyway — `probe()` treats `404` as alive, so a
  server without `/health` reads as ready either way — but it needed confirming,
  not assuming.)
- **Regression — clean**: all **25** orchestrator e2e live smokes green (507 s) on
  Gemma 4 31B q4_0 + bge-m3 (external `llama-server`, `--jinja`) — memory/
  self-model/notes/graph/cross-organ links/RAG/attachments/control tools/i18n/MCP.
  The full set is the right scope: `apply_embed` builds the embedder every memory
  path then uses, so "the status is now honest" had to be shown not to have cost
  anything downstream.

### Post-M9: periodic server health monitoring (done)
- **The other half of the previous entry** (user request; design doc
  [docs/server-health-monitoring.md](../../docs/server-health-monitoring.md), forks
  **F1–F6 confirmed 2026-07-28** — F2/F4 put to the user explicitly, the rest taken
  by recommendation). Branch `feat/server-health-monitoring`, stacked on
  `fix/embed-server-probe`.
- **The framing that shaped the design**: the obvious symptom is a stale chip, but
  the same one-shot probe had a sharper consequence nobody had named — **a server
  that isn't up when the app starts stays unusable until the user intervenes**. The
  initial probe fails → `Disconnected` → `backend_if_ready` refuses → nothing ever
  re-probes, so starting the app before `llama-server` (the ordinary order for a
  local setup) left generation blocked even after the server came up. So the
  feature is really **recovery**, and that's the half users feel; the honest chip is
  the by-product. This drove F2: the interesting question isn't "how fast do we
  notice a failure" (the failing request answers that immediately, with a better
  message) but "how fast do we notice a *fix*".
- **`spawn_probe` became a monitor** (`supervisor.rs`): phase 1 is the existing
  `wait_until_ready` (a managed GGUF loads for minutes), phase 2 keeps watching.
  Cadence `HEALTHY_POLL=60s` / `RECHECK_POLL=5s`, the fast one used while down
  **or** while a failure streak is pending — without that second condition,
  confirming a failure at N=3 would take ~3 minutes instead of ~15 s. Hysteresis
  (`FAILURES_TO_UNHEALTHY=3` down, one success up) lives in a small pure `Health`
  type, so the transition rules are testable without a server or a clock; only a
  **flip** is published, so a steady server never wakes the UI.
- **Managed relaunch** (F4a): a dead child leaves a port no amount of probing will
  revive, so `Orchestrator::relaunch_dead_managed_servers` re-`apply`s it under a
  `RestartBudget` (≤3 per 5 min, cleared on reaching `Ready` — the budget guards a
  crash *loop*, not a machine's lifetime outages). Deliberately a second small
  implementation rather than sharing `McpManager::allow_restart`: unifying them is a
  mechanical refactor and doesn't belong in a behavior change (AGENTS.md §2).
  A launch that fails **synchronously** (the missing-model preflight) publishes no
  status and so never reaches this path — retrying it would be pointless until the
  settings change, and that's also what keeps the relaunch from looping.
- **External/cloud are never relaunched** — we don't own the process; their monitor
  recovers them by itself. Cloud isn't even monitored (F1a): the only way to check
  it is a real API call, which costs money and quota to answer a question the next
  real request answers for free.
- **Live measurements decided three things, none of them guessed**: (1) `/health`
  answers **200 during active generation** on this llama.cpp build (5 probes while a
  600-token completion streamed) → the monitor needn't pause during generation,
  though hysteresis covers builds that answer `503` under load; (2) a **real managed
  child killed externally is reported in ~150 ms** from the exit signal; (3) the
  same process going away *without* the exit signal takes **76 s** (one healthy poll
  + the streak) — which is exactly the gap that justifies watching `exited`.
- **A test that failed for the right reason, worth recording**: the first version of
  the managed smoke killed the child by **dropping the handle** and asserted <10 s.
  It measured 76 s and reported a *probe* error rather than the exit message —
  because dropping the handle is **us** stopping the server deliberately, which the
  process monitor treats differently (and must: that's what a re-`apply` does, and
  it has to stay silent). Wrong premise, right code; the fix was to kill the process
  from outside (`taskkill` on the PID from `netstat`), which then reported in 148 ms.
- **Tests**: pure `Health` (a streak of N−1 does **not** flip — the assertion that
  fails if someone simplifies the counter away; a success mid-streak resets; a steady
  server publishes nothing; the cadence follows suspicion, not just state);
  `RestartBudget` (cap, window pruning, cleared on recovery, independent per server);
  the monitor end to end against a **toggleable** stub listener — `Ready` → the
  server goes away → `Disconnected` → it comes back → `Ready`, all with paused time;
  a steady server stays quiet over 10 virtual polls; the orchestrator relaunching a
  dead managed server until the budget stops it, and never relaunching an external
  one. **1484 unit tests green** (+12), **69 `#[ignore]`** (+2), clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`; plus a
  local `llama-server` + bge-m3 for the managed case):
  `monitor_does_not_flap_against_a_live_server` — the realistic failure mode the stub
  can't speak to is a status that flaps against a *real* server on a *real* network;
  watched for a full healthy interval + margin, **not one spurious status**.
  `managed_child_death_is_noticed_at_once` — **148 ms** (see above).
  **Regression — clean**: all 25 orchestrator e2e live smokes green.

### Post-M9: no server restart when nothing effectively changed (done)

- **Closes the consequence recorded in §5.1** of
  [settings-undo.md](../../docs/history/settings-undo.md): `handle_update_config` marked
  a restart by diffing the incoming config against the **previous edit**, so an
  engine edit plus its `Ctrl+Z` marked it twice and the debounce produced one
  restart that reloaded the server with the values it already had — on a managed
  server, unloading and reloading a multi-GB GGUF for nothing. Branch
  `fix/idle-server-restart`. No forks — the approach was already recorded in the
  plan, so no design doc (AGENTS.md §1: a simple task).
- **The fix splits two meanings that had been conflated.** The `RestartQueue`
  flag keeps saying "something was edited" (cheap, set at edit time); whether a
  restart is *worth doing* moved to `flush_restarts`, which compares the final
  config against what each server was **actually launched with**. A flag can't be
  un-set, so deciding at flush time is the only place the answer can be right.
- **The recorded identity is (settings, key blob)**, because `apply_*` resolves
  the key itself via `stored_key` — equal settings with a different blob still
  warrant a relaunch. The comparison uses the stored **ciphertext**, never the
  plaintext: no extra copy of a secret is kept, and re-encrypting the same key
  yields a different nonce, so it errs towards restarting — the safe direction.
- **Recorded by `apply_*` itself, not by the flush.** That's what keeps
  `relaunch_dead_managed_servers` correct by construction: it re-applies the same
  settings on purpose after a crash, and must never be skipped. Impersonation
  records in **both** arms of its `match`, including `shared` — that mode is a
  state the server can be *in*, so switching away and back must not read as
  "nothing to do".
- **MCP got the same guard** (`McpManager::is_current`): a re-apply there kills
  and respawns `npx` → node processes, the most expensive one on the screen, and
  it is reached from the same debounce.
- **A deliberate loss, recorded rather than papered over**: since a re-apply now
  needs a real difference, the undocumented trick of "wiggle a settings field to
  reload the server" no longer works — including as a way to pick up a changed
  env-var API key (`api_key_env`). It was never a designed feature and depended
  on the two edits landing on opposite sides of the debounce; the honest fix, if
  wanted, is an explicit "restart servers" action.
- **The test had to prove a restart *didn't* happen**, which can't rest on
  waiting for an absent event. `an_edit_and_its_undo_cost_no_restart` advances
  virtual time past the deadline (under `start_paused` tokio runs the
  orchestrator's timer first, so its flush has provably run), asserts the counter
  is unchanged, and then makes a **genuine** change and checks the counter
  against *its* status event — a deterministic marker that also proves the flush
  path still works. Mutation-tested: restoring the unconditional flush fails it.
  **1675 unit tests green** (+1), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): the change is *when* `apply_*` is
  called, not what it does — the launch path, its arguments and every provider
  protocol are untouched, and `MockSupervisor`'s call counter is exactly the
  observable in question.

### Post-M9: history compression — stage 1 (core, `/compact`) (done)

- **The first stage of the "history compression" track** (research + forks
  [docs/research/history-compression.md](../../docs/research/history-compression.md),
  decided by the user 2026-08-07; **stage 0's probe returned GO**, §9a there).
  Behaviour — spec §6.7. Branch `feat/history-compaction`, stacked on the
  research branch. The whole conversation was sent on every request, so a long
  chat eventually hit the model's context ceiling — measured on the live stack:
  llama-server answers **HTTP 400 before the SSE stream starts**, and
  `--context-shift` does not rescue an oversized prompt (it evicts during
  generation, discarding the **system prompt first**).
- **The decision that dissolves most of the hard problems: compression changes
  what a *request* carries, never what the chat holds.** `Chat.messages` is
  untouched, so the feed, full-text search, the `F5` export, TTS, the reflection
  watermark and content indexing all keep working with no changes at all. That
  single constraint is why stage 1 is additive almost everywhere.
- **`Chat.compaction: Option<Compaction>`** (`summary`/`upto`/`boundary_id`/
  `compacted_at`/`rolls`; additive, no migration — ADR 0006 F12). `upto` is a
  fast path only: `Chat::compaction_view(enabled)` **re-finds the boundary by
  `boundary_id`** on every read, so an edit that shifts indices cannot leave the
  summary silently covering the wrong span, and a boundary that is gone makes the
  summary inert (the full history is sent) rather than pinned to whatever now
  occupies that index.
- **The cut always lands on a `User` message** (`features::compaction::plan_cut`,
  snapping back from the tail budget). Not cosmetic: cutting inside an assistant
  turn would separate it from its tool results, breaking Anthropic's strict
  alternation and Gemini 3's per-call thought-signature replay.
- **The digest carries tool activity** — a deliberate difference from the title
  digest. Tool results are persisted in history, replayed on every turn and
  invisible in the feed, so they are the largest hidden cost in a long chat; a
  digest that dropped them would compress the cheap half. (The agent that built
  it also established which source is real: `ToolCallRecord.result` is populated
  unconditionally at the one production site; the `Tool`-message fallback is
  reachable only for imported or hand-edited chats.)
- **The roll is `title.rs`'s shape, not `tool_loop.rs`'s** — one single-turn
  request, no history, no tools, reasoning muted (`reasoning_budget = 0`), result
  back on a typed channel. `SilentLoop` was the wrong fit by construction: its
  done channel carries `Result<(), String>`, and a summary is precisely the text.
- **Two prompt requirements came from stage 0's measurements, not from taste**:
  the length limit is stated **in words inside the prompt** (`max_tokens` is only
  a safety net far above it — measured, a bare cap truncates mid-sentence instead
  of making the model prioritize), and the roll template tells the model to drop
  what later parts superseded (without that, the summary **grows with every
  roll**). `finish_reason == Length` is logged as "the summary was cut" rather
  than silently accepted.
- **Fork F10 — on by default, and *inert* when off**: no splice (the request is
  byte-for-byte what it was before the feature existed), `/compact` refuses with
  a pointer at the setting, no divider — and the stored summary is **kept**, so
  off then on then off is lossless in both directions.
- **UI**: `/compact`; a muted divider at the boundary carrying the summary as a
  foldable block that expands **together with the "thoughts" blocks** (`Ctrl+T`,
  fork F8c) — no new hotkey and no new `FeedView` field; a quiet status-bar chip
  while a roll runs; three fields in a new "Context" group opening the "Memory"
  settings section. New `AppEvent::Notice` — a plain informational note in the
  feed, the counterpart of `Error`, since "nothing to compress yet" is not a
  failure.
- **Three defects found during the work, each by something other than reading
  the code.** (1) `inject_compaction` used `summary?`, which returns `None` from
  the function and therefore **dropped the persona** whenever no summary existed
  — caught by a *pre-existing* request test, which is exactly what a regression
  suite is for. (2) A roll finishing for a **non-active** chat would overwrite
  the open chat's boundary and post its note in the wrong conversation — found by
  the agent building the feed, closed with the same `chat_id` staleness guard the
  streaming events already use. (3) The new bundle prefix `compact.` made the
  i18n gate read the scratch filename `"compact.db"` (`shared/storage/db`) as a
  key; fixed at the source by naming the two model-facing keys `compaction.*`
  rather than adding a scanner exception.
- **Delegation note** (the hazard is now twice-observed): an agent finished
  writing a file it owned *after* I had edited the same file, silently reverting
  the key rename inside it. Re-apply and re-verify after an agent completes.
- **1897 unit tests green** (+62), **77 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Live run — GO** (Gemma 4 31B q4_0, external `llama-server`, `--jinja`):
  `compaction_preserves_a_planted_fact_e2e_live` — an identifier planted early,
  12 messages folded, and the model still answers `ZARYA-8823` when those
  messages are no longer sent verbatim (44 s). **The first version of this smoke
  was wrong and was fixed**: the control question was asked *last*, so its own
  answer stayed in the verbatim tail and the final assertion could have been
  satisfied by reading that instead of the summary. The control now runs
  immediately after the seed so both fall behind the boundary, and the test
  additionally asserts the summary itself carries the identifier.
- **Regression — clean**: the full orchestrator e2e live set, **26 passed / 0
  failed** (706 s) on the same stack — memory/self-model/notes/graph/cross-organ
  links/RAG/attachments/control tools/tool confirmation/MCP/i18n. The right scope
  even though the feature is opt-in per chat: `build_request` now slices
  `messages` and sits on **every** turn, so "it only changes what a compacted
  chat sends" had to be demonstrated rather than argued.
- **One honest observation from that run**: on this fixture the summary came out
  extremely terse (42 characters for 12 messages) — it kept the identifier and
  dropped the generic Q&A entirely. Defensible under the prompt (that filler
  contains no decisions, constraints or open questions), but a reminder that a
  summary is lossy for anything not decision-shaped, which is the case stage 3's
  read-back tools (fork F9b) exist to answer.
- **Next**: stage 2 — the automatic trigger (`GenResult` carries `usage`, budget
  resolution from `/props` and from the 400 body, the threshold in settings, the
  overflow error naming `/compact`); stage 3 — `history_read`/`history_search`.

### Post-M9: history compression — stage 2 (the automatic trigger) (done)

- **Stage 2 of the track** (research
  [docs/research/history-compression.md](../../docs/research/history-compression.md);
  track-level forks F1–F10 decided 2026-08-07, **sub-decisions S1–S8 recorded
  before implementation and S1/S2/S3 confirmed by the user 2026-08-08**, all as
  recommended). Behaviour — spec §6.7. Branch
  `feat/history-compaction-auto`. Stage 1 made compaction possible; this makes
  it happen without being asked, which is the half that saves the 8k-window user
  who does not know the command exists.
- **Reading the stage-1 code first turned the plan's one line into eight
  questions**, and the three with real trade-offs went to the user rather than
  being decided quietly:
  - **S1 — the budget's source.** `EngineBackend::context_budget()` with a
    `None` default, implemented by `OpenAiClient` over llama.cpp's `/props`. The
    trait **is** the engine contract (ADR 0004), so "what window does this engine
    have" belongs on it; the alternative (resolve it in the supervisor's
    readiness probe, which already holds the concrete client) would widen a
    trait shared by three servers and hang a budget off a channel that exists to
    carry a status. Every existing backend compiles unchanged, and the default
    means **"cannot say", never "unlimited"** — a backend that has no answer
    leaves the trigger inactive rather than acting on a guess. `n_ctx` is read
    **as given**: measured in stage 0 (M2), with no `-np` flag the server reports
    four slots over an *undivided* context, so dividing would be wrong by 4x.
  - **S2 — which number.** The exact `usage.prompt_tokens` and nothing else. The
    byte estimate is not a fallback here because its error **changes sign** by
    content type (§9a M9: +68% on Russian prose, −20% on code, −7% on JSON tool
    results), i.e. it is unsafe on exactly the tool-heavy chats that overflow
    first. A provider reporting no usage leaves the trigger silent — every
    provider we speak to does report it, so the exclusion is theoretical.
  - **S3 — learning the window from the 400 body: deferred**, on a measured
    redundancy rather than for effort: the body shape carrying `n_ctx`
    (`exceed_context_size_error`) **is** llama.cpp's, and llama.cpp answers
    `/props`. What the failure genuinely needed was the hint (S4), and the user
    is not stuck without it — `/compact` needs no budget at all.
- **The rest, as recommended**: the reply reserve is folded into the threshold's
  remaining 25% rather than being a second knob (S5); a **background** failure
  spends a strike in the existing failure streak while a **typed** command
  reports immediately, so `CompactResult` carries its origin (S6 — the stage-1
  code comment asked for exactly this); the trigger runs on the chat whose turn
  just finished rather than "the active one" (S7); cloud is an explicit setting
  or nothing, with no provider→window table (S8).
- **The planning seam PR #275 asked for exists now**: `plan_roll` decides what a
  roll would fold and builds its request with no engine and no side effects, and
  `spawn_roll` launches it. `/compact` turns a `None` into "nothing to compress
  yet", the automatic path just stays quiet, and a test asserts on the plan
  instead of inferring it from what a roll happened to send.
- **Discovery is epoch-guarded and self-healing.** `ContextDiscovery` asks once
  per applied engine and is invalidated on an engine change *and* on a readiness
  flip — so a server that came up after the app is not left unmeasured, while an
  answer about an engine that has since been replaced is dropped (switching from
  a local 8k model to a cloud one must not leave the cloud measured against the
  local window). The status channel only carries flips, so this is not a
  per-probe cost.
- **The overflow message must not create a dead end** (S4): with compression on
  it names `/compact`, with it off it names the setting. Pointing at a command
  that would refuse is the defect class this journal has already recorded three
  times — the by-reference attachment block, `youtube_watch`'s unconfigured
  path, `python_exec`'s sandbox. Detection is a pure function over the error
  text (a small documented marker list, best-effort: a marker that stops
  matching costs the hint, never correctness, and the raw body is still there).
- **A pre-existing defect found by the live smoke refusing to fire, and it is
  the most valuable thing in this stage** (§9b of the research): the client
  **never emitted `ChatChunk::Usage` at all** on the llama.cpp path.
  llama-server sends the `include_usage` chunk **after** the one carrying
  `finish_reason` (`choices` empty, then `[DONE]` — re-measured on the wire),
  and `chat_stream` `break`ed the moment it saw a finish reason. Consequences:
  the status bar's exact figure never arrived, so the `~` estimate was in
  practice the only number the user ever saw — contrary to what spec §11.1
  claimed — and stage 2's trigger, which reads that figure and nothing else, had
  nothing to fire on. Fixed by holding the reason until the stream's own
  terminator. The other clients are unaffected, **checked rather than assumed**:
  Anthropic carries usage in `message_delta`, OpenAI Responses in
  `response.completed`, Gemini in the same part as `finishReason` — all
  alongside the finish signal, not after it.
- **Two of my own diagnostics were instrumentation bugs, not findings**, worth
  recording because both looked like evidence: `run_turn_capture` drains events
  up to `Finished`, so it had already consumed the `TokenUsage` events the
  diagnostic was looking for — twice, before and after moving the collection
  inside the turn. Only isolating the question to the client (stream one
  request, print the chunks) answered it. The first smoke failure was a third
  such artifact: four short exchanges came to ~330 tokens against a threshold of
  491, so the test grew the prompt with **the user's own text** rather than
  relying on how verbose a model feels like being.
- **Tests**: the trigger's gates (fires past the threshold; the reply counts
  towards the next prompt; silent without exact usage; both the switch and a
  zero threshold disable it; not started twice; a conversation with nothing left
  to fold is silent rather than nagging every turn); budget resolution
  (explicit outranks all, managed reads its own `-c`, a discovered window is
  used and re-asked after an invalidation, an answer for a replaced engine is
  dropped, an engine that cannot say leaves it unknown); failure semantics by
  origin, including that an empty summary is counted and not announced;
  `/props` parsing against a stub, every way of not knowing, and an unreachable
  server; the overflow detector against one real body per provider plus five
  negatives; end-to-end through the real loop, that a full window is explained
  and never points at a dead end. **1921 unit tests green** (+24), **79
  `#[ignore]`** (+2), clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean. The SSE ordering fix was **mutation-tested** — restoring the `break`
  fails its test and only that one.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`,
  `--jinja`): `props_reports_the_context_window_live` — the real server reports
  **16384**, which is the half only a live stack can answer (a stub would only
  prove our fixture parses). `auto_compaction_fires_without_the_command_live` —
  deliberately in **external** mode so the budget can come from nowhere but
  `/props`: the exact prompt grew 3642 → 3908 → 4192 and the conversation was
  folded **with nobody typing `/compact`**, on the turn after the window was
  discovered — which also demonstrates the "the first turn kicks the question
  off and the next one acts on it" self-healing.
- **Regression — clean**: the full orchestrator e2e live set, **28 passed / 0
  failed** (683 s) on the same stack. The right scope twice over: the trigger
  sits in `handle_done` on every turn, and the client fix changes stream
  termination for **all** OpenAI-compatible traffic — memory/self-model/notes/
  graph/cross-organ links/RAG/attachments/control tools/tool confirmation/MCP/
  i18n all green.
- **Groundwork**: learning the window from the 400 body (S3);
  `estimate_prompt_tokens` still ignores `req.tools`, which matters for the `~`
  figure though no longer for the trigger; `cached_tokens` is reported by
  llama-server and still ignored (it would let the prefix-cache trade-off of
  §2.3 be *verified* rather than reasoned about); impersonation (`Ctrl+U`) still
  builds its own full-history request. Next: **stage 3** — the
  `history_read`/`history_search` read-back tools (fork F9b), after which the
  summary block stops saying the verbatim text is unreachable and names them.

### Post-M9: history compression — stage 3 (the read-back tools) (done)

- **Completes the track** (research
  [docs/research/history-compression.md](../../docs/research/history-compression.md);
  track-level fork **F9(b)** decided 2026-08-07, **sub-decisions S9–S16 recorded
  before implementation and S11/S12/S13/S14 confirmed by the user 2026-08-08**,
  all as recommended). Behaviour — spec §6.7. Branch `feat/history-readback`.
  Stage 1 made a conversation foldable and stage 2 made it fold by itself; both
  left the same honest admission in the summary block — *the verbatim text of
  those messages is not available to you*. This removes it.
- **Reading the code first turned the stage's one line into a different stage.**
  F9(b) had specified the `attachment_read`/`attachment_search` shape, i.e. a
  chat-scoped **embedding** index. But `cache.db` — the full-text index behind
  `Ctrl+F` — already covers every message of every chat, and, checked rather than
  assumed, `search::indexed_messages` filters only on *empty text*: **`Tool`-role
  messages are indexed too, at full length**. That is §1.3's invisible bulk,
  already searchable, already kept in step by the post-save hook, and reachable
  per chat through `CacheDb::matching_messages_in_chat`.
- **S11 — so search is full-text, not semantic**, and the decisive argument is
  the audience rather than the cost: an embedding server is a **separate** server
  (ADR 0002) and is routinely unconfigured, while the local user with an 8k
  window is exactly who this track exists for — a search that needed an embedder
  would be absent precisely where it is needed. It also costs no new table, no
  indexing task, no generation-stamping and no `/reindex` integration. And the
  complementarity runs the *opposite* way from how it first looks: the **summary
  is already the semantic view** of that range; what it loses is verbatim
  detail, which is what lexical search is best at — trigram matches substrings,
  so `8823` finds `ZARYA-8823`. The embedding variant is recorded as groundwork
  if a live run ever shows lexical misses.
- **S12 — the tools are offered only while the chat has a folded range**, a
  special case in `effective_tool_ids` exactly like the one `get/set_sampling`
  already has. Two schemas cost prompt on **every** turn, and this feature's
  audience is people fighting a ceiling. More importantly it earns an invariant:
  the same `compaction_view` decides the tool set *and* whether the block is in
  the prompt, so the block can name the tools without ever promising an absent
  one — which is why there is **one** block wording rather than the
  with-tools/without-tools pair attachments need (S15).
- **S9/S10/S13/S14, briefly**: the folded range is an `Arc<HistoryView>` **turn
  snapshot** on `ToolContext`, built where `attachments` already is — the
  orchestrator stays the sole owner of `Chat`, there is no I/O on the tool path,
  and a roll landing mid-turn cannot change what this turn's tools describe;
  pages are token-budgeted slices cut by `entities::attachment::paginate`, so
  `history_read` inherits the guarantee `attachment_read` exists for (walk
  `1..M` and *know* you read everything) and both readers cut text the same way;
  only the folded range is readable, since the verbatim tail is already in the
  prompt; and the page size is its own `compaction.page_tokens` (default 800,
  smaller than the attachment page — this reader serves conversations already
  pressing against their window).
- **One renderer, parameterized by the clip.** `features/compaction.rs` now
  renders a message range once, and the tool-result budget is a parameter: the
  digest clips to `TOOL_RESULT_CLIP`, the reader does not clip at all — paging
  back to a `fetch_url` result only to receive the same 200 characters the
  summary already carried would defeat the point. Sharing the renderer makes
  "what was summarized is what can be re-read" true by construction.
- **The detail that makes search work at all**: a tool result is rendered
  *inside* the assistant block whose call produced it, so the `Tool` message has
  no block of its own — but it has its own row in the index and is the likeliest
  hit. `HistoryView` therefore keeps an alias `Tool` message id → that block, or
  a hit on the bulk this feature is most about would map to no page.
- **A search hit carries its page number** (S16), which is what makes the pair
  compose the way the attachment pair does — search says *where*, reading
  guarantees *everything*. Escaping stays in its one home
  (`features::chat_search::to_fts_query`): raw input cannot reach `MATCH`, since
  `C++`, `cost-benefit` and `50%` are all FTS5 syntax errors on ordinary text.
- **A hole in my own S15 reasoning, found by re-reading it after it was
  written.** S15 argued for a *single* block wording because block and tools are
  gated on the same `compaction_view` — true of the chat-level gate, but a
  **profile** can switch the two tools off, and then the block would name tools
  the model does not have: precisely the dead end the sentence exists to
  prevent. So the wording follows the turn's real tool set after all
  (`compaction.block.tools` / `…no_tools`), exactly as an attachment's entry only
  offers `attachment_search` for a file that has an index. The invariant S15
  wanted survives in the form that matters — the block never names an absent
  tool — it just needed one more input to hold.
- That input pushed `build_request` to eight parameters, so its injection inputs
  moved into a `PromptContext` (the `ToolDeps`/`ToolParams` pattern): the next
  thing the system prompt is assembled from will not lengthen the signature
  again.
- **Tests**: the view (walking `1..M` reassembles the transcript; `locate` maps
  a message — **including a `Tool` one** — to its page and block; the reader sees
  what the digest had to clip; conversation order; blocks separated); the tools
  (pages walked and a bad one reporting the real count; both tools explaining a
  conversation with nothing folded; search finding a **tool result** and naming
  its page; a substring of an identifier; a match in the verbatim tail correctly
  **not** surfacing; every refusal pointing at `history_read`); the visibility
  gate; and end to end through a real turn — the tools absent before a compaction
  and present after it, travelling with the block that names them. **1942 unit
  tests green** (+21), **80 `#[ignore]`** (+1), clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **Six mutations, five caught first time — and the sixth is the useful one.**
  Dropping the `Tool` alias, clipping in the reader, letting tail hits through,
  dropping the visibility gate and hardcoding "a range exists" each failed their
  own test. But **bypassing the FTS escaping survived**: my punctuation test
  asserted `is_ok()`, and the tool *deliberately degrades* an index failure into
  a normal answer — so an unescaped query would never run while the test passed.
  Rewritten to assert *which* answer comes back (not the "index unavailable"
  one), it now fails under the mutation. Worth remembering as a shape: on a code
  path built to degrade gracefully, `is_ok()` can never be the assertion.
- **Live run — GO** (Gemma 4 31B q4_0, external `llama-server`, `--jinja`):
  `history_read_back_answers_what_the_summary_dropped_live` — the model called
  `history_search` twice and answered with a code that existed nowhere but in
  messages no longer being sent. That is the half unit tests cannot reach: they
  prove the tools return the right page, not that a real model reaches for one
  instead of guessing.
- **The fixture took four attempts, each failing its own precondition rather
  than the feature**, and the last is the most instructive. (1) Fifteen
  item→code pairs survived a 250-word summary whole — and the model filed them
  with `note_save` first, so it could have answered from memory; the smoke now
  enables **only** the two read-back tools, the same "remove the alternative
  rather than hope it is not taken" move `spawn_orch_live_no_embed` makes. (2)
  With the target the only *named* entry among "item number N", the summary
  dropped the rest and kept exactly the one that had to be lost. (3) Sixty
  entries as adjective × noun were **grouped by adjective** and all sixty codes
  still fitted — any structure is compressible, so the fixture went to 200
  entries against a 120-word limit, which is arithmetic rather than a hope about
  the model's judgement. (4) **The control question was itself the confound**:
  asked about the target and landing inside the folded range, it made that entry
  the most salient thing in the range, so the summary kept precisely what the
  test needed dropped — twice in a row, which is what showed it was not chance.
  The control now asks about a **different** entry. Note the mirror image in
  stage 1's smoke, where the control *helps* because that test wants the fact
  kept: the same device is an aid or a confound depending on which way the
  assertion runs.
- **Regression — clean**: the full orchestrator e2e live set on the same stack,
  **29 passed / 0 failed** (821 s). The right scope, since `effective_tool_ids`,
  the `TurnInfo` snapshot and `build_request` all sit on **every** turn. The
  whole `#[ignore]` suite was then run as well — **80 passed / 0 failed**
  (913 s), with the cloud, alternate-embedder and managed-server smokes skipping
  on their env gates.
- **The Sonar gate failed on new-code duplication (6.5% against 3%), and two of
  the three blocks it flagged were worth fixing on their own account.** The new
  live smoke had copied the stage-1 smoke's "filler turns → `/compact` → unwrap
  the event" tail verbatim (now `fill_then_compact`, with only the topics as a
  parameter), and the request tests repeated the whole `PromptContext` literal at
  every call site (now `request_of`). That took it to 3.85%.
- **The rest was a real duplication I had first written off as unavoidable.**
  `attachment_search` and `history_search` take the same two arguments for the
  same reason — one names *where* to look, a reader then fetches it — and each
  spelled the contract out twice: once as a JSON schema, once as the parsing in
  `invoke`. One contract, written four times. It is now `search_parameters` +
  `search_args` in `features/tools/mod.rs`, the rule this codebase already
  applies to the FTS escaper. Duplication on new code: **0.0%**, better than the
  ~1% predicted — extracting the schema also pulled the two `impl Tool` heads
  apart, so their 16-line match stopped reaching the detection threshold.
- **A trap on the way**, and the i18n gates caught it immediately: the first
  version built the bundle keys from a prefix (`format!("{prefix}.param.query")`),
  which makes them **invisible to the key scanner** — four keys read as dead and
  the template as a key that does not exist. That is exactly why the dynamic
  `ui.tool.label.*` family carries a gate test of its own. The keys are passed
  whole instead, which also shows in each tool's own file which text it presents.
- **Groundwork**: a semantic index over the folded range (S11's rejected
  variant, if lexical misses ever show up live); impersonation still builds its
  own full-history request; and the block, the tool set and the reader are now
  three consumers of one `compaction_view`, which is the seam any future
  per-chat tool gating would reuse.

### Post-M9: impersonation sends the compacted conversation (done)

- **The last leftover of the history-compression track with a real user
  effect**, and the framing is what made it worth doing: `Ctrl+U` builds its own
  request ([impersonation.rs](../../src/app/orchestrator/impersonation.rs), spec
  §11.8) and took `chat.messages` **whole**, so a long chat hit the very context
  ceiling the whole track exists to remove — only from a different key. Branch
  `fix/impersonation-compaction`. A simple task by AGENTS.md §1 (one module, no
  cross-layer contract, no new dependency), so no design doc; the one genuine
  unknown was measured first, below.
- **~20 lines of production code**, because the pieces already existed: the view
  is `Chat::compaction_view(config.compaction.enabled)` — the same call
  `generation.rs` makes — and the block comes from the shared
  `request::inject_compaction`, so there is **one** wording in one place. Passed
  as the pair the view already returns (`Option<(&str, usize)>`) rather than two
  parameters, so a summary can never arrive without the cut it describes.
  Ordering follows `build_request`'s volatility rule: persona → summary → the
  interlocutor model → the seed continuation, which stays last as the immediate
  instruction.
- **The block is injected with `tools = false`, and that falls out of an existing
  decision rather than needing a new one**: impersonation carries no tools, so
  sub-decision S12's gate produces the "work from this summary" wording instead
  of pointing at `history_read`/`history_search` the model cannot call. The dead
  end this project has now closed four times would otherwise have reopened here.
- **The one unknown was settled live before any code** (three real providers,
  our own clients rather than curl — the journal's corrupted-`awk` lesson): a cut
  always lands on a `User` message, and the swap turns it into a **leading
  assistant turn**, which impersonation had never produced. **Anthropic
  (claude-haiku-4-5), native Gemini (gemini-3.5-flash) and OpenAI Responses
  (gpt-5.6) all accept it** — no 400, and all three wrote a clean one-line user
  message. So no guard was needed and the estimate stayed at ~20 lines.
- **Two of my own probe runs were wrong, and both would have produced a false
  conclusion.** The first ended the history on an **assistant** turn: Anthropic
  and Gemini returned empty text with `Stop` — not a rejection but a **prefill**,
  since a trailing assistant turn is continued rather than answered. Real
  impersonation never sends that shape (the chat ends with the assistant's reply,
  which swaps to `user`), so the fixture was simply wrong. The second used
  `max_tokens: 64` and produced empty replies from the two 3.x models — plausibly
  the shape, actually the cap (thinking tokens count against it, already recorded
  here for Responses); at 512 all six answered. The control arm ("leading user",
  today's shape) is what made both diagnosable at all.
- **That prefill rule is now a test, not a comment**
  (`compacted_impersonation_starts_with_assistant_and_ends_with_user`): the head
  is fine and the **tail** is the fragile half, so a future change to the cut that
  left a trailing assistant turn would fail here instead of silently producing an
  empty preview.
- **Tests**: 4 unit (the folded prefix is not sent while the summary is, the
  no-tools wording with neither tool named, the switch off giving byte-for-byte
  the previous request while the stored summary stays dormant, and the role
  invariant) + 1 integration through the real `run` loop — placed in
  `tests/compaction.rs`, since driving a real roll needs that module's fixtures,
  and it is the only test that can catch the one-line wiring in
  `handle_impersonate`. **All four load-bearing behaviours mutation-tested**:
  reverting the wiring, the slice, the injection, or `tools=false` each fails its
  own test — the last one exactly one test, which is the precision worth having.
  **1947 unit tests green** (+5), **81 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Live run — GO** (Gemma 4 31B q4_0, external `llama-server`, `--jinja`):
  `impersonation_after_compaction_still_writes_live` — 4 messages folded into a
  229-character summary, and the impersonated message came back as a coherent
  user-style follow-up on the conversation's last topic. The assertion is
  deliberately just "non-empty": both ways this can fail — a refused leading role,
  or a tail read as a prefill — end in the same silence rather than an error.
- **Regression — clean**: the three live compaction smokes and the read-back one
  green (127 s + 102 s). Deliberately not the full orchestrator e2e set: unlike
  the track's own stages this touches **no** shared turn path — `generation.rs`,
  `build_request` and `inject_compaction` are untouched, and the blast radius is
  the one function `Ctrl+U` calls.
- **A note the scanner earned**: `cyrillic_scan.py` flagged a *comment* quoting a
  Cyrillic fixture value in a test file. Correct — the wholesale test allowlist
  covers fixture data in code position, not prose; the comment was reworded to
  describe the index instead of quoting it.

### Post-M9: Grok (xAI) as a cloud provider (done)

**Task**: add xAI's Grok models as a first-class engine mode (the user had a
console.x.ai account and a key). Research first —
[docs/research/grok-xai-provider.md](../research/grok-xai-provider.md), written
against the **live** API rather than the docs, because xAI documents almost none
of the per-model parameter rules.

- **The finding that shaped the work**: xAI's plain `/v1/chat/completions`
  streams reasoning in `delta.reasoning_content` — byte-for-byte the field
  `OpenAiClient` has parsed since it was written for llama.cpp — and accepts a
  replayed assistant turn with `tool_calls` and **no** thinking signature. Every
  other cloud needed a client of its own for exactly those two reasons
  (Anthropic's signed thinking block, Responses' reasoning item with
  `encrypted_content`, Gemini 3's per-call `thoughtSignature`). Grok needed
  none: no new wire, no contract change, no persisted field, no migration.
  `cloud_chat_setup` hands it an `OpenAiClient` and the existing accumulator,
  thoughts parser and token accounting all work unchanged.
- **The one trap that would have shipped broken**: the orchestrator sets
  `reasoning_effort: "none"` on its auxiliary turns — title generation,
  compaction and impersonation all do it deliberately, and llama.cpp reads it as
  "don't think". xAI rejects the *value* (`400 This model does not support
  reasoning_effort value none`). Ordinary chat would have worked while those
  three failed — the miserable kind of partial breakage. Fix:
  `OpenAiClient::with_effort_none_omitted(true)`, set only for Grok, which drops
  the field instead of sending it; the Anthropic and Gemini wires already map
  `None => None` the same way. A live smoke pins it.
- **Sampling, measured rather than assumed**: a `200` proves nothing on xAI —
  unknown fields are silently ignored (`{"totally_bogus_field":1}` → 200). Sending
  a **wrong type** separates the cases: a field in xAI's schema fails
  deserialization (422), an unknown one is dropped. That gives
  `supported_sampling_fields(Grok)` = `temperature`/`top_p`/`max_tokens`/`seed`
  + reasoning. `top_k`/`min_p`/`repeat_penalty` are *not* in the schema (offering
  them would be a lie), and `presence_penalty`/`frequency_penalty` are a hard
  `400` on grok-4.5/4.3/4.20 (offering them would break the request). Worth
  recording: `stop` also `400`s — the project's anti-self-cutoff invariant, which
  exists for an unrelated reason, is what keeps that from ever being hit.
- **No embeddings at xAI** (`/v1/embedding-models` returns an empty list), so
  `apply_embed` folds Grok into the `Claude` arm — RAG needs a separate embedder.
- **Rejected**: binding to xAI's `/v1/responses` instead. It works — our exact
  `ResponsesClient` payload returns 200 with the same event names and
  `encrypted_content` shape — but `ResponsesClient` sends no
  `temperature`/`top_p`/`seed`, and what it buys in exchange (stored responses,
  `/responses/compact`) is dead weight for a client that keeps its own history and
  compacts it itself. The Anthropic-compatible `/v1/messages` surface works too
  (its `thinking` blocks come back with an **empty** `signature`). Both stay cheap
  options if xAI-only features ever matter.
- **Small refactor taken along**: `cloud_ref`/`cloud_mut` took one
  `&CloudSettings` per provider positionally; a fourth would have made them
  six-argument functions and a fifth eight. They now take a
  `CloudProvider::ALL`-ordered array indexed by `CloudProvider::index`, with a
  test asserting each provider sits at its own index — a wrong slot would hand
  back another provider's key and model name with no type error to catch it.
- **Live run** (`MINDFORK_GROK_KEY=… cargo test grok_smoke -- --ignored`): three
  smokes green against `api.x.ai` — reasoning deltas arrive, a tool result
  replays without a signature, and `effort=none` no longer fails. 1962 unit tests
  green.

### Post-M9: engine failures stop being silent (stage 1 of retry/backoff) (done)
- **Context**: stage 1 of the retry/backoff track
  ([docs/research/cloud-retry-backoff.md](../research/cloud-retry-backoff.md), forks
  settled 2026-08-12). Reading the code for that research found the roadmap's
  one-line item ("clients surface the error body but don't retry") was really
  **three** defects, and two of them were invisible. This stage fixes those two and
  builds the plumbing the retry decorator needs; the retry itself is stage 2.
- **Defect 1 — a mid-stream failure was silent truncation.** Once the HTTP response
  arrived, nothing could reach the user: a failure yielded
  `ChatChunk::Finished(FinishReason::Error)`, and `finish_generation` pushes a feed
  note only for `Cancelled`. So the partial reply was kept, persisted, and left on
  screen with *nothing* saying it was a fragment. Worse for **Anthropic**: its
  documented in-stream `error` event (an `overloaded_error` there is the same
  condition as a pre-stream `529`) fell into `AntStreamEvent::Other`, the connection
  then closed, and the `None` arm reported `Finished(`**`Stop`**`)` — an overloaded
  truncation was byte-for-byte a normal completion. **Gemini** was the same shape by
  a different route: every field of `GenResponse` is `#[serde(default)]`, so an error
  payload deserialized as a perfectly valid empty chunk. llama.cpp's
  error-object-inside-a-200-stream (ggml-org/llama.cpp#14566) failed chunk parsing and
  was skipped with a log line. Four providers, four ways to end a broken turn as a
  finished one.
- **Defect 2 — a hung connection could not be escaped.** No engine client set any
  `reqwest` timeout (`Client::new()` has none at all), and the initial `send()` sat
  **outside** the `select!` on the cancel token, so cancellation only took effect once
  SSE events were flowing. A host that accepts nothing (firewall drop, dead VPN, wrong
  port) left the turn in `Generating` forever; `Esc` moved it to `Cancelling` and there
  it stayed. The TTS and video clients had both halves right already — the engine
  clients were the outlier.
- **Defect 3 — the status was thrown away** (the retry blocker). All four clients ended
  a non-2xx with `bail!("… status {status}: {body}")`, so the code and any
  `Retry-After` survived only as substrings of a sentence. And a *transport* failure
  reached the feed as a **bare URL**: the call sites wrapped it in
  `.with_context(|| format!("POST {url}"))` and `anyhow`'s `Display` prints only the
  outermost context, so "connection refused" was never shown at all.
- **What shipped**:
  - `shared/api/error.rs` — `EngineError { kind, status, retry_after, message }`
    (`thiserror`, satisfying the CLAUDE.md convention that `shared` uses it).
    `Display` is the message, rendered at construction, so the text is
    **byte-for-byte** what the four `bail!`s produced — `OVERFLOW_MARKERS`, the locale
    strings and the tests reading them are untouched, and a test pins it. One
    `check_status` helper replaces four copies of the same block. `retry_after` reads
    all three forms providers actually use: the seconds header (OpenAI, Anthropic),
    OpenAI's `retry-after-ms`, and — Gemini having no header — the undocumented
    `google.rpc.RetryInfo.retryDelay` in the 429 body. Nothing consumes it yet; stage 2
    does.
  - `shared/api/http.rs` — one `engine_client()` with a **connect** timeout of 10 s
    (deliberately *only* connect: an SSE stream lives for minutes and a local prefill
    can be silent for tens of seconds, so a total or idle timeout would cut healthy
    generations — which is why none was ever set), and `send_cancellable`, which puts
    the initial POST inside the token's reach. Cancellation there answers with
    `cancelled_stream()`, never an error, so it cannot read as a failed turn (nor, in
    stage 2, be retried).
  - `ChatChunk::Error { message, transient }` — the stream's error channel, since a
    post-response failure cannot be returned as `Err`. Every client yields it
    immediately before `Finished(Error)`, so a consumer watching only `Finished`
    behaves exactly as before. Adding the variant made the compiler enumerate all
    **seven** consumers, which is the reason it is a variant rather than a side channel.
  - The orchestrator finally says so: one `engine_error_note`/`engine_error_key` pair
    serves **both** failure paths (pre-stream `Err` and in-stream `Error`), so the two
    can never drift. A fourth answer, `ui.err.generation_interrupted`, fires when text
    or "thoughts" already reached the screen — that reply is a *fragment*, and saying
    only "generation error" leaves the user guessing whether what they can see is the
    whole answer (lessons §4). Overflow still wins over it: pointing at `/compact`
    beats "press `Ctrl+R`", which would just overflow again.
  - Background turns log the reason instead (the failure streak already reports them),
    with one exception: a **compaction roll** carries it out as an error, because
    `/compact` was just typed and is owed an answer — otherwise a roll killed
    mid-stream reported whatever fragment arrived as if it were a summary.
- **`transient` classification** lives in exactly two places: `RETRYABLE_STATUSES`
  (`408/429/500/502/503/504/529` — the set converges across all five providers) and
  `TRANSIENT_ERROR_NAMES` for failures with no status line left to read. `400` is
  deliberately absent: llama.cpp's `exceed_context_size_error` is compaction's job, not
  a retry's. Name matching is **exact**, so a code that merely ends in `_error` cannot
  be read as transient.
- **Mutation-tested, six mutations, all caught** — after one initially survived and one
  exposed a bad test:
  - dropping the mid-stream note; adding `400` to the retryable set; removing `429`
    from it; collapsing the `interrupted` branch; putting the Anthropic `error` event
    back into `Other` — each turns a test red.
  - **The survivor**: loosening exact name matching to `contains` changed nothing,
    because no real error name contains another as a substring — the *fixture* could
    not discriminate, so the exactness was untested rather than wrong. Fixed by
    asserting the function's input contract instead: hand it the composed text from
    `stream_error_text` (the shape a call site could plausibly confuse it with) and it
    must answer "not transient" — failing closed on a misread input, and the only case
    that makes the exactness observable at all.
  - **The bad test**: dropping the note made the orchestrator test **hang** rather than
    fail, because the shared `wait_for` blocks until the event channel closes. It now
    uses a bounded wrapper — a hang in CI reads as broken infrastructure, not a broken
    promise.
- **Tests**: 2011 unit (from 1977), 86 `#[ignore]`. Two of the new ones drive the
  Anthropic client through a **real socket and a real SSE body** rather than serde
  alone — which is precisely how the swallowed `error` event survived: the type parsed
  fine, the client dropped it.
- **Smoke — GO** (2026-08-12, reference stack `gemma-4-31B_q4_0-it` + `bge-m3`, and all
  four clouds with real keys).
  - **New live smoke** `an_oversized_prompt_is_a_typed_non_transient_error` — GO on both
    stacks: a real `400` arrives as `status=Some(400)`, `transient=false`, body intact
    (`exceed_context_size_error … "n_ctx":16384`), so the overflow advice still fires.
  - `OpenAiClient` **7/7**, OpenAI Responses **3/3**, Gemini **4/4**, Grok/xAI **3/3**,
    Anthropic **3/3**, orchestrator e2e **30/30** (750 s).
  - Two things the run cost, both worth recording. **First**, the *first* e2e run was
    27/30 — `attachment_read`, `history_read_back…` and `impersonation_after_compaction…`
    failed, then passed individually and the whole set came back 30/30 on a clean rerun:
    flakes, and the reason the project's rule is to distrust a single run. **Second**,
    a genuinely wrong-premise smoke was found and fixed:
    `extended_thinking_streams_thoughts_and_signature` asserted that
    `display:"summarized"` yields non-empty "thoughts". Measured on `claude-opus-4-8`
    (five runs, clean streams): `thoughts=0, signature=380, text=50`. A signature that
    long only exists for a real thinking block, so thinking *did* run — Anthropic
    summarized a short one to nothing. It now asserts the signature (empty the moment
    `thinking` leaves the request — verified by a live mutation) and logs the summary's
    absence, matching the OpenAI Responses sibling that already said "on a trivial task
    the summary might be absent".
  - Diagnosing that turned up the same defect this branch fixes **inside the smokes'
    own harness**: their collect loops swallowed `ChatChunk::Error` in a `_ => {}` arm,
    so the failure read as "empty thoughts" with no word of an engine error. Every cloud
    smoke now prints it — and the silence of that line is what proved the Anthropic
    stream was clean rather than failing.

### Post-M9: retry with backoff on transient cloud failures (stage 2, track complete) (done)
- **What**: a transient provider failure (`429`, `5xx`, Anthropic's `529`
  `overloaded_error`, a dropped connection) is retried automatically instead of ending
  the turn. Stage 2 — and the last — of
  [docs/research/cloud-retry-backoff.md](../research/cloud-retry-backoff.md); stage 1
  made those failures visible and typed, which is what made this implementable at all.
- **Shape: a decorator, not a change to the clients** (fork F1(a)). `RetryBackend`
  wraps an `EngineBackend`, so one implementation serves all five providers and every
  call site, the clients stay ignorant of policy, and the whole thing is testable
  against a scripted inner backend with **no network**. Applied to **cloud and
  external** only (F2(a)): a managed child that died reloads for minutes, and there the
  health monitor plus `RestartBudget` are the honest recovery mechanism — a
  three-attempt burst against a loading server is noise. External is wrapped *because*
  it is usually cloud-shaped: that URL is as often a proxy or gateway (LiteLLM,
  OpenRouter) as a local server, and a genuinely local one loses nothing since its
  `503 Loading model` is pre-stream and rides out in a single wait.
- **The safety property, and how it is enforced.** A retry is allowed only while the
  turn is *uncommitted* — nothing but `Usage` has been yielded (F3(a)). The moment
  `Text`/`Thoughts`/`ThoughtsSignature`/`ToolCall` appears the stream is handed over
  untouched. Because **a tool call is content**, a round that emitted calls can never be
  replayed, so no tool effect can fire twice: the property falls out of the commit rule
  rather than needing a rule of its own. `Usage` deliberately does *not* commit —
  Anthropic reports it in `message_start`, before a single token, so treating it as
  content would have made every Anthropic turn unretryable. Retrying happens at this one
  layer (the SRE single-layer rule): the orchestrator and the agentic loop re-issue
  nothing.
- **Why attempt 1 runs eagerly.** `chat_stream` awaits the first attempt before
  returning, so a failure that will *not* be retried keeps exactly the shape stage 1
  gave it — an `Err` for a pre-stream failure, an `Error` chunk for an in-stream one.
  That matters beyond tidiness: `title.rs` turns an `Err` into "title generation failed:
  <reason>" but an empty reply into a bare "title is empty", so a decorator that
  converted every failure into chunks would have quietly degraded that message. Only
  once a wait is unavoidable is the stream returned, with the remaining attempts running
  inside it — which is the only way a chip can be shown *during* the wait rather than
  after it.
- **Policy** (F4, constants — F6(a)): `MAX_ATTEMPTS=3` (both first-party SDKs' default),
  `1 s → 2 s` (`BASE_DELAY`×`BACKOFF_FACTOR`), jitter **downward** by ≤25% (the
  openai-python shape: spreading retries earlier cannot push a client past a deadline it
  was told about), `Retry-After` honoured **as given** up to `RETRY_AFTER_CAP=30 s` and
  treated as non-retryable beyond it. Jitter reads the wall clock rather than adding a
  `rand` dependency — the requirement is only that many clients not pick the same
  instant, and it keeps working under `tokio::time::pause`, which freezes timers but not
  `SystemTime`. Quota-vs-rate `429` discrimination stayed out (F5(a)): two short waits
  cost seconds, and a rejected request is not billed.
- **The wait is visible and interruptible** (F7(a)): `ChatChunk::Retry{attempt, max,
  delay}` → `AppEvent::Retrying` → a quiet status-bar chip ("retrying 2/3 in 2 s"),
  composed into the same `·`-separated strip as the background tasks and cleared by
  whatever ends the wait — content or the turn finishing — so it cannot outlive what it
  describes. The backoff sleeps inside a `select!` on the turn's `CancellationToken`,
  so `Esc` ends the turn at once instead of after the wait (the `managed.rs` readiness
  poll's shape).
- **Tests**: 2026 unit (from 2011), 86 `#[ignore]`. Thirteen for the decorator, all
  hermetic and instant under `start_paused` virtual time; the load-bearing assertion
  throughout is the inner backend's **call count**, because "retried" and "did not
  retry" are indistinguishable from the chunks alone when the outcome matches.
  **Seven mutations, all caught**: making content stop committing the turn (the tool-replay
  hazard), ignoring the `Retry-After` cap, making a permanent failure retryable, an
  off-by-one budget that allows a fourth attempt, dropping cancellation from the backoff
  sleep, forgetting to delegate `context_budget`, and not clearing the chip on content.
  The `context_budget` one guards a real trap: it is how auto-compaction learns the
  window (spec §6.7), so a decorator that answered `None` would have silently switched
  the automatic trigger off for every wrapped backend.
- **Smoke — GO** (2026-08-12, reference stack `gemma-4-31B_q4_0-it` + `bge-m3`, and all
  four clouds with real keys). Nothing here can *force* a real `429`, so the live gate's
  job was non-regression of every path now that each one runs through the decorator:
  `OpenAiClient` 7/7, OpenAI Responses 3/3, Gemini 4/4, Grok 3/3, Anthropic 3/3, and the
  orchestrator e2e set **30/30**, run twice — once before wrapping and once after
  (**786 s → 771 s**, so the eager first attempt costs no measurable latency; zero
  retries fired against a healthy stack, which is the other half of the claim).
- **The plan's own testing gap, found by checking rather than assuming.** §8 of the
  research called for live runs as "non-regression of every provider path **through the
  decorator**" — and the live e2e harness builds its backend directly via
  `MockSupervisor`, so the wrapping was covered by unit tests only. A decorator that
  mishandled the head it replays, the commit point, or the delegated `context_budget`
  would never have met a real model. Fixed by wrapping the harness's `live_backend()` the
  same way the supervisor wraps an external server, which puts all 30 e2e tests through
  it on every future run — and is what the 771 s figure above measures.

### Post-M9: images in a message — stage 1 (local + Grok) (done)

**What.** `/image attach <path>` · `/image remove <name|#N>` · `/image list`: an image
is staged for the **next** message and reaches a vision-capable model as a content part.
Stage 1 covers the local `llama-server` (`--mmproj`) and xAI Grok — one wire change
serves both, since their request shapes are byte-identical. The three remaining cloud
formats are stage 2. Research, live measurements and the eight forks:
[docs/research/multimodal-images.md](../research/multimodal-images.md); behaviour —
spec §9.10.

**Why this shape.** The obvious move was to copy `/file` wholesale, and it is wrong.
An attachment is chat-scoped and lives in the **system prompt**; an image cannot,
because every provider takes images as *message* content parts only. Fork F1 therefore
made staging message-scoped, and the measurement backs it: replaying an image turn keeps
llama.cpp's prefix cache (`cache_n = 73` of 78, an appended turn prefills 23), whereas a
chat-scoped set would rewrite the head of the prompt on every attach and remove.

**Key decisions** (all eight forks confirmed by the user on 2026-08-12):

- **The payload lives base64 in the chat file** (F2), like an attachment's text
  snapshot: self-contained backup/export, no I/O while building a request. Affordable
  only because of the next point.
- **Preparation at attach time** (F3, the `image` crate, trimmed features): downscale to
  1568 px long edge and normalize to png/jpeg. Both halves were load-bearing rather than
  polish — xAI accepts *only* jpg/png, so an un-normalized webp would attach fine and
  fail at send on exactly one provider; and an unscaled phone photo rides every
  subsequent turn. An image already png/jpeg and already within the ceiling is passed
  through **byte for byte**, so a screenshot keeps its exact pixels.
- **Capability is asked, not guessed** (F4). `EngineBackend::vision()` reads
  llama.cpp's `/props` → `modalities.vision` — the same fetch `context_budget` already
  makes — and the clouds answer statically. Deliberately no model-name allowlist: that is
  the trap the Grok research recorded for reasoning detection. `Unknown` (any server
  without `/props` — vLLM, LM Studio, a proxy) attaches **optimistically** with a neutral
  note; refusing there would break every user running a vision model behind a plain
  OpenAI-compatible endpoint.
- **A no-image request is byte-identical to before.** `WireMessage.content` widened to an
  untagged `Text | Parts`, and parts are emitted **only** when an image is present. This
  is pinned by its own test: llama.cpp reuses the prefix cache on a matching rendered
  prompt, and Gemma's template routes string content and a parts array through different
  branches — a blanket switch would have silently re-prefilled every stored conversation
  and changed every text-only turn on every provider.
- **A per-image label part** (F5, `Image #1 — "chart.png":`, axis A) — Anthropic
  documents labelling, and it is what lets a model name the file the user sees in
  `/image list`.
- **Staging is session-only.** What was staged and never sent is a half-finished thought;
  persisting it would resurrect images into a message written days later. It is consumed
  by the send and by nothing else, after every early return, so a failed send leaves it
  intact.

**Measured live before any code** (2026-08-12, `gemma-4-31B_q4_0-it` + its mmproj,
llama.cpp b10322, `-c 16384`): single-image, two-image, composition and history-replay
questions all answered correctly; a 3000x2000 / 1.5 MB png is resized server-side rather
than refused; image tokens are included in `usage.prompt_tokens` (+51 for 128x128, ~+258
from a megapixel up), so the auto-compaction trigger needed no change at all. One
real-key probe per cloud pinned the other three wire shapes for stage 2. A first probe
round returned four empty answers and was a fixture bug, not a finding — `max_tokens: 64`
starves a reasoning model before it emits any content (docs/lessons.md §3).

**Tests.** Unit coverage across the slice: the entity (patch-formula estimate, `#N`
resolution, additive serde), `image_prepare` (byte-for-byte pass-through, downscale
arithmetic, alpha forcing png, a photographic source shrinking as jpeg, undecodable and
empty input), the parser (`/image` vs `/file` vs `/images`, quoted paths, `delete` is not
a verb, plus the all-locales gate), the wire (the byte-identical guarantee, part order,
several images, no empty trailing text part), and seven orchestrator integration tests
(staged image reaches the model on the message and not in the system prompt, persists,
is consumed by the send and not repeated, unstaging, idempotent restage, the count cap's
message naming both ways out, an image-only message still sending). Live:
`image_attachment_e2e_live` — two turns, so history replay is proven and not assumed.

### Post-M9: images in a message — stage 2 (the three cloud formats, track complete) (done)

**What.** The remaining three request builders learn image content parts, so `/image
attach` now reaches every provider the app supports: Anthropic `image` blocks, Gemini
`inline_data` parts, and OpenAI Responses `input_image`. Stage 1 had already covered
llama.cpp and Grok, whose Chat Completions shapes are byte-identical. Track complete.

**Why it was three small changes and not three clients.** The shapes were measured live
before stage 1 was written (research §2.2), so nothing here was discovery — each backend
already had the right seam. Anthropic's content was **already** a tagged block array, so
it gained one variant; Gemini's parts are untyped `Value`, so it gained one `json!`;
Responses builds `input` items as `Value`, so its user item's `content` widened from a
string to an array. No contract change, no new persisted field, no migration.

**The invariant that had to hold four times over.** Each builder emits parts **only** when
a message carries images, and each has its own test asserting a text-only turn is
unchanged — a bare string for Chat Completions and Responses, a single `text` block for
Anthropic, one `text` part for Gemini. Stage 1 established why this matters for llama.cpp
(prefix cache; Gemma's template routes the two content shapes through different branches);
for the clouds the argument is narrower but the cost of getting it wrong is the same, and
a per-backend test is cheaper than reasoning about which providers care.

**Sub-decisions.**

- **Order is uniform across all four backends**: images first, each behind its label part,
  then the user's text. Anthropic documents images-before-text as the better-performing
  order and nobody else cares, so one order everywhere means a prompt behaves the same
  wherever it is sent.
- **An image-only message emits no empty text part** in any format. On Anthropic this is
  not cosmetic — a blank `text` block is a `400`.
- **Base64 inline, not the upload APIs.** Anthropic's Files API and Gemini's Files API
  both exist and both are recommended for large or reused media. Neither is used: the
  payload is already in hand (the chat file stores it), downscaling at attach time keeps
  it far inside Gemini's 20 MB request ceiling, and an upload service would add a second
  lifecycle to manage for no gain at our sizes. Recorded here so it is not re-derived.
- **One shared fixture for the four smokes** (`shared/api::blue_square_png_base64`, the
  prompt and the assertion next to it): the same bytes reach all four providers, so a
  disagreement is the wire format rather than the picture. Generated and geometric for the
  same reason the compaction smokes plant an invented code — no model can answer it from
  pretraining, and two humans describe it identically.

**Smoke — GO** (2026-08-13, real keys, all four green on the first run, one tiny request
each): Anthropic `claude-opus-4-8` — *"The background colour is blue, and there is a white
square in the centre."*; Gemini `gemini-2.5-flash` — *"Blue background, white square."*;
OpenAI `gpt-5.5` (Responses) — *"Blue background, white square."*; xAI `grok-4.5` — *"Blue
background, white square."* The Grok smoke is new too: stage 1 shipped its wire but only
llama.cpp exercised it live, and the same builder serving a cloud is worth its own
assertion. The local `orchestrator::tests::live` set was re-run as well, since the
`ApiImage` constructor introduced here sits on `request::message_to_api`, which is on
every turn.


### Post-M9: images in a message — attach by URL (done)

**What.** `/image attach` now takes a web address as well as a path: the image is
downloaded, prepared and staged exactly as a file is. This closes the smaller of the two
items the multimodality track left open (roadmap, "Feed and chat UI"); the other one —
rendering images in the feed — is untouched. Design and the seven forks, all confirmed
before implementation: [image-url-attach.md](../research/image-url-attach.md).

**The design was decided by reading the staging code first** (lessons §3), and it shrank
the task twice over. `stage_image` already took an `ImageSource` whose only job is to
produce bytes, with the cap check, the `/props` capability probe, the background encode,
the chip and the notes all *below* that seam — so a URL is a third variant, not a third
pipeline. The one genuine addition is that a download is **async** while decoding is
blocking, hence a `Staging` step in front: `Staging::Url` resolves to
`ImageSource::Downloaded` before the blocking half runs, and `Staging::Local` passes
straight through. Every arm is reachable; there is no "cannot happen" branch. The wire
adapters, all four request shapes and the byte-identical-without-images guarantee were not
touched at all — what is staged is an ordinary `MessageImage`.

**Always download; never hand the provider the URL** (fork F1). Four of the five engines
accept a remote URL natively and Gemini does not, so a pass-through would have meant a
second wire shape per provider, a format matrix the user discovers at send time (xAI takes
png/jpeg only, and normalization happens on our side), a URL handed to the provider, and —
the deciding one — a *stored* conversation that breaks when the link dies. Downloading
costs one upload on the first turn and buys uniformity everywhere else.

**The premise the roadmap recorded for this item was wrong.** The deferred note said a
client-side download "drags in SSRF concerns `fetch_url` already had to solve"; `fetch_url`
validates the **scheme and nothing else**, follows redirects with the default policy, and
inspects no address. So there was no guard to reuse, and the question had to be answered
rather than inherited (fork F2). The answer follows *who picks the URL*: this one is typed
by the user, in the same box as `/image attach <path>`, which reads any file on the disk —
so an address filter would defend the user against themselves while breaking the LAN case
this project's own users live in (an image on a NAS or a local dashboard, and the loopback
address the unit tests serve from). What is enforced instead holds no matter who typed it:
`http`/`https` only **re-checked after every redirect**, at most 5 hops followed by hand,
connect and request timeouts, and a hard byte ceiling enforced **on the stream** rather
than only on `Content-Length` — a header that may be absent or a lie. The model-driven
half of the question, where the reasoning inverts, is recorded as its own roadmap item
rather than smuggled in here.

**The refusal names what came back** (fork F6). The bytes are sniffed by the decoder, which
is the truth; the served `Content-Type` is carried along only so that linking the *page*
instead of the image on it — by far the likeliest mistake in this feature — answers "that
address served text/html, not an image … open the image itself" rather than "unsupported
format", which would send the user to re-save a file that was never the problem
(lessons §4). Each failure names its own cause: a 403, a redirect loop and an
over-the-limit image call for three different next actions.

**Tests.** +22 unit (2143 green, 95 `#[ignore]`). The download is covered against a
hand-rolled `TcpListener` stub, the project's idiom — a 200, a redirect chain inside the
cap and one over it, a redirect out of `http`, a 404, an empty body, `text/html` at an
image address, and name derivation from a query string, an empty path and a 200-character
segment. Two size tests, not one: the `Content-Length` refusal and the same body served
**without** the header, because the header-only test passes with the stream cap deleted
(lessons §2 — a gate that passes for the wrong reason). Orchestrator: a URL attach reaches
the staging slot, dedupes against itself, and a page or an error status stages nothing. The
help-popup gate caught a real regression on the way: a longer `ui.help.image_attach`
description pushed the "staged for the *next* message" clause — the whole difference from
`/file` — off the right edge of the commands panel in `ru` (lessons §7), and only in `ru`,
which is the trap exactly as recorded; so the description stayed as it was
and the new argument form went into the key column instead. The credits gate caught the new
direct dependency (`percent-encoding`, already in the graph via `url`).

**Smoke — pending.** `image_url_attachment_e2e_live` serves the fixture from a local
listener (no public URL: a smoke must not depend on someone else's uptime, and loopback is
exactly the case F2 keeps reachable), attaches it by address and asks a real vision model
what it sees — with a **control arm that stages nothing** and must fail to answer, since
the last two image tracks each produced a probe that looked green and was a hallucination.
The reference stack was not up when the work was finished; the run and its outcome belong
here before the PR (AGENTS.md §3).
