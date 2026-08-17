# The external server's API key, entered in settings

Design plan. Closes the groundwork item ADR 0008 named for itself: *"the same
mechanism for external-proxy keys"* ([ADR 0008](../decisions/0008-api-key-storage.md)
§Consequences, research [api-key-storage.md](../research/api-key-storage.md) §11).

## 1. Why

ADR 0008 moved the **cloud** providers' API keys out of environment variables and
into the settings screen, machine-encrypted inside `settings.json`. Its stated
reason was that the target user of a TUI chat "has a shaky grasp of what an
environment variable is". The **external** mode — connect to any already-running
OpenAI-compatible server — was left out of that decision on one argument
(research §F4, decision point R4): *an arbitrary URL cannot be pinned to a
provider*, and the mode was filed as a power-user feature that env already
covers.

That argument was about **storage addressing**, not about the user. It is no
longer true that only power users get there: `external` is the mode for
OpenRouter, LiteLLM, LM Studio, vLLM and every self-hosted gateway, and those
*do* require a Bearer key. Today the settings screen offers such a user a field
called "API key (env)" that takes the **name of an environment variable** —
i.e. exactly the barrier ADR 0008 removed everywhere else, still standing in the
one mode where the URL is typed by hand.

The addressing objection also has a plain answer that the MCP work has since
established: a secret does not have to be addressed by *provider*. An MCP
server's environment value is addressed by `(server, variable)`
([mcp-server-editor.md](mcp-server-editor.md) §9), under the same
per-machine entry, with no new scheme and no migration. An external slot can be
addressed the same way — by the **slot**, which is what the user is looking at
when they type the URL.

## 2. What exists (read before designing — §3 of lessons.md)

- **`shared/secrets.rs`** — needs no new mechanism. `put_key`/`stored_key`/
  `is_ours` are keyed by a plain storage-name string; `SecretKey` is the typed
  key over it (`Provider | BackupPassword | McpEnv`) and exists precisely because
  *the side effects of storing differ per kind*.
- **`app/supervisor.rs`** — `resolve_api_key(stored, api_key_env)` is already the
  "stored wins, env is the fallback" funnel, and every cloud path goes through
  it. The external paths call it as `resolve_api_key(None, Some(env))` — the
  `None` is the hole this plan fills.
- **`ServerSupervisor`** — every method already takes one already-decrypted
  `stored_key: Option<&str>`. **No trait signature changes**: the orchestrator
  decides *which* secret that is, the supervisor never knows the storage format.
- **`app/orchestrator/engines.rs`** — `stored_key(api_keys, provider)` maps the
  active mode to a provider key. This is the one place that has to learn that a
  mode can address a non-provider secret.
- **`screens/settings/`** — the `XApiKey`/`IxApiKey`/`EApiKey`/`TtsApiKey` field
  ids, the masked editor, the empty seed, `Del`-deletes, the status-not-value row
  and the `SetSecret` intent **already exist**; they are simply not built into the
  `External` arm of the field list. `secret_field_key` maps a field id to a
  `SecretKey` by reading `mode.cloud_provider()`.

Consequence: this is additive plumbing, not a new subsystem. The one genuinely
new thing is a **name for the slot**.

## 3. Scope — which slots

Four settings slots have an `external` sub-section, each with its **own URL**:

| Slot | Setting | Storage name |
|---|---|---|
| assistant chat | `engine.external` | `external-chat` |
| impersonation | `impersonation_engine.external` | `external-impersonation` |
| embeddings | `embed.external` | `external-embed` |
| speech (TTS) | `tts.external` | `external-tts` |

All four get the field. Leaving any of them out would mean the same row means
"type a variable name" in one tab and "type the key" in the next — the settings
screen's own consistency is the reason, and each is three lines of code once the
slot exists. `video` has no external mode (only Gemini takes video, spec §9.9)
and is out of scope.

The `external-` prefix cannot collide with a provider key
(`openai`/`gemini`/`claude`/`grok`), with `backup-password`, or with `mcp-<id>-<VAR>`.
A hyphen, not a dot, for the reason recorded in `secrets.rs`: a dotted literal
reads as an i18n bundle key to the `tools/cyrillic_scan.py` sibling gate
(lessons.md §7).

## 4. Forks

**User's decision (2026-08-17): F1(a), F2(a), F3(a), F4(a) — every recommendation
as written, and the §3 scope of all four slots.**

- **F1. Slot granularity.** **(a) one key per slot — recommended** (four names as
  above). (b) One shared `external` key for all four slots. (b) is smaller, and
  wrong for the case that motivates the feature: the four sub-sections hold four
  independent URLs, and the common real configuration is a cloud gateway for chat
  with a *local* `llama-server` for embeddings — one shared key would send the
  gateway's Bearer token to localhost, and vice versa. Per-slot is also what
  research §F4(b) itself sketched when it deferred the item.
- **F2. Resolution order.** **(a) stored key wins, env is the fallback —
  recommended**: identical to ADR 0008 §3, and the external row has exactly the
  cloud row's shape (a key, plus an optional variable *name* naming a source).
  (b) The MCP rule instead — "one origin per variable, decided by what the row
  says" (§9 of mcp-server-editor.md): there the `env` row is the *declaration* of
  the variable, so naming a source is an explicit instruction a stored value must
  not silently override. No declaration exists here, so (b) would only make the
  two engine paths disagree with each other.
- **F3. What happens with no key at all.** **(a) unchanged — recommended**: a
  missing key stays **not an error** for external mode (a local `llama-server`
  needs none), so the resolution is `resolve_api_key(stored, env).ok()` and an
  empty result simply sends no `Authorization` header. (b) Refuse to connect
  without a key, as the cloud paths do — wrong here, it would break every local
  server.
