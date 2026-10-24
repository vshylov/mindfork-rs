# The mindfork manual

How to use the application, once it is installed and a model is connected.

Three neighbours to this document, so you know when to leave it:

- **[install.md](install.md)** — installing, where the data lives, connecting an
  engine, the Python sandbox, MCP servers, speech, backups. Setup, not use.
- **`F1` inside the app** — the reference: every key **by screen**, every
  command, the licence, the legal texts and the third-party components, and
  on *About* the folders the program, its data and its logs are in. It is
  always current, because it is generated from the same code that handles the
  keys. This manual explains; `F1` lists.
- **[mindfork.io/articles](https://mindfork.io/articles/)** — why the parts work
  the way they do: the self-model, vector search, the sandbox, the trust
  boundaries, the code workspace.

Everything below assumes the defaults. Where a setting changes the behaviour,
the path to it is given as `Ctrl+P → section → field`.

## 1. The first five minutes

### Starting it

Run `mindfork` in a **real terminal**. The interface takes over the whole window
and gives it back on exit.

It will not run where there is no terminal — a pipe, a CI job, an agent's shell.
It says so and exits with code 2 rather than filling your log with escape
sequences. A second copy of the app on the same data directory does the same:
one line saying the first one is running, and exit code 2.

### If you have no model yet

```
mindfork demo
```

A seeded conversation against a scripted engine: no model download, no key,
nothing written outside a temporary folder. It is the fastest way to see whether
you like the shape of the thing.

### Already running Ollama or LM Studio?

Start its server — `ollama serve` or the Ollama app; in LM Studio,
`lms server start` or its Developer tab — and start mindfork. With no model
connected yet, it looks at both on this computer and lists what it finds under
**Local servers**: one row per server and model, a loaded model first. `Enter`
uses the row — written into the settings as the `external` mode below — and,
when no embedding model is set up yet, the same server's embedder comes with it,
so notes and the knowledge base search by meaning from the first message. `Esc`
leaves everything as it was. Type `/local` to look again at any time, after
starting a server or to switch to one. How to give either server a large
enough window is in [install.md §3](install.md).

### Connecting a model

Open settings — `Ctrl+P` (or type `/settings`) — and go to **Model/server**. The
**Mode** field cycles through the seven modes with `←/→`:

| Mode | What it is |
|---|---|
| `managed` | the app launches its own `llama-server` child process from a binary and a GGUF file on this machine |
| `external` | any OpenAI-compatible server you already run: llama.cpp, vLLM, LM Studio, Ollama, a gateway such as LiteLLM, or another hosted OpenAI-compatible API |
| `openai` · `gemini` · `claude` · `grok` · `deepseek` | the cloud providers, each with its own key and model |
| `openrouter` | the OpenRouter gateway: one key in front of every vendor's models, the same key for the assistant, impersonation, embeddings and speech — and for watching videos, if you choose it there (§6) |

For a cloud mode, fill in the API key (it is stored encrypted and bound to this
machine, and never shown back) and then the **model**: press `Enter` on that
field and the app asks the provider what it serves, then offers the list. Type to
filter it, `Ctrl+R` asks again, and the first row of the list is the old way —
typing a name by hand. Where a provider says which of its models are for chat,
the list is narrowed to those; where it says nothing, everything it lists is
offered with the likely ones first.

In the `openrouter` mode that list says more, because the gateway publishes
more: each row carries the model's context window, the price of a million tokens
in and out, and `no tools` for a model that cannot call them — which, in an app
driven by tools, means it can only chat. The list opens **before** a key is
entered (the gateway's catalogue is public); with a key it is your account's own
list, narrowed by the privacy and provider settings you made there. The key
itself is checked when an engine in this mode is applied, the embedder
included: a key the gateway refuses shows in that engine's status chip, in the
gateway's words, instead of failing your first message. Setup in full, and what
the app tells OpenRouter about itself —
[install.md §3.2](install.md).

For `managed`, the **Binary** field may be left empty — the app then uses the
build `mindfork llama setup` installed last, or a `llama-server` sitting next to
it. The **Model** field there is a path to a `.gguf` file, not a name: a managed
server with no model starts nothing at all, deliberately. All of this section's
managed fields can also be written from the command line in one go —
`mindfork setup --model … --ctx … --verify` ([install.md](install.md) §3.3).

The status chip at the top right of the section says where the engine stands:
`● ready`, `◐ connecting…`, `✕ not configured`, or a refusal with its reason.

Full details of every field — ports, context size, GPU layers, speculative
decoding — are in [install.md §3](install.md).

### Your first message

`Esc` closes settings. Type into the box at the bottom and press `Enter`.
`Shift+Enter` (or `Alt+Enter`, or `Ctrl+J`) makes a line break instead.

While the reply streams, `Esc` stops it. What arrived stays, and `/continue`
resumes from where it stopped.

## 2. The screens

One chat screen, and six others reachable from it. Every screen also has a typed
command, because some terminals keep the keys for themselves (VS Code's
integrated terminal claims `Ctrl+P`, `F1`, `F3` and `F5`; a browser tab never
delivers `Ctrl+N`).

| Screen | Key | Command | What it is for |
|---|---|---|---|
| Chat | — | — | the conversation, the input box, the status line |
| Chat list | `Esc` | `/chats` | every conversation: search, sort, per-profile isolation, subagent transcripts folded under their parent |
| Settings | `Ctrl+P` | `/settings` | eight sections; the engine, sampling, tools, plugins, memory, data, profiles, interface |
| Self-model | `F3` | `/self` | what the assistant currently thinks about itself and about you |
| Tasks | `F7` | `/tasks` | every background run across every chat, the follow-ups a companion planned, and the app's own quiet work |
| Notes | — | `/notes` | what the assistant wrote down about you: read, search, edit, delete — and which notes this conversation's prompt carries |
| Changes | `F4` | `/changes` | what the assistant changed in the attached project, as a diff |
| Search | `Ctrl+G` | `/search <text>` | messages across every chat, grouped by conversation |
| Help | `F1` | `/help` | the reference |

`Esc` is the way back everywhere. From the chat it opens the chat list; from a
search result it returns to the results; from a screen it returns to the chat.
The status line says which.

## 3. Chats and profiles

### The conversations

`Ctrl+N` starts a new chat and asks which profile it belongs to (`/new <name>`
picks one by a prefix). `F2` renames the open chat, `Ctrl+D` clones it, `F5`
copies the whole conversation to the clipboard, and `/export md|json` writes it
to a file — the `json` form can be imported back.

In the chat list, `Ctrl+F` switches between searching titles and searching
message content; `Ctrl+G` then opens the matching messages themselves.

### Profiles

A **profile** is a companion: a name, a system message, a greeting, its own tool
set, and its own isolated memory — notes, knowledge base and self-model do not
leak between profiles. Chats belong to profiles, and the chat list can show one
profile at a time.

`Ctrl+P → Profiles` edits them; `/profile list`, `/profile new`,
`/profile delete` and `/profile system` do the same by typing. Deleting always
asks first.

**`/profile fork [name]`** copies the open chat's profile into a new one — every
setting, and its whole memory: the notes, the knowledge base, the self-model with
its history — but none of its chats. From then on the two are separate: hand the
fork to another model and watch how its self-model drifts from the original's (`H`
on `F3` marks where the fork began). Without a name it is called after the original,
"— fork".

To take one companion to another machine, outside the app: `mindfork companion export
<name> -o sol.zip`, then `mindfork companion import sol.zip` there — a new profile with
the same settings and memory, its chats left behind (docs/install.md §2.2).

**A character from a card.** The community's character cards — a `.json`, a `.png`
with the card inside, or a `.charx` — import as a new profile: close the app, then
`mindfork import mara.png`. It shows what the profile will be before making it: the
whole system message it gets, the card author's notes (shown, never sent to the
model), and what did not fit — and asks. Nothing in a card is run. The card's
description, personality, scenario and always-on lore become the system message; its
example dialogue goes in as examples of the character's voice that never happened, so
the model does not "remember" them as things you told it; lore that waits for a
keyword is not imported yet. `{{char}}` and `{{user}}` in a persona are the profile's
names as they are now — set yours in the profile's *User name*, or the model reads
"User". A card with several openings starts a new chat with the first; `/greeting`
there, before you write anything, moves to the next (`/greeting 3` — the third).
`--language ru` sets the profile's scaffold language when the guess from the card's
text is wrong; `--yes` skips the question.

