# Self-model injection: per-section budgets

> Design plan for one PR. Decisions **D1–D5 confirmed by the user 2026-08-08**.
> Found while measuring for [prompt-caching.md §2.1](research/prompt-caching.md).
> Behaviour: spec §9.4 (self-model), the injected block.

## 1. The defect

`SelfModel::render_for_prompt` assembles the passively-injected block in this
order — description → active goals → interlocutor model → observations — and
**only the first section has a budget** (`max_chars / 2`, added by
summary-as-snapshot stage 3). Goals, traits, interests and the relationship
dynamic are unbounded; the whole block is then cut at `prompt_cap`. So it is not
a budget, it is a queue: whoever renders first eats.

Measured on the real dev profile (66 `@self` observations), against the default
`prompt_cap = 1200`:

| section | chars | limit today |
|---|---:|---|
| description | 1349 | truncated to 600 |
| active goals (5) | 855 | **none** |
| traits (16) | 1893 | **none** |
| interests (16) | 1039 | **none** |
| relationship dynamic | 223 | **none** |
| observations | — | whatever is left |

5359 chars of content against a 1200 budget. What actually reaches the model is
the description (600) and the goals, cut mid-item — **and nothing else**. So on
this profile neither the **interlocutor model** nor the **observations** are
passively injected at all: `update_user_model` writes into a structure the model
never passively sees, and `injection_recent` queries the embedder every turn for
a relevance selection that truncation then discards — while the injected text
tells the model that observations "surface by relevance".

The code comment states the intent correctly ("the description gets no more than
half the limit, so a bloated summary doesn't crowd goals/interlocutor/
observations out of the injection"); it was only ever implemented for the
description.

**Not a data-loss defect**: `render_full` (what `get_self_model` returns and `F3`
shows) is not truncated. This is about what is *passively* injected.

## 2. Decisions

- **D1 — enforce at render time** (not by limiting what is stored, and not by
  raising the cap alone). Storage limits would change user data that the model
  itself curates; raising the cap alone just moves the wall and pays context on
  every turn.
- **D2 — fixed shares with carry-forward**: each section gets a percentage of
  the budget plus whatever earlier sections did not use. Because a share is a
  *ceiling*, a bloated early section cannot eat a later one's allowance — which
  is the floor D2 asks for, without a second mechanism.
- **D3 — lists are truncated by dropping whole items**, never mid-item, and the
  block says how many were dropped, so the model knows it is seeing a part and
  can read the rest with `get_self_model`.
- **D4 — a truncated section says so.** Prose keeps its `…`; lists carry the
  dropped-item marker from D3.
- **D5 — the default `prompt_cap` goes 1200 → 4000.**

## 3. Design

Shares, in render order: **description 40%, goals 20%, interlocutor 20%,
observations 20%**. Each section's allowance is `max_chars * share / 100 + carry`,
where `carry` is the unused remainder of every earlier section.

On the measured profile at the new default (4000): description 1349 ≤ 1600
(carry 251) → goals 855 ≤ 1051 (carry 196) → interlocutor 3155 vs 996 →
truncated, listing as many traits/interests as fit plus "+N more" → observations
≤ 800. **All four sections render**, which is the point.

At the old 1200 the same code still degrades sensibly: every section gets
something rather than the first two taking everything.

The final `truncate_chars(out, max_chars)` stays as a safety net — section
budgets count content, not the fixed labels around it — but it should now
essentially never fire.

## 4. Scope

In: `entities/self_model.rs` (the renderer), one locale key pair for the
dropped-item marker, the `prompt_cap` default.

Out:
- **Existing installs keep their stored `prompt_cap`** — `settings.json` pins the
  value, so D5 only affects fresh installs unless the user edits the field. A
  value migration is deliberately not done: it would also overwrite a
  deliberately-chosen 1200 (ADR 0006 treats a value rewrite as a real
  migration, and this one has no correctness argument behind it).
- `render_full` — deliberately untruncated, unchanged.
- The size of what the model *writes* (summary target, trait counts) — a
  different question, already served by the summary size gate.

## 5. Tests

- Each section gets its share; a bloated description cannot starve the others
  (the direct regression — assert all four sections present on data shaped like
  the measured profile).
- Carry-forward: a short description gives goals more room.
- Lists drop whole items and report the count; a single oversized item still
  degrades to a character cut rather than vanishing.
- Observations render when earlier sections overflow (the defect, pinned).
- The default is 4000, and an old `settings.json` keeps its stored value.
