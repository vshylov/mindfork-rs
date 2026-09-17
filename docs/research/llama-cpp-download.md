# The engine, downloaded — llama.cpp's backends named, fetched and pointed at

> **Status:** researched and decided (2026-09-09), not yet implemented. Every
> number in §3 was measured against `github.com/ggml-org/llama.cpp` on
> 2026-09-09 (newest build `b10883`, published that morning); §3.3's drift rows
> come from `b9000` (2026-05-02) and `b6000` (2025-07-27). **Every fork in §6
> is at its recommendation** — F1, F2, F8 and F9 put to the user and chosen
> explicitly (the user's decision, 2026-09-09), F3–F7 stated with their
> recommendations in the same exchange and not contested.
>
> The gap: [install.md](../install.md) §3 names llama.cpp `llama-server` as the
> recommended backend and then assumes it exists — the one step between
> "mindfork is installed" and "a local model answers" that the app does not
> help with. The app already downloads, verifies and unpacks a third-party
> toolchain for the other half of its local stack (`mindfork sandbox setup`,
> ADR 0005); this asks what the same shape costs for the engine.

## 1. Why, precisely

`mindfork sandbox setup` fetches wasmer and thirteen wheels, verifies each
against a pinned sha256, unpacks them into `data/sandbox/`, and — with
`--enable-python` — turns the tool on in `settings.json`. Two commands later a
user has a working Python sandbox and never saw a URL.

The engine has no such command. A user who picks **managed** mode is told to
type a path into *Model/server → llama-server binary* ([install.md](../install.md)
§3, `install.md:310`) and left to find the binary themselves. Doing that by
hand means: open the releases page, understand that the newest tagged release
is *not* the newest build, know that `bin-win-cuda-12.4-x64` is a server build
and `cudart-...-cuda-12.4-x64` is the CUDA runtime it will not start without,
pick between seven Windows archives that differ by a token in the middle of a
filename, and unpack the pair into one directory. Every one of those is a place
to get it wrong quietly — the commonest failure being a CUDA build without its
runtime, which does not error: it loads no CUDA device and runs on the CPU.

The asset table the sandbox uses cannot be copied here. Wasmer is pinned —
four `(os, arch)` rows, four URLs, four digests, edited by hand at a version
bump. llama.cpp publishes **thirteen nightlies a day** (§3.1) across **seven
usable Windows archives and six Linux ones** (§3.2) whose names have been
renamed twice in fourteen months (§3.3). What has to be built is not a table
but a **derivation**: read the release, keep what this platform can run, and
name the choices back to the user.

**Requirements.**

- **R1.** The list of backends is **derived from the release**, never
  enumerated in our source. A backend upstream renames (`hip-radeon` →
  `rocm-10.0`, measured in §3.3) or adds must appear without a mindfork
  release.
- **R2.** Filtered to the running OS and architecture. Nothing that cannot run
  here is offered; nothing whose shape we do not recognise is guessed at.
- **R3.** Every downloaded byte is verified against a digest published by the
  same source, before it is unpacked — the promise
  [SECURITY.md](../../SECURITY.md) already makes for `sandbox setup`.
- **R4.** A CUDA backend arrives complete. If the pieces upstream ships
  separately are needed to run, they are fetched together or the install is
  refused — never a half-install that silently degrades.
- **R5.** The install is **verifiable after the fact**: the command proves the
  binary it just wrote runs, and says which build and which compute devices it
  found.
- **R6.** Pointing the settings at the result is a **separate, opt-in flag** —
  the shape ADR 0005 §5 fixed for `--enable-python`, for the same reason
  (a path written into user data is a deliberate act).
- **R7.** Nothing changes for a user who already has llama.cpp: the existing
  path field, `MINDFORK_LLAMA_BIN`, and external mode are untouched.

## 2. What exists (inventory)

### 2.1 The precedent — `mindfork sandbox setup`

[`src/features/sandbox_setup.rs`](../../src/features/sandbox_setup.rs) (808
lines), dispatched from [`src/main.rs:609`](../../src/main.rs) as
`run_sandbox_setup`. The shape worth copying, item by item:

- **Progress is a callback, not a print.** `setup(..., mut progress: impl
  FnMut(&str))` — the `features` layer never touches stdout; `main.rs:629`
  supplies `|msg| println!("{msg}")`. That is how the "no `println!`" rule
  (CLAUDE.md §Conventions) and a multi-minute console job coexist, and the
  Windows installer depends on it (`packaging/windows/mindfork.iss:260`, run
  **without** `runhidden` precisely so the console shows progress).
- **The single-instance guard is taken first** (`main.rs:619`) — provisioning
  must not run while the app might be reading what it replaces.
- **Every asset is sha256-verified** (`verify_sha256`, `verify_file_sha256`),
  with a hand-rolled `hex_lower` (no `hex` crate — the precedent
  ADR 0008 §"hex" set).
- **Provisioning repairs, it does not refuse** (`sandbox_setup.rs:313`): a
  present-but-mismatched file is replaced; only a *freshly downloaded* file
  that still mismatches is fatal.
- **Zip-slip is guarded** with `entry.enclosed_name()`; tar relies on
  `tar-rs`'s own containment.
- **An unsupported platform is a localized refusal, not a guess**
  (`sandbox.setup.wasmer.no_platform`).
- **`--enable-python` is applied past the `?`** (`main.rs:631`) — a failed
  provisioning cannot enable anything — and goes through
  `data_migration::run` + language seeding (`main.rs:641-669`), because it is
  the only CLI path that writes user data.

Two gaps in the precedent this track must not inherit:

- **No timeout of any kind.** `http_client` (`sandbox_setup.rs:404`) sets a
  user-agent and nothing else, while `shared/api/http.rs:26` sets a deliberate
  10 s connect timeout for exactly the reason a bare client is wrong. For a
  645 MB asset (§3.6) a stalled socket is a hung command.
- **No resume, no `.part`.** `download_to_file` truncates through
  `File::create`. Acceptable for a 45 MB wheel set; not for 645 MB.

### 2.2 Where a binary path is read from

`ManagedSettings.binary: Option<String>` —
[`src/shared/config.rs:349`](../../src/shared/config.rs), `#[serde(default)]`
on the struct, so the JSON key is the field name verbatim. It appears three
times:

| JSON path | Struct | Default port |
|---|---|---|
| `engine.managed.binary` | `ManagedSettings` (`config.rs:349`) | 8000 |
| `impersonation_engine.managed.binary` | the same struct (`config.rs:734`) | 8002 |
| `embed.managed.binary` | `ManagedEmbedSettings` (`config.rs:791`) | 8001 |

All three name the **same executable** — the embedder is `llama-server
--embeddings` (`managed.rs:187`), the impersonation engine is a second
`llama-server` on another port. One install serves all three.

The launcher is `Command::new(&cfg.binary)` —
[`src/shared/api/managed.rs:303`](../../src/shared/api/managed.rs) — the string
verbatim: an absolute path is used as given, a bare name falls through to the
OS `PATH`. Nothing searches next to the application. (spec §3.4 says the binary
"is looked up in `PATH` and next to the application binary"; only the first
half exists in code. Recorded in §8.)

The settings row is `text_row(ids.binary, "ui.settings.field.binary")` in
`managed_rows()` —
[`src/screens/settings/helpers.rs:187`](../../src/screens/settings/helpers.rs)
— free text, no picker, no completion, validated only at launch preflight
(`managed.rs:259-336`).

### 2.3 What `data/` holds, and what backup does with it

`Paths::sandbox_dir()` = `<root>/sandbox`
([`src/shared/paths.rs:361`](../../src/shared/paths.rs)); `<root>` is
`<exe_dir>/data` in portable mode — `target/debug/data/` in dev. A sibling
`llama_dir()` is a two-line addition.

Backup and restore both work off **allow-lists**: `TOP_DIRS = ["chats",
"dictionaries", "locales"]` plus named top files
([`src/features/backup.rs:158`](../../src/features/backup.rs)), and
`clear_user_data` (`backup.rs:792`) removes exactly that set. So a new
`data/llama/` is neither swept into an archive nor deleted by a restore —
the same silence `data/sandbox/` already enjoys, and the right one: these are
re-downloadable assets, not user data. `mindfork.iss:265` takes the same line
for uninstall ("re-downloadable rather than being ours to delete").

### 2.4 The crates already in the graph

Every piece is present. **This track adds no dependency.**

| Need | Crate | Already used for |
|---|---|---|
| HTTP + streaming body | `reqwest` 0.13.4 (`json`, `stream`, `rustls`) | every provider, `sandbox setup` |
| JSON | `serde_json` 1.0.150 | everything |
| sha256 | `sha2` 0.10 | sandbox asset verification |
| `.zip` (deflate) | `zip` 2.2.2 (`deflate`, `aes-crypto`) | backup, wheel unpacking |
| `.tar.gz` | `tar` 0.4 + `flate2` 1.0 | wasmer unpacking |
| byte stream | `futures-util` 0.3.32 | `sandbox setup` |

The zip crate's feature set is the one risk worth checking rather than
assuming, and it checks out: **all 52 entries of every llama.cpp Windows
archive are plain deflate** (§3.4), so the deliberate absence of `bzip2`/`zstd`
(native C dependencies, `Cargo.toml:151`) costs nothing here.

## 3. What upstream actually publishes (measured 2026-09-09)

### 3.1 The releases are nightlies, and `latest` is not one of them

```
GET /repos/ggml-org/llama.cpp/releases/latest   →  tag v0.4.0, 1 asset
                                                   (nightly-tag.txt, 7 bytes)
GET /repos/ggml-org/llama.cpp/releases?per_page=15
   b10883 pre=true assets=27  2026-09-09T17:29Z     ← 13 builds published
   b10881 pre=true assets=27  2026-09-09T15:32Z       on 2026-09-09 alone
   …
```

Three facts follow, and the first is a trap:

1. **`releases/latest` is useless here.** It excludes prereleases, and every
   build tag `bNNNNN` *is* a prerelease. It returns the semver release
   `v0.4.0` (2026-09-04), whose single asset is a 7-byte `nightly-tag.txt`
   containing `b10809` — upstream's own pointer at the nightly matching that
   source tag, and a legitimate third option for "which build" (§6, F2).
2. The unfiltered list is returned **newest first**, so page 1 is the newest
   build. The `Link` header reports `page=7140` as last at `per_page=1` —
   roughly seven thousand releases; enumerating them is not a design.
3. **Thirteen builds a day.** "Newest" is a fast-moving target, and the
   sandbox track's hardest-won lesson applies verbatim
   (`sandbox_setup.rs:26-55`: a bad upstream publish produced sandboxes that
   could not run Python at all, and only a *fresh* install could tell you).
   Whatever is installed, the exact tag must be recorded and re-installable.

### 3.2 The 27 assets of a build, and the three families

`b10883`, sorted; the families are distinguishable by name alone:

```
llama-b10883-bin-win-cpu-x64.zip              18.4 MB   ┐
llama-b10883-bin-win-cuda-12.4-x64.zip       254.1 MB   │
llama-b10883-bin-win-cuda-13.3-x64.zip       149.7 MB   │  server builds
llama-b10883-bin-win-vulkan-x64.zip           31.7 MB   │  llama-<tag>-bin-…
llama-b10883-bin-win-rocm-10.0-x64.zip       244.2 MB   │
llama-b10883-bin-win-sycl-x64.zip            119.8 MB   │
llama-b10883-bin-win-openvino-2026.3.1-x64.zip 80.3 MB  │
llama-b10883-bin-ubuntu-x64.tar.gz            16.8 MB   │  ← note: no "cpu"
llama-b10883-bin-ubuntu-vulkan-x64.tar.gz     30.2 MB   │
llama-b10883-bin-ubuntu-rocm-10.0-x64.tar.gz 218.4 MB   │
llama-b10883-bin-ubuntu-sycl-fp16-x64.tar.gz  54.0 MB   │
llama-b10883-bin-ubuntu-sycl-fp32-x64.tar.gz  53.7 MB   │
llama-b10883-bin-ubuntu-openvino-…-x64.tar.gz100.8 MB   │
llama-b10883-bin-{win-cpu,win-cuda-13.4,ubuntu-vulkan}-arm64…             │
llama-b10883-bin-{macos-arm64,macos-x64,android-arm64,ubuntu-s390x,      │
                  win-opencl-adreno-arm64}…                              ┘
cudart-llama-bin-win-cuda-12.4-x64.zip       391.4 MB   ┐  CUDA runtime
cudart-llama-bin-win-cuda-13.3-x64.zip       391.0 MB   │  (Windows only,
cudart-llama-bin-win-cuda-13.4-arm64.zip     153.3 MB   ┘   no build tag)
llama-b10883-ui.tar.gz                         3.1 MB   ┐  not servers
llama-b10883-xcframework.zip                  87.0 MB   ┘
```

Three things the user's sketch got right and one it did not:

- The backend id **is** the middle of the name, exactly as sketched: strip
  `llama-<tag>-bin-`, strip the OS token, strip the arch token, and `cpu`,
  `cuda-12.4`, `cuda-13.3`, `vulkan`, `rocm-10.0` fall out. Windows x64 at
  `b10883` yields two more the sketch omitted — `sycl` and
  `openvino-2026.3.1`.
- The `cudart-*` pairing is by **(backend token, arch)** and carries **no
  build tag**: `cudart-llama-bin-win-<backend>-<arch>.zip` sits in the same
  release as `llama-<tag>-bin-win-<backend>-<arch>.zip`.
- **On Linux the CPU build has no backend token at all** —
  `llama-b10883-bin-ubuntu-x64.tar.gz`. The derivation must read an empty
  middle as `cpu`, or Linux gets a backend named "" that nobody can type.

### 3.3 The names have drifted — derive, do not enumerate

Two older builds, fetched by tag:

| | `b6000` (2025-07-27) | `b9000` (2026-05-02) | `b10883` (2026-09-09) |
|---|---|---|---|
| assets | 13 | 28 | 27 |
| Linux/macOS extension | **`.zip`** | `.tar.gz` | `.tar.gz` |
| AMD on Windows | `win-hip-radeon-x64` | `win-hip-radeon-x64` | **`win-rocm-10.0-x64`** |
| CUDA runtimes | 12.4 | 12.4, 13.1 | 12.4, 13.3, 13.4-arm64 |
| oddities | — | `310p-openEuler-x86`, `910b-openEuler-x86-aclgraph`, `macos-arm64-kleidiai` | `ubuntu-s390x`, `win-opencl-adreno-arm64` |
| `digest` in API | yes | yes | yes |

So: the archive **extension changed** for a whole platform inside a year; the
AMD backend was **renamed**; the arch is **not always the last token**
(`-aclgraph`); the OS is **not always the first token** (`310p-openEuler-…`);
`x86` appears beside `x64`. A hard-coded enum of backends would have broken
twice already, and a "split on dashes and take position N" parser breaks on
the openEuler rows today.

**The rule that survives all three sets** — anchored on both ends, skipping
anything that does not match exactly:

```
llama-<tag>-bin-<os>[-<backend…>]-<arch>.<ext>
      ^^^^^ the release's own tag      ^^^^^^ ∈ {x64, arm64}
                    ^^^^ ∈ {win, ubuntu, macos}   ^^^^ read, never assumed
```

`<backend…>` is everything between, joined with `-`; empty → `cpu`. Applied to
the three sets it keeps every real server build for a supported platform and
drops, without a special case: `ui`, `xcframework` (no `-bin-`), `android-*`,
`*-openEuler-*` (OS token), `ubuntu-s390x`, `macos-arm64-kleidiai` (arch
token). The extension is **read off the name**, so a return to Linux zips
costs nothing.

### 3.4 What is inside an archive

Read through HTTP range requests against the zip central directory (GitHub's
asset host answers `206`, `Accept-Ranges: bytes` — measured), and by streaming
the tarball for Linux:

| archive | entries | unpacked | layout |
|---|---|---|---|
| `bin-win-cpu-x64.zip` | 51, all deflate | 46.7 MB | **flat**, no root dir |
| `bin-win-cuda-12.4-x64.zip` | 52, all deflate | 593.9 MB | flat (`ggml-cuda.dll` alone is 547 MB) |
| `cudart-…-cuda-12.4-x64.zip` | 3, all deflate | 574.1 MB | flat: `cublasLt64_12.dll`, `cublas64_12.dll`, `cudart64_12.dll` |
| `bin-win-rocm-10.0-x64.zip` | 55, all deflate | 1124.1 MB | flat; **bundles its runtime** (`amdhip64_7.dll`, `amd_comgr.dll`) |
| `bin-win-vulkan-x64.zip` | 52, all deflate | 90.0 MB | flat |
| `bin-ubuntu-x64.tar.gz` | ~50 | ~47 MB | **root dir `llama-b10883/`**, symlinks, mode `0755` |

Four consequences:

1. **`llama-server` is now a thin launcher.** 17 864 bytes on Linux, ~10 KB on
   Windows, next to `llama-server-impl.{so,dll}` and ~50 siblings. It cannot
   be copied out of its directory — a fact the settings hint should say out
   loud, since the field invites exactly that.
2. **Linux needs no `LD_LIBRARY_PATH`.** Read straight out of the ELF of
   `b10883`'s `llama-server`: `DT_RUNPATH = ['$ORIGIN']`, `DT_NEEDED =
   [libllama-server-impl.so, libstdc++.so.6, libgcc_s.so.1, libc.so.6]`. So
   `Command::new(path)` as it stands is enough, and the supervisor needs no
   change. (The `libstdc++`/`libc` floor is the Ubuntu builder's — §5.)
3. **The two layouts differ by exactly one root component.** One rule covers
   both: if every entry shares a single leading path component, strip it.
4. **CUDA is not self-contained and ROCm is.** `ggml-cuda.dll` links
   `cublas64_12` / `cudart64_12`, which ship only in the `cudart-*` archive;
   Windows resolves DLLs from the spawned executable's own directory, so the
   pair must be unpacked into **one** directory. Without it nothing errors —
   the CUDA backend simply fails to load and the server runs on the CPU. That
   is R4, and §3.5's post-condition is how a user finds out.

### 3.5 The post-condition is on the binary itself

`llama-server` answers two questions without a model, both exiting `0`
(measured against the local build 10807):

```
llama-server --version        → stderr:  version: 0.3.0-dev (build 10807, commit 163a4079…)
llama-server --list-devices   → stdout:  Available devices:
                                           (none)
```

The build number is the tag's number, so `--version` proves *what* was
installed and that the whole DLL/so set loads. `--list-devices` proves the
*backend* initialised: on a CUDA install with no driver or no `cudart` it
prints `(none)`, which is the difference between "you have a CUDA build" and
"you have a CUDA build that will run on your CPU". Note the streams differ —
version on stderr, devices on stdout.

### 3.6 The four routes to the list, measured

| route | bytes | structured | sha256 | limit |
|---|---|---|---|---|
| `api.github.com/…/releases?per_page=1` | 63 400 | JSON | **yes** (`digest: "sha256:…"`) | 60/h per IP unauthenticated (`X-RateLimit-Limit: 60`) |
| `api.github.com/…/releases/tags/<tag>` | ~63 KB | JSON | yes | same |
| `github.com/…/releases` (HTML index) | 489 980 | scrape | no | none published |
| `github.com/…/releases/expanded_assets/<tag>` | 113 628 | scrape | no | none published |

The HTML index does carry the links (270 of them — the ten newest releases ×
27), so the sketched approach works. It is nonetheless the worse of the two:
eight times the bytes, a markup contract instead of a documented one, and — the
decisive one — **no digests**, which would force R3 back onto a hand-maintained
pin table, i.e. exactly the thing §3.3 says cannot be maintained. The API's
`digest` field is present even on the 2025 release. Fork F1.

### 3.7 What a backend costs

Windows x64, `b10883`, download → on disk:

| backend | download | on disk | note |
|---|---|---|---|
| `cpu` | 18 MB | 47 MB | runtime-dispatched `ggml-cpu-*.dll` per ISA |
| `vulkan` | 32 MB | 90 MB | works on NVIDIA **and** AMD via the driver's Vulkan |
| `cuda-13.3` | 150 + 391 MB | ~1.1 GB | + `cudart-…-13.3` |
| `cuda-12.4` | 254 + 391 MB | ~1.17 GB | + `cudart-…-12.4` |
| `rocm-10.0` | 244 MB | 1.12 GB | runtime bundled |
| `sycl` | 120 MB | — | Intel |
| `openvino-2026.3.1` | 80 MB | — | Intel |

Linux x64 is smaller across the board (`cpu` 17 MB, `vulkan` 30 MB,
`rocm-10.0` 218 MB) and has no CUDA archive at all — the CUDA rows are Windows
only. A 645 MB download landing 1.17 GB in `target/debug/data/` during
development, one `cargo clean` from oblivion, is worth knowing before it
happens (§5).

## 4. Design

### 4.1 The command surface

A new top-level noun beside `sandbox`, parsed by the same hand-rolled parser
([`src/features/cli.rs`](../../src/features/cli.rs)); subcommand and flag names
are protocol, not translated (`cli.rs:17`):

```
mindfork llama backends [--build <tag>]
mindfork llama setup --backend <id> [--build <tag>] [--set-binary]
                     [--no-cudart] [--force]
mindfork llama installed
```

- **`backends`** — resolve the build (§4.2), print one line per backend for
  this OS/arch: id, download size, unpacked size, and `(installed)` when it
  already is. This is the command the user asked for, and the answer to
  `setup` with no `--backend`.
- **`setup`** — download, verify, unpack, verify again (§3.5). `--backend` has
  **no default**: omitted, it prints the `backends` list and exits `2`, so a
  645 MB choice is never made on the user's behalf (F3).
- **`--build <tag>`** — pin, e.g. `--build b10883`. Omitted: the newest build
  (§4.2). This is `sandbox_setup.rs:26`'s lesson in a flag.
- **`--set-binary`** — the R6 flag; long form only, like `--enable-python`,
  and applied only past the `?` (F6).
- **`--no-cudart`** — for a host that already has the CUDA toolkit on `PATH`.
- **`installed`** — what is in `data/llama/`, with sizes, so the disk cost of
  three experiments is visible without a file manager.

### 4.2 Resolving a build, and deriving the backends

One request either way:

- with `--build <tag>` → `GET /repos/ggml-org/llama.cpp/releases/tags/<tag>`;
- without → `GET /repos/ggml-org/llama.cpp/releases?per_page=5`, take the
  first entry that yields at least one backend by §3.3's rule. (Five, not one,
  so a semver release like `v0.4.0` landing at the top costs a retry, not a
  failure.)

The derivation is a **pure function over asset names**, `fn backends(assets:
&[Asset], tag: &str, os: &str, arch: &str) -> Vec<Backend>`, applying §3.3
exactly: anchored on `llama-<tag>-bin-`, the OS token from a three-row map
(`windows→win`, `linux→ubuntu`, `macos→macos`), the arch token from a two-row
map (`x86_64→x64`, `aarch64→arm64`), the middle joined with `-`, empty → `cpu`,
extension read from the name. Non-matching names are skipped silently. Its
input is a list of names; its tests are the three real name sets of §3.3.

For a backend starting with `cuda-` on Windows, the same release is searched
for `cudart-llama-bin-win-<backend>-<arch>.zip`; found, it is a second asset of
the same install (R4). Not found on a `cuda-*` backend → the install is
**refused** with the reason, rather than producing §3.4's silent CPU fallback.

### 4.3 Where it lands

`Paths::llama_dir()` = `<root>/llama`, beside `sandbox_dir()`
(`paths.rs:361`), one directory per install:

```
data/llama/
├─ cpu-b10883/          llama-server[.exe] + ~50 siblings, flat
├─ cuda-12.4-b10883/    …including the three cudart DLLs
└─ .tmp-cuda-12.4-b10883/   transient: unpacked here, renamed on success
```

`<backend>-<tag>` reads back as what it is, lets two installs coexist for a
comparison, and makes removal one directory. It inherits `data/`'s three
properties for free (§2.3): portable, out of backups, out of a restore's
sweep. In dev that is `target/debug/data/llama/` — §5.

### 4.4 Download, verify, unpack

Per asset, into the install's own `.tmp-…` directory:

1. Stream `reqwest` → `<asset>.part`, hashing with `sha2` as the bytes pass
   (the `sandbox_setup.rs:413` shape), progress through the same `impl
   FnMut(&str)` callback so `main.rs` owns the only `println!`.
2. On a client built with an explicit **`connect_timeout(10s)` and
   `read_timeout(60s)`** — closing §2.1's gap, not inheriting it. No total
   timeout: 645 MB on a slow line is not an error.
3. If `<asset>.part` already exists and is shorter than the asset's `size`,
   resume with `Range: bytes=<len>-` (measured: `206` + `Content-Range`),
   seeding the hasher from the existing bytes (F5).
4. Compare against the release's `digest` (R3). A mismatch after a resumed
   download deletes the part and retries once from zero; a mismatch on a fresh
   download is fatal, named, localized.
5. Unpack by the **name's extension**: `.zip` → `zip` + `enclosed_name()`;
   `.tar.gz` → `flate2` + `tar::Archive::unpack` (which preserves the `0755`
   of §3.4 and creates the tarball's symlinks). Strip a single shared leading
   component if there is one. On unix, a `.zip` gets `0o755` set explicitly —
   `unpack_wheel`'s `fs::write` drops modes, which is harmless for wheels and
   would be a silent breakage here if upstream returned to §3.3's Linux zips.
6. `rename` `.tmp-…` → the final directory. `--force` removes an existing one
   first; without it, an existing directory is reported and kept.

### 4.5 What is proved afterwards

Run `<dir>/llama-server --version` (stderr) and require the build number to
equal the tag's — that is R5, and it is also the only thing that proves the
DLL/so set is complete. Then `--list-devices` (stdout) and print what it says.
If the backend is not `cpu` and the device list is empty, print an actionable
line — driver missing, or the runtime is — **without failing the install**: the
files are on disk and correct; what is missing is on the host.

### 4.6 Pointing the settings at it

`--set-binary` writes `<dir>/llama-server[.exe]` into `engine.managed.binary`,
and into `impersonation_engine.managed.binary` / `embed.managed.binary`
**only where they are currently empty** (never overwriting a path the user
chose). It reuses the `--enable-python` precautions verbatim
(`main.rs:641-669`): `data_migration::run` first (the ADR 0006 downgrade
guard), language seeding when the file does not yet exist, and the whole thing
past the `?` so a failed install writes nothing. That helper wants extracting
from `main.rs` into something both flags call.

Whether it also sets `engine.mode = managed` is F7.

### 4.7 What the user sees

```
> mindfork llama backends
Build b10883 (2026-09-09), windows/x86_64:
  cpu                  18 MB  →   47 MB
  cuda-12.4           645 MB  → 1170 MB   (+ CUDA runtime)
  cuda-13.3           541 MB  → 1100 MB   (+ CUDA runtime)
  openvino-2026.3.1    80 MB
  rocm-10.0           244 MB  → 1120 MB
  sycl                120 MB
  vulkan               32 MB  →   90 MB

> mindfork llama setup --backend vulkan --set-binary
Downloading llama.cpp b10883 vulkan (windows/x86_64), 32 MB…
  32 / 32 MB
Verifying…
Extracting…
Installed: data\llama\vulkan-b10883
  version: 0.4.0 (build 10883)
  devices: Vulkan0 (NVIDIA GeForce RTX 4090)
The engine binary is set in the settings.
```

Every line from the locale bundle, in the CLI language the peek phase already
resolves (`main.rs:31-55`).

## 5. Difficult spots

1. **The 60/hour API limit is per IP.** One request per invocation makes it
   unreachable for a person, and reachable for an office behind one NAT. The
   error must say so in words rather than surfacing a bare `403`; a
   `GITHUB_TOKEN`-if-set header is a cheap escape hatch (F1's tail).
2. **`linux → ubuntu` is an assumption with a floor.** The archives are built
   on Ubuntu and `DT_NEEDED` names `libstdc++.so.6`, `libgcc_s.so.1`,
   `libc.so.6` (§3.4). On an older distro the loader fails — a legible message
   is the whole mitigation available.
3. **`data/` under `target/` in dev.** 1.17 GB inside `target/debug/data/`,
   deleted by `cargo clean`, and re-downloaded. Not a defect (it is what
   portable mode means), but it belongs in the docs and probably in
   [lessons.md](../lessons.md).
4. **No free-space check.** `std` has none and no dependency provides one; a
   full disk surfaces as an `io::Error` mid-unpack, into a `.tmp-…` directory
   that the next `--force` removes. Acceptable; naming the required space up
   front (§4.7 prints it) is most of the value.
5. **Windows holds files open.** The single-instance guard (`main.rs:619`)
   covers the app; a `llama-server` a user started by hand from the same
   directory would make `--force` fail with a permission error, which needs
   its own message.
6. **A freshly downloaded `.exe` meets SmartScreen and antivirus.** Nothing to
   do about it beyond not being surprised; the sandbox track has the same
   exposure with `wasmer.exe` and has not been bitten.
7. **Thirteen builds a day means the newest is untested.** `--build` and the
   recorded tag in the directory name are the answer; `installed` makes a
   rollback one command away because the previous install is still there.
8. **A `cuda-*` release with no matching `cudart-*`** — refused (§4.2), so an
   upstream reshuffle turns into a message rather than a CPU-speed "GPU"
   install.
9. **The settings write is the only CLI path into user data** besides
   `--enable-python`, and inherits both of its precautions or none.

## 6. Forks

> **Resolved 2026-09-09.** Every fork below is adopted **at its
> recommendation**. F1, F2, F8 and F9 — the four where the recommendation
> extended or contradicted the sketch — were put to the user and chosen
> explicitly; F3–F7 were stated with their recommendations in the same
> exchange and not contested.

- **F1 — the source of the list: GitHub API JSON, or the releases HTML.**
  The sketch asked for the HTML page. Measured (§3.6), the API is 8× smaller,
  documented, and the only route that carries a **sha256 per asset**, which R3
  needs and §3.3 says a pin table cannot supply. Its one cost is 60
  requests/hour per IP.
  → **Recommendation: the API**, with the rate-limit error spelled out in
  words and an `Authorization` header when `GITHUB_TOKEN` is set. HTML
  scraping stays in §8 as the fallback to build only if the limit is ever hit
  in practice. *(Chosen by the user, 2026-09-09.)* **Amended 2026-09-17:** the
  `GITHUB_TOKEN` header was removed — reading a credential from a variable the
  user never named contradicts PRIVACY.md §3
  ([public-release-readiness.md](public-release-readiness.md) §3.4, F3).
- **F2 — which build, by default.** (a) the newest `bNNNNN`; (b) the tag named
  by `nightly-tag.txt` inside the newest semver release — upstream's own
  blessed nightly, five days behind at the time of writing; (c) a tag pinned
  in our source, bumped by hand like wasmer's.
  → **Recommendation: (a) newest, with `--build` to pin.** (b) is tempting
  and undocumented — the file's contract is upstream's business and could
  change without notice; (c) contradicts R1. *(Chosen by the user, 2026-09-09.)*
- **F3 — `setup` with no `--backend`.** (a) print the list, exit `2`; (b)
  default to `cpu`; (c) guess from the hardware.
  → **Recommendation: (a).** (b) hides a decision that costs between 18 MB and
  645 MB; (c) needs GPU detection the codebase does not have (§8).
- **F4 — the CUDA runtime.** (a) always fetched with a `cuda-*` backend,
  `--no-cudart` to opt out; (b) a `--with-cudart` opt-in; (c) never, documented.
  → **Recommendation: (a).** §3.4: without it the install silently runs on the
  CPU, which R4 exists to prevent.
- **F5 — resume.** (a) `.part` + `Range` resume + rename; (b) `.part` +
  rename, no resume; (c) the precedent's truncate-and-retry.
  → **Recommendation: (a).** The range support is measured, the code is short,
  and 645 MB restarted from zero on a dropped connection is the difference
  between a feature and a grievance.
- **F6 — what `--set-binary` writes.** (a) `engine.managed.binary` always, the
  impersonation and embed paths only when empty; (b) the assistant's only;
  (c) all three, overwriting.
  → **Recommendation: (a).** One install genuinely serves all three (§2.2),
  and never overwriting a path the user typed is the conservative half.
- **F7 — does `--set-binary` also set `engine.mode = managed`?** (a) yes when
  the mode is still at its default and no external URL is configured; (b) no,
  print "switch to managed mode in the settings"; (c) always.
  → **Recommendation: (a).** It matches `MINDFORK_LLAMA_BIN`, which already
  implies the mode (`main.rs:775`), and (b) leaves a user with a correct
  setting and no engine.
- **F8 — the noun.** `mindfork llama …` or `mindfork engine …`.
  → **Recommendation: `llama`.** "Engine" is the app's abstraction over four
  providers; this command is about one of them, and `sandbox` set the
  precedent of naming the thing being provisioned. *(Chosen by the user, 2026-09-09.)*
- **F9 — stages.** (a) one PR: discovery, `backends`, `setup`, `installed`,
  `--set-binary`; (b) two — everything but `--set-binary` first, the settings
  write second.
  → **Recommendation: (b) as two stages on one branch** if the user prefers
  (AGENTS.md §2's "one PR = one stage" has bent before for a small additive
  stage 2); the settings write is ~60 lines and one extracted helper, and
  splitting it into its own PR buys little. *(Chosen by the user, 2026-09-09: two stages, one branch.)*

## 7. Tests and the live run

**Unit (offline, the pure core — the `archive_for` pattern,
`sandbox_setup.rs:621`).** The three real asset-name sets of §3.3 go in as
fixtures:

- backend derivation on `b10883` × {windows, linux} × {x86_64, aarch64} —
  including `ubuntu-x64 → cpu` and the arm64 rows;
- the skip rows: `ui`, `xcframework`, `android-arm64`, `ubuntu-s390x`,
  `310p-openEuler-x86`, `910b-openEuler-x86-aclgraph`, `macos-arm64-kleidiai`;
- the drift rows: `b6000`'s `.zip` extension for ubuntu, `win-hip-radeon-x64`
  surfacing as backend `hip-radeon`;
- `cudart` pairing found / absent → refusal;
- `digest: "sha256:<hex>"` parsing, and a mismatch producing a localized error
  (the en/ru assertion pattern of `sha_mismatch_error_is_localized`);
- the "strip one shared root component" rule against a synthetic tar (with a
  root) and a synthetic zip (flat), built in memory as
  `extract_targz_roundtrip` and `unpack_wheel_extracts_into_site_packages`
  already do;
- zip-slip refusal.

**`#[ignore]` smokes.**

- `live_the_newest_build_still_names_a_cpu_backend` — one API request; asserts
  the shape of §3.3 still holds. This is the test that fails instead of a user
  when upstream renames something (the `the_python_package_is_pinned_exactly`
  precedent).
- `live_install_cpu_into_a_tempdir` — the whole path end to end on the
  cheapest asset (18 MB): resolve, download, verify, unpack, then
  `--version` and assert the build number equals the tag's.

**Live run (mandatory — this touches the engine).** On the Windows stack:
`llama backends`, then `llama setup --backend cuda-12.4 --set-binary`, then a
real chat turn through the managed server the app launches at the path it just
wrote; and `--list-devices` reporting the 4090. On Linux (CI or the LAN box):
the same with `cpu`, to exercise the tar path, the `0755` bits and `$ORIGIN`.
Both recorded in [journal/engine.md](../journal/engine.md) in the "Smoke — GO"
shape.

## 8. Not in this track (recorded so they are not re-derived)

- **GPU auto-detection.** Nothing in `src/` knows about CUDA, Vulkan or ROCm
  (grep: zero hits); GPU-ness is inferred from the user's `gpu_layers`
  (`managed.rs:105`). Recommending a backend needs a detector, and F3 says the
  choice is the user's anyway.
- **A button in the settings screen.** The rows are free text with no picker
  (§2.2); an "install an engine" action there is a UI track of its own.
- **Downloading models.** The GGUF is the other half of a working managed
  setup, and Hugging Face is a different API with different auth. Adjacent,
  bigger, separate.
- **`llama remove` / pruning old installs.** `installed` makes the cost
  visible; deleting a directory is a thing a user can do, and an automatic
  prune of the build you were about to roll back to is a trap.
- **HTML scraping as the discovery route** — F1's loser, kept written down.
- **macOS.** The archives exist; the project targets Windows and Linux
  (CLAUDE.md), and the OS map has the row ready when that changes.
- **Adjacent finding: spec §3.4 over-promises the binary lookup.** It says the
  path is looked up "in `PATH` and next to the application binary"; only
  `PATH` is implemented (`managed.rs:303`). With `data/llama/` existing, "next
  to the application" acquires a real meaning — either implement it or correct
  the spec, in its own small PR.

## 9. Documentation touch list (AGENTS.md §4)

| What | Where |
|---|---|
| journal entry (implementation PR) | [docs/journal/engine.md](../journal/engine.md) + its `## Entries` index; test count + date in CLAUDE.md's Status |
| the command, sizes, backend choice | [docs/install.md](../install.md) §3 — a new §3.x mirroring §4.1's Python-sandbox block |
| user-visible: a new CLI command | [CHANGELOG.md](../../CHANGELOG.md) `[Unreleased]` → Added; [README.md](../../README.md) command list |
| what the app downloads and verifies | [SECURITY.md](../../SECURITY.md) — the paragraph that already covers `sandbox setup` |
| behaviour and contracts | [spec.md](../../spec.md) §3.4 (managed lifecycle), §12.1 (configuration) |
| module map | [docs/architecture.md](../architecture.md) §6 (engine layer), §12 (entry point, CLI) |
| the dev-mode `target/data` trap | [docs/lessons.md](../lessons.md) |
| Windows installer, if a checkbox is added later | `packaging/windows/mindfork.iss`, `privacy.rtf` — out of scope here, recorded |
| gates | `python tools/link_check.py`, `python tools/cyrillic_scan.py`, `python tools/doc_index_check.py` |
