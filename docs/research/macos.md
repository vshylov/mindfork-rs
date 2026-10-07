# macOS — mindfork on a Mac, built and tested without one

Status: **MVP probe GO; every fork as recommended (the owner, 2026-10-04);
stages 1 and 2 built (§10–§12), shipped in 0.15.0; stage 3, a day on a
rented Mac, done on 2026-10-07 (§13), measured in §14**. Part of the promotion plan's stage 1, the
friction of a first try ([promotion.md §5](promotion.md)): until 0.15.0 a Mac
user had no download at all.

The owner has no Mac. Everything below was measured on GitHub's Apple Silicon
runners (§3) or read from the code (§4) and from Apple's, GitHub's and
Homebrew's own documents (§5–§6); what only a real Mac with a real screen can
answer is listed as such (§6.2) and is not guessed; a rented Mac answered it
on 2026-10-07 (§14).

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
since macOS 26**; before it, `full` is unreadable (measured, §14.4). The
remedy is a third pass that maps each RGB colour to the nearest of the 256,
taken where `TERM_PROGRAM=Apple_Terminal` comes without `COLORTERM=truecolor`.
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

The day's plan, with what is prepared before it and what runs headless: §13.

## 7. Stages

- **Stage 0 — this document and its probe.** GO: it builds on macOS 15 and
  26, 3879 of 3892 tests pass, the TUI runs in a pty, and llama.cpp with Metal,
  the managed engine and the Python sandbox work on a runner.
- **Stage 1 — the code, with no Mac needed**, in two pull requests:
  - **1a** — what a Mac needs to run right: the secret scheme (F2), the lock
    (§4.2), the symlink (§4.3), `.DS_Store`, the `metal` name, the dyld message,
    the two tests (§3.3); macOS joins the CI test matrix (F3), so every pull
    request from then on is tested there — 1b included. Built: §10.
  - **1b** — the keys (F5), a change to how every OS reads them. Built: §11.
