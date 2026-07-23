# Installing and running mindfork-rs

A console (TUI) AI chat app. Platforms: **Windows** and **Linux**.
By default all data lives **next to the binary** (portable): a separate folder/flash
drive is self-contained. The storage mode can be changed (§2.1), and data can be
backed up and restored (§2.2).

## 1. Installation

### Prebuilt binaries (releases)

Prebuilt binaries for **Windows** and **Linux** are published on
[GitHub Releases](https://github.com/vshylov/mindfork-rs/releases): download the
`mindfork-rs-vX.Y.Z-x86_64-{windows.zip,linux.tar.gz}` archive, optionally verify the
checksum against `sha256sums.txt`, unpack it, and run the binary (`mindfork-rs --version`
prints the version). The Linux build is built against glibc 2.35 (`ubuntu-22.04`) and
runs on most current distros (Ubuntu 22.04+, Debian 12+, Fedora 36+, Arch;
RHEL/Rocky 9 with glibc 2.34 is **not** supported).

The portable archive keeps data **next to the binary** (in `data/`); it's a
self-contained folder/flash drive. For an "installed" setup — see the packages below.

### Linux packages (deb / rpm / pkg.tar.zst)

Each release ships system packages:

```bash
sudo apt install ./mindfork-rs_X.Y.Z-1_amd64.deb        # Debian/Ubuntu
sudo dnf install ./mindfork-rs-X.Y.Z-1.x86_64.rpm       # Fedora
sudo pacman -U  ./mindfork-rs-X.Y.Z-1-x86_64.pkg.tar.zst # Arch
```

The packages place the binary in `/usr/lib/mindfork-rs/` (symlinked from
`/usr/bin/mindfork-rs`), the dictionaries next to it, and keep data in the
**standard OS folder** (`~/.local/share/mindfork-rs` — set by the
`/usr/lib/mindfork-rs/defaults.json` file with `{"mode":"system"}`). The interface
language on first run is detected from the system locale. Packages are unsigned —
verify integrity against the release's `sha256sums.txt`.

### Windows installer

Each release ships `mindfork-rs-vX.Y.Z-x86_64-setup.exe` (Inno Setup). It installs
**for the current user without administrator rights** (an "all users" option is
available). The wizard has two custom steps: **application language**
(Russian/English) and **data location** — the standard OS folder
(`%APPDATA%\mindfork-rs\data`, recommended), portable (next to the app), or a custom
folder. The choice is written to `defaults.json` next to the binary and **is not
overwritten on upgrade**. Silent install:
`setup.exe /VERYSILENT /NORESTART`.

The installer is **unsigned** — on first run Windows SmartScreen will show a warning
("Windows protected your PC" → "More info" → "Run anyway"). File integrity can be
verified against the release's `sha256sums.txt`. As an alternative — the portable
`windows.zip` (no install).

### Building from source

You need **Rust** (edition 2024, a recent stable toolchain). On **Linux** you also
need the ALSA headers — speech playback (`/tts`, §4.3) is built via `rodio`/`cpal`:

```bash
sudo apt-get install -y libasound2-dev     # Debian/Ubuntu
sudo dnf install -y alsa-lib-devel         # Fedora
```

On Windows nothing extra is needed (WASAPI via `windows-rs`). The prebuilt packages
pull in the runtime library themselves (`libasound2t64`/`libasound2` / `alsa-lib`).

```bash
cargo build --release        # binary in target/release/
```

The release profile enables LTO and `strip` (smaller size, higher speed). The
profile keeps `panic = unwind` — needed for guaranteed terminal restoration on
panic.

Checks before committing (green on both platforms):

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 2. Where the data lives (portable)

In the `data/` subdirectory next to the executable (separates data from build
tooling/caches; in dev — `target/debug/data/`), the following are created:

| Path | Purpose |
|---|---|
| `settings.json` | global configuration (model/server, sampling, tools, interface) |
| `profiles.json` | AI companion profiles |
| `chats/{id}.json` | chats (+ `.bak` — backup on overwrite) |
| `data.db` | notes and RAG (SQLite + sqlite-vec), isolated by profile |
| `dictionaries/` | spellcheck dictionaries (Hunspell) |
| `personal_dictionary.txt` | personal dictionary |
| `backups/` | backups (see §2.2) |
| `logs/` | logs (stdout is occupied by the TUI) |

In a dev build this is `target/debug/data/` next to the binary.

### 2.1. Installation defaults (`defaults.json`)

By default, data is stored in a `data/` subdirectory **next to the binary**
(portable). This can be changed with a `defaults.json` file **next to the
executable** (the file itself always lives next to the binary, **not** inside
`data/` — it's about installation, not user data; it's not included in backups):

```jsonc
{ "mode": "portable" }                 // data/ subdirectory next to the binary (default)
{ "mode": "system" }                   // standard OS user folder
{ "mode": "path", "path": "D:\\mindfork-data" }  // arbitrary directory
{ "mode": "portable", "default_language": "en" } // + new-profile scaffold language
```

- **`mode`** — storage mode: **`system`** → Windows `%APPDATA%\mindfork-rs\data`,
  Linux `~/.local/share/mindfork-rs`; **`path`** → the given directory (created if
  needed; **without** a `data/` subfolder).
- **`default_language`** — the **agent-scaffold language** (prompts, self-model
  scaffold, tool results) the first profile and new profiles are created with, **and**
  the interface language on a fresh install: `ru` or `en`. This is **not** the
  language the model responds in (that's set by the profile's system message). The
  installer fills this field in based on the language chosen during setup. **The
  field is optional:** if it's absent (e.g. the deb/rpm package writes only
  `{"mode":"system"}`), the language is determined **from the OS locale** — a
  Russian locale → Russian, otherwise English. See [docs/history/i18n.md](history/i18n.md).
