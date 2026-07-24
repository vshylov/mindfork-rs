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

Forks Р1–Р9 (§10) were accepted by the user on 2026-07-21 — they remain valid on
**behavior** (the command, Markdown filtering, stop points, UI placement); only the
**engine choice** (Р2) and, as a consequence, mode priority (Р1) need revisiting.
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
option). So **sub-option (в)** was taken — the MIT `piper` binary: text goes
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

Proposed rules (Р4):

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
(Р9): on Windows, WinRT voices are legacy quality ("Microsoft Irina", natural
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
  Р6) — it's a settings toggle "Speak roles," not behavior hardwired to `N > 1`.
- A new `/tts` command **interrupts** the current playback and starts a new one;
  `/tts stop` stops it manually.
- **Automatic stops** (Р8, user's decision):
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

    // behavior (Р6/Р8, user's decisions)
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

## 9. Настройки в UI

Решение (Р7): **отдельная вкладка «Озвучивание»** — четвёртая в таб-стрипе секции
«Модель» (Ассистент · Имперсонация · Эмбеддинги · Озвучивание): TTS это ещё один
«серверный слот» с теми же mode-driven полями. Состав:

- группа **«Движок»** — селектор режима, модель, голос, скорость; у external — URL;
  ключ — общее поле-статус провайдера (ADR 0008, вводить заново не нужно);
- группа **«Поведение»** — тумблеры «Озвучивать роли» (Р6), «Прерывать при
  переключении чата» (вкл), «Прерывать при начале генерации» (выкл) (Р8).

Отвергнутая альтернатива — группа в «Интерфейсе»: поля движковые, а не
оформительские, и вкладка даёт им mode-driven visibility «бесплатно».

## 10. Решения (приняты пользователем 2026-07-21)

Р4 и Р7 подтверждены явно; Р5, Р6 и Р8 приняты **с поправками пользователя**
(отражены в §7–§9); остальные — по рекомендациям. **Р1 и Р2 требуют пересмотра
после реверта — см. §13.**

- **Р1. Объём MVP (этап 1)** — *по рекомендации*: команда `/tts` + экстрактор +
  плеер + режимы **OpenAi / Gemini / External**. Локальный сайдкар (managed) —
  этапом 2. **Пересмотр (§13.5): порядок меняется — локальный движок становится
  первичным, облака отходят на роль опциональных.**
- **Р2. Локальный движок сайдкара** — *было*: **sherpa-onnx** (Apache-2.0) +
  piper-голоса `ru_RU-dmitri` (CC0). Подварианты: (а) CLI-сайдкар
  `sherpa-onnx-offline-tts`; (б) официальный Rust-крейт in-process; (в) piper
  MIT-бинарь 2023. Итог спайка: (а) непригоден для кириллицы на Windows
  (ANSI-argv, §4) → взят (в). **Пересмотр (§13.2): (в) провалился по качеству;
  новые кандидаты — Qwen3-TTS через `qwentts.cpp` и Supertonic 3.**
- **Р3. Воспроизведение** — *по рекомендации*: `rodio` в процессе (§6). Принятая
  цена: системная C-зависимость ALSA на Linux + MPL-2.0 (Symphonia) в `deny.toml`.
  **В силе.**
- **Р4. Фильтрация Markdown** — *подтверждено*: таблица §5. **В силе.**
- **Р5. Синтаксис команды** — *поправка пользователя*: `/tts`, `/tts N`,
  `/tts all`, `/tts stop` (§7). **В силе.**
- **Р6. Роли** — *поправка пользователя*: тумблер «Озвучивать роли», действует во
  всех трёх вариантах команды. **В силе.**
- **Р7. Настройки в UI** — *подтверждено*: вкладка «Озвучивание» в секции
  «Модель» (§9). **В силе.**
- **Р8. Прерывание воспроизведения** — *поправка пользователя*: две независимые
  настройки + безусловные остановки. **В силе.**
- **Р9. Режим «ОС-TTS без моделей»** — *по рекомендации*: **не в объёме**.
  **В силе.**

## 11. План этапов (история)

1. **Этап 1 `feat/tts-core`** — был сделан, **откатан** (`839df19`): конфиг +
   вкладка «Озвучивание», команда `/tts`, речевой экстрактор с пометками, плеер
   rodio, клиенты OpenAi/Gemini/External, точки остановки, чип статуса, i18n,
   CI/packaging-правки (ALSA, deny.toml), смоуки.
2. **Этап 2 `feat/tts-sidecar`** — был сделан, **откатан** (`6fd8529`):
   managed-режим на бинаре `piper` + `mindfork tts setup` + голоса ru/en.
3. **ADR 0009** — был написан, **удалён** вместе с реализацией.
4. **Новый план** — §13.5.

Инфраструктурная часть (экстрактор, команда, плеер, точки остановки, UI-вкладка)
провал не затронул — она отработала как задумано; переписывать её при повторном
заходе не нужно, можно поднять из отревертнутых коммитов (`git show 19ead9f`).

## 12. Ключевые источники (первая волна, 07.2026)

- OpenAI: справка `createSpeech` и гайд Text-to-speech (developers.openai.com) —
  параметры/лимиты/цены/`stream_format`; страница модели `gpt-4o-mini-tts`.
- Gemini: «Speech generation» (ai.google.dev, актуальная и legacy-страницы) —
  формы `generateContent`, голоса, языки, цены, стриминг с 3.1.
- Anthropic: обзор API (platform.claude.com/docs/en/api/overview) — поверхность без
  speech/audio.
- Локальные серверы: репозитории Kokoro-FastAPI, speaches (+HF-реестр piper-голосов
  ru_RU), LocalAI, AllTalk v2 (wiki OpenAI-эндпоинта), chatterbox-tts-api,
  openedai-speech (архив).
- Локальные движки: sherpa-onnx (релизы v1.13.4 + model zoo + официальный крейт
  docs.rs/sherpa-onnx); piper (rhasspy/piper — архив, релиз 2023.11.14-2;
  OHF-Voice/piper1-gpl v1.5.0 — GPL, pip-only); HF `rhasspy/piper-voices`
  (MODEL_CARD голосов ru_RU — лицензии данных: denis/dmitri CC0, irina Unknown,
  ruslan CC BY-NC-SA); silero-models (LICENSE CC BY-NC-SA; MIT только v5_cis_base);
  Supertonic (supertone-inc/supertonic + HF Supertone/supertonic-3, OpenRAIL-M);
  llama.cpp `tools/tts` + Draft-PR #12794 (OuteTTS-1.0); HF OuteAI/OuteTTS-1.0-0.6B.
- Воспроизведение: rodio 0.22 (docs.rs/CHANGELOG/UPGRADE), cpal (README/CHANGELOG —
  Send-стримы 0.17, windows-rs на WASAPI, ALSA-требования), Symphonia (MPL-2.0),
  cpal#384 (шум ALSA в stderr); tts-rs, NaturalVoiceSAPIAdapter, msedge-tts /
  edge-tts#290/#458 (история блокировок).

---

## 13. Ревизия после реверта (разведка 2026-07-22)

Вторая волна исследования — три параллельных веб-обзора (ландшафт локальных
движков; качество русского; рантаймы для Rust) + личная перепроверка ключевых
лицензий по карточкам HF и репозиториям. Ограничения, заданные пользователем на
входе: **GPU допустим** (RTX 4090), поставка — **только бинарь + модель, без
Python-окружения**, **русский и английский одинаково важны**.

### 13.1 Диагноз: почему провалились все три движка

Это важнее списка кандидатов — оно объясняет, что именно искать.

- **Gemini — «Франкенштейн».** Причина **не в Markdown** (экстрактор §5 отработал)
  и не в нарезке. Gemini TTS — **авторегрессионный LLM**: он порождает речь
  токенами и на каждом чанке заново «выбирает» тембр и интонацию. Дрейф между
  чанками — структурное свойство класса, а не дефект интеграции. Плюс `503`
  на preview-моделях (GA-версии TTS у Gemini нет).
- **piper — «читает обычный человек».** Корень — **акустическая модель и данные**,
  а не отсутствие ударений:
  - работа Balalaika (arXiv 2507.13563) обучила VITS на лучшем русском датасете:
    общий MOS **3.618**, но **Intonation MOS 2.532** — плоская интонация присуща
    классу; добавление пунктуации и ударений улучшило метрики, но «gaps were
    modest»;
  - оценка Н. Шмырева (alphacephei, 14 движков): «you can deal with plain
    intonation but artifacts are really annoying» — плоскость признана свойством;
  - голос `irina` — файнтюн с английского `lessac` примерно на **1 часе** данных
    RHVoice;
  - маркер ударения в piper для русского **сломан**
    ([rhasspy/piper#684](https://github.com/rhasspy/piper/issues/684), репозиторий
    заархивирован 06.10.2025).

  **Следствие:** прикручивание расстановщика ударений (RUAccent) к piper убрало бы
  ошибки в омографах, но **не** сделало бы речь дикторской. Этап был бы потрачен зря.
- **OpenAI** — известный дефект `gpt-4o-mini-tts`; со стороны клиента не лечится.

**Критерий выбора, вытекающий отсюда:** движок должен быть (а) обучен на живой
речи, а не на начитке, и при этом (б) **детерминирован** либо жёстко привязан к
фиксированному тембру — иначе выразительность покупается ценой возврата «дрейфа».

### 13.2 Два кандидата (лицензии проверены лично по первоисточникам)

| | **Qwen3-TTS** | **Supertonic 3** |
|---|---|---|
| Код | Apache-2.0 ([QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)) | MIT («This project's sample code is released under the MIT License») |
| **Веса** | **Apache-2.0** ✅ (карточка [Qwen3-TTS-12Hz-1.7B-Base](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base), релиз 22.01.2026) | **OpenRAIL-M** ⚠️ — коммерция и редистрибуция разрешены, но с use-restrictions, которые надо пробрасывать конечному пользователю |
| Русский | ✅ 1 из 10 языков; **WER 3.212 против 3.878 у ElevenLabs** и 4.281 у MiniMax (техотчёт, Table 6) | ✅ 1 из 31 языка; WER 3.99 (Minimax-MLS-test) |
| Класс | LLM-TTS, 1.7B (есть 0.6B) | не-LLM, **~99M**, чистый ONNX |
| Рантайм без Python | ✅ [qwentts.cpp](https://github.com/ServeurpersoCom/qwentts.cpp) — **MIT**, GGML, CPU/CUDA/Vulkan/Metal, `buildcuda.cmd`/`buildvulkan.cmd`, **`tts-server` с `/v1/audio/speech`**; также [чистый C](https://github.com/gabriele-mastrapasqua/qwen3-tts) и [Rust на candle](https://github.com/TrevorS/qwen3-tts-rs) (оба MIT) | ✅ нативный Rust в апстриме (`rust/`, на `ort` 2.0-rc), 4 ONNX-файла + `voice.bin`; либо крейт `sherpa-onnx` (с оговоркой §13.3) |
| Скорость | RTF 0.35 на RTX 5050; 0.5–0.7 на CPU для 0.6B (int4/int8, M1) | RTF ≈0.3 **на электронной книге**; ~6.1× realtime против 2.0× у Kokoro-82M |
| **G2P / espeak-ng** | не нужен | **не нужен** — работает с сырым символьным текстом (arXiv 2503.23108), G2P-модуль отсутствует архитектурно |
| Стабильность голоса | 9 фиксированных тембров CustomVoice **без reference-аудио** (дрейфа тембра от zero-shot нет), `--seed`/низкая `temperature`; но LLM → **просодия между вызовами может слегка гулять** | **детерминирован по построению** — иммунен к классу отказа Gemini |
| Ударения/омографы | ❌ обучен без разметки ударений ([discussion #185](https://github.com/QwenLM/Qwen3-TTS/discussions/185), ответа мейнтейнеров нет) | ❌ явного фронтенда ударений тоже нет |

**Оговорка по метрике WER:** отличный WER и проблема омографов не противоречат друг
другу — WER считается через ASR-транскрипцию, а распознавание к ударению почти
нечувствительно («за́мок» и «замо́к» дают одну строку). Цифра эту проблему **не
видит**.

**Оговорка по тембрам Qwen:** 9 пресетов CustomVoice — это zh/en/ja/ko
(ryan, aiden, vivian, serena, uncle_fu, dylan, eric, ono_anna, sohee). Русского
пресета нет; читать по-русски они по идее должны (тембр и язык развязаны), но это
**подлежит проверке ушами**, а не вере на слово. Русско-английский код-свитчинг
официально измерен только на паре zh-en.

### 13.3 Лицензионная мина: espeak-ng внутри sherpa-onnx

Важный для MIT-проекта результат, вскрытый второй волной и **не учтённый** в §4:

- `CMakeLists.txt` sherpa-onnx: при `SHERPA_ONNX_ENABLE_TTS` (**ON по умолчанию**)
  подключается `espeak-ng-for-piper`; в статических сборках присутствует
  `libespeak-ng.a`. Prebuilt-архивы, которые качает build-script Rust-крейта,
  почти наверняка его содержат.
- espeak-ng — **GPL-3.0**; просьба о смене на LGPL
  ([espeak-ng#2131](https://github.com/espeak-ng/espeak-ng/issues/2131)) закрыта
  как «not planned».
- Сам k2-fsa это признал: [sherpa-onnx#3731](https://github.com/k2-fsa/sherpa-onnx/issues/3731)
  (08.07.2026) — «incompatible with the Apache-2.0 license of sherpa-onnx»,
  удаление запланировано в **2.0.0** (breaking).

Последствия по способам использования: статическая линковка в наш `.exe` → бинарь
как единое произведение попадает под GPL-3.0; сайдкар-процесс → «на расстоянии
вытянутой руки», претензий нет (**именно поэтому прежний piper-сайдкар был чист**).
**Оба кандидата §13.2 espeak не требуют вовсе — вопрос не возникает.**

Отдельно: пермиссивного русского G2P в 2026 фактически не существует
(`piper-plus-g2p` — MIT, но без русского; gruut/OpenPhonemizer — Python либо только
английский). Единственный чистый выход — модель, которой G2P не нужен.

### 13.4 Что перепроверено и осталось отрицательным

- **llama.cpp / GGUF** — путь по-прежнему мёртв: PR #12794 (OuteTTS 1.0) в статусе
  **draft**, последний содержательный комментарий 19.05.2025; `/v1/audio/speech` в
  `llama-server` нет (то, что добавили в #13714 — audio **input**, ASR-сторона).
- **ESpeech-TTS-1** (лучший русский по отзывам Habr, обучен на подкастах →
  «дикторская» подача, встроен RUAccent) — **дисквалифицирован дважды**: только
  PyTorch (нарушает «без Python») и **недоказанное происхождение весов** — построен
  на F5-TTS, у которого предобученные веса **CC-BY-NC** («due to the training data
  Emilia»); заявленный Apache-2.0 может быть юридически несостоятелен, публичного
  заявления «trained from scratch» нет.
- **Silero** — MIT только у `v5_cis_base`/`_nostress` (без авто-ударений); все
  удобные `v5_*_ru` и именные голоса — **CC-BY-NC**. Плюс `.pt/.jit` → PyTorch.
- **F5-TTS_RUSSIAN** (Misha24-10) — качество отличное, веса **CC-BY-NC-4.0**.
- **Fun-CosyVoice3-0.5B** — Apache-2.0, русский хорош, но дисклеймер «for academic
  purposes only» противоречит тегу, и **12.25 с на CPU**; Python.
- **Higgs Audio v2** — веса под Boson Community License (порог MAU, обязательная
  атрибуция), на русском «сильный акцент».
- **VibeVoice** — MIT, но «research purpose only» и только EN/ZH.
- **Chatterbox Multilingual** — MIT + русский есть, но качество русского слабое
  («проблемы с ударениями и интонацией») и это PyTorch 0.5B.
- **vosk-tts** — самая чистая лицензия обзора (Apache-2.0 код и веса, ONNX), но по
  звучанию класс piper, то есть исходная претензия сохраняется; C/Rust-биндингов нет.
- **TTS Arena для русского бесполезна** — арена оценивает только английский; любые
  «топ open TTS 2026» (Step Audio EditX, Voxtral, Kokoro, Maya1) к русскому
  отношения не имеют. Voxtral вдобавок CC-BY-NC и без русского.

### 13.5 Рекомендация и новый план

> **Устарело спайком (2026-07-22/23) — см. §13.7.** Рекомендация ниже отражает
> состояние *до* прослушивания. Итог: Qwen3-TTS и Supertonic 3 — **NO-GO** по
> звучанию русского, единственный прошедший планку — **vosk-tts** (§13.7). Оставлено
> как история хода мысли.

**Первичный кандидат — Qwen3-TTS через сайдкар `qwentts.cpp`.** Три причины:

1. Лицензии чисты с обеих сторон (Apache-2.0 веса + MIT порт) — в отличие от
   Supertonic (OpenRAIL-M) и ESpeech (недоказанная чистота).
2. Единственный кандидат с объективным преимуществом по русскому над коммерческими
   движками (WER лучше ElevenLabs).
3. **Архитектурный бонус:** `tts-server` отдаёт **OpenAI-совместимый
   `/v1/audio/speech`** — то есть проверяется уже написанным (и отревертнутым)
   `external`-клиентом, **без единой новой строки** в движковом слое. Спайк = поднять
   сайдкар и ткнуть в него существующим кодом.

**Вторичный — Supertonic 3**: иммунен к дрейфу по построению, в 17 раз меньше, без
GPL и без G2P, есть Rust в апстриме. Против: OpenRAIL-M (не классический
permissive) и непроверенная просодия русского. Годится и как CPU-фолбэк рядом с
GPU-Qwen (мягкая деградация в духе проекта).

**Что не решает ни один:** ударения и омографы. RUAccent (MIT, точность 0.96 на
омографах, COLING 2025) — **только Python**, в посадку «бинарь + модель» не лезет.
На старте — словарь омографов либо принять как есть.

**План — сначала спайк, потом код** (прошлый заход умер именно потому, что качество
проверяли *после* реализации двух этапов):

1. **Спайк, без правок в `src/`**: собрать `qwentts.cpp` с CUDA под Windows,
   поднять `tts-server`, скачать `Qwen3-TTS-12Hz-CustomVoice`. Прогнать три куска:
   чистый русский абзац; русский с английскими терминами; **20–30 предложений
   подряд одним тембром с фиксированным seed** — послушать, гуляет ли просодия
   между чанками (главный критерий: на нём умер Gemini). Тем же способом прогнать
   [демо Supertonic 3](https://supertonic3.github.io/) и сравнить.
2. **GO/NO-GO по звучанию** — и только потом план этапа.
3. При GO — поднять инфраструктуру этапа 1 из отревертнутых коммитов (экстрактор,
   команда, плеер, точки остановки, вкладка настроек: `git show 19ead9f`), режим
   `external` на локальный `tts-server`; managed-сайдкар с провизией по lock-списку
   (паттерн ADR 0005) — следующим шагом.
4. **Пересмотр Р1**: локальный движок становится первичным режимом, облака — только
   опциональными (после трёх провалов доверять им как основному пути нельзя).

### 13.6 Источники (вторая волна)

- Qwen3-TTS: [QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) ·
  [HF Qwen3-TTS-12Hz-1.7B-Base](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-Base) ·
  [arXiv 2601.15621](https://arxiv.org/html/2601.15621v1) ·
  [discussion #185 (ударения)](https://github.com/QwenLM/Qwen3-TTS/discussions/185)
- Порты: [ServeurpersoCom/qwentts.cpp](https://github.com/ServeurpersoCom/qwentts.cpp) ·
  [gabriele-mastrapasqua/qwen3-tts](https://github.com/gabriele-mastrapasqua/qwen3-tts) ·
  [TrevorS/qwen3-tts-rs](https://github.com/TrevorS/qwen3-tts-rs)
- Supertonic: [supertone-inc/supertonic](https://github.com/supertone-inc/supertonic) ·
  [HF Supertone/supertonic-3](https://huggingface.co/Supertone/supertonic-3) ·
  [arXiv 2503.23108](https://arxiv.org/abs/2503.23108) ·
  [sherpa-onnx: SupertonicTTS](https://k2-fsa.github.io/sherpa/onnx/tts/supertonic.html)
- Лицензионная мина: [sherpa-onnx#3731](https://github.com/k2-fsa/sherpa-onnx/issues/3731) ·
  [espeak-ng#2131](https://github.com/espeak-ng/espeak-ng/issues/2131) ·
  [sherpa-onnx#849 (stdin/text-file)](https://github.com/k2-fsa/sherpa-onnx/issues/849)
- Качество русского: [Habr: обзор Open Source TTS (03.02.2026)](https://habr.com/ru/companies/raft/articles/991844/) ·
  [Habr: можно ли заменить диктора](https://habr.com/ru/companies/raft/articles/1031560/) ·
  [alphacephei: Evaluation of Russian TTS](https://alphacephei.com/nsh/2024/07/12/russian-tts.html) ·
  [Balalaika, arXiv 2507.13563](https://arxiv.org/html/2507.13563v1) ·
  [rhasspy/piper#684](https://github.com/rhasspy/piper/issues/684)
- Отклонённые: [F5-TTS (веса CC-BY-NC)](https://github.com/SWivid/F5-TTS) ·
  [ESpeech-TTS-1_RL-V2](https://huggingface.co/ESpeech/ESpeech-TTS-1_RL-V2) ·
  [silero-models](https://github.com/snakers4/silero-models) ·
  [Fun-CosyVoice3-0.5B](https://huggingface.co/FunAudioLLM/Fun-CosyVoice3-0.5B-2512) ·
  [llama.cpp PR #12794](https://github.com/ggml-org/llama.cpp/pull/12794)
- Ударения: [RUAccent (MIT)](https://github.com/Den4ikAI/ruaccent) ·
  [COLING 2025](https://aclanthology.org/2025.coling-main.444/)
- Рантаймы: [crates.io/sherpa-onnx](https://crates.io/crates/sherpa-onnx) ·
  [pykeio/ort](https://github.com/pykeio/ort) ·
  [mush42/sonata](https://github.com/mush42/sonata) ·
  [lucasjinreal/Kokoros](https://github.com/lucasjinreal/Kokoros)

> **Оговорка о достоверности.** Лицензии проверены по карточкам HF и файлам
> LICENSE в репозиториях (не по блогам), ключевые — перепроверены лично. Но
> HF-тег `apache-2.0` — это метаданные, а не подписанный документ; для Qwen риск
> минимален (Alibaba последовательно выпускает Qwen под Apache-2.0), для ESpeech —
> наоборот. Данные о Qwen3-TTS и его портах относятся к январю–июлю 2026; они
> проверены веб-запросами к первоисточникам, но **живьём не запускались** —
> отсюда обязательный спайк §13.5.

### 13.7 Итоги спайка (2026-07-22/23): живой прогон трёх движков

Спайк проведён **вне репозитория** (`C:\tts-spike`, ни строки в `src/`), на реальном
железе (Windows 11, RTX 4090, CUDA 12.4). Проверены три локальных кандидата, каждый —
на одних и тех же текстах: чистый русский абзац (с омографами `за́мок/замо́к`,
`до́рог/дорога́`), русский с англицизмами (`llama-server`, `Q4_K_M`, `bge-m3`, `503`),
длинный текст ~30 предложений (одним вызовом **и** отдельными запросами по
предложению — прямой тест дрейфа, на котором умер Gemini). Качество оценивал
пользователь на слух; дрейф — объективно по разбросу F0/спектрального центроида с
контролем на односессионном синтезе.

**Ключевой методический результат — дрейф измерим.** CV основной частоты (F0) на
непрерывном синтезе против склейки отдельных запросов того же текста тем же
голосом/seed:

| движок | CV F0 одним вызовом | CV F0 по предложению | вывод |
|---|---|---|---|
| Qwen3-TTS | 6.7% | **13.1%** (удвоение) | дрейф реален, слышен как «франкенштейн» |
| Supertonic 3 | 6.7% | **6.1%** (не растёт) | дрейфа нет |
| vosk-tts | 5.9% | **6.3%** (не растёт) | дрейфа нет |

**Qwen3-TTS — NO-GO.** Собран `qwentts.cpp` (грабли сборки: VS-генератор CMake требует
MSBuild-CUDA-интеграции, которой в BuildTools нет → Ninja; длинные имена
CUDA-объектников → короткий путь). Производительность отличная: RTF **0.113–0.120** на
GPU, первый фрейм 49 мс; русский подтверждён в `language_names` GGUF;
OpenAI-совместимый `/v1/audio/speech` работает (наш `external`-клиент подключился бы
без правок). Но по звучанию русского провал по **четырём** осям: (1) неверные ударения
даже в неомографах (`подстрОке`, `дорОгая` — контекста достаточно, модель просто не
знает); (2) сильный **китайский акцент** на англицизмах (модель китайская, латиница
через мандаринскую фонетику); (3) почанковый синтез — тот же «франкенштейн», что у
Gemini (подтверждено и приборами, и слухом); (4) немотивированная эмоциональность
(свойство авторегрессионного LLM-TTS). Апстрим-веса Apache-2.0, порт MIT — лицензия
была бы идеальна, но качество непригодно.

**Supertonic 3 — NO-GO для русского.** Нативный Rust + `ort`, сборка 37 с, **без Python
и без espeak** (G2P не нужен архитектурно — подтверждено составом зависимостей). RTF
0.155 **на CPU** (GPU не понадобился), 44.1 кГц. **Дрейфа нет** (лучший показатель),
недетерминизм ограничен тонкой текстурой: одинаковый вход даёт файл **того же размера
до байта** (длительности/темп детерминированы duration-предиктором) — механизма
«переизобрести интонацию на чанке» нет по построению. Пользователь: непрерывный и
почанковый синтез звучат **однородно** («склейки ощущаются, но звучание однородное»),
подача заметно живее piper. **Но ударения не чинятся.** Проверено: метка ударения
U+0301 — известный токен модели (id 146, не заглушка), доходит и влияет (изменяет
синтез, замерено по размеру файла) — однако (а) собственная точность модели по
ударениям очень низкая, размечать нужно **почти всё**; (б) сплошная разметка звучит
неестественно («эффект Suno» — знакомый пользователю); (в) часть слов метку
**игнорирует** (`сто́ит` во всех позициях читается неверно). Итог: управляемого пути к
верным ударениям нет. Лицензия весов — **OpenRAIL-M** (не классический permissive),
что тоже было бы шагом вниз. Отдельная **переносимая находка**: транслитерация
англицизмов кириллицей (`llama-server`→«ла́ма-се́рвер`, `Q4_K_M`→«ку четы́ре ка эм»)
**убирает акцент начисто** на любом движке, а без сплошных меток — и механичность.

**vosk-tts — GO (с оговорками).** `vosk-model-tts-ru-0.9-multi` (746 МБ), ONNX через
`onnxruntime`, **без torch в рантайме**. Единственный кандидат, берущий **обе**
проблемы, убившие прошлые попытки:

- **ударения решает сам, по контексту** — в пайплайне BERT-эмбеддинги
  (`g2p_multistream`), поэтому тексты подавались **без единой метки**. Промах только на
  настоящих омографах (`за́мок на двери`, `в слове до́рог`, `сто́ит` — держит дефолт
  «стО́ит» независимо от контекста). Это меньшинство и лечится точечным словарём —
  большой прогресс против Qwen/Supertonic, где ошибки были повсеместны;
- **роботичность настраиваема** — решающий тест. Дефолт механичен, но параметр
  `duration_noise_level` (вариация длительностей) + замедление темпа заметно оживляют.
  Пользователь принял конфиг `slow-exp` (`noise_level=1.0`, `duration_noise_level=1.3`,
  `speech_rate=0.9`) как «наилучший на вкус»; на полном контенте (1-ru, длинный текст)
  звучит **приемлемо**;
- **дрейфа нет** (CV F0 5.9%/6.3%);
- **лицензия чистейшая** — **Apache-2.0 и код, и веса** (единственный такой из трёх).

Оговорки vosk: **22050 Гц** (глуше 44.1 кГц Supertonic); RTF 0.26–0.75 на CPU
(медленнее, но для конвейера годно); **латиница не поддерживается вовсе**
(`KeyError: 'a'`) — транслитерация обязательна, не опциональна; фронтенд падает на
ёлочках `«»`/тире и на pre-composed `ё` — нужен санитайзер + NFD-нормализация;
транслитерированные англицизмы звучат «плоховато, но лучше двух предыдущих движков».

**Свод по полю (все шесть проверенных движков):**

| движок | ударения | естественность | звук | дрейф | лицензия весов | вердикт |
|---|---|---|---|---|---|---|
| OpenAI gpt-4o-mini-tts | — | монотонно | — | — | облако | ✗ (реверт) |
| Gemini TTS | — | хорошо | хорошо | **франкенштейн** + `503` | облако | ✗ (реверт) |
| piper | плохо | **плоско** | 22 кГц | — | смешанная | ✗ (реверт) |
| Qwen3-TTS | плохо | эмоц. дрейф | 24 кГц + кит. акцент | **удвоение CV** | Apache-2.0 ✅ | ✗ NO-GO |
| Supertonic 3 | **очень плохо** | **хорошо** | 44 кГц ✅ | нет ✅ | OpenRAIL-M ⚠️ | ✗ NO-GO (ru) |
| **vosk-tts** | **хорошо** (промах на омографах) | механично→**настраиваемо** | 22 кГц | нет ✅ | **Apache-2.0** ✅ | **✓ GO** |

### 13.8 Стратегическая развилка и высокоуровневый план

> **Развилка закрыта (2026-07-23) в пользу OpenAI TTS** — см. §13.9. Текст ниже
> оставлен как формулировка развилки и высокоуровневый план **vosk на случай
> задела** (offline/не-OpenAI аудитория).

По итогам спайка **главная развилка сместилась** — с «какой локальный движок» на
«локальный **или** облачный/внешний вообще»:

- **Локальный (vosk-tts).** Плюсы: бесплатно, приватно, offline, лицензия
  Apache-2.0, дрейфа нет, ударения решает движок. Минусы: **высокая сложность
  интеграции** (см. план ниже) — для поставки «без Python» нужен свой русский фронтенд;
  22 кГц; омографы всё же промахиваются. Пользователь прямо отметил: «слишком много
  сложностей».
- **Облачный / внешний платный.** Резон пользователя: облачные LLM уже оплачиваются, а
  TTS едва ли дороже (данные §3 это подтверждают: OpenAI ≈ $0.015/мин, у Gemini flash
  есть бесплатный тир — на фоне токенов Opus это копейки). **Но:** «облако» повторно
  открывает вопрос качества, на котором умер первый заход — Gemini давал франкенштейн,
  OpenAI монотонен. То есть «пойти в облако» ≠ автоматическое решение: нужен облачный
  движок, который **и звучит хорошо, и не дрейфует**. Это отдельная короткая оценка
  (аналог этого спайка, но для облаков — ElevenLabs-класс, Azure, новые голоса OpenAI),
  которую стоит провести **до** выбора, а не после. `external`-режim уже покрывает любой
  OpenAI-совместимый сервер (в т.ч. премиум-облака через прокси, §3.4).

**Развилка не закрыта — решает пользователь.** Ниже — высокоуровневый план **на случай
выбора vosk** (если победит облако, применяется куда более тонкая ветка: поднять
отревертнутые `external`/`OpenAi`/`Gemini`-клиенты из `git show 19ead9f` + выбрать
провайдера по короткой оценке качества).

**Высокоуровневый план интеграции vosk-tts** (детали — при старте направления):

1. **Инфраструктура из отревертнутых коммитов** (`git show 19ead9f`): речевой
   экстрактор `pulldown-cmark` (§5), команда `/tts`/`N`/`all`/`stop` (§7), плеер `rodio`
   (§6), точки остановки, вкладка «Озвучивание» (§9). Провал их не касался —
   переписывать не нужно, только поднять.
2. **Русский фронтенд на Rust** (главная и самая дорогая часть, аналог отдельной
   подсистемы): BERT-разметка ударений (`tokenizers` — родной Rust-крейт + ONNX-BERT
   через `ort`), g2p, санитайзер пунктуации (`«»`/тире/`ё`→NFD), **обязательная
   транслитерация латиницы кириллицей** (таблица правил; находка §13.7 — работает на
   любом движке). Риск — точно воспроизвести их `g2p_multistream`; словарь ударений
   RUAccent (MIT) переиспользуем как **данные**.
3. **Акустика через `ort`**: 4–5 ONNX-графов vosk + BERT-эмбеддинги на вход;
   дефолтные параметры синтеза — `slow-exp` (`duration_noise_level=1.3`,
   `speech_rate=0.9`); частота 22050 Гц.
4. **Провизия `mindfork tts setup`** по lock-списку sha256 (паттерн ADR 0005): модель
   vosk (746 МБ) + BERT + словари в `data/tts/`.
5. **Рантайм-развилка отложена** (решение пользователя «пока не решать»): порт
   фронтенда in-process (чистейше, но риск точности g2p) **против** Python-сайдкара в
   wasmer/WASIX (нулевой риск g2p — их же код, но `onnxruntime` под WASIX — большой
   вопрос). Разобрать отдельным зондом-разведкой перед реализацией.
6. **Омографы** остаются нерешёнными точечно (`сто́ит` и пр.): либо мириться, либо
   маленький словарь контекстных правил поверх фронтенда — задел.

**Открытые заделы независимо от выбора:** переносимая транслитерация англицизмов
(снижает акцент на **любом** локальном движке — задел для локальной ветки); короткая
оценка качества премиум-облаков (ElevenLabs/Azure/новые OpenAI-голоса) на тех же
текстах — **проведена, §13.9**.

### 13.9 Облачная оценка (2026-07-23) — итог: OpenAI GO

Прогон облачных TTS на тех же четырёх текстах, что локальный спайк (вне репозитория,
ключи из env, не из чата). Пользователь просил ElevenLabs / Azure / OpenAI; **доступны
были ключи только OpenAI и Gemini** (ElevenLabs/Azure — нет). Скрипты для ElevenLabs
(`eleven_multilingual_v2`) и Azure (`ru-RU-DmitryNeural`, SSML) написаны и готовы к
запуску (`C:\tts-spike\cloud-*.py`), но **не потребовались** — см. решение ниже.

**OpenAI `gpt-4o-mini-tts` — GO.** Голос `onyx` + инструкция «natural clear Russian,
calm male voice». Вердикт пользователя:

- **ударения** — 1 ошибка на весь абзац (`за́мок на двери` — истинный омограф,
  разрешимый только знанием мира). Это **лучше всех локальных**, включая vosk;
- **англицизмы** — «идеально»: латиница читается **нативно**, транслитерация не нужна
  (снимает целый пласт работы, обязательный для локального пути);
- **дрейф** — нет: CV F0 **2.7%** одним запросом / 3.2% по предложению (лучший
  показатель среди всех проверенных, локальных и облачных). F0 медиана 94 Гц —
  уверенно мужской (жалоба реверта «мужской читался женским» не воспроизвелась).

**Почему OpenAI провалился при реверте, а теперь нет.** Первый заход дал «монотонный
женский» на выбранном мужском голосе — это была **ошибка конфигурации** (голос/модель/
отсутствие `instructions`), а не предел OpenAI. С `gpt-4o-mini-tts` + `onyx` +
инструкцией на русский результат отличный. Урок: движок был отвергнут по симптому
неверной настройки.

**Диагноз «выросших пауз» (единственное замечание пользователя к chunked).** Тот же
длинный текст: одним запросом **129.9 с**, по предложению — **154.6 с**. Разница 25 с
— это накопленные паузы от **нарезки** (30 самостоятельных синтезов дают ритм
«стоп-старт»), а не свойство OpenAI. Обрезка хвостовой тишины **не помогает** (проверено
— дело не в тишине, а в пофразовой финальной интонации). **Правильное решение —
не нарезать:** лимит запроса **4096 символов**, типичное сообщение (тест 1720) уходит
**одним запросом** → непрерывная просодия, пауз-артефакта нет. Нарезка (по абзацам, не
по предложениям) — только для редких сообщений > 4096 символов. Это отличает облачную
интеграцию от локальной, где пофразовый конвейер был нужен ради быстрого первого звука.

**ElevenLabs / Azure — не проверялись, отклонены по продуктовому доводу** (решение
пользователя): «пользователи программы не захотят регистрироваться в стороннем сервисе
ради одной озвучки». OpenAI этого недостатка лишён — у пользователя облачного чата ключ
уже есть (ADR 0008). ElevenLabs (вероятно лучший по звучанию, но ~$0.06–0.30/мин и своя
регистрация) и Azure (сильный русский, нужен ключ + регион) остаются **заделом**, если
понадобится премиум-качество; `external`-режим их покрывает (в т.ч. через прокси, §3.4).

**Свод облачной ветки:**

| движок | ударения | англицизмы | дрейф | новый vendor для юзера | вердикт |
|---|---|---|---|---|---|
| **OpenAI gpt-4o-mini-tts** | **1 ошибка/абзац** | **нативно** ✅ | нет (CV 2.7%) | **нет** (ключ есть) | **✓ GO — основной** |
| Gemini TTS | — | — | франкенштейн (§13.1) | нет (ключ есть) | опция (в объёме этапа) |
| ElevenLabs | — | — | — | да | задел |
| Azure Neural | — | — | — | да | задел |

**Итоговое решение (2026-07-23).** Основной режим — **OpenAI TTS**. Объём этапа —
**OpenAI + Gemini + external** (Gemini включён как опция для тех, у кого его ключ, при
всех его минусах; external — свой сервер/премиум-облако через прокси). **Локальный
сайдкар в этап не входит** (vosk — задел, §13.8). Пересмотр Р1 (§13.5) отменён: облака
не «опциональны», а **основной путь**; локальный — задел. Реализация ложится на
отревертнутую инфраструктуру (`git show 19ead9f`): экстрактор, команда `/tts`, плеер
`rodio`, клиенты OpenAi/Gemini/External уже написаны — поднять и допилить one-shot для
сообщений < 4096 символов.
