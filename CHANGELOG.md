# Changelog

All notable changes to the project are tracked in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
the project follows [semantic versioning](https://semver.org/).

Sections: **Added** (new functionality), **Changed** (to existing functionality),
**Fixed** (bugs), **Removed**, **Data** (storage formats and migrations — most
important to users: an update should never lose data), **Security**.

Detailed engineering history lives in the [CLAUDE.md](CLAUDE.md) log.

## [Unreleased]

### Added

- **Backups can be password-protected.** Give a password with
  `mindfork backup --password …`, or set it once in settings → "Data" → "Backup
  password" and every copy is encrypted with it — including the ones the app
  makes by itself before a restore or a data-format update. The archive uses
  standard AES-256, so 7-Zip or WinZip still open it with the password if you
  ever need the files without the application. Restoring accepts an encrypted
  copy *and* an old unencrypted one, with nothing to switch; a wrong password is
  refused before anything is replaced, and in a terminal `mindfork restore`
  simply asks for it. The stored password is encrypted and tied to this
  computer, just like a cloud API key — so **write it down somewhere else**: it
  does not move to another machine, and without it an encrypted copy cannot be
  restored. Use a long passphrase (the zip format's password protection is weak
  against a short one), and note that file names and sizes inside the archive
  stay visible — it is their contents that are encrypted.

- **`Ctrl+Z` on the settings screen takes an edit back** (and `Ctrl+Y` re-applies
  it). Settings are saved the moment you change them, so until now a wrong
  keystroke was final — you had to remember what it was and set it back by hand.
  Now one press restores the previous value, and the cursor **jumps to the field
  it just reverted**, so you can see what changed even if it was in another
  section. Cycling a switch past the value you wanted comes back in a single
  press, not one per step. Undo covers what you can edit in a visit to the
  screen; while a text field is open for editing, `Ctrl+Z` still undoes your
  typing, as before. A few things stay outside it by nature: an API key (the app
  never keeps it in the screen), creating or deleting a profile, and confirming
  an MCP server's tool list.

### Fixed

- **A setting's hint is no longer cut off mid-sentence.** The panel at the
  bottom of the settings screen had room for three lines, and longer hints — the
  API key, MCP servers, speculative decoding — simply ran past it, with the part
  that explained what to actually do left unread. The panel is now as tall as
  the longest hint in the section needs, and stays that height while you step
  through its fields, so the list underneath doesn't shift about.
- **Zig code blocks are highlighted — and twenty-one other languages with them.**
  A ` ```zig ` block came out as flat text on a grey background, and so did
  `toml`, `dockerfile`, `powershell`, `swift`, `typescript`, `kotlin`, `scss`,
  `sass`, `graphql`, `terraform`, `elixir`, `solidity`, `julia`, `nix`, `dart`,
  `protobuf`, `cmake`, `nginx`, `vue`, `svelte` and `nim`: the syntax set the
  app shipped with covers only what Sublime Text's defaults did, and none of
  these were in it. Real grammars for all twenty-two now ship with the app.
  `jsonc`/`json5` and `v` highlight too, through the closest grammar the app
  has. Nothing to configure, and startup is no slower — the grammars are
  compiled in when the app is built.
- **Editing a server setting and taking it back no longer reloads the model for
  nothing.** A change and its undo both asked for a restart, and the app then
  restarted the server with the settings it was already running — on a local
  model that means unloading and reloading a multi-gigabyte file. The app now
  compares against what each server is actually running, so a restart only
  happens when something really differs. The same applies to MCP servers, where
  a needless re-apply killed and respawned their processes.

### Changed

- **Code blocks without highlighting are drawn as a neat rectangle.** Their
  background used to follow the ragged right edge of the text, so a block read
  as a stack of bars of differing length. Now every row of the block — the
  ` ``` ` fences included — is filled to one width, blank lines inside it too,
  with a blank column along the right edge so the text doesn't run into it.
  The block is sized to its own content and does not stretch across the panel
  (the same rule tables follow); a line too long for the panel is wrapped into
  the rectangle instead of leaving a ragged tail.

- **Moving around the settings screen no longer changes settings by accident.**
  `→` used to step from the section list into the parameters, so `→` and then `←`
  looked like the way in and back out — but `←` on a switch changes its value, and
  the first parameter of most sections is a switch (the server mode, the theme).
  The way back was a silent edit, applied at once and restarting the server. Now
  the model is: **the arrows change a value, `Enter` goes into the parameters,
  `Esc` goes back out** (a second `Esc` closes the screen), and `Tab` switches
  section without moving your focus. The hotkey line at the bottom now changes
  with your focus, so it always says what `Enter` and `Esc` will do, and each of
  the two panes has a marker in its title that turns green when it is the one
  listening to you. Note the two habits that change: `→` no longer enters the
  parameters, and leaving the screen from inside them takes two `Esc` presses.

- **`mindfork backup` and `mindfork restore` now compact the database.** `data.db`
  holds on to the space freed by deleted notes, `/rag remove` and chats whose
  attachment index went away — a backup now packs a compacted copy of it (a
  smaller archive), and a restore compacts what it unpacked, including archives
  made by older versions. A file that can't be read as a database is copied
  as-is, so an unreadable one still gets backed up; the backup never modifies
  the data it is copying.

### Added

- **Ask before a tool does something outside the app.** A new switch in settings
  → Tools → "Confirm dangerous calls" (off by default) makes the assistant stop
  and show you the call before it runs Python, writes a file, or calls an MCP
  server's tool: you see the code or the path it is about to use. `Enter` runs
  it, `A` runs it and stops asking about that tool until the answer is finished,
  `Esc` declines — declining does not throw away the answer, the assistant is
  told and carries on. Reading files, searching the web and the assistant's own
  notes are never asked about.

- **Search inside the conversation you have open** — `Ctrl+F` in a chat finds text
  in that chat: every match is highlighted at once, a counter shows which one you
  are on out of how many, `Enter` (or `↓`) steps to the next and `Shift+Enter`
  (or `↑`) back, `Esc` closes. Stepping lands on the **line** the match is on, not
  at the top of the message, which matters in a long answer. The message you were
  writing is left completely alone, and the query is remembered if you reopen the
  search in the same chat. Two things worth knowing: the search looks at what is
  actually drawn, so it also finds words inside "thoughts" and tool cards, and it
  cannot find text the renderer has reshaped (a formula, a diagram).

- **Search inside chats, not just their titles** — `Ctrl+F` in the chat list
  (`Esc`) switches the search box between titles and **message text**; the list
  narrows to the chats containing a match, ordered by the sort you already chose.
  Matching is by fragment, so part of a word finds the whole one and `C++`
  searches for itself; words shorter than three characters are ignored, and
  "thoughts" and tool calls are not searched. The index is a separate `cache.db`
  next to your data, filled in the background — deleting it is a safe repair (it
  is rebuilt on the next launch), and it is deliberately not part of backups.
  From there you can go on to the **messages themselves**: `Ctrl+G` opens a
  full-screen list of every matching message, grouped by chat, each shown with an
  excerpt around the match (highlighted), who wrote it and when. `Enter` on a hit
  opens that chat **right at that message**, which is marked in the feed so you
  can see where you landed, with **the word you searched for highlighted inside
  it** — the highlight marks what is actually drawn on screen, so it can also
  land in that message's thoughts or a tool card, and it will not find text the
  renderer transformed (a formula, a diagram). `Esc` from there goes **back to
  the results**, exactly as you left them — same selection, same scroll — so you
  can work through the hits one at a time; a further `Esc` goes on to the chat
  list with the search still running, and the status bar's `Esc` hint says which
  of the two it currently means. `Enter` on a chat in content mode also opens it
  at its first matching message now, instead of at the end of the conversation. A
  very broad query is capped at 200 hits, and the header says "showing N of M"
  rather than quietly truncating.

- **Input prefixes for the embedding model** — a new "Input prefixes" setting in
  the Embeddings tab (Model section): `none` (default), `e5`, `e5-instruct`. Some
  embedding families expect each input marked with its role (`query: ` /
  `passage: `); others, including the default bge-m3, expect bare text and score
  *worse* with a marker — so nothing is applied unless you select it, and when a
  model change is detected whose name looks like an e5, the notice simply says
  which convention it suggests. On a 40-document test the prefixes changed no
  ranking on e5 but widened the gap between a relevant and an irrelevant
  passage — most noticeably in the near-tie cases. Switching the setting counts
  as a change of embedding model: memory rebuilds itself, and `/reindex` is
  offered for the knowledge base.

- **Attaching text files to a chat** — three new input-box commands:
  `/file attach <path>` adds a file to the current chat, `/file remove <name|#N>`
  takes it away, `/file list` shows what is attached (they head the "Commands"
  tab of the help dialog, `F1`). An attached file's text is passed to the model
  with every message of that chat, so it can be asked about at any point in the
  conversation — and removing it genuinely takes it out of what the model sees.
  Attachable: any text file (source code, configs, logs — anything valid UTF-8),
  plus `.html`, `.pdf` and `.docx`, whose text is extracted the same way `/rag
  add` does it. A file's content is snapshotted when attached, so the
  conversation stays coherent even if the file later changes or is deleted.
  A large file is **not refused**: it is attached "by reference" — the prompt
  gets its name, size and the beginning, and the model reads the rest page by
  page on demand, so even a multi-megabyte file can be worked through without
  flooding the context. Such a large file is also indexed for **semantic search**
  in the background, so instead of paging through hundreds of pages the model can
  jump straight to the place it needs — the index belongs to that one chat and
  never mixes with the profile's knowledge base (`/rag add`). Indexing needs an
  embedding server; without one it is simply skipped, with a note, and everything
  else keeps working. Budgets are in the settings "Memory" section
  ("Attachments" group), and the status bar shows a `§ files: N (~tokens)` chip
  with what the attachments actually cost per message.
- **Changing the embedding model is now noticed and reported.** Vectors stored by
  one embedding model are meaningless to another, so on the first use of a new
  one the app says so and sets aside everything the previous model indexed —
  without deleting any of it. Memory (notes and self-observations) rebuilds
  itself as you keep using it; the search indexes of attached files and the
  knowledge base — your own data — are restored by `/reindex`, and until the
  knowledge base is rebuilt, search over it says plainly that it cannot compare
  its vectors. The check does not rely on the vector size, so it also catches a
  swap between two models of the same size and a model file replaced in place. On
  a first run there is nothing to compare against, so nothing is reported and
  nothing is touched.
- **`/reindex`** — a new input-box command that rebuilds every stored vector with
  the current embedding model in one pass: memory, the search indexes of attached
  files and **every** profile's knowledge base (one embedding server serves them
  all, so a model change affects them all at once). It re-embeds the text already
  stored, so it needs no source files on disk, repairs old entries whose file is
  long gone, and brings the search indexes of attached files back without
  re-attaching each one. Runs in the background with a progress banner and is safe
  to interrupt — whatever it has already rebuilt stays rebuilt, and running it
  again continues from there. Listed in the "Commands" tab of the help dialog
  (`F1`).

