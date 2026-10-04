# macOS — mindfork on a Mac, built and tested without one

Status: **research, MVP probe GO (2026-10-04)** — the forks in §8 wait for the
owner. Part of the promotion plan's stage 1, the friction of a first try
([promotion.md §5](promotion.md)): today a Mac user has no download at all.

The owner has no Mac. Everything below was measured on GitHub's Apple Silicon
runners (§3) or read from the code (§4) and from Apple's, GitHub's and
Homebrew's own documents (§5–§6); what only a real Mac with a real screen can
answer is listed as such (§6.2) and is not guessed.

## 1. Scope

- **Macs only.** For a terminal application Apple's devices are Macs: an iPhone
  or an iPad runs no native terminal binaries.
- **Apple Silicon only — `aarch64-apple-darwin`.** Intel Macs are left out, and
  not only because Apple named macOS 26 the last release for them (WWDC 2025):
  Wasmer dropped its `darwin-amd64` build in 7.2.0, the version the sandbox pins,
  so an Intel Mac would have no Python sandbox; llama.cpp's `macos-x64` build has
  Metal off; Homebrew moved Intel to its Tier 3 in September 2026. Intel is
  10.2 % of the Macs in Steam's September 2026 survey. Fork F1.
- **The terminals that matter**: Terminal.app (it ships with macOS, so most
  first tries start there), iTerm2 and Ghostty — by Homebrew's cask installs over
  a year, Ghostty 369 507 and iTerm2 310 729, then Warp 141 121, kitty 53 100,
  WezTerm 43 947; Terminal.app is not counted, being built in.

## 2. What the code already knows about macOS

More than expected, because most platform code is gated `#[cfg(windows)]` /
`#[cfg(unix)]` and macOS falls into the unix half — **nothing failed to
compile** (§3):

- `llama setup` maps the OS token `macos` and the arch `arm64`
  (`features/llama_setup.rs`), and found, downloaded and unpacked the macOS
  build on the runner;
- the sandbox's lock list has a `wasmer-darwin-arm64` row with its digest
  (`features/sandbox_setup.rs`);
- opening a file or a folder maps macOS to `open` (`shared/os_open.rs`);
- the `system` data mode resolves through `directories::ProjectDirs`, so on a
  Mac it is `~/Library/Application Support/mindfork-rs` — nothing hard-coded;
- audio is rodio → cpal → CoreAudio, with no system package to install (ALSA
  is a Linux-only dependency of cpal), and output needs no permission;
- the clipboard is arboard's NSPasteboard backend, which a plain command-line
  process may use;
- process trees are ended with `killpg` on every unix, macOS included.

## 3. The MVP probe — measured

A workflow on the `spike/macos-probe` branch (never merged; `macos-probe.yml`
there), one job per runner image, every step after the build allowed to fail
so one run reports everything.

### 3.1 Round 1 — `macos-15` (run 37228006192)

The machine: macOS 15.7.9, `Apple M1 (Virtual)`, 3 cores, 7 GB.

