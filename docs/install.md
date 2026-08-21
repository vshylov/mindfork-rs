# Installing and running mindfork-rs

Project site: [mindfork.io](https://mindfork.io) ·
sources and releases: [GitHub](https://github.com/vshylov/mindfork-rs).

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
available). It opens with the **MIT license** (which has to be accepted to
continue) and the **disclaimer** — the same text as
[DISCLAIMER.md](../DISCLAIMER.md) and the app's `F1` → "Disclaimer" tab, covering
what the models may say and do; both are shown in English in either wizard
language, and both files are installed next to the program. Then come two custom
steps: **application language**
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
| `cache.db` | search index over chat content — derived data, safe to delete (see below) |
| `dictionaries/` | spellcheck dictionaries (Hunspell) |
| `personal_dictionary.txt` | personal dictionary |
| `backups/` | backups (see §2.2) |
| `logs/` | logs (stdout is occupied by the TUI) |

In a dev build this is `target/debug/data/` next to the binary.

**`cache.db` is disposable.** It holds the full-text index behind searching chats by
content (`Ctrl+F` in the chat list) — everything in it is derived from `chats/*.json`,
nothing is stored there and nowhere else. It fills in the background: the first launch
after the update runs an initial pass (a few seconds — ~3 s for a corpus of 171 chats),
later launches cost nothing when nothing has changed, and the app stays usable
throughout (search simply covers whatever is indexed so far). If search ever
misbehaves, **deleting the file is a supported repair** — it is rebuilt automatically
on the next launch. It is deliberately **not** included in backups (§2.2), so a
restored copy just rebuilds its own index.

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

# Password-protected copy (AES-256). Without -p the password from the settings
# is used, if one is set there.
mindfork backup -o D:\copy.zip --password "a long passphrase"
mindfork restore D:\copy.zip --password "a long passphrase"
```

**A password can also be set once in the settings** — the "Data" section, the
"Backup password" field. Every backup is then encrypted with it, including the
copies the app makes on its own (the pre-restore copy, and the one taken before a
data-format migration). The password is stored the same way as a cloud API key: it
is encrypted and **bound to this computer**, cannot be shown again, and `Del`
clears it. `--password` overrides the stored one; an empty value means "no
password", so `--password ""` makes a deliberately unencrypted copy without
touching the setting.

Restoring accepts both kinds of archive — encrypted with that password, or not
encrypted at all — so nothing has to be switched when restoring an older copy. A
missing or wrong password is refused **before** any data is replaced, and in a
terminal `restore` simply asks for it (the input is not echoed).

Three things worth knowing before relying on it:

- **Write the password down somewhere other than the copy.** It does not decrypt
  on another computer (that is the point of binding it), so an archive whose
  password lived only in the settings of a machine that died cannot be restored.
- **Use a long passphrase.** The zip format fixes the key-derivation function to a
  weak one, so a short or dictionary password can be brute-forced offline by
  whoever obtains the file.
- **File names and sizes stay visible** without the password — the zip format
  encrypts the *contents* of the entries. Chat titles and messages are inside the
  encrypted files; chat file names are UUIDs.

The archive is standard: 7-Zip, WinZip and the like open it with the password, so
files can be pulled out by hand without the application.

The archive includes: `chats/`, `dictionaries/`, `locales/`, `data.db`,
`personal_dictionary.txt`, `profiles.json`, `settings.json`, all `*.bak` files, and
the file tools' "sandbox" directory (`tools.fs_root`) — **only if** it's inside the
data directory. `backups/`, `logs/`, the disposable `cache.db` (rebuilt after a
restore, §2), and the defaults files `defaults.json`/`location.json` are excluded.

**The database is compacted** on both commands. `data.db` keeps hold of the space
freed by deleted notes, `/rag remove`, and chats whose attachment index went away,
so a backup packs a compacted copy of it (which also makes the archive smaller),
and a restore compacts what it unpacked — including an archive made by an older
version. If the file can't be read as a database it's copied as-is instead; the
backup never modifies the data it's copying.

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
  - **Vision projector** (`--mmproj`): the settings field "Vision projector
    (--mmproj)" sits directly under the GGUF path, and what it enables is image
    input — `/image attach` (spec §9.10). A vision model ships as *two* files: the
    weights and an `mmproj-*.gguf` projector that turns pixels into tokens the
    language model reads. For the reference model
    (`google/gemma-4-31B-it-qat-q4_0-gguf` on Hugging Face) the projector is in the
    same repository, next to the weights — download both. Without it `llama-server`
    serves a text-only model: it still answers normally, but images are refused.
    Like the model and draft paths, the projector's path is checked before launch (a
    typo gives an immediate "projector file not found …" instead of a server that
    quietly has no vision), and changing it **restarts** the server.
- **external** — connects to an already-running server by URL.

**How the app knows whether images are accepted.** It asks the server, rather than
matching model names: `llama-server` reports `modalities` on its `/props` endpoint
(`{"vision": true, "video": true, "audio": false}`) — the same request that already
supplies the context window. `vision: true` → images are accepted, `false` → the
attach is refused with a message pointing at the `--mmproj` field. A server that
reports no `modalities` at all (vLLM, LM Studio, a proxy, any cloud) is treated as
"cannot say" and the attach is allowed: a send-time provider error is a better
outcome than refusing a setup that works. The four cloud providers answer
"supported" outright — every current-generation model on them takes images.

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

Besides a local server, the engine can be a cloud: **OpenAI**, **Google Gemini**,
**Claude** (Anthropic), or **Grok** (xAI). The mode is chosen in settings
(`Ctrl+P` → "Model/server" → "Mode" field), where the model name is also set
(e.g. `gpt-4o`, `gemini-2.5-pro`, `claude-opus-4-8`, `grok-4.5` — keys are
issued at `console.x.ai` for the last one).

Neither Anthropic nor xAI offers embeddings, so under a `claude`/`grok` engine
RAG needs a separate embedder (a local `llama-server --embeddings`, OpenAI, or
Gemini) — set it in the same section's "Embeddings" tab.

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
- one key serves **chat, impersonation, embeddings and speech** for that provider.

The key is protected against moving/copying the file, but not against programs
running under your own user account on the same computer (this is how browser
password managers work too).

**Alternative — an environment variable** (for CI, scripts, and systems without
`machine-id`): the "API key (env)" field stores the **name** of the variable, e.g.
`OPENAI_API_KEY`, and the key itself is read from the environment. A key entered in
settings takes priority; the env one is used if no key was entered.

**An external server's key works the same way.** In `external` mode — connecting to
an OpenAI-compatible server you run or rent (a local `llama-server`, vLLM, LM Studio,
or a gateway like LiteLLM or OpenRouter) — the same "API key (opt.)" field appears
above "API key (env, opt.)", with the same behaviour and the same encrypted storage.
Two differences from a cloud key:

- it is **optional**: a local `llama-server` requires no authorization, and with no
  key stored and no variable named, none is sent — start such a server with
  `--api-key <key>` if you do want it to require one;
- it belongs to **that one server**, not to a provider: the "Assistant",
  "Impersonation", "Embeddings" and "Speech" tabs each hold their own external URL, so
  each holds its own key. A cloud gateway for chat beside a local embedding server is
  the normal case, and sharing one key would send the gateway's token to localhost.

### Quick start via environment variables (dev)

Env takes priority over `settings.json` (handy for smoke runs), affecting only the
inference/embedding servers:

```powershell
# external chat server (any OpenAI-compatible one)
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
# or a managed llama-server:
$env:MINDFORK_LLAMA_BIN = "C:\path\to\llama-server.exe"
$env:MINDFORK_MODEL     = "C:\GGUF\google_gemma-4-E4B-it-Q4_1.gguf"
$env:MINDFORK_MMPROJ    = "C:\GGUF\mmproj-google_gemma-4-E4B-it.gguf"  # vision (opt.)
$env:MINDFORK_NGL       = "99"     # GPU layers (opt.)
$env:MINDFORK_CTX       = "8192"   # context (opt.)
$env:MINDFORK_PORT      = "8000"   # opt.
```

Embeddings for RAG — a **dedicated** server (ADR 0002):
`MINDFORK_EMBED_URL` (external) or `MINDFORK_EMBED_BIN` + `MINDFORK_EMBED_MODEL`
+ `MINDFORK_EMBED_PORT` (managed). If not configured — RAG returns an error, but the
app doesn't crash. The same server also powers **search inside large files
attached to a chat** (`/file attach`, spec §9.7): with no embedder those files are
simply not indexed (a note says so) and are still read page by page.

> **Input prefixes.** e5-family models expect each input marked with its role
> (`query: ` / `passage: `; the `-instruct` variants want an instruction on the
> query and a bare passage). Pick the convention in settings — Model →
> Embeddings → "Input prefixes"; the default `none` suits bge-m3 and most other
> models, and applying a marker to a model that does not want one measurably
> *worsens* retrieval. Changing this setting changes the vector space, so the app
> treats it as a model change: memory re-embeds itself and `/reindex` rebuilds
> the knowledge base. See spec §9.3.5.

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
/rag rebuild                    # reindex the base (after changing the chunk settings)
/reindex                        # re-embed everything (after an embedding-model change)
```

- Only `*.txt` and `*.md` are supported so far. The path can be quoted if it
  contains spaces.
- **Chunk/overlap sizes** are configurable in the "Tools" section of the settings
  screen (`Ctrl+P`): "RAG: chunk size", "RAG: overlap", "RAG: chunk cap" (in
  characters).
- **`/rag list`** shows the active profile's knowledge base sources — the fragment
  count and indexing date for each.
- **`/rag rebuild`** reindexes the active profile's base from scratch with the
  current chunk sizes — needed after changing them. Each source's original text is
  stored in the base, so reindexing **doesn't require the source files on disk**
  (and if the text somehow wasn't saved — e.g. the source was added by an older
  version — an attempt is made to re-read the file by its path). After an
  **embedding-model** change use `/reindex` instead (below): re-chunking is not
  what a model change calls for.
- **A change of the embedding model is detected automatically** — including a
  swap between two models of the *same* vector size, which nothing could see
  before. Vectors made by one model are meaningless to another, so on the first
  use of a new one the app reports it and sets aside everything the previous
  model indexed — without deleting anything. Memory (notes and self-observations)
  rebuilds itself as you use it; the search indexes of attached files and the
  knowledge base wait for `/reindex`. Until the base is rebuilt, `rag_search`
  refuses over it instead of answering from vectors it cannot compare. On a first
  run there is nothing to compare against, so nothing is reported.
- **`/reindex`** re-embeds every stored vector with the current model, in one
  pass over the whole database — memory, the attached-file indexes and **every**
  profile's knowledge base (the embedding server is global, so a model change
  invalidates them all at once). It works from the text already stored, so it
  needs no source files, repairs old entries whose file is long gone, and brings
  attachment indexes back without re-attaching each file. It runs in the
  background with a progress banner and is safe to interrupt: whatever it has
  already rebuilt stays rebuilt, and running it again continues from there. If
  the new model has a different vector size, that is handled by the same pass.
