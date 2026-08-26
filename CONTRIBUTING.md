# Contributing to mindfork-rs

Thanks for your interest in the project! Issues and pull requests are welcome —
this page tells you where things live and what a change is expected to look
like. It is deliberately short: the project's real process documentation is
[AGENTS.md](AGENTS.md), and everything below routes into it.

## Bugs, questions, ideas

Open a GitHub issue. For a bug in the TUI, the environment usually matters as
much as the steps, so please include:

- the app version (`F1` → the About tab, or `mindfork --version`);
- your OS **and terminal emulator** (Windows Terminal, conhost, GNOME
  Terminal, kitty, …) — rendering and keyboard behavior differ a lot between
  them;
- the engine mode (managed / external / which cloud provider) and, for local
  modes, the model you ran;
- steps to reproduce, and what you expected instead;
- the relevant part of the log — logs go to files under `logs/` next to the
  binary (the TUI owns stdout, so nothing useful is printed to the terminal).

Security problems are the one exception: please do **not** open a public
issue — see [SECURITY.md](SECURITY.md).

Feature ideas are welcome too; [docs/roadmap.md](docs/roadmap.md) shows what
is already on the list.

## Working on the code

Orientation first — it will save you time:

1. **[CLAUDE.md](CLAUDE.md)** — the entry point: what this project is and the
   map of which document answers which question. The project's documentation
   is large and chaptered on purpose; **read the section your task needs, not
   the whole file**.
2. **[AGENTS.md](AGENTS.md)** — the task workflow (design doc → branch → live
   run → docs → PR). It is written with AI agents in mind, because much of
   this codebase is built by them under review — but it is simply the
   project's workflow, and it applies to human contributors the same way.
3. **[docs/lessons.md](docs/lessons.md)** — traps this project has already
   hit, written down so you don't hit them twice. Read it before
   implementing, not after.

The short version of the workflow:

- **Branch off `main`** before the first commit: `feat/<slug>`, `fix/<slug>`,
  `refactor/<slug>`, `docs/<slug>` or `spike/<slug>` (short English
  kebab-case). One PR = one task; a mechanical refactor and a behavior change
  never share a PR.
- **A complex task starts with a design doc**, with its open questions
  confirmed before implementation — see AGENTS.md §1 for what counts as
  complex and where the doc goes. A simple fix needs no ceremony.
- **Tests are written alongside the code**, not at the end. Anything that
  needs a real model or server is an `#[ignore]` smoke test, and functionality
  touching the engine / memory / tools must actually be run against a live
  stack before the PR (AGENTS.md §3; commands in the
  [README](README.md#development)). No GPU and no local `llama-server`?
  `cd docker && docker compose up --build` brings up a CPU stack plus a
  browser JupyterLab terminal to drive the app from — see
  [docker/README.md](docker/README.md).
- **Documentation is part of the task**: AGENTS.md §4 has the "what changed →
  what to update" table (journal entry, CHANGELOG, spec/architecture sections,
  README key tables).

## Quality gates

All of these must pass before every commit — CI enforces the same set:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python tools/cyrillic_scan.py      # source language is English
python tools/link_check.py         # relative links in the docs resolve
python tools/doc_index_check.py    # documentation structure is intact
```

Two conventions the gates guard that are worth knowing up front:

- **The source language is English** — code, comments, commit messages and
  docs. User-facing text is localized instead (`locales/*.json`), and Russian
  is a fully supported locale: interface strings belong in the locale files,
  never hardcoded.
- The TUI needs a **real terminal**; in a headless environment the app
  appears to hang (that is the missing TTY, not a bug). Live verification
  happens in a real terminal.

## AI assistance and attribution

Much of this project is written by AI models working under human review, and
the history is honest about it: every commit whose code a model wrote carries
a trailer naming **the actual model** (for example
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`), and every PR has a
**"Models"** section listing which models did what (see
[the PR template](.github/pull_request_template.md)). The same rule applies to
your PRs: if an AI assistant wrote part of the change, say which one and what
it did; if no model wrote anything, say so in the Models section. Reviewers
read AI-written and human-written code with the same care — attribution is
about honest history, not a different quality bar.

## License

The project is under the [MIT License](LICENSE). By contributing, you agree
that your contribution is licensed under the same terms — the standard
"inbound = outbound" arrangement, no CLA.
