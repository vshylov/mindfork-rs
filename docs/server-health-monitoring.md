# Periodic server health monitoring

> Status: **forks confirmed 2026-07-28, implemented**. Follows on from
> [PR #224](https://github.com/vshylov/mindfork-rs/pull/224) (a readiness probe
> for the embedding server), which fixed the *startup* half of this problem and
> recorded the rest as roadmap groundwork.

## 1. What the user asked for

> Add periodic re-checking of the servers' statuses.

Variant 2 from the diagnosis of the green-embeddings-chip report: every server's
`/health` probe is currently **one-shot**, so the status reflects the moment the
server was configured — not the present.

## 2. What already exists

`apply_chat` / `apply_embed` / `apply_impersonation` each return an immediate
status and spawn `spawn_probe` → `wait_until_ready`, which polls `/health` until
it gets a verdict and then **posts once and exits**
([supervisor.rs](../src/app/supervisor.rs)). Each server has its own status
channel into the orchestrator's `run` loop and its own `CancellationToken` for
invalidating a stale probe ([engines.rs](../src/app/orchestrator/engines.rs)).
Managed servers additionally have an `exited` token, raised by the process
monitor when the child dies ([managed.rs](../src/shared/api/managed.rs)).

So the plumbing is all in place. What's missing is that nothing ever asks again.

## 3. The problem is bigger than a stale chip

The obvious symptom is a status that lies after the fact: a host that goes down
*after* connecting keeps a green chip until a restart or a settings change.

But the same one-shot probe has a second, sharper consequence that the report
didn't mention and that arguably matters more:

**A server that isn't up when the app starts stays unusable until the user
intervenes.** The initial probe fails → `Disconnected` → `backend_if_ready`
returns `Err` → send/regenerate/impersonate are refused. Nothing re-probes, so
starting the app before `llama-server` — the ordinary order of operations for a
local setup — leaves generation blocked even after the server finishes coming
up. The only ways out are restarting the app or touching an engine setting to
force a re-`apply`.

Periodic probing therefore delivers **recovery**, not just honest reporting, and
recovery is the half users actually feel. This shapes the cadence choice below
(§5, F2): the interesting case isn't "how fast do we notice a failure" but "how
fast do we notice a *fix*".

## 4. Measurements (taken, not assumed)

Two facts the design depends on, measured against the live stack
(external `llama-server`, Gemma 4 31B q4_0 on `:8000`, bge-m3 on `:8001`):

- **`/health` returns `200` during active generation** (5 probes, 1 s apart,
  while a 600-token completion streamed). So a periodic probe does not need to
  pause during generation. Caveat worth encoding anyway: older llama.cpp builds
  answered `503` when all slots were busy, and a third-party OpenAI-compatible
  server (vLLM/LM Studio/Ollama) may differ — which is an argument for
  hysteresis (F3) regardless of what this build does.
