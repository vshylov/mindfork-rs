# Research: robustness and the shipped defaults

**Status:** measurements done, 2026-09-18; the §4 forks decided by the user the same
day — **F1(a), F2(a), F5(a)** at the recommendation, **F3(b)** and **F4(c)** against
it — and the stage lands as **two** pull requests: 4a, the fixes below, and 4b, the
model picker.
Stage 4 of [public-release-readiness.md](public-release-readiness.md) §4 — the
"Should" lines about robustness, the Windows install and the defaults, plus the
one item stage 1 deliberately left with "measure before changing the message".

What this stage is about: **the ways a first run ends badly that are nobody's
fault but ours.** A backend that dies without saying so, a server that reports
itself ready with nothing loaded, an error message that vanishes with its
console window, and hints that name models which no longer lead anywhere. None
of it is new code; all of it is what a stranger meets in the first ten minutes.

**Related:** [public-release-readiness.md](public-release-readiness.md) §2.3 and
§3.2, architecture §4 (the UI loop) and §5 (the supervisor),
[installers.md](../history/installers.md), spec §1.4, §3.4, §4.4,
[docs/journal/ci.md](../journal/ci.md), [docs/lessons.md](../lessons.md) §3, §6.

## 1. What this stage covers

| # | Item | From |
|---|---|---|
| D1 | a panic in the orchestrator leaves a live UI with a dead backend | §2.3 "Robustness" |
| D2 | a managed server with a build but **no GGUF** | §3.2, left for stage 4 with "measure first" |
| D3 | a fatal error on a double-click vanishes with the console window | §2.3 "Windows install" |
| D4 | "already running" exits 0 | §2.3 "Windows install" |
| D5 | `REFERENCE = Ru` — a missing key in an external locale falls back to Russian | §2.3 "Defaults" |
| D6 | the installer does not put `mindfork` on `PATH`, while the app's own hints tell you to run it | §2.3 "Windows install" |
| D7 | the settings hint names models that are one or two generations old | §2.3 "Defaults" |

Deliberately **not** here: the model picker from `/v1/models` (a feature, not a
fix — it needs its own design), `terminal_compat` on legacy conhost (a detection
question with no measurement yet), and the public documents and website, which
are stage 5.

## 2. Measurements

Run on this machine on 2026-09-18, before any design was written (lessons §3).

### 2.1 D2 — a build with no model starts a **router**

The audit could not check what a recent `llama-server` does when the managed
config has a binary and no GGUF; stage 1 recorded the question rather than guess
an error message. Measured now, with the installed build (`cpu-b10883`,
`0.4.0-dev`) and exactly the arguments `build_args` produces minus `-m`:

- it does **not** exit. It prints `starting server in router mode. models will
  be automatically loaded on-demand` and listens;
- `GET /health` → `{"status":"ok"}` — **the readiness probe passes**, so the app
  reports the engine as ready;
- `GET /props` → `"role":"router"`, `"model_path":"none"`,
  `"default_generation_settings":{"params":null,"n_ctx":0}` — a context window of
  **zero**, where the app reads that number for compaction;
- `GET /v1/models` → the models found in this machine's Hugging Face cache, here
  one unrelated OCR model, each with the `--hf-repo` arguments that would fetch
  it;
- `POST /v1/chat/completions` with the model field the app sends → **400**,
  `"model name is missing from the request"`.

So today the failure is: *"the server is ready"*, then every message fails with a
line written for a different audience. And the second half is worse than a bad
message — a router with `models_autoload: true` will **fetch a model from
Hugging Face on demand** if a name happens to match. Managed mode does not offer
router mode anywhere in the UI, so that is a mode nobody asked for. `llama-server`
has no flag to refuse it (`--models-dir`, `--models-preset`,
`--models-autoload/--no-models-autoload` configure it; nothing turns it off) —
the refusal has to be ours, before the process starts.

Where it is today [code]: `managed_chat_setup` returns `NotConfigured` for an
empty **binary** only (`supervisor.rs:461`); `model_path: None` simply omits `-m`
(`managed.rs:161`).

### 2.2 D3 — who owns the console

A console application started from Explorer gets a console of its own, and
Windows destroys it the moment the process exits — so a startup error is printed
to a window that closes before it can be read. `GetConsoleProcessList` answers
whether that is the case, measured both ways on this machine:

| Launch | Processes attached |
|---|---|
| from a shell (this session) | **3** |
| in a console of its own (as Explorer starts it) | **1** |

One attached process means nobody else is there to read what is left behind.

### 2.3 D1 — what a dead orchestrator looks like

[code] `runtime.spawn(orchestrator::run(...))` in `main.rs:323` drops the
`JoinHandle`, and the UI loop drains events with
`while let Ok(event) = evt_rx.try_recv()` (`app/runtime/mod.rs:521`), which reads
a **closed** channel exactly as it reads an empty one: as "no events this tick".
So if the orchestrator task panics, the panic hook restores the terminal, the
task dies, and the UI keeps running and repainting: every command goes into a
channel with no reader, and nothing ever answers. The process stays up until the
user quits it.