### Changed

- **Content arriving on its own no longer drags the feed back to the bottom.**
  If you have scrolled up to re-read something, a tool card, a service note or
  the assistant's next message landing mid-turn now leaves your position alone —
  previously anything arriving snapped the view straight back to the end, which
  made reading during a long answer nearly impossible. What *you* start still
  scrolls to the bottom, as it should: sending a message, opening a chat,
  starting a generation. Scrolling back down to the last line resumes following
  the tail as before.

- A change of the embedding model no longer discards anything: memory and the
  search indexes of attached files are set aside and re-embedded from the text
  already stored, rather than being dropped and rebuilt from scratch. So nothing
  has to be re-attached, and switching **back** to the previous model costs
  nothing at all.
- Switching to an embedding model with a different vector size (via
  `/rag rebuild`) still rebuilds the search index of files attached to chats from
  scratch — it was built by the previous model. Re-attaching a file rebuilds it,
  and so does `/reindex`; reading a file page by page is unaffected.

### Fixed

- **Pasted HTML no longer disappears from the conversation.** A message containing
  a block of raw HTML — a table copied out of a README, an answer a model wrote in
  HTML — rendered as an empty space: the whole block, prose and all, was dropped.
  Its text is now shown (tags stripped, cells reading across the line, `&amp;` and
  friends decoded, an `<img>` showing its alt text and address). Style sheets and
  scripts stay hidden, as does the markup itself — those are not part of what was
  written.

