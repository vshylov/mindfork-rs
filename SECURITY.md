# Security Policy

mindfork-rs is a terminal chat client that runs language models with tools —
which means it handles API keys, executes model-requested actions behind
switches, and stores conversations on disk. This page says what the app
promises, what counts as a vulnerability, and how to report one.

## Reporting a vulnerability

Please do **not** open a public issue for a security problem. Instead, use
GitHub's private reporting: **Security → Report a vulnerability** on the
[repository page](https://github.com/vshylov/mindfork-rs/security). Include
the app version, your OS, the engine mode if it matters, and a reproduction
or proof of concept.

This is a small project, but reports are taken seriously: you can expect an
acknowledgment within **7 days**. Please give us a reasonable window to ship
a fix before publishing details; the advisory will credit you unless you
prefer otherwise.

## Supported versions

While the project is pre-1.0, fixes land in the **latest release only** —
there are no backport branches.

| Version | Supported |
|---|---|
| the latest [release](https://github.com/vshylov/mindfork-rs/releases) | yes |
| anything older | no |

## What the app promises

- **No telemetry.** The app makes network requests only to the endpoints you
  configure (inference server, embedder, TTS, MCP servers) — plus, when you
  explicitly enable the web tools (**off by default**), the search engines and
  pages they fetch, and the one-time Python-sandbox setup downloads, every one
  of which is verified against a pinned sha256: the lock list for what the app
  downloads itself, and `PYTHON_PACKAGE_SHA256` for `python.webc`, which
  `wasmer` fetches on our behalf and which is therefore checked as a finished
  file.
- **Secrets are stored encrypted and machine-bound**: cloud API keys and MCP
  server tokens entered in settings are encrypted with DPAPI on Windows and a
  `machine-id`-derived key on Linux, so a copied settings file does not carry
  usable secrets ([ADR 0008](docs/decisions/0008-api-key-storage.md)).
- **Dangerous tools are off by default and gated**: Python execution, file
  access and MCP plugins each sit behind a global switch plus per-profile
  toggles (MCP is a double opt-in), with an optional confirmation prompt
  before every dangerous call.
- **The Python sandbox is isolated**: `python_exec` runs in a Wasmer/WASIX
  sidecar with no host filesystem access and network behind a toggle
  ([ADR 0005](docs/decisions/0005-python-sandbox-wasmer.md)).
- **The file tools can be jailed**: an optional `fs_root` confines
  `fs_read` / `fs_write` / `fs_list` to one directory, and escaping via `..`
  is blocked.
- **MCP tool catalogs are TOFU-pinned**: a server changing its tool set or
  descriptions after you approved it requires re-confirmation
  ([ADR 0007](docs/decisions/0007-plugins-mcp-host-import-format.md)).
- **Backups can be password-protected** (AES-256), and a wrong password is
  refused before anything is replaced.

A report that breaks any of these promises is exactly what this policy is
for. The same ground from the user's side — every file the app writes, every
destination it can contact and the setting that has to be on first — is
[PRIVACY.md](PRIVACY.md).

## Scope

**In scope** — for example:

- escaping the Python sandbox to the host's files or network while the
  toggles say otherwise;
- escaping the `fs_root` jail (path traversal or any other route);
- recovering API keys, MCP tokens or backup passwords from the settings file
  in usable form on another machine, or finding them written to disk or logs
  in plaintext;
- bypassing the MCP TOFU pinning, the double opt-in, or the dangerous-call
  confirmation — any way a tool fires although its switch says it must not;
- weaknesses in the backup encryption;
- the app sending data to an endpoint the user neither configured nor
  enabled;
- a dependency vulnerability that is actually reachable through the app.

**Out of scope:**

- **What a model says or does with the tools you enabled.** The app ships no
  model and applies no content moderation by design; harmful, wrong or
  manipulative model output is covered by [DISCLAIMER.md](DISCLAIMER.md), not
  by this policy. A model invoking a tool you switched on is the feature
  working; a tool firing despite its switch being off is a vulnerability —
  report that.
- Prompt injection as such (a web page or document convincing the model to
  misuse an *enabled* tool). The mitigations are the switches, the jail and
  the confirmation prompt — bypasses of those are in scope, the persuasion
  itself is not.
- Vulnerabilities in the software around the app: llama.cpp and other
  inference servers, third-party MCP servers, cloud providers, your terminal
  emulator. Please report those upstream.
- Attacks that require an already-compromised machine or user account: the
  machine-bound encryption protects a *copied* config file, and explicitly
  does not defend against malware running as you (that is DPAPI's boundary
  too).
- Denial of service against your own local server by your own prompts.

## Dependencies

Dependency advisories, licenses, duplicate versions and sources are checked
weekly in CI with `cargo-deny` (the `Audit` workflow; configuration in
`deny.toml`).
