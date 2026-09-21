# Changelog

All notable changes to the project are tracked in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
the project follows [semantic versioning](https://semver.org/).

Sections: **Added** (new functionality), **Changed** (to existing functionality),
**Fixed** (bugs), **Removed**, **Data** (storage formats and migrations — most
important to users: an update should never lose data), **Security**.

Detailed engineering history lives in the [docs/journal/](docs/journal/) log,
split by subsystem.

## [Unreleased]

## [0.11.0] — 2026-09-21

**From a bare Linux machine to a local model that answers, in one line.** A new
command installs the Python sandbox and llama.cpp and writes the model, its
projector, the embedding model and the context into the settings — checked
before anything is downloaded, and started once to prove it loads; an install
script puts the app itself on the machine first. Written for a rented GPU box
that is new at every stop, and just as good on a desktop. And `llama setup` can
install CUDA builds on Linux, which it had been refusing.

### Added
- **`mindfork setup` — a working local engine from one command.** For a machine
  that is new every time (a rented GPU box, a container), and any other:
  `mindfork setup --sandbox --llama cuda-12 --model … --mmproj … --embed-model …
  --ctx 32768 --verify` installs the Python sandbox and llama.cpp, writes the
  model, projector, embedding model and context into the settings and switches
  the engine to managed mode — no settings screen in between. Paths and keys are
  checked before anything is downloaded; a step that fails does not stop the
  others, and running the same line again repeats only what is missing.
  `--set engine.managed.sessions=4` reaches any other setting, and `--verify`
  starts the servers once and reports how long they took to load, the context,
  whether the model takes images — or why it did not start.
- **An install script for Linux, and a recipe for a rented GPU box.** `curl -fsSL
  …/releases/latest/download/install.sh | sh` unpacks the portable build, checks
  it against the release's checksums, installs the one system library a bare
  image lacks, and can hand the rest of the line to `mindfork setup`. It is safe
  to run again: on RunPod, where a stopped pod loses everything outside
  `/workspace`, the same line puts back what is missing in seconds
  (docs/install.md §1, §3.4).
- **`mindfork llama setup --backend cuda-12` — a backend family.** llama.cpp
  renames its CUDA builds whenever it moves to the next toolkit (`cuda-13.3`
  became `cuda-13.4` inside two weeks), so a command you had saved stopped
  working. A family installs the one build of it on offer; one that fits two
  backends (`cuda`, `sycl`) is refused and both are named.

### Fixed
- **`mindfork llama setup` refused every CUDA build on Linux.** llama.cpp has
  published CUDA builds for Linux since mid-September 2026, but the command
  looked for their runtime archive under the name the Windows one has, found
  nothing, listed the backend as "no CUDA runtime published" and would not
  install it.
- **The installed llama.cpp's version read as a log line.** Recent builds print
  a log line ahead of their version, so `llama setup` showed that line instead —
  and the check that the binary you got is the build you asked for stopped
  checking, without saying so. The version is found by what it says now, and a
  binary that names no build number is reported as such.
- **`llama setup` could pick a half-uploaded build.** With no `--build`, it
  takes the newest build that actually has the backend you named, complete,
  instead of the newest with anything in it.

## [0.10.2] — 2026-09-20

**For data that lives on more than one computer.** A new command says which copy
is the newest and what each one holds that the other lacks — without restoring or
changing anything — and opening a chat no longer flashes the one you were leaving.

### Added
- **`mindfork stats` — which copy of your data is the newest.** With the data on
  several computers, one command now prints when the last message was written,
  how many chats and messages there are (and how many are deleted), the attached
  files, images, projects, notes and the knowledge base — for the data on this
  computer, or for a backup archive without restoring it
  (`mindfork stats copy.zip`, with `--password` for an encrypted one). It only
  reads: nothing is created or unpacked, so it is safe while the app is open, and
  an encrypted archive is never written out decrypted. The summary ends with a
  fingerprint: two computers that print the same one hold identical data.
- **`mindfork stats --compare` — what each copy holds that the other lacks.** The
  newest copy is not always the most complete one. Take a snapshot on the other
  computer (`mindfork stats --json > laptop.json`, a small file with no message
  text in it) or use a backup archive of it, and `--compare` prints a verdict —
  identical, this copy has everything, the other has everything, or each holds
  something — and the lists behind it: chats only here, only there, continued
  here or there, and **diverged** (one chat continued on both computers). Chats
  are compared by the ids of their messages, not by counts or dates, so a chat
  continued on two machines is never reported as merely "newer there".

### Changed
- **`restore` asks for the password on stderr**, so redirecting a command's output
  no longer hides the question. And when the password that fails is the one saved
  in the settings, the message says that and points at `--password`, instead of
  "wrong backup password" for a password you never typed.

### Fixed
- **Opening a chat no longer flashes the previous one.** Picking a chat in the
  chat list showed the conversation you were leaving for a fraction of a second
  before the one you asked for. The list now stays on screen until the chat is
  ready, so the switch is a single step — and the same goes for a new chat, a
  clone, a search hit and a run opened from the tasks screen. A clone that is
  refused now says so in the list instead of in the chat behind it.

## [0.10.1] — 2026-09-19

**The first release that also goes to a package registry.** Nothing on screen
changed; what did is where the program can be installed from, and one line in
your shell profile if you read its logs.

### Added
- **`cargo install mindfork`.** The package is published to
  [crates.io](https://crates.io/crates/mindfork) as `mindfork` — the repository
  keeps its `-rs`, which says "written in Rust". It builds from source and
  installs the binary alone, so the spellcheck dictionaries are not part of it
  and the portable data directory lands next to the installed executable;
  [docs/install.md](docs/install.md) §1 says what that means and how to change
  it. The prebuilt archives, the Linux packages and the Windows installer carry
  the dictionaries and every licence text, and remain the recommended way in.

### Changed
- **The log filter is `mindfork`, not `mindfork_rs`.** `MINDFORK_LOG` takes
  crate names, and the crate was renamed for the registry — so a filter written
  as `MINDFORK_LOG=mindfork_rs=debug` no longer names anything in this program,
  and the lines it used to turn on stay off. Write
  `MINDFORK_LOG=mindfork=debug`.
- **The crypto stack that encrypts stored API keys moved up a generation**
  (`sha2` 0.10 → 0.11 with `hkdf`/`hmac` to match, on Linux). The derived key is
  byte-for-byte the one the old crates produced — pinned by a test against a
  vector measured on them — so keys stored by an earlier version keep opening.

## [0.10.0] — 2026-09-18

**The first public release.** Everything before this shipped to a handful of
people who knew where to look; this is the one a stranger downloads. Most of the
work behind it is invisible on screen and only shows up when something would
otherwise have gone wrong: a Windows binary that starts on a clean machine, file
tools that cannot reach outside the folder you give them, a sandbox with no route
to your own network, the licence of every dependency travelling with every
download, and a release that is checked against this changelog before it is
built. What you *will* notice: you now pick a model from the provider's own list
instead of typing its name, a thinking model's answer is no longer cut off in the
middle, and there is a manual.

### Added
- **A manual.** [docs/manual.md](docs/manual.md) is the document the project did
  not have: how the app is actually used — the screens and how to move between
  them, what it remembers and where each memory lives, files and images and an
  attached code project, the tools and what each switch opens, the settings worth
  knowing early, the full list of keys and commands, and what to do when
  something goes wrong. [docs/README.md](docs/README.md) is the index over it and
  everything else.
- **Pick the model from a list, instead of typing its name.** `Enter` on a model
  field in settings — the assistant's, impersonation's or the embedder's — asks
  the provider what it serves and shows the list, with a filter line to type into
  and `Ctrl+R` to ask again; the first row of the list is still "type a name by
  hand". Where a provider says what a model is for, the list is narrowed to it
  (Gemini, xAI and Anthropic do; the embedder's list then holds embedding models
  only); where it says nothing, everything it lists is offered — ordered so that
  the ones whose names look like image, audio or embedding models sit at the
  bottom rather than the top, newest first within each group, with the models the
  provider is retiring marked. Nothing is ever hidden: whatever the provider
  lists can be chosen. Nothing is requested until you open the list,
  and if there is no list to show — no key yet, no answer, an address that serves
  no catalogue — the field works exactly as before and says why. The hint on those
  fields has stopped naming example models: it points at the list instead, which
  cannot go out of date.
- **The Windows installer can put `mindfork` on `PATH`.** A box on the "Additional
  tasks" page, **off** by default: with it, `mindfork llama setup` and the other
  commands the app suggests work from any terminal instead of only from the install
  folder. It is added once however many times you upgrade, and removed when you
  uninstall.
- **Every download now carries the licences of what it is built from.** The archives, the
  Linux packages and the Windows installer include `THIRD-PARTY-NOTICES.md` — the licence
  text of every Rust package the binary is built from, generated for that exact release —
  and the licences of the vendored syntax grammars under `licenses/syntaxes/`. The `deb`,
  `rpm` and Arch packages also carry the privacy policy, which until now only the archives
  and the installer had.

- **A gateway's own catalogue now configures the app.** Point `external` mode at a
  service that publishes one (OpenRouter and the like) and two things stop being
  your job: long chats are compacted automatically, measured against the window
  the catalogue gives for the model you named — before, that number had to be
  typed in or nothing was ever folded — and the sampling settings show only the
  parameters that endpoint actually takes, instead of the full local-model set of
  which it silently drops half. The same narrowing reaches the assistant's own
  `set_sampling` and the per-message record of what was applied. A typed context
  window still wins, and nothing changes against a local server, which publishes
  no catalogue.
- **A fresh install says how to connect a model.** An empty chat with no model
  configured lists the ways to start — a cloud provider in the settings (`Ctrl+P` or
  `/settings`), a local llama.cpp build through `mindfork llama setup`, or
  `mindfork demo` to look around — and sending a message there points at the settings
  instead of answering "LLM server is not configured".

### Fixed
- **A managed server would not start with "No mmap" ticked.** llama.cpp replaced
  `--no-mmap` with `--load-mode` and then removed the old flag entirely, so any
  current build — including the one `mindfork llama setup` installs — refused to
  launch with `error: invalid argument: --no-mmap`. The app now asks the binary
  which spelling it takes and sends that one, so the box works both on a current
  llama.cpp and on an older one you already have. The setting's row is labelled
  "No mmap" rather than by a flag name that depends on the build.
- **A thinking model's answer is no longer cut off in the middle.** The reply limit
  counts the model's own reasoning on every provider that charges for it that way,
  and the shipped limit of 2048 tokens was set before any of them did: measured on
  Gemini 2.5 Pro, a school arithmetic question spent 1697 tokens thinking, left 347
  for the answer, and the reply ended mid-explanation. The default is now 16384 —
  a limit you typed yourself is kept as it is.
- **A local model server is no longer started without a model.** With a llama.cpp
  build installed and no GGUF chosen, the app started the server anyway; it came up
  in a mode that answers "ready" while refusing every message, and could have
  downloaded a model from the internet on its own. It now says the model is not
  configured, and points at the setting — the same guidance an empty chat shows.
- **The app no longer keeps running after the part that answers has stopped.** If
  that part failed, the interface stayed up, accepted messages and answered none of
  them, for as long as you kept trying. It now closes with one line saying so and
  where the log is; what was written before the failure is saved.
- **A startup error no longer disappears with its own window.** Double-clicked from
  Explorer, mindfork gets a console that Windows closes the instant the program
  exits — so "another copy is already running", a broken settings file or a data
  folder that cannot be opened flashed by unread. In that window the program now
  waits for Enter. Started from a terminal, nothing changes.
- **On a clean Windows the app now starts.** The binary needed `VCRUNTIME140.dll`, part of
  the Visual C++ redistributable — a library Windows does not include and the installer
  did not bring — so on a machine that had never installed a C++ application it failed
  with a missing-DLL dialog before showing anything. The runtime is now built into the
  binary, which costs it 365 KB.

- **Starting mindfork without a terminal no longer hangs.** With its output redirected
  to a file or a pipe, or with no console, the app filled that output with screen codes
  and waited for keys forever. It now says it needs a terminal and exits with code 2,
  creating nothing; `mindfork demo` the same.
- **A failed chat search no longer writes what you searched for into the log**, which
  the privacy policy says never holds message text.
- **A chart from `python_exec` is no longer described to the model as shown when it was
  not.** On a model that cannot see images — a local server without its projector, or a
  text-only model behind a gateway — the chart's line in the result said it was shown
  while a note below said it was not. The line now says it was not shown and why; on a
  model that sees, it reads as before.
- **A chat with images keeps working after a switch to a model that cannot see them.**
  An image stays in the chat's history and was sent again on every turn, so once the
  model changed to one without vision — a local server started without its projector,
  or a text-only model behind a gateway — every message in that chat failed with
  "image input is not supported" or "No endpoints found that support image input". The
  images now go as a note telling the model it cannot see them, so it says so instead
  of guessing, and the chat mentions once that they were not sent. They are still in
  the chat and reach the model again after switching back to one that sees.
- **A text-only model behind a gateway no longer breaks a chat with an image.** In
  `external` mode against a service that publishes a catalogue (OpenRouter and the
  like), an image sent to a model that cannot see one was refused with "No endpoints
  found that support image input" — and, since the image stays in the chat's history,
  so was every later message in that chat. That includes a chart from `python_exec`,
  with nothing attached by hand. The app now reads from the catalogue whether the model
  takes images: `/image attach` refuses up front and says to pick a vision model, and a
  tool's picture is kept back with a note telling the model it has not seen it. For a
  model that does take images, attaching one no longer warns that the engine cannot say.
- **A reply the provider's content filter stopped now says so.** OpenAI, Anthropic,
  Gemini, OpenRouter and other OpenAI-compatible servers each report when their
  moderation cut a reply short, and the app showed the fragment as a finished answer
  — or, with OpenAI, as a reply that hit the length limit, suggesting `/continue`,
  which only meets the same filter again. The fragment is kept with a note saying
  why it ends there; impersonation says the same under the draft, and a subagent
  tells the assistant that delegated to it. Gemini's English "did not produce a
  response" line no longer lands inside the reply itself.
- **The thinking switch works through a gateway.** In `external` mode against a
  service that publishes a catalogue (OpenRouter and the like), turning thinking on
  did nothing unless a reasoning effort was also chosen, and turning it off did not
  stop a model that reasons by default — the gateway never read the field the app
  sent. It now receives the switch in its own spelling. Thinking is on in the default
  settings, so models there that reason only when asked (Claude Haiku 4.5, Sonnet 4.6,
  Gemma 4) now **do** reason — and spend the tokens — unless you switch it off. A model
  that must always reason is not asked to stop, and a local server is unchanged.
- **`/file open 1` works.** The number `/file list` shows can now be typed with or
  without its `#` — in `/file open`, `/file remove` and `/image remove` — where before
  a bare `1` was refused as "not attached" and only the file's name worked. A number
  no file has is answered with the numbers there are, and a file actually named `1`
  is still reached by that name.
- **A chart or screenshot from a tool reaches the model through a gateway.** On
  OpenRouter and similar services some providers refused a request whose tool
  result carried a picture, and some quietly dropped the picture — after which
  the model described an image it had never seen. Where the app is talking to a
  gateway, a tool's pictures now travel in a message of their own right after the
  tool's result, which every provider measured reads. A local server is sent
  exactly what it was before.
- **`/continue` no longer corrupts a reply through a gateway.** Through OpenRouter
  and similar services most models do not resume a partial reply — they start the
  answer over — and the app stored that new answer glued onto the old fragment
  (`…the capital of France isThe capital of France is Paris.`), with no error. On a
  gateway `/continue` now resumes only the models measured to continue there
  (Claude up to the 4.5 generation, Gemini) and says so for the rest, pointing at
  `/regen`; the note after an interrupted reply no longer offers `/continue` where
  it would refuse. A local server, which publishes no catalogue, is unaffected.
- **Auto-titles, compaction and impersonation work on a gateway whose model always
  reasons.** Those three run with reasoning turned off — and a service that cannot
  turn it off (OpenRouter with DeepSeek R1, for one) answered them with an error,
  so a chat never got its title, long conversations were never compacted and
  `Ctrl+U` did nothing, while ordinary replies kept working and nothing said why.
  The app now asks again without that setting and remembers the answer for that
  server. Nothing changes against a local server, which accepts the request.
- **A gateway's "thoughts" reach the feed.** Connecting `external` mode to a
  cloud gateway (OpenRouter and the services that copy its API) meant a
  reasoning model answered with its thinking invisible: the app looked for the
  field name a local server uses, the gateway sends its own, and the difference
  was silent — a model that thinks and a model that does not looked exactly the
  same. Both names are now read. Nothing changes for a local `llama-server`.

### Changed
- **The README is a front page again** — a third of its former length: what the
  app is, the screenshots, how to install it and connect a model, and links out.
  The keys and commands moved into the manual, where they are no longer competing
  with an introduction.
- **A second copy of mindfork exits with an error code** (2, the code it already
  uses when it refuses to start) instead of reporting success.
- **An interface language you add yourself falls back to English** for anything it
  does not translate. It used to fall back to Russian, from the days when the
  program's own texts were written in Russian; the translation template
  `mindfork locales export` writes is English for the same reason.

- **Setting up a gateway in `external` mode is written down** (install.md §3):
  the model name a gateway requires, the context window you have to type in
  yourself — without it long chats are not compacted automatically — and which
  of the sampling settings actually survive the trip.
- **`mindfork llama setup` sends no credentials.** A `GITHUB_TOKEN` in the environment
  used to be sent to GitHub; it is no longer read, since the privacy policy promises no
  variable is read for a key you did not name. Behind a shared address GitHub's limit of
  60 requests an hour can refuse the command — it resets within the hour.
- **The privacy policy now matches what the app does.** It was missing the llama.cpp
  download, the `/props` and `/models` requests the Grok cloud receives, several things
  sent to the embedding service, the pages `fetch_url` attaches whole, the environment an
  MCP server inherits, what a backup contains and the working folder of a Python run —
  each written from the code, in English and Russian, in the installer and on the site.
- **The file tools need a folder to work in.** `fs_read`, `fs_write` and `fs_list` used
  to reach the whole disk while the sandbox directory was empty; now they refuse until it
  is set, and tell the assistant which setting that is. If you had them on with the row
  empty, set it (Ctrl+P → Tools → Sandbox directory); `C:\` or `/` still gives the whole
  disk, as a choice rather than a default.
- **With confirmation on, a background sub-agent is not given dangerous tools.** It used
  to run them without asking, since no one is there to ask; now it plans without them.
- **`python_exec` says when its network is off.** The runtime's own prompt about the
  missing flag used to land in the output as if the script had printed it; the result now
  says the network is off, where the setting is, and what to do instead.

### Data

- **`settings.json` 2 → 3**: the default reply limit rises from 2048 to 16384
  (above). A limit left at the old default is raised; one you set yourself is left
  alone. As with every schema change, the file is backed up before it is migrated.

### Security
- **A TLS flaw in the library every network request goes through is fixed.** The TLS
  stack accepted handshake messages sent at the wrong encryption level instead of
  closing the connection, so a server could send in plain text what must be
  encrypted. Nobody on the network could alter or complete a handshake with it, and
  the fix is an updated dependency (rustls).
- **Two flaws in the XML parser that reads DOCX files are fixed** (`/rag add` on a `.docx`),
  and a withdrawn version of a cryptography package no longer goes into the build.
- **What builds a release is pinned and verified.** The package builder for the Linux
  packages used to be downloaded from an unsigned repository at whatever version it served
  that day, and every build step was named by a tag its author can repoint. Both are now
  fixed versions checked against a hash, and a release is published as a draft after its
  version has been checked against the source — so what you download is what this
  repository's code says it is.

- **The Python sandbox's network no longer reaches this machine.** Code the model runs
  could open connections to services on your own computer and to your local network — a
  router's page, a database, a company wiki — because the sandbox was given the host's
  network whole. It now reaches public addresses only, refusing the same ranges the web
  tools refuse; `tools.web_allow_private` lifts both, as before. Downloading data from the
  internet still works.
- **Code the model writes runs without your API keys.** The local Python interpreter and
  the code workspace's build and test commands started with the whole environment, so any
  key exported in the shell that launched mindfork was readable by a script the model
  wrote, or by a build script it had just edited. Those variables are now removed for
  those two; `llama-server` and MCP servers, which you chose to run, are unchanged.
- **On Linux your data folder is yours alone.** It was created with the system default,
  which usually lets every other account on the machine read your chats and the encrypted
  keys. The folder is set to owner-only at every start, including folders older versions
  created, and new files are written the same way.
- **A page `fetch_url` retrieves is read under a size ceiling** (32 MB), counted as it
  arrives, so a model-chosen address cannot fill memory with a multi-gigabyte download. A
  page past it is refused with a message saying so.
- **The assistant's file and project tools can no longer reach mindfork's own folders.**
  A file tool given a whole drive, or a project that contains the data folder, could read
  every stored key and conversation, or rewrite the settings so that something would run
  at the next start. Those folders are now refused whatever the tools were given.
- **A symbolic link can no longer carry a write out of the allowed folder.** A link whose
  target did not exist passed the folder check by its own name, and the write followed it
  outside — say, a cloned repository's link into an autostart folder.
- **The project tools no longer write under `.git/`**, where a hook would run at your
  next commit. Reading there still works.

## [0.9.9] — 2026-09-13

### Added

- **A memory limit for the local Python interpreter** — in Settings, the Python group in local mode now has a per-process memory limit (Windows only). A script that tries to take more gets a `MemoryError` instead of the machine's memory, and so does any process it starts. Off by default, and separate from the sandbox's limit, since the sandbox needs about a gigabyte just to start.

- **The local-interpreter mode exchanges files too.** Running Python on your own machine
  instead of in the sandbox used to mean the assistant's code could neither read this
  chat's files nor save anything for you. It now works exactly as the sandbox does: the
  call names the files it needs, each is copied in before the code runs, and what the code
  saves comes back into the chat's files and into `/file list`. Nothing else changes about
  the mode — it is still your machine, with your permissions and your network.

- **Opening a chat's file in the system**: `/file open <name|#N>` opens one of the files
  `/file list` shows — an attachment, a file the assistant saved, an image of the
  conversation — in whatever application the system uses for it, and `/file folder` opens
  the chat's files folder. Only document types (images, pdf, csv, txt, md, json, xlsx,
  docx) open directly; anything else — a script, a shortcut, an HTML page the assistant
  wrote — opens the folder it sits in instead, so nothing the assistant produced can run
  by being opened. The path is always printed, so a file the system will not open is a
  copy-paste away.

- **Files into the Python sandbox.** The assistant's code can now read this
  chat's files: it names them in the call, by the number `/file list` shows or by
  name, and each is copied into the sandbox before the code runs. Attachments go
  in as their text, stored files and pictures as themselves. The copies are the
  sandbox's own — changing one changes nothing here, and only what the code saves
  to `/w/out` comes back — so a chart or a table one call made is readable by the
  next. `/file attach` now also **keeps the file itself**: a workbook, a zip or
  any other binary is no longer refused (it is kept with the chat, and only code
  can read it), and a PDF, DOCX or web page keeps its original beside the text
  the assistant reads; the two are one entry in `/file list`, and removing it
  removes both — never your own file. `/file list` also shows the chat's
  pictures, with the same numbers the assistant uses. When *Confirm dangerous
  tool calls* is on, the confirmation now states which files would go into the
  sandbox, their sizes and whether it has network access.

- **Files out of the Python sandbox.** What the code saves directly into
  `/w/out` — a matplotlib chart, a CSV, a workbook — is kept with the chat, in
  `data/files/<chat-id>/`, and the call's card shows the folder and every file. A
  PNG or JPEG is also shown to the model, so it can check the chart it drew;
  *Show charts to the model* under Settings → Tools → Python turns that off. Up to
  10 files and 50 MB a call; a file of a name already saved gets a numbered copy
  rather than overwriting the first. `/file list` shows stored files after the
  attachments, `/file remove` deletes them, and backups include them.

- **More packages in the Python sandbox.** `mindfork sandbox setup` now also
  installs sympy (symbolic maths), networkx (graphs), lxml, pyyaml, regex,
  feedparser, openpyxl and pypdf (reading Excel and PDF files), tabulate
  (Markdown tables from pandas), pillow and matplotlib — about 28 MB more to
  download. An existing sandbox gets them by running `mindfork sandbox setup`
  again, which fetches only what is missing. matplotlib is installed and draws
  correctly.

- **`mindfork llama remove <id>`** deletes a downloaded engine build from
  `data/llama/` — by the name `mindfork llama installed` prints, or by a
  bare backend name while only one build of it is installed. If the
  settings point at that build the command refuses and says which fields
  do, so it cannot quietly leave them aiming at nothing; `--force` deletes
  anyway. It then reports how much was freed and what an empty
  *llama-server binary* field resolves to now. There is deliberately no
  `--all` and no automatic prune.

- **The engine binary is found, not just typed.** The *llama-server binary*
  field may now be left empty: the app takes the build `mindfork llama
  setup` installed last, or a `llama-server` sitting next to the
  application — so downloading one, or unpacking a llama.cpp archive beside
  `mindfork`, is enough to run in managed mode with nothing to type. A bare
  name like `llama-server` is looked for beside the application and then in
  `PATH`; a path you actually typed is used exactly as written. The same
  applies to the impersonation server and the embedder, which run the same
  binary.

- **The engine, downloaded.** `mindfork llama backends` lists the
  llama.cpp `llama-server` builds published for your OS and architecture —
  `cpu`, `vulkan`, `cuda-13.3`, `rocm-10.0`, … — with their download sizes,
  and `mindfork llama setup --backend <id>` fetches one into
  `data/llama/<backend>-<tag>/`. Every file is checked against the sha256
  the release publishes, an interrupted download resumes instead of
  starting over, and a CUDA build brings the CUDA runtime with it (without
  it the backend does not load and the server quietly runs on the CPU).
  When it is unpacked the command runs the binary and reports the build
  number and the compute devices it found, so a GPU backend with a missing
  driver says so instead of pretending. `--build <tag>` pins a build,
  `mindfork llama installed` shows what is on disk, and several builds can
  live side by side. The list of backends is read out of the release, so
  one that upstream adds or renames appears without an app update.
  `--set-binary` puts the path into the settings for you once the install
  succeeds — the assistant's engine always, the impersonation engine and
  the embedding server only if they had no path of their own; the engine
  mode is never switched behind your back.

- **A note when the server processes prompts slowly.** A local
  `llama-server` looks at its queue only between batches of prompt tokens,
  so a background request stopped or displaced during its prompt holds its
  slot for a whole batch — tens of seconds on a CPU. The app now reads the
  server's own timing of every prompt — the history compression's
  request, the background reflection and consolidation runs, a page
  summary a tool asked for, the automatic title and, on the shared
  engine, impersonation (whose prompt is the whole conversation, processed
  afresh) included,
  the prompts a server that kept its cache still processes whole — and, once per server session, says in the feed how
  fast prompts are processed, how long such a hold would be, and the one
  change to make: the *Batch (-b)* setting for a managed
  server, `-b 256 -ub 256` on the launch line for an external one. Nothing
  is said on a GPU host, on the clouds, or where the batch is already
  small.

- **A limit on how long quitting waits for background work.** *Tools* →
  *Quit: wait for background work (s)*: empty — wait until every task has
  landed (each has its own run time limit); a number — at most that many
  seconds; `0` — leave at once.

- **`/tasks stop <kind>`.** The typed route to stopping one of the app's own
  background tasks — `reflection`, `notes`, `self` or `compact` — for a
  terminal where `F6` on the tasks screen never arrives. Bare `/tasks stop`
  stops the only task running; with several running it lists them, with none
  it says so; `/tasks stop all` stops every one running. `/tasks` on its own
  still opens the screen.

- **A batch setting for the managed server.** *Performance* → *Batch (-b)*:
  how many prompt tokens `llama-server` processes per pass. Empty means
  auto — 256 when *GPU layers* is 0, the server's default otherwise; a
  number is passed as is. Changing it restarts the server.

- **Stop the app's own background task.** On the tasks screen (`F7`) `F6`
  now also stops one of the app's own tasks — a reflection, a consolidation,
  a history compaction — when its row says *running* or *waiting*, not only
  a background run. The task lands as cancelled: nothing it already wrote is
  undone, it is not counted as a failure, and the next scheduled one runs as
  usual; a `/compact` you asked for answers with a short notice.

- **A tasks screen.** `F7` (or `/tasks`) shows everything the app is doing
  in the background on one screen: every sub-agent and dialogue run across
  all your chats — the ones running, with the round and tool they are in and
  how long they have been out, then the ones that landed, with how they
  ended — and the app's own quiet work (reflection, consolidation, history
  compaction) as running or idle. `Enter` opens a run's transcript, `P` its
  chat, `F6` stops a running background run, and `Esc` from a chat opened
  there brings you back to the list.