- **Memory's similarity checks adapt to the model.** The gates that decide when
  two notes, observations or traits say the same thing are cut-offs on a cosine
  score, and every model rates similarity on its own scale — on
  `multilingual-e5-large-instruct` the usable range is about 2.6× narrower than
  on `bge-m3`, so a cut-off tuned for one lands in the wrong place on the other.
  When the app first notices a model it measures that scale (one extra request
  of 32 short strings, alongside the change check above) and shifts the cut-offs
  to match. Nothing changes for a setup that hasn't changed models, and if the
  measurement fails the previous behaviour is kept.
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
  pandas, requests, beautifulsoup4, …). Doesn't need Python on the machine.
- **Local interpreter** — the system `python`/`python3` (the previous behavior, no
  isolation; the path is configurable).

For the sandbox mode, set it up once (downloads `wasmer` ~206 MB, `python.webc`,
and packages into `data/sandbox/`; ~300 MB on disk):

```bash
mindfork-rs sandbox setup                    # --force — re-download
mindfork-rs sandbox setup --enable-python    # …and switch the tool on afterwards
```

`--enable-python` turns on "Python execution" (`tools.python_enabled`) in the
settings **after** a successful setup — never on failure, so the tool is never
enabled without its assets. Without the flag the setting is left alone, and the
default stays off.

