# ADR 0012 — Concurrent tool calls in a round: the segment and the per-tool mark

**Status:** accepted (2026-09-04). Design, measurements and the decided forks —
[docs/research/concurrent-tools.md](../research/concurrent-tools.md).
Implemented on `feat/concurrent-tools`:
[src/app/orchestrator/generation.rs](../../src/app/orchestrator/generation.rs)
(`TurnLoop::resolve_round`, `segment_end`, `run_segment`, `invoke_member`),
[src/features/tools/mod.rs](../../src/features/tools/mod.rs)
(`Tool::concurrent`, `ToolRegistry::is_concurrent`, `ToolContext::sessions`),
[src/features/tools/fetch.rs](../../src/features/tools/fetch.rs) (the
summary's permit). Related: [ADR 0004](0004-engine-contract-multi-provider.md)
(the client-side agentic loop whose contract this amends), [ADR 0010](0010-subagent-nested-turn.md)
(the round's parallel group of sub-agents, whose amendment named this track).

## Context

The agentic loop (spec §6.3) executed a round's tool calls one after another
in the model's order, with one exception since ADR 0010's amendment: the
round's `call_subagent` calls run as a group, after every ordinary call.
Every model on the live gate and every cloud emits several *ordinary* calls
in one reply when a task has several independent reads — measured 26/26
with the app's own `fs_read`/`fetch_url` descriptions and no prompting — and
each such call then waited for the previous one, seconds per page where the
call waits on a network. The sub-agent group's shape could not simply be
reused: a child observes none of the round's effects, so its group may run
after everything else; an ordinary read can observe a write of the same
round, so moving it across one — in either direction — changes its result.

## Decision

1. **A per-tool mark: `Tool::concurrent()`, default `false`.** `true` is a
   claim by the tool's author, stated in the trait's doc comment: the call
   has no effect a sibling call of the same round could observe, changes
   nothing outside the application, holds no exclusive resource, needs no
   cleanup if its future is dropped, and is never `danger()`. The catalog
   carries the bit; a registry test pins the marked set to the documented one
   and that no marked tool is dangerous. The first set: the file, project,
   attachment, chat, history and introspection readers, and `fetch_url`.
2. **The unit of parallelism is the segment** — a maximal run of
   *consecutive* marked calls in the model's order. Anything else (a writer,
   a disabled or unknown name, a control call, a sub-agent call) ends the
   segment and resolves at its own position exactly as before. A segment's
   members run as futures inside the generation task (`buffer_unordered`),
   every card opening first and closing as its result lands; the results,
   the tool messages, the records **and the effects** are written in the
   model's order afterwards. The result therefore equals a sequential
   round's by construction: no read is ever moved across a write. The
   sub-agent group is unchanged and still runs after the ordinary phase.
3. **The width is a field of each engine section, `concurrent_calls`,** with
   a default per kind of engine: **1 on managed and external** (the
   sequential round, bit for bit — the machine the user sits at, one context
   pool shared by every stream of the turn) and **4 on the four clouds**
   (someone else's fleet; the overlap out of the box). At 1 no segment is
   formed at all. The settings row sits beside `sessions` and its hint names
   the marked tools from the catalog, so it cannot advertise what the rule
   keeps sequential.
4. **A tool's own engine request obeys `sessions`.** The turn's semaphore is
   shared with its `ToolContext`; `fetch_url`'s page summary takes a permit
   around its stream and around nothing else, so three pages fetched at once
   on a one-session engine are summarised one after another. A background
   task's context carries no budget, as ADR 0010's F9 decided for the loops.
5. **Out, by name:** `web_search` (the keyless chain's throttling under
   concurrent requests is unmeasured), `note_recall` (it writes vectors
   inside a read), `youtube_watch`, `python_exec`, the command tools, every
   MCP tool (its `readOnlyHint` is untrusted input and may never relax a
   safety property), the loop-executed tools and the control pair. Nothing
   is told to the model: it emits the calls unprompted.

## Consequences

- A chat whose rounds never carry two consecutive marked calls, and every
  chat on a local engine at the default, is unchanged on the wire and on
  disk; the new field is `#[serde(default)]` with no schema step.
- Where a segment forms, only wall time changes: the stored message, the
  request history and each call's result are what a sequential round would
  have produced. Cards of a segment open together and may close out of
  order, which the feed already tolerated for the sub-agent group.
- The mark is a contract a tool author signs; a new tool runs alone until
  someone says otherwise. Marking a tool that writes, or that holds a
  resource, is the one way to break the invariant, and the registry test is
  the door that stays locked.
- `sessions` is now honest for every stream of the turn, not only the
  loops' own — a small behaviour change for a sub-agent whose `fetch_url`
  summary used to run beside a sibling's stream on a one-session engine.