- No file, or an empty one → **portable** mode + language from the OS locale. For
  backward compatibility, the old **`location.json`** file (storage mode only) is
  also read if `defaults.json` is absent.
- The file is read with a UTF-8 BOM too. A corrupted JSON file is a startup error
  (so a typo in the path doesn't silently leave you working with an empty data set
  in the wrong place).
- **Spellcheck dictionaries** in a non-portable mode (`system`/`path`) are looked up
  first in the data directory, then in the portable layout **next to the binary**
  (`data/dictionaries/` — where the installer/package places them), so spellcheck
  works under a system install too.

### 2.2. Backup and restore

The commands run without the TUI (the app must be closed) and exit:

```bash
# Create a copy (zip). Without -o, the name is auto-generated under backups/.
mindfork backup
mindfork backup -o D:\copy.zip -c 6     # path and compression level 0..9 (0 — no compression)

# Restore from a copy.
mindfork restore D:\copy.zip
```

The archive includes: `chats/`, `dictionaries/`, `locales/`, `data.db`,
`personal_dictionary.txt`, `profiles.json`, `settings.json`, all `*.bak` files, and
the file tools' "sandbox" directory (`tools.fs_root`) — **only if** it's inside the
data directory. `backups/`, `logs/`, and the defaults files `defaults.json`/
`location.json` are excluded.

**Restoration is transactional.** The archive is validated first; if the data
directory already has something in it, it's automatically saved to `backups/` (a
pre-restore copy), then the data is cleared and the given archive is unpacked. If
unpacking fails (e.g. the archive is corrupted partway through), a **rollback** to
the pre-restore copy is performed — all actions are reported to the console.

### 2.3. External locales (`data/locales/`) — editing text and new languages

All scaffold text (prompts, self-model scaffold, tool results — axis A) and all UI
chrome (axis B) is stored in JSON bundles **baked into** the binary (`ru`/`en`). On
startup the app additionally scans the **`data/locales/`** directory and layers any
files found **on top of** the baked-in ones — you can edit text and add languages
**without rebuilding**. The directory is created automatically (empty).

- **Template for editing.** Export a baked-in bundle to a file with a command (no
  TUI needed):
  ```bash
  mindfork locales export en -o data/locales/en.json    # edit English
  mindfork locales export ru -o data/locales/de.json    # a stub for a new language (translate it)
  ```
  `ru`/`en` are exported as **verbatim source** (with string arrays — convenient to
  edit). An existing file is not overwritten — point to a new path. The full list of
  keys can always be obtained this way, without having the project's source.
- **Editing an existing language.** Place `data/locales/en.json` (or `ru.json`) — an
  **override** file: only the keys present in it are overridden, everything else is
  taken from the baked-in bundle. Keep **only** the keys you're changing in the file
  (a sparse override) — this way future improvements to the baked-in text aren't
  "shadowed" by your full copy. Example — rewriting one UI heading:
  ```json
  { "ui.feed.role.assistant": "AI" }
  ```
