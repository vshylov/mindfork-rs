# Installing and running mindfork

Project site: [mindfork.io](https://mindfork.io) ·
sources and releases: [GitHub](https://github.com/vshylov/mindfork-rs).

A console (TUI) AI chat app. Platforms: **Windows** and **Linux**.
By default all data lives **next to the binary** (portable): a separate folder/flash
drive is self-contained. The storage mode can be changed (§2.1), and data can be
backed up and restored (§2.2).

## 1. Installation

### Prebuilt binaries (releases)

Prebuilt binaries for **Windows**, **Linux** and **macOS on Apple Silicon** (a
preview — see [macOS](#macos-apple-silicon--a-preview) below) are published on
[GitHub Releases](https://github.com/vshylov/mindfork-rs/releases): download the
`mindfork-rs-vX.Y.Z-{x86_64-windows.zip,x86_64-linux.tar.gz,aarch64-macos.tar.gz}`
archive, optionally verify the checksum against `sha256sums.txt`, unpack it, and run
the binary (`mindfork --version` prints the version). The Linux build is built against glibc 2.35 (`ubuntu-22.04`) and
runs on most current distros (Ubuntu 22.04+, Debian 12+, Fedora 36+, Arch;
RHEL/Rocky 9 with glibc 2.34 is **not** supported).

The portable archive keeps data **next to the binary** (in `data/`); it's a
self-contained folder/flash drive. For an "installed" setup — see the packages below.

### The install script (Linux, macOS)

One line puts the portable build into a directory and makes it startable — the
route for a machine that is new every time (a container, a rented GPU box, §3.4),
and a perfectly good one for a desktop:

```bash
curl -fsSL https://github.com/vshylov/mindfork-rs/releases/latest/download/install.sh | sh
```

It is an asset of the release like the archive itself — listed in
`sha256sums.txt`, covered by the same build attestation — so piping it adds no
party to trust that the binary had not already added. If you would rather read
before you run, download it, read it, and run `sh install.sh`; it is one short
POSIX script.

| | |
|---|---|
| `--dir DIR` | where to install; default `~/mindfork`. The app's data lives in `DIR/data` (portable mode, §2) |
| `--version vX.Y.Z` | a particular release; default — the latest |
| `--from DIR` | install from files already on disk (the archive and `sha256sums.txt`) — no network at all. On a Mac, an archive a browser saved is quarantined, and the script then installs it **without starting it** — macOS would ask about it on the screen — and says how to clear the mark; a `.tar` Safari left in place of the `.tar.gz` is refused, with the `curl` line that fetches the one the checksum covers |
| `--no-deps` | do not install the system libraries (below); the script then only says what is missing |
| `--no-link` | do not link the binary into `/usr/local/bin` |
| `-- ARGS…` | when the install is done, run `mindfork ARGS…` — typically `-- setup …` (§3.3) |

What it does, in order: refuses anything but Linux x86_64 with glibc 2.35+ or a
Mac with Apple Silicon, in words; finds the latest release from the *redirect* of the releases page (not
the API, whose 60 requests an hour are shared by everyone behind a datacenter's
address); downloads the archive and `sha256sums.txt`, **refuses the archive if
the two disagree**, and unpacks it; then **runs the binary** to see whether it
starts. On a bare image it does not — the app needs ALSA's runtime library
(`libasound.so.2`, for speech playback) and minimal images do not carry it — so
the script installs that one package with the system's package manager
(`apt-get`, `dnf`, `pacman` or `zypper`; as root, or through `sudo` when it needs
no password), and if it cannot, stops with exit code 3 and the exact command to
run. When the command after `--` installs llama.cpp (`setup --llama …`,
`llama setup …`), it also makes sure of **OpenMP's runtime** (`libgomp.so.1`):
llama.cpp's Linux builds need it, and a bare image lacks it too (measured on
`ubuntu:24.04`; RunPod's images carry it). The binary that needs it is not on
disk yet, so the system's library cache is asked instead, and the package —
`libgomp1`, `libgomp` on Fedora, `gcc-libs` on Arch — is installed the same way;
if it cannot be, the script says so and goes on, since the app itself runs
without it and `llama setup` names it again. Finally it links
`/usr/local/bin/mindfork` and prints the version. On a Mac there is nothing to
install — no glibc, no ALSA, no OpenMP — and the link goes into
`/usr/local/bin` or, failing that, Homebrew's `/opt/homebrew/bin`.

**Running it again is safe and cheap**: a version that is already in place is
recognised and not downloaded, your data in `DIR/data` is never touched (the
archive carries only the bundled dictionaries there), and an upgrade works even
while the app is running. The checksum proves the bytes are the ones the release
published — not truncated, not corrupted; it cannot prove *who* published them,
because both files come from the same place. That is what the attestation is
for: `gh attestation verify <archive> --repo vshylov/mindfork-rs`.

### Linux packages (deb / rpm / pkg.tar.zst)

Each release ships system packages:

```bash
sudo apt install ./mindfork-rs_X.Y.Z-1_amd64.deb        # Debian/Ubuntu
sudo dnf install ./mindfork-rs-X.Y.Z-1.x86_64.rpm       # Fedora
sudo pacman -U  ./mindfork-rs-X.Y.Z-1-x86_64.pkg.tar.zst # Arch
```

The packages place the binary in `/usr/lib/mindfork-rs/` (symlinked from
`/usr/bin/mindfork`), the dictionaries next to it, and keep data in the
**standard OS folder** (`~/.local/share/mindfork-rs` — set by the
`/usr/lib/mindfork-rs/defaults.json` file with `{"mode":"system"}`). The interface
language on first run is detected from the system locale. Packages are unsigned —
verify integrity against the release's `sha256sums.txt`.

### Windows installer

Each release ships `mindfork-rs-vX.Y.Z-x86_64-setup.exe` (Inno Setup). It is a
64-bit program, like the application it carries, so on a system that cannot run
mindfork it does not open at all. It installs
**for the current user without administrator rights** (an "all users" option is
available). It opens with three read-only pages, in the wizard's own language —
Russian gets the unofficial translations from `docs/legal/`, and the English
originals govern:

- the **MIT license** ([LICENSE](../LICENSE)), which has to be accepted to continue;
- the **disclaimer** — the same text as [DISCLAIMER.md](../DISCLAIMER.md) and the
  app's `F1` → "Legal" tab, covering what the models may say and do;
- the **privacy policy** ([PRIVACY.md](../PRIVACY.md)): what the program keeps on
  your machine, what leaves it and which setting has to be on first — the same
  text the app shows under the disclaimer on that `F1` tab. It grants nothing and
  asks for nothing, so `Next` simply continues.

All of those files are installed next to the program. Then come two custom
steps: **application language**
(Russian/English) and **data location** — the standard OS folder
(`%APPDATA%\mindfork-rs\data`, recommended), portable (next to the app), or a custom
folder. The choice is written to `defaults.json` next to the binary and **is not
overwritten on upgrade**.

On the "Additional tasks" page, three optional boxes, all **off** by default: a
desktop shortcut, downloading the Python sandbox, and **adding the install folder
to `PATH`** — which is what makes `mindfork llama setup` and the other commands
the app suggests work from any terminal rather than only from the install folder.
The entry goes into the user's own environment (the machine's for an "all users"
install), is added once however many times you upgrade, and is removed when you
uninstall. Silent install:
`setup.exe /VERYSILENT /NORESTART` (add `/TASKS="addtopath"` to take that box).

The installer is **unsigned** — on first run Windows SmartScreen will show a warning
("Windows protected your PC" → "More info" → "Run anyway"). File integrity can be
verified against the release's `sha256sums.txt`. As an alternative — the portable
`windows.zip` (no install).

### Scoop (Windows)

```powershell
scoop bucket add mindfork https://github.com/vshylov/scoop-bucket
scoop install mindfork/mindfork
```

The bucket ([vshylov/scoop-bucket](https://github.com/vshylov/scoop-bucket))
installs the release's portable `windows.zip`, its hash checked, and writes a
`defaults.json` beside the binary with `{"mode":"system"}` (§2.1): the data lives
in `%APPDATA%\mindfork-rs\data`, so `scoop update mindfork` — and `scoop
uninstall` — leave it in place. `mindfork` goes on `PATH`, and a Start-menu entry
is made. The bucket picks up a new release by itself within four hours. Scoop
downloads the zip rather than running an installer, so SmartScreen has nothing to
say.

### macOS (Apple Silicon) — a preview

```bash
brew install vshylov/tap/mindfork
```

The tap ([vshylov/homebrew-tap](https://github.com/vshylov/homebrew-tap))
installs the release's `aarch64-macos.tar.gz`, its digest taken from the
release's `sha256sums.txt`, and writes a `defaults.json` beside the binary with
`{"mode":"system"}` (§2.1): the data lives in `~/Library/Application
Support/mindfork-rs`, so `brew upgrade` and `brew uninstall` leave it in place.
The tap picks up a new release by itself within six hours. The install script
above works on a Mac too, with the same line as on Linux.

Both routes download with `curl`, so macOS does not quarantine what they
install. **The archive downloaded in a browser is quarantined** — and so is
everything unpacked from it — and since the binary is signed only ad hoc, not
notarized, macOS refuses to start it until the mark is cleared:
`xattr -dr com.apple.quarantine <the unpacked folder>`. Or the long way, as
measured on macOS 26: Safari leaves a `.tar` (it unpacks the `.gz` itself), a
double click unpacks that, and the first start is refused — *"“mindfork” Not
Opened"*; choose **Done**, not *Move to Trash*. Then *System Settings → Privacy
& Security → Open Anyway*, start it again, and answer the second question with
**Open Anyway**.

- **Apple Silicon only.** An Intel Mac has no prebuilt build: the Python
  sandbox's Wasmer has none for it, and llama.cpp's Intel build has no Metal.
  `cargo install --locked mindfork` (below) builds one.
- **Local models run on Metal**: `mindfork llama setup --backend metal
  --set-binary` (§3.1).
- **The keys**: macOS keeps `Ctrl+←/→` for switching Spaces, so words are
  Option+←/→; the `F` keys need `fn` ([the manual](manual.md) §9). iTerm2 and
  Ghostty break a line with `Shift+Enter`. Terminal.app breaks it with
  `Ctrl+J`, erases a character (not a word) with Option+Backspace, and keeps
  Home, End and Page Up/Down for its own scrolling: add `Shift` to reach the
  app. iTerm2 is the other way round: Page Up/Down reach the app, and with
  `Shift` they scroll iTerm2. In Terminal.app a link in the conversation opens
  with Cmd+double-click — a single Cmd+click does nothing.
- **Ghostty** sends Cmd+→ as `Ctrl+E`, which deletes the last exchange (your
  message returns to the input), Cmd+← as `Ctrl+A` (select all) and
  Cmd+Backspace as `Ctrl+U` (write a message as you). Either turn on *Settings
  → Interface → Confirm regenerate / delete*, or turn those keys off in
  `~/.config/ghostty/config` — the shell then loses them too:

  ```
  keybind = super+arrow_left=unbind
  keybind = super+arrow_right=unbind
  keybind = super+backspace=unbind
  ```
- **Terminal.app before macOS 26** has no 24-bit colour, so mindfork draws
  there in its 256: the themes look a little coarser than elsewhere.
- **A preview**: built and tested on GitHub's Apple Silicon runners, and on a
  rented Mac mini (M4) with macOS 26 and 15 on 2026-10-07 — what it found is
  [docs/research/macos.md](research/macos.md) §14; reports are welcome. For a
  key that does not work, `mindfork keys` prints what your terminal sends; its
  lines are the most useful part of a report.

### From crates.io (`cargo install`)

```bash
cargo install --locked mindfork
```

`--locked` builds the dependency versions the release was built and tested with —
the crate carries its `Cargo.lock` (`crates-io.yml` publishes with `--locked`);
without it cargo resolves the newest compatible ones. The same line run later
upgrades to the newest release.

The same prerequisites as "Building from source" below — **Rust 1.96 or newer**
(the crate's `rust-version`; an older cargo refuses before compiling anything),
and on Linux the ALSA headers plus `pkg-config`, which `alsa-sys` finds them
with. Two consequences are worth knowing before choosing this way in, because
`cargo install` keeps **only the executable**:

- **no spellcheck dictionaries.** They are copied next to the binary at build
  time (§4), and that copy is not what gets installed — so spellcheck starts
  off. Dropping the Hunspell pairs into the data directory's `dictionaries/`
  turns it on.
- **the data directory lands next to the installed binary** —
  `~/.cargo/bin/data/` — because the default layout is portable (§2). A
  `defaults.json` beside the binary moves it (§2.1).

The prebuilt archives, the Linux packages and the Windows installer carry the
dictionaries and every licence text and are the recommended way in; this one is
for a machine that already builds Rust.

### Building from source

You need **Rust** (edition 2024, a recent stable toolchain). On **Linux** you also
need the ALSA headers — speech playback (`/tts`, §4.3) is built via `rodio`/`cpal`:

```bash
sudo apt-get install -y libasound2-dev pkg-config     # Debian/Ubuntu
sudo dnf install -y alsa-lib-devel pkgconf            # Fedora
```

(`libasound2-dev` does not depend on `pkg-config`, and `alsa-sys`'s build script
runs it to find the headers. Without the headers the build stops there: "The
system library `alsa` required by crate `alsa-sys` was not found".)

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

**`data.db` is not, and it travels with the rest.** Moving to another machine
means moving the **whole** data directory, not just `chats/`: notes, the
assistant's self-model, every profile's knowledge base and the semantic index
over files attached to chats live in `data.db` alone. Copy only the chat files
and the conversations arrive whole while the memory behind them does not — the
app starts, says so once in the feed (and in the log), and creates an empty
database. The way that cannot go wrong is §2.2: `mindfork backup` on the old
machine, `mindfork restore` on the new one.

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
  Linux `~/.local/share/mindfork-rs`, macOS `~/Library/Application
  Support/mindfork-rs`; **`path`** → the given directory (created if
  needed; **without** a `data/` subfolder). `mindfork stats` prints the data
  root a binary uses on its second line, and creates nothing; inside the app,
  `F1` → **About** → *Locations* names the program's folder, the data folder
  with the mode that chose it, and the logs.
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
- **Your own dictionaries** go in the data directory's `dictionaries/`, which the app
  creates on startup if it is missing and seeds with a `README.txt` (in the interface
  language) explaining what to put there. The bundled dictionaries keep loading; one
  you add under the same name replaces the bundled one. See [§4](#4-spellcheck-dictionaries).

### 2.2. Backup and restore

The commands run without the TUI (the app must be closed — except `stats`
below, which only reads) and exit:

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

The archive includes: `chats/`, `dictionaries/`, `locales/`, `themes/`, `data.db`,
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

#### Which copy is the newest — `mindfork stats`

With the data on several computers, `mindfork stats` answers which copy to keep
without opening each one:

```bash
mindfork stats                          # the data on this computer
mindfork stats D:\copy.zip              # a backup archive, without restoring it
mindfork stats D:\copy.zip --password "a long passphrase"
mindfork stats --json > this-pc.json    # a snapshot, to carry to another computer
mindfork stats --compare laptop.json    # what each copy holds that the other lacks
mindfork stats --compare D:\copy.zip    # the same, against a backup archive
```

It prints when the last message was written and when anything last changed, how
many profiles, chats and messages there are (and how many of them are deleted),
the attached and stored files, images, projects, notes and the knowledge base.
Messages are counted the way the chat list counts them. Times are in UTC, so the
output of two computers can be compared line by line.

The command **only reads**. Nothing is created, migrated or unpacked, so it can be
run while the app is open, and an archive stays a closed archive: an encrypted one
is read in memory and never written out decrypted. The password is found the way
`restore` finds it — `--password`, else the one in the settings, else you are
asked. The password in the settings belongs to this computer, so for an archive
made elsewhere with a different one, pass `--password`.

The summary ends its first block with a **fingerprint**. If two computers print the
same one, their chats, notes, knowledge base and self-models are identical, and there
is nothing more to check.

**The newest copy is not always the most complete one**: a chat continued on a laptop
after the desktop's backup was taken exists only on the laptop. `--compare` answers
that. Take a snapshot on the other computer (`mindfork stats --json > laptop.json` —
a few hundred kilobytes, holding ids, titles and checksums but no message text), bring
it over, and run `mindfork stats --compare laptop.json`; or point `--compare` straight
at a backup archive of the other copy. It prints a verdict first:

- *the copies are identical*, or *hold the same messages and differ in details* (a
  rename, a deleted mark, an attachment);
- *this copy holds everything the other has* — keeping this one loses nothing — or
  the reverse;
- *each copy holds something the other lacks* — keeping only one loses what is listed
  for the other.

Then the lists: chats only here, only there, with more messages here or there, and
**diverged** — one chat continued on both computers, each side holding messages the
other lacks. Chats are compared by the ids of their messages, not by counts or dates,
which is what tells "continued there" from "continued on both". Notes, knowledge-base
documents and self-models are listed the same way, by which side is newer.

mindfork does not merge copies. When each side holds something, keep both until you
have decided what to do with what is listed.

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
  edit; the pieces of an array are joined with one space, so a piece should not
  begin or end with a space of its own, or the text shows two). An existing file is
  not overwritten — point to a new path. The full list of
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

### 2.4. Themes of your own (`data/themes/`)

In the **full** colour mode the app paints its own background, and the theme
that says which is a file: `data/themes/<name>.json`. Its name is the file's,
and *Settings → Interface → Theme* lists it after the built-in `dark` and
`light`. The directory is created automatically (empty).

- **A template.** Export a built-in theme and edit the copy (no TUI needed):
  ```bash
  mindfork themes export dark -o data/themes/mine.json
  ```
  An existing file is not overwritten — point to a new path.
- **What a theme is.** One JSON object, a colour per role, every colour
  `#rrggbb`. Only `canvas` — the background — is required:
  ```json
  { "canvas": "#002b36", "accent": "#b58900" }
  ```
  The roles are listed in [manual.md](manual.md) §8. A key that starts with `_`
  is yours — a note, a credit — and is not read.
- **What you leave out is fitted.** A colour the file does not name starts as the
  built-in theme's — `dark` for a dark canvas, `light` for a light one — and is
  made lighter or darker, keeping its hue, just far enough to stay readable on
  your canvas: 4.5:1 for text, 7:1 for body text, on the background and on a
  selected row alike.
- **What you name is yours.** A colour the file names is drawn exactly as
  written, whatever its contrast — the app reports a low one and does not
  correct it.
- **Checking a theme.** Without starting the app:
  ```bash
  mindfork themes check mine              # a theme of data/themes, by name
  mindfork themes check drafts/mine.json  # any file, by path
  mindfork themes check                   # every theme there is
  ```
  It prints every colour, where it came from and its contrast, and exits with
  `1` when there is something to say — a colour below its floor, a key that
  names nothing, a file that is not a theme.
- **Names.** Letters, digits, `-` and `_`, 32 at most. `dark` and `light` are
  taken: a file of that name is skipped.
- **Fault tolerance.** A file that is not a theme gets a warning in the log
  (`logs/`) and is **skipped**; the rest still load. A theme named in the
  settings that is not there — a file that went away, settings from another
  machine — draws as `dark`, and the name stays in the settings. File changes
  take effect on **restart**.

## 3. Inference engine (llama.cpp `llama-server`)

The app is an **HTTP client** to a local **OpenAI-compatible** server. The protocol
is universal, so in external mode any such server works (llama.cpp `llama-server`,
vLLM, LM Studio, Ollama …). The recommended and verified backend is
**llama.cpp `llama-server`** (prebuilt binaries, CUDA included, for Windows and
Linux). The whole chain
(streaming, EOS stop, tool-calling, "thoughts") is verified on Gemma 4 E4B-it.

> **No `llama-server` yet?** `mindfork llama backends` lists the builds
> published for your machine and `mindfork llama setup --backend <id>` downloads
> one — see [§3.1](#31-downloading-llamacpp-mindfork-llama).

> Originally designed around `xinfer`, but it turned out too raw (incoherent
> output on Gemma 4, builds poorly on Windows). The protocol is the standard
> OpenAI-compatible one (`/v1/chat/completions` streamed over SSE, `/v1/embeddings`),
> spoken by llama.cpp as well as vLLM/LM Studio/Ollama; key fields and quirks
> (`reasoning_content`, `tool_calls`, EOS) are described below and in the "Inference
> engine" / "Sampling parameters" sections of the spec ([spec.md](../spec.md)).

Two modes (configured on the settings screen, `Ctrl+P`, "Model/server" section):

- **managed** — the app itself launches a child `llama-server` (path to the binary
  + GGUF model `-m`, `-ngl`, `-c`, `--jinja`, no-mmap, host/port). **The binary
  field may be left empty**: the app then takes the build `mindfork llama setup`
  installed last (§3.1), or a `llama-server` sitting next to the application —
  so unpacking a llama.cpp archive beside `mindfork` is enough, with nothing to
  type. A bare name like `llama-server` is looked for beside the application and
  then in `PATH`; a path with a directory in it is used exactly as written.
  **No mmap**
  loads the weights fully into RAM instead of mapping the file from disk — helps on
  network/slow drives, but needs more memory (off by default). The flag behind it
  is whichever the binary takes: llama.cpp folded `--no-mmap` into
  `--load-mode none` in July 2026 and removed the old spelling in September, so
  the app asks the binary before sending it. Changing the model in
  settings **restarts** the server.
  - **The batch on a CPU-only host.** `llama-server` looks at its queue between
    batches of `-b` prompt tokens (2048 by default), so a stream the app cancels
    during its prompt processing — a compaction or reflection displaced by your
    message, a task stopped from the tasks screen — holds its slot until the
    batch runs out: 23 s on a CPU-only host, measured
    ([docs/research/cpu-batch.md](research/cpu-batch.md) §3.1). With *GPU layers*
    at 0 the app therefore launches with **`-b 256 -ub 256`**: the wait falls to
    6.5 s, prompt processing slows by about a seventh. The *Batch (-b)* field in
    *Performance* overrides it (2048 restores the server's default; 128 buys a
    2.8 s wait for a fifth); a GPU host at its defaults gets no `-b` at all. The
    server's log says `n_batch = 256` when it applies. For an **external** CPU
    server pass the flags yourself, e.g. `llama-server … -b 256 -ub 256` — the
    app tells you when it matters: on a server whose prompt processing is slow
    it says once, in the feed, how fast prompts run, how long a stopped or
    displaced background request would hold its slot, and this very line
    ([docs/research/slow-prefill-detection.md](research/slow-prefill-detection.md)). Before launch the model file's presence is
  checked: if the GGUF isn't found or isn't accessible, the status bar immediately
  shows a clear error ("model file not found or inaccessible …") instead of hanging
  in "connecting…".
  - **A multi-file (split) GGUF.** A model too large for one file ships as
    several parts — `<name>-00001-of-00003.gguf`, `<name>-00002-of-00003.gguf`, …
    (Hugging Face caps a single file at 50 GB). Download **all** of them into one
    directory, leave the names alone, and put the **first** part in the GGUF
    field: `llama-server` finds the rest itself. Pointing at any other part is
    refused by llama.cpp outright, and a part left behind by an interrupted
    download would let the server start and then die while loading — the
    preflight catches both before launch and names the file to point at or the
    part that is missing. In the interface such a model is called by its name
    without the tail (`gpt-oss-120b-Q8_0`, not
    `gpt-oss-120b-Q8_0-00001-of-00003`).
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
  - **Parallel sessions** (`sessions`, the "Parallel sessions" group): how many
    request streams the app keeps open against the server at once — the
    assistant's own reply and the sub-agents of one reply share them. **1** by
    default, and then the launch line is exactly what it was — which means the
    server's own default: **four slots over one pool**, where the app's own
    background requests (the title, reflection, consolidation, the compaction
    roll) go one at a time, waiting for room beside your reply instead of
    overfilling the pool together with it. A ceiling, not a
    switch: how many sub-agents of one reply *start* together is
    `tools.subagent_parallel` (Settings → Tools, "Subagent: parallel runs",
    1 by default), so raise both. Above 1 the server
    is launched with `-np N --kv-unified`: N slots over the **one** context pool
    that `-c` sizes — the same shape `llama-server` picks on its own when `-np`
    is not given (four slots over a unified pool, its default since December
    2025), so the pool costs no extra memory; what N sessions cost is sharing
    it — and the app keeps that sharing within the pool: a sub-agent's round
    (or a page summary) that would not fit next to the streams already open
    waits for one of them to end, instead of provoking the server's "Context
    size has been exceeded", which ends *every* running conversation at once.
    Raise `-c` to buy room and the waits get rarer. And mind the server's
    **RAM prompt cache** (`--cache-ram`, 8 GiB by default, host RAM): a
    conversation that is not streaming is parked there and restored in a
    fraction of a second, but only while the parked set fits — measured on
    Qwen 3.6 27B, the default holds about four 14k-token contexts; on Gemma
    4 31B it holds **one** 16k conversation, because a parked conversation
    there is 3.7 GiB fresh and grows by ~0.8 GiB with every exchange (the
    server keeps checkpoints of the sliding-window layers with it) — and
    one past the bound a rotation of runs restores *none* of them, every
    round paying the full prefill
    ([docs/research/parallel-subagents.md](research/parallel-subagents.md)
    §3.6–§3.7). Raising `subagent_parallel` on a long-context profile is a
    reason to raise `--cache-ram` with it: budget ~2.5 GiB per parked
    conversation on the 27B and ~5 GiB on the 31B — and a sub-agent run out
    in the background (`start_subagent`, spec §9.3.2) is one more
    conversation to park beside the chat it left. The field's hint shows how many slots the running server
    reports (`total_slots` on `/props`).
  - **Parallel tool calls** (`concurrent_calls`, the same group): how many of
    the tool calls the assistant issues in one reply run at once, when they
    are reads — file, project, attachment, chat and history reads, the
    introspection tools and `fetch_url`; writers, commands, plugins and
    sub-agents always keep their turn, and the results land in the
    assistant's order. **1** on managed and external (one after another,
    exactly as before) and **4** on the cloud providers. A `fetch_url` page
    summary is a request stream and counts against *Parallel sessions* above.
  - **Extra arguments** (the *Advanced* group, last in each managed section —
    the assistant's, impersonation's and the embedder's): raw `llama-server`
    arguments added to the end of the line the app builds, for everything
    llama.cpp offers and the settings do not. Type them as a command line —
    whitespace separates, quote an argument that contains spaces; no shell is
    involved, so `|` in an `-ot` pattern is just a character. The case it was
    built for is a large **mixture-of-experts** model on a card too small for
    it: `--n-cpu-moe N` keeps the experts of the first N layers in RAM.
    Measured on gemma-4-26B-A4B Q4_1 at `-c 16384` on an RTX 4090: 17.3 GB of
    VRAM at 142 tokens/s on the GPU alone, 12.8 GB at 45 tokens/s with
    `--n-cpu-moe 10`, 8.4 GB at 28 tokens/s with `--n-cpu-moe 20` — pick the
    smallest N that fits (`-ot "<regex>=CPU"` places tensors by name for the
    same purpose). Other uses: `-t 8`, `--cache-type-k q8_0`, `-ub 1024`,
    `--kv-unified-per-slot N`; on the embedder, `-c` and `-ngl`, which its
    section has no field for. Changing the line **restarts** the server.
    Refused, with the editor left open and the reason in its title: a flag one
    of the section's own fields writes (`-c`, `--ctx-size`, `--port`, …; the
    message names the field — llama.cpp takes the last of two copies of a flag,
    and has deprecated sending two), and flags that would lock the app out of
    its own server (`--api-key`, `--api-prefix`, `--ssl-*`), give the server a
    shell for whoever reaches its port (`--tools`, `--agent`, `--mcp-servers-*`)
    or make it download models by itself (`-hf`, `--model-url`, `--models-dir`).
    For those, start `llama-server` yourself and use **external** mode. A typo
    llama.cpp refuses shows in the status as its own words — *"llama-server
    refused to start: error: invalid argument: --n-cpu-mo"*. From the command
    line the field is a JSON list:
    `mindfork setup --set 'engine.managed.extra_args=["--n-cpu-moe","20"]'`.
    **Environment variables.** llama.cpp also reads `LLAMA_ARG_<NAME>` for most
    options (`LLAMA_ARG_N_CPU_MOE=20`; `--help` names each one), and the managed
    servers inherit the app's environment — a way to set a flag for **all
    three** servers at once, which the command line overrides. The variables of
    the refused flags (`LLAMA_API_KEY`, `LLAMA_ARG_TOOLS`, `LLAMA_ARG_HF_REPO`,
    `HF_TOKEN`, …) are removed from the managed servers' environment: a
    `LLAMA_API_KEY` set for some other tool would otherwise give a server that
    reports ready and refuses every request. Design and measurements:
    [docs/research/managed-extra-args.md](research/managed-extra-args.md).
- **external** — connects to an already-running server by URL. The **"Model
  (opt.)"** field next to it is optional but not decorative: it is **sent as the
  request's `model`**, which is what a multi-model endpoint routes on
  (`llama-server --router`, LM Studio, Ollama, LiteLLM, OpenRouter all refuse a
  request without it), while a single-model `llama-server` ignores it. Leave it
  blank against a single-model server and the app **asks the server** what it is
  running — `GET /v1/models` when it lists exactly one model, otherwise
  `/props` — and shows that name next to the chat title and on every reply
  (`Ctrl+P` → Interface → "Model name in the feed"). A name you type always wins,
  and a server that cannot say leaves the caption empty. The **"Parallel
  sessions"** field here is the number of streams the app may open against
  *your* server at once — for llama.cpp, what you started it with (`-np`, or
  its four-slot default); the hint under the field shows what a llama.cpp
  reports, and a vLLM/LM Studio/Ollama, which report nothing, leave it blank.
  Leave it at 1 unless the server has the slots. The same **"Parallel tool
  calls"** field sits beside it, **1** by default here as on managed; the
  cloud modes default to **4**.

**Ollama is an `external` server**, at `http://localhost:11434/v1`, with the
model's name — the one `ollama list` prints — in "Model (opt.)": it serves every
model it has pulled and routes on that field. `Enter` on the field lists them.
One thing needs setting, in Ollama: **the context window** — the app reads it
from there.

**Nothing has to be typed, though**, when Ollama or LM Studio runs on the same
computer: a mindfork with no model connected looks for both at the start, and
`/local` looks whenever it is typed. What answers is listed — `Enter` writes the
row into these settings, and the same server's embedding model too while none
is set up ([manual.md](manual.md) §1). The lines below are the same settings
for a script or a machine without a terminal to look from.

```bash
ollama pull gemma4:e4b
mindfork setup --set engine.mode=external --set engine.external.url=http://localhost:11434/v1 --set engine.external.model_name=gemma4:e4b
```

- **Ollama's window is its own setting** — the context slider in the Ollama
  app's settings, or `OLLAMA_CONTEXT_LENGTH` for `ollama serve` — and by
  default it follows the GPU's memory: 4096 tokens under 24 GB, 32768 up to
  48 GB, 262144 above ([Ollama's
  documentation](https://docs.ollama.com/context-length)). `ollama ps` shows the
  window a loaded model runs with.
- **A prompt that outgrows it is cut in silence**, in one of two ways, and
  answered with a `200` (measured on Ollama 0.35.1). A conversation loses its
  **oldest messages** whole until the rest fits — the system message and the
  last message are kept, and the `usage` lands just under the window (2017 of
  2048). When the system message and the last message alone do not fit — a
  long paste, a large tool result — the prompt is **cut to half the window from
  the front**, `keep=5`: the system message goes first (2656 tokens in 2048
  became 1027), and the model answers without its instructions.
- **The app tells a cut prompt** from Ollama's `usage`, which reports what it
  processed: it knows the exact size of the chat's previous request, and a
  request that extends it cannot hold less. A cut is said in the feed once per
  session — what the server processed of how much at least, and Ollama's
  setting — and a fold into a summary starts if there is anything to fold, after
  which `/regen` asks again. The half-window cut is told whenever the chat's
  previous request was over half the window; a few of the oldest messages
  dropped near the window are told only when they outweigh the slack the bound
  allows — and that cut leaves the `usage` near the window, where compaction
  starts anyway ([prompt-cut-detection.md](research/prompt-cut-detection.md)).
  Measured at 4096: a page pasted after a first turn of 3446 tokens was cut from
  5210 to 2051, and the feed said *of at least 4357 tokens it processed 2051*.
- **mindfork's first turn is already about 3500 tokens** — its instructions,
  the memory and the tool schemas (measured on `gemma4:e4b`: 3503, then 3813
  after a one-word exchange), so a 4096 window is gone by the third.
- **The app reads Ollama's window** from its `/api/ps`, which lists the models
  it has loaded and the window of each — after the first turn, since Ollama
  loads a model on its first request. Automatic compaction then folds the
  conversation into a summary at 75 % of it, before Ollama would cut anything.
  **0.15.0 and earlier cannot**, and need the number typed into **Settings →
  Memory → Context** (`compaction.context_tokens`), the same as Ollama's — add
  `--set compaction.context_tokens=16384` to the line above. A typed number wins
  over the one read, so if you typed one, keep it equal to Ollama's or clear it.
- **A window too small for the conversation is said in the feed**, once per
  session: when a turn ends over the 75 % with nothing earlier to fold — at
  Ollama's 4096, the second turn — the note names the window, the prompt and
  Ollama's setting. Measured at 4096: *3651 tokens of this server's 4096-token
  window*, before Ollama had cut anything.

What else was measured against Ollama 0.35.1 (`gemma4:e4b`): streaming, the
thoughts (it sends them as `delta.reasoning`), tool calls and the usage of
every stream work as they do against a llama-server; `/health` is a `404`,
which the readiness probe reads as ready.

**LM Studio is an `external` server** too, at `http://localhost:1234/v1`, with
the model's key — the one `lms ls` prints — in "Model (opt.)". Its server is off
until started: `lms server start`, or the Developer tab of the app. A model that
is not loaded is loaded by the first request that names it.

```bash
lms server start
mindfork setup --set engine.mode=external --set engine.external.url=http://localhost:1234/v1 --set engine.external.model_name=google_gemma-4-e4b-it
```

- **LM Studio's window is its own setting** — the model's *Context Length* in
  the app's load settings, or `lms load <model> -c 16384`. A model loaded on a
  request takes its default configuration; measured on LM Studio 1.1.7 with
  `gemma-4-E4B` on a 24 GB GPU, that was the model's maximum, 131072. `lms ps`
  shows the window a loaded model runs with. Loading a loaded model again starts
  a **second instance** (`<model>:2`) and requests naming the model keep going
  to the first, so to change the window, `lms unload <model>` first.
- **The app reads it** from LM Studio's `/api/v1/models`, which lists every
  model with its loaded instances and the window of each — at the start, and
  once more after the first turn, since a model may be loaded by that turn.
  Automatic compaction then folds at 75 % of it.
- **A prompt that outgrows it is answered in one of two ways** (measured on
  1.1.7). When the last message does not fit beside mindfork's instructions,
  LM Studio refuses it with a `400`, and the feed says the conversation no
  longer fits. When the conversation does not fit, LM Studio asks its runtime
  again with **the middle removed** — the instructions, the first message and
  the last, everything between them dropped at once — and answers with a `200`.
  The app tells that cut from the `usage` as it does Ollama's, in LM Studio's
  words: measured at 8192, a second page pasted into a conversation was cut from
  at least 7368 tokens to 6332, and the feed named the model's Context Length and
  the `lms unload` / `lms load` line for it.
- **The window-too-small note** names LM Studio's setting the same way. At
  4096 it comes after the first turn — mindfork's 3446 tokens are already over
  75 % of it.

What else was measured against LM Studio 1.1.7: streaming, the thoughts
(`delta.reasoning_content`), tool calls and the usage of every stream work as
against a llama-server. It answers every path it does not serve — `/health`,
`/props` among them — with a `200` and an `error` body, which the app reads as
no answer; a model name it does not hold is answered by whatever model is
loaded, with no error, so check the key with `lms ls`. `Enter` on the model
field lists every model LM Studio holds, its embedding models among them.

**A cloud gateway is an `external` server too** — LiteLLM, a vLLM behind a
proxy, or any OpenAI-compatible reseller. **OpenRouter has a mode of its own**,
`openrouter` (§3.2), and that mode is the way to use it: one key for every slot,
a request written in the gateway's own dialect, and nothing inferred from
whether a catalogue happened to answer. `external` pointed at
`https://openrouter.ai/api/v1` keeps working exactly as it did — the URL row
then says that the mode exists, and nothing is moved for you. For every other
gateway, and for OpenRouter kept in `external`, these are worth knowing before
you point the app at one (the compatibility review they come from was made
against OpenRouter:
[docs/research/openrouter-external.md](research/openrouter-external.md)):

- **the URL carries `/v1` and the model name is mandatory** — against
  OpenRouter, `https://openrouter.ai/api/v1` and a slug such as
  `deepseek/deepseek-r1` in
  "Model (opt.)". The key goes into the same "API key (opt.)" field described in
  §3.2. A gateway routes on the request's `model` and refuses a request without
  one, so the field is only "optional" against a single-model server. `Enter` on
  that field lists what the gateway itself publishes on `GET /v1/models`, so the
  slug can be picked rather than transcribed;
- **the context window is taken from the endpoint's catalogue**, when it
  publishes one. Automatic compaction measures against a window, and a gateway
  serves no `/props` (that is llama.cpp's own endpoint) — so the app reads
  `context_length` for the model you named from `GET /v1/models` instead, and the
  trigger works without you doing anything. An endpoint that publishes no
  catalogue still needs the number typed in, Settings → Memory → Context;
  without a source the automatic trigger stays inactive and a long chat ends in
  the provider's "context length exceeded" instead of a rolling summary
  (`/compact` works either way). A typed number always wins over the catalogue.
  The missing `/props` still leaves the "Parallel sessions" hint blank and the
  model-name caption empty unless you typed a name;
- **the sampling settings narrow to what that catalogue lists.** The set is
  llama.cpp's, and a gateway takes part of it: `temperature`, `top_p`, `top_k`,
  `min_p`, `max_tokens`, `seed`, `frequency_penalty` and `presence_penalty` go
  through; dynamic temperature, adaptive-p, typical-p, top-n-sigma, mirostat,
  DRY, XTC and the sampler order are dropped on the way. Where the endpoint says
  which it takes, the settings screen and the assistant's own `set_sampling` stop
  offering the rest, and a message's "what was applied" record stops naming them
  — previously all three showed knobs that did nothing. `repeat_penalty` is the
  one to know about: gateways spell that field `repetition_penalty`, and the app
  recognises the two as the same knob **when it decides what to offer**. The
  request itself is unchanged in `external` — that mode is also every local
  `llama-server`, which reads llama.cpp's spelling — so through a gateway the
  penalty is offered and, measured on OpenRouter, reaches no model. The
  `openrouter` mode sends it under the gateway's name (§3.2);
- **a tool's images and `/continue` depend on the provider the gateway routes
  to**, so the app adapts where the catalogue shows it is talking to a gateway.
  A picture a tool returns (a `python_exec` chart, an MCP screenshot) is sent in
  a user message right after the tool's result, the one place every measured
  route reads it — inside the result itself some refused the request and some
  silently dropped the picture. `/continue` resumes a reply only on Claude up to
  the 4.5 generation and on Gemini; every other model there starts the answer
  over, so the command refuses and points at `/regen` instead of storing the new
  answer glued onto the old fragment. A local server is unaffected by both;
- **whether the model takes images comes from that catalogue too.** For a model
  it lists as text-only, `/image attach` refuses and says to pick a vision model,
  and a tool's picture is kept back with a note telling the model it has not seen
  it — where before the gateway refused the whole request, and kept refusing every
  later turn of that chat, since an image stays in its history. A chat that
  already holds images from a vision model keeps working after a switch to a
  text-only one: the images go as a note to the model that it cannot see them,
  and the chat says once that they were not sent. For a model the catalogue lists
  with images, the attach no longer adds the "does not report" caveat;
- **"Parallel sessions" and "Parallel tool calls" default to 1** here, as for a
  local server. A gateway is a cloud in practice: raising both (4 is the cloud
  modes' default for tool calls) is what makes a reply's reads overlap.

Thinking models work through a gateway as they do anywhere else: the "thoughts"
arrive under the field name the gateway uses (`reasoning`) as readily as under
the one a local server uses (`reasoning_content`).

**How the app knows whether images are accepted.** It asks the server, rather than
matching model names: `llama-server` reports `modalities` on its `/props` endpoint
(`{"vision": true, "video": true, "audio": false}`) — the same request that already
supplies the context window. `vision: true` → images are accepted, `false` → the
attach is refused with a message pointing at the `--mmproj` field. A server that
reports no `modalities` at all (vLLM, LM Studio, a proxy, any cloud) is treated as
"cannot say" and the attach is allowed: a send-time provider error is a better
outcome than refusing a setup that works. The four cloud providers answer
"supported" outright — every current-generation model on them takes images. In
the `openrouter` mode the answer is the model's own entry in the gateway's
catalogue, in both directions: an entry that lists image input attaches
silently, one that does not refuses the attach.

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
falls back to parsing `<think>…</think>` from the text. A gateway sends the same
text in a field of its own (`reasoning`) — the app reads either.

### 3.1. Downloading llama.cpp (`mindfork llama`)

If you have no `llama-server` yet, the app can fetch one. It reads the
[llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases), keeps the
builds for **your** OS and architecture, and names the backends they were built
with:

```bash
mindfork llama backends                       # what is on offer, and how big
mindfork llama setup --backend vulkan         # download, verify and unpack it
mindfork llama installed                      # what is already downloaded
mindfork llama remove vulkan-b10883           # delete one you no longer need
```

```
Build b10883 (2026-09-09), windows/x86_64:
  cpu                   17 MB
  cuda-12.4            615 MB  (+ CUDA runtime)
  cuda-13.3            515 MB  (+ CUDA runtime)
  openvino-2026.3.1     76 MB
  rocm-10.0            232 MB
  sycl                 114 MB
  vulkan                30 MB
```

**Which backend.** `cpu` works anywhere and is the smallest. `vulkan` is the
easy GPU choice — it runs on NVIDIA *and* AMD through the driver you already
have, and costs 30 MB against CUDA's 600. `cuda-*` is the fastest on NVIDIA;
pick the version your driver supports — the *CUDA Version* `nvidia-smi` prints;
a newer build can still list the GPU and then stop at the first kernel (§3.4) —
and note that the CUDA runtime DLLs are downloaded with it (that is where most of
the size goes). On **Linux** the CUDA
builds exist since September 2026 (`cuda-12.8`, `cuda-13.3` at the time of
writing); upstream builds them on Ubuntu 24.04, so expect them to want a system
at least that new — the command runs the binary after unpacking and refuses a
build that cannot start, naming a missing library and its package (on a bare
image that is OpenMP's `libgomp.so.1`; the download is kept, so the same command
after installing it fetches nothing). `rocm-*` is AMD's own
stack, `sycl`/`openvino` are Intel's. The list is derived from the release, so a
backend upstream adds or renames shows up without an app update.

**Options.**

| | |
|---|---|
| `--backend <ID>` | which backend to install; there is no default — the sizes differ too much to choose for you. A **family** works too: `cuda-12` installs the one `cuda-12.*` build on offer, so a command you saved keeps working when llama.cpp moves to the next CUDA minor. A family that fits two backends (`cuda`, `sycl`) is refused and both are named |
| `--build <TAG>` | pin a build, e.g. `--build b10883`. Without it, the newest one **that has the backend you named** — llama.cpp publishes about a dozen builds a day and now and then one arrives empty or half-uploaded. Pinning is how you keep a known-good one |
| `--no-cudart` | skip the CUDA runtime (only if it is already installed on the machine) |
| `--set-binary` | write the installed binary's path into the settings afterwards |
| `--force` | download and unpack again over an existing install |

Each install goes into its own directory — `data/llama/<backend>-<tag>/`, e.g.
`data/llama/vulkan-b10883/` — so several can coexist and rolling back to the
previous one is a matter of pointing the setting at it. Every downloaded file is
checked against the sha256 the release publishes, and an interrupted download is
resumed rather than restarted. When the unpacking is done the command runs the
binary it just wrote: it prints the build number (which must match the tag) and
the compute devices the backend found. On a GPU backend an empty device list is
reported explicitly — that means the driver or its runtime is missing and the
server would silently run on the CPU.

You do not have to do anything else: an **empty** *llama-server binary* field
resolves to the build installed last, so the very next launch in managed mode
uses what you just downloaded. To pin a particular one, put the printed path into
the settings (`Ctrl+P` → *Model/server* → *llama-server binary*) or into
`MINDFORK_LLAMA_BIN` — or let the command do it:

```bash
mindfork llama setup --backend vulkan --set-binary
```

`--set-binary` writes the path **after** a successful install, never on failure.
It always sets the assistant's engine; the impersonation engine and the embedding
server get the same path only if they had none of their own, because one install
serves all three but a path you typed there is a deliberate choice (a different
build for the embedder is a legitimate setup). It does **not** switch the engine
mode: `managed` is already the default, so a config in another mode is one you
switched on purpose — the command says so instead, and the path waits.

**Keep the whole folder:** `llama-server` is a small launcher that loads its
libraries from the files next to it, so moving the executable elsewhere breaks
it.

The downloads are not small (see the table), and `data/` is next to the binary in
portable mode — in a development checkout that means `target/debug/data/`, which
`cargo clean` removes. Nothing under `data/llama/` is included in a backup or
touched by a restore: it is re-downloadable, not user data. Beside it,
`data/cuda-cache/` holds the GPU kernels the NVIDIA driver compiled for these
servers — the app points the driver there unless `CUDA_CACHE_PATH` names
another place — and it is derived data too: delete it and the next start
compiles again (§3.4).

**Deleting one.** Builds are not small and several add up, so
`mindfork llama remove <id>` takes them back off disk — `<id>` being the name
`llama installed` prints (`vulkan-b10883`), or a bare backend (`vulkan`) while
only one build of it is installed. There is no `--all` and no automatic prune:
the build worth deleting and the build you were about to fall back to look the
same from here. If the settings point at what you are deleting, the command
**refuses** and names which of the three fields do, so that you set another
build first; `--force` deletes anyway. The closing line says what an empty
binary field resolves to now, since that answer changes when the build it was
resolving to is the one that just went away.

> Behind a shared address you can run into GitHub's unauthenticated API limit (60
> requests an hour per address). The command says so in plain words; the limit
> resets within the hour. The command sends no credentials: a `GITHUB_TOKEN` in
> the environment is not read.

### 3.2. Cloud providers and API keys

Besides a local server, the engine can be a cloud: **OpenAI**, **Google Gemini**,
**Claude** (Anthropic), **Grok** (xAI), or the **OpenRouter** gateway — one
account in front of every vendor's models, described under its own heading
below. The mode is chosen in settings
(`Ctrl+P` → "Model/server" → "Mode" field), where the model name is also set.
**`Enter` on the model field asks the provider what it serves** and offers that
list — type to filter it, `Ctrl+R` asks again, and the list's first row is the
old way, typing a name by hand. What it shows is the provider's own catalogue,
narrowed to chat models where the provider says which those are; so this page
names no model, and cannot recommend one that has since been retired. (For xAI,
keys are issued at `console.x.ai`.)

Neither Anthropic nor xAI offers embeddings, so under a `claude`/`grok` engine
RAG needs a separate embedder (a local `llama-server --embeddings`, OpenAI,
Gemini, or OpenRouter) — set it in the same section's "Embeddings" tab.

**The key is entered right in settings** — the API-key field under the model
name, labelled with the provider it belongs to ("OpenAI API key", "Gemini API
key", …): `Enter` opens a blank input (characters hidden as `•`), `Enter` saves, `Del`
removes it. The key is stored in `settings.json` **encrypted and bound to this
computer** (Windows — the system DPAPI; Linux — a key derived from
`/etc/machine-id`), so:

- the settings file can be **moved between computers** — on a new one the key won't
  decrypt and needs to be entered again (it will land as a separate entry), and
  moving back to the original computer the original key reads again;
- a saved key **cannot be viewed or copied** from the app — editing means re-entering
  it; the field only shows "configured (this computer)";
- one key serves **chat, impersonation, embeddings and speech** for that provider
  (OpenRouter's key included: all four tabs have its mode) — and the video tool,
  which reads the key of the provider it is set to, Gemini or OpenRouter (§4.4).

The key is protected against moving/copying the file, but not against programs
running under your own user account on the same computer (this is how browser
password managers work too).

**Alternative — an environment variable** (for CI, scripts, and systems without
`machine-id`): the "… API key (env)" field stores the **name** of the variable, e.g.
`OPENAI_API_KEY`, and the key itself is read from the environment. A key entered in
settings takes priority; the env one is used if no key was entered.

**An external server's key works the same way.** In `external` mode — connecting to
an OpenAI-compatible server you run or rent (a local `llama-server`, vLLM, LM Studio,
or a gateway like LiteLLM — or OpenRouter, if you keep it in `external`) — the same
"API key (opt.)" field appears
above "API key (env, opt.)", with the same behaviour and the same encrypted storage.
Two differences from a cloud key:

- it is **optional**: a local `llama-server` requires no authorization, and with no
  key stored and no variable named, none is sent — start such a server with
  `--api-key <key>` if you do want it to require one;
- it belongs to **that one server**, not to a provider: the "Assistant",
  "Impersonation", "Embeddings" and "Speech" tabs each hold their own external URL, so
  each holds its own key. A cloud gateway for chat beside a local embedding server is
  the normal case, and sharing one key would send the gateway's token to localhost.

#### The OpenRouter gateway (`openrouter`)

OpenRouter is one account and one key in front of several hundred models from
many vendors. It is a mode of its own on all four tabs of the section: on
**Assistant**, **Impersonation** and **Embeddings** next to `managed`,
`external` and the four clouds, and on **Speech** next to `openai`, `gemini` and
`external` (§4.3) — so a local server and the gateway each keep their settings
and switching between them retypes nothing. The video tool reaches it too, by a
**Provider** row of its own in Settings → Tools (§4.4), so the one key serves
everything the app asks a model for: chat, impersonation, embeddings, speech
and video. Design and measurements:
[docs/research/openrouter-mode.md](research/openrouter-mode.md).

**Setting it up.**

1. `Ctrl+P` → "Model/server" → set **Mode** to `openrouter`.
2. Enter the key in "OpenRouter API key" — or name the environment variable that
   holds it (`OPENROUTER_API_KEY`, say) in the field below; no variable name is
   assumed. **One stored key serves every tab in this mode**: the impersonation,
   embedding and speech slots need no key of their own, only a model — and
   neither does the video tool, once its provider is `openrouter` (§4.4).
3. `Enter` on the **Model** row lists the gateway's catalogue. Each row is the
   model's id, its context window, the price of a million tokens in and out
   (`free` where both are zero) and `no tools` for a model that lists none —
   this application is driven by tools, so such a model chats and does nothing
   else. With a key the list is the **account's own** (`/models/user` — what
   your account's privacy and provider settings leave), without one the public
   list: this is the one cloud whose picker opens before a key is entered. The
   Embeddings tab lists the gateway's embedding models. `:batch` slugs are left
   out, because the gateway itself refuses them for chat. A slug can still be
   typed by hand — the list's first row — a `:free`-style variant suffix or a
   `~…-latest` alias included: the gateway resolves both when the app asks it
   about the model.
4. "Base URL (opt.)" stays empty unless you reach the gateway through a proxy;
   the default is `https://openrouter.ai/api/v1`.

**The status chip.** When the engine is applied — at start-up and after an edit
of this section — the app asks the gateway **once** whether the key is good
(`GET /key`, a request that spends nothing), and shows "connecting…" for the
moment that takes:

| The gateway's answer | Status |
|---|---|
| the key is accepted | ready |
| the key is refused (`401`/`403`) | unavailable, in the gateway's words — *"OpenRouter refused the API key: User not found."* |
| an outage or a rate limit — the key was not judged | ready: a request will speak for itself |
| no answer at all (no network, DNS, TLS) | the reason, and the question repeats every 5 s until something answers |

After the first answer nothing is asked periodically. The check is made for
each of the three tabs that have a chip — the assistant's engine,
impersonation's and the embedder — so a key the gateway refuses shows on the Embeddings tab's
chip as well, instead of first appearing inside the result of whichever tool
embedded first. The embedder's chip is informational, as in every mode: it holds
no call back, so an embedding asked for on a refused key is still asked for, and
fails in the gateway's words. The Speech tab has no chip and its key is not
asked about ahead of time — nothing of it is running until `/tts` is typed — so
there a refused key is what the first `/tts` says (§4.3). The video tool has no
chip either, and there a refused key is what the first video says (§4.4).

**What the app learns from the gateway.** The model's context window (what
automatic compaction measures against), the sampling parameters it takes, whether
it takes images and how it reasons all come from the gateway's entry for that one
model (`GET /model/<slug>`), asked when the engine is applied. llama.cpp's
`/health` and `/props` are never asked here, so the "Parallel sessions" hint is
blank. A number typed into Settings → Memory → Context still wins over the
catalogue's.

**Sampling.** The screen offers what the gateway reads — temperature, top-k,
top-p, min-p, max_tokens, seed, the frequency and presence penalties, the repeat
penalty, thinking and the reasoning effort — narrowed to what the chosen model's
entry lists. The repeat penalty is sent as `repetition_penalty`, the gateway's
name for it. llama.cpp's own knobs (dynamic temperature, typical-p, top-n-sigma,
adaptive-p, mirostat, DRY, XTC, the sampler order, `repeat_last_n`) are neither
offered nor sent.

**Reasoning.** A chosen effort travels as `reasoning_effort`, and the thinking
switch as the gateway's own `reasoning` field where the model's entry lists
reasoning. The turns the app makes silently — a chat's title, the compaction
summary, impersonation — ask for reasoning to be off; on a model whose entry says
it **must** reason they ask for the lowest effort that entry lists instead
(`minimal` on Gemini 3.5 Flash), since such a model refuses "off" with a `400`.

**Tool images and `/continue`** behave as described for a gateway in §3, without
waiting for a catalogue to answer: a picture a tool returns is sent in a user
message right after the tool's result, and `/continue` resumes a reply on Claude
up to the 4.5 generation and on Gemini, and refuses on every other model.

**The app names itself to OpenRouter.** Every request to the gateway — chat,
embeddings, speech, a video, the key check, the question about the model and
the lists of models for the picker — carries two headers: `HTTP-Referer: https://mindfork.io` and `X-OpenRouter-Title: mindfork`.
They name the application and say nothing about you; by OpenRouter's own
description, it counts its public application rankings by them. On by default; **"Name the app to
OpenRouter"**, in the provider's group on any tab whose mode is `openrouter` —
and at the end of the video group in Settings → Tools while its provider is
`openrouter` — turns them off for every slot at once
([PRIVACY.md](../PRIVACY.md) §3.1).

**Embeddings.** The Embeddings tab in this mode sends the same request the other
cloud embedders do, under the two rules every cloud embedder is under (the
embedding server's notes under "Quick start via environment variables",
below): a request carries at most 64 texts, a longer one going out in parts,
and a request that failed for a passing reason — a rate limit, an overloaded
provider, a lost connection — is sent again, up to three attempts. The first
matters here in particular: two of the gateway's embedding models — Gemini's,
`google/gemini-embedding-2` among them — refuse a request of more than a
hundred texts, which is what a long text handed to `rag_add` used to be.

**Moving the embedder between a local server and the gateway** does not by
itself call for `/reindex`. What decides is whether the model that answers is
the model that indexed, and the app measures that rather than reading names
("Loading files into the knowledge base", below): measured on 2026-09-29, a
local `bge-m3` Q8_0 and the gateway's `baai/bge-m3` agree on the check's fixed
sentence to 0.999491, above the 0.999 the check asks for, so an index built on
the one answers a query embedded by the other and nothing is offered. A model
of the same vector size that is another model
(`intfloat/multilingual-e5-large`, in the same run) is caught on its first
request, and `rag_search` refuses over the knowledge base until `/reindex` —
the search that noticed the change included.

**Speech.** `/tts` speaks through the gateway when the Speech tab's mode is
`openrouter`: the same key, a list of the gateway's speech models behind the
Model row and of the chosen model's voices behind the two voice rows. It has no
default model — nothing is spoken until one is chosen. Setup, and what differs
from the other speech providers: §4.3.

**Video.** `youtube_watch` watches through the gateway when the **Provider**
row of the video group (Settings → Tools) is `openrouter`: the same key, and a
list of the gateway's models that take video behind the Model row, the Gemini
family first. It has no default model — until one is chosen the tool answers
in its reduced form, with the video's title, channel, length and the author's
description. Through the gateway **the whole video
is read and charged, even when a part of it is asked for**. Setup, and what
differs from Google's own API: §4.4.

**Coming from `external`.** An `external` section whose URL is on `openrouter.ai`
shows a hint that this mode exists. Nothing is moved automatically: set the mode,
enter the key once, pick the model. On the Speech tab the hint says more,
because there the `external` section does **not** keep working with that
address: an external speech server is asked for WAV, and OpenRouter answers in
PCM or MP3 and refuses anything else — speech through it needs the mode.

**Older versions and `settings.json`.** This version writes `settings.json` and
the chat files at schema 5 (4 was the gateway's mode; 5 is the reasoning
effort's `max`). A mindfork older than it refuses to start on those files,
saying the data was created by a newer version; the copy made before the upgrade,
`backups/pre-migrate-<date>.zip`, is the way back. The video tool's provider
(§4.4) added fields to the file and no step to the schema: a file written
before them reads as it always did, with Gemini watching.

### 3.3. Everything in one go (`mindfork setup`)

One command takes a machine from "mindfork is unpacked" to "a local model
answers" — the Python sandbox, llama.cpp, and the settings of the managed engine
— without opening the settings screen. It exists for machines that are **new
every time** (a rented GPU box, a container), and is just as usable on a desktop:

```bash
mindfork setup --sandbox --llama cuda-12 \
  --model /models/gemma-4-31b-it-q4_0.gguf \
  --mmproj /models/mmproj-gemma-4-31b-it-f16.gguf \
  --embed-model /models/bge-m3-Q8_0.gguf \
  --ctx 32768 --verify
```

| | |
|---|---|
| `--sandbox` | install the Python sandbox and switch Python execution on — `sandbox setup --enable-python` (§4.1) |
| `--llama <BACKEND>` | install that llama.cpp backend, or family (`cuda-12`) — `llama setup --backend` (§3.1); `--llama-build <TAG>` pins the build |
| `--model <GGUF>` | the chat model; **also switches the engine to managed mode**, and says so if it was in another |
| `--mmproj <GGUF>` | the vision projector that ships beside a vision model |
| `--embed-model <GGUF>` | the embedding model (memory, the knowledge base); also switches embeddings to managed mode |
| `--ctx <N>` / `--ngl <N>` | the chat server's context window and GPU layers |
| `--set <KEY>=<VALUE>` | any other setting, by its path in `settings.json` — `--set engine.managed.sessions=4 --set engine.managed.no_mmap=true`. Repeatable |
| `--verify` | afterwards start the configured servers once, report what they say about themselves, and stop them |

How it behaves, which matters more than the list:

- **Every option is one step, and a step you do not name is not run.**
  `mindfork setup --ctx 65536` alone is a perfectly good use.
- **Paths and keys are checked before anything is downloaded or written.** A
  model path that names no file, a part of a multi-file GGUF that is not the
  first, a `--set` key that does not exist (`engine.managed.modle_path`) or a
  value of the wrong type is an error *now*, in the same words the app would
  have used at launch — not a status line ten minutes and a gigabyte later.
  Relative paths are stored absolute.
- **A step that fails does not stop the others.** If the sandbox download fails,
  llama.cpp is still installed and the settings are still written; the command
  ends with the list of what failed and a non-zero exit code. **Run the same
  line again** and it repeats only what is missing — everything that is already
  in place is recognised and skipped.
- **It writes `settings.json`**, once, at the end. That is the difference from
  the environment variables below: what it sets is what the settings screen
  (`Ctrl+P`) shows, and what you change there afterwards sticks.
- **It writes no path to the `llama-server` binary** — an empty *Binary* field
  already finds the build installed last (§3.1). The one exception runs the
  other way: after a successful `--llama`, a binary path in the settings that
  names **no file** — typically one that arrived with data restored from another
  machine — is cleared, and the command says so.
- **`--set` takes the value as JSON when it parses as JSON** (`true`, `4`,
  `null` to unset, `"quoted"`), and as plain text otherwise, so paths, hosts and
  mode names need no quoting. API keys cannot be set this way — a key typed on
  a command line ends up in the shell's history; give the key's *variable name*
  instead (`--set engine.openai.api_key_env=MY_KEY`, §3.2).

**`--verify`** starts exactly what the app will start — the chat server and, if
one is configured, the embedding server, **both at once**, since whether the two
fit the GPU together is one of the questions — waits for each to load, and
prints one line per server:

```
== Starting what was configured
  chat server: starting on port 8000…
  embedding server: starting on port 8001…
  chat server: ready in 41 s — context 32768, takes images, slots 4
  embedding server: ready in 6 s
```

A context too large for the card, a projector that belongs to another model, a
port something else already listens on — each shows up here as `FAILED` with the
reason, while the command line can still be edited. The servers' own output goes
to the log (`data/logs/`), and the command names the folder when something
fails. A cloud or external engine is not started and not failed: there is
nothing of ours to start.

### 3.4. On a rented GPU box (RunPod and similar)

A rented pod is a machine that is **new every time**, and on RunPod more so than
it looks: the container's own disk is cleared **whenever the pod stops**, not only
when it is terminated — what survives is the volume, mounted at `/workspace`. So
everything goes onto the volume, and one line both installs and repairs:

```bash
curl -fsSL https://github.com/vshylov/mindfork-rs/releases/latest/download/install.sh \
  | sh -s -- --dir /workspace/mindfork -- setup \
      --sandbox --llama cuda-12 \
      --model /workspace/models/gemma-4-31b-it-q4_0.gguf \
      --mmproj /workspace/models/mmproj-gemma-4-31b-it-f16.gguf \
      --embed-model /workspace/models/bge-m3-Q8_0.gguf \
      --ctx 32768 --verify
tmux new -A -s mindfork mindfork
```

The first run downloads the app, the sandbox and llama.cpp and writes the
settings; **after a restart the same line takes seconds** — everything under
`/workspace/mindfork` is recognised as present, and only what the container lost
(the ALSA package, the link in `/usr/local/bin`) is put back. Pasted into a
template's *start command* (followed by the image's own `/start.sh`), a restart
needs no typing at all.

**What it looks like, measured** (2026-09-22, a RunPod pod with an RTX PRO 6000
Blackwell, 96 GB; the line above with a 31B Q8_0 model and `--ctx 131072`,
the models already on the volume): the app recognised as installed, the
sandbox downloaded and packed (245 MB of wasmer, Python, 30-odd packages, the
cache warmed), llama.cpp `cuda-12.8` installed with its runtime — 161 + 566 MB
unpacked — and its device list reading `CUDA0: NVIDIA RTX PRO 6000 Blackwell
(97251 MiB)`, the settings written, and then:

```
== Starting what was configured
  chat server: starting on port 8000…
  embedding server: no model configured — skipped (memory search and the knowledge base stay off)
  chat server: ready in 4 s — context 131072, text only, slots 4
Done. Start the app with: mindfork
```

Four seconds is a model already in the page cache from an earlier run; the same
model cold, after a pod reset, took 33 s with its projector beside it. `text only` is what a model
without `--mmproj` reports; the projector is a second file, §3.

- **The models are yours to bring, and `hf download` is how to bring them** —
  onto the volume, before the line above (`setup` then takes the paths, §3.3):

  ```bash
  pip install -U huggingface_hub
  hf download bartowski/Ateron_Gemma-4-MoonGem-31B-GGUF \
      --include "Ateron_Gemma-4-MoonGem-31B-Q8_0.gguf" \
      --include "mmproj-Ateron_Gemma-4-MoonGem-31B-f16.gguf" \
      --local-dir /workspace/models
  ```

  Not `curl` on a file's `resolve/main/…` link. That is one HTTP stream, and
  on a pod it took **hours** for a 34 GB pair that `hf download` — the Hub's
  own client, fetching a file as parallel chunks through its Xet backend —
  brought in **minutes** (measured 2026-09-22). Run again after a pod reset,
  with the files already on the volume, the same command finished in 20 s
  and downloaded nothing: `hf` keeps a record of what it fetched beside the
  files (`.cache/huggingface/` under `--local-dir`) and skips what matches,
  so the command is safe in a start command. The `pip install -U` is there
  because an image's preinstalled `huggingface_hub` may predate both the `hf`
  command and the Xet backend. A gated repository wants `HF_TOKEN` in the
  environment for `hf`; the app never reads it, and keeps it out of what it
  starts (§4.1).
- **Pick an Ubuntu 24.04 image** (`runpod/pytorch:…-ubuntu2404`, or any
  `nvidia/cuda:…-ubuntu24.04`). llama.cpp's CUDA builds for Linux are built on
  24.04; on an older system `setup` installs them and then reports that the
  binary does not start. `cuda-12` is a *family* (§3.1): it keeps working when
  llama.cpp moves to the next CUDA minor — across three pod runs it went
  `b11081` → `b11101` → `b11102` and the line did not change.
- **`cuda-12`, not `cuda-13`, unless the driver is at least as new as the build.**
  On a card the build has no kernels of its own for — A100, H100/H200, B200 —
  the driver compiles the build's PTX when the server starts, and a driver can
  only compile PTX from a CUDA no newer than itself (`nvidia-smi` prints its
  *CUDA Version*). Measured 2026-10-01 on a B200 whose driver is 13.2: the
  `cuda-13.4` build installed, listed the GPU, and both servers stopped at their
  warm-up with `CUDA error: the provided PTX was compiled with an unsupported
  toolchain` — which the app shows, with the way out
  (`mindfork llama setup --backend cuda-12 --set-binary`). `cuda-12` runs there:
  measured 2026-10-02 on a B200 with a CUDA 13.0 driver (580.105.08), the chat
  server was `ready in 93 s` the first time, about 60 s of it that compile, and
  `35 s` once the kernels were cached. The app keeps that cache in its data
  directory (`DIR/data/cuda-cache/`, 134 MB here), which is on the volume — so
  the compile is paid once per card, not after every stop, with nothing to set.
  A `CUDA_CACHE_PATH` of your own still wins.
- **A pod *reset* clears the container disk but keeps the volume**, so after one
  the line finds the app and the sandbox's downloads in place, repacks the
  sandbox image (that lives on the container disk) and **downloads llama.cpp
  again** — its 727 MB sit under `DIR/data/llama/`, which is on the volume only
  if `DIR` is. Keep `--dir` on `/workspace`.
- **Use tmux.** The managed `llama-server` is a child of the app: a dropped SSH
  session takes the app down, the app takes the server down, and reconnecting
  means loading twenty gigabytes again. `tmux new -A -s mindfork` attaches to the
  session if it is still there. In the browser terminal `Ctrl+N` and `Ctrl+T`
  belong to the browser and never arrive — type `/new` and `/thoughts` instead
  (`F1` → *Commands* lists the command behind every key).
- **Port 8001 is taken on every RunPod pod** — by the image's own `nginx`
  (`ss -ltnp` shows it), and 8001 is the embedding server's default. `--verify`
  says so and names the way out, and so does the app, which does not start a
  server on a port another program holds (*Settings → Model/server → Embeddings*
  carries the reason); add it to the line once:
  `--set embed.managed.port=8011`. The chat server's 8000 is free there.
- **A model on a network volume may load slowly through mmap**; if `--verify`
  shows minutes where you expected seconds, add `--set engine.managed.no_mmap=true`.
  For scale: on the pod's own volume a cold 31B Q8_0 with its projector — 34 GB
  — was `ready in 33 s`; the same files warm, 4 s.
- **API keys: by variable name.** A container's `/etc/machine-id` is empty, so the
  app cannot store a key there (its storage is bound to the machine, §3.2) and
  says so. Put the key into the pod's environment (RunPod: a *secret* referenced
  from the template) and name the variable:
  `--set engine.openai.api_key_env=OPENAI_API_KEY`.
- **Your data is on someone else's disk**, unencrypted, and terminating the pod
  deletes the volume. Before you terminate: `mindfork backup -p <password> -o
  /workspace/mindfork-backup.zip` and copy it off; at home, `mindfork stats
  --compare` says which copy is the newer (§2.2). To start *from* your data,
  `mindfork restore <archive>` before `setup` — a binary path it brings from
  another machine is cleared by `setup --llama` (§3.3).
- **The meter does not stop by itself.** RunPod neither stops an idle pod nor
  stops billing when the container exits — stop or terminate it yourself.

Elsewhere the same line works unchanged, with the directory of your choice:
Vast.ai (put it into the *on-start script* — in its SSH and Jupyter modes the
image's entrypoint is not called), and the providers that rent full VMs (Lambda,
TensorDock, DigitalOcean GPU droplets), where nothing is cleared on reboot and
the line is simply run once, or given as cloud-init user data.

### Quick start via environment variables (dev)

Env takes priority over `settings.json` (handy for smoke runs), affecting only the
inference/embedding servers. It **overrides on every launch** rather than
configuring — and an overridden value is written back the first time you change
anything on the settings screen — so for a setup meant to last use
`mindfork setup` (§3.3) instead:

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

> **How many texts a request carries.** Not the physical batch above, which
> counts tokens of one text: this is the number of texts in one HTTP request.
> The app sends **at most 64**, in every mode; a longer input goes out in
> several requests, one after another, and comes back as one answer. Providers
> count differently — measured on 2026-09-29, Gemini's OpenAI-compatible
> endpoint refuses the 101st text (`400 "at most 100 requests can be in one
> batch"`), DeepInfra takes 1024, OpenAI 2048 — and 64 is under all of them.
> Indexing a file never came near the limit (it embeds 16 chunks at a time); the
> `rag_add` tool did, since it embeds a whole text at once, and a text of more
> than a hundred chunks was refused by Gemini. An answer with a different number
> of vectors than there were texts is an error, not a shorter index.

> **A cloud embedder is asked again.** A request to OpenAI, Gemini or the
> OpenRouter gateway that failed for a passing reason — no answer at all, or a
> `408`, `429`, `500`, `502`, `503`, `504` or `529` — is sent again: three
> attempts in all, about 1 s and then about 2 s apart, a provider's
> `Retry-After` obeyed up to 30 s and, above that, not waited for. A local or
> external embedding server is **not** retried — it is up or it is down: the
> health check behind its status chip notices when it answers again, and a
> managed one that died is relaunched.

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
  refuses over it instead of answering from vectors it cannot compare — the
  search whose own query was the new model's first request included. The check
  is made again after every change of the embedding settings, not only after a
  start, and the input prefixes chosen there take effect with it. On a first
  run there is nothing to compare against, so nothing is reported.
- **`/reindex`** re-embeds every stored vector with the current model, in one
  pass over the whole database — memory, the attached-file indexes and **every**
  profile's knowledge base (the embedding server is global, so a model change
  invalidates them all at once). It works from the text already stored, so it
  needs no source files, repairs old entries whose file is long gone, and brings
  attachment indexes back without re-attaching each file. It also **rebuilds an
  attachment index that is missing entirely** — the case you get by copying
  `chats/` to another machine without `data.db` (§2): the file's text is in the
  chat file, so search over it comes back without the original file being
  anywhere near. It runs in the
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

The three bundled dictionaries are third-party work: where each came from, the
exact upstream commit, its licence and the digest of the file as shipped are
recorded in [dictionaries/SOURCES.md](../dictionaries/SOURCES.md), and the
licence texts travel with them — `data/dictionaries/licenses/` in every archive,
package and install. Beside them every artifact carries `THIRD-PARTY-NOTICES.md`
(the licence text of every Rust package that release was built from, generated
from its own `Cargo.lock`) and `licenses/syntaxes/` (the vendored syntax
grammars, which are compiled into the binary and so have no file of their own to
sit beside — [syntaxes/SOURCES.md](../syntaxes/SOURCES.md)). In the Linux
packages both live under `/usr/share/doc/mindfork-rs/`.

**Adding your own.** Drop a Hunspell pair `<name>.aff` + `<name>.dic` into the data
directory's `dictionaries/` and restart — ready-made dictionaries for most languages
are published by the [LibreOffice project](https://github.com/LibreOffice/dictionaries).
The app creates that directory on startup when it is missing (which is the normal case
under an installed `system`/`path` build, where the bundled dictionaries live next to
the binary instead) and puts a `README.txt` in it saying the same thing in the interface
language. That file is written once, at creation: delete it and it stays deleted. A
dictionary added under a bundled one's name replaces it (§2.1).

## 4.1. Python sandbox (`python_exec`)

The `python_exec` tool works in two modes ("Tools" setting → "Python"):

- **Wasmer sandbox** (default) — code runs isolated in WASIX (no access to the
  machine's files; network is a toggle), with preinstalled packages (numpy,
  pandas, sympy, networkx, requests, beautifulsoup4, lxml, feedparser, pyyaml,
  regex, openpyxl, pypdf, tabulate, pillow, matplotlib). Doesn't need Python on
  the machine.
- **Local interpreter** — the system `python`/`python3` (the previous behavior, no
  isolation; the path is configurable).

For the sandbox mode, set it up once (downloads `wasmer` ~206 MB, `python.webc`
and the packages into `data/sandbox/`). Measured on disk after setup: the unpacked
`wasmer` ~730 MB, `python.webc` 43 MB, the packages ~210 MB with their compiled
bytecode, the sandbox image that packs Python and the packages together (~260 MB;
code in the sandbox runs from it and cannot change it), and the compilation cache
~420 MB — about 1.7 GB. Running it again on an
installed sandbox fetches only what is missing, which is how packages a newer
release adds arrive:

```bash
mindfork sandbox setup                    # --force — re-download
mindfork sandbox setup --enable-python    # …and switch the tool on afterwards
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
| `openrouter` | model and voice, each picked from the gateway's own list — no default for either; key shared with the other tabs in the `openrouter` mode (§3.2) |
| `external` | URL of any OpenAI-compatible TTS server (`http://127.0.0.1:8880/v1`), opt. model/voice |

The **Mode** row cycles through them in that order.

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

### Speech through the OpenRouter gateway (`openrouter`)

Some twenty speech models of a dozen vendors behind the key the rest of the
`openrouter` mode uses (§3.2). The tab shows, in this order: **Model**,
**Voice**, **User voice**, **OpenRouter API key**, **OpenRouter API key
(env)**, **Base URL (opt.)**, **Name the app to OpenRouter**, and then
**Speech rate** and the three behaviour switches every speech mode has. There
is **no "Instructions" row**: it is not a field of the gateway's speech route,
so nothing typed there would reach a model.

1. Set **Mode** to `openrouter`. If the key is already stored for another tab,
   there is nothing to enter; otherwise enter it here, or name the variable that
   holds it.
2. `Enter` on **Model** lists the gateway's speech models — with a key your
   account's own list, without one the public list, so it opens before a key is
   entered. The first row is "Type a name by hand…". A row is the model's slug
   and `voices: N` where the model lists voices. It shows **no price**: the
   catalogue publishes a number without its unit, and the unit differs by model
   — measured, `x-ai/grok-voice-tts-1.0` is priced per *character*, Gemini's
   speech models per token — so "per 1M tokens" beside it would be the app's
   claim, and for most models a wrong one.
   **There is no default model**: until one is chosen, `/tts` says speech is not
   configured.
3. `Enter` on **Voice** — and on **User voice**, for a second voice that reads
   your own lines — lists the voices of the model the Model row names. The list
   comes out of the same answer as the model list, so it costs no second
   request; `Ctrl+R` in the list asks again. A model that lists no voice, or a
   model name typed by hand that the list does not hold, has nothing to pick
   from: there `Enter` opens the ordinary text field.

What to expect, all of it measured on 2026-09-29
([docs/research/openrouter-mode.md](research/openrouter-mode.md) §4.4, §13):

- **Most models need a voice.** Without one the gateway refuses — *"An explicit
  voice is required for this TTS provider."* — while a few (Fish Audio's) list
  none and speak without. The app does not refuse ahead of the gateway: what is
  missing is said in the gateway's own sentence, in the chat.
- **A refusal reads as the gateway wrote it**: `TTS: status 401 Unauthorized:
  User not found.` for a key it does not know. The key is **not checked ahead
  of time** on this tab (§3.2), so that sentence arrives with the first `/tts`.
- **The audio format is settled by the app.** No format is taken by all of the
  gateway's speech models — raw samples (`pcm`) by 19 of 21, `mp3` by 18, and
  nothing else by the route. The app asks for raw samples first; a model that
  refuses them is asked **once** for MP3, and the answer is remembered for that
  model until the app is closed, so the refused request is paid once a session
  rather than once a fragment. Rate and channels are read from the answer's own
  label: most models speak at 24 kHz, Fish Audio's at 44.1 kHz, one answers in
  stereo.
- **Speech rate may do nothing.** The rate is sent, and a model may ignore it:
  Gemini and Grok through the gateway produce the same length of audio at 0.5,
  1.0 and 2.0.
- **Long messages** go out in fragments of at most 2000 characters, cut on
  sentence boundaries, as for every speech provider (OpenAI: 4096).
- **The two headers** that name the app to OpenRouter (§3.2) travel with speech
  requests and with the list request too, under the same switch.

**An `external` speech server pointed at `openrouter.ai` does not work**, unlike
the other tabs' `external` sections: an external server is asked for WAV, which
the gateway refuses. The URL row says so when the address is OpenRouter's; use
the mode.

> **MP3 from any speech server is played whole now.** A server that streams an
> MP3 writes the length into the file's first frame before it knows it. The
> player used to trust that number: on one of the gateway's models its
> arithmetic overflowed — a panic in a build that checks for it — and another's
> sentence stopped at 2.35 s of 4.78 s. It no longer trims by that tag — in
> every speech mode, `external` included — at the cost of the encoder's few
> dozen milliseconds of padding left in.

## 4.4. Watching YouTube videos (`youtube_watch`)

The `youtube_watch` tool tells the assistant what a video **says and shows** —
a description with timestamps, not a raw transcript. Configured in the "Video"
group of the "Tools" section (`Ctrl+P`):

| Field | Meaning |
|---|---|
| Provider | who watches: `gemini` (default) — Google's own API — or `openrouter` — the same Gemini models behind an OpenRouter key (below) |
| Model | the Gemini model that watches (`gemini-3.5-flash` by default) |
| Input resolution | how finely frames are sampled — **measured to change nothing on Gemini 3.x**; on 2.5 it is `low` ≈ 100 vs `medium` ≈ 295 tokens per second of video |
| Max video length | refuse anything longer (default 30 min; `0` — no ceiling) |
| Gemini API key | the Gemini key, stored on this computer (encrypted with a machine key, ADR 0008) |
| Gemini API key (env, opt.) | env-variable name — a fallback when no key is stored in settings |

These are the rows with `gemini`. With `openrouter` the group is the gateway's,
described under its own heading below; each provider keeps its own model and
its own key variable, so switching between them retypes nothing.

**The key is the shared Gemini one** (ADR 0008): if it is already entered for
chat or embeddings, nothing else is needed — and if it is not, enter it right
here. That row exists because the "Model" section only shows a key field for a
slot whose mode *is* that cloud, so with a local or OpenAI setup there would
otherwise be nowhere to put a Gemini key at all. It works whatever your chat engine
is — including a local `llama-server` — because the tool calls the provider
itself and returns text into the conversation. Gemini is currently the only
model family that takes a YouTube video — OpenAI and Anthropic take text and
images only — and it is reached either way: by Google's own API, or through the
OpenRouter gateway.

Cost is per second of footage, not per video: on the default model a 10-minute
video is ~55k tokens on Google's side (a few hundred in your conversation, since
only the answer comes back). Hence the length ceiling — above it the tool
refuses and suggests a segment, and the model can pass `start`/`end` in seconds
to watch just part of a long talk. That holds for Google's own API, which cuts
the segment and charges for the segment; through the gateway a part costs the
whole video (below).

Without a key the tool still works in a reduced form: title, channel, length and
the author's description, read from the public watch page. The same metadata is
what `fetch_url` now returns for a YouTube link, instead of failing to find
readable text on it. Only **public** videos can be watched — not private or
unlisted ones. Both paths need the web-access switch on.

### Watching through the OpenRouter gateway (`openrouter`)

The same Gemini models behind the key the rest of the `openrouter` mode uses
(§3.2) — for when the gateway's key is the one you have. With **Provider** set
to `openrouter` the group shows, in this order: **Provider**, **Model**, **Max
video length (min)**, **OpenRouter API key**, **OpenRouter API key (env,
opt.)**, **Name the app to OpenRouter**. There is **no "Input resolution"
row**: the gateway carries no such setting, so nothing chosen there would reach
a model.

1. Set **Provider** to `openrouter`. If the key is already stored for another
   tab, there is nothing to enter; otherwise enter it here, or name the variable
   that holds it.
2. `Enter` on **Model** lists the gateway's models that take video — with a key
   out of your account's own list, without one the public list, so it opens
   before a key is entered. The first row is "Type a name by hand…". A row is
   the model's slug, its context window and the price of a million tokens in
   and out; a model that takes no tools is not marked here, since a model that
   watches is asked to describe, not to act. **The Gemini family opens the
   list, and it is the one to choose from**: measured, it is the family that
   takes a YouTube *link*, while the others that list video input go to
   download the link as a file and refuse (`qwen/qwen3.6-flash`: *"Missing
   Content-Length of multimodal url"*). They are listed all the same, after
   Gemini: the list is what the gateway says takes video, and the app orders it
   without leaving anything out.
   **There is no default model**: until one is chosen, the tool works in its
   reduced form.

What to expect, all of it measured on 2026-09-29
([docs/research/openrouter-mode.md](research/openrouter-mode.md) §4.5, §14):

- **The whole video is read and charged, even when a part is asked for.** The
  gateway takes no segment bounds. When the model asks for a part
  (`start`/`end`), the app names that part in the request, in words; the answer
  is about the part, the bill is for the whole video, and the tool's result
  says so — so that the model knows asking for the next part costs the whole
  video again. Through Google's own API a 20-second part of a 67-second video
  is 1 874 tokens against 6 147 for the whole.
- **So the length ceiling measures the whole video.** A video longer than "Max
  video length" is refused whatever part of it is asked for, and the refusal
  does not suggest a segment, which would cost the same: the ceiling and the
  provider are yours to change. A video whose length could not be read from
  YouTube is refused as well, unless the ceiling is `0` — through Google's API
  such a video is clipped to the ceiling, and the gateway has nothing to clip
  by.
- **A video costs what it costs through Google's API**: the 67-second one is
  about 6 100 prompt tokens either way — $0.002 on
  `google/gemini-3.5-flash-lite`, a little over a cent on
  `google/gemini-3.5-flash`.
- **Any link you paste is fine.** The app hands the gateway the video's
  canonical address, built from the video's id, and nothing else of the link.
  It matters: measured, the same address with one more parameter (`&t=20s`,
  `&feature=share`) was read as a web page and not as a video — 551 337 prompt
  tokens and $0.165, ninety times the price.
- **An answer that was not made from the video is an error.** The gateway can
  answer `200` with a plausible description of a link it did not read as a
  video. The app tells the two apart by the gateway's own count of video
  tokens: where there are none, the text is thrown away and the tool reports
  *"the gateway answered without reading the video…"*.
- **A refusal reads as the gateway and the provider wrote it**: `OpenRouter
  video: status 502 Bad Gateway: Provider returned error: The caller does not
  have permission` is a video that is private or does not exist; `404 "No
  endpoints found that support input video"` is a model that takes no video;
  `401 … User not found.` is a key the gateway does not know. The key is **not
  checked ahead of time** here (§3.2), so that last sentence arrives with the
  first video.
- **Reasoning is kept to the least the model allows**, since the reply's
  ceiling covers it. Before its first video the app asks the gateway about the
  model (`GET /model/<slug>`) and then asks for the lowest reasoning effort the
  model lists — `minimal` on Gemini 3.5, which cannot be told not to reason. A
  model that lists none is asked nothing about reasoning.
- **The two headers** that name the app to OpenRouter (§3.2) travel with video
  requests, with the question about the model and with the list request,
  under the same switch.

**The base URL has no row.** To reach the gateway through a proxy, set
`video.openrouter.url` in `settings.json`; empty, it is
`https://openrouter.ai/api/v1`.

## 5. Importing from other apps

A one-off idempotent import of profiles and chats from a file in the neutral
**mindfork-import** format (spec — [docs/import-format.md](import-format.md)):

```bash
mindfork import path/to/export.json
```

The file is emitted by an **external converter** that knows the source app's
format (for non-public apps, like LameLLaMA, the converter lives in a separate
private repository). Profiles, chats, and (optionally) global sampling/interface
settings are imported; unknown fields are ignored, a file from a newer format
version is rejected. The command doesn't launch the TUI and exits the process;
re-running it doesn't create duplicates (deterministic ids from stable keys).

> The former `import-lamellama <dir>` command has been removed — its role is now
> played by the "external converter → `mindfork import`" combo.

## 6. Running

```bash
cargo run            # dev
./target/release/mindfork   # release
```

To look around before configuring anything, run the **demo**: sample data and
a scripted engine in a throwaway temp folder (removed on exit), no model or
key needed —

```bash
mindfork demo
```

Needs a **real terminal** (TUI): started with its output redirected or with no
console, the app says so and exits with code 2. With no model connected yet, an
empty chat lists the ways to connect one. Basic keys: `F1` — help, `Ctrl+P` —
settings, `Esc` — chat list
(open/close) and cancel generation, `Ctrl+N` — new chat,
`Ctrl+U` — write a message as the user (impersonation), `Ctrl+Q` — quit.
Scrolling the feed — `PageUp`/`PageDown` or the mouse wheel; mouse capture for the
wheel is a toggle, `Ctrl+W` (off by default, so native text selection works;
with capture on, text is selected while holding `Shift`).
Ctrl shortcuts work under any keyboard layout (including Russian).

**Theme and the terminal's background.** The default theme (`auto`) asks the
terminal for its background colour at start-up — OSC 11 — and matches the code-block
shading and the selection highlight to the answer. Windows Terminal, VS Code's
terminal, JupyterLab's and a plain SSH session all answer; the **legacy Windows
console does not**, and is treated as dark, which is what it is. Nothing waits on
the reply: the question goes out before the app opens its storage. Override it
with

```bash
MINDFORK_TERMINAL_BG=light   # or: dark, off (don't ask at all)
```

— useful in a container, in a test, or on a terminal that answers wrongly.
Choosing `dark`/`light` in *Settings → Interface → Theme* does the same thing
permanently. See spec §11.6 and
[terminal-background-detection.md](terminal-background-detection.md).

All of that is the **system** colour mode, where the terminal supplies the
background. *Settings → Interface → Colour mode → full* has the app paint its
own — background and text colour together, with the contrast held by the theme
rather than by whatever the terminal is set to — and asks the terminal nothing.
From the command line:

```bash
mindfork setup --set interface.theme_mode=full --set interface.full_theme=light
```

**No colours at all.** *Colour mode → monochrome* draws with the terminal's own
two colours and no bold, italic or underline — for a terminal that renders
attributes badly, an e-ink panel, a screen reader. The app also starts in it
when **`NO_COLOR`** is set (to anything but an empty string) and no colour mode
was chosen in the settings; a chosen one wins over the variable.

```bash
NO_COLOR=1 mindfork                                  # this run, if no mode was chosen
mindfork setup --set interface.theme_mode=mono       # always
mindfork setup --set interface.theme_mode=system     # colours, NO_COLOR or not
mindfork setup --set interface.theme_mode=null       # back to following the environment
```

See [manual.md](manual.md) §8 and [history/theme-modes.md](history/theme-modes.md).

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

**A multi-model endpoint needs a third variable** — `MINDFORK_ENGINE_MODEL`
(and `MINDFORK_EMBED_MODEL` for the embedder). A gateway routes on the request's
`model` and answers `400` without one, so the whole live set was a single-model
stack's privilege until these existed; unset, nothing is sent and the request is
what it always was. Against OpenRouter:

```powershell
$env:MINDFORK_ENGINE_URL         = "https://openrouter.ai/api/v1"
$env:MINDFORK_ENGINE_KEY         = $env:OPENROUTER_API_KEY
$env:MINDFORK_ENGINE_MODEL       = "<vendor/model>"   # any model on the account
$env:MINDFORK_LIVE_GATEWAY_MODEL = "<vendor/model>"   # …and, for the smoke below, one that reasons

cargo test a_gateway_streams_thoughts_under_its_own_field_name -- --ignored --nocapture
cargo test tool_call_is_emitted_and_parsed                     -- --ignored --nocapture
cargo test simple_generation                                   -- --ignored --nocapture
```

The settings' thinking switch reaches a gateway as its own `reasoning` field
(docs/history/gateway-thinking-switch.md); its smokes each read a declared model, and
fail rather than skip once one is declared:

```powershell
$env:MINDFORK_LIVE_SWITCH_ON_MODEL           = "anthropic/claude-haiku-4.5"  # reasons only when asked
$env:MINDFORK_LIVE_SWITCH_OFF_MODEL          = "qwen/qwen3.6-27b"            # reasons by default
$env:MINDFORK_LIVE_MANDATORY_REASONING_MODEL = "deepseek/deepseek-r1"        # must reason

cargo test a_gateway_reasons_when_the_switch_is_on                 -- --ignored --nocapture
cargo test a_gateway_stops_reasoning_when_the_switch_is_off        -- --ignored --nocapture
cargo test the_switch_off_completes_on_an_endpoint_that_must_reason -- --ignored --nocapture
```

**Do not run the whole `--ignored` set against a paid gateway.** It is ~235
smokes, 121 of them multi-round end-to-end conversations written for a local
stack where a token costs nothing and a 20k-token ballast is free: against a
metered endpoint that is hours of wall clock and a real bill. Name the smokes
you need, as above. One more thing follows from "written for a local stack":
some of those smokes assert **llama.cpp's** behaviour rather than an
OpenAI-compatible server's — `accepts_creative_sampling_extensions` checks that
the sampling *extensions* are accepted, which a gateway drops by design (§3).
A red result there is a statement about the stack, not necessarily a defect.
(`auto_compaction_fires_without_the_command_live` used to be the other one: it
leaves the window for the engine to report, and a gateway reported none. The
catalogue answers that now, §3 — so on a gateway whose catalogue lists
`context_length` it is no longer red by construction, though no run has
re-measured it there since.)

`a_gateway_streams_thoughts_under_its_own_field_name` is the one that must be
green: it runs only when `MINDFORK_LIVE_GATEWAY_MODEL` **declares** a model that
reasons, and then fails rather than skips if no "thoughts" arrive — the
gateway's field name is the thing it is there to prove
([docs/research/openrouter-external.md](research/openrouter-external.md) §8).

**The `openrouter` mode has smokes of its own**, since everything above reaches
the gateway through the `external` client — which is their control arm, not
their subject. They are declared by one variable and build the client the way
the supervisor builds it:

```powershell
$env:MINDFORK_OPENROUTER_KEY = $env:OPENROUTER_API_KEY
$env:MINDFORK_ENGINE_URL     = "http://127.0.0.1:8000/v1"   # a local llama.cpp, for the switch

cargo test gateway_live -- --ignored --nocapture --test-threads=1
```

That filter is the mode's whole live gate, **34 smokes**: the twelve of the
chat side, described here, the embedder's four, the ten of speech and the
eight of video, all described below. The twelve are about
ten cents — most of it the orchestrator's turns, which send the app's whole
prompt and tool set: the client's nine (thoughts and who
served, a tool round trip, a muted turn against its `external` control, a tool's
image with a blind control, a `:nitro` slug, the key, the catalogues,
embeddings), and the orchestrator's three on the production supervisor — a chat
moved from the local server to the gateway and back inside one session,
`/continue` on both arms of the route table, a refused key as the status. Each
names the model it was measured on and takes another from a variable of its own
— `MINDFORK_OPENROUTER_MODEL`, `…_REASONING_MODEL`, `…_MUST_REASON_MODEL`,
`…_ALWAYS_REASONS_MODEL`, `…_VISION_MODEL`, `…_EMBED_MODEL`, `…_CONTINUES_MODEL`,
`…_RESTARTS_MODEL` — because a model's name ages and its **kind** is what the
smoke is about; a model that turns out not to be of that kind fails the smoke
rather than skipping it. Without `MINDFORK_ENGINE_URL` the switch is skipped and
the rest run.

**The embedder in that mode has four smokes more**, in a module of their own
(`app/orchestrator/tests/gateway_live_embed.rs`). They run on the production
supervisor and through the tools the model calls, so what is embedded goes the
whole road — the input convention, the model-change guard, the batch cap, the
retry, the client — and they need no chat engine:

```powershell
$env:MINDFORK_EMBED_URL      = "http://127.0.0.1:8001/v1"   # a local bge-m3
$env:MINDFORK_OPENROUTER_KEY = $env:OPENROUTER_API_KEY
$env:MINDFORK_GEMINI_KEY     = "<key>"                      # Google's own endpoint

cargo test gateway_live_embed -- --ignored --nocapture --test-threads=1
```

Well under a cent a run — a few hundred short texts; the gateway's own meter
did not move by a millionth of a dollar over one. What they hold:

- **An index built locally answers a query the gateway embedded.** The
  knowledge base is filled on the local `bge-m3`, the embedder is moved to the
  gateway's `baai/bge-m3` by an edit of the settings inside the session, and the
  passage is found with **no reindex offered** — no notice, the same embedding
  generation, the base not stale; what the gateway embedded is found from both
  sides after the move back. The control arm is the guard itself: the same move
  to a model of the same width that is another model
  (`intfloat/multilingual-e5-large`) is said once and the search refuses.
  Needs `MINDFORK_EMBED_URL` and the gateway's key;
  `MINDFORK_OPENROUTER_EMBED_MODEL` names another model for the first half.
- **A text of more than a hundred chunks is added whole through the gateway**,
  on `google/gemini-embedding-2` — or on what
  `MINDFORK_OPENROUTER_CAPPED_EMBED_MODEL` names, which has to be a model that
  takes a hundred inputs and no more. Its control arm is the refusal: the bare
  client, asked for the very chunks the tool was given in one request, must
  answer `at most 100`.
- **The same through Google's own endpoint** (`gemini-embedding-001`), which is
  where the number was measured. Needs `MINDFORK_GEMINI_KEY`.
- **A refused key is the embedder's status.** It reads no variable — the key
  under test is a wrong one the smoke makes up — and needs only the network.

A smoke whose variable is unset is skipped and says which.

**Speech through the gateway has ten**, in two modules: five on the mode's own
client and one on the player, which needs no network
(`shared/tts/gateway_live_tests.rs`), and four on `/tts` through the
application (`app/orchestrator/tests/gateway_live_speech.rs`). They are declared
by the same one variable and need no local server:

```powershell
$env:MINDFORK_OPENROUTER_KEY = $env:OPENROUTER_API_KEY

cargo test gateway_live_speech -- --ignored --nocapture --test-threads=1   # the application's four alone
```

A few cents at most. **The assertion is the text**: a `200` with audio in it is
not yet speech, so each clip is transcribed back through the gateway's own
`/audio/transcriptions` and the sentence is looked for in what was heard. What
they hold:

- **The client's five**: the speech list, with a key and without; a model that
  takes raw samples only, asked once; a model that takes MP3 only — refused,
  asked again, remembered — where what is transcribed is what the
  application's own decoder made of the container; a model at 44.1 kHz, its
  rate read from the label and, through the sound card, heard to last what the
  clip lasts; a refused key, in the gateway's words.
- **The player's one**: a second of audio at 44.1 kHz mono and a second at
  24 kHz in stereo each last a second through the sound card. It stands in for
  the one model that answers in stereo, which takes 11 to 23 s a sentence and
  is not spoken by the smokes.
- **The application's four**: `/tts` over a chat, with the engines built from
  the settings, the playback queue and the sound card — the MP3-only model in
  **two voices** (the assistant's and the user's, two engines on one memo), the
  44.1 kHz model with no voice set, the raw-samples model, and a model that
  needs a voice and has none, which is an error in the feed in the gateway's
  sentence.

Each names the model it was measured on and takes another from a variable of
its own, on the terms of the chat side's — the **kind** is the subject, and a
model that is not of that kind fails the smoke:

| Variable | The kind | Measured on |
|---|---|---|
| `MINDFORK_OPENROUTER_TTS_RAW_MODEL` | takes raw samples only | `google/gemini-3.8-flash-lite-tts` |
| `MINDFORK_OPENROUTER_TTS_MP3_MODEL` | takes MP3 only | `minimax/speech-2.8-turbo` |
| `MINDFORK_OPENROUTER_TTS_44K_MODEL` | answers at 44.1 kHz | `fish-audio/s1` |
| `MINDFORK_OPENROUTER_LISTENER` | the speech-to-text model that hears the speech back | `openai/whisper-large-v3-turbo` |

**The playback halves need a sound card, and are audible.** On a machine
without one they skip, saying so: the application's four and the player's one
whole, and of the client's five the half of the 44.1 kHz smoke that hears the
rate — the half that reads it still runs.

**Video through the gateway has eight**, in two modules: five on the mode's own
client (`shared/video/gateway_live_tests.rs`) and three on `youtube_watch` as
the agentic loop calls it, on the client the registry builds
(`features/tools/gateway_live_tests.rs`). They are declared by the same one
variable and need no local server and no chat engine; the tool's three also
need the network to YouTube, for the video's title and length:

```powershell
$env:MINDFORK_OPENROUTER_KEY = $env:OPENROUTER_API_KEY

cargo test video::gateway_live -- --ignored --nocapture --test-threads=1   # the client's five alone
cargo test tools::gateway_live -- --ignored --nocapture --test-threads=1   # the tool's three alone
```

About a cent for the eight: one watch of the video is about $0.002. **The video
is one no model can describe from memory** — 67 seconds, published 2026-09-08
(`IwZVXmQdX1E`) — and what is asserted of it is a line of what is said in it
("bound for the moon"), and, by the client itself, that the gateway counted
video tokens. What they hold:

- **The client's five**: the list of models that take video, with a key and
  without; the video watched — a line of what is said in it; a link the gateway
  does not read is an error, not a description; a video that is not there is a
  refusal in the provider's words; a refused key, in the gateway's.
- **The tool's three**: a link as a person pastes it, with `&t=20s&feature=share`
  in it, is watched and transcribed; a segment named in words is what the
  answer is about, with the note that the whole video was read; a video over
  the ceiling is refused whatever part of it is asked for — nothing spent.

The smoke of the unread link sends `https://example.com/`, which the gateway
answers `200` with no video tokens for $0.000005. **The expensive form of the
same defect — a YouTube address with one more parameter, $0.165 a request — is
not sent by the gate**: it is covered by a unit test, against the body the
gateway answered with when it was measured.

| Variable | The kind | Measured on |
|---|---|---|
| `MINDFORK_OPENROUTER_VIDEO_MODEL` | takes a YouTube link as a video — of the Gemini family | `google/gemini-3.5-flash-lite` |

On Windows the application itself can be driven through the mode, in a hidden
console on a scratch data root: `python tools/console_probe.py --scenario
gateway` types a question into the chat, reads both of the mode's chips off
the status line, indexes a file through the gateway's embedder (`/rag add`),
opens the settings and the model list, and checks that the key reached neither
the log nor the settings file. It also types `/tts` and reads the `speaking`
chip on and off the status line, then reads the Speech tab, the list of speech
models and the list of voices off the screen — so it, too, is **audible**, and
needs a sound card. And it finds the video group by the settings' search, reads
its rows — the provider named, no row for the resolution — and the list behind
the model row: the Gemini family first, and no "no tools" mark. It reads
`OPENROUTER_API_KEY` by name, as the settings do;
`MINDFORK_OPENROUTER_VIDEO_MODEL` names the video model it sets.

### 7.2. The remote gate (rented GPU, no local stack)

`tools/e2e_hf.py` runs the same suite against **ephemeral Hugging Face Inference
Endpoints**, so the live gate no longer requires a machine with a GPU. It rents
three: a real `llama-server` (HF's llama.cpp engine, on the GPU the chosen
model's record names), an
embedding endpoint (the same engine in `embeddings` mode, bge-m3 Q8_0 on a T4),
and a *second, different* embedding model (multilingual-e5-large-instruct q8_0)
for the smokes that guard against an embedding-model change. All deliberately
the same GGUFs and quantizations as the local stack, because the memory gates'
similarity thresholds are calibrated against exactly those models.

The chat endpoint runs **one model per run**, chosen with `--chat-model`:

| `--chat-model` | What it is | GPU |
|---|---|---|
| `gemma-4-31b` | the default, and what the memory thresholds were calibrated against | L40S 48 GB, us-east-1 |
| `qwen-3.6-27b` | a second model family, so a Gemma-shaped assumption is caught here | L40S 48 GB, us-east-1 |
| `gpt-oss-120b` | weights **split across two files** (63.39 GB, Q8_0), OpenAI open-weights, text-only | H200 141 GB, us-west-2 |

The name is one decision — it carries the repository, the weights, the vision
projector, the download filter and the hardware together, so an impossible
combination cannot be typed and a 63 GB model cannot be sent to a 48 GB card.
The projector is deployed with the model: three smokes require one and **fail
rather than skip** on a text-only server, by design.

`gpt-oss-120b` is the exception that proves that rule, and it is handled by
declaration rather than by exception. It has no projector *in existence*, so the
runner sets `MINDFORK_LIVE_TEXT_ONLY=1` and those three skip, saying so in the
log; and its weights are split, so it sets `MINDFORK_LIVE_SPLIT_MODEL=1`, which
turns on the smoke that checks a part number (`-00001-of-00002`) never reaches a
chat header. Both are **derived from the model that was deployed**, never passed
as flags, so neither can disagree with the stack. `--no-mmproj` deliberately
does not set the first: that flag exists to deploy a *sighted* model blind.

```powershell
$env:HF_TOKEN = "hf_..."          # fine-grained, Inference Endpoints
python tools/e2e_hf.py run        # create, run the suite, delete
python tools/e2e_hf.py run --dry-run          # payloads only, spends nothing
python tools/e2e_hf.py run --chat-model qwen-3.6-27b   # the other model family
python tools/e2e_hf.py run --chat-model gpt-oss-120b   # split weights, H200, text-only
python tools/e2e_hf.py run --filter e2e_live  # a subset
python tools/e2e_hf.py run --shard 2/4        # every fourth smoke, on endpoints of its own
python tools/e2e_hf.py run --no-alt-embed     # skip the second embedding model
python tools/e2e_hf.py list                   # what is running right now
python tools/e2e_hf.py sweep --dry-run        # what the sweeper would remove
python tools/e2e_hf.py delete-run --run-id 1009-1944   # one run's endpoints, by name
```

The token needs **two** boxes ticked under User Permissions → Inference:
*Manage Inference Endpoints* and *Make calls to Inference Endpoints*. Holding
only the first creates an endpoint that then rejects every request — i.e. it
fails after the meter has started. `python tools/hf_probe.py doctor` reports
which one is missing, and distinguishes that from the other causes of a 403 (no
payment method on the account, or an org token pending approval).

**Cost and the guarantee.** The whole suite is ~70 minutes of smokes on one
card, ≈ $4 (L40S $1.80/hr + two T4s at $0.50/hr, billed by the minute; measured
2026-10-09) — so CI deals it across shards, each a job with three endpoints of its
own, and four shards cost about one run plus three deploys. A `gpt-oss-120b` run
costs about three times as much, because the H200 that holds it is $5.00/hr. The endpoints are deleted from `finally`, from
`atexit` and from the SIGINT/SIGTERM handler, and the deletion is **verified** —
a failed delete exits non-zero even when the tests passed. In CI the runner is
`exec`ed, so a cancelled job's signal reaches it, and the job's last step runs
`delete-run` whatever happened: the names are derived from the run id, so it
needs nothing from a runner that died. If both fail, the endpoints scale to zero
after their idle window (15 min, so ≈ $0.57 worst case) and the sweeper removes
them, which also reclaims endpoint quota (≈$1.25 on an H200). Use `--keep` only
when debugging, and delete afterwards — by name, or the whole run with
`delete-run`.

In CI: **Live e2e (HF Inference Endpoints)** — `workflow_dispatch` only, with
test-filter, model, GPU and shard-count inputs (four shards by default, each job
under a 60-minute ceiling); it needs the `HF_TOKEN` repository secret. HF does not
always have the card: a shard whose endpoint *fails to start* deletes what it
created and fails alone, and `gh run rerun <run-id> --failed` runs just that shard
again ([docs/research/e2e-gate-budget.md](research/e2e-gate-budget.md) §8). **Live e2e
sweeper** runs every six hours as the backstop (hourly until 2026-08-21; the cadence
bounds how long an orphan holds endpoint quota, not money). Design and decisions:
[docs/history/remote-e2e-hf.md](history/remote-e2e-hf.md),
[docs/research/remote-e2e-gpu.md](research/remote-e2e-gpu.md); the third model
and what it exists to cover:
[docs/research/e2e-gpt-oss-120b.md](research/e2e-gpt-oss-120b.md).

Two smoke groups still need the local machine and are **not** covered remotely:
the managed-server smoke (`MINDFORK_LLAMA_BIN` — it needs a child process of
our own) and the Python sandbox smokes (they need a provisioned `wasmer`
sidecar). The cloud-provider smokes (Anthropic / Gemini / OpenAI / TTS) need
their own keys and are unrelated to the GPU.

### 7.3. The container gate (JupyterLab + a CPU stack, no GPU)

`docker/` holds a compose stack that ends in a **browser JupyterLab terminal**
with `mindfork` built from the current working tree, talking to two CPU
`llama-server` containers — Gemma 4 E2B-it Q8_0 for chat, bge-m3 Q8_0 for
embeddings (the chat container runs `-b 256 -ub 256`, the CPU batch of §3 —
the containers are external servers to the app, so the line is the compose
file's, not the launcher's):

```bash
cd docker && cp .env.example .env && docker compose up --build
# → http://localhost:8888/lab?token=mindfork → Terminal → mindfork
```

Two things it is for. The first is **JupyterLab itself**: its terminal is
xterm.js in a browser tab, which is where `Ctrl+N`/`Ctrl+T` never arrive, OSC 52
is dropped and `/export` becomes the way out (§ the notes in
[docs/history/command-only-control.md](history/command-only-control.md)) — and
that host reproduces on no Windows terminal. The second is that both server
ports are published, so the `#[ignore]` suite runs from the host against it:

```powershell
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
$env:MINDFORK_EMBED_URL  = "http://127.0.0.1:8001/v1"
cargo test -- --ignored --nocapture --test-threads=1
```

The lab container also carries Node and the reference MCP filesystem server, so
the plugin host (§4.2) is exercisable there — pre-configured and scoped to the
mounted work directory, with its master switch off, as the double opt-in
requires.

It can also be reached over **SSH**, which is the way to check the *opposite* of
what JupyterLab is for: escape-sequence behaviour differs per emulator, so
anything terminal-facing has to be seen from a terminal that is not xterm.js.
Put a **public** key in `docker/.env` and the container starts an sshd — off
entirely when the variable is empty:

```bash
LAB_SSH_PUBKEY="ssh-ed25519 AAAAC3... you@host"
```

```bash
ssh -p 2222 jovyan@127.0.0.1
```

`mindfork` and `tmux` are both on `PATH` there. It runs as uid 1000 on port
2222, keys only, `jovyan` only, published on the loopback interface alone — a
session lands where `docker exec` already lands, so it is a transport rather
than a privilege. Details and the `StrictModes` caveat:
[docker/README.md](../docker/README.md) §6.1.

**It does not replace §7.2.** The memory and self-model gates are calibrated on
31B-class models; a 2B-effective one fails some of them for reasons that are not
defects. Use it for protocol, streaming, tool-call and RAG plumbing, and keep the
rented gate for a verdict worth recording. Budget ~9–10 GiB of Docker VM memory
and ~6.2 GiB of disk for the weights. Full instructions and every knob:
[docker/README.md](../docker/README.md); the design and what was rejected:
[docs/research/docker-jupyter-env.md](research/docker-jupyter-env.md).
