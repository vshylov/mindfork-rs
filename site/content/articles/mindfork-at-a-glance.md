+++
title = "mindfork at a glance"
description = "The shape of the app: an engine contract over OpenAI-compatible servers, a client-side agentic loop, one turn from start to finish, layered memory, boring durable storage — and where the trust boundaries are."
updated = "2026-09-20"
weight = 1

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-08-10"
+++

*A short architectural tour — the shape of the thing, not a manual. It
describes mindfork {{ config.extra.app_version }}; each section ends with the article
that tells its full story.*

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

Getting an engine is one command. `mindfork llama backends` lists the
llama.cpp builds published for your machine — `cpu`, `vulkan`, a CUDA or a
ROCm build — and `mindfork llama setup --backend <id>` downloads one, checks
every file against the release's checksums and runs it once to report the
build number and the compute devices it found. Leave the binary field
empty and the app uses the build installed last, or a `llama-server`
sitting beside it. Two more servers may run alongside: a small embedding
server for the memory and, optionally, a second chat server for
impersonation.

*Full story: [Why the engine is a server, not a library](/articles/engine-as-a-server/) ·
[The same answer, sooner](/articles/local-speed/).*

## The agentic loop is client-side

Tool calls run in the app, not on a server. The orchestrator — the sole
owner of chat state — drives the loop: the model asks for a tool, the tool
returns a result plus a set of effects, the orchestrator applies them. No
locks, no shared mutable state, and every provider gets the same tools:

- **the web** — search, page fetching, YouTube — off until you switch it on;
- **files**, and a **code project** attached to the chat: the assistant
  reads, searches and edits it, and runs the build, run and test lines
  *you* typed — never ones it composed — with every change reviewable as a
  diff and revertible per file;
- **Python** — a WebAssembly sandbox by default, or your own interpreter
  one setting away; a call names the chat's files it needs, and whatever
  the code saves comes back into the chat, a chart shown to the model so it
  can check its own plot;
- **delegation** — a subagent runs as a nested turn with the same tools,
  and a staged **dialogue** between two personas is directed by the
  assistant line by line; both transcripts are conversations of their own
  in the chat list, searchable, streaming while they run;
- **plugins** over MCP, **speech**, and **introspection** — the assistant
  can read its own self-model and sampling, and name the model that is
  generating it.

None of it has to hold the conversation. A delegation or a dialogue can
run in the background: the assistant keeps talking while the run is out,
the result arrives later as a task notification, and a tasks screen
(`F7`) lists everything in flight across every chat. The app's own quiet
work goes through the same door — reflection, consolidation, history
compaction and the automatic title run one at a time, yield to your
message when the server cannot hold both, and each reserves its place in
the server's context pool so that two streams never overfill it together.
Parallel sessions, parallel tool calls and parallel runs are settings; all
default to the sequential behaviour.

{% <screenshot name="chat" title="chat"> %}A turn with tool calls: each card opens the moment the call starts and fills in when the result lands.{% </screenshot> %}

*Full story: [Why the Python sandbox is WebAssembly, not Docker](/articles/python-sandbox/) ·
[The code workspace](/articles/code-workspace/) ·
[Background runs, and the pool they share](/articles/background-runs/).*

## One turn, start to finish

```
you ──▶ message
         ├─ system prompt   profile · self-model block (summary, goals, traits,
         │                  the observations relevant to this message) ·
         │                  attachments · the attached project
         ├─ context         rolling summary of the folded history · recent turns
         └─ tools           the profile's set, as schemas
       engine ──▶ stream    thoughts · text · tool calls
                  round     readers may run together, writers one after another,
                            a dangerous call may ask you first; effects applied
                  … until the reply ends
       landed ──▶ later     title · reflection · consolidation · compaction —
                            one at a time, in the background
```

The system prompt is assembled, not typed: the profile's own text, a
budgeted block from the self-model — its observations chosen by embedding
your latest message and picking the topically relevant ones, plus the
freshest for continuity — and the chat's attachments, inline when they
fit and by reference when they do not. Notes and the knowledge base are
not injected; the model reaches them through tools, when it decides it
needs them. Then the stream: thoughts and text token by token, tool calls
collected into a round — the readers may run together, the writers one by
one, and with the confirmation switch on, a call that changes something
outside the app shows you its code first. When the reply lands, the quiet
work is scheduled behind it, never in front of your next message.

Two details keep this honest under a local server. The app counts what a
request will occupy — the tool schemas are the largest part — and reserves
it, corrected by the exact counts the server has already reported; and a
server error inside an open stream ends the reply as an error, never as a
finished answer cut mid-word. `/continue` picks a cut reply up exactly
where it stopped, on the engines that can resume one.