- **A server that starts after the app is now picked up on its own.** The
  readiness check ran once and then stopped, so starting the app before
  `llama-server` — the ordinary order of things for a local setup — left
  generation refused even after the server had finished coming up, until the app
  was restarted or an engine setting was touched. Servers are now re-checked
  continuously: every minute while healthy, every five seconds while unavailable,
  so a server that comes back is noticed within seconds and simply starts working.
  The same applies in reverse — a machine that goes down no longer keeps a green
  indicator. To avoid a nervous indicator, an available server is only reported as
  unavailable after three checks in a row fail, while a single successful check
  restores it immediately.

- **A managed server whose process dies is now reported instantly and restarted.**
  When the app launches `llama-server` itself and that process dies (a corrupt
  model file, out of memory), this is now noticed in a fraction of a second rather
  than on the next check, and the server is relaunched automatically — up to three
  times in five minutes, after which it is left alone and reported as unavailable,
  so a model that cannot load doesn't spin in an endless restart loop. A server
  running elsewhere is never restarted by the app — it isn't ours to restart — but
  it is watched, and it recovers by itself once it comes back.

- The embeddings indicator no longer reports "ready" for a server that cannot be
  reached. Its status was derived from the settings alone — a filled-in address
  was enough to light the chip green — so an embedding server on a machine that
  was switched off, or simply not running, still looked healthy, and the problem
  only surfaced later as an error from the first search over memory or the
  knowledge base. The app now checks the server the same way it already checked
  the chat server: the chip shows "connecting…" while the check runs, then either
  "ready" or the reason it is unavailable (the settings window spells the reason
  out). A cloud embedding provider has nothing to wait for and stays ready
  immediately, as before.

