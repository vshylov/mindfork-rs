+++
title = "mindfork at a glance"
description = "The shape of the app: an engine contract over OpenAI-compatible servers, a client-side agentic loop, layered memory, and boring, durable storage."
weight = 1
+++

*A short architectural tour — the shape of the thing, not a manual.*

## One engine contract, many backends

mindfork does not embed an inference runtime. The engine is an
**OpenAI-compatible HTTP server** behind a small backend trait: in managed
mode the app launches and supervises a local llama.cpp `llama-server`
itself; in external mode any OpenAI-compatible endpoint works — vLLM,
LM Studio, Ollama, a remote box of your own. OpenAI, Anthropic, Gemini and
Grok are sibling implementations of the same trait, so a chat can hop
between a local Gemma and a cloud frontier model without changing shape.

Two deliberate consequences follow. Embedding an inference library as a
Rust dependency was rejected — it drags GPU toolchains into every build and
couples the app's release cycle to the engine's. And the sampling surface
is honest: the standard OpenAI fields plus the llama.cpp extensions
(min-p, DRY, XTC, mirostat, …), each sent only when the active provider
actually accepts it.

## The agentic loop is client-side

Tool calls run in the app, not on a server. The orchestrator — the sole
owner of chat state — drives the loop: the model asks for a tool, the tool
returns a result plus a set of effects, the orchestrator applies them. No
locks, no shared mutable state, and every provider gets the same tools:
web search and page fetching, a WASM-sandboxed Python runtime, notes, file
access, YouTube ingestion — and anything you plug in over MCP.

## Memory is layered, not a vector dump

- a **self-model**: a summary the assistant maintains about itself and
  you, goals with statuses, dated observations — consolidated on a
  schedule ("sleep", if you like);
- **notes** with a typed link graph and semantic recall;
- **RAG** over attached documents (HTML, PDF, DOCX) with per-chunk
  indexing;
- **history compression**: past a threshold, older turns fold into a
  rolling summary the model can still search and page back through.

All of it is per-profile, and all of it lives on your disk.

## Storage is deliberately boring

JSON for config, profiles and chats — atomic writes with `.bak` copies.
SQLite (with sqlite-vec) for notes and RAG. Soft delete everywhere. A
disposable full-text cache that can be deleted and rebuilt at will. The
data directory is portable: it sits next to the binary and moves when it
moves. Backups are one command, optionally AES-encrypted.

## The TUI is the point

ratatui, dark and light themes, English and Russian. Markdown with syntax
highlighting, tables, Mermaid diagrams and LaTeX — all rendered as text,
in the terminal, with collapse toggles for thoughts and tool cards,
in-chat search, and a spellchecker in the input box.

If that sounds like your kind of tool:
[GitHub](https://github.com/vshylov/mindfork-rs) · [news](/blog/).
