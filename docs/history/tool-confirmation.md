# Confirmation for dangerous tools — design plan

Human-in-the-loop before a tool call that can change something outside the app:
running arbitrary Python, writing a file, calling a third-party MCP server. From
[roadmap.md](../roadmap.md) §Tools ("Confirmation for dangerous tools").

Status: **complete** (one PR). Forks F1–F8 accepted by the user as recommended,
2026-07-30, with one
addition: the whole thing must be **fully switchable off in settings** — when
`tools.confirm_dangerous` is off nothing asks, nothing is gated, and the loop
behaves exactly as it does today (F2, and the reason `danger()` is consulted only
behind that switch).

---

## 1. What is missing today

Once a tool is enabled in the profile and its global switch is on, the agentic
loop calls it **unattended**. `python_exec` runs whatever the model wrote,
`fs_write` overwrites whatever path it chose, and an MCP server's tool does
whatever that server does. The user finds out afterwards, from the tool card in
the feed.

Three things already limit the blast radius, and they are why this is a *second*
layer rather than the first:

- `tools.python_enabled`, `tools.fs_enabled` and `mcp.enabled` are all **off by
  default** — a user with these on has already made a deliberate choice;
- `python_exec` runs in the WASIX sandbox by default (ADR 0005), and `fs_*` can
  be confined to `tools.fs_root`;
- MCP catalogs are TOFU-pinned (ADR 0007), so a server cannot silently change
  what its tools claim to be.

What none of them give is a **look before the specific call**: this file, this
code, this argument, right now.

## 2. What the code already provides

| Piece | Where | Note |
|---|---|---|
| The one invocation site | `orchestrator/generation.rs`, the `else` branch around `registry.invoke` | Already wrapped in a `select!` with the turn's `CancellationToken`, and already has three "do not run it" branches (disabled / control tool / discarded rewrite round) whose results are localized text |
| Per-tool metadata on the trait | `features/tools/mod.rs` — `group`/`ui_label`/`gate`/`enabled_by_default` | The single-source-of-truth decision: a tool declares its own metadata, and `ToolRegistry::infos()` snapshots it |
| A confirmation popup | `screens/chat/mod.rs` `ConfirmAction` + `popups.rs::render_confirm` | Already wired to `interface.confirm_destructive_keys` for `Ctrl+R`/`Ctrl+E`; `Enter` = yes, `Esc` = no, other keys ignored |
| Formatting a call for display | `features/tools/present.rs` | Turns `(name, arguments, result)` into blocks — what the feed already shows for the same call |

## 3. The one architectural problem

The agentic loop is **a background task** (`spawn_generation`). It talks to the
UI in one direction only: it sends `AppEvent`s. Asking a question needs the other
direction — task → event → UI → command → **back into the running task**.

Nothing in the codebase does that yet. The existing internal channels
(`title_tx`, `imp_done`, the background-task done channel) all run task →
orchestrator. This is the piece that has to be designed rather than assembled,
and it is fork **F8**.

Everything else is small: a gate in front of one `else` branch, a trait method
with a default, a popup variant, a setting, and their tests.

---

## 4. Forks — **decided by the user 2026-07-30**

### F1 — how a tool is marked dangerous

- **(a) A `Tool::danger()` method with a safe default** — each tool declares its
  own, exactly as it already declares `group`/`ui_label`/`gate`. *Recommended*:
  it is the decision the project already took for tool metadata, and it makes a
  new tool's author state the answer instead of a central table quietly omitting
  it.
- (b) A central list keyed by tool id, next to the other catalog data.

Under (a), the proposed initial marking:

| Dangerous | Not |
|---|---|
| `python_exec` (arbitrary code), `fs_write` (overwrites any path in the sandbox), every MCP tool (third-party, see F3) | `fs_read`/`fs_list` (read-only), `web_search`/`fetch_url` (network reads), notes/self-model/RAG/attachments (our own DB, visible in the UI, reversible), `calculate`/`current_time` (pure) |

### F2 — on or off by default

- (a) **On** — safest, but every `python_exec` in an agentic loop stops for a
  keypress, including for users who deliberately enabled the sandbox to let the
  model work.