The **Windows installer** can do all of this for you: the "Additional tasks" page
has an *Install the Python sandbox and enable Python execution* checkbox (off by
default), which runs exactly the second command above right after the files are
copied, in a console window that shows the download progress. If the download
fails, the installation still succeeds and the tool stays off — the sandbox is
optional and the command can be re-run at any time. The **Linux packages
(deb/rpm/pkg.tar.zst) deliberately don't offer this**: they install
non-interactively as root, while the sandbox lives in the *user's* data directory
(`~/.local/share/mindfork-rs/sandbox`), so root couldn't provision it for the right
user anyway — run the command yourself after installing.

Provisioning follows a lock list with sha256 verification; at the end the
**compilation cache is warmed up** (`python.wasm` + numpy), so the first real tool
call is already warm. The command doesn't launch the TUI. The tool is enabled by
the master toggle "Python execution" (**off** by default). If the sandbox isn't
set up, the tool returns a clear message (you can switch to local mode instead).
A custom `wasmer` binary can be set via the `MINDFORK_SANDBOX_WASMER` env variable.

## 4.2. MCP server tools (plugins)

Custom model tools are attached via external **MCP servers** (stdio; any server
from the Model Context Protocol ecosystem works). Servers are configured in the
settings screen — the **"Plugins"** section: `Ctrl+N` adds a server, the fields
below it are its command line and environment, `Ctrl+D` deletes it. The same data
can be edited by hand in `settings.json` (the `mcp` section; the file is in the
data root, see §2) — which is what the example below shows. Enabling a server
takes two steps (double opt-in):