- **A connect to a *closed* local port costs ~2.0 s on Windows** (SYN retry;
  measured while building PR #224's tests). This is the dominant cost of a
  failing probe and it bounds how tight the cadence can sensibly be, and it means
  a probe against a down server is *slow*, not instant.
- **A managed child killed from outside is reported in ~150 ms** — from the exit
  signal, against a real `llama-server` (bge-m3) killed with `taskkill`. Compare
  the same process going away *without* the exit signal: **76 s**, i.e. one
  healthy poll plus the failure streak. That gap is why F4 watches `exited`
  rather than relying on polling.
- **Worst-case detection for a server that stops answering without dying**
  (a hung process, an unplugged host): `HEALTHY_POLL + 3 × RECHECK_POLL` ≈ **75 s**.
  A request issued in the meantime fails immediately with the real error, so this
  bounds how long the *chip* can be wrong, not how long the user is misled.

## 5. Forks

**All adopted as recommended (user's decision, 2026-07-28)** — F2 and F4 were put
to the user explicitly (they change what gets built); F1, F3, F5, F6 were taken by
recommendation.

### F1 — Which servers get periodic probing?

- **(a) Local/external only (managed + external); cloud stays as it is.**
  The cloud has no `/health`. The only way to check it is a real API call, which
  costs money and quota and can trip rate limits — to answer a question the next
  real request answers for free. A cloud outage surfaces as an error on the
  request itself, with the provider's own message.
- (b) Also probe the cloud with a cheap call (e.g. a 1-token completion or a
  models list) on a slow cadence.

**Recommendation: (a).** Paying per poll to pre-empt an error that already
reports itself clearly is a bad trade, and a "ready" chip for a cloud provider
means "configured with a key", which is honest — there's nothing to be
disconnected *from* until a request is made.

### F2 — Cadence

- **(a) Asymmetric: rarely while healthy, often while broken.** e.g. **60 s**
  when `Ready`, **5 s** when `Disconnected`/`NotConfigured`-but-configured.
- (b) One fixed interval (e.g. 30 s) in both states.
- (c) Exponential backoff after each failure, capped.

**Recommendation: (a).** The two states have genuinely different stakes. While
healthy, a probe buys almost nothing — the next real request would reveal a
failure anyway — so it should be cheap and rare. While broken, the probe *is*
the recovery mechanism (§3), and 5 s is the difference between "it just started
working" and "I restarted the app". Note the measured asymmetry in cost, which
happens to point the same way: a healthy probe is ~1 ms, a failing one ~2 s, so
the failing state is self-limiting even at 5 s.

### F3 — Flapping: how many failures before the chip goes red?

- **(a) Hysteresis: `Ready` → `Disconnected` only after N=3 consecutive
  failures; `Disconnected` → `Ready` on the first success.**
- (b) Flip on the first failure.

**Refinement found while implementing:** the cadence keys off *both* the state
and the failure streak — `Ready` **and** streak 0 → 60 s, otherwise 5 s. Without
this, confirming a real failure at N=3 would take ~3 minutes (three 60 s polls);
with it, the first failure switches to the fast cadence and the verdict lands in
~10 s. Hysteresis then costs almost nothing in detection latency while still
absorbing a single dropped poll.

**Recommendation: (a).** Asymmetric on purpose. A single missed poll is not
evidence a server is down — it can be one dropped packet, a GC pause, a slot
contention `503` on a build that reports that way (§4) — and a chip that
flickers red mid-conversation is its own kind of lie, one that teaches the user
to ignore the indicator. A success, by contrast, is self-proving: the server
answered. With the fast-recheck refinement above, N=3 costs ~15 s on top of the
first missed poll (~75 s worst case in total, §4); a failure that matters sooner
than that is reported by the failing request itself, immediately and with a
better message. A dead *managed* child doesn't wait for any of this — the exit
signal reports it in ~150 ms (F4).

### F4 — Managed servers: probe, or watch the process?

A managed child that dies leaves a dead port. Re-probing it can never succeed —
recovery requires relaunching the process.

- **(a) Watch `exited` continuously (instant `Disconnected` on child death) and
  relaunch under a restart budget, reusing the `McpManager` pattern (≤3
  restarts per 5 min, then `Disconnected` until manual intervention).**
- (b) Watch `exited`, report, don't relaunch.
- (c) Treat managed like external — just probe the port.

**Recommendation: (a).** The `exited` token already exists and is already
plumbed; consulting it continuously is nearly free and strictly better than
waiting for a probe timeout. The restart budget is an established pattern in this
codebase ([mcp.rs](../src/app/orchestrator/mcp.rs), `allow_restart`) and it
exists precisely to stop a crash-looping server (a corrupt GGUF, an OOM) from
being respawned forever. (c) is wrong — it would report "connecting…" against a
port nobody is listening on until the timeout.

*If you'd rather keep this PR narrow, (b) is a clean subset and the relaunch can
follow separately.*

### F5 — Where does the timer live?

- **(a) Turn `spawn_probe` into a looping monitor task per server** (one task,
  the existing per-server `CancellationToken` already stops it; the existing
  status channel already carries its verdicts).
- (b) A deadline in the orchestrator's `select!` loop, like `SaveQueue`/
  `RestartQueue`.

**Recommendation: (a).** The probe is I/O with a multi-second worst case; in the
`select!` loop it would either block the orchestrator or need spawning anyway.
Everything it needs — cancellation, a status channel, invalidation on
re-`apply` — is already wired per server, so this is a change of loop shape, not
of architecture. `SaveQueue`/`RestartQueue` are deadline-in-loop because their
work is instant and touches owned state; this isn't.

### F6 — Should the status gate anything new?

- **(a) No change.** Chat keeps gating generation exactly as today; embeddings
  and impersonation stay informational.
- (b) Use a fresher status to pre-empt requests.

**Recommendation: (a).** Out of scope, and (b) risks making a transient probe
failure block a request that would have succeeded — strictly worse than letting
the request try and report its own error.

## 6. Stages

One PR, stacked on `fix/embed-server-probe` (it builds directly on that
plumbing, and the repo has precedent for linear stacks):

1. `spawn_probe` → a looping monitor: asymmetric cadence (F2), hysteresis (F3),
   `exited` watched continuously (F4).
2. Managed relaunch under a restart budget (F4a) — droppable to keep the PR
   narrow.
3. Docs: architecture §6, spec §11.1, CHANGELOG, [journal](journal/engine.md), roadmap
   (close the groundwork item).

## 7. Tests

- Pure: the hysteresis counter (N failures flips, N−1 doesn't, a success resets);
  cadence selection by state; `allow_restart` reuse.
- Against the local stub listener from PR #224 (accept-and-answer /
  accept-and-hang-up, so failures are instant): `Ready` → server stops answering
  → after N polls `Disconnected`; then it answers again → `Ready` on the first
  success. Paused time keeps the intervals virtual.
- A stale monitor (settings changed) stops and posts nothing.
- Live smoke: stop the embedding server, watch the chip go red, start it, watch
  it recover — the recovery half (§3) is the point and can't be shown offline.

## 8. Out of scope

- Auto-restart policy for *external* servers (we don't own the process).
- Surfacing per-server latency/uptime in the UI.
- Probing cloud providers (F1b) — revisit only if a real need appears.