- **Dialogues in the background.** The same switch that offers background
  sub-agents now also offers `start_dialogue`: the assistant stages a
  directed scene between two personas, gets its `chat://` address at once,
  and keeps talking to you while the scene plays out — its closing result
  arrives later as a task notification, exactly as a background sub-agent's
  does. The transcript is a row of the chat list while it runs and streams
  line by line if you open it, `F6` there stops it (as does
  `/subagents stop [n]`), and the cap counts scenes and sub-agents together.
  The director reads the conversation as it was when you asked for the
  scene. The setting is now called "Background runs (subagent/dialogue)".

- **Sub-agents in the background.** Switch on "Subagent: background runs"
  in Settings → Tools and the assistant gains `start_subagent`: a
  delegation that returns at once — the assistant keeps answering you while
  the sub-agent works, past the end of its own reply — and whose result
  arrives later as a **task notification** in the chat, worded for the
  assistant to read; when the chat is open and idle the assistant replies
  to it by itself ("Subagent: report background runs", on by default),
  otherwise the note waits for your next message. The run is a row of the
  chat list while it is out and its transcript opens as it streams;
  `Esc` stops your reply and not the run — `/subagents stop [n]` does, as
  does `F6` on the run's own open transcript, where `Esc` just takes you
  back; taking back the exchange that started it, or quitting, ends it and
  says so on its transcript. A result that arrives while you are looking
  somewhere else marks its chat **unread** in the chat list until you open
  it, so nothing waits for you unannounced. A background run never asks you to confirm a
  tool call: switch off the tools you would not let run unattended. Up to
  "Subagent: background runs at once" (2) may be out at a time; the
  status bar shows how many.
