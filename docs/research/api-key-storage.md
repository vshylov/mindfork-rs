# Research: entering API keys in settings and machine-bound storage in the config

**Status:** track **done** (forks R1–R6 (§9) accepted by the user, following the
recommendations, 2026-07-20). **Stage 1 (core)** — `feat/api-key-store`: storage
format, both encryption schemes, "stored > env" resolution, the `SetApiKey` command
with encryption/persistence. **Stage 2 (UI)** — `feat/api-key-ui`: masked mode for
`InputBox`, an "API key" field with status in the cloud subsections, flags in the
settings snapshot, documentation. Result recorded in
**[ADR 0008](../decisions/0008-api-key-storage.md)**
(refines ADR 0004: "secrets don't go to disk" → "not to disk **in plaintext**").

**Hardening beyond the original design (stage 2):** the `AppEvent::Settings`
snapshot **clears** `config.api_keys` and carries only `api_keys_present` flags — the
UI never carries secrets, not even as ciphertext, and the `handle_update_config`
round-trip guard goes from a "just in case" safety net to a load-bearing invariant.

**Implementation deviations from this document** (deliberate, recorded here):
ciphertext is encoded as **hex**, not base64 — the project's own codec precedent
(`features::sandbox_setup::hex_lower`) avoids a new dependency, and the difference
in string length is immaterial for a config file. Resolving the stored key lives in
`EngineManager` (`app/orchestrator/engines.rs`), and the supervisor accepts an
already-decrypted `stored_key: Option<&str>` — so the server-launch layer knows
nothing about the secret storage format and its tests don't need encryption.

## 1. Task

Cloud API keys (OpenAI / Gemini / Claude) are currently set **only via env
variables**: the config stores only the variable's *name* (`api_key_env`, ADR 0004
"secrets don't go to disk"). Regular users have a weak grasp of environment
variables — keys need to be entered right in the settings screen and stored in the
config file, but **securely**.

Requirements (user's brief, 2026-07-20):

1. Keys are entered in the settings window (not env).
2. Stored in the config file.
3. Storage is secure: bound to a specific computer (OS instance),
   encrypted with a machine key.
4. The config migrates between computers; keys are **per-machine**:

> Scenario A/B/A: on computer A I enter keys and work. I move the config to
> computer B — there the keys need to be entered again. I go back to A with the
> same config file — the keys were already entered there, they're just read from
> the file.

The scenario implies a key format property: the config carries **several key
records — one per machine**, each decryptable only by "its" machine.

## 2. Current state (inventory)

- **Config** (`shared/config.rs`): `CloudSettings { model_name, api_key_env, url }`
  — one instance per provider (`openai`/`gemini`/`claude`) in **each** of three
  slots: assistant engine (`EngineSettings`), impersonation
  (`ImpersonationEngineSettings`), embeddings (`EmbedSettings`). Plus
  `ExternalSettings.api_key_env` (Bearer for an OpenAI-compatible proxy).
  Total: one OpenAI key today has to be specified (by env name) up to three times.
- **Resolution** (`app/supervisor.rs`): `resolve_api_key(api_key_env)` →
  `std::env::var` in `apply_chat`/`apply_impersonation`/`apply_embed`;
  structured error `ApiKeyError { NoName, Missing(var) }`, localized into the
  status chip (`ui.err.server.no_api_key_env` / `env_missing`).
- **UI** (`screens/settings/`): field "API key (env)" (`XApiKeyEnv`/`IxApiKeyEnv`/
  `EApiKeyEnv`) — text field, value = variable name; setters in `spec.rs`
  route by mode (external/cloud).
- **MCP** (`config.mcp.servers[].env`) — the same "source variable names" model
  (fork R8 of the plugins track). This research **doesn't touch** MCP (env for
  MCP servers is arbitrary variables, not 3 providers), but the chosen mechanism
  applies there too in the future.
- `windows-sys 0.61.2` is already a target dependency (sandbox Job Object) with
  the `Win32_System_JobObjects`/`Win32_Foundation`/`Win32_Security` features.
