<!-- Process and readiness criteria — AGENTS.md at the repo root. -->

## What was done

<!-- The gist of the change, 2-6 bullets. Link the design doc / research / ADR, if any. -->

## Tests

<!-- Bottom line: N unit tests green, M #[ignore]. Live smokes: which ones, against what
     setup (model/server/key), outcome. If a live run isn't needed (pure UI/refactor) —
     say so explicitly. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`

## Documentation

<!-- Check off what was updated; delete inapplicable items. "What changed → what to
     update" table — AGENTS.md §4. -->

- [ ] CLAUDE.md — a changelog entry + the test count in "Status"
- [ ] CHANGELOG.md — an entry in `[Unreleased]` (if there's a user-visible effect / a data format change)
- [ ] docs/architecture.md — the affected §§
- [ ] spec.md — the affected §§
- [ ] README.md — user-facing functionality
- [ ] docs/install.md — install/run/env
- [ ] docs/decisions/ — an ADR (an accepted architectural decision)
- [ ] docs/roadmap.md — groundwork

## Models

<!-- All models that wrote code/tests/docs for this PR, with their role. For example:
- Claude Fable 5 — implementation and tests
- Claude Opus 4.8 — design doc, review
-->
