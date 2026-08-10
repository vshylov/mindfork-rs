+++
title = "Hello from mindfork 0.9.5"
description = "The project gets a home on the web — and a status report: an interactive demo, a fourth cloud provider, history compression."
+++

mindfork is a terminal AI chat written in Rust. It runs local models through
llama.cpp's `llama-server`, speaks to OpenAI, Anthropic, Gemini and Grok in
the cloud, and keeps a memory of its own: a self-model, notes with a link
graph, and RAG over your documents. Windows and Linux, one native binary.

This site is its new home. Short project news lands here in the blog — there
is an [Atom feed](/atom.xml) — and longer pieces live in
[Articles](/articles/).

## Where the project stands

Version 0.9.5. The original M0–M9 plan is finished, and the last few months
added, among other things:

- **`mindfork demo`** — an interactive demo mode: the real TUI on seeded
  data with a scripted engine. No model download, no API key, nothing
  written outside a temp folder.
- **Grok (xAI)** as the fourth cloud provider.
- **History compression** — a rolling summary keeps a long conversation
  inside the model's context window (`/compact`, plus an automatic
  trigger), with read-back tools for the folded range.
- **Generated screenshots** — every screenshot on this site is rendered
  from the current code by the test suite and guarded against rot in CI.

## What's next

Cloud-error retry/backoff and image input sit at the top of the roadmap.
News will land here as it ships.
