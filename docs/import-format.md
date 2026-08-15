# `mindfork-import` exchange format (v1)

The documented **neutral import format** for mindfork-rs data: profiles and
chats from third-party apps. An external (possibly private) **converter**
reads the source app's format and emits a single JSON file in this format;
the app imports it with:

```
mindfork-rs import <file.json>
```

**The app also emits this format**: `/export json` writes one chat (plus the
profile it belongs to) as a v1 document with explicit `id`s, so importing it
back lands on the same entities rather than copies. What an export cannot carry
is what the format has no place for — tool calls; `/export md` keeps those, as
readable text. See [chat-export-file.md](history/chat-export-file.md).

The "external converter → documented neutral file → import" pattern was
chosen by research [docs/research/plugin-system.md §5](research/plugin-system.md)
(precedents: beancount/beangulp, KeePass, Netscape bookmarks). Knowledge of
non-public source apps (e.g. LameLLaMA) lives in the converters, not in
mindfork-rs.

## Properties

- **Idempotency.** Entity ids are deterministic (§Identifiers): re-importing
  the same file overwrites the same profiles/chats instead of creating
  duplicates.
- **Strict about structure, tolerant of extension.** Unknown fields are
  silently ignored (a compatible format extension doesn't require a version
  bump); a violation of the described structure (wrong `format`, duplicate
  keys, a reference to a missing profile, an unknown role) is an error with
  an explanation.
- **Downgrade guard.** `version` newer than the app supports → refusal asking
  you to update the app (silent corruption is worse than a refusal).
- The file is read as UTF-8; a leading BOM is allowed and stripped.

## Structure

```jsonc
{
  "format": "mindfork-import",     // required, exactly this string
  "version": 1,                    // required; 1 is supported

  "profiles": [                    // optional (empty by default)
    {
      "key": "assistant-anna",     // required, non-empty, unique among profiles
      "name": "Anna",              // required
      "id": null,                  // opt.: explicit UUID instead of the deterministic one
      "language": "ru",            // opt.: agent-scaffold language ("ru"/"en"/an
                                   // external locale code); absent → default language
      "system_message": "…",       // opt.: the profile's system message
      "greeting": null,            // opt.: assistant greeting in a new chat
      "character_names": {         // opt.; missing fields fall back to defaults
        "user": "…", "assistant": "…", "system": "…"
      },
      "sampling": {                // opt.: the profile's default sampling.
        "temperature": 0.8         // Supported subset of mindfork fields
      }                            // (see below); unknown fields are ignored
    }
  ],

  "chats": [                       // optional (empty by default)
    {
      "key": "conv-123",           // required, non-empty, unique among chats
      "profile_key": "assistant-anna",  // required: a profile key from THIS file
      "id": null,                  // opt.: explicit UUID instead of the deterministic one
      "title": "…",                // opt. (empty is fine)
      "created_at": "2026-06-14T23:47:46+03:00",  // opt., RFC 3339
      "modified_at": "2026-06-14T20:51:05Z",      // opt., RFC 3339
      "system_message": "…",       // opt.; absent → taken from the first system message
      "character_names": { … },    // opt. (same as the profile's)
      "messages": [
        {
          "role": "user",          // user | assistant | system (case-insensitive)
          "text": "…",
          "thoughts": null,        // opt.: the assistant's chain-of-thought (CoT) block
          "timestamp": "…"         // opt., RFC 3339
        }
      ]
    }
  ],

  "settings": {                    // optional: the source's global settings
    "sampling": { … },             // default global sampling
    "interface": {
      "spellcheck_enabled": true,  // opt.
      "dictionaries": ["ru_RU"],   // opt.: enabled spellcheck dictionaries
      "theme": "auto"              // opt.: auto | dark | light
    }
  }
}
```

## Semantics

### Identifiers (idempotency)

An entity's id is derived **deterministically** from `key` — a UUIDv5 over
the key's bytes in a fixed namespace:

| Entity | UUIDv5 namespace |
|---|---|
| profile | `6d696e64-666f-726b-696d-706f72740001` |
| chat | `6d696e64-666f-726b-696d-706f72740002` |

(`6d696e64 666f726b 696d706f7274` = ASCII `mindfork import`; the tail is the
entity tag.)

The converter should derive `key` from a **stable** source attribute
(configuration name, conversation id) — then re-importing updates the same
entities.

The optional `id` field (a canonical UUID string) **overrides** the derived
identifier — for converters that need continuity with data imported earlier
by another path (e.g. the former `import-lamellama` command, which wrote
chats under their original ids).

### Profiles and chats

- A chat's `profile_key` must reference a profile **from the same file**
  (otherwise an error). An imported profile gets mindfork's standard tool set.
- A chat's `system_message`: the explicit field takes priority; if absent,
  the **first** message with role `system` in `messages` is used. Messages
  with role `system` are **not** added to the chat history (in mindfork the
  system message is stored separately from the history).
- Message roles: `user`, `assistant`, `system` (case-insensitive). Any other
  role is an error (with the chat key and message index): service messages
  (tool calls etc.) are not carried into format v1 — the converter must drop
  them or flatten them to text.
- Timestamps are RFC 3339. Missing/unreadable timestamps are not an error:
  `created_at`/`modified_at` fall back to the "epoch" (1970-01-01), a
  message's `timestamp` falls back to the import moment.

### Sampling

The `sampling` object (on a profile and in `settings`) is read into
mindfork's sampling config: only fields known to the app are carried over
(e.g. `temperature`, `top_k`, `top_p`, `min_p`, `max_tokens`,
`frequency_penalty`, `presence_penalty`, `seed`, `thinking`,
`reasoning_effort`, and other `SamplingConfig` fields); unknown fields are
silently ignored. All fields are optional — unset ones stay at their defaults.

### Settings

`settings.sampling` replaces the default global sampling;
`settings.interface` applies the spellcheck/dictionaries/theme. The section
is optional and may be absent entirely.

## Format evolution

- **Compatible extensions** (new optional fields) are added without a
  `version` bump — old apps ignore them.
- **Breaking changes** (renaming/semantic change/new required fields) —
  `version` + 1; the app rejects files whose version is newer than it
  supports.

## Minimal example

```json
{
  "format": "mindfork-import",
  "version": 1,
  "profiles": [{ "key": "p1", "name": "Assistant" }],
  "chats": [{
    "key": "c1", "profile_key": "p1", "title": "First chat",
    "messages": [
      { "role": "user", "text": "Hi!" },
      { "role": "assistant", "text": "Hello!" }
    ]
  }]
}
```
