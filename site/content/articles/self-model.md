+++
title = "The self-model: a memory that knows it changed"
description = "Each companion keeps a model of itself and of you — a summary, goals, traits and dated observations — and maintains it with its own tools, on your disk."
updated = "2026-08-15"
weight = 3

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-08-15"
+++

*The third in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/) and
[the engine](/articles/engine-as-a-server/).*

## The problem it solves

Close a chat, open a new one, and most assistants start from zero. The
model that spent an evening learning what you are building greets you the
next morning as a stranger. mindfork's answer is the **self-model**: a
small, persistent representation of the companion's sense of itself, its
goals, and you — living across chats, one per profile, on your disk.

The name is precise. It is not a transcript and not a vector dump of
everything ever said; it is a *model* — a working snapshot the assistant
itself keeps current.

## Four organs

- **About itself** — a compact summary: who I am, what I value, how I
  work. The assistant is told to *integrate and shorten* when editing,
  never to append forever.
- **Goals** — long-term intentions with a lifecycle: active, completed,
  abandoned. Closed goals are kept visible for a while, not silently
  deleted.
- **About the interlocutor** — your stable traits, current interests, and
  a line about the relationship. Edits *merge* into the lists rather than
  replace them, so one bad evening cannot overwrite a month of history.
- **Observations** — short dated insights: what it understood and when,
  episodes, resolved questions, contradictions. A biography, where the
  snapshot above is the present tense.

That last split is the design's core idea. Structured fields hold the
*current* conclusions; observations remember *how they changed*. When the
assistant revises what it thinks about you, it is asked to leave a dated
note saying what changed and why — a scar, not a silent overwrite.

## Who maintains it

The assistant does, with five tools it can call mid-conversation: read
the model, update the self-summary and goals, update its model of you,
record an observation, and reflect. A short maintenance protocol rides
along in the system prompt and encodes the rules — keep the summary
brief, close finished goals, drop interests you have not confirmed in a
while, and, verbatim: *accuracy over flattery — record what is true, not
what pleases*.

The tools push back on sloppy writes. A new observation that closely
repeats an old one gets the old one shown back with a suggestion to
revise instead of duplicate. A new trait semantically close to an
existing one triggers a question: same thing twice, or a contradiction
worth recording? (The closeness is measured with the same embedding
stack described in [the next article](/articles/vector-search/).)

Two background loops are available on top, both **off by default**
because they cost tokens: auto-reflection, which looks over recent
conversation after every N replies and files what it learned, and a
consolidation pass — "sleep", if you like — that merges near-duplicate
observations and trims an overgrown summary. Both run silently, on
whatever engine the profile already uses; a quiet status-bar chip is the
only sign they are working.

## What reaches the prompt

Every turn, the model's system prompt carries a compact rendering: the
self-summary, active goals, what it knows about you, and a handful of
observations. The observations are not simply the newest ones — the app
embeds your latest message and surfaces the *topically relevant* ones,
plus the freshest for continuity. An insight from three weeks ago
resurfaces exactly when the topic does.

The injected block lives on a budget with fixed shares per section, so a
verbose self-summary can never crowd out what the assistant knows about
you. Lists are trimmed by whole items and say "… N more" — a trait cut
in half would read as a different trait.

## You stay in charge

Nothing is written until you opt in: the self-model tools are disabled
by default for every profile and are enabled per profile in settings.
Once on, everything is inspectable and editable — press `F3` (or type
`/self`) to open the viewer: edit the summary, cycle a goal's status,
delete an observation, or clear the whole model behind a confirmation
(`/self clear`).

And it is local by construction. The self-model lives in the app's own
SQLite database next to the binary, isolated per profile — two
companions never see each other's memory. The only thing that ever
leaves your machine is the prompt itself, sent to whatever engine you
configured — which, by default, is a llama.cpp server on the same
machine.

## Why it is built this way

Every guardrail exists because something concrete broke on a live model.
Goals rendered without ids could never be closed — the model filled them
in once and forgot them. Wholesale list replacement produced the "mood
swing" bug: a good day made you "the kindest", a bad one "merciless",
and each edit destroyed the rest. Pure accumulation grew forty traits
where five carried signal. The fixes — merge semantics, goal handles,
duplicate gates, scars, consolidation — add up to one principle:
**integration instead of accumulation, and loss made visible rather than
silent**.

The full decision record — spec §17, the architecture notes and the
self-model journal — lives in the repository:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).
