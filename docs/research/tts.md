# Research: chat message speech synthesis (TTS)

**Status:** track **rolled back** (revert `15dc2d0`/`839df19`/`6fd8529` — removed
PR #197 "research", #198 `feat/tts-core`, #199 `feat/tts-sidecar`). Code and
ADR 0009 removed from the repo; this document was restored from `19ead9f` and
extended with **§13 "Post-revert revision"** — it covers the failure analysis, new
candidates (July 2026 recon), and **live spike results** (§13.7).

**Spike outcome (2026-07-22/23):** three local engines tested on live hardware.
Qwen3-TTS and Supertonic 3 — **NO-GO** on Russian sound quality (wrong stress,
Chinese accent/drift); vosk-tts — GO with caveats (the only local engine that
resolves stress from context, Apache-2.0, no drift; downsides — 22 kHz, needs its
own Russian frontend for a Python-free build, misses homographs). Results — §13.7.

**DECISION (2026-07-23): primary engine — OpenAI TTS** (`gpt-4o-mini-tts`, §13.9).
The cloud evaluation showed OpenAI sounds excellent (stress — 1 error per
paragraph, better than all local engines; English loanwords native — no
transliteration needed; no drift). Key product argument: it's **not a new
vendor** — the OpenAI key is already stored (ADR 0008), so the user doesn't need
a new signup; ElevenLabs/Azure would require one and were therefore rejected.
**Stage scope: OpenAI + Gemini + external** (no local sidecar). Local vosk is
**groundwork** for an offline/non-OpenAI audience (§13.8). The §13.8 fork is
**closed** in favor of OpenAI.

**Stage `feat/tts` implemented (2026-07-23), outcome — [ADR 0009](../decisions/0009-tts-speech-synthesis.md):**
OpenAI + Gemini + external restored from the reverted stage 1 (`git show 19ead9f`),
default voice — `onyx`, plus `/tts pause`/`resume` and per-role voices (`/tts all`);
a local managed sidecar is not included (groundwork). 1258 unit tests green, 58 `#[ignore]`;
a live OpenAI smoke test + an interactive TUI run by the user — GO.

**Why it was rolled back:** none of the implemented engines delivered acceptable
quality on real text (detailed analysis — §13.1):

- **OpenAI** — a male voice was selected, but it read in a mechanical monotone
  female voice;
- **piper** (managed sidecar, stage 2) — low quality: "reads like an ordinary
  person, no trained delivery";
- **Gemini** — good quality, but Markdown text turned into a "Frankenstein"
  (every phrase in a different voice and intonation), plus recurring "server
  overloaded" failures.

Forks R1–R9 (§10) were accepted by the user on 2026-07-21 — they remain valid on
**behavior** (the command, Markdown filtering, stop points, UI placement); only the
**engine choice** (R2) and, as a consequence, mode priority (R1) need revisiting.
The data in §3–§9 on protocols, playback, and Markdown extraction is verified and
reusable as-is.

Engine/API data was verified via web research against primary sources (July 2026;
three parallel surveys: local engines, cloud APIs, audio playback in Rust) and
supplemented by a second recon pass (§13).

## 1. Task (user's framing, 2026-07-21)

1. Read chat message text aloud; are **local and cloud** models both feasible.
2. **Skip parts of Markdown** during readout — code, diagrams, etc.
3. Control — via a **command in the input box**:
   - by default read the **last message**;
   - allow specifying **how many recent messages** (user's and the model's) to read;
   - an option to read the chat's **entire conversation**.

**Short answer:** all feasible. Cloud — OpenAI (`/v1/audio/speech`) and Gemini
(our own native `generateContent` with `responseModalities:["AUDIO"]`), both with
Russian, keys already stored (ADR 0008). Locally — the "external" mode covers any
OpenAI-compatible TTS server, and a managed sidecar is a local engine without a
server (engine choice — §4, revisited — §13). Skipping code/diagrams is natural
work for our own markdown pipeline built on `pulldown-cmark` (ADR 0003): the
"speech" extractor is just another walker over the same events. The command
`/tts [N|all|stop]` fits into the existing `/rag` groove.

## 2. Current state (codebase inventory)

There's no audio subsystem in the project yet (the only "multimedia" side effect
is the `arboard` clipboard). Everything else for the feature already exists:

- **Input-box commands** — precedent `/rag` (`features/rag_command.rs`): a pure
  parser → `ChatIntent` → `AppCommand` → a background task with a
  `CancellationToken` (`reset_rag_cancel`: a new command cancels the previous one)
  and progress events. Yellow command highlighting (`ChatScreen::input_is_command`)
  extends with one line.
- **Conversation snapshot** — precedent `F5` copy (`chats.rs::handle_copy_chat`):
  the orchestrator — sole owner of `Chat` — collects messages and hands them to a
  pure formatter (`chat_export::format_conversation`); "User/Assistant" role labels
  are already in the bundles (`ui.export.*`), system/tool message filtering is
  already written.
- **Our own markdown pipeline** (`shared/markdown/`, ADR 0003): `pulldown-cmark`
  events, LaTeX helpers `normalize_delimiters`/`latex_to_unicode` are directly
  reusable; mermaid blocks are ordinary code blocks with info string `mermaid`.
- **Sentence chunking** already exists — `features/tools/rag.rs::split_sentences`
  (private; make it `pub(crate)`).
- **Cloud keys are provider-centric** (ADR 0008): `AppConfig.api_keys` is indexed
  by `CloudProvider::key()` — an OpenAI/Gemini key already entered is automatically
  available to the TTS client too, nothing needs re-entering.
- **Multi-mode config** — modeled on `EmbedSettings` (a third "server slot"):
  `mode: ServerMode` + per-mode sub-structs, `#[serde(default)]` without
  migrations; UI — mode-driven field visibility + a tab strip of subsections in
  the "Model" section.
- **Sidecars** — battle-tested patterns: a managed process
  (`shared/api/managed.rs`, kill/exited monitor), a sidecar with asset
  provisioning via a lock list of URL+sha256 (`mindfork sandbox setup`, ADR 0005) —
  a ready-made template for `mindfork tts setup`.
- **i18n**: status/error text — axis B (`ui.tts.*`); **voice notices**
  ("code block skipped") are content in the message's language → axis A, the
  profile's language (`Profile.language`), like tool text.
- **CI/packages**: the ubuntu jobs in `ci.yml`/`release.yml`/`packaging.yml` have
  no audio headers; `nfpm.yaml`'s deb dependency is only `libc6` — playback will
  add build-dep `libasound2-dev` and runtime-dep `libasound2`/`alsa-lib` (§6).

## 3. Cloud TTS (data verified against official docs, July 2026)

### 3.1 OpenAI — `POST /v1/audio/speech`

A separate endpoint next to Responses (same Bearer key; **org verification is
not required** — unlike reasoning summaries).

- **Models:** `gpt-4o-mini-tts` (current snapshot `…-2025-12-15`) — primary;
  `tts-1`/`tts-1-hd` — legacy (no `instructions` or SSE). Nothing newer exists in
  the dedicated-TTS class (OpenAI's newer audio models are the realtime family, a
  different protocol).
- **Request:** `model` + `input` (**≤ 4096 chars**; `gpt-4o-mini-tts` also caps
  at 2000 input tokens) + `voice` (13 names; `marin`/`cedar` recommended) +
  `instructions` (tone/emotion/language — "speak Russian without an accent") +
  `response_format` (`mp3`|`opus`|`aac`|`flac`|`wav`|`pcm`; `pcm` = **24 kHz
  s16le mono, no header**) + `speed` 0.25–4.0 (**known defect:
  `gpt-4o-mini-tts` ignores it** — ask for speed via `instructions`) +
  `stream_format` (`audio` — chunked body | `sse` — base64 deltas).
- **Russian:** supported (languages "per the Whisper list"); quality is uneven, a
  slight accent is possible, `instructions` helps.
  **Live-run outcome (§13.1): unusable** — monotone reading, voice didn't match
  the one selected.
- **Pricing:** `gpt-4o-mini-tts` — $0.60/1M text input + $12/1M audio output
  (≈ $0.015/min); `tts-1` — $15/1M characters, `tts-1-hd` — $30/1M characters.

