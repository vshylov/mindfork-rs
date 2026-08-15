# Exporting a conversation to a file

Status: **research — forks pending the user's decision.**
Date: 2026-08-15.

## 1. What and why

A conversation can be copied to the clipboard (`F5`/`/copy`) and nowhere else.
The roadmap has asked for saving it to disk (Markdown/JSON) since the chat-
management list was written; two things now push it up:

- **The clipboard is not always reachable.** OSC 52 fixed the SSH case, but
  JupyterLab's terminal drops the escape and its pty is server-side, so there is
  *no* route from a conversation to the user's own machine
  ([osc52-clipboard.md](../history/osc52-clipboard.md) §2). A file on the server
  is a route: the notebook interface can open and download it.
- **A copy is transient and capped.** OSC 52 refuses past 74 994 bytes and a
  system clipboard holds one thing at a time; a long conversation is exactly
  what someone wants to keep.

## 2. What already exists

Almost all of it, which is why this is small.

- **The formatter.** `features::chat_export::format_conversation(title,
  messages, &CopySettings, names, loc)` walks the conversation, labels roles
  (custom names honoured), skips system/tool messages, and folds "thoughts",
  tool arguments and tool results in per `CopySettings`. It is pure and already
  tested; the clipboard copy is its only caller.
- **What a copy includes** is already a user setting (`config.copy`, the
  "Interface" section): text only by default, optionally thoughts / tool
  arguments / tool results.
- **A documented JSON shape for conversations already exists**:
  `mindfork-import` v1 ([import-format.md](../import-format.md)), read by
  `mindfork-rs import <file>`. A chat carries `key`, `profile_key`, optional
  explicit `id`, title, timestamps, system message, character names, and
  messages of `{role, text, thoughts?, timestamp?}`. **It does not carry tool
  calls** — that is the one thing an export in this format would lose.
- **Precedents for writing a file**: `backups_dir()` next to the data (and a
  `backup [output]` CLI subcommand), `locales-export <code> <output>`.
- **Precedent for a command that names a path**: `/file attach <path>`,
  `/image attach <path>`.

## 3. Design

**One command, `/export`** (§5 F1), on the chat screen, next to the commands the
typed-routes track added. It reuses the existing formatter, writes a file, and
answers with the **full path** — the path is the point of the feature in the
JupyterLab case, where the next step is opening that file from the notebook
interface.

**Markdown through the same walk, not a second one.** The clipboard's plain
text (`User:` / `Assistant:` labels) is not good Markdown. Rather than a
parallel formatter — the duplication seam this project keeps paying for — the
existing walk takes a `Style` (`Plain` for the clipboard, `Markdown` for the
file), which changes only how a role label, a "thoughts" block and a tool block
are decorated (`## User`, a fenced block, a `<details>`-free indented block).
The message text itself is already Markdown: it is what the feed renders.

**Attachments and images are not included**, and the file says so where they
were, rather than dropping them silently: a picture in a conversation is
exactly what someone re-reading the export would notice missing.

## 4. Open questions this design does not settle

The chat list's own selection (`F5` copies the *selected* chat there) could take
an export key too; deferred to F7 rather than assumed.

## 5. Forks

- **F1. Surface.**
  (a) **A TUI command, `/export [md|json] [path]`** — you export the
  conversation you are looking at, and it joins the command family that already
  covers every other action. *(recommended)*
  (b) Also a CLI subcommand (`mindfork-rs export <chat-id> …`) — scripting and
  bulk, but there is no way to name a chat outside the TUI except by UUID, and
  the whole data directory is already copyable with `backup`.
  (c) A key as well — nothing spare is left that a host does not claim, and the
  typed-routes track made the command the primary route anyway.
  User's decision: —
- **F2. Formats.**
  (a) **`md` (the default) and `json` in the `mindfork-import` v1 format**,
  with the tool-call loss stated in the note that reports the write and in the
  docs. Re-importable, so export/import become a pair rather than a one-way
  street, and the explicit `id` field means a re-import lands on the same chat.
  *(recommended)*
  (b) `md` only — nothing is lost and nothing is promised; JSON waits for a
  format that can carry tool calls.
  (c) `md` + a raw dump of the stored chat — lossless but undocumented, and the
  same file is already on disk under `data/chats/<id>.json`, which a user can
  copy without our help.
  User's decision: —
- **F3. Where the file goes.**
  (a) **Bare `/export` writes `exports/<date>-<slug>.md` under the data
  directory** (beside `backups/`), and an explicit path argument overrides it;
  either way the note prints the full path. A generated name cannot collide
  (it carries the timestamp), and the bare form is the one that works when you
  do not know what the filesystem looks like — the JupyterLab case. *(recommended)*
  (b) Require a path always — explicit, but hostile in exactly the case that
  motivated this.
  (c) Default to the working directory — surprising for a portable app whose
  data lives beside the binary.
  User's decision: —
- **F4. What the Markdown includes.** (a) **Reuse `config.copy`** — one mental
  model, "what a copy includes", already in the settings *(recommended)*;
  (b) always include everything (thoughts and tools), since a file is an archive
  rather than a paste; (c) a second setting.
  User's decision: —
- **F5. An existing file.** (a) **Refuse and say so**, naming the path
  *(recommended — overwriting someone's export silently is the kind of loss this
  project avoids elsewhere)*; (b) overwrite; (c) auto-suffix `-2`.
  User's decision: —
- **F6. The Markdown shape.** (a) **`# title`, `## role` headings, thoughts and
  tool blocks as fenced code** *(recommended: renders anywhere, survives being
  pasted into an issue)*; (b) reuse the clipboard's plain text verbatim under a
  `.md` name — no new code at all, but `User:` lines are not Markdown.
  User's decision: —
- **F7. The chat list's selection.** (a) **Defer** — `/export` covers the open
  chat, and the list can grow its own route when someone wants it
  *(recommended)*; (b) do it now, mirroring `F5` there.
  User's decision: —
- **Considered and rejected here:** HTML/PDF (a renderer's job, and the app
  already has one for the terminal only); embedding images as base64 in the
  Markdown (it would multiply the file size for something most viewers show as
  a wall of text); exporting *every* chat at once (that is `backup`).

## 6. Test plan

- **Unit**: the Markdown style's shape (headings, fenced thoughts/tools) and
  that `Plain` is unchanged byte-for-byte — the clipboard's existing tests are
  the regression net, and a shared walk is only safe if they stay green; the
  filename slug (spaces, punctuation, non-Latin titles, an empty title, a title
  longer than the filesystem tolerates); the JSON export parsed back by
  `features::import` in the same test — a round trip, which is the whole claim
  of F2a; the refusal on an existing file; the note naming the full path.
- **Gate**: the export directory is created on demand and confined to the data
  root when the path is generated (the path-confinement rule the tools already
  follow).
- **Live**: not required (no engine, memory or tool path). The acceptance pass
  is manual and one machine is enough: `/export`, open the file.

## 7. Scope and documentation

One stage. On completion: spec §11.2/§11.7, README (the command tables),
CHANGELOG, `docs/journal/ui-screens.md` or `ui-input.md` (whichever the command
lands in), `docs/import-format.md` (a line saying the app can now *emit* it),
and the roadmap item closes — along with a note on the JupyterLab entry, which
this partly answers.
