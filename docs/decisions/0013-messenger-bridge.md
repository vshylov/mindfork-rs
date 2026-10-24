# ADR 0013 — The messenger bridge: a seat of its own for a turn whose chat is not on screen, Telegram first

**Status:** accepted (2026-10-23) — the owner chose D1 (a), D2 (a), D3 (a), D4 (a)
and D5 (b) (*User's decision* below). Stage B built (`feat/messenger-bridge`; spec
§11.13, [docs/journal/tools.md](../journal/tools.md)); stage C's forks (D6–D9) decided
by the owner on 2026-10-24 after a probe — D6 (a), D7 (a), D8 (b), D9 (a) (*Stage C*
below). The track's
design and the forks already decided (F6 (a) with Telegram first, F9 (a), F10 (a)) —
[docs/research/companion-platform.md](../research/companion-platform.md) C5, §7, §8,
§9 stage 6. Related: [ADR 0008](0008-api-key-storage.md) (the bot's token is a
credential), [ADR 0009](0009-tts-speech-synthesis.md) (replies as voice notes),
[ADR 0010](0010-subagent-nested-turn.md) (a child inherits the turn's tool set).

## Context

A companion lives where its person is, and that is a phone, not a pod's `tmux`
(research C5). The owner chose a bridge inside the running app, built so it can move
out of process later, Telegram first behind a channel interface and Matrix second
(F6 (a)(i)); §7's rules for everything that acts unasked (F9 (a)); speech input with
the bridge (F10 (a)).

What the codebase does today decides most of the shape:

1. **A turn belongs to the open chat.** `AppCommand::SendMessage(String)` names no
   chat; there is one `GenState` for the application; switching chats cancels the
   running turn (`chats.rs::switch_to`, except a move into its own transcript); the
   stream reaches the screen keyed by a generation id, which the screen takes as the
   open conversation's. A message from a phone has no chat on screen to run in.
2. **A seat already exists.** A background sub-agent run (`background_runs.rs`) runs
   beside the turn under the app-wide session budget, keeps a mirror, has its stream
   forwarded to the screen only while its transcript is open (`forward_child`), and
   lands on its chat by id. The landing work after a turn (`handle_done`) is keyed by
   the chat, not by the screen, apart from the generation gate, the confirmation
   sender, the in-flight mirror and the open-chat wake.
3. **A wake waits for the person to look.** A follow-up delivered to a chat that is
   not open is answered when the chat is opened (F5 (i)); a background run's
   notification wakes a turn only in the open, idle chat. A phone cannot open a chat.
4. **Dangerous tools are four families** (`Tool::danger`): `python_exec`, `fs_write`,
   the code workspace's editing and command tools, and every MCP tool. Their
   confirmation is a terminal popup and is off by default (`tools.confirm_dangerous`);
   a background run asks nothing (background-subagents fork F3). A messenger is remote
   control of whatever a turn may call (research C5, *Risks*).
5. **Concurrent writers are already safe where it matters.** The self-model is written
   by an atomic read-edit-write (`Db::self_model_update`) because reflection, `F3` and
   a turn's tools already race; notes and attachments are SQLite rows; the session
   budget is shared by every stream.

What Telegram's Bot API offers (version 10.3 of 2026-08-24, read 2026-10-23,
[core.telegram.org/bots/api](https://core.telegram.org/bots/api)):

- **Long polling** (`getUpdates`) is an outbound request, so a pod behind NAT needs no
  address; updates wait on the server for at most 24 hours; only one consumer may
  poll a bot (a second one is refused, `409 Conflict` in practice), and polling does
  not work while a webhook is set. A bot cannot start a conversation: the person
  writes first.
- **`sendMessageDraft`** streams a partial reply into a private chat — a preview that
  lives about 30 seconds and must be followed by `sendMessage`; an empty text shows
  *Thinking…*; with `can_stop` the person gets a button that stops it, and the bot
  receives a `stopped_message_generation` update.
- **Voice**: a voice note arrives as OGG/Opus; a bot downloads files up to 20 MB;
  `sendVoice` takes OGG/Opus, MP3 or M4A up to 50 MB.
- **Identity**: a user id has at most 52 significant bits and never changes (the
  username does); `forward_origin` marks a forwarded message.
- **Privacy**: a bot chat is encrypted between each party and Telegram's servers, not
  end to end ([FAQ](https://telegram.org/faq)).

## Forks

**D1. Where a turn that came from the messenger runs.**
(a) **A seat of its own, like a background run's** *(recommended)*. The turn runs
beside whatever the terminal is doing, under the shared session budget — on a
one-slot server the two take turns between requests. A chat never has two turns at
once: a message arriving while its chat's turn runs waits, and when that turn lands
everything that waited goes in as the person's messages and one turn answers them (a
phone sends three short lines in a row). The stream goes to the messenger, and to the
screen only while that chat is open, from the same mirror a background run keeps.
Switching chats at the terminal does not cancel it; `Esc` in that chat and `/stop` in
the messenger do. The landing is the existing one — title,
compaction, reflection, follow-ups, the language-model history — keyed by the chat.
Cost: the landing learns a second source, and the fourteen places that ask whether a
turn runs split into "the terminal's turn" and "this chat's turn".
(b) **The app's one turn, queued.** Smallest. But a phone waits behind the terminal's
turn — on a pod running an hour of agentic work, for the whole hour — the terminal
is blocked while the phone talks, and switching chats would have to learn not to
cancel a turn it did not start.
(c) The chat opened on screen. Rejected: it takes the person's screen and cancels
what runs there.

**D2. Which conversation the messenger reaches.**
(a) **One chat per bridged profile; `/new` in the messenger starts the next**
*(recommended)*. Created when the account is paired, marked in the chat list,
openable at the terminal like any chat. The companion's replies to the messenger's
messages go to the messenger, and so does what it writes unasked in that chat
(follow-ups, a background run's result); what the person types into it at the
terminal is answered at the terminal.
(b) Any chat, bound with `/bridge here` at the terminal.
(c) A new chat per day.

**D3. What tools a turn from the messenger may call.**
(a) **The profile's set without the four dangerous families, whatever
`confirm_dangerous` says** *(recommended)*. Nobody at the terminal can answer a
popup, and those four are what a stolen account would want. Notes, memory, the
self-model, web search and fetch, attachments and follow-ups stay; a sub-agent
inherits the reduced set (ADR 0010 §3).
(b) The same, plus the dangerous ones asked in the messenger with Allow/Deny buttons,
no answer within a few minutes being a refusal.
(c) A tool list of its own per profile for bridged turns.
(d) No tools at all.

**D4. One bot per profile, or one for the app.**
(a) **One bot per profile** *(recommended)*: the companion is a contact of its own,
its name and picture set at BotFather; its token kept in the key store under the
profile; a fork (`/profile fork`) and a carried companion (`companion export`) take
neither the bot nor the paired account — two copies polling one bot is a `409`.
(b) One bot for the app, the profile picked by a command in the messenger.

**D5. What the messenger is shown of a turn.**
(a) **The reply as it is written (`sendMessageDraft`, with the stop button), then the
finished reply as a message; thoughts and tool cards stay at the terminal**
*(recommended)*.
(b) *Typing…* while the turn runs, then the whole reply.
(c) (a), plus one line per tool call ("searched the web").

### User's decision (2026-10-23)

- **D1: (a)** — a seat of its own; a chat never has two turns at once.
- **D2: (a)** — one bridged chat per profile, `/new` in the messenger starts the next.
- **D3: (a)** — the profile's set without the four dangerous families.
- **D4: (a)** — one bot per profile.
- **D5: (b)** — *typing…* while the turn runs (`sendChatAction`, which lasts five
  seconds, sent again while it runs), then the whole reply; no draft, so a turn is
  stopped by `/stop` in the messenger or `Esc` at the terminal.

## Rules that are not forks

1. **In-process, against plain data.** One bot per bridged profile — a poller and a
   sender, started with the app and stopped with it. They talk to the orchestrator in
   plain data only, each way on a channel of its own — `Inbound` up, `Outbound` down —
   so moving the bridge into a program of its own (research C5 (c)) is a serialisation
   of these two types and a socket, not a redesign. (Built as the orchestrator's own
   channels, like its other tasks', rather than as an `AppCommand` variant: the
   commands are what the screen sends.) No listening socket: long polling only. The
   channel interface is these two types — Telegram now, Matrix next; an account id is
   a string, since Matrix's are. Code: the Bot API client in `shared/messenger/`, the
   markup and its cuts in `shared/markdown/telegram.rs`, the pairing, the commands and
   the reading of Telegram's messages in `features/bridge/`, the bots and the seats in
   `app/orchestrator/bridge.rs` and `bridge_task.rs`.
2. **Who may write: one paired account.** A one-time code shown at the terminal, valid
   for ten minutes, with a `t.me/<bot>?start=<code>` link Telegram turns into
   `/start <code>`; the sender's **user id** becomes the profile's allowed account.
   Anyone else gets no answer, nothing of theirs is stored, and the log has one line
   without the content. A forwarded message reaches the model labelled as forwarded,
   never as the person's own words.
3. **Only while the app runs.** The bridge is part of the running app, like
   follow-ups; on a pod that is the TUI in `tmux`. Telegram keeps what was sent for a
   day, so a laptop that slept answers when the app starts again. A headless mode is
   not built.
4. **What the person is told first.** Before a channel is turned on, the setting says
   who can read what: Telegram's servers read every message and voice note of a bot
   chat; the history stays on this machine; the token is the bot. The manual says it
   too.
5. **Formatting.** Markdown becomes Telegram's HTML subset (bold, italic, code, code
   blocks, links, quotes); a reply longer than a message is split at a paragraph, then
   a line. The messenger's commands are `/new`, `/stop` and `/start <code>`; the rest of
   the app's commands stay at the terminal.
6. **Every failure said once, in the terminal.** A revoked token (`401`) stops that
   profile's bridge; a `409` names the cause (another copy polls this bot, or a webhook
   is set); network errors retry with a back-off and stay quiet.
7. **Data.** `Profile.bridge` (the channel, the paired account, the bridged chat) is
   written only once set, so a profile that never bridges is written as before; the
   token is `SecretKey::BridgeToken(profile)` in the machine-bound store (ADR 0008).

## Stages and the live gate

- **A** — this ADR.
- **B** — the seat and Telegram text: pairing, the tool set, the typing status, `/new`,
  `/stop`, the settings rows. Live: an `#[ignore]` smoke through a live model against a
  Bot API stub (a local server answering `getMe`, `getUpdates`, `sendChatAction`,
  `sendMessage`), and one run with the owner on real Telegram — a bot from BotFather,
  paired from a phone — before the pull request.
- **B, second half** — pictures from the phone, which the owner's run of B asked for:
  a photo, an image file or an album, fetched by the bot for the paired account only and
  prepared as `/image attach` prepares one, on the person's message with its caption
  (spec §11.13).
- **C** — voice (C7 (b)): a voice note transcribed, where the speech is recognised
  decided then with a probe; a reply sent as a voice note when the profile speaks its
  replies (`speak_replies`), the text kept beside it.
- **Later** — Matrix, for end-to-end encryption and for people Telegram does not
  reach (research C5, *Channels*).

## Stage C — voice notes (decided 2026-10-24)

The probe that was to decide where speech is recognised is research §3.8. What it
found that shapes the forks below:

- Every recogniser that speaks the transcription API took a note as Telegram sends it
  (OGG/Opus, 48 kHz mono): OpenAI's, eleven of the gateway's, Gemini's
  `generateContent`, Whisper served by vLLM. Only llama.cpp's chat endpoint wants WAV.
- The cloud answers a note in about a second, for under a cent a minute. On Russian no
  one model is best: Gemini 3.8 Flash heard crowd speech best but took 8 s a note (46 s
  at worst); ElevenLabs Scribe read speech best; OpenAI's `gpt-transcribe` heard
  Russian full of English terms best, all sixteen of them.
- A local Whisper (vLLM, an RTX 4090) heard Russian about as well as OpenAI's
  `whisper-1`, five times faster, through the same API. Gemma 4 E2B, the small model
  llama.cpp hears with, came last.
- Two traps: `gpt-4o-transcribe` drops everything after a pause; a wrong language hint
  makes Whisper translate the note into English or give up.
- `opus-rs` (a pure-Rust port of libopus 1.6, no C) with the `ogg` crate writes a voice
  note ffmpeg and `gpt-transcribe` read without a fault, and reads Telegram's back into
  samples.

### Forks

**D6. Where a voice note is heard.**
(a) **A speech-recognition slot of its own, built like the speech slot (ADR 0009)**
*(recommended)*. Modes: `openai` (OpenAI's `/audio/transcriptions`, default model
`gpt-transcribe`); `openrouter` (the gateway's, no default model — a picker over its
transcription list, 28 models on 2026-10-24); `external` (any server with that API:
vLLM, speaches, LocalAI, whisper.cpp's server — for a person who wants nothing beyond
Telegram to hear the note). A cloud mode uses the provider's stored key, as the speech
slot does (ADR 0008); `external` has a key of its own. No Gemini mode: its transcription
model answered `generateContent` with no text, and its chat model with eight seconds a
note — both are reachable through the gateway.
(b) **The chat model hears the note**: the audio goes into the turn for an engine that
takes audio. No second provider, but most engines cannot (neither of the gate's models,
Claude, DeepSeek); the chat would keep sound rather than words, so search, notes and
compaction would have nothing to read, and every later turn would send the audio again.
(c) **A local recogniser the application starts**: a second `llama-server` with Gemma
4 E2B. Offline, but it came last on Russian, llama.cpp calls its audio experimental,
and a local Whisper is already reachable through (a)'s `external`.

**D7. What a voice note becomes in the chat.**
(a) **The transcript, as the person's message, marked as spoken; the recording not
kept** *(recommended)*. The terminal shows the mark and the length (*🎤 0:12*); the
model reads the words under a short label (*voice message, transcribed*), so a misheard
word reads as the recogniser's, not the person's.
(b) (a), with the recording kept beside the chat, playable at the terminal.
(c) The transcript unmarked, as if typed.

**D8. When the companion answers with a voice note** (only while the profile speaks its
replies, `speak_replies`).
(a) Every reply in the bridged chat, and what it writes there unasked — stage C as this
ADR first wrote it.
(b) **A voice note is answered by a voice note, a typed message by text**
*(recommended)*: the person picks the medium each time — a voice note on the move, text
in a meeting — and unasked messages stay text.
(c) A switch of the messenger's own beside `speak_replies`.

**D9. How the voice and the text of a reply go together.**
(a) **The text at once, the voice note when it is ready** *(recommended)*: the reply is
not held back for the synthesis, which takes seconds for a long one; if the synthesis
fails, the text has already arrived.
(b) The voice note with the text as its caption: one message, but a caption holds 1024
characters, so a longer reply still needs a second message, and nothing arrives until
the voice is ready.
(c) The voice note alone; the text only at the terminal.

### User's decision (2026-10-24)

- **D6: (a)** — a speech-recognition slot of its own: `openai`, `openrouter`, `external`.
- **D7: (a)** — the transcript as the person's message, marked as spoken; the
  recording not kept.
- **D8: (b)** — a voice note answered by a voice note, a typed message by text.
- **D9: (a)** — the text at once, the voice note when it is ready.

### Rules that are not forks

1. **One road for the sound, in Rust.** A note goes to the recogniser as Telegram sent
   it. A server that refuses it gets the same note once more as 16 kHz WAV, decoded in
   process, and that is remembered for the session — as a refused effort is
   (`shared/api/effort.rs`). A reply is synthesised as the terminal would speak it — the
   same text, code blocks passed over, the profile's voice — and every speech mode's
   answer is made into samples, then written once as OGG/Opus 48 kHz mono, the format
   Telegram shows as a voice message. The codec is `opus-rs` with `ogg`: nothing to
   install, nothing in C.
2. **No language hint unless one is set.** The recogniser detects the language. A
   setting may name the one the person speaks, for a model that needs it (the gateway's
   Whisper and nova-3 did); Telegram's `language_code` is never used — it is the
   client's language, and a wrong hint made Whisper translate.
3. **Voice notes only.** A `voice` message; an audio file and a round video stay
   unsupported in this stage.
4. **Failures are said where they matter.** With no recognition set up, or a refusal,
   the person is told in one line in the messenger, and the terminal says why once. A
   synthesis that fails costs the voice note only: the text has gone, and the terminal
   says why once (`speak_replies`' rule).
5. **What the person is told first** gains a line: the recogniser hears the note too,
   and the setting names which provider.
6. **The message's mark is a field written only when set**, so a chat without voice
   notes is written as before.

### Live gate

An `#[ignore]` smoke through the test's Bot API: a synthesised note sent from the
phone's side, heard by a live recogniser, answered by the gate's model, and — with
`speak_replies` on — sent back as a voice note that the stub hears back through a
recogniser. Then the owner on real Telegram.

## Consequences

- A turn stops being a property of the screen: a chat can be answered while another
  is open, which is also what a follow-up or a background result in a bridged chat
  needs to reach the phone. §7's limits on unasked messages — quiet hours, the day's
  cap — apply unchanged.
- A second surface writes into a chat file the terminal may have open; the
  orchestrator stays its only writer, so this is ordering, not locking.
- A person on a phone has fewer tools than at the terminal (D3 (a)), by design.
- Telegram sees the conversation. Whoever needs it unseen waits for Matrix.
