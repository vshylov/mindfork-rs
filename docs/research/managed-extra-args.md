# Raw `llama-server` arguments in managed settings

> **Status:** forks settled (the user's decision, 2026-09-26): F1 (a), **F2 (b)
> — the embedder too**, F3 (a), F4 (a), F5 (a), F6 (a), F7 (a). The roadmap item of the same name
> ([roadmap.md](../roadmap.md), "Engine and providers"): `ManagedConfig.extra_args`
> exists and `build_args` appends it, but every caller passes an empty list, so
> managed mode can send only the flags that have a field of their own. The item
> names its own cost — a free-form argv into a child process, a field that can
> break a launch in ways no preflight checks, and a security review of its own
> ([cloud-provisioning.md §9](cloud-provisioning.md)). This document is that
> review, the measurements it rests on, and the forks.

## 1. Why, precisely

A large mixture-of-experts model on a consumer GPU is launch-line work:
`--n-cpu-moe N` keeps the experts of the first N layers in RAM, `-ot
<regex>=CPU` places tensors by name. Neither has a field, and neither should —
llama.cpp's option set is hundreds of flags and moves every week. Today such a
user starts `llama-server` by hand and switches to external mode, which costs
them what managed mode is for: the restart on a settings change, the monitor's
relaunch, the preflights, the impersonation server beside it. The same holds
for every other flag the app has no field for — `-t`, `--cache-type-k`,
`--kv-unified-per-slot` (the admission track's F7b), `--chat-template-kwargs`,
`--tensor-split`.

## 2. What exists

- `ManagedConfig.extra_args: Vec<String>` (`shared/api/managed.rs`), appended
  **last** by `build_args`. Both callers — `managed_config` (chat and
  impersonation) and `managed_embed_config` (the embedder), in
  `app/supervisor.rs` — pass `vec![]`.
- `ManagedSettings` (`shared/config.rs`) is `#[serde(default)]`, shared by the
  assistant's and the impersonation engine's `managed` sub-section; the
  embedder has its own `ManagedEmbedSettings`. A change to `config.engine` or
  `config.impersonation_engine` already restarts that server
  (`app/orchestrator/settings.rs`), so a new field needs no restart plumbing.
- The settings screen builds both engines' managed rows in one place
  (`screens/settings/helpers.rs::managed_rows`, groups Server / Model /
  Performance / Speculative), and an MCP server's `args` already round-trips a
  `Vec<String>` through a text field with `shared::cmdline::{join, split}`.
- `mindfork setup --set KEY=VALUE` writes any settings field through the
  config's own types, and `--verify` starts exactly what the app would
  (`managed_config`), so both reach a new field with no code of their own.
- On an early exit the status chip says only *"llama-server exited before
  becoming ready (corrupt GGUF or out of memory? — see logs)"*
  (`ui.err.managed.early_exit`); the server's own words go to the log file.

## 3. Measurements (2026-09-26)

llama.cpp build 11191 (`4b1a27fa0`, MSVC, CUDA), Windows 11, RTX 4090; the
server launched by hand with `bge-m3-Q8_0` as the model unless said otherwise.

| # | Line | What happened |
|---|---|---|
| M1 | `--bogus-flag` | exit 1, `error: invalid argument: --bogus-flag` |
| M2 | `-c abc`, `--n-cpu-moe` (no value), `-ot bad` | exit 1, `error while handling argument "-c": invalid stoi argument` / `…: expected value for argument` / `…: invalid value`, then the option's usage |
| M3 | `--ctx-size=1536` | exit 1, `error: invalid argument: --ctx-size=1536` — llama.cpp has no `--flag=value` form |
| M4 | `-c 1024 -c 2048`; `-c 1024 --ctx-size 3072` | `/props` `n_ctx` 2048 and 3072: the **last** occurrence wins, whatever the spelling |
| M5 | `--port 18110 … --port 18111` | the server listens on 18111 only, and logs `W DEPRECATED: argument '--port' specified multiple times, use comma-separated values instead (only last value will be used)` |
| M6 | `--alias first --alias second` | the model id is `first`: a list-valued option does not follow M4 |
| M7 | `LLAMA_ARG_CTX_SIZE=4096` in the environment | `n_ctx` 4096: `LLAMA_ARG_*` works, and the managed child inherits the app's environment |
| M8 | `LLAMA_ARG_CTX_SIZE=4096` and `-c 1024` | `n_ctx` 1024, with `warn: LLAMA_ARG_CTX_SIZE environment variable is set, but will be overwritten by command line argument -c` |
| M9 | `LLAMA_API_KEY=secret` in the environment | `/health` **200**, `/props`, `/v1/models`, `/v1/embeddings` **401** |
| M10 | a text file named `fake.gguf` | exit 1; the last lines are timestamped `E` records, `… E srv  llama_server: exiting due to model loading error` |

What they mean for this feature:

- **M1–M3: a typo is the likeliest failure, and llama.cpp names it exactly** —
  on stderr, which today reaches only the log. The status chip blames a corrupt
  GGUF or memory.
- **M4–M5: an extra flag that repeats one of the app's own silently wins** —
  `--port` moves the server away from the port the app probes, and `-c` makes
  the settings screen show a context the server is not running. Upstream has
  also **deprecated** the repetition itself (M5), so an override that works
  today is one a future build may refuse to start with.
- **M7: the roadmap's untested route works**, and answers a different
  question: it is process-wide — the chat, impersonation and embedding servers
  all inherit it — and invisible in the settings.
- **M9 is an adjacent defect, independent of this feature**: a user whose
  environment carries `LLAMA_API_KEY` for any other purpose gets a managed
  server the probe calls ready and every request refused.

## 4. Security review

**Who can write the field.** The user on the settings screen; `settings.json`
by hand; `mindfork setup --set`; a restored backup. **Not the model**: tool
effects are `ChatEffect`s scoped to one chat (`features/tools/mod.rs`), the
control tools steer the conversation only, and since the safe-defaults track no
file or code tool reaches the data root where `settings.json` lives
(`features/tools/reach.rs`, [safe-defaults.md](safe-defaults.md) 2a).

**What it adds over what the settings file already grants: nothing new in
kind.** `engine.managed.binary` names any executable, so a restored stranger's
backup could already run anything — the same class as the roadmap's "restoring
a stranger's backup restores its MCP commands". The spawn is argv-direct, no
shell (`tokio::process::Command::args`), so no quoting of ours can become
command injection.

**What it does add: accidents a copied command line carries in.** Four kinds,
each measured or read off `--help` above:

1. **A flag the app already writes** (`-c`, `-ngl`, `-m`, `--port`, …): wins
   silently (M4), repeats a flag upstream has deprecated repeating (M5), and
   leaves the screen showing a value that is not running.
2. **A flag the app's connection to its own child depends on**: `--api-key`,
   `--api-key-file` (M9's lockout, by flag), `--api-prefix` (moves `/v1`),
   `--ssl-key-file`/`--ssl-cert-file` (the client speaks `http://`),
   `--embeddings` on the chat server.
3. **A flag that turns the server into an agent with the host's hands**:
   `--tools` (`read_file`, `write_file`, `edit_file`, `exec_shell_command`, …),
   `-ag/--agent`, `--mcp-servers-config`/`--mcp-servers-json`,
   `--ui-mcp-proxy`. Their own help says *do not enable in untrusted
   environments*; enabled, whoever reaches the port runs commands as the user —
   every local process at the default host, the LAN at `0.0.0.0`. The app's
   whole tool layer (confirmation, `reach.rs`, the sandbox) is bypassed.
4. **A flag that makes the managed server fetch or route**: `-hf`/`--hf-repo`
   and its draft and file variants, `-mu/--model-url`, `-mmu/--mmproj-url`,
   `-dr/--docker-repo`, `--models-dir`/`--models-preset`. Managed mode already
   refuses router mode for exactly this reason (`ManagedConfig::is_runnable`,
   robustness-and-defaults.md §2.1), and [PRIVACY.md](../../PRIVACY.md) lists
   the app's network contacts.

Kinds 2–4 have a door that works and that the refusal can name: start
`llama-server` yourself and connect in **External** mode — the escape hatch the
roadmap item already describes.

## 5. Forks

- **F1 — how the field is stored.**
  (a) `extra_args: Vec<String>` in `ManagedSettings`, edited as one command
  line through `cmdline::join`/`split` — the MCP `args` precedent; the stored
  shape is the launcher's, and `--set` takes a JSON array
  (`'["--n-cpu-moe","20"]'`).
  (b) A `String`, split at launch — `--set` takes the plain line.
  **Recommendation: (a).**
- **F2 — which servers.**
  (a) Chat and impersonation — both `ManagedSettings`, both `managed_rows`, so
  one field lands in both sections for free.
  (b) (a) plus the embedder — one more field on `ManagedEmbedSettings`; its
  likeliest uses (`-c`, `--pooling`) overlap the separate "embedder context
  setting" item and would collide with the `-c` the app writes for it.
  **Recommendation: (a)**; the embedder stays its own roadmap item.
  **Decided: (b)** (2026-09-26). F3 then reads per section: the embedder's
  section has fields for the model and the port only, so its `-c`, `-ngl` and
  `-b`/`-ub` stay allowed (the last occurrence wins, M4), and `--host`, which
  the app pins to loopback for it, is a kind-2 flag there.
- **F3 — the app's own flags (kind 1).**
  (a) Refuse any spelling of a flag that **has a field in this section**,
  naming the field (`-c` → "Context"). Flags the app writes *without* a field
  (`-ub`, `--kv-unified`, `--reasoning-format`) stay allowed — `-ub` is the
  common GPU tuning, and the app writes it only when `-b` is set.
  (b) Allow everything; the last occurrence wins (M4) and the hint says so.
  (c) Refuse every flag the app's line can carry.
  **Recommendation: (a)** — one source of truth per knob, and no line of ours
  that depends on a deprecated behaviour (M5).
- **F4 — kinds 2–4.**
  (a) Refuse, with the message naming External mode.
  (b) Allow, with a warning in the hint.
  **Recommendation: (a).**
- **F5 — where the refusal happens.**
  (a) **Both**: the settings editor refuses on `Enter` (the editor stays open,
  red, with the reason, as a malformed number does today), **and**
  `ServerHandle::launch` refuses before spawning — the guard inside the type,
  since `settings.json`, `--set` and a restore never pass the editor
  ([lessons.md §2](../lessons.md), "a guard inside the type is an invariant").
  One function in `shared` answers both.
  (b) Launch only.
  **Recommendation: (a).**
- **F6 — a launch that fails on an argument (M1–M3).**
  (a) Keep the last stderr lines of the child; on an early exit, the status
  chip carries llama.cpp's own `error…` line (`error: invalid argument:
  --n-cpu-mo`) instead of the corrupt-GGUF guess.
  (b) Leave it to the log file.
  **Recommendation: (a)** — the field turns a rare failure into the common
  one, and the server already says exactly what is wrong.
- **F7 — the environment (M7, M9).**
  (a) Remove from the managed child's environment the `LLAMA_*` variables of
  kinds 2–4 (`LLAMA_API_KEY`, `LLAMA_ARG_API_PREFIX`, `LLAMA_ARG_TOOLS`,
  `LLAMA_ARG_HF_REPO`, …) — the same table, so a refused flag cannot come in
  by the side door, and M9's lockout is gone.
  (b) Leave the environment alone; record M9 in the roadmap.
  **Recommendation: (a)**; `LLAMA_ARG_*` for everything else keeps working and
  is documented in install.md as the process-wide route.

## 6. Plan (one PR, `feat/managed-extra-args`)

1. `shared/api/managed.rs` (or a sibling `llama_args.rs`): the table — option
   groups with every spelling from build 11191's `--help`, each with its
   refusal (its field's label key, or the kind-2/3/4 reason) and its
   environment names; `check_extra_args(&[String]) -> Result<(), Refusal>`.
   `ServerHandle::launch` checks before spawning (F5) and strips the
   environment (F7); the stderr tail feeds the early-exit message (F6).
2. `ManagedSettings.extra_args` (`#[serde(default)]`), copied by
   `managed_config`; the embedder stays `vec![]` (F2).
3. Settings: an "Extra arguments" row in a fifth managed group, the editor's
   error widened from a static key to a formatted message; hint in `en`/`ru`.
4. Tests: the table against every flag `build_args` can emit (no field-owned
   flag escapes F3), each refusal kind, the spawn-time guard with a stub
   binary, the round trip through the field, the stderr tail's pick. Live
   (`#[ignore]`): a real launch with `--n-cpu-moe` on `gemma-4-26B-A4B` and an
   alias the catalogue must report, a control arm with a misspelled flag whose
   error must reach the status, and a check of the table against the real
   binary's `--help` so the next spelling change is caught by a run, not a
   user.
5. Docs: spec §3.4 and §11.6, architecture §5 and §10, install.md §3 (the
   field, and `LLAMA_ARG_*` as the process-wide route), README, CHANGELOG,
   journal/engine.md, roadmap (the item closed; the embedder half stays).