- **Stage 2 — the release.** `aarch64-macos` in `release.yml` (the archive, its
  checksum and attestation, the notices' target), `install.sh` on macOS, the
  tap (F4's channels), install.md and the site's install page marking macOS a
  preview. Built: §12 — the user-facing documents wait for the release that
  carries the archive.
- **Stage 3 — a day on a rented Mac** (§6.2), the fixes it finds, and the
  call for testers.
- **Later, on demand** — signing and notarization (F4), Intel (F1), a 256-colour
  pass if macOS 15's Terminal.app needs one (§4.5).

## 8. Forks

**The owner's decision, 2026-10-04: every fork as recommended** — (a) for each.

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
  **Changed to (b) on 2026-10-06**: Scaleway refused the owner's card (§13.1).

## 9. Risks

- **The VM is not a Mac's GPU.** The runner's Metal is a paravirtual device
  without flash attention or bfloat; Metal results there are a smoke, not a
  measure. The rented day covers it.
- **Terminal.app's answers are disputed** (§4.4) — nothing about the keys is
  decided until the rented day.
- **Unsigned builds** may cost the browser-download audience (F4).
- **The paused-clock test** may fail on any slow runner, not only a Mac
  (§3.3).

## 10. Stage 1a — built

Each item of §4 that needs no Mac to fix, in the order of §7:

- **Stored secrets — `platform-uuid-v1`** (`shared/secrets.rs`, F2). The Linux
  scheme's HKDF and ChaCha20-Poly1305 keyed by the hardware UUID, which
  `gethostuuid(2)` returns (in `libc` already — no new dependency). It is keyed
  in the form `ioreg` prints as `IOPlatformUUID`, upper-case and hyphenated, so
  a later reader through IOKit derives the same key. Each scheme answers for its
  own identity only: an entry under another OS's scheme is a foreign machine's,
  as before. `machine_label` falls back to `gethostname(3)`, which a Mac has
  where `HOSTNAME` and `/etc/hostname` are not.
- **The lock** (`shared/instance.rs`): on macOS the crate is given a path in
  the per-user temporary directory rather than a bare name; a test asserts that
  nothing lands in the working directory.
- **The symlink** (`shared/paths.rs`): `exe_dir_of` resolves `current_exe` on
  every unix — a no-op on Linux, where it already is the target — and not on
  Windows, where `canonicalize` would return a `\\?\` path. A unix test
  starts it through a link.
- **`.DS_Store`**: `chat_files::unlisted` skips dot-files — Finder's, and its
  `._name` companions on a volume without extended attributes.
- **`metal`**: the plain build is `metal` on macOS arm64 and `cpu` everywhere
  else, the Intel Mac's build included (Metal off upstream).
- **dyld**: `missing_library` reads dyld's line. Measured on a runner first —
  b11396 with `libggml-base.0.dylib` moved away prints
  `dyld[2653]: Library not loaded: @rpath/libggml-base.0.dylib` and dies of
  SIGABRT (exit 134) — and the test holds that line.
- **The two tests**: the backends listing names its OS; the two monitor tests
  wait for their first verdict in real time and pause the clock after it.
- **CI**: `macos-latest` in the test matrix, building only on `main` as Windows
  does, plus a clippy pass on a pull request — the macOS-only code is compiled
  nowhere else.

The OS-specific halves — `gethostuuid`, the lock path — run only on the macOS
runner; the pull request's `Tests (macos-latest)` is their test.

The pull request's `Tests (macos-latest)` passed: **3898 tests, none failing**,
the lock and the secret scheme among them, and clippy clean on macOS.

## 11. Stage 1b — built

F5's chords, on every OS ([docs/journal/ui-input.md](../journal/ui-input.md)):
`Alt+←/→` and `Alt+b`/`Alt+f` move by words, `Alt+Backspace/Delete` delete one,
an `Alt`+character is never typed, and `Ctrl+J` breaks a line in every
multi-line field — named in the footer in Terminal.app. The settings a Mac user
may turn on (Option as Meta) are in the manual's keys section; install.md's macOS
section comes with stage 2. What only a Mac answers stays on §6.2's list.

## 12. Stage 2 — built

- **The release** (`release.yml`): `macos-latest` joins the build matrix with
  `MACOSX_DEPLOYMENT_TARGET=11.0` — the first macOS for Apple Silicon, rustc's
  default for the target, said once so the C code `cc` builds has the same
  floor. The release job packs `mindfork-rs-vX.Y.Z-aarch64-macos.tar.gz` in the
  Linux layout (the binary, `data/dictionaries`, the licences and documents),
  inside `sha256sums.txt` and the attestation like every asset. The macOS build
  job runs `install_test.sh` with the binary it just built — the one place a Mac
  binary and a Mac meet before a user's machine.
- **The notices**: `about.toml` lists `aarch64-apple-darwin`, so the crates only
  a Mac links (objc2, coreaudio, security-framework) carry their licences into
  every archive; `packaging.yml` now runs when `about.toml` or `about.hbs`
  changes, which it did not.
- **`install.sh` on a Mac** (`packaging/linux/install.sh` — the path stays):
  `Darwin` + Apple Silicon (asked of the machine, so a Rosetta shell gets the
  Apple Silicon build) installs `aarch64-macos`; an Intel Mac is refused with
  `cargo install` as the way. The digest is read with `sha256sum` or `shasum`;
  ALSA, glibc and OpenMP stay Linux's. The link goes into `/usr/local/bin`, or
  Homebrew's `/opt/homebrew/bin`, which Apple Silicon has instead and its user
  can write. An archive a browser saved, installed with `--from`, carries its
  quarantine onto what `tar` unpacks (measured, below); the script says so and
  names `xattr -dr com.apple.quarantine`, and does not clear it itself — that
  is the question macOS means to ask a person.
- **Its scenarios on a Mac**: `install_test.sh` packs with bsdtar's flags and
  checks with `shasum` on a Mac; new arms stub `uname`/`sysctl`, so the
  platform choice (another OS, an Intel Mac, a Rosetta shell) is tested on both
  runners; a Mac adds the quarantine note and its control arm, and the OpenMP
  arms are Linux's. `packaging.yml` runs them on `macos-latest`; on Linux all 41
  passed in an `ubuntu:24.04` container before any of it was pushed.
- **The Homebrew tap** — its own repository, `vshylov/homebrew-tap`
  (fork F4's channel): `formula.rb.in` installs the archive into `libexec`,
  writes `defaults.json` with `{"mode": "system"}` beside the binary — the data
  in `~/Library/Application Support/mindfork-rs`, out of the Cellar an upgrade
  replaces — and links `bin/mindfork`, which the app follows back (§10's
  symlink fix is what makes that work). `bump.py` and a workflow every six hours
  keep it at the latest release with a macOS build — Scoop's Excavator again,
  with no secret: the workflow installs the formula from a tap of its own
  commit, tests it, and only then pushes.

**Measured before any release had it** — a rehearsal on the spike branch:

| What | Result (`macos-latest`, macOS 26.6.2; run 37237653131) |
|---|---|
| The release build with the floor set | 9 min 03 s; `LC_BUILD_VERSION`: **`minos 11.0`**, `sdk 26.5` |
| The archive, packed as `release.yml` packs it | `mindfork-rs-v0.14.1-aarch64-macos.tar.gz`, 12.3 MB, and its `sha256sums.txt` line |
| `install_test.sh` with the real binary | **35 passed, 0 failed** — the macOS arms among them |
| `install.sh --from` that archive | installed into `~/mf`, **linked `/usr/local/bin/mindfork`**; through the link the data root is **`~/mf/data`** — beside the real file |
| The formula, rendered by `bump.py` with a `file://` URL, from a tap of a local repository | `brew tap` + `brew install vshylov/tap/mindfork` + `brew test` pass; `brew audit --strict` reports nothing; `/opt/homebrew/bin/mindfork` → the Cellar; `mindfork --version` 0.14.1; the data root **`~/Library/Application Support/mindfork-rs`**; the dictionaries in `libexec/data/dictionaries`; after `brew uninstall` the data is still there |

**Quarantine, measured on a runner** (Gatekeeper's assessments on): `curl`
leaves none; a mark put on an archive is copied by `tar -xzf FILE` onto every
file it unpacks, and not by `… | tar -xzf -`; `spctl` rejects the ad-hoc-signed
binary, which a shell on the runner still started — whether a Terminal window on
a Mac does is §6.2's item 9.

## 13. Stage 3 — the plan for the rented day

The checklist of §6.2 as a day's work: what is prepared before it, what runs
headless, what needs a person at a screen, and what is measured rather than
looked at. Forks F7–F9 (§13.8): **every one as recommended — (a) — the
owner, 2026-10-06.**

### 13.1 The machine

**AWS EC2, `mac-m4.metal`.** F6 chose Scaleway; on 2026-10-06 Scaleway
refused the owner's card, and the day moved to the AWS account the site already
runs on. Read the same day from AWS's documentation:

- **`mac-m4.metal`**: a 2024 Mac mini, M4 (10-core GPU), **24 GiB** —
  **$1.23 an hour** in `us-east-1` (Frankfurt $1.476). A Mac is a *Dedicated
  Host* with a **24-hour minimum** before it can be released: **~$30 a day**.
  24 GiB holds Gemma 4 E4B and bge-m3 with room to spare; one server at a time
  all the same.
- **A quota first.** Every Mac host quota of a new account is 0 (measured on
  this one, read-only). *Running Dedicated mac-m4 Hosts* (`L-2CBA8B92`) = 1 in
  `us-east-1` was requested on 2026-10-06; people review it, in hours or days.
- **macOS**: AWS's AMIs carry Sequoia 15 (15.6 or later on an M4) and Tahoe 26
  — 15.8 and 26.7 in the 2026-09-23 release — and already Golden Gate 27.0.
  Preinstalled: Homebrew, the Command Line Tools, Safari, the AWS CLI;
  `ec2-user` has `sudo` without a password.
- **Another macOS on the same host** means stopping the instance: the host is
  *scrubbed* — up to **4.5 hours** on Apple silicon, unbilled — and a new one
  is launched from the other AMI.
- **Access**: SSH as `ec2-user` with the key pair named at launch (a `.pem`
  that stays the owner's). The desktop: a password set with `sudo passwd
  ec2-user`, Screen Sharing turned on with `launchctl`, and VNC through an SSH
  tunnel — `ssh -L 5900:localhost:5900 …`, then an ARD-capable VNC client on
  Windows pointed at `localhost:5900`.
- **Released by hand** once the 24 hours have passed: the instance stopped,
  then *Release host*. A forgotten host bills by the hour.

Scaleway (§6.1) would have been a sixth of the price; MacinCloud's hourly plan
has no admin rights, which the casks need.

### 13.2 What never goes on it

- **No real API key** and no personal account (no Apple ID is needed). The
  secret store is checked with a made-up value; speech with a local stub
  (§13.4).
- The desktop's password and the key pair stay the owner's.
- When the work is done the host is released (§13.1) — and with it the disk,
  which AWS scrubs.

### 13.3 Who does what

- **Headless, over SSH** — the installs, the managed engine on Metal, the live
  gate, the servers' APIs (§13.5). Run by the agent if the owner gives it a host
  alias (F9), pasted by the owner otherwise. Every step's output is kept as a
  file, so the results come back as logs rather than as recollection.
- **At the screen, over VNC** — everything a terminal decides: the keys, the
  colour, the mouse, the clipboard, a browser's download (§13.6). The owner at
  the keyboard; the agent in the session, reading the key echo's lines and
  writing the results down.

What a remote screen cannot show, known in advance:

- **The keyboard is a PC's, through VNC.** Alt has to arrive as Option — checked
  first in TextEdit, where Option+←/→ moves by words natively. The F keys
  arrive as F keys, so whether `fn`+F1 reaches the app is left to testers.
- **No sound** over VNC: playback is checked for running to its end without an
  error, not heard.
- **The VNC client's clipboard sharing off**: the clipboard is checked between
  programs on the Mac (TextEdit ↔ mindfork), not between the two machines.

### 13.4 Before the day — the agent's

1. **A key echo** (F7): what crossterm reports for a chord under the app's own
   terminal setup — bracketed paste, the kitty push where the terminal answers
   the query (`app/runtime/mod.rs`), mouse capture on a key — and what the
   input box does with it — printed, and kept in a file the agent reads over
   SSH. It turns "Shift+Enter did nothing" into the event that arrived.
   Built: §13.9.
2. **A speech stub**: a standard-library Python server that answers
   `/v1/audio/speech` with a short WAV, for the `external` speech mode — the
   playback path (rodio → CoreAudio) without a key. Built: §13.10,
   `tools/tts_stub.py`.
3. **The headless script**: §13.5's steps in order, each logging to its own
   file, stopping at none. Built: §13.10, `tools/mac_probe.py`.

The tap already serves 0.17.0 (it took the release by itself), so the day tests
what a Mac user would install that day.

### 13.5 The day, headless

These are the steps of `tools/mac_probe.py`, M1–M5; M0 is the host itself.

1. **The install routes.**
   - `curl -fsSL …/install.sh | sh` → the portable build (the probe gives it
     `--dir`), the link, `mindfork --version`, the data beside the binary.
   - `brew install vshylov/tap/mindfork` (the AMI has Homebrew) → the data in
     `~/Library/Application Support/mindfork-rs`, the dictionaries in
     `libexec`.
   - rustup (no password), then `cargo install --locked mindfork` → 0.17.0
     built from crates.io; the build time noted.
2. **The managed engine on a real GPU.** The Gemma 4 E4B GGUF (Q4_1) and
   `bge-m3-Q8_0` downloaded with `curl`; `mindfork setup --sandbox --llama metal
   --model … --embed-model … --ctx 16384 --verify` → the build named `metal`,
   `ready in N s — context 16384`. `llama-bench` from the same build: prompt
   and generation tokens per second — the first number a Mac user asks.
3. **The live gate on a Mac.** A checkout of `main`; `cargo test` on real
   hardware (its count beside the runner's), then `cargo test -- --ignored
   --test-threads=1` against the managed engine of step 2 — passed, failed and
   skipped, the table the RunPod gate keeps. The runner's GPU is paravirtual
   (§9); this is the first measure of Metal rather than a smoke.
4. **Ollama** (`brew install ollama`, `ollama serve`, `ollama pull
   gemma4:e4b`): `/api/version`, and the window `/api/ps` reports on 16 GB of
   unified memory — 4096 was measured under 24 GiB of VRAM on the 4090 box.
5. **LM Studio** (the app installed by the owner over VNC, its terms the
   owner's to accept; `lms` over SSH after): the GGUF and the **MLX** build of
   the same model. For each, `/api/v1/models` — whether an MLX instance reports
   `type` and `config.context_length` as the llama.cpp engine does — and a
   prompt over the window: a refusal, the middle cut 1.1.7 measured on Windows
   ([local-servers.md](local-servers.md) §2), or something else. Most Mac users
   of LM Studio run MLX; 0.17.0 was measured on llama.cpp only.

### 13.6 The day, at the screen

**In each terminal** — Terminal.app (macOS 26), iTerm2, Ghostty (both as casks),
and Terminal.app on macOS 15 (F8) — the same list, kept as a table of terminal
by check:

1. **A line break**: `Shift+Enter`, `Option+Enter`, `Ctrl+J`; the chord the
   footer names works; the key echo's line for each.
2. **Words**: Option+←/→, Option+Backspace/Delete; `Ctrl+←/→` with the Spaces
   shortcuts on (expected to never arrive) and off.
3. **Paging**: Home/End, Page Up/Down and their `Shift` forms in the feed and
   the chat list; F1.
4. **Colour**: `full` in a dark and a light theme, `mono`, `NO_COLOR=1`. In
   Terminal.app on macOS 15: is 24-bit colour drawn, approximated or garbled —
   the 256-colour pass of §7's *later* waits on this answer.
5. **The mouse**: native selection with capture off, the wheel after `Ctrl+W`,
   a link — Cmd+click (the terminal's own) and the app's.
6. **The clipboard**: the app → TextEdit, TextEdit → the app (bracketed paste),
   an image copied in Preview and pasted as an attachment.
7. **A Russian layout** (added to the Mac by the owner): `Ctrl`+letter
   shortcuts with it active (`shared/keys.rs` — the static table, unverified on
   a Mac), Cyrillic typed, the spellcheck's underline.

**Once, in Terminal.app on macOS 26:**

1. `mindfork demo` — the first try that needs nothing set up.
2. 0.17.0's first run with Ollama serving → the offer, `Enter`, a turn; again
   with LM Studio's MLX model loaded → the offer, its window read; `/local`.
3. The managed chat of §13.5 step 2: a turn with its thoughts, `python_exec`
   in the sandbox, a note saved and found by meaning, `/file folder` opening
   Finder, then a restart — no chat recovered from Finder's `.DS_Store`.
4. A made-up key stored for OpenAI, the app restarted, the key still there
   (`platform-uuid-v1`, §10).
5. `/tts` through the stub of §13.4 — plays to its end with no error.
6. The window shrunk below what a frame needs → the notice; `Ctrl+Q` quits.
7. A second `mindfork` refused while the first runs (the lock in `$TMPDIR`).

**The browser's road** (§6.2 item 9): Safari downloads the
`aarch64-macos.tar.gz`; unpacked with a double click (Archive Utility — does the
quarantine travel, as `tar` was measured to carry it, §12?); started in
Terminal → Gatekeeper's refusal in its own words; `xattr -dr
com.apple.quarantine` → it starts; and the other road, *System Settings →
Privacy & Security → Open Anyway*. `install.sh --from` that download says its
quarantine note.

### 13.7 After the day

- What was measured goes here, as §14, and into the journal of each area it
  touched; each defect gets its own branch and pull request.
- install.md's macOS section says what was seen instead of "not yet on a Mac
  with a screen".
- The call for testers — a Discussions post in the owner's words, after the
  owner's yes; the key echo (F7 a) is what it asks them to run.

### 13.8 Forks

- **F7 — the key echo.** (a) a diagnostic subcommand, `mindfork keys`, shipped:
  built from `main` on the day, and in the next release for testers to run and
  paste; (b) a scratch program built on the Mac that day; (c) none — behaviour
  only. **Recommended: (a)** — the same question comes back from every tester
  with a terminal not on this list, and (b) answers it once.
- **F8 — macOS 15 and 26.** (a) one M4 on 26 for the whole list, then
  relaunched on 15 for Terminal.app's half (§13.6, its first table only) — one
  host, a wait in the middle (on AWS the scrub, up to 4.5 hours, §13.1), the
  short install repeated; (b) two hosts at once — twice the price, no wait;
  (c) 26 only. **Recommended: (a)** — the 24 hours have room for the wait, and
  macOS 15 is needed for one question (colour) and a repeat of the keys.
- **F9 — the headless half.** (a) run by the agent: the owner puts the machine
  in `~/.ssh/config` under an alias with the key pair's file, which stays
  theirs, and the agent only runs `ssh <alias> …`; (b) pasted by the owner
  from the script. **Recommended: (a)** — half the list runs while the owner is
  not at the screen, and its logs land in the session as they are written.

### 13.9 F7 — built

`mindfork keys [--output FILE]` (`app/key_echo.rs`, spec §11.5) shows each key
as the app receives it, beside what the chat's input box does with it. The box
starts every key from `one two |three`.

The echo uses the app's own key handling (`runtime::enable_key_modes`,
`read_batch`, `chunk_batch`): the terminal is put into the app's key modes, and
input is read in the app's batches. So each line is exactly what the screens are
handed.

The header names:
- the OS;
- `TERM_PROGRAM` and its version, `TERM`, `COLORTERM`;
- whether the kitty protocol was answered;
- the line break the footer names.

Live on Windows, `console_probe --scenario keys` passed 18/18. On the day it is
built from `main` (§13.5 step 3). In the next release, it is what the call for
testers asks them to run.

### 13.10 The day's tools — built

Both are in `tools/` and use Python's standard library only. They target the
python3 of Apple's Command Line Tools (3.9). Their offline arms run in the
`lint` job (`--self-test`).

**`tools/tts_stub.py`** stands in for a speech server. It answers `POST
/v1/audio/speech` with a two-note tone:
- `wav` for the `external` mode;
- `pcm` at 24 kHz, as the OpenAI cloud sends it.

Any other `response_format` is refused with a 400 that names the parameter.
The tone lasts as long as the text takes to read, between 0.4 and 6 s. Each
request is printed, so the session sees what was asked even where nothing can
be heard. The app's own smoke of the mode, `external_server_synthesizes_live`,
passed against it on Windows.

**`tools/mac_probe.py`** is §13.5 as one command over SSH: `python3
mac_probe.py [DIR] [--only M2,M4]`. It writes one report and a log per
command. It is shaped like `tools/pod_probe.sh`:
- no step stops the next;
- the environment is never dumped, and the hardware UUID — the key material of
  `platform-uuid-v1` — is reported as a hash prefix only;
- everything lands under DIR, except what a step exists to measure.

Two traps of a command sent over SSH are handled before they could cost a paid
hour:
- **The bare `PATH`.** It has none of Homebrew's directories, so `brew` and
  what it installs would be "not found". The probe puts them first.
- **The SSH session's end.** It would take `ollama serve` down with it, so the
  server is started in a session of its own and is left running for the screen
  half.

M4 and M5 speak only HTTP. Run on Windows against the stands there, they gave
back what had been measured by hand:
- Ollama 0.35.1 in Docker: the window 4096.
- LM Studio 1.1.7 with Gemma 4 E4B Q4_1 at 4096:
  - the model's `format: gguf`;
  - a last message over the window refused with a 400 naming 6148 tokens of 4096;
  - a conversation over it answered after a cut: 1924 prompt tokens of about
    6256 sent.

So the classifier tells the two answers apart. On the Mac the same step asks
the same of the MLX build, which 0.17.0 was never measured on.

## 14. Stage 3 — the rented day, measured (2026-10-07)

One AWS `mac-m4.metal` (Mac mini, M4, 10-core GPU, 24 GiB), macOS **26.7**
(25G229) for the day and **15.8** (24H23) for its last hour, a 200 GiB root
volume. Every item of §13.5 and §13.6 was run. The defects are §14.7; each gets
its own branch and pull request (§13.7).

### 14.1 The machine and the access

- **Times.** The host was allocated at 18:32 UTC and SSH answered at 18:50.
  The headless run (M0–M4) took 1 h 40 min, 1 h 32 min of it the live gate.
  The screen half took the evening.
- **macOS 26 → 15 without a scrub.** `create-replace-root-volume-task` with the
  15.8 AMI swapped the root volume of the running instance in 6 minutes, and
  SSH answered 21 minutes later. The host, the address and the 200 GiB stayed;
  the host key, the password and every privacy grant went with the old volume.
  This is quicker than stopping the instance (a scrub of up to 4.5 hours, §13.1).
- **VNC from Windows, three traps:**
  - **RealVNC Viewer cannot log in.** macOS 26 offers security types 30, 33, 36
    (Apple's) and, with the legacy password on, 2. RealVNC picks 30 and
    refuses its key: *"Protocol error: key length too large"*. The key is 512
    bytes (4096 bits), measured. **TigerVNC 1.16.2** takes keys up to 1024
    bytes: `vncviewer SecurityTypes=DH localhost::PORT` logs into the user's
    own session.
  - **The legacy VNC password is the wrong road.** It logs into a new login
    window, not the user's session, and that window froze. Type 30 (DH) does
    not.
  - **Ports 5868–5967 are reserved on this Windows** (Hyper-V's excluded
    range), so `ssh -L 5900:…` fails with *Permission denied*. Any port outside
    the range works (15900).
- **No Option key through VNC.** Apple's server maps both Alt keys and the
  Windows key to Command (Win+A selected all; Alt+←/→ moved to the line's ends).
  Option is `ISO_Level3_Shift`, which a US or a Russian layout on Windows does
  not have; US-International's right Alt did not give it either. So the keys
  were sent on the Mac itself, through System Events (Accessibility for
  `sshd-keygen-wrapper`). The mouse was sent through CoreGraphics. Screenshots
  were taken through Terminal.app, which holds Screen Recording. These are the
  events the Mac's own keyboard and mouse post, minus the hardware.

### 14.2 Headless (M0–M5)

| Step | Measured |
|---|---|
| `install.sh` (the README's line) | 2 s, checksum ok, `mindfork 0.17.0`. `/usr/local/bin` is root's on AWS's image, so the link went through `sudo -n`, as designed. |
| `brew install vshylov/tap/mindfork` | 9 s; `defaults.json` `{"mode":"system"}`, 3 dictionaries beside the binary, data in `~/Library/Application Support/mindfork-rs` |
| `cargo install --locked mindfork` | 3 min 8 s → 0.17.0 |
| `setup --sandbox --llama metal … --ctx 16384 --verify` | 40 s with the sandbox. llama.cpp b11476 `metal` (11 MB), `MTL0: Apple M4 (18186 MiB)`; the chat server *ready in 5 s — context 16384, takes images, slots 4*; the embedder 5 s |
| `llama-bench`, Gemma 4 E4B Q4_1 | **pp512 402 t/s, tg128 29.7 t/s** |
| `main` (83cde2d5): `cargo build`, `cargo test` | 58 s, 56 s; **3977 passed, 0 failed, 250 ignored** |
| the live gate (`--ignored`, two llama-servers, the speech stub) | 232 passed, 18 failed, 5499 s |
| Ollama 0.40.0 (`brew`) | `gemma4:e4b` loaded in 11 s, window **4096** — the same as on the 4090: Ollama's default does not follow memory |
| LM Studio (the Mac app is **Bionic** 1.1.7+7, `ai.elementlabs.bionic`; `lms` says LM Studio) | GGUF (Q4_0 QAT) at 4096: one message over the window refused (400, 6148 of 4096); a conversation over it **cut**, 1924 of ~6256. **MLX** (4-bit) at 4096: `/api/v1/models` reports `format: mlx` and the loaded window as llama.cpp's engine does; one message over the window refused with **another text** (§14.7 D3); a conversation over it **cut**, 2038 of ~6250 |

The 18 failures, by cause. None is a macOS defect:

| Cause | Tests |
|---|---|
| The model (E4B, smaller than the gate's usual one) did not call the tool or say the answer | `fetch_url_address_policy`, `history_read_back…`, `local_mode_files_round_trip`, `rag_en`, `a_withheld_chart…`, `control_tools_are_callable` |
| The answer was right, in Russian (the default profile's language), and the test looks for an English word | `image_attachment` (*blue*), `image_url_attachment` (*green*) |
| The slow-prefill note fired on a host the tests take for fast: 295–374 t/s, a slot held 5.5–6.9 s against the 5 s limit (§14.7 D5) | `impersonation_prefill`, `loop_prefill`, `roll_prefill`, `slow_prefill`, `title_prefill` |
| No `node`/`npx` on the host | the three `mcp` tests |
| llama.cpp publishes only `metal` for macOS/arm64, and two tests expect a `cpu` build | `live_install_cpu_into_a_tempdir`, `live_the_newest_build_still_names_a_cpu_backend` |

### 14.3 The keys

`mindfork keys` from `main`. Each chord sent as the Mac's keyboard sends it;
the cell is what the app was handed.

| | Terminal.app 470.2 (26) and 455.1 (15)¹ | iTerm2 3.7.3 | Ghostty 1.3.1 |
|---|---|---|---|
| kitty protocol | not answered | answered, on | answered, on |
| `COLORTERM` | `truecolor` on 26, **none on 15** | `truecolor` | `truecolor` |
| Shift+Enter, Option+Enter | **Enter** (a send) | line break | line break |
| Ctrl+J | line break | line break | line break |
| Option+←/→ | Alt+b / Alt+f (words) | Alt+←/→ (words) | Alt+b / Alt+f (words) |
| Option+Backspace | **Backspace** | Alt+Backspace (a word) | Alt+Backspace (a word) |
| Option+Fwd Delete | nothing | Alt+Delete | Alt+Delete |
| Ctrl+←/→ | nothing (Spaces) | nothing | nothing |
| Cmd+←/→ | nothing | nothing | **Ctrl+A / Ctrl+E** (§14.7 D1) |
| Home, End, Page Up/Down | **nothing** (scroll Terminal's buffer) | arrive | arrive |
| Shift+Home/End/Page | arrive **without Shift** | Home/End with Shift; Page **nothing** | arrive with Shift |
| F1 | F1 | F1 | F1 |
| Option+X | `≈` | `≈` | Alt+X |
| Cmd+V of two lines | one paste | one paste | one paste |

¹ The Shift rows and Option+Fwd Delete were sent on 26 only; every other row
is the same on both.

The footer names the line break each terminal has: *Ctrl+J* in Terminal.app,
*Shift+Enter* in iTerm2 and Ghostty.

**The Russian layout**, switched with Ctrl+Space. Letters arrive Cyrillic in
all three terminals. A `Ctrl`+letter arrives as the Latin letter in
Terminal.app and as the Cyrillic one in iTerm2 and Ghostty. Either way the app
read every one by its physical key: Ctrl+N, P, F, U, E, Q, W, and Ctrl+J as a
line break (`shared/keys.rs`'s table, unverified on a Mac until now). Option+R
gives `®`. The spellcheck underlined the misspelt Russian words and left the
right ones alone.

### 14.4 Colour and small windows

- **Terminal.app on macOS 26** draws everything: `full` in the dark and the
  light theme (the canvas painted, applied at once), `mono`, and `NO_COLOR=1`
  starting in `mono`.
- **Terminal.app on macOS 15 (455.1) cannot draw `full`.** The canvas is not
  painted, so the dark theme's light text sits on white, barely visible, and
  the bottom row comes out bright green: an RGB sequence's numbers read as
  plain attributes. `system`, the default, is fine: the text colours over the
  terminal's own background. This terminal sets no `COLORTERM`, and Terminal
  on 26 sets `truecolor`, so the two can be told apart (§14.7 D2, §4.5).
- **Small windows.** At 24×6 the chat keeps the feed, the input and the status
  row. Terminal.app narrows no further than 20 columns, and at 20×2 the
  notice says *Window too small 20×2 — needs 20×3*; Ctrl+Q quits from it.

### 14.5 The app at the screen (Terminal.app, macOS 26)

- **`mindfork demo`** starts with nothing set up.
- **The local servers.** The Homebrew build's first run with Ollama serving
  offered it; Enter, and a turn with its thoughts. `/local` listed Ollama's and
  LM Studio's models, the MLX one included. For an overlong turn on the MLX
  model see §14.7 D3.
- **The managed chat of §14.2:**
  - a turn with its thoughts;
  - `python_exec` in the sandbox (338 350);
  - a note saved, and found by meaning in a new chat (`note_recall`);
  - `/file folder` opening Finder on the chat's folder.

  The model wrote its file into the working directory instead of the output
  folder, twice. The app then said *this chat has saved no files yet* (right),
  and the model claimed it had saved the file.
- **Finder's `.DS_Store`**, copied into `data/`, `chats/` and a chat's files
  folder. After a restart the list showed the same two chats, and the log had
  no error.
- **A made-up OpenAI key** shows as *set (this computer)*. `settings.json`
  holds it under `platform-uuid-v1`, with no plain text. It was still set
  after a restart, and Del removed it.
- **`/tts` through `tools/tts_stub.py`** (`external`): two WAV requests of 190
  and 165 characters, *♪ speaking* in the status row, ended with no error.
- **A second `mindfork`** (the portable one while the Homebrew one ran) is
  refused: *mindfork is already running on this machine*.
- **The clipboard.** A PNG put on the clipboard and pasted with Ctrl+V
  became an attachment (256×256, ~100 tokens), which the model described. A
  feed line selected natively and copied with Cmd+C came out as the line's
  text, no frame characters.
- **The mouse.** The wheel scrolls the feed after Ctrl+W. A URL in the feed
  opens with **Cmd+double-click**, which is Terminal.app's own; a single
  Cmd+click did not. The app draws no OSC 8 links of its own.

**iTerm2 after Gatekeeper's question.** iTerm2 came from `brew install
--cask`, so it carried a quarantine. After *Open* it sat stopped (state `T`)
with no window, and clicking its icon did nothing. Killed and opened again, it
ran. Ghostty, from the same cask run, opened at once.

### 14.6 The browser's road

1. **Safari unpacks the `.gz` by itself.** Its default *Open "safe" files* left
   `mindfork-rs-v0.17.0-aarch64-macos.tar`, not `.tar.gz`, in Downloads, with
   the quarantine mark.
2. **A double click in Finder** (Archive Utility) unpacks the `.tar`, and
   **all 43 files carry the mark**. Opening the archive with `open` from a
   shell did nothing visible.
3. **The first start, in Terminal, is refused:** *"“mindfork” Not Opened —
   Apple could not verify “mindfork” is free of malware that may harm your Mac
   or compromise your privacy."* The buttons are **Move to Trash** (the
   default) and Done.
4. **System Settings → Privacy & Security → Open Anyway** asks no password.
5. **The next start asks again:** *"Open “mindfork”? Apple is not able to
   verify…"*, now with **Open Anyway**. After it the binary runs (`mindfork
   0.17.0`, exit 0), and later starts ask nothing, SSH included. The mark stays,
   its flags go from `0081` to `00c1`, and `spctl` still says *rejected*.
6. **`xattr -dr com.apple.quarantine <folder>`** works at once, as install.md
   says.
7. **`install.sh --from ~/Downloads`** finds nothing: it looks for the
   `.tar.gz` Safari no longer left. Fed a quarantined `.tar.gz`, it prints its
   quarantine note and then checks the binary by starting it. That start
   raises Gatekeeper's dialog and **waits on it**. Answered with Done, the start
   is killed (`Killed: 9`) and the script says *the installed binary does not
   start* (§14.7 D4).

### 14.7 What follows

| | Defect | Weight | Next |
|---|---|---|---|
| D1 | **Ghostty sends Cmd+→ as Ctrl+E, which deletes the last exchange without a question** (the confirmation is off by default). Cmd+← is Ctrl+A, select all. Measured on the demo: one keypress, and the answer was gone. | high | an owner's fork: Ctrl+E/Ctrl+A as the line's ends, and the deletion on another key, or a confirmation on by default |
| D2 | **`full` is unreadable in Terminal.app before macOS 26** | high | the 256-colour pass of §4.5, chosen where `TERM_PROGRAM=Apple_Terminal` comes without `COLORTERM=truecolor` |
| D3 | **LM Studio's MLX refusal is not taken for an overflow.** It arrives as a 200 whose stream is an `event: error` reading *"The number of tokens to keep from the initial prompt is greater than the context length…"*, which no marker of `features/compaction.rs` matches. So the user gets the raw text, not *the conversation no longer fits… /compact*. | medium | a marker, and its unit test |
| D4 | **`install.sh --from` and a browser download:** a `.tar` is not found, and the start check waits on Gatekeeper | medium | take a `.tar`; skip the start check, or name the remedy, while the mark is on |
| D5 | **The slow-prefill note fires on an M4 with E4B**, after every turn: 295–374 t/s measured by the app (`llama-bench`: 402), so a cancelled request would hold the slot 5.5–6.9 s at the default batch, and the limit is 5 s | question | whether -b 256 is the right advice on Apple silicon — measure before changing |
| D6 | **The documents:** Terminal.app's Home/End/Page need Shift; Option+Backspace is a character there; Shift+Page does not reach the app in iTerm2; URLs open with Cmd+double-click; the browser's road takes two *Open Anyway*s | low | install.md and the manual |
| D7 | **Tests:** the two image tests look only for English words; two `llama_setup` tests expect a `cpu` build that macOS does not have | low | language-free assertions; the backend list per platform |
| — | The probe: it unlinked a root-owned link without `sudo`, and found no failed names under `--nocapture` | fixed | in this change |

The call for testers (§13.7) waits on D1 and D2: a tester in Ghostty or in
Terminal.app on macOS 15 would meet them first.
