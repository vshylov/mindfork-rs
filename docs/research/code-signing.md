# Windows code signing, and the privacy policy that comes with it

Status: **research, forks open** — F1–F8 await the user's decision.
Date: 2026-09-01.

Subject: how `mindfork.exe` and `mindfork-rs-vX.Y.Z-x86_64-setup.exe` get an
Authenticode signature; whether accepting donations costs the project the free
certificate; and what the privacy policy that the signer, the donations and the
EU regulator each ask for must actually say.

Prior art: [docs/history/installers.md](../history/installers.md) §5 — the
July 2026 survey of the signing landscape and its recommendation **R8**, which
the user deferred with a precise reason: *private repo, no site/icon*. All three
have since changed — the site is live, the branding shipped, and the repository
is about to go public — so this document does not repeat that survey. It records
what moved since (§2), reads the signer's conditions in full **against this
repository** (§3), answers the two questions the deferral never had to face
(§4, §5), specifies the pipeline change with the traps found by measurement
(§6–§7), and derives the privacy policy from what the code actually does (§8).
Neighbours: [inno-setup-7.md](inno-setup-7.md) (the installer's compiler),
[docs/journal/release.md](../journal/release.md) (the packaging entries),
[docs/roadmap.md](../roadmap.md) (where signing sits as groundwork).

## 1. Short answer

**SignPath Foundation, and donations do not endanger it.** Their conditions
prohibit four things — malware, a non-OSI licence or commercial dual-licensing,
proprietary components (*especially* ones published by the maintainer), and
security-scanning/hacking tools. Donations, sponsorship, Patreon and GitHub
Sponsors appear nowhere in the conditions or in the code of conduct, and their
own project list is full of donation-funded projects (Espanso, Playnite, Heroic
Games Launcher, sniffnet, Starship, OrcaSlicer, Zen, Feather Wallet, Mudlet,
OpenCode). What *would* end eligibility is a closed "pro" build, a paid feature
in closed code, or dual licensing — none of which is on this project's roadmap.

The work is smaller than the survey implied, and it is not in the signing call:

- **two signing requests** in [release.yml](../../.github/workflows/release.yml),
  at the two points where a PE file already exists and nothing downstream has
  hashed it yet (§6.1) — the job graph already has the right shape;
- **the metadata is wrong today, and measurably so** (§6.2): the built `.exe`
  says `ProductName = mindfork-rs`, the installer will say `mindfork` with
  version `0.0.0.0`. SignPath enforces product name and version through file
  metadata restrictions, so this is a blocker, not a polish item;
- **a "Code signing policy" page** on mindfork.io, with wording they dictate (§7);
- **a privacy policy**, which the same page has to link (§8).

The one genuine complication from donations is not SignPath's — it is the Cyber
Resilience Act's definition of commercial activity (§5), and it is about how the
donations are *framed*, not whether they exist.

The unwelcome part of the answer came from writing the privacy policy rather
than from the signer: inventorying every outbound connection in the code turned
up **two sentences in [SECURITY.md](../../SECURITY.md) that the code does not
keep** — the web tools ship *on*, not "when you explicitly enable" them, and one
sandbox download is not covered by the sha256 lock list (§3.2). Both sit under
the condition about transfers to systems the user did not specify, so they are
worth settling before a stranger reads the repository with the terms in hand
(F8).

## 2. What moved since the July survey

| Since | Then (installers.md §5) | Now (2026-09-01) |
|---|---|---|
| Microsoft's own guidance | recommends SignPath for OSS | unchanged, and the article was refreshed 2026-08-29 — the OSS section still names SignPath Foundation by name |
| Trusted Signing | "$9.99/mo, individuals US/Canada only; geography promised to expand — recheck" | **rechecked: it did not expand.** Renamed **Azure Artifact Signing**; organizations USA/Canada/EU/UK, **individuals still USA and Canada only** |
| EV vs OV | EV lost instant SmartScreen reputation | unchanged and now stated flatly by Microsoft: paying the EV premium for SmartScreen alone is not justified |
| Certum Open Source | ~€69 first year, individuals almost anywhere | still sold; €49 for the SimplySign cloud variant, from €25 as an electronic code for an existing card |
| The two blockers on R8 | private repo, no site, no icon | **all three gone** — mindfork.io is live, branding shipped, the repo goes public |

So the decision tree in installers.md §5.5 resolves on its first branch — public
repository → SignPath Foundation — and the alternative it named for the second
branch (Certum, "the only cheap path in your own name") stays the fallback if
the application is refused (F1).

## 3. Their conditions, against this repository

Read in full at [signpath.org/terms](https://signpath.org/terms). Only the rows
that require an action or a judgement are listed; the rest (no malware, provide
uninstallation, announce system changes) are satisfied by what the project
already is.

| Condition | Where we stand |
|---|---|
| OSI licence, no commercial dual-licensing | MIT, single-licensed — [LICENSE](../../LICENSE) ✔ |
| No proprietary component, *especially* from the maintainer | nothing closed ships, and this is the row donations must not break (§4) — but the redistributables need one gap closed first (§3.1) → |
| Actively maintained, already released in the form to be signed | 2740 tests, releases exist ✔ (the releases become *public* with the repo) |
| Documented on the download page | mindfork.io + [README.md](../../README.md) ✔ |
| No hacking tools — no scanning for or exploiting vulnerabilities of the execution environment | mindfork is an agent, not a scanner: `python_exec` is sandboxed, `fs_*` optionally jailed, `web_search`/`fetch_url` closed to non-routable addresses. Their list already carries close relatives (OpenCode, better-ccflare, shob), so the category is accepted — but the application should say this rather than let a reviewer guess (§9, F2) |
| Respect user privacy; software that transfers user data to systems **not specified by the user** needs a privacy policy, shown during installation, with an option to disable | the comfortable answer — "everything goes where the user pointed it" — turns out to be **false for one subsystem** (§3.2) → |
| MFA on GitHub and SignPath | to confirm before applying → |
| Team roles: Authors / Reviewers / Approvers | a solo maintainer states himself in all three; the page has to say so → |
| "Code signing policy" on the home page, with their attribution line, roles and privacy statement | **to write** (§7) → |
| Product name and version attributes set and enforced | **broken today** (§6.2) → |
| Every release approved manually; artifacts built from source verifiably | approval is a human step in the release ritual (§6.3); the build is already reproducible from a tag → |
| For OSS: every job up to the signing request runs on GitHub-hosted agents | `release.yml` uses `ubuntu-22.04` and `windows-latest` throughout ✔ |
| Certificate is issued to **SignPath Foundation** — they are the publisher | accepted cost: dialogs will say "SignPath Foundation", not "Vladimir Shylov" (F1) |

The soft one is the last in their FAQ: for downloadable executables they want *a
certain verifiable reputation* — they will not sign code nobody knows. This is
the only condition that argues for sequencing (F2).

### 3.1 The dictionaries have no provenance record

Checking the "no proprietary component" row turned up something the project
already knows how to do and did not do here. The vendored grammars are exemplary
— [syntaxes/SOURCES.md](../../syntaxes/SOURCES.md) pins, per file, the upstream
repository, the exact commit, the licence and the path to the vendored licence
text, and `tools/fetch_syntaxes.py --check` verifies the working copy against
those pins.

The spellcheck dictionaries have nothing of the sort. `dictionaries/` holds six
files and no record: `en_GB.aff` happens to state "Released under LGPL" in its
own header, `en_US.aff` and `ru_RU.aff` say nothing at all, and no document names
where any of them came from. That is a redistribution question with or without a
signer — these files ship in every archive, every Linux package and the Windows
installer — and it is exactly the question a reviewer asks when the condition
they are checking is "no component that is not open source".

The fix is the pattern that already exists: a `dictionaries/SOURCES.md` with
upstream, commit or release, licence and vendored licence text per dictionary.
Small, independent of everything else here, and a prerequisite to applying (§10).

### 3.2 The clause that does bite: the web tools ship on

Writing the privacy policy meant inventorying every outbound connection in the
code rather than trusting the summary in SECURITY.md, and the inventory
disagrees with the summary twice. Both matter here, because the row above turns
on exactly this question.

**`tools.web_enabled` defaults to `true`.** On a fresh installation a model can
call `web_search` and `fetch_url` with nothing enabled by the user, and
`web_search` queries a chain the *app* chose — DuckDuckGo, Mojeek, Ecosia — with
a search string derived from the conversation. Fetching the result pages is on by
default too. Sharper still: the keyed backend is preferred when a key is
available, and the key is looked up in `TAVILY_API_KEY` **by default**, so a user
who exports that variable for an unrelated tool starts sending queries to
`api.tavily.com` without ever making a decision inside the app. SECURITY.md says
the app reaches search engines "when you explicitly enable the web tools", which
is not what the code does.

Read against the condition, those search engines are plainly *not* "systems
specified by the user". Whether a reviewer reads it that way is a judgement — the
transfer only happens inside a conversation the user started, and the data is a
query rather than a corpus — but this is not a place to be clever in an
application whose weakest condition is the reviewer's comfort. It is also the one
clause with a *concrete* remedy attached: describe it, show it at install time,
and offer an off switch during installation. F8 chooses between satisfying the
condition by default (make the switch opt-in) and satisfying it by disclosure.

**The sandbox lock list has a hole.** SECURITY.md also says the one-time Python
sandbox downloads are "verified against a sha256 lock list". The `wasmer` release
archive and all thirteen wheels are. `python.webc` — the largest piece — is not:
the app shells out to the freshly downloaded `wasmer` binary to fetch it from the
Wasmer registry, and the only post-condition is that the file exists. The
known-good digest is even written down in a comment next to the code, and never
compared. That is a supply-chain statement the project makes publicly and does
not fully keep, and it wants fixing on its own merit; for this track it matters
because "no malware" and "respect user security" are conditions someone may check
against the same document.

Neither of these is a signing problem. Both are things to settle *before* a
stranger reads the repository with the terms in hand.

## 4. Donations do not cost the subscription

Read literally, the conditions restrict the *code and the licence*, never the
project's funding. There is no clause about revenue, donations, sponsorship or
commercial intent; the single occurrence of "commercial" is `commercial
dual-licensing`, and the neighbouring clause bans proprietary components. The
code of conduct adds nothing on the subject.

The project list corroborates it: it is full of projects funded by GitHub
Sponsors, Patreon, Ko-fi and Open Collective, and includes several backed by
companies with paid hosted offerings. A donation button has never been the
distinguishing feature of a rejected application.

What actually ends eligibility, and is worth writing down because all three are
easy to drift into once money is involved:

1. **A closed "pro" build or a paid closed-source plugin** — this is the
   "no proprietary code, especially from a maintainer" clause, and it is the
   most likely way a donation-funded project loses the certificate later.
2. **Dual licensing** — MIT for everyone plus a commercial licence for money.
3. **Signing something that is not ours** — a sponsor's binary, an upstream
   build. The certificate covers this project's own artifacts only.

None of these follows from accepting donations; all three follow from
*monetising features*. The distinction is the whole answer.

## 5. Where donations do cost something: the CRA

Not SignPath — Regulation (EU) 2024/2847. Recital 15 defines when supplying
software is a commercial activity, and lists among the characteristics
*"accepting donations exceeding the costs associated with the design,
development and provision of a product with digital elements"*, immediately
followed by: *"Accepting donations without the intention of making a profit
should not be considered to be a commercial activity."*

Charging for technical support beyond cost recovery is on the same list, as is
requiring the processing of personal data as a condition of use — mindfork does
neither, and its "no telemetry" promise ([SECURITY.md](../../SECURITY.md)) is
what keeps the third off the table.

Consequences for how the donations are set up, none of them onerous:

- keep them **donations**, not tiers that promise anything in return; a reward
  attached to a payment is a price, and a price is commercial activity;
- do not offer paid support beyond actual costs;
- be able to show that the money covers costs — domain, AWS, the rented live-gate
  GPU hours, provider keys. This project already documents those costs, which is
  a convenient accident.

Timing: the regulation applies from **11 December 2027**, Article 14 (reporting
actively exploited vulnerabilities) from **11 September 2026**, Chapter IV from
11 June 2026. While the project is not monetised it is out of scope as a
non-commercial supply, and none of the manufacturer obligations attach. This is
worth knowing *before* the donation page exists, not after.

## 6. The pipeline

### 6.1 Two signing requests, and where they go

The job graph in [release.yml](../../.github/workflows/release.yml) already
serialises correctly: `build` (windows) → `windows-installer` → `publish`, with
the installer consuming the `bin-windows` artifact and `publish` computing
`sha256sums.txt` last. Two insertions, no restructuring:

1. in `build`, after the release build: upload `mindfork.exe`, submit it, and
   let the **signed** exe be what `bin-windows` carries — so the installer
   compiles the signed binary in, and the archives pack it;
2. in `windows-installer`, after `ISCC`: submit `mindfork-rs-vX.Y.Z-x86_64-setup.exe`
   and publish the signed result.

`publish` needs no change: it downloads what the upstream jobs produced, so the
checksums are computed over signed bytes for free. Getting that order wrong is
the classic mistake here — a `sha256sums.txt` that describes pre-signature bytes
is worse than none, because it looks authoritative.

The integration is `signpath/github-action-submit-signing-request@v2`, which
takes the artifact by `artifact-id` from a preceding `actions/upload-artifact`
step (v4+; the workflow is already on v5). Two knobs need a decision at
implementation time and neither is default: `actions/upload-artifact` zips by
default, so either the artifact configuration is rooted at `<zip-file>` or the
upload passes `archive: false` and the signing step `skip-decompress: true`.

### 6.2 The metadata is wrong today — measured

SignPath requires all product-name attributes to be the project's name and all
product-version attributes to be the same value in every build, and enforces
this with file metadata restrictions in the artifact configuration. Both sides
of the release fail that today, for different reasons:

**The binary.** [build.rs](../../build.rs) builds a `winresource` resource only
to attach the icon, and `WindowsResource::new()` fills the rest from Cargo
metadata: `ProductName` and `FileDescription` from `package.name`,
`FileVersion`/`ProductVersion` from `package.version`. Read off the built
artifact:

| Field | Value today | Wanted |
|---|---|---|
| `ProductName` | `mindfork-rs` | the project name, one value everywhere (F3) |
| `FileDescription` | `mindfork-rs` | the app name Windows shows in the UAC dialog — it is *this* field, not `ProductName`, that names the program above "Verified publisher" |
| `FileVersion` / `ProductVersion` | `0.9.8` | unchanged, it is already the single source of truth |
| `CompanyName`, `LegalCopyright`, `OriginalFilename` | **empty** | set — the signature says who vouched for the file, these say whose file it is |

**The installer.** [mindfork.iss](../../packaging/windows/mindfork.iss) sets no
`VersionInfo*` directive at all, and Inno's defaults are not the ones that would
save it: `VersionInfoProductName` falls back to `AppName` (`mindfork` — a
*different* string from the exe's `mindfork-rs`), while `VersionInfoVersion`
defaults to **`0.0.0.0`** and `VersionInfoProductVersion` follows it. So the
setup executable currently ships a zero version and a product name that
disagrees with the binary it installs.

Both are one-line fixes (`res.set(...)` in build.rs, `VersionInfoVersion={#AppVersion}`
and friends in the `.iss`), but they have to happen *before* the artifact
configuration is written, because the restriction pins exactly these strings.
They are also worth doing on their own merit — this is metadata Explorer,
"Apps & features" and every AV heuristic already read.

### 6.3 Manual approval is the trap, not the signing

Every release needs a human approval in SignPath's UI. The action's
`wait-for-completion-timeout-in-seconds` defaults to **600** — ten minutes
between the tag being pushed and someone clicking Approve, or the release job
fails after a successful build. Two ways out (F5): raise the timeout to
something a human release ritual fits in, or split the workflow so signing waits
in its own job. The release checklist in AGENTS.md §6 grows one step either way:
after pushing the tag, approve two signing requests.

### 6.4 What stays unsigned

The Linux packages (`deb`/`rpm`/`pkg.tar.zst`) are not Authenticode's business;
signing them is GPG and a different key story, out of scope here. The archives
are containers — signing the `.exe` inside them is what matters, and that
happens upstream in `build`.

### 6.5 One thing this unblocks

installers.md §5.4: winget does not require a signature but does not bypass
SmartScreen either, which is why the roadmap's winget item is pinned to the
portable zip. Once the setup executable is signed, the winget manifest can point
at the installer, and that groundwork item stops being blocked.

## 7. What the site must carry

A page headed exactly **"Code signing policy"**, linked from the home page and
the download/release page, containing:

- the attribution line, verbatim: *"Free code signing provided by SignPath.io,
  certificate by SignPath Foundation"*;
- the team roles — Authors, Reviewers, Approvers — with their members;
- a privacy statement: either a link to the policy or their canned sentence,
  *"This program will not transfer any information to other networked systems
  unless specifically requested by the user or the person installing or
  operating it"*, which happens to be literally true of mindfork, plus the
  policies of the third-party services a user may configure.

The honest form for this project is both: the canned sentence, because it is
accurate, followed by a link to [PRIVACY.md](../../PRIVACY.md) for the list of
services. On the Zola site that is two `site/content/*.md` pages rendered by the
existing `page.html`, and two entries in the `foot-links` block of
[base.html](../../site/templates/base.html) — which today carries GitHub, the
Atom feed and the copyright line, and is exactly where a reader looks for both.
The home page also needs the words "Code signing policy" to appear as a link,
since that is where their condition points.

## 8. The privacy policy

It is wanted by three separate parties, and they ask for slightly different
things — which is why one document has to serve all three:

1. **SignPath**, as above: a statement, and the third-party services named.
2. **The user**, who is running an agent that can read files, execute code and
   send their conversation to a provider they picked. [SECURITY.md](../../SECURITY.md)
   already promises no telemetry and [DISCLAIMER.md](../../DISCLAIMER.md) §5
   already says the content of a request leaves the machine — but a promise
   spread across two documents that are about other subjects is not a policy.
3. **The donors**, whenever donations start: a donation platform is the
   controller for the payment, but the recipient still sees names, handles,
   amounts and messages, and that is personal data with a purpose and a
   retention period. This is the second, smaller way donations create work —
   and, unlike §5, it is unconditional.

[PRIVACY.md](../../PRIVACY.md) is written in this branch, from what the code
does rather than from a template: what stays on the machine and where, what
leaves it and only on which setting, what is off by default, and what the author
never receives (everything). It is deliberately a repository document first —
the same shape as SECURITY.md and DISCLAIMER.md — so that the site page can
render it rather than fork it.

Not in this branch, and specified here so the follow-up is not invented from
scratch (F6, F7): the `/privacy` and `/code-signing-policy` pages on mindfork.io,
`PRIVACY.md` in the release archive and the installer's `[Files]`, a Russian
translation under `docs/legal/` next to
[DISCLAIMER.ru.md](../legal/DISCLAIMER.ru.md), and — only if the app ever grows
a data-collecting feature — the installation-time page SignPath would then
require.

One deliberate omission belongs on that list too. The obvious place for a
pointer is [DISCLAIMER.md](../../DISCLAIMER.md) §5, which already describes what
leaves the machine — but that file is not a leaf: `tools/wizard_rtf.py` renders
it *and its Russian translation* into the installer's two wizard pages and
verifies the result in CI, and `shared/credits.rs` embeds both for the `F1`
tabs. So one sentence there is four coordinated edits and a regenerated pair of
RTFs, which is a packaging change, not a docs one. It belongs in stage 2, next
to F7 — where the installer is being touched anyway and the answer to "does the
policy ship with the release" is already known. README and SECURITY.md, which
have no such machinery behind them, carry the links today.

## 9. Forks

**F1 — the certificate.**
 (a) **SignPath Foundation** — free, Microsoft-recommended, publisher shown as
 "SignPath Foundation", manual approval per release. *Recommended.*
 (b) Certum Open Source (~€69/€29) — your own name in the subject, but the key
 lives on a card or in SimplySign and CI automation is a workaround; keep as the
 fallback if (a) is refused.
 (c) Azure Artifact Signing — ruled out for an individual outside the US/Canada.
 (d) Stay unsigned — every release yellow, Smart App Control blocks outright.

**F2 — when to apply.**
 (a) **After the repository is public and one or two public releases exist**, so
 that the "verifiable reputation" condition has something to look at, and the
 application can point at the site, the policy page and SECURITY.md. *Recommended.*
 (b) Immediately on going public — faster, and risks a refusal that is awkward to
 appeal ("we generally don't discuss policy").

**F3 — the product name in the metadata.**
 (a) **`mindfork`** — the brand, matching `AppName`, the shortcut and the binary;
 the string a user sees in a UAC dialog. Consistent with the naming rule adopted
 in [binary-rename.md](binary-rename.md): the brand for presentation, the project
 id for file names. *Recommended.*
 (b) `mindfork-rs` — the package name and the archive prefix; today's value, and
 the one the artifact names use.
 Whichever is chosen must be the same in the `.exe` and in the setup, and is then
 frozen by the metadata restriction.

**F4 — what gets signed now.**
 (a) **`mindfork.exe` + the setup executable.** *Recommended.*
 (b) Also GPG-sign the Linux packages — a separate key, a separate track.

**F5 — how the workflow waits for approval.**
 (a) **One workflow, `wait-for-completion: true`, timeout raised** (e.g. 3600 s)
 in both jobs; the release ritual gains "approve two requests". Simplest, and the
 failure mode is a job that waited too long, not a half-published release.
 *Recommended.*
 (b) Split: build → publish unsigned draft → sign in a re-dispatched job. More
 moving parts, no benefit while a single person approves.

**F6 — where the privacy policy lives.**
 (a) **`PRIVACY.md` in the repository, rendered as `/privacy` on the site.**
 One text, two surfaces. *Recommended.*
 (b) Site only — then the repository and the offline installer have no copy.

**F7 — does `PRIVACY.md` ship with the release?**
 (a) **Yes** — one more entry in the `DOCS=(…)` array in `release.yml`'s archive
 step, where README/CHANGELOG/LICENSE/DISCLAIMER and the two Russian
 translations already are, plus the installer's `[Files]`. *Recommended.*
 (b) No — a link on the site is enough. Cheaper, but an offline user has the
 disclaimer and not the policy, which is an odd pair.

**F8 — the web tools' default, and the two SECURITY.md sentences (§3.2).**
 (a) **Make `tools.web_enabled` opt-in**, so that "everything goes where you
 pointed it" becomes true without qualification, and correct the sandbox sentence
 (either verify `python.webc` against the digest already written in the comment,
 or stop claiming it is verified). The condition is then satisfied by design and
 the installer needs no privacy page. *Recommended — the honest default for a
 local-first app, and it removes the only judgement call from the application.*
 (b) **Keep the default and disclose**: PRIVACY.md §3.3 already says plainly that
 the web tools ship on; add the same to SECURITY.md, and add a privacy page plus
 an "enable web tools" checkbox to the installer, which is what the condition asks
 for when transfers to unspecified systems exist. More work than (a), and it
 leaves a reviewer a question to ask.
 (c) Change nothing and apply as is — not recommended: it leaves a published
 promise contradicted by the code in the same repository the reviewer is reading.
 Either way `TAVILY_API_KEY` should stop being a default env name; a key found in
 the ambient environment is not a decision the user made in this app.

## 10. Stages

1. **This branch (docs only).** This document and `PRIVACY.md`, with links from
   README and SECURITY.md and a pointer from the roadmap. No code, no pipeline
   change, no journal entry (AGENTS.md §4 exempts a pure-docs PR). `PRIVACY.md`
   describes the software **as it behaves today**, §3.2 included — so if F8 goes
   to (a), its §3.3 and §4 change with the default.
2. **F8's answer** (§3.2) — whichever of the two it is, this is the only stage
   that touches a published promise, and it should land before the application
   rather than after: either `feat/web-tools-opt-in` plus the sandbox digest
   check, or the SECURITY.md correction plus the installer's privacy page.
3. **`feat/release-metadata`** — the product name, description, company and
   version fields in `build.rs` and `mindfork.iss` (§6.2), plus `PRIVACY.md` in
   the archive and the installer (F7). Independently useful, and a prerequisite
   for the artifact configuration.
4. **`docs/dictionary-provenance`** — `dictionaries/SOURCES.md` in the shape of
   `syntaxes/SOURCES.md` (§3.1). Independent of the rest, and the one item that
   could turn into a question mid-review.
5. **The site pages** — "Code signing policy" and "Privacy" (§7, F6).
6. **The application**, once 1–5 are visible on a public repository (F2).
7. **`feat/code-signing`** — the two signing steps, the artifact configurations,
   the approval step in the release checklist, a journal entry in
   [docs/journal/release.md](../journal/release.md), and R8 closed in the
   roadmap.

Stages 2–5 do not depend on the application's outcome and are worth doing
regardless; stage 7 is the only one that does.

## 11. Sources

Signing: [SignPath Foundation conditions](https://signpath.org/terms) ·
[the signed-projects list](https://signpath.org/projects) ·
[SignPath docs — trusted build systems](https://docs.signpath.io/trusted-build-systems/) ·
[the GitHub connector and its OSS checks](https://docs.signpath.io/trusted-build-systems/github) ·
[Microsoft — code signing options (2026-08-29; recommends SignPath for OSS)](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options) ·
[Microsoft — SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) ·
[Certum Open Source Code Signing](https://shop.certum.eu/open-source-code-signing.html).

Metadata: [Inno Setup — VersionInfoVersion](https://jrsoftware.org/ishelp/topic_setup_versioninfoversion.htm)
and the neighbouring `VersionInfo*` topics (defaults quoted in §6.2);
`winresource` 0.1.31 `lib.rs` (the Cargo-metadata defaults), verified against the
built `target/debug/mindfork.exe`.

Regulation: [Regulation (EU) 2024/2847 (Cyber Resilience Act)](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX%3A32024R2847) —
recital 15 (commercial activity, donations) and Article 71 (application dates) ·
[the Commission's CRA and open source page](https://digital-strategy.ec.europa.eu/en/policies/cra-open-source).