## Memory is layered, not a vector dump

- a **self-model**: a summary the assistant maintains about itself and
  you, goals with statuses, dated observations — consolidated on a
  schedule ("sleep", if you like);
- **notes** with a typed link graph and semantic recall;
- **RAG** over attached documents (HTML, PDF, DOCX) with per-chunk
  indexing, and the chat's own attachments searchable the same way;
- **history compression**: past a threshold, older turns fold into a
  rolling summary the model can still search and page back through;
- **the other conversations** — off by default; switched on, the assistant
  can search and read your other chats of the same profile, and cites
  them by a `chat://` address the feed turns into a link;
- **the record of which model ran when**, so the assistant can name its
  model and tell you when it changed.

All of it is per-profile, and all of it lives on your disk.

{% <screenshot name="self-model" title="self-model"> %}The self-model screen: what the assistant keeps about itself and about you, and the dated observations behind it.{% </screenshot> %}

*Full story: [The self-model: a memory that knows it changed](/articles/self-model/) ·
[Where vector search earns its keep](/articles/vector-search/).*

## Storage is deliberately boring

JSON for config, profiles and chats — atomic writes with `.bak` copies.
SQLite (with sqlite-vec) for notes and RAG. Soft delete everywhere. A
disposable full-text cache that can be deleted and rebuilt at will. Beside
them, two plain folders: `files/` for what the assistant's code saved for
a chat, and `workspace/` for the pre-image of every project file it
edited, which is what revert puts back. The data directory is portable: it
sits next to the binary and moves when it moves.

Every format carries a schema version. When a release changes one, the app
backs the data up first, migrates, and stamps the new version; an older
build refuses a newer file with a plain message rather than misreading it.
Backups are one command, optionally AES-256 encrypted, and carry the
stored files and the workspaces with the rest. With the data on more than
one computer, `mindfork stats` reads a copy — the live data or a closed
archive — and says when it was last written and what it holds, and
`--compare` says what each of two copies has that the other lacks.

## Where the trust boundaries are

An assistant that can browse, run code and edit a project needs edges,
not promises — a page it reads can ask it to open something on your side
of the router. So the edges are in code, per tool, and off by default.

- **The network.** Every outbound connection is to an endpoint you
  configured or a tool you switched on; the web tools start off in a fresh
  installation. An address the model chose is resolved and checked against
  the public internet — your LAN, loopback and the address cloud providers
  keep credentials behind are refused, on every redirect. A search key is
  used only where you put it. MCP servers are local subprocesses behind a
  double opt-in.
- **The machine.** The sandbox begins with nothing: a scratch directory, a
  read-only package image, sockets only with the network toggle. The
  project tools stay inside the attached directory and run only the lines
  you typed. `/file open` launches document types and nothing else, and a
  workbook the code wrote opens in Protected View.
- **Consent.** One switch asks before any call that changes something
  outside the app — Python, a file write, a project edit or command, every
  plugin tool — and shows the code it is about to run. A background run
  never asks, so its permissions are the profile's tool set.
- **Secrets, and the author.** Keys are stored encrypted and bound to the
  machine, never displayed back — or named as an environment variable and
  never held at all. Nothing reaches the author: no telemetry, analytics,
  crash reports or update checks; the privacy policy is in the app and on
  this site.

{% <screenshot name="settings-tools" title="settings — tools"> %}Every tool sits behind a switch, and the switches start off.{% </screenshot> %}

*Full story: [Where the trust boundaries are](/articles/trust-boundaries/) ·
[privacy policy](/privacy/).*

## The TUI is the point

ratatui; dark and light themes, and an `auto` one that asks the terminal
for its background; English and Russian. Markdown with syntax
highlighting, tables, Mermaid diagrams and LaTeX — all rendered as text,
in the terminal, with collapse toggles for thoughts and tool cards, search
inside the conversation and across every chat, and a spellchecker in the
input box. Pictures go in — from a file, the clipboard or a URL, to a
local vision model or a cloud one — and speech comes out, with `/tts`.
Every action has a typed command for a terminal that keeps the chord for
itself, copying works over SSH, a conversation exports to a file, and `F1`
opens each screen's own key list.

If that sounds like your kind of tool:
[GitHub](https://github.com/vshylov/mindfork-rs) · [news](/blog/).

*Read on: [the engine](/articles/engine-as-a-server/) ·
[local speed](/articles/local-speed/) ·
[the self-model](/articles/self-model/) ·
[vector search](/articles/vector-search/) ·
[the Python sandbox](/articles/python-sandbox/) ·
[the trust boundaries](/articles/trust-boundaries/) ·
[the code workspace](/articles/code-workspace/) ·
[background runs and the pool](/articles/background-runs/).*