- Search over memory, the knowledge base and attached files no longer degrades
  in silence after the embedding model changes. Nothing ever re-embedded an
  existing note, so recall kept matching new queries against vectors from the old
  model; with a model of a different vector size it returned arbitrary notes
  outright, and the duplicate checks that keep memory from bloating stopped
  firing altogether. Knowledge-base search now refuses plainly, naming
  `/reindex`, instead of answering from vectors it cannot compare.
- The checks that decide when two pieces of memory say the same thing — duplicate
  notes and observations, near-identical traits — now follow the embedding model
  in use instead of being tuned to one particular model. Every model rates
  similarity on its own scale, so after a model change a fixed cut-off can drift
  into "nothing is ever a duplicate" or, just as bad, "everything is": on one of
  the models tested, memory would have been told that entirely unrelated traits
  meant the same thing. The app now measures the new model's scale once, when it
  first notices it, and shifts the cut-offs to match. Nothing changes for a setup
  that has not changed models, and if the measurement fails the previous
  behaviour is kept.
- Knowledge-base search results (`rag_search`) are readable again: found
  fragments are numbered and set apart from one another, and their text is shown
  exactly as it is in the source. Previously a fragment several lines long ran
  into the next one, and a heading inside a fragment was rendered as a heading of
  the reply itself, tearing the result apart — which happened with practically
  every `.md` file, since each of its fragments starts with its section heading.

## [0.9.4] — 2026-07-26

### Added

- **beautifulsoup4 in the Python sandbox** — `mindfork-rs sandbox setup` now also
  installs BeautifulSoup (with `soupsieve` for CSS selectors), so sandboxed code
  can parse HTML — a natural companion to the already-available `requests`. An
  existing sandbox picks it up by re-running the same command; everything already
  installed is skipped.

- **Custom names for the user and the assistant** — two new fields in the
  settings "Profiles" section ("Persona" group). When set, the name replaces the
  role headers in the chat feed (in caps: `GAIA` instead of `YOU`) and the labels
  when copying the conversation with `F5` (`Gaia:` instead of `User:`). Both are
  empty by default, which keeps the usual labels in the interface language; the
  names apply to the profile's existing chats immediately, so they can be changed
  at any time.

- **Impersonation profiles** — the user personas the model writes a message as
  (`Ctrl+U`) are now a list of their own, each with its own name and system
  message, instead of a single text field buried on the assistant profile. The
  "Impersonation" subsection of the settings "Profiles" section now edits exactly
  that list (`Ctrl+N` — create, `Ctrl+D` — delete), and an assistant profile picks
  which persona its chats use via the new "Impersonation profile" field. Several
  assistant profiles can share one persona; not choosing one keeps the previous
  behaviour (a shared default text).

### Changed

- **The project site `mindfork.io`** is now carried by the package metadata as
  well: the Windows installer shows it as the publisher link in "Apps &
  features" (support and updates there now point at GitHub issues and releases
  respectively), and the Linux packages use it for `Homepage:`/`URL:`. Every
  GitHub Release page also gains a footer linking the site and the install guide
  as of that release. The site itself is not up yet — the repository stays the
  live destination for issues and downloads.

### Fixed

