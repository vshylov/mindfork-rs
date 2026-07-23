# ADR 0002 — RAG embeddings: dedicated embedding server

**Status:** accepted (2026-06-14). Closes the `[R]` "xinfer embeddings" from
[plan.md §M5](../history/plan.md).

**Context:** RAG (`rag_add`/`rag_search`) and semantic `note_recall` need
embeddings. An OpenAI-compatible server can serve `/v1/embeddings` off the
**already-loaded chat model**, which technically removes the need for a second
process. But embedding quality from an autoregressive chat model is usually
lower than from a specialized embedding model.

## Decision

**Dedicated embedding server.** Embeddings are not taken from the chat engine,
but from a **separate** `xinfer` process, launched with an embedding model on
a separate port. In code this is reflected by a trait split:

- `EngineBackend` (chat) — only `chat_stream`.
- **`Embedder`** (new) — only `embed`. Implemented by the same `XinferClient`,
  but pointed at the embedding server's port.

`ToolContext` carries `Arc<dyn Embedder>` separately from `Arc<dyn EngineBackend>`.

### Lifecycle

- The embedding server is configured via env (`MINDFORK_EMBED_URL` — external;
  `MINDFORK_EMBED_BIN`/`MINDFORK_EMBED_MODEL`/`MINDFORK_EMBED_PORT` — managed)
  and, if set, is launched at app startup through the same supervisor as the
  chat server (`ServerHandle::launch`), and stays alive until exit (kill on
  drop).
- If the embedding server is not configured — `UnavailableEmbedder` is used:
  RAG tools return a clear error (they don't panic), everything else works.
- Deviation from the plan's wording "use and stop after each call": reloading
  the model costs a minute, so we keep the process alive. A fully lazy start
  "on first `rag_*`" and UI settings — for M8.

### Dimensionality

Fixed from the first `/v1/embeddings` response and stored in the sqlite-vec
schema (`meta.rag_dim`) — already implemented in `shared/storage/db.rs`.
Switching the embedding model to a different dimensionality requires
reindexing (out of scope for M5).

### Testability

For RAG tool unit tests, a deterministic in-process double `MockEmbedder`
(bag-of-chars + L2 normalization) is used — this is **not** the production
path, but a test substitution via `Arc<dyn Embedder>`.

## Consequences

- Plus: better search quality, isolation from the chat model (can be changed
  independently).
- Minus: a second process and memory; embedding model/port configuration (env
  in M5, settings section — M8).
- Transport (`XinferClient::embed`, `/v1/embeddings`) has already been ready
  since M1.
