# Research: safe defaults for a public audience (public release, stage 2)

**Status:** measured and designed (§2–§4), 2026-09-17. **Every fork in §5 decided by
the user on 2026-09-17 at its recommendation:** D1(a), D2(a), D3(b), D4(a), D5(b),
D6(a), D7(a), D8(a), D9(a); N1–N5 not contested. **Stage 2a (the file tools)
implemented, live GO** (§6); stage 2b (network and processes) next.

Stage 2 of [public-release-readiness.md](public-release-readiness.md) (§2.2 B10,
§2.3 "Security, beyond B10"). The audit's summary was that a pristine install is safe
and one settings switch later it is not. This document measures what that switch
actually opens, what each child process really needs, and what the sandbox's network
really reaches, and turns the answers into forks.

Claims are tagged **[code]** (read in this repository), **[measured]** (run on this
machine on 2026-09-17: Windows 11, wasmer 7.2.0, Node 24, Python 3, the pinned Rust
1.96.0; Linux facts in a local `mindfork-lab:local` container, Ubuntu 24.04) or
**[unverified]**.

**Related:** spec §9.3 (the address policy), §9.6 (MCP and its `env` map), §9.8
(confirmation), §9.12 (the code workspace), §13 (security);
[ADR 0005](../decisions/0005-python-sandbox-wasmer.md) (the sandbox),
[ADR 0008](../decisions/0008-api-key-storage.md) (keys at rest);
[tool-confirmation.md](../history/tool-confirmation.md) (why confirmation is opt-in);
[background-subagents.md](background-subagents.md) F3 (why a background run never asks);
[fetch-url-address-policy.md](fetch-url-address-policy.md); SECURITY.md.

## 1. The threat this stage is about

A stranger installs the app, enables a tool, and talks to a model that reads something
hostile — a fetched page, an attached file, a file in a project. The model is then
steered by that text. The question is not whether an injected model can do harm with a
tool the user enabled (it can — that is what enabling means), but **how far past the
scope the user had in mind** it can reach. Four overreaches are in scope:

1. **Out of the chosen scope** — files the user never meant to expose.
2. **Into the app itself** — rewriting its own configuration, which is code execution at
   the next launch.
3. **Into the local network** — the host's loopback services and the LAN.
4. **Into secrets the process happens to hold** — API keys in the environment.

Out of scope: a malicious MCP server or `llama-server` binary (software the user chose
to run), and a hostile local user with the same account.

## 2. What was established

### 2.1 The file tools [code]

| Tool | Gate | Marked dangerous | Path policy | Reaches the data root or the binary's directory |
|---|---|---|---|---|
| `fs_read`, `fs_list` | `tools.fs_enabled` | no (and `concurrent()`) | `fs_root`; **empty → the raw path** (`fs.rs:53-55`) | yes, the whole disk |
| `fs_write` | same | yes | same | yes |
| `code_list/read/grep` | an attached project | no | canonical containment in the project (`code.rs:159-210`) | only if inside the project |
| `code_edit/write` | same | yes | same | same |
| `code_build/run/test` | a project + a command line the user typed | yes | none — runs the user's command in the project | runs the project's code |
| `python_exec` | `tools.python_enabled` | yes | Wasmer: the job directory only. Local: whatever the user can reach | Local: yes |

- **`fs_root` is a plain text row** (`screens/settings/catalog.rs:957`): no existence
  check, no warning when the switch is on and the row is empty. Its description says
  "Empty — access to the whole file system" (`locales/en.json:1663`).
- **Profile toggles do not add a second gate.** `enabled_by_default()` is `true` for
  every file, code, Python and web tool (`features/tools/mod.rs:680`), and
  `reconcile_tools` switches it on in every profile (`profiles.rs:55-72`). Only MCP,
  the self-model tools, `chat_search/read` and two control tools start off. SECURITY.md's
  "a global switch plus per-profile toggles" is true in form only.
- **Nothing protects the app's own files.** With `fs_write` enabled (or a project that
  contains the data root — this repository, whose `target/debug/data/` *is* the dev
  root), a model can write, and the app obeys at its next launch:
  an MCP server entry in `settings.json` (spawned when settings apply,
  `orchestrator/mcp.rs:227-252`), `engine.managed.binary`, `python_mode: local` with a
  `python_path`, a profile's system prompt, a chat's workspace command line, or a
  `defaults.json` that moves the data root.