### 3.2 Gemini — TTS via our own native `generateContent`

Key win: Gemini's TTS lives **on the same protocol** `GeminiClient` already uses
(`x-goog-api-key`, same URL template) — no new transport needed.
(`generateContent` is marked "Legacy" in favor of the new Interactions API, but
it's fully functional, including for the newest TTS model.)

- **Models:** `gemini-3.1-flash-tts-preview` (new), `gemini-2.5-flash-preview-tts`,
  `gemini-2.5-pro-preview-tts` — **all preview**, no GA TTS model exists.
- **Request:** an ordinary `generateContent` with
  `generationConfig.responseModalities:["AUDIO"]` and
  `speechConfig.voiceConfig.prebuiltVoiceConfig.voiceName` (30 voices: Kore, Puck,
  Charon, …). Style is set in natural language right in the prompt. There's
  multi-speaker support (`multiSpeakerVoiceConfig`, a voice per speaker) —
  interesting groundwork for reading a "User/Assistant" dialogue in different
  voices.
- **Response:** `inlineData.data` — base64 **raw PCM 24 kHz 16-bit mono** (no
  WAV header; `mimeType` looks like `audio/L16;codec=pcm;rate=24000` — parse the
  rate from it, don't hardcode it). `streamGenerateContent` for TTS — **3.1+
  only**.
- **Found via live run (stage 1):** the text must be phrased as a **reading
  directive** (`Say cheerfully: …` / `Read this text aloud verbatim: …`). On a
  "bare" short line the model treats it as a task and responds with `400 Model
  tried to generate text, but it should only be used for TTS`. So the client
  substitutes a neutral directive when "Instructions" is empty.
- **Russian:** supported (code `ru`), the language is auto-detected from the
  text. **Live-run outcome (§13.1): good quality, but voice/intonation drift
  between chunks + `503` overload errors** — unusable as the primary mode.
- **Pricing:** 3.1-flash — $1/1M input + $20/1M audio output; 2.5-flash — $0.50 +
  $10; the flash models have a **free tier**. TTS context — 32k tokens.

### 3.3 Anthropic — no TTS (confirmed)

The full API surface listing (Messages, Batches, Token Counting, Models + Files/
Skills/Agents/Sessions beta) contains no speech/audio. Design consequence: **the
TTS provider is configured independently of the chat engine** (its own "engine"
slot, like embeddings, ADR 0002) — a Claude user speaks output via
OpenAI/Gemini/local.

### 3.4 Local OpenAI-compatible TTS servers (for `external` mode)

The ecosystem of servers implementing `POST /v1/audio/speech` is mature — one
`external` mode covers all of them:

| Server | Alive (07.2026) | Windows | Russian | Notes |
|---|---|---|---|---|
| Kokoro-FastAPI | ✅ v0.6.0 | ✅ native | ❌ | textbook compatibility, streaming |
| speaches | ✅ | ⚠️ Docker | ✅ piper ru_RU ×4 | ex-faster-whisper-server |
| LocalAI | ✅ v4.7.1 | ❌ Docker/WSL | ✅ piper/XTTS | `wav` without ffmpeg |
| AllTalk v2 | ⚠️ | ✅ installer | ✅ XTTS | ignores `model` |
| chatterbox-tts-api | ✅ | ⚠️ Docker | ✅ (23 languages) | nonstandard extra params |
| openedai-speech | ❌ archived 01.2026 | — | — | itself points to speaches/Kokoro |
| openai-edge-tts | ✅ | ✅ | ✅ | proxies the online Edge service — gray area |
| **qwentts.cpp `tts-server`** | ✅ | ✅ (`buildcuda.cmd`) | ✅ Qwen3-TTS | **see §13.2 — main candidate** |

**Common parameter denominator** (what `external` mode must be able to send):
`model` + `input` + `voice` + `response_format` + `speed`; `voice` is a **free-text
field** (each server has its own names), `model` is ignored by many, `speed` isn't
always present → send only what's set (our `skip_serializing_if` pattern). The
most portable response format is **`wav`** (the only one some servers support,
LocalAI's default); streaming rests on a chunked body. Nobody has a "language"
parameter — the language is set via the voice/model or by auto-detection. Azure
OpenAI is covered by `external` with a **full URL override** (it has a
`?api-version=…` query); ElevenLabs/Google Cloud TTS are OpenAI-incompatible
(covered by third-party proxies like LiteLLM → the same `external`).

## 4. Local TTS as a sidecar (no server)

> **Historical section.** Reflects the state as of 2026-07-21 and justifies the
> choice of piper, which **didn't pan out** on quality. The current engine choice
> is §13.

**A negative result important for the project:** `llama-server` has **no** TTS
endpoint (`/v1/audio/speech` is absent as of July 2026) — the already-running chat
engine cannot be reused. The `llama-tts` example (OuteTTS) is a narrow CLI
experiment: Russian only appeared in OuteTTS-1.0 (Apache-2.0), but its llama.cpp
support is an unfinished Draft PR (#12794); the working path is the Python
library. Rejected with the note "revisit if `llama-server` gains audio/speech."
(Re-checked 07.2026 — §13.4: the PR is still draft, last substantive comment
2025-05-19. This path is dead.)

The landscape by "Russian+English, CPU, Windows+Linux, permissive" criteria:

- **sherpa-onnx (k2-fsa, Apache-2.0)** — the recommended runtime. Precompiled
  CLI distributions (`sherpa-onnx-offline-tts`; release v1.13.4 from 07.07.2026:
  ~19 MB win-x64 / ~28 MB linux-x64), supports the VITS/Piper, Matcha, Kokoro,
  KittenTTS, Supertonic, and ZipVoice families; **piper voices are converted into
  its model zoo** — Russian `vits-piper-ru_RU-{dmitri,irina,ruslan}-medium` are
  confirmed (denis isn't confirmed in the zoo, convertible with their own
  script). There's also an **official Rust crate `sherpa-onnx` 1.13.4** —
  in-process, no sidecar at all (statically linked C++ library; the build script
  downloads a prebuilt archive at build time — pin via `SHERPA_ONNX_LIB_DIR` for
  reproducibility), gives a per-frame streaming callback and removes the
  per-call model-load cost.
  **⚠️ A licensing landmine, uncovered later — §13.3: the build statically pulls
  in espeak-ng (GPL-3.0).**
- **Piper**: the live successor **piper1-gpl** (v1.5.0, 17.07.2026) is
  **GPL-3.0 and pip-only distribution** (pulls in a Python environment — against
  the "binary next to the exe" spirit, though "GPL as a separate process,
  downloaded by a setup command" is formally allowed); the **old MIT binary
  2023.11.14-2** (~22–26 MB) works and has an ideal sidecar protocol (stdin→WAV,
  `--output-raw` — streams PCM 16-bit mono 22050 Hz as it generates,
  `--json-input` — long-lived loop), but upstream is archived (10.2025) — no
  bugfixes coming.
- **ru_RU voices** (piper family, medium, 22.05 kHz, ~63 MB ONNX per voice):
  **denis/dmitri — CC0** ✅, irina — "License: Unknown" ⚠️, **ruslan —
  CC BY-NC-SA** ❌ (non-commercial — not going into the permissive set). Dozens
  of English voices.
- **Latin script in a Russian voice** — a known piper/espeak-frontend defect:
  English insertions in Russian text read with a strong accent/distortion
  (noticeable for a chat full of loanwords and API names). Our-side mitigation
  — **alphabet-based routing**: cut the text into Cyrillic/Latin runs and
  synthesize with two voices (ru+en) — groundwork for stage 2.
- **Silero TTS** — the best Russian in its class (stress, SSML), but
  **CC BY-NC-SA** (MIT only for the base `v5_cis_base`) and PyTorch JIT only
  (outside Python — libtorch runs to hundreds of MB). An "NC trap" — not taking
  it, flagged explicitly.
- **Supertonic 3** — a 2026 dark horse: MIT code + OpenRAIL-M weights
  (commercial use allowed, but with use restrictions — not classic permissive),
  pure ONNX ~99M, **31 languages including Russian**, an official Rust example,
  RTF 0.3 even on an e-reader. Russian quality unheard; sherpa-onnx currently
  packages v2 without ru. A candidate to "listen to at the start of stage 2."
  **Update 07.2026: sherpa-onnx v1.13.2 (13.05.2026) added Supertonic 3 with
  Russian — see §13.2.**
- **Dropped**: Kokoro (no Russian), XTTS-v2 (CPML weights — non-commercial;
  Coqui shut down, no one to buy a license from), Fish Speech/OpenAudio
  (research license), F5-TTS-ru (NC weights), Chatterbox (MIT and Russian
  exist, but Python+torch 0.5B — not a CPU sidecar),
  Orpheus/KittenTTS/MeloTTS/Dia/Pocket TTS (no Russian), vosk-tts (Apache-2.0
  and Russian, but ru-only and Python distribution), RHVoice (pre-neural
  quality, NC voices).

**A stage-2 spike finding (2026-07-21) that killed sub-option (a):**
`sherpa-onnx-offline-tts` accepts text **only as a positional argument**, and its
narrow `main()` forces the Windows CRT to convert argv from UTF-16 to the ANSI
code page (`1252` on the dev machine) — **all Cyrillic becomes `?`, silence comes
out** (verified from both bash and PowerShell; an external manifest with
`activeCodePage=UTF-8` is ignored, and the CLI has no "text from file/stdin"
option). So **sub-option (c)** was taken — the MIT `piper` binary: text goes
**via stdin** (UTF-8 bytes), raw PCM comes out on stdout (`--output_raw`). A
nuance: piper requires the **original** voices from HuggingFace — re-exported
ONNX from sherpa's model zoo crashes it (`STATUS_STACK_BUFFER_OVERRUN`). Spike
measurements (Windows, CPU, 2 threads): model load ≈0.8 s, synthesis RTF 0.05
(piper voices, 22.05 kHz) and 0.19 (Supertonic 3, 44.1 kHz).

> **Important:** the argv defect is a property of the **CLI wrapper**, not the
> library. Using the Rust crate `sherpa-onnx` (in-process, `&str` — native UTF-8)
> the problem doesn't occur at all. This is a standalone argument against a CLI
> sidecar.

**Landing (stage 2, was implemented and rolled back)**: provisioning modeled on
ADR 0005 — `mindfork tts setup` downloads the binary + voices (ru CC0 + en) into
`data/tts/` from a lock list of URL+sha256. The sidecar overhead — loading the
model on every spawn (63 MB ONNX, ~0.8 s) — was mitigated by coarser chunking
(`max_input_chars` = 2000); the long-lived `--json-input` protocol is groundwork.
**piper's quality turned out to be unacceptable (§13.1) — the stage was rolled
back.**

## 5. Extracting "speakable" text from Markdown

The answer to "can code and diagrams be skipped" is yes, and we're in an ideal
spot for it: the feed renderer is already ours (ADR 0003), so the "speech"
extractor is a second consumer of the same `pulldown-cmark` events, living
alongside it (`shared/markdown/speak.rs`): `speakable_text(markdown, loc) -> String`.

**This section is untouched by the §13 revision** — extraction doesn't depend on
the engine and remains valid in full. Note separately: the "Markdown turned into
a Frankenstein" complaint relates **not** to extraction (it worked fine) but to
Gemini's autoregressive nature — see §13.1.

Proposed rules (R4):

| Element | Read-aloud behavior |
|---|---|
| Code block (fenced/indented) | **skip** + a short voice notice "(code block skipped)" |
| ` ```mermaid ` | **skip** + notice "(diagram skipped)" (it's a code block with info string `mermaid` — no separate detection needed) |
| Table | **skip** + notice "(table skipped)" (reading cell by cell is torture; "read row by row" is groundwork) |
| Display math (`$$…$$`) | **skip** + notice "(formula skipped)" |
| Inline math (`$…$`) | converted via the existing `latex_to_unicode` (a short `x²` reads tolerably; better than a hole in the sentence) |
| `inline code` | read as plain text (short identifiers are useful in speech) |
| Link `[text](url)` | reads **the text**, URL omitted |
| Bare URL / autolink | replaced with the domain ("link: example.com") |
| Image | alt text if present; otherwise a notice |
| Headings, lists, blockquotes | read as text; a period/break at boundaries (a pause for TTS) |
| **Thoughts (CoT)** | **not read aloud** (a separate `Message.thoughts` field, not part of the text) |
| **Tool calls/results** | **not read aloud** (the Tool role and `tool_calls` are skipped, as in the `F5` export by default) |
| Emoji | left as-is (engines either read the name or ignore it; a filter is groundwork) |

Voice notices come from the locale bundles in the **profile's language** (axis A:
a notice is part of the speech content, not UI chrome; precedent — tool text).
The extractor is a pure function, tested with golden tests without the engine.

## 6. Audio playback (data verified, July 2026)

**This section is untouched by the §13 revision** — the player choice doesn't
depend on the engine choice.

**Recommendation: `rodio` 0.22 in-process** (`default-features = false`,
`features = ["playback", "wav", "mp3"]`) — the only candidate that has the
needed semantics "out of the box":

- **Queue**: `Player::append()` plays sources sequentially — the pipeline of
  "synthesizing chunk N+1 while N plays" comes for free; when the queue empties
  — silence until the next chunk (ideal for TTS). **Cancellation is instant**:
  `Player::clear()`/`stop()`, and there's `skip_one()`.
- **API 0.21/0.22 broken compatibility** (materials older than 2025 are stale):
  `OutputStream` → `MixerDeviceSink`, `Sink` → `Player`, entry point —
  `DeviceSinkBuilder::open_default_sink()`. With cpal 0.17 the handle and player
  are **`Send+Sync`** — a dedicated audio thread is no longer required, the
  handle can live as a field of a task/orchestrator.
- **Headless/CI**: `open_default_sink()` returns `Result` — **an error, not a
  panic** → graceful degradation to "audio unavailable" (the `UnavailableEmbedder`
  pattern).
- **Formats**: Symphonia decodes WAV/MP3 (pure Rust); raw PCM (OpenAI `pcm`,
  Gemini L16) needs no decoder at all — `SamplesBuffer` (s16le → f32).
- **Two TUI traps**: `log_on_drop(false)` is mandatory (otherwise rodio prints
  to stderr over the TUI); on Linux, libasound itself is noisy on stderr while
  enumerating devices (cpal#384) — silence it with `snd_lib_error_set_handler`
  or open the sink before entering the alt screen.
- **Cost**: the only new **system** C dependency in the project — ALSA on Linux
  (build: `libasound2-dev` in the ci/release/packaging apt steps; runtime:
  `libasound2` (deb) / `alsa-lib` (rpm/arch) in `nfpm.yaml`; Windows — pure
  windows-rs, nothing needed). PipeWire/PulseAudio systems work through the
  standard `pipewire-alsa`. Plus `deny.toml`: **add MPL-2.0** (Symphonia; weak
  file-level copyleft, no obligations without modifying the files). Adds
  ≈ 1 MB to the binary.
- **Alternatives rejected**: kira/awedio/tinyaudio — queue+cancellation would
  have to be built by hand, or carry ecosystem risk; **sidecar players don't
  work** — Windows has no suitable built-in CLI player (PowerShell
  `SoundPlayer` is WAV-only, spawning a process per chunk), cancellation and
  chunk stitching would be worse by construction.

**Platform OS TTS** ("no models at all") — evaluated, not recommended for the MVP
(R9): on Windows, WinRT voices are legacy quality ("Microsoft Irina", natural
voices are closed off to apps), the correct path is a direct
`Windows.Media.SpeechSynthesis` call synthesizing **into a WAV buffer** → our own
queue (the `tts`-rs crate doesn't return PCM — its own audio path bypasses the
queue and cancellation); on Linux, speech-dispatcher = espeak-ng (robotic) and
isn't preinstalled everywhere. The `msedge-tts` crate (the online Edge service)
is unofficial, with a history of 403 blocks/token rotations — can't be the default.

## 7. The `/tts` command and UX

Syntax (mirrors `/rag`, case-insensitive; parser — `features/tts_command.rs`):

```
/tts            read the last message aloud
/tts N          read the last N messages aloud (user's and the model's)
/tts all        read the chat's entire conversation aloud
/tts stop       stop playback
```

- **What counts as a message** = user/assistant with non-empty text (system/tool
  are skipped — the same rules as in the `F5` export). Order — chronological.
- **Voice role prefixes** ("User." / "Assistant.", in the profile's language) are
  available **in all three variants**, including a bare `/tts` (user's decision,
  R6) — it's a settings toggle "Speak roles," not behavior hardwired to `N > 1`.
- A new `/tts` command **interrupts** the current playback and starts a new one;
  `/tts stop` stops it manually.
- **Automatic stops** (R8, user's decision):
  - *configurable* — on **chat switch** (default: **interrupt**) and on
    **generation start** (default: **don't interrupt**): two independent settings;
  - *unconditional* — on **deleting an exchange** (`Ctrl+E`), **regeneration**
    (`Ctrl+R`), and **deleting the chat**: the text being spoken no longer
    exists, nothing to continue.
- The command also works while generation is running (like `/rag`): a
  **snapshot** taken at command time is read aloud.
- **Indicator**: a quiet status-bar chip "♪ speaking" for the duration of
  synthesis/playback (glyph `♪` U+266A is in WGL4 — no substitute needed in
  compat mode; `*` if desired). Errors (engine not configured, no audio
  device, HTTP error) — as a note in the feed.
- A syntax error → a hint in the feed (like `/rag`); the command is highlighted
  yellow (`input_is_command` + `tts_command::parse`); `/tts` — in the help
  overlay (`F1`) and README/spec.

## 8. Implementation architecture (sketch)

Mode taxonomy — mirrors the engines (a flat selector, ADR 0004):

```
TtsSettings {
    mode:     TtsMode { Managed | External | OpenAi | Gemini },   // managed — sidecar, §4/§13
    managed:  TtsManagedSettings { …engine paths/voices… },
    external: TtsExternalSettings { url, model_name, voice, api_key_env },
    openai:   TtsOpenAiSettings  { model = "gpt-4o-mini-tts", voice = "marin", instructions },
    gemini:   TtsGeminiSettings  { model = "gemini-2.5-flash-preview-tts", voice = "Kore" },
    speed:    f32,   // where supported; OpenAI 4o-mini — via instructions

    // behavior (R6/R8, user's decisions)
    speak_roles:              bool,  // role prefixes — in all three command variants
    stop_on_chat_switch:      bool,  // true by default
    stop_on_generation_start: bool,  // false by default
}
```

`speak_roles` default is **off** (user's decision 2026-07-21): the most common
scenario (`/tts` on the last reply) doesn't need a prefix, and one toggle
covers all three command variants at once.

Unconditional stops (delete exchange / regenerate / delete chat) are an
**invariant, not a setting**: the text being spoken no longer exists.

- Everything `#[serde(default)]` — no migrations. Cloud keys are **shared**
  `api_keys` entries keyed by `CloudProvider::key()` (ADR 0008): nothing new to
  enter. Not configured → `/tts` returns a clear error suggesting opening
  settings.
- **FSD layout**: `features/tts_command.rs` (parser); `shared/markdown/speak.rs`
  (speech extractor); `shared/tts/` — `mod.rs` (trait
  `TtsEngine { synthesize(text, cancel) -> AudioClip }`, `AudioClip { format, bytes }`),
  `openai.rs` (one client for both cloud OpenAI **and** external — different
  base_url/key), `gemini.rs` (native generateContent+AUDIO), `playback.rs` (the
  player: a lazy `MixerDeviceSink` + `Player` queue + cancellation),
  `sidecar.rs` (the local engine). Clients are stateless (reqwest is already a
  dependency) — built per call from a config snapshot, no manager slot needed
  (one would appear for the sidecar, if it needs to stay alive long-lived).
- **Orchestration** (`app/orchestrator/tts.rs`, mirrors `rag.rs`):
  `AppCommand::Tts { scope }`/`TtsStop`; `handle_tts` — **no** generation
  gates (we read a snapshot); `tts_cancel: CancellationToken` (a new command
  cancels the previous one; `Quit` cancels too); `spawn_tts` (a background
  task): messages → `speakable_text` → chunks by sentence (`split_sentences`,
  grouped up to ~1–2k characters, capped at OpenAI's 4096) → a pipeline
  "synthesize chunk i+1 while i plays" → `AppEvent::Tts { active }` for the
  chip.
- **Stop points** — one private helper `stop_tts()` (trips the token), called
  from: `handle_switch` (if `stop_on_chat_switch`), `start_generation` (if
  `stop_on_generation_start`), and **unconditionally** — `handle_regenerate`,
  `handle_delete_last`, `handle_delete_chat`. This way behavior is defined in
  one place, not scattered across handlers (precedent — `reset_rag_cancel`).
- **Formats per mode**: OpenAI — request `pcm` (24 kHz s16le → `SamplesBuffer`,
  no decoder); Gemini — already returns PCM (parse the rate from `mimeType`);
  external — request `wav` (the most portable) → `Decoder`. Streaming within a
  single chunk (SSE/chunked) isn't needed for the MVP — the sentence pipeline
  already delivers a fast first sound; SSE is groundwork.
- **Tests**: pure (command parser, speech-extractor golden tests, chunking,
  message selection/role prefixes), client request wire shapes; `#[ignore]`
  smokes: OpenAI (`MINDFORK_OPENAI_KEY`), Gemini (`MINDFORK_GEMINI_KEY`),
  external (`MINDFORK_TTS_URL` against a local server). CI playback coverage —
  only "headless opens with an error, not a panic"; the actual sound is a
  manual check (a live run per AGENTS.md §3).

## 9. Settings in the UI

Decision (R7): **a separate "Speech" tab** — the fourth in the tab strip of the
"Model" section (Assistant · Impersonation · Embeddings · Speech): TTS is another
"server slot" with the same mode-driven fields. Composition:

- an **"Engine"** group — mode selector, model, voice, speed; for external — URL;
  key — the shared provider-status field (ADR 0008, no need to re-enter);
- a **"Behavior"** group — toggles "Speak roles" (R6), "Interrupt on chat
  switch" (on), "Interrupt on generation start" (off) (R8).

Rejected alternative — a group in "Interface": the fields are engine fields, not
appearance ones, and the tab gives them mode-driven visibility "for free."

## 10. Decisions (accepted by the user 2026-07-21)

R4 and R7 were confirmed explicitly; R5, R6, and R8 were accepted **with the
user's amendments** (reflected in §7–§9); the rest — per the recommendations.
**R1 and R2 need revisiting after the revert — see §13.**

- **R1. MVP scope (stage 1)** — *per the recommendation*: the `/tts` command +
  extractor + player + modes **OpenAi / Gemini / External**. A local sidecar
  (managed) — as stage 2. **Revised (§13.5): the order changes — the local
  engine becomes primary, clouds move to an optional role.**
- **R2. The sidecar's local engine** — *was*: **sherpa-onnx** (Apache-2.0) +
  piper voices `ru_RU-dmitri` (CC0). Sub-options: (a) a CLI sidecar
  `sherpa-onnx-offline-tts`; (b) the official Rust crate in-process; (c) the
  piper MIT binary 2023. Spike outcome: (a) unsuitable for Cyrillic on
  Windows (ANSI argv, §4) → (c) was taken. **Revised (§13.2): (c) failed on
  quality; new candidates — Qwen3-TTS via `qwentts.cpp` and Supertonic 3.**
- **R3. Playback** — *per the recommendation*: `rodio` in-process (§6). The
  accepted cost: a system C dependency ALSA on Linux + MPL-2.0 (Symphonia) in
  `deny.toml`. **Still in effect.**
- **R4. Markdown filtering** — *confirmed*: the table in §5. **Still in
  effect.**
- **R5. Command syntax** — *user's amendment*: `/tts`, `/tts N`, `/tts all`,
  `/tts stop` (§7). **Still in effect.**
- **R6. Roles** — *user's amendment*: a "Speak roles" toggle, applies in all
  three command variants. **Still in effect.**
- **R7. Settings in the UI** — *confirmed*: the "Speech" tab in the "Model"
  section (§9). **Still in effect.**
- **R8. Interrupting playback** — *user's amendment*: two independent
  settings + unconditional stops. **Still in effect.**
- **R9. An "OS TTS with no models" mode** — *per the recommendation*: **out of
  scope**. **Still in effect.**

## 11. Stage plan (history)

1. **Stage 1 `feat/tts-core`** — was done, **rolled back** (`839df19`): config
   + the "Speech" tab, the `/tts` command, the speech extractor with notices,
   the `rodio` player, OpenAi/Gemini/External clients, stop points, the status
   chip, i18n, CI/packaging tweaks (ALSA, deny.toml), smokes.
2. **Stage 2 `feat/tts-sidecar`** — was done, **rolled back** (`6fd8529`): a
   managed mode on the `piper` binary + `mindfork tts setup` + ru/en voices.
3. **ADR 0009** — was written, **removed** along with the implementation.
4. **New plan** — §13.5.

The infrastructure part (extractor, command, player, stop points, the UI tab)
wasn't affected by the failure — it worked as intended; no need to rewrite it
on a repeat pass, it can be restored from the reverted commits
(`git show 19ead9f`).

## 12. Key sources (first wave, 07.2026)

- OpenAI: the `createSpeech` reference and the Text-to-speech guide
  (developers.openai.com) — parameters/limits/pricing/`stream_format`; the
  `gpt-4o-mini-tts` model page.
- Gemini: "Speech generation" (ai.google.dev, current and legacy pages) —
  `generateContent` shapes, voices, languages, pricing, streaming from 3.1.
- Anthropic: the API overview (platform.claude.com/docs/en/api/overview) — a
  surface with no speech/audio.
- Local servers: the Kokoro-FastAPI, speaches (+ the HF piper-voices ru_RU
  registry), LocalAI, AllTalk v2 (the OpenAI-endpoint wiki), chatterbox-tts-api,
  openedai-speech (archived) repositories.
- Local engines: sherpa-onnx (releases v1.13.4 + the model zoo + the official
  crate docs.rs/sherpa-onnx); piper (rhasspy/piper — archived, release
  2023.11.14-2; OHF-Voice/piper1-gpl v1.5.0 — GPL, pip-only); HF
  `rhasspy/piper-voices` (the MODEL_CARD for ru_RU voices — data licenses:
  denis/dmitri CC0, irina Unknown, ruslan CC BY-NC-SA); silero-models (LICENSE
  CC BY-NC-SA; MIT only for v5_cis_base); Supertonic
  (supertone-inc/supertonic + HF Supertone/supertonic-3, OpenRAIL-M);
  llama.cpp `tools/tts` + Draft PR #12794 (OuteTTS-1.0); HF
  OuteAI/OuteTTS-1.0-0.6B.
- Playback: rodio 0.22 (docs.rs/CHANGELOG/UPGRADE), cpal (README/CHANGELOG —
  Send streams 0.17, windows-rs on WASAPI, ALSA requirements), Symphonia
  (MPL-2.0), cpal#384 (ALSA noise on stderr); tts-rs, NaturalVoiceSAPIAdapter,
  msedge-tts / edge-tts#290/#458 (a history of blocks).

---

## 13. Post-revert revision (recon 2026-07-22)

A second wave of research — three parallel web surveys (the local-engine
landscape; Russian quality; Rust runtimes) + a personal recheck of the key
licenses against HF cards and repositories. Constraints set by the user going
in: **GPU is allowed** (RTX 4090), delivery — **binary + model only, no
Python environment**, **Russian and English equally important**.

### 13.1 Diagnosis: why all three engines failed

This matters more than the candidate list — it explains exactly what to look
for.

- **Gemini — a "Frankenstein."** The cause **isn't Markdown** (the extractor
  in §5 worked fine) and isn't chunking. Gemini TTS is an **autoregressive
  LLM**: it generates speech token by token and re-"picks" timbre and
  intonation on every chunk. Drift between chunks is a structural property
  of the class, not an integration defect. Plus `503` errors on preview
  models (Gemini has no GA TTS version).
- **piper — "reads like an ordinary person."** The root cause is the
  **acoustic model and data**, not missing stress marks:
  - the Balalaika paper (arXiv 2507.13563) trained VITS on the best Russian
    dataset: overall MOS **3.618**, but **Intonation MOS 2.532** — flat
    intonation is inherent to the class; adding punctuation and stress marks
    improved the metrics, but "gaps were modest";
  - N. Shmyrev's evaluation (alphacephei, 14 engines): "you can deal with
    plain intonation but artifacts are really annoying" — flatness is
    acknowledged as a property;
  - the `irina` voice is a fine-tune from the English `lessac` on roughly
    **1 hour** of RHVoice data;
  - piper's Russian stress marker is **broken**
    ([rhasspy/piper#684](https://github.com/rhasspy/piper/issues/684), the
    repository was archived on 2025-10-06).

  **Consequence:** bolting a stress-placement tool (RUAccent) onto piper
  would remove homograph errors, but would **not** make the speech sound
  professionally delivered. The stage would have been wasted.
- **OpenAI** — a known `gpt-4o-mini-tts` defect; not fixable client-side.

**The selection criterion this implies:** the engine must be (a) trained on
natural speech, not read-aloud narration, and at the same time
(b) **deterministic** or hard-pinned to a fixed timbre — otherwise
expressiveness is bought at the price of "drift" coming back.

### 13.2 Two candidates (licenses personally verified against primary sources)

| | **Qwen3-TTS** | **Supertonic 3** |
|---|---|---|
| Code | Apache-2.0 ([QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)) | MIT ("This project's sample code is released under the MIT License") |
| **Weights** | **Apache-2.0** ✅ (the [Qwen3-TTS-12Hz-1.7B-Base](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base) card, released 2026-01-22) | **OpenRAIL-M** ⚠️ — commercial use and redistribution allowed, but with use restrictions that must be passed through to the end user |
| Russian | ✅ 1 of 10 languages; **WER 3.212 vs. 3.878 for ElevenLabs** and 4.281 for MiniMax (tech report, Table 6) | ✅ 1 of 31 languages; WER 3.99 (Minimax-MLS-test) |
| Class | LLM-TTS, 1.7B (a 0.6B variant exists) | non-LLM, **~99M**, pure ONNX |
| Runtime without Python | ✅ [qwentts.cpp](https://github.com/ServeurpersoCom/qwentts.cpp) — **MIT**, GGML, CPU/CUDA/Vulkan/Metal, `buildcuda.cmd`/`buildvulkan.cmd`, **`tts-server` with `/v1/audio/speech`**; also [pure C](https://github.com/gabriele-mastrapasqua/qwen3-tts) and [Rust on candle](https://github.com/TrevorS/qwen3-tts-rs) (both MIT) | ✅ native Rust upstream (`rust/`, on `ort` 2.0-rc), 4 ONNX files + `voice.bin`; or the `sherpa-onnx` crate (with the caveat in §13.3) |
| Speed | RTF 0.35 on RTX 5050; 0.5–0.7 on CPU for 0.6B (int4/int8, M1) | RTF ≈0.3 **on an e-reader**; ~6.1× realtime vs. 2.0× for Kokoro-82M |
| **G2P / espeak-ng** | not needed | **not needed** — works on raw character text (arXiv 2503.23108), the G2P module is architecturally absent |
| Voice stability | 9 fixed CustomVoice timbres **with no reference audio** (no timbre drift from zero-shot), `--seed`/low `temperature`; but LLM → **prosody may wander slightly across calls** | **deterministic by construction** — immune to Gemini's failure class |
| Stress/homographs | ❌ trained with no stress annotation ([discussion #185](https://github.com/QwenLM/Qwen3-TTS/discussions/185), no maintainer reply) | ❌ no explicit stress frontend either |

**A caveat on the WER metric:** an excellent WER and the homograph problem aren't
contradictory — WER is computed via ASR transcription, and speech recognition is nearly
insensitive to stress (the two stress variants of a Russian homograph transcribe to the
same line). The number simply **doesn't see** this problem.

**A caveat on Qwen's timbres:** the 9 CustomVoice presets are zh/en/ja/ko
(ryan, aiden, vivian, serena, uncle_fu, dylan, eric, ono_anna, sohee). There's no Russian
preset; they should in principle read Russian (timbre and language are decoupled), but this
**needs verifying by ear**, not taking on faith. Russian-English code-switching has been
officially measured only for the zh-en pair.

### 13.3 A licensing landmine: espeak-ng inside sherpa-onnx

A finding important for an MIT project, uncovered by the second wave and **not
accounted for** in §4:

- sherpa-onnx's `CMakeLists.txt`: with `SHERPA_ONNX_ENABLE_TTS` (**ON by
  default**), `espeak-ng-for-piper` is pulled in; static builds contain
  `libespeak-ng.a`. The prebuilt archives downloaded by the Rust crate's
  build script almost certainly contain it.
- espeak-ng is **GPL-3.0**; a request to switch to LGPL
  ([espeak-ng#2131](https://github.com/espeak-ng/espeak-ng/issues/2131)) was
  closed as "not planned."
- k2-fsa itself acknowledged this:
  [sherpa-onnx#3731](https://github.com/k2-fsa/sherpa-onnx/issues/3731)
  (2026-07-08) — "incompatible with the Apache-2.0 license of sherpa-onnx",
  removal is planned for **2.0.0** (a breaking change).

Consequences by usage mode: static linking into our `.exe` → the binary as a
combined work falls under GPL-3.0; a sidecar process → "arm's length," no
claim arises (**exactly why the previous piper sidecar was clean**). **Neither
candidate in §13.2 needs espeak at all — the question doesn't arise.**

Separately: a permissive Russian G2P effectively doesn't exist in 2026
(`piper-plus-g2p` — MIT, but no Russian; gruut/OpenPhonemizer — Python or
English only). The only clean way out is a model that doesn't need G2P.

### 13.4 What was rechecked and stayed negative

- **llama.cpp / GGUF** — this path is still dead: PR #12794 (OuteTTS 1.0) is
  still **draft**, the last substantive comment 2025-05-19; `llama-server` has
  no `/v1/audio/speech` (what was added in #13714 is audio **input**, the ASR
  side).
- **ESpeech-TTS-1** (the best Russian per Habr reviews, trained on podcasts →
  a "professional-narrator" delivery, RUAccent built in) — **disqualified
  twice**: Python-only (violates "no Python") and **unproven weight
  provenance** — built on F5-TTS, whose pretrained weights are **CC-BY-NC**
  ("due to the training data Emilia"); the claimed Apache-2.0 may be legally
  unsound, there's no public statement of "trained from scratch."
- **Silero** — MIT only for `v5_cis_base`/`_nostress` (no auto-stress); all
  the convenient `v5_*_ru` and named voices are **CC-BY-NC**. Plus
  `.pt/.jit` → PyTorch.
- **F5-TTS_RUSSIAN** (Misha24-10) — quality excellent, weights are
  **CC-BY-NC-4.0**.
- **Fun-CosyVoice3-0.5B** — Apache-2.0, Russian is good, but the "for
  academic purposes only" disclaimer contradicts the tag, and **12.25 s on
  CPU**; Python.
- **Higgs Audio v2** — weights under the Boson Community License (an MAU
  threshold, mandatory attribution), "a strong accent" in Russian.
- **VibeVoice** — MIT, but "research purpose only" and EN/ZH only.
- **Chatterbox Multilingual** — MIT + Russian is present, but Russian
  quality is weak ("stress and intonation problems") and it's PyTorch 0.5B.
- **vosk-tts** — the cleanest license in the survey (Apache-2.0 code and
  weights, ONNX), but its sound is in the piper class, i.e. the original
  complaint stands; no C/Rust bindings.
- **TTS Arena is useless for Russian** — the arena only evaluates English;
  any "top open TTS 2026" list (Step Audio EditX, Voxtral, Kokoro, Maya1) is
  irrelevant to Russian. Voxtral is also CC-BY-NC and has no Russian.

### 13.5 Recommendation and new plan

> **Superseded by the spike (2026-07-22/23) — see §13.7.** The recommendation
> below reflects the state *before* listening. Outcome: Qwen3-TTS and
> Supertonic 3 — **NO-GO** on Russian sound quality, the only one clearing
> the bar — **vosk-tts** (§13.7). Left in as a record of the thought
> process.

**Primary candidate — Qwen3-TTS via the `qwentts.cpp` sidecar.** Three
reasons:

1. Licenses are clean on both sides (Apache-2.0 weights + MIT port) —
   unlike Supertonic (OpenRAIL-M) and ESpeech (unproven cleanliness).
2. The only candidate with an objective Russian-quality edge over
   commercial engines (WER better than ElevenLabs).
3. **An architectural bonus:** `tts-server` exposes an **OpenAI-compatible
   `/v1/audio/speech`** — i.e. it's verified by the already-written (and
   reverted) `external` client, **without a single new line** in the engine
   layer. The spike = spin up the sidecar and point the existing code at it.

**Secondary — Supertonic 3**: immune to drift by construction, 17× smaller,
no GPL and no G2P, has Rust in the upstream. Against: OpenRAIL-M (not
classic permissive) and unverified Russian prosody. Also fits as a CPU
fallback next to GPU-side Qwen (graceful degradation in the project's
spirit).

**What neither one solves:** stress and homographs. RUAccent (MIT, 0.96
accuracy on homographs, COLING 2025) is **Python-only**, doesn't fit the
"binary + model" deployment. At the start — a homograph dictionary, or
accept it as-is.

**Plan — spike first, code later** (the previous pass died precisely
because quality was checked *after* implementing two stages):

1. **A spike, no edits to `src/`**: build `qwentts.cpp` with CUDA on
   Windows, bring up `tts-server`, download `Qwen3-TTS-12Hz-CustomVoice`.
   Run three pieces: a clean Russian paragraph; Russian with English terms;
   **20–30 sentences in a row in one timbre with a fixed seed** — listen for
   whether prosody drifts between chunks (the key criterion: this is what
   killed Gemini). Run the
   [Supertonic 3 demo](https://supertonic3.github.io/) the same way and
   compare.
2. **GO/NO-GO on sound quality** — only then the stage plan.
3. On GO — restore stage 1's infrastructure from the reverted commits
   (extractor, command, player, stop points, the settings tab:
   `git show 19ead9f`), the `external` mode pointed at the local
   `tts-server`; a managed sidecar with provisioning via a lock list (the
   ADR 0005 pattern) — as the next step.
4. **Revising R1**: the local engine becomes the primary mode, clouds
   become optional only (after three failures they can't be trusted as the
   primary path).

### 13.6 Sources (second wave)

- Qwen3-TTS: [QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) ·
  [HF Qwen3-TTS-12Hz-1.7B-Base](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base) ·
  [arXiv 2601.15621](https://arxiv.org/html/2601.15621v1) ·
  [discussion #185 (stress)](https://github.com/QwenLM/Qwen3-TTS/discussions/185)
- Ports: [ServeurpersoCom/qwentts.cpp](https://github.com/ServeurpersoCom/qwentts.cpp) ·
  [gabriele-mastrapasqua/qwen3-tts](https://github.com/gabriele-mastrapasqua/qwen3-tts) ·
  [TrevorS/qwen3-tts-rs](https://github.com/TrevorS/qwen3-tts-rs)
- Supertonic: [supertone-inc/supertonic](https://github.com/supertone-inc/supertonic) ·
  [HF Supertone/supertonic-3](https://huggingface.co/Supertone/supertonic-3) ·
  [arXiv 2503.23108](https://arxiv.org/abs/2503.23108) ·
  [sherpa-onnx: SupertonicTTS](https://k2-fsa.github.io/sherpa/onnx/tts/supertonic.html)
- Licensing landmine: [sherpa-onnx#3731](https://github.com/k2-fsa/sherpa-onnx/issues/3731) ·
  [espeak-ng#2131](https://github.com/espeak-ng/espeak-ng/issues/2131) ·
  [sherpa-onnx#849 (stdin/text-file)](https://github.com/k2-fsa/sherpa-onnx/issues/849)
- Russian quality: [Habr: an Open Source TTS survey (2026-02-03)](https://habr.com/ru/companies/raft/articles/991844/) ·
  [Habr: can it replace a narrator](https://habr.com/ru/companies/raft/articles/1031560/) ·
  [alphacephei: Evaluation of Russian TTS](https://alphacephei.com/nsh/2024/07/12/russian-tts.html) ·
  [Balalaika, arXiv 2507.13563](https://arxiv.org/html/2507.13563v1) ·
  [rhasspy/piper#684](https://github.com/rhasspy/piper/issues/684)
- Rejected: [F5-TTS (weights CC-BY-NC)](https://github.com/SWivid/F5-TTS) ·
  [ESpeech-TTS-1_RL-V2](https://huggingface.co/ESpeech/ESpeech-TTS-1_RL-V2) ·
  [silero-models](https://github.com/snakers4/silero-models) ·
  [Fun-CosyVoice3-0.5B](https://huggingface.co/FunAudioLLM/Fun-CosyVoice3-0.5B-2512) ·
  [llama.cpp PR #12794](https://github.com/ggml-org/llama.cpp/pull/12794)
- Stress: [RUAccent (MIT)](https://github.com/Den4ikAI/ruaccent) ·
  [COLING 2025](https://aclanthology.org/2025.coling-main.444/)
- Runtimes: [crates.io/sherpa-onnx](https://crates.io/crates/sherpa-onnx) ·
  [pykeio/ort](https://github.com/pykeio/ort) ·
  [mush42/sonata](https://github.com/mush42/sonata) ·
  [lucasjinreal/Kokoros](https://github.com/lucasjinreal/Kokoros)

> **A reliability caveat.** Licenses were checked against HF cards and
> LICENSE files in the repositories (not blogs), the key ones — personally
> rechecked. But the HF `apache-2.0` tag is metadata, not a signed document;
> for Qwen the risk is minimal (Alibaba consistently ships Qwen under
> Apache-2.0), for ESpeech — the opposite. Data on Qwen3-TTS and its ports
> covers January–July 2026; it was verified via web queries against primary
> sources, but **not run live** — hence the mandatory spike in §13.5.

### 13.7 Spike results (2026-07-22/23): a live run of three engines

The spike was run **outside the repository** (`C:\tts-spike`, not a single line
in `src/`), on real hardware (Windows 11, RTX 4090, CUDA 12.4). Three local
candidates were tested, each on the same texts: a clean Russian paragraph
(with the homographs `за́мок`/`замо́к`, `до́рог`/`дорога́` — words whose meaning <!-- cyrillic-ok -->
changes with stress placement), Russian with English loanwords
(`llama-server`, `Q4_K_M`, `bge-m3`, `503`), a long ~30-sentence text (one
call **and** separate per-sentence requests — a direct drift test, the one
that killed Gemini). Quality was judged by ear by the user; drift —
objectively, by the spread of F0/spectral centroid, with a control on
single-session synthesis.

**A key methodological result — drift is measurable.** The coefficient of
variation of the fundamental frequency (F0) on continuous synthesis vs.
splicing together separate requests for the same text with the same
voice/seed:

| engine | CV F0, single call | CV F0, per sentence | conclusion |
|---|---|---|---|
| Qwen3-TTS | 6.7% | **13.1%** (doubles) | drift is real, audible as a "Frankenstein" |
| Supertonic 3 | 6.7% | **6.1%** (doesn't grow) | no drift |
| vosk-tts | 5.9% | **6.3%** (doesn't grow) | no drift |

**Qwen3-TTS — NO-GO.** `qwentts.cpp` was built (build gotchas: CMake's VS
generator needs MSBuild-CUDA integration, which BuildTools lacks → Ninja;
long CUDA object-file names → a short path). Performance is excellent: RTF
**0.113–0.120** on GPU, first frame 49 ms; Russian is confirmed in the
GGUF's `language_names`; the OpenAI-compatible `/v1/audio/speech` works (our
`external` client would connect with no changes). But on Russian sound
quality it fails on **four** axes: (1) wrong stress even on non-homographs
(context is enough for a human, the model simply doesn't know); (2) a strong
**Chinese accent** on English loanwords (the model is Chinese, Latin script
is rendered through Mandarin phonetics); (3) per-chunk synthesis — the same
"Frankenstein" as Gemini's (confirmed both by instruments and by ear);
(4) unmotivated emotionality (a property of autoregressive LLM-TTS). The
upstream weights are Apache-2.0, the port MIT — the license would have been
ideal, but the quality is unusable.

**Supertonic 3 — NO-GO for Russian.** Native Rust + `ort`, a 37 s build,
**no Python and no espeak** (G2P isn't needed architecturally — confirmed by
the dependency list). RTF 0.155 **on CPU** (GPU wasn't needed), 44.1 kHz.
**No drift** (the best result); non-determinism is limited to fine texture:
identical input gives a file of the exact **same size, byte for byte**
(durations/tempo are set by the duration predictor) — the "reinvent
intonation per chunk" mechanism doesn't exist by construction. The user:
continuous and per-chunk synthesis sound **uniform** ("the seams are
noticeable, but the sound is uniform"), the delivery is noticeably livelier
than piper's. **But stress isn't fixed.** Verified: the stress mark U+0301
is a known model token (id 146, not a no-op), it reaches through and has an
effect (changes the synthesis, measured by file size) — however (a) the
model's own stress accuracy is very low, **almost everything** needs
marking; (b) marking everything sounds unnatural (the "Suno effect" —
familiar to the user); (c) some words **ignore** the mark (`сто́ит` reads <!-- cyrillic-ok -->
wrong in every position). Bottom line: there's no controllable path to
correct stress. The weights' license is **OpenRAIL-M** (not classic
permissive), which would also be a step down. A separate **portable
finding**: transliterating English loanwords into Cyrillic (e.g.
`llama-server` → a Cyrillic phonetic spelling) **removes the accent
entirely** on any engine, and — without full-text stress marking — the
robotic-sounding delivery too.

**vosk-tts — GO (with caveats).** `vosk-model-tts-ru-0.9-multi` (746 MB),
ONNX via `onnxruntime`, **no torch at runtime**. The only candidate that
handles **both** problems that killed the previous attempts:

- **resolves stress itself, from context** — BERT embeddings in the
  pipeline (`g2p_multistream`), so the texts were fed **with no stress
  marks at all**. It only misses on genuine homographs (`за́мок на двери`, <!-- cyrillic-ok -->
  `в слове до́рог`, `сто́ит` — it keeps the default stress regardless of <!-- cyrillic-ok -->
  context). This is a minority and is fixable with a targeted dictionary —
  a big improvement over Qwen/Supertonic, where errors were pervasive;
- **robotic-ness is tunable** — the decisive test. The default is
  mechanical, but the parameter `duration_noise_level` (duration variation)
  + slowing the tempo noticeably livens it up. The user accepted the
  `slow-exp` config (`noise_level=1.0`, `duration_noise_level=1.3`,
  `speech_rate=0.9`) as "the best to taste"; on the full-length content
  (1-ru, the long text) it sounds **acceptable**;
- **no drift** (CV F0 5.9%/6.3%);
- **the cleanest license** — **Apache-2.0 for both code and weights** (the
  only one of the three like that).

vosk's caveats: **22050 Hz** (duller than Supertonic's 44.1 kHz); RTF
0.26–0.75 on CPU (slower, but good enough for a pipeline); **Latin script
isn't supported at all** (`KeyError: 'a'`) — transliteration is mandatory,
not optional; the frontend chokes on guillemets `«»`/dashes and on
pre-composed `ё` — needs a sanitizer + NFD normalization; transliterated
English loanwords sound "not great, but better than the previous two
engines."

**Field summary (all six engines tested):**

| engine | stress | naturalness | sound | drift | weights license | verdict |
|---|---|---|---|---|---|---|
| OpenAI gpt-4o-mini-tts | — | monotone | — | — | cloud | ✗ (reverted) |
| Gemini TTS | — | good | good | **Frankenstein** + `503` | cloud | ✗ (reverted) |
| piper | poor | **flat** | 22 kHz | — | mixed | ✗ (reverted) |
| Qwen3-TTS | poor | emotional drift | 24 kHz + Chinese accent | **CV doubles** | Apache-2.0 ✅ | ✗ NO-GO |
| Supertonic 3 | **very poor** | **good** | 44 kHz ✅ | no ✅ | OpenRAIL-M ⚠️ | ✗ NO-GO (ru) |
| **vosk-tts** | **good** (misses on homographs) | mechanical→**tunable** | 22 kHz | no ✅ | **Apache-2.0** ✅ | **✓ GO** |

### 13.8 Strategic decision point and high-level plan

> **The decision point is closed (2026-07-23) in favor of OpenAI TTS** — see
> §13.9. The text below is left as the statement of the decision point and
> the high-level **vosk** plan **for future groundwork** (offline/non-OpenAI
> audience).

Following the spike, **the main decision point shifted** — from "which local
engine" to "local **or** cloud/external at all":

- **Local (vosk-tts).** Pros: free, private, offline, Apache-2.0 license, no
  drift, the engine resolves stress. Cons: **high integration complexity**
  (see the plan below) — a "no Python" delivery needs a custom Russian
  frontend; 22 kHz; homographs still get missed. The user noted directly:
  "too much complexity."
- **Cloud / paid external.** The user's reasoning: cloud LLMs are already
  paid for, and TTS is hardly more expensive (§3's data confirms this:
  OpenAI ≈ $0.015/min, Gemini flash has a free tier — against Opus token
  costs it's pocket change). **But:** "cloud" reopens the quality question
  that killed the first pass — Gemini produced a Frankenstein, OpenAI was
  monotone. So "going to the cloud" ≠ an automatic solution: a cloud engine
  is needed that **both sounds good and doesn't drift**. That's a separate
  short evaluation (an analogue of this spike, but for clouds — the
  ElevenLabs class, Azure, OpenAI's newer voices) that should be done
  **before** choosing, not after. `external` mode already covers any
  OpenAI-compatible server (including premium clouds via a proxy, §3.4).

**The decision point isn't closed — the user decides.** Below is a
high-level plan **for the case vosk is chosen** (if the cloud wins, a much
thinner branch applies: restore the reverted `external`/`OpenAi`/`Gemini`
clients from `git show 19ead9f` + pick a provider from the short quality
evaluation).

**High-level plan for integrating vosk-tts** (details — when the track
starts):

1. **Infrastructure from the reverted commits** (`git show 19ead9f`): the
   speech extractor over `pulldown-cmark` (§5), the `/tts`/`N`/`all`/`stop`
   command (§7), the `rodio` player (§6), stop points, the "Speech" tab
   (§9). The failure didn't touch them — no need to rewrite, only restore.
2. **A Russian frontend in Rust** (the main and most expensive part,
   roughly a separate subsystem): BERT-based stress marking (`tokenizers` —
   a native Rust crate + an ONNX BERT via `ort`), g2p, a punctuation
   sanitizer (`«»`/dashes/`ё`→NFD), **mandatory transliteration of Latin
   script into Cyrillic** (a rule table; a §13.7 finding — works on any
   engine). Risk — reproducing their `g2p_multistream` exactly; reuse the
   RUAccent (MIT) stress dictionary as **data**.
3. **Acoustics via `ort`**: vosk's 4–5 ONNX graphs + BERT embeddings as
   input; default synthesis parameters — `slow-exp`
   (`duration_noise_level=1.3`, `speech_rate=0.9`); sample rate 22050 Hz.
4. **`mindfork tts setup` provisioning** via an sha256 lock list (the
   ADR 0005 pattern): the vosk model (746 MB) + BERT + dictionaries into
   `data/tts/`.
5. **A runtime decision point is deferred** (the user's decision: "not
   deciding yet"): an in-process frontend port (cleanest, but a
   g2p-accuracy risk) **vs.** a Python sidecar in wasmer/WASIX (zero g2p
   risk — their own code, but `onnxruntime` under WASIX is a big question
   mark). Investigate with a separate research spike before implementation.
6. **Homographs** stay unresolved in specific spots (`сто́ит` etc.): either <!-- cyrillic-ok -->
   live with it, or a small dictionary of contextual rules on top of the
   frontend — future work.

**Open groundwork regardless of the choice:** portable transliteration of
English loanwords (reduces the accent on **any** local engine — future work
for the local branch); the short quality evaluation of premium clouds
(ElevenLabs/Azure/OpenAI's newer voices) on the same texts — **done,
§13.9**.

### 13.9 Cloud evaluation (2026-07-23) — outcome: OpenAI GO

A run of cloud TTS on the same four texts as the local spike (outside the
repository, keys from env, not from chat). The user asked for
ElevenLabs / Azure / OpenAI; **only OpenAI and Gemini keys were available**
(ElevenLabs/Azure — none). Scripts for ElevenLabs (`eleven_multilingual_v2`)
and Azure (`ru-RU-DmitryNeural`, SSML) were written and ready to run
(`C:\tts-spike\cloud-*.py`), but **weren't needed** — see the decision
below.

**OpenAI `gpt-4o-mini-tts` — GO.** Voice `onyx` + the instruction "natural
clear Russian, calm male voice." The user's verdict:

- **stress** — 1 error for the whole paragraph (a genuine homograph,
  resolvable only by world knowledge). This is **better than every local
  engine**, including vosk;
- **English loanwords** — "perfect": Latin script is read **natively**, no
  transliteration needed (removes a whole layer of work that's mandatory
  for the local path);
- **drift** — none: CV F0 **2.7%** with a single request / 3.2% per
  sentence (the best result among everything tested, local and cloud).
  Median F0 94 Hz — confidently male (the revert's complaint "the male
  voice read as female" didn't reproduce).

**Why OpenAI failed at the revert but not now.** The first pass gave
"monotone female" on the selected male voice — that was a **configuration
error** (voice/model/missing `instructions`), not an OpenAI ceiling. With
`gpt-4o-mini-tts` + `onyx` + a Russian instruction the result is excellent.
Lesson: the engine was rejected for a symptom of misconfiguration.

**Diagnosis of "grown pauses" (the user's only complaint about chunked
mode).** The same long text: **129.9 s** in a single request, **154.6 s**
per sentence. The 25 s difference is accumulated pauses from **chunking**
(30 independent syntheses give a "stop-start" rhythm), not an OpenAI
property. Trimming trailing silence **doesn't help** (checked — the issue
isn't silence but per-sentence final intonation). **The right fix is not to
chunk:** the request limit is **4096 characters**, a typical message (the
1720-character test) goes out **as a single request** → continuous
prosody, no pause artifact. Chunking (by paragraph, not by sentence) — only
for rare messages > 4096 characters. This distinguishes the cloud
integration from the local one, where a per-sentence pipeline was needed
for a fast first sound.

**ElevenLabs / Azure — not tested, rejected on a product argument** (the
user's decision): "users of the app won't want to sign up for a
third-party service just for speech." OpenAI has no such drawback — a
cloud-chat user already has a key (ADR 0008). ElevenLabs (probably the best
sound, but ~$0.06–0.30/min and its own signup) and Azure (strong Russian,
needs a key + region) remain **future work** if premium quality is ever
needed; `external` mode covers them (including via a proxy, §3.4).

**Cloud-branch summary:**

| engine | stress | English loanwords | drift | new vendor for the user | verdict |
|---|---|---|---|---|---|
| **OpenAI gpt-4o-mini-tts** | **1 error/paragraph** | **native** ✅ | no (CV 2.7%) | **no** (key already there) | **✓ GO — primary** |
| Gemini TTS | — | — | Frankenstein (§13.1) | no (key already there) | optional (in the stage's scope) |
| ElevenLabs | — | — | — | yes | future work |
| Azure Neural | — | — | — | yes | future work |

**Final decision (2026-07-23).** The primary mode — **OpenAI TTS**. Stage
scope — **OpenAI + Gemini + external** (Gemini included as an option for
those who have its key, despite all its downsides; external — one's own
server/a premium cloud via a proxy). **A local sidecar isn't in the stage**
(vosk — groundwork, §13.8). The R1 revision (§13.5) is cancelled: clouds
are not "optional" but the **primary path**; local — groundwork. The
implementation rests on the reverted infrastructure (`git show 19ead9f`):
the extractor, the `/tts` command, the `rodio` player, the
OpenAi/Gemini/External clients are already written — restore and finish it
as one-shot for messages < 4096 characters.
