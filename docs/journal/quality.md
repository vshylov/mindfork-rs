# Journal — Static analysis and the repository's gates

The checks that block a pull request and the cleanups they drove: SonarQube analysis, the quality gate and its backlog, and the gates guarding the repository's own structure — source language, relative links, documentation layout.

**Reference documents for this area:** architecture.md §12, AGENTS.md §4, §6

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (13)

- Post-M9: broken documentation links, and a gate that stops them recurring (done)
- Post-M9: SonarQube Cloud analysis in CI (done)
- Post-M9: the SonarQube quality gate became blocking (done)
- Post-M9: SonarQube backlog — triage + stage 1 (Python tools) (done)
- Post-M9: SonarQube backlog — stage 2 (UI screens and widgets) (done)
- Post-M9: SonarQube backlog — stage 3 (runtime and shared) (done)
- Post-M9: SonarQube backlog — stage 4 (orchestrator and tools) (done)
- Post-M9: the quality gate stopped judging new-code coverage (done)
- Post-M9: the documentation refactor — CLAUDE.md became a router (done)
- Post-M9: SonarQube follow-up — the doc gate's regexes and one test's complexity (done)
- Post-M9: SonarQube follow-up — the screenshots SVG writer (done)
- Post-M9: SonarQube follow-up — `chat_search`'s renderer (done)
- Post-M9: SonarQube follow-up — two new lint families, and eight complexity findings (done)

### Post-M9: broken documentation links, and a gate that stops them recurring (done)
- **28 relative links in the docs pointed at nothing**, and had for a while.
  Nobody noticed because a stale link is *valid Markdown*: `cargo`, `clippy` and
  `cyrillic_scan` all pass, and it only 404s when someone clicks it on GitHub.
  Branch `fix/markdown-links`.
- **The cause is procedural, not careless.** AGENTS.md §4 says a finished track's
  plan moves to `docs/history/` — the file gains a directory level, and every
  relative link *inside* it silently breaks (`../src/foo.rs` starts resolving to
  `docs/src/foo.rs`). Links **to** the plan are the obvious half and do get
  updated; links **inside** it are the half that gets missed.
  `refactoring-solid.md` alone carried 23, because it cites source files
  heavily. One more was a different cause: CLAUDE.md still pointed at
  `src/shared/api/client.rs`, which moved to `openai/client.rs` in the ADR 0004
  Phase 2 split.
- **`tools/link_check.py`** (stdlib, the `cyrillic_scan.py` pattern: tracked
  files, `--list`, non-zero exit) now runs in CI's `lint` job — before the
  toolchain setup, since it needs neither Rust nor ALSA. It is **deliberately
  narrow**, so a green run means something: relative links only (checking the
  network would be slow and flaky), the file part only (heading slugs are a
  rendering detail, and chasing them would fail on every heading edit), and
  **code is skipped**.
- **Skipping code is what removes the need for an exception list.** A first,
  naive scan reported 32 hits, three of which were `[t](u)`, `√[n](x)` and
  `[text](url)` — all inside backticks, i.e. *examples* of links rather than
  links. Blanking fenced blocks and inline spans drops them by construction; the
  real count was **28**, not the 29 I had estimated by eye. Worth the note: the
  tool corrected its author before it corrected the docs.
- **Mutation-tested in both directions**, because a link checker that passes is
  indistinguishable from one that does nothing: re-breaking a single link the way
  a history move does turns it red; a file containing *only* code-span links
  stays green; adding one real link to that same file turns it red again.
- **The gate is placed where the bug is.** The `lint` job runs on every PR
  including a docs-only one (which skips the `test` job), which is exactly when
  links break. AGENTS.md §1 and the §4 table now name the trap and the tool.