### 2.4 D4, D5, D6, D7 — read in the code

- **D4** `run_tui` prints the localized "already running" line and returns
  `ExitCode::SUCCESS` (`main.rs:202`), so a script cannot tell a refusal from a
  clean run. The CLI already uses **2** for a refusal it is sure about (a wrong
  invocation, and B8's no-terminal launch).
- **D5** `REFERENCE = Lang::Ru` (`i18n.rs:63`). `en`/`ru` are at full key parity
  under test, so this is invisible for the two built-in bundles and decides one
  thing only: what an **external** `data/locales/<code>.json` falls back to for a
  key it is missing. Since the source language is English, that answer should be
  English.
- **D6** `mindfork.iss` has no `ChangesEnvironment`, no `[Registry]` entry and no
  `PATH` task — while the empty-feed guidance, README and `install.md` all tell
  the user to run `mindfork llama setup`. After an install, that command works
  only from the install directory.
- **D7** `ui.settings.desc.model_name` names `gpt-4o, gemini-2.5-pro,
  claude-opus-4-8, grok-4.5` (`en.json:1421`). Checked: `gpt-4o` still answers on
  the API but was retired from ChatGPT in February 2026 and is two generations
  behind the current line (GPT-6 Astra / GPT-5.6) [docs]; `claude-opus-4-8` is
  behind Opus 5. A hint that names a model is a hint that ages — which is the
  design question in F4, not just a string edit.

## 3. Decisions taken without a fork

- **N1. The refusal in D2 is the stage-1 shape, reused.** A managed engine with
  no model reports `NotConfigured`, which is already wired to the empty feed's
  "here is how to connect a model" and to the send error. What changes is one
  more condition, and the guidance naming the GGUF row.
- **N2. The embedding server gets the same check.** It takes a model the same
  way; a router that answers `/health` and refuses every embedding request is the
  same failure one screen further.
- **N3. D4 exits with 2**, the code the CLI already uses for "refused to start".
- **N4. D5 is a one-line change plus its test**: `REFERENCE = Lang::En`, and the
  fallback test asserts an English string for a locale missing a key.

## 4. Forks

**Decided by the user, 2026-09-18: F1(a), F2(a), F3(b), F4(c), F5(a); two pull
requests.**

**F1. What a dead backend does to the session.** The UI loop learns that the
event channel is closed — then what?
(a) **Quit, and say why**: leave the alternate screen, print one localized line
naming the log file, exit non-zero. The session's chat is already on disk (the
orchestrator is the sole writer, spec §4.4.2), so nothing is lost that was
written before the panic.
(b) Quit silently, with the reason in the log only. Least code, and a user whose
app vanished mid-answer learns nothing.
(c) Keep the UI up and show a modal: "the engine stopped; the log is here",
letting them read the chat before quitting. Friendliest, but every command is
already unanswerable, so it is a screen that can only say no.
**Recommendation: (a)** — a UI that cannot do anything should not pretend to be
running, and the exit code is what makes it visible to whatever started it.

**F2. The managed server with no model — where the refusal lives.**
(a) **Refuse before the process starts**: `NotConfigured` when `model_path` is
empty, exactly as for an empty binary. No router, no download-on-demand, and the
first-run guidance is already written.
(b) Launch as today and **detect the router** from `/props` (`role: "router"`),
then report a specific error. Truthful about what happened, but the process is
already running, `models_autoload` is already on, and the user has already waited
for a start that should not have happened.
**Recommendation: (a).**

**F3. `PATH` on Windows.**
(a) An **optional task, checked by default**: "Add mindfork to PATH", appending
`{app}` to the user's `Path` (`HKCU\Environment` for the per-user install that
this installer is), with `ChangesEnvironment=yes` so open shells are notified.
The commands the app itself suggests then work.
(b) The same task, **unchecked** by default: nothing touches the environment
unless asked.
(c) Leave `PATH` alone and make every hint print the full path to the binary.
**Recommendation: (a)** — the app's own messages tell people to run `mindfork`,
and an installer that does not make that true is the thing at fault.

**F4. The model-name hint, which ages either way.**
(a) **Name today's models and say where the list lives** — one current example
per provider plus "the provider's model list" in the same line.
(b) **Name no models**: the hint says the field takes the provider's model id and
points at their documentation. It cannot go stale, and it helps a newcomer least.
(c) Fetch the provider's catalogue and offer a **picker**. The real answer, and a
track of its own — the code that reads `/v1/models` exists, the UI does not.
**Recommendation: (a)**, with (c) written into the roadmap rather than smuggled
into this stage.

**F5. `max_tokens: 2048` with thinking on.** The audit flagged it
[unverified]: on a reasoning model the budget covers the thinking too, so a long
chain can leave nothing for the answer. Measuring it needs a reasoning model —
the cloud keys are available, and one run each with the switch on and off decides
it.
(a) **Measure it in this stage**, and raise or unset the default only if it
reproduces.
(b) Leave it out: stage 4 is already six items, and the default has been shipped
this way for every release so far.
**Recommendation: (a)** — it is one measurement, and "the model stopped
mid-sentence" is exactly the kind of first impression this stage is for.