- **(b) Off, an opt-in setting** (`tools.confirm_dangerous`). *Recommended*:
  mirrors `interface.confirm_destructive_keys`, which is off; the dangerous tools
  are already behind master switches that are off by default, so the user who
  turned them on has consented once already. This layer is for those who want to
  consent *per call*.

### F3 — MCP tools

The server-supplied `destructiveHint`/`readOnlyHint` annotations are **untrusted
input** — a malicious or careless server can claim anything, so they may never
*relax* a decision. (Our client does not parse them at all today.)

- **(a) Treat every MCP tool as dangerous**, ignore annotations. *Recommended*:
  simple, safe, and the noise is bounded by F2 (opt-in) and F4 ("for this turn").
- (b) Parse annotations and use them only to *escalate* — which, given (a)
  already escalates everything, buys nothing until some MCP tools are considered
  safe.
- (c) Leave MCP out of this PR.

### F4 — granularity of approval

- (a) **Every call asks.** Honest, but an agentic loop calling `python_exec` five
  times asks five times.
- **(b) Per call, plus "allow this tool for the rest of this turn".**
  *Recommended*: the turn is the natural unit — it is the scope of one user
  request, it ends by itself, and it is what `Esc` already cancels. No persistent
  state to get wrong.
- (c) Also "for this chat" / "always" — persistent allowances, which is a
  standing permission the user then has to remember they granted.

### F5 — what the model sees when the user declines

A localized text result ("the user declined this call"), like the existing
`loop.tool_disabled` / `loop.tool_cancelled` — the loop **continues**, so the
model can explain itself or try another way. *No real alternative*: an error
would end the turn and lose the answer already streamed. Recorded here because it
is a visible behaviour, not because it is contested.

### F6 — waiting for the answer

- **(a) No timeout.** *Recommended*: `Esc` already cancels the turn, `Quit`
  cancels it too, and a silent auto-decline after N seconds would be a surprising
  way to lose work. The cost is that a generation task can sit waiting until the
  user comes back.
- (b) Auto-decline after a timeout.

### F7 — what the popup shows

- **(a) Tool name + arguments, rendered through `present.rs`** — the same
  formatting the feed will show for that call afterwards, truncated to a budget
  with an explicit "…". *Recommended*: consistency, and it makes `python_exec`'s
  code readable rather than a JSON blob.
- (b) Name + raw argument JSON.
- (c) A scrollable popup showing everything.

### F8 — how the answer reaches the running task **(the architectural one)**

- **(a) A confirmation channel into the turn.** `GenSpawn` carries the receiving
  end; the orchestrator keeps the sender for the in-flight turn and routes
  `AppCommand::ConfirmTool { generation_id, .. }` into it, dropping replies whose
  `generation_id` is stale (the same guard `AppEvent::TokenUsage` already uses).
  The wait is a `select!` against the turn's `CancellationToken`, so `Esc` works
  while the popup is open. *Recommended*: smallest change that keeps the loop
  where it is.
- (b) Move tool execution into the orchestrator, which owns the UI conversation.
  A large restructure of the agentic loop for no other gain.

---

## 5. Proposed staging

One PR. The pieces are interlocking (a gate with no popup is untestable end to
end), and each is small.

1. `Tool::danger()` + the initial marking (F1) + `ToolInfo` carrying it.
2. The confirmation channel (F8) and the gate in `generation.rs`, with the
   decline result (F5) and "for this turn" (F4).
3. The popup variant (F7), the setting (F2), i18n for both.
4. Tests: pure (which tools are dangerous; the turn-scoped allowance), screen
   (the popup opens during generation, `Enter`/`Esc`), orchestrator integration
   through the real `run` loop (a confirmed call executes, a declined one returns
   the decline text and the loop continues, a stale reply is ignored, `Esc` while
   waiting cancels the turn).
5. A live `#[ignore]` smoke on the local stack: the model calls `python_exec`,
   the confirmation is requested, and both answers behave.

## 6. Out of scope

Persistent allowances (F4c); a per-tool confirmation policy in the profile
(this is one global switch); confirming *reads*; an audit log of approvals;
using MCP annotations to mark a tool safe (F3 — untrusted by construction).
