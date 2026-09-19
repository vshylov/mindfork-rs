+++
title = "The same answer, sooner"
description = "FlashAttention and speculative decoding in managed mode: two llama.cpp levers that change how fast tokens arrive — without trading away the answer."
updated = "2026-08-17"
weight = 5

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-08-17"
+++

*The fifth in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/),
[the self-model](/articles/self-model/) and
[vector search](/articles/vector-search/).*

## Two levers, one rule

In [managed mode](/articles/engine-as-a-server/) mindfork launches and
supervises a local llama.cpp `llama-server` itself — which means it owns
the command line that server starts with. Two of the most useful flags
on that line are speed levers, and the settings expose both:
**FlashAttention** and **speculative decoding**.

They share one property worth stating before any mechanism. Neither is
a quality dial. A smaller model answers faster by knowing less; a
harsher quantization answers faster by remembering less precisely.
These two change how fast tokens arrive without changing which model
answers or what it knows.

## FlashAttention: the same math in a better order

Attention compares every new token against everything already in the
context. Done naively, that materializes a large intermediate table and
drags it through the slowest memory on the card — and the table grows
with the *square* of the context length. FlashAttention computes the
same attention in small tiles that stay inside the GPU's fast on-chip
memory: an optimization of order, not an approximation. Nothing about
the result is traded away.

The longer the context, the more it pays. A short question barely
notices; a conversation deep into a 16k context, with attachments and
history along for the ride, is exactly where prompt processing stops
being free.

In settings the lever is a tri-state, and the default is deliberately
humble: **auto** passes no flag at all and lets llama.cpp decide for
your backend and model; **on** and **off** exist for the day you know
better — a combination you want forced, or one you are ruling out while
chasing a bug.

## Speculative decoding: a draft the model checks

Generation is sequential: one token costs one full pass, and the pass
spends most of its time streaming the model's weights out of VRAM — the
GPU waits on memory, not arithmetic. Speculative decoding puts the idle
capacity to work. A cheap **draft** proposes several next tokens; the
main model checks the whole proposal in a single pass. Where the draft
guessed right, several tokens land for the price of one trip; at the
first wrong guess, the main model's own token stands and the rest of
the draft is discarded.

The draft never decides. Every token that reaches the screen is one the
main model endorsed — the draft only changes how often it gets to say
"yes, all of that" instead of one word at a time. Which is also the
honest caveat: drafting is not free. On text the draft cannot predict —
high-temperature prose, a topic it has no grip on — the acceptance rate
drops, and speculation can cost more than it saves. It is a lever to
measure on your workload, not a checkbox that is always right.

## Where a draft comes from

llama.cpp offers several drafters, and the settings expose the full
menu:

- **A small sibling model** (`draft-simple`) — a model from the same
  family as your main one, a fraction of its size, loaded alongside it.
  The classic setup; costs a download and some VRAM.
- **EAGLE-3** (`draft-eagle3`) — a trained speculation head shipped as
  its own small GGUF, built specifically to predict its base model.
- **MTP** (`draft-mtp`) — multi-token-prediction companions, such as
  the `mtp-*.gguf` files published alongside Gemma 4: the family's own
  trained guess at its next few tokens.
- **N-gram** (`ngram-simple`, `ngram-map-k`, …) — no second model at
  all. The draft is guessed from text already in the context, which
  makes it free to set up and strongest exactly where chat work lives:
  quoting a document back, editing code that is already in the window,
  structured output that echoes itself.

Pick a `draft-*` type and the settings reveal the draft-model fields —
the GGUF path, its GPU layers, how many tokens to draft per step; pick
an n-gram type and those fields stay out of the way.

## What managed mode adds

The flags themselves are llama.cpp's. What the app contributes is the
handling around them:

- **Defaults change nothing.** FlashAttention on `auto` and speculation
  off pass no flags: the launch line is byte-for-byte what it was
  before these settings existed, and llama.cpp's own defaults rule.
- **A typo fails fast.** The draft model's path is checked before
  launch, like the main model's — a wrong path is an error message, not
  a server that hangs loading while the health probe waits it out.
- **Every managed chat server gets it**, and the embedding server is
  deliberately left out: it does not generate tokens, so there is
  nothing to speculate.
- **External and cloud modes are untouched.** Your own server keeps its
  own flags, and the cloud providers already run whatever speculation
  they run behind the API.

Configured, the launch line the app builds for you looks like this:

```
llama-server -m gemma-4-12b-it.gguf -ngl 99 -c 16384 --jinja \
    --flash-attn on --spec-type draft-mtp \
    -md mtp-gemma-4-12B-it.gguf -ngld 99
```

The decision record — the engine journal and the install guide's
managed-mode section — lives in the repository:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).