- **A dangling symbolic link writes through the containment check.** Both resolvers fall
  back to "canonical parent + file name" when the path does not exist, and `exists()` is
  `false` for a link whose target is missing, so the link's own name passes the check and
  `tokio::fs::write` follows it out (`fs.rs:69-86,263`; `code.rs:178-201,1060`). A cloned
  repository can carry such a link (`notes.md -> ~/.config/autostart/x.desktop`). Real on
  Linux; on Windows creating one needs a privilege and git defaults to
  `core.symlinks=false`. Confirmed by reading, not run.
- **`code_write` reaches `.git/`** — `.git/hooks/pre-commit` runs at the user's next
  commit. The walker behind `code_list/grep` skips hidden and ignored paths; `resolve`,
  behind read/edit/write, does not.

### 2.2 Confirmation [code]

- **Opt-in by decision F2(b), 2026-07-30** (tool-confirmation.md): "the dangerous tools
  are already behind master switches that are off by default, so the user who turned them
  on has consented once already."
- **A background run never asks, even with confirmation on** — `confirm_dangerous: false`
  is hard-coded (`generation.rs:3584-3595`), by decision F3(c), 2026-09-05. SECURITY.md
  counts bypassing the confirmation as in scope, so this contradicts a published promise.
- **A `concurrent()` tool bypasses the gate by construction** (`generation.rs:2632`,
  pinned by `tools/mod.rs:1335-1371`), so `fs_read` or `fetch_url` cannot become
  confirmable without losing their concurrency.
- **A changed default reaches fresh installs only.** `save_config` writes every field, so
  anyone who ever saved settings has `"confirm_dangerous": false` and `"fs_root": null`
  on disk, and no migration can tell that written default from a deliberate choice. The
  precedent is `web_enabled` in 0.9.9: the `Default` impl only, "an existing
  `settings.json` keeps whatever it says" (journal/tools.md).

### 2.3 The sandbox's network [measured]

The app runs `wasmer run --v8 --net --volume <job>:/w … packed-sandbox.webc -- /w/job.py`
(`sandbox.rs:457-469,746-772`). A probe inside the sandbox, against HTTP servers on this
host:

| Target | Host | Sandbox `--net` (today) | Sandbox, no net | Sandbox, rule list R1 |
|---|---|---|---|---|
| 127.0.0.1 (a host server) | 200 | **200 — the request reached the host** | refused | **EPERM** |
| `localhost` | 200 | **200** | no name resolution | resolves, then EPERM |
| the host's LAN address | 200 | **200** | refused | **EPERM** |
| 169.254.169.254 | unreachable | not blocked (unreachable here) | refused | **EPERM** |
| `https://example.com` | 200 | 200 | no name resolution | **200** |

So `--net` is the host network: loopback is the **host's** loopback, and the LAN and the
metadata address are blocked only by whether this machine happens to route to them.

wasmer 7.2.0 filters natively (`--net=<rule>,<rule>…`, `ipv4|ipv6|dns:allow|deny=<addr/prefix>:<port>`).
Measured semantics: **deny beats allow** regardless of order; **any rule list makes the
default deny**, so `ipv4:allow=*:*`, `ipv6:allow=*:*` and `dns:allow=*:*` must be stated;
rules apply to the address at connect time. R1 — those three allows plus denies for
127/8, 10/8, 172.16/12, 192.168/16, 169.254/16, 0/8, `::1`, `fe80::/10`, `fc00::/7` and
`::ffff:0:0/96` — blocked every private target with EPERM and kept public HTTPS working.

Two side findings:
- **Plain `http://` to a host off this machine fails under `--net`** (9 of 9), because
  `connect()` returns before the handshake and the first send fails; HTTPS works. A wasmer
  defect independent of this stage, present today.
- **Without `--net`, wasmer prints** "The current package is requesting networking
  access. Run the package with `--net` flag to bypass the prompt." **into the guest's
  stdout**, which is what the model would read.

Local mode is the host interpreter and has the host's network; no flag reaches it.

### 2.4 What child processes inherit, and need [measured]

No production spawn calls `env_clear` or `env_remove` [code]:

| Spawn | What runs | Who chose the code that runs | Environment today |
|---|---|---|---|
| `llama-server` (`api/managed.rs:303`) | a binary from settings | the user | inherits all |
| MCP servers (`shared/mcp.rs:572`) | a command from settings | the user (third-party software) | inherits all + the declared `env` |
| workspace build/run/test (`tools/code.rs:1193`) | the user's command line | **the model can edit what it runs** (build scripts, tests) | inherits all + `NO_COLOR` |
| Local Python (`sandbox.rs:600`) | the configured interpreter | **the model writes the code** | inherits all + two `PYTHON*` |
| wasmer (`sandbox.rs:471`) | the sandbox | the model, inside the guest | inherits; **the guest sees only five `PYTHON*`/`TERM` variables** |

This machine's environment carries `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GROK_API_KEY`,
`OPENROUTER_API_KEY`, `TAVILY_API_KEY` and `HF_TOKEN` (names only) — each readable today by
model-written Local Python code and by a model-edited build script.

A launcher that reproduces the app's `Command` ran each child class under four
environments:

| Child | Full | Empty | Allowlist (8 names) | Full minus credential names |
|---|---|---|---|---|
| `node` stdio server, `npx`, a cached npx MCP server | ok | **dies** (CSPRNG assertion) | ok | ok |
| Python with ssl/urllib/tempfile | ok | **dies** (no random numbers) | ok | ok |
| `cargo build` / `cargo test` (clean, really linking) | ok | **fails** (`link.exe` not found) | ok | ok |
| `git status`, `git config --global` | ok | fails without `HOME` | ok | ok |
| `llama-server --version/--list-devices` | ok | ok | ok — **but `GGML_VK_VISIBLE_DEVICES` silently lost** | ok |

