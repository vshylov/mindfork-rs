+++
title = "Why the engine is a server, not a library"
description = "mindfork nearly embedded an inference runtime. The contract that replaced it made a local GGUF and a cloud frontier model the same thing."
updated = "2026-08-11"
weight = 2

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-08-11"
+++

*The second in a short series on how mindfork is put together. The
[first](/articles/mindfork-at-a-glance/) is the overview.*

## The obvious design, tried first

A Rust chat app wants a Rust inference crate — link it, call it, ship one
binary. The first design did exactly that, and reality disagreed twice:
output quality on the models we actually cared about was not there, and the
build story on Windows — a platform this project treats as first-class — was
painful. Embedding also carries a quieter tax: an inference runtime drags
GPU toolchains into every build, and the app's release cycle gets chained to
the engine's.

## The replacement: one HTTP dialect

llama.cpp ships `llama-server`, which speaks the same HTTP dialect as
OpenAI's API. That turns "the engine" into a **contract instead of a
dependency**: mindfork talks to an OpenAI-compatible endpoint and does not
care what is behind it.

Two local modes fall out of it. In **managed** mode the app launches and
supervises a `llama-server` for your GGUF itself — health checks, restarts,
a relaunch budget. In **external** mode you point it at any compatible
endpoint: vLLM, LM Studio, Ollama, a box across the room.

The cloud providers are not special cases bolted on the side. OpenAI,
Anthropic, Gemini and Grok are **sibling implementations of the same small
trait** the local server sits behind — streamed text, "thoughts", tool
calls, token usage, finish. A conversation can hop from a local Gemma to a
cloud frontier model and back without changing shape.

## The details that keep it honest

- **Sampling is per-provider, not lowest-common-denominator.** The standard
  OpenAI fields plus llama.cpp's extensions (min-p, DRY, XTC, mirostat, …)
  are all there, and each field is sent only when the active provider
  actually accepts it — one source of truth decides, so no server gets
  fields it would misread.
- **Stopping is a token, not a string.** Generation ends on the end-of-turn
  token id, server-side; the `stop` strings field is deliberately never
  sent. A model that *writes* the word you chose as a stop string should
  not be cut off mid-thought by its own prose.
- **"Thoughts" are first-class.** Reasoning deltas arrive on their own
  channel per provider, with a parsing fallback for models that still
  inline `<think>` blocks — so the UI can fold them regardless of where
  the model runs.

## What it buys

Building mindfork needs no GPU toolchain. A wedged local server gets
restarted under the app without taking the chat down. And every feature
above the engine — the agentic tool loop, memory, compaction — is written
once, against the contract, and works identically over a laptop GGUF and a
frontier API.

The full decision record lives in the repository — ADR 0004, "engine
contract and multi-provider inference" — alongside the architecture notes:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).
