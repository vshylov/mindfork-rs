# ADR 0008 — API key storage: input in settings + machine-bound encryption

**Status:** accepted (2026-07-20). Refines [ADR 0004](0004-engine-contract-multi-provider.md)
("secrets not on disk" → "not on disk **in plaintext**"). Research and decision points —
[docs/research/api-key-storage.md](../research/api-key-storage.md) (D1–D6 confirmed by
the user 2026-07-20).

**Context.** ADR 0004 (multi-provider Phase 0) decided that the cloud API key is read
from an **environment variable**, with only its name stored in `settings.json`. For the
target TUI-chat user this turned out to be a barrier: "regular users have a shaky grasp
of what an environment variable is." The requirement is to enter the key in the settings
screen and store it in the config file, but safely, while preserving config
**portability**: keys must be per-machine (enter it on A → move the config to B → enter
it again there → back to A → the key still reads from the same file).

## Decision

### 1. The key is encrypted with a machine key and lives in the config

`AppConfig.api_keys: Vec<ApiKeyEntry>` — **a list of entries, one per computer**
(`#[serde(default)]` + `skip_serializing_if` → additive, no migration or schema bump,
[ADR 0006](0006-data-schema-versioning.md) F12 policy). An entry carries `label`
(machine name + date, for humans only), `scheme`, `check`, and `keys: provider →
ciphertext`.

**"Ours" is recognized by decrypting the `check` probe**, not by storing a machine-id:
the machine identifier never shows up in a portable file, and on the Windows path it
isn't needed at all. Foreign entries are never touched — they come back to life on their
own machines; an entry with an **unfamiliar scheme** is read, kept, and treated as
foreign, so the format stays extensible.

Rejected options: **OS keychain** (`keyring`) — the secret isn't in the file (fails the
letter of the requirement) and is absent on headless Linux; **plaintext** — contradicts
the goal; **master password** — the only option that protects against local malware, but
prompting for a password on every chat-app launch is unacceptable. The `scheme` field
leaves the keychain as a cheap future addition.

### 2. Two encryption schemes

- **`dpapi`** (Windows): the system's `CryptProtectData`/`CryptUnprotectData` — the OS
  holds the *user's* master key; decryption by another user or on another machine is
  impossible. `pOptionalEntropy` is an application constant (not a secret, but it cuts
  off generic "DPAPI dumpers"). Cost: +1 feature on the `windows-sys` crate we already have.
- **`machine-key-v1`** (Linux): HKDF-SHA256 over `/etc/machine-id` (systemd explicitly
  advises against using the raw machine-id — the `sd_id128_get_machine_app_specific`
  pattern; falls back to `/var/lib/dbus/machine-id`) + ChaCha20-Poly1305 (AEAD, random
  nonce prefixed). The username goes into `info` → per-user binding, like DPAPI. No
  machine-id → the scheme is unavailable, the env path remains.

The ciphertext is encoded as **hex** by our own codec (precedent:
`features::sandbox_setup::hex_lower`) — a base64 dependency isn't needed, and the string
length difference is irrelevant for a config file.

### 3. Resolution: stored key → env fallback

