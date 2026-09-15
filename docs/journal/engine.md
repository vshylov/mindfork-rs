# Journal — Engine, generation and providers

Inference engine and the client-side agentic loop: the `EngineBackend` contract and its four implementations (llama.cpp/OpenAI-compatible — which also serves the xAI Grok cloud, OpenAI Responses, Anthropic, native Gemini), the managed `llama-server` launcher, readiness probing and health monitoring, sampling, streaming and token accounting, impersonation, history compaction.

**Reference documents for this area:** architecture.md §5–§6, spec.md §3, §6-§8

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (67)

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
- Post-M9: a model split across several GGUF files (managed mode) (done)
- Post-M9: `/continue` — stage 0: the live continuation probe (done)
- Post-M9: `/continue` — stage 1: resuming an interrupted reply in place (done)
- Post-M9: `/continue` — stage 2: the clouds (track complete)
- Post-M9: the model's name in `external` mode — asked of the server, and sent to it (done)
- Post-M9: parallel sub-agents — stage 1, sessions per engine and the slot count (done)
- Post-M9: several reasoning items in one reply — each resent as its own item (done)
- Post-M9: the silent tasks under the app-wide budget — the budget's silent lane (done)
- Post-M9: the silent stream yields to the turn — preemption on the session budget (done)
- Post-M9: the batch a cancel waits for — `-b` on the CPU build's launch line (done)
- Post-M9: a stop gives the window back — the silent task returns at the next landing (done)
- Post-M9: a quit gives the window back too — the fact the loop keeps in the open (done)
- Post-M9: "acted on" by effect — a silent task's window is consumed by a write, not by a round (done)
- Post-M9: the quit waits for the landing — a stop's own path decides, the state rule only past a cap (done)
- Post-M9: the quit's settle hears the roll, and its cap is a setting (done)
- Post-M9: a slow prefill, detected on the fly — the batch a cancel waits for, told to the user (done)
- Post-M9: the roll's timings — the session's coldest prompt, sampled (done)
- Post-M9: the loops' timings — the silent tasks' cold prompts, sampled at the landing (done)
- Post-M9: the roll's usage for the budget — and the estimate it would calibrate (done)
- Post-M9: the title's and impersonation's usage for the budget — and the ratio they would erase (done)
- Post-M9: the page summary's usage — the one kind that under-counts, and a summary that came back empty (done)
- Post-M9: the one-shot requests' samples — impersonation's prompt is the session's largest, and its own twice (done)
- Post-M9: impersonation on Gemma's template — the swapped conversation must open with the assistant's silence (done)
- Post-M9: the engine, downloaded — llama.cpp's backends named, fetched and pointed at (done)
- Post-M9: the engine binary is found, not just typed — an empty field resolves (done)
- Post-M9: `llama remove` — a build comes off disk, and says what that changed (done)
- Post-M9: the turn asks about images once, and stops waiting for the answer (done)
- Post-M9: `external` against a gateway — a thought under a second name, and the rest of the review (done)
- Post-M9: the gateway's remaining measurements — the silent turns are a `400`, not a bill (done)
- Post-M9: a refusal to stop reasoning is answered, not reported (done)
- Post-M9: the endpoint is asked what the model can do (done)
- Post-M9: through a gateway, a tool's image and `/continue` belong to the route — M3 measured (done)
- Post-M9: `/continue` follows the route table through a gateway — and F5 closes on a measurement (done)
- Post-M9: a tool's images reach the model through a gateway — re-homed into a user message (done)
- Post-M9: the thinking switch reaches a gateway in its own field (done)

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

**Smoke — GO** (2026-08-13, reference stack: `llama-server` at 192.168.1.20:8000,
gemma-4-31B q4_0 + mmproj, `/props` → `modalities.vision: true`; embedder on :8001).
`image_url_attachment_e2e_live` serves the fixture from a local listener (no public URL: a
smoke must not depend on someone else's uptime, and loopback is exactly the case F2 keeps
reachable), attaches it **by address** and asks a real vision model what it sees. The
control arm ran first and is what makes the result mean anything — with nothing staged the
model answered *"Please provide the image you are referring to, as it wasn't included in
your message"*, so the question is not answerable without the pixels; with the downloaded
image it answered *"Green background, white square."* The attach reported
`figure.png, 512×512, 5001 bytes, ~361 tok` — the name taken from the URL's path.
The **whole** local live set was re-run rather than just this test, because the shared
`staged()` tail this change extracted now sits on the file and clipboard paths too, and
`message_to_api` is on every turn: **32 passed / 0 failed, 782 s**, `image_attachment_e2e_live`
included.
### Post-M9: a model split across several GGUF files (managed mode) (done)

**The question** (2026-08-23, asked of the code before anything was written): does
managed mode work with a model published as several GGUF files — the case in point
`unsloth/gpt-oss-120b-GGUF` `Q8_0/`, three `gguf-split` parts, because Hugging Face
caps a single file at 50 GB. The answer was **yes, already**: `build_args` passes
`model_path` into `-m` verbatim, and llama.cpp, handed the first part, reads the
rest itself by name out of the same directory. Nothing in the app parses a GGUF, so
there was nothing to teach it. What the reading did turn up is three smaller things,
and this entry is those.

**1. The preflight checked the path, not the model.** `ServerHandle::launch` asks
`Path::is_file()` of the configured path — which passes for part 1 of a model whose
other parts never finished downloading, and passes for part 2 of a model llama.cpp
will refuse outright ("model must be loaded with the first split"). Both then land
in the *early-exit* path: the server starts, dies while loading, and the status bar
says the process exited before it was ready — true, and useless. The same shape as
the `--mmproj` trap (lessons §3): a check that validates the string it was given
while the format the string names implies files nobody looked at. `check_split_model`
now runs after the existing `is_file()`: a name in the `-00001-of-00003.gguf` shape
must be the first part (the error names the file to point at) and every sibling must
be on disk (the error names the missing one). A name not in that shape means a
single-file model and no check at all — the old behaviour, byte for byte.

**2. The parsing lives in `shared/gguf.rs`, not in the launcher.** Two callers on
opposite sides of the dependency graph need the same fact and do not import each
other — the preflight, and `config.rs`'s `active_model_name`. The module is pure
(no filesystem: `Shard::all()` returns the paths, the caller stats them), which is
what makes the numbering rules cheap to test. Deliberately strict: exactly the
fixed five-digit tail `gguf-split` writes, `1 <= index <= total`, a non-empty name
in front of it. A false positive would be the worst outcome available here —
inventing missing parts for a model that has none, and refusing a launch that would
have worked.

**3. The model was named after a file.** `active_model_name` cut the directory and
`.gguf` and stopped there, so the assistant's header (`interface.show_model_name`)
and every message's metadata snapshot recorded `gpt-oss-120b-Q8_0-00001-of-00003`.
The part tail names a file; the model is `gpt-oss-120b-Q8_0`. Both copies of that
derivation (engine and embeddings) now call `gguf::display_name`, which also
retires a duplicated four-liner.

**Rejected: silently loading the first part when a later one is configured.** It
is one `Shard::first()` call away, and it would be the app quietly deciding the
user meant a different file than the one they typed — invisible until it is the
wrong model. The error costs one edit and says exactly which path to write
(lessons §4).

**Not built: raw arguments for the managed server.** Fitting a 120B MoE on a
consumer GPU is `--n-cpu-moe`/`-ot` work, and managed mode has no field for either:
`ManagedConfig::extra_args` exists and `build_args` appends it, but the supervisor
passes it empty. Out of scope here (it is a settings track of its own, and external
mode is the working escape hatch) — recorded in roadmap.md, "Engine and
reliability".

**Tests.** +8 unit (2518 green, 107 `#[ignore]`; 2511 on Linux). Five on the parser
— a split path parsed and rebuilt, a Windows path, a non-ASCII name (the tail check
walks bytes, so a multi-byte name is where a naive `split_at` would panic), the
display name in all its shapes, and one table of near-misses that must **not** parse
(`-1-of-3`, `_of_`, `00000`, an out-of-range index, a bare tail, a non-`.gguf`
extension) — that last one being the actual risk, per §2 above. Three on the
launcher, over a `tempfile` directory: a missing part is named; with every part
present the launch gets past the preflight and fails on `spawn` instead (a negative
control — without it the test would pass with the whole check deleted, lessons §2);
and a later part points at the first.

### Post-M9: `/continue` — stage 0: the live continuation probe (done)

**The question** (2026-08-27, research doc `docs/research/continue-generation.md` §7;
the forks were settled the same day, every one at its recommended option): does the
mechanism `/continue` would ride — a trailing assistant message continued in place
(assistant prefill / continue-final-message) — actually work on the providers the
design claims, and how does it interact with thinking models, the explicit
continuation knobs, and the tool grammar. Eight `#[ignore]` smokes in
`src/shared/api/continue_probe.rs`, run against the reference stack and three cloud
keys.

**The instrument had to be rebuilt after the first run** (lessons §3 — suspect the
fixture before the feature). llama.cpp *echoes the prefill* back in the response, so
a continuation and a verbatim restart are the same bytes unless the partial carries
a marker no model would regenerate on its own — the fixture now opens with an
invented "Per the Zorbville atlas …" (the compaction smokes' invented-code
discipline), which makes echo / bare continuation / restart three distinguishable
outcomes. And the first fixture's mid-word cut ("…is Par" of a one-token "Paris")
is a state no real interruption can produce — streams break at token boundaries —
and it measured exactly like the out-of-distribution state it is: Gemma welded
"Par" into "Parise."/"Pariz.", haiku-4-5 slipped a U+00AD soft hyphen into the
seam, Gemini a `"\n"`. The asserted arm cuts at a word boundary; the mid-word arm
stays in the smoke as a recorded, never-asserted exhibit. The Qwen re-run forced a
third correction: the invented marker is also a *premise* — Qwen 3.6 answered the
fixture in-universe ("…the capital of France is **Zorbville.**") where Gemma,
haiku-4-5 and Gemini answered from geography, so the assertion accepts either
completion and only a restart or garbage fails it.

**Measured (8/8 green, 2026-08-27, stack: `llama-server` at 192.168.1.20:8000,
gemma-4-31B q4_0; llama-side arms re-run the same day on Qwen 3.6, build
b10659):**

- **llama.cpp**: a plain trailing-assistant request **continues exactly** —
  `" Paris."`, the seam a single space, `finish=Stop` — with the prefill echoed at
  the head of the stream, so the feature's append path must strip the echoed
  prefix before appending (the one design change this probe forced). **Prefill and
  a tool schema coexist**: the continued round finished its sentence and emitted a
  parsed `get_weather` call (`finish=ToolCalls`) — fork F5's include-the-tools GO.
  The explicit `continue_final_message`/`add_generation_prompt` pair is *unknown*
  to this build — a wrong-typed value sails through as 200, knobs-off is ignored —
  so on llama.cpp the default prefill alone carries the feature, and the explicit
  fields ride along harmlessly for vLLM's sake. The Qwen 3.6 re-run reproduced
  all of it on **b10659, the day's build**: same echo, same one-space seam, same
  `finish=Stop`, tools still GO (`get_weather` parsed, `finish=ToolCalls`) — and
  the knob pair is unknown to the *current* build too, so in practice it is
  vLLM-only.
- **Anthropic**: `claude-haiku-4-5` continues cleanly — `" Paris."`, no echo.
  `claude-opus-4-8` rejects with the exact wording now pinned in the smoke: *"This
  model does not support assistant message prefill. The conversation must end with
  a user message."* — the 4.6+/5-family gate the capability table needs.
- **Gemini** (`gemini-2.5-flash`): continues cleanly — `" Paris."`, no echo.
- **Grok** (`grok-4.5`): **restarts** — the reasoning opens with "The user
  asked…" and the reply is a fresh sentence without the marker. xAI has no
  continuation semantics; the research doc's §2 row goes from "unknown" to a
  measured no, and `/continue` will refuse on Grok.
- **Qwen 3.6 (thinking), build b10659**: the #21889 rejection is **gone** — a
  plain trailing-assistant prefill is accepted and the server silently **skips
  thinking** (empty thoughts, and no `<think>` residue: #21511 not reproduced) —
  which is exactly the semantics a continuation wants: a visible answer resumes
  without re-opening reasoning. The `chat_template_kwargs
  {"enable_thinking": false}` escape continues too, so the design keeps sending
  it for the #21889-era builds and accepts both behaviours. The mid-word exhibit
  got more vivid on the way: "Par" welded into "Parapluie.".

**Verdict: GO** for stage 1 on the settled forks (managed/external first), with one
design amendment recorded in the research doc: the OpenAI-compatible path returns
prefill+continuation while Anthropic/Gemini return the continuation alone, so the
seed-append site normalizes by stripping the seed's text when the stream opens
with it. The thinking gate turned out **build-dependent rather than absolute**;
the capability story for managed/external stays "send the kwarg, accept both
behaviours".

**Tests.** +8 `#[ignore]` (2617 green, 117 `#[ignore]`); no unit delta — the probe
is the deliverable.

**The duplication gate, one round.** The PR opened red at 4.6% new-code duplication
(bar 3%) with everything else green, and both causes were already in lessons §2: the
three launcher tests shared an opening (a fixture, not a test), and — the half worth
remembering — rewriting the five-line `Managed` arm inside the two identical
`active_model_name` copies wrote new lines into a range `cloud()`/`cloud_mut()` had
already flagged. Extracting a shared helper for those two made it *worse*, both call
sites becoming the same six lines; what passed was the smallest edit, one changed line
in each copy with the rest byte-identical to `main`. Final: **2.1% duplication, 0 new
issues, 0 security hotspots, 98.8% coverage on new code**. The test fixture also closed
a latent flake it made visible — these are the first tests here to reach `spawn`, so
with a real `llama-server` on `PATH` the launch would have *succeeded* and the test
panicked; the binary is now a path inside the temp directory that cannot exist.
Recorded in lessons §2 as the eighth instance, with the offline scan that reproduces the
gate closely enough to iterate before pushing (1.2% where Sonar read 2.1% — right
blocks, low density).

**Smoke — GO** (2026-08-23, both halves, on a rented GPU box — the development
container has neither a `llama-server` binary nor the ~100 GB of weights). Stack:
`unsloth/gpt-oss-120b-GGUF` `Q8_0`, **two** parts, managed mode against
`/workspace/llama.cpp/build/bin/llama-server`, `-ngl 99`, `-c 131072`, `--jinja`.

- *The model loads and answers.* `-m` pointed at
  `…/gpt-oss-120b-Q8_0-00001-of-00002.gguf`; the server came up, the chat answered
  two turns, and `thinking` arrived on both — so llama.cpp's harmony handling
  reaches the app as `reasoning_content` through the path already in place. The
  header reads **`gpt-oss-120b-Q8_0 · 128k ctx`**: `display_name` doing its job,
  the model named rather than its first file.
- *A part removed is named.* With the second file gone the status bar reads
  `chat: no connection: part of the multi-file model is missing:
  /workspace/gpt-oss-120b-GGUF/Q8_0/gpt-oss-120b-Q8_0-00002-of-00002.gguf` —
  refused before `spawn`, which is the whole point of the check: without it this is
  a server that starts, dies while loading, and says only that it exited.

What is still taken from llama.cpp's `llama_model_loader` source rather than
observed here is the third case, "a non-first part is refused" — the preflight
stops that one before the server can have an opinion.

### Post-M9: `/continue` — stage 1: resuming an interrupted reply in place (done)

**What** (2026-08-28, [docs/research/continue-generation.md](../research/continue-generation.md),
every fork at its recommended option; spec §6.4, §11.7). A reply cut by `Esc`,
a stream failure or the length/context limit resumes **in place**: the history
goes back out with the partial as its trailing assistant message, the engine
continues it (assistant prefill — the mechanism stage 0 measured), and what
arrives appends into the same `Message` — same id, same bubble, no separator.
A turn interrupted between tool rounds (a tool-result tail) resumes the
agentic loop instead, with nothing to prefill. Nineteen-plus-one becomes the
twenty-third typed route; no chord, per fork F6.

**The pieces.** `MessageFinish` on `MessageMetadata` (additive, ADR 0006) —
`finalize_message` finally records *why* a reply ended, which is what makes
eligibility readable (`stop` refuses, `cancelled`/`error`/`length` and
pre-field `None` continue); `ChatRequest.continue_final` mapped by the
OpenAI-compatible wire to `continue_final_message: true` +
`add_generation_prompt: false` + `chat_template_kwargs {"enable_thinking":
false}` — one byte-compare test pins that the flag off changes nothing;
`ServerMode::supports_continuation` (managed/external only — the
`supported_sampling_fields` pattern) gates the command and words the notes;
`handle_continue` → `start_generation(continuation)` → a `ContinuationSeed`
through the turn; `merge_continuation` in `handle_done` folds the turn's
first assistant message into the seed (model name kept unless the
continuation outgrew the partial — fork F7); `GenerationStarted {
continuation }` / `Finished { continuable }` / `LiveTurn { continues }` carry
the one bit each surface needs; the feed's `begin_continuation` re-opens the
last assistant bubble past any trailing notes, and a mid-turn rebuild folds
the filed round into the seed's view before `from_messages` stitches — the
`"\n\n"` seam machinery never fires inside a continued reply.

**The echo filter.** Stage 0 measured llama.cpp returning *prefill +
continuation*; `EchoFilter` in `stream_round` withholds bytes while they
match the seed, drops them on a full match, and flushes them intact the
moment the stream diverges — a non-echoing server (Anthropic/Gemini in a
future stage 2, vLLM) loses nothing, and a stream that dies mid-echo appends
nothing twice. Placed upstream of the sink, so the screen, the in-flight
mirror and the stored message all see continuation-only text.

**Doors closed** (lessons §4). Every refusal answers: nothing to continue,
a reply that finished on its own, a thoughts-only fragment (F4 — no chat API
resumes a reasoning trace), an unsupported provider, a read-only transcript.
The cancelled and cut-short notes gained `_continuable` variants that name
`/continue` only when the mode supports it — and `FinishReason::Length`,
invisible in the main path until now, finally produces a note at all
(`ui.err.reply_truncated[_continuable]`): a length-cut reply used to stop
mid-sentence in silence.

**Tests.** +14 unit (2631 green) and +1 live (`#[ignore]` 118): the echo
filter's four shapes, the merge's F7/F8 rules, the wire's byte-compare, the
metadata's additivity, five orchestrator flows (append-in-place, echo-strip
through the real loop, `Length` recorded and announced, tool-tail resume,
the refusal ladder on a bare orchestrator) and five screen tests (the
resumed bubble, the note moved above the reply it described, both `Length`
notes, the cancel-note variants, a mid-continuation rebuild). The flow tests
share one `interrupted_turn` fixture from birth — the third test starting
like the first two is a fixture, not a test (lessons §2).