- **A newly created profile is now selectable right away.** `Ctrl+N` in the
  settings "Profiles" section created the profile, but the settings screen never
  learned about it — it couldn't be selected or edited until the app was
  restarted. The new profile is now delivered to the screen and selected
  automatically; deleting a profile likewise refreshes the list immediately.
- **Ctrl shortcuts now work under any keyboard layout on Windows** — previously
  only the standard Russian one was handled (via a built-in table), so under a
  Greek, Hebrew, Georgian, Bulgarian, Armenian, … layout `Ctrl+Q`, `Ctrl+P` and
  the rest simply did nothing. The physical key is now resolved through the
  layout itself, which also covers the Russian letters sitting on punctuation
  keys (which the table never had) and non-standard variants such as Russian
  Typewriter. On Linux, Cyrillic works as before, plus whatever the terminal
  itself handles (the GNOME Terminal family copes with every layout on its own).

### Data

- Role names (`Profile.character_names`) used to be seeded with placeholder values
  that nothing ever displayed. Now that they are shown, those seeds are cleared once
  at startup, so the feed and the `F5` export keep using the interface language's
  labels; a name you chose yourself is left alone. New profiles start with the fields
  empty. No schema-version bump.
- The legacy impersonation system message stored on an assistant profile
  (`Profile.impersonation_system_message`) is migrated once at startup into a named
  impersonation profile ("«profile name» (impersonation)") and linked back. The
  migration is idempotent and the old field is left on disk untouched, so nothing is
  lost and a downgrade still finds its data. No schema-version bump (the new fields
  are additive).

## [0.9.3] — 2026-07-24

### Changed

- **The help dialog (`F1`/`?`) was redesigned in KDE/Qt style** — instead of one
  long hotkey list, it's now a modal window with a logo in the header and tabs:
  **"About"** (author, version, links to the site/repository/crate), **"Shortcuts"**,
  **"Commands"** (the `/rag …`/`/tts …` commands, split out of the shortcuts list),
  **"License"** (MIT text), and **"Components"** (third-party dependencies with
  versions and licenses). Switching tabs — `Tab`/`←→`, scrolling — `↑↓`/`PgUp`/`PgDn`,
  closing — `Esc`; the last open tab is remembered. The window and dialog title is
  `mindfork v<version>`.
- **The project's source language is now English** — all documentation, code
  comments and internal diagnostics were translated from Russian, in
  preparation for going open source. This is not an i18n rollback: user-facing
  text stays localizable, and Russian remains a fully supported interface and
  agent language.
- **New profiles get English default character names** ("You"/"Assistant"/
  "System") instead of Russian ones. Existing profiles keep their stored names
  (no migration), and the names stay editable in the profile settings.

### Fixed

- **Several user-facing strings ignored the selected language** and always
  appeared in Russian: server connection errors, `/rag` command errors and the
  usage hint, the "conversation copied" confirmation, agentic-loop notices
  (tool disabled, time limit, round limit), the interlocutor description used
  for impersonation, and the Python-mode labels in the settings. All of them
  now follow the interface or agent language, as appropriate.
- **A failed `fs_read` was rendered as a highlighted code block** in the feed
  for profiles in any language other than Russian — the failure was detected by
  matching Russian text.

## [0.9.2] — 2026-07-23

### Added

- **Chat message text-to-speech (TTS)** — the `/tts` command in the input box:
  `/tts` reads the last message aloud, `/tts N` — the last N, `/tts all` — the
  whole conversation, `/tts stop` — stops it, `/tts pause`/`/tts resume` — pause
  and resume (handy for long text). A separate voice can be set for the user —
  then `/tts all` reads the user's and the assistant's lines in different voices.
  Code blocks, mermaid diagrams, tables, and formulas are not read aloud — a short
  note ("code block skipped") is spoken instead; "thoughts" and tool calls are
  never voiced at all. The provider is chosen separately from the chat engine, on
  the new "Speech" tab of the "Model" section: the OpenAI cloud (default,
  `gpt-4o-mini-tts`, voice `onyx`), the Gemini cloud, or any third-party
  OpenAI-compatible TTS server; the cloud API key is the same one already entered
  for chat. The same tab holds the voice, tone instructions, speed, and behavior
  (voice roles; stop on chat switch / on generation start). Playback is pipelined
  (the next chunk is synthesized while the current one plays). While speech is
  playing, a "♪ speaking" icon is shown in the status bar. If there's no sound
  card (e.g. an SSH session), the app reports this and keeps working.