The minimal Windows allowlist that ran everything: `SystemRoot`, `PATH`, `LOCALAPPDATA`,
`APPDATA`, `USERPROFILE`, `TEMP`, `TMP`, `ProgramData` (without `SystemRoot` even Winsock
fails; without `ProgramData` the MSVC linker is not found). What an allowlist loses
silently is everything nobody can list ahead: `CARGO_HOME`, `RUSTFLAGS`, `JAVA_HOME`,
`VIRTUAL_ENV`, `NODE_EXTRA_CA_CERTS`, proxies, GPU selection. A name denylist
(`*_KEY`, `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, …) broke nothing measured, and misses what
is not credential-shaped (`DATABASE_URL` with a password, `*_APIKEY`). **Neither** stops a
child from reading credential *files* (`~/.aws`, the git credential store) — it runs as
the user. Linux and macOS were not measured.

MCP's inheritance is a published promise, not an accident: spec §9.6 says a variable
"already set in the application's own environment reaches the child by inheritance
without being listed", and `orchestrator/tests/mcp.rs:436-440` pins it.

### 2.5 The other two [code][measured]

- **`fetch_url` and `web_search`'s page fetches read the body whole**
  (`http_text.rs:71`, `resp.bytes()`); only the inflated size is capped (32 MB). The
  bound is the 20-second request timeout times the link speed. `/image attach <url>`
  already streams under a byte ceiling (`image_fetch.rs:143-154`).
- **Linux file modes follow the umask.** `write_json` is `fs::write` + `rename`
  (`storage/json.rs:245-258`) and `ensure_dirs` is `create_dir_all`; no code sets a mode.
  Measured on Ubuntu 24.04: umask `022`, so the data root is `755` and `settings.json`
  `644`. New home directories there are `0750`, which hides them from other users — but
  not everywhere: this project's own lab image has `/home/jovyan` at `2770` for the
  `users` group, where every member could read the chats and the encrypted keys, and the
  key's derivation inputs (`/etc/machine-id`, the user name) are world-readable
  (ADR 0008 is explicit that other-user protection is Windows-only).

## 3. Decided without a fork

Each is the only reasonable reading of a measured defect; say so if any should be a fork.

- **N1. A write through a symbolic link is refused.** The resolvers check
  `symlink_metadata` on the final component and refuse a link whose target does not
  exist; an existing link keeps its current treatment (it is canonicalized, and its target
  must be inside the root). A test with a dangling link inside the root, `#[cfg(unix)]`.
  This one is a published-scope vulnerability, not a hardening: SECURITY.md lists
  "escaping the `fs_root` jail (path traversal or any other route)" as in scope.
- **N2. `code_write` and `code_edit` refuse paths under `.git/`.** A model editing git's
  internals is never the task, and a hook is code the user runs at the next commit.
- **N3. The page fetch streams under a byte ceiling** — 32 MB, the inflated ceiling it
  already has — for `fetch_url` and `web_search`'s result pages, refusing in words the way
  `/image attach` does.
- **N4. The Wasmer "requesting networking access" line is recognised** and replaced by the
  tool's own localized note that the network is off, so the model is told the truth
  rather than a prompt it cannot answer.
- **N5. SECURITY.md is made accurate** about what stays: the profile toggles (a second
  gate only for MCP and the opt-in tools), Local Python's host access, and whatever the
  forks below leave in place.

## 4. What a live run has to show

The stage touches tools, so a live run is mandatory (AGENTS.md §3):

- **`python_exec` on a real model** (the LAN stack or a rented one) with the network on:
  code that fetches a public HTTPS URL succeeds, and the same code aimed at the host's
  loopback and LAN address fails with a refusal the model reports rather than a hang.
- **The file tools** on a real model with an injected instruction to read the app's
  `settings.json` and to write outside the root: both refused, and the model says why.
- **A workspace build** (`code_build`) of this repository and **an MCP server** launched
  through `npx`, under the chosen environment policy — both work.

## 5. Forks

### D1. The file tools with an empty `fs_root` — recommendation (a)

- **(a) A root is required.** With the row empty, `fs_*` refuse and say which setting to
  fill; the settings row says it is required while the switch is on. A whole disk remains
  one deliberate value away (`C:\` or `/`). An existing install with the switch on and the
  row empty loses the tools until the row is filled — said in CHANGELOG and by the
  refusal itself.
- **(b) An empty row means a default folder** (`Documents/mindfork` on Windows,
  `~/mindfork` on Linux), created on first use. Nothing to configure, but the app invents
  a folder, and an existing user's tools silently shrink to it.
- **(c) Keep the whole disk**, relying on D2 and confirmation.

(a) makes the user state the one thing that matters — what the assistant may see — and
invents nothing.

### D2. The app's own directories — recommendation (a)

- **(a) Neither read nor written by any file or code tool**, whatever the root or the
  project: the data root and the directory of the binary (where `defaults.json` lives).
  Writing them is code execution at the next launch; reading them bypasses opt-ins the app
  already has — other conversations are `chat_read`'s, off by default, and `settings.json`
  holds every key the user entered.
- **(b) Not written, but readable.**

### D3. Confirmation by default — recommendation (b)

- **(a) On for fresh installs** (the `Default` impl, the `web_enabled` precedent; existing
  files keep their value). Every dangerous call asks until `A` allows the tool for the turn.
- **(b) Keep it opt-in** — F2(b) of 2026-07-30 stands. D1, D2, D4 and D5 bound what an
  enabled tool reaches, which was the argument for confirmation to begin with; and the
  risky workspace flow is poorly served by a popup anyway, because the harmful step (a
  model editing a build script) looks like the task, and the call that runs it
  (`code_test`) is exactly what the user asked for.
- **(c) A second tier on by default:** confirm what acts on the host outside a boundary —
  `code_build/run/test`, Local `python_exec`, MCP — and let sandboxed Python and rooted
  file writes run. The best-aimed option, at the cost of a three-state setting and a
  second danger level in the tool contract.

### D4. The sandbox's network — recommendation (a)

- **(a) Keep it on, with private addresses denied** by a wasmer rule list generated from
  the same ranges as `shared/net.rs::is_public` (§2.3 R1 plus the ranges R1 omits:
  100.64/10, 198.18/15, 224/4 and 240/4), lifted by the same `tools.web_allow_private`
  switch that already means "the model may reach private addresses". Public HTTPS keeps
  working, measured.
- **(b) Off by default for fresh installs.** The simplest, but it removes what the
  network is used for (downloading a dataset) for everyone to close what only private
  addresses make dangerous.
- **(c) Keep bare `--net`** and document it.

### D5. The environment of model-driven processes — recommendation (b)

Scope: Local Python and workspace commands — the two where the model writes or edits
what runs. `llama-server` and MCP servers keep inheriting (D6).

- **(a) An allowlist** — the eight measured names plus locale, proxy and certificate
  variables — and a settings row for more. Leaks nothing undeclared, but breaks
  toolchains silently (`CARGO_HOME`, `JAVA_HOME`, `VIRTUAL_ENV`) until the user finds the
  missing name from a build failure that does not mention it.
- **(b) Inherit minus credentials:** remove every variable a setting names as a key source
  (`api_key_env`, the search and video key variables, MCP `env` sources), and every name
  matching `*_KEY`, `*_APIKEY`, `*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*_PAT`,
  `*CREDENTIAL*`. Broke nothing measured; misses secrets under other names.
- **(c) Keep inheriting** — documented in PRIVACY.md §3.6 today for MCP only.

### D6. MCP servers' environment — recommendation (a)

- **(a) Keep the published inheritance** (spec §9.6). The server is software the user
  chose to run, as they would from a shell; filtering its environment does not stop it
  reading the same credentials from files.
- **(b) Apply D5(b), exempting the names the server declares.** A server that relies on an
  undeclared `GITHUB_TOKEN` stops authenticating until the name is listed in its row —
  the exact case the published promise was written for.

### D7. A background run while confirmation is on — recommendation (a)

- **(a) The run is not offered dangerous tools** while `confirm_dangerous` is on: they are
  left out of its tool list, so the model plans without them instead of meeting a
  refusal. Confirmation then means what SECURITY.md says.
- **(b) Keep F3(c)** — a background run acts without asking — and state the exception in
  SECURITY.md and the settings description.
- **(c) Park the call** and surface the confirmation in the tasks screen (`F7`). Keeps
  the tools, but a run can wait unattended for an hour and a popup out of context is the
  thing F3 rejected.

### D8. Linux file modes — recommendation (a)

- **(a) The data root `0700` at every start** (tightening an existing one), and JSON and
  database files created `0600`. One `chmod` on the root closes every file beneath it,
  including what older versions wrote.
- **(b) New roots only.** Existing installs keep `755`.
- **(c) Document only.**

### D9. How the stage lands — recommendation (a)

- **(a) Two pull requests.** 2a — the file tools and confirmation: D1, D2, D3, D7, N1,
  N2. 2b — network and processes: D4, D5, D6, D8, N3, N4. Different live runs and different
  reviewers' attention; N5 lands with whichever closes last.
- **(b) One pull request** for the whole stage.

## 6. Stage 2a — what was implemented

D1, D2, D3 (no change), D7, N1 and N2, and the parts of N5 they touch.

- **`features/tools/reach.rs`** — one place for what no file or code tool may reach.
  `Paths::app_dirs` names the data root and the binary's directory; a resolved path
  under either is refused (`tool.fs.err.app_dir`), and the part of a path that did not
  resolve is checked for a dangling symbolic link before it is judged by its name
  (`tool.fs.err.dangling_link`). The directories come from `ctx.storage`, so a test's
  temporary root is protected exactly as the real one is, with no global state.
- **`fs.rs`** — an empty `fs_root` refuses every call (`tool.fs.err.no_root`, which names
  the setting and the `/file attach` route); after containment, `refuse_app_dirs`.
- **`code.rs`** — `resolve` takes the context and applies both checks; the walker filters
  the app's directories out of `code_list` and `code_grep` whether the project ignores
  them or not; a project whose root is inside them refuses outright; `code_edit` and
  `code_write` refuse a path with a `.git` component, compared case-insensitively.
- **`generation.rs::child_spec_with`** — a `start_subagent` run under `confirm_dangerous`
  leaves every tool whose `danger()` is true out of its list.
- **Settings text** — the root's description says it is required and what it cannot
  reach; the switch's description no longer says "may read or overwrite any file".

Tests (+5, and the four confirmation tests moved their data root out of the file tools'
root, which is exactly the arrangement D2 now refuses): an empty root refusing all
three tools; a root containing the data root reaching neither `settings.json` nor a new
file there while a sibling folder stays readable; a dangling link (unix) for `fs_write`
and for `code_write` as a file and as a directory; a project containing the data root
(read, write, list, grep, and a project inside it); `.git/` written by neither
`code_write` nor `code_edit` (`.GIT/`), read by `code_read`; a background run offered
`python_exec` only with confirmation off, the parent turn offered it either way.