`resolve_api_key(stored, api_key_env)`: the stored key wins (entered by an explicit
in-app action; the target user doesn't see env), env remains for CI, power users, and
systems without a machine-id. Keys are **provider-centric**: one OpenAI key serves chat,
impersonation, and embeddings (removes the former "state the variable name three times").
The external proxy stays env-only — an arbitrary URL can't be pinned to a provider.

Decryption lives in `EngineManager` (`app/orchestrator/engines.rs`); the supervisor
receives an already-decrypted `stored_key: Option<&str>` — the server-launch layer knows
nothing about the secret-storage format, and its tests don't need encryption.

*Amended 2026-08-17* ([docs/external-api-key.md](../history/external-api-key.md)): the external
proxy no longer stays env-only. The objection above was about **addressing**, not about
the user, and the MCP reuse below answered it — a secret need not be pinned to a
provider. An external key is pinned to its **slot** (`SecretKey::External(ExternalSlot)`
— chat / impersonation / embeddings / speech), which is the sub-section the user typed
the URL into. Which secret a slot reads is now the settings struct's own answer
(`secret_key()`), consulted by both the orchestrator and the settings screen, so a row
cannot address a different secret than the server resolves.

### 4. The secret never leaves its own path

Plaintext lives only on the entry path (`SettingsIntent::SetApiKey` →
`AppCommand::SetApiKey` → encryption in the orchestrator) and in the HTTP client. Keys
**never enter** the `AppEvent::Settings` snapshot at all: `config.api_keys` is cleared on
emit, the UI only gets `api_keys_present` flags. `handle_update_config` restores the
value on the way back (round-trip protection, as with `last_active_chat` and MCP TOFU
pins). The settings field shows a **status**, not the secret; the editor opens empty and
masked (`InputBox::set_mask`), and selecting text out of a masked field doesn't reach the
clipboard.

## Threat model (an honest boundary)

We protect **the file**: transfer, copy, backup, cloud-syncing the config — outside
"your own" machine it's a useless ciphertext; on Windows, additionally, from other users
on the machine. **We do not protect** against code running as the same user on the same
machine: it will call the same DPAPI / dump the same key. This is fundamental to any
"the app decrypts on its own, no user input" scheme — Chrome and Git Credential Manager
work the same way. The previous env path was no safer (any process owned by the user can
read the variable, and it typically sits in plaintext in the shell profile), so this
decision is no weaker than the status quo on any dimension and is strictly stronger for
"the file on the wire."

## Consequences

- **Plus:** the key is entered in the app — the env barrier for regular users is gone;
  the config stays portable, keys are per-machine (the A/B/A scenario works with no
  extra action).
- **Plus:** one key per provider instead of three variable-name mentions.
- **Plus:** the format is extensible (`scheme`), no migration was needed.
- **Minus:** a platform fork (DPAPI vs HKDF) and a small `unsafe` for winapi (precedent —
  the sandbox Job Object, ADR 0005); the Linux path is checked by CI, not by development
  on Windows.
- **Minus:** the protection doesn't extend to local malware — documented in the UI
  (field description), README, install.md, and the module doc, so it doesn't create
  false expectations.
- **Reused, as this predicted:** the **backup password** (spec §12.3,
  [docs/history/backup-password.md](../history/backup-password.md)) is stored by the very same
  mechanism, under a reserved entry key (`backup-password`) in the same
  per-machine entry — no second list, no new scheme, no migration. The field on
  disk is still called `api_keys`; renaming it is what the additive-only rule
  ([ADR 0006](0006-data-schema-versioning.md) F12) forbids, so its *meaning*
  widened to "this machine's secrets" instead. One consequence is sharper there
  than for an API key: a key that can't be re-entered on a new machine is an
  inconvenience, whereas a **backup** password that can't be is unreadable data —
  hence the field's hint tells the user to record it elsewhere.
- **Reused again, as predicted:** an **MCP server's environment values** (spec §9.6,
  [mcp-server-editor.md](../history/mcp-server-editor.md) §9) — under
  `mcp-<server>-<VARIABLE>` in the same per-machine entry. Resolution differs
  from §3 on purpose: there the env variable is a *fallback* under a stored key,
  while here the two live in the same row, so naming a source is an explicit
  instruction and a stored value must not silently override it — one origin per
  variable, decided by what the row says. It also
  generalized the plumbing: one typed `SecretKey { Provider | BackupPassword |
  McpEnv }` behind one `SetSecret` command and one `secrets_present` list in the
  settings snapshot, instead of a parallel command and flag per kind. This is the
  first reuse where the secret is consumed by a **child process** rather than by an
  HTTP client, and where a *rename* can orphan a stored value — deliberately not
  collected automatically, because the config snapshot undo restores cannot restore
  a secret.
- **Reused a third time, closing this ADR's own groundwork item:** the **external
  server's** Bearer key (spec §11.6, [external-api-key.md](../history/external-api-key.md)),
  under `external-<slot>` in the same per-machine entry. Resolution follows §3 exactly
  — stored wins, env is the fallback — because the external row has the cloud row's
  shape (a key plus an optional variable *name*), unlike the MCP row above, where the
  variable is *declared* and naming a source is an instruction. Two things are new.
  The key is addressed **per slot**, not per provider, which inverts §3's
  "provider-centric" rule for a good reason: one provider is one account, whereas four
  `external` URLs are four independent servers, and the ordinary setup — a cloud
  gateway for chat beside a local `llama-server` for embeddings — would otherwise send
  the gateway's token to localhost. And having **no** key stays legitimate rather than
  a misconfiguration, since a local server needs none; that is why the external paths
  keep `resolve_api_key(...).ok()` where the cloud paths report `Disconnected`.
- **Groundwork:** an OS keychain as an additional `scheme`; UI management of other
  machines' entries ("forget this computer"), which is also where an explicit cleanup
  of orphaned MCP secrets belongs.