- **Entering API keys directly in settings** — environment variables are no
  longer required. In the cloud modes (OpenAI / Gemini / Claude) a new "API key"
  field appeared: `Enter` opens a masked input (characters shown as `•`), `Del`
  removes the key. The field only shows a status — "set (this computer)" or
  "not set"; a stored key can't be viewed or copied, editing it means re-entering
  it. One key serves chat, impersonation, and embeddings for a given provider. The
  previous approach — the "API key (env)" field with an environment-variable name —
  remains as a fallback and is used when no key has been entered.
- **Application icon on Windows**: `mindfork-rs.exe` now carries its own icon —
  visible in Explorer, the taskbar, Alt+Tab, and shortcuts. The installer got the
  same icon and a logo in the wizard header.
- **Application-menu entry on Linux**: the packages (deb/rpm/pkg.tar.zst) install
  a `.desktop` file and theme icons, so mindfork-rs shows up in the menu with an
  icon (launches in a terminal).
- **Logo in the help dialog**: the help overlay (`F1`) shows the mindfork mark
  using terminal glyphs — the glyph and the word `mindfork` side by side, in the
  header above the hotkey list. It's drawn only when the window is large enough
  to fit both the mark and the whole list.

### Data

- **Stored API keys in `settings.json`**: keys entered in the app go into a new
  `api_keys` section — **encrypted and tied to this computer**. The settings file
  can still be moved between machines: on a new one the keys need to be re-entered,
  and moving back to the original machine makes them readable again. The format is
  extended without a migration — old settings are read as-is.

### Security

- The key is stored in the config only in encrypted form (Windows — the system
  DPAPI, Linux — a key derived from the machine identifier) and is never shown in
  the UI: a copy of the settings file, a backup, or cloud sync does not expose the
  key. This scheme does not protect against programs running under your own
  account on the same computer — the same limitation browser password managers
  have.

### Changed

- **The terminal window title on Windows** now shows the program name —
  "mindfork" (visible in the taskbar and Alt+Tab).
- **Unified selection style in the spellcheck suggestions popup (`Ctrl+G`)**: the
  selected option is no longer highlighted by inverting the whole line — it's now
  highlighted the same way as in the chat list, settings, and the self-model
  screen: a soft background with a green bar on the left.

### Fixed

- **The emoji popup (`Ctrl+B`) no longer leaves a mark on screen**: after closing
  the window, a colored "leftover" remained where the selected emoji had been —
  half of a wide glyph that the terminal didn't clear. Such a frame is now
  redrawn in full, without flicker.
- **Stray space after `❤️` when returning from the chat list / self-model screen
  (`F3`)**: a feed line with such an emoji would drift one column to the right
  and only straighten out after scrolling. Switching screens now redraws the
  frame in full. The same drift could appear in the input box when editing text
  to the left of an emoji — also fixed.
- **Emoji feed artifacts during streaming**: while a reply with emoji was
  streaming (and after adding a note — e.g. "Conversation copied to clipboard"
  via `F5`), leftover pieces of the previous frame stuck around on older
  terminals and disappeared only after scrolling. Now a feed content change also
  triggers a full redraw, not just scrolling.
- **Scrolling a feed with emoji like `🗂️`/`🕸️` no longer shifts lines**: during a
  full redraw, the tail of such a line used to drift one column to the right —
  a neighboring wide emoji could vanish and the feed's right border could shift.
  The full redraw was reworked so it no longer touches the second half of a wide
  glyph.
- **The spellcheck suggestions popup (`Ctrl+G`) no longer leaves a highlight
  trace**: the "➕ Add to dictionary" item could keep half of its selection
  background after the popup closed — the same defect as in the emoji popup.
- **Emoji popup: rows no longer drift**: two emoji in the grid (`❤️`, `✌️`) took
  up an extra terminal column beyond what was accounted for, which could make a
  neighboring emoji disappear and shift the popup's border. They were replaced
  with the equal-width `💖` and `🤞`; the rest of the set is unchanged.

## [0.9.1] — 2026-07-18

### Added

- **RAG indexes HTML** (`.html`/`.htm`): readable text is extracted from the page
  (article paragraphs, without nav/header/footer/scripts) and added to the
  knowledge base alongside `.txt`/`.md` via the `/rag add` command. No new
  dependencies.