- **Several sub-agents at once.** When the assistant delegates several
  tasks in one reply — several `call_subagent` calls at once, the way Claude
  Code fans out its agents — they now run **in parallel**: "Subagent:
  parallel runs" in Settings → Tools says how many at a time (**1** by
  default, so nothing changes until you raise it), each run is its own row
  under the chat while it runs, the status bar counts them, and their
  results land in the order the assistant asked. Their streams share the
  engine's sessions (below): with one session they take turns round by
  round, with more they stream together. Above 1 the tool's description
  tells the model it may delegate several tasks in one reply — measured to
  make even a small model do so every time.
- **Parallel tool calls.** The reads and page fetches the assistant issues in
  one reply — two files, three pages — now run **at once** on a cloud engine
  (4 at a time by default); a local engine keeps the round one call after
  another until the new "Parallel tool calls" setting, beside "Sessions" on
  the engine's Model tab, is raised. Only tools that change nothing run
  together, and the results land in the assistant's order.
- **A "Parallel sessions" setting for the assistant's engine.** Every engine
  mode (managed, external, and each cloud provider) has its own `sessions`
  — how many request streams the app may keep open against that engine at
  once. It is **1** by default, and then nothing changes: the assistant and
  its sub-agents take turns as before. Above 1 a managed `llama-server` is
  launched with `-np N --kv-unified` (N slots sharing the one context pool
  `-c` sizes — no extra memory), and the field's hint shows how many slots
  the running server reports. Measured on one RTX 4090: two sessions gain
  little on a 31B, four gain 2.9× on Qwen 3.6 27B.

- **The spellcheck dictionaries now say where they come from — and carry their
  licences.** They are somebody else's work, redistributed with the program, and
  until now nothing in the release said whose or under what terms. Each
  dictionary's origin, version and licence is recorded in
  `dictionaries/SOURCES.md`, and the licence texts are installed next to the
  dictionaries themselves (`data/dictionaries/licenses/`) in every archive,
  package and install.

- **The privacy policy is readable in the app.** `F1` → the tab that used to be
  "Disclaimer" is now **"Legal"** and carries both documents — the disclaimer
  first, then the policy, in the interface language. One tab rather than two
  because the tab strip has no room for a seventh, and renamed because a tab
  called "Disclaimer" holding a privacy policy is a tab nobody looks in for one.

- **A privacy policy, and the installer shows it.** `PRIVACY.md` says what the
  program keeps on your machine, what leaves it and which setting of yours has
  to be on first, and what reaches the author — which is nothing. The Windows
  installer now has a page for it, after the disclaimer, in the language the
  wizard is running in; the file is installed next to the program and ships in
  the release archives, with a Russian translation beside the two the project
  already had.

- **A place of your own for spellcheck dictionaries.** In an installed (not
  portable) build the dictionaries that come with mindfork live next to the
  program, in a folder you are not meant to write to — so the data folder now
  gets its own `dictionaries/` at startup, with a short `README.txt` in your
  language: which two files a dictionary is (`<name>.aff` + `<name>.dic`),
  where ready-made ones are published, and that a dictionary you add under the
  name of a bundled one replaces it. The file is written once, when the folder
  is created: delete it or edit it and it stays that way.

- **The assistant can name its language model — and tell you when it changed.**
  Two new tools, on by default: `get_llm_name` answers "which model are you?"
  with the model actually generating the reply (or says honestly that the
  engine does not report a name), and `get_llm_history` lists the profile's
  dated history of model changes — recorded automatically after each exchange
  whose model differs from the last recorded one. The names are deliberately
  distinct from the `get_self_model` family: the LLM is not the assistant's
  stored personality. **The history does not start blank**: on first launch
  after the update, a profile that has none is filled from the conversations
  already on disk — every stored reply records the model that wrote it, so the
  history goes back as far as your chats do, with the real dates.

- **A dialogue of two personas, staged and directed by the assistant**
  (`run_dialogue`). Ask for a scene and the assistant composes two characters,
  writes the opening line, and directs the dialogue as it unfolds — sending a
  character private stage directions, asking for a retake, rewriting a line
  outright — and stops it when the scene reaches its ending. The script shows
  as a child chat under the conversation, each side under its character's
  name, with the director's interventions visible as notes; the assistant's
  reply carries the transcript's `chat://` address. One engine session, one
  request at a time — no extra VRAM on a local model; a new "Dialogue: run
  time limit" setting bounds a runaway scene (30 minutes by default).
  **The open transcript is live**: each line streams in token by token on its
  speaker's side, a director's retake or rewrite updates the view in place,
  and the status bar shows which line is being written — or that the director
  is judging the scene — while the dialogue runs.

- **The self-model screen (`F3`) can be read from either end.** A new setting,
  *Interface → Self-model: observations*, chooses whether the observation list
  starts from the newest or the oldest one. Newest first is the default — what
  the assistant noticed last is usually what you opened the screen for.

- **`/continue` — resume an interrupted reply from where it stopped.** A reply
  cut by `Esc`, by a connection failure, or by the length/context limit can now
  be continued in place: the model picks up exactly at the cut, the text grows
  inside the same message, and a turn interrupted between tool calls resumes
  its tool loop. Works on the local/managed and external (llama.cpp/vLLM)
  engines, on Gemini, and on Claude models up to the 4.5 generation; the
  providers that cannot resume a partial reply (OpenAI, Grok, current Claude
  models) are told apart, and the command says so instead of guessing. The
  notes shown for a cancelled or cut-short
  reply now name `/continue` where it applies — and a reply that hit the
  length limit finally gets a note at all, instead of stopping mid-sentence in
  silence.

- **An external server's model is named on screen, even when you did not name
  it.** Connect to a `llama-server` (or vLLM, LM Studio, a gateway) by URL and
  leave the "Model (opt.)" field blank, and the app now asks the server what it
  is running: the name appears next to the chat title and is recorded on every
  reply, so `Ctrl+P` → Interface → "Model name in the feed" finally has something to
  show in this mode. A file path is shortened to the model's name
  (`D:\GGUF\gemma-4-31B_q4_0-it.gguf` → `gemma-4-31B_q4_0-it`); a name you typed
  yourself always wins; a server that cannot say leaves the caption empty, as
  before.

### Changed

- **An API-key field says whose key it is.** Every row that holds a key now
  names its provider — *OpenAI API key*, *Gemini API key (env)*, *Tavily API
  key (env, opt.)* — in the cloud modes of *Model/server*, *Impersonation*,
  *Memory → embeddings* and *Speech*, and in the *Web search* and *Video*
  groups of *Tools*. One key serves everything that provider does, and the
  four slots can be set to four different providers at once, so a row reading
  just *API key* never said which one it was for. In *external* mode there is
  no provider to name and the field is unchanged.

- **A stopped background task no longer skips what it was about to read.**
  Stopping a reflection or a consolidation (`F6` on the tasks screen,
  `/tasks stop`) before it had done anything gives its window back: the
  same replies are reflected on, or the notes consolidated, after the next
  reply, as if the task had never started. A task stopped after it had
  already written something keeps its place, so nothing is written twice;
  one that had only looked — read its self-model, searched the notes —
  gives the window back. Quitting the app while one runs follows the same
  rule: the next launch picks the task up where it was interrupted — and
  if the task was in the middle of a tool call, the app waits for it to
  finish so the decision is exact: as long as it takes by default, or at
  most the limit set in *Tools* → *Quit: wait for background work*. A
  history compaction that had just finished when you quit is kept rather
  than dropped, and quitting during one no longer pauses.

- **A CPU-only host's server now runs a smaller batch.** With *GPU layers*
  at 0 the app launches `llama-server` with `-b 256 -ub 256`. The server
  looks at its queue between batches, so a background request the app
  cancels while it is still reading its prompt — a compaction or reflection
  your message displaced, a task you stopped from the tasks screen — used
  to hold its slot for the whole default batch (measured: 23 s); now for
  6.5 s, at about a seventh slower prompt processing. A GPU host at its
  defaults is untouched.

- **Your message no longer waits for the app's own request.** When one of
  the app's background requests — a history compaction, a reflection, a
  title — is streaming and your message would not fit beside it on the
  server, the app now cancels that request and makes it again after your
  reply, instead of holding your message until the request ends — what
  remains is the server finishing the batch it is processing (measured: a
  message that waited 46 s on a CPU-only server now waits about 24 s, one
  that waited 5.6 s on a GPU under two). A request yields at most three
  times, then finishes; the impersonation
  preview (`Ctrl+U`) is never cancelled this way. The background tasks'
  time limits no longer count the time they spent waiting for the server.

- **The app's own background requests run one at a time.** The automatic
  title, reflection, the two consolidations, history compaction and
  impersonation on the shared engine now take turns rather than opening
  together after a reply, and each waits for room in the server's context
  pool beside your reply or a background run. The tasks screen (`F7`) says
  which of them is *waiting*.

<!-- cyrillic-ok:start (the ru interface strings this entry is about) -->

- **The tasks screen's Russian section header.** «ПРОГОНЫ» — a literal
  rendering of "RUNS" — now reads «СУБАГЕНТЫ И ДИАЛОГИ», which is what the
  section actually lists.

<!-- cyrillic-ok:end -->

- **Parallel sessions never overfill the server's context pool.** Above one
  session a managed `llama-server` shares one context pool between the
  streams the app keeps open, and when they outgrew it together the server
  ended *every* running conversation at once — two sub-agents dying with
  "Context size has been exceeded" after a minute of visible progress. Now
  every stream reserves what it will occupy (its prompt, corrected by the
  exact sizes the server has already reported in this reply, plus its reply
  cap) and a stream that would not fit waits for room instead. Nothing
  changes at one session, on the clouds, or on an external server that does
  not report its slots; the *Parallel sessions* hint says "waits" where it
  said "fails".
- **A newer British dictionary.** `en_GB` moves from the 2018 word list to the
  current one from its author (V 4.0.9): about 14 000 more stems, so fewer
  correct words get underlined. It is the same variant as before — both
  *organise* and *organize* are accepted — and it now states its licence
  (LGPL v3 or later) in the file itself, with the full text installed beside it.

- **The key hints at the bottom of every screen now sit in the same place, and
  name only the keys that work.** They line up flush with the right edge on the
  chat list, the settings, self-model, changes and search screens, the way the
  chat screen's already did — the four that used to start at the left and fray
  at the right no longer do. And each screen's hints follow what you have
  selected: on the self-model screen (`F3`) an observation no longer offers
  "Enter edit" (observations are deleted, not edited) and `Space` shows only on
  a goal; on the changes screen (`F4`) `↑↓` says *file* or *scroll the diff*
  depending on which pane you are in, and `R` is offered only on a file that can
  actually be put back; in the settings `←→`, `Space` and `Del` appear on the
  fields they apply to. `F1` — which opens the full key list from anywhere — is
  now among the hints on every screen, along with `Ctrl+Q` on the two screens
  that quit without saying so.

