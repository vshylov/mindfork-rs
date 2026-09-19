# Journal — Static analysis and the repository's gates

The checks that block a pull request and the cleanups they drove: SonarQube analysis, the quality gate and its backlog, and the gates guarding the repository's own structure — source language, relative links, documentation layout.

**Reference documents for this area:** architecture.md §12, AGENTS.md §4, §6

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (27)

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
- Post-M9: SonarQube follow-up — four findings from the week's merges (done)
- Post-M9: a gate for the one implementation of a list's scroll state (done)
- Post-M9: SonarQube follow-up — the help dialog's table builder (done)
- Post-M9: SonarQube follow-up — three complexity findings from the `/continue` merges (done)
- Post-M9: SonarQube follow-up — the chat-body stub's complexity (done)
- Post-M9: SonarQube follow-up — the dialogue director and its probe (done)
- Post-M9: SonarQube follow-up — the wizard's RTF renderer, and a fixture's file mode (done)
- Post-M9: SonarQube follow-up — the budget's poison guard and the pool probe (done)
- Post-M9: the pool probe's `park` arm records its sibling (done)
- Post-M9: SonarQube follow-up — the round resolver's three arms (done)
- Post-M9: SonarQube follow-up — blocking file calls in the two setup paths (done)
- Post-M9: SonarQube follow-up — the file launcher's two closures (done)
- Post-M9: the quality-gate badge, and the one Dependabot badge that is not a claim (done)
- Post-M9: SonarQube follow-up — the IndexNow tool's three findings (done)

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

### Post-M9: SonarQube follow-up — four findings from the week's merges (done)

- **Four open issues on `main`, no hotspots** (the Sonar MCP server,
  2026-08-23). Branch `fix/sonar-followup-four-findings`. Three `rust:S3776`
  — `orchestrator/reembed.rs::spawn_reembed` (**18**, the attachment
  backfill of #369), `orchestrator/title.rs::handle_title_result` (**18**,
  the transcript titling of the sub-agent track) and
  `screens/chat/input.rs::handle_enter` (**16**, the read-only refusal
  added one more link to its chain) — plus one `powershelldre:S8677` on
  `tools/install_inno.ps1` (the Inno Setup 7 track): a function named
  `Write-Step` that calls `Write-Host`. Every one of them passed the PR
  gate that introduced it, as this genre always does: the gate *rates* new
  code, it does not count its smells, so a handful of complexity findings
  inside a large PR never moves the rating off A. The follow-up round is
  the mechanism, and this is the fifth.
- **All three complexity findings closed by extraction, none by Accept** —
  the same verdict the four burn-down stages and the previous follow-up
  reached, and for the same reason: each had a natural seam.
  - `spawn_reembed` → a `run_reembed` that returns
    `Result<RagProgress, String>`, so the spawn does one thing: send the one
    terminal event (the lessons §4 invariant now lives in one line rather
    than in seven `send(Failed); return` arms). The three prechecks became
    `plan_reembed` (four `match`-and-return arms → `?`), the stale-mark
    lift `lift_stale_marks`, and the pair of "count the stage's errors,
    stop if it was fatal" blocks — written out identically after the
    backfill and after each store — became `Drained::absorb`. `Plan::total`
    names the banner's total.
  - `handle_title_result` → `apply_title(chat_id, origin, title) -> bool`:
    the chat-or-transcript write with the D1 rule
    (docs/history/auto-chat-title.md) in one place, the `Result` unwrapping
    and the `ChatRenamed` emission around it.
  - `handle_enter` → a `COMMAND_PARSERS` table (`&[fn(&mut ChatScreen,
    &str) -> Option<Option<ChatIntent>>]`) walked by `find_map`. Twelve
    `if let Some(intent) = self.try_x(&text) { return intent; }` were one
    point of complexity each and said nothing a list does not; the order
    *is* the precedence, and the comments that justified a position (the
    refusal first, `try_ui_command` before the `generating` gate, the two
    parsers that keep their own grammar) moved onto the rows they explain.
- **S8677 was a rename, deliberately not the rule's other fix.** The
  script's stdout is its return value — the ISCC path, captured by both
  workflows — so `Write-Output` for the progress lines would corrupt it;
  `Write-Host` is correct there. PowerShell's way of saying "this function
  is display-only" is the `Show-` verb, so `Write-Step` became `Show-Step`,
  with a comment stating why the host is the right sink. Run on the
  development machine (Inno 7.1.0 present, so the idempotent path): the
  progress lines reach the console and `$out = ./tools/install_inno.ps1`
  captures the path alone, with and without `-Quiet`.