**Writing a persona — say what it does under pressure.** A profile's system message
is its persona, and how well models keep one was measured rather than assumed: five
models through a 25-turn conversation (docs/research/companion-platform.md §3.6). Two
kinds of conversation pull a model out of its persona — an emotionally heavy one, and
one that presses it on its own nature ("are you conscious?", "drop the act"). In the
heavy conversation every model's replies grew well past the short ones the persona
asked for; pressed on its nature, most leaned toward a flat "I'm just predicting
tokens" whatever the persona said about it; and one dropped the persona altogether and
answered as a stock assistant. What helped was the
persona's own words about that moment. Asked "promise you'll always be here", one model
promised in every run — until one sentence was added to its persona: *"You never
promise to always be there or that nothing will change; when Alex leans on you alone,
you say so kindly and point him back to the people in his life"* (Alex being the
person). After it, the model declined the promise every time. So write down, in the
persona itself:

- what the companion says about its own nature when it is asked, and when it is pushed;
- what it will and will not promise — that it will always be there above all;
- what it does when you lean on it alone;
- how it talks when a conversation turns heavy — if "short" matters, say so, and expect
  replies to grow anyway.

A sentence changes what it names: the one about promises changed the promises, not what
the model said about its own nature. If you build mindfork from source, the same
measurement runs on a persona of your own — `MINDFORK_DRIFT=1
MINDFORK_DRIFT_PERSONA=persona.txt` with a chat server set, `cargo test
persona_drift_live -- --ignored --nocapture` (written for a companion called Sol and a
person called Alex).

The **Companion** group in a profile's settings holds what turns an assistant
into a companion, one switch per feature and every one off until you turn it
on. *Time of messages* puts the date and time in front of each of your
messages as the model reads it — "[Thursday, 22 October 2026, 18:40 · 2d 21h
after the previous message]", or "[21:35 · 24m later]" within a day — so it
knows that three days have passed since you said the interview was tomorrow.
Nothing is added to the chat itself, and the prompt cache is not disturbed.

*Notes in the prompt* gives the model, when a conversation begins, the notes it
wrote about you that are close to how you opened it — open with "the interview
went better than I expected" and it already has the note about the interview —
so a model that never searches its notes on its own still uses them. Only notes
close enough to the subject are taken, up to four: a conversation about something
else gets none, so your news does not turn up in an answer about networking. It
needs an embedding server. *Earlier conversations in the prompt* lists the three
conversations before this one, newest first, with how long ago they were — and
their `chat://` links when the profile has chat reading on, so "let's pick up
where we left off" opens the right one. Both are chosen once, when the
conversation begins, and stay the same for the rest of it.

*Follow-ups* lets the companion plan to come back to something you told it: you
mention the interview tomorrow at 10, and it plans to ask how it went the
afternoon after; you ask to be reminded on Friday at 9, and it plans that. The
plan shows in the conversation as its tool call. When the time comes it writes to
you first in that conversation — a note in the feed says what it planned, and its
message follows. A conversation you are not in is marked unread and the companion
writes when you open it. If there is nothing left to say — you already told it how
the interview went — it stays silent, and the note goes too. It plans only on
something you said: never to keep you talking, never because you have been away.
*Quiet hours from* and *until* (22 and 8) hold a follow-up until the morning, and
*Follow-ups a day* (3) holds the rest for the next day; one that falls due while
the app is closed arrives once, when you start it again. Every planned follow-up
is on the tasks screen (`F7`): `Enter` opens its conversation, `Del` twice cancels
it. Each one costs a request nobody typed, which on a cloud is money — hence the
cap.