- **RAG indexes PDF and DOCX** (`.pdf`/`.docx`): plain text is extracted from the
  document and added to the knowledge base via `/rag add` (a scanned/image-only
  PDF with no text layer yields nothing — that's expected). DOCX is parsed with
  existing tooling; a pure-Rust `pdf-extract` dependency was added for PDF.
- **Chunk-level RAG indexing progress**: when adding a large file, the banner now
  shows "chunks N/M" and advances as embedding proceeds, instead of freezing
  until the whole file is done (embedding now runs in size-limited batches).
- **RAG search removes duplicates across sources**: if the same passage was
  indexed from different files, it's shown only once in search results (the
  model doesn't get a repeat). Relevance order is preserved.
- **Self-model auto-consolidation ("sleep")**: every N assistant replies, a
  background task tidies up the self-model on its own — merges duplicate
  observations, shrinks an oversized description, links contradictions (the
  model silently calls its own tools; the chat is untouched). A separate
  "Auto-consolidation (every N)" toggle lives in the "Memory" → "Self-model"
  section (0 — off, by default; only works in profiles with self-model tools
  enabled). A quiet "self sleep" indicator shows in the status bar while it
  runs. During consolidation/reflection the model also sees where a paragraph of
  its self-description semantically duplicates an already-recorded observation,
  so it can move the repeat into an observation or merge the description with
  it. It also keeps the interlocutor's interest list "current" by dropping ones
  that haven't been confirmed in a while.
- **Plugins: MCP server tools** — connect external tools from the Model Context
  Protocol ecosystem (files, git, GitHub, databases, …) without rebuilding the
  app: servers are described in `settings.json` (the `mcp` section), their tools
  are enabled via profile toggles grouped under "Plugins (MCP)"; server statuses
  and full tool descriptions are visible in settings. Cancelling with Esc is no
  longer blocked by a long call to any tool. See
  [docs/install.md §4.2](docs/install.md).
- **Import from other apps**: the `mindfork import <file>` command reads the
  documented, neutral [mindfork-import](docs/import-format.md) format (JSON with
  profiles and chats). The file is produced by an external converter from the
  source app's format; the import is idempotent (running it again updates the
  same profiles/chats instead of creating duplicates), and a file from a newer
  format version is rejected.
- **Mermaid diagrams in the feed**: ` ```mermaid ` blocks (flowchart and
  sequence) render as text art instead of printing the source. On any failure —
  broken syntax, a diagram too wide, an unsupported type — the block falls back
  to showing the source, as before. The "Mermaid diagrams" toggle is in the
  "Interface" section (on by default); in old-terminal compatibility mode the
  diagram is drawn with ASCII glyphs.
- Official release builds for **Windows** and **Linux** are published on GitHub
  Releases (archives with the binary and docs, plus a `sha256sums.txt` checksum
  file).
- **Spellcheck dictionaries** (`data/dictionaries/`) were added to the release
  archives — spellcheck works out of the box after extraction.
- **Auto-detecting the interface language from the OS locale** on a fresh
  install with no `settings.json`/`defaults.json` (a bare portable archive, a
  deb/rpm package): a Russian locale → Russian UI, otherwise English. An
  explicitly set language (in `settings.json` or `defaults.json`) still takes
  priority.
- **Linux packages**: releases now ship `deb` (Debian/Ubuntu), `rpm` (Fedora),
  and `pkg.tar.zst` (Arch). They install the app system-wide (data goes to the
  standard OS user folder); dictionaries are picked up from the install
  directory.
- **Windows installer** (`setup.exe`, Inno Setup): a per-user install requiring
  no administrator rights, a wizard for choosing the app language and data
  location, a bilingual UI (Russian/English). The installer is unsigned
  (SmartScreen will warn).

### Changed

- Spellcheck now also finds dictionaries in the **portable layout next to the
  binary** (`data/dictionaries/`), not only in the data directory. With a system
  install (data in the user's folder), dictionaries placed next to the program
  are now picked up — previously spellcheck silently didn't work in that mode.
- The install-defaults file `defaults.json` is now read correctly even with a
  UTF-8 BOM (an installer or an editor may add one) — startup no longer fails
  because of the byte-order mark.

### Fixed

- Mermaid diagrams no longer **flicker while a reply is streaming**: until the
  server finishes the block (no closing ` ``` ` yet), it's shown as source, and
  only then rendered as a diagram — previously a partially received block would
  alternate between rendering as a truncated diagram and falling back to source
  on every chunk.
- A corrupted `settings.json` is no longer silently overwritten with defaults —
  the app reports an error instead of losing the file.
