# Documentation

Two audiences, and they want different documents.

## Using the app

| Document | What it answers |
|---|---|
| **[install.md](install.md)** | installing on Windows and Linux, where the data lives, connecting an engine (managed `llama-server`, an external server, the four clouds), the Python sandbox, MCP servers, speech, backups, the environment variables |
| **[manual.md](manual.md)** | how to use it: the screens, chats and profiles, what it remembers, files and images, your code project, the tools and their switches, the keys and the commands, and what to do when something goes wrong |
| **[import-format.md](import-format.md)** | the neutral JSON format for bringing conversations in from another application |
| **[../CHANGELOG.md](../CHANGELOG.md)** | what each release changed, in user language |
| **[roadmap.md](roadmap.md)** | what may come next — and what was considered and deliberately left out, with the reason |
| **[legal/](legal/)** | the licence, disclaimer and privacy policy in Russian (unofficial translations; the English originals at the repository root are the texts with legal force) |

The application also carries its own reference: **`F1`** from any screen lists
every key *by screen*, every command, the licence, the legal texts and the
third-party components.

## Working on the code

| Document | What it answers |
|---|---|
| **[../AGENTS.md](../AGENTS.md)** | the workflow every change follows: a design doc with its forks, a branch, tests beside the code, a live run, the documentation table, a pull request |
| **[../CONTRIBUTING.md](../CONTRIBUTING.md)** | the human-sized version of that, plus the gates and how to build and test without a GPU |
| **[../CLAUDE.md](../CLAUDE.md)** | orientation: which document answers which question. Written for an agent, useful to anyone new |
| **[../spec.md](../spec.md)** | the behaviour, chapter by chapter — "what" and "why". The source of truth |
| **[architecture.md](architecture.md)** | the code map: layers, modules, flows, lifecycles, invariants |
| **[lessons.md](lessons.md)** | the traps this project has already hit, written down so nobody hits them twice |
| **[decisions/](decisions/)** | the ADRs — the architectural decisions that were adopted, and what they cost |
| **[journal/](journal/)** | the engineering log, split by subsystem: what was done, why, what was measured, what was rejected |
| **[research/](research/)** | design documents written *before* the code, with their forks and measurements |
| **[history/](history/)** | the original request and the plans of finished tracks |

`spec.md` and `architecture.md` are large chaptered references with stable
numbering — the code cites them by section, and so should you. Read the section,
not the file.