```jsonc
"mcp": {
  "enabled": true,                  // master switch (false by default)
  "servers": [{
    "id": "fs",                     // slug [a-z0-9-] — part of tool names mcp__fs__*
    "command": "npx",               // the same on every platform (see the note below)
    "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/work"],
    "env": { "GITHUB_TOKEN": "" },  // the variables the server needs; "" = the value is entered in the app, a name = take it from that OS variable (no secrets in the file either way)
    "enabled": true,
    "tool_timeout_secs": 60,        // timeout for one call
    "max_result_chars": 20000       // result clip (goes into the prompt)
  }]
}
```

1. Turn on the "MCP servers" master toggle (settings → "Plugins") — the server
   editor and the live statuses are in the same section;
2. turn on the desired tools in the profile (settings → "Profiles", "Plugins
   (MCP)" group; focusing the toggle shows the tool's **full description** from
   the server at the bottom).

Both steps are required, and the server's status row says where you are: `ready ·
tools: 14 · in profile: 0` means the server is up and the model still sees none of
it — step 2 is missing.

**Tokens for a hosted server** (GitHub, Slack, …) are entered in the same section.
List the variables the server needs in its "Variables" row, comma separated and by
name alone (`GITHUB_TOKEN, SLACK_TOKEN`) — a row then appears underneath for each of
them: `Enter` opens a masked field for the value, `Del` deletes a stored one. The
value is **encrypted with this computer's key** (the same storage as the cloud API
keys, §3.1, and the backup password, §2.2), so it never lands in `settings.json` and
it is **not** carried along if you copy the file to another machine — enter it again
there.

The list holds only *overrides*: the server already inherits the application's own
environment, so a variable you have set in the OS reaches it without being listed at
all — which is what makes CI and scripted setups work unchanged. Write
`GITHUB_TOKEN=OTHER_NAME` only when the value has to come from a variable with a
**different** name; such a variable gets no value row — instead its row reports
whether that OS variable is actually there. Note that the application sees the
environment it was **started with**: a variable you set after launching it reads as
missing until you restart the application.

**Importing an existing configuration.** The "Import from a file" row takes the
**path** of an `mcpServers` JSON — the format used by `claude_desktop_config.json`
and the clients that copied it (Windows
`%APPDATA%\Claude\claude_desktop_config.json`, Linux
`~/.config/Claude/claude_desktop_config.json`). A path rather than pasted text on
purpose: the file usually carries live tokens, which would otherwise be sitting on
your screen. Every `env` value in it is stored as an encrypted value as described
above, so nothing is written to `settings.json` in the clear. Imported servers arrive
**switched off** — review the command, then turn "Enabled" on. A server whose name
already exists is skipped rather than overwritten (so re-importing is harmless; to
refresh one, delete it first), and entries that aren't stdio (`"type": "sse"`/
`"http"`) are skipped as well — only stdio servers are supported.

Notes:

- **The command is resolved the way a shell resolves it**, so one config works on
  every platform: `"command": "npx"` needs no `cmd /c` wrapper on Windows. (Why it
  is needed at all: `cmd.exe` completes a bare name using `PATHEXT`, and Rust does
  not — `npx` on Windows is really `npx.cmd`. Note npm ships an extensionless
  `npx` next to it, a Unix shell script Windows cannot run, so a bare name is
  always completed from `PATHEXT` and never taken as-is.) A `.bat`/`.cmd` command
  is allowed: `std` escapes batch-file arguments and refuses the ones it cannot
  escape, which is stricter than routing them through `cmd.exe`.
- **The tool catalog is pinned on first startup** (protection against tampering):
  if a server changes its tool set/descriptions after an update, they won't be
  available to the model until you confirm the new catalog (Enter on the server's
  row in settings). With nothing to confirm, the same Enter **reconnects** the
  server — the way to bring one back after you have fixed whatever it needed.
- A server added in the settings screen is created **switched off**: fill in the
  command and arguments, then turn "Enabled" on.
- **No secret is ever written to `settings.json`**, by either route: the environment
  row holds the *name* of an operating-system variable, and a value entered in the
  app is stored encrypted for this computer. Undo (`Ctrl+Z`) restores settings, not
  stored values — undeleting a server does not bring its tokens back, and renaming a
  server means entering them again under the new name.
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

## 4.4. Watching YouTube videos (`youtube_watch`)

The `youtube_watch` tool tells the assistant what a video **says and shows** —
a description with timestamps, not a raw transcript. Configured in the "Video"
group of the "Tools" section (`Ctrl+P`):

| Field | Meaning |
|---|---|
| Model | the Gemini model that watches (`gemini-3.5-flash` by default) |
| Input resolution | how finely frames are sampled — **measured to change nothing on Gemini 3.x**; on 2.5 it is `low` ≈ 100 vs `medium` ≈ 295 tokens per second of video |
| Max video length | refuse anything longer (default 30 min; `0` — no ceiling) |
| API key | the Gemini key, stored on this computer (encrypted with a machine key, ADR 0008) |
| API key (env) | env-variable name — a fallback when no key is stored in settings |

**The key is the shared Gemini one** (ADR 0008): if it is already entered for
chat or embeddings, nothing else is needed — and if it is not, enter it right
here. That row exists because the "Model" section only shows a key field for a
slot whose mode *is* that cloud, so with a local or OpenAI setup there would
otherwise be nowhere to put a Gemini key at all. It works whatever your chat engine
is — including a local `llama-server` — because the tool calls Gemini itself and
returns text into the conversation. Gemini is currently the only provider that
accepts video at all; OpenAI and Anthropic take text and images only.

Cost is per second of footage, not per video: on the default model a 10-minute
video is ~55k tokens on Google's side (a few hundred in your conversation, since
only the answer comes back). Hence the length ceiling — above it the tool
refuses and suggests a segment, and the model can pass `start`/`end` in seconds
to watch just part of a long talk.

Without a key the tool still works in a reduced form: title, channel, length and
the author's description, read from the public watch page. The same metadata is
what `fetch_url` now returns for a YouTube link, instead of failing to find
readable text on it. Only **public** videos can be watched — not private or
unlisted ones. Both paths need the web-access switch on.

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

To look around before configuring anything, run the **demo**: sample data and
a scripted engine in a throwaway temp folder (removed on exit), no model or
key needed —

```bash
mindfork-rs demo
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

