# ADR 0009 — Chat message speech synthesis (TTS): cloud provider + our own speakable-text extractor + in-process player

**Status:** accepted (2026-07-23). An independent "server slot," like embeddings
([ADR 0002](0002-embeddings-dedicated-server.md)); cloud keys are shared with chat
([ADR 0008](0008-api-key-storage.md)); the speakable-text extractor is a second consumer
of our markdown renderer's events ([ADR 0003](0003-own-markdown-renderer.md)). Research,
spike, and decision points D1–D9 — [docs/research/tts.md](../research/tts.md) (decision
2026-07-23, §13).

**Context.** The chat speech-synthesis track was implemented once and **reverted**: none
of the engines available at the time delivered acceptable quality (OpenAI monotone,
piper "without proper delivery," Gemini a "Frankenstein" due to voice drift between
chunks + `503`). Re-research via a live spike (6 local + 3 cloud candidates on real
hardware, quality judged by the user's ear, drift measured objectively via F0 spread)
showed: local engines are either NO-GO on Russian (wrong stress, a Chinese accent) or
need their own Rust frontend (vosk-tts), while **OpenAI TTS sounds great** (~1 stress
error per paragraph, native-sounding loanwords, no drift). The key product argument —
OpenAI is **not a new vendor**: the key is already stored (ADR 0008), the user has
nothing new to sign up for.

## Decision

### 1. Cloud is the primary path; a local engine is groundwork

The ordering from the original plan (D1: "local comes first") is **reversed**. The
primary mode is **OpenAI TTS** (`gpt-4o-mini-tts`, default voice `onyx` — the only one
auditioned in the spike); scope is **OpenAI + Gemini + external** (three `TtsMode`
variants). **There is no local sidecar.** A local engine (vosk-tts + our own
BERT-based Russian stress frontend, `mindfork tts setup` modeled on
[ADR 0005](0005-python-sandbox-wasmer.md)) is groundwork for an offline/non-OpenAI
audience; ElevenLabs/Azure are not adopted (they require their own signup, which
contradicts the "not a new vendor" argument). A managed mode was not added to the
selector while no implementation exists (precedent: `Claude` before `AnthropicClient` —
we don't ship a non-working entry).

Rejected (research §13, with measurements): **Qwen3-TTS** (Chinese accent on
loanwords, drift under chunking, wrong stress), **Supertonic 3** (stress isn't fixed
even with markup; OpenRAIL-M weights), **vosk-tts** (Apache-2.0 and context-aware
stress, but 22 kHz and its own Rust frontend subsystem) — left as the leading candidate
for the groundwork item.

### 2. TTS is an independent "server slot"

Anthropic has no TTS at all, so the speech-synthesis provider is configured **separately
from the chat engine** — as a dedicated embedding server (ADR 0002). A Claude chat user
speaks their responses via OpenAI/Gemini/external. Clients are **stateless**: built from
a config snapshot per call (`engines_from_config`), no manager slot is needed. The cloud
key is taken from the shared `api_keys` entries by provider (ADR 0008) — no re-entry
needed; the env-variable fallback is preserved. The `TtsSettings` config is entirely
`#[serde(default)]`, no migrations.

### 3. One OpenAI client for cloud and external; Gemini gets its own transport

`OpenAiTts` serves **both** the OpenAI cloud (`pcm` format — raw s16le 24 kHz mono,
bypassing the decoder) **and** any third-party OpenAI-compatible server (`wav` format —
the most portable, via the decoder) — they differ by base URL/key/response format.
Gemini uses native `generateContent` with `responseModalities:["AUDIO"]` (the same
transport as `GeminiClient`), PCM from `inlineData`; a **mandatory read-aloud
directive** (default `Read this text aloud verbatim`) guards against the
`400 Model tried to generate text` error. Gemini's known downsides (503 overload, voice
drift between chunks) are why it's an option, not the default.

### 4. The speakable-text extractor — a second walker over `pulldown-cmark` events

Since the feed's renderer is already our own (ADR 0003), the "speakable" extractor
(`shared/markdown/speak.rs::speakable_text`) is just another consumer of the same
events, not a separate parser. Code blocks, ` ```mermaid `, tables, and display-math
are **skipped with a short spoken note** ("code block skipped") in **the profile's
language** (i18n axis A — this is spoken content, not UI chrome); inline code is read as
text, inline math goes through `latex_to_unicode`, links become text. Thoughts (CoT) and
tool blocks are never spoken (they're not part of `Message.text`). The extractor is a
pure function, covered by golden tests with no engine involved.

### 5. Playback — `rodio` inside the application process

An in-process player (`shared/tts/playback.rs`), not a sidecar player: Windows has no
suitable built-in CLI player, and `Player::append` gives us a queue → a **pipeline**
("synthesize chunk N+1 while N plays") for free, with instant cancellation. The device
is opened **lazily** (on the `/tts` command, not at startup — libasound is noisy on
stderr during enumeration, cpal#384) and **in the command handler** (not in the task),
wrapped in `Arc<Playback>` (`Send+Sync`) and shared with the background task — so that
`/tts pause`/`resume` (via `rodio::Player::pause`/`play`) apply instantly. No sound card
(headless/CI) → `open` returns `Err` rather than panicking: graceful degradation, "audio
unavailable" (the `UnavailableEmbedder` pattern, ADR 0002).

### 6. Orchestration: snapshot + chunking up to the provider limit + role in the pipeline

Modeled on the RAG commands: `/tts` → a conversation snapshot from the orchestrator (the
sole owner of `Chat`) → a cancellable background task; generation is **not gated**
(the snapshot is what gets spoken). Stop points live in one helper (`stop_tts`): by
setting (chat switch, generation start) and unconditionally (exchange deletion `Ctrl+E`,
regeneration `Ctrl+R`, chat deletion). Text is split by sentence and **packed up to the
provider limit** (OpenAI 4096, Gemini/external 2000): a typical message goes out as
**one request** — no "grown pauses" from per-sentence slicing. The pipeline **carries a
role** (`Vec<(MessageRole, String)>`): when a separate "user voice" (`user_voice`) is set
and differs from the assistant's voice, a **second engine** is built — user lines are
read by it (`/tts all`). The voice is a property of the client at construction time, so
it's two instances rather than a `synthesize` parameter (minimal changes to the trait);
with an empty `user_voice` there's a single voice.

### 7. Restored from the reverted stage, not rewritten

The implementation was recovered from the reverted stage 1 (`git show 19ead9f`): `main`
was, code-wise, **identical** to the TTS baseline (the reverts purely undid the feature,
which itself had grown on top of API keys), so recovery amounted to a clean restore of
the files — the code was already verified (had passed tests + a live run before the
revert), built, and passed the gates unchanged.

## Consequences

- **Plus:** the quality of the primary path is confirmed by a live spike and the user's
  ear (including on a challenging philosophical text). All three failure reasons from
  the first attempt are closed: OpenAI monotony was a configuration error
  (voice/model/missing `instructions`); "grown pauses" were an artifact of per-sentence
  slicing, fixed by packing up to the limit (one message = one request); Gemini drift
  remains an inherent property, so it's an option while OpenAI, which has no drift, is
  the default.
- **Plus:** not a new vendor for the user (the OpenAI key already exists, ADR 0008); TTS
  is cheaper than the LLM.
- **Plus:** the speakable-text extractor and the player reuse existing pieces (ADR 0003
  renderer, ADR 0002 graceful-degradation pattern); no config migrations.
- **Minus:** a new **system** C dependency — ALSA on Linux (build `libasound2-dev`,
  runtime `libasound2t64`/`alsa-lib`; Windows is pure windows-rs), + MPL-2.0 (Symphonia)
  in `deny.toml`. The Linux path and packages are checked by CI, not by development on
  Windows.
- **Minus:** OpenAI TTS requires an OpenAI key — a chat user on Claude/Gemini/a local
  model may not have one; TTS is optional, and `external` remains available offline.
- **Groundwork:** a local engine (vosk + our own Rust stress frontend,
  `mindfork tts setup`); stitching chunks across speakable-block boundaries (even
  smoother multi-paragraph delivery); SSE streaming within a chunk; audio caching;
  native Gemini multi-speaker (one request, several voices); speaking a selected message
  from the feed; OS TTS.