**Smoke — GO** (2026-08-28, stack: `llama-server` b10659 at
192.168.1.20:8000, Qwen 3.6; embedder on :8001). `continue_e2e_live` cancels
a real streamed reply after its first text chunk, `/continue`s it, and
asserts the one equality that carries the feature: the concatenation of
everything both turns streamed equals the stored text byte-for-byte — the
real server's echo was stripped, nothing doubled, nothing fell into the
seam, and the reply is one message ("one" through "thirty", 246 chars,
finish `Stop`). The full orchestrator live set was re-run on the same stack
for turn-path non-regression.

### Post-M9: `/continue` — stage 2: the clouds (track complete)

**What** (2026-08-28, the widening §8 of
[docs/research/continue-generation.md](../research/continue-generation.md)
recorded in the roadmap). `/continue` now also works on **Gemini** and on
**Anthropic models up to the 4.5 generation**; OpenAI and Grok stay refused —
one measured never continuing, the other measured restarting. The capability
gate grew a model axis: `ServerMode::supports_continuation(model)`, with the
Anthropic arm an **allowlist by version** read off the model id (`≤4.5`
continues, `4.6+` rejects — the removal is forward-going, so a blocklist by
name would rot open; an id whose generation cannot be parsed refuses, the
capability-is-asked discipline). The version reads off the id's first two
short numeric segments, which covers the old `claude-3-5-sonnet-20241022`
naming, the new `claude-haiku-4-5`, dated and `@`-suffixed variants — the
matrix is pinned in `shared::config`'s own test.

**The Anthropic wire earns its two constraints.** A continuation request
drops the extended-thinking block (Anthropic rejects a prefill combined with
thinking — the same "resuming a visible reply must not re-open reasoning"
choice the OpenAI-compatible wire makes with `enable_thinking: false`) and
right-trims the **wire copy** of the trailing prefill (Anthropic rejects
trailing whitespace; the stored message keeps its bytes — the seam caveat of
research §4h). One byte-compare test pins that the flag touches only those
two things. The Gemini wire is deliberately untouched: the measured
configuration is the unmodified one — the trailing `model` turn *is* the
mechanism.