- **The self-model screen (`F3`) reads as two named halves.** The
  self-description and the goals now sit under an **"Assistant"** header, and
  the traits, interests and relationship under **"User"** — where you have
  given the assistant or yourself a name in the profile settings, the screen
  uses that name instead of the label, matching the names over the messages in
  the chat. Blank lines now separate every section, and every observation from
  the next, so the model's own prose no longer runs together into one block.

- **The chat list counts messages the way the conversation reads.** The row's
  `N msg` is now the number of messages you see when you open the chat — your
  questions and the assistant's replies. It used to count every stored row,
  including each tool call's result and each round of an agentic loop, so a
  chat with one question and one tool-assisted answer could say "34 msg".
  Subagent transcript rows count the same way.

- **The Russian settings screen names spellchecking in Russian.** The toggle
  under *Орфография* now reads *Проверять орфографию* instead of the <!-- cyrillic-ok -->
  transliterated *Спелл-чек*. The English label is unchanged. <!-- cyrillic-ok -->

- **One word for a subagent, in both languages.** Each interface spelled it
  more than one way, and two spellings could show on the same screen: the role
  header over a subagent's reply disagreed with the label on the tool card that
  started it, and the settings screen with both — three ways in Russian, two in
  English. The unhyphenated spelling now stands everywhere: the feed's role
  header, the transcript rows in the chat list, `/subagents`, the settings
  fields, and the tool descriptions the assistant itself reads, which now match
  the name of the tool they describe.

### Fixed

- **Attaching a large document no longer holds up the app while its copy is written** — `/file attach` keeps the original of a PDF, a workbook or another binary with the chat, and hashing, writing and syncing that copy (up to 32 MB) ran on the loop that handles every command: about a tenth of a second on a fast disk, and as long as a slow or network disk takes. It now happens with the read, in the background.
- **A script's output can no longer pose as the tool's own sections in a result card** — a line such as `files:` or `stderr:` printed by a Python script or a build command opened a section of the card, so the output could show a list of "saved files" that were never saved, or turn the rest of itself red as errors; an ordinary YAML `files:` key did it by accident. Results now say how many lines each section holds, and the card takes exactly those. Results saved before this are shown as before.
- **A Python call can no longer copy a chat's whole file store into one run** — a call may name at most twenty of the chat's files, 100 MB in all; past that it is refused before anything is copied, with the numbers and a way to split the work — and the confirmation popup says so before you approve such a call. And in a chat with no files, naming one is refused with a plain "this chat has no files" instead of a list that ended on its own heading.
- **`sandbox setup` no longer hangs on a stalled step, and a saved chart is not silently lost** — each `wasmer` step the setup runs now has a time limit (ten minutes to unpack or build, thirty for the Python package download), past which the command stops and says which step it was. When collecting the files a Python call saved went wrong, the call reported that it had saved nothing; it now names those files as unreadable and logs the failure.
- **`/file open` says whose file it is, and its note stays in its chat** — a `.py` you attached yourself opened its folder with a note blaming “a file the assistant wrote”; it now says only that the type does not open from here. The note about a launch landed in whichever chat was open when the desktop answered; a success now stays with the chat it was asked in, and a failure is still shown. A stored file's name read back from the chat file is also checked before it is opened, as every other path into the chat's folder already was.
- **The "show charts" switch is shown in local Python mode too** — `tools.python_images` decides whether an image the code saved goes back to the model, and the local interpreter collects its output folder exactly as the sandbox does; the settings row, written when the flag meant nothing there, appeared only in sandbox mode. Off and switched to local, the charts were withheld with nowhere to see why.
- **One name means one file, outside ASCII too** — a handle typed as a name folded case over ASCII only, so a Cyrillic file name typed in lower case did not reach the file the listing had just shown; the listing's own “two items share this name” check folded the same narrow way. Both now fold over Unicode, as the chat's folder always did. Beside them: a very long name whose start is blank no longer survives shortening as an empty name, and an image a tool returns is numbered against the chat rather than the call — two rounds each drawing a chart produced two `tool-image-1.png`, which the model could then not name.

- **A stalled engine can no longer freeze a reply that produced a picture.** Before
  sending an image to the model the app asks the server whether it accepts images — a
  question with no time limit, asked again for every picture in the reply, and one that
  `Esc` could not interrupt. It is now asked once per reply, gives up after five seconds,
  and ends with the rest of the turn when you cancel.

- **Running Python on your own machine no longer reports a finished script as timed out.**
  If the code started a background process, the call waited for that process instead of
  the script — then gave up after the time limit and threw away everything the script had
  printed. It now waits for the script itself.

- **Temporary copies of a chat's files are cleaned up even after a crash.** Each call
  copies the files it needs into a temporary folder; if the app was killed, or something
  the code started was still holding that folder, it stayed on disk indefinitely. Anything
  left over for more than a day is now removed.

- **Naming one file for the code no longer silently names none.** When the assistant asked
  for a single file by name — without wrapping it in a list — nothing was copied in and
  nothing was said: the code then failed to find the file and the assistant tried the same
  thing again. A single name is now accepted, and an argument that makes no sense as file
  names is refused with an explanation instead of being ignored.

- **A run that produced thousands of files no longer floods the conversation.** Everything
  a call could not keep was listed one line per file, with no limit, in a result that then
  rode along in every later request. The first twenty are named and the rest counted.

- **A spreadsheet is no longer mistaken for a picture.** A file whose first characters
  happened to be `BM` — a CSV whose first column is `BMI`, say — was listed as an image,
  kept from the assistant as text, and then failed to display anyway, with two notes
  contradicting each other. Two letters are no longer enough to call something an image.

- **A file name can no longer rewrite itself on screen.** Invisible marks that reverse the
  text after them let a name the assistant chose be shown as something else entirely — a
  program displayed as a document. Those marks are now stripped from stored file names, as
  other forbidden characters already were.

- **The confirmation popup no longer hides the code it is asking you to approve.** When
  the line listing the files going into a call was long enough to wrap — two named files
  is enough — the popup drew one row short and the row it lost was the code itself. It is
  now sized by what it actually draws.

- **"Attach it again" now actually brings a lost file back.** When a chat's copy of a file
  went missing from its folder — a pruned data directory, a half-finished sync — the list
  marked it missing and the assistant refused to use it, telling you to attach the file
  again. Doing that did nothing: the name and the contents matched what was already listed,
  so nothing was written and the next attempt refused all the same. Offering the same file
  again now puts the copy back, and the assistant says so when its own code re-creates one.

- **A failed `sandbox setup` no longer costs you the sandbox you had.** The new Python
  image was put in place first and only then started, so a build that would not run left
  you with a broken sandbox *and* without the working one it replaced — while the app went
  on reporting Python as ready and every attempt to run code failed. The freshly built
  image is now started before it replaces anything: if it does not run, the command says so
  and your existing sandbox is untouched. A build that times out now says that, instead of
  failing with an empty message.

- **A file's number stays that file's for the whole reply.** The assistant is told your
  files as a numbered list before it starts working — `#1`, `#2`, `#3`. If something was
  added while it worked (a page it fetched, a chart it saved), the numbering underneath
  shifted, and a `#2` it had been given could quietly become a different file: the code ran
  against the wrong one, or against a name that no longer existed. Numbers now belong to
  the file they were given for until the reply ends, and anything new gets a number of its
  own after them.

- **Every "you weren't shown this image" note now says it in words that work.** The same
  measurement was run against the rest of them — the notes for a model that takes no
  images, for a picture too large to show, for charts held back by the Python settings or
  by the four-per-call cap, and for an SVG. Where the polite wording left the assistant
  describing a picture it had never seen, it now tells it plainly not to; the SVG note
  still points at saving a PNG instead, because there that is the useful thing to do.

- **A picture a plugin held back is now said out loud — in words that work.** With "Let
  servers send images" off, an image a server's tool returned was dropped in silence: the
  assistant got a result that looked complete and went on to describe a screenshot it had
  never seen. The result now says how many images were held back and tells the assistant
  to say it cannot see them. Measured on a local model, the polite version of that
  sentence changed nothing at all — the assistant invented a description just as often as
  with no note — so the wording is the blunt one.

- **A backup now carries the code workspaces too.** The change journal of a chat's attached
  project — the original of every file the assistant edited, which is what "revert" puts
  back — was left out of `mindfork backup`, so a restored chat could still show what had
  changed but no longer undo it. `workspace/` is packed and restored with the rest of the
  data now.

- **An image a tool returns but the model does not get is now said so.** An MCP
  server's image that was too large or would not decode vanished without a word,
  and a model without vision was sent a tool's images anyway; now neither is sent,
  and the tool's result tells the model it has not seen them.

- **`/file remove` and `/image remove` no longer guess between two items of
  one name.** With `notes.md` attached from two different folders,
  `/file remove notes.md` removed whichever came first and said only
  "notes.md", while `/file list` showed two identical lines. A shared name now
  removes nothing and lists each item's number and path to choose from; the
  listing shows the path wherever a name is shared, and the removal note says
  which one went. `/image remove` behaves the same way.
- **A web page attached to the chat is named after the page, not its site.**
  A long page the assistant reads is attached under a name, and on some sites
  that name was the site's own on every page: every article of an old magazine
  archive carried the archive's banner, every chapter of the Rust Book "The
  Rust Programming Language", every post of some blogs the blog's name — with
  a second page from the site told apart only by a file name tacked on. The
  name now comes from what the page says about itself in two places at once,
  so each page is named by its own title.
- **Files in an older encoding are read — and edited — as they are.** A text
  file saved in windows-1251, KOI8-R or cp866, or as Notepad's "Unicode",
  reached the assistant with every Russian letter replaced by `�` — when it
  read the file and in the project tools — could not be found by its words,
  and was refused by `/file attach` and `/rag add`. Worse, an edit the
  assistant made in an attached project **rewrote the whole file**: every
  letter it did not touch became `�` on disk, while the changes screen showed
  a single changed line. Files are now read in their own encoding (for a
  short file the interface language helps guess it); the project tools and
  the attach note say which encoding when it is not UTF-8; an edit is written
  back in the file's own encoding, and refused with nothing written when that
  encoding cannot hold the new text; and the changes screen no longer calls a
  changed file unchanged. A file an earlier edit damaged stays on that chat's
  changes screen, where `r` puts back the original.
- **Pages in an older encoding are readable.** Reading a web page and web
  search took every page for UTF-8 whatever it declared, so a page in
  windows-1251, KOI8-R, Shift_JIS or GBK — common on older sites — came
  back with every letter replaced by `�`: in the answer, in the attachment
  a long page becomes, and in that attachment's search index, which left
  the assistant fetching the page again some other way. A page is now read
  in the encoding its server or the page itself declares; a site that moved
  to UTF-8 but kept its old declaration is still read correctly; and a page
  that declares nothing is recognised from its text. A page the server
  compressed without being asked is unpacked instead of arriving as noise.
  An attachment already garbled this way stays garbled — fetch the page
  again.
- **Spellcheck no longer stumbles over a stress mark.** A word carrying
  one — `Alt+0769`, or text pasted from Wikipedia, a dictionary or a
  grammar reference — was cut in two at the mark and each half judged
  on its own, so correct text was underlined (up to three underlines on
  a two-word phrase) and, when both halves happened to be words, a real
  error vanished instead. A mark now belongs to the letter it sits on,
  and a word that fails as typed is checked once more without its
  marks: stressed text is left alone, a genuine typo is still
  underlined — as the whole word, once — and the suggestions popup
  offers corrections for the word rather than for its tail. "Add to
  dictionary" stores the word without the mark, so one add covers every
  placement of the stress. The same fix covers a `ё` that arrives
  written as `е` plus a separate diaeresis.
- **A directed dialogue on Gemma 3 failed at its second checkpoint.**
  The director's memory of its earlier verdicts was kept as tool calls
  with an acknowledgement, which Gemma 3's chat template renders as two
  user turns in a row and refuses ("roles must alternate") — so every
  scene longer than one exchange ended with an engine error. The
  director now remembers its verdicts as its own words; on every other
  model the verdicts are the same.
- **Impersonation on Gemma 3.** `Ctrl+U` answered with an engine error on
  every chat you had opened yourself: the request swaps the roles of the
  conversation, so it began with an assistant turn, which Gemma 3's chat
  template refuses ("roles must alternate"); two of your messages in a
  row were refused the same way. The conversation sent now alternates on
  every model — your opening line travels in the persona's instructions,
  consecutive messages of one side are one turn — and where it already
  worked the reply is the same.
- **A page summary on a thinking model came back empty.** `fetch_url`'s
  summary let the model think, and a model that thinks by default spent
  the whole reply on thoughts and answered with nothing — the page's text
  came back under "summary unavailable", or an attached page with no
  summary at all. The summary now runs with thinking off, like the
  automatic title and the history compression.
- **A page summary's reservation is corrected like every other request's.**
  A page's text — an API's JSON, a source file, an English article —
  counts more tokens than the app's estimate says, and the summary's
  request was the one kind that never reported its exact count, so its
  share of a shared context was reserved a quarter under on such pages.
  It reports now, and the server's timing of its prompt counts for the
  slow-prompt note like a turn's.
