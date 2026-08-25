# Journal — CI: the pipeline and its cost

The workflows themselves — job structure, caches, Actions minutes, the Windows job's runtime, the rented live-test gate on HF Inference Endpoints, and the test-side changes made to keep all of it affordable.

**Reference documents for this area:** architecture.md §12, AGENTS.md §6

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (13)

- Post-M9: cutting GitHub Actions minutes (done)
- Post-M9: skipping the test job for docs-only pull requests (done)
- Post-M9: the remote live e2e gate on HF Inference Endpoints (stages 0–3, done)
- Post-M9: live-smoke diagnostic log in English (done)
- Post-M9: what the first real CI run of the remote gate found (done)
- Post-M9: one Actions cache per job instead of one per branch (done)
- Post-M9: cutting the Windows CI job from 19 minutes (done)
- Post-M9: tool tests run against in-memory storage (done)
- Post-M9: orchestrator fixtures — two convertible, the rest not (done)
- Post-M9: a second chat model on the live gate — Qwen 3.6 27B (stages 1–2, done)
- Post-M9: the rewrite probe's flake — two models, two causes (done)
- Post-M9: a ceiling on every job, and two orphaned workflows (done)
- Post-M9: a containerised test environment — JupyterLab, the app, a CPU stack (done)

### Post-M9: cutting GitHub Actions minutes (done)
- **Trigger**: the `v0.9.4` release run was refused by GitHub with *"The job was
  not started because recent account payments have failed or your spending limit
  needs to be increased"* — the 2000 included minutes of the Free plan ran out.
  Diagnosis note: the run showed up as `cancelled`, but the real cause was one
  matrix job failing at **scheduling** (no steps, 1 s) with `fail-fast: true`
  cancelling the rest; the reason is only visible in the job's **annotation**,
  not in the run/job status. `gh run rerun` is futile while the block is in
  place (attempts 2–4 all failed identically).
- **Where the minutes went** (measured, not estimated — GitHub rounds each job
  up to a whole minute and bills `windows-latest` at **2x**): CI on a PR =
  ubuntu 105 s + windows 467 s + lints 69 s → **20 billable min**; CI on the
  push to `main` = the same thing again → **20 min**; Packaging on `main` →
  **9 min**. The Windows test job alone is 16 of the 20.
- **The redundancy**: GitHub runs `pull_request` against the **merge result**
  (`refs/pull/N/merge`), so the push-to-`main` run re-tests code that has
  already passed on that exact tree — visible as a pair in the run list on every
  single merge.
- **Fix** (two workflow edits, no Rust): (1) `ci.yml` — the `test` matrix is now
  an expression, both OSes on `pull_request` and **Linux only** on `push`
  (`github.event_name == 'push' && fromJSON(...) || fromJSON(...)`); (2)
  `packaging.yml` — the `push: [main]` trigger removed entirely (`workflow_dispatch`
  kept). Per merge: CI 40 → 24 min, Packaging 18 → 9 min, ≈ **43 % less**.
- **Why `main` keeps a Linux CI run** rather than dropping the trigger outright:
  the README CI badge tracks the default branch and would go stale without one,
  and a Linux run still catches a semantic conflict between two separately-green
  PRs. It costs 4 min against the 16 saved by dropping Windows there.
