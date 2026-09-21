# The mindfork manual

How to use the application, once it is installed and a model is connected.

Three neighbours to this document, so you know when to leave it:

- **[install.md](install.md)** — installing, where the data lives, connecting an
  engine, the Python sandbox, MCP servers, speech, backups. Setup, not use.
- **`F1` inside the app** — the reference: every key **by screen**, every
  command, the licence, the legal texts and the third-party components. It is
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

### Connecting a model

Open settings — `Ctrl+P` (or type `/settings`) — and go to **Model/server**. The
**Mode** field cycles through the six modes with `←/→`:

| Mode | What it is |
|---|---|
| `managed` | the app launches its own `llama-server` child process from a binary and a GGUF file on this machine |
| `external` | any OpenAI-compatible server you already run: llama.cpp, vLLM, LM Studio, Ollama, or a gateway such as OpenRouter or LiteLLM |
| `openai` · `gemini` · `claude` · `grok` | the cloud providers, each with its own key and model |

For a cloud mode, fill in the API key (it is stored encrypted and bound to this
machine, and never shown back) and then the **model**: press `Enter` on that
field and the app asks the provider what it serves, then offers the list. Type to
filter it, `Ctrl+R` asks again, and the first row of the list is the old way —
typing a name by hand. Where a provider says which of its models are for chat,
the list is narrowed to those; where it says nothing, everything it lists is
offered with the likely ones first.

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
`Shift+Enter` (or `Alt+Enter`) makes a line break instead.

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
| Tasks | `F7` | `/tasks` | every background run across every chat, and the app's own quiet work |
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

### The self-model

`F3`. A structured picture the assistant maintains of itself and of you: a
summary, goals, traits, and an observation narrative. It is updated between turns
by the assistant itself, not by a background job you have to trust — and
`/self clear` wipes it if you want a clean slate. It is per profile.

### Notes

The assistant writes notes for itself and links them into a graph; a note can
supersede another, and the recall path is semantic rather than keyword. You do
not manage them; you can read them through the tools' output, and the graph is
what makes a long acquaintance feel continuous.

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
| **Video (YouTube)** | `youtube_watch` — needs a Gemini key, and works whatever your chat engine is |
| **MCP servers** | tools from any Model Context Protocol server; a double opt-in, and a server that changes its tool list has to be re-confirmed |
| **Background runs (subagent/dialogue)** | `start_subagent` and `start_dialogue` — work that outlives the reply |

Two tools are always available and always safe: `calculate` and `current_time` —
no network, no disk.

### Confirmation

`Ctrl+P → Tools → Confirm dangerous calls` shows exactly what a call is about to
do before it runs: the tool, the arguments, the files it touches, whether it
reaches the network. `Enter` allows it, `Esc` declines — and declining does not
derail the answer; the assistant is told and carries on.

### Background work

`call_subagent` delegates inside the reply; `start_subagent` and
`start_dialogue` return at once and keep working after it. `F7` (`/tasks`) lists
every run across every chat — running with the round and tool it is in, or landed
with how it ended — with the app's own quiet work below. `Enter` opens a run's
transcript, `P` its chat, `F6` stops a running one.

When a background run finishes, the assistant reports it in the chat if that chat
is open and idle; otherwise the chat list marks it unread.

## 7. Reading aloud

`/tts` speaks the last message, `/tts N` the last N, `/tts all` the whole
conversation; `/tts stop|pause|resume` controls playback. The voice is configured
at `Ctrl+P → Model/server → Speech` — OpenAI, Gemini, or any OpenAI-compatible
speech server — and your own lines can have a different voice. Code, tables and
diagrams are skipped with a short spoken note instead of being read out
character by character.

## 8. Settings worth knowing

Eight sections, and these four are the ones worth a visit early.

- **Model/server** — four tabs: Assistant, Impersonation, Embeddings, Speech.
  Besides the mode and the model: **Sessions (parallel streams)**, how many
  requests may be open at once (1 by default, so the assistant and its subagents
  take turns), and **Parallel tool calls**, how many of one reply's read-only
  calls run together (1 on a local engine, 4 on a cloud).
- **Sampling** — temperature and the rest, per slot. What the screen offers is
  narrowed to what the endpoint says it accepts, so a gateway stops showing knobs
  it would silently drop. The **reply budget** (`max_tokens`) covers the model's
  reasoning as well as its answer; the default of 16384 is deliberately generous
  for that reason.
- **Memory** — the context window when the engine cannot say it, what triggers
  compaction, and how much of the knowledge base a turn may pull in.
- **Interface** — theme, whether the model's name is shown in the reply header,
  mouse capture, and the OSC 52 clipboard mode for sessions over SSH.

Every field has a description under the list, and `/` opens a search across every
setting in every section.

## 9. Keys

`F1` lists these **by screen**, which matters — a few chords mean different
things on different screens. The highlights:

| Key | Action |
|---|---|
| `Enter` / `Shift+Enter` | send / line break (`Alt+Enter` — the same break for terminals without the kitty protocol, KDE Konsole among them; the input box's footer names the one your terminal can deliver) |
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
| `Home` / `End` | a ladder: first the on-screen row, then the whole line |
| `Ctrl+W` | toggle mouse capture: wheel scrolling ↔ native text selection |
| `PageUp` / `PageDown` / wheel | scroll the feed |

> The mouse wheel and text selection share one terminal mechanism, so capture is
> a toggle (`Ctrl+W`): off (default) — select text natively; on — the wheel
> scrolls the feed and selection needs `Shift`. `Ctrl` shortcuts are
> layout-independent: on Windows under any installed layout, elsewhere under
> Cyrillic.

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
| `/tasks` (`F7`) | every background run and the app's own quiet work; `Enter` opens a transcript, `P` its chat, `F6` stops a running one |
| `/rag add <path> [-r]` · `/rag remove <path>` | index a file or directory into the knowledge base / remove it |
| `/rag list` · `/rag rebuild` | show the store's sources / reindex after changing chunking |
| `/reindex` | re-embed everything with the current embedding model |
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
| `/continue` | — | resume the last interrupted reply from where it stopped — local and external engines, Gemini, and Claude up to the 4.5 generation |
| `/impersonate [text]` | `Ctrl+U` | the model writes your next message, continuing the text you give it |
| `/stop` | `Esc` | cancel the running generation |
| `/find [text]` · `/search <text>` | `Ctrl+F` · `Ctrl+G` | find in this conversation / find messages across every chat |
| `/links` | `Ctrl+L` | follow a `chat://` reference the assistant wrote |
| `/thoughts` · `/toolcalls` · `/mouse` · `/emoji` | `Ctrl+T` · `Ctrl+O` · `Ctrl+W` · `Ctrl+B` | the feed and the input box |
| `/subagents [expand\|collapse\|stop [n]]` | `Ctrl+O` in the list · `F6` | this chat's subagent transcripts; `stop [n]` ends the n-th background run |
| `/profile list` · `/profile new [name]` · `/profile delete <name>` | `Ctrl+N` · `Ctrl+D` in settings | your companion profiles |
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
`logs/` next to the binary — the interface owns stdout, so nothing useful is ever
printed to the terminal.

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