- **No live run needed** (AGENTS.md §3): documentation, one CI step and a
  standalone script — no engine, memory or tool path touched. **1486 unit tests
  green**, 69 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`
  clean. No CHANGELOG entry — internal docs and tooling (§4).

### Post-M9: SonarQube Cloud analysis in CI (done)

- **Asked for directly**: the project side on sonarcloud.io was already set up
  (`SONAR_TOKEN` present in the repository secrets), only CI was missing. Branch
  `ci/sonarcloud` (the `ci/` prefix has precedent —
  `ci/reduce-actions-minutes`). A simple task by AGENTS.md §1 — CI configuration,
  no cross-layer contract and no code — so no design doc; the three genuine
  forks were put to the user instead (**decided 2026-08-05**).
- **Rust is a first-party Sonar analyzer now**, not the old community plugin, and
  that decided the shape: it **runs `cargo clippy` itself** (hence the toolchain
  and ALSA in the job) and imports coverage as LCOV or Cobertura
  (`sonar.rust.lcov.reportPaths`). So the job is deliberately *not* wired to our
  own `cargo clippy --all-targets -- -D warnings` gate: the analyzer's own run
  carries the 85 Clippy rules mapped to first-party Sonar rules, and mixing the
  two report paths is documented to **duplicate** issues.
- **Forks, all as recommended.** (1) **Coverage on** — `cargo llvm-cov` produces
  `lcov.info` in the same job; it is the metric every quality-gate condition is
  built around, and without it "Coverage on New Code" is simply `n/a`. (2) The
  job is **advisory** — it does not wait for the verdict
  (`sonar.qualitygate.wait` is left off), the same posture as `audit.yml`: a new
  analyzer on a 1833-test codebase will surface a batch of findings at once, and
  a gate that reddens every PR on day one stops being read. (3) Project key
  `vshylov_mindfork-rs` / organization `vshylov` — the standard GitHub-import
  scheme.
- **The minutes policy is respected rather than worked around**: the job takes
  `needs: changes`, so the docs-only PRs this repo produces constantly skip it
  (there is no new code to analyze), and it carries the same two guards as
  `test` — `!= 'true'` for the *value* and `!cancelled()` for the *job*, since a
  failed `needs` skips a dependent job whatever `if` says. It runs on `push:
  main` as well, deliberately: main is the baseline the "new code" comparison is
  made against.
- **Two builds in one job, and that is not an oversight**: the instrumented
  coverage build and the analyzer's plain clippy build have different
  fingerprints, so no arrangement makes them share one. Feeding the analyzer an
  external clippy report instead would still cost a second build (different
  RUSTFLAGS again) *and* add the duplicate-issue risk above — so the simpler
  option wins. `cargo llvm-cov` keeps its own `target/llvm-cov-target`, so it
  cannot poison the normal target dir (the journal already records what a
  poisoned `target/` looks like: seven tests "failing" on paths baked in from
  another tree).
- **`llvm-tools-preview` is added in the job, not in `rust-toolchain.toml`** — a
  developer checkout should not download the LLVM tools for a component only CI
  uses.
- **Two things the user has to keep true on the Sonar side** (recorded because
  neither is visible from this repo): **Automatic Analysis must stay off** for
  the project — CI-based analysis refuses to run while it is enabled — and the
  repository is **private**, which on SonarQube Cloud is a paid plan rather than
  the free public tier.
- **The coverage step was run locally rather than left to CI to discover** — it
  is the step most likely to fail, and a failure there costs runner minutes to
  learn: `cargo llvm-cov --workspace --lcov` ran the suite under instrumentation
  (**1833 passed, 76 ignored**, 41 s) and produced a 2 MB `lcov.info` over 166
  files, **87.0% region / 86.3% line** coverage. One detail confirmed rather than
  assumed: the report's `SF:` paths are **absolute**, which resolves correctly
  because they sit inside the scanner's base directory — the scan runs in the
  same workspace. Instrumented artifacts go to `target/llvm-cov-target`, so the
  ordinary `target/` is untouched.
- **Verification of the rest**: the workflow YAML parses and the `sonar` job's
  steps and guards are as intended; `cyrillic_scan`/`link_check` clean; **1833
  unit tests** unchanged (no Rust code touched). **No live model run is
  required** (AGENTS.md §3) — no engine, memory, tool or provider path exists in
  this change. What only the first CI run can settle is the analyzer's own clippy
  invocation and the upload. No CHANGELOG entry — dev infrastructure with no
  user-visible effect (§4).
- **Groundwork**: turning the gate blocking once the first analysis is triaged;
  a quality-gate badge in the README (a private project's badge does not render
  for anonymous readers without a token); and folding the coverage run into the
  Linux `test` job if the first measurements say the duplicated test run is the
  expensive half.

**What the first live runs found** (2026-08-05, recorded because two of the
three findings are invisible from the workflow's own status):

- **The job was green while the analysis had failed.** On the PR everything
  passed — Quality Gate, 0 new issues — but the push-to-`main` analysis was
  **rejected server-side**, and `ci.yml` reported success anyway. Not a bug:
  `ANALYSIS SUCCESSFUL` in the scanner log means *the report was uploaded*, and
  processing is asynchronous (the next log line says so). That is the exact blind
  spot of the advisory posture. What surfaces it is the Sonar app's **own** check
  on the commit, `SonarCloud Code Analysis` — it read "❌ The last analysis has
  failed" while ours read green. So the pair is: our job answers "did the scan
  run", that check answers "was the report accepted". Enabling
  `sonar.qualitygate.wait` would collapse the two — the scanner then waits for the
  CE task and fails on it — which is a second argument for the groundwork item
  above, beyond gate enforcement.
- **The cause was the organization's LOC quota, and two wrong hypotheses were
  discarded on evidence before it.** `Administration → Background Tasks → Show
  error details` gave it verbatim: the free plan allows **50 000 lines per
  organization**, 14 143 were already used, and this analysis brought **81 411**
  (`rust=80 174, py=1 237`). The first guess — that the project's Main Branch was
  named something other than `main` — was refuted by the Branches page (`main` is
  the main branch, simply never analyzed). The second — that exclusions could fit
  the project into the remaining 35 857 — was refuted by **measuring**: `src` is
  80 059 ncloc, of which `**/tests.rs` is 6 692 and inline `#[cfg(test)]` modules
  are **24 729**, and the latter cannot be excluded at all because `sonar.exclusions`
  works per *file*, not per region. Even production-only (48 638) does not fit.
  So it was a plan decision, not a configuration one: **Team, $34/month for up to
  100k LOC** (user's decision) — 81 411 used, ~18 600 of headroom.
- **Exclusions deliberately not added** despite fitting the option as offered:
  trimming `**/tests.rs` + `tools/` buys 7 900 lines while 24 729 lines of inline
  test modules keep counting, i.e. it makes the analysis *inconsistent* (some test
  code counted, some not) for less than half the headroom already available. The
  lever stays documented for whenever 100k is approached.
- **Re-running the analysis needed no commit**: `gh run rerun <id> --job <sonar>`
  re-ran only that job against the same commit, so lint and the tests were not
  paid for twice. Afterwards the check became **"Quality Gate not computed"**
  (`neutral`) — the normal state of a *first* main-branch analysis, since there is
  no new-code baseline to compare against yet.
- **The open question is closed: coverage really is imported.** `main` reports
  **85.2%**, against 86.3% line coverage measured locally — the gap is the ~1 240
  Python lines in `tools/`, which have no coverage report and therefore land in
  the denominator as uncovered. That distinguishes a working import from the
  silent failure mode it could not otherwise be told apart from ("0.0% Coverage on
  New Code" on a PR that touches no Rust looks identical either way).
- **Timing**: 7 min 51 s cold, **4 min 06 s** warm (analysis itself ~1 min; the
  analyzer's own Clippy run is the bulk of the rest).

### Post-M9: the SonarQube quality gate became blocking (done)

- **Closes the groundwork item opened when the analysis was set up**: the job was
  advisory on purpose (a new analyzer on a 1833-test codebase can surface a batch
  of findings at once, and a gate that reddens every PR on day one stops being
  read), to be flipped once the first analyses were triaged. They came back clean
  — PR #258 `Quality Gate passed` with 0 new issues, `main` likewise once it had
  a baseline — so the reason to wait was gone. Branch `ci/sonar-blocking-gate`.
- **`sonar.qualitygate.wait=true`** in `sonar-project.properties` (not an
  argument in the workflow: analysis parameters live in one place). The scanner
  now polls the compute-engine task and fails the job on a failed gate.
- **The bigger half is not enforcement, it is honesty about the upload.** The
  first live run showed the failure mode this closes: `ANALYSIS SUCCESSFUL` in
  the scanner log means *the report was uploaded*, processing is asynchronous,
  and the report was then **rejected** server-side (over the organization's LOC
  quota) while `ci.yml` still reported success. Waiting makes the job answer "was
  this analysis accepted and did it pass" instead of "did the scan run" — the
  distinction that previously only the Sonar app's own commit check could make.
- **What can and cannot redden a PR**: the gate judges **new** code, so the
  existing backlog never blocks; what blocks is new uncovered code (Coverage on
  New Code, which the `cargo llvm-cov` step feeds), new issues, or an analysis the
  server refuses. Worth knowing the corollary: the project's **New Code
  definition** now has teeth, since it decides what "new" means.
- **Cost**: the job waits for processing, which measured ~2–5 s on this project
  (`sonar.qualitygate.timeout` defaults to 300 s and is deliberately left alone).
- **Verification**: the YAML and properties parse; **1833 unit tests** unchanged
  (no Rust code touched); no live run required (AGENTS.md §3) — no engine, memory,
  tool or provider path exists here. The change is self-verifying in a way the
  previous ones were not: if the gate did not actually block, this PR's own
  `sonar` job would not be reporting a verdict at all. No CHANGELOG entry — dev
  infrastructure with no user-visible effect (§4).

### Post-M9: SonarQube backlog — triage + stage 1 (Python tools) (done)

- **The 114-issue initial-analysis backlog was triaged** (user decisions
  2026-08-06): the ~49 `rust:S2208` wildcard-import issues are the **documented
  `use super::*` convention** from the god-object split playbook
  (docs/history/refactoring-god-objects.md, architecture.md §3) — **Accepted in
  Sonar**, not "fixed"; the ~61 `S3776` cognitive-complexity issues are
  **triaged**: refactor the genuinely tangled functions, Accept the deliberate
  table/parser shapes. Staged as one bookkeeping pass + four fix PRs
  (Python tools → UI screens/widgets → runtime/shared → orchestrator/tools).
- **Stage 0 — Sonar bookkeeping, no code** (57 status changes via the MCP):
  49 × S2208 Accepted; `python:S5332` (link_check.py:49) **False Positive** —
  the flagged `http://` is the `SKIP_SCHEME` tuple used to *skip* external
  links, not a request; 7 deliberate-shape S3776 Accepted with rationale —
  `settings/spec.rs::field_spec` (147 — the single-source-of-truth access
  table, SOLID stage 3.2; splitting it recreates the four scattered matches it
  replaced), `markdown/latex.rs` ×4 (63/37/25/25 — converter tables),
  `calc.rs::tokenize` (31 — the recursive-descent evaluator),
  `wrap.rs::wrap_ranges` (29 — the hot-path width algorithm). **57 open
  issues remain** (48 Rust S3776 + 9 Python), which is what stages 1–4 burn
  down.
- **Stage 1 — `fix/sonar-python-tools`** (this entry's branch). The one real
  defect: `link_check.py`'s `CODE_SPAN` regex `` (`+)[^`]*?\1 `` is
  super-linear (S8786 — the greedy opener + backreference retries every
  opener length at every position). Replaced with a **linear manual scanner**
  (`blank_code_spans` + `_close_run`: an opening run of k backticks closes at
  the next run of ≥ k, an unclosed run stays literal). **Parity measured, not
  assumed**: over the whole 40k-line markdown corpus the 328 line-level
  diffs are all bare ``` fence lines (handled by the fence branch *before*
  spans, so unreachable) or inline mentions of triple-backtick fences, where
  the new behaviour is closer to CommonMark — and the tool's verdict on the
  repo is unchanged (clean).
- **Eight Python complexity refactors, all pure extractions** (dev scripts;
  behaviour pinned by offline probes rather than eyeballing):
  `cyrillic_scan.py::print_fmt_spans` (36) → per-state steps
  (`_find_macro`/`_open_quote`/`_close_literal` — a step returning `None` for
  the position ends the line, the returned state carries over);
  `cyrillic_scan.py::main` (40) → `scan_file`/`_line_flagged`/`report`;
  `e2e_hf.py::cmd_run` (41) → `Plan`/`print_plan`/`dry_run`/
  `create_endpoints`/`bring_up_all`; `cmd_sweep` (16) → `sweep_verdict` (the
  fail-safe "unknown createdAt → keep" reasoning moved into its docstring);
  `hf_api.py::cleanup` (20) → `_keep_endpoints`/`_verify_deletions` (the
  "a name leaves CREATED only once proven gone" invariant untouched — the
  verifier mutates, never rebinds); `hf_probe.py::cmd_doctor` (35) →
  `_granted_permissions`/`_report_fine_grained`/`_print_403_advice`;
  `probe_chat` (20) → `_check_tool_calling`/`_check_streaming`; `cmd_run`
  (30) → `_run_embed_only`/`_create_and_probe_embed`.
- **Verification** (the HF scripts have no offline entry beyond argparse, so
  the extracted pure parts were probed directly): `py_compile` on all five
  files; every `--help` path; `scan_file` against 11 cases including the
  cross-line print-macro tracker, the value-argument exemption and the
  `cyrillic-ok` markers; `blank_code_spans` against 7 cases plus the
  pathological 5000-backtick input the old regex choked on; `sweep_verdict`
  against 6 cases including both fail-safe unknowns and the
  fractional-second timestamp; full repo runs of `link_check` and
  `cyrillic_scan` — both still clean. **1835 unit tests green** (no Rust
  touched), clippy `-D warnings`/fmt clean. **No live run required**
  (AGENTS.md §3): dev tooling — no engine, memory or tool path. No CHANGELOG
  entry — internal tooling (§4).

### Post-M9: SonarQube backlog — stage 2 (UI screens and widgets) (done)

- **Stage 2 of the backlog burn-down** (`refactor/sonar-ui`, stacked on stage 1
  — every stage adds a journal entry here, so independent branches would
  conflict; the linear-stack precedent from the installers track). Seventeen
  `rust:S3776` functions across the UI layer reduced below the threshold by
  **mechanical, behavior-preserving helper extraction** — and, notably, **none
  needed an Accept**: unlike `field_spec` or the LaTeX tables, every one of
  these had natural seams (a popup branch, a render section, a per-arm body).
  Delegated to three parallel subagents over disjoint file clusters, each with
  the constraints spelled out (extraction only, comments travel with the code,
  tests must pass unedited, no tree copies — the poisoned-`target/` lesson).
- **Chat screens + self-model** (4): `chat/input.rs::handle_key` (**62**, the
  worst offender of the whole backlog) split into a modal router, the
  Ctrl-shortcut ladder, the plain-key match and per-slash-command helpers —
  routing order preserved (the tool-confirmation popup stays checked before
  the generation gate); `chat/render.rs::render` (24) → input-area/overlay
  helpers; `chat/rag.rs::set_rag_progress` (21) → per-arm helpers;
  `self_model.rs::render` (20) → row-expansion/row-drawing/editor-popup.
- **Settings screens** (6): `render_fields` (**47**) → `desc_panel_height`/
  `render_fields_header`/`build_field_items`/`render_desc_panel`/
  `group_toggle_counts`; `render` (18) → `footer_hints`/`render_editor_popup`;
  `handle_key_inner` (24) → `is_quit_key` + `plugins_list_key`/
  `profiles_list_key` (returning `Option<Option<_>>` — "consumed?" × result,
  because the originals' `_ => {}` arms fall through to the rest of the
  dispatcher) + `navigation_key`; `handle_fields_key` (18) → `field_enter`;
  `helpers::render_field_line` (25) → `field_value_and_style`/`row_marker`;
  `helpers::parse_args` (26) → `read_double_quoted`/`read_single_quoted`
  (taking `&mut impl Iterator<Item = char>`, so the tokenizer state machine is
  untouched).
- **Widgets** (7): `message_feed::from_messages` (21) → `merge_round`;
  `build_lines` (20) → `refresh_cached_block`/`highlight_block_tail` (the
  `CachedBlock` invariants moved verbatim); `highlight_line` (19) →
  `span_cut_points`; `status_bar::lines` (26) → `state_spans`/
  `token_counter_span`/`hotkey_list`; `status_bar::right_grid` (25) —
  triaged rather than assumed: the *algorithm* (`grid_layout`) was already its
  own under-threshold function, so the renderer split cleanly along
  row-lead/cell seams (`push_row_lead`/`push_hotkey_cell`) and no Accept was
  needed; `chat_list::on_key_search` (28) → `on_ctrl_search`/`open_selected`/
  `select_down`/`start_rename`; `input_box::on_key` (17) →
  `on_ctrl_shortcut`/`on_edit_key` (mutator calls moved untouched, so the
  `touch()`/`record_undo` sequencing is byte-identical).
- **No test was edited anywhere** — the 1835-test suite is the safety net the
  whole stage leans on, and it stayed green as-is: **1835 passed / 0 failed**,
  76 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
  **No live run required** (AGENTS.md §3): pure UI refactor — no engine,
  memory, tool or provider path is touched (the precedent set by the whole
  settings-redesign track). No CHANGELOG entry — internal refactor (§4).

### Post-M9: SonarQube backlog — stage 3 (runtime and shared) (done)

- **Stage 3** (`refactor/sonar-runtime-shared`, stacked on stage 2). Twelve
  `rust:S3776` functions across `app/runtime`, `shared`, `entities`,
  `features/spellcheck` and `main.rs` — again **all closed by extraction, none
  by Accept**: the three flagged going in as "may be inherent" (the raw-HTML
  state machine, both cloud wire translations) each turned out to have a seam
  the existing structure had already drawn.
- **Runtime** (2 agents in parallel over disjoint files, as in stage 2):
  `run_loop` (**46**) → `spellcheck_upkeep`/`spinner_frame_needed`/
  `draw_frame`/`handle_input_tick` — tick ordering and the `dirty` semantics
  untouched, the load-bearing comments (synchronized output, full-redraw
  triggers, paste batching) traveling with their code; `apply_event` (29) →
  seven arm-body helpers, the match itself staying the dispatcher;
  `process_input_batch` (17) → `handle_key_event`.
- **Shared**: `markdown/html.rs::html_block_to_lines` (28) → `apply_tag` — the
  split follows the seam the `DROPPED_ELEMENTS`/`LINE_BREAKING`/
  `WORD_SEPARATING` tables already drew, so the scanner itself (indices,
  quotes, comments, the unterminated-`<` fallback) stays one piece;
  `i18n.rs::dotted_literals_in_src` (22 — a *test-module* gate scanner, the
  runtime fallback chain untouched) → nested fns lifted + `key_run`;
  `theme.rs::hotkey_grid` (20) → `grid_col_widths`/`grid_cols`/`grid_keycap`;
  `mcp.rs::read_loop` (18) → `deliver_reply`/`answer_server_request`;
  `gemini/wire.rs::thinking_config` (22) → `gemini3_level`/`gemini25_budget` —
  extracted rather than Accepted because the per-generation split is exactly
  the seam and all three thinking-config tests pin it (both 2.5-Pro clamps
  included); `responses/wire.rs::build_input` (17) → `push_assistant_items`,
  pinned by the reasoning-item-ordering tests.
- **The rest**: `dict.rs::load_dir` (19) → `load_entry`;
  `entities/self_model.rs::apply_edit` (19) → `assign_trimmed`/`assign_list`/
  `set_goal_text`/`clear_all` (now essentially the dispatch);
  `main.rs::apply_env_overrides` (16) → `apply_engine_env`/`apply_embed_env`.
- **No test edited**: **1835 passed / 0 failed**, 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean. **No live run
  required** (AGENTS.md §3): the two wire-file extractions are
  byte-identical request-shape moves pinned by their own wire unit tests, and
  the cloud protocols additionally ride the `MINDFORK_GEMINI_KEY`/
  `MINDFORK_OPENAI_KEY` smokes outside CI; nothing else touches an engine,
  memory or tool path. No CHANGELOG entry — internal refactor (§4).

### Post-M9: SonarQube backlog — stage 4 (orchestrator and tools) (done)

- **The final code stage** (`refactor/sonar-orchestrator-tools`, stacked on
  stage 3): nineteen `rust:S3776` functions on the engine/memory/tool paths —
  the most concurrency-critical part of the backlog, so the brief to both
  subagents was "fewer, larger, verbatim-moved helpers" with the invariants
  named up front. Again **all closed by extraction, none by Accept** — across
  the whole triage, the only functions whose complexity proved genuinely
  inherent were the seven identified up front in stage 0.
- **Orchestrator** (8): `generation.rs::spawn_generation` (**50**, the
  agentic loop) — the per-turn state moved into a private `TurnLoop` struct
  whose methods carry the loop verbatim (`run` / `tool_round` /
  `execute_call` / `resolve_call_result`, following the module's existing
  parameter-struct pattern — `GenSpawn`, `ConfirmGate`); channel order,
  effect timing, the confirmation round-trip, control-tool recognition,
  `sync_attachments` mirroring and the thinking-signature accumulation all
  moved unchanged. `tool_loop.rs::run_rounds` (19) → `read_round`/
  `invoke_allowed`; `tts.rs` ×3 (18/18/22) → `append_pieces`/
  `split_giant_word`/`synth_chunk` (the synthesize-ahead pipeline and
  pause/cancel semantics verbatim); `rag.rs::spawn_rag_rebuild` (26) — split
  along its own numbered phases into `gather_rebuild_sources`/
  `prepare_rebuild`; `reembed.rs::drain_store` (16) → `write_batch`, with
  both loop guards and the vector-then-stamp ordering staying where they
  were; `search.rs::reconcile` (17) → `reconcile_file` (the hidden-chat
  rationale comment traveling whole).
- **Tools/features** (11): `web.rs` ×2 (20/21 — provider order, throttling
  detection and best-effort degradation verbatim), `youtube.rs` (16),
  `self_model.rs` ×2 (**40**/22 — the atomic `self_model_update` closures now
  call helpers from *inside* the closure, so the atomicity boundary is
  untouched), `rag.rs` ×3 (20/23/22 — the chunk/stitch algorithms had natural
  seams after all), `notes/recall.rs` (21), `notes/self_notes.rs` (29),
  `mcp_import.rs` (18).
- **No test edited**: **1835 passed / 0 failed**, 76 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean — the exact
  pre-change baseline. **Live run — GO** (AGENTS.md §3): this stage touches
  the agentic loop and every tool path, and the LAN stack was unreachable at
  implementation time (both `192.168.1.20` servers timing out) — so the gate
  ran through the **remote HF runner** (`tools/e2e_hf.py run`, the exact
  scenario it was built for): Gemma 4 31B q4_0 on nvidia-l40s + bge-m3 and
  the alternate e5 embedder on T4s, llama.cpp `server-cuda`. **76 passed /
  0 failed** — the *entire* `#[ignore]` suite, not just the orchestrator
  e2e set: memory/self-model/notes/graph/cross-organ links/RAG/attachments/
  control tools/tool confirmation/MCP (a real `npx server-filesystem`,
  14 tools)/i18n/web/fetch, plus the four model-change smokes on the
  alternate embedder. Ready in 139 s, suite 1151 s, total 1290 s; all three
  endpoints deleted and verified gone, `list` empty.
- **With this stage the backlog burn-down is code-complete**: 57 open issues
  at the end of stage 0 → 0 expected after the four stage merges re-analyze
  on `main` (48 Rust + 9 Python fixed across stages 1–4).

### Post-M9: the quality gate stopped judging new-code coverage (done)

- **Found by the four backlog PRs meeting the blocking gate** — the first time
  the decision "make the gate blocking" (2026-08-05) met a pure refactor of
  legacy code. Three of the four failed, and the shape of the failure is the
  whole story: **zero new issues on all four**, ratings A, duplication 0%, and
  the *only* failing condition `new_coverage ≥ 80` — #269 at 0.0%, #271 at
  66.2%, #272 at 72.5%, with #270 passing at 96.1%. Analysis read through the
  SonarQube MCP server (registered in the user-scope Claude Code config this
  session) plus the REST API for the per-file breakdown the MCP tools do not
  expose.
- **Not a coverage regression, and that was measured rather than argued.**
  Overall project coverage: `main` 85.2%, the four PRs 85.2 / 85.3 / 85.2 /
  85.2. What changed is the *ledger*: extraction rewrites lines, so branches
  that had been uncovered for a long time stop being "old code" and start
  counting against the gate. Cross-checked against the repo's own `lcov.info`
  baseline, taken from `main` **before** these branches, and it matches file by
  file — `app/runtime/mod.rs` 29.6% on main → 63 of 63 new lines uncovered,
  `main.rs` 8.4% → 6 of 6, `orchestrator/tool_loop.rs` 26.7% → 27 of 27,
  `runtime/input.rs` 52.9% → 19 of 19.
- **The concentration is not a coincidence**: high cognitive complexity lives
  exactly where unit tests cannot reach — the TUI loop (needs a real terminal),
  the network/audio/cloud paths, the background tasks. Refactoring the most
  complex function in a file preferentially rewrites its *uncovered* half. And
  the percentage understates reality for those files: `web.rs`, `tts.rs`,
  `youtube.rs`, `tool_loop.rs` **are** tested — by the 76 `#[ignore]` live
  smokes (stage 4 ran 76/76 on the remote HF runner), which `cargo llvm-cov`
  does not execute. "Uncovered" there means "covered only by the live gate".
- **The fork that decided the design: `sonar.qualitygate.wait=true` does two
  jobs**, and the journal records the *second* as the reason it was enabled —
  it waits for server-side processing, which is what caught the report the
  server later rejected over the LOC quota while the job stayed green. So the
  obvious response ("make the gate non-blocking") would have thrown that away
  as collateral, since one flag carries both meanings. Four options were
  weighed: `wait=false` (loses the upload check), `continue-on-error` on the
  job (loses it too, and leaves a verdict that is visible but unread), coverage
  exclusions on `src` (inflates the number and hides real gaps in Rust code
  that *is* testable), and a custom gate.
- **Decision (user, 2026-08-06): a custom gate, blocking kept.** The built-in
  `Sonar way` cannot be edited (`isBuiltIn=true`; the organization has only the
  two built-ins), so the applied gate is now **"Sonar way without new-code
  coverage"** — its five other conditions, verified read-back to be identical
  to the source. The reasoning is marginal value: `clippy -D warnings`, 1835
  tests, `cyrillic_scan`, `link_check` and `cargo deny` already block, and what
  Sonar adds on top is cognitive complexity, security hotspots and duplication
  — which is what produced the 114-issue backlog and still blocks. Coverage is
  a metric the project already measures, stable at 85.2% and printed on every
  PR; blocking on it in a codebase with a deliberately untestable core was
  blocking on an accounting artifact.
- **The cost is stated rather than glossed** (in `sonar-project.properties` and
  the roadmap): a genuinely untested new feature can now pass the gate. What
  remains against that is the reported number and AGENTS.md §3.
- **`sonar.coverage.exclusions=tools/**` — done for measurement honesty, not
  for the gate** (which no longer judges coverage). The Rust-only lcov report
  cannot cover Python, so ~1240 lines of dev scripts sat in the denominator as
  100% uncovered, dragging the reported figure down for files that have no test
  harness by design. Coverage-only: they stay in `sonar.sources` and keep being
  analyzed for issues, so stage 1's nine Python findings are unaffected.
- **No CHANGELOG entry** — dev infrastructure with no user-visible effect
  (AGENTS.md §4), consistent with every other Sonar/CI entry. No Rust code
  touched, so the suite is unchanged at **1835 unit tests**, 76 `#[ignore]`.
  **No live run required** (§3): no engine, memory, tool or provider path
  exists in this change.

### Post-M9: the documentation refactor — CLAUDE.md became a router (done)

- **Trigger**: a session began with ~450K of its 1M-token window already spent,
  before any work started. Measured rather than estimated, and the cause was one
  file: `CLAUDE.md` is the only document loaded **in full, automatically**, and it
  had grown to **994 562 bytes (~249K tokens)** — of which the guide a session
  actually needs every time was **8 273 bytes** and the rest was a 243-entry
  engineering journal appended chronologically since M9. The remaining ~200K came
  from the standing instruction to also read `architecture.md` (144 KB) and the
  ADRs, both read whole though both are chaptered documents where a task needs two
  or three sections. Design plan and the measurements —
  [documentation-refactor.md](../history/documentation-refactor.md); forks F1–F7 all
  adopted as recommended (user's decision, 2026-08-08).
- **The diagnosis that shaped the split: the unit of loading was the file, the unit
  of need is the entry.** The average entry is ~4 KB and a typical task needs two or
  three; we were loading 243. And `docs/` was organized by **lifecycle**
  (`research/` before a decision, `history/` after a track) rather than by
  **subject**, so nothing answered "what must I read to work on the settings
  screen?".
- **What the journal is actually for** (established by reading across it before
  touching it, because the answer decides what may be moved): it is not a changelog
  — `CHANGELOG.md` is. Its unique content is **measurements** (85 entries) and
  **traps** (128 entries carry "worth recording"/"caught by"/"mutation-tested"
  language). The entries know it: ten cite an earlier entry's trap, and the
  `git checkout <file>` revert is recorded **three separate times** because it bit
  three times. The rest — what/why, forks, test counts — is largely duplicated into
  CHANGELOG, spec/architecture, and the design docs.
- **Split by subsystem along `architecture.md`'s chapters** (F1a), so the map is
  self-evident and mechanically checkable rather than ad-hoc: `docs/journal/`
  {`engine`, `storage`, `tools`, `self-model`, `notes`, `rag`, `ui-feed`,
  `ui-input`, `ui-screens`, `i18n`, `release`, `ci`, `quality`, `refactors`,
  `milestones`}`.md`, each with a reference header naming its architecture/spec
  sections and an index of its entries. Chronological **within** a file; the M3–M9
  milestone log moved to `milestones.md` rather than being deleted.
- **Two files were split again before the PR closed**, both at the user's request and
  both for the same reason — past ~150 KB a file stops being read and starts being
  grepped, which is the failure mode this track exists to fix. `memory.md` (180 KB,
  45 entries) became `self-model` / `notes` / `rag`, the project's own vocabulary for
  the three organs; attachments and the embedding stack went with `rag` as retrieval
  infrastructure, rather than earning a fourth file for machinery that serves all
  three. `platform.md` (152 KB, 32 entries) became `release` / `ci` / `quality`,
  split by who the work is for: shipping the product, running the pipeline, and the
  gates that block a PR. The seam that decides these is **track cohesion** — the six
  `release engineering` stages stay together even though stage 1 is a CI pipeline and
  stages 3–4 are schema migrations, because a track read in pieces is worse than a
  file with a slightly soft edge.
- **Moved verbatim (F3a), and that is what made it verifiable**: 243 entries,
  **961 820 bytes byte-identical**, zero missing, zero extra, matching SHA-256. The
  alternative on the table — compressing older entries while moving — was rejected
  because it is editorial judgement applied 243 times, unverifiable, and destroys
  exactly the measurements and reasoning that are the journal's unique content.
- **`docs/lessons.md` is the piece that makes the split safe** (F2a): 65 lessons in
  10 sections, deduplicated, each a rule plus the incident that produced it plus a
  pointer to its journal entry. While the journal was one file an agent working on
  *anything* incidentally knew every trap the project had hit; this file is that
  serendipity, made deliberate. Recurrence counts are kept as evidence — a trap
  recorded three times is a trap that is easy to hit.
- **CLAUDE.md is now a router**: **994 562 → 11 253 bytes** (−98.9%). Orientation,
  the standing decisions, FSD structure, conventions, commands, a short Status, and
  a **document map with trigger conditions** ("working on X → architecture §N ·
  spec §M · `docs/journal/<area>.md`") rather than a list of documents. Two standing
  rules stated there: *read the section, not the file*, and *the journal is per
  area*. `spec.md` and `architecture.md` were deliberately **not** split (F7a) —
  their stable numbering is cited from dozens of places (`spec §9.6`), and reading
  by section already gets the saving.
- **A gate, because the structure rots silently** (F5a, the house answer since
  `link_check.py`/`cyrillic_scan.py`): `tools/doc_index_check.py` (331 lines,
  stdlib) checks each file's index against its headings **as an ordered sequence**,
  duplicate titles within and across files, two-way reachability between the router
  and the journal files, that `lessons.md` exists and is linked, and that CLAUDE.md
  stays under `CLAUDE_MD_MAX_BYTES = 30_000`. A missing or empty `docs/journal/` is
  a **failure**, not a pass. Runs in the `lint` job — the job that runs on the
  docs-only PRs where documentation structure actually breaks.
- **Six traps hit while doing it**, all worth the record:
  (1) the first verification script compared the journal files against `CLAUDE.md`
  — which had already been rewritten, so it would have passed **vacuously**; the
  real check reads the original from `git show HEAD:CLAUDE.md`.
  (2) The classifier's `^M3 ` patterns were uppercase and matched against a
  lowercased title, so every milestone heading fell through to a keyword rule.
  (3) Relative links from `docs/journal/*.md` need one more `../` than the source
  they were copied from — the same class of miss the "plan moves to
  `docs/history/`" rule warns about, and **106 entry bodies carried it**. The
  interesting part is why it stayed hidden: `link_check.py` walks **git-tracked**
  files, `docs/journal/` was untracked, and the gate therefore reported *clean*
  three times in a row while never once looking at its subject. It only spoke up
  after `git add`. So the entries are verbatim with exactly one mechanical
  exception — root-relative link targets gained a `../../` prefix — and the
  byte-identity proof normalizes that prefix away before hashing, which is what
  keeps it a proof rather than an assertion.
  (4) The gate's own footer hardcoded `docs/documentation-refactor.md`, which moved
  to `docs/history/` mid-session — a stale path inside the tool that exists to
  prevent stale paths.
  (5) **12 entry titles contain `→` (U+2192)**, which cp1252 cannot encode, so the
  gate's console guard is load-bearing rather than ceremonial.
  (6) A scripted multi-edit pass that computes every replacement from the
  **original** text and writes once at the end keeps only the last edit per file;
  `link_check` cannot see it, because every surviving path still resolves. Re-run
  sequentially and re-read the diff.
- **Classification was scripted, then corrected by hand** — keyword rules over 244
  headings, then **38 manual overrides**, because the rules failed predictably:
  "status bar" entries landed in `ui-input`, memory-track refactors in `refactors`,
  and `settings-screen redesign — stage 2` in `engine` because its title contains
  the word "server". A heading is a weak classifier; what froze the result was the
  byte-identity check, not the rules.
- **Inbound references**: 91 across 41 files. **59 re-pointed** to the matching
  journal file (pointers to "the log", and process instructions telling an agent to
  write a journal entry); **32 left alone** because they genuinely still mean
  CLAUDE.md (orientation, `§Conventions`, `§Commands`, `§Status`). Deliberately
  untouched: the six mentions in `docs/history/english-source-migration.md`, which
  are statements of historical fact about translating the *file* — re-pointing them
  would falsify the record. Seven stale pointers in `src/**/*.rs` doc comments were
  fixed too, which is why this PR compiles code it does not change.
- **Result**: automatic load **~249K → ~2.8K tokens**; a typical task ("work on the
  settings screen" — router + architecture §10 + the relevant spec §§ +
  `journal/ui-screens.md` + lessons) is **~40–60K instead of ~450K**, with nothing
  deleted and everything reachable from the map.
- **Verification**: 243/243 entries byte-identical (hash-compared against the
  pre-change file in git); **1952 unit tests green, 81 `#[ignore]`** (unchanged — no
  behaviour touched); `link_check.py`, `cyrillic_scan.py` and the new
  `doc_index_check.py` clean; the gate mutation-tested in nine directions (appended
  entry without its bullet, stale bullet, swapped index order, duplicate title
  across files, unreferenced journal file, dangling reference, missing
  `lessons.md`, emptied journal directory, oversized router) — each fails on its
  own and only its own. **No live model run is required** (AGENTS.md §3):
  documentation, doc comments and one dev script; no engine, memory, tool or
  provider path is touched. **No CHANGELOG entry** — no user-visible effect (§4).
- **Process rules updated**: AGENTS.md §1 now sends a task through the map instead
  of through the whole of CLAUDE.md and states the read-the-section rule; §4's
  "journal entry" row names the matching `docs/journal/<area>.md` **and its index**,
  and a new row sends a recurring trap to `docs/lessons.md`.

### Post-M9: SonarQube follow-up — the doc gate's regexes and one test's complexity (done)

- **The first analyses after the 2026-08-07/08 merges left three open issues on
  `main`** (the quality gate itself stayed OK): two `python:S8786` super-linear
  regexes in the brand-new `tools/doc_index_check.py`, and one `rust:S3776`
  (21 > 15) on `message_feed.rs::cache_matches_fresh_render` — the cache-parity
  test, pushed over the threshold when the history-compression track made the
  compaction boundary a sixth nested loop (1+2+3+4+5+6 = 21 exactly). Branch
  `fix/sonar-followup`.
- **The regexes are the trap stage 1 fixed in `link_check.py`, reborn in a file
  that did not exist then** — now also a `docs/lessons.md` line, since twice is
  a pattern. `ENTRY_HEADING`/`INDEX_BULLET` ended `(.+?)\s*$`, where `\s+`, `.`
  and `\s*` all match the same whitespace, so every lazy expansion rescans the
  same tail. The title is now captured `(\S(?:.*\S)?)` — pinned to non-space at
  both edges, no overlap, linear. Atomic groups/possessive quantifiers were not
  an option: they need Python 3.11+, the project's declared floor is 3.10.
  Behaviour change: only the degenerate all-whitespace heading/bullet (old:
  an entry titled `" "`; new: not an entry at all) — none exist, and the new
  reading is the truer one.
- **Parity measured, not assumed** (the stage-1 pattern): old and new agree on
  every one of the **44 289 lines across the 93 tracked Markdown files**. On the
  pathological input (`"### a" + " "*n + "\nx"` — the embedded newline defeats
  `$`; unreachable through the tool's own `split("\n")`, but a pattern should
  not lean on its caller for its own complexity) the old pattern grows
  **27 → 99 → 397 ms** as n doubles through 2k/4k/8k — quadratic — while the new
  one stays under 0.2 ms. Also verified against the analyzer itself via the MCP
  snippet tool, in both directions: the old shape is flagged, the new one is
  clean — a positive control, not just an absence.
- **The test refactor is the stage-2/4 recipe**: the four inner loops
  (width × palette × thoughts × tools) moved **verbatim** into
  `assert_warm_matches_fresh(messages, compaction)`; the test keeps
  scenarios × compaction and the call. Complexity 3 + 10, both under 15, and
  no assertion, comment or grid point changed — the same
  scenarios × 2 × 2 × 2 × 2 × 2 sweep.
- **Verification**: **1952 unit tests green** (0 failed, 81 `#[ignore]` — the
  exact pre-change baseline; the helper is not a `#[test]`, so the count is
  unchanged), clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`/
  `doc_index_check` all clean. **No live run required** (AGENTS.md §3): a dev
  script and a test-module refactor — no engine, memory, tool or provider path
  is touched. No CHANGELOG entry — internal tooling and tests (§4).

### Post-M9: SonarQube follow-up — the screenshots SVG writer (done)

- **The 2026-08-10 analysis of `main` left two open issues** — the quality
  gate itself stayed OK, because it judges new-code *ratings*, which a pair of
  maintainability smells cannot flip: `python:S3776` on
  `tools/screenshots.py::render_svg`, at cognitive complexity **50** against
  the 15 allowed, and `python:S1871` — two identical "extend the current run"
  branches in its pass-2 dispatch. The function arrived with the site track's
  S4 (vector screenshots) carrying the whole SVG pipeline in one body: the
  scoped `<style>` block, per-row background-rect merging, and the text-run
  state machine as a `flush()` closure over five `nonlocal`s. Branch
  `fix/sonar-screenshots`.
- **Closed by the stage-2/4 recipe — mechanical extraction, no Accept**, along
  the seams the code's own comments had already drawn. "Pass 1" became
  `svg_bg_rects` (+ `bg_rect` for the rect string it emitted from two places);
  "Pass 2" became `svg_text_spans` over `cell_traits` (the per-cell
  classification); the closure's five `nonlocal`s became a `TextRun` state
  holder (`start`/`extend`/`take`) with a module-level `flush_run`; and a
  `SvgGrid` NamedTuple — the sub-pixel cousin of the raster `Metrics` —
  carries cell_w/cell_h/ascent/pad. `render_svg` keeps parsing, the header,
  the style block and the row loop.
- **The S1871 twins merged into one guard** instead of staying as two arms
  with one body: a cell extends the run iff the style key matches and —
  hidden — it is a plain space, or — visible — the primary face can place it
  at native advance (`key == run.key and (s == " " if hidden else not solo)`).
  The six-way behaviour table is unchanged; the remaining branches are
  distinct again.
- **Verification leaned on the pipeline's determinism** (the lessons.md
  regeneration recipe): all 10 dumps re-rendered **byte-identical to the 20
  committed artifacts** (10 PNG + 10 SVG) — first *before* the change, proving
  the local faces faithful, then after it, proving zero output drift. The
  refactored file also went back through the analyzer itself (the MCP snippet
  tool, this follow-up series' pattern): **0 issues** — both findings gone,
  nothing new introduced.
- **1977 unit tests green, 85 `#[ignore]`** (no Rust touched — the exact
  baseline), clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`/
  `doc_index_check` all clean. **No live run required** (AGENTS.md §3): a dev
  script only — no engine, memory, tool or provider path. No CHANGELOG entry —
  internal tooling (§4). The gate-green-but-findings-ship gap is now a
  `docs/lessons.md` §10 line.

### Post-M9: SonarQube follow-up — `chat_search`'s renderer (done)

- **The two issues open on `main` after the `/export` merge**, both raised by
  tracks that shipped since the last sweep. The quality gate stayed OK
  throughout — it judges new-code *ratings*, which two maintainability smells
  cannot flip (the §10 gap again: green gate, findings shipped). Branch
  `fix/sonar-chat-search-complexity`.
- **`rust:S3776` on `ChatSearch::invoke`** (`src/features/tools/chats.rs`),
  cognitive complexity **17** against the 15 allowed. Arrived with the
  cross-chat search track. Not a deliberate shape — a linear pipeline
  (scope → query → search → count) with a two-level rendering nest bolted onto
  its end, so the fix is the stage-2/4 recipe: the nest moved **verbatim** into
  `render_grouped_hits(ctx, hits, query, k) -> (String, usize)`, and the
  per-hit heading (the `Some(page)`/`None` pairing plus its snippet) into
  `render_hit`. Both new functions and the remaining `invoke` are well under
  the bar, so the threshold is *met* by every part rather than waived for the
  whole. `shown` is returned rather than recomputed because `k` bounds the
  hits, not the conversations — the header's count is what the loop reached.
- **The one behavioural detail worth stating**: `build_snippet` used to run
  before the page lookup and now runs after it. Both are pure and neither
  reads the other's output — the emitted string is byte-identical, which the
  existing `chat_search` tests pin (the paged, unpaged, grouping and `top_k`
  cases all assert on rendered text).
- **`rust:S2208` on `src/screens/chat/commands.rs:15`** — `use super::*`, the
  god-object-split convention from
  [refactoring-god-objects.md](../history/refactoring-god-objects.md) and
  architecture.md §3. The file is stage 1 of the command-only-control track and
  postdates the 2026-08-06 triage, so it re-raised a rule whose other 49
  instances are already **Accepted**. Accepted in Sonar with the same
  rationale (user decision 2026-08-15) rather than fixed: nine sibling modules
  of `screens/chat/` carry the same line, and spelling out one of them alone
  buys nothing but a divergence.
- **And then stopped from recurring**, which is the part worth keeping. Marking
  the issues Accepted one at a time is what 2026-08-06 did, and this entry is
  the proof it does not scale: a standing decision that has to be re-applied by
  hand every time a chapter is added is a decision the repository does not
  actually hold. It holds it now — `sonar.issue.ignore.multicriteria` in
  `sonar-project.properties` turns `rust:S2208` off in the **six directories
  that are splits** (34 files: `app/runtime`, `screens/chat`,
  `screens/settings`, `features/tools/notes`, `shared/markdown`,
  `shared/storage/db`) and nowhere else, so a wildcard import elsewhere in
  `src/` is still a finding. `*.rs`, not `**/*.rs`: none of the six has nested
  modules today, and a future one should have to argue for itself. Test
  modules need no entry — the rule text already exempts any module with `test`
  in the name, which is what covers `app/orchestrator/tests/` (21 more files).
- **The property is settable from the file, and the reason is the note already
  at the top of it**: what cannot read `sonar.issue.ignore.*` is *Automatic
  Analysis*, which this project keeps off because CI-based analysis refuses to
  run beside it. Checked against the docs rather than assumed, together with
  the two alternatives that were rejected for being undocumented for Rust —
  `#[allow(clippy::wildcard_imports)]` and `// NOSONAR` appear nowhere in the
  Rust analyzer's documentation, and a suppression nobody guarantees is worse
  than none. Deactivating the rule in a custom quality profile was the other
  option and was declined: it is invisible from the repository, and this
  project's decisions live in files that show up in review.
- **The exclusion is measured, not assumed** — and the control turned out
  cleaner than the one planned for it. The branch itself could not prove
  anything (the `commands.rs` issue was Accepted, so absent either way, and a
  PR analysis reports only *new* issues), so the plan was to reopen it and read
  the next analysis of `main`. The post-merge analysis answered without that:
  **all 50 `rust:S2208` issues went `CLOSED`**, the 49 accepted in 2026-08-06
  included. The second direction came free from the same dump — the **7
  deliberate-shape `rust:S3776`** acceptances (`settings/spec.rs`,
  `calc.rs`, `latex.rs` ×4, `wrap.rs`) are still `RESOLVED`. So the server does
  **not** close accepted issues on its own, which is exactly the alternative
  explanation that had to die: the fifty closed because they stopped being
  raised. Project-wide open issues: **0**.
- **Worth keeping for the next time this rule comes up**: an issue exclusion
  does not merely hide findings from a list, it removes the issues, Accepted
  ones with them. That is the intended outcome here — the acceptances were
  bookkeeping for a decision now stated in the properties file — but it means
  an exclusion is not reversible into the old state: reverting the property
  would re-raise all fifty as OPEN, not as Accepted.
- **Verification**: **2282 unit tests green** (0 failed, 97 `#[ignore]` — the
  exact pre-change baseline; a pure extraction adds no test), clippy
  `-D warnings`/fmt clean. **No live run required** (AGENTS.md §3) — the
  change moves rendering code inside one tool module and touches no engine,
  memory, storage or provider path; the tool's own behaviour is unchanged. No
  CHANGELOG entry — internal refactor with no user-visible effect (§4).

### Post-M9: SonarQube follow-up — two new lint families, and eight complexity findings (done)

- **The backlog was back at 30 open issues** on `main` (read through the Sonar
  MCP server, 2026-08-21) — and two thirds of it is not a regression at all.
  Branch `fix/sonar-backlog-followup`. The split: **14 × `rust:S1612`**
  ("replace this closure with a reference to the method"), **8 × `rust:S8863`**
  ("remove this redundant `'static`"), **7 Rust + 1 Python `S3776`**
  (cognitive complexity). The two lint families had never raised anything here
  before and their sites date back to 2026-06-17 — code that had passed every
  analysis since the four-stage burn-down closed the backlog at zero. An
  analyzer that gained rules, not a codebase that slipped.
- **`cargo clippy --all-targets -- -D warnings` is green on all twenty of
  them**, before and after — measured, not assumed, on a scratch crate
  reproducing the four shapes:
  - `S1612` maps to clippy's `redundant_closure_for_method_calls`, which is
    **pedantic**, so the project's default warn set never sees it;
  - `S8863` maps to `redundant_static_lifetimes`, which *is* on by default —
    but it fires only on a free `const`/`static`. It does not reach an
    **associated** const (`impl Lang { const ALL: &'static [Lang] }`,
    `SelfModelScreen::HOTKEYS`) nor a `'static` nested inside a generic
    argument (`static POOL: OnceLock<Mutex<HashSet<&'static str>>>`) — which
    is every one of the eight sites. Enabling the lint explicitly changes
    nothing; Sonar's implementation is simply broader.
- **The twenty lint fixes are mechanical and behaviour-free**: nine
  `.and_then(|e| e.to_str())` → `.and_then(OsStr::to_str)` (one
  `use std::ffi::OsStr;` per file), three `.map(|id| id.to_string())` →
  `.map(ToString::to_string)`, one `take_while(char::is_ascii_alphanumeric)`,
  one `existing.map(<[u8]>::to_vec)`, and the eight `'static` deletions — in
  a `const`/`static` item type an elided reference lifetime already *is*
  `'static`, so nothing changes but the character count. Only the item types
  were touched; `&'static` in a signature, a field or a `Box::leak` return is
  not redundant and was left alone, as was the surrounding prose that
  correctly still talks about `'static`.
- **All eight complexity findings closed by extraction, none by Accept** —
  the same verdict the four burn-down stages reached, and for the same reason:
  each had a natural seam.
  - `tools/code.rs::grep` (**37**) → `searchable` / `clip_hit` / `scan_text` /
    `search_files`. Most of the 37 was the `spawn_blocking` closure, which is
    a *sync* function that had been written inline; naming it also let the
    nested `match` over the glob collapse into one `.transpose()`.
  - `tools/code.rs::strip_ansi` (24) → `skip_csi` / `skip_osc`, one function
    per escape family. `tools/code.rs::list` (17) → `collect_entries`, the
    same "the blocking half has a name" move as `grep`.
  - `orchestrator/generation.rs::handle_done` (19) → `is_first_reply` +
    a free `apply_effects` (the effect `match` cannot be a method: it runs
    inside the `chat` borrow, which is the whole reason attachments are
    collected rather than applied). `::tool_round` (16) → `final_round`
    (the round-limit final round, comments and all) + `file_round` (the
    rewrite/followup dispatch).
  - `tools/present.rs::compact_args` (17) → `code_block` / `big_string_block`
    / `scalar_pairs`; the "tool's own code field outranks the large-string
    fallback" precedence, previously an `if blocks.is_empty()` after the
    fact, is now literally `code_block(..).or_else(|| big_string_block(..))`.
  - `chat_export.rs::format_assistant` (16) → `thoughts_part` / `tool_parts`.
  - `tools/probe_runs.py::main` (29) → `_utf8_streams` / `_measure` / `_tally`.
    `_measure` returning `None` is the "the filter matched nothing, stop
    everything" case, which is the one branch in that script that must not be
    mistaken for a zero (docs/lessons.md §9); its docstring says so now.
- **Two tests added, and only two** — a pure extraction does not need them,
  but two of the moved branches had never been covered and were about to
  become their own functions: `strip_ansi`'s OSC-closed-by-ST path (`ESC \`
  rather than BEL), its two-character escapes, and three unterminated
  sequences; plus `clip_hit`'s character-counted cap. Everything else is
  pinned by the tests that already existed — the `code_grep` set, the 24
  `present.rs` cases, the `chat_export` formatting set — none of which was
  edited. `_tally` has no such suite, so it was exercised directly (GO,
  NO-GO and a missing verdict) rather than only read; its format strings
  moved verbatim.
- **Verification**: **2418 unit tests green** (0 failed, 106 `#[ignore]` —
  the 2416 baseline plus the two above), clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check`/`doc_index_check`/`wizard_rtf --check` clean.
  **Live run — GO** (AGENTS.md §3): the change touches the agentic loop and
  the code-workspace tool paths, so the whole orchestrator e2e set ran
  against the LAN stack (Gemma 4 31B q4_0 on `192.168.1.20:8000`, bge-m3 on
  `:8001`) — **42 passed / 0 failed** in 815 s, the code-workspace smokes
  (`code_workspace_navigate`/`_gate`, `code_edit`/`_ambiguity`/`_recovers`,
  `code_build`, `code_command_gate`) among them, plus the round-limit,
  rewrite/followup and auto-title paths that `tool_round` and `handle_done`
  carry. No CHANGELOG entry — internal refactor with no user-visible effect
  (§4).