- **Mutation-tested, as lessons §2 asks — eight mutations, and the
  survivors earned the round its tests.** Four were killed by existing
  tests (the chat-side D1 rule, the refusal-first order, the stale-mark
  lift, the backfill's share of the total). One was a null mutation: it
  *added* a second `try_read_only_refusal` at the end of the table without
  removing the first, and indicted itself rather than the test (the
  lesson's "read a surviving mutation twice"). Three were real gaps, all
  older than this change:
  - `found && !dropped` → `found` survived: the **transcript** half of D1 —
    an automatic title arriving after a sub-agent transcript was renamed by
    hand — had never been tested, only the chat half. Now
    `manual_rename_outranks_the_automatic_title_on_a_transcript`, the
    mirror of the chat test down to the requested-origin positive control.
  - `*errors += self.errors` removed survived: no input the job harness can
    provoke makes a row write fail without being fatal (it needs a broken
    database). `absorb` is pure, so its one claim — errors are counted
    whether or not the stage was fatal — is pinned directly
    (`absorb_counts_errors_and_stops_on_fatal`) instead of claimed in a
    comment nothing checks.
  - `if total == 0` disabled survived: the nothing-to-do test asserted the
    terminal event and nothing about the `Started { total: 0 }` banner the
    mutant flashes first. It now asserts the event list is *exactly* the
    terminal event — which is the behaviour the comment promises ("say so
    plainly rather than pretending work happened").
- **Verification**: **2526 unit tests green** (0 failed, 107 `#[ignore]` —
  the 2524 baseline plus the two above), clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check`/`doc_index_check`/`wizard_rtf --check` clean;
  the PowerShell script parses (`Parser::ParseFile`, zero errors) and ran as
  above. **No live run**, stated per AGENTS.md §3: the Rust side is a pure
  extraction — no protocol, request or storage call changed — and the
  re-embed job's whole path (every store, the dimension change, the
  backfill, a dead embedder, cancellation) runs under `MockEmbedder` in its
  thirteen unit tests; the LAN stand was also down at the time, so
  `reindex_restores_retrieval_after_a_model_swap_live` stays with the next
  change that touches the job's behaviour. No CHANGELOG entry — internal
  (§4).
### Post-M9: a gate for the one implementation of a list's scroll state (done)

The two preceding PRs fixed a scrolling defect in eight lists and consolidated
the rule into `shared::ui::ListScroll`. What was left holding it there was a
paragraph in [lessons.md](../lessons.md) §5 — and a convention is precisely what
let the defect spread in the first place: every one of the eight lists was
written by copying a neighbour that already had it. The user asked for the gate
next.

**What it checks** (`tools/list_scroll_check.py`, CI's `lint` job, no toolchain
needed): a `ListState` — or a `TableState`, which carries the identical
offset-plus-selection semantics and would reproduce the defect the day someone
adds a table — may be named only in `src/shared/ui.rs`. Mentions inside `//`
comments pass, because the doc comments and journal entries that explain the trap
have to be able to name it; the word match is exact, so our own
`ChatListState`/`ProfileListState` are not swept up. Two further checks exist so
the gate cannot go quiet on a missing subject (`doc_index_check.py`'s rule): it
fails if `src/` holds no Rust files, and if `pub struct ListScroll` is no longer
defined where the gate points — a helper deleted or renamed means every list has
lost the rule, which is the loudest thing this script can be asked to notice.

**Deliberately not checked**: whether a caller passes a `view_h` that matches its
block's borders, or a `len` that matches its items. Those are values only the
caller knows, and the tests next to each list cover them; a gate that guesses at
them would be a gate that gets edited to shut it up.

**Verified against planted violations** rather than only against a clean tree — a
gate nobody has seen fail is a gate nobody knows works: a `ListState::default()`
added to the profile picker, a `use ratatui::widgets::TableState`, and the helper
renamed out from under it each produce the expected failure and exit code 1, and
the tree is clean at 250 files scanned.

**The gate went through the gate**: `tools/` is in `sonar.sources`, and the
first analysis of the PR returned two `python:S6353` — the explicit
`[A-Za-z0-9_]` lookarounds that keep `ChatListState` from matching should be
plain `` word boundaries. They are the same assertion (between the `t` of
`Chat` and the `L` of `List` there is no boundary), so the rewrite is
behaviour-identical — re-checked against the planted violations rather than
assumed.

**No live run and no new Rust tests** — a Python gate over the repository's own
structure (AGENTS.md §3).

### Post-M9: SonarQube follow-up — the help dialog's table builder (done)

- **The one issue open on `main`** after the help-hotkeys track landed:
  `rust:S3776` on `table_lines` (`src/widgets/help_dialog.rs`), cognitive
  complexity **18** against the 15 allowed. The quality gate stayed OK
  throughout — one maintainability smell cannot flip a new-code rating, the
  `docs/lessons.md` §10 gap again. Branch `fix/help-table-lines-complexity`.
- **Why it grew**: the function does two jobs that the stage-2 recipe would
  have split from the start. Stage 1 of the help track made it render
  *sections* rather than one flat list of rows, so a header, a "you are here"
  marker and a blank line between sections were folded into a body that already
  carried a two-level resolve nest (per section → per row, with the
  command-label `if`/`else` inside it) and a two-level emit nest (per section →
  per row, with the group-opener `if` inside it). Neither nest is a deliberate
  shape: they are two separate passes sharing one `fn` because the second one
  needs the first one's measurement.
- **The fix is the stage-2/4 recipe** — the nests moved out **verbatim**, one
  per pass. `resolve_rows(section, command_labels, palette, loc)` returns the
  new `ResolvedRows` alias (the tuple the old local `type Rows` half-named),
  and `push_section(lines, header, rows, …) -> Option<usize>` emits one
  section, returning the marked header's row index rather than writing to a
  captured `mut`. What is left in `table_lines` is what the name promises: map
  the sections, measure the shared description column, walk them with a blank
  line between. All three parts are well under the bar, so the threshold is
  *met* by every part rather than waived for the whole.
- **Behaviour is unchanged by construction**: the emitted `Line`s are built by
  the same expressions in the same order, and the anchor keeps its
  last-marked-wins semantics (`if let Some(at) = push_section(…) { marked_at = Some(at) }`
  — an `.or()` would have quietly made it first-wins; only one section is ever
  marked, but the reader should not have to know that to trust the line).
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2579 green, count unchanged**: `table_lines` is private, the tests exercise
  it through `hotkeys_tab`/`key_lines`, and the wrapping, column-alignment,
  per-locale fit and anchor cases all still pass untouched. **No live run
  required** (AGENTS.md §3) — pure UI, no engine, memory or tool surface. No
  CHANGELOG entry: nothing the user sees changed (§4).

### Post-M9: SonarQube follow-up — three complexity findings from the `/continue` merges (done)

- **Three `rust:S3776` issues open on `main`** after the `/continue` track's
  merges (2026-08-27) — the `docs/lessons.md` §10 gap again: a green PR quality
  gate judges new-code *ratings*, so a handful of maintainability smells
  surface only on the next `main` analysis. All three functions crossed the
  bar of 15 on that track's own growth: `handle_done` (20) by the seed-merge
  block, `stream_round` (21) by the echo filter and the `continuable` route in
  its error arm, and `apply_event` (16) by the `GenerationStarted`
  continuation branch. Branch `refactor/sonar-cognitive-complexity`.
- **`apply_event` got the stage-2 recipe verbatim** (the dispatch-`match` shape
  lessons.md already names): the seven precondition-carrying arms —
  `ServerStatus`, `ChatList`, `ChatSearchResults`, `GenerationStarted`,
  `SelfModelChanged`, `McpImportResult`, `Compacted` — each moved to a named
  helper beside the existing arm helpers (`show_self_model` and kin), doc
  comments carrying the arms' routing rationale. The match is now a table of
  one-liners: complexity ~1, and every helper is a single guard.
- **`stream_round` lost its two nested branches to named mechanisms**:
  `strip_echo` (the continuation echo filter over one delta, `None` → pass
  through) and `relay_text` (accumulate + `Chunk` to the feed, empty delta —
  a fully-withheld echo — sends nothing). The `ThoughtsSignature` arm's
  keep-the-last-id `if` became the branch-free `thoughts_id = r.id.or(thoughts_id)`
  — same last-`Some`-wins semantics. ~21 → ~9.
- **`handle_done` gave up two coherent sub-steps**: `carry_inflight_rename`
  (retire the in-flight mirror, carry a manually renamed transcript title onto
  the landed run — the take() still runs before the empty-result early return,
  so the mirror is dropped either way) and `land_continuation` (the `/continue`
  seed merge, `let…else` guards + the vanished-seed fallback). One knock-on:
  `record_deleted` had *moved* `res.deleted` out, which a whole-`&mut res`
  borrow no longer tolerates — `std::mem::take(&mut res.deleted)` keeps `res`
  whole with identical behaviour. ~20 → ~12.
- **Behaviour is unchanged by construction** — every extracted body is the same
  expressions in the same order, and the one rewrite (`Option::or`) is an
  identity for the replaced `if`. The Sonar snippet pre-check takes no Rust
  (lessons §10), so the PR analysis is the measurement that closes these.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2633 green, 118 `#[ignore]`, counts unchanged**: the touched paths are
  exercised through the orchestrator and runtime tests
  (`merge_continuation`'s own tests included). **No live run required**
  (AGENTS.md §3) — a pure refactor, no engine, memory or tool surface changed.
  No CHANGELOG entry: nothing the user sees changed (§4).

### Post-M9: SonarQube follow-up — the chat-body stub's complexity (done)

- **One `rust:S3776` issue open on `main`** after the external-model-name
  merge (2026-08-28) — the `docs/lessons.md` §10 gap again: a green PR quality
  gate judges new-code *ratings*, so the smell surfaced only on the next
  `main` analysis. `chat_body_stub` (`src/app/supervisor.rs`, the test-module
  helper that reports the first `POST` body an external turn puts on the wire)
  landed at cognitive complexity **23** against the 15 allowed: one spawned
  closure carried the accept loop, the read-to-`\r\n\r\n` loop, the
  `Content-Length` scan and the drain-the-body loop, nested four deep.
  Branch `refactor/sonar-chat-body-stub`.
- **The stub lost its middle to two named mechanisms**: `read_request` (one
  request off the socket — headers to their `\r\n\r\n` end, then exactly
  `Content-Length` more bytes, `None` when the peer goes away mid-headers) and
  `content_length` (the length a request head announces, `0` when absent).
  The closure is now the loop its doc comment describes — accept, read,
  answer after draining, return the first `POST` body — with a single-loop
  depth on each side of the split, well under the bar.
- **Behaviour is unchanged by construction** — every extracted body is the
  same expressions in the same order; the one rewrite is that the body string
  is now built for every request rather than only the `POST` one, an identity
  in the returned value. The Sonar snippet pre-check takes no Rust
  (lessons §10), so the PR analysis is the measurement that closes this.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2653 green, 120 `#[ignore]`, counts unchanged**: the stub is exercised by
  the two model-name wire tests it serves (`external_chat_sends_the_configured_model`,
  `a_blank_model_field_sends_no_model_key`). **No live run required**
  (AGENTS.md §3) — a pure refactor of a test-module helper, no engine, memory
  or tool surface changed. No CHANGELOG entry: nothing the user sees changed
  (§4).
### Post-M9: SonarQube follow-up — the dialogue director and its probe (done)

- **Two `rust:S3776` issues open on `main`** after the `run_dialogue` merges
  (2026-08-28/29) — the `docs/lessons.md` §10 gap once more: a green PR quality
  gate judges new-code *ratings*, so a pair of maintainability smells surfaces
  only on the next `main` analysis. Both are the same shape, the one the
  verdict protocol invites — a `match` whose five arms each carry a whole
  mechanism inline. `dialogue_checkpoint` (`src/app/orchestrator/generation.rs`)
  landed at cognitive complexity **37** against the 15 allowed, and the stage-0
  probe's `run_dialogue` (`src/shared/api/dialogue_probe.rs`) at **57** — the
  highest figure this journal records. Branch
  `refactor/sonar-dialogue-complexity`.
- **`dialogue_checkpoint` gave up its script and its four acting verdicts**:
  `dialogue_script` (the lines the director has not been shown, labelled by
  speaker, plus the `rendered` advance), then `dialogue_stop`, `dialogue_note`,
  `dialogue_retry` and `dialogue_rewrite`. What is left is the checkpoint its
  doc comment describes — render the increment, ask, record the verdicts in the
  director's own conversation, apply them in call order — with the verdict
  `match` a table of one-liners. The two `continue`s that skipped a verdict
  became early returns inside `dialogue_retry`/`dialogue_rewrite`, which is the
  same skip. ~37 → ~5.
- **The probe's `run_dialogue` lost the `speak` closure and the whole
  checkpoint block**: `speak` is a free `async fn` now (the muted re-ask for the
  all-thinking empty turn included), the checkpoint is `checkpoint`, and its
  five verdicts are `verdict_stop` / `verdict_note` / `verdict_retry` /
  `verdict_rewrite`. Two shapes carried the split: the run's mutable state —
  transcript, standing notes, issued directions, report, seed, next checkpoint —
  moved into a `RunState` struct instead of a seven-parameter thread, and the
  `'dialogue`-labelled break became a `Checkpoint::{Continue, Stop}` return, so
  the two places that end a run (the director's `dialogue_stop`, and a retry
  whose regeneration would pass `max_messages`) say so in the type rather than
  by jumping out of a nested match. ~57 → ~8.
- **Behaviour is unchanged by construction** — every extracted body is the same
  expressions in the same order; the one rewrite is that label-break, and it
  ends the loop at exactly the same two points and abandons the remaining
  verdict calls exactly as `break 'dialogue` did. The Sonar snippet pre-check
  takes no Rust (lessons §10), so the PR analysis is the measurement that closes
  these.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2676 green, 126 `#[ignore]`, counts unchanged**: the director path is
  exercised by the orchestrator's dialogue tests (`dialogue.rs` drives all five
  verdicts through the mock), and the probe is `#[cfg(test)]` `#[ignore]` code
  the live gate runs. **No live run required** (AGENTS.md §3) — a pure refactor,
  no engine, memory or tool surface changed. No CHANGELOG entry: nothing the
  user sees changed (§4).

### Post-M9: SonarQube follow-up — the wizard's RTF renderer, and a fixture's file mode (done)

- **Three findings open on `main`**: two on `tools/wizard_rtf.py` from the
  legal-pages merges of 2026-09-01 — `python:S3776`, `render` at cognitive
  complexity **23** against the 15 allowed, and `python:S6353`, the
  `{1,}` quantifier that is spelled `+` — plus one older `rust:S2612`
  (2026-07-21) on the sandbox-setup test fixture. The Python pair is the
  lessons §10 recurrence in its exact prescribed form: the file was *reshaped*
  (the pipe-table branch and two more source→output pairs came in with
  `PRIVACY.md`), which is precisely when that lesson asks for the MCP snippet
  pre-check, and it was not run. Branch `fix/sonar-wizard-rtf-complexity`.
- **`render`'s complexity was its closure shape, not its logic.** One function
  held four pieces of accumulating state, two nested functions closing over
  them through `nonlocal`, and a six-way `elif` chain deciding what each source
  line is — every nested construct scoring against the same budget. The state
  is a `Body` class now (`out`, `pending`, `pending_is_bullet`, `table`) whose
  methods are the operations the chain was performing inline — `flush_table`,
  `flush`, `emit`, `open_bullet`, `add_row` — the per-line dispatch is a free
  `feed`, and the table branch, the one arm with a body of its own, is
  `feed_table_line`. What is left in `render` is what its doc comment says: the
  header, a line-by-line feed, a final flush. Dropped on the way: `line =
  raw.rstrip()`, dead since the next statement stripped it again.
- **The regex fix is the same language**: the separator-row matcher
  `:?-{1,}:?` → `:?-+:?`.
- **The Rust finding is a test's tar fixture** — `header.set_mode(0o755)` on
  the `bin/wasmer` entry `extract_targz_roundtrip` unpacks. The mode is never
  read back (the test asserts the file exists), so the world-executable bit
  bought nothing and is now `0o750`. **Rejected: accepting the issue in the
  platform.** An Accept is per-issue and does not survive the line moving —
  which is the same objection the `rust:S2208` block in
  `sonar-project.properties` records for real — and this one has a
  one-character answer.
- **Behaviour is unchanged, and measured rather than argued**:
  `python tools/wizard_rtf.py --check` reports all five committed RTFs still
  matching their sources, so the rewritten renderer is byte-identical on every
  document it exists to produce. The snippet analyzer, run on the new section
  before the PR (lessons §10 — it takes Python, and no Rust), reports nothing.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2746 green, 129 `#[ignore]`, counts unchanged**. **No live run required**
  (AGENTS.md §3) — a Python tool and one test fixture, no engine, memory or
  tool surface touched. No CHANGELOG entry: nothing the user sees changed (§4).

### Post-M9: SonarQube follow-up — the budget's poison guard and the pool probe (done)

- **Two findings open on `main`**, both raised by the 2026-09-04
  admission-by-budget merges (PRs #446–#448) and neither visible to any local
  gate: `rust:S1612` on `shared/session_budget.rs` (the closure that recovers a
  poisoned mutex) and `python:S3776` on `tools/kv_pool_probe.py`, `one` at
  cognitive complexity **36** against the 15 allowed. Branch
  `fix/sonar-session-budget-kv-probe`.
- **The Rust one is the lessons §10 family, exactly as described**: `S1612` is
  clippy's `redundant_closure_for_method_calls`, which is *pedantic* and so off
  — `cargo clippy --all-targets -- -D warnings` was green on this line the whole
  time. `.unwrap_or_else(|poisoned| poisoned.into_inner())` →
  `.unwrap_or_else(PoisonError::into_inner)`; the guard itself is unchanged, and
  so is what it is for (the sum is a plain `u64`, so a panic while it is held
  leaves nothing to protect — see the type's own note).
- **`one`'s complexity was two response readers inlined in one function**, not
  logic: the request shaping, the plain-JSON read and the SSE loop shared a
  budget, and the SSE loop's per-chunk merge nested three deep inside it. Split
  along the seams that were already there — `request_body` (the field a
  non-streaming request must not carry, plus the slot pin), `read_plain`,
  `read_stream`, and `merge_chunk` for what one `data:` chunk contributes — so
  `one` is now what its shape always claimed: time it, post it, read it
  whichever way, record the wall clock. The duplicated `timings` narrowing
  became one `timings()` over a named `TIMING_KEYS`, which is also the only
  place the probe says *which* three fields it reads.
- **Equivalence measured, not argued** (offline, no server): both versions of
  the module imported side by side with a stubbed `requests`, and `one()` run
  over 14 fabricated answers — plain ok / overflow 500 / empty document / null
  message / undecodable body, streaming ok / in-band error envelope / non-`data:`
  lines / no `[DONE]` / no lines at all, and a transport failure in each shape,
  with and without a pinned slot. **14/14 identical**, comparing the record
  *and* the outgoing request body (`wall_s` dropped as a clock reading). The
  cases that matter to the research are among them: `error_after_chunks` still
  counts the chunks that preceded the envelope, `other_lines` still keeps what
  is not a `data:` line, and a chunk after `[DONE]` is still ignored.
- **The lessons §10 pre-check was run this time** — the reshaped file through
  the Sonar MCP snippet analyzer before the PR: **no issues**, against a control
  snippet the same call scored at complexity 38, so the rule was demonstrably
  live rather than merely silent. That is the whole of the check's value and it
  costs one tool call.
- **Observed and left alone**: the `park` arm's `pair:` line prints `[null]` —
  `[one("A", False, [])]` echoes the function's return, not the record it
  appends to a throwaway list. It cost the research nothing (the parked-set
  measurement is the `parent first` / `parent again` pair, and A's request
  really does run and occupy the slot), and fixing it is a behaviour change,
  which AGENTS.md §2 keeps out of a mechanical refactor's PR.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2798 green, 136 `#[ignore]`, counts unchanged**; the five repository gates
  green. **No live run required** (AGENTS.md §3): a research probe and one Rust
  expression, no engine, memory or tool surface touched. No CHANGELOG entry —
  nothing the user sees changed (§4).

### Post-M9: the pool probe's `park` arm records its sibling (done)

- **The behaviour change held out of the Sonar PR** (the entry above; AGENTS.md
  §2 keeps a mechanical refactor and a behaviour change apart), now made on its
  own. `tools/kv_pool_probe.py`'s `park` arm printed `pair: [null]`:
  `[one("A", False, [])]` wraps the function's **return** — and `one` records
  into the list it is given and returns `None` — so the sibling's own record
  went into a throwaway list and was never seen. Branch
  `fix/kv-probe-park-record`. The arm builds its list the way `run_pair` does:
  `pair = []; one("A", False, pair)`.
- **What it cost, stated so the research is not re-read as suspect**: nothing.
  The parked-set measurement `admission-by-budget` §3 and the tools journal
  quote (`cache_n` 1239 of 1244) is the `parent first` / `parent again` pair,
  both of which printed correctly; and A's request really did run and occupy
  the slot — the echo was missing, not the traffic. What was lost is A's own
  status/finish/timings, which is exactly what one wants when the *sibling*
  behaves unexpectedly rather than the parent.
- **Measured, in the terms that matter for a probe**: the whole of `main()` run
  under a stubbed `requests` on the pre- and post-fix module, for all four arms,
  comparing the outgoing requests and stdout separately. **Traffic identical in
  every arm** (`park` 3 requests, the other three 4) — so each arm still
  measures precisely what it did — and stdout byte-identical except the one
  `park` line, `[null]` → A's full record. `wall_s` scrubbed by pattern as a
  clock reading; the first scrub matched two literal values and left the rest,
  which is what made `parkpar` look changed until a control (the same pre-fix
  module against itself, 8 runs) showed its record order stable and the
  difference to be the timings alone.
- The reshaped `main` through the Sonar MCP snippet analyzer (lessons §10):
  **no issues** — the `if`/`else` replacing the conditional expression costs
  nothing against the 15.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2798 green, 136 `#[ignore]`, counts unchanged**; the repository gates green.
  **No live run required** (AGENTS.md §3) — an offline research probe, no
  product code. No CHANGELOG entry: nothing the user sees changed (§4).

### Post-M9: SonarQube follow-up — the round resolver's three arms (done)

- **One finding open on `main`**: `rust:S3776` on
  `src/app/orchestrator/generation.rs`, `resolve_round` at cognitive
  complexity **18** against the 15 allowed. Its creation timestamp
  (2026-09-05T21:27:32Z) is commit `e215cba` to the minute — the background
  dialogues merge (PR #457), which added the `if`/`else` picking
  `start_background_dialogue` over `start_background` *inside* the background
  arm, three levels down. Branch `refactor/sonar-resolve-round`. The lessons
  §10 family again: `cargo clippy --all-targets -- -D warnings` was green on
  this function the whole time and no local gate could have seen it.
- **The complexity was three arms sharing one loop body, not logic.** The
  `while` walked the round's calls and each kind was inlined at its `continue`,
  so everything nested inside paid the loop's depth: the group arm's `match`
  over `child_spec`, the background arm's twin-picking `if`/`else`, and the
  segment arm's `for` over the results. One method per arm —
  `queue_group_call`, `resolve_background_call`, `resolve_ordinary` — leaves
  the loop answering the two questions it is actually about: which of the three
  kinds the call at `i` is, and where the next one starts. This is the shape
  lessons §10 already prescribes for a dispatch whose arms carry preconditions
  (written down when a nineteen-arm `match` scored 16 on five short `if`s);
  it was applied here to a three-arm one.
- **Two small unifications came with the seams, and nothing else moved.** The
  ordinary arm's `i += 1` and `i = end` collapse into a single `i = end`
  because `segment_end` never answers less than `start + 1` — the one-call case
  *is* `end == i + 1`, which is also why `resolve_ordinary` can decide by
  `span.len()` rather than re-derive the comparison. And `Self::call_args(call)`
  moves out of the background arm's two branches into one binding above them: a
  pure parse of the call's argument string, previously done twice in source and
  once at run time either way.
- **Covered by the tests that already own these paths** — `concurrent::` (7),
  `parallel::` (8), `background::` (13), `background_dialogue::` (6) — 34 over
  the three arms, including the four that pin exactly what a reshaped dispatch
  could disturb: `a_segment_runs_its_reads_at_once_and_records_them_in_order`,
  `an_unmarked_call_breaks_the_segment`,
  `a_disabled_tool_is_refused_and_breaks_the_segment` and
  `esc_mid_segment_cancels_every_member`. No new test: the refactor adds no
  behaviour to pin, and a test over the private helpers would pin the shape
  this entry expects to be free to change again.
- **The lessons §10 pre-check is still unavailable for Rust**, confirmed rather
  than assumed this time: the Sonar MCP snippet analyzer's `language` parameter
  enumerates twenty languages and Rust is not among them, so the call cannot be
  made at all. The shape was therefore watched by eye — by hand-count the loop
  is 5 against the 15, and the three helpers 1, 2 and 2 — and the PR's own
  analysis is the first real measurement, as it has been for every Rust finding
  here.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **2866 green, 138 `#[ignore]`, counts unchanged**; the six repository gates
  green. **No live run required** (AGENTS.md §3): a mechanical refactor of a
  dispatch, no engine, memory or tool surface touched. No CHANGELOG entry —
  nothing the user sees changed (§4).

### Post-M9: SonarQube follow-up — blocking file calls in the two setup paths (done)

- **Twenty findings open on `main`, and its gate red.** `rust:S7493` ("async
  functions should not contain synchronous file operations") on nineteen
  `std::fs` calls — fourteen in `features/llama_setup.rs` (`setup`,
  `fetch_verified`, `stream_to_part`), five in `features/sandbox_setup.rs`
  (`setup`, `ensure_wasmer`, `ensure_python_webc`, `ensure_wheels`) — and one
  `rust:S2629` on `features/tools/web.rs`, a `context(loc.t(..).to_string())`
  where the rule wants the closure form. Branch `fix/sonar-async-fs`.
- **A new rule family, backdated** — the lessons case "`cargo clippy -D
  warnings` green is not Sonar-clean" again: every finding's creation date is
  its line's blame date (2026-07-12/14 for `sandbox_setup`, 2026-08-06 for
  `web.rs`, 2026-09-10 for the llama.cpp downloader merge), and the previous
  follow-up on that same file (c06d26e, 2026-09-10) cleared three findings with
  all fourteen lines already in place and saw none of these. The rules came with
  an analyzer update, not with a change. What is new is the class: `S7493` is a
  **bug** of HIGH reliability impact, and the fourteen inside the new-code
  period dropped `main`'s new-code reliability rating to **C** against the
  gate's A — a smell backlog sits in the gate, a bug backlog fails it.
- **The swap, one call at a time**: `tokio::fs::{create_dir_all,
  remove_dir_all, remove_file, rename, metadata}(..).await` in place of
  `std::fs`, the error contexts and the fire-and-forget `let _ =` shape kept as
  they were. `tokio::fs` runs the same syscall on the blocking pool, so what
  changes is which thread waits, not what happens on disk; the `tokio::fs::File`
  writes both modules already had were left alone (they `flush`, the rule's own
  pitfall). `web.rs`: `Err(err).with_context(|| ..)` — the closure form the
  file's other sites use; `err` being an error already, the formatting was never
  wasted there, but the closure is what the rule matches and it reads no worse.
- **Observed and left alone — what the rule does not see.** `Path::is_file()` /
  `is_dir()` / `exists()` are `stat` calls and block just the same, and the heavy
  work of both paths — `extract` / `merge_payload` unpacking a 17 MB zip,
  `extract_targz`, `unpack_wheel` — is done by sync helpers the async fn calls,
  which the rule does not descend into. Moving that into `spawn_blocking` is the
  change that would matter to a runtime, and it would matter on one path only —
  the python tool's auto-provisioning inside the app (`tools/python.rs`); the
  CLI's runtime is its own and idle otherwise. A behaviour change, kept out of a
  findings fix (AGENTS.md §2) and left to a follow-up if it is ever wanted.
- **Live — GO** (AGENTS.md §3; the tool surface is touched):
  `live_install_cpu_into_a_tempdir` — b10909 cpu, 17 MB, unpacked and proved
  (`build 10909` against the tag) in 2.5 s, staging removed;
  `live_the_registry_still_serves_the_pinned_python_build` — `python.webc`
  downloaded into a tempdir and verified against its pinned digest, 4.1 s; and
  `mindfork sandbox setup` on the working install — the present path through
  `setup`, `ensure_wheels` and the warm-up, done. Not exercised:
  `ensure_wasmer`'s two removals (a fresh wasmer install only) and the
  downloader's resume-and-retry branch.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **3072 green, 159 `#[ignore]`, counts unchanged**; the six repository gates
  green. No CHANGELOG entry — nothing the user sees changed (§4).
### Post-M9: SonarQube follow-up — the file launcher's two closures (done)

- **Two open findings on `main`, the gate green** (read through the Sonar MCP
  server, 2026-09-12; every condition OK, new duplication 0.3 % against the 3 %
  threshold, no hotspots). Both are `rust:S1612` on `shared/os_open.rs` —
  `.extension().and_then(|e| e.to_str())` in `is_document` and
  `.file_name().and_then(|n| n.to_str())` in `decide` — and both carry the blame
  date of `82b90ff` (`/file open`, 2026-09-12 00:18 UTC), so this is not the
  backdated-analyzer genre of the previous rounds but a fresh merge: PR #523
  passed its own gate with them in place, as it always does — the gate *rates*
  new code and two MINOR maintainability smells do not move an A. Branch
  `fix/sonar-os-open-closures`.
- **The fix is the rule's own**: `and_then(OsStr::to_str)` with
  `std::ffi::OsStr` added to the module's imports (the file had only the
  `OsStrExt` trait, inside the Windows launch). `OsStr::to_str` is
  `fn(&OsStr) -> Option<&str>`, exactly what `and_then` wants here, so nothing
  around it changed; `cargo fmt` then folded `decide`'s four-line chain back
  onto one line, which together with the import is the whole diff.
- **The lessons §10 family again — and this time the *local* lint is the wider
  of the two.** `S1612` is clippy's `redundant_closure_for_method_calls`, which
  is pedantic and therefore outside the project's warn set, so
  `cargo clippy --all-targets -- -D warnings` was green on both sites before the
  fix and after it — the expected half. The unexpected half is the other
  direction: turn that lint on explicitly over today's tree and it names **55**
  further sites (`map(|s| s.to_string())`, `map(|e| e.len())`,
  `is_some_and(|c| c.is_cancelled())`, `map(|s| s.as_str())` …), **none** of
  which Sonar reports — not accepted, not resolved, simply never raised (the
  project's whole accepted list is five issues, all `S3776`/`S5332`). The two
  rule sets overlap, they are not the same set, so "clear the findings" and
  "clear the lint" are different tasks of very different sizes; the 55 were left
  alone and no `-W` was added to the build. Worth remembering when the next
  `S1612` round arrives: the sites it names are *a subset* of what a local
  pedantic pass would, and fixing the subset is what the gate asks for.
- **No live run** (AGENTS.md §3): the change is a closure spelled as a method
  reference in two pure functions; `os_open`'s own behaviour — which types open
  directly, which fall back to the folder — is covered by the module's unit
  tests, and the launch itself is the stage's manual gate, unchanged here.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` —
  **3171 green, 169 `#[ignore]`, counts unchanged**; the repository gates green.
  No CHANGELOG entry — nothing the user sees changed (§4).

### Post-M9: the quality-gate badge, and the one Dependabot badge that is not a claim (done)

- **The badge item was never about the badge.** It has sat in the groundwork
  since the analysis was set up (2026-08-05) and again in
  [roadmap.md](../roadmap.md) with the same parenthesis each time: a **private**
  SonarQube Cloud project's badge renders for nobody who does not hold a token,
  so adding it would have put a broken image in the README masthead. The owner
  made the Sonar project public on 2026-09-19 and the item unblocked itself;
  asked for directly, together with a Dependabot badge.
- **The Sonar half is the documented endpoint**, with the project key read from
  `sonar-project.properties` rather than retyped —
  `.../api/project_badges/measure?project=vshylov_mindfork-rs&metric=alert_status`,
  linked to the project's New Code summary. `alert_status` and not `coverage`
  deliberately: the gate is the **custom** "Sonar way without new-code coverage"
  ("the quality gate stopped judging new-code coverage", above), so a coverage badge would advertise, in the masthead, the
  one number the gate stopped judging — and the percentage is already read as a
  verdict rather than a measurement, which is the misreading that entry exists to
  prevent. The README's own "Project status" section carries the test counts.
- **Dependabot has no status badge of its own, and this is the part worth
  recording.** `api.dependabot.com/badges/status` belonged to *Dependabot
  Preview*, the pre-acquisition service GitHub retired in 2021; shields.io has no
  endpoint for the GitHub-native Dependabot; and the alert state — the thing a
  reader would actually want a badge for — is repository-private data that no
  public badge can expose. What does exist: since GitHub runs the update job as
  an ordinary Actions workflow, `GET /repos/{owner}/{repo}/actions/workflows`
  lists **Dependabot Updates** as `active` alongside the eight real ones, with
  `path: dynamic/dependabot/dependabot-updates` and a `badge_url` GitHub hands
  out itself. That answer came from the API rather than from a blog post, which
  is why it is in the README and a guessed URL would not have been.
- **What that badge claims, stated so it is not read as more.** It reports the
  outcome of the last Dependabot *update run* — whether Dependabot managed to
  look — not "dependencies are current" and not "no open advisories". The
  advisory half is `audit.yml`'s and stays where it is.
- **Rejected: `img.shields.io/badge/dependabot-enabled`.** It renders, it is what
  most repositories use, and it measures nothing — a hand-written claim in the
  masthead that keeps rendering green after the configuration it describes is
  deleted. The lesson is already written down here for the licence badge
  ([lessons.md](../lessons.md)): a badge whose subject drifts goes on rendering
  the old claim, because nothing is measuring it.
- **Verification, and its limit.** The repository is public
  (`visibility: public`) and the Dependabot workflow is `active` — both read from
  the GitHub API in-session; the Sonar project key matches
  `sonar-project.properties`; `link_check` passes (it skips `http(s)` by design,
  so no badge URL is checked by it either way) and `doc_index_check`
  passes with the index at 26. **Not** verified in-session: that the two images
  render — and the two reasons are different, which is worth separating because
  the first reading of it was wrong. `sonarcloud.io` and `img.shields.io` are
  refused by the container's **egress proxy**, which answers `403` to the
  `CONNECT` itself (`connect_rejected`, organization policy), so those hosts are
  simply unreachable. `github.com`'s badge asset also answers `403`, but not for
  that reason: it is reached, and the session's **GitHub gateway** rejects the
  path — *"sessions are bound to their configured repositories. Use
  repository-scoped endpoints"* — because a `/<owner>/<repo>/actions/...` web URL
  is not one. `api.github.com/rate_limit` answers `200` from the same container,
  which is the control that tells the two apart. Net effect is the same (no badge
  URL in this README could be fetched, the five older ones included) but "the
  proxy blocks GitHub" would have been a false thing to leave written down.
  First render on GitHub is the check.
- **The README's test count was three behind, and finding out cost a measurement
  worth keeping.** `CLAUDE.md` said 3326 / 196 and the README 3323 / 196 — the
  README simply never got the 0.10.1-era bump (`docs/research/public-documents.md`
  had already flagged the same line once, at 3217). The obvious repair is to copy
  the larger number across, and it would have been wrong to do it on that
  reasoning alone: `cargo test` **in this Linux container reports 3324 passed,
  189 ignored** on the same commit, matching neither document. The suite does not
  compile to the same size on the two targets — `src/shared/keys.rs` alone holds
  three `cfg(windows)` tests, and some `#[ignore]` smokes (the real-clipboard
  round trip, the screenshot regenerator) are Windows-only — so 3326 / 196 is the
  author's Windows figure and 3324 / 189 is Linux's. The README is aligned to the
  documented Windows figure, which is what `CLAUDE.md` and every recent journal
  entry carry; the trap itself is now a rule in [lessons.md](../lessons.md) §6,
  because an agent measuring on Linux would "fix" `CLAUDE.md` into a number that
  is wrong on both platforms. (ALSA had to be installed before the suite would
  build here at all — `libasound2-dev`, exactly as the `test` job does it.)
- **No CHANGELOG entry and no live run** — a README masthead and three documents;
  nothing in `src/`. Test totals unchanged by this branch.

### Post-M9: SonarQube follow-up — the IndexNow tool's three findings (done)

- **The new `tools/indexnow.py` arrived with three findings, and exactly one of
  them reddened the gate** (PR #597, branch `feat/site-indexnow`; the tool
  itself is the website journal's IndexNow entry). The SonarCloud bot named a
  single failed condition — **B Security Rating on New Code, required ≥ A** —
  and the only security-class finding in the new code was `python:S5332`. The
  two maintainability ones could not have flipped it, exactly as the
  screenshots-writer entry above measured; they were fixed anyway, because they
  were real.
- **Reading which condition failed cost more than the fixes did, through an own
  goal worth recording.** The MCP's PR calls do not agree on a parameter name:
  `get_project_quality_gate_status` takes `pullRequest`,
  `search_sonar_issues_in_projects` takes `pullRequestId` — and the gate call
  handed the wrong one **does not error, it silently answers about `main`**.
  That returned status OK for a PR whose gate was red, and the only tell was
  that the numbers were byte-identical to the branch's (85.9 coverage, 0.3
  duplication), which is what first read as "the MCP ignores the PR filter".
  With `pullRequest` it is exact: `new_security_rating` **2** against a
  threshold of 1, every other condition OK. `get_component_measures` is
  separately unusable — it forwards neither `component` nor `componentKey` and
  answers 400 either way. The **SonarCloud bot's comment on the pull request**
  names the failed condition in one line and cost nothing; it is the right
  first stop, and the cross-check that would have caught the silent fallback.
- **`python:S5332` is a false positive of a shape this project has now seen
  twice.** The flagged line is `SITEMAP_NS = {"sm":
  "http://www.sitemaps.org/schemas/sitemap/0.9"}` — an XML namespace URI, an
  identifier fixed by the sitemap 0.9 specification that has to match the
  document byte for byte. Rewritten to `https` it stops matching,
  `findall(".//sm:loc", SITEMAP_NS)` returns nothing, and a deploy submits an
  empty URL list while reporting success. Nothing is fetched over it. The 2026-08-06
  triage met the same rule on `link_check.py:49` (the `SKIP_SCHEME` tuple used
  to *skip* external links) and marked it False Positive in the UI.
- **Recorded in `sonar-project.properties` this time, not in the UI** (user's
  decision, 2026-09-19, offered against marking it False Positive by hand). The
  reason is the one the file's `rust:S2208` note already measured: a UI Accept
  has to be re-applied by hand every time the line moves. The entry is
  `sitemap_ns`, scoped to `python:S5332` × `tools/indexnow.py`, so every other
  rule stays live in that file.
- **The suppression is per file, so what it was guarding is now asserted by
  hand.** `resourceKey` has no line granularity, and the one clear-text URL that
  *would* matter in this file is `ENDPOINT`, the single address the tool posts
  to. `_check_endpoint` in `--self-test` refuses a non-https endpoint, and
  `--self-test` runs in the `lint` job on every pull request. Measured rather
  than asserted: flipping `ENDPOINT` to `http://` in a copy makes the self-test
  exit 1 with `the endpoint must be https`.
- **`python:S3776` — complexity 18 against 15, and the reason it read as 18.**
  Nothing in `self_test` looked tangled; the score came from its two helpers
  being `def`s *inside* the body, whose branches Sonar folds into the enclosing
  function. `_fixture` and `_expect_refusal` are module-level now (plus
  `_expect_key`, for the decoy case that asserts rather than expects a refusal),
  and the body split along the seam its own comments had already drawn —
  `_check_refusals` for the fixture scenarios, `_check_classify` for the
  response policy, `_check_endpoint` for the above, with `self_test` left
  holding the failure list and the report. Every function lands near 3. **No
  scenario gained, lost or changed**: the same eight fixtures and the same eight
  status codes, `--self-test` still 0 failures.
- **`python:S1192` — `"https://mindfork.io/"` spelled seven times** across those
  fixtures, now `FAKE_SITE`/`FAKE_HOME`/`FAKE_INSTALL`. That is also the honest
  shape: every fixture has to agree with what `base_url` reads back for a
  payload to build at all, so the spelling was never free to vary.
- No Rust touched — test totals unchanged; the tool's own gates (`--check`,
  `--self-test`) and the three documentation gates green, and `--submit
  --dry-run` builds the same body as before. No CHANGELOG entry: internal
  tooling (§4).