- **A new language.** Place `data/locales/<code>.json`, where `<code>` is a language
  code (BCP-47-like: lowercase Latin letters, digits, and hyphen separators — `de`,
  `fr`, `pt-br`, `zh-tw`). The language will appear in the "Scaffold language"
  (profile, axis A) and "Interface language" (axis B) selectors. The reserved key
  `ui.lang.name` sets the language's name in its own script for the selector
  (otherwise the code is shown).
- **Missing-key fallback chain.** By default, missing keys fall back to the Russian
  reference. If the language was translated from English, add the meta key
  `"_fallback": "en"` — then gaps are filled from English instead of Russian. The
  full chain: your file → `_fallback` → `ru` → the key itself (a warning is logged
  once per missing key).
- **Number-neutral wording.** The engine has **no** pluralization (grammatical
  number agreement) by design — all templates are phrased neutrally ("notes: {n}",
  "×{n}"). For languages with complex number agreement (Polish, Czech, …), stick to
  the same style.
- **Fault tolerance.** A corrupted/unreadable file, or an invalid name, gets a
  warning in the log (`logs/`) and is **skipped** (the baked-in text keeps working).
  Content is additionally checked against the reference: an unknown key (a typo) or
  a mismatch in `{…}` placeholders gets a warning in the log. File changes take
  effect on **restart**.

## 3. Inference engine (llama.cpp `llama-server`)

The app is an **HTTP client** to a local **OpenAI-compatible** server. The protocol
is universal, so in external mode any such server works (llama.cpp `llama-server`,
vLLM, LM Studio, Ollama …). The recommended and verified backend is
**llama.cpp `llama-server`** (prebuilt Windows/CUDA binaries). The whole chain
(streaming, EOS stop, tool-calling, "thoughts") is verified on Gemma 4 E4B-it.

> Originally designed around `xinfer`, but it turned out too raw (incoherent
> output on Gemma 4, builds poorly on Windows). The protocol is the standard
> OpenAI-compatible one (`/v1/chat/completions` streamed over SSE, `/v1/embeddings`),
> spoken by llama.cpp as well as vLLM/LM Studio/Ollama; key fields and quirks
> (`reasoning_content`, `tool_calls`, EOS) are described below and in the "Inference
> engine" / "Sampling parameters" sections of the spec ([spec.md](../spec.md)).

Two modes (configured on the settings screen, `Ctrl+P`, "Model/server" section):

- **managed** — the app itself launches a child `llama-server` (path to the binary
  + GGUF model `-m`, `-ngl`, `-c`, `--jinja`, `--no-mmap`, host/port). `--no-mmap`
  loads the weights fully into RAM instead of mapping the file from disk — helps on
  network/slow drives, but needs more memory (off by default). Changing the model in
  settings **restarts** the server. Before launch the model file's presence is
  checked: if the GGUF isn't found or isn't accessible, the status bar immediately
  shows a clear error ("model file not found or inaccessible …") instead of hanging
  in "connecting…".
  - **FlashAttention** (`--flash-attn`): `auto`/`on`/`off` (default `auto` — llama.cpp
    decides). Speeds up attention and saves VRAM on supported GPUs.
  - **Speculative decoding** (`--spec-type`): speeds up generation with a "draft"
    running ahead. `draft-*` requires a separate draft model (`-md`, plus `-ngld`,
    `--spec-draft-n-max/-n-min`); for **MTP models** (`mtp-gemma-4-12B-it.gguf`) —
    `draft-mtp`; `ngram-*` don't require one. Draft-model fields in settings are
    shown only for `draft-*` types; the draft model's path is also checked before
    launch.
- **external** — connects to an already-running server by URL.

Example of a manual launch (external):

```bash
llama-server -m google_gemma-4-E4B-it-Q4_1.gguf \
  --host 0.0.0.0 --port 8000 \
  -ngl 99 -c 8192 \
  --jinja          # the model's built-in chat template — required for correct
                   # Gemma formatting and for tool-calling
```

"Thoughts" (`reasoning_content`) are enabled on `llama-server` with the
`--reasoning-format` flag (e.g. `auto`) for thinking models; otherwise mindfork
falls back to parsing `<think>…</think>` from the text.

