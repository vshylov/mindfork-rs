+++
title = "Install"
description = "Get mindfork for Windows or Linux: the installer, the packages, the portable archives or crates.io — what each one is for, how to verify it, and what to do first."
updated = "2026-09-28"
template = "doc.html"

[extra]
# Include the schema.org SoftwareApplication here too: this is the page a search
# engine should be able to answer "where do I get it" from. See doc.html.
software = true
+++

Every download below is on the [releases page](https://github.com/vshylov/mindfork-rs/releases),
and the same version is on [crates.io]({{ config.extra.crates }}) for `cargo install`.
One binary, no runtime to install first, nothing that phones home.

## Windows

**The installer — `mindfork-rs-v<version>-x86_64-setup.exe`.** It puts the app in
Program Files (or your user folder, your choice), makes the Start-menu entry, and
offers an optional box to add `mindfork` to your `PATH` so the command works from
any terminal. Uninstalling takes all of it back out.

> **SmartScreen will warn you.** The binaries are **not code-signed yet** — an
> unsigned installer earns a blue "Windows protected your PC" screen, and the way
> past it is *More info → Run anyway*. That is a real warning about a real
> absence, not a formality: check the download's checksum below if you want more
> than our word for it. The state of signing is on the
> [code signing policy](/code-signing-policy/) page.

**Scoop**, if that is how your terminal gets its tools — the release's archive,
its hash checked, with your data kept in `%APPDATA%` so an update leaves it alone:

```powershell
scoop bucket add mindfork https://github.com/vshylov/scoop-bucket
scoop install mindfork/mindfork
```

Scoop runs no installer, so SmartScreen has nothing to say.

**The archive — `mindfork-rs-v<version>-x86_64-windows.zip`.** The same binary
with nothing installed anywhere: unpack it and run `mindfork.exe`. The app keeps
its data next to itself, so the folder — or the USB stick it sits on — is
self-contained and moves with you.

Windows 10 or 11, x86-64. Nothing else: the C runtime is linked statically, so
there is no Visual C++ redistributable to chase.

## Linux

**Packages**, if you want the app on your `PATH` and in your package manager's
records:

```bash
sudo apt install ./mindfork-rs_<version>-1_amd64.deb        # Debian, Ubuntu
sudo dnf install ./mindfork-rs-<version>-1.x86_64.rpm       # Fedora, RHEL
sudo pacman -U   ./mindfork-rs-<version>-1-x86_64.pkg.tar.zst   # Arch
```

On **Arch** the same package is in the AUR, updated by every release:
`yay -S mindfork-rs-bin` (or `paru`, or any helper).

**The archive — `mindfork-rs-v<version>-x86_64-linux.tar.gz`** — is the portable
route, the same as on Windows: unpack, run `./mindfork`, and the data folder
lives beside the binary.

**One line**, which unpacks that archive into `~/mindfork`, checks it against the
release's checksums and makes it startable — on a bare image it installs the one
system library the app needs (ALSA's), or tells you the command:

```bash
curl -fsSL https://github.com/vshylov/mindfork-rs/releases/latest/download/install.sh | sh
```

The script is an asset of the release like the archive itself — listed in
`sha256sums.txt`, covered by the same attestation — and short enough to read
first: download it and run `sh install.sh`. `--dir` chooses the folder, and
everything after `--` is handed to `mindfork`, which is how a rented GPU box
goes from nothing to a loaded model in one line
([the guide](https://github.com/vshylov/mindfork-rs/blob/main/docs/install.md),
§3.3–§3.4). Running it again is safe: nothing already in place is downloaded
twice, and your data is never touched.

x86-64, glibc 2.35 or newer (Ubuntu 22.04 and anything later). A terminal
emulator you already have.

## Verify what you downloaded

Every release carries `sha256sums.txt` with a line for each artifact:

```bash
sha256sum -c sha256sums.txt --ignore-missing          # Linux
```

```powershell
Get-FileHash .\mindfork-rs-v<version>-x86_64-setup.exe -Algorithm SHA256   # Windows
```

## From crates.io

Every release is also published to [crates.io]({{ config.extra.crates }}) as the
crate `mindfork` — the same tag the downloads are built from — so a machine that
already builds Rust can install it with cargo, on Windows and Linux alike:

```bash
cargo install --locked mindfork          # the binary lands in ~/.cargo/bin
```

`--locked` builds against the exact dependency versions the release was tested
with, from the lock file the crate carries; without it cargo resolves the newest
compatible ones instead. The same line run later upgrades to the newest release,
or says the one you have is current.

It compiles on your machine, so it needs a toolchain: **Rust 1.96 or newer** —
cargo refuses an older one before building anything (`rustup update` fixes that).
On Linux, add the ALSA headers, which speech playback links against, and
`pkg-config`, which finds them:

```bash
sudo apt install libasound2-dev pkg-config     # Debian, Ubuntu
sudo dnf install alsa-lib-devel pkgconf        # Fedora, RHEL
```

Two things differ from the downloads above, because `cargo install` keeps only
the executable:

- **No spellcheck dictionaries.** The archives, packages and installer carry them;
  a cargo install starts with spellcheck off. Hunspell `.aff`/`.dic` pairs dropped
  into the data folder's `dictionaries/` turn it on.
- **The data folder sits next to the binary** — `~/.cargo/bin/data/`, since the
  default layout is portable. To keep it in your user folder instead
  (`~/.local/share/mindfork-rs` on Linux, `%APPDATA%\mindfork-rs\data` on Windows),
  put a `defaults.json` beside the binary before the first run:

```bash
echo '{ "mode": "system" }' > ~/.cargo/bin/defaults.json      # Linux
```

```powershell
# Windows. Not `>`: Windows PowerShell writes UTF-16 with it, which the app refuses.
$file = "$env:USERPROFILE\.cargo\bin\defaults.json"
Set-Content $file '{ "mode": "system" }' -Encoding ascii
```

The downloads also carry every licence text and stay the recommended way in; this
route is for a machine that has Rust on it anyway.

## Build it yourself

The same toolchain as for crates.io — Rust 1.96 or newer, plus the ALSA headers
and `pkg-config` on Linux — and a clone:

```bash
git clone https://github.com/vshylov/mindfork-rs
cd mindfork-rs
cargo build --release          # target/release/mindfork
```

## First run

```
mindfork demo
```

opens the app on sample conversations against a scripted engine — no model, no
API key, nothing written outside a temporary folder. It is the fastest way to see
what the thing is before deciding whether to feed it a model.

For the real thing you need an engine, and there are three routes: a **cloud
provider** (a key and a model, both entered in the settings screen), an
**OpenAI-compatible server** you already run, or a **managed `llama-server`**
that the app downloads and supervises for you —
`mindfork llama setup --backend vulkan --set-binary` fetches a llama.cpp build
for your machine, verified against the checksum the release publishes.

- **[The manual](https://github.com/vshylov/mindfork-rs/blob/main/docs/manual.md)** —
  how to use the app: the screens, what it remembers, the tools, the keys.
- **[Setup in detail](https://github.com/vshylov/mindfork-rs/blob/main/docs/install.md)** —
  every engine setting, where the data lives, the Python sandbox, MCP servers,
  speech, backups.

mindfork is a terminal application: it needs a **real terminal**. Started with
its output redirected or with no console at all, it says so and exits rather than
pretending to run.

## What it does with your machine

Nothing you did not ask for. No telemetry, no update check, no account: the app
makes network requests to the engine you configured and to nothing else until you
switch a tool on yourself. The full account is in the
[privacy policy](/privacy/).
