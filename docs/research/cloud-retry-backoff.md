# Retry/backoff on cloud provider errors — research

> Status: **implemented — both stages, 2026-08-12**. Forks settled the same day
> (§6); what actually shipped, and the two places reality differed from this
> design, are in §10. Journal:
> [engine.md](../journal/engine.md); behaviour: spec §6.8.
>
> The item this closes: *"Retry/backoff on cloud provider network errors —
> clients currently surface the error body but don't retry (transient
> 429/5xx/timeouts)"* ([roadmap](../roadmap.md) §Engine and reliability).

## 1. Problem

Every cloud provider sheds load routinely: OpenAI answers `429`/`500`/`503`,
Anthropic has a dedicated `529 overloaded_error`, Gemini returns
`429 RESOURCE_EXHAUSTED`/`503 UNAVAILABLE`, xAI rate-limits per tier. These are
*transient by design* — the provider's own guidance for all four is "back off
and retry". Our clients never do: any of them ends the turn on the spot. On
top of the roadmap's stated pain, reading the code for this research found
that the failure surface is really **three defects of different severity**,
two of them invisible:

1. **Pre-stream HTTP errors end the turn, loudly** (the roadmap item). A
   `429` on the initial POST becomes a feed note with the provider's body
   (`ui.err.generation_failed`), and the user re-sends by hand. In a turn
   with tool calls there are up to `max_tool_rounds+1` = 9 requests
   (`generation.rs:840-897`), so one transient blip can kill a turn that had
   already spent eight rounds of work.