**Tests.** +2 unit (2633 green; the capability matrix and the Anthropic wire
byte-compare), the orchestrator refusal ladder gained the per-model Claude
arm, and the probe's cloud arms now go **through the app's wire**
(`continue_final: true`) with two new live arms: extended thinking requested
(sent as-is the API answers 400 — the wire's suppression is what passes) and
a trailing-whitespace partial (same shape, the trim is what passes).

**Smoke — GO** (2026-08-28, real keys + `llama-server` b10659/Qwen 3.6):
`continue_probe` 8/8 — all three haiku-4-5 arms continued with a clean
one-space seam (` Paris.`), Gemini 2.5-flash likewise through the wire,
opus-4-8 still rejects with the pinned wording, Grok still restarts
(recorded), and the llama-side arms are unchanged — no regression from the
signature change. The orchestrator path above the client is byte-identical to
the managed one already covered by `continue_e2e_live`, so no cloud-side
orchestrator harness was built for this.

### Post-M9: the model's name in `external` mode — asked of the server, and sent to it (done)

- **What and why.** `external` is the one mode where the app did not know what it
  was talking to. `EngineSettings::active_model_name()` answers from settings for
  every other mode — a managed server's GGUF, a cloud provider's required model
  name — and for `external` it answers from the optional "Model (opt.)" field,
  which is normally blank: `MINDFORK_ENGINE_URL` fills the URL and nothing else,
  and connecting to a `llama-server` by URL is what most local users do. The feed
  title's caption and every message's `MessageMetadata.model` therefore went
  empty, and the `interface.show_model_name` toggle had nothing to show — the
  docker lab seeds `model_name` explicitly for exactly that reason. Now the
  **engine is asked**, and the answer feeds the same two surfaces the managed and
  cloud modes already fed. Design and the six forks:
  [external-model-name.md](../research/external-model-name.md).
- **A defect found while reading the code, and fixed first because it decides the
  design.** `external_chat_setup` never passed the configured model name to its
  client — the value was not among the function's parameters at all, so it could
  not reach the wire. `ExternalSettings::model_name` is documented as "for a
  multi-model server", `OpenAiClient::model` as "a multi-model proxy requires
  it", and install.md §3.1 recommends LiteLLM and OpenRouter as `external`
  targets — none of which can work without it: llama.cpp's own router mode
  answers `400 "model name is missing from the request"` on an empty `model`
  (`server-models.cpp:1550`). The same omission was in `apply_embed`'s external
  arm; TTS already sent its model. Fixed in all three. An **empty** field still
  sends no `model` key, so a bare `llama-server` request is byte-identical to
  before — measured: that server ignores a `model` it has never heard of and
  answers with the one it loaded.
- **Why the catalogue is asked before `/props`.** Three channels report the model
  and, measured on b9769, they agree: `/v1/models` → `data[0].id`, `/props` →
  `model_alias`/`model_path`, and the chat completion's own `model` field. Two
  facts pick the order. `/v1/models` is **public** — an `--api-key` server answers
  it `200` with no header while `/props` is `401` — and it is the standard
  endpoint, so vLLM, LM Studio and every gateway answer it while `/props` is
  llama.cpp's alone. A catalogue listing **several** models is deliberately not
  guessed at: there the request's own `model` picks one, so the name is in
  settings already or the setup does not work — inventing an answer would name a
  model that did not reply.
- **The reply's `model` field was rejected as the source**, though it is the one
  channel that is always right. It arrives *after* the header is drawn, so the
  live bubble would rename itself mid-turn — the exact failure the "model's name
  on the assistant's header" entry solved by resolving the name **once** at turn
  start — and it says nothing at all before the first message, which is when a
  caption is already on screen. It also has a history: before llama.cpp PR
  #17668 (2025-12-02) the field was `request.model ?? "gpt-3.5-turbo"`, so an
  older server asked without a `model` — precisely what this app sends — answers
  with a flat lie. `/props` and `/v1/models` were honest even then.
- **`ModelDiscovery` is `ContextDiscovery` with two differences.** Same epoch /
  pending / answered shape, invalidated when the engine is applied and when
  readiness flips. But the question is asked **eagerly** rather than on first use
  (a compaction budget is needed as a conversation approaches its window; a
  caption is needed before the first message), and only **when the configuration
  cannot name a model** — a name the user typed is never second-guessed, and
  asking anyway would spend a round trip to confirm what settings already say.
  Nothing is written to `settings.json`: had the discovered name been stored, F1
  would then have put it on the wire as a routing key, which is a behaviour
  change nobody asked for.
- **One value, one meaning.** `AppEvent::EngineModel` carries only the
  *discovered* half; the screen already holds the configuration and prefers it,
  so the fallback rule lives in one place per consumer instead of being baked
  into a merged value. The turn reads `Orchestrator::effective_model_name()` —
  still a single read per turn, so the streaming header and the stored metadata
  cannot disagree.
- **A reported id is normalized only when it is a file.** `gguf::display_id`
  applies `display_name` to anything ending in `.gguf` and passes everything else
  through: an un-aliased `llama-server` reports the whole `-m` path (the live
  stack answers `D:\LLM\GGUF\gemma-4-31B_q4_0-it.gguf`), while `meta-llama/Llama-3-8B`
  and `anthropic/claude-opus-4.5` are org-qualified ids whose first segment says
  whose model it is.
- **`RetryBackend` did not delegate the new method — and only the live run said
  so.** Every stub test passed against a client built directly; production wraps
  `external` in that decorator, and `external` is the *only* mode that asks, so
  the one missing line was the entire feature for every real user of it. This is
  the **third** occurrence on this decorator (`context_budget`, `vision`,
  `model_id`), and the two earlier ones each left a comment saying it would
  happen again. Generalized into lessons §9: a defaulted trait method plus a
  decorator is a silent-`None` hole that no test of the inner type can see.
- **Tests**: +17 unit (`display_id`'s two shapes; `model_id` taking a single
  listed model without asking `/props`, normalizing a reported path, falling
  through to `/props` on a list of several and on an empty alias, and staying
  silent on every way of not being told; the decorator's delegation; the
  configured name reaching the request body and a blank field sending no `model`
  key; the discovery's epoch, the settings-first rule, the "cannot say" case, the
  emission, the clear-on-change, and a named engine never being asked; the
  caption's fallback). Suite **2633 → 2650**, +2 `#[ignore]` (118 → 120).
- **Smoke — GO** (2026-08-28, `llama-server` b10659 / Gemma 4 31B at
  `192.168.1.20:8000`, "Model (opt.)" blank): `model_id()` → `gemma-4-31B_q4_0-it`,
  normalized from the reported Windows path; the streaming bubble's header and
  the stored `MessageMetadata.model`, read back through a fresh activation, both
  the same string. The whole `orchestrator::tests::live` set was re-run as the
  turn-path regression — **46 passed, 0 failed** in 866 s, so sending `model` on
  the external wire (blank here, hence absent) regressed nothing. **Not covered live:** the multi-model gateway half — it
  needs a `llama-server --router` or a LiteLLM container, and what the app
  controls (the request body carrying the name) is pinned by a unit test instead.
- **A behaviour worth knowing**: discovery is a network round trip started when
  the engine is applied, so a turn sent in the same tick as the bootstrap still
  records no name. Correct — blocking a turn on a round trip to satisfy a caption
  would be the wrong trade — but it is why the end-to-end smoke waits for
  `EngineModel` before sending.


### Post-M9: parallel sub-agents — stage 1, sessions per engine and the slot count (done)
- **What**: the engine half of running several sub-agents at once
  ([docs/research/parallel-subagents.md](../research/parallel-subagents.md),
  all ten forks at their recommended options, user's decision 2026-09-03).
  Every engine section — `engine.managed`, `engine.external`, the four
  clouds — carries `sessions: u32` (default 1, `EngineSettings::active_sessions`
  reads the active one and never returns below 1); `tools.subagent_parallel`
  (default 1) exists in config for stage 2. A turn's request streams run
  under a `tokio::sync::Semaphore` on `TurnShared` sized from the active
  section: `TurnLoop::stream` takes a permit for the duration of
  `stream_round` and for nothing else (`acquire_session`, cancellable — a
  loop cancelled while waiting lands what it has as a `cancelled_round`), so
  at the default the turn's loops take turns exactly as before. `build_args`
  adds `-np N --kv-unified` when `ManagedConfig.parallel` (the section's
  `sessions`) is above 1 and nothing at 1. `EngineBackend::parallel_slots`
  (llama.cpp's `total_slots` on the `/props` fetch the other three questions
  make) is the fourth self-description question, delegated by
  `RetryBackend` **with its test in the same commit** — the hole that opened
  three times before (docs/lessons.md §9). The orchestrator asks it in
  `slots.rs` (the `ModelDiscovery` shape: epoch, pending, re-asked on apply
  and on a readiness flip) and `AppEvent::EngineSlots` carries the answer to
  the chat screen, which hands it to every settings screen it builds; the
  new "Parallel sessions" group on the assistant's Model tab shows the field
  in all six modes, with *"The server reports N slots."* appended for a
  managed or external server that answered. The impersonation and
  embeddings tabs have no such row.
- **Why the flag pair, in numbers.** On the local CPU build (b10788, Gemma 3
  4B, `-c 4096`): `-np 3 --kv-unified` → `/props` reports 3 slots and
  `n_ctx 4096` per slot — the whole pool for each; `-np 3` alone → 3 slots
  of **1536** (4096/3, padded down to 256). An explicit `-np` switches
  llama.cpp to its split shape, so the unified pool has to be asked for by
  name; the pair is what the server does on its own for its auto default
  (`n_parallel < 0 → 4, kv_unified = true` since December 2025), which is
  why a managed server already had four slots before this stage and why
  `sessions` costs no extra KV memory (R5 of the research).
- **One amendment to the plan.** The research's stage 1 listed the
  `subagent_parallel` settings row; it ships with stage 2 instead. Nothing
  reads the field until the parallel group exists, and a knob that does
  nothing is the advertised no-op docs/lessons.md §4 keeps recording. The
  config field, its default and its test are in.
- **Screenshots**: the new row changed the Model-tab dumps (a scrollbar
  glyph and the field count) and, through the same scroll state, the
  Tools-tab dumps; regenerated with the site's JetBrains Mono faces written
  back out as TTFs (docs/lessons.md §1) — the renderer reproduced every
  committed image byte-for-byte before the change, which is what made the
  regeneration trustworthy.
- **Live run — GO.** `props_reports_the_slot_count_live` and
  `props_reports_the_context_window_live` against the LAN stack (llama.cpp
  b10791, `gemma-4-E2B-it-Q8_0`, `-np 4` split): slots 4, window 4096.
  `managed_sessions_launch_three_slots_over_one_pool_live` on the local CPU
  build with Gemma 3 4B: the launcher's own line brings up 3 slots over the
  whole 4096 pool, 2 s. Retry decorator: the delegation test asserts the 4
  the inner client answers, which the default could not produce.
  **The orchestrator e2e set — GO on Gemma 4 31B** (2026-09-04,
  `gemma-4-31B_q4_0-it` + mmproj, llama.cpp b10791, launched exactly as
  stage 1 would at `sessions = 4`: `-np 4 --kv-unified -c 16384`, so four
  slots over the whole 16384): **49 passed, 0 failed**, 72 min; the TTS
  smoke skipped for want of a cloud key. Two false starts worth writing
  down: the stack of the previous day was the 2B model on four *split*
  4096-token slots, raised for the research's throughput probes, and the
  set cannot be read on it — the model answers the planted facts with empty
  content and no tool call, and the set's prompts overflow 4096
  (`exceed_context_size_error` at ~4200 tokens); the 31B in the same split
  shape has the same 4096 ceiling. A red run on the wrong shape is not
  evidence either way (docs/lessons.md §9); the shape the app launches is
  the one to test on. **GO on Qwen 3.6 27B too** (`Qwen3.6-27B-Q4_K_M`,
  the same line without a projector): 47 passed, 27 min, and the two
  image smokes skip in writing under `MINDFORK_LIVE_TEXT_ONLY=1` — run
  once without the flag they failed honestly ("the server accepts no
  images"), which is the flag's whole point. The two models measure very
  differently on the same card (research §3.5): the Gemma stack prefills
  at ~50 tok/s and gains 1.22× from four streams, the Qwen stack at ~2200
  tok/s and gains 2.9× — the 31B's slow prompt path, not the hardware,
  and a stack question rather than this track's.
- **Tests**: 2759 green (+13: `active_sessions` by mode and its floor,
  the defaults on an old file; `build_args` at 1 and at 3; the `/props`
  slot read and its two "cannot say" shapes; the decorator's delegation;
  the semaphore's take-and-return and the cancelled wait; `SlotsDiscovery`'s
  epoch; six settings-screen tests — the row in every mode of the Assistant
  tab and absent from Impersonation/Embeddings, the edit routed to the active
  section with the others untouched, the floor at 1, the unparsable input
  ignored, the slots hint for local servers only), 131 `#[ignore]` (+2 live).

### Post-M9: several reasoning items in one reply — each resent as its own item (done)

**Symptom** (the user, 2026-09-06, the first run with four parallel sub-agents
on gpt-5.6): one of the four ended "the engine failed before completion" at
its second round while its three siblings completed; the parent re-ran it and
the report was whole. The log had the cause — `400 Bad Request`,
`invalid_request_error` / `invalid_encrypted_content`: *"The encrypted content
for item rs_… could not be verified. Reason: Encrypted content could not be
decrypted or parsed."* Non-transient by the error classifier, so no retry —
correctly: the request was malformed.

**Cause: a reply carries several reasoning items, and the loop fused them into
one.** `stream_round` accumulated every `ThoughtsSignature` chunk by appending
its ciphertext to one string and keeping the last id; `push_assistant_items`
then sent **one** `reasoning` item — the last id over the concatenation of every
item's `encrypted_content`, which is precisely what the API cannot decrypt. The
shape had never shown itself because every reply seen until then carried one
item. gpt-5.6 does not: measured on the user's key with the failed run's own
brief, **8 of 16** replies carried two to five reasoning items, and one of five
tool-calling replies had four items ahead of three calls — the failed run's
shape. The tool-less English probes of the same day gave one item every time,
which is why the first draft of the live smoke (a shortened brief, no tool)
saw eight single items in a row and was rewritten around the verbatim brief.

**Measured before the fix** — the echo of a 4-item, 3-call reply:

| Echo | Result |
|---|---|
| the app's: one item, last id, contents concatenated | 400 `invalid_encrypted_content`, the log's text verbatim |
| every item in its place | 200 |
| one item alone (first or last, its own content) | 200 |
| no reasoning item at all | 200 |

OpenAI's reasoning guide: *"pass back all reasoning items, function call items,
and function call output items since the last user message … untouched"*. The
causes of this error known in the field — another organisation's key, a model
switch mid-conversation, stale items — did not apply: same key, same model, a
fresh item. Parallelism did not cause it either: reproduced on a single stream.

**Fix.** `ApiMessage.thinking` is a `Vec<ThinkingBlock>` — one block per
reasoning item on Responses, one on Anthropic — and `ThinkingAccumulator`
(contract.rs, beside `ToolCallAccumulator`) replaces the two locals of
`stream_round`: a reference **with** an id is an entry of its own; one
**without** appends to a trailing id-less entry, so Anthropic's one signature,
however many deltas carry it, stays one block — the previous behaviour by
construction. `push_assistant_items` emits every id-bearing block, in order,
before the text and the calls (every shape observed had the reasoning items
first; positions are not otherwise tracked); `build_messages` (Anthropic) emits
every block, which for one block is what it emitted before. The round's
thoughts text rides on the first block (Anthropic resends it with the
signature; Responses sends only id + ciphertext). `with_thinking(Option)` is
gone; `with_thinking_blocks(Vec)` is the builder.

**Not done, and why.** Positions by `output_index` (a reasoning item *between*
two calls): no such shape appeared in the 16 probed replies or in the field,
and the accumulator keeps the reply's order, which is what the API checked.
The Anthropic interleaved-thinking beta (several blocks per reply) is not
requested by the app; were it, per-block text would be needed — the loop
already resends each block on its own.

**Tests**: 5 new unit — the accumulator (one entry per id-bearing item, in
order; id-less deltas fused into one block; an id-less delta never appended to
an item; empty), the Responses wire (three blocks → three `reasoning` input
items under their own ids, in order, then the call); and one new `#[ignore]`
live smoke, `several_reasoning_items_round_trip_each_as_its_own_item`: the
verbatim brief with the search tool, streamed until a multi-item reply
arrives, echoed through the app's wire, plus a control arm sending the fused
shape, which the API must still reject. 2871 unit tests green (138 → 139
`#[ignore]`). **Live**: the five Responses smokes on gpt-5.6, 5/5 — the new
one caught a 2-item, 3-call reply on its fourth attempt, the echo through the
app's wire was accepted (a 23 480-character reply), and the control arm's
fused item was rejected with `invalid_encrypted_content`; the four Anthropic
smokes (the single-block signature round trip intact) and the two continue
probes, 6/6.

### Post-M9: the silent tasks under the app-wide budget — the budget's silent lane (done)
- **What**: the item three tracks recorded and two designs had argued away
  (parallel-subagents F9, background-subagents §4.7/§8, admission-by-budget
  §8, tasks-screen §8); the design
  [docs/research/silent-tasks-budget.md](../research/silent-tasks-budget.md),
  every fork at its recommendation (the user's decision, 2026-09-07). Seven
  request paths reached `chat_stream` outside the session budget — the
  title, the three silent loops, the compaction roll, impersonation on the
  shared engine, a `fetch_url` summary inside a silent loop — and at the
  default `sessions = 1` the launcher writes no `-np`, so the server runs
  **four slots over one unified pool** while `pool_for` answered *none*.
  Now `SessionBudget` has a **silent lane**: one permit for the app's own
  requests (`acquire_silent`, a label per request) over the *same* pool sum,
  so they take turns among themselves and never overfill the pool beside a
  turn or a run, and a turn that does not fit beside an open silent round
  waits for that one round; `pool_for` keys on the server's reported slots
  (managed: the context budget unless exactly one slot is reported; the
  launcher's default reads as four); `handle_done` asks for the roll right
  after the title and ahead of the loops (fork F6); the tasks screen gained
  a third state, *waiting*, read off `silent_streaming` against
  `background::lane_label`, with the runtime re-asking for the rows on its
  tick while a silent task runs. A silent loop's later rounds are floored by
  the previous round's exact `usage` (the loop reads the chunk it used to
  ignore); a wait cancelled ends the loop `Ok` with nothing run. On the way:
  the roll's failure was worded with the title's key
  (`ui.err.title_gen_failed`) — it says the summary request failed now.
- **Measured before designing** (stage 0, research §3.1): the CPU build
  (b10807, Gemma 3 4B) launched with the managed launcher's own line at one
  session logs `n_slots = 4, kv_unified = 'true'` over `-c 2048`; the probe —
  a chat seeded with 55 % of the pool, a background run holding another
  55 %, `/compact` at the landing — reproduced the collision through the
  app's own paths: two streams at once, the run `Failed` with a one-glyph
  reply, the roll "Context size has been exceeded", the server's batch
  halved 1024 → 1 over 36 s; the guarded arm was red on `main` by the same
  event. A landing with every cadence due opened **five** silent requests at
  once. The collision was invisible on the silent side: `read_round` logged
  the in-stream error as a `warn`, took the `Finished(Error)` as a round's
  end, and the task landed `Ok` — the streak reset, reflection announced
  `SelfModelChanged` for a window it never read.
- **Rejected**: the silent tasks on the *interactive* permits (F2b — at one
  session every silent round would stall the next message, and the fan-out
  would serialise ahead of a wake turn); room without a permit (F2c — five
  streams still open at once); preemption of the silent stream (F3b —
  deferred behind the same `acquire_silent`, with the watermark-at-success
  bookkeeping it needs); a knob for the wait (F3c). Keeping the pool rule
  keyed on `sessions` (F4b) would have left the default unguarded.
- **Two things the tests taught.** The default profile speaks Russian, so a
  recorder keyed on the English prompts routed every silent request to the
  parent's fallback queue and ate its scripts — the harness switches the
  profile to English first; and the self-model tools are off by default, so
  reflection never fires on a fresh profile until they are enabled. The
  `sessions` hint grew past the longest settings hint (694 → 763 chars) and
  the settings hint panel gained a row on every tab — visible only as four
  drifted demo dumps; the hint was trimmed to 702 and the dumps came back
  byte-identical, so no screenshot was regenerated.
- **Tests**: +16 — `session_budget.rs` (the silent lane is one wide, the
  lanes share the pool both ways, a silent stream alone is admitted, a
  cancelled silent waiter leaves nothing), `pool.rs` (managed unless one
  slot is reported, none reported reads as four), `tests/silent.rs` (the
  fan-out one at a time with all four slots taken; the roll waiting for the
  run and the wake turn waiting for the roll on a 4000 pool at two sessions
  — `open_at_arrival` 0, `max_in_flight` 1; a loop's later round floored by
  its exact size — `[1, 0]`; the title, the roll, then the loops; impersonation
  on the shared engine labelled on the lane; a cancelled wait opening no
  stream), the tasks screen's *waiting* word — 2881 unit tests green,
  141 `#[ignore]` (+2: the probe's two arms).
- **Live** (mandatory — every path here is an engine path): the probe's two arms on the CPU build after the lane —
  the control arm (belief 8192) still reproduces the collision (two streams
  at once, the run `Failed`, the roll "The summary request failed: … Context
  size has been exceeded", 133 s), the guarded arm now passes (the roll
  waited for the run, one stream at a time, the run `Completed` with its
  codename and the roll a summary, 88 s); and the regression on the LAN
  stack (Qwen 3.6 27B Q4_K_M, `-np 4 --kv-unified -c 16384`, b10807):
  `admission_e2e_live`, `background_subagent_e2e_live`,
  `parallel_subagents_e2e_live` — 3/3 in 83 s, and the probe's guarded arm
  there too (`silent_roll_e2e_live`: the roll waited for the run, most open
  1, both completed, 16.5 s).

### Post-M9: the silent stream yields to the turn — preemption on the session budget (done)
- **What**: the silent-lane design's fork F3(b), deferred until the wait it
  introduced was measured and chosen against: an interactive stream — a
  turn's round, a sub-agent's, a dialogue's line, a background run's round,
  a turn's own summary — that does not fit beside the app's own open request
  no longer waits for that request's round; the request is **displaced**
  (cancelled, and made again by its own task after the turn). The design
  [docs/research/silent-preemption.md](../research/silent-preemption.md),
  every fork at its recommendation (the user's decision, 2026-09-07).
- **Measured before designing** (stage 0, research §3.1): two `#[ignore]`
  arms of `preemption_smoke` in `tests/live.rs` over the hybrid
  `ScriptedParent` (which gained `live_after_scripts`: a request with no
  script left goes live, so the seed is scripted and the measured turn is
  live). On the CPU build launched as the managed launcher launches it at
  one session, a one-word turn sent half a second behind the compaction
  roll waited **45.9 s** for the roll's end (the roll 46.4 s), where a turn
  sent after a cancelled one had its first token in **0.79 s**; on the LAN
  stack 5.6 s against 0.35 s. The server honours the closed connection in
  1 ms (`stop: cancel task`) and releases the slot 110 ms later. Two
  protocol facts: a one-exchange chat has nothing for `plan_cut` to fold
  (the seed is two exchanges, the second longer than the tail), and a
  request sent in the gap before the server's release lands on another slot
  by LRU and prefills cold — 36 s for a prompt whose prefix the cancelled
  slot still held (lessons §3).
- **How**: `SessionBudget` — a silent `Reservation` carries a **child
  token** of the holder's own (`stream_token`), and the budget records the
  silent stream open (`SilentOpen`: label, token, tokens, `yields`). An
  interactive waiter that does not fit calls `displace`: when the silent
  stream yields and the waiter would fit without its reservation, the child
  is cancelled (once — a second waiter only counts itself) and the waiter is
  marked **displacing** (`Displacing`, a drop guard: uncounted with a
  wake-up when the waiter is admitted — after its reservation is in — or
  gives up); a silent request takes no room while a displacing waiter is
  pending, so the displaced task's retry cannot slip back in ahead of it. A
  holder reads `displaced()` (the child fired, the parent did not).
  `acquire_silent` gained `yields`; `SILENT_YIELDS_MAX = 3`. The holders:
  `spawn_title` and `spawn_compact` loop — acquire (yielding while the
  count is under the cap), stream on the child token, and on a
  `Finished(Cancelled)` that reads displaced go round with the same request;
  a cancelled stream that is not a displacement (the app's quit) ends the
  title silently and the roll with the timeout's wording — never a fragment
  as the summary. `tool_loop::run_rounds` is split into `stream_round`
  (the reservation, the stream under what is left of the task's clock,
  `Streamed::{Round, Displaced, Cancelled, TimedOut}`) and `run_tools`
  (the calls under the clock's remainder): the task's `timeout` is now a
  **clock over streaming and tools**, the waits for the lane and for room
  outside it (`RoundsEnd::TimedOut` is what the spawn maps to the
  time-limit outcome). Impersonation and the in-loop `fetch_url` summary
  acquire with `yields: false`. Nothing on the tasks screen changed: a
  displaced task's label leaves the budget with its reservation and the
  row reads *waiting* on the next tick. `KeyedRecorder` ends a delayed
  script with `Finished(Cancelled)` when its token fires, as the engine does.
- **Tests**: +13 — `session_budget.rs` (a yielding stream displaced by a
  waiter that would then fit; one that would still not fit displaces
  nothing; a holding stream never; a silent waiter never; the displacing
  waiter admitted before the displaced task's retry; a waiter that gives up
  uncounts itself; no pool, no displacement; the holder's own cancel is not
  a displacement), `tests/silent.rs` (the roll waiting for the run and
  yielding to the wake turn — the turn streaming between the roll's two
  identical requests, the stored summary the retry's and never the
  fragment; a reflection round and the title displaced and made again;
  impersonation holding while a stream waits for it; a task holding after
  its third displacement; the loop's wait for room off its clock and its
  stream on it) — **2894 unit tests green, 143 `#[ignore]`**. The lane's
  own budget tests pass `yields: false`, so they still pin the lane and
  nothing else; the fan-out order test waits for the title to land before
  the second turn, which would otherwise displace it.
- **Live** (mandatory — every path here is an engine path): the CPU
  build's two arms after the change — the turn's first token at **62.9 s**
  against 85.2 s before, and before the roll's end (the roll 112.3 s:
  cancelled 1.1 s into its prefill, its slot released only when that batch
  had run out 23 s later, the retry placed cold by LRU, 49 s); the floor
  unchanged at 0.83 s. The LAN stack — the turn at **7.9 s** against
  12.1 s (the roll 14.3 s against 6.1 s), the floor 0.37 s — and the
  regression `silent_roll_e2e_live`, `admission_e2e_live`,
  `background_subagent_e2e_live` green: 5/5 in 80 s. What the server's log
  added (research §3.2): a cancel is honoured *between batches*, so a
  stream cancelled during its prefill holds its slot to the batch's end —
  the turn's wait fell to the roll's prefill batch (about 24 s on the CPU
  build, under two on the GPU), not to the floor — and the parked cache of
  a cancelled stream is evicted under pressure (`failed to find free space
  in the KV cache, retrying with smaller batch size`), while the retry
  prefilled cold. A smaller `-ub` is the launch-line follow-up, recorded and
  not taken.
- **Rejected**: the retry in the orchestrator (the task landing *displaced*,
  the watermark, the counters and the owed title refunded — four handlers
  for what one loop inside the task does with the spawn-time bookkeeping
  left honest); a fourth word on the tasks screen for a state that lasts a
  round; displacing whenever the waiter does not fit (above one session it
  cancels rounds that were not the problem); an unbounded number of yields
  (a 32-round turn could starve a task and waste a round per round).

### Post-M9: the batch a cancel waits for — `-b` on the CPU build's launch line (done)
- **What**: the item the preemption track recorded and did not take
  (silent-preemption §8): llama.cpp honours a cancel between batches, and on
  the CPU build a stream displaced or stopped during its prefill held its
  slot for the whole default batch — 23 s. The design
  [docs/research/cpu-batch.md](../research/cpu-batch.md), every fork at its
  recommendation (the user's decision, 2026-09-07).
- **Measured before designing** (stage 0, research §3.1): five launch lines
  on the CPU build, a fresh server per arm, the probe's new third arm
  (`preemption_cold_e2e_live`: the one-word turn alone, the cold prefill
  reference) beside the roll displaced during its prefill; the batch read off
  the server's log as the gap from `cancel task` to `release`. **Linear in
  `-b`**: 23.3 s at the default 2048, 13.1 at 512, 6.5 at 256, 2.8 at 128,
  with the cold prefill +5 / +14 / +20 %; `-b 2048 -ub 128` worse on both
  counts (39.8 s, +21 %) — the micro-batch sets the matmul's width, the batch
  sets when the server looks at its queue. The knee at 256: a quarter of the
  wait for a seventh of the prefill. The retry's cold prefill and the
  KV-pressure halving stay the server's.
- **How**: `ManagedSettings.batch_size: Option<u32>` (`#[serde(default)]`
  on the struct — an old `settings.json` reads `None`) → `ManagedConfig`
  → `build_args`: for the chat server, `-b n -ub min(n, SERVER_UBATCH)` when
  a number was typed, `-b 256 -ub 256` (`CPU_BATCH`) when none was and
  `gpu_layers == 0`, nothing otherwise — a GPU host's line byte for byte;
  the embedding server keeps `-ub <ctx> -b <ctx>`. A *Batch (-b)* row in
  the *Performance* group after `-ngl` (`num_row`: empty reads auto), the
  same for the impersonation engine, a hint under the dumps' ceiling. The
  docker stand's chat container gets the flags in `compose.yaml` — it is an
  external server to the app, so F5's "left to auto" was corrected to the
  line where the batch actually lives.
- **Tests**: +5 — `managed.rs` (`-ngl 0` → `-b 256 -ub 256`; a GPU line
  byte for byte; a typed 1024 → `-ub 512`, 128 → 128, 2048 on a CPU host →
  the server's default; the embedding server unchanged), `settings/tests.rs`
  (the field stores a number, an emptied field reads auto, the hint under
  the ceiling; the field is described) — **2908 unit tests green, 146
  `#[ignore]`**; the settings dumps and screenshots regenerated.
- **Live**: `managed_cpu_line_launches_with_the_measured_batch_live` — the
  app's own line for `-ngl 0` (`… -c 2048 -b 256 -ub 256 -m … --jinja`)
  launched through `ServerHandle::launch` against the CPU build, ready with
  four unified slots over 2048; the LAN regression 2/2 in 55 s. The gain is
  §3.1's 256 row, measured on the same flags by hand.
- **Rejected**: `-ub` as the knob (measured: it is not); the field without
  auto (a CPU host would have to know the flag exists); auto without the
  field; 512 (most of the wait kept) and 128 (+20 % for 3.7 s more) as the
  auto value; a runtime detection of a slow prefill (the batch is a
  launch-time argument).

### Post-M9: a stop gives the window back — the silent task returns at the next landing (done)

**What.** The stop track's fork F4b
([docs/research/stop-silent-task.md](../research/stop-silent-task.md) §7),
taken on the premise that track named as the condition for revisiting it:
users stop a silent task to *postpone* it, not to lose what it was about
to read. The design
[docs/research/stop-refunds-window.md](../research/stop-refunds-window.md),
every fork at its recommendation (the user's decision, 2026-09-08); no
stage-0 probe.

**The problem.** Three of the four silent tasks advance their cadence at
spawn — reflection its watermark `Chat.reflected_upto` and stamp, the two
consolidations their per-chat reply counters — and the lane and preemption
tracks kept them there for a reason: a task that fails or is displaced
after its first round has *read* its window, and re-reading it at every
landing would be a loop, or for reflection a duplicate of every
observation it wrote (the digest is over the window only for exactly that
reason). The stop track inherited the rule as "a stop skips this window".
So a reflection stopped seconds after its chip appeared — before its first
stream had ended, nothing written — lost the replies it was about to
reflect on for good.

**How.** What the spawn advances is remembered on the task's slot:
`BgSlot.window: Option<Window>` — `Window::Reflection { chat, upto, at }`
(the values before the spawn) or `Window::Counter { chat, count }` (what
the reset took), passed to `begin_bg` by the three spawn tails; the roll
passes `None` (an automatic roll stopped is already planned again, a
manual one was typed and stopped by the same hand). The loop reports the
one fact that decides: `RoundsEnd::Cancelled { rounds }` — `run_rounds`'
`round` at the moment the stop is read, which is exactly the number of
rounds whose tools had run, `0` for a stop while waiting for the lane, for
room, or during the first stream — mapped by `spawn_silent_loop` onto
`BgOutcome::Cancelled { consumed: rounds > 0 }`. `handle_bg_done` takes the
slot's window on every outcome and, on `Cancelled { consumed: false }`
only, gives it back (`give_back`): reflection's watermark and stamp
restored and the chat marked dirty — saved the way the advance was; a
counter **added back** (`+= count`), since the landings during the run
incremented it legitimately and the sum is what it would read had the
spawn never happened. Then the ordinary cadence does the rest: at the next
landing `reflect_window` counts the old window plus the new replies, `due`
holds, the gates run, the task spawns — over the same window extended by
what came after. Nothing is re-scheduled by hand; a task stopped twice is
refunded twice. On `Done`, `Failed` and `Cancelled { consumed: true }` the
window is dropped and the advance stands.

**Tests**: +7 — `tests/reflection.rs` (a reflection spawned through
`maybe_auto_reflect` and landed `consumed: false` → watermark `None`, stamp
`None`, the chat dirty, the next `maybe_auto_reflect` spawns again to
`Some(2)`; `consumed: true` keeps `Some(2)` and marks nothing; `Done` and
`Failed` keep it; a consolidation's counter at `3` with a window of `5`
reads `8` after `consumed: false` and `3` after every other outcome; a
window for a chat that is gone marks nothing), `tests/self_consolidation.rs`
(a "sleep" spawned at `every = 1`, a landing during the run → `1`, the stop
→ `2`, the next landing spawns again and resets), `tests/silent.rs` (a loop
whose first round called a tool and whose second stream is stopped lands
`consumed: true`; the three existing stops — mid-stream, waiting, retry —
land `consumed: false`) — **2932 unit tests green, 146 `#[ignore]`**.

**Live**: not required — the engine path is the stop track's, measured
(its §6.1); what changed is the orchestrator's bookkeeping after the
landing, under unit test. The LAN regression (`stop_silent_task_e2e_live`,
`silent_roll_e2e_live`, `background_subagent_e2e_live`; Qwen 3.6 27B, four
slots over 16384): 3/3 in 44.9 s.

**Rejected**: refunding always (a read window written twice — reflection's
observations duplicated with nothing to merge them unless
self-consolidation is on, which it is not by default); refunding only a
stop made while *waiting* (too narrow — most stops land in the first
stream, where nothing is written until it ends); restoring a counter
rather than adding it back (the landings during the run would be lost);
carrying the window through the loop (the loop would learn about chats'
watermarks); re-planning a stopped manual `/compact`; a note at the stop
saying the task returns (the feed is not a log); refunding on `Quit` (the
loop's fact is unavailable there — recorded as a later item).

### Post-M9: a quit gives the window back too — the fact the loop keeps in the open (done)

**What.** The refund track's fork F6b
([docs/research/stop-refunds-window.md](../research/stop-refunds-window.md)
§7): a stop gives a silent task's window back when no round of its tools
ran, but a quit cancelled every slot's token and returned — nothing landed,
the spawn-time advance stayed, and a restart mid-reflection skipped the
window as every stop used to. The design
[docs/research/quit-refunds-window.md](../research/quit-refunds-window.md),
every fork at its recommendation (the user's decision, 2026-09-08); no
stage-0 probe.

**The missing piece was a fact, not a decision.** "A round of the task's
tools ran" was `run_rounds`' own `round`, returned in
`RoundsEnd::Cancelled { rounds }` and read at the landing; a quit has no
landing, and waiting for one at the door would hold the exit for a round of
tools while the outcome channel's reader had already returned.

**How.** The spawn tail creates `acted: Arc<AtomicBool>`, hands a clone to
the loop (`SilentLoop.acted` → `run_rounds`) and keeps the original on the
slot beside the window — `Refund { window, acted }`, `begin_bg(kind, cancel,
Option<Refund>)`, the roll passing `None` as before. The loop stores `true`
at the very line it increments `round` — "a round of tools is about to run"
— so the flag and `rounds > 0` are one fact from one line; the landing keeps
reading the outcome's `consumed`, the quit reads the flag.
`cancel_all_bg(&self)` became `quit_bg(&mut self)`: every slot's token
cancelled, then every window whose flag is unset given back through the
same `give_back` a stop uses, before `run`'s exit flush writes the restored
watermark. The one race — the orchestrator reading `false` between the end
of a `ToolCalls` stream and the loop's store, then the tools writing into a
refunded window — is closed by a token check placed **after** the store:
the quit cancels before it reads, so a loop that stored after that read
sees the cancel at its check and starts no tools (`Cancelled { rounds }`
with the round counted — conservative, and consistent with the flag). On
its own the check also stops a task from running a round nobody will read
when a stop lands in a stream's last chunks.

**Tests**: +5 — `tests/silent.rs` (a loop whose first round calls a tool
has the flag set once its second stream opens, a loop stopped in its first
stream has it unset; through the orchestrator's own loop: a quit
mid-reflection leaves the chat on disk with the pre-spawn watermark and
stamp, a quit after a tool-calling round keeps them),
`tests/reflection.rs` (`quit_bg` after a spawn whose advance was already
flushed: the watermark and stamp back, the chat dirty, and after
`flush_saves` the file reads `None`; a slot whose flag is set keeps its
counter and the roll's slot is only cancelled) — **2937 unit tests green,
146 `#[ignore]`**. The guard's race itself is reasoned, not timed: the
recorder cannot end a stream `ToolCalls` after its token fired.

**Live**: not required — the loop gained a store on one line and a check
before its tools. The LAN regression (`stop_silent_task_e2e_live`,
`silent_roll_e2e_live`, `background_subagent_e2e_live`; Qwen 3.6 27B, four
slots over 16384): 3/3 in 40.1 s.

**Rejected**: refunding blind at a quit (a read window written twice at the
next launch — the defect the criterion exists for); waiting at the quit for
every task to land (the door held for a round of tools, the outcome
channel read by a loop that has returned); reflection's window only (one
path; a counter's refund at a quit is moot and harmless); deriving the
landing's `consumed` from the flag and dropping `rounds` (the last track's
shape stands, both set at one line); no guard before the tools (a race of
microseconds whose cost is one duplicate).

### Post-M9: "acted on" by effect — a silent task's window is consumed by a write, not by a round (done)

**What.** The item the refund track recorded and the quit track kept
([docs/research/stop-refunds-window.md](../research/stop-refunds-window.md)
§7, [quit-refunds-window.md](../research/quit-refunds-window.md) §7): a
stopped or quit silent task gets its window back unless it had *acted
on* it, and "acted on" had meant "a round of its tools was about to run" —
honest in the rule's direction, coarse the other way. Reflection's tool
set is half readers, and a first round that only looks (`get_self_model`,
a recall, a note's neighbours) is the common shape of a run stopped
seconds after it starts; under the round criterion that stop kept the
window for a run that changed nothing. The design
[docs/research/acted-by-effect.md](../research/acted-by-effect.md), every
fork at its recommendation (the user's decision, 2026-09-08); no stage-0
probe.

**Why not the mark that exists.** `Tool::concurrent()` (ADR 0012) is a
read-only claim for a different purpose, and not the reader set the loops
need: `note_recall` is deliberately unmarked ("it writes vectors inside a
read"), `note_neighbors` unmarked at all. A criterion on it would keep a
window that a recall merely searched.

**How.** The fact is the writer's own: `ToolOutcome` gained `wrote: bool`
(`false` from every constructor; `.wrote()` / `.wrote_if(created)`
builder-style), set on the success path of each of the nine memory
writers — `add_insight`, `update_self_model`, `update_user_model`,
`note_save`, `note_revise`, `note_supersede`, `note_merge`, `note_link`
(when the link is new), `note_cite_source` (when the citation is new) — on
the line that returns after the storage call; a refusal (a missing id,
nothing to change) reports nothing. `invoke_allowed` returns the fact
beside the result (`Ok(o) → o.wrote`; `Err → true`, since a tool that
failed may have written before it failed; a name outside `allowed` →
`false`), `run_tools` ORs it into the round's, and the loop accumulates
it: `RoundsEnd::Cancelled { wrote }` → `BgOutcome::Cancelled { consumed:
wrote }` — the round count had no reader left. Because the fact now
arrives *after* the tools, the slot's flag became a three-valued state,
`Acting::{Idle, InTools, Wrote}` behind `Acted(AtomicU8)`: the loop stores
`InTools` where the quit track stored `true` (the guard unchanged — the
token is checked right after the store), then `Wrote` if the round wrote,
else back to `Idle` unless already `Wrote`; `quit_bg` refunds `Idle` only,
so a quit during a round's tools keeps the window — the round may be
about to write, and the conservative side is the rule's. `handle_bg_done`
and `give_back` are untouched.

**Tests**: +4 net — `self_model.rs` (the reader reports nothing;
`add_insight` and a changing `update_self_model` report a write, an empty
edit does not), `notes/tests.rs` (`note_save`, a revise, a supersede and a
new link report; a revise of a missing id and an existing link do not),
`tests/silent.rs` (a round of reads — `get_self_model` allowed and run —
lands `consumed: false` with the state back at `Idle`; a round that wrote
lands `consumed: true` and stays `Wrote`; a loop stopped in its first
stream never left `Idle`; a disallowed call runs nothing; through the
orchestrator's own loop, a quit after a round of reads leaves the chat on
disk with the pre-spawn watermark and a quit after a write keeps it — the
two previous tracks' round-based tests replaced), `tests/reflection.rs`
(a quit during a round of tools keeps the counter) — **2941 unit tests
green, 146 `#[ignore]`**.

**Live**: not required — the loops' engine paths are the same. The LAN
regression (`stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live`; Qwen 3.6 27B, four slots over 16384):
3/3 in 61.4 s.

**Rejected**: a static per-tool claim (`Tool::writes_memory()`) — simpler,
coarser: a refusal would consume; reusing `concurrent()` (above); refunding
at a quit unless `Wrote` (a quit during a writing tool would refund a
window being written); an `Err` not counting (the loop cannot know how far
the tool got); keeping the round count beside the fact (no reader); marking
only the seven writers in the loops' sets (the contract would lie for a
later caller).

### Post-M9: the quit waits for the landing — a stop's own path decides, the state rule only past a cap (done)

**What.** The item the effect track recorded
([docs/research/acted-by-effect.md](../research/acted-by-effect.md) §7):
at a quit, a silent task whose round of tools was running (`InTools`)
kept its window because the round's write, if any, had not reported —
the conservative side of the rule, taken for a case a few hundred
milliseconds wide, and exactly the round the effect track was about (a
first round of reads interrupted while a recall's embedding is in
flight). The design
[docs/research/quit-waits-for-the-landing.md](../research/quit-waits-for-the-landing.md),
every fork at its recommendation (the user's decision, 2026-09-08); no
stage-0 probe.

**The observation.** A cancelled loop lands on its own, and fast: its lane
wait returns at once (`acquire_silent` selects on the token first), its
stream ends at the next chunk, its tools finish and the next wait returns
— and the landing arrives on `bg_done_rx`, the very channel `run` polls,
carrying the exact fact. The quit did not have to *decide*; it had to
listen a little longer before the exit flush and let the stop's own path
decide.

**How.** `quit_bg` became `cancel_bg_all` — every slot's token cancelled,
the refunds left on the slots. `run`, once its `select!` loop has broken
on the `Quit` arm and before `flush_saves`, calls
`settle_silent_tasks(&mut bg_done_rx, QUIT_SETTLE)`: while any slot is
active, `timeout_at(deadline, bg_done_rx.recv())`, each landing through
`handle_bg_done` — `consumed` from the loop, `give_back` on `false`,
nothing on `true`; over as soon as no slot is active, or at the cap.
`QUIT_SETTLE` is **2 s** in all: a task not in its tools lands in
milliseconds, a round of reads in tens, an embedding in hundreds on a GPU
host; two seconds covers a CPU embedding and is still a quit. Then
`refund_unlanded` — the old `quit_bg` minus the cancel — decides whatever
has not landed by its state: `Idle` given back, `InTools` and `Wrote`
kept. And `stream_round` checks the token before opening a stream, so a
loop cancelled during its tools lands without a request even where the
engine has no session budget (the lane wait already covers the pooled
ones). The roll's channel is not drained: a cancelled roll has no window.

**Tests**: +5 — `tests/silent.rs`, at the orchestrator level with a slow
test tool registered through `extra_tools` + `rebuild_registry` and a loop
calling it whose token and state sit on the reflection slot the way a
spawn leaves them: a quit mid-**reads** (300 ms under a 2 s cap) waits
for the landing and gives the window back, over in under 1.5 s and with
no request after the cancel; mid-**write** keeps the advance; a reader
that outlasts a 300 ms cap is decided by its state (`InTools`, kept) and
the quit is over at the cap; a loop cancelled in its stream lands within
milliseconds and is refunded; an unbudgeted loop cancelled before its
stream sends no request. The quit-track tests through `run` keep their
expectations under the new path. — **2946 unit tests green, 146
`#[ignore]`**.

**Live**: not required — the exit path. The LAN regression
(`stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live`; Qwen 3.6 27B, four slots over 16384):
3/3 in 37.9 s.

**Rejected**: polling the `Acted` state until it leaves `InTools` and
deciding by the state rule (a second decision path beside the landing's);
a `Notify` in `Acted` (the same second path); an unbounded wait (a quit
stays a quit); waiting only for `InTools` slots (the idle ones land at
once, and one rule is simpler); no request guard (a cloud request ended
at once still costs a connection).

### Post-M9: the quit's settle hears the roll, and its cap is a setting (done)

**What.** The two items the settle track recorded
([docs/research/quit-waits-for-the-landing.md](../research/quit-waits-for-the-landing.md)
§7). The design
[docs/research/quit-settle-roll-and-cap.md](../research/quit-settle-roll-and-cap.md);
the user's decisions (2026-09-08): F1 as recommended; **F2 — the cap in
the Tools group**, beside the other time limits, not the interface
section; **F3 — seconds, and no cap by default**, so a task caught
mid-tool finishes; F4 one PR.

**The roll was a regression.** The compaction roll takes a silent slot
like the three loops, but it lands on `compact_rx`, not on `bg_done_rx`;
`handle_compact_result` — a `select!` arm of `run` — applies a finished
summary and clears the slot. The settle track's `settle_silent_tasks`
listened on `bg_done_rx` alone, so a quit during an automatic roll — the
commonest silent task on a long conversation, firing exactly when it is
largest — waited the whole two-second cap for a landing that never came on
the channel it watched, and a roll that had finished just before the quit
was dropped with its summary unread. Recorded there as "a cancelled roll
has nothing to decide" — true of the window, wrong about the wait.

**How.** `settle_silent_tasks(&mut bg_done_rx, &mut compact_rx, cap:
Option<Duration>)`: a `select!` over both channels under one optional
deadline (`timeout_at` when there is a cap, a plain await when there is
none); a loop's outcome through `handle_bg_done`, a `CompactResult`
through `handle_compact_result` — a cancelled roll clears its slot at once,
a finished one is applied and marked dirty for the flush; `refund_unlanded`
unchanged (the roll has no window). The cap: `ToolsSettings.quit_settle_secs:
Option<u32>` (`#[serde(default)]`, `None`), a *Tools* row after the
dialogue's run time limit in the `Batch` shape — empty reads "until every
task has landed", a typed number is seconds, `0` included; `run` reads it
at the quit; `QUIT_SETTLE` is gone. The settings screen's Tools tab is in
the demo dumps, so the dumps and screenshots were regenerated.

**Tests**: +4 — `tests/silent.rs` (an automatic roll streaming at the quit
lands on its own channel and the settle is over in under a second, its
slot clear, nothing folded; a finished roll's `CompactResult` sitting in
the channel is applied by the settle and the chat marked dirty; no cap
waits for a 300 ms round of reads and refunds, a zero cap decides at once
and keeps a mid-tools task), `screens/settings/tests.rs` (the row follows
the dialogue time limit in the same group, empty by default, stores `5`
and `0`, an emptied field reads no cap, described) — **2950 unit tests
green, 146 `#[ignore]`**; the demo dumps regenerated.

**Live**: not required — the exit path and a setting. The LAN regression
(`stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live`; Qwen 3.6 27B, four slots over 16384):
3/3 in 50.8 s.

**Rejected**: marking the compaction slot landed at the cancel without
reading its channel (a finished summary would be lost); the cap in the
interface section (the user: every timeout lives in Tools); a default cap
(the user: better that the tool finishes); a ceiling on the value.

### Post-M9: a slow prefill, detected on the fly — the batch a cancel waits for, told to the user (done)

**What.** The item the batch track recorded
([docs/research/cpu-batch.md](../research/cpu-batch.md) §7). The design
[docs/research/slow-prefill-detection.md](../research/slow-prefill-detection.md),
every fork at its recommendation (the user's decision, 2026-09-08); stage
0 was a measurement made while designing — the signal is on the wire.

**Why.** The batch track fixed the one slow prefill it could see (`-ngl 0`
→ `-b 256`). A managed server with a partial offload, one on a slow GPU,
an external `llama-server` at the default batch, a CPU box in external
mode: each holds its slot for a whole batch when a stream is cancelled
during its prompt — tens of seconds — and none said so. The app cannot
change a launch flag at runtime and must not change a setting the user
did not type; it can measure and say.

**The signal.** `llama-server`'s OpenAI-compatible stream ends with a chunk
carrying `usage` **and** `timings`: `prompt_n` — the prompt tokens actually
processed, net of `cache_n`, the prefix reused from the slot — and
`prompt_ms`. Measured on the LAN stack before designing; the app's client
dropped it (`dialogue_probe.rs` read it by hand for its own purpose). No
other provider sends it, which is the quiet on the clouds by construction.

**How.** `TokenUsage.prefill: Option<Prefill { tokens, ms }>` from the
wire chunk's `timings` (`ChatCompletionChunk.timings`, `Timings { prompt_n,
prompt_ms }`), `None` from the other four clients; the turn loop keeps the
round with the largest `prompt_n` (`RoundOutput.prefill` →
`TurnUsage.prefill`, a session's first round on a cold cache), and
`handle_done` hands it to `note_slow_prefill` ahead of the roll. The rule
is pure, beside the batch it needs (`shared/api/managed.rs`):
`launched_batch(batch_size, gpu_layers)` — the typed number, else
`CPU_BATCH` at `-ngl 0`, else `LLAMA_DEFAULT_BATCH` = 2048 — and
`prefill_hold(batch, prefill)`: the hold `batch / tps` in seconds, said
when the sample is at least `PREFILL_SAMPLE_MIN` = 256 processed tokens
(the per-request overhead read as 154 tok/s on the 4090 over a 16-token
prompt), the hold above `PREFILL_HOLD_LIMIT_SECS` = 5, and the batch above
the knee. An external server's batch is assumed 2048 (`/props` does not
expose `n_batch`), and the note says so. One `AppEvent::Notice` per chat-
server session — `EngineManager.prefill_noted`, cleared in `note_recovery`
when the chat server reaches `Ready` — naming the throughput, the hold,
the batch and the route: the *Batch (-b)* field for a managed server,
`-b 256 -ub 256` on the launch line for an external one; the raw figures
go to the log every time.

**Tests**: +8 unit (the wire's `timings` beside `usage` and their absence;
the client's `prefill` and `None`; `launched_batch`; the rule over the CPU
build's figure, the knee, a GPU's second, a short sample, a typed 1024;
the note once per managed session and again at `Ready`; nothing at the
knee, the CPU auto, a fast prefill, a cloud, no sample; the external
wording; the whole path from a scripted usage chunk to the note and
silence on the second turn) — **2958 unit tests green, 147 `#[ignore]`**.

**Live — GO.** `slow_prefill_e2e_live` (external mode, a forty-paragraph
seed; `MINDFORK_EXPECT_SLOW_PREFILL` names the host's expectation): the
**CPU build** (b10807, Gemma 3 4B Q8_0, `-ngl 0 -c 2048`, no `-b`) — the
turn 37.9 s, the reply *Lamp*, the note: **38 tokens/s, a hold of 54 s at
the default batch of 2048**, the launch-line wording; the batch track's
"a 1400-token prompt already takes 38 s" is the same figure from the
engine's clock. The **LAN stack** (Qwen 3.6 27B, the 4090) — the turn
4.4 s, the reply *lamp*, no note; with the regression trio 4/4 in 49.8 s.

**Rejected**: time to first token over the app's estimate (a byte ratio,
the cache invisible); a throughput floor alone (ignores the batch the
user set); skipping external servers (the CPU-only external box is the
user the batch track could not reach); setting the field and restarting
(a restart nobody asked for); a hint on the settings field only (read by
nobody at the moment it matters); reading the roll's timings (its usage
is not carried — the turn's first round is the sample on every session).

### Post-M9: the roll's timings — the session's coldest prompt, sampled (done)

**What.** The item the slow-prefill track recorded
([docs/research/slow-prefill-detection.md](../research/slow-prefill-detection.md)
§7, fork F5b). The design
[docs/research/roll-timings.md](../research/roll-timings.md), every fork at
its recommendation (the user's decision, 2026-09-08); stage 0 a
measurement on the GPU stack made while designing.

**Why.** The detection track samples the turn's rounds, on the reading that
a session's first turn processes the system prompt and the history cold.
It does on a fresh server. On one that kept running — the external mode's
ordinary day, the app reconnecting to a `llama-server` that never stopped —
the chat's prefix is still in a slot's cache, and every turn of the session
processes only its own tokens, under the rule's 256-token floor: the rule
stays silent for the whole session, on exactly the CPU-only external box
the batch track could not reach. The compaction roll sends a *different*
prefix — the summarizer's system prompt and a digest that is new every
roll — so its prompt is processed cold, the largest a session makes; and
it is the very stream the note is about, the one a turn displaces. Its
`Usage` chunk was matched and dropped.

**Measured (stage 0, the LAN stack, Qwen3.6-27B, 4 slots).** A cold turn
with a forty-paragraph seed: 1430 tokens at 2351 tok/s. The same turn
again: 4 tokens, 38 tok/s — the per-request overhead. A second turn on the
warm prefix: 31 tokens, 125 tok/s. A roll-shaped request — the summarizer's
system prompt and a digest of the same text: 1466 tokens cold at 2335
tok/s; again, 4 tokens. Three readings: the warm turn is under the floor,
and rightly (its figure is overhead, not throughput); the roll is cold by
construction and gives the cold turn's figure; the signal arrives on the
roll's stream as on a turn's — the same client, the same `include_usage`.

**How.** `collect_roll`, lifted out of `spawn_compact` (the analyzer's
complexity bar was near), reads one attempt at the stream into
`Collected { text, thoughts, truncated, cancelled, prefill }`, the last off
the `Usage` chunk — which the client hands over *before* `Finished`, since
it reads on past `finish_reason` for exactly that chunk. The loop's break
value became `(text, prefill)`; `CompactResult` gained
`prefill: Option<Prefill>`, `None` on a cancelled, failed or timed-out
attempt (the usage chunk is the stream's last, so a stream that ended
short never received it; a displaced roll's retry is a new collect);
`handle_compact_result` offers it to `note_slow_prefill` after
`handle_bg_done`, so `Compacted` precedes the note in the feed, and offers
it whatever the landing made of the text — a summary discarded for a
vanished boundary was still a prompt the engine processed at its speed.
Nothing in the rule changed: the same batch reading, the same floor, the
same one claim per server session.

**Live.** The CPU build (gemma-3-4b-it Q8, `-ngl 0 -c 2048`, the default
batch): a chat of four exchanges seeded on disk, the app started on it,
`/compact` the session's first request — the roll 78.7 s, six messages
into 1457 chars, and the note from the roll: 37 tok/s, a hold of about
55 s at 2048, `-b 256 -ub 256` — the figure the turn gave on the same
server in the previous track (38, 54 s). The GPU stack: 2.7 s, 304 chars,
no note. The LAN regression pair after it, 2/2. One run the smoke lost
while being written: a data root seeded with chats and no `data.db` makes
the bootstrap put its own notice in the feed after activation, which a
wait for "the first `Notice`" took for the roll's; the smoke opens the
storage once before the app starts. Unit: 2962 green, 148 ignored.

**Rejected**: the whole `TokenUsage` on the result (the roll's cost has no
consumer today; the budget's calibration is fed by turns and loops, and a
digest is not the request shape the ratio is calibrated on); the three
loops' samples through `BgOutcome::Done` (four constructors and every test
on them, for a reflection's first round that prefills a window the turn
before it usually already processed); offering only an applied summary
(ties the engine's fact to a check about the chat); no live gate (the
unit tests script the figure, the live arm is what shows it arriving on a
real roll's stream).

### Post-M9: the loops' timings — the silent tasks' cold prompts, sampled at the landing (done)

**What.** The item the roll-timings track recorded
([docs/research/roll-timings.md](../research/roll-timings.md) §7, fork
F2b). The design [docs/research/loop-timings.md](../research/loop-timings.md),
every fork at its recommendation (the user's decision, 2026-09-09); stage
0 a measurement on the GPU stack made while designing.

**Why.** The slow-prefill note is computed from one sample per server
session. The turn gives it on a fresh server, the roll on a warm one — and
the warm server's ordinary day has neither: the app reconnecting to a
`llama-server` that kept running, a conversation under the compaction
threshold, every turn processing tens of tokens under the rule's floor.
The three silent loops fire there on their own cadence, each with a
prefix that is nobody else's — its instructions, its tool schemas, a
digest — so each first round is processed cold and whole, the largest
prompt a session makes. `record_round_usage` read that usage for the
budget's calibration and kept nothing else; the loop landed as
`(BackgroundKind, BgOutcome)` and the figure was gone.

**Measured (stage 0, the LAN stack, Qwen3.6-27B, a scratch print of each
round's usage, reverted).** The reflection after one short turn: round 1
2798 prompt tokens, all 2798 processed, 1054 ms — 2655 tok/s; round 2
2839 prompt tokens, 45 processed — the tool result on the warm prefix;
round 3 cut by the quit, no usage. Against the day before: a cold turn
1430, the roll 1466, a turn on a warm prefix 31. Three readings: the first
round is the sample, the later rounds ride the cache, and a stream that
ends short has nothing to give — the usage chunk is the stream's last.

**How.** `run_rounds` takes an out-parameter `prefill: &mut Option<Prefill>`
— the shape `run_tools` already has for `wrote` — and `record_round_usage`
keeps the round's sample on it when its `tokens` exceed what is there; an
out-parameter rather than a field of `RoundsEnd`, since the sample must
survive every way out of the loop (`Done`, `Cancelled`, `TimedOut`, the
`?` on a stream error) and the sample is the engine's fact, not the
task's verdict. The channel's tuple became `BgDone { kind, outcome,
prefill }`; `handle_bg_done(kind, outcome, prefill)` offers the sample to
`note_slow_prefill` at its end, after the slot's events, for every kind —
and the explicit offer in `handle_compact_result` went: the roll passes
`CompactResult.prefill` through the same landing. Nothing in the rule
changed.

**Live.** `loop_prefill_e2e_live`, two phases on one data root: phase 1 a
long turn with the memory tools enabled (the reflection is gated on the
profile's tool set, and the schemas are part of the prefix phase 2 must
find warm), no reflection, a quit; phase 2 the app restarted on the same
root against the same server, a short turn, the reflection at its landing.
The CPU build (gemma-3-4b-it Q8, `-ngl 0 -c 8192`, the default batch):
the turn 35.2 s with **52 tokens** processed — under the floor, and
rightly, since the server was prefilling phase 1's cold title request
beside it and 3 tok/s is not its throughput — the reflection landed 54 s
later, and the note came from that landing: 35 tok/s, a hold of about
58 s at 2048, `-b 256 -ub 256`. A second CPU run with a print of every
offer said so exactly: the reflection's first round 1664 prompt tokens
(Gemma's tokenizer; Qwen's 2798), 514 processed in 15.0 s — 47 s of
prefill for the whole prompt on this host, inside the loop's 120 s limit,
so the design's §4 risk did not bite. The GPU stack: the turn 3.0 s, the
reflection 38.9 s later, no note. The LAN trio 3/3. Unit: 2967 green,
149 ignored.

**Rejected**: the sample inside `BgOutcome::Done { prefill }` (the item's
own label — a stopped, timed-out or failed loop would drop its sample,
and keeping the roll's R3 would put the field on `Failed` too); a cell on
the slot as `Acted` is (right for a fact read before the landing; nothing
reads this one early); the first round's only (the same figure today,
and a rule to explain); each spawn's own landing offering (two sites for
one rule); reflection only; no live gate.

**Two traps, both recorded.** A smoke that expects a silent loop must
enable the profile's tools first, and wait for the spawn before the
landing — "never spawned" and "never landed" are otherwise one silence
(lessons §9). And a scratch print is removed by reversing its
replacement, never by `git checkout --` of a file that carries the
stage's uncommitted work (lessons §1): the checkout restored HEAD and
took the stage's patch with the print.

### Post-M9: the roll's usage for the budget — and the estimate it would calibrate (done)

**What.** The item the roll-timings track recorded
([docs/research/roll-timings.md](../research/roll-timings.md) §7). The
design [docs/research/roll-usage-calibration.md](../research/roll-usage-calibration.md),
every fork at its recommendation (the user's decision, 2026-09-09); stage
0 a measurement that turned the item on its precondition.

**Why.** The session budget prices every stream's reservation from the
app's prompt estimate scaled by the latest exact-to-estimate ratio a
round has recorded — "the tokenizer's density on this conversation's
text" (admission-by-budget §4.3), floored at 1.0. The roll priced with it
and never recorded; the question was whether it should.

**Measured (stage 0, the LAN stack, Qwen3.6-27B).** Every calibration of
one live session beside the request's parts, a scratch print reversed by
its own script: a fresh chat's first request estimated **85** tokens
against **4358** exact — 24 tool schemas, 18 116 bytes of JSON, about 4270
tokens the estimator never counted (`estimate_prompt_tokens` summed the
system message, the message texts and the tool-call arguments, not
`ChatRequest.tools`). Three readings. The first request of an app session
is priced at a fiftieth of its size, with no ratio yet to correct it. The
ratio is the overhead in disguise — 51 falling to 6 across the
conversation as the messages grew under a constant 4270 — and works only
because consecutive turns carry the same schemas. A request without
schemas is priced by a ratio about schemas: the roll estimated 1106,
measured 710, and reserved `1106 × 6.03 + 2048 = 8722`; had it recorded
its own ratio (0.64, floored to 1.0) the next turn would have been priced
905 against 5204 exact — the under-count R6 exists to prevent. With the
schemas counted at four bytes a token the turn's estimate lands 6–12 %
over the exact, and the roll's price falls to 3109.

**How.** `wire::tools_json` renders the tool block exactly as the request
builder puts it on the wire — one `wire_tools` behind both — and
`estimate_prompt_tokens` feeds it to `estimate_prompt` as one more part
at the text's four bytes a token; no constant of its own, the calibration
absorbs the 7 % the compact JSON runs denser. `Collected` carries the
usage chunk whole (`usage`, the prefill read off it where it was), and
`spawn_compact` records `(estimate, prompt_tokens)` on the budget when an
attempt reaches its usage — the loops' line, where the reservation was
priced. The title and impersonation are left alone (the one's ratio is
noise at its size, the other's is the turn's own text, already recorded).

**Live.** `prompt_estimate_e2e_live` — a probe engine that keeps every
request beside the exact count the server reports, three turns and a
roll on the GPU stack: the turns 0.91 / 0.89 / 0.88 exact over estimate,
the title 0.61, the roll 0.59; no under-count. The pair after it 2/2.
Unit: 2969 green, 150 ignored. One test moved with the estimate — the
child-reservation test had set its pool on "about 2100" per child, the
estimate without the schemas; at the new size two raw streams need a
16 000 pool and the floored round reports 12 000 to stay out. One probe
run was lost to the probe: the consumer drops a stream at `Finished`
without reading it to its end, so a record after the loop never ran —
it records at the usage chunk.

**Rejected**: leaving the estimator and keeping the roll out (the first
request stays a fiftieth under, the roll nine times over); `/tokenize`
(a round trip per request, no cloud); recording from the title and
impersonation; a bytes-per-token constant for JSON; at-the-landing
recording (no estimate there); no live gate.

**The lesson (§3).** A ratio that "calibrates" one shape can be a missing
term in disguise: it corrects the requests that share the term and
misprices every other. Check what the estimate counts before trusting
what the ratio corrects — the check here was one print of the request's
parts beside its exact count.

### Post-M9: the title's and impersonation's usage for the budget — and the ratio they would erase (done)

**What.** The item the roll-usage-calibration track recorded
([docs/research/roll-usage-calibration.md](../research/roll-usage-calibration.md)
§7, fork F2b). The design
[docs/research/title-impersonation-usage.md](../research/title-impersonation-usage.md),
every fork at its recommendation (the user's decision, 2026-09-09);
stage 0 a measurement that turned the item on the budget's one rule for
its ratio.

**Why.** Since the estimate counts the tool schemas, a request's
exact-to-estimate ratio is the tokenizer's density on that request's
text — the turns 0.9, the prose requests (the roll, the title,
impersonation) about 0.6 — and the budget keeps **one** ratio, written by
the turn's rounds, the loops' rounds and the roll, floored at 1.0, the
latest winning. The title and impersonation never recorded theirs; the
question was whether they should.

**Measured (stage 0, the LAN stack, Qwen3.6-27B).** The two: the title
0.65, impersonation 0.59 — the app's most over-counting requests. And a
turn the other way: a message carrying 25 KB of JSON — the shape of a
tool result — estimated 11 058 tokens against 14 767 exact, **1.34**
(JSON runs at about 2.4 bytes a token on this tokenizer). A turn of Rust
code stayed at 0.91: at that size the schemas dominate. Under "the
latest wins" the next silent request after the JSON turn — the roll at
the landing, the title, an impersonation — records 0.6, stores 1.0, and
the following turn's first round is priced a quarter under its size: the
under-count R6 exists to prevent. The hazard is the rule's, not the
item's — the roll and the loops record already — but the item as named
adds the two requests that trip it most surely.

**How.** `SessionBudget` keeps one ratio per **shape** —
`Shape::{Turn, Run, Loop, Roll, Title, Impersonation, Summary}`, one
atomic each — and `price`, `record_usage` and `density` take the shape;
the rule inside a shape is the rule as it was. The turn's rounds record
`Turn`, a child run's `Run` (`TurnLoop::shape` reads its depth), a
loop's `Loop`, the roll `Roll`; the title's collect records `Title` at
its usage chunk, impersonation's `run` records `Impersonation` there
when it has a budget (the shared engine — a separate impersonation
server has no pool and no budget), and the page summary inside a tool
prices as `Summary` and records nothing, as before. A shape's first
request prices at its raw estimate; nothing crosses shapes.

**Live.** `prompt_estimate_e2e_live`, extended — two prose turns, a
third carrying the catalogue, `/compact`, an impersonation: the prose
turns 0.91 / 0.90, the JSON turn **1.32**, the title 0.64, the roll 0.60,
impersonation over the JSON **1.61** — the ratio its own kind now keeps,
and the one that would have priced the next turn under the old rule. The
pair after it 2/2. Unit: 2972 green, 150 ignored. One test was rewritten
with the rule it asserted: the parent's exact usage had been expected to
price the children's reservations; now the parent's record is the
parent's, the children still fit, and a child's own record is what makes
its second round wait — both halves asserted. One unit run was lost to a
fixture: a bare orchestrator's chat is not active until the test says
so, and `handle_impersonate` on no active chat sends an error and
returns.

**Rejected**: one ratio, the latest wins (the hazard accepted; measured,
a title at 0.65 after a turn at 1.34 prices the next turn a quarter
under); one ratio that an over-count never overwrites (sticky for the
session — a JSON chat's 1.34 pricing every later prose chat a third over
until restart); two populations, with and without schemas (a loop's
first round, schemas over a prose digest, would still erase the turn's);
the silent lane's label as the key (the turn and the runs have none, and
a map under a mutex on every price); the children on the turn's ratio
(a persona's prompt and the turn's tools are a population of their
own); not recording the two (their record is 1.0 on any measured text —
closed by the population alone, but every request the server vouched
for records).

### Post-M9: the page summary's usage — the one kind that under-counts, and a summary that came back empty (done)

**What.** The item the title-impersonation-usage track recorded
([docs/research/title-impersonation-usage.md](../research/title-impersonation-usage.md)
§7): the page summary inside `fetch_url` prices its reservation under
`Shape::Summary` and never records its exact `usage`. The design
[docs/research/page-summary-usage.md](../research/page-summary-usage.md),
every fork at its recommendation (the user's decision, 2026-09-09); stage
0 a measurement that turned the item's reading over and found two more
things beside it.

**Measured (stage 0, the LAN stack, Qwen3.6-27B).** Five pages through
`fetch_url` under a budget, a scratch probe at the `Usage` chunk: a
documentation page 1.04, Cyrillic prose 0.76, an English article with its
citation marks 1.16, an API's JSON 1.28, a Rust source 1.17. The reading
that left the kind unrecorded — "a request of prose, over-counted" —
holds for one page in five: the summary's text is a *page*, and the
summary is the one kind whose ratio runs above 1.0 as a rule (the turn's
does only with a tool result's JSON in it). Priced at 1.0, a JSON page's
summary reserved 28 % under its size — under a shared pool the
under-count the budget calls the failure. Beside it: on the thinking
model the summary came back **empty** on both pages tried — the whole
768-token cap spent on thoughts, `finish = Length`, no text — where every
other one-shot request (the title, the roll, impersonation, the director)
mutes reasoning up front; muted, the same pages summarized in 587 and 161
tokens. And the summary's prompt is cold and large — 3236 tokens for a
12 000-character head, above any silent task's first round — and its
`Usage` chunk was dropped, so the slow-prefill note never saw it; a page
fetched again minutes later rode a slot's prefix cache (4–6 tokens
processed).

**How.** `summarize_text` records `Shape::Summary` at the `Usage` chunk
beside the estimate its reservation was priced from, keeps the chunk's
`prefill`, and returns both (`Summarized { text, prefill }`); its sampling
carries `reasoning_budget: Some(0)`, the title's shape. `ToolOutcome.prefill`
is the `wrote` shape — a fact the tool reports for the loop that called
it, `with_prefill` set by the two callers, `None` from the constructors —
and the loops fold it: the turn's `keep_tool_sample` (a `CallDone.prefill`
out of a concurrent segment or the parallel group, the sequential path
inline) into `last_usage.prefill` through `Prefill::keep_larger`, the one
rule the turn's rounds and the loop's `record_round_usage` now share; the
silent loop's `run_tools` into `ToolsReport { wrote, prefill }` — the
round's two reports in one struct, clippy's argument bar — which
`run_rounds` folds into its out-parameter *before* the timeout check, so a
round the clock cut short still lands what its finished calls said. A
background run's landing and `run_child`'s `CallDone` carry none (§7).

**Live.** `summary_usage_e2e_live` (fork F4a: `fetch_url` invoked directly
under a budget of four over 16 384, the JSON page): the LAN stack 8.7 s,
exact 2286 against 1816, **1.29** in the `Summary` slot, the sample 2286
tokens in 1203 ms, a seventeen-line summary; the CPU build 86.2 s, 2735
against 1816 (Gemma's tokenizer, **1.51**), the sample 2735 tokens in
76 139 ms — 36 tok/s, the note's figure — a four-sentence summary, **4 s
inside** the summary's 90 s limit, as the design counted. Unit: 2978
green, 151 ignored (2973 / 150 before).

**Not in this track** (§7): the one-shot requests' own samples
(impersonation's — the whole conversation, cold — and the title's), a child
run's sample, the summary's limit against a slow host (4 s to spare here;
the loop's limit's sibling), the first request of a kind, and
`reasoning_tokens` reading 0 on a stream of thoughts.

### Post-M9: the one-shot requests' samples — impersonation's prompt is the session's largest, and its own twice (done)

**What.** The item the page-summary-usage track recorded
([docs/research/page-summary-usage.md](../research/page-summary-usage.md)
§7): the automatic title and impersonation still dropped their `Usage`
chunk's timing where every other cold prompt the app makes offers its
sample to the slow-prefill note. The design
[docs/research/oneshot-samples.md](../research/oneshot-samples.md), every
fork at its recommendation (the user's decision, 2026-09-09).

**Measured (stage 0, the LAN stack, Qwen3.6-27B).** Every request of the
estimate smoke's session with the engine's timing, and a second
impersonation right after the first: the fresh chat's first turn 4349
tokens cold (its schemas), the second turn 94 warm, the JSON turn 10 428,
the title 200 (under the rule's 256-token floor), the roll 434,
**impersonation 10 642** — the whole conversation with the roles swapped
under its own system, nothing of it in any slot's cache, the largest
prompt of the session — and the second impersonation **4**: the swapped
prefix stayed in a slot. On the CPU build's 38 tok/s that impersonation
is 280 s, inside its 600 s limit: every first `Ctrl+U` on a long chat
pays the whole conversation's prefill, the very figure the note exists to
tell. The title's digest is capped at 4000 characters — a thousand tokens
at most — so its sample clears the floor only on a long opening.

**How.** `spawn_impersonation` keeps the chunk's `prefill` under the
record's own condition — a budget, which is exactly the shared engine,
the one server whose batch the rule names and whose session the claim
counts (fork F3) — and lands `ImpDone { id, reason, prefill }`;
`handle_imp_done` emits `ImpersonationFinished` and then offers the
sample, the roll's shape; a superseded generation lands nothing but its
prompt was processed on this server, so its sample is offered too. A
separate impersonation server has a batch and a session of its own the
rule does not know: `None` there (F1). The title's rides
`TitleResult.prefill`, the largest across its attempts
(`Prefill::keep_larger`; a displaced attempt's usage never arrives), and
`handle_title_result` offers it after `apply_title_result` whatever the
text made of it — an error result's prompt was processed all the same
(F2).

**Live** (F4a, two seeded smokes — no turn before the request, since a
turn's cold schemas would claim the session's one note first). The 4090:
impersonation 8.2 s, the title 1.2 s, no note. The CPU build:
impersonation 66.7 s — 1246 tokens in 34.3 s, 36 tok/s — the note (a
56 s hold at the default batch) after `ImpersonationFinished`; the title
32.2 s — 1119 tokens in 31.7 s, 35 tok/s — the note after `ChatRenamed`.
The CPU arm's first run answered `400` before any prefill: Gemma's
template refuses the swapped conversation of a chat that opens with the
user's message (`Conversation roles must alternate`) — the seed gained an
assistant opener, and the defect is recorded in the design's §7 as
impersonation's own on that template family. Unit: 2982 green, 153
ignored (2978 / 151 before).

**Not in this track** (§7): a rule of the impersonation server's own
(with the own-engine question), impersonation's limit against a slow
host, a child run's sample, the first request of a kind, and
impersonation on Gemma's template.

### Post-M9: impersonation on Gemma's template — the swapped conversation must open with the assistant's silence (done)

**What.** The defect the one-shot samples track found in passing
([docs/research/oneshot-samples.md](../research/oneshot-samples.md) §7):
impersonation swaps the roles of the history, so a chat the user opened
— nearly every chat — became a conversation that opens with an assistant
turn, and the Gemma 3 chat template refused it with a `400` before any
prefill. The design
[docs/research/gemma-impersonation.md](../research/gemma-impersonation.md),
every fork at its recommendation (the user's decision, 2026-09-09).

**Measured (stage 0).** Five role shapes through `/v1/chat/completions`
on Gemma 4 31B (the LAN stack) and Gemma 3 4B (the CPU build): Gemma 4's
template (no `raise_exception`, `assistant` rendered as `model` wherever
it stands) takes every shape; Gemma 3's refuses a leading assistant turn
and two same-role turns in a row (`Conversation roles must alternate`),
and delivers the system prompt only inside a leading user turn — without
one the persona never reaches the model: 4 tokens of prompt for a
54-token system, and a reply about a ceramic mug. Then three
impersonation shapes on a four-message user-opened chat: the natural one
a `400` on Gemma 3; the human's opening folded into the persona, and an
empty opening user turn, both the reply the natural shape gives on
Gemma 4 (135 tokens against 130) and a good reply on Gemma 3 (127). The
fold needs no provider branch — Anthropic rejects empty text — so it is
the one taken.

**How.** `alternate_for_template` in `impersonation.rs`, applied by
`build_impersonation_request` after the user hint and before the seed's
hint, for every provider (F2): adjacent same-role turns merge with a
blank line (F3); a leading assistant turn that a user turn follows moves
into the persona as one localized sentence, `prompt.impersonation.opening`
(F1). A chat with only the user's opening stays a lone assistant turn —
both templates accept it, the model continues it, and the fold with an
empty user turn behind it made the 4B model answer as the assistant once
(F4). Two tests that pinned the leading assistant turn now pin the fold;
the compaction cut — always on a user message — folds the same way, the
tail still ending on `user`.

**Live** (F5a). The one-shot smoke's seed lost its assistant opener:
Gemma 4 a reply in 1.5 s; Gemma 3 on the CPU build a reply in 69.8 s —
1246 tokens in 38.3 s, 32.5 tok/s, the slow-prefill note — and no `400`
in the server's log. `prompt_estimate_e2e_live` on Gemma 4: the prose
turns 0.79 / 0.78, the JSON turn 1.25, the title and the roll 0.62,
impersonation over the JSON 1.60 — its classifier reads the folded
system too, since the roll's cut lands on the JSON message. Unit: 2986
green, 153 ignored (2982 / 153 before).

**Not in this track** (§7): the opening-only chat on Gemma 3 (the
persona dropped with the lone turn), a dialogue's first line (a system
with no turns, unmeasured), the clouds re-measured.

### Post-M9: the engine, downloaded — llama.cpp's backends named, fetched and pointed at (done)

- **Why**: install.md §3 named llama.cpp `llama-server` as the recommended
  backend and then assumed it existed. A user picking **managed** mode was told
  to type a path into *Model/server → llama-server binary* and left to find the
  binary themselves — which means knowing that the newest *tagged* release is
  not the newest build, that `bin-win-cuda-12.4-x64` is a server build while
  `cudart-…-cuda-12.4-x64` is the runtime it will not start without, and which
  of seven Windows archives to take. The app already downloads, verifies and
  unpacks the other half of the local stack (`mindfork sandbox setup`, ADR
  0005); this is the same shape for the engine. Research, with the release
  surface measured on 2026-09-09:
  [docs/research/llama-cpp-download.md](../research/llama-cpp-download.md);
  every fork at its recommendation (the user's decision, 2026-09-09).
- **The command** (`features/llama_setup.rs`, CLI in `features/cli.rs`,
  dispatch in `main.rs`): `mindfork llama backends [--build <tag>]` /
  `llama setup --backend <id> [--build <tag>] [--no-cudart] [--force]` /
  `llama installed`. `setup` takes the single-instance guard, prints through the
  `impl FnMut(&str)` callback `sandbox_setup` established (the only `println!`
  is `main.rs`'s), and installs into `data/llama/<backend>-<tag>/`
  (`Paths::llama_dir`).
- **Derivation, not a table.** The sandbox's asset list is four pinned rows
  edited by hand at a version bump. llama.cpp publishes ~13 nightlies a day
  across 27 assets, and the *names have drifted twice in fourteen months*: the
  Linux/macOS archives were `.zip` at `b6000` (2025-07) and are `.tar.gz` now,
  and the AMD build went `win-hip-radeon-x64` → `win-rocm-10.0-x64` between
  `b9000` and `b10883`. So the backends are read out of the release, by a parse
  anchored on **both** ends — `llama-<tag>-bin-<os>[-<backend…>]-<arch>.<ext>`,
  the OS and arch tokens matched exactly, the middle joined with `-`, an **empty
  middle meaning `cpu`** (which is how the Linux CPU build spells itself:
  `llama-b10883-bin-ubuntu-x64.tar.gz`). Everything that does not match is
  skipped rather than interpreted, and that single rule drops `ui`,
  `xcframework`, `android-*`, `ubuntu-s390x`, `310p-openEuler-x86`,
  `910b-openEuler-x86-aclgraph` and `macos-arm64-kleidiai` without a special
  case for any of them. The archive kind is read off the name too, so a return
  to Linux zips costs nothing. All three real name sets are test fixtures.
- **The API, not the releases page** (fork F1 — the sketch asked for the HTML).
  Measured: `releases?per_page=1` is 63 KB of JSON against the index page's
  490 KB, and — decisively — it carries `digest: "sha256:…"` per asset, which
  the HTML does not. Without it the integrity promise SECURITY.md makes would
  fall back to a hand-maintained pin table, i.e. exactly the thing the naming
  drift says cannot be maintained. An asset the release publishes no digest for
  is not installed. Cost: 60 requests an hour per address unauthenticated — one
  per invocation, so only a shared address reaches it; the refusal says so in
  words and `GITHUB_TOKEN` lifts it.
- **`releases/latest` is not a build.** Every `bNNNNN` tag is a *prerelease*, so
  `latest` returns the semver release (`v0.4.0`) whose one asset is a 7-byte
  `nightly-tag.txt`. Resolution is therefore `releases?per_page=5`, taking the
  newest entry that actually yields backends — five, so a semver release landing
  on top costs a retry rather than a failure.
- **CUDA arrives complete or not at all.** `ggml-cuda.dll` links
  `cublas`/`cudart`, which upstream ships in a separate `cudart-…` archive
  (paired by backend token and arch, and carrying no build tag in its name).
  Without it nothing errors — the backend fails to load and the server runs on
  the CPU. So it is fetched with the build by default, a `cuda-*` release
  missing it is **refused**, and `--no-cudart` is the opt-out for a host that
  already has the runtime.
- **Proved after the fact, on the binary itself.** `llama-server --version`
  (stderr) must report the tag's build number — that is also the only thing that
  proves the ~50-file library set is complete — and it runs on the staged
  directory *before* the rename, so a broken unpack never becomes an install
  `llama installed` would list. Then `--list-devices` (stdout) is reported; on a
  non-`cpu` backend an empty list is named out loud without failing the install,
  since the files are correct and what is missing is on the host.
- **Two gaps in the precedent deliberately not inherited.** `sandbox_setup`'s
  client sets no timeout of any kind and its download truncates through
  `File::create`. Here: `connect_timeout(10s)` + `read_timeout(60s)` (the gap
  between chunks, not the transfer — 645 MB on a slow line is not an error), and
  a `.part` file resumed with a `Range` request, the hasher seeded from what is
  already on disk. A digest that still mismatches after a resume costs one retry
  from zero; a mismatch on a fresh download is fatal.
- **One rule for two layouts.** The Windows zip is flat; the Linux tarball wraps
  everything in `llama-<tag>/`. Rather than a second pass over the stream, the
  archive is unpacked and then collapsed — "one directory and nothing else"
  means that directory is the payload. Zip-slip is guarded with
  `enclosed_name()`; the zip path sets the mode explicitly on unix, which
  matters only if upstream returns to Linux zips but is exactly the silent
  breakage that would then follow.
- **Where it lands.** `data/llama/<backend>-<tag>/` — self-describing, several
  installs coexist (which is what makes a rollback one command), and removal is
  one directory. It inherits `data/`'s properties for free: `features::backup`
  works off an allow-list, so nothing there is packed into an archive or removed
  by a restore. `Command::new(path)` needs no change — read out of the ELF of
  `b10883`'s Linux `llama-server`, `DT_RUNPATH` is `$ORIGIN`, so the binary
  finds its siblings without `LD_LIBRARY_PATH`.
- **The bundle namespace is `llamacpp.`, not `llama.`** — lessons.md §7's
  prefix-collision trap, and this feature is the likeliest place to hit it: the
  gate reads any dotted literal under a bundle prefix as a key, and
  `"llama.dll"` is a real entry of the archives being unpacked (with
  `llama.exe` one fixture away). Renaming the namespace, not growing the
  scanner's whitelist — the answer the `compact.db` case already recorded.
- **Stage 2 — `--set-binary`.** The path goes into the managed configs after
  a successful install, past the `?`: `engine.managed.binary` always,
  `impersonation_engine`'s and `embed`'s only when empty (one install serves all
  three servers, but a path the user typed there is a deliberate choice — fork
  F6). Fork **F7 turned on reading the code**: it had been recommended as "set
  the mode to managed when it is still at its default", and `ServerMode::Managed`
  *is* the default — so the rule reduces to a no-op for anyone who has not
  switched, and for anyone who has, switching them back would undo a deliberate
  act. The mode is therefore never touched and the state is reported instead.
  The two CLI writers of user data (`--enable-python` and this one) now share
  `open_config_for_cli_write`, so neither can drift from the ADR 0006 downgrade
  guard and the fresh-file language seeding.
  Live: on a config in `openai` mode with an embedder path already typed —
  assistant overwritten, impersonation filled, **embed left alone**, mode
  untouched and named in the output.
- **What is not here**: GPU auto-detection (nothing in `src/` knows about CUDA,
  Vulkan or ROCm, and the choice is the user's anyway — fork F3: an omitted
  `--backend` prints the list and exits `2`), a settings-screen button, model
  downloads, `llama remove`, HTML scraping.
- **Live run (2026-09-10), Windows 11, `gemma-3-4b-it-q8_0` on the CPU.**
  `llama backends` → build b10883, seven backends for windows/x86_64.
  `llama setup --backend cpu` → 17 MB, unpacked, `build 10883` matching the tag.
  `--backend cpu --build b10871` → installs beside it, reports `build 10871`.
  `--backend vulkan` → `Vulkan0: Intel(R) Iris(R) Xe Graphics (16157 MiB,
  15389 MiB free)`, i.e. the device probe reads a real adapter. `--force`
  reinstalls and leaves no staging behind; a second `setup` reports
  "already installed" and re-probes.
  **Smoke — GO**: the downloaded binary launched through the app's own
  supervisor (`managed_cpu_line_launches_with_the_measured_batch_live`) — line
  `-ngl 0 -c 2048 -b 256 -ub 256 --jinja`, four unified slots over 2048 — and
  then served the whole `openai::client::ignored_smoke` set. Against the
  **hand-built** local `llama-server` (build 10807, MSVC) on the same model the
  results are identical arm for arm: 5 passed, the same 4 failed
  (`tool_call_is_emitted_and_parsed`, `control_tools_are_callable`,
  `emits_thoughts_for_reasoning_model`, `tool_result_image_is_seen_live` — a
  4B instruct model that does not tool-call, has no reasoning channel and no
  vision, not the binary). The downloaded Clang build ran the set in 31.8 s
  against the local MSVC build's 45.4 s.
- **Tests**: 3015 green, 155 ignored (2987 / 153 before the track — 21 unit
  tests over the derivation and the unpacking, 3 over the CLI surface, 5 over
  the settings write). Two new `#[ignore]` smokes:
  `live_the_newest_build_still_names_a_cpu_backend` — one API request, the test
  that fails instead of a user when upstream renames something — and
  `live_install_cpu_into_a_tempdir`, the whole path end to end on the cheapest
  asset.

### Post-M9: the engine binary is found, not just typed — an empty field resolves (done)

- **Why**: spec §3.4 had said the `llama-server` path "is looked up in `PATH`
  and next to the application binary" since M1, and the launcher was
  `Command::new(&cfg.binary)` verbatim (`shared/api/managed.rs:303`) — only the
  `PATH` half, and that half only because the OS does it. The half that was
  missing had nothing to point at until the downloader landed; with
  `data/llama/<backend>-<tag>/` on disk it does, and the roadmap item the
  downloader track opened asked to implement the promise or strike it.
- **The rule** (`features::llama_setup::resolve_binary`, called through
  `app::supervisor::BinaryLookup`), by what the field contains:
  - **a path with a directory part** — used exactly as written, never
    second-guessed. A typo must surface as the preflight's "model/binary not
    found", not as a silent launch of something else;
  - **a bare name** (`llama-server`) — beside the application first, otherwise
    handed to the OS, i.e. `PATH`. That order costs one `stat` instead of a
    `PATH` walk of our own, prefers what ships beside the app to whatever the
    machine happens to have, and makes the two platforms agree: Windows'
    `CreateProcess` already searches the calling image's directory, Unix's
    `execvp` does not — so the promise was half-true on one OS and false on the
    other;
  - **empty** — the build installed **last** under `data/llama/`, else a
    `llama-server` next to the application, else nothing, and only then
    `NotConfigured`. So `llama setup` without `--set-binary` already gives a
    working server, and unpacking a llama.cpp archive beside `mindfork` is
    enough with nothing typed.
- **"Installed last", not "newest tag"** (the user's decision, 2026-09-10).
  Ordering by build number would move a user from `vulkan-b10871` to
  `cpu-b10883` — off the GPU — because the CPU build happened to be published
  later. What they last ran `llama setup` for is what they meant; anything else
  is what `--set-binary` is for. Read from the directory's creation time,
  falling back to its modification time. An interrupted install (a directory
  with no binary in it) and a `.tmp-…` staging directory are not candidates.
- **Where the directories come from.** `LlamaSupervisor` was a unit struct; it
  now carries a `BinaryLookup` built from `Paths` (`llama_dir` + the new
  `Paths::exe_dir()` accessor), and `Default` — what every existing test uses —
  leaves both `None`, which reduces the resolution to "an explicit path or
  nothing", i.e. the exact behaviour those tests were written against. The
  embedder and the impersonation server resolve through the same lookup: one
  install runs all three.
- **The field's hint says so.** The *llama-server binary* row had no
  description; a field whose *empty* value now means something needs one, so
  `ui.settings.desc.binary` spells out the three cases and repeats that the
  folder must stay whole.
- **Live run (2026-09-10)** — the new smoke
  `empty_binary_launches_the_downloaded_build_live`: a managed section with
  `binary: None`, a real `data/llama/` holding `cpu-b10871`, `cpu-b10883` and
  `vulkan-b10883`, and `gemma-3-4b-it-q8_0`. Resolved to `vulkan-b10883` — the
  one installed last, and the case that separates the two rules, since by tag it
  would have tied with `cpu-b10883` — launched it through `apply_chat` and the
  probe reported `Ready`. **Smoke — GO.**
- **Caught by the lesson written the day before**: the fixture for "an entry
  that is not a directory" was `notes.txt`, which the i18n gate reads as a key
  under the `notes.` prefix. Renamed to a dotless `stray-file` (lessons.md §7).
- **Tests**: 3024 green, 156 ignored (3015 / 155 before) — 8 over the resolution
  rule, 2 over the supervisor wiring, 1 new `#[ignore]` smoke.

### Post-M9: `llama remove` — a build comes off disk, and says what that changed (done)

- **Why**: the last open item of the downloader track. `llama installed` showed
  what was on disk and its size, and nothing took one back off; a CUDA install
  is 1.1 GB and several accumulate as soon as anyone compares backends.
- **Identifying the build.** The directory name `llama installed` prints
  (`cuda-12.4-b10883`), or a bare backend (`vulkan`) **while it is
  unambiguous** — with two builds of one backend it names neither and the
  refusal says so in its own sentence, separate from "no such build". Guessing
  between two builds of the same backend is how the wrong gigabyte gets deleted.
- **The settings are read, never written.** A build any of the three managed
  fields points at is **refused** unless `--force`, naming which fields and the
  three ways forward (set another build with `--set-binary`, clear the field, or
  force). Removing it silently would leave them aiming at nothing and the
  failure would surface at some later launch instead of here. The comparison is
  `starts_with` on the path as stored — the shape `--set-binary` writes — and
  then through `canonicalize`, which settles a hand-typed path or a symlinked
  data root; a sibling whose name merely shares a prefix is not inside it.
- **What the closing line says.** The removal reports the megabytes freed and
  then **what an empty binary field resolves to now** — that answer changes when
  the build it was resolving to is the one that just went away, and "nothing
  left" is exactly the state worth naming out loud.
- **No `--all`, no prune.** The build worth deleting and the build the user was
  about to fall back to are indistinguishable from here; that was the whole
  reason the roadmap item asked for an explicit id rather than a policy. The
  single-instance guard is taken as `setup` takes it — on Windows a running
  server holds the very files this deletes.
- **Live run (2026-09-10)**, on the dev root holding `cpu-b10871`, `cpu-b10883`
  and `vulkan-b10883`: an unknown id listed what exists; `cpu` alone was refused
  as ambiguous and named both; with the settings pointed at `cpu-b10871` by
  `--set-binary`, the removal refused and named *the assistant* and
  *impersonation*, exiting `1`; `--force` deleted it, reported 44 MB freed and
  that an empty field now resolves to `vulkan-b10883`. **Smoke — GO.**
- **Tests**: 3032 green, 156 ignored (3024 / 156 before) — 6 over finding,
  the use check and the removal, 1 over the CLI surface.

### Post-M9: the turn asks about images once, and stops waiting for the answer (done)

The last of tier 2. Whether the engine takes images belongs to the server and the model
behind it, so it cannot change inside a turn — but `land_call` asked on **every** result
that carried one, and asking is a real HTTP round trip: `OpenAiClient::vision` fetches
`/props` and nothing memoizes it. A turn whose rounds each returned a chart paid the trip
each time, up to `max_tool_rounds` of them, which in this repository's own working config is
32.

Two ways it could stop the turn outright, both closed. The engine client sets a **connect**
timeout and no request timeout — correct for a stream that may take minutes, wrong for a
probe — so a server that accepted the connection and then stalled the response waited for
ever. And it was a bare `await`, outside the loop's cancellation: `Esc` could not end it and
the turn sat in `Cancelling` with nothing to show for it.

`TurnLoop::vision` now answers from a per-loop memo, and the one real ask is bounded by
`VISION_PROBE` (5 s) inside a `select!` on the loop's token. A probe that does not answer is
`VisionSupport::Unknown` — already the answer for everything that is not llama.cpp, so
nothing about what gets sent changes; the turn simply stops waiting to find out.

**The test had to be made honest first.** Counting the engine's `vision()` calls over a
three-round turn gave **1** with the memo *removed*, because the `Charting` fixture stored
identical bytes every round: round two got `Stored::Unchanged` and returned without an
image, so only one round ever asked. The fixture now draws a different chart per round when
a test asks it to — which is what a tool called three times normally does — while the dedupe
test keeps the identical bytes it is about. With that, the mutation gives 3 against the
memo's 1. A test that cannot tell the fix from its absence is worth less than no test, and
this one could not until the fixture matched the case.

**Live.** `sandbox_outputs_e2e_live` on Gemma 4 31B q4_0 (b10807): the probe runs against a
real `/props` there, which is the half a scripted engine cannot check.

### Post-M9: `external` against a gateway — a thought under a second name, and the rest of the review (done)

The question was "how compatible is `external` mode with OpenRouter", and the
honest answer needed reading rather than guessing, because `external` is the
mode we recommend for exactly that: install.md names OpenRouter, the settings
hint names it, and `ExternalSettings::model_name` exists because a gateway
routes on the request's `model`
([external-model-name.md](../research/external-model-name.md)). The review:
[openrouter-external.md](../research/openrouter-external.md).

**The core loop was fine, and one channel of three was not.** Streaming, tool
calls, images, the token counter, the Bearer key, the retry decorator and the
`{"error":…}` envelope inside an open `200` all already speak that wire — and
the keep-alive SSE **comment** a gateway parks in the stream
(`: OPENROUTER PROCESSING`), which is the classic way a hand-rolled reader
breaks, is inert here for a reason worth recording: `eventsource-stream` 0.2.3
discards a comment line (`RawEventLine::Comment(_) => {}`) and dispatches
nothing for an event whose data buffer is empty, so the app sees no chunk and
not even a parse warning. Read in the crate's own source, then pinned by a unit
test against a raw-body SSE stub, because "the parser looked right" is not a
test.

**What was broken: thoughts, in the quietest way available.** The client read
`delta.reasoning_content`; a gateway sends `delta.reasoning`. An unknown field
deserializes away in silence, so a reasoning model reached through OpenRouter
answered with its thinking dropped — and the `<think>` fallback cannot rescue
it, since the gateway has already lifted the reasoning out of `content`. A model
that thinks and a model that does not produced the same feed. `wire::Delta`
gained the second field and a `thoughts()` accessor that **takes both**,
`reasoning_content` first: a server that sends both names sends one trace twice,
and precedence keeps the local stack byte-identical to before. Rejected:
`#[serde(alias)]`, which cannot express precedence and leaves the outcome to the
server's key order.

**What the review decided *not* to change** matters as much. `reasoning_effort:
"none"` on the three silent turns (title, roll, impersonation) looked like the
next fix — until the protocol said the flat field is the supported legacy
spelling, that sending it *and* the nested `reasoning: {effort}` is rejected,
and that `external` is also every local `llama-server`, which reads `"none"` as
"do not think". So: no wire change, a cost risk recorded, and a measurement
(§8 M4) that would reopen it. The context window and the sampling set got
documentation rather than code for a related reason — the relief already exists
(`compaction.context_tokens`), and the code version of both wants the same
unmade measurement: a gateway's catalogue carries `context_length` and
`supported_parameters` per model, which is one request serving F3(b) and F4(c)
at once. Left as roadmap proposals with `reasoning_details` (F5), not built
blind.

**Tests**: 3221 green, 177 ignored (3217 / 176 before) on the tracked count —
this change was measured on Linux, where the same suite is 3214 / 172 (3210 /
171 before) because the Windows-only tests do not compile there; the delta is
+4 and +1 either way. Three over the new
parse (a gateway's field becomes thoughts; `reasoning_content` wins when both
arrive; a keep-alive comment adds nothing), plus one `#[ignore]` smoke,
`a_gateway_streams_thoughts_under_its_own_field_name`, declared by
`MINDFORK_LIVE_GATEWAY_MODEL` on the `MINDFORK_LIVE_TEXT_ONLY` pattern: the run
states that the named model reasons, and the smoke **fails** rather than skips
if no thoughts arrive (lessons §9). Both new parse tests were mutation-checked,
and the precedence fixture had to be sharpened to earn it — with one string
under both keys, swapping the precedence passed unnoticed. The fourth is the
model-variable derivation below.

**Live — GO, on the author's machine.** The session itself had no route to
`openrouter.ai` (the environment's egress proxy answers `403` to the `CONNECT`,
and its own README forbids routing around an organization policy denial) and the
account key added afterwards does not reach a container that started before it —
so the smoke was written, declared and handed over. Run on
**`deepseek/deepseek-r1` through OpenRouter, 2026-09-14**:
`a_gateway_streams_thoughts_under_its_own_field_name` **green** in 49.5 s, with a
full reasoning trace in `thoughts` and a non-empty answer beside it —
`delta.reasoning` reaches the feed, which is the whole of F1 and exactly what the
old parse dropped. `tool_call_is_emitted_and_parsed` green in 4.3 s (native
`tool_calls` do come back through a gateway), `simple_generation` green.
**Smoke — GO.**

**Two more things the same run measured, neither of them asked for.** F3 stopped
being a `[docs]` prediction: `auto_compaction_fires_without_the_command_live`
failed with "nothing folded. Last exact prompt: Some(22567) tokens" — that smoke
sets `context_tokens: None` on purpose so the window can only come from the
engine, and a gateway has no `/props` to give it. Not a defect; the documentation
half of F3 is what it needed. And the same output proves the counter it measured
against: `exact prompt: Some(4875) … Some(22567)`, each flagged exact, so
OpenRouter's `usage` parses and a 22.5k-token prompt streams through without
trouble.

**The failure mode to recognise next time.** In the first run,
`attachment_read_e2e_live` ended with DeepSeek's own chat template sitting in the
reply *text* — `function<|tool_sep|>attachment_read … <|tool_call_end|>` — after
that very turn had issued six correct native tool calls. Neither "R1 cannot call
tools" nor our wire: on a gateway the template → `tool_calls` parse belongs to the
**routed provider**, and when it misses, the model's raw special tokens arrive as
ordinary content, the loop sees no call and the turn ends on junk. Parsing every
vendor's template is the rabbit hole ADR 0004 exists to avoid, so nothing was
changed — but that is what a "the model went mad" report from a gateway user will
turn out to be. M3–M5 are still owed
([openrouter-external.md](../research/openrouter-external.md) §8.2).

**A correction the run also earned:** the first instructions said to run the
whole `--ignored` set, which is right for a local `llama-server` and wrong for a
metered endpoint — ~235 smokes, 121 of them multi-round e2e conversations built
where a token is free and a 20k-token ballast costs nothing. install.md §7.1 and
§8 now name the three smokes that answer M1 and M2 in under a minute, and say
which local-stack smokes are expected to be red on a gateway for reasons that are
statements about the stack.

**The live set could not have been pointed at a gateway at all** — found while
writing those commands, which is the value of writing them out. `live_client`
names a stack by a pair of variables (URL + key) and sent **no** `model`, so
every `#[ignore]` smoke in the repository — this client's, the orchestrator's
e2e set, the embedder's — would have met `400 "model name is missing"` on
OpenRouter, and only the new gateway smoke would have passed, because it sets
its own. The helper now derives a third variable from the same convention the
seventeen call sites already spell (`MINDFORK_ENGINE_URL` →
`MINDFORK_ENGINE_MODEL`; a suffixed stack keeps its suffix, as its key variable
does) and sends it when set — unset, the request is byte-identical to before.
Derived rather than passed because a third argument at seventeen call sites buys
nothing the convention does not already guarantee; the derivation itself is
pinned by a test, since a wrong name there would not fail loudly — it would read
an unset variable and leave a correctly-configured-looking run answering `400`.
Test-only code (`#[cfg(test)]`), and install.md §7.1 now carries the OpenRouter
invocation.

### Post-M9: the gateway's remaining measurements — the silent turns are a `400`, not a bill (done)

The three measurements [openrouter-external.md](../research/openrouter-external.md)
§8.2 still owed after the first PR, run by the author on OpenRouter (the session
has no route to the service). Two came back, and one of them **refutes a
conclusion this track shipped**.

**M4: `reasoning_effort: "none"` is not ignored — it is refused.** The exact body
`title.rs` builds, sent to `deepseek/deepseek-r1`, is answered `HTTP 400`,
"Reasoning is mandatory for this endpoint and cannot be disabled". The review had
concluded "change nothing" here, reasoning from OpenRouter's documented "an
unsupported parameter is dropped and the rest forwarded" that the worst case was
a **cost** — the silent turns paying for reasoning nobody wanted. It is not a
cost. On a model that always reasons, the **title, the compaction roll and
impersonation all fail** while ordinary chat keeps working — which is, word for
word, the failure `with_effort_none_omitted` was written for on xAI
([grok-xai-provider.md](../research/grok-xai-provider.md) §2.3), arriving by a
different road. And the nested `reasoning: {enabled: false}` is no escape: the
endpoint is not refusing a *spelling*, it is refusing the *request to disable
reasoning*.

So the fix is not "which spelling" but "when to ask at all", and the fork is open
(research §5, F2): recover from the refusal in the engine layer and memoise it
per backend; stop sending the field on `external` and lean on `reasoning_budget:
0`, which `title.rs`'s own comment already calls the one that actually works on
llama.cpp; or ask the catalogue, which lists `reasoning`/`include_reasoning` and
**not** `reasoning_effort` for this model. Recommended: the first, with the third
when the catalogue fetch lands — it is the only one that cannot be wrong, and it
covers LiteLLM and every future wording drift too. The control (M4a) then isolated the
field: a minimal request carrying `reasoning_effort: "none"` and nothing else of
ours is refused identically, so the llama.cpp-only fields the silent turns also
send are not involved and the fix has one target. Not implemented here — which
of the three shapes to build is the user's decision, and two of them need a
measurement this session cannot make (a local `llama-server` for (b), a gateway
for (a)).

**M5: both stage-2 proposals are buildable, and the sampling prediction was
exact.** `deepseek/deepseek-r1` carries `context_length: 64000` — the number
F3(b) wants, so the compaction window can be filled from the catalogue instead of
by hand — and a `supported_parameters` list that confirms §4.2 field by field:
`temperature`, `top_p`, `top_k`, `max_tokens`, `seed`, `frequency_penalty`,
`presence_penalty`, `stop`, `tools`, `tool_choice`, `response_format` get
through; **none** of `min_p`, `typical_p`, `top_n_sigma`, dynatemp, adaptive,
mirostat, DRY, XTC or `samplers` is there; and the penalty it takes is spelled
`repetition_penalty` where we send `repeat_penalty`. One fetch serves F3(b),
F4(c) and F2(c), which is what makes stage 2 one piece of work rather than three.

**Documentation only; no code changed.** Tests untouched (3221 / 177 on the
tracked count), so no live run of our own was needed beyond the two measurements
recorded here. M3 is still owed.

### Post-M9: a refusal to stop reasoning is answered, not reported (done)

The fix for the defect the previous entry measured, on the shape the user picked
(research §5 F2, **user's decision 2026-09-14: (a), F2 alone**).

**What was broken.** Three turns ask for reasoning to be off — the auto-title,
the compaction roll, impersonation — and an endpoint may be unable to honour it:
`reasoning_effort: "none"` against a model that always reasons is answered `400
"Reasoning is mandatory for this endpoint and cannot be disabled"`. So on such a
model those three failed while ordinary chat worked, which is the worst shape a
failure can take — the app looked fine and quietly stopped titling, compacting
and impersonating.

**What it does now.** `chat_stream` splits its one attempt into `send_chat` and
calls it twice when that refusal arrives: once as before, then again without the
field, remembering the answer in an `AtomicBool` on the client so a session pays
one refusal rather than one per silent turn. The memo feeds the **existing**
`omit_effort_none` switch that xAI is configured with, so the fix reuses that
mechanism instead of adding a second one, and a local `llama-server` — which
accepts the request — never takes the path at all.

**Narrow on purpose**, three conditions each ruling out a way of being wrong: the
turn must have asked and the field must have gone out; the status must be `400`,
because a `503` carrying the same words is the retry decorator's (spec §6.8) and
dropping a sampling field to answer an outage would file it as a capability; and
the message must name reasoning **and** its disabling — a pair of substrings
rather than the sentence, since providers reword, and a false positive costs one
round trip because the second attempt meets the same error and it surfaces
unchanged.

**The tests had to be rebuilt before they were worth anything.** The first
version of the three negative arms counted requests at the stub — and every one
of them survived removing the status check, because a second request against a
spent script is refused by the OS and still arrives as "an error". They now each
end with a reply that *would succeed*, so a wrong retry turns the turn green and
`expect_err` catches it. With that, all five mutations fall: never recover; do
not remember; drop the status check; drop the "did it ask" check; loosen the
message match to "reasoning" alone. The stub also grew a body reader that honours
`Content-Length` instead of taking one `read`, so a split request makes these
tests fail rather than flake.

**Tests**: 3224 green, 178 ignored (3221 / 177 before) on the tracked count; on
Linux 3217 / 173 (3214 / 172 before). **Live — GO**, run by the author (this session
has no route to the service): `a_muted_turn_survives_an_endpoint_that_must_reason`
on `deepseek/deepseek-r1` through OpenRouter — `finish=Some(Stop)`, the title back
as *Database Indexing Explained*, 8.2 s, where before this change the same request
**was** the `400`. The smoke sends `title.rs`'s own shape and is declared by
`MINDFORK_LIVE_MANDATORY_REASONING_MODEL`, so it fails rather than skips.

And it prints what the fix does not do: `thoughts=772 chars`. The model reasoned
anyway — on that endpoint it cannot be asked not to — so the turn's tokens are
still spent on thinking nobody wanted. That residual is what the original "cost
risk" reading of F2 was about, and it is genuinely unavoidable here; what the fix
buys is the turn completing at all. Worth keeping straight, because a later reader
looking at a title that cost 772 characters of reasoning might otherwise think the
recovery failed.

### Post-M9: the endpoint is asked what the model can do (done)

F3(b) and F4(c) of the OpenRouter review, built as one track because they are one
HTTP request. Plan, forks and the decisions:
[gateway-capabilities.md](../gateway-capabilities.md).

**Both defects were silent, and both are measured.** A gateway serves no
`/props`, so `context_budget` had no source and automatic compaction simply never
fired — a conversation reached 22 567 tokens with nothing folded. And
`supported_sampling_fields(None)` returns *every* field for `external`, because
`external` used to mean llama.cpp: through a gateway the extensions are dropped
on the way, `repeat_penalty` worst of all, since that field is spelled
`repetition_penalty` there and so looked set while doing nothing. The catalogue
the app **already fetches** for the model's name carries both answers —
`context_length` and `supported_parameters`, per model — so the marginal cost was
parsing two more keys plus the plumbing.

**What the shape had to get right.** `EngineBackend::model_capabilities` defaults
to `None` and only `OpenAiClient` overrides it, so every cloud keeps its
compile-time table — a gateway's catalogue has no standing to trim a protocol's
own limits. `RetryBackend` delegates it, with the test lessons §9 demands: three
times before, a defaulted question-method was left un-delegated and answered
"cannot say" invisibly. One background task asks both questions and lands one
answer (`EngineFacts`), because two tasks racing to fill two memos against one
endpoint is a heisenbug waiting to be written. The landing rebuilds the tool
registry — but only when the published set actually changed, since `set_sampling`'s
schema is baked into it and a re-ask after a readiness flip should cost nothing.

**Silence is never a claim**, and that is the invariant the whole track rests on:
no catalogue, an empty list, a blank model field, a llama.cpp `/v1/models` that
carries ids and nothing else — each leaves behaviour exactly as it shipped. The
feature can only narrow from a positive answer. The window is used as reported
and sits **after** `/props`: a running server describes the process serving this
turn, a catalogue describes the model in the abstract. An explicit setting still
beats both.

**The narrowing travels as far as the user's choice took it** (fork G3(ii)): the
settings screen, the `set_sampling` schema and `Message.metadata`'s record of
"what was applied" — but **not** the wire. The list is per model while the request
is served by a per-provider route, so dropping a field ourselves on that evidence
would trade their silent drop for ours, and ours would be unrecoverable. The two
vocabularies are translated explicitly rather than assumed equal
(`repetition_penalty` ↔ `repeat_penalty`, one `reasoning` ↔ `thinking` +
`reasoning_effort`), and the settings screen's own per-provider predicate was
deleted: it was the first half of `available_sampling_fields`, and a second
spelling is how the two drift apart.

**Tests**: 3232 green, 179 ignored (3224 / 178 before) on the tracked count; on
Linux 3225 / 174. Eight new: the catalogue answers for the configured model and
for no neighbour; silence in each of its three shapes; the decorator delegation;
the gateway window and `/props` winning over it; the published fields reaching
the gates; and the narrowing itself, whose fixture is the list OpenRouter
actually returned for `deepseek/deepseek-r1` rather than an invented one.

**Live — GO on N1 and N2**, run by the author on `deepseek/deepseek-r1` through
OpenRouter (this session has no route to the service).
`a_gateways_catalogue_answers_for_the_configured_model` came back with
`context_length: 64000` — the window a gateway previously never had — and a
fourteen-name list that the narrowing turned into **ten** offered fields. The
output is also where the alias table proves itself: every llama.cpp extension is
gone (`min_p` included — that endpoint does not list it), while `repeat_penalty`
survives **under the catalogue's own `repetition_penalty`** and the two reasoning
switches survive on the single `reasoning` entry. Without the table the first of
those would have been dropped from the offer while remaining exactly the field the
gateway ignores.

**N3 — GO, and it is the regression half**: a local `llama-server` seen
unchanged, 2026-09-15, `external` against gemma-4-31B on Windows. The app's own
log carries the whole answer, in what it says and in what it does not.
`engine reported its context window context_budget=16384` is `/props` answering
first, as it always did; "the endpoint's catalogue answered for the configured
model" — the line the landing logs whenever `caps` is `Some` — appears nowhere,
so nothing narrowed, the registry was not rebuilt and the settings screen was
handed `None`, which is every field. And the catalogue was not merely silent, it
was **never asked**: the same log's `engine reported the model it is running` is
emitted only when the configuration names no model, and a blank model field is
exactly where `catalogue_entry` returns before the request. The residual worth
naming is the other half of that condition — with the model field *filled in*
against a local server, `model_capabilities` does make one `GET /v1/models` per
applied engine that did not happen before, and still answers `None`, since
llama.cpp's catalogue carries neither key ([gateway-capabilities.md](../gateway-capabilities.md) §5).

### Post-M9: through a gateway, a tool's image and `/continue` belong to the route — M3 measured (done)

The last measurement the OpenRouter review owed, and the one that decided what
F6 is. Plan, tables and forks:
[gateway-images-and-continue.md](../gateway-images-and-continue.md).

**M3 — GO for the app.** Run by the author in a real terminal, 2026-09-15:
`anthropic/claude-haiku-4.5` through OpenRouter (served by Amazon Bedrock — the
call id `toolu_bdrk_…` says so), the Wasmer sandbox, `tools.python_images` on.
The request closed the text channel — numpy seed 7, bar colours drawn from the
same generator, print nothing — and the model named the tallest bar's colour and
height from the chart alone, matching the PNG. `/file list`, `open`, `folder` and
`remove` each did what spec §9.7 says, and the catalogue answered for a second
model (200 000, twelve fields). One defect surfaced beside the track: a bare `1`
is not a handle and the refusal does not say `#1` is — filed separately.

**F6 hardened, both halves.** The review had called them provider-dependent and
not worth a blind change; measured per pinned route with blind controls, each is
a real limit:

- **a tool's images** — of 29 route-and-model pairs, 20 see the image, 6 refuse
  the request (DeepInfra's `422` names the tool message's content "should be a
  valid string"; ModelRun's `400` counts zero media markers in its template) and
  **3 answer confidently about a picture they never received** (Chutes on Gemma,
  Venice on Qwen and Mistral). Moved into a `user` message right after the tool
  result — the fallback Gemini already takes — every pair that answered, 28 of
  28, saw it;
- **`/continue`** — continues on Anthropic ≤ 4.5 (four routes) and Gemini;
  restarts on OpenAI and on every open-weight route (Gemma: all eleven that
  answered); `claude-sonnet-4.6` passes through its `400`. The three fields
  `/continue` adds changed nothing on any route, and `enable_thinking: false`
  does not arrive, so Qwen reasons for thousands of characters before it restarts.
  And the app **stores the restart glued onto the partial** — `EchoFilter` lets a
  stream that diverges at byte 0 flow into the same message — so the failure is a
  corrupted reply, not an error.

**The instrument needed three corrections before its tables meant anything.**
The client discards the routed provider, so a failing smoke (`ครั้ง` on Gemma)
could only be attributed by pinning `provider.only` per route in a raw replay of
the same body. The first replay used a 128 px fixture where the smoke uses
256 px, and read the OpenAI route as silently blind — at 256 px it sees 5/5 in
both shapes, so the size was a second variable of that route, not a finding
about the shape. And the Mistral routes rejected the fixture's `call-1` id before
reading the image at all. The blind arms earned their place once more: three Qwen
blind runs in sixteen, across two routes, passed the keyword criterion for a
picture they were never shown — one of them a *"green … black circle"* — so a
single seeing run on that family would have proved nothing, and the table reads
8/8 against 1/8 instead.

### Post-M9: `/continue` follows the route table through a gateway — and F5 closes on a measurement (done)

Stage H2 of [gateway-images-and-continue.md](../gateway-images-and-continue.md),
on the user's decisions of 2026-09-15 (H2 (b), H3 (ii): this first, the images
second), with the review's last item measured beside it.

**What was broken.** `ServerMode::supports_continuation` answered `true` for all
of `external`, because `external` meant llama.cpp when the table was written.
Through OpenRouter only Anthropic ≤ 4.5 and Gemini continue a trailing assistant
message; OpenAI and every open-weight route restart — and the echo filter, which
withholds bytes only while they match the seed, let a restart flow into the same
stored message: `…France isThe capital of France is Paris.`, no note, no error.

**What it does now.** On `external` with a catalogue — the positive sign of a
gateway the previous track already lands — the gate reads the slug's vendor
against the spec §6.4 table: `anthropic/…` through the existing version
allowlist, `google/gemini-…`, everything else refused with a note of its own
(the generic one says external engines continue, which is the lie). The one
answer is now computed once and snapshotted into the turn, so `Finished.continuable`
and the mid-stream interruption note can no longer offer `/continue` where the
command refuses — they had each asked the mode separately.

**The sub-decision the gate forced (H2.1).** The engine's facts were asked lazily,
at the first turn. `/continue` as the first command after a restart is that
command's main case, and it would have met an unanswered question and fallen back
to the behaviour a gateway does not have. The facts are now asked when the engine
is applied and on a readiness flip — the rule the model's name already followed.
A `:variant` suffix is stripped before the version is read, or
`claude-sonnet-4.6:batch` would parse as 4.0 and be allowed; the unit test that
pins it was written for that line.

**F5 — closed as measured, nothing built.** A tool round with reasoning forced,
the second request three ways, on `claude-haiku-4.5` (four routes) and
`claude-sonnet-4.6`: no blocks (what this client sends), the exact echo and a
text-tampered echo all answered `200`, indistinguishable. A **garbage signature**
was answered `400 "Invalid signature in thinking block"` — through the gateway and
on Anthropic's own API alike — which proves the blocks are forwarded and checked,
and that the rejection the reports describe is reachable only by sending blocks.
The echo bought no observable continuity (zero reasoning tokens in the second round
in every arm), so building it would add the one failure the current wire cannot
hit. Found beside it and filed separately: `thinking: true` alone enables
reasoning at no route; only `reasoning_effort` does.

**Tests**: 3236 green, 180 ignored (3232 / 179 before) on the tracked count. Four
new unit tests (the route table, the gate on a bare orchestrator, the whole route
on a running one, where the catalogue must land before any turn, and the readiness
flip's re-ask) and one smoke. **Mutation-tested**: eight mutations — the gateway
arm, the `:variant` strip, Gemini's row, the lazy re-ask at each of its two sites,
the gate ignoring the catalogue, the note never chosen, `Finished.continuable`
ignoring the turn's answer. The first run left the readiness-flip site surviving:
the fixtures learn the catalogue at settings apply and never flip, so the loop arm
became `handle_chat_status` and got a test of its own — which then "survived" once
more, because the mutation script's test filter did not name it. Read twice, that
survivor indicted the instrument; with the filter fixed every mutation is caught.

**Live — GO on all three declared stacks**, 2026-09-15, one smoke
(`continue_through_a_gateway_live`) through the app's own orchestrator:
`gateway-refuses` on `google/gemma-4-31b-it` via OpenRouter — the cut announced
`continuable=false` and `/continue` answered with the gateway note;
`gateway-continues` on `anthropic/claude-haiku-4.5` — `"The capital of France"` +
`" is Paris."`, no restart; and `local`, the regression half, on the LAN
`llama-server` with Gemma 4 31B and no catalogue — `continuable=true` and the same
clean `" is Paris."`, exactly as before. **The first run's fixture was wrong twice,
and the run said so**: at a 6-token cap the cut already held "Paris" and Haiku's
continuation was a lone `"."`, which proves little; and the local Gemma spent the
whole cap reasoning, left no visible text, and was — correctly — not continuable.
A 4-token cap with `reasoning_budget: 0` fixed both, and the catalogue landed
before the first turn on every stack, which is H2.1 measured rather than argued.

### Post-M9: a tool's images reach the model through a gateway — re-homed into a user message (done)

Stage H1 of [gateway-images-and-continue.md](../gateway-images-and-continue.md), on
the user's decision of 2026-09-15 (H1 (b), with the switch inside the client,
H1.1 (i)) — the second half of F6.

**What was broken.** The OpenAI-compatible client puts a tool's images on the
`role:"tool"` message as content parts, a shape measured good on llama.cpp and on
xAI and outside the OpenAI spec's letter. Through OpenRouter the routed provider
decides what it means: of 29 route-and-model pairs 20 saw the image, 6 refused the
request (DeepInfra's `422` names the tool message's content "should be a valid
string"; ModelRun's `400` counts no media marker in its template; CoreWeave a
`502`) and **3 answered confidently about a picture they never received** —
while the app, having sent it, had no way to say otherwise.

**What it does now.** `wire::rehome_tool_images` takes the conversation and moves
the images of every run of tool results into one user message right after the run
— one per run, because a round's tool messages must stay contiguous after its
`tool_calls` — in call order, behind the labels they already carry. The client
applies it first thing in `chat_stream`, and only when the request carries a tool
image **and** the endpoint's catalogue answered for the model; a request without
one does not even look the catalogue up, and a llama.cpp, which publishes nothing,
is sent the body `build_chat_request` always produced. The builder itself is
unchanged, which is why the seventeen tests calling it are too.

**Two sub-decisions taken at implementation, recorded in the plan.** The label: the
plan had said "a label naming the call", and the labels the images already carry
name the file the tool's own result names — adding a call name would need the
profile's language, which the wire layer does not have, for nothing the file name
does not already tie together (Gemini's F1-A fallback made the same call). And the
memo: reading the plan's H1.1 against the code found that `catalogue_entry` issued
a fresh `GET /v1/models` on every call — harmless for a once-per-engine question, a
request per turn on the chat path. It now sits behind a `OnceCell` filled by a
fetch that *answered*; a transport failure, `5xx` or `429` stays unremembered, so a
gateway briefly unavailable is asked again rather than filed as having no
catalogue.

**Tests**: 3240 green, 181 ignored (3236 / 180 before) on the tracked count. Four
new unit tests — two on the wire (runs and order, labels kept, the
tool results bare strings; a user's own image left alone) and two on the client (the
memo, including "not now"; the re-homed body for a gateway, the unchanged body for a
llama.cpp-shaped catalogue, no catalogue request for a turn without tool images) —
and one smoke that replays the **client builder's own** body pinned to a route.
The first mutation run **hung instead of failing**: the mutation that skipped the
catalogue changed the order of requests, the scripted stub's thread waited for a
connection that never came, and both client tests joined it with no limit —
sixteen minutes at zero CPU before it was noticed. The first repair — a timeout
around a `spawn_blocking` join — hung the rerun past the harness's new ten-minute
cap all the same, because the test's runtime waits at shutdown for a blocking task
still running. The bound now lives in the stub: `scripted_server` accepts under a
ten-second deadline and hands back what it saw, so a missing request fails the
test (lessons §2, recorded a second time). And the first CI run of the pushed branch
failed on both runners with `connection reset` — in the new tests and in two
unchanged refusal tests: the same stub never said `Connection: close`, so the client
pooled the socket after the catalogue's answer and sent the turn down a connection
the stub had dropped. It says so now (lessons §2, the stub entry, recorded a second
time and no longer Windows-only).

**Live — GO**, 2026-09-15. The builder's re-homed body replayed pinned to the routes
that failed today's shape, blind arm beside each: on `google/gemma-4-31b-it`
Chutes (silently blind before), DeepInfra (`422`) and ModelRun (`400`) all see the
image; on `qwen/qwen3.6-27b` Venice (silently blind) and DeepInfra (`422`) too.
Through the client on default routing `tool_result_image_is_seen_live` is green on
Gemma 4, Qwen 3.6, gpt-4.1-mini and Haiku 4.5; and on the LAN `llama-server`
(Gemma 4 31B with its projector, no catalogue) green on the unchanged shape.

**Forks open, nothing built** — H1 (re-home a tool's images when the catalogue
answered), H2 (refuse `/continue` on a gateway except the slugs whose direct mode
continues) and H3 (scope) wait on the user. **Documentation only; no code
changed** — tests untouched (3232 / 179 on the tracked count).

### Post-M9: the thinking switch reaches a gateway in its own field (done)

Found beside F5 ([gateway-images-and-continue.md](../gateway-images-and-continue.md)
§9): through OpenRouter the settings' thinking switch did nothing. The
OpenAI-compatible wire sends it as a top-level `thinking` — a llama.cpp field — and
`reasoning_effort` only when an effort is chosen. Plan, table and forks:
[gateway-thinking-switch.md](../gateway-thinking-switch.md).

**Measured before building**, one raw streamed request per shape, the body the
client builds for a tool turn with only the reasoning fields varied, reading
`usage.completion_tokens_details.reasoning_tokens`. It was wider than the F5 note:
"on" alone gave Claude Haiku 4.5 **0** reasoning tokens (on the default route and
pinned to Bedrock, Anthropic and Vertex) against 66–81 with `reasoning: {enabled:
true}`; "off" alone left Qwen 3.6 reasoning **100** tokens against 0 with `enabled:
false`. The lessons §3 wrong-type probe settled why: `thinking: "banana"` is a `200`
on both, `reasoning.enabled: "banana"` a `400`. The trap sat on the third model:
DeepSeek R1 answers `enabled: false` with the very `400 "Reasoning is mandatory…"`
the F2 memo recognises — but the memo fired only for `reasoning_effort: "none"`, so
mapping "off" naively would have failed ordinary chat turns there. The catalogue
turned out to carry a per-model `reasoning` object — 314 of 446 models, `mandatory`
always, 103 of them `true` — which is the positive signal that guard needed.

**Decided with the user, 2026-09-15**: T1(a) — "on" with no effort sends
`reasoning: {enabled: true}`; T2(ii) — "off" too, only where the entry states
`mandatory: false`, with the F2 recovery widened to a refused `enabled: false`;
T3(a) — a request with an effort goes out as before.

**What reading the call sites added**, after the forks: the orchestrator mutes some
turns with `reasoning_budget: 0` alone while the user's `thinking: true` stays — the
empty-reply re-ask, the director's checkpoints, `fetch_url`'s page summary — so a
rule reading `thinking` would have sent `enabled: true` on exactly those. A zero
budget is "off" now, as the Responses and Anthropic wires already read it. And
`AppConfig`'s default is `thinking: Some(true)`: gateway models that reason only when
asked now reason on an untouched configuration, which the CHANGELOG says in so many
words.

**Code**: `wire::gateway_reasoning` holds the whole rule as a pure function;
`ChatCompletionRequest.reasoning` is never set by `build_chat_request`, so every body
that does not pass the client's catalogue step is unchanged by construction.
`ModelEntry` keeps the `reasoning` key as raw JSON behind `lists_parameter` and
`reasoning_mandatory`, so an odd spelling cannot fail the list that carries the
window and the parameters. `OpenAiClient::reasoning_switch` consults the memoised
catalogue only for a turn that could gain the field; `should_stop_asking` counts a
sent `enabled: false`; the retry recomputes the switch under the memo, which drops
"off" and keeps "on".

**Tests**: 3251 green, 184 ignored (3246 / 181 before). Five unit tests — a 13-row
table over `gateway_reasoning`, held in one literal against the duplication gate
(lessons §2); the lenient `reasoning` key; the body per model kind through the
client; a llama.cpp-shaped catalogue giving the byte-identical body, and a switchless
turn not asking the catalogue at all; a refused "off" answered, remembered, and "on"
untouched by it. **Ten mutations, all caught**, each by a named failing test: the
effort guard, the `reasoning` listing, the zero budget, an unstated `mandatory` read
as optional, the memo ignored for "off", the flag never read, the recovery ignoring a
refused `enabled: false`, the retry keeping the refused switch, a switchless turn
asking the catalogue, the field never put on the body. The harness validated every
replacement before writing and restored the files byte-identical — and counts a
mutation caught only when a test *fails*, not when `cargo` exits non-zero: written
the first way, a run overlapping the live smoke would have failed to relink the
locked test binary on Windows and reported ten catches for nothing (lessons §2, a
gate that passes for the wrong reason).

**Live — GO**, 2026-09-15, through the client. OpenRouter:
`a_gateway_reasons_when_the_switch_is_on` on Haiku 4.5 — 53 reasoning tokens (0
before), and **red** at 0 with the "on" branch mutated to withhold the field, so the
smoke can fail; `a_gateway_stops_reasoning_when_the_switch_is_off` on Qwen 3.6 — 0
(100 before; with its reasoning really off it answered the "32 years before 2024"
question with *Atlanta, 1996*); `the_switch_off_completes_on_an_endpoint_that_must_reason`
on R1 — completes, no `400`, 239 reasoning tokens. The local half: the LAN stack was
unreachable, so the CPU `llama-server` build with `gemma-4-12b-it-qat-q4_0` ran
`a_gateway_streams_thoughts_under_its_own_field_name` with the model named — the turn
asked the catalogue, got ids alone, and streamed thoughts through
`reasoning_content` to `Stop` as **one** task in the server log (166 s). The probes
cost cents.