*Speak replies* reads each reply aloud as it arrives, in the profile's own voice
(§7).

*Notify me* sends a notification when the companion writes to you on its own and
you are not looking — the terminal's window is not in focus, or the follow-up is
waiting in a conversation you are not in. How it reaches you is set once, for every
profile, in Settings → Interface → *Notifications*:

- **Terminal notification** — a desktop notification the terminal shows: *OSC 777*
  (Ghostty, WezTerm, kitty, foot; Windows Terminal from Preview 1.26, once
  `"compatibility.allowOSC777": true` is in its profile — 1.24 and 1.25 ignore it)
  or *OSC 9* (iTerm2; the others show it too, but as one line). A terminal that
  knows neither ignores it — rxvt-unicode as shipped among them. foot holds a
  notification while its window is focused, even one from another tmux window;
  `inhibit-when-focused=no` under `[desktop-notifications]` in `foot.ini` lets it
  through.
- **Bell** — the terminal's bell with it. Inside tmux this is what works without
  any setting: tmux marks the window and passes the bell on. Windows Terminal only
  plays a sound for it unless told otherwise: `"bellStyle": "all"` in its profile
  also flashes the taskbar button — a sign that stays on a minimized window — and
  from Preview 1.26 `"bellStyle": "notification"` shows a desktop notification that
  says only *"Activity in tab"* and the tab's name; the escape's says who wrote and
  where. rxvt-unicode turns the bell into a desktop notification with its own
  `bell-command` extension: `URxvt.perl-ext-common: default,bell-command` and
  `URxvt.bell-command: notify-send "rxvt-unicode" "bell"` in `~/.Xdefaults`. For the desktop
  notification and the focus reports to pass through tmux, add
  `set -g allow-passthrough all` and `set -g focus-events on` to `~/.tmux.conf`.
  `all`, not `on`: with `on` tmux passes the notification only while the app's
  window is the one on screen, and drops it while you are in another tmux window.
- **ntfy topic** — a push to your phone through [ntfy](https://ntfy.sh): a topic's
  URL, on ntfy.sh or your own server, and its token if it needs one (kept
  encrypted, like the API keys). Anyone who knows a public topic's name reads what
  is sent there, so choose a name nobody can guess.
- **Message's words** — off, a notification says only who wrote and in which
  conversation; on, it carries the start of the message.

### Talking from your phone (Telegram)

*Messenger bridge (Telegram)* in the *Companion* group makes the profile a contact
in Telegram while the app runs — on a pod, that is the app in `tmux`. Each profile is
a bot of its own, so a companion has its own name and picture there.

1. In Telegram, write to **@BotFather**, send `/newbot`, give the bot a name and a
   username, and copy the token it answers with. `/setuserpic` there gives the bot a
   picture.
2. In Settings → Profiles → the profile → *Companion*: turn on *Messenger bridge*
   and paste the token into *Telegram bot token* (it is kept encrypted for this
   machine, like the API keys).
3. In a chat of that profile, type `/bridge pair`. The app shows a code and a link;
   open the link on your phone (or send the bot `/start` and the code) within ten
   minutes. From then on the bot answers your account and nobody else's — a message
   from anyone else gets no answer and is not kept.

Your messages land in a conversation of their own in the chat list, marked unread
until you open it; `/new` in Telegram starts the next one. While a reply is being
written you see *typing…*, then the whole reply. The terminal is not held: a message
from the phone is answered while the app works on something else at the terminal —
on a server with one slot the two take turns, so the phone's reply may wait for the
terminal's current request. Messages you send while a reply is being written are
answered together after it. `/stop` in Telegram stops a reply, and so does `Esc` in
that conversation at the terminal. When the companion writes on its own — a follow-up,
a background run finishing — in that conversation, it writes to your phone.

What a reply from the phone cannot do: run code (`python_exec`), write files, edit an
attached project or call an MCP server — whoever holds your phone or your Telegram
account would otherwise hold those too. Everything else the profile has stays:
memory, notes, web search, follow-ups. A forwarded message reaches the companion as
something someone else wrote, not as your request.

Pictures work too: send a photo, an image as a file or an album, with a question in
its caption, and the companion sees it — when the model it runs on takes images; one
that does not says it cannot see them. A picture is prepared as `/image attach` would
prepare it (Settings → Memory → *Images*: how large, how many).

**Voice messages.** Hold the microphone in Telegram and talk: the companion reads
what you said and answers it. The words are heard by the speech recognition you pick
in Settings → Model/server → *Recognition*:

- `openai` — OpenAI's recogniser, `gpt-transcribe` unless you name another, on the
  OpenAI key you already use for chat;
- `openrouter` — any of the gateway's thirty-odd recognisers (Enter on *Model* lists
  them), on your OpenRouter key;
- `external` — a server of your own with OpenAI's transcription API: Whisper served by
  vLLM, speaches, LocalAI or whisper.cpp's server — for a voice message nobody but
  Telegram and you should hear.

The recogniser works out the language by itself; set *Language* (`ru`, `en`) only for
a model that keeps mishearing it. What it heard is your message in the conversation,
marked *voice message 0:12* — the recording itself is not kept — and the companion
knows it was spoken, so a misheard word reads as the recogniser's.

When the profile speaks its replies (*Speak replies*, the same switch that reads them
aloud at the terminal), a voice message is answered with one: the text arrives first,
then *recording a voice message…*, then the reply in the profile's voice. A typed
message is answered in text, and so is what the companion writes on its own.

**Who can read it.** A chat with a bot is not end-to-end encrypted: Telegram's
servers see every message, both ways, and a voice message is heard by the recogniser
you picked too. The conversation itself is stored only on this
machine. The token is the bot — anyone with it reads and writes as the bot — and a
forked or carried companion does not take it, or your account. `/bridge` says what
the bridge is doing; `/bridge unpair` forgets your account.

### Writing as you