- **A background request no longer resets the correction a turn taught
  the budget.** The size the app reserves for a request beside others on
  a shared context is corrected by the last exact count the server gave —
  and one correction served every kind of request, so a short background
  request over prose (a chat's title, a compression roll) could reset the
  correction a turn carrying a large tool result had just recorded, and
  the next turn was reserved a quarter under its size. Each kind of
  request now keeps its own correction; the automatic title and
  impersonation report theirs too.
- **The conversation's token estimate counts the tool schemas.** The `~`
  figure shown before the server's exact count, and the size the app
  reserves for a request beside others on a shared context, had counted
  the messages and not the tool schemas — the largest part of a request
  with tools: a fresh chat read about 85 tokens where the server counted
  4358. Both now count the schemas as they are sent; the estimate lands a
  tenth over the exact count instead of a fiftieth under, and a
  compression roll — which carries no tools — is reserved at its own size
  rather than nine times it.
- **A compaction or a reflection beside a running reply could end both.** At
  the default of one session a local `llama-server` runs four slots over
  one context pool, and the app's background requests landed on them
  outside the guard that keeps sub-agents from overfilling it — a history
  compaction fires exactly when the conversation is at its largest, so the
  next reply and the compaction could exceed the pool together and the
  server ended both ("Context size has been exceeded"), the compaction
  failing silently. The guard now covers every request the app makes.
- A failed history compaction was reported with the title generator's
  wording ("Title generation error"); it now says the summary request failed.

- **A tool round on OpenAI no longer fails when the model reasons in
  stages.** gpt-5.6 often returns two to five reasoning items in one reply;
  the app sent them back fused into one, which OpenAI rejected
  (`invalid_encrypted_content`), so a sub-agent that had just made its
  searches ended with "the engine failed" and the assistant had to run it
  again — the same shape could end a tool round in the main chat. Every
  reasoning item now goes back as itself, in order.

- **The "Sessions (parallel streams)" hint now says what it does not do.**
  Raising it alone never made the sub-agents of one reply run together —
  that number is "Subagent: parallel runs" in Settings → Tools, which
  stayed at 1 while the hint's "1 (default): they take turns" read as if
  sessions were the switch. The hint now names the second field and its
  default, and says to raise both.

- **A long chat or run title no longer ends mid-word.** Every title was cut
  to 100 characters when it was stored, without a mark to say so — so a
  sub-agent run named after the first line of its instruction read as its own
  full name, and on a maximized window the tasks screen showed it cut with
  half the row still empty. Titles are no longer shortened when they are
  stored: what does not fit is cut where it is drawn, to the columns that
  screen actually has, and always ends in "…". Two places that used to clip a
  long title silently now cut it the same way: the chat panel's top border
  (the model caption keeps its corner) and the chat-reference picker (the
  date keeps its place). Titles already on disk are repaired on the next
  start where the text is recoverable — see **Data**.

- **A server error inside an open stream no longer passes for a finished
  reply.** When `llama-server` (or an OpenAI-compatible proxy) put an error
  object into an already-open stream — its "Context size has been
  exceeded." to every running conversation, for one — the reply simply
  stopped, cut mid-word, and was recorded as complete; a sub-agent's parent
  read the fragment as the answer. The error now ends the reply as an
  error, with the note on screen and, where nothing had been delivered yet,
  the retry that was always meant to run.
- **The program's own name, as Windows shows it.** `mindfork.exe` announced
  itself as `mindfork-rs` — the package name — everywhere Windows reads a file's
  properties: Task Manager, Explorer's details, the "unknown publisher" dialog.
  It now says `mindfork`, like everything else, and carries the author and the
  copyright it was missing. The installer, in turn, had been shipping a version
  of `0.0.0.0`; it now carries the real one.

- **In KDE Konsole the input box promised a key the terminal cannot send.** The
  footer said "Shift+Enter newline", but Konsole maps that combination to a
  sequence the app never receives — pressing it did nothing at all. The line
  break itself has always had a second chord, `Alt+Enter`, which works there;
  the footer (and the same footer in the settings and self-model editors) now
  names whichever of the two your terminal can actually deliver. Nothing
  changed for terminals that do support `Shift+Enter` — Windows, and any unix
  terminal speaking the kitty keyboard protocol. If you would rather keep the
  chord in Konsole, point its keytab at `\E[13;2u` for `Return+Shift`.

- **The self-model screen listed its observations oldest first.** The `F3`
  screen showed the oldest observation directly under the "Observations"
  header, so seeing the most recent one meant scrolling past the whole
  narrative. The list now starts with the newest (and the new setting above
  flips it back if you prefer reading the narrative forward).

- **The status bar's `Esc` hint now says "cancel" while a reply is generating.**
  `Esc` cancels a running generation and only goes back — to the chat list, the
  search results or the conversation you came from — once it has finished, but
  the bar kept naming the destination for the whole turn, one row under an input
  box that says "generation… Esc cancel". The two hints agree now, and the
  destination comes back the moment the turn ends.

- **The status bar no longer stacks the key hints into a column.** While a turn
  was running, a status line full of indicators (generation, the token counter,
  attached files) left the hints too little room and pushed them one per line —
  up to six rows of status bar in a window where they fit on one. The hints now
  stay a compact block in the bar's right corner, at most two rows tall: when
  the indicators leave it too little room, the block temporarily hides its
  least important hints instead of wrapping — every hidden shortcut is still on
  the `F1` help — and shows them again once the turn's indicators go. The bar
  never grows past two rows, and hints never land under the indicators.

- **The Python sandbox installs and runs again.** `mindfork sandbox setup`
  fetched the Python package without a version, and a build published in the
  Wasmer registry on 18–21 August 2026 cannot be compiled by the runtime the
  app pins — so a sandbox installed after that date could not run Python at all,
  failing with a compile error the moment a script was executed. The package is
  now pinned to an exact version. An **already installed** sandbox was never
  affected; if you installed one in that window, re-run
  `mindfork sandbox setup --force`.

- **The external server's "Model (opt.)" field is now actually sent to it.** It
  had never left the settings file, which made every multi-model endpoint
  unusable in `external` mode — `llama-server` in router mode, LM Studio,
  LiteLLM and OpenRouter all pick the model from that field and refuse a request
  without it. The chat, impersonation and embedding sections all send it now.
  A blank field still sends nothing, so a single-model local server is
  unaffected.

### Data

- Chats gain an optional list of stored files, and the data folder a `files/`
  directory beside `chats/`. Older chat files read unchanged, with no migration.

- **Chat files: `CHAT_SCHEMA` 3 → 4.** A sub-agent run whose title the old
  100-character limit had cut gets the rest of it back, re-derived from the
  instruction the run still holds — so runs from before this version read
  the same way as new ones. Only a title that is exactly 100 characters, was
  not renamed by you, and matches the start of that instruction is touched;
  chat titles and dialogue titles are left as they are. As with every
  migration, the app makes a full backup before writing anything.

- Chat files move to schema **v3** (a version stamp, no shape change): a chat
  may now carry a dialogue transcript, and an older mindfork refuses such a
  file with a clear message instead of failing to read it. Existing chats are
  re-stamped at first launch through the usual backup-then-migrate path.

### Security

- **A workbook or document the assistant wrote opens in Protected View** — on Windows, an
  Excel workbook or Word document saved by `python_exec` opened as a trusted local file.
  Files the code writes now carry the same mark as a download, so Office opens them in
  Protected View whether you use `/file open` or the folder. A CSV is not covered: Excel
  opens a marked CSV normally unless its setting for untrusted text files is on, and what
  keeps a formula in one from starting a program is Excel's own DDE setting, off by
  default. An attached document's copy keeps the mark of the file it came from, and
  restoring a backup marks its stored files.
- **The Python sandbox's packages can no longer be changed from inside it.** The
  sandbox mounted its package directory writable, so code run in one call could
  leave a file there that then ran inside every later call, in every chat.
  `mindfork sandbox setup` now packs Python and its packages into one image the
  code cannot alter. **An existing sandbox stops running code until you run
  `mindfork sandbox setup` again**: it packs what is already downloaded, in
  seconds, and the tool names that command when it refuses.

- **The web tools are now off until you turn them on.** `web_search`,
  `fetch_url` and `youtube_watch` used to be enabled in a fresh installation.
  Everything else mindfork connects to is an address you chose — your model
  server, your embedder, your MCP servers — but the search engines behind
  `web_search` are picked by the app, and the query it sends is built from your
  conversation. Now nothing goes to them until you switch the tools on in
  Settings → Tools. An existing installation keeps whatever your settings
  already say.

- **A search key is only used where you put it.** The app no longer assumes the
  environment variable `TAVILY_API_KEY`: if a key for a paid search provider was
  sitting in your environment for some other program, mindfork could route
  searches through it — and spend its credits — without you deciding anything
  here. Name the variable in Settings → Tools, or enter the key there, and it
  works as before.

- **The Python sandbox's largest download is now checked too.** `mindfork
  sandbox setup` verified every file it downloaded itself against a pinned
  checksum, except the Python distribution, which `wasmer` fetches on its
  behalf. That file is now verified as well, and one that does not match is
  replaced instead of used.

## [0.9.8] — 2026-08-27

### Added

- **The build date on the "About" tab.** `F1` → "About" now shows, right under
  the version, the day the copy you are running was built (`2026-08-27`, UTC) —
  so a bug report can name a build the version number alone cannot tell apart.
  The row appears in released builds; a build you made yourself from source in
  development mode does not show it, because the date there could be older than
  the code and a wrong date is worse than none.

- **Web search can use a provider with an API key.** The free search engines
  throttle automated requests hard — measured, they answer about twice in a row
  before blocking, and the block lasts far longer than a conversation — which is
  why a long chain of searches used to fall apart. Settings → Tools → Web search
  now takes an API key for **Tavily** — 1000 searches a month, no card needed.
  Enter one and the assistant searches through Tavily first; the free engines
  stay as the fallback, so nothing changes if you enter no key at all. You can
  also point the app at an environment variable instead (`TAVILY_API_KEY`), or
  turn the key off for search entirely. Search results now say which service
  answered.

- **Sub-agent transcripts fold under their chat.** The chat list now keeps a
  chat's transcript rows collapsed by default, with a muted `▸ n` beside the
  count saying how many it holds; `Ctrl+O` in the list unfolds/refolds the
  selected chat's (on a transcript row — its parent's), and `/subagents` typed
  in the chat does the same for the open conversation (bare — a toggle;
  `expand`/`collapse` set it outright). The choice is remembered per chat, and
  a search still surfaces a matching transcript under its collapsed chat.

- **The sub-agent has the assistant's tools.** `call_subagent` no longer asks
  one tool-less question: the sub-agent runs as a nested turn with the same
  tools the assistant has in this chat (except creating sub-agents, reading
  the folded history and the self-model), over the same attached files and
  project, under the persona the assistant composes — with an optional `name`
  for it. A dangerous call inside the run asks you exactly as outside. Its
  whole transcript is kept on the call inside this chat (the list, a read-only
  view and search follow in the next stages), and the assistant gets the
  sub-agent's final reply plus the transcript's `chat://` address.
- **Sub-agent transcripts in the chat list.** Each delegation shows as a row
  nested under the chat that made it (`└`), in call order, with a mark when the
  run did not complete. It opens like a chat — the sub-agent's persona on top as
  a system message, the instruction headed by your assistant's name, the replies
  by the sub-agent's — but read-only: sending and anything that would change a
  conversation refuse and point you to the parent chat; renaming (`F2`,
  `/rename`, `Ctrl+R` for a model-written title), copying, exporting, searching
  and speaking work. `Del` and `Ctrl+D` refuse on a transcript — it goes away
  with the exchange that made it (`Ctrl+E`/`Ctrl+R` in the parent) or with the
  parent. The `chat://` address a sub-agent's result carries is now a link.
- **Sub-agent transcripts are searchable.** The chat list's content search
  (`Ctrl+F`) finds text said inside a delegation and shows the transcript's
  row under its chat; `Enter` on that row opens the transcript on its first
  match. The message-level results (`Ctrl+G`) group a transcript's hits
  under its chat's header with a `└`, and `Enter` opens the transcript on the
  message. With the optional `chat_search`/`chat_read` tools on, the
  assistant can search and read sub-agent transcripts of this profile's
  conversations too — including this chat's own — each labelled as the
  transcript of its conversation and reachable by its `chat://` address.
- **You can see a sub-agent working.** While the assistant waits on a
  delegation the status bar shows a quiet chip — the sub-agent's name, the
  round it is on and the tool it is using — instead of a bare "generating"
  for minutes.
- **Coming back to a chat mid-reply shows the reply so far.** Step into a
  running sub-agent's transcript and back, and the chat shows everything the
  assistant has written in the current round — text, thoughts, the tool calls
  it opened — and keeps streaming into it, instead of catching up only when
  the reply lands.
- **A sub-agent's transcript streams while you watch.** Open a running
  delegation and the sub-agent's reply arrives word by word, its thoughts and
  tool calls included, with its own token counter — and opening it mid-reply
  shows what it has written so far rather than starting from the next round.
- **A tool call shows up the moment it starts.** The card for a tool call
  appears in the reply as soon as the assistant makes the call, marked
  *running…* where the result will go, and fills in when the result arrives —
  so a long call (a sub-agent run, a project build, a Python script) is
  visible where it happens rather than only as a status-bar chip.
- **A sub-agent's transcript is there while it runs.** The moment the
  assistant delegates, the transcript appears in the chat list under the chat,
  marked *running*, its message count growing round by round. Open it to read
  what the sub-agent has done so far — it grows as you watch — and go back to
  the chat: moving between the two no longer cancels the turn (any other chat
  switch still does). Renaming it while it runs keeps your title.
- **Sub-agent transcripts name themselves.** With automatic titling on
  (either setting), a finished delegation gets a model-written title the
  moment it lands, like a new chat does; a title you gave it by hand is
  kept. The demo (`mindfork demo`) now includes one such transcript.

- **The licence and the disclaimer in Russian** — with the interface language
  set to Russian, the `F1` → "Licence"/"Disclaimer" tabs and the Windows
  installer's two legal pages show a Russian text instead of an English one.
  The translations are unofficial and say so in their first paragraph: the
  English originals are the texts that have legal force. Both travel with the
  product, next to the originals, in the archives, the Linux packages and the
  installed folder.
- **See what the assistant changed in your project, and undo it** — `F4` (or
  `/changes`) opens a screen listing every file it touched, with the changes
  shown as a diff against the file as it was before it was first touched.
  `r` puts one file back the way it was, after asking; a file the assistant
  created is removed instead. Files it could not diff — binary, very large, or
  deleted since — say so rather than showing nothing.
- **The assistant can build, run and test the attached project** — through
  command lines you type: `/project build-cmd cargo build`, and the same for
  `run-cmd` and `test-cmd` (`/project clear build|run|test` removes one, and
  `/project status` shows all three). It runs them exactly as written and can
  never change one, add a flag to it or compose a command of its own; a slot you
  have not filled gives it no such tool at all. A command that outruns its time
  limit is stopped together with everything it started, and whatever it printed
  by then still comes back. Pipelines and redirects are not run — the refusal
  says so when you set the line, and points at wrapping the steps in a script.
  New settings under Tools → Workspace: the command time limit, how much output
  reaches the model, and how many rounds one answer may spend inside the project.
- **The assistant can change the attached project** — it edits a file by
  replacing an exact fragment, or writes a file whole. Every file's previous
  content is saved before it is first touched, so a coming release can show the
  changes and put them back; a change that cannot be saved that way is refused
  rather than made. Turn on "confirm dangerous tool calls" in settings to approve
  each change before it happens. Reading and editing the project no longer spend
  the tool-call limit — a fix takes as many steps as it takes.
- **A code project can be attached to a chat** — `/project attach <directory>`,
  `/project detach`, `/project status`. With a project attached the assistant
  can see its structure, read its files (with line numbers) and search them by
  regular expression, all confined to that directory and honouring
  `.gitignore`. Attaching is the whole permission: with no project attached
  nothing changes about what the assistant can reach, and the tools are not
  even offered to it. Editing, build/run/test commands and a changes screen
  come in the following stages.
- **The app says so when it starts without its database.** `data.db` holds the
  notes, the self-model, the knowledge base and the search index over files
  attached to chats — and copying a data folder to another machine without it
  used to be silent: the conversations were all there and the assistant
  remembered nothing about them, with no explanation. Now a launch that finds
  chats but no `data.db` writes a warning to the log and opens with one note in
  the feed saying what is empty, what survived (the conversations and the text
  of their attachments) and how to get the rest back — put the file next to
  `chats/`, or restore a backup. A first launch, having no chats either, stays
  quiet as before.
- **`/autotitle` — the model-written title as a typed command.** The action
  lived only behind `Ctrl+R` in the chat list, and in a browser tab that key
  reloads the page. The command titles the open conversation; failures land as
  a note in the chat when the list is not there to show them.
- **The impersonation profiles are operable by commands.** `/impersonation
  list · new [name] · delete <name> · use <name|default> · system
  [text|clear]` — the user personas `Ctrl+U` writes as, until now editable
  only in settings behind `Ctrl+N`/`Ctrl+D`, which a browser tab keeps for
  itself. Deleting always asks first; `use default` returns the profile to
  the shared text.
- **The profile's texts are editable from the input box.** `/profile system
  [text|clear]` and `/profile greeting [text|clear]` edit the open chat's
  profile — bare, the current text comes back as an editable command line;
  `clear` removes it. Both apply to new conversations with that profile, and
  the notes say so.

### Changed

- **The confirmation setting is named after what it guards.** Interface →
  Behavior read "Confirm Ctrl+R / Ctrl+E" — the chords it intercepts rather than
  what it protects, and its description spelled both out again. It now reads
  "Confirm regenerate / delete" and says in full what it asks before:
  regenerating the last reply and deleting the last exchange, both of which
  throw away text that is already written. The keys stay where they are already
  listed — the help (`F1`) and the README — so renaming one no longer leaves a
  setting pointing at a chord that moved.

- **The help lists keys by screen, and `F1` works everywhere.** The shortcuts
  tab is now sections — globally · chat · chat list · settings · self-model
  · changes · found messages. A key that means different things on different
  screens (`Ctrl+O`, `Ctrl+F`, `Ctrl+R`…) is one short row per screen instead
  of one row trying to say both, and the settings, self-model, changes and
  search screens' keys are listed at all for the first time. `F1` now opens
  the help from **any** screen — not just the chat — landing right on the
  section of the screen you were on, marked *you are here*; `Esc` puts you
  back exactly where you were.
- **The help is `F1` (or `/help`) — `?` is just a character now.** The
  shortcuts tab used to advertise `F1 / ?`, but `?` only ever opened the help
  on the chat screen, and there only with an empty input box: in the chat
  list's filter, the settings' search or any box with text in it, it typed a
  question mark. A key listed as global that works in one place out of six is
  worse than no key, so it is gone — from the input handler, from the open
  dialog (which no longer closes on it) and from the help's own table. `F1`
  opens the help from anywhere, `/help` types the same.

- **The command is `mindfork` now.** The binary lost the `-rs` suffix — that
  stays the project's name: type `mindfork` (`mindfork demo`, `mindfork
  sandbox setup`, …), the Linux packages symlink `/usr/bin/mindfork` and the
  application-menu entry says *mindfork*, and the Windows installer installs
  `mindfork.exe` with matching shortcuts. Where your data lives, the package
  names and the archive names are unchanged.
- **`/reindex` now also rebuilds an attached file's search index when it is
  missing entirely.** Before, it re-embedded vectors that already existed, which
  is what an embedding-model change leaves behind — but an index can be gone
  altogether: copy a data folder to another machine without `data.db` and the
  conversations arrive with every attached file's text and no index over any of
  it. Since that text lives in the chat itself, nothing needs re-attaching:
  `/reindex` walks the chats and rebuilds the missing indexes from it, so search
  over an attached file works again even when the original file is on the other
  machine. Files small enough to be sent whole are untouched, as always, and so
  are deleted conversations.

- **A model split across several GGUF files.** Large models are published as
  parts (`…-00001-of-00003.gguf`, `…-00002-of-00003.gguf`, …); point the GGUF
  setting at the **first** part, with the rest beside it, and the server loads
  them as one model — as it always could, but the two ways of getting it wrong
  are now caught before launch instead of showing up as a server that quietly
  exits. Pointing at a later part says which file to use, and a part missing
  from the directory (an interrupted download) is named. Such a model is also
  called by its own name in the interface — `gpt-oss-120b-Q8_0`, not
  `gpt-oss-120b-Q8_0-00001-of-00003`.
- **The sub-agent's limits.** The whole run — every model round and tool call —
  is bounded by a new *Subagent: run time limit* (600 s by default), replacing
  the one-request timeout; the per-reply token cap's default rises to 4096;
  the round budget is `max_tool_rounds`, as for the assistant itself.
- **An expanded tool card lists the call's arguments in the tool's own order.**
  Before, they were sorted alphabetically, which read as scrambled: a subagent
  call showed the request above the instruction it was sent with, and a file
  edit showed the replacement text above the file's path. Every card now follows
  the order the tool declares its arguments in, and the collapsed one-line header
  follows the same order.
- **The Windows installer is a 64-bit program now.** It installs exactly what it
  installed before, but it refuses a system that cannot run mindfork before
  the wizard opens rather than on a page inside it — the program is 64-bit only,
  and now so is its installer. The download grows by about 0.7 MB. The wizard's
  title bar also drops the word "version" before the number, following the
  installer toolkit's own change.
- **The "About" tab of the help dialog (`F1`) reads across the full width.**
  Its values now line up in one column against the right edge, with dotted
  leaders bridging the gap — the layout the "Components" tab already uses —
  instead of hugging the left half of the window. Two rows joined them: the
  app's license and the build target (operating system and CPU architecture).

### Fixed

- **Plugin (MCP) tool switches line up with the rest again.** A tool served by
  an MCP server carries its full name in the profile's tool list
  (`mcp__<server>__<tool>`), and one longer than the column pushed its own `[x]`
  a step to the right — in a list of a dozen such tools the checkboxes came out
  ragged. The name is now trimmed with `…` at the column instead, so every
  switch in the section sits on one vertical line; the short tool name still
  stands beside it, and its full description is in the panel below.

- **The `auto` theme really follows your terminal now.** It always took its role
  colours from the terminal's palette, but two things it could not express that
  way — the shading of code blocks and the backdrop every selected row is drawn
  on — were fixed to dark regardless. On a light terminal (a JupyterLab terminal
  on the light lab theme, most visibly) that meant dark-tuned syntax colours and
  a dark bar on white wherever something was selected. The app now asks the
  terminal for its background colour at start-up and matches both to it.
  Terminals that answer include Windows Terminal, VS Code's, JupyterLab's, a
  plain SSH session and tmux; the legacy Windows console does not answer and is
  treated as dark, which is what it is. Nothing waits on the reply — the question goes
  out before the app opens its storage. Set `MINDFORK_TERMINAL_BG` to `dark`,
  `light` or `off` to override it, or just pick `dark`/`light` in settings.

- **Web search said "no results" when it had actually been blocked.** Search
  engines cut off traffic that looks automated, and one of them now serves that
  block as an ordinary-looking page — which the app read as "the web has nothing
  on this" and reported to the assistant as an empty result. The assistant would
  then stop searching and answer from memory. A block is now recognised for what
  it is, and the assistant is told the search is temporarily unavailable so it
  can try again. Searching *for* captchas and anti-bot topics still works.
- **Repeated searches in one answer no longer retry a provider that just blocked
  them.** When the assistant runs several searches in a row, the ones after the
  first used to start over with the provider that had already refused, wasting
  seconds on every call. A provider that blocked is now tried last for a few
  minutes — but never dropped, so nothing is silently left unsearched.

- **A fresh install can change the profile's framework language.** The empty
  chat the first launch creates counted as the profile's data, so the
  "Framework language" field was born locked — and deleting that chat only
  created another. An untouched chat no longer locks the language: while a
  profile has no conversations, no «self-model» and no notes, the language can
  be switched, and the untouched defaults follow it — the profile's name and
  system message, the empty chat's title. Anything you wrote yourself (a
  renamed chat, your own system message) stays exactly as you typed it; the
  first real message locks the language as before.

- **A taken-back message no longer runs into what you were typing.**
  Deleting the last exchange (`Ctrl+E`, `/takeback`) returns your message to
  the input box; if something was already typed there, the two are now
  separated by an empty line instead of a single space.

- **Lists scroll the same way up as they do down.** Moving the selection up
  with `↑` scrolled the whole list on every press, keeping the selection pinned
  to the bottom row; now it walks up to the top visible row first, and only then
  does the list scroll — the mirror image of `↓`. Fixed in the chat list, in the
  settings screen's section menu, field pane and field search (`/`), and in the
  profile, reference (`Ctrl+L`) and spellcheck popups. The settings field pane
  also keeps a row of context around the selection, so the group header you are
  standing under stays visible.

- **A typed command no longer resurfaces in the input box after doing its
  job.** `/takeback` glued itself onto the restored message ("your text/takeback"),
  `/regen` came back into the box, a `/clone`'d chat opened holding `/clone`,
  and the chat `/new` was typed in kept it as a saved draft — all one defect:
  the box's cleared draft reached the orchestrator one step after the command
  itself. The draft is now flushed first. The restored message also gets a
  separating space when text was already in the box, instead of fusing with it
  mid-word.

- **When a copy cannot reach your clipboard, the app now says what will.** In a
  browser terminal such as JupyterLab's, the OSC 52 sequence a copy sends is
  silently dropped — and the note said only that the terminal "may not support"
  it, leaving nowhere to go. Every such message now points at `/export`, which
  writes the conversation to a file and works everywhere; the JupyterLab case is
  named outright, since that is the terminal known to drop it. The same applies
  to a conversation too large for the sequence.

- **`/project` commands are highlighted as you type them, like every other
  command.** A line starting with `/project` stayed the colour of an ordinary
  message right up to `Enter` — and was spellchecked as prose, so a path or a
  slot name could pick up red underlines under a line that was never going to be
  sent to the model. It is now coloured from the first recognised word, exactly
  like `/rag`, `/file`, `/image` and the rest.
- **Attaching a different project no longer mixes up the change list.** A chat's
  record of what the assistant changed is now dropped the moment you attach
  another directory. Before, it survived until the assistant's next edit — so
  `F4` could list the previous project's files against the new one's directory,
  and reverting a file the two projects both have (`Cargo.toml`, `README.md`)
  wrote the previous project's contents into it. Re-attaching the same directory
  keeps the change list, as before.
- **The settings hint panel keeps one height everywhere.** The description
  panel under the field list is now sized to the longest hint of the whole
  settings catalog rather than the current section's, so switching sections no
  longer resizes it by several rows on every Tab. Text the panel still cannot
  show whole — the preview of a long system message, a hint in a very small
  window — now ends with a visible `…` instead of stopping mid-word as if the
  text simply ended there.
- **The cloud "Model" field describes itself again.** Its hint had been
  silently replaced by the "Model name in the feed" toggle's text (a duplicated
  translation key — JSON keeps the later entry); the two fields have their own
  keys now, and a new gate keeps duplicate keys out of the bundles.
- **The indexing banner keeps its counter on screen.** While a file or an
  attachment is being indexed, the `RAG:` row above the input box used to be cut
  off at the right edge, and a long name — a web page's title, a deep folder —
  took the `64/128` progress with it. The row now fits the window: the folder
  is left out first, then the name is shortened in the middle (`GitHub - open…
  weight models`), and the counter always stays.
- **The collapsed tool call's `▸ details · Ctrl+O` no longer breaks in two.**
  When it did not fit after a long call header, the label stayed on one row and
  the key landed alone on the next, under the icon. It now moves to the next
  row whole, aligned under the call's name; the "images returned" chip does the
  same.

### Data

- **Chat files schema 1 → 2** (migrated automatically on the first start, after
  the same pre-migration backup): every sub-agent call made by the old
  tool-less `call_subagent` gets a transcript reconstructed from what the call
  already stored — the persona, the question and the reply — so old
  delegations appear as child chats like new ones. Nothing is removed: a
  migrated file is the old file plus the transcripts, and it now records its
  schema version. An older build refuses to open migrated data rather than
  misread it — restore the pre-migration backup to go back.
- **`settings.json` schema 1 → 2** (migrated automatically on the first start,
  after a pre-migration backup): `tools.subagent_timeout_secs` becomes
  `tools.subagent_run_timeout_secs`. A value left at the old default (60)
  takes the new default (600); a value you changed is carried over. A
  `subagent_max_tokens` left at the old default (1024) takes the new one
  (4096). Chat files are unchanged: a sub-agent's transcript is a new,
  optional field on the tool call that made it, and older files read as
  before.

## [0.9.7] — 2026-08-17

### Added

- **The model's name can be shown in the feed.** A new "Model name in the feed"
  toggle in Settings → Interface prints the model that wrote each reply next to
  the `✦ ASSISTANT` header, in the same muted grey as the "thoughts" block. The
  name comes from the message itself, so a conversation reopened after switching
  providers still says which model actually answered — and a reply saved before
  the app recorded that shows nothing. Off by default.

- **New chats name themselves.** After the first reply the model writes the
  conversation a short title — no more lists full of "New chat". On by default;
  a setting (Interface → Behavior) moves it to right after your first message
  (the way cloud chat UIs do it) or turns it off. A chat you renamed yourself
  is never touched, and `Ctrl+R` in the chat list still asks for a fresh title
  whenever you want one.

- **An external server's API key can be entered in settings.** Connecting to a
  server that requires authorization — a gateway such as LiteLLM or OpenRouter, or
  a `llama-server` started with `--api-key` — no longer means setting an
  environment variable first: the `external` mode now has the same "API key"
  field the cloud modes have, stored encrypted and bound to this computer, never
  shown back. It is optional (a local server needs none, and then nothing is
  sent), the "API key (env)" field stays as the fallback for CI and scripts, and
  each `external` tab — Assistant, Impersonation, Embeddings, Speech — keeps its
  own key, because those are four independent servers.

- **`/export` saves a conversation to a file.** `/export` writes it next to
  wherever you started the app, naming the file after the chat and the date;
  `/export path/to/name.md` puts it where you say. Two formats: **md** — exactly
  what `F5` copies — and **json**, which `mindfork-rs import` can read back onto
  the same chat (it carries no tool calls, and the app says so when you use it).
  An existing file is never overwritten. This is also the way out of a browser
  terminal such as JupyterLab's, where the clipboard cannot reach your machine
  at all.

- **Copying works over SSH.** The clipboard the app writes belongs to the
  machine it runs on, so over SSH a copy went to the server — and on a headless
  server, where there is no clipboard at all, it simply failed. A copy is now
  also handed to the clipboard of the machine **your terminal** runs on (OSC 52),
  automatically when the session looks remote; the new "Clipboard over the
  terminal" setting under "Interface" makes that `always` or `off`. Terminals do
  not confirm receiving it, so the app says the text was *sent* rather than
  promising it arrived, and a conversation too large for the sequence (~75 KB)
  is reported instead of being quietly cut in half. Not every terminal supports
  it — GNOME Terminal, Terminal.app and JupyterLab's terminal do not.

- **Every action now has a typed command, not just a key.** In a terminal
  embedded in something else — VS Code's integrated terminal, a JupyterLab
  terminal in a browser tab — the host takes many key combinations for itself
  before the app ever sees them, and some of the app's features were simply
  unreachable there. Nineteen new commands cover the rest of the interface the
  way `/exit` already covered quitting: `/settings`, `/self`, `/chats`,
  `/help`, `/new [profile]`, `/rename [title]`, `/clone`, `/copy`, `/regen`
  (`/retry`), `/takeback`, `/impersonate [text]`, `/stop`, `/find [text]`,
  `/search <text>`, `/links`, `/thoughts`, `/toolcalls`, `/mouse` and `/emoji`.
  Each does exactly what its key does, confirmation prompts included — and,
  unlike a key, a command that cannot run right now says why and tells you what
  will work. `/search` also goes straight to the message search that used to
  take three keys to reach. The keys are unchanged.

  The two actions that lived inside other screens followed: **`/profile list`**,
  **`/profile new [name]`** and **`/profile delete <name>`** (the settings
  screen's `Ctrl+N`/`Ctrl+D`), and **`/self clear`** (`Ctrl+K` twice in the
  self-model screen). Both destructive ones always ask first — a command names
  its target by word, and a shortened name could match a profile you did not
  have in mind — and creating or deleting a profile now leaves a note saying
  what happened, whichever route you took.

- **Links to conversations.** When the assistant mentions another of your
  conversations it now writes its address as `chat://<id>`, and the feed draws
  that as a link. **`Ctrl+L`** lists the conversations the open chat links to
  and opens the one you pick — and with mouse capture on (`Ctrl+W`) you can
  simply click a link, the first thing in the feed that is clickable at all.
  **`Esc` takes you back** to the conversation you came from, the way it already
  does after opening a search hit — the status bar says where it currently goes.
  The way back lasts as long as you are *reading*: start working in the chat you
  arrived at (send, regenerate, take back an exchange, `/compact`, attach a
  file) and `Esc` goes back to meaning "the chat list", so it can never
  teleport you out of a conversation you have settled into. The same now applies
  after opening a search hit. Only addresses that really lead somewhere — a
  conversation of the current companion — are drawn as links, so a link is
  never a dead end; the rest stays ordinary text. (Requires the cross-chat
  tools below to be enabled — they are what hands the assistant the addresses.)

- **The assistant can search your other chats — if you let it.** Two new
  tools, `chat_search` and `chat_read`, let the model find and read what other
  conversations of the same profile said (message text only), for questions
  like "we discussed this in another chat". Both are **off by default** and
  not even shown to the model until you enable them per profile in settings →
  Tools; the current conversation, hidden chats and other profiles stay out
  of reach.
- **`/exit` and `/quit` leave the app.** Until now the only ways out were `Ctrl+Q`
  and `F10`, and some terminals keep both keys for themselves — VS Code's
  integrated terminal binds `Ctrl+Q` to the editor and `F10` to the debugger, which
  left no advertised way out at all. A typed command reaches the app whatever the
  host binds. Either spelling works, both quit mid-answer just like the keys, and
  the commands are listed in `F1` → "Commands".

### Changed

- **The `F1` help window looks after its own layout.** The window now grows with
  the terminal (between its old 76×34 and a readable cap of 96×44) instead of
  always taking the small fixed size; in the "Hotkeys" and "Commands" tabs every
  description starts in the same column, wrapped lines hang under that column,
  and related entries sit in small groups with a breathing line between them.
  The "Components" tab lays its tables out like a table of contents — name on
  the left, version and license aligned to the right edge, a faint dotted
  leader between — instead of leaving the right half of the window empty.

### Fixed

- **The `F1` help no longer cuts its descriptions off.** In the "Hotkeys" and
  "Commands" tabs a line too long for the window simply lost its tail mid-word —
  `/image paste` ended at "works in every termin". Long descriptions now wrap onto
  the next line, indented under the text they continue.

## [0.9.6] — 2026-08-13

### Added

- **The Windows installer now shows the license and the disclaimer.** The wizard
  opens with the MIT license (accept it to continue) and then the disclaimer —
  what the models may say and do, what the tools may do on your machine, and what
  leaves it for a cloud provider. It is the same text as the app's `F1` →
  "Disclaimer" tab and the `DISCLAIMER.md` installed next to the program; both
  texts stay in English in the Russian wizard.

- **The assistant can no longer be talked into fetching your local network.**
  `fetch_url` and the pages `web_search` reads now refuse addresses that are not
  on the public internet — your own machine, your LAN, and the address cloud
  providers keep their credentials behind. It matters because the links the
  assistant follows usually come from a page it just read, and such a page can
  ask it to open something on your side of the router. If you *do* want one
  reachable — a wiki or a dashboard on your network — there is a new switch in
  settings → Tools: **Allow local addresses**, off by default. Attaching an image
  by address (`/image attach <url>`), which you type yourself, is unaffected, and
  so is the engine address in settings.

- **You can show the model a picture.** `/image attach <path>` puts an image on
  your next message, `/image paste` takes one straight off the clipboard (so a
  screenshot needs no file at all — `Ctrl+V` does it too, in terminals that let
  the key through; Windows Terminal keeps it for its own paste, which is why the
  command exists), `/image list` shows what is waiting, and
  `/image remove <name|#N>` takes one back off. It works with a local
  vision-capable model (start `llama-server` with a projector — the new
  "Vision projector (--mmproj)" setting, or `MINDFORK_MMPROJ`) and with **all
  four cloud providers** — OpenAI, Claude, Gemini and Grok. png, jpeg, webp, gif
  and bmp are accepted and converted to what the providers take; a large image is
  shrunk once, when you attach it, so it does not cost you upload and tokens on
  every later turn. The status bar shows what is staged and roughly what it will
  cost. If the engine says it cannot see images, the attach is refused up front
  and tells you what to change instead of failing later.

- **An image can be attached by its web address.** `/image attach https://…`
  downloads the picture and stages it for your next message, the same as a file —
  useful when the image is in a browser rather than on disk. The image itself is
  stored in the conversation, so it keeps working later even if the link stops
  working, and it reaches every provider, including the ones that accept no
  remote addresses. An address that serves a page rather than a picture says so,
  and names what to attach instead.

- **A tool's screenshot now reaches the model.** When an MCP server's tool
  returns an image, it is shown to the model instead of the old
  "[image content omitted]" note — so a screenshot or a chart a plugin produces
  can actually be looked at. The tool block in the feed says how many images came
  back. At most four per result (the rest are counted out loud, not dropped
  quietly), and each is shrunk to the same limits your own attachments get. New
  switch in settings → "Plugins": **Let servers send images**, on by default —
  turn it off if you would rather no third-party picture reached the model, since
  instructions can be painted into pixels where you would not see them.

- **A blip on a cloud provider no longer costs you the turn.** When a provider
  rate-limits or sheds load (`429`, `5xx`, Claude's "overloaded"), the request
  is retried automatically — three attempts, waiting about a second and then
  two, honouring the provider's own `Retry-After` when it sends one. The status
  bar says which attempt is running and how long the wait is, and `Esc` cuts it
  short. This matters most in a long turn: one blip on the eighth tool round
  used to throw away the whole turn's work. Retries stop as soon as the answer
  starts arriving, so nothing you have already read is ever re-generated and no
  tool runs twice; a provider asking for more than 30 seconds is reported
  instead of hidden behind a spinner. Applies to the cloud providers and to
  "external" servers (a proxy or gateway); a locally managed `llama-server` is
  still recovered by the existing health monitor.

- **Demo mode — try the app without a model.** `mindfork demo` boots the real
  TUI on sample data with a scripted engine: a showcase conversation (a table,
  a flowchart drawn in the terminal, a tool call), a filled chat list, a
  living self-model on `F3`, and streamed canned replies that say plainly what
  they are. No server, no API key; nothing outside a temporary folder is
  touched, and the folder is removed on exit. The feed header honestly labels
  the engine `demo (mock engine)`; a real engine connects any time in settings
  (`Ctrl+P`).

### Fixed

- **A reply that gets cut off now says so.** When the engine failed *after* the
  answer had started arriving — an overloaded cloud provider, a dropped
  connection — generation simply stopped: the half-written reply stayed on
  screen with nothing indicating it was a fragment, and on Claude such a
  truncation was indistinguishable from a finished answer. Every provider's
  mid-answer failure is now reported in the feed, with what the provider said
  and a pointer to `Ctrl+R`; what did arrive is still kept.
- **`Esc` interrupts a request that is still connecting.** The engine clients
  had no connect timeout and ignored cancellation until the first bytes of the
  reply arrived, so a wrong port or a silently dropping firewall left the chat
  generating forever with no way back.
- **A network failure now shows the reason.** "Connection refused" and friends
  were replaced by the bare request URL before reaching the screen.

## [0.9.5] — 2026-08-09

### Added

- **Grok (xAI) as a model provider.** Alongside a local model, OpenAI, Gemini and
  Claude, you can now point mindfork at xAI's Grok models: settings (`Ctrl+P`) →
  "Model/server" → "Mode" → `grok`, then a model name (`grok-4.5`, `grok-4.3`,
  `grok-4.20`…) and an API key from [console.x.ai](https://console.x.ai/). The key
  is entered in settings and stored encrypted for this computer, exactly like the
  other providers', and one key serves chat, impersonation and embeddings.
  Reasoning ("thoughts") and tool calling work as with the other clouds; the
  sampling section shows only the parameters xAI actually honours (temperature,
  top-p, seed, token limit and the reasoning depth) instead of ones it would
  reject or ignore. Note that xAI, like Anthropic, offers no embedding models —
  under a `grok` engine the knowledge base needs an embedder from somewhere else
  (a local server, OpenAI or Gemini) in the same section's "Embeddings" tab.

- **A disclaimer about what the models can say and do** — a new `DISCLAIMER.md`,
  readable in the app on the `F1` → "Disclaimer" tab and shipped in the archives,
  packages and installer next to the license. The app carries no model of its own:
  every word on screen is written by a model you chose and downloaded, it may be
  wrong or harmful, and nothing here filters or moderates it. The notice spells out
  what that means for warranty and liability, for the tools a model can run on your
  machine, and for what leaves it when you use a cloud provider. The MIT license
  itself is unchanged — the disclaimer supplements it and takes nothing away.
  The Russian label of the `F1` hotkeys tab was shortened to make room for the
  new tab.

- **History compression (`/compact`).** A long conversation eventually stops fitting
  the model's context window — the engine then refuses the request outright. The
  older part of a chat can now be folded into a rolling summary that is sent in
  place of those messages. **Nothing is deleted**: the feed, search and export still
  show the whole conversation; only what the request carries changes, and the feed
  marks the boundary with a divider you can unfold (together with the "thoughts"
  blocks, `Ctrl+T`) to read the summary. On by default where it can act, and fully
  switchable off in settings → "Memory" → "Context" — off, the whole history is sent
  exactly as before. It also **happens by itself**: once a conversation reaches a
  share of the model's context window (75% by default, adjustable there), the older
  part is folded in the background while you keep typing. The window is worked out
  on its own — from a managed server's own setting, or by asking a llama.cpp server
  — and you can state it yourself for a model that cannot be asked. And if the
  window fills up anyway, the error now says what to do about it instead of showing
  the server's raw reply. Finally, **a summary is no longer the end of the story**:
  what it had to leave out is still reachable, because the assistant can read the
  folded part back page by page and search it by words — so asking about a detail
  from the beginning of a long conversation gets an answer from the actual messages
  rather than a guess. The search needs no embedding server, and the page size is in
  the same settings group. **Writing a message as the user** (`Ctrl+U`) sends the
  same folded conversation, so it no longer runs into the ceiling the rest of the
  chat is already protected from.
- **The Windows installer can set up the Python sandbox for you.** A new
  *Install the Python sandbox and enable Python execution* checkbox on the
  "Additional tasks" page (off by default) installs it during the installation and
  switches the `python_exec` tool on, instead of leaving you to run
  `mindfork-rs sandbox setup` and find the setting yourself. It downloads about
  300 MB and takes a few minutes, with the progress visible in a console window;
  if it fails, the installation still succeeds and the tool stays off — it is
  never enabled without a working sandbox. The Linux packages don't offer this —
  they install as root, while the sandbox belongs to your user account, so run the
  command yourself after installing.
- **`mindfork-rs sandbox setup --enable-python`** does the same from the command
  line: install the sandbox, then turn Python execution on — but only if the
  install succeeded. Without the flag the setting is left untouched.
- **Tool calls in the feed fold away, like "thoughts".** `Ctrl+O` collapses and
  expands them; collapsed, a call keeps its header — the tool's name and a short
  argument — so you still see *what* ran, while the arguments and the result move
  out of the way. **Collapsed is the new default**, so a turn full of tool work
  reads as the reply it produced. The collapsed/expanded choice for both kinds of
  block (`Ctrl+T` — "thoughts", `Ctrl+O` — tool calls) is now remembered **for
  each chat separately** and survives a restart: one conversation can be read with
  everything open while another stays compact. **Expanded, the call is laid out
  properly**: the tool's name on the header line, every argument listed under it
  one per line, a gap, then the result. The header line has always been a
  summary — it cuts a long value at a hundred characters and cannot show a list
  or a nested value at all — so a request could not be read in full anywhere.
- **MCP servers are configured in the settings window.** A new **"Plugins"**
  section holds the master switch, the server list and its editor: `Ctrl+N` adds
  a server, the fields below set its command line and environment, `Ctrl+D`
  deletes it — no more hand-editing `settings.json` (which still works and is the
  same data). A server added here starts switched off, so nothing is launched
  while you are still typing its command, and an identifier that would make the
  server invisible is refused as you enter it.
- **An MCP server's token can be entered in the app.** List the variables the
  server needs by name (`GITHUB_TOKEN, SLACK_TOKEN`) and a row appears for each:
  press `Enter` and type the value into a masked field, `Del` deletes it. The
  value is encrypted with a key belonging to **this computer** and never appears in the settings file — the
  same storage the cloud API keys and the backup password use, so copying the
  configuration elsewhere is still safe (on another computer you enter the value
  again). Scripted setups are unaffected: the server inherits the application's
  own environment, so a variable you already set in the system reaches it
  without being listed at all. Previously a hosted server (GitHub, Slack, …)
  meant setting a system variable and restarting the application.
- **A configuration from another MCP client can be imported.** "Import from a
  file" in the "Plugins" section takes the path of a `mcpServers` JSON —
  Claude Desktop's `claude_desktop_config.json` and the clients that share its
  shape. Imported servers arrive **switched off** so nothing starts unreviewed,
  their tokens are stored encrypted for this computer rather than written to the
  settings file, a server whose name is already taken is skipped (so importing
  twice changes nothing), and entries this application cannot run — the ones
  that connect over the network rather than as a program — are reported instead
  of quietly disappearing.
- **One MCP server config now works on every platform.** The launch command is
  resolved the way a shell resolves it, so `"command": "npx"` no longer has to be
  written as `cmd /c npx …` on Windows — configs can be copied between machines
  and between operating systems as they are. Existing `cmd /c …` configs keep
  working.
- **A stuck MCP server can be reconnected from settings.** `Enter` on a server's
  row confirms a changed tool catalog as before — and, when there is nothing to
  confirm, restarts the server. Previously a server that had crashed too often
  could only be brought back by restarting the application.

- **The assistant can watch a YouTube video** — the new `youtube_watch` tool
  tells what is **said and shown** in it, with timestamps, and `focus` narrows
  that to your question. It needs a Gemini API key, but **not** a Gemini chat:
  the tool calls Gemini itself, so this works with a local model or with Claude
  just the same (Gemini is currently the only provider that accepts video at
  all). Long videos are refused with a suggestion to ask for a segment —
  watching is billed per second of footage — and the ceiling, the model and the
  frame-sampling detail live in settings → "Tools" → "Video". Without a key the
  tool still returns the title, channel, length and the author's description,
  and says plainly that it could not watch. Only public videos.
- **A YouTube link is no longer a dead end for `fetch_url`.** It used to answer
  "failed to extract readable text" (a watch page carries none); now it returns
  the same free metadata and points at `youtube_watch`.
- **`youtube_watch` can bring back the words, not just a description.** Pass
  `transcript: true` and the assistant also gets a transcript of the speech with
  timestamps. If it is short, it comes straight back in the answer; if it is
  large, it is **attached to the chat** (it shows up in `/file list`, and the
  assistant reads it page by page or searches it by meaning) instead of filling
  the conversation. The transcript costs exactly as much as watching — it is the
  same request — so it is not requested by default. Timestamps are always counted
  from the start of the video, even when you asked for a segment.

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

- **A fetched page keeps its code examples.** `fetch_url` used to hand the
  assistant prose only — section headings and every code block were dropped
  before it ever saw the page. On documentation that is not a cosmetic loss:
  each "here is an example:" led nowhere, so the assistant concluded the page had
  arrived damaged and went looking for the source elsewhere, spending several
  tool calls on it. Headings and code (fenced, with the language) now come
  through in their place in the text.
- **A page too large for one answer is attached to the chat instead of being
  cut.** It arrives as an ordinary attachment — visible in `/file list`, read
  page by page and searchable by meaning — so nothing is lost and the assistant
  can reach the parts a summary skipped. Previously such a page was silently
  truncated mid-word with nothing saying so, which made a long page
  indistinguishable from a complete one. A ceiling still exists for genuinely
  enormous pages, and reaching it is now stated in the answer. Such an
  attachment is named after the page's own heading rather than the browser tab
  title — many documentation sites give every page the same tab title, which
  would have left two attached pages sharing one name and the assistant reading
  whichever came first. If two attachments do end up sharing a name, reading one
  by that name now says so instead of picking one.
- **`Home` and `End` reach further with each press.** In the input box they
  still go to the start/end of the row you see on screen — but pressing the same
  key again, when the cursor is already there, now goes on to the whole line you
  typed. Previously a line broken across several rows by word wrap could only be
  traversed with `Ctrl+Home`/`Ctrl+End`, which jump to the ends of the entire
  text. `Home` also stops at the **first non-space character** before the line's
  very beginning, so an indented line is entered at its text.

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

- **The assistant's model of *you* was not reaching it.** The self-model block the
  assistant carries into every conversation is assembled from four parts — its
  self-description, its active goals, what it has concluded about you, and its
  recent observations — but only the description had a size limit, so on a
  mature profile the description and goals used up the whole block and **the
  other two were silently dropped**. Measured on real data: everything the
  assistant had recorded about the interlocutor, and every observation it had
  written about itself, never left the database. Each part now gets its own share
  of the space, and what one part does not use goes to the next; when a list has
  to be shortened it drops whole entries and says how many are hidden, instead of
  cutting one in half. The assistant can still read the whole thing at any time —
  that view was never truncated. The default size of the block was also raised
  (1200 → 4000 characters), which mostly matters for a self-model that has grown;
  an existing installation keeps its own setting, in "Memory" → "Self-model".

- **The token counter now shows the real number, not an estimate.** With a
  llama.cpp server the exact count it reports arrived a moment *after* the reply
  ended, and was being discarded — so the counter kept showing its own `~`
  approximation, which is off by well over half on some kinds of text.
- **Two labels in the feed ignored the interface language** and were always
  Russian: the heading above an **expanded** "thoughts" block, and the exit-code
  line under a `python_exec` console.
- **Web search no longer reports "nothing found" when it was actually blocked.**
  One of the search engines serves its "prove you are human" page with an
  ordinary success status, so it counted as a normal answer that happened to
  contain no results — and that suppressed the honest "every search engine is
  refusing us right now" message. The assistant was told the web knows nothing
  about the subject and, quite reasonably, went off inventing ways around it.
- **The assistant is told that Python code does not carry over between calls.**
  Each `python_exec` call gets a fresh sandbox: files written by one call —
  `/tmp` included — are gone by the next. Nothing said so, so the assistant would
  download a large file in one call and find it missing in the next, then
  download it again.
- **`backup` and `restore` no longer look like they have hung.** Packing or
  unpacking real data takes seconds, and until now the commands printed nothing
  until it was all over — hardest to read right after `restore` asks for the
  password, where nothing is echoed either, so there is no sign the password was
  taken. Each step now says what it is doing before it does it (checking the
  archive, saving the previous data, clearing, restoring, compacting the
  database), and packing/unpacking counts its files as it goes.
- **Keys pressed while `backup`/`restore` was working no longer land in the
  shell.** They used to sit in the terminal's buffer untouched and be replayed as
  commands the moment the program exited — pressing `Enter` a few times while
  waiting produced a few stray prompts afterwards. They are discarded on the way
  out: they were typed at mindfork, not at the shell.
- **The settings panel no longer explains the wrong thing.** The line about a
  tool being "disabled by a global switch" used to appear under **any** row
  flagged for attention — an MCP server whose tool catalog had changed, or an
  environment variable whose source is missing — where it is about neither that
  row nor anything the user can act on. It now shows only for a tool that really
  is gated, and names the section that holds the switch: the MCP master toggle
  lives in "Plugins", not "Tools".
- **Appending to a file could silently lose what was appended.** `fs_write` with
  `append` did not flush before closing the file, so the text sometimes never
  reached disk — the file simply stayed as it was.
- **An MCP server's status no longer hides that its tools are switched off.** The
  row now reads `ready · tools: 14 · in profile: 0` and points at the "Profiles"
  section: a server can be running while the model sees none of its tools, because
  they are enabled per profile — previously the row just said "ready", and the only
  way to find out was to ask the assistant and be told it has no such tool.

- **Spellcheck no longer underlines links and email addresses.** A URL
  (`https://…`, `www.…`, a bare `example.com/path`) or an address
  (`user@example.com`, `mailto:` and all) in the input box is skipped whole, so
  its host and path fragments are not flagged word by word, and the suggestions
  popup (`Ctrl+G`) stays quiet inside one. Prose around it is checked as
  before — including a missing space after a period (`end.Next`), which is still
  a typo and not a domain, and a mention like `@username`.
- **A Gemini key can now be entered where `youtube_watch` is configured.**
  The "Model" section only offers a key field for a slot that is actually set to
  that cloud, so with a local or OpenAI setup there was nowhere to enter a Gemini
  key — and the video tool needs one whatever the chat engine is. The "Video"
  group in "Tools" now has its own "API key" row, stored on this computer like
  any other key; it is the same shared Gemini key, so entering it here also
  configures Gemini chat and embeddings.
- **`youtube_watch` no longer sends the assistant hunting for a workaround.**
  When video understanding is not configured (or the provider fails), the
  answer now says plainly that the video's content cannot be obtained any other
  way — captions, downloading and web search are all dead ends — so the model
  answers from what it has instead of spending several tool calls trying to
  scrape subtitles, install packages or find `yt-dlp`.

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

### Security

- An MCP server's token, whether typed in or imported from another client's
  configuration, is stored the way the cloud API keys are: encrypted with a key
  belonging to this computer, never written to the settings file in the clear
  and never shown in the interface. An imported configuration therefore does not
  turn its literal tokens into plaintext on your disk. The same limitation
  applies as for the API keys — this protects the file, not against programs
  running under your own account.

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
history is in the [docs/journal/](docs/journal/) log).

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

[Unreleased]: https://github.com/vshylov/mindfork-rs/compare/v0.11.0...HEAD
[0.11.0]: https://github.com/vshylov/mindfork-rs/compare/v0.10.2...v0.11.0
[0.10.2]: https://github.com/vshylov/mindfork-rs/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/vshylov/mindfork-rs/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/vshylov/mindfork-rs/compare/v0.9.9...v0.10.0
[0.9.9]: https://github.com/vshylov/mindfork-rs/compare/v0.9.8...v0.9.9
[0.9.8]: https://github.com/vshylov/mindfork-rs/compare/v0.9.7...v0.9.8
[0.9.7]: https://github.com/vshylov/mindfork-rs/compare/v0.9.6...v0.9.7
[0.9.6]: https://github.com/vshylov/mindfork-rs/compare/v0.9.5...v0.9.6
[0.9.5]: https://github.com/vshylov/mindfork-rs/compare/v0.9.4...v0.9.5
[0.9.4]: https://github.com/vshylov/mindfork-rs/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/vshylov/mindfork-rs/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/vshylov/mindfork-rs/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/vshylov/mindfork-rs/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/vshylov/mindfork-rs/releases/tag/v0.9.0