- **F4. The env-name field.** **(a) keep it, next to the new key row —
  recommended** (the cloud rows show both; CI, `MINDFORK_*` workflows and the
  live-gate scripts use it, and removing it is a breaking change for the people
  who are on external today). (b) Replace it. Not considered further.

## 5. Design

### 5.1 `shared/secrets.rs` — one new `SecretKey` variant

```rust
pub enum ExternalSlot { Chat, Impersonation, Embed, Tts }

pub enum SecretKey {
    Provider(CloudProvider),
    External(ExternalSlot),      // new
    BackupPassword,
    McpEnv { server: String, var: String },
}
```

`storage_name()` → `format!("external-{}", slot.key())`. No format change, no
migration, no schema bump: the value lands in the same per-machine entry's `keys`
map ([ADR 0006](../decisions/0006-data-schema-versioning.md) F12, additive).

### 5.2 `shared/config.rs` — the mode decides which secret its slot uses

Each engine-shaped settings struct gains one method:

```rust
impl EngineSettings { pub fn secret_key(&self) -> Option<SecretKey> { … } }
```

External → `External(<its slot>)`; a cloud mode → `Provider(p)`; managed/shared →
`None`. Four implementations (`EngineSettings`, `ImpersonationEngineSettings`,
`EmbedSettings`, `TtsSettings`), each a two-line match over the mode.

This is the single source of truth the change needs, and it **removes** a
duplicate: today both `engines.rs` and the settings screen's `secret_field_key`
spell out "mode → provider" independently. After this they both ask the settings
struct. That matters more than the line count — a mode that addresses the wrong
secret is exactly the "server left running with an old secret" defect class
lessons.md §3 records.

### 5.3 Resolution — three call sites, no new ones

- `engines.rs`: the local `stored_key()` helper takes the slot's `SecretKey`
  (from §5.2) instead of an `Option<CloudProvider>`, and decrypts by
  `storage_name()`. `apply_chat`/`apply_embed`/`apply_impersonation` are otherwise
  untouched, and so is the staleness comparison — it already compares the whole
  key blob, so a changed external key relaunches the server.
- `supervisor.rs`: `external_chat_setup` gains the `stored_key` parameter and its
  body becomes `resolve_api_key(stored_key, api_key_env).ok()`; the external arm
  of `apply_embed` does the same. Both keep `.ok()` (F3).
- `shared/tts/mod.rs`: the `External` arm resolves
  `stored_key.filter(non-empty).or_else(|| env_key(...))` — the same order the
  cloud arm three lines above already uses; `orchestrator/tts.rs` passes
  `config.tts.secret_key()` instead of `mode.cloud_provider()`.

### 5.4 Side effects of storing — `handle_set_secret`

`SecretKey::External(slot)` marks exactly the one server that slot feeds for a
restart: `Chat → mark_chat`, `Impersonation → mark_impersonation`,
`Embed → mark_embed`, `Tts → nothing` (the speech engine is built per utterance,
like the backup password's "nothing to restart"). Deliberately a match, not a
blanket "restart everything": that is the *reason* `SecretKey` is typed.

### 5.5 UI — no new field id

The `External` arm of each field list gains the secret row that its cloud sibling
already builds, above the existing env-name row:

```
URL (external)
Model (optional)
API key (optional)        ← new: status, masked editor, Del deletes
API key (env, optional)
```

`secret_field_key` switches from `mode.cloud_provider()` to `settings.secret_key()`
(§5.2) and therefore resolves `XApiKey` to `External(Chat)` when the mode is
external — every other behaviour (empty seed, mask, `SetSecret`, the
"unavailable on this machine" description, `Del`) is inherited unchanged from
`is_secret_field`. `secrets_present` gains the four slots as candidates.

Two new bundle keys in **both** locales: `ui.settings.field.api_key_opt` (the
label: the key is optional here, mirroring `api_key_env_opt`) and
`ui.settings.desc.ext_api_key` — whose text has to close the door (lessons.md §4):
it says the key is optional, that a local `llama-server` needs none, and that a
key entered here **wins over** the variable named in the next row, since two
fields that look like alternatives must say which one decides.

## 6. Tests

Unit: `storage_name` for the new variant and its non-collision with the other
kinds; `secret_key()` per mode for all four settings structs; `external_chat_setup`
with a stored key / with only an env name / with neither (the header present,
falling back, and absent); the embed and TTS external arms likewise;
`handle_set_secret` marking exactly one server per slot and not the others;
the settings screen resolving `XApiKey` to `External(Chat)` in external mode and
to `Provider(p)` in a cloud mode, and the row appearing in the external field
list; `secrets_present` reporting a stored external key.

Live: **required** — this is an engine path (lessons.md §9). The stack is an
external `llama-server` reached over `MINDFORK_ENGINE_URL`, which needs no key,
so the smoke to run is the ordinary orchestrator set proving the external path
still connects with no key stored (F3), plus a probe with a stored key against a
server that requires one.

## 7. Documentation

`docs/install.md` §3 (the env path stays, documented as the fallback), README
(settings/keys), spec §11.6 (the settings screen's field list) and §6/ADR 0004's
mode description if it states env-only, ADR 0008 (a "Reused, as predicted" bullet
and the groundwork item struck), `docs/architecture.md` §5/§7 (the resolution
chain), `docs/journal/storage.md` (the entry — the secrets mechanism is that
file's area, as for the backup password and the MCP env values), CHANGELOG
`[Unreleased]` → Added.