### 3.1. Cloud providers and API keys

Besides a local server, the engine can be a cloud: **OpenAI**, **Google Gemini**, or
**Claude** (Anthropic). The mode is chosen in settings (`Ctrl+P` → "Model/server" →
"Mode" field), where the model name is also set.

**The key is entered right in settings** — the "API key" field under the model
name: `Enter` opens a blank input (characters hidden as `•`), `Enter` saves, `Del`
removes it. The key is stored in `settings.json` **encrypted and bound to this
computer** (Windows — the system DPAPI; Linux — a key derived from
`/etc/machine-id`), so:

- the settings file can be **moved between computers** — on a new one the key won't
  decrypt and needs to be entered again (it will land as a separate entry), and
  moving back to the original computer the original key reads again;
- a saved key **cannot be viewed or copied** from the app — editing means re-entering
  it; the field only shows "configured (this computer)";
- one key serves **chat, impersonation, and embeddings** for that provider.

The key is protected against moving/copying the file, but not against programs
running under your own user account on the same computer (this is how browser
password managers work too).

**Alternative — an environment variable** (for CI, scripts, and systems without
`machine-id`): the "API key (env)" field stores the **name** of the variable, e.g.
`OPENAI_API_KEY`, and the key itself is read from the environment. A key entered in
settings takes priority; the env one is used if no key was entered.

### Quick start via environment variables (dev)

Env takes priority over `settings.json` (handy for smoke runs), affecting only the
inference/embedding servers:

```powershell
# external chat server (any OpenAI-compatible one)
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
# or a managed llama-server:
$env:MINDFORK_LLAMA_BIN = "C:\path\to\llama-server.exe"
$env:MINDFORK_MODEL     = "C:\GGUF\google_gemma-4-E4B-it-Q4_1.gguf"
$env:MINDFORK_NGL       = "99"     # GPU layers (opt.)
$env:MINDFORK_CTX       = "8192"   # context (opt.)
$env:MINDFORK_PORT      = "8000"   # opt.
```

Embeddings for RAG — a **dedicated** server (ADR 0002):
`MINDFORK_EMBED_URL` (external) or `MINDFORK_EMBED_BIN` + `MINDFORK_EMBED_MODEL`
+ `MINDFORK_EMBED_PORT` (managed). If not configured — RAG returns an error, but the
app doesn't crash.

> **Embedder physical batch size.** Embedding models are non-causal: the whole input
> must fit into a single **physical batch** (`n_ubatch`). By default llama-server's
> `n_ubatch=512`, and it sets `n_batch` to match — so a chunk longer than ~512
> tokens (for Cyrillic/code that's only a few hundred characters) is rejected with
> "input is too large to process. increase the physical batch size", and the file
> doesn't get indexed. In **managed** mode the app launches the embedding server
> with `-ub`/`-b` sized to the context — large chunks are accepted whole. For an
> **external** embedding server, raise the batch yourself, e.g.:
> `llama-server -m bge-m3-Q8_0.gguf --embeddings --host 0.0.0.0 --port 8001 -ngl 99 -c 8192 -ub 8192 -b 8192`.

### Loading files into the knowledge base (RAG)

Besides the `rag_add` tool (used by the model), files and directories can be
indexed manually — with commands right in the chat input box:

```
/rag add d:\dir\file.txt        # a single file
/rag add d:\docs                # all .txt/.md in the folder (no subfolders)
/rag add d:\docs -r             # recursively, including subfolders
/rag remove d:\docs\file.txt    # remove a file from the base
/rag remove d:\docs             # remove a whole folder (with everything under it)
/rag list                       # sources in the base (chunk count, date)
/rag rebuild                    # reindex the base (after changing settings/model)
```

- Only `*.txt` and `*.md` are supported so far. The path can be quoted if it
  contains spaces.
- **Chunk/overlap sizes** are configurable in the "Tools" section of the settings
  screen (`Ctrl+P`): "RAG: chunk size", "RAG: overlap", "RAG: chunk cap" (in
  characters).
- **`/rag list`** shows the active profile's knowledge base sources — the fragment
  count and indexing date for each.
- **`/rag rebuild`** reindexes the base from scratch with the current chunk sizes
  and embedding model. Each source's original text is stored in the base, so
  reindexing **doesn't require the source files on disk** (and if the text somehow
  wasn't saved — e.g. the source was added by an older version — an attempt is made
  to re-read the file by its path). Needed after changing the chunk sizes **or** the
  embedding model to one with a different vector dimensionality (which used to
  require manual reindexing). If several profiles share the base and the
  dimensionality changed, reindexing needs to be run under each profile (the vector
  dimensionality is shared by the whole base).