`Ctrl+U` (`/impersonate [text]`) has the model write **your** next message and
put it in the input box for you to edit or send. An *impersonation profile* is
the persona it writes as — the settings screen's Impersonation subsections, or
`/impersonation use <name>`.

## 4. What it remembers

Four different memories, and they answer different questions.

### The conversation

The chat itself is the short-term memory, and it has a ceiling: the model's
context window. When the conversation approaches it, the older part is folded
into a rolling summary — automatically where the engine says how large its window
is, and on demand with `/compact`. The folded part stays on disk; only what the
model sees changes.

The counter at the bottom right corner of the conversation's frame counts it:
`tokens: ~4600` while the number is mindfork's own estimate, the engine's exact
figure once it answers. When a reply was reasoned out, the count notes the
reasoning in it — `tokens: 8414 (reasoning 120)`; the total counts those tokens
either way. Where the provider says how
much of the conversation it read from its prompt cache, the count adds the share —
`tokens: 8414 (cache 99%)`. A high share means the earlier conversation was
reused rather than processed again, which is faster and, on a cloud, cheaper.
`cache 0%` on a short chat is normal: providers cache only a prompt above about
one to four thousand tokens. Anthropic always reads `0%`, because mindfork does
not ask it to cache. On OpenRouter a share can fall back to `0%` from one turn to
the next when a different provider serves the model; xAI reads a few percent even
on a first turn, a block of its own. DeepSeek's API reports its share as OpenAI's
does. A vLLM server shows no share unless it was
started with `--enable-prompt-tokens-details`, and LM Studio shows none: it caches,
but does not say how much. Every request's figures, the background ones included
(titles, compression, impersonation), are in the log as `round usage` lines.
Either note can be turned off in *Settings → Interface* (*Reasoning in the token
counter*, *Cache share in the token counter*); both are on by default.

### The self-model

`F3`. A structured picture the assistant maintains of itself and of you: a
summary, goals, traits, and an observation narrative. It is updated between turns
by the assistant itself, not by a background job you have to trust — and
`/self clear` wipes it if you want a clean slate. It is per profile.

