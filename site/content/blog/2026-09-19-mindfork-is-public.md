+++
title = "mindfork is public"
description = "The repository is open, the first public release is out, and this is what the project is — an AI chat that lives in your terminal, with memory that persists and tools that stay behind switches you set."
draft = true
+++

<!--
A DRAFT, deliberately: `draft = true` keeps it out of the build until you set
the date in the filename and the front matter and remove this line. Two things
to check before publishing (public-release-readiness.md B1): the repository is
public and https://github.com/vshylov/mindfork-rs/releases answers 200 to an
anonymous visitor, and the release itself is published rather than a draft —
every link below points at both.
-->

The repository is open, and the first public release is on the
[releases page](https://github.com/vshylov/mindfork-rs/releases).

mindfork is an AI chat that lives in your terminal. It runs local models through
llama.cpp, or talks to OpenAI, Anthropic, Gemini and Grok — all four configured
at once if you like, switching between them without losing anything. It is one
native binary for Windows and Linux, it keeps its data in a folder next to
itself, and it has no telemetry, no update check and no account.

## What it is actually for

Most clients are a window onto a model. This one is built on a different
premise: that a local Gemma or Qwen becomes **more interesting to talk to** when
it is given room to remember and to act.

So it keeps a **self-model** — a summary, goals, traits and an observation
narrative that the assistant maintains about itself and about you, and that you
can read and wipe on one screen. It writes **notes** and links them into a graph
where one note can supersede another. It builds a **knowledge base** out of your
own files, retrieved semantically rather than by keyword. All of it per companion
profile, all of it on your disk.

And it has **tools**: web search and page fetching, a Python sandbox with no
access to your files, a file-access tool jailed to one directory you name, an
attached code project it can read and change under a diff you approve, video
understanding, and anything else you plug in over MCP. Every one of those is off
until you turn it on, and an optional confirmation shows exactly what a call is
about to do before it does it.

## What a first run looks like

```
mindfork demo
```

Sample conversations, a scripted engine, nothing written outside a temporary
folder. It takes a minute and answers the only question that matters early:
whether you want this shape of thing at all.

After that, [the install page](/install/) has the downloads and
[the manual](https://github.com/vshylov/mindfork-rs/blob/main/docs/manual.md)
has the rest.

## How it was built, and why the repository is worth a look

Two years of small reviewed tracks, each one a design document with its
alternatives written down, a branch, tests beside the code, and — for anything
touching a model — a run against a real one before it shipped. The suite is over
three thousand tests, and the engineering log says what was measured and what was
rejected, not just what was done.

Much of the code was written by AI models under review, and the history says so:
every commit names the model that wrote it. That is not a disclaimer, it is the
same honesty the rest of the project tries for — the
[journal](https://github.com/vshylov/mindfork-rs/tree/main/docs/journal) is full
of measurements that overturned the plan they were meant to confirm.

The code is [MIT](/license/). The
[disclaimer](/disclaimer/) says what shipping no model means for what appears on
your screen, and the [privacy policy](/privacy/) says what leaves your machine —
which is nothing you did not configure.

If you try it, [issues](https://github.com/vshylov/mindfork-rs/issues) are the
place to say what broke.
