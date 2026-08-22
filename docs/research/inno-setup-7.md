# Inno Setup 7 for the Windows installer

Status: **accepted 2026-08-22** — all five forks decided by the user (F1a, F2a,
F3**b**, F4**b**, F5**b**: migrate, pin the official release asset by hash, build a
**64-bit** setup, take Inno 7's new caption wording, and let the script require 7).
**Implemented** in `feat/inno-setup-7`, with the wizard driven and screenshotted
on the real 64-bit build (§6).
Date: 2026-08-22.

Subject: whether
[`packaging/windows/mindfork.iss`](../../packaging/windows/mindfork.iss) should
move from the Inno Setup 6 line to Inno Setup 7. Prior art:
[docs/history/installers.md](../history/installers.md) §3, which chose Inno
Setup 6.7.x in July 2026 and listed "Inno Setup 7 — just released, not on the
GHA image" as the reason to defer;
[docs/journal/release.md](../journal/release.md) (the installer entries),
[docs/branding.md](../branding.md) §4.1 (the wizard images).

## 1. Short answer

**The script needs no changes at all** — it compiles unmodified on Inno Setup
7.1.0 and produces an installer that behaves identically, verified down to the
bytes of the file it writes (§3). So the migration is not a script question.
(The one directive the script did gain, `SetupArchitecture=x64`, is a decision
taken *on top of* the migration — F3 — not something Inno 7 asked for.)

It is a **CI supply question**, and there the finding is the interesting one:
the chocolatey `innosetup` package our two workflows install has been **stalled
at 6.7.1 since 2026-02-17**, while upstream has shipped 6.7.2, 6.7.3, 7.0.2 and
7.1.0 since (§4). We are already compiling releases with a compiler two
maintenance releases behind the 6 line, and the GitHub runner's preinstalled
copy is the same 6.7.1 from the same stale package. Fixing that requires
replacing the `choco install` step **whichever line we stay on** — and once the
step is replaced, Inno 7 costs nothing extra over Inno 6.7.3.

That is the whole case for the move. There is no feature in 7 this installer
needs today (§5).

## 2. Versions (2026-08-22)

| Where | Version | How it gets there |
|---|---|---|
| Upstream stable | **7.1.0** (2026-08-12); 6.7.3 (2026-05-26) on the 6 line | jrsoftware.org / the `jrsoftware/issrc` GitHub releases |
| This dev machine | 6.7.3 (`C:\Program Files (x86)\Inno Setup 6`) **and** 7.1.0 64-bit edition (`C:\Program Files\Inno Setup 7`) | installed side by side, which upstream supports and which is what made the A/B in §3 possible |
| chocolatey `innosetup` | **6.7.1**, published 2026-02-17 | what `release.yml` and `packaging.yml` install today |
| GHA `windows-latest` image | **6.7.1** | the image installs the same chocolatey package (`toolset-2025.json`, `choco.common_packages`), so its shim is the same build |
| winget `JRSoftware.InnoSetup` | 7.0.2, 7.1.0 (under a `7` manifest folder), plus the whole 6.x line | — |

There is no `innosetup7` package id on chocolatey, and no 7.x under the
existing id. The community package is an "automatic" one that tracks
`files.jrsoftware.org` with per-major streams, so a 7.x publish is plausible
eventually — but the 6 line stopping at 6.7.1 in February says the automation
is not currently running, so "wait for chocolatey" has no date on it.

**Licensing is unchanged.** `license.txt` is **byte-identical** between the
installed 6.7.3 and 7.1.0 (`diff` clean): the same permissive terms, commercial
use included. Both `ISCC.exe` binaries print the same `Non-commercial use only`
line under the banner — that is the 6.5.0-era optional-commercial-license
notice, not a new restriction in 7.

## 3. What was measured

All of it on this machine, against the current `mindfork.iss` with **no edits**,
`/DAppVersion=9.9.9` and a stub `mindfork-rs.exe` (the same trick
`packaging.yml` uses).

**Compile.** Exit 0 on 7.1.0, **zero warnings**, no deprecation notices. The
`[Code]` section compiled unchanged.

**Output.** 3 129 233 bytes (6.7.3) vs 3 129 594 bytes (7.1.0) — **+361 bytes**.
Both are 32-bit PE images (`machine 0x014C`): `SetupArchitecture` defaults to
`x86` in 7, so a plain recompile does *not* silently change the installer's
bitness.

**Runtime parity**, silent install into a temp directory
(`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=… /LOG=…`) from each build, then
its own uninstaller:

* the installed tree is identical — the same 17 files, the same layout,
  `data\dictionaries\` included;
* `defaults.json` is **byte-identical**: 46 bytes, `EF BB BF` BOM,
  `{"mode":"system","default_language":"en"}`. This is the one artefact the
  installer's Pascal code actually produces, and it is the app's contract, so
  it is the byte comparison worth having;
* the uninstaller removes the directory completely in both cases.

**The only differences are in the install log**, and both are documented 7.0
changes: paths are logged in extended-length form
(`Dest filename: \\?\C:\…`) and file time stamps are logged in UTC ISO-8601
(`2026-08-22T16:20:18.646Z`) rather than local time. The end user still sees
normal paths.

**A 64-bit installer** needs exactly one added directive: with
`SetupArchitecture=x64` and nothing else changed, the compile is clean, the
image is `machine 0x8664` — and it grows to 3 875 751 bytes, **+746 KB
(+23.9 %)**, which is the whole cost of the option (§7, F3).

**ISCC's new `--no-compression` (`-nc`)** takes the full compile from **2.00 s
to 0.63 s** — relevant only to `packaging.yml`, whose job is a syntax check that
does not need the payload compressed at all.

## 4. The CI change, and one trap

Today both workflows do:

```
choco install innosetup --no-progress -y
$iscc = (Get-Command ISCC.exe).Source           # …then a Program Files glob as fallback
```

Three things are wrong with that as of today, independent of the 6-vs-7 question:

1. **It installs what is already there.** The runner image already carries the
   same chocolatey package, so the step is a network round trip for nothing.
2. **It pins nothing.** `choco install innosetup` is "whatever is latest",
   which for a release-signing-adjacent tool is a supply-chain shrug. It
   happens to be frozen at 6.7.1, which is why nobody noticed.
3. **It is two maintenance releases behind** the 6 line it is meant to be on.

**The trap**, if we install Inno 7 without touching the resolution: chocolatey
puts a shim for `ISCC.exe` on `PATH`, so `Get-Command ISCC.exe` keeps resolving
to the preinstalled **6.7.1**, the `Program Files*` glob never runs, and the
release would go on being compiled by Inno 6 while the workflow log says it
installed 7. Any migration must **pin the absolute ISCC path** and, ideally,
assert its version — `ISCC.exe --version` prints `7.1.0` and exists only in 7,
which makes the assertion free.

The replacement step (F2a), for both `release.yml` and `packaging.yml` — sketched
here, and shipped as [`tools/install_inno.ps1`](../../tools/install_inno.ps1) so
that the pin lives in one place:

```powershell
$url = 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe'
$sha = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
Invoke-WebRequest $url -OutFile inno.exe
if ((Get-FileHash inno.exe -Algorithm SHA256).Hash -ne $sha.ToUpper()) { throw 'Inno Setup hash mismatch' }
Start-Process .\inno.exe -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-' -Wait
$iscc = 'C:\Program Files\Inno Setup 7\ISCC.exe'
if ((& $iscc --version) -notlike '7.*') { throw 'unexpected ISCC version' }
```

The download is 14 304 168 bytes; the release also carries an `.issig`
signature file next to it, and 7.0.2 added a documented *Verifying Inno Setup
Downloads* page — a pinned SHA-256 of a specific release asset is the cheaper
half of that and is what the snippet uses. Version bumps become a deliberate
two-line edit, which is the point.

## 5. What Inno 7 changes that could touch this script

Filtered from the 7.0/7.1 release notes to things our script actually meets.
Everything else in the notes (64-bit Pascal type widths, `EnableFsRedirection`
removal, type-library registration, `.cab`/`.zst` archive extraction, DLL
loading) is unreachable from a script with no DLLs, no archives, no registry
work and no external calls — which is exactly what ours is.

**Default changes:**

* **`AppVerName` now defaults to `AppName AppVersion`** instead of the
  localized `AppName version AppVersion`. We do not set it, so the wizard
  caption changes from *"Setup - mindfork-rs version 0.9.7"* to *"Setup -
  mindfork-rs 0.9.7"*. This is the **only user-visible difference** the
  migration produces (F4). One line restores the old wording:
  `AppVerName={cm:NameAndVersion,mindfork-rs,{#AppVersion}}`, localized in both
  our languages.
* **`TimeStampsInUTC` now defaults to `yes`** — installed files keep UTC time
  stamps rather than being shifted to the build machine's local time. No entry
  of ours uses `touch` or timestamp comparison (every `[Files]` line is
  `ignoreversion`), so this is invisible outside a file listing, and the new
  default is the more correct one.
* `ArchiveExtraction` gained an `auto` default — no archives here, no effect.

**New, and relevant only if we want it:**

* `SetupArchitecture=x64` (§3, F3) — high-entropy ASLR and a bigger LZMA
  dictionary, at +746 KB.
* Extended-length path support throughout Setup and Uninstall. Our reachable
  paths are short (`%LOCALAPPDATA%\Programs\mindfork-rs`), but the "Data
  location" page lets the user type an arbitrary directory, and that is the one
  place where MAX_PATH was ours to hit.
* The `stellar` wizard style, alongside the `WizardBackColor` /
  `WizardBackImageFile` work that already landed in 6.7.0. A branding option
  ([docs/branding.md](../branding.md) §4.1), not a migration argument.
* ISCC gained `--no-compression`, `--no-signing`, `--version` and
  `--messages-jsonl`. Only the first two are of any use to us, and only in the
  syntax-check job.

**Security hardening we get for free:** the compiler now rejects an `.iss`/`.isl`
containing bytes invalid in its code page (ours is UTF-8 with a BOM and compiles
clean), and `.isl` message files may no longer carry `#include` or ISPP
directives — we use the two stock ones, `compiler:Default.isl` and
`compiler:Languages\Russian.isl`, so this costs nothing and closes a path we
would never have noticed.

**Pascal API:** the script uses `CreateInputOptionPage`, `CreateInputDirPage`,
`ActiveLanguage`, `IsAdminInstallMode`, `WizardDirValue`, `ExpandConstant`,
`FileExists`, `AddBackslash`, `StringChangeEx` and `SaveStringsToUTF8File`.
None is changed, deprecated or removed in 7. The functions 7 *did* remove are
the file-system-redirection ones, which we never called.

## 6. The GUI pass on the 64-bit build

Compiler and silent-install evidence says nothing about a wizard, and a wizard
is what this is. So the **x64 setup was driven and captured** with the technique
the sandbox-checkbox entry left behind in
[docs/journal/release.md](../journal/release.md) — `PostMessage(BM_CLICK)` to
advance, `PrintWindow` to capture — against the real build, nothing installed
(the wizard was killed before the Ready page, and its temp directories removed).

What it showed, on Inno Setup 7.1.0, `SetupArchitecture=x64`:

* **The caption is the F4 evidence**: `Setup - mindfork-rs 9.9.9` in English and
  `Установка — mindfork-rs 9.9.9` in Russian <!-- cyrillic-ok: the ru window caption, quoted -->
  — the new `AppVerName` default, the word "version" gone, taken as chosen.
* The **licence page** renders the plain-ASCII `LICENSE` in English, and
  `license-ru.rtf` in Russian: the "unofficial translation" first paragraph, the
  em dashes and the guillemets all intact. That is the answer for both generated
  RTFs — the same RichEdit draws the disclaimer page, and the licence is the
  longer document of the two.
* **Both custom Pascal pages** draw correctly — "Application language"
  (English first, English selected) and "Data location" with all three options,
  the portable one included, as a per-user run should show.
* The **PNG `WizardSmallImageFile`** renders in the page header, and the
  embedded icon in the title bar, on a 64-bit setup binary — the one branding
  claim that was asserted rather than measured on 7.

Two things the pass deliberately did not cover, because nothing in this change
can reach them: the upgrade path (`defaults.json` present → the three pages
skipped) and the elevated per-machine run (portable hidden). Both are pure
`[Code]` logic, unchanged, and both were exercised by the silent installs.

A note for the next such run, on top of the two the journal already carries:
control text carries its **accelerator ampersand** (`I &accept the agreement`,
`&Next`), so a `-like` pattern written from the screenshot matches nothing; and
the capturing process must call `SetProcessDPIAware` first, or `GetWindowRect`
returns logical pixels while `PrintWindow` renders physical ones and the capture
comes back cropped.

## 7. Forks

**F1 — Do we migrate?**

* **(a) [recommended]** Move to Inno Setup 7.1.0. The CI supply has to be
  rebuilt regardless (§4), 7 rides along for free, and the 7 line is where
  upstream's work happens — 6.7.2 and 6.7.3 were explicitly *backports from 7*.
* (b) Stay on the 6 line, but pin 6.7.3 the same way. Defensible: nothing needs
  7. It buys a compiler that is current on a line in maintenance.
* (c) Wait for chocolatey to ship 7.x. No date; the package has been frozen for
  six months.

**User's decision (2026-08-22): (a).**

**F2 — How does CI get the compiler?**

* **(a) [recommended]** Pinned download of the official GitHub release asset +
  SHA-256 check + silent install + absolute ISCC path (§4). Reproducible,
  auditable, one edit to bump.
* (b) `winget install JRSoftware.InnoSetup.7`. Fewer lines, but winget on the
  Actions runners is historically unreliable in non-interactive sessions and
  pins nothing.
* (c) Keep chocolatey. Impossible for 7, and stale for 6.

**User's decision (2026-08-22): (a)** — implemented as
[`tools/install_inno.ps1`](../../tools/install_inno.ps1) rather than inline in
both YAMLs, so the pinned version and hash exist once: a gate compiling with a
different Inno Setup than the release is a gate that has stopped testing the
release.

**F3 — 32-bit or 64-bit setup binary?**

* **(a) [recommended]** Keep the default 32-bit installer. It installs a
  64-bit application either way (`ArchitecturesAllowed=x64compatible`,
  `ArchitecturesInstallIn64BitMode=x64compatible`), and 746 KB on a 3.1 MB
  download is a poor trade for ASLR on a process that lives for a minute.
* (b) `SetupArchitecture=x64`, if we ever meet an organization that requires
  64-bit executables. It is a one-line change we can make later; nothing about
  choosing (a) now closes it.

**User's decision (2026-08-22): (b)** — against the recommendation, and for a
reason the recommendation had not weighed: the program is 64-bit, so the
installer should be too, to **cut off an attempt to install it on an
unsupported OS**. Worth recording precisely, because the two refusals are not
the same refusal. `ArchitecturesAllowed` already refuses — from inside a wizard
that has started, on the page after the user has read a licence. A 64-bit setup
does not start at all there: Windows itself declines to load the image. The
cost is the +746 KB measured in §3, and what is bought is that the wizard can
never get as far as drawing a page on a machine that cannot run the program.

**F4 — The wizard caption wording.**

* **(a) [recommended]** Pin the old wording with
  `AppVerName={cm:NameAndVersion,mindfork-rs,{#AppVersion}}`, so the migration
  changes nothing a user can see and stays a pure infrastructure change. Drop
  the pin later if we prefer the shorter caption — as a deliberate choice, not
  as a side effect of a toolchain bump.
* (b) Accept the new default.

**User's decision (2026-08-22): (b).** Verified by eye afterwards (§6): the
caption reads `Setup - mindfork-rs 9.9.9`, and Russian.isl's own separator makes
it `Установка — mindfork-rs 9.9.9`. <!-- cyrillic-ok: the ru window caption, quoted -->

**F5 — Does the script stay compilable on Inno 6?**

* **(a) [recommended]** Yes. Nothing we adopt requires 7 (unless F3b), so a
  contributor with 6.7.x installed can still build the installer locally. The
  header comment changes from "compatible with Inno Setup 6.7.x" to naming both
  lines, and CI is the one that pins 7.
* (b) Adopt 7-only directives and require 7 for a local build.

**User's decision (2026-08-22): (b)** — which F3b decides on its own:
`SetupArchitecture` is a 7 directive, so the script is 7-only whatever F5 said.
Inno 6 now refuses it by name — *Unrecognized [Setup] section directive
"SetupArchitecture"* — which is the failure worth having: a contributor on 6
gets told what is missing, rather than a setup.exe that quietly differs from the
released one. `tools/install_inno.ps1` is also how they get the right compiler.

Not a fork, a consequence: **CI must pin the absolute `ISCC.exe` path** and
assert the major version, or the migration silently does nothing (§4).

## 8. What the change came to

Estimated at half a day, and that is roughly what it was, most of it the GUI
pass. What landed:

* **[`tools/install_inno.ps1`](../../tools/install_inno.ps1)** — the pin (version,
  URL, SHA-256), the verified download, the silent install, the version
  assertion, and the absolute ISCC path returned rather than searched for.
  Idempotent, so it is also how a developer gets the exact compiler the releases
  use. The repository's first PowerShell script; `cyrillic_scan.py` covers it
  already, since that gate skips by extension and `.ps1` is not on the list.
* **`release.yml` and `packaging.yml`** — the `choco install` step and both
  copies of the `Get-Command ISCC.exe` resolution replaced by that script plus
  `& $env:ISCC`.
* **`mindfork.iss`** — one directive (`SetupArchitecture=x64`) and comments: why
  the setup is 64-bit, why the `Architectures*` pair stays explicit beside it,
  and that the script now needs Inno 7.
* **Docs** — this section, a [journal entry](../journal/release.md), the
  CHANGELOG (the installer is 64-bit now, which is user-visible), and one
  sentence in [docs/install.md](../install.md). [docs/branding.md](../branding.md)
  §4.1 was left alone on purpose: its Inno 6 statements are a **dated record of
  what was verified on 2026-07-18**, and rewriting a measurement's subject after
  the fact is how a verification log stops meaning anything. The PNG claim is
  re-measured here instead (§6).

## 9. Sources

Primary, all consulted 2026-08-22: the 7.1.0 `whatsnew.htm` shipped with the
local install (identical in size to the release asset), `license.txt` from both
installs, `ISCC.exe` on both lines, the compiled installers and their install
logs. Online: [jrsoftware.org/isdl.php](https://jrsoftware.org/isdl.php),
the [`jrsoftware/issrc` releases](https://github.com/jrsoftware/issrc/releases),
[TimeStampsInUTC](https://jrsoftware.org/ishelp/topic_setup_timestampsinutc.htm),
the chocolatey OData feed for `innosetup`, the `microsoft/winget-pkgs`
manifests, and `actions/runner-images` (`toolset-2025.json`, the Windows 2025
software report).
