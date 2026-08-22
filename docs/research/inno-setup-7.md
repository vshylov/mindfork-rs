# Inno Setup 7 for the Windows installer

Status: **research, no decision** — the forks in §7 are open and none has been
implemented. Nothing was changed outside this file.
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

The replacement step (F2a), for both `release.yml` and `packaging.yml`:

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

## 6. What is still unverified

Everything above is compiler- and silent-install evidence. The product here is
a **wizard**, and a wizard is verified by looking at it. Before any migration
merges, one manual GUI run of the 7-built installer, in both languages, is
required — the same checklist [installers.md](../history/installers.md) §8 (stage 3's
definition of done) used for the original track:

* the license page (English `LICENSE`, plain text) and the Russian
  `license-ru.rtf`, then the disclaimer page in each language — the RTF pages
  are where a compiler change could plausibly show up at all, since the
  `\uNNNN?`-escaped Cyrillic is rendered by Setup's RichEdit;
* the two custom pages, all three data modes, and the directory picker;
* the upgrade path (`defaults.json` present → all three pages skipped);
* the per-machine (elevated) run, where the portable option is hidden;
* the PNG `WizardSmallImageFile` in the header, whose acceptance on par with
  BMP was verified against Inno 6 and is asserted, not measured, on 7;
* the wizard caption, which is where F4 is decided by eye.

None of this needs a live model or a live engine — it is packaging.

## 7. Forks

**F1 — Do we migrate?**

* **(a) [recommended]** Move to Inno Setup 7.1.0. The CI supply has to be
  rebuilt regardless (§4), 7 rides along for free, and the 7 line is where
  upstream's work happens — 6.7.2 and 6.7.3 were explicitly *backports from 7*.
* (b) Stay on the 6 line, but pin 6.7.3 the same way. Defensible: nothing needs
  7. It buys a compiler that is current on a line in maintenance.
* (c) Wait for chocolatey to ship 7.x. No date; the package has been frozen for
  six months.

**F2 — How does CI get the compiler?**

* **(a) [recommended]** Pinned download of the official GitHub release asset +
  SHA-256 check + silent install + absolute ISCC path (§4). Reproducible,
  auditable, one edit to bump.
* (b) `winget install JRSoftware.InnoSetup.7`. Fewer lines, but winget on the
  Actions runners is historically unreliable in non-interactive sessions and
  pins nothing.
* (c) Keep chocolatey. Impossible for 7, and stale for 6.

**F3 — 32-bit or 64-bit setup binary?**

* **(a) [recommended]** Keep the default 32-bit installer. It installs a
  64-bit application either way (`ArchitecturesAllowed=x64compatible`,
  `ArchitecturesInstallIn64BitMode=x64compatible`), and 746 KB on a 3.1 MB
  download is a poor trade for ASLR on a process that lives for a minute.
* (b) `SetupArchitecture=x64`, if we ever meet an organization that requires
  64-bit executables. It is a one-line change we can make later; nothing about
  choosing (a) now closes it.

**F4 — The wizard caption wording.**

* **(a) [recommended]** Pin the old wording with
  `AppVerName={cm:NameAndVersion,mindfork-rs,{#AppVersion}}`, so the migration
  changes nothing a user can see and stays a pure infrastructure change. Drop
  the pin later if we prefer the shorter caption — as a deliberate choice, not
  as a side effect of a toolchain bump.
* (b) Accept the new default.

**F5 — Does the script stay compilable on Inno 6?**

* **(a) [recommended]** Yes. Nothing we adopt requires 7 (unless F3b), so a
  contributor with 6.7.x installed can still build the installer locally. The
  header comment changes from "compatible with Inno Setup 6.7.x" to naming both
  lines, and CI is the one that pins 7.
* (b) Adopt 7-only directives and require 7 for a local build.

Not a fork, a consequence: **CI must pin the absolute `ISCC.exe` path** and
assert the major version, or the migration silently does nothing (§4).

## 8. Effort

Half a day, and most of it is the manual GUI pass.

* `release.yml` + `packaging.yml`: replace the install step, pin the path,
  assert the version — ~15 lines each, and the two jobs stay under their
  15-minute ceilings (the download replaces a `choco install` of comparable
  size).
* `mindfork.iss`: 0 lines, or 1 with F4a, or 2 with F3b.
* Docs: the header comment in the `.iss`, the "Inno 6" statements in
  [docs/branding.md](../branding.md) §4.1 (the PNG note, the Welcome-page note),
  a [docs/journal/release.md](../journal/release.md) entry, and a line in
  [docs/roadmap.md](../roadmap.md) if F1 is deferred rather than taken.
* The GUI smoke of §6.

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