- Indexing runs **in the background** with a progress indicator and spinner;
  re-adding the same file **replaces** its fragments instead of creating
  duplicates.
- **Chunking** — follows best practices: splitting at sentence/word boundaries with
  **overlap** (a query near a chunk boundary doesn't lose context), small paragraphs
  are grouped together. `*.md` is split **semantically** — by headings, protecting
  code blocks (a `#` inside ``` doesn't count as a heading). During search, adjacent
  fragments from the same source are **stitched** together by their overlap, without
  duplication.
- Documents are written to the **active profile's** RAG (isolation). A configured
  embedding server is required (see above) — otherwise the command reports the
  embedder as unavailable.
- Removal works by the stored path and **doesn't require** the file to still be on
  disk. Command input is highlighted yellow and not spellchecked.

## 4. Spellcheck dictionaries

The app reads dictionaries from `dictionaries/` **in the data directory**
(portable — `data/dictionaries/`). Place Hunspell pairs `*.aff` + `*.dic`
(`en_US.*`, `en_GB.*`, `ru_RU.*`) into the project root's `dictionaries/` —
`build.rs` copies them into `<profile>/data/dictionaries/` at build time, so
`cargo run` sees spellcheck right away. No directory → spellcheck is simply off.
Active dictionary selection is on the settings screen ("Interface" section).

## 4.1. Python sandbox (`python_exec`)

The `python_exec` tool works in two modes ("Tools" setting → "Python"):

- **Wasmer sandbox** (default) — code runs isolated in WASIX (no access to the
  machine's files; network is a toggle), with preinstalled packages (numpy,
  requests, …). Doesn't need Python on the machine.
- **Local interpreter** — the system `python`/`python3` (the previous behavior, no
  isolation; the path is configurable).

For the sandbox mode, set it up once (downloads `wasmer` ~206 MB, `python.webc`,
and packages into `data/sandbox/`; ~300 MB on disk):

```bash
mindfork-rs sandbox setup          # --force — re-download
```

Provisioning follows a lock list with sha256 verification; at the end the
**compilation cache is warmed up** (`python.wasm` + numpy), so the first real tool
call is already warm. The command doesn't launch the TUI. The tool is enabled by
the master toggle "Python execution" (**off** by default). If the sandbox isn't
set up, the tool returns a clear message (you can switch to local mode instead).
A custom `wasmer` binary can be set via the `MINDFORK_SANDBOX_WASMER` env variable.

## 4.2. MCP server tools (plugins)

Custom model tools are attached via external **MCP servers** (stdio; any server
from the Model Context Protocol ecosystem works). Servers are described in
`settings.json` (the `mcp` section; the file is in the data root, see §2), and
enabling them takes two steps (double opt-in):

```jsonc
"mcp": {
  "enabled": true,                  // master switch (false by default)
  "servers": [{
    "id": "fs",                     // slug [a-z0-9-] — part of tool names mcp__fs__*
    "command": "cmd",               // Windows: npx is a .cmd shim, run it via cmd /c
    "args": ["/c", "npx", "-y", "@modelcontextprotocol/server-filesystem", "D:/work"],
    // Linux/macOS: "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/me/work"]
    "env": { "GITHUB_TOKEN": "MINDFORK_GITHUB_PAT" },  // child variable → NAME of the source variable (secrets aren't in the file)
    "enabled": true,
    "tool_timeout_secs": 60,        // timeout for one call
    "max_result_chars": 20000       // result clip (goes into the prompt)
  }]
}
```

1. Turn on the "MCP servers" master toggle (settings → "Tools" → "Plugins
   (MCP)") — server statuses are shown there too;
2. turn on the desired tools in the profile (settings → "Profiles", "Plugins
   (MCP)" group; focusing the toggle shows the tool's **full description** from
   the server at the bottom).

Notes:

- **`.bat`/`.cmd` as the server command are forbidden** (the BatBadBut
  vulnerability); `npx`/`uvx` servers on Windows are launched as `cmd /c npx …` or
  via a direct exe path.
- **The tool catalog is pinned on first startup** (protection against tampering):
  if a server changes its tool set/descriptions after an update, they won't be
  available to the model until you confirm the new catalog (Enter on the server's
  row in settings).
- An MCP server is an ordinary program running with your user's rights: only
  connect trusted ones. A server that crashes often (3 crashes in 5 minutes) is
  disabled until you fix the settings; server stderr is written to `logs/`.

## 4.3. Speaking messages aloud (`/tts`)

An input-box command reads messages aloud (see spec §11.9):

```
/tts            the last message
/tts N          the last N messages
/tts all        the whole chat conversation
/tts stop       stop
```

The provider is configured **separately from the chat** — the "Speech" tab of the
"Model" section (`Ctrl+P`), because Anthropic has no speech synthesis at all:

| Mode | What to set |
|---|---|
| `openai` (default) | model (`gpt-4o-mini-tts`), voice (`onyx`, `cedar`, …) — key **shared with chat** (ADR 0008) |
| `gemini` | model (`gemini-2.5-flash-preview-tts`), voice (`Kore`, `Puck`, …) — key shared with chat |
| `external` | URL of any OpenAI-compatible TTS server (`http://127.0.0.1:8880/v1`), opt. model/voice |

The OpenAI key is **the same as for chat** — if it's already entered (in settings
or via an env variable), OpenAI speech works with no extra setup. Speed for the
`gpt-4o-mini-tts` model is set with **words** in the "Instructions" field ("speak in
Russian, calmly, a bit slower") — that model de facto ignores the `speed` param.
There's no local engine without a server (a managed sidecar) yet — that's future
work; for offline/non-OpenAI speech use `external` mode with any of
Kokoro-FastAPI / speaches / LocalAI.

Only the "speakable" text from markdown is read aloud: code, ` ```mermaid `
diagrams, tables, and block formulas are skipped with a short note; "thoughts" and
tool calls aren't read. No sound card (headless/SSH) — the command shows "audio
unavailable", the app keeps running.

## 5. Importing from other apps

A one-off idempotent import of profiles and chats from a file in the neutral
**mindfork-import** format (spec — [docs/import-format.md](import-format.md)):

```bash
mindfork-rs import path/to/export.json
```

The file is emitted by an **external converter** that knows the source app's
format (for non-public apps, like LameLLaMA, the converter lives in a separate
private repository). Profiles, chats, and (optionally) global sampling/interface
settings are imported; unknown fields are ignored, a file from a newer format
version is rejected. The command doesn't launch the TUI and exits the process;
re-running it doesn't create duplicates (deterministic ids from stable keys).

> The former `import-lamellama <dir>` command has been removed — its role is now
> played by the "external converter → `mindfork-rs import`" combo.

## 6. Running

```bash
cargo run            # dev
./target/release/mindfork-rs   # release
```

Needs a **real terminal** (TUI). In a headless environment the app "hangs" — that's
normal. Basic keys: `F1` — help, `Ctrl+P` — settings, `Esc` — chat list
(open/close) and cancel generation, `Ctrl+N` — new chat,
`Ctrl+U` — write a message as the user (impersonation), `Ctrl+C` — quit.
Scrolling the feed — `PageUp`/`PageDown` or the mouse wheel; mouse capture for the
wheel is a toggle, `Ctrl+W` (off by default, so native text selection works;
with capture on, text is selected while holding `Shift`).
Ctrl shortcuts work under any keyboard layout (including Russian).

## 7. Live-model smoke tests

Unit tests don't need a server. Scenarios against a real server are marked
`#[ignore]` and run manually with `MINDFORK_ENGINE_URL` set:

```powershell
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
cargo test ignored_smoke -- --ignored --nocapture --test-threads=1
```

Covers: streaming/finish, **anti-self-termination on EOS text** (Qwen's
`<|im_end|>` and Gemma's `<end_of_turn>`), **tool-calling**
(`finish_reason=tool_calls` + `delta.tool_calls` parsing), and **"thoughts"**
(`reasoning_content` → `Thoughts`). Verified green on
`google_gemma-4-E4B-it-Q4_1.gguf` via `llama-server`.