The full set (memory, self-model, notes, RAG, attachments, MCP) additionally
needs an embedding server in `MINDFORK_EMBED_URL`; smokes whose variable is
unset are silently skipped.

### 7.1. Against an authenticated server (`MINDFORK_ENGINE_KEY`)

The URL variables have optional key companions — `MINDFORK_ENGINE_KEY` and
`MINDFORK_EMBED_KEY` — sent as `Authorization: Bearer`. Unset or empty means no
header at all, i.e. exactly the behaviour above against a local `llama-server`.
Setting them points the same smokes at **any** authenticated OpenAI-compatible
server (a hosted endpoint, a proxy):

```powershell
$env:MINDFORK_ENGINE_URL = "https://<endpoint>/v1"
$env:MINDFORK_ENGINE_KEY = "<token>"
cargo test -- --ignored --nocapture --test-threads=1
```

### 7.2. The remote gate (rented GPU, no local stack)

`tools/e2e_hf.py` runs the same suite against **ephemeral Hugging Face Inference
Endpoints**, so the live gate no longer requires a machine with a GPU. It rents
three: a real `llama-server` (HF's llama.cpp engine, the model on an L40S), an
embedding endpoint (the same engine in `embeddings` mode, bge-m3 Q8_0 on a T4),
and a *second, different* embedding model (multilingual-e5-large-instruct q8_0)
for the smokes that guard against an embedding-model change. All deliberately
the same GGUFs and quantizations as the local stack, because the memory gates'
similarity thresholds are calibrated against exactly those models.

The chat endpoint runs **one model per run**, chosen with `--chat-model`:
`gemma-4-31b` (the default, and what the thresholds were calibrated against) or
`qwen-3.6-27b`. The name is one decision — it carries the repository, the
weights and the vision projector together, so an impossible combination cannot
be typed. The projector is deployed with the model: three smokes require one and
**fail rather than skip** on a text-only server, by design.

```powershell
$env:HF_TOKEN = "hf_..."          # fine-grained, Inference Endpoints
python tools/e2e_hf.py run        # create, run the suite, delete
python tools/e2e_hf.py run --dry-run          # payloads only, spends nothing
python tools/e2e_hf.py run --chat-model qwen-3.6-27b   # the other model family
python tools/e2e_hf.py run --filter e2e_live  # a subset
python tools/e2e_hf.py run --no-alt-embed     # skip the second embedding model
python tools/e2e_hf.py list                   # what is running right now
python tools/e2e_hf.py sweep --dry-run        # what the sweeper would remove
```

The token needs **two** boxes ticked under User Permissions → Inference:
*Manage Inference Endpoints* and *Make calls to Inference Endpoints*. Holding
only the first creates an endpoint that then rejects every request — i.e. it
fails after the meter has started. `python tools/hf_probe.py doctor` reports
which one is missing, and distinguishes that from the other causes of a 403 (no
payment method on the account, or an org token pending approval).

**Cost and the guarantee.** A run is ~25 minutes and ~$1 (L40S $1.80/hr + two
T4s at $0.50/hr, billed by the minute). The endpoints are deleted from `finally`, from
`atexit` and from the SIGINT/SIGTERM handler, and the deletion is **verified** —
a failed delete exits non-zero even when the tests passed. If the process is
killed outright, the endpoints scale to zero after their idle window (15 min, so
≈ $0.57 worst case) and the sweeper removes them, which also reclaims
endpoint quota. Use `--keep` only when debugging, and delete by hand afterwards.

In CI: **Live e2e (HF Inference Endpoints)** — `workflow_dispatch` only, with
test-filter, model and GPU inputs; it needs the `HF_TOKEN` repository secret. **Live e2e
sweeper** runs every six hours as the backstop (hourly until 2026-08-21; the cadence
bounds how long an orphan holds endpoint quota, not money). Design and decisions:
[docs/history/remote-e2e-hf.md](history/remote-e2e-hf.md),
[docs/research/remote-e2e-gpu.md](research/remote-e2e-gpu.md).

Two smoke groups still need the local machine and are **not** covered remotely:
the managed-server smoke (`MINDFORK_LLAMA_BIN` — it needs a child process of
our own) and the Python sandbox smokes (they need a provisioned `wasmer`
sidecar). The cloud-provider smokes (Anthropic / Gemini / OpenAI / TTS) need
their own keys and are unrelated to the GPU.
