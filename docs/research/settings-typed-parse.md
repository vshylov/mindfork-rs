# Research: a settings file that is JSON but not settings

**Status:** reproduction and measurements done, 2026-09-28. The forks in §4
**await the user's decision**; nothing but the reproducing test is written.

Found in passing during the colour-modes track, which worked around it for one
field (`interface.theme_mode`, `lenient_theme_mode`) and wrote the trap down
([lessons.md](../lessons.md) §8, "a new *value* of an existing enum is not
additive"). This note is about the defect itself.

**Related:** spec §12.1–§12.2, architecture §7,
[ADR 0006](../decisions/0006-data-schema-versioning.md),
[docs/journal/storage.md](../journal/storage.md), lessons §4 and §8.

## 1. The defect

A `settings.json` that parses as JSON and fails the typed parse into `AppConfig`
resets the **whole** configuration to the defaults, in silence, and the next
save writes the defaults over the file.

1. The start-up gate (`features/data_migration.rs::assess_file`) reads the file
   as an untyped `Value`. At the current schema version it is `UpToDate` and
   passes; only a *migrated* file gets the typed `control_parse`.
2. `launch_tui` loads with `load_config().unwrap_or_default()` — the typed parse
   fails, the error is dropped, the defaults are used. Nothing is logged, nothing
   is shown.
3. The orchestrator saves the config on its first routine write, and
   `write_json` keeps one `settings.bak`.

Two realistic ways to get such a file: a hand edit with a typo
(`"theme": "drak"`), and a file written by a newer version holding an enum value
this binary does not know — an additive change needs no schema bump, so the
downgrade guard does not fire.

Reproduced by `main.rs::tests::repro_one_bad_enum_value_resets_the_whole_config`:
four sections of the user's values and one typo in; `AppConfig::default()` out,
and after two saves the user's values are in neither `settings.json` nor
`settings.bak`. The test asserts the defect and passes on today's code; the fix
turns it over.

## 2. Measurements

Taken on this machine, 2026-09-28, with a scratch probe over
`AppConfig::default()` serialized to JSON (not committed).

### 2.1 How much of the file can do this

| | |
|---|---|
| leaves in a default `settings.json` | 289 |
| leaves that fail **the whole file** when given a value of the wrong type | 289 of 289 |
| leaves that fail it when given an unknown *string* | 205 |
| enum-valued fields (the "unknown value" case proper) | 20, over 15 enum types |
| fields read leniently today | 1 — `interface.theme_mode` |

The 20: `engine.mode`, `embed.mode`, `impersonation_engine.mode`, `tts.mode`,
`embed.convention`, `flash_attn` ×2, `spec_type` ×2, `tools.python_mode`,
`tools.web_provider`, `video.media_resolution`, `interface.theme`,
`interface.auto_title`, `interface.clipboard_osc52`,
`interface.self_model_note_order`, and `reasoning_effort` / `verbosity` in both
sampling blocks. So the defect is not about themes: **any** value in the file is
one keystroke from costing all of them.

### 2.2 What serde says

```
unknown variant `drak`, expected one of `auto`, `dark`, `light` at line 3 column 32
invalid type: string "3", expected u32 at line 1 column 23
```

Parsed from the file's bytes, the error names the value, the values that would
have been accepted, and the line and column. It does not name the key; the line
does. Parsed from a `Value` (what `control_parse` does) the position is lost.

### 2.3 How soon the file is overwritten

At start-up. A default config has `last_active_chat: None`, so the first chat
the orchestrator activates is a switch, and `remember_active_chat` saves. The
user's file moves to `settings.bak`; the second save — any settings change, the
next chat switch — replaces that too.

### 2.4 Every reader of the file, and what it does with defaults

| Reader | With a file it cannot parse |
|---|---|
| `launch_tui` (`main.rs`) | the defect of §1. With the settings go the **stored secrets** (`api_keys`: cloud keys, the backup password), the engine mode (→ `managed`), the interface language (→ `ru`) |
| `backup_config` — `backup`, `restore`, `stats <archive>` | the stored password reads as *none*: `mindfork backup` writes a **plaintext** archive for a user whose settings say "encrypted"; `tools.fs_root` is not packed |
| `run_llama_remove` | the in-use check sees no binary in use and removes a build the settings name, without `--force` |
| `run_import` | runs the gate (which passes), then **saves** defaults plus the imported values over the file |
| `open_config_for_cli_write` — `setup`, `--enable-python`, `--set-binary` | refuses already: the error is propagated. The precedent |
| `try_settings_language` (the CLI's language) | typed parse of the whole file for one field, so a refusal about this file would be printed in the wrong language |
| `profiles.json`, in the orchestrator | not silent and nothing is lost — `load_profiles()?` ends the orchestrator, and the session ends pointing at the log — but the message names neither the file nor the value |

## 3. What holds whichever way the fork goes

- **A file that could not be read whole is never written over.** No call site
  keeps `unwrap_or_default()` on a load whose result is saved or acted on.
- The on-disk format does not change; no schema bump.
- What the user is told is localized and names the file and serde's error.

## 4. Forks

### F1. What a start does with such a file

**(a) Refuse to start** — recommended. The gate gives a file at the current
schema the typed parse it gives a migrated one, from the file's bytes, and
refuses with a localized message: the file, serde's error with its line and
column, that nothing was changed, and the three ways out (correct the value,
restore a backup, delete the file).

- For a typo this is the most useful answer there is: the person has just edited
  the file, and is told the value, the accepted values and the line.
- It is what the gate already does next to it — corrupt JSON refuses, a newer
  schema refuses, and spec §12.2 gives the reason: a silent guess is worse than
  a refusal.
- Nothing is guessed. `engine.mode` falling back to `managed` starts a local
  server the user did not ask for; a refusal starts nothing.
- Cost: an older binary that meets a newer version's enum value does not start
  until the value is edited or the app updated — the downgrade guard's
  behaviour, reached without a schema bump. Lessons §8 already says how a
  growing enum avoids it (a field of its own, or lenient from its first
  release), and (a) does not stop any one field from being made lenient later.

**(b) Load leniently.** On a failed parse, walk the tree, drop every key whose
subtree fails on its own so that its default applies, and log each one.

- The app always starts, and an older binary tolerates every future enum value.
- The typo is reported by a notice after the fact, not at the point of the
  mistake; the app runs on a value the user did not choose.
- The dropped value is gone from the file at the next save, so to keep §3 the
  original has to be set aside first — a new file in the data root that
  `backup`/`restore` must know about — and the TUI needs a start-up notice,
  which it has no surface for today.
- Roughly three times the code of (a), most of it in what surrounds the parse.

**(c) Hybrid** — lenient for an unknown enum value, refusal for a wrong type.

- Everything (b) costs, for the 20 fields, plus telling the two failures apart:
  serde reports both as the same category, so it is either matching on the
  error's text or a `deserialize_with` on each of the 20 fields and a test that
  poisons every string leaf to catch the 21st.
- What it buys over (a) is scenario two only, which the rule in lessons §8
  already covers field by field.

### F2. `mindfork backup` on such a file

**(a) Refuse**, as the start does.

**(b) Best effort** — recommended. `backup`, `restore` and `stats` need two
values: the stored password and `tools.fs_root`. Read them from the raw JSON,
the way the pre-migration backup already reads the password
(`data_migration::stored_backup_password`). A person whose app has just refused
to start is exactly the one who wants a backup before touching the file
(lessons §8, "prefer best-effort to refusal on paths that protect data"), and
the archive stays encrypted. A file that is not JSON at all still backs up, and
the command says that the stored password could not be read.

### F3. `profiles.json` under the same gate

**(a) Yes** — recommended. The same arm, one more message key: a refusal that
names the file and the value instead of a session that ends pointing at the log.

**(b) No** — settings only; profiles keep today's behaviour.

## 5. Plan, for the recommended answers

One pull request, `fix/settings-typed-parse`. No engine, memory or tool path is
touched, so no live run is owed.

1. `data_migration::assess_file`: the typed parse for an `UpToDate` file, from
   bytes; keys `migrate.err.settings_invalid` / `migrate.err.profiles_invalid`
   in both locales.
2. `main.rs`: the start-up load becomes a function that returns the error
   (testable at its real call site, lessons §2); `run_import` and
   `run_llama_remove` propagate; `backup_config` reads raw; and
   `try_settings_language` reads the one field it wants from the raw JSON.
3. Tests: the reproduction turned over (refused, file byte-identical, no
   `settings.bak`); a wrong type; a newer version's value; a valid file still
   passes with no backup made; the plaintext-backup case; each call site.
4. Docs: spec §12.2, [storage.md](../journal/storage.md), CHANGELOG (Fixed),
   and lessons §8 — the entry there describes the trap as open.