| What | Result |
|---|---|
| `cargo build --release --locked` | **builds**, 10 min 20 s cold (fat LTO, one codegen unit, three cores) |
| The binary | 22.9 MB, `Mach-O 64-bit executable arm64`; links only system frameworks (CoreFoundation, AppKit, CoreGraphics, AudioToolbox, CoreAudio, Foundation, Security) and `libSystem`, `libobjc`, `libiconv`; signature `adhoc,linker-signed` — the linker signs it, as Apple Silicon requires |
| `cargo test` | 4 min 53 s to build, 50 s to run: **3878 passed, 14 failed** (§3.3) |
| `mindfork demo` in a real pty (tmux 3.7c) | **renders** — the hero chat, the flowchart, the tool card, the footer; `Esc` opens the chat list, `Ctrl+Q` quits. The input box names **`Alt+Enter`** for a line break: tmux does not answer the kitty keyboard protocol |
| `mindfork llama setup --backend cpu --set-binary` | **works** — `llama-b11396-bin-macos-arm64.tar.gz`, 11 MB; the build reports `MTL0: Apple Paravirtual device (4778 MiB)` and `BLAS: Accelerate`. Our listing names it **`cpu`** although it is the Metal build (§4.6) |
| `llama-server -ngl 99` on a 19 MB model | **Metal works on the runner**: the layers go to `MTL0` and it generates; flash attention falls back to the CPU (the paravirtual GPU lacks it). llama.cpp's own CI reads the same device as `MTLGPUFamilyApple5`, without simdgroup reduction or bfloat |
| `mindfork setup --model … --verify` | **works** — `chat server: ready in 1 s — context 128, text only, slots 4` (the model's own context is 128) |

### 3.2 Round 2 — `macos-15` and `macos-26` (run 37229252389)

macOS 26.6.2 beside 15.7.9, the same `Apple M1 (Virtual)`. Round 1 again on
both — the same results, a release build in 7–8 minutes — and the defects the
code audit predicted (§4), each measured:

| What | Result, both images |
|---|---|
| `cargo test` | **3879 passed, 13 failed** — the 12 of §3.3 that lack a secret scheme, and the OS-assuming test. The paused-clock test passed on both: 1 failure in 3 runs |
| `mindfork demo` with no terminal | exit 2 and the sentence saying why, as on Linux |
| `M-b M-f` in the input box — what Terminal.app sends for Option+←/→ | **types `bf`** into the message (§4.4) |
| `M-Left M-Left`, then `X` | the cursor moved **one character** each time: `Alt+←` is a plain `←` |
| `mindfork stats` through a symlink in `/tmp/mfbin` | **`Data root: /tmp/mfbin/data`**, where the direct run says `…/target/release/data` (§4.3) |
| `mindfork sandbox setup --enable-python` | **works** — Wasmer 7.2.0's `darwin-arm64` build, the packed image (269 MB) **starts** under V8; 56 s on 15, 62 s on 26 |
| the single-instance lock | a file **`mindfork-rs-single-instance` in the checkout** after `sandbox setup`; the app started in `/tmp/a` and `/tmp/b` at once **ran twice**, one lock file in each (the second reported port 8000 taken by the first's server); started from `/`: `Error: failed to initialize the single-instance lock: file open or create error`, exit 1 (§4.2) |

### 3.3 The failing tests

| Tests | Why |
|---|---|
| 11 settings and orchestrator tests, and the screenshot drift gate (`settings-tools-*`: its Tavily key row reads "unavailable on this system" where the committed still says "not set") | **no stored secrets on macOS** (§4.1): the tests need the machine-key scheme, and some already return early without it |
| `the_backends_listing_marks_what_is_already_installed` | the test lists the B10883 fixture for `std::env::consts::OS` with arch `x86_64` — on a Mac that is `macos`, whose only build is `cpu`, and its `vulkan` line is not there. The test's assumption, not a defect |
| `monitor_stays_quiet_while_the_server_is_steady` (round 1 only) | a paused tokio clock against a real TCP stub: on a slow VM the 15 s readiness timeout auto-advanced before the socket answered. A race in the test, not macOS — it passed on both images in round 2 |

## 4. What is missing — the code

Read from the code by an audit and confirmed by hand where marked.

### 4.1 Stored secrets — the largest gap

`shared/secrets.rs` has two schemes: `dpapi` on Windows and `machine-key-v1`
on Linux, an HKDF of `/etc/machine-id`. A Mac has no machine-id, so no scheme is
available: **no API key, backup password or MCP secret can be stored** — the
settings rows say "unavailable on this system" and point at the environment
variables, which still work. A cloud provider is therefore usable only from the
environment. (Confirmed by the probe: §3.3.)

A Mac's stable identity is its **`IOPlatformUUID`**, which `gethostuuid(3)`
returns without spawning `ioreg`. ADR 0008 rejected the OS keychain because the
secret would not be in the file; on a Mac there is a second reason — an
ad-hoc-signed binary is a new application to the Keychain after every update, so
"Always Allow" does not carry over and every upgrade asks again. Fork F2.

`machine_label()` reads `HOSTNAME` or `/etc/hostname`, neither of which a Mac
has (zsh does not export it), so the label would read `?` — cosmetic.

### 4.2 The single-instance lock lands in the working directory

Confirmed in the crate's source: `single-instance` 0.3.3 is a named mutex on
Windows and an abstract socket on Linux, but on macOS it **treats the name as a
file path** — `File::create("mindfork-rs-single-instance")` and `flock` — so the
lock is a file in whatever directory the app was started from:

- it litters that directory;
- two instances started from two directories **both get the lock** and run on
  one data root — the race the lock exists to prevent (`data.db`, the JSON
  files);
- started from a directory that is not writable (`/`), the lock fails and the
  start with it.

All three measured (§3.2). The fix is a path of our own — the per-user
temporary directory, which on a Mac is `/var/folders/…/T/`, per user like the
other two — or our own `flock`.

### 4.3 A symlinked binary moves the portable data root

Confirmed in std's source: on Linux `current_exe()` reads `/proc/self/exe`,
which is the symlink's target; on Apple it is `_NSGetExecutablePath`, which is
the path the binary was started by. `Paths::resolve` (`shared/paths.rs`) takes
the data root, `defaults.json` and the bundled dictionaries from that directory,
so through `/usr/local/bin/mindfork` — what `install.sh` makes on Linux — or a
Homebrew `bin/` link, they are looked for **beside the link** (measured, §3.2).
The fix is one `canonicalize`.

### 4.4 The keyboard

- **Word movement.** The input box moves by words on `Ctrl+←/→` only. macOS
  binds those to switching Spaces by default (Mission Control), so they never
  reach the terminal; a Mac's word movement is **Option+←/→**.
- **Option+arrows type letters.** Terminal.app sends Option+←/→ as `ESC b` /
  `ESC f`, which crossterm reports as `Alt+'b'` / `Alt+'f'` — and the input box
  types any character without `Ctrl`, so a `b` or an `f` lands in the message —
  measured (§3.2), and true on Linux for the same keys. `Alt+←/→`, which
  iTerm2 and others send, move one character.
- **A line break in Terminal.app.** The app names `Shift+Enter` where the
  terminal reports a modified `Enter` (the kitty keyboard protocol) and
  `Alt+Enter` elsewhere (`shared/keys.rs`, after Konsole). Terminal.app speaks
  no kitty protocol, and Option is not Meta there by default, so `Alt+Enter`
  sends a bare CR. Whether its `Shift+Enter` is distinguishable is **disputed**
  — Claude Code's documentation says it works without setup, two 2026 reports
  say it sends the same CR. `Ctrl+J` (a line feed, which crossterm reports as
  `Ctrl+'j'` in raw mode) reaches the app from every terminal, and nothing
  binds it. Ghostty sends `CSI 27;2;13~` for `Shift+Enter` even without the
  protocol — whether crossterm parses it is a question for the Mac (§6.2).
- **Home/End and Page Up/Down** scroll Terminal.app's own buffer by default
  rather than reaching the application — to be confirmed (§6.2); the chat list
  and the diff view use them.
- **`F1`–`F7`** need `fn` on a Mac keyboard, where the top row is brightness,
  volume and the like. `F1` is the help everywhere; the footer says `F1`.

Fork F5.

### 4.5 Colour

The themes are 24-bit RGB with no colour-depth detection (`shared/theme.rs`;
the colour passes are `full` and `mono`). **Terminal.app has 24-bit colour only
since macOS 26**; before it, how it draws an RGB sequence is to be seen (§6.2) —
the likely remedy is a third pass that maps each RGB colour to the nearest of
the 256, taken where `TERM_PROGRAM=Apple_Terminal` on a macOS before 26.
iTerm2, Ghostty, WezTerm and kitty have had 24-bit colour for years.

### 4.6 Smaller things

- **`.DS_Store` would be adopted as a chat file.** `/file folder` opens the
  chat's folder in Finder, which writes `.DS_Store` there; at the next start
  `chat_files::unlisted` takes any unlisted regular file as `Recovered`
  (confirmed). Dot-files should be skipped — on every OS.
- **The llama.cpp build is named `cpu`.** Upstream's macOS asset has no
  backend in its name, and `backend_of` names a nameless build `cpu`; on a Mac
  that is the Metal build. Naming it `metal` there is a one-line rule, and the
  `--backend` value users type must accept both.
- **A dyld failure is not recognised.** `missing_library` reads glibc's "error
  while loading shared libraries"; dyld says "Library not loaded: @rpath/…".
- **Licence notices.** `about.toml` lists the Windows and Linux targets only, so
  macOS-only crates (objc2, coreaudio, security-framework) would be missing from
  `THIRD-PARTY-NOTICES.md` in a macOS archive.
- **The local Python default** is `python3`; on a Mac without the Command Line
  Tools `/usr/bin/python3` is a shim that opens an install dialog. The sandbox
  is the default and does not need it.
- **Texts.** The MCP import help names the Windows and Linux paths of
  `claude_desktop_config.json`; on a Mac it is `~/Library/Application
  Support/Claude`.
- **Case-insensitive APFS.** `/rag remove` lowercases paths only on Windows, so
  a differently cased path for a file already gone matches nothing on a Mac.

### 4.7 The release

`release.yml` builds `ubuntu-22.04` and `windows-latest` only; `install.sh`
refuses anything but Linux x86_64 with glibc (`uname -s`, `getconf
GNU_LIBC_VERSION`) and relies on `sha256sum` (a Mac has `shasum -a 256`),
`ldconfig` and apt/dnf/pacman. A macOS archive, an `install.sh` branch and the
notices' target are stage 2 (§7).

## 5. Distribution — what a Mac lets in

### 5.1 Gatekeeper

- **Quarantine is set by the downloading application**, not by the file:
  browsers set `com.apple.quarantine`; `curl`, `scp` and a program writing a
  file through an HTTP library do not (Apple DTS; checked on macOS 26 by
  Eclectic Light). So **`curl … | sh`, a Homebrew formula, `cargo install` and
  the builds `llama setup` downloads are not quarantined**.
- **Extraction propagates it**: Archive Utility marks everything it unpacks,
  and Apple's `tar` applies an archive's quarantine to each file it extracts
  from it (`tar -xf file`; `curl … | tar -x` stays clean).
- **A quarantined unsigned binary** is refused. Since Sequoia the Control-click
  override is gone; the way past is *System Settings → Privacy & Security →
  Open Anyway*, or `xattr -d com.apple.quarantine <file>`.
- **Every arm64 binary must be signed**, and an ad-hoc signature suffices; the
  linker makes one (measured, §3.1).

### 5.2 Signing and notarization

The Apple Developer Program, $99 a year; a **Developer ID Application**
certificate; the hardened runtime and a secure timestamp; submission to the
notary service as a zip. **A ticket cannot be stapled to a bare binary** —
only to an app, a disk image or a package — so Gatekeeper checks a bare
binary's ticket online. `rcodesign` (apple-codesign) signs and notarizes from
Linux or Windows CI with an App Store Connect API key; its last release is
0.29.0 (2024-11-29), the repository is maintained but not releasing. Fork F4.

### 5.3 Homebrew

- **homebrew-core** wants 225 stars, 90 forks or 90 watchers for a project its
  owner submits (75/30/30 otherwise), and builds from source on Homebrew's CI.
  Out of reach today; the same "notability" as Scoop's Extras.
- **A tap of our own** (`vshylov/homebrew-tap` → `brew install
  vshylov/tap/mindfork`) may install a prebuilt binary, and a formula's
  download is not quarantined. Since Homebrew 6 a third-party tap needs trust,
  which `brew install user/tap/formula` grants for that formula. Homebrew 7
  sandboxes formula operations and deprecates a third-party `post_install`, so
  `defaults.json` has to be written by `install` itself — as Scoop's manifest
  does it.
- **Casks are no route**: homebrew/cask disabled every cask that fails
  Gatekeeper on 2026-09-01, and a third-party cask is quarantined.

### 5.4 The channels

| Channel | Quarantine | Data | Updates |
|---|---|---|---|
| `curl -fsSL …/install.sh \| sh` | none | portable, `~/mindfork` | re-run |
| Homebrew tap formula | none | `~/Library/Application Support/mindfork-rs` (`defaults.json`, mode `system`) | `brew upgrade` |
| `cargo install --locked mindfork` | none | beside the binary, or `system` via `defaults.json` | re-run |
| the archive from the releases page, in a browser | **yes** | portable | by hand |

`cargo install` already builds on a Mac (§3.1 is the same source) — it is the
one route that exists today, with every gap in §4.

## 6. Testing without a Mac

### 6.1 Three layers

- **CI** — the standard runners are free and unlimited for a public
  repository: the build, the unit tests, the drift gates, `llama setup` and a
  managed server with Metal (§3). At most five macOS jobs run at once on the
  Free plan. `macos-latest` has been macOS 26 since July 2026; `macos-15` and
  `macos-26` are both M1 VMs (3 cores, 7 GB). Fork F3.
- **A rented Mac for a day** — Apple's licence leases a cloud Mac for 24 hours
  at least:

  | Provider | Machine | Price | A day |
  |---|---|---|---|
  | Scaleway | M4, 16 GB (`M4-S`) | €0.22/h | ~€5.3 |
  | Scaleway | M1, 8 GB (`M1-M`) | €0.11/h | ~€2.6 |
  | AWS EC2 | `mac2.metal`, M1 (us-east-1) | $0.65/h | ~$15.6 |
  | AWS EC2 | `mac2-m2.metal`, M2 | $0.878/h | ~$21.1 |
  | MacinCloud | pay as you go, no admin rights | $1/h | 25 h prepaid |
  | MacStadium | monthly only | from $109/month | — |

  Scaleway gives a VNC session from its console and runs macOS 15 by default —
  the Terminal.app without 24-bit colour, which is itself worth testing; macOS
  26 is the other half. On EC2 the GUI is Screen Sharing over an SSH tunnel.
  Creating the account and the machine is the owner's to do. Fork F6.
- **Testers** — the first macOS release marked a *preview*, with Mac users
  asked to report (Discussions; r/LocalLLaMA when the post goes out).

### 6.2 What only a Mac with a screen answers

A checklist for the rented day, each item in Terminal.app (macOS 15 and 26),
iTerm2 and Ghostty:

1. what `Shift+Enter`, `Option+Enter` and `Ctrl+J` deliver (§4.4);
2. Option+←/→, Option+Backspace; whether `Ctrl+←/→` reach the app with the
   Spaces shortcuts off;
3. Home/End, Page Up/Down, `fn`+F1;
4. how Terminal.app on macOS 15 draws the RGB theme (§4.5), and the colours on
   26;
5. the mouse: selection, the wheel, a click on a link;
6. the clipboard both ways, and an image pasted;
7. speech playback;
8. a real model with Metal on a real GPU (`llama setup` + `setup --verify`
   with a small Gemma), and the sandbox running Python;
9. the release archive downloaded in Safari — the quarantine path, end to end.

## 7. Stages

- **Stage 0 — this document and its probe.** GO: it builds on macOS 15 and
  26, 3879 of 3892 tests pass, the TUI runs in a pty, and llama.cpp with Metal,
  the managed engine and the Python sandbox work on a runner.
- **Stage 1 — the code, with no Mac needed.** The secret scheme (F2), the lock
  (§4.2), the symlink (§4.3), the keys (F5), `.DS_Store`, the `metal` name, the
  dyld message, the two tests (§3.3); macOS joins the CI test matrix (F3), so
  every pull request from then on is tested there.
- **Stage 2 — the release.** `aarch64-macos` in `release.yml` (the archive, its
  checksum and attestation, the notices' target), `install.sh` on macOS, the
  tap (F4's channels), install.md and the site's install page marking macOS a
  preview.
- **Stage 3 — a day on a rented Mac** (§6.2), the fixes it finds, and the
  call for testers.
- **Later, on demand** — signing and notarization (F4), Intel (F1), a 256-colour
  pass if macOS 15's Terminal.app needs one (§4.5).

## 8. Forks

- **F1 — which Macs.** (a) Apple Silicon only; (b) Intel too, as a second
  target. (b) would ship without the sandbox (no Wasmer build) and without
  Metal, to 10 % of Macs on an OS whose last release is out. **Recommended:
  (a).**
- **F2 — secrets on a Mac.** (a) a scheme of the same kind as Linux's: HKDF of
  the `IOPlatformUUID` (via `gethostuuid`) with the user's name, ChaCha20-
  Poly1305, under a scheme name of its own; (b) the Keychain. (b) is what ADR
  0008 rejected — the secret leaves the file — and an ad-hoc-signed binary
  re-asks for access after every update. **Recommended: (a)** — no new
  dependency, no prompt, and ADR 0008's threat model unchanged.
- **F3 — macOS in CI.** (a) in the pull-request test matrix beside Linux and
  Windows, the cache saved from `main` as Windows' is; (b) only on `main`, or
  nightly. The minutes are free; the cost is a third job — about 6 minutes
  cold (§3.1), less on a warm cache — running beside Windows' longer one. **Recommended: (a)** — a gap that only a
  merge finds is what stage 1 exists to close.
- **F4 — signing and notarization.** (a) not now: ship to the channels that do
  not quarantine (§5.4), and say how to clear the quarantine for a browser
  download; (b) the Developer Program and notarization from CI before the first
  macOS release. **Recommended: (a)**, revisited when testers report the
  browser path as the one they take — the same reputation-first order as
  Windows signing ([code-signing.md](code-signing.md)).
- **F5 — the keys on a Mac.** (a) chords that work in every terminal, on every
  OS: Option+←/→ (`Alt+←/→`) and `Alt+b`/`Alt+f` move by words, an
  Alt-character is never typed, and `Ctrl+J` breaks a line — named in the
  footer where neither `Shift+Enter` nor `Alt+Enter` is deliverable
  (`TERM_PROGRAM=Apple_Terminal` without the kitty protocol); (b) document the
  terminal settings (Option as Meta, the Spaces shortcuts) and change nothing.
  **Recommended: (a)**, with the settings in install.md as well — Option as Meta
  also gives the rest of the `Alt` chords.
- **F6 — the rented day.** (a) Scaleway, an M4 for ~€5; (b) AWS, where the
  site's account already is, an M1 for ~$16; (c) skip it and rely on testers.
  **Recommended: (a)** — the cheapest, the newest GPU, a console VNC — after
  stage 2, so the day tests the archive a user would download.

## 9. Risks

- **The VM is not a Mac's GPU.** The runner's Metal is a paravirtual device
  without flash attention or bfloat; Metal results there are a smoke, not a
  measure. The rented day covers it.
- **Terminal.app's answers are disputed** (§4.4) — nothing about the keys is
  decided until the rented day.
- **Unsigned builds** may cost the browser-download audience (F4).
- **The paused-clock test** may fail on any slow runner, not only a Mac
  (§3.3).