## 5. The live run

Stage 4 touches the supervisor, so a live run is required (AGENTS.md §3) and it
is local: the installed `cpu-b10883` build and a GGUF from this machine.

1. **D2 both arms**: with the GGUF row empty the engine reports "not configured"
   and no `llama-server` process starts; with a real GGUF it starts and answers a
   message as before.
2. **D1**: a panic injected into the orchestrator (a test-only command) ends the
   session with the line and the exit code F1 chooses, and the log holds the
   panic.
3. **D3**: the installed binary double-clicked from Explorer with a fatal startup
   error — the window stays until a key is pressed.
4. **F5**, if (a) is chosen: one reasoning model, thinking on, the default budget
   against a raised one.

## 6. Implementation plan

**Stage 4a — the fixes.** Branch `fix/robustness-and-defaults`, one pull request.

1. **D5** `REFERENCE = Lang::En`, with the fallback test asserting English.
2. **D4** the "already running" refusal returns `ExitCode::from(2)`.
3. **D2** `managed_config`'s result with no `model_path` reports `NotConfigured`,
   for the chat server and the embedding server alike; the existing first-run
   guidance names the GGUF row. Tests: both arms of the condition, and that no
   process is spawned.
4. **D1** the UI loop distinguishes `TryRecvError::Disconnected` from `Empty` and
   ends the session; `main` keeps the orchestrator's `JoinHandle` so the panic
   itself reaches the log, prints one localized line naming the log file, and
   exits non-zero.
5. **D3** a Windows-only `owns_console()` (`GetConsoleProcessList` == 1, measured
   §2.2): when a refusal or a fatal error is printed and nobody else is attached
   to the console, wait for Enter before exiting. Never when stdout is not a
   terminal — that is B8's case, which must stay non-interactive.
6. **D6** an **unchecked** `[Tasks]` entry adds `{app}` to the user's `Path`
   (`HKCU\Environment`, `ChangesEnvironment=yes`), removed on uninstall.
7. **F5** one measurement against a cloud reasoning model: thinking on, the
   default `max_tokens: 2048`, and whether the answer is cut. The default changes
   only if it reproduces.

**Stage 4b — the model picker (D7).** Its own branch, its own measurement of what
each provider's catalogue returns (OpenAI, Gemini, Anthropic, xAI, and an
OpenAI-compatible local server), and its own live run. The stale hint text
(`gpt-4o`, `claude-opus-4-8`) is 4b's to replace: the user chose a picker over a
string edit, so 4a leaves the string alone rather than churn it twice.

## 7. Outcome

**Stage 4a — done, 2026-09-18, live GO.**

| Item | What it is now |
|---|---|
| D1 | `drain_events` tells a closed channel from an empty one; the session ends with a localized line naming the log, a non-zero exit, and the panic in the log (`main` keeps the `JoinHandle`) |
| D2 | an unset model is `NotConfigured` for the chat server and the embedder alike (`ManagedConfig::is_runnable`) |
| D3 | a refusal printed into a console this process owns alone waits for Enter (`shared/console.rs`) |
| D4 | "already running" exits **2** |
| D5 | `REFERENCE = Lang::En`, which also makes `locales export` write an English template |
| D6 | an **unchecked** installer task adds `{app}` to `PATH`, removed on uninstall |
| F5 | the default reply budget is **16384**, with a `settings.json` 2→3 step for installs sitting at the old default |

**Live run** — `managed_without_a_model_starts_nothing_e2e_live`, locally against the
installed `cpu-b10883` build and `gemma-3-4b-it-q8_0`: with no model, `NotConfigured`,
no child owned and **nothing listening on the port** two seconds later (a router would
have been); with the model, `Ready` and a server on the port.

**D3 verified both ways** on the built binary: launched with a wrong flag in a console of
its own it was still alive four seconds later, waiting; from a shell the same refusal
printed and exited 2 at once.

**Two things the tests caught that the design had not:** a migration that *dropped* the
old default would have meant no cap at all (`SamplingConfig::default()` has none, and
serde never consults `AppConfig::default()` for a section that is present), so the step
writes the number; and the reply cap is what a stream reserves in a managed server's KV
pool, so raising it changes admission above one session — not at the default of one,
where there is no pool, and never into a deadlock, since a single oversized stream is
admitted alone. The suites now pin their own cap instead of inheriting the shipped one.

**Not verified here, and left to the owner:** the installer's `PATH` box end to end (the
script compiles against the pinned Inno Setup 7.1.0; installing is a system change), and
a real double-click from Explorer, which `Start-Process` stands in for.

**Stage 4b — the model picker. Done, 2026-09-18, live GO**
([model-picker.md](model-picker.md)): `Enter` on a model row asks the provider
what it serves and offers the list, with "type a name by hand" as its first row;
D7's hint names no model any more, so it cannot age.
