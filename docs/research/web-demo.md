# An interactive demo on mindfork.io — research

Status: **route decided (the owner, 2026-11-10): the emulator, starting with the
probe** (§9); **stage 1 — the probe — GO** (the owner, 2026-11-10, in Chrome,
Firefox and Edge; §12.7); **stage 2 — the pipeline — in progress**: the guest
and its window-sized terminal are built (§13.4), the release job is not yet. Four routes compared; what this machine could measure
is measured (§3), what it could not is named (§10).

## 1. Context and goal

The owner's request (2026-11-09): put a demo of the program — `mindfork demo` — on
the site, so that a visitor can decide whether an interface of this kind suits them
**without installing it**.

What exists today:

- **`mindfork demo`** ([manual §1](../manual.md)): the real TUI on a throwaway data
  root with a scripted engine — a seeded conversation, no model, no key, no network
  (`run_demo` in `src/main.rs`, `DemoSupervisor` in `src/app/supervisor.rs`, the
  fixture in `src/features/demo.rs`).
- **The site** shows ten stills and the animated demo that every release draws from
  its own interface ([demo-reel.md](demo-reel.md)), and its hero prints
  `$ mindfork demo`.
- **Installing is a command or an installer** on every platform the app runs on
  ([install.md §1](../install.md)).

So a visitor can already *watch* the interface, and *try* it a minute after
installing. What nothing answers is the step between: how the interface responds to
their own hands, before they have decided to install anything.

## 2. What an answer has to satisfy

Each of these is a standing decision of the project, not a new wish:

1. **The real interface.** The frames must come from the app's own code. A page that
   imitates the interface in JavaScript is the thing the reel's director was built
   to avoid — "animating a lie" ([demo-reel.md §3.1](demo-reel.md)).
2. **Made by the release, not by hand.** The application changes every few days;
   whatever is published has to be produced from the tag by a script
   ([demo-reel.md §1](demo-reel.md)).
3. **The site stays what it is**: static files behind CloudFront for about a cent a
   month, no server, no cookies, no request to anyone else's host, pages that load
   like a terminal program starts
   ([mindfork-io-website.md §3.1, §5.6, §6](mindfork-io-website.md)).
4. **A visitor can cost and harm nothing** — neither the project nor themselves.
5. **Not a gate.** A release without a fresh demo is still a release, as with the
   reel ([demo-reel.md §3.6](demo-reel.md)).

## 3. Measured on this machine

A 4-vCPU x86_64 droplet, Ubuntu 24.04, Rust 1.96.0, `main` at 0.25.0 (`283e4229`),
2026-11-09/10. The scripts lived in a scratch directory; nothing here is committed
code.

### 3.1 What the demo costs natively