- A single corrupted chat file no longer blocks startup: it's skipped with a
  warning logged, and stays on disk for manual recovery.

### Removed

- The `import-lamellama` command: LameLLaMA import is now handled by an external
  converter that emits a mindfork-import file, plus the `import` command
  (running the old command prints a hint about the replacement). Previously
  imported data is unaffected.

### Data

- The schemas of stored data (`settings.json`, `profiles.json`, `chats/*.json`,
  and the `data.db` SQLite database) are now versioned, and the app checks their
  compatibility on startup. A migration framework is now in place: future format
  changes will update the data automatically (DB migrations run in
  transactions — an interrupted update won't leave the database in a partial
  state), and a backup is created before migrating.
- Data created by a **newer** version of mindfork is no longer read "as best it
  can": the app will report that you need to update or restore a backup
  (protection against silent corruption when rolling back to an older version).
- A schema-version manifest was added to backups; when restoring a backup made
  by a newer app version, `mindfork restore` prints a warning.

### Security

- MCP servers are enabled via a **double opt-in** (a master switch, off by
  default, plus a per-tool toggle in the profile); a server's tool catalog is
  **pinned on first approval** — if a server changes its tool set/descriptions
  after an update, they're unavailable to the model until reconfirmed
  (protection against tool substitution). Secrets are passed to servers only as
  environment-variable names; `.bat`/`.cmd` commands are forbidden (BatBadBut).
- Updated `anyhow` to 1.0.103 (fixes RUSTSEC-2026-0190 — unsoundness in
  `Error::downcast_mut`). Added a weekly dependency audit (`cargo-deny`).

## [0.9.0] — 2026-07-15

First tracked release. The project completed the entire M0–M9 plan plus
extensive post-M9 work; below is a summary of features by track (detailed
history is in the CLAUDE.md log).

### Added

- **Chat with local and cloud models.** Local Gemma 3/4 and Qwen 3.5/3.6 via
  llama.cpp `llama-server` (managed subprocess or external), plus cloud APIs:
  OpenAI (Responses), Google Gemini (native `generateContent`), and Anthropic
  (Claude, Messages) — a single mode selector, the key stored as an
  env-variable name.
- **Isolated profiles**, multi-chat, input-box drafts, chat auto-naming,
  regenerate/delete/edit of the last exchange, copying the conversation (`F5`).
- **Client-side agentic loop with tools**: introspection (editing the system
  message and sampling), notes, RAG (a knowledge base with `/rag` commands),
  web search, `fetch_url`, a calculator, date/time, files, a sub-agent,
  "thoughts" (CoT).
- **Agent self-model** (`F3`): self-description, goals, a user model, an
  observation narrative; manual editing + auto-reflection. **Notes
  connectivity**: semantic search, a link graph, supersession with a "scar",
  consolidation ("sleep"), cross-organ links (notes ↔ observations ↔ RAG).
- **Impersonation** (`Ctrl+U`) — the model writes a line on the user's behalf.
- **Python sandbox** (Wasmer/WASIX) for `python_exec` with isolation; assets
  are installed with `mindfork sandbox setup` (numpy/pandas/requests out of the
  box).
- **Multi-language support**: agent language (prompts/tools), interface
  language, external locales (`data/locales/*.json`) and new languages without
  a rebuild; all CLI text lives in locale bundles.
- **UI**: a custom markdown renderer (tables, LaTeX, code highlighting), a
  custom multiline input box (selection, undo/redo, mouse, emoji), themes
  (auto/dark/light), an old-terminal compatibility mode, spellcheck,
  scrollbars, a settings screen with field groups and search.
- **CLI**: backup/restore (`backup`/`restore`), import from LameLLaMA (.NET),
  locale-bundle export.
- **CI** (GitHub Actions): lint + tests on Linux and Windows; a pinned
  toolchain.

### Data

- Storage: JSON (config/profiles/chats, atomic writes + `.bak`) and SQLite
  (notes/RAG/self-model, sqlite-vec, per-profile isolation). Schema format is
  v1; schema versioning and migrations are formalized in later releases.

[Unreleased]: https://github.com/vshylov/mindfork-rs/compare/v0.9.4...HEAD
[0.9.4]: https://github.com/vshylov/mindfork-rs/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/vshylov/mindfork-rs/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/vshylov/mindfork-rs/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/vshylov/mindfork-rs/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/vshylov/mindfork-rs/releases/tag/v0.9.0