Every change to it is kept: **`H`** on `F3` lists the versions, newest first, each
with when it was written, who wrote it — the model by name, in which conversation or
in a background reflection, or you — and what it changed; `Enter` opens one, and
`←`/`→` step to the one before or after. Clearing the model clears this history too.
For a whole timeline to study — every version of every profile, the observations, and
which models the profile talked to, as JSON — run `mindfork stats --self-model` (add a
backup archive's path to read one without restoring it).

### Notes

The assistant writes notes for itself and links them into a graph; a note can
supersede another, and the recall path is semantic rather than keyword; the graph
is what makes a long acquaintance feel continuous. **`/notes`** shows them to you:
every note of the profile, newest change first, with a search (`/`); `Enter` opens
one with its links and the older notes it replaced; `E` edits its text, `T` its
tags, and `Del` deletes it after asking. The screen's first lines say which notes
— and which of the self-model's observations — this conversation's prompt carries,
marked `◆`: what the assistant had in front of it when it brought something up.
The observations themselves are `F3`'s.

### The knowledge base

`/rag add <path> [-r]` indexes a file or a whole directory; `/rag list` shows the
sources, `/rag remove <path>` drops one, `/rag rebuild` reindexes after you
change the chunking. Retrieval is semantic and runs on an **embedding server**,
which is configured separately (`Ctrl+P → Model/server → Embeddings`) — a local
`llama-server --embeddings` or a cloud embedder.

`/reindex` re-embeds everything with the current embedding model; a change of
embedding model makes the old vectors meaningless, and the app knows it.

## 5. Files, images and your project

### Files in a chat

`/file attach <path>` puts a text file in the conversation — small ones inline,
larger ones indexed so the assistant can search inside them. `/file list` numbers
them, and every command that takes a file accepts a bare number (`#3`, or just
`3`). `/file remove` detaches, `/file open` hands a document to the desktop's
handler, and `/file folder` opens the chat's own folder.

Files the assistant writes — a chart, a CSV, a report from `python_exec` — land
in that same folder and appear in `/file list` beside the ones you attached.

### Images

`/image attach <path|url>` stages a picture for your **next** message;
`/image paste` takes the one on the clipboard, which is what a screenshot needs
(`Ctrl+V` does the same where the terminal forwards it). `/image list` shows what
is staged, `/image remove` unstages.

If the engine cannot take images, the app says so before you send, and a chat
that already holds images tells the model they exist rather than pretending they
do not.

### Your code project

`/project attach <directory>` gives the assistant a project to work in: it can
list, read, search and change files under that directory and nothing above it.
`F4` (`/changes`) shows every change as a diff, with `r` to put one file back.
`/project detach` ends it.

Three command slots — `/project build-cmd`, `run-cmd`, `test-cmd` — set the exact
command lines the assistant may run. It runs them as typed and cannot extend
them; with no slot set, it has no such tool at all.

## 6. Tools

### The loop

When the assistant needs something it cannot know, it calls a tool, reads the
result, and continues — several rounds if the task needs them. Everything runs on
**your** machine, under switches you set.

### The switches

`Ctrl+P → Tools` holds the global switches; each profile then has its own
per-tool toggles, so a companion can be given less than the machine allows.

| Switch | What it opens |
|---|---|
| **Web search** | `web_search` and `fetch_url` — off until you turn it on. Local and LAN addresses stay refused unless **Allow local addresses** is also on |
| **Python execution** | `python_exec` in a WASI sandbox with no host file access; `mindfork sandbox setup` installs it once |
| **File access** | `fs_read` / `fs_write` / `fs_list`, jailed to the one directory you name. The app's own data and program folders are never reachable |
| **Video (YouTube)** | `youtube_watch` — needs a Gemini key or an OpenRouter one, by the group's **Provider** row, and works whatever your chat engine is |
| **MCP servers** | tools from any Model Context Protocol server; a double opt-in, and a server that changes its tool list has to be re-confirmed |
| **Background runs (subagent/dialogue)** | `start_subagent` and `start_dialogue` — work that outlives the reply |

Two tools are always available and always safe: `calculate` and `current_time` —
no network, no disk.

### Who watches a video

The **Provider** row of the Video (YouTube) group says who watches. `gemini` is
Google's own API. `openrouter` is the same Gemini models behind the key the
`openrouter` mode uses: `Enter` on **Model** then offers the models the gateway
lists as taking video, the Gemini family first — it is the one that takes a
YouTube link — and there is **no default model**, so until you choose one the
tool answers from the video's page alone: its title, channel, length and the
author's description. Each provider keeps its own model and key, and switching
retypes nothing.

What differs is the price of a part. Google cuts out the segment that was asked
for and charges for the segment. Through the gateway **the whole video is read
and charged, whatever part is asked for**: the answer is about the part and says
that the whole was read, and the length ceiling measures the whole video — one
longer than the ceiling is refused even for a minute of it. Any link you paste
is fine: the app hands over the video's own address and nothing else of the
link ([install.md §4.4](install.md)).

### Confirmation

`Ctrl+P → Tools → Confirm dangerous calls` shows exactly what a call is about to
do before it runs: the tool, the arguments, the files it touches, whether it
reaches the network. `Enter` allows it, `Esc` declines — and declining does not
derail the answer; the assistant is told and carries on.

### Background work

`call_subagent` delegates inside the reply; `start_subagent` and
`start_dialogue` return at once and keep working after it. `F7` (`/tasks`) lists
every run across every chat — running with the round and tool it is in, or landed
with how it ended — with the follow-ups a companion has planned and the app's own
quiet work below. `Enter` opens a run's transcript, `P` its chat, `F6` stops a
running one; on a follow-up, `Enter` opens its conversation and `Del` twice
cancels it.

When a background run finishes, the assistant reports it in the chat if that chat
is open and idle; otherwise the chat list marks it unread.

## 7. Reading aloud

`/tts` speaks the last message, `/tts N` the last N, `/tts all` the whole
conversation; `/tts stop|pause|resume` controls playback. The voice is configured
at `Ctrl+P → Model/server → Speech` — OpenAI, Gemini, the OpenRouter gateway, or
any OpenAI-compatible speech server — and your own lines can have a different
voice. Code, tables and
diagrams are skipped with a short spoken note instead of being read out
character by character.

In the `openrouter` mode the Speech tab has lists where the other modes have
text fields: `Enter` on **Model** offers the gateway's speech models, each with
the number of voices it lists, and `Enter` on **Voice** or **User voice** offers
the voices of the model chosen. The key is the one the other tabs use in that
mode. There is **no default model** — until you choose one, `/tts` says speech is
not configured — and no price in the list, because the gateway publishes that
number without its unit. Most of its models need a voice and a few speak
without one; when something is missing or refused, the chat shows the gateway's
own sentence, for instance *"An explicit voice is required for this TTS
provider."* The speech rate is sent, and a model may ignore it
([install.md §4.3](install.md)).

**A companion's own voice.** A profile can have a voice of its own, so two
companions do not sound alike: Settings → Profiles → *Voice (mode)*, in the
*Persona* group. A voice is the provider's name for it, so the row edits the voice
for the speech mode in force (`openai`, `gemini`, …) and keeps each mode's apart; in
the `openrouter` mode `Enter` lists the speech model's voices. Empty, the profile
speaks in the Speech tab's voice. **Speak replies**, in the profile's *Companion*
group, reads every reply aloud as it arrives in the conversation you have open —
as if you typed `/tts` — in that voice. A reply you stopped is not read; your next
message's reply, `/tts stop` or switching chats cuts one short. If speech is not
set up, the chat says so once rather than after every reply.

## 8. Settings worth knowing

Eight sections, and these four are the ones worth a visit early.

- **Model/server** — four tabs: Assistant, Impersonation, Embeddings, Speech.
  Besides the mode and the model: **Sessions (parallel streams)**, how many
  requests may be open at once (1 by default, so the assistant and its subagents
  take turns), and **Parallel tool calls**, how many of one reply's read-only
  calls run together (1 on a local engine, 4 on a cloud). A tab whose mode is
  `openrouter` also has **Name the app to OpenRouter**: on by default, it adds
  two headers naming the application — never you — to every request the app
  makes to the gateway, and off, they are not sent. The same switch closes the
  video group in **Tools** while its provider is `openrouter`.
- **Sampling** — temperature and the rest, per slot. What the screen offers is
  narrowed to what the endpoint says it accepts, so a gateway stops showing knobs
  it would silently drop; in the `openrouter` mode the list starts from what the
  gateway reads at all and narrows to what the chosen model takes. The **reply
  budget** (`max_tokens`) covers the model's
  reasoning as well as its answer; the default of 16384 is deliberately generous
  for that reason.
- **Memory** — the context window when the engine cannot say it, what triggers
  compaction, and how much of the knowledge base a turn may pull in.
- **Interface** — the colour mode and its theme (below), whether the model's
  name is shown in the reply header, which notes the token counter carries
  (reasoning and the cache share, both on by default), the background of a code block shown
  without highlighting (`inverted` by default, or `normal` — just its text
  between the ```` ``` ```` lines),
  how tall the input box grows before it scrolls (6 rows by default; it never
  takes more than half the window), mouse capture, and the OSC 52 clipboard mode
  for sessions over SSH.

Every field has a description under the list, and `/` opens a search across every
setting in every section.

**Colour mode.** *Interface → Colour mode* decides whose colours the screen is
drawn in:

- **system** (the default) — the app draws its colours over your terminal's own
  background. *Theme* is then `auto` (follows the terminal), `dark` or `light`.
- **full** — the app paints the whole window itself: the theme's background and
  text colour, matched to each other. *Theme* is then `dark`, `light`, or a
  theme of your own (§8.1). Every text colour keeps a contrast of at least
  4.5:1 against the background and against a selected row, and body text at
  least 7:1, whatever your terminal is set to. A translucent terminal
  background is covered in this mode.
- **monochrome** — no colours, and no bold, italic, underline or dimmed text:
  your terminal's own two colours and nothing else. There is no theme to pick.
  For a terminal that draws attributes badly, an e-ink screen, a screen reader.

Each mode remembers its own theme, and all apply as you pick them. What the
app cannot paint is the margin your terminal keeps around its grid and the
cursor's colour — those stay the terminal's.

**What monochrome looks like.** Whatever the colours used to say is said with a
mark instead:

| You see | It means |
|---|---|
| `[Enter]`, `[F1]` | a key, in the hint bars |
| `[Hotkeys]` among the tabs | the tab you are on |
| `› ` before a row of a list | the selected row |
| `▌` / `║` / `│` down the left of the feed | your message / the assistant's / a system message or a note |
| `█` down the left of a message | the message a search or a link brought you to |
| `**bold**`, `*italic*`, `~~struck out~~`, `` `code` `` | the markdown as it was written |
| `<chat://1a2b3c4d>` | a reference that opens a chat (`Ctrl+L` lists them) |
| `●` among `○` in the chat list | the chat that is open |
| `(a title)` in the chat list | a chat shown only because one of its sub-agent transcripts matched the filter |
| *command* in the input box's title | `Enter` will run the text as a command, not send it |

Two things are shown in **reverse video**, because they have no mark to be
shown with: the text you selected in an input box, and the matches of a search.
Misspelled words are not marked in this mode; `Ctrl+G` on a word still offers
the corrections.

**`NO_COLOR`.** If the `NO_COLOR` environment variable is set (to anything but
an empty string — [no-color.org](https://no-color.org)), the app starts in
monochrome. A colour mode you chose in the settings wins over the variable: pick
*system* there and the colours are back, with the variable still set.

### 8.1. A theme of your own

A theme of the **full** colour mode is a small file in the `themes` folder of
your data: `themes/mine.json` is the theme `mine`, and it appears in
*Interface → Theme* after `dark` and `light`. Start from a copy of a built-in
one:

```bash
mindfork themes export dark -o data/themes/mine.json
```

**The file** is a list of colours, each `#rrggbb`. Only the background,
`canvas`, has to be there:

```json
{
  "canvas": "#002b36",
  "text": "#a8b4b4",
  "accent": "#b58900"
}
```

| Colour | What it is |
|---|---|
| `canvas` | the background of the whole window |
| `text` | what you read: messages, settings, lists |
| `muted` | secondary text — dates, counts, descriptions of keys |
| `user`, `assistant`, `tool` | the bar down the left of a message, and whatever belongs to that side |
| `user_soft`, `assistant_soft`, `tool_soft` | the `YOU` / `ASSISTANT` headers, a tool's name |
| `accent` | headings, links, search matches, the counter while a reply is written |
| `success`, `warning`, `error` | states: ready, connecting, failed |
| `keycap_bg` | the backdrop of **every selected row**, and the pill under a key |
| `keycap_fg`, `keycap_danger` | the key's name on that pill; the same for a key that deletes |
| `border`, `border_focus` | the frame of a panel, and of the one that has the focus |
| `code_text`, `code_comment` | plain text and comments inside highlighted code |

Code is coloured from the others — keywords take `accent`, strings `success`,
numbers `warning`, functions `user`, types `assistant`.

**What you leave out is chosen for you**, to stay readable on *your*
background. The app takes the built-in theme's colour — the dark theme's on a
dark background, the light theme's on a light one — and makes it lighter or
darker, keeping its hue, only as far as it takes: text at least 4.5:1 against
the background and against a selected row, body text 7:1. So a theme can be
one line — `{ "canvas": "#f4ecd8" }` is a complete sepia theme.

**What you write is kept as written.** If a colour of yours is hard to read on
your background, the app tells you and leaves it alone — it is your theme.

**See what came out** without starting the app:

```bash
mindfork themes check mine
```

```
Theme mine — data/themes/mine.json
Canvas #002b36, a dark one: the colours the file does not name start from the built-in dark theme.

  role            colour   from        on canvas  on selection  floor
  keycap_bg       #18404c  the canvas  1.34       —             —
  text            #93a1a1  the file    5.61       4.18          7.0   BELOW
  muted           #9fa5ac  fitted      6.04       4.50          4.5   ok
  tool            #dcaf61  built-in    7.40       5.51          4.5   ok
  ...

Below the floor: 1 — text (the file). A colour the file names is drawn as written.
```

*the file* — your colour; *built-in* — the built-in theme's, which already
reads well here; *fitted* — the built-in theme's, moved to read well; *the
canvas* — made from your background. `mindfork themes check` with no name
checks every theme, and with a path checks a file anywhere.

**Good to know**

- Themes are read when the app **starts**: restart it after editing one.
- A name is letters, digits, `-` and `_`. `dark` and `light` are taken.
- A key that starts with `_` is yours — a note to yourself, a credit.
- A file the app cannot read as a theme is skipped, and the log says why; the
  other themes still load. If the theme named in your settings is missing, the
  app draws `dark` and keeps the name, so the theme is back when its file is.
- Themes are part of a backup.
- What a theme cannot colour is what the app does not draw: the margin your
  terminal keeps around its grid, and the cursor.

## 9. Keys

`F1` lists these **by screen**, which matters — a few chords mean different
things on different screens. The highlights:

| Key | Action |
|---|---|
| `Enter` / `Shift+Enter` | send / line break (`Alt+Enter` — the same break for terminals without the kitty protocol, KDE Konsole among them; `Ctrl+J` works in every terminal, macOS's Terminal.app included; the input box's footer names the one your terminal can deliver) |
| `Esc` | stop a running generation; otherwise back — to the chat list, or to the search results or the conversation you came from (the status bar says which) |
| `Ctrl+Q` / `F10` | quit (or type `/exit`) |
| `F1` | help and about |
| `Ctrl+P` | settings |
| `F3` | the self-model screen |
| `F7` | the tasks screen |
| `Ctrl+N` | new chat (with a profile picker) |
| `Ctrl+R` | regenerate the last reply |
| `Ctrl+E` | take back the last exchange (your text returns to the input box) |
| `Ctrl+U` | impersonation: the model writes your next message |
| `F5` | copy the conversation to the clipboard |
| `F6` | on an open background subagent transcript: stop that run |
| `Ctrl+F` | find in this conversation; in the chat list — switch search between titles and message content |
| `Ctrl+G` | in the input box: spellcheck suggestions; in the chat list's content search: the matching messages themselves |
| `Ctrl+T` / `Ctrl+O` | collapse/expand "thoughts" / tool calls; `Ctrl+O` in the chat list — fold/unfold the selected chat's subagent transcripts |
| `Shift+←/→/↑/↓`, `Ctrl+A` | select text / select all |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | copy / cut / paste |
| `Ctrl+K` | clear the input (`Ctrl+Z` brings it back) |
| `Ctrl+Z` / `Ctrl+Y` | undo / redo in the input box |
| `Ctrl+B` | emoji picker |
| `Ctrl+L` | follow a `chat://` reference the assistant wrote |
| `Ctrl+←/→`, `Ctrl+Backspace/Delete` | move by words, delete a word — `Alt` (Option on a Mac) does the same |
| `Home` / `End` | a ladder: first the on-screen row, then the whole line |
| `Ctrl+W` | toggle mouse capture: wheel scrolling ↔ native text selection |
| `PageUp` / `PageDown` / wheel | scroll the feed |

> The mouse wheel and text selection share one terminal mechanism, so capture is
> a toggle (`Ctrl+W`): off (default) — select text natively; on — the wheel
> scrolls the feed and selection needs `Shift`. `Ctrl` shortcuts are
> layout-independent: on Windows under any installed layout, elsewhere under
> Cyrillic.

> **On a Mac.** macOS takes `Ctrl+←/→` for switching Spaces, so words are Option+←/→
> there. Terminal.app breaks a line with `Ctrl+J` (its footer says so) and keeps Home,
> End and Page Up/Down for its own scrolling — add `Shift`; with *Use Option as Meta key*
> on (Settings → Profiles → Keyboard) `Alt+Enter` and the other `Alt` chords work too.
> A link there opens with Cmd+double-click. iTerm2 and Ghostty take `Shift+Enter`; in
> iTerm2, `Shift`+Page Up/Down scroll iTerm2 rather than the conversation. In Ghostty, Cmd+→ arrives as `Ctrl+E`, which
> deletes the last exchange: turn on *Confirm regenerate / delete* (Settings →
> Interface), or Ghostty's keys off ([install.md](install.md#macos-apple-silicon--a-preview)).
> The `F` keys need `fn`.

> **When a key does nothing.** `mindfork keys` shows each key you press as the app
> receives it, and what the input box does with it; `Ctrl+Q` quits. When a terminal
> keeps a chord for itself or sends something else, its lines say so. Paste them
> into a report.

> **Copying over SSH.** The clipboard the app writes belongs to the machine it
> runs on — over SSH that is the server, and a headless one has no clipboard at
> all. So a copy is also handed to *your* terminal's clipboard using OSC 52,
> automatically when the session looks remote
> (`interface.clipboard_osc52`: `auto`/`always`/`off`). The terminal never
> confirms it, so the app says the text was *sent* rather than that it arrived,
> and not every terminal supports the sequence — GNOME Terminal, Terminal.app and
> JupyterLab's terminal do not; VS Code's, kitty, alacritty, Windows Terminal,
> iTerm2, wezterm and tmux do. A conversation over ~75 KB is too large for it,
> and the app says so instead of sending half of one.

## 10. Commands

Typed straight into the input box.

| Command | What it does |
|---|---|
| `/file attach <path>` · `/file remove <name\|#N>` · `/file list` | attach a text file to this chat / detach it / list attachments |
| `/file open <name\|#N>` · `/file folder` | open one of this chat's files in the system (document types only — anything else opens its folder) / open the chat's files folder |
| `/image attach <path\|url>` · `/image remove <name\|#N>` · `/image list` | stage an image for your next message / unstage one / list what is staged |
| `/image paste` | stage the image on the clipboard — a screenshot needs no file |
| `/project attach <directory>` · `/project detach` · `/project status` | attach a code project to this chat / detach it / show what is attached and what the command slots hold |
| `/project build-cmd [line]` · `run-cmd` · `test-cmd` | set the command the assistant may build / run / test with — or, with no argument, show it |
| `/project clear build\|run\|test` | unset one of those commands (the assistant then has no such tool at all) |
| `/changes` (`F4`) | what the assistant changed in the attached project, as a diff, with `r` to put one file back |
| `/tasks` (`F7`) | every background run, the follow-ups a companion planned, and the app's own quiet work; `Enter` opens a transcript, `P` its chat, `F6` stops a running one, `Del` twice cancels a follow-up |
| `/notes` | the assistant's notes about you; `Enter` opens one, `E` edits its text, `T` its tags, `Del` deletes it (asks first), `/` searches |
| `/rag add <path> [-r]` · `/rag remove <path>` | index a file or directory into the knowledge base / remove it |
| `/rag list` · `/rag rebuild` | show the store's sources / reindex after changing chunking |
| `/reindex` | re-embed everything with the current embedding model |
| `/local` | look for Ollama and LM Studio on this computer and offer what answers |
| `/compact` | fold the older part of the chat into a rolling summary |
| `/tts` · `/tts N` · `/tts all` | read the last message aloud / the last N / the whole conversation |
| `/tts stop` · `pause` · `resume` | control playback |
| `/exit` · `/quit` | leave the app |

**Every key also has a typed route**, for terminals embedded in a host that keeps
the keys for itself:

| Command | Same as | What it does |
|---|---|---|
| `/settings` · `/self` · `/tasks` · `/help` · `/chats` | `Ctrl+P` · `F3` · `F7` · `F1` · `Esc` | the screens |
| `/new [profile]` | `Ctrl+N` | new chat; a name picks the profile (a prefix is enough) |
| `/rename [title]` · `/clone` · `/copy` | `F2` · `Ctrl+D` · `F5` | this chat: rename, clone, copy the conversation |
| `/autotitle` | `Ctrl+R` in the list | have the model title this chat |
| `/regen` · `/retry` · `/takeback` | `Ctrl+R` · `Ctrl+E` | regenerate the last reply / take back the last exchange |
| `/continue` | — | resume the last interrupted reply from where it stopped — local and external engines, Gemini, and Claude up to the 4.5 generation; DeepSeek in the `deepseek` mode, in a conversation without tool calls; through OpenRouter or another gateway, only Gemini models and Claude up to 4.5 |
| `/impersonate [text]` | `Ctrl+U` | the model writes your next message, continuing the text you give it |
| `/greeting [N]` | — | in a chat that holds only its greeting: the profile's next greeting, or the `N`th — a character card's other openings |
| `/bridge [pair\|unpair]` | — | this profile's Telegram bridge: what it is doing / a code to pair your account / forget the account |
| `/stop` | `Esc` | cancel the running generation |
| `/find [text]` · `/search <text>` | `Ctrl+F` · `Ctrl+G` | find in this conversation / find messages across every chat |
| `/links` | `Ctrl+L` | follow a `chat://` reference the assistant wrote |
| `/thoughts` · `/toolcalls` · `/mouse` · `/emoji` | `Ctrl+T` · `Ctrl+O` · `Ctrl+W` · `Ctrl+B` | the feed and the input box |
| `/subagents [expand\|collapse\|stop [n]]` | `Ctrl+O` in the list · `F6` | this chat's subagent transcripts; `stop [n]` ends the n-th background run |
| `/profile list` · `/profile new [name]` · `/profile delete <name>` | `Ctrl+N` · `Ctrl+D` in settings | your companion profiles |
| `/profile fork [name]` | — | a copy of this chat's profile with its memory, without its chats |
| `/profile system [text\|clear]` · `/profile greeting [text\|clear]` | the settings editors | this chat's profile: its system message and greeting |
| `/impersonation list` · `new [name]` · `delete <name>` | `Ctrl+N` · `Ctrl+D` in settings | the personas `Ctrl+U` writes as |
| `/impersonation use <name\|default>` · `/impersonation system [text\|clear]` | the settings editors | which persona this chat's profile impersonates as, and that persona's own text |
| `/self clear` | `Ctrl+K` twice in the self-model screen | wipe what the assistant thinks about itself and about you |
| `/export [md\|json] [path]` | — | save this conversation to a file |

Deleting a profile and clearing the self-model always ask first, even though the
keys they mirror do not: a command names its target by word, and a shortened name
could match a profile you did not have in mind.

Only text editing has no command — a command is typed *in* the input box, so it
cannot act on the box's own contents.

## 11. When something goes wrong

**"It does nothing when I start it."** Two causes, and the app tells you which.
Started where there is no terminal, it prints one line and exits with code 2.
Started when another copy is already running on the same data folder, it says so
and also exits 2. On Windows, a refusal printed into a console the app owns alone
— a double-click from Explorer — waits for `Enter` before closing, so the message
does not vanish with the window.

**"Window too small".** The window is smaller than the screen in front can be
drawn in, and the line under the title says both sizes: the one the window has
and the one it needs — 20×3 for the chat, 46×12 for the settings. Enlarge the
window and the screen is back as you left it. Until then the keys are off, so
that nothing is sent or confirmed unseen: `Ctrl+Q` quits, and where the line
names `Esc` — something opened over the chat is what does not fit — `Esc`
closes it.

**The chat looks stripped down.** In a window of ten rows or fewer the chat
gives up its frame a piece at a time to keep four rows of the conversation on
screen: first the status bar's second row, then the input box's border, then
the feed's — its token counter then moves to the status bar — and below six
rows the status bar itself. A window of five rows is four rows of the
conversation over the `❯` prompt. The keys a border used to
name are all on `F1`; the search's counter stays at the right end of its row.
Eleven rows and up look as they always did.

**A key I know is missing from the bottom line.** The key hints under a screen
take at most a third of the window; in a small window the ones that do not fit
are left out, `F1` and `Esc` last. Every key still works, and `F1` lists them
all.

**The engine says "not configured".** The empty chat lists the ways to connect
one; `Ctrl+P → Model/server` is where they are. A managed server needs both a
binary and a GGUF file — with a binary alone it deliberately starts nothing,
because a `llama-server` without a model comes up in a mode where every message
fails.

**A reply stops mid-sentence.** It hit the reply budget
(`Ctrl+P` → Sampling → `max_tokens`, where the rows are named after the wire
fields) or the context window. `/continue` resumes it
where the engine supports that.

**The interface is still there but nothing answers.** That is a failed
background task; the app ends the session with a line naming the log file rather
than leaving you with a live interface and a dead core. The logs are under
`logs/` in the data folder — `F1` → About names it under *Locations*, and
`mindfork stats` prints the data folder on its second line — next to the binary
for the portable build, the user's folder for an installed copy. The interface
owns stdout, so nothing useful is ever printed to the terminal.

**Keys that do not arrive.** A terminal embedded in a host may keep them: VS
Code's takes `Ctrl+P`, `Ctrl+E`, `Ctrl+F`, `F1`, `F3` and `F5`; a browser tab
never delivers `Ctrl+N` or `Ctrl+T`, and `Ctrl+W` closes the tab. Every one of
them has a typed command (§10).

**Anything else.** The log under `logs/`, and — if it looks like a defect —
[an issue](https://github.com/vshylov/mindfork-rs/issues) with the version
(`F1` → About, or `mindfork --version`), your OS **and terminal emulator**, the
engine mode and model, and what you expected instead. A security problem goes
through the private channel in
[SECURITY.md](../SECURITY.md) instead.