2. **Mid-stream failures are silent truncation.** Once the SSE stream is
   open, an error never reaches the user:
   - a transport drop yields `ChatChunk::Finished(FinishReason::Error)`
     (`openai/client.rs:173-177` and siblings), the partial reply is
     persisted (`generation.rs:1431-1451`), and **no note is shown** —
     `finish_generation` notes only `Cancelled` (`screens/chat/feed.rs:343-355`);
     `AppEvent::Error` is emitted only from the pre-stream `Err` arm
     (`generation.rs:1233-1250`). The only trace is a `tracing::warn!` in the
     file log.
   - **Anthropic's in-stream `error` events are ignored by design**: they
     fall into `AntStreamEvent::Other` (`anthropic/wire.rs:233-234, 254-255`),
     the connection then closes, and the `None` arm reports
     `Finished(Stop)` (`anthropic/client.rs:101-104`) — an overloaded-mid-answer
     truncation is **indistinguishable from a normal completion**. Anthropic
     documents that stream errors (e.g. `overloaded_error`, the 529
     equivalent) arrive exactly this way.
   - llama.cpp can also put an error object into an HTTP-200 stream
     (ggml-org/llama.cpp#14566); it fails chunk deserialization and is
     skipped with a log warning (`openai/client.rs:233-235`) — same silent
     shape. Gemini's SSE errors (same JSON shape as non-streaming errors)
     would be skipped identically (`gemini/client.rs:151-154`).
3. **A hung connection cannot be escaped.** No engine client sets any reqwest
   timeout (`openai/client.rs:48`, `responses/client.rs:40`,
   `anthropic/client.rs:41`, `gemini/client.rs:46` — plain
   `reqwest::Client::new()`, and reqwest defaults to *no* timeout), and the
   initial `send().await` is **outside** the `select!` on the cancel token
   (`openai/client.rs:128-133` and siblings) — cancellation is honoured only
   once SSE events are flowing. A blackholed endpoint (firewall drop, dead
   VPN) therefore leaves the turn in `Generating`, `Esc` moves it to
   `Cancelling`, and there it sits. The tool-layer clients already do both
   things right (`features/tools/web.rs:163-165` sets a timeout;
   `shared/tts/openai.rs:126-130` and `shared/video/gemini.rs:179-191` wrap
   `send()` in `select!` against cancel).

Defects 2 and 3 belong to the project's most-repeated defect class — "a
message must close the door" ([lessons](../lessons.md) §4): the situation is
not described, and the user cannot know a reply was cut short.

Auxiliary engine calls (auto-title `title.rs:138`, compaction roll
`compaction.rs:442`, impersonation `impersonation.rs:258`, reflection
`tool_loop.rs:151`) share the same backends and today spend a strike in the
background failure streak (`BACKGROUND_FAILURE_ALERT=3`,
`orchestrator/background.rs:19-66`) on any transient blip.

## 2. What the providers actually promise (verified 2026-08-12)

| Provider | Retryable | Never retry | `Retry-After` | First-party SDK default | Mid-stream errors |
|---|---|---|---|---|---|
| OpenAI (Responses) | 429 rate limit, 500, 503; SDK also 408/409 + connection errors | 400, 401, 403, 404; 429 quota/billing variants | header, seconds (also `retry-after-ms`); SDK honours it only when 0 &lt; v ≤ 120 s | 2 retries, exp. 1 s → 64 s cap, 25% jitter | typed `error` + `response.failed` SSE events; resume exists only in background mode (`store:true`) |
| Anthropic | 429, 500, 529 `overloaded_error`, 409, 504; connection errors | 400, 401, 402, 403, 404, 413 | header, seconds; "Earlier retries will fail" | 2 retries, exp. backoff, honours `retry-after` | documented in-stream `error` event (incl. `overloaded_error`); `ping` events any time |
| Gemini (v1beta) | 429 rate limit, 408, 500, 503 | 400, 401, 403, 404; 429 daily-quota | **no header**; 429 body often carries `google.rpc.RetryInfo.retryDelay` (undocumented; Google's own gemini-cli parses it) | google-genai: 4 attempts, ~1 s initial, 60 s max | SSE error events, same JSON shape as non-streaming |
| xAI (Grok) | 429 (tier RPS/TPM); guidance: exponential backoff | 400, 401, 403, 404, 405, 422 | none documented | n/a (OpenAI-compatible) | not documented; expect Chat Completions semantics |
| llama.cpp server | 503 `Loading model` (readiness gate already handles it); transport | **400 `exceed_context_size_error`** — that is compaction's job, never a retry's | never sets it | n/a | error object inside a 200 stream (#14566) |

Sources: OpenAI error codes and rate limits
(https://developers.openai.com/api/docs/guides/error-codes,
https://developers.openai.com/api/docs/guides/rate-limits — "follow the
`Retry-After` header when it's present… if it's missing, use exponential
backoff with jitter"; SDK internals
https://github.com/openai/openai-python/blob/main/src/openai/_base_client.py —
`MAX_RETRY_AFTER_DELAY = 120 s`, jitter `1 − 0.25·rand`, `x-should-retry`);
Anthropic errors and streaming
(https://platform.claude.com/docs/en/api/errors,
https://platform.claude.com/docs/en/build-with-claude/streaming,
https://platform.claude.com/docs/en/api/rate-limits); Gemini
(https://ai.google.dev/gemini-api/docs/api-errors,
https://ai.google.dev/gemini-api/docs/troubleshooting — retry 429/408/5xx,
never 400/403; RetryInfo evidence
https://github.com/google-gemini/gemini-cli/issues/9248); xAI
(https://docs.x.ai/developers/rate-limits, https://docs.x.ai/developers/debugging);
llama.cpp (https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md,
https://github.com/ggml-org/llama.cpp/issues/14566).

Cross-provider facts that shape the design:

- **The retryable set converges**: transport errors, 408, 429, 500, 502, 503,
  504, 529. The never-retry set converges too: every other 4xx. No
  per-provider policy table is needed (the `supported_sampling_fields`
  pattern, `entities/sampling.rs:247-272`, was considered and is
  unnecessary here) — only *`Retry-After` extraction* differs per client,
  and that belongs where the response body is already in hand.
- **Nobody can resume a broken stream.** The one real mechanism — OpenAI
  background mode with a `sequence_number` cursor — requires `store:true`,
  and our Responses client deliberately sends `store:false`
  (ADR 0004). Every surveyed client (first-party SDKs, aider, LibreChat,
  Open WebUI) retries only requests that have not yet rendered output;
  mid-answer death is surfaced, the partial kept, regeneration manual.
  aider is the closest UX precedent: it prints "Retrying in 1.0 seconds…"
  and re-sends the whole request.
- **Retry budgets are small everywhere**: 2 retries (OpenAI and Anthropic
  SDKs) to 4 attempts (Gemini SDK). The Google SRE book's rules apply:
  ~3 attempts per request, retry at a **single layer** only.

## 3. Current state in the code

- **Contract** — `EngineBackend::chat_stream(&self, req, cancel) ->
  Result<ChatStream>` (`contract.rs:287-310`), `ChatStream =
  Pin<Box<dyn Stream<Item = ChatChunk> + Send>>` (`contract.rs:283`).
  `ChatRequest` is `Clone` (`contract.rs:159`) — a request can be re-issued
  verbatim. Errors are `anyhow` end to end; there is no typed error in
  `shared/api`, and the status code survives only inside the message string:
  all four clients share a copy-pasted non-2xx block that logs and
  `bail!`s `"engine returned status {status}: {detail}"` with the first 500
  chars of the body (`openai/client.rs:137-146`, `anthropic/client.rs:76-85`,
  `gemini/client.rs:108-117`, `responses/client.rs:64-73`, embeddings
  `openai/client.rs:303-312`).
- **Classification today is string matching** on that message:
  `features/compaction.rs:303-331` (`OVERFLOW_MARKERS`) recognizes context
  overflow across all four providers to pick the right advice
  (`generation.rs:1233-1250`). Any new classification must not break those
  markers or the tests that pin the message shape.
- **One turn = up to 9 requests.** `TurnLoop::run` re-enters
  `stream_round` per tool round (`generation.rs:840-863`), plus a final
  no-tools round at the limit (`generation.rs:870-897`). Each is an
  independent retry candidate; a round whose stream already delivered tool
  calls must never be silently re-issued (tool effects are applied by the
  orchestrator after the round — a replay would re-run tools).
- **Cancellation** is a `CancellationToken` created per turn
  (`generation.rs:294`), honoured inside every SSE loop via `select!`
  (`openai/client.rs:158-163`). Any retry sleep must `select!` on the same
  token; the interruptible-sleep precedent is
  `shared/api/managed.rs:288-299`.
- **Cloud backends are born `Ready`, unmonitored, never relaunched**
  (`app/supervisor.rs:484-499`, journal engine.md "periodic server health
  monitoring": *"Cloud isn't even monitored (F1a): the only way to check it
  is a real API call, which costs money"*). So for the cloud, a per-request
  retry is the **only** recovery mechanism there will be — unlike managed,
  where the health monitor (`RECHECK_POLL` 5 s) and
  `RestartBudget` (≤3 per 5 min, `engines.rs:33-76`) own recovery, and a
  request-level retry would just burn attempts while a GGUF reloads for
  minutes.
- **Retry-adjacent machinery that already exists** (prior art for shape and
  taste): the health monitor's cadence/hysteresis constants
  (`supervisor.rs:35-40`) — deliberately constants, not settings
  (roadmap §Engine, "Configurable health-check cadence"); the MCP restart
  budget (`orchestrator/mcp.rs:35-37, 484-492`); the background failure
  streak (`orchestrator/background.rs:19-66`); web_search's *deliberate*
  no-retry ("throttling is sticky, retrying the same provider is pointless"
  — `features/tools/web.rs:97-100`), which this design must not disturb:
  anti-bot 429s and API rate-limit 429s have opposite retry semantics.
- **Timeouts**: engine clients none (see §1.3); tool clients 15–20 s;
  auxiliary turns bound themselves wall-clock from above
  (`TITLE_TIMEOUT` 60 s, `COMPACT_TIMEOUT` 180 s, `IMPERSONATION_TIMEOUT`
  600 s, `REFLECT_TIMEOUT` 120 s).

## 4. Ecosystem: crates vs a hand-rolled loop

- `backoff` — unmaintained, flagged RUSTSEC-2025-0012; do not adopt.
- `backon` — maintained, jitter, `.when()` gating, adjustable delay (can
  honour `Retry-After`); the strongest crate option.
- `tokio-retry2` — maintained; `RetryError` distinguishes
  transient/permanent and carries custom delays.
- `reqwest-retry` + `reqwest-middleware` — maintained, but classifies at the
  middleware layer, does not honour `Retry-After` out of the box, and would
  pull all four clients through a middleware stack for one call site each.

Recommendation: **hand-rolled**. The loop is ~30 lines around one call site
(§5), needs `select!` on our token inside the sleep, wants to emit progress
into a `ChatStream`, and must read `Retry-After` off an error the clients
construct — every crate integration is bigger than the loop itself. This
matches the project's habit (debounce, restart budget, health hysteresis are
all small local implementations, deliberately — `engines.rs:50-51`).

## 5. Design

Six pieces, ordered by dependency; the forks over them are §6.

**(a) A typed error where the status dies today.** A `thiserror` enum in
`shared/api` (the CLAUDE.md convention — thiserror in `shared` — currently
unmet there):

```rust
pub struct EngineError {
    pub kind: EngineErrorKind,        // Transport | Status
    pub status: Option<u16>,
    pub retry_after: Option<Duration>, // header (OpenAI/Anthropic), body RetryInfo (Gemini)
    pub message: String,               // exactly today's text, incl. the 500-char body
}
```

`Display` reproduces today's strings byte-for-byte ("engine returned status
{status}: {detail}", "POST {url}: …"), so `OVERFLOW_MARKERS`, the locale
messages and every existing test keep working; the orchestrator can
`downcast_ref::<EngineError>()` where it wants structure. A shared
`fail_for_status(resp) -> EngineError` helper replaces the four copy-pasted
non-2xx blocks (also deduplication the Sonar gate will like); the Gemini
variant additionally tries `RetryInfo.retryDelay` from the 429 body.
`is_transient()` = `Transport || status ∈ {408, 429, 500, 502, 503, 504, 529}`.

**(b) Two contract additions carry errors and progress through the stream:**

```rust
ChatChunk::Error { message: String, transient: bool }  // terminal, precedes Finished(Error)
ChatChunk::Retry { attempt: u32, max: u32, delay: Duration }
```

Matches are exhaustive across the codebase, so the compiler enumerates every
consumer (main loop `generation.rs:1177-1231`, title, compaction,
impersonation, reflection's tool_loop). `MockBackend` only constructs chunks
and is untouched.

**(c) Clients surface what they already see.** Anthropic wire gains an
`Error { error: { type, message } }` stream-event variant (today swallowed
by `#[serde(other)] Other`) → yields `ChatChunk::Error` (transient when
`type == "overloaded_error"` or `"api_error"`) + `Finished(Error)`. The
Chat Completions client, when chunk deserialization fails, additionally
tries an `{"error": …}` object (llama.cpp #14566) instead of only skipping.
Gemini and Responses attach their already-parsed error messages to
`ChatChunk::Error` instead of a log-only `warn!`. Transport-drop arms
(`Some(Err)`) do the same with the reqwest error text.

**(d) The initial POST becomes cancellable and bounded.** All four clients:
`Client::builder().connect_timeout(10 s)` (no total timeout — an SSE
stream legitimately lives for minutes; no idle timeout — a local prefill
can be silent for minutes on big contexts), and `send()` moves inside
`select!` with the cancel token (the TTS/video pattern), yielding
`Finished(Cancelled)` on cancel. This alone fixes §1.3.

**(e) `RetryBackend`, a decorator over `EngineBackend`.** Constructed in
`supervisor.rs` around cloud (and, per F2, external) backends —
`cloud_chat_setup` is a one-line wrap per provider; managed stays bare. It
returns its stream immediately and drives attempts inside it:

```text
attempt = 1
loop:
    match inner.chat_stream(req.clone(), cancel):
        Err(e) | first chunk is Error{transient} before any content:
            if !transient(e) or attempt > MAX_ATTEMPTS or retry_after > CAP:
                yield Error{message} ; yield Finished(Error) ; return
            yield Retry{attempt, max, delay}
            sleep(delay) selected against cancel     # delay = retry_after or backoff(attempt)
            attempt += 1 ; continue
        stream delivers content:
            forward chunks verbatim; a later failure is forwarded
            (Error + Finished(Error)) and never retried
```

"Content" = the first `Text`/`Thoughts`/`ToolCall`/`ThoughtsSignature`
chunk — the commit point. `Usage` alone does not commit (Anthropic's
`message_start` carries usage before any content; re-emitting it on a retry
is harmless — the counter is overwritten). Because tool calls are content,
a round that produced calls is *by construction* never replayed — no tool
effect can double-fire, and the retry stays at a single layer (the SRE
rule): the orchestrator and the agentic loop never re-issue anything.

**(f) The orchestrator finally tells the user.** `stream_round` handles the
two new chunks: `Error{message}` goes through the same classification the
`Err` arm uses today (extracted into one helper: overflow → the two
compaction keys; otherwise `ui.err.generation_failed`) and becomes an
`AppEvent::Error` feed note — closing the silent-truncation defect for
*every* failure path, retried or not. `Retry{…}` becomes a transient
status-bar chip ("retrying 2/3 in 4 s…", localized both axes-B locales),
cleared by the next chunk or finish — the compaction-roll chip pattern, not
a feed note per attempt.

Policy constants (see F4): `MAX_ATTEMPTS = 3` (initial + 2 retries — the
SDK consensus), backoff 1 s → 2 s, ±25% jitter, `Retry-After` honoured when
≤ `RETRY_AFTER_CAP = 30 s` and otherwise treated as non-transient (an
interactive user is watching; a server that demands minutes is saying
"quota", and the body — which we already surface — says so better than a
frozen spinner). Worst case added latency ≈ 30+30+3 s ≈ 63 s, which stays
under every auxiliary wall-clock timeout except `TITLE_TIMEOUT` 60 s — an
acceptable loss (a title that times out is retried by the next turn's
auto-title, and a *retrying* title beats today's instantly-failed one).

What deliberately does **not** change: managed-mode recovery (supervisor
owns it), web_search's no-retry, the readiness gate, `GenState` (retries
happen inside one `stream_round`, the `generation_id` never changes), the
`stop`-field and EOS invariants, storage.

## 6. Forks (settled)

> **User's decision (2026-08-12): every fork below goes to its recommended
> option — F1(a), F2(a), F3(a), F4 as stated, F5(a), F6(a), F7(a), F8(a),
> F9(a).** Implementation follows §7 in two PRs: stage 1
> `fix/engine-error-surfacing`, stage 2 `feat/cloud-retry-backoff`.

- **F1 — where the retry lives.**
  **(a) `RetryBackend` decorator over `EngineBackend`, progress as stream
  chunks — recommended** (one implementation for all five providers and all
  five call sites; testable against a `MockBackend`-style failing inner;
  keeps clients dumb).
  (b) A loop inside each client's `chat_stream` before the stream is
  returned — no contract changes, but four copies, no coverage of
  pre-content stream deaths, and pre-stream retries are invisible to the UI.
  (c) In the orchestrator (`stream_round`) — entangles retry with
  `GenState` and every auxiliary caller's own deadline; violates
  single-layer.
- **F2 — which modes are wrapped.**
  **(a) Cloud + external — recommended** (external is cloud-shaped: a
  proxy/OpenRouter/LiteLLM answers 429/502 and we don't own the process;
  the local-llama.cpp external case loses nothing — its 503-loading is
  pre-stream, transient, and rides out in one 1-s retry).
  (b) Cloud only — the literal roadmap item.
  (c) Everything incl. managed — rejected: a dead child reloads for
  minutes; the supervisor's relaunch + readiness gate is the honest story,
  and a 3-attempt burst against a loading server adds noise.
- **F3 — the retry window.**
  **(a) Pre-stream errors *and* in-stream failures before the first content
  chunk — recommended** (covers "200 then immediate overloaded_error",
  which is exactly how Anthropic sheds load mid-accept; the user has seen
  nothing, so a retry is invisible and safe).
  (b) Pre-stream `Err` only — simpler, misses the Anthropic case.
  (c) Any mid-stream failure — rejected outright: no provider can resume a
  stream, splicing a regenerated answer under already-rendered text is a
  lie, and no surveyed client does it. After first content: surface, keep
  the partial (already persisted today), let the user decide.
- **F4 — the numbers.** **Recommended: `MAX_ATTEMPTS=3`, base 1 s, factor
  2, ±25% jitter, honour `Retry-After ≤ 30 s`, beyond the cap → fail
  fast.** Alternatives: the OpenAI-SDK cap (120 s) — batch-oriented, feels
  frozen in a TUI; Gemini's 4 attempts — one more 4-s wait for marginal
  gain. All constants, `pub(crate)`, next to the policy.
- **F5 — quota-vs-rate 429 discrimination.** **(a) None in v1 —
  recommended**: two wasted 1–2 s retries, then the body (which names
  `insufficient_quota` etc.) surfaces as today. (b) A marker blacklist for
  billing/quota bodies — the OVERFLOW_MARKERS pattern; add later only if
  the wasted retries actually bite (they cost seconds, not money — a
  rejected request bills nothing).
- **F6 — configurability.** **(a) Constants, no settings — recommended**;
  the precedent is the health-monitor cadence, kept as constants until
  someone asks (roadmap records the same taste for `HEALTHY_POLL`).
  (b) `retry.max_attempts`/`retry.enabled` in settings → "Model" — where
  they would go *if* asked for.
- **F7 — visibility of a retry in progress.**
  **(a) Status-bar transient chip, feed note only for the final failure —
  recommended** (a note per attempt is noise; silence up to 30 s is the
  door left open).
  (b) Silent (file log only). (c) Feed notes per attempt.
- **F8 — how much of §5 ships, and in how many PRs.**
  **(a) Two PRs: `fix/` = (a)-(d)+(f) — typed error, cancellable/bounded
  POST, mid-stream errors surfaced; then `feat/` = (e) retry itself —
  recommended** (the fix half closes real defects and stands alone; the
  feature half is then a small, reviewable decorator).
  (b) One PR — fewer moving parts, mixes a defect fix with a feature.
  (c) Retry only, defects filed as roadmap items — ships the headline while
  leaving silent truncation in place; rejected.
- **F9 — other HTTP callers.** **Out of scope, recorded as groundwork —
  recommended**: embeddings (`Embedder` — would help `/reindex` and
  consolidation bursts), TTS, `youtube_watch`. Same policy could wrap them
  later; each has its own degradation story today.

## 7. Scope estimate

- Stage 1 (`fix/engine-error-surfacing`): `EngineError` + shared
  `fail_for_status` (−4 duplicated blocks), `connect_timeout` +
  cancellable `send()` in four clients, `ChatChunk::Error` + Anthropic
  `error` event + CC error-object parse + Gemini/Responses message
  attachment, the orchestrator note + classification helper, locale keys
  (en/ru). Touches `shared/api/*`, `app/orchestrator/generation.rs`,
  `locales/`. ~10 files. Unit tests: wire fixtures per provider error
  event, classification, no-note→note regression.
- Stage 2 (`feat/cloud-retry-backoff`): `RetryBackend` + policy constants +
  `ChatChunk::Retry` + status chip + supervisor wiring (cloud/external per
  F2) + i18n. ~8 files. Unit tests: §8.
- Docs per AGENTS.md §4: spec §6 (a new §6.8 "Transient errors and retry" +
  §6.1 trait sketch is stale anyway — notes `embed` on `EngineBackend`,
  misses `Usage`/`ThoughtsSignature`/`Error`), architecture §5/§6 (error
  flow), journal engine.md entries, CHANGELOG (user-visible: errors now
  visible; auto-retry), this doc → decisions recorded inline.

## 8. Testing plan

- **Hermetic HTTP-level tests** against a local `TcpListener` serving
  canned responses (the readiness-probe tests' pattern; keep the listener
  real — journal engine.md records the 63 s "dead port on Windows" trap):
  429→200 recovery; `Retry-After` honoured and capped; 400 → exactly one
  request (count on the stub); budget exhaustion → `Error` chunk carries
  the last body; cancel during the backoff sleep → prompt
  `Finished(Cancelled)`; drop after first content → no second request +
  note; drop before content → retried. Sleeps under
  `tokio::time::pause` — "the retry sleeps stay virtual"
  (`supervisor.rs` test precedent).
- **Wire fixtures**: Anthropic `error` event (overloaded mid-stream, both
  pre- and post-content), llama.cpp error-object chunk, Gemini SSE error.
- **Mutation-check the classification** (lessons §2): flip `is_transient`
  on 400 and on 429 — a test must fail each way; assert the request
  *count*, not `is_ok()` (a degraded path returning Ok is this project's
  known vacuous-assertion trap).
- **Live gate**: retry cannot be forced against a real cloud, so live runs
  are for **non-regression of every provider path through the decorator**:
  the standard engine `#[ignore]` smokes against the live llama.cpp stack,
  plus the cloud smokes (OpenAI/Anthropic/Gemini/xAI keys) that already
  exist. Record stack + outcome in the journal entry per AGENTS.md §3.

## 9. Open questions

- Does anything depend on the *absence* of a note after a mid-stream error
  (e.g. demo fixtures, screenshot dumps rendering a truncated reply)? To be
  checked in stage 1's test sweep.
- `RETRY_AFTER_CAP` — 30 s is a taste call on "how long may a TUI visibly
  wait"; the chip makes even 30 s legible, but the user may prefer 10 s.
- Anthropic's documented "continue from where it stopped" recipe (a user
  message asking to continue after a captured partial) — a possible later
  feature riding on the same `Error` surfacing; out of scope here.
- Whether the embeddings path (F9) should come right after: `/reindex`
  re-embeds hundreds of chunks and one 429 currently voids a batch.

## 10. What shipped (2026-08-12)

Both stages landed as designed, with every fork taken at its recommended option.
Two things came out differently, and one gap in this document's own testing plan
turned out to matter.

**Stage 1** (`fix/engine-error-surfacing`): `EngineError` with status,
`Retry-After` (header, `retry-after-ms`, or Gemini's body `RetryInfo`) and the
provider's body as fields, `Display` byte-identical to the four `bail!`s it
replaced; one `check_status` in place of four copies; a 10 s **connect** timeout
and a cancellable initial POST; `ChatChunk::Error{message,transient}` plus the
Anthropic in-stream `error` event, the Gemini error payload, the llama.cpp
error-object-in-a-200-stream and a reason on `response.failed`; one
`engine_error_key`/`engine_error_note` pair serving both failure paths, with a
fourth answer for a reply cut short.

**Stage 2** (`feat/cloud-retry-backoff`): `RetryBackend` over cloud + external,
3 attempts, ~1 s/~2 s jittered downward, `Retry-After` honoured to 30 s,
`ChatChunk::Retry` → `AppEvent::Retrying` → a status-bar chip, an interruptible
backoff.

**Difference 1 — the first attempt runs eagerly.** §5(e) had the decorator return
its stream immediately and drive every attempt inside it. Implemented that way, a
failure that is *not* retried (a `400`, a bad key — the common case) would have
been converted from an `Err` into an `Error` chunk, and `title.rs` distinguishes
those: an `Err` becomes "title generation failed: «reason»", an empty reply becomes
a bare "title is empty". So attempt 1 is awaited *before* returning, and only an
unavoidable wait causes the stream to be returned early. The UI still sees the wait
live, because the wait is exactly the case that returns the stream.

**Difference 2 — `Usage` had to be exempted from the commit rule.** §5(e) already
said "Usage alone does not commit", and the reason turned out to be load-bearing
rather than incidental: Anthropic sends usage in `message_start`, before any
content, so counting it as commitment would have made **every** Anthropic turn
unretryable — the provider whose 529 this feature exists for.

**Gap in §8's plan — the live gate did not cover the decorator.** The plan called
for hermetic HTTP-level tests (delivered: 13, all instant under virtual time) and
for live runs as "non-regression of every provider path through the decorator". But
the live e2e harness builds its backend directly via `MockSupervisor`, so nothing
about the wrapping was exercised against a real model — the decorator would have
had unit coverage only. Fixed by wrapping the harness's `live_backend()` the same
way the supervisor wraps an external server, which puts the whole 30-test e2e set
through it on every run.

**What was measured rather than assumed:** the retry policy's numbers come from the
provider docs in §2 (SDK defaults, header semantics), not from taste. The one
number chosen by taste is `RETRY_AFTER_CAP` = 30 s — the reference SDK trusts the
header up to 120 s, which suits a batch client and not a TUI.

**Groundwork left**, now recorded in the roadmap: the same policy for embeddings,
TTS and `youtube_watch` (F9); quota-vs-rate `429` discrimination (F5); an
HTTP-date `Retry-After` (no provider sends one).