`mindfork demo` in a 116×44 pseudo-terminal (the hero's size), keys written by a
script, CPU read from the scheduler's per-thread counters
(`/proc/<pid>/task/*/schedstat`), release profile, two runs each:

| | x86_64 | i686 (§3.3) |
|---|---|---|
| Binary, stripped | 33 121 352 B | 30 535 236 B |
| … `xz -9` / `zstd -19` / `gzip -9` | 10.4 / 11.3 / 14.2 MB | 10.5 / 11.5 / 13.9 MB |
| Shared libraries it loads | libc, libm, libgcc_s, libasound | the same; 5.0 MB with the loader |
| Start → first frame, CPU | 57–60 ms | 69–72 ms |
| A typed character, CPU | 1.2–1.5 ms | 0.9–1.7 ms |
| `PageUp` / `PageDown` | 0.7–0.8 ms | 0.7–0.8 ms |
| `F1`, `F3`, `Ctrl+P` (a screen opens) | 1.2–1.9 ms | 1.3–2.2 ms |
| Idle | ≈ 1.2 ms per second | ≈ 1.3 ms per second |
| Resident memory | 29–35 MB | 25–30 MB |
| Threads | 6–7 | 6–7 |

A streamed scripted reply costs 10–20 ms of CPU in all on either build (the
process's own accounting, whose step is 10 ms; three runs each). **The demo is a
light program**: about 70 ms to start and one to two milliseconds per key. That
number decides §5.

### 3.2 WebAssembly: what refuses to compile

`cargo check --keep-going` for the two targets a browser build could use:

| Crate | `wasm32-unknown-unknown` | `wasm32-wasip1` | Why |
|---|---|---|---|
| `crossterm` 0.29 | fails | fails | it has a `sys` layer for unix and for windows, and nothing else |
| `tokio` (`rt-multi-thread`, `process`) | fails, in `mio` | fails | "Only features sync,macros,io-util,rt,time are supported on wasm" |
| `socket2` | — | fails | no sockets on the target |
| `arboard` | fails | fails | no clipboard platform |
| `single-instance` | fails | fails | no platform |
| `tracing-appender` | fails | fails | file rolling needs symlinks |
| `uuid` | fails | — | needs a randomness feature chosen for the target |
| `onig_sys` (syntect's regex engine, C) | fails | fails | no C compiler for the target here; upstream has no `wasm32-unknown-unknown` build at all |
| SQLite (`sqlite-wasm-rs` / `libsqlite3-sys`, C) | fails | fails | needs clang |
| `sqlite-vec` (C) | fails | fails | needs clang |
| `aws-lc-sys` (rustls's crypto, C and assembly) | — | fails | needs a C toolchain for the target |

**Nine crates on one target and ten on the other fail before a line of the
application is type-checked.** Behind them, the application itself — about 320 000
lines in one binary crate — names `std::fs` or `tokio::fs` in 91 files, threads or
`spawn_blocking` in 34, child processes in 15 and `Instant` in 55; on
`wasm32-unknown-unknown` files and processes always return an error, and a thread
or `Instant::now()` panics. The UI loop
blocks in `crossterm::event::poll` (`src/app/runtime/mod.rs`), which a browser's
main thread cannot do.

### 3.3 32-bit x86: it builds, links and runs

Every emulator that runs an unmodified Linux binary in a browser tab without special
headers is 32-bit (§5), so the question was whether the application is a 32-bit
program at all.

- `cargo check --target i686-unknown-linux-gnu`: **the whole crate, 0 errors,
  0 warnings.**
- Debug and release builds **link**. The machine has no multilib and the session no
  root, so the 32-bit C library, compiler runtime and ALSA came from Ubuntu's own
  packages (`libc6-dev-i386`, `lib32gcc-13-dev`, `libasound2-dev:i386`) unpacked
  into a scratch sysroot. A release build took 6 min 18 s.
- The binary **runs** — an x86_64 kernel executes 32-bit programs — through the
  32-bit loader: `--version` prints `mindfork 0.25.0`, a launch without a terminal
  exits with code 2 as designed, and `demo` draws.
- **The same frames.** Seven beats (`PageUp`, `PageDown`, `F1`, `Esc`, `F3`,
  `Ctrl+P`, `Down`) each wrote the same number of bytes to the terminal as the
  x86_64 build: 5163, 5223, 12852, 9387, 7775, 9208, 3655.
- Its cost is the right-hand column of §3.1 — the same as 64-bit.

Not done: running it inside an emulator (§10).

### 3.4 What the site's delivery allows

- CloudFront compresses on its own only objects of **1 000 to 10 000 000 bytes**
  whose `Content-Type` is on its list — `application/wasm` and JavaScript are,
  `application/octet-stream` is not — and passes through, untouched, an object the
  origin already stored compressed
  ([Serving compressed files](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/ServingCompressedFiles.html)).
  Anything larger than 10 MB, or binary, has to be compressed before upload.
- The distribution sends the managed security headers and no
  `Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy`
  (`infra/website.cfn.yaml`). A route that needs `SharedArrayBuffer` needs both,
  on every page that hosts it.

### 3.5 What a visitor can do inside the demo

The demo is the whole application with its engine replaced. Its settings screen
still has the Plugins section, where a server is a `command` with `args` that the
host launches (spec §9.6); the code workspace runs build and test commands (spec
§9.12); `/rag add`, `/image attach`, `/backup` and `/restore` take paths, and two of
them take URLs. **Whoever is given the demo is given a shell** wherever it runs. In
their own browser tab that is their own sandbox. On a server it is the server.

### 3.6 What the app already knows about a browser terminal

The application has been run inside xterm.js before — JupyterLab's terminal, the
host the container gate exists for ([install.md §7.3](../install.md)):

- a browser tab never delivers `Ctrl+N` or `Ctrl+T`, and `Ctrl+W` closes the tab —
  which is why **every key has a typed command** ([manual §10, §11](../manual.md),
  [command-only-control.md](../history/command-only-control.md));
- xterm.js answers the background query, so the app's theme follows the
  terminal's ([auto-theme-detection.md](auto-theme-detection.md));
- it drops OSC 52 unless the page registers a handler
  ([osc52-clipboard.md](../history/osc52-clipboard.md)).

Checked against the browsers' own sources: in an ordinary tab Chrome and Firefox
reserve new tab, new window, close tab, tab switching and their own quit chord;
`F1`, `F3`, `F5`, `F6`, `Ctrl+P`, `Ctrl+F`, `Ctrl+S` reach a page that cancels them.
In script-initiated fullscreen the reserved ones can be delivered too —
`navigator.keyboard.lock()` in Chromium, `requestFullscreen({ keyboardLock })` in
Firefox 151+ and Safari 26.4+ (§11).

## 4. Option A — compile the application to WebAssembly

The interface compiled for the browser and drawn there directly. The answer with the
best experience on paper: instant, a few megabytes, nothing emulated.

**What exists.** [Ratzilla](https://github.com/ratatui/ratzilla) 0.3.1 (2026-06-08,
in the ratatui organisation, on ratatui 0.30.1) draws a ratatui buffer into the DOM,
a canvas or WebGL2; published applications build both ways with it (`vortex-tui`,
`ballista-cli`). Its minimal demo is 1.8 MB of wasm, 1.2 MB gzipped, before any
application code.

**What it would take here.**

- **The eleven crates of §3.2**, each gated out or replaced: crossterm has no wasm
  build and nothing merged upstream (issue
  [#995](https://github.com/crossterm-rs/crossterm/issues/995); PR
  [#1066](https://github.com/crossterm-rs/crossterm/pull/1066), which would make its
  event types compile, is open and unreviewed) — so the key and mouse types the
  screens match on need a neutral replacement; syntect must move to its pure-Rust
  regex engine, which `Cargo.toml` records as rejecting the Swift grammar;
  SQLite becomes `sqlite-wasm-rs`, in memory.
- **A different core.** No multi-thread runtime, no `spawn_blocking`, no blocking
  read of input, timers that panic on a target that cannot block. Either the
  orchestrator gains a single-threaded variant driven by the browser's frame
  callback, or the demo ships without the orchestrator and a script plays its part —
  which is the reel's director, and stops being the application the moment a visitor
  presses a key the script did not expect.
- **Its own input layer.** Ratzilla delivers `keydown` only, has no wheel and no
  paste, cancels no browser default, and turns any non-ASCII character into
  `Unidentified` — a Cyrillic letter cannot be typed.
- **A standing gate.** Every later dependency and every use of a file, a thread or a
  clock would have to hold on a second target, checked on every pull request, or the
  demo breaks at a release.

The WASI variant (`wasm32-wasip1`, a terminal emulator in the page) keeps files and
clocks but loses nothing else from that list, has no published ratatui precedent,
and the browser runtimes with a usable terminal are alpha and need the isolation
headers of §3.4.

**Verdict.** A port, and a permanent tax on a codebase whose product is the
terminal. Weeks to build; never finished.

## 5. Option B — the real binary in an x86 emulator in the tab

A PC emulated in WebAssembly, a minimal Linux in it, the release's own binary
running there, its terminal drawn by xterm.js. Static files; the visitor's tab is
the machine.

**Candidates.**

| | Guest | Headers | Licence | Speed (published) | State |
|---|---|---|---|---|---|
| [v86](https://github.com/copy/v86) | 32-bit x86 to SSE3, one CPU | none | BSD-2-Clause | "a 10x slowdown compared to native is expected" (its author) | active — last commit 2026-10-30, 23.7k stars, used by sandbox.bio |
| [CheerpX](https://cheerpx.io/docs/licensing) 1.4 | 32-bit x86, user mode | COOP + COEP | proprietary | 5–10×, 2–3× at best (vendor) | commercial; free use only from the vendor's CDN — "If you wish to self-host CheerpX, you will need a Commercial License" |
| [container2wasm](https://github.com/container2wasm/container2wasm) / qemu-wasm | x86_64 | COOP + COEP, threads | Apache-2.0 / GPL-2.0 | slower; no native-relative figure | "experimental"; its Debian demo is about 200 MB |

CheerpX breaks requirement 3 twice (a third party's host, isolation headers), and
the one 64-bit demo measured is two hundred megabytes. **v86 is the candidate**, and it is
32-bit: 64-bit guests have been an open issue since 2017
([#133](https://github.com/copy/v86/issues/133)).

**Why the numbers allow it.** At the author's ten times, §3.1 becomes about 0.7 s
of CPU to start and about 15 ms for a key. An estimate, not a measurement — the
emulator has three layers of warm-up and software x87 — but it is well inside
what typing needs, because the demo is light. Rust's
`i686-unknown-linux-gnu` is Tier 1 with an SSE2 baseline, under v86's SSE3 ceiling.

**How it would work.**

1. A release job builds the 32-bit binary from the tag (§3.3 is the recipe; about
   six minutes) — not a gate.
2. The guest is a kernel, a static shell, and the four libraries and the loader
   of §3.1.
3. The job boots it under Node, starts `mindfork demo`, waits for the first frame
   and saves the machine's **state**. A visitor restores that state and lands in the
   running application: no boot. v86 documents the recipe
   ([`build-state.js`](https://github.com/copy/v86/blob/master/tools/docker/alpine/build-state.js))
   and decompresses `.zst` states itself; published states are 5.5–19.5 MB.
4. The engine (0.36 MB of JavaScript, 2.0 MB of wasm, BIOS 0.16 MB), xterm.js and
   the state ride as release assets, covered by the checksums and the attestation,
   and the deploy fetches them from the newest release that has them — the reel's
   mechanism, unchanged ([demo-reel.md §3.6](demo-reel.md)).
5. A page of its own. Nothing is downloaded before a **Start** button; a phone, or
   a window too small, gets the reel.

**What a visitor gets.** Every screen, every command, real streaming, the settings
as they are — and `/exit`, after which the page offers to start again. What they do
not get: a model (the engine is scripted, and the guest has no network at all),
their own terminal's fonts and colours, native speed, and the chords the browser
keeps (§3.6) unless they go fullscreen.

**What it costs to keep.** One more build target, compiled only at a release and
only for the demo. If it breaks, the site keeps the previous release's demo and
says which version it is. The state is tied to the exact v86 build and its options,
so both are pinned and made in the same job.

**Risks.**

- **Feel.** The one thing an estimate cannot settle, and the one thing the demo is
  for. A sluggish demo of a fast program is worse than none.
- **32-bit is a shrinking world.** Debian 13 has no i386 kernel or installer;
  Debian 12's LTS covers i386 until 2028-06-30; Alpine 3.24 still ships x86. No
  proposal to demote Rust's 32-bit Linux target was found, but its 32-bit Windows
  hosts are being demoted. The demo depends on none of this *today*; it could in a
  few years.
- **Copyleft binaries.** The guest redistributes a Linux kernel (GPL-2.0), glibc
  and ALSA (LGPL) and a shell from mindfork.io, which makes the site a distributor
  of them, with the source obligations that carries. A notice and the exact sources
  are the usual answer; it is the owner's call (F5).
- **Window size.** A serial console carries none. v86's virtio console has a resize
  message the page must send itself; a fixed 116×44 grid is the fallback.
- **Upstream.** v86 does not accept issues or pull requests written with generative
  AI; anything found there is reported by the owner in person or not at all.

## 6. Option C — a terminal on a server

The release binary in a pseudo-terminal on a machine of the project's, reached from
a page over a WebSocket (ttyd 1.7.7, the GoTTY fork 1.11.0) or by
`ssh demo.mindfork.io`. No change to the application, native speed plus the
network's round trip — and over SSH the one thing no browser route offers: the
visitor's **own terminal**, fonts and keys included.

It fails requirements 3 and 4, and adds a duty the project does not have today:

- **§3.5 makes every visitor a shell user.** Public playgrounds answer that with
  nsjail (Compiler Explorer), gVisor (Go Playground) or a microVM per session; a
  microVM needs KVM, which the small AWS instances do not expose. Neither ttyd nor
  GoTTY has a session time limit or a per-address limit — the wrapper and the edge
  have to supply them.
- **It is a server.** Always-on at AWS is $3.50–7.50 a month against today's one
  cent; a microVM per session elsewhere (Fly Machines, Cloudflare Containers) is
  under two dollars per thousand ten-minute sessions with a 1–3 s start, on a
  second provider. CloudFront can front the WebSocket at no charge; SSH needs an
  address of its own.
- **It is operated**: patched, watched for abuse, redeployed at every release.

**Verdict.** The best fidelity and the worst fit. If an SSH demo is ever wanted for
its own sake, it is a track of its own.

## 7. Option D — recordings, as today

What almost everyone does: of about twenty well-known terminal programs checked
(Helix, lazygit, k9s, btop, yazi, Zellij, Textual, the Charm tools, opencode, aider
and others), none hosts an interactive demo; the live ones are programs written to
serve many sessions themselves — two games and a shop, over SSH. mindfork.io is
already ahead of that: its recording is drawn from the release's own interface.

It could go one step further without leaving this option: play the reel's frames as
**text** instead of pixels — sharp at any zoom, pausable, steppable beat by beat.
It would still be watching.

**Verdict.** Costs nothing, risks nothing, and does not answer the question.

## 8. Comparison

| | A — WebAssembly | B — emulator (v86) | C — server | D — recordings |
|---|---|---|---|---|
| The real interface | its code, another build, parts replaced | **the release's binary**, 32-bit | **the release's binary** | its frames |
| Responds to the visitor | yes | yes | yes | no |
| Change to the application | a port, and a gate for good | **none** | **none** | none |
| Hosting | static | **static** | a server | static |
| Each month | ≈ $0 | **≈ $0** | $3.50–7.50, or per session | $0 |
| What a visitor can break | nothing | **nothing** | a sandbox of ours | nothing |
| Download | a few MB (not measured) | 21.5 MB on a click, measured (§12) | xterm.js | 1 MB today |
| Response to a key | native | 9–16 ms for a typed character, measured (§12) | the network's round trip | — |
| To build | weeks | a probe, then two stages | days, then operations | — |

## 9. Recommendation and forks

**Recommendation: B with v86, starting with a probe whose only question is feel;
D stays as it is and remains the fallback.** B is the only route that is the real
binary, on static hosting, with no change to the application — and §3.1 and §3.3
settled two of its unknowns: the demo is light enough that a tenfold slowdown
leaves it responsive on paper, and the application is a working 32-bit program.
What is left can only be judged in a browser, by a person.

**Stage 1 — the probe** (`spike/web-demo-v86`; nothing deployed, nothing in a
release). The 32-bit build, a guest, a state, one local page. It answers: does the
binary run on v86's CPU; click → first frame, with a state and without; key →
frame in Chromium, Firefox and Safari; the state's size; which keys arrive; whether
resize works; the tab's memory. **Go/no-go: the owner tries it in their own
browsers.** Floors proposed for that judgement: first frame within 5 s of the click
on a 50 Mbit/s line, no visible lag between a key and its character, at most 25 MB
downloaded.

**Stage 2 — the pipeline**: the release job, the assets, pinned v86 and xterm.js.
**Stage 3 — the page**: its template, the deploy's fetch, the notices, the hero's
link. Fullscreen with the keyboard lock, the light look and Russian come after.

Forks for the owner:

- **F1 — the route.** (a) **B: the v86 probe — recommended**; (b) A, the
  WebAssembly port; (c) C, a server; (d) stop at D.
- **F2 — the guest** (if F1a). (a) **a kernel, a static shell, and the libraries
  and loader from a distribution's i386 packages, unmodified and named —
  recommended** for the probe: it is how §3.3 already ran; (b) a static musl build as the only
  userland — the smallest guest and the least copyleft, but it needs ALSA built for
  musl or audio compiled out, which is a change to the application.
- **F3 — where the visitor meets it.** (a) **a page of its own behind Start —
  recommended**; (b) in the home page's hero in place of the reel.
- **F4 — the probe's floors.** The three numbers above, or the owner's.
- **F5 — redistributing GPL and LGPL binaries from mindfork.io.** (a) **accept, with
  notices and the exact sources — recommended**; (b) not acceptable, which sends F2
  to (b) or F1 to (d).

**The owner's decisions (2026-11-10):** F1(a) — the v86 probe; F4 — the proposed
floors stand; F5(a) — the licences are named on the page, with links. One
requirement added: **the page says plainly that it is a demo of the program's
interface, not a full session with a real model.** F2 and F3 were not answered
apart from that; the probe goes by their recommendations, and the page is where
both show.

**What would bring A back:** crossterm compiling for wasm upstream, the application
being split into crates for a reason of its own, or the 32-bit route closing.

## 10. Not measured

- **v86 itself was not run** when §5 was written: the session could not execute
  downloaded third-party code until the owner had chosen the route. Stage 1 ran
  it; §12 has the figures, and §5's estimates stand beside them as written.
- A wasm build's size: §3.2 never reached the application's code.
- Safari's reserved keys, from a current primary source.
- Whether GitHub's runners build `--platform linux/386` containers natively — only
  indirect evidence (WebVM's deploy workflow does it with no emulation step).

## 11. Sources

Fetched 2026-11-09/10.

- **v86**: [repository](https://github.com/copy/v86) (licence, README: 32-bit only,
  "around Pentium 4 level, including full SSE3 support"); issues
  [#133](https://github.com/copy/v86/issues/133),
  [#1005](https://github.com/copy/v86/issues/1005) (the 10× remark);
  [filesystem.md](https://github.com/copy/v86/blob/master/docs/filesystem.md);
  [`v86.d.ts`](https://github.com/copy/v86/blob/master/v86.d.ts) (states);
  [`serial.js`](https://github.com/copy/v86/blob/master/src/browser/serial.js) and
  [`virtio_console.js`](https://github.com/copy/v86/blob/master/src/virtio_console.js)
  (xterm.js adapters, resize);
  [sandbox.bio](https://github.com/sandbox-bio/sandbox.bio) (an embedder).
- **CheerpX**: [licensing](https://cheerpx.io/docs/licensing),
  [plans](https://cheerpx.io/licensing),
  [getting started](https://cheerpx.io/docs/getting-started) (the headers).
- **container2wasm**: [repository](https://github.com/container2wasm/container2wasm),
  [demo](https://ktock.github.io/container2wasm-demo/).
- **Ratzilla**: [repository](https://github.com/ratatui/ratzilla),
  [backends](https://docs.rs/ratzilla/latest/ratzilla/backend/index.html),
  [`event.rs`](https://github.com/ratatui/ratzilla/blob/main/src/event.rs).
- **Rust on wasm**:
  [tokio's WASM support](https://docs.rs/tokio/latest/tokio/#wasm-support),
  [`wasm32-unknown-unknown`](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html),
  [rusqlite 0.38.0](https://github.com/rusqlite/rusqlite/releases/tag/v0.38.0),
  [rust-onig #153](https://github.com/rust-onig/rust-onig/issues/153).
- **Rust on 32-bit x86**:
  [platform support](https://doc.rust-lang.org/rustc/platform-support.html),
  [RFC 3999](https://rust-lang.github.io/rfcs/3999-std-only-i686-msvc.html).
- **32-bit distributions**:
  [Debian 13 release notes §5.1.2](https://www.debian.org/releases/trixie/release-notes/issues.en.html),
  [Debian LTS](https://wiki.debian.org/LTS),
  [Alpine downloads](https://alpinelinux.org/downloads/).
- **Browser keys**: Chromium's
  [`accelerator_table.cc`](https://github.com/chromium/chromium/blob/main/chrome/browser/ui/accelerator_table.cc)
  and
  [`browser_command_controller.cc`](https://github.com/chromium/chromium/blob/main/chrome/browser/ui/browser_command_controller.cc);
  Firefox's
  [`browser-sets.inc.xhtml`](https://github.com/mozilla-firefox/firefox/blob/main/browser/base/content/browser-sets.inc.xhtml);
  [Keyboard Lock](https://wicg.github.io/keyboard-lock/);
  [Fullscreen](https://fullscreen.spec.whatwg.org/).
- **CloudFront**:
  [compressed files](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/ServingCompressedFiles.html),
  [WebSockets](https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/distribution-working-with.websockets.html).
- **Server-side**: [ttyd](https://github.com/tsl0922/ttyd),
  [GoTTY](https://github.com/sorenisanerd/gotty),
  [nsjail](https://github.com/google/nsjail),
  [Compiler Explorer's sandbox](https://github.com/compiler-explorer/compiler-explorer/blob/main/docs/NsjailSandbox.md),
  [Go Playground's](https://go.googlesource.com/playground/+/HEAD/sandbox/README.md),
  [EC2 nested virtualization](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/amazon-ec2-nested-virtualization.html);
  prices: [EC2](https://aws.amazon.com/ec2/pricing/on-demand/),
  [Lightsail](https://aws.amazon.com/lightsail/pricing/),
  [Fly.io](https://docs.fly.io/about/pricing),
  [Cloudflare Containers](https://developers.cloudflare.com/containers/pricing/).

## 12. Stage 1 — the probe: what was built and measured

Branch `spike/web-demo-v86`, 2026-11-10. Nothing is deployed and nothing is in a
release; what the tools download and build lives under `target/web-demo/`.

### 12.1 What was built

[`tools/web_demo/`](../../tools/web_demo/README.md):

- **`build_i686.sh`** — the 32-bit release binary, against 32-bit libraries
  unpacked from Ubuntu's own packages with no root (§3.3's recipe as a script;
  run from nothing, it gave the same 30 535 236 bytes in six minutes).
- **`fetch.py`** — the third-party pieces, each pinned by a hash and checked on
  every run: v86 0.5.473 and xterm.js 6.0.0 from their npm tarballs (the
  registry's own integrity values), SeaBIOS 1.16.2's two images from v86's
  repository at a named commit, and the guest's kernel image. v86 comes from its
  versioned package, not from the repository's rolling release: a state restores
  only on the build that saved it, and the two were byte-identical on the day.
- **The guest** is the Buildroot image v86's own site boots — Linux 6.8.12,
  BusyBox, uClibc — with the binary, its four libraries and loader, and a
  certificate bundle copied into a RAM disk. A probe's shortcut: F2's guest from
  named packages is stage 2's.
- **`make_state.mjs`** boots it under Node, starts `mindfork demo` on the serial
  terminal at 116×44 and saves the machine once the first frame is drawn;
  **`verify_state.mjs`** restores that state in a fresh emulator and types into
  it. `machine.json` holds the machine's options for both and for the page.
- **The page** (`page/`): the notice the owner asked for — a demo of the
  interface, not a session with a real model — then **Start**, the terminal,
  the keys the browser keeps and their typed commands, what is running, and the
  licences with links. Nothing but the page and its typeface (about 0.2 MB) is
  loaded before the click.

### 12.2 The three floors

Measured on the droplet of §3 (four shared vCPUs), in headless Chromium 156 and
under Node 22 — the same JavaScript engine. A visitor's laptop, Firefox and
Safari are the owner's run.

| Floor | Measured |
|---|---|
| First frame within 5 s of the click on a 50 Mbit/s line | **4.1 s and 4.2 s** in two runs — Chromium throttled to 50 Mbit/s with 20 ms of latency, its cache off. 9.3 s at 20 Mbit/s; 0.4 s from a local server; 0.35 s on a second visit |
| No visible lag between a key and its character | the character is handed to the terminal a **median of 12 ms** after the key (twelve keys, two runs; the session's first, 60–69 ms), and painted on the browser's next frame. Whether that reads as none is the owner's to say |
| At most 25 MB downloaded | **21.5 MB** (21 489 834 B in the build measured; the state differs by some kilobytes from one build to the next), counted uncompressed in transit: the state 18.45 MB, the emulator 2.37 MB, the terminal 0.50 MB, the BIOS 0.17 MB |

### 12.3 A key to its frame

Milliseconds from the key to the last byte of the frame it caused:

| Beat | Native (§3.1's run) | v86 under Node | v86 in Chromium |
|---|---|---|---|
| A typed character | 1–2 of CPU | 9–16 | 10–19 |
| `PageUp` / `PageDown` | 3 | 12–27 | 15–20 |
| `F1` — help opens | 7 | 28–86 | 78–80 |
| `Esc` — back to the chat | 5–8 | 16–43 | 18–37 |
| `F3` — the self-model | 56 | 81–111 | 96–105 |
| `Ctrl+P` — settings | 6–8 | 35–84 | 61–63 |
| `Down` in settings | 3 | 31–47 | 37 |
| `Enter` — a message sent, to the end of its scripted reply | 458 | 494–591 | 571–609 |

Emulation costs a frame between four and fifteen times what it costs natively —
around the author's ten — and the slowest screen, the self-model, still arrives
in about a tenth of a second, where it takes 56 ms natively. The reply streams at
its own pace: it is the script's delays, not the processor, that set it. The frames are the native ones: `F1`, `Esc`, `F3` and
`Ctrl+P` write the same 12852, 9387, 7775 and 9208 bytes as §3.3's.

Also checked in Chromium: Cyrillic typed into the input box arrives and is drawn;
`Ctrl+Q` ends the demo, the page says so, and **Start again** has the next one
running in 0.35 s; the console logged no error.

### 12.4 The state

The machine is 60.2 MB, and 18.5 MB as `zstd --ultra -22`. How it got there:

| | Compressed |
|---|---|
| As first saved | 33.6 MB |
| The kernel zeroing what it frees (`init_on_free=1`) | 23.5 MB — the guest alone 8.8, the files 13.5, the running application 1.2 |
| The image's own `/usr` deleted before the start | **18.5 MB** |

It restores in 0.3 s and needs no kernel image beside it, so that image is not
downloaded. With no state the same machine boots to a shell in 2 s and draws the
demo's first frame 3.6 s after being asked.

### 12.5 Found on the way

- **The application panics at start where the system has no certificate
  bundle**: `guarded_client` in `src/shared/net.rs` ends in an `expect` that
  fires when the TLS backend loads no CA certificate — `demo` included, which
  never opens a connection. The guest is given a bundle; the defect is the
  application's own and is on the roadmap.
- **The demo opens with a draft in its input box**, so what a visitor types is
  appended to it; the page says that `Enter` sends it.
- **A terminal that restores a machine has to be shown the frame the application
  already drew** — the application will only send what changes. The state is
  saved with that frame's bytes beside it (`primer.bin`, 7 KB), and the page
  plays them first, with nothing listening to the terminal's answers to the
  queries inside.
- **The grid has to fit the window's height, not only its width**: at 44 rows the
  input box is the first thing to fall below the fold. The page takes the largest
  font at which both fit — 13 px in a 1400×900 window.
- **The tab weighs about 380 MB** more once the demo runs (the renderer's
  resident memory): the guest's 128 MB, the state decompressed, the emulator's
  translated code.
- Seven glyphs come from the browser's fallback fonts, the seven the reel's
  renderer also lacked (`✦ ⚒ ⟳ ∝ ₖ ᵥ ＋`).

### 12.6 Not done

- **Safari** — nobody here has a Mac. Firefox, Chrome, Edge and a real keyboard
  were the owner's run (§12.7).
- **Resize** (the virtio console's message) and **fullscreen with the keyboard
  lock**: the grid is fixed and scaled.
- **The multilib path of `build_i686.sh`**, the one a CI runner would take.
- **The light look and Russian**: the state holds one theme and one language.

### 12.7 The go/no-go — GO

**The owner, 2026-11-10**, on Windows in Chrome, Firefox and Edge (Edge as Chrome):
"the demo works excellently — no worse than JupyterLab", and an unambiguous go.

- **Speed**: "very decent". A lag shows only while `Up` or `Down` is held on a list
  in the settings — a key repeating thirty times a second against a screen that
  takes 31–47 ms.
- **Keys**: the only ones that do not arrive are the three the page names. `F5` and
  `Ctrl+R` reach the application and do not reload the tab.
- **The page**, four defects, all the page's own and all fixed on the branch:
  - a scrollbar under the grid's right edge, eating its last column. xterm.js's
    viewport keeps a classic scrollbar wherever scrollbars are not overlays — a
    headless browser hides them, which is why the probe's own run never drew it —
    and the page's `.terminal` rule had been applied twice, because xterm.js gives
    that class to its own element;
  - a window a few characters wide while the demo loaded, and the same one under
    "The demo has ended". The window now has the grid's size in all three states,
    with the progress of the download in it; the size is worked out as the
    terminal works it out, and checked at four window sizes and four display
    scales;
  - the design is the probe's and not the site's — stage 3 uses the site's own
    styles.
- **The application**, one: the settings named Russian as the framework language
  of the demo's English companion. The fixture's defect, in any terminal; fixed on
  a branch of its own (`fix/demo-profile-language`).

## 13. Stage 2 — groundwork, measured before the pipeline is written

Branch `feat/web-demo-pipeline`, 2026-11-10. Two questions the pipeline's design
hangs on were put to the emulator first.

### 13.1 A guest made of named packages (F2)

The probe's guest was the Buildroot image v86's site boots: it works, and nobody
can say exactly what is in it. The same demo was started on **Alpine 3.24's own
32-bit kernel** (`linux-virt` 6.18.62) **and an initramfs written here**
([`tools/web_demo/initramfs.py`](../../tools/web_demo/initramfs.py)): a static
BusyBox from Alpine's package, a certificate bundle, the application with its four
libraries and loader, and an `/init` of a dozen lines. No distribution is installed
in the guest; no file in it is of unknown origin.

- **It boots and draws**: the kernel reaches `/init` in 3.8 s, the demo's first
  frame is complete 7.5 s after power-on, and it is the same 7 259 bytes as on the
  Buildroot kernel. Nothing but the two files is needed — no 9p share, no shell
  session to script.
- **The state is 3 MB larger**: 69.8 MB, 21.6 MB compressed, against the probe's
  18.5. The bare kernel is 7.5 MB of that — a distribution's kernel has 10 MB of
  code where Buildroot's had 5. A click then downloads about **24.7 MB**: inside
  the floor of 25, by 0.3 MB, counted uncompressed in transit. CloudFront will
  compress the emulator and the terminal on the way (§3.4), which is some 2 MB
  off; the margin is real but thin, and a kernel built for the purpose is what
  would widen it.
- **Two things the initramfs had to learn.** Alpine's BusyBox has no `cttyhack`:
  the application gets its controlling terminal by being started in a new session
  that opens the terminal itself. And **a terminal nobody holds open forgets its
  size**: `stty` on the device, then the application, gave a window of nothing by
  nothing and a frame with nothing in it — the size is now set on the very
  descriptors the application inherits.

### 13.2 A terminal the size of the visitor's window

The probe scales one grid, 116×44, to whatever window it finds: 10 px text in a
1280×720 window. The alternative is a real resize, and a serial line cannot carry
one. **The virtio console can**: Alpine's kernel has it built in, v86 implements
its resize message, and with the application started on `/dev/hvc0`:

| | Bytes | Key or message → last byte |
|---|---|---|
| `F1` | 12 818 | 89 ms |
| `Esc` | 9 372 | 40 ms |
| the window resized | 8 440 | 80 ms |
| resized again, larger | 10 101 | 73 ms |

The application redraws itself for the new size in the time a screen takes to
open, as it does when a real terminal window is dragged.

**One trap**: the two numbers arrived swapped. A message of 100 columns by 30 rows
gave the application a window 30 columns wide. v86 0.5.473 and this kernel
disagree on the order of the two fields in the message; the page has to send them
in the order the pinned kernel reads, and the state's check has to prove it by
resizing and measuring what comes back — or a kernel update turns every visitor's
terminal on its side.

### 13.3 What stages 2 and 3 are, after this

**Stage 2 — the pipeline** (`feat/web-demo-pipeline`):

1. The tools move from the probe's guest to §13.1's, on the virtio console, and
   `verify_state.mjs` gains the resize check of §13.2.
2. A manifest written by the build — every redistributed piece with its version,
   licence and source — so that the page's list is the build's own and not prose.
3. A `web-demo` job in `release.yml`, **not a gate**, as the reel's is: the 32-bit
   build with the runner's multilib, the state, its check, one asset
   (`mindfork-webdemo.tar`) covered by the checksums and the attestation.
4. Rehearsed with a prerelease tag before the release that carries it
   (AGENTS.md §6).

**Stage 3 — the page** (`feat/web-demo-page`): a page of the site, in the site's
templates and styles; the deploy fetches the asset from the newest release that
has one, as it fetches the reel; the terminal takes the window's size at a fixed,
readable font; the notice, the keys and the licences from the manifest; the
hero's `$ mindfork demo` hint gains a link.

Forks for the owner:

- **F6 — pinning the guest's kernel.** Alpine keeps only the newest build of a
  package in a branch, so a URL pinned by hash stops resolving within weeks.
  (a) **Alpine's release images, which are kept forever** — the kernel out of
  `alpine-virt-3.24.4-x86.iso`, BusyBox and the certificates out of
  `alpine-minirootfs-3.24.4-x86.tar.gz`, each pinned by its published sha256 —
  **recommended**: it never rots and fails closed; it costs a 51 MB download per
  build and thirty lines that read a file out of an ISO. (b) The branch's packages
  pinned by hash: simplest, and broken by the next kernel update. (c) The branch's
  newest packages, verified against Alpine's signing key: never rots, but the
  kernel floats under the demo — and §13.2's trap is a kernel's to spring.
- **F7 — the page's address.** `/demo/` is where the reel's files live, and the
  deploy treats that prefix as its own. (a) **`/try/` — recommended**; (b) another
  name; (c) `/demo/`, with the deploy's two passes reworked around it.
- **F8 — the certificate bundle.** (a) **Fix the application's panic (§12.5) and
  ship no bundle — recommended**: one piece fewer to carry and to name; (b) carry
  the bundle and leave the defect on the roadmap.

**The owner's decisions (2026-11-10):** "as you recommend" — F6(a), Alpine's
release images; F7(a), `/try/`; F8(a), the panic fixed and no bundle shipped. And
on how often the demo's binary changes: **only at a release** — the job builds it
from the tag, and the site takes it from the newest published release that has
one, exactly as it takes the reel.

### 13.4 Stage 2, first part — what is built

`tools/web_demo/` now builds §13.1's guest on §13.2's console, end to end:

- **`fetch.py`** pins Alpine 3.24.4's two release images by their published
  sha256 — the kernel (Linux 6.18.60, `linux-virt`) is read out of
  `alpine-virt-3.24.4-x86.iso` by thirty lines that know an ISO 9660 directory,
  BusyBox and musl out of `alpine-minirootfs-3.24.4-x86.tar.gz` — and no longer
  fetches the Buildroot image.
- **`initramfs.py`** writes the guest: `/init`, BusyBox with musl, the
  application with its four libraries and loader. **No certificate bundle**: the
  application starts without one since `fix/no-ca-start`
  (`shared::net::build_client`), and `--ca-bundle` remains only for a build older
  than that.
- **`make_state.mjs`** boots the kernel with that archive and saves the machine
  once the first frame is drawn — and refuses a frame that is not the size
  `machine.json` names, which is how a terminal that forgot its size, or an
  application that never started, fails the build instead of being published.
- **`verify_state.mjs`** restores the state, presses a key, and **resizes**:
  wider and shorter at once, so that a swapped pair cannot pass, then measures the
  frame that comes back. `130x38 asked, 130x38 drawn, 64 ms`. The order the
  message's numbers go in is `machine.json`'s `resize_sends`.
- **The page's terminal is the size of the window**: 15 px text and as many
  cells as fit — 134×40 in a 1400×900 window, 134×49 at 1920×1080, 108×30 at
  1024×700 — and the application is told when the window changes. Smaller text
  only where even 80×24 would not fit.

Measured on this build: the state 69.6 MB, 21.5 MB compressed; a click downloads
**24.6 MB**; restored in 0.32 s; in Chromium a typed character a median of 16 ms
after its key, `F1` in 64 ms, a reply in 572 ms.

**Found on the way: a second machine in the same page can sit silent.** "Start
again" used to destroy the machine and make another. With this guest the second
one, started more than about ten seconds into the page's life, answered nothing —
not a key, not a resize — for seconds, sometimes for longer than a test would
wait; a first machine answers within a quarter of a second whenever it is started,
and under Node five machines restored one after another in one process all answer.
Waiting for the old machine's `destroy()` to finish did not cure it, and the cause
is not established. **"Start again" is now a reload of the page that starts by
itself** — every demo is a first machine, with everything it needs in the browser's
cache: eight restarts out of eight answer, where three of five did not.

**Still to do in stage 2**: the manifest of redistributed pieces, the `web-demo`
job in `release.yml` with the runner's multilib build, the asset, and the
prerelease-tag rehearsal.