- `InputBox` **has no** masked mode (password entry) — will be needed.
- `settings.json`: atomic write + `.bak`, backup whitelist, downgrade guard,
  additive-field policy `#[serde(default)]` without a schema bump (ADR 0006 F12).

## 3. Threat model — what we protect, what we don't (honestly)

**Machine-bound encryption protects the file, not the running machine.**

Protects (and this is exactly the user's scenario):

- copying/moving `settings.json` (cloud sync, backup zip, "send me your
  settings", a USB stick) **doesn't leak keys** — off "its own" machine it's
  useless ciphertext;
- `mindfork backup` backups sit safely anywhere (the archive gets ciphertext);
- casually eyeballing the config (a settings screenshot, `cat settings.json`)
  doesn't show the key;
- on Windows (DPAPI) — also from **other users on the same machine**, including
  an administrator without the victim's password (the DPAPI master key is
  protected by the logon password).

Does NOT protect (fundamental to any "the app decrypts itself, without user
input" scheme):

- against malicious code running **under the same user on the same machine** —
  it can call the same DPAPI / derive the same machine key. This is how Chrome
  (DPAPI for passwords/cookies), Git Credential Manager, etc. work too — it's an
  industry standard, not a weakness of ours.

Comparison with the status quo matters: **an env variable is no safer** — it's
readable by any process of the same user (`/proc/<pid>/environ`, Process
Explorer) and usually sits in plaintext in a shell profile (`$PROFILE`,
`.bashrc`). The proposed scheme is no weaker than the current one in any
dimension, and strictly stronger for "the file while in transit."

Linux nuance: `/etc/machine-id` is world-readable → **another local user** who
manages to read our `settings.json` can derive the key and decrypt it.
Mitigation: file permissions (see §5, `0600` hardening) + the model itself
("we're protecting the file-in-transit"). Windows DPAPI doesn't have this
weakness.

## 4. Options

| Option | File-portable? | Headless Linux | Strength | Complexity |
|---|---|---|---|---|
| 1. Plaintext in config | yes | yes | none | ~0 |
| 2. OS keychain (`keyring`) | **no** (secret in OS store) | **no** (needs Secret Service/D-Bus) | high | medium |
| 3. **Encryption in config with a machine key** | **yes** | yes | Win: high; Linux: medium | medium |
| 4. Master password at startup | yes | yes | maximum | UX-unacceptable |

**4.1 Plaintext** — reject: directly contradicts the brief.

**4.2 OS keychain** (`keyring` crate: Windows Credential Manager / Secret
Service / KWallet). A strong, "proper" scheme, but: (a) the secret lives in the
OS store, not the config file — the letter of the request isn't fulfilled
(scenario A/B/A formally works — keys persist on the machine — but the user
specifically asked for a file); (b) on **headless Linux** (SSH to a server,
no DE — a common environment for a TUI app) Secret Service is absent → needs a
fallback → doubles the complexity; (c) a new dependency with platform backends
(zbus and others). Reject for v1; the `scheme` field in the format (§5) leaves
the door open — a keychain can be added later as one more scheme without
breaking the format.

**4.3 Encryption in config with a machine key — recommendation.**

- **Windows — DPAPI** (`CryptProtectData`/`CryptUnprotectData`, crypt32):
  the system mechanism with exactly this semantics — encrypts with the *user's*
  master key (keys managed by the OS; "user + machine" binding; decryption on
  another machine or by another user is impossible). Verified against the
  vendored source: both functions are in `windows-sys 0.61.2` under the
  `Win32_Security_Cryptography` feature (which itself requires the already-
  enabled `Win32_Security`; `LocalFree` to release the blob is in the already-
  enabled `Win32_Foundation`). Cost: +1 feature on an existing dependency + a
  small unsafe wrapper (precedent: the Job Object unsafe in `shared/sandbox.rs`).
  `pOptionalEntropy` is an application constant (not a secret, but it filters
  out generic "DPAPI dumpers" that don't know the entropy). Known nuance:
  domain roaming profiles — the user's DPAPI keys travel with the profile, so a
  record may decrypt on another domain machine of the same user; this is more
  of a bonus (the keys "moved" with the profile) than a defect.
- **Linux — HKDF(machine-id) + ChaCha20-Poly1305**: key =
  `HKDF-SHA256(ikm = contents of /etc/machine-id, salt = app constant,
  info = "mindfork-rs api-key v1" + username)`; encryption — AEAD
  ChaCha20-Poly1305, random 12-byte nonce prefixed to the ciphertext.
  `/etc/machine-id` is a stable OS-instance identifier (systemd); systemd's own
  documentation specifically instructs not to use it raw but to derive an
  app-specific key via HKDF (the `sd_id128_get_machine_app_specific` pattern) —
  that's exactly what we do. The username in `info` gives per-user binding
  (mirroring DPAPI semantics). ikm fallback: `/var/lib/dbus/machine-id`; if
  neither exists (non-systemd exotica) → the scheme is unavailable, the status
  chip explains it, the env path remains. A cloned VM image (identical
  machine-id) will decrypt the record on both copies — that's machine-id
  semantics, acceptable.

**4.4 Master password** — the only option that honestly protects against local
malware, but asking for a password on every chat app startup is unacceptable UX
for the target "regular user." Out of scope (not promised for the future
either).

## 5. Design (option 4.3)

### Format in `settings.json`

```jsonc
"api_keys": [
  {
    "label": "DESKTOP-AB12 · 2026-07-20",   // human-only: computer name + date
    "scheme": "dpapi",                       // "dpapi" | "machine-key-v1"
    "check": "hex…",                         // ciphertext of a constant — "is this our record" probe
    "keys": { "openai": "hex…", "claude": "hex…" }  // provider → key ciphertext
  },
  { "label": "laptop · 2026-07-21", "scheme": "machine-key-v1", "check": "…", "keys": { … } }
]
```

- Field `AppConfig.api_keys: Vec<ApiKeyEntry>`, `#[serde(default)]` +
  `skip_serializing_if = "Vec::is_empty"` → **additive, no migration or schema
  bump** (ADR 0006 F12 policy); old configs read as-is.
- **Record identification is a decryption probe** on the `check` field
  (ciphertext of a fixed constant): on load we iterate the records, "ours" is
  the one whose `check` decrypted successfully (DPAPI returned success / AEAD
  authentication matched). There's **no** explicit machine-id in the file —
  we don't expose a machine identifier in a portable config (a privacy bonus),
  and on Windows the concept of machine-id isn't needed at all. `label` is an
  interned string for humans (`COMPUTERNAME` / `/etc/hostname`, fallback `"?"`),
  not used in the logic.
- Record semantics: entering a key → "our" record exists — replace it in place;
  none exists — create it. Deleting a key (empty commit) → remove it from the
  record; an emptied record is removed. Other machines' records are never
  touched (they "come alive" on their own machines).

### Provider-centric storage

Keys are stored **per provider** (`openai`/`gemini`/`claude`), not per slot
(chat/impersonation/embeddings): one OpenAI key serves all three slots. This
removes today's "specify the env name three times." External proxy (arbitrary
URL, power-user feature) stays env-only (R4).

### Key resolution

`resolve_api_key(provider, stored, api_key_env)`, order:

1. **this machine's stored key** (decrypted from `api_keys`);
2. fallback — the env variable named by `api_key_env` (as today).

The stored key wins: it's set by an explicit in-app action, while the target
user doesn't see env at all. env remains for CI, power users, and machines
without machine-id. Decryption happens at `apply_*` (like today's `env::var`),
not per-request.

### Code layout

- **`shared/secrets.rs`** (new; `thiserror` per the `shared` convention): pure
  core `encrypt_with_key`/`decrypt_with_key`/`derive_key(ikm, user)` —
  testable on any OS by injecting ikm — plus platform bits: `machine_ikm()`
  (Linux) and the DPAPI wrapper (Windows) behind `#[cfg]`. Public surface:
  `encrypt(plaintext) -> ApiKeyCipher`, `try_decrypt(entry) -> Option<String>`,
  `probe(entry) -> bool` (for `check`).
- **Plaintext lives only on the entry path**: `SettingsIntent::SetApiKey
  { provider, key }` → `AppCommand::SetApiKey` → the orchestrator encrypts
  (`shared::secrets`) and puts it into `config.api_keys` → `save_config` →
  a mark in `RestartQueue` (re-launch of affected servers, debounced, as with
  engine edits). Neither plaintext nor ciphertext ever reaches the UI config
  snapshot: the UI only gets "configured on this machine" flags (an extension
  to `AppEvent::Settings`, precedents `language_locked`, `McpSnapshot`).
- **Round-trip guard**: `handle_update_config` restores `api_keys` from the
  prior config (the UI copy may be stale and carries no keys at all) —
  precedents `last_active_chat` and inheriting MCP pins.
- **Supervisor**: `apply_*` receive already-resolved keys (or a slice of
  `api_keys` + a call into `shared::secrets`) — an implementation detail of the
  stage; the status chip gets a new reason "key not set: enter it in settings
  or set the env variable" (updated `ui.err.server.*`).
- **Discipline**: keys are never logged (an existing rule — client error bodies
  are already truncated; add attention to it in review), the UI never displays
  a stored key (see §6).

### Hardening (optional, cheap)

- Set `0600` on `settings.json` on Unix during the atomic write (currently
  default permissions). Windows ACLs are not touched (the user profile is
  already locked down).
- `zeroize` for plaintext in memory — **not doing this**: the key already lives
  in a `String` in the HTTP client headers for the whole session; the ceremony
  doesn't pay for itself.

## 6. UI (settings screen)

- In the cloud subsections (Assistant / Impersonation / Embeddings when in
  openai/gemini/claude mode) — a new field **"API key"** above the existing
  "API key (env)":
  - value-status: "configured (this computer)" / "not configured" — **the key
    itself is never shown**; Enter opens an **empty** masked editor (editing =
    entering it again; the secret can't be recovered via the UI);
  - committing a non-empty value → `SetApiKey`; committing empty → delete this
    machine's key; `Del` on the field — also deletes (mirroring the `Del` reset);
  - the field's description explains: the key is shared across chat/
    impersonation/embeddings for this provider, stored encrypted and bound to
    this computer; on another computer it's entered again.
- **`InputBox` — `mask` mode** (new, modeled on `single_line`): renders `•` per
  character, spellcheck and underlines are disabled, `Ctrl+C`/`Ctrl+X` (copying
  content) is a no-op, paste (`Ctrl+V`) works. `•` width = 1 — cursor and
  hscroll need no changes.
- "API key (env)" stays below as a power-user field (description — "fallback,
  used if no key was entered").
- Other machines' records (`api_keys` from other machines) are not shown or
  edited in the UI in v1 (R6); `/` search picks up the new field automatically.
- i18n: new keys `ui.settings.field.api_key`, `ui.settings.desc.api_key`,
  status values, updated `ui.err.server.*` (ru+en; parity gates cover them
  automatically).

## 7. Dependencies and licenses

| What | Why | Nature |
|---|---|---|
| `chacha20poly1305` (RustCrypto) | AEAD for the Linux scheme | pure Rust, MIT/Apache-2.0 (in `deny.toml` allowlist) |
| `hkdf` (RustCrypto) | key derivation from machine-id | pure Rust, tiny; `sha2` is already a direct dependency |
| `windows-sys` | DPAPI | **already present**; +`Win32_Security_Cryptography` feature |

No new C dependencies; `cargo deny` stays clean (licenses in the allowlist).

## 8. Testing

- **Unit tests** (any OS): `encrypt`/`decrypt` round-trip for the Linux scheme
  with an injected ikm; decryption with someone else's key → fails (AEAD);
  `probe` against `check`; serde format (default/skip, an unknown `scheme`
  doesn't fail parsing — the record is simply "not ours"); "stored > env"
  resolution; `handle_update_config` round-trip guard; **persistence carries no
  plaintext** (after `SetApiKey` the serialized config doesn't contain the key
  as a substring); UI (status field, mask, empty editor when a key is
  configured, intent).
- **`#[cfg(windows)]`**: DPAPI round-trip on a live CI/dev machine (the Windows
  CI job already exists).
- **Live run**: the engine is essentially untouched (the same key goes into the
  same header) — a manual smoke test is enough: enter a real key in settings →
  cloud chat works; copy `settings.json` to another machine (or test with a
  substituted ikm) → status "not configured," enter it again → works; go back
  to the first machine → works without re-entering. The existing cloud
  `#[ignore]` smokes (`MINDFORK_ANTHROPIC_KEY` and others) are untouched — they
  test the clients.

## 9. Forks (for confirmation)

- **F1. Storage scheme.** (a) **encryption in the config with a machine key —
  recommendation** (the letter of the request, headless-compatible, scenario
  A/B/A "out of the box"); (b) OS keychain `keyring` — reject for v1 (secret
  not in the file; headless Linux); (c) plaintext — reject. The `scheme` field
  leaves (b) open as a future add-on.
- **F2. Windows mechanism.** (a) **DPAPI — recommendation** (native, stronger:
  also protects against other users of the machine; +1 feature on an existing
  dependency); (b) a single HKDF(MachineGuid) path on both OSes — ~50 lines
  simpler, but weaker and still needs registry reads.
- **F3. Resolution priority.** (a) **stored key > env — recommendation**
  (an explicit in-app action; the target user doesn't see env at all);
  (b) env > stored (the 12-factor convention, but silently overrides the UI).
- **F4. v1 scope.** (a) **three cloud providers, provider-centric —
  recommendation** (the key is shared across chat/impersonation/embeddings;
  external proxy stays env-only); (b) + keys for external slots (one per
  chat/imp/embeddings — complicates the format for a power-user feature that
  env already covers).
- **F5. Storage location.** (a) **`settings.json` — recommendation** (the
  letter of the request; atomic write, `.bak`, backup whitelist, downgrade
  guard already work); (b) a separate `secrets.json` (a slightly cleaner
  separation, but a new file would need to be threaded through backup/
  migrations/restore, and "config file" in the brief means settings itself).
- **F6. Other machines' records.** (a) **v1 without UI management —
  recommendation** (a handful of records, cleanup by editing the file; "forget
  other computers" groundwork in the roadmap); (b) an in-settings action right
  away.

## 10. Implementation plan (after forks are confirmed)

Two PRs (track stages, linear):

1. **`feat/api-key-store` — core**: `shared/secrets.rs` (both schemes + tests),
   `AppConfig.api_keys` (+serde tests), resolution in the supervisor
   ("stored > env", new status-chip text), `AppCommand::SetApiKey` +
   encryption/persistence in the orchestrator, round-trip guard, i18n error
   keys.
2. **`feat/api-key-ui` — UI**: `mask` mode for `InputBox`, an "API key" field
   in the cloud subsections (+embeddings), "configured" flags in
   `AppEvent::Settings`, `SettingsIntent::SetApiKey`, descriptions/i18n,
   README/spec/CHANGELOG (Added + Security).

Documentation to follow: a new ADR "API key storage: machine-bound encryption
in the config" + a note on ADR 0004 (refining the "secrets don't go to disk"
statement to "not to disk in plaintext"), architecture §6/§12, spec (settings
section), install.md (the env path remains and is documented as a fallback).

## 11. Out of scope / groundwork

- OS keychain as an additional `scheme` (Windows Hello / Secret Service).
- Master password (opt-in "paranoid" mode) — not promised.
- Keys for the external proxy and MCP server env maps via the same mechanism.
- UI management of other machines' records ("forget computer X").
- `0600` on `settings.json` on write (Unix) — could be picked up in stage 1 as
  a small hardening item.