- **Not done** (offered as follow-up): skipping the `test` job for docs-only
  changes. It is the biggest remaining win for this repo (the CLAUDE.md journal
  makes many PRs almost entirely `.md`, and PR #212 burned 20 min on one), but
  job-level path filtering needs either a third-party action or a hand-rolled
  `git diff` gate, and a wrong verdict silently skips the test gate. The `lint`
  job must keep running regardless — `cyrillic_scan` guards the docs themselves.
- Branch protection could not be consulted (403: unavailable for private repos on
  Free), so there are no required status checks to strand.

### Post-M9: skipping the test job for docs-only pull requests (done)
- The follow-up left open above (stacked on `ci/reduce-actions-minutes`). This
  repo's PRs are frequently near-pure documentation — the CLAUDE.md journal alone
  makes that the norm — and each one paid the full **20 billable minutes**
  (PR #212, a docs/packaging change, is the example). A new `changes` job
  classifies the PR's files and the `test` job gains
  `if: needs.changes.outputs.docs_only != 'true'`. `lint` is deliberately
  **untouched and always runs**: `cyrillic_scan` is a guard on the docs
  themselves, so skipping it exactly when only docs change would remove the gate
  where it matters most.
- **The allowlist is the whole design, and it is narrow**: `docs/**` plus
  top-level `*.md`. It is an allowlist rather than a denylist, so anything new or
  unrecognized falls through to running the tests. Files that *look* like docs
  but are read by the build or a gate test — established by grepping
  `include_str!`/`include_bytes!`/`CARGO_MANIFEST_DIR`, not by assumption — and
  therefore must never be in it: **`LICENSE`** (`include_str!` in
  `shared/credits.rs`, asserted by a test), **`artwork/*.svg`** (parsed by the
  `widgets/logo.rs` "code ≡ asset" gate), **`locales/*.json`** (`include_str!` +
  the i18n parity gates), **`Cargo.toml`/`Cargo.lock`** (the credits gates), plus
  `tools/`, `packaging/`, `dictionaries/`, `tests/fixtures/` and `.github/`
  itself. Note `.github/pull_request_template.md` is a `.md` that correctly does
  **not** match (the pattern anchors a top-level name, no slash).
- **Fail-safe by construction**: `docs_only=true` requires a `pull_request`
  event **and** a successful API call **and** the returned file count matching
  the PR's `changed_files` **and** every path inside the allowlist. Any other
  outcome runs the tests. The count check exists because the files endpoint
  truncates on very large PRs, and a partial page could otherwise hide a source
  file and look docs-only. The gate is `!= 'true'`, not `== 'false'`, so an
  empty output still runs the tests.
- **Corrected right after merging**: that value guard was *not* sufficient on its
  own. A failed `needs` dependency skips the dependent job **regardless of
  `if`**, so an infrastructure failure in `changes` silently skipped the test
  gate — observed for real in run 30217314709, where the billing block stopped
  `changes` from starting and `Tests` came out "skipped". Fixed by adding
  `!cancelled()` to the condition (`always()` would be wrong — cancelling the
  workflow must still cancel the job). Lesson: in a `needs` + `if` gate, the
  value and the upstream job's *status* are two separate failure modes.
- **Verified by executing the script**, not by reading it: the `run:` block was
  extracted from the YAML and run against a stubbed `gh` for six scenarios —
  docs-only → skip; code present → run; **truncated listing** → run; empty list
  → run; API failure → run; non-PR event → run. The path pattern was separately
  checked against PR #212's real file list and against each trap above.
- **Cost**: the `changes` job bills 1 minute per PR run (GitHub's per-job
  minimum) and saves 20 on a docs-only PR. Kept as its own job rather than an
  output of `lint` on purpose — reusing `lint` would serialize `test` behind it
  and couple a lint failure to the test gate.
- Applies to `pull_request` only. On `push` the trimmed Linux-only run from the
  previous commit is already just 4 minutes, and the `before`-SHA edge cases
  (branch creation, force push) would add real risk for very little gain.

### Post-M9: the remote live e2e gate on HF Inference Endpoints (stages 0–3, done)
- **The mandatory live gate stopped depending on one machine.** AGENTS.md §3
  requires a live run for anything touching engine / memory / tools, and until
  now that meant `run_all_tests.bat` → `http://192.168.1.20:8000/v1`: not
  reproducible by anyone else, not runnable in CI, and a llama.cpp regression
  catchable only by hand. Research
  [docs/research/remote-e2e-gpu.md](../../docs/research/remote-e2e-gpu.md) (forks
  **R1–R8 accepted by the user as recommended, 2026-07-28**), plan
  [docs/history/remote-e2e-hf.md](../../docs/history/remote-e2e-hf.md). Branches
  `spike/hf-endpoint-probe` (stages 0–1) and `feat/e2e-hf-runner` (stage 2).
- **Why HF Inference Endpoints and not a rented pod** (R1a): the survey's real
  question was not price but *"can we guarantee the GPU is released when the run
  crashes"*. No rented-pod provider gives one — you build it, and every external
  watchdog is another machine that can also fail. A managed endpoint moves the
  guarantee into the platform: idle auto-scale-to-zero **is** the dead-man's
  switch, so the worst case is one wasted idle window rather than a GPU running
  until someone notices. RunPod is ~4× cheaper per run; at ten runs a month that
  gap buys away an entire class of problem for ~$7. HF also runs **its own
  llama.cpp engine** — a real `llama-server`, so the llama.cpp-specific paths
  (request-body extensions, `--jinja` tool calling, `/health` + `503 Loading
  model`, embedding batch behaviour) are genuinely exercised, which no
  vLLM-backed serverless option would do.
- **Stage 0 — the probe (`tools/hf_probe.py`), verdict GO.** Seven unknowns, all
  settled against real throwaway endpoints for ≈ $0.17, mostly by reading the
  API's own 422 bodies (the script uses raw REST rather than `huggingface_hub`
  precisely so a rejected payload *teaches* the schema). Two findings improved
  the design: **`LlamacppMode` has an `embeddings` value**, so bge-m3 is served
  by the llama.cpp engine itself — the *same GGUF and quantization the
  similarity gates were calibrated on* — instead of TEI as §3.1 of the research
  had reasoned; and **the container `url` is ours to supply**, so the build can
  be pinned to a tag instead of tracking `master`, retiring the reproducibility
  caveat. Also: `ctxSize` is an explicit field (asked 16384, got `n_ctx=16384`),
  not the indirect Max Tokens × Max Concurrent Requests story in the docs, and
  deploy took **21 s** for a 17.65 GB model, because HF serves the weights from
  its own storage.
- **The trap worth remembering:** `EndpointType` is `public | authenticated |
  private`, and the API **silently coerces** an unknown value instead of
  rejecting it. The older wording `protected` produced a `private`
  (PrivateLink-only) endpoint no CI runner could reach — with a 200 and a
  healthy-looking response. The client now refuses to continue when the echoed
  type differs from the requested one.
- **Stage 1 — the enabling change** (~10 lines, useful on its own):
  `shared/api::live_client(url_var, key_var)` replaced six hand-rolled
  `OpenAiClient::new(env)` sites, and `probe()` now sends the key too. An unset
  or empty key sends no header — byte-for-byte the previous behaviour against a
  local `llama-server` — so this only *adds* the ability to point the same
  smokes at any authenticated OpenAI-compatible server. The `probe()` half is
  load-bearing rather than cosmetic: without it an authenticated `/health`
  answers 401, and 401 is not 503, so the probe reported "ready" whatever the
  key was and the two supervisor smokes would have passed for the wrong reason.
- **Stage 2 — the runner.** `tools/e2e_hf.py`: create both endpoints → wait for
  `running` → **wait for `/health`** → run the suite → delete and verify. Three
  departures from the plan's sketch, each earning its keep:
  - **`tools/hf_api.py` — the client was extracted and shared** with the probe
    rather than copied. Two copies of the create payload would drift the moment
    the schema moved, and two copies of the cleanup would mean two places where
    a bug leaks a billing GPU.
  - **The tests are compiled before the GPU exists** (`cargo test --no-run`), so
    a cold runner does not spend minutes of billed L40S time linking, and a
    build error costs nothing at all.
  - **Cleanup sends every DELETE before verifying any of them.** A cancelled CI
    job gives the handler ~7.5 s before SIGKILL; the calls that stop the meter
    must not queue behind a confirmation round-trip for the previous endpoint.
- **`running` is not loaded** — encoded as `wait_healthy()`, and it is the same
  distinction `OpenAiClient::probe()` exists to draw: HF's `running` means the
  container is up while `llama-server` still answers `503 Loading model`.
  Gating on the endpoint state alone fails the suite's first request.
- **The failure drill found a real leak — not the one it was designed to find.**
  The drill script itself crashed on a cp1252 encode error while echoing the
  runner's output; that broke the runner's stdout pipe, and **both endpoints
  leaked**. Root cause: `cleanup()` emptied the name list *before* its first
  `print`, that `print` raised `BrokenPipeError`, and the `atexit` re-entry then
  found nothing to do. Two fixes, both about not depending on being able to
  talk: prints inside cleanup go through a `say()` that swallows I/O errors, and
  **a name leaves the list only once its endpoint is proven gone**, so a
  cleanup that dies partway is retryable instead of amnesiac. Verified against a
  stubbed HTTP layer (deletes with a dead stdout; a crash mid-cleanup leaves the
  rest retryable; DELETEs all precede the verifications).
- **The sweeper fails safe towards keeping.** `e2e-*` endpoints older than 90
  minutes are deleted hourly, but an endpoint whose `createdAt` cannot be parsed
  is **kept and reported loudly**: deleting one could kill a run still using it,
  destroying real work for a false red, whereas a leak is already money-bounded
  by scale-to-zero. The 90-minute threshold must stay above the live job's
  45-minute timeout, or the backstop becomes a saboteur.
- **A checked assumption that was wrong.** The sweeper's decision logic was
  exercised against a fabricated listing before spending anything, and that
  caught a genuine bug: the fractional-second truncation in the timestamp parser
  also ate the digits of the timezone offset. The live API emits exactly
  `"2026-07-28T17:01:14.686Z"`, so this was on the main path, not a corner —
  every endpoint would have read as "age unknown" and never been swept.
- **Workflows:** `e2e-live.yml` (`workflow_dispatch` only — R5a: a live run is a
  considered act, ~$1 and ~25 min, and the non-hermetic smokes would flake
  unattended) and `e2e-sweeper.yml` (hourly `cron`, active only once on the
  default branch). Inputs reach the shell through the environment, never
  interpolated into a `run:` script. The job warms the npm cache before the GPU
  exists, since a cold `npx` can outlast the MCP smoke's 120 s readiness
  timeout.
- **No Rust changed in stage 2** — **1486 unit tests green** (the +2 over the
  previous entry are stage 1's), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. **No CHANGELOG entry**: dev infrastructure with no
  user-visible effect (AGENTS.md §4).
- **Smoke — GO** (2026-07-28, `gemma-4-31B_q4_0-it.gguf` on **nvidia-l40s** x1 +
  `bge-m3-q8_0.gguf` on **nvidia-t4** x1, aws us-east-1, llama.cpp
  `server-cuda`, `authenticated`, ctx 16384): **69 passed, 0 failed** —
  `cargo test -- --ignored --nocapture --test-threads=1`, both endpoints ready
  in **41 s**, suite **929 s**, total **970 s**, ≈ **$0.62**. Of the 69, **41
  actually exercised the endpoints**; 28 skipped for want of local assets or
  other credentials (11 Python sandbox, 12 cloud keys, 4 `MINDFORK_EMBED_URL_ALT`
  — stage 3, 1 managed `MINDFORK_LLAMA_BIN`). Both endpoints deleted and
  verified gone; `list` empty. This is the first fully green remote run — stage
  0's was 67/2, and both failures were fixed on that branch beforehand.
- **Failure drill — GO**: the runner was signalled **58 s into a live suite**;
  the handler deleted and verified both endpoints, exited 130, and a *separate*
  process confirmed none remained. The only difference from a cancelled CI job
  is which signal arrives (SIGBREAK on Windows, SIGINT there) — the handler and
  cleanup path are the same, and SIGBREAK is registered precisely so the drill
  can be run on a dev box.
- **Stage 3 — the second embedder (done).** Four smokes (`embed_guard` ×2,
  `reembed`, `embed_prefix`) guard the embedding-model-change track and need a
  *second, different* model via `MINDFORK_EMBED_URL_ALT`; without it they skipped
  **while reporting ok**, which is the exact failure a gate exists to prevent. A
  third llama.cpp endpoint on a T4 now serves
  `Ralriki/multilingual-e5-large-instruct-GGUF` / `…-q8_0.gguf` — the same
  quantization as the local stand, since the calibration constants were measured
  against that file, and deliberately a model that is **also 1024-d**, because
  the whole point of `same_dimension_model_swap_detected_live` is that no
  dimensionality check can tell the two apart. **No Rust change was needed**:
  stage 1 had already routed those smokes through `live_client`, key variable
  (`MINDFORK_EMBED_KEY_ALT`) included. **On by default** (`--no-alt-embed` opts
  out) against the plan's "optional" — ~$0.13 of a ~$1 run is the wrong thing to
  optimise when the alternative is four memory-critical smokes silently not
  running.
- **Smoke — GO (stage 3)**, and the interesting part is *how* green: the rented
  endpoints **reproduce the LAN stand's measurements to 3–5 decimals**, which is
  the evidence that matters when swapping the infrastructure under
  calibration-sensitive tests. bge-m3 calibrated to **0.41317 / 0.81843**
  against the reference constants 0.4128 / 0.8176; e5 to **0.78967 / 0.94557**
  against the journal's earlier live figure of 0.78968 / 0.94561 — identical to
  four decimals — mapping 0.72 → 0.9080 and 0.85 → 0.9580 exactly as designed;
  the prefix margins came out `e5 0.1223 → 0.1791` and `bge 0.4417 → 0.3330`
  against 0.1213 → 0.1789 and 0.4406 → 0.3317 measured locally. **5 passed, 0
  failed** (the 4 model-change smokes + the embedding readiness probe), ready in
  108 s, suite 78 s, total 187 s, ≈ $0.15; all three endpoints deleted and
  verified.
- **Still deliberately not done**: a *scheduled* live run; the managed-server
  smoke, which needs a child process of our own and so cannot run remotely at
  all; and the cloud-provider smokes, which need their own keys.
- **Merged in on the way**: `refactor/live-test-log-english` (the live-smoke log
  translation and the print-macro lint that closes the gap by position rather
  than by discipline). Not scope creep — the branches collide by construction:
  both edit `orchestrator/tests/live.rs`, and the stricter `cyrillic_scan.py`
  flagged 47 lines here without the translation, so CI's lint would have gone
  red whichever side landed first.

### Post-M9: live-smoke diagnostic log in English (done)
- **The developer-facing log of the live smokes was half-Russian** — 31 lines of
  `eprintln!` labels across `orchestrator/tests/live.rs` (29) and `tests/mcp.rs` (2)
  ("session 1: tools=…", "self-notes (observations) in DB: …", "gate showed a similar
  observation: …" were all Russian). Now English, matching the convention flipped by
  the english-source migration — and the precedent set there for provisioning progress:
  "it is a developer-facing test log". Branch `refactor/live-test-log-english`.
- **Why it survived the migration**: `tools/cyrillic_scan.py` allowlists test files
  **wholesale** (`path.endswith("tests.rs") or "/tests/" in path`) — a deliberate
  allowance, since these files legitimately hold Cyrillic fixture data and `ru`-locale
  assertions, and the scanner cannot tell a label from a fixture. So the gate was never
  going to catch it; it only became visible once the live suite started running in CI.
- **The scope line is what matters here** — only label text inside print macros moved.
  Untouched: the prompts sent to the model, the `ru`-locale substrings the gate
  assertions match (the `r.contains(…)` checks in `self_model_gate_e2e_live`,
  `summary_gate_e2e_live`, `trait_gate_e2e_live`,
  `self_consolidation_overview_e2e_live` and `recall_includes_self_e2e_live`),
  note/document fixture bodies, the A2 calibration probe pairs, and the TTS
  config/prompt strings. Translating any of those would have quietly stopped the tests
  testing what they test.
- **The one line that named a Russian value in its label** — how many notes cite the
  RAG source — was **not** solved with an opt-out marker but by binding that (Russian)
  source name once to a `const SOURCE` and interpolating it (`notes citing
  {SOURCE:?}`). That deduplicates a literal that had been repeated three times in code
  positions (two DB lookups + the log, where a typo in one would have failed
  confusingly), keeps the log naming the actual source, and leaves the **label** pure
  English. Values are data; labels are prose — interpolation is what separates them,
  and it is now the documented escape hatch.
- **A mislabel fixed in the same log** (the A2 calibration smoke, spotted while
  translating): both loops printed `MATCH?`, but the second is the *non*-match loop —
  so the log was already wrong in English. Now `MATCH?`/`NON-MATCH?`, padded to a
  common width, since the point of that smoke is eyeballing the two groups' cosines to
  place a threshold between them, and misreading which group a row belongs to is
  exactly the error it invites.
- **The gap is now closed by the gate, not by discipline** (`tools/cyrillic_scan.py`):
  test files stay allowlisted wholesale — the scanner cannot tell a fixture from a
  label **by file** — but it now can **by position**. A new `print_fmt_spans` tracks
  the format string of `print!`/`println!`/`eprint!`/`eprintln!` across lines (the
  string routinely opens on the `eprintln!(` line and carries its text on
  `\`-continuation lines), and Cyrillic inside that span translates even in a test.
  Deliberately narrow: **only the first literal**, so a value argument like
  `eprintln!("gate: {}", r.contains("<ru string>"))` is untouched; and
  **`assert!`/`panic!` are excluded**, because their *condition* sits in the same macro
  call as the message and flagging them would hit precisely the ru-locale assertion
  data that must stay. `write!`/`writeln!` too — in tests they usually build an
  expected-value buffer.
- **Mutation-tested in both directions** (a throwaway probe file, since the rule is
  cross-line state that a single-line reading can't confirm): it flags a single-line
  Russian label, a label on a **continuation line**, and one whose format string opens
  on the line *after* the macro; it does not flag ru-locale assertion data, fixture
  prompts, a Cyrillic literal in a value argument, an interpolated value, or a line
  carrying the `cyrillic-ok` opt-out. Repo-wide the rule has a **clean baseline** — a
  survey before writing it found exactly one candidate line, the one now interpolated.
- **No live run needed** (AGENTS.md §3): these are log strings and a lint — no behavior
  change, no engine/memory/tool path touched, and the changed lines only execute under
  `--ignored`. `--all-targets` compiles the ignored tests, so the `const`/interpolation
  change is still compile-verified. **1484 unit tests green** (count unchanged —
  string-only), 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan` clean. No
  CHANGELOG entry (§4: purely internal tests/tooling). Convention recorded in
  **AGENTS.md §3**.

### Post-M9: what the first real CI run of the remote gate found (done)
- The gate's first `workflow_dispatch` on GitHub (run 30396557877) is the run
  that mattered: the pipeline worked end to end — build → three endpoints →
  health gating → suite → cleanup, ~24 min inside a 45-min timeout, **62
  passed** — and everything it got wrong was invisible from a developer machine.
  Branch `fix/e2e-gate-ci-findings`.
- **A false `CLEANUP FAILED`, and it is the serious one.** The DELETE returned
  200 and the verification GET **~400 ms** later still saw the endpoint: HF
  acknowledges the delete and the endpoint disappears a moment afterwards. Only
  the *last* endpoint hits it — cleanup sends every DELETE before verifying any
  (the ~7.5 s SIGKILL window), so the earlier ones get incidental delay for free.
  Two consequences, the second worse than the first: the loudest signal in the
  system cried wolf over an endpoint it had genuinely deleted, and **exit 3
  masked the four real test failures**. The fix rests on a distinction worth
  keeping: **the DELETE is the action, the GET is only the proof**, so only the
  proof may wait — `verify_gone` now polls for ~10 s, which is safe on every path
  precisely because the meter is already stopped. A real leak still exits 3
  (pinned by a test).
- **The alternate embedder scaled to zero mid-suite** → the smoke that woke it
  got a `503`. Systematic, not a flake: `embed_guard` uses it early and
  `embed_prefix` some twenty minutes later, past the 15-minute idle window. The
  local stage-3 run could not have caught this — `--filter embed` packed all four
  smokes into 78 s. The obvious fix (widen the window) was the wrong one:
  **that window *is* the leak ceiling**, the thing that bounds a crashed run and
  the reason this platform beat a rented pod. One knob was doing two jobs. A
  keep-alive daemon thread now issues a real inference request every 5 minutes
  while the suite runs, keeping `lastUsedAt` fresh and leaving the guarantee
  intact (user's decision, 2026-07-29); pings are best-effort and can never fail
  the run.
- **Two smokes cannot run on a GitHub runner at all**, and were failing for
  environment facts rather than regressions: `plays_generated_tone_live` needs a
  sound card (ALSA finds none on a headless runner), and
  `live_search_returns_results` needs an IP that search engines do not throttle —
  a datacenter one is throttled far harder than a home one. Both now **skip** on
  the condition, as the sandbox and cloud-key smokes already do; a gate that is
  permanently red for an environment fact stops being read. The web one matches
  the **bundle key** rather than the prose, so it survives the locale, and leans
  on a distinction the tool already drew between "throttled" and "broken".
- **Left alone**: `simple_generation` returned an empty response once. The same
  model and stack passed it forty minutes earlier locally, so it is a flake until
  it recurs — worth naming rather than silently hardening around.
- **Smoke — GO, in the environment that produced the failures** (run
  30399806549, a second dispatch on the fix branch — three of the four are
  invisible anywhere else): **66 passed, 0 failed**, suite 1120 s, endpoints
  ready in 257 s. Each fix left its own evidence: `keepalive: 3 round(s) of
  pings` with the alternate embedder never leaving `running`, so
  `conventions_behave_as_measured_live` passed; `skip: no audio device
  available`; `skip: every search provider is throttling this IP`; all three
  deletes verified `gone` and exit 0. `simple_generation` passed, which is what
  makes calling it a flake honest rather than convenient.
- Gates green: **1486 unit tests**, 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan` clean. No CHANGELOG entry — dev infrastructure and tests
  (AGENTS.md §4).

### Post-M9: one Actions cache per job instead of one per branch (done)

- **Found while reading the logs of the SonarQube PR**, not by looking for it:
  the repository's Actions cache stood at **10.37 GB in 21 caches against a
  10 GB cap**, i.e. GitHub was already evicting by age. The listing showed the
  cause — five and six near-identical copies of the same key
  (`v0-rust-test-Windows_NT-x64-2afb1257-…`). A cache belongs to **the branch
  that wrote it**, so with `save-if` at its default every PR branch stores its
  own copy of the same build. Branch `ci/cache-save-on-main`.
- **Why it costs minutes rather than just space**: eviction is by age and
  repository-wide, so a stream of per-branch copies pushes out the caches that
  every run depends on, and the next `lint`/`test`/`sonar` builds from scratch.
  That is the same currency the trimmed matrix and the docs-only skip were
  bought with.
- **`save-if: ${{ github.ref == 'refs/heads/main' }}`** on the three `ci.yml`
  jobs: one copy per job, written by the push-to-`main` run, and a pull request
  still *restores* it — GitHub lets a branch read its base branch's caches. Per
  PR this drops what gets written from ~1.9 GB (lint 317 MB + Linux test 515 MB
  + sonar 634 MB + Windows test 445 MB) to just the Windows one.
- **Windows is the deliberate exception** (`|| runner.os == 'Windows'`), and
  getting it wrong would have been worse than the bug: the `main` matrix is
  Linux-only *by design* (the minutes work — `pull_request` already runs against
  the merge result, and Windows bills at 2x), so nothing would ever write a
  Windows cache and **every** PR would rebuild it from scratch on the expensive
  runner. Seeding it by adding Windows to the `main` matrix would cost ~16
  billable minutes per merge — precisely what was removed.
- **Scoped to `ci.yml`**: `packaging.yml`/`release.yml`/`e2e-live.yml` also use
  the action, but run on a tag, a dispatch, or a `packaging/**` PR, so they are
  not what churns — and two of them are rare *and* expensive, where a cold build
  hurts most.
- **Along the way**: `sonar.python.version=3.10, 3.11, 3.12` in
  `sonar-project.properties` — the only WARN the scan emits is the Python
  analyzer saying it assumes "all of Python 3" for `tools/*.py`. The list is what
  actually runs them (3.10 on the development machine, 3.12 on the ubuntu-24.04
  runner), not a guess. It rides this PR because that one already pays for a full
  test run and is the same CI plumbing; a docs-only PR would not have carried it,
  since `sonar-project.properties` is deliberately outside the docs allowlist.
- **Verification had to wait for the merge** — the property is "a PR branch stops
  writing Linux caches", which cannot be observed while the change is still on a
  branch. **Confirmed the same day** (2026-08-05), and earlier than expected,
  because a stacked PR already carried the rule: `gh cache list` by ref shows
  `refs/pull/259/merge` and `refs/pull/260/merge` holding **only** the Windows
  test cache, while `refs/pull/258/merge` — from before the change — still has a
  `sonar` copy, and the Linux trio (`lint` 317 MB, `sonar` 634 MB, `test-Linux`
  515 MB) now exists once, under `refs/heads/main`. Usage went **10.37 GB / 21
  caches → 9.65 GB / 19** and keeps falling as older per-branch copies age out.
  The YAML parses and all three `save-if` expressions render as intended; **1833
  unit tests** unchanged (no Rust code touched); no live run required (AGENTS.md
  §3) and no CHANGELOG entry — dev infrastructure with no user-visible effect (§4).
- **A property of the docs-only classifier, learned the same day**: it reads the
  **whole PR file list**, not the last push. Merging a CI branch into a docs PR
  therefore makes that PR non-docs-only, and it correctly runs the full matrix —
  which is what happened when this stack was merged child-first.

### Post-M9: cutting the Windows CI job from 19 minutes (done)

- **Asked directly**: CI was taking up to 20 minutes, most of it Windows tests —
  and whether to move to `cargo-nextest`. Branch `ci/windows-speedup`. A simple
  task by AGENTS.md §1 (CI configuration plus a test-only fixture change, no
  cross-layer contract), so no design doc; the one genuine fork — billable
  minutes versus wall clock — was put to the user instead (**decided
  2026-08-05**, warm the cache on `main`).
- **The Windows job was the entire CI wall clock.** Everything else finishes
  inside 4 minutes, so 19 minutes *was* the Windows job. Measured on run
  31016421114 rather than estimated: **7m31s cold compile + 9m09s test run +
  1m51s cache save**.
- **The cache was never warm, and the cause was precise.** The `rust-cache` step
  completed in **4 seconds** — a miss, not a restore. The `main` matrix was
  Linux-only, so a Windows cache was never written to a ref a pull request can
  read: GitHub scopes a cache to the branch that wrote it and only lets a branch
  read its **base**. So every PR paid a full cold build and then spent 1m51s
  saving 445 MB to its own `refs/pull/N/merge`, which nothing could ever restore
  and which dies with the PR — the cache list showed eight such copies. Checked
  the obvious escape hatch too: a *second* run on the same branch was also cold.
  Now `main` builds Windows without running the suite (it already passed on the
  PR; building is what fills the cache), **kept in the same job** because
  rust-cache embeds the job id in the key — visible in the key names themselves
  (`v0-rust-test-…`, `v0-rust-lint-…`, `v0-rust-sonar-…`). A separate warming job
  would have silently written a key the test job could never restore, i.e.
  warmed nothing while looking correct.
- **~800 fsyncs in two seeding fixtures.** `fragmented_db` and backup's
  `seed_fragmented_db` inserted 200 rows through `rag_insert`, which issues two
  statements in autocommit — and `delete_matching` loops the same way, so
  batching only the inserts would have fixed half of it. Now one transaction per
  phase, and deliberately **two** phases: the file has to grow and only *then*
  have pages freed, or no freelist is left behind and the tests stop testing
  anything. Assertions untouched and verified to still hold with margin — 48% of
  the file is free pages before the vacuum, and the vec0 index still joins by
  rowid after it. `Db::batch` is `#[cfg(test)]`, so it does not exist in a
  non-test build. Locally at the runner's four threads: the compaction tests
  **19.35s → 0.49s**, the full suite **68.67s → 45.36s**.
- **Measured result**, still with a cold cache since `main` had not yet saved
  one: job **19m00s → 15m00s / 16m22s / 15m29s** over three runs, test run
  **549.1s → 359.1s / 469.0s / 376.5s**, cache save **1m51s → 2s**. The third
  run is the one with the Defender step already removed, and it lands between
  the other two — confirming the removal cost nothing, as
  `RealTimeProtectionEnabled = False` predicts. The remaining ~6 minutes of cold
  compile go on the *next* pull request, once this merge leaves a warm Windows
  cache behind.
- **Confirmed after the merge** (2026-08-05): the warming run left
  `v0-rust-test-Windows_NT-x64-2afb1257-dc2e291a`, 445 MB, on
  `refs/heads/main` — the **same key** the pull-request job looks for, which was
  the whole point. It cost 9m31s of build plus 1m06s to save, i.e. **~19
  billable minutes** per merge rather than the ~15 estimated beforehand; the
  estimate in the workflow header was corrected to the measured figure.
- **The warm pull request, measured** (run 31033936114 — this very docs PR,
  which touches `.github/workflows/ci.yml` and is therefore deliberately outside
  the docs-only allowlist, so it ran the full matrix and served as the
  measurement without anyone having to manufacture a throwaway PR). The Windows
  job went **19m00s → 9m11s**, and the breakdown is where the shape of it shows:
  cache restore 4s (a miss) → **32s** (a real 445 MB restore), compile **7m31s →
  1m45s**, cache save 1m51s → 1s, test run 549.1s → 386.1s. The compile figure
  is the one that matters — it is exactly the predicted behaviour, dependencies
  coming from the cache and only our own crate rebuilding, which is also what
  the Linux `lint` job had been demonstrating on every pull request all along.
  **The compile half of the win is structural and reproducible; see the next
  entry for why the total is not.**
- **The second warm run corrected that headline, and it is worth keeping the
  correction rather than the claim.** Run 31034769688, the very next push to the
  same branch, came in at **18m26s** — the cache behaved exactly as designed
  (48s restore, compile **1m36s**, confirming the structural fix twice over),
  but the **test phase took 926.8s against 386.1s** on identical code. Two
  pieces of evidence in that same run rule out "a uniformly slow machine": the
  Linux job ran the suite in its usual **44.6s**, and the Windows *compile* in
  the very same job was fast. So it is the Windows test phase specifically.
  Collected figures for it, post-fixture-fix: **359.1 / 469.0 / 376.5 / 386.1 /
  926.8s** — four clustered around ~390s and one 2.4x outlier. The honest
  summary is therefore: compile reliably drops ~7m31s → ~1m40s, the cache save
  reliably drops 1m51s → ~1s, and the total lands anywhere between **~9 and ~18
  minutes** depending on how the test phase happens to run. The earlier "~9
  minutes" was one lucky sample stated as a result.
- **The lead was chased and settled: it is disk I/O, not a timeout-sensitive
  test.** A throwaway workflow on its own branch (`push`-triggered, so it needed
  no pull request and could not touch `ci.yml`) ran the suite once under nextest
  purely for its per-test timings — the reason nextest was borrowed rather than
  adopted, since stable libtest has no `--report-time`. Run 31043133962 (deleted
  on 2026-08-21 with its orphaned workflow — see the "a ceiling on every job"
  entry; its `nextest.log` artifact was downloaded first), a
  *normal* instance (438.2s wall against 52.3s local, the usual ~8x), joined
  against local per-test times for all 1833 tests:
  - **The timeout hypothesis is dead.** The genuinely timeout-bound tests are
    **1.0x** — `embed_external_url_is_available` 2.068s on CI against 2.065s
    locally, `wait_until_ready_bails_on_early_exit` 2.074s against 2.068s. A
    fixed 2s timeout is a wall-clock constant and does not stretch on a slow
    machine, so those tests cost the same everywhere and cannot produce a tail.
  - **The slowdown is broad but *not* uniform, and it tracks I/O.** Median
    per-test ratio **3.9x** (p10 2.2, p90 6.0), while the worst offenders run
    **8–19x**: `consolidation_overview_uses_the_calibrated_threshold` 18.75s
    against 1.34s, `note_cite_source_links_and_recall_shows_it` 14.52s against
    0.90s, and so on — every one of them a notes/rag/search test that goes
    through `ctx_with_storage`, which builds a **tempdir plus a file-backed
    SQLite** (`Storage::open(Paths::with_root(…))`). CPU-bound tests sit at the
    3.9x median; disk-bound ones are penalised several times over.
  - **Parallel efficiency corroborates it**: the per-test times sum to 876.1s
    against a 438.2s wall, i.e. only **2.0x** from four cores, where locally the
    same suite gets **4.4x** (231.3s over 52.3s). Losing more than half the
    parallelism is the signature of contention on one shared resource — four
    concurrent fsync-heavy tests serialising on the disk — not of a slow CPU.
  - **Shape**: a long flat tail rather than a few fat tests — the top 10 are
    12.6% of the total and it takes **100 tests to reach 47%**. So there is no
    single test to fix; the tail is the aggregate.
- **Groundwork that follows from it** — **done**, see the next entry:
  `ctx_with_storage` handed every tool test a file-backed SQLite that almost
  none of them need.
- **A hypothesis of mine that the measurement killed — twice over.** I
  attributed the runner being ~8x slower than a local Windows box at the same
  four-thread parallelism (37s local vs 549s) mostly to Defender scanning every
  file rustc writes, and wrote that into the workflow as the exclusion step's
  rationale. First the arithmetic undercut it: the test run improved 34.6% while
  the fixture fix alone had predicted 33.9% locally, leaving nothing for
  Defender to explain. Then the direct check settled it — the step was made to
  log `Get-MpComputerStatus`, and the runner answered
  **`RealTimeProtectionEnabled = False`**. Defender is already off on the image,
  so excluding paths from a scanner that is not running buys nothing; the step
  was removed and the comment now warns against re-adding it without checking
  that flag. The residual gap is the runner's CPU and disk.
- **Runner variance is large enough to matter when reading these numbers.**
  Three post-fix runs gave test runs of 359.1s, 469.0s and 376.5s — a 30%
  spread, with the middle one an outlier. So the honest attribution is that the
  controlled local measurement (68.67s → 45.36s at four threads) is the
  trustworthy one, and single-run CI comparisons here should not be read to two
  significant figures. Worth knowing before anyone tunes this workflow against
  one green run.
- **cargo-nextest — evaluated and rejected, measured rather than reasoned.**
  Slower here at both parallelism levels: **46.3s vs 37.4s** at full parallelism
  and **52.3s vs 45.4s** at the runner's four threads. The reason is structural:
  this is a single binary crate whose 1833 tests live in one executable, and
  nextest runs each test in its own process, which Windows charges for. It also
  does nothing about compilation, which was the larger half of the job. Its one
  attractive feature here, `--partition` sharding, multiplies the compile cost
  across shards — exactly what the cache warming just bought back. It did earn
  its keep once, though: its per-test timings are what found the five slow tests,
  since `--report-time` needs nightly on stable libtest.
- **A bug caught by validating the YAML rather than by eye**: the matrix was
  first written `os: '["ubuntu-latest","windows-latest"]'`, which GitHub expands
  only inside a `${{ }}` expression — as a plain string it would have produced a
  single matrix entry with that literal name.
- **1833 unit tests green** (count unchanged — a fixture optimization), 76
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No
  live run required** (AGENTS.md §3): CI configuration and a `#[cfg(test)]`
  fixture — no engine, memory, tool or provider path is touched. No CHANGELOG
  entry — dev infrastructure with no user-visible effect (§4).

### Post-M9: tool tests run against in-memory storage (done)

- **The follow-up the CI diagnostic pointed at**, asked for as its own PR
  (branch `refactor/tool-tests-in-memory-storage`). The nextest probe had shown
  that the Windows runner's penalty is **disk**, not CPU: a 3.9x median per-test
  ratio against local, but **8–19x** for every one of the worst offenders — all
  of them notes/rag/search tests reaching storage through the tools testkit,
  which built a tempdir plus a **file-backed** SQLite. Almost none of them need
  a file.
- **The change is three lines of substance.** `Db::open_in_memory` and
  `CacheDb::open_in_memory` already existed; the facade was the only gap, so
  `Storage::open_in_memory(paths)` joins them and keeps `JsonStore` on the real
  `paths` — chats/profiles/config behave exactly as before, only `data.db` and
  `cache.db` stop being files. The two testkit constructors
  (`ctx_with_storage_lang`, `ctx_with_backends`) and one test that shares a
  single `Arc<Storage>` between two profiles now call it.
- **No call site changed.** Both helpers keep returning the `TempDir` — the JSON
  half still needs a root, and callers keep it alive — so all ~126 uses across
  16 files are untouched. Checked rather than assumed that nothing depended on
  the files: only two tests bind the directory handle at all, and both merely
  pass it through; no tool test references `data.db`/`cache.db` or reopens
  storage.
- **Measured, locally at the runner's four threads**: full suite **40.68s →
  24.09s** (−41%), and at full parallelism **37.4s → 16.41s** (−56%). Per-test,
  the sum over `features::tools::*` goes **103.3s → 16.4s (−84%)**, and the
  individual tests CI measured worst collapse from ~1s to **0.03s** —
  `consolidation_overview_uses_the_calibrated_threshold` (18.75s on CI) 1.34s →
  0.03s, `note_cite_source_links_and_recall_shows_it` (14.52s on CI) 0.90s →
  0.03s. Those are exactly the tests the runner's disk was punishing 8–19x, so
  the CI effect should be larger than the local one.
- **Measured on CI, and it closed the diagnosis** (run 31045675223): the Windows
  job went **9m11s → 5m59s** and its test phase **386.1s → 189.9s (−51%)**,
  against the original cold-cache **19m00s / 549.1s**. Two details make this
  more than a speedup. The CI gain (−51%) **exceeded the local one** (−41%),
  which is what the disk hypothesis predicted, since the runner charges more for
  exactly the I/O that was removed. And in the same run **Linux barely moved** —
  38.7s against 41.0s before — because a Linux runner was never paying that
  penalty. A change that helps one OS 2x and the other not at all is the
  signature of the cause being the disk, not the code, so this run is both the
  fix and the confirmation of the earlier diagnosis.
- **Where CI now stands overall**: from ~19–20 minutes at the start of this work
  to **~6**, via three independent fixes — the warm Windows cache (compile 7m31s
  → ~1m45s), the fsync fix in the seeding fixtures, and this one. The 927s tail
  observed earlier should also shrink, since it was the disk-bound tests that
  were most exposed to a slow instance, but that is a claim about variance and
  needs more than one run to assert.
- **Two guards, because the win is invisible to every other test.** Switching
  the testkit back to `Storage::open` would hand all the fsyncs back and nothing
  would fail. So `tool_context_storage_touches_no_disk` (`features::tools`)
  writes a real note through the testkit's storage and asserts neither database
  file appears — **mutation-tested**: reverting the testkit fails it with the
  message naming the cause — and `in_memory_storage_works_but_writes_no_database_files`
  (`shared::storage`) pins the facade itself, both that the SQLite halves are
  real (schema applied, sqlite-vec registered, note and vector round-trip) and
  that no file is created. The first test's doc comment originally claimed the
  second one's coverage; corrected, since a test cannot see which constructor
  its caller picked.
- **Deliberately unchanged**: backup, migration and compaction tests keep
  `Storage::open` — they assert on the files themselves, and an in-memory
  database dies with its connection. The orchestrator fixtures also keep it:
  they genuinely exercise persistence (`flush_saves`, restart scenarios, the
  search-cache reconciliation that reads chat files), and two of the CI-worst
  tests are theirs, so that is a separate question rather than an oversight.
- **1835 unit tests green** (+2 guards), 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No live run required**
  (AGENTS.md §3): a test-only storage path — `Storage::open_in_memory` is
  `#[cfg(test)]` and does not exist in a non-test build, and no engine, memory,
  tool or provider behaviour changes. No CHANGELOG entry — internal tests
  (§4).

### Post-M9: orchestrator fixtures — two convertible, the rest not (done)

- **Asked as a follow-up to the tool-test change**: with `features::tools::*`
  moved off disk, `app::orchestrator::*` became the dominant remaining cost —
  **384.5s of the 876.1s** CI per-test sum in the nextest probe, against tools'
  419.1s. The question was whether the same trick applies.
- **Mostly it does not, and finding out *why* is the useful part.** The
  orchestrator fixtures split three ways:
  - `spawn_orch_at` — **two-phase** tests that restart the app on one data root.
    An in-memory database dies with its connection, so phase two would start
    from an empty `cache.db` and exercise the index *rebuild* path instead of
    the *restore* one: the test keeps passing while covering something else.
  - `spawn_orch_cfg`/`spawn_orch` — single-phase, so they *look* convertible.
    They are not: roughly thirty tests built on them reopen storage afterwards
    (`let reopened = Storage::open(Paths::with_root(&root))`) to assert what
    actually reached disk. That idiom is how this suite checks persistence at
    all.
  - `bare_orch_rx` and `orchestrator::rag::test_deps` — no test on either
    reopens storage, verified by scanning every test file rather than assumed.
    **Converted.**
- **The measurement that settled it, and the near-miss worth recording.**
  Converting `spawn_orch_cfg` failed **6 tests** outright — and, far more
  interesting, **2 more kept passing for the wrong reason**:
  `removing_an_attachment_drops_its_index` and `an_inline_file_is_not_indexed`
  assert an *absence*, and against an always-empty reopened store they pass
  whether or not the code works. A green suite would have hidden that, which is
  exactly why the conversion was reverted rather than patched test-by-test: the
  hazard is not the six that shout, it is the two that do not. The reasoning
  sits in `spawn_orch_cfg`'s doc comment so the next person does not rediscover
  it by breaking something.
- **Measured** (locally, four threads, per-test sums): `app::orchestrator::*`
  **78.9s → 54.0s (−32%)**, whole-suite sum **140.2s → 113.4s (−19%)** on top of
  the tool-test change. The two heaviest converted tests —
  `reflection::reflect_failures_alert_once_then_reset` (7.80s on CI) and
  `rag::index_source_reports_chunk_progress_in_subbatches` (9.07s on CI) — both
  drop to **0.01s**. Cumulatively with the previous entry the per-test sum goes
  231.3s → 113.4s (−51%).
- **Confirmed on CI, and it closes the whole CI track.** The Windows test phase
  measured **258.2s** on the tool-test PR's run and **129.2s** on this one — the
  two safe fixtures halved it again, because they were exactly where the
  runner's disk charged most. End to end the Windows job went **19m00s →
  4m56s**, and its test phase **549.1s → 129.2s** (4.3x). Post-merge `main` is
  green in under five minutes, with its Windows job at 2m42s since it only
  builds.
- **A cache detail worth knowing** (it looks like a bug and is not): the Windows
  cache on `main` keeps its original timestamp and is *not* re-saved after these
  merges. Its key is a fingerprint of the **dependencies**, which source-only
  changes do not touch, and GitHub cache keys are immutable — so rust-cache
  correctly skips saving and the existing entry keeps serving. It only rotates
  when `Cargo.lock` or the toolchain moves.
- **Where the CI work ended up overall**: ~19–20 minutes → ~5, from four
  independent fixes — the warm Windows cache (compile 7m31s → ~1m45s), the
  ~800-fsync seeding fixtures, in-memory storage for the tool tests, and the two
  safe orchestrator fixtures — plus the removed useless cache save. The runner's
  variance (359 / 469 / 376 / 386 / 927s on identical code, back when the base
  was 4x higher) has not gone away; the base is simply much lower now, and
  whether the tail shrank proportionally needs several more runs to claim.
- **1835 unit tests green**, 76 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3):
  test fixtures only; `Storage::open_in_memory` is `#[cfg(test)]`. No CHANGELOG
  entry — internal tests (§4).

### Post-M9: a second chat model on the live gate — Qwen 3.6 27B (stages 1–2, done)
- **Trigger**: the live gate ran on exactly one chat model, `gemma-4-31B` q4_0,
  so everything it asserted about a model was asserted about *that* model. Plan
  and forks: [docs/history/e2e-second-chat-model.md](../history/e2e-second-chat-model.md).
- **The product needed no change.** First local run against
  `Qwen3.6-27B-Q4_K_M` + `mmproj-Qwen3.6-27B-Q8_0` (`ggml-org/Qwen3.6-27B-GGUF`,
  `-c 16384 --jinja`) and `bge-m3-Q8_0`: **95 passed, 2 failed** of 97, and all
  44 orchestrator smokes were green from the first attempt — memory, self-model,
  notes, attachments, compaction, MCP, images, retry. The app runs on Qwen 3.6 as
  it stands. Both failures were in the low-level client smoke set, and both were
  one thing: a token ceiling sized for how much Gemma thinks.
- **What the new model actually exposed** — three Gemma-shaped assumptions:
  `simple_generation` spent all 64 tokens on `reasoning_content` (thinking on
  "Reply with exactly: pong" costs 357–949 characters, so the ceiling went to
  1024 with thinking left on); `control_tools_are_callable` does not converge on
  *any* ceiling (1024: 0/3 runs called the tool, 2048: 2/3, 4096: 3/4, one run
  burning the whole 4096 on thinking over 129 s), because its prompt is
  open-ended; and `tool_call_is_emitted_and_parsed` was **already flaky before
  any edit** — the model emits the correct call and then repeats it, up to 15
  times, until `max_tokens` cuts it off and the finish reason arrives as `Length`.
- **Two options died on measurement, which is why they are written down.** A
  bigger ceiling does not fix an open-ended prompt for a reasoning model — it
  buys a coin flip whose red side costs 2+ minutes of billed GPU. And temperature
  is *not* the lever for the repetition: matching the orchestrator's 0.1 made it
  **worse** (5/20 failures vs 2/11 at the server default), against 0/20 with
  thinking muted. The repetition rides on the thinking loop.
- **Muting costs no coverage**, which is the only reason it is acceptable: tool
  calls *with* thinking on are exercised by the orchestrator smokes, on the real
  app path with tool results fed back and `thinking` at the server's default —
  the stronger test, green on both families. Each decision, with its numbers and
  its rejected alternative, lives in the smoke's own doc comment.
- **The gate was blind, and would have gone red for an unrelated reason.** Three
  smokes require a projector and **fail loudly rather than skip** against a
  text-only server, deliberately (lessons §9) — while `chat_payload` omitted
  `mmprojModelPath`, just as deliberately, in a decision written *before* the
  images track existed. Nothing had reconciled the two. `mmprojModelPath` is now
  sent; both repositories ship the file (`gemma-4-31B-it-mmproj.gguf` 1.20 GB,
  `mmproj-Qwen3.6-27B-Q8_0.gguf` 0.63 GB) and both fit an L40S at `ctxSize 16384`.
- **A model is one decision, not three flags.** `CHAT_MODELS` holds each family
  as a `(tag, repo, gguf, mmproj)` record and `--chat-model` picks one, so nobody
  can compose a repository/file pair that does not exist and discover it twenty
  minutes into a deploy; `--gguf`/`--mmproj` remain per-file overrides. The tag
  goes into the endpoint name (`e2e-chat-qwen-<run-id>`) so a listing, the
  sweeper's log and an orphan hunt all say which model is being held.
- **One dispatch, one model** (workflow input `model`, default `gemma-4-31b`).
  Both families in one job would run the suite twice — ~50 min against a
  45-minute `timeout-minutes` that cannot rise without crowding the sweeper's
  90-minute threshold, which is the gap that stops the sweeper deleting a live
  run's endpoints. Gemma stays the default: the memory thresholds are calibrated
  against that stack, and Qwen's repetition is currently *muted rather than
  understood*.
- **After the fix: 97 passed, 0 failed**, 1303 s (from 1460 s — the muted
  thinking gave back ~2.5 minutes). **2282 unit tests green**, 97 `#[ignore]`;
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`/`doc_index_check` clean.
  **Live run — GO** on **both** stands: `Qwen3.6-27B-Q4_K_M` + `bge-m3-Q8_0`
  (full suite 97/97) and `gemma-4-31B_q4_0-it` with its projector (the client
  smoke set, 5 runs, 8/8 each — the edits cost Gemma nothing, which is what
  "model-agnostic" has to mean). The three vision smokes were checked rather than
  assumed on both models, and the control arm earned its keep on Gemma: asked
  about a fixture it could not see, the blind model answered "dark purple" with
  confidence while the seeing one said green. No CHANGELOG entry — dev
  infrastructure, no user-visible effect (AGENTS.md §4).
- **One CI dispatch per model, both endpoints sets deleted and verified**: Gemma
  ([31907378154](https://github.com/vshylov/mindfork-rs/actions/runs/31907378154))
  93 passed / 1 failed, ready 473 s + suite 1362 s; Qwen
  ([31909712156](https://github.com/vshylov/mindfork-rs/actions/runs/31909712156))
  **94 passed / 0 failed**, ready 303 s + suite 1696 s. The three vision smokes
  ran and were green on both — before this change they could not have passed at
  all, the endpoint having no projector and those smokes failing rather than
  skipping. Both runs fit `timeout-minutes: 45`, but at 31–33 minutes the
  headroom is thinner than the July figures implied; worth knowing before
  anything else joins the suite.
- **The one red was not ours, and the diagnostics said so immediately.**
  `rewrite_tool_e2e_live` (untouched by this branch) reported
  `saw_rewrite=false, deleted=0` with the assistant text
  `"2+2=5\n<call:rewrite_current_message/>\n2+2=4"` — the model *wrote the call
  as prose*, in a syntax no protocol here defines, so no call happened and no
  archive followed. Tool calling was fine elsewhere in the same run, which also
  clears the rolling `server-cuda` image of drift. Measured afterwards on the
  local Gemma stand, the test's historical home: **1 failure in 8**, identical in
  shape; green on Qwen. Left unpatched deliberately — the two smokes this track
  did change were changed because measurement said what to change them *to*, and
  no such measurement exists here. The structural half is worth carrying forward
  though: that test was written as a **manual** probe ("the model is unstable —
  run manually"), while the gate sweeps up every `#[ignore]` without distinction,
  and a $1-per-run gate cannot carry compliance probes that flake one run in
  eight.

### Post-M9: the rewrite probe's flake — two models, two causes (done)
- **The item the previous entry left open**, and it closed in a shape neither the
  roadmap item nor the plan expected. Both assumed a *split*: a deterministic
  mechanism test for the gate, the model-compliance probe kept manual, plus
  whatever machinery excluding it would need. Neither half was necessary.
- **The mechanism already had strictly stronger coverage.**
  `rewrite_tool_discards_partial_and_saves_it` drives the whole path against a
  `MockBackend` and asserts the exact history, the discarded text *and* its tool
  call, where the live probe asserted only "a rewrite happened and the archive is
  non-empty". So the live probe was never the mechanism's guard — it answers the
  one question a mock cannot: does a real model reach for the tool.
- **The flake was in the test's own wording.** It asked the model to "demonstrate
  `rewrite_current_message` strictly by steps, skipping none: step 1 write X,
  step 2 (MANDATORY) call the tool, step 3 write Y" — an invitation to narrate
  the sequence, which is exactly what it got back:
  `"2+2=5
<call:rewrite_current_message/>
2+2=4"`, the call written as prose.
  Reworded as a *situation the tool answers* ("your draft is no good — call the
  tool and write it again"): **1 failure in 8 → 0 in 30** on Gemma.
- **And the second family flaked for a different reason, which is the part worth
  carrying forward.** With the prompt fixed, Qwen 3.6 still failed 1 in 20 — not
  by narrating but by returning **nothing**: no tool call, empty text, the whole
  turn spent in `reasoning_content`, the same shape that once broke
  `simple_generation`. Muting thinking for that turn: **0 in 20**. Had the rate
  alone been watched instead of the failure *mode*, "much better on Gemma" would
  have shipped a probe still red one dispatch in twenty.
- **One promising fix was measured and rejected.** Narrowing the profile to the
  single tool under test looks like the "remove the alternative" rule this repo
  already records — and measured **7 failures in 20** against 1 with every tool
  enabled, all of them the empty turn. That rule is about a model satisfying the
  request through a *different* tool; a one-tool list does nothing about a model
  spending the turn thinking, and appears to invite it. The lesson gained the
  caveat.
- **Two smaller things fell out.** The probe now prints the tool names its
  messages carry, as its sibling always did — without them "never called it" and
  "called it and the effect did not land" are indistinguishable, which is half
  the diagnosis. And `control_tools_are_callable` had been handing the model
  *both* control schemas while only ever asserting `send_followup_message`, so
  `rewrite_current_message`'s description was shown and never checked; it is now
  table-driven over both, with both schemas offered each time, so the model also
  has to pick the right one of two.
- **Final measurements, both families**: `rewrite_tool_e2e_live` **0 in 20** on
  `gemma-4-31B_q4_0-it` and **0 in 20** on `Qwen3.6-27B-Q4_K_M`;
  `control_tools_are_callable` 0 in 10 and 0 in 15; the untouched sibling
  `followup_tool_e2e_live` 0 in 12, confirming it was not disturbed. **2282 unit
  tests green**, 97 `#[ignore]`; fmt/clippy/`cyrillic_scan`/`link_check`/
  `doc_index_check` clean. No CHANGELOG entry — test-side only, no user-visible
  effect (AGENTS.md §4).

### Post-M9: a ceiling on every job, and two orphaned workflows (done)
- **Trigger**: on 2026-08-19 two CI runs (32270019208, a pull request;
  32274156069, the push to `main` forty minutes later) each had Linux jobs that
  never finished — one in the first, **three** in the second (`Lints`, `Tests
  (ubuntu-latest)`, `SonarQube Cloud`) while the Windows entry of the same run
  passed in three minutes. They were noticed and cancelled by hand after ~2.5
  hours, and the cancel itself took about a minute to land. The incident was
  read as "a runner froze"; the logs say otherwise.
- **Diagnosis, from the four job logs**: every one stopped on the *same step*,
  `System dependencies (ALSA)` — `sudo apt-get update && sudo apt-get install
  -y libasound2-dev`, a step that normally takes ~10 s. The trace is identical
  in all four: the image's apt mirror `azure.archive.ubuntu.com` answered
  nothing (`Ign:` with apt's 1-2-4 s backoff), apt fell back to
  `archive.ubuntu.com`, fetched the four `InRelease` files there — and then
  printed nothing more for 2.5 hours, until `##[error]The operation was
  canceled`. A stalled package-index download behind an unhealthy mirror, which
  the runner-images tracker records as a recurring class (actions/runner-images
  #6894, #6913, #7048, #12949). Nothing on the runner was frozen: the
  cancellation propagated, the post-steps ran, the other jobs were fine.
- **Why it cost 2.5 hours rather than 10 minutes**: not one job in `ci.yml`,
  `packaging.yml`, `release.yml`, `site.yml` or `audit.yml` had a
  `timeout-minutes`, so the only ceiling was GitHub's default — **360 minutes**
  ("each job in a workflow can run for up to 6 hours"). What was actually burned:
  155 + 3 × 113 ≈ **494 Linux minutes** (≈ $3.95 at the overage rate). What the
  default would have allowed: 4 × 360 = **1440 minutes — 24 hours, 72 % of the
  Free plan's monthly 2000** — for a step whose outcome was already known at
  minute two. Only the two live-gate workflows had ceilings, because they hold
  real money (45 min, and the sweeper's 10). The lesson is the asymmetry: a
  ceiling that fires on a legitimately slow run costs one rerun; a missing one
  costs six hours per job, billed.
- **The minute-long cancel is GitHub's, not ours.** A cancel reaches the runner
  over its long-polling message channel (up to ~50 s), and the step's process
  then gets SIGINT, a 7.5 s grace, SIGTERM, a further 2.5 s, and a kill. A
  `timeout-minutes` cancel travels the same path — the difference is that it
  happens without anyone watching.
- **Fix, two layers, all six workflows** (no Rust):
  1. **`timeout-minutes` on every job** — seventeen jobs. Each value was sized
     from the job's *measured* history, every run the API still lists
     (2026-07-17 onward, 1454 job rows), plus room for a cold cache, and the
     figure is written next to the value so the next reader can re-check it:
     `changes` 5 (max 0.1 over 296 runs); `lint` 15 (p50 1.1, max 5.2 / 291);
     `test` **15 on Linux, 35 on Windows** — an expression on `matrix.os` — from
     Linux p50 2.3 / max 4.7 (269) and Windows p50 6.1 / max **27.6** (231): the
     worst Windows figures pre-date the 2026-08-05 speedup (max 16 since), but a
     cold cache on top of the 2.4x disk-contention tail the earlier entry
     records still reaches the low twenties, so Windows keeps the wider ceiling
     rather than the current-regime one; `sonar` 30 (p50 4.0, max 8.1 / 199,
     plus up to five minutes of `sonar.qualitygate.wait`); packaging `build` 30
     (max 10.0 / 26), its smokes 10 (max 0.8), the `.iss` compile 15; release
     `build` 45 (Linux 7.8–8.8, Windows 14.9–17.9 — LTO builds on a tag ref
     with no warm cache to count on), packages 10, installer 15, publish 10;
     site 10 + 10 (max 0.3); audit 15 (0.9).
  2. **The apt step bounded on both sides** — a step-level `timeout-minutes: 5`
     on every `apt-get` step (seven of them, including the nfpm installs and
     the live gate's own copy, which runs before any endpoint exists), and
     `-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30`
     on every `apt-get update`. The step timeout is the bound; the options make
     a dead connection fail and retry *inside* that budget (4 × 30 s against
     apt's default 120 s per attempt), so the usual outcome is apt's own
     "Connection timed out" — an error that names its cause — rather than a
     kill. Honest caveat, recorded so it is not over-trusted: the options
     narrow a known class of stall, but the 19 August hang outlasted apt's own
     120 s timeout by two hours, so it is the GitHub-level timeout that is
     demonstrably the fix, and the options are the defence in depth.
- **Rejected shapes.** A local *composite action* for the apt line (one place
  instead of seven): step-level `timeout-minutes` on a composite step is a
  documented grey zone (actions/runner #646, #1979, #2415), and a bound that is
  silently inert is the lesson-10 pattern this project already has a name for
  — seven explicit copies of one line, with the reasoning on the first and
  pointers on the rest. Skipping `apt-get update` in favour of the image's
  stale index with a fallback: saves ~7 s of an already 2-minute job and only
  moves the exposure from the index to the `.deb`, for a two-path step. A
  scheduled "watchdog" workflow that cancels long runs: redundant once every
  job has a ceiling, and it would itself be an hourly billable job. Defender
  exclusions, `nextest`, a self-hosted runner: unrelated to this failure.
- **Orphaned workflows.** The Actions sidebar listed nine workflows against
  seven files: `crossterm base-layout verify` and `nextest timings (temporary)`
  were throwaway probes on their own branches (`ci/crossterm-base-layout-verify`,
  2026-07-24, the upstream-patch check recorded in
  [layout-independent-hotkeys.md](../research/layout-independent-hotkeys.md);
  `ci/nextest-timings`, 2026-08-05, the timing probe of the "cutting the Windows
  CI job" entry). The branches were long gone — GitHub keeps a workflow in the
  list for as long as one of its runs exists, and each had exactly one. Both
  runs were deleted (`DELETE /repos/…/actions/runs/{id}`), after the nextest
  run's `nextest-timings-windows` artifact — the 1833 per-test timings, 438.2 s
  wall — and both job logs were downloaded, since the journal cites that run by
  id. The list is seven again, and every one of the seven is live: CI and Site
  on pull requests, Packaging path-filtered, Release on tags, Audit weekly, the
  live gate by dispatch, the sweeper on its schedule.
- **A finding beside the task — the sweeper's minutes — and the decision it
  got.** The hourly sweeper had become the repository's single largest consumer
  of Actions minutes. Each sweep runs 5–9 s and GitHub bills a job at a minimum
  of one minute: 438 runs in the first 21 days of August → **~630 billable
  minutes a month**, about 26 merges' worth of CI, as a backstop for a gate
  dispatched five times since it was built. Its bound is *time and endpoint
  quota*, not money (scale-to-zero already bounds that at one 15-minute idle
  window), so the cadence is a trade between how long an orphan may hold quota
  and what the backstop costs — and, being part of the remote gate's design
  (remote-e2e-hf.md §6), not a tweak to make unasked. It was raised as a finding;
  **user's decision (2026-08-21): every six hours** — `17 */6 * * *`, the
  off-the-hour minute kept. An orphan now holds quota for up to ~7.5 h (one
  interval plus the 90-minute age threshold) against ~2.5 h before, at roughly a
  sixth of the cost (~105 billable minutes a month); the 90-minute threshold
  itself is unchanged, since what it guards is the live job's 45-minute ceiling,
  not the cadence. `docs/install.md` and the plan's §6 say so.
- **Verification**: the workflows were validated by *rendering* (lessons §10) —
  a PyYAML pass over all seven files printing every job's ceiling and every
  bounded step's command, so the folded `>-` scalars and the matrix expression
  are seen as GitHub will see them; the pull request's own CI run is the live
  render (all five jobs green on the first run, the bounded apt step at 12 s
  with the new options accepted). The sweeper's new schedule can only be seen
  live once merged — a `schedule` fires on the default branch alone — so the
  first post-merge day's four runs are its check. No Rust changed: **2425 unit
  tests green, 106 `#[ignore]`**, unchanged; `cyrillic_scan`/`link_check`/
  `doc_index_check` clean. No CHANGELOG entry — dev infrastructure, no
  user-visible effect (AGENTS.md §4).

### Post-M9: a containerised test environment — JupyterLab, the app, a CPU stack (done)
- **Trigger**: three tracks in a row were opened by findings from a **JupyterLab
  terminal** — command-only control, `/export`, commands stage 3 — and every one
  of them was found by a manual pass on a borrowed host. `Ctrl+N`/`Ctrl+T` never
  arriving, OSC 52 being dropped, "the current directory" meaning the folder the
  file browser is rooted in: none of it reproduces in Windows Terminal, so
  verifying a fix meant finding a JupyterLab first. The second half of the
  trigger is the live gate: the `#[ignore]` suite (AGENTS.md §3) runs either
  against a hand-started `llama-server` or against rented HF endpoints at ~$1 and
  ~25 minutes a go, and there was nothing in between.
- **What and why**: `docker/` — a four-service compose stack. `models` is a
  one-shot downloader into a named volume; `chat` and `embed` are the official
  CPU `ghcr.io/ggml-org/llama.cpp:server` image serving Gemma 4 E2B-it Q8_0
  (+ the vision projector) and bge-m3 Q8_0; `lab` is
  `quay.io/jupyter/minimal-notebook` with the `mindfork` binary built from the
  working tree by a Rust stage in the same Dockerfile. `docker compose up
  --build` ends in a browser tab whose terminal is xterm.js — the host the app is
  hard to test on — and the two server ports are published, so
  `cargo test -- --ignored` also runs from the Windows host against it. Design,
  measurements and the rejected alternatives:
  [docker-jupyter-env.md](../research/docker-jupyter-env.md); operation:
  [docker/README.md](../../docker/README.md); user-facing summary: install.md §7.3.
- **The forks, and what was chosen** (user's decision, 2026-08-26): a compose
  stack over a single all-in-one container (rebuilding the app after a code
  change must not reload 4.6 GiB of weights); a named volume filled by a
  downloader over baking the models into the image, with `MODELS_DIR` switching
  the same mount to a host directory; JupyterLab with the Launcher, a terminal,
  the Python kernel and the RAM/CPU status bar, without the screenshot's Java
  kernel (a JDK buys nothing here); and the vision projector on by default, the
  host's Docker VM having 16 GiB.
- **The engine is configured by a seeded `settings.json`, not by
  `MINDFORK_ENGINE_URL`** — the one decision worth stating on its own.
  `apply_env_overrides` (src/main.rs) forces `mode: external` on every launch
  whenever that variable is set, which would make the cloud providers
  untestable in the very environment built for testing: switching to Claude in
  the settings screen would not survive a restart. The start-up hook instead
  writes a **partial** `AppConfig` document (legal only because the struct and
  every section it names carry `#[serde(default)]`), naming the two server URLs
  and the `*_API_KEY` variable each cloud key arrives in. A new unit test
  parses that seed file and asserts what it configures, so renaming a config
  field cannot silently turn the seed into a no-op.
- **Four traps, each paid for once and now written down.** (1) Runtime ALSA on
  Ubuntu 24.04 must be `libasound2t64` — the same time_t64 trap `packaging/`
  already documents. (2) `/etc/machine-id` ships **empty** in the base image, so
  without generating one `shared::secrets` refuses to store any API key at all;
  it is generated per build, which is why the container's durable route for a
  cloud key is the env variable. (3) `docs/` cannot be excluded wholesale from
  the build context — `shared/credits.rs` pulls `docs/legal/*` through
  `include_str!`, so excluding it turns a ten-minute build into a compile error
  at the very end. (4) A start-up hook named `*.sh` is **sourced** by
  docker-stacks' `run-hooks.sh`, leaking `set -u`/`pipefail` into `start.sh`;
  named without the suffix it is executed as a subprocess instead.
- **Measured, not assumed** (2026-08-26, against the live registries):
  `ghcr.io/ggml-org/llama.cpp` has no `latest` tag (`server`, `full`, `light`
  and per-build `server-b<NNNN>`); the chat GGUF is 4 967 497 184 B, the
  projector 985 653 760 B, the embedder 634 553 760 B; and
  **`jupyterlab-system-monitor` — the package that draws the RAM/CPU status bar
  in the screenshot this was modelled on — pins `jupyterlab ~=3.0` and cannot be
  installed on JupyterLab 4 at all**; `jupyter-resource-usage` is the equivalent
  that can.
- **The download needed two curl flags, both found by failing.** A 4.63 GiB
  transfer from the HF CDN died at 4.34 GiB with exit 92, *"stream error in the
  HTTP/2 framing layer"* — so the fetch runs `--http1.1`. And curl's plain
  `--retry` covers transient HTTP statuses and connection failures, **not** a
  mid-stream protocol error: without `--retry-all-errors` the one failure that
  actually happens is the one not retried. Bytes land in `<name>.part` and are
  renamed only after the size check passes, so an interrupted `up` resumes
  instead of leaving a truncated GGUF that `llama-server` accepts and then dies
  loading. The expected size is re-read from the server on every run rather than
  hardcoded, so changing the quant in `.env` needs no second edit.
- **What it deliberately does not do.** It does not replace the rented gate:
  several live smokes assert on model *behaviour* and were calibrated on
  31B-class models, and a 2B-effective one fails some of them for reasons that
  are not defects — `tools/e2e_hf.py` stays the gate of record, and a journal
  entry claiming "Smoke — GO" must keep naming the stack it ran on. It is also
  not a shipping artefact: the supported installation paths are unchanged.
- **Verification**: the `lab` image builds and `mindfork --version` reports
  0.9.7 inside it, with `libasound.so.2` resolved and a non-empty machine-id;
  the seeding hook writes the expected `settings.json`/`defaults.json`;
  `fetch.sh` was exercised on all four paths (fresh, already-complete, resume
  from a 4.15 GiB partial, and a wrong file name → `HTTP 404` and a non-zero
  exit that keeps the servers from starting). Live run — TODO. No Rust changed
  beyond one new test: **2600 unit tests green, 109 `#[ignore]`**;
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cyrillic_scan`/`link_check`/`doc_index_check` all clean. No CHANGELOG entry —
  developer infrastructure, no user-visible effect (AGENTS.md §4).
