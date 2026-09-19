+++
title = "Where vector search earns its keep"
description = "One small embedding server powers the knowledge base, large attachments, notes and memory — and the app notices when you swap the model under it."
updated = "2026-08-15"
weight = 4

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# explainer revised later must not arrive there as fresh. `page.html`
# reads it for the byline and for schema.org `datePublished`.
published = "2026-08-15"
+++

*The fourth in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/) and
[the self-model](/articles/self-model/).*

## Search by meaning

An embedding model turns a piece of text into a vector — a long list of
numbers — so that texts with similar *meaning* land close together, even
when they share no words. Ask about «деплой на сервер» <!-- cyrillic-ok -->
and a chunk about "rolling out to production" is nearby. That one trick, **search by
meaning instead of by substring**, is what this article is about: where
mindfork uses it, and what it took to make it boring and reliable.

## A second, smaller server

mindfork never asks the chat model for embeddings. An autoregressive
chat model embeds poorly; a dedicated embedding model — the reference
here is **bge-m3**, multilingual and comfortably good in Russian — does
it well and runs in a fraction of the memory. So embeddings come from a
separate process on its own port, speaking the same OpenAI-compatible
dialect as [the engine](/articles/engine-as-a-server/):

```
llama-server -m bge-m3-Q8_0.gguf --embeddings \
    --host 0.0.0.0 --port 8001 -ngl 99 -c 8192 -ub 8192 -b 8192
```

The `-ub`/`-b` flags matter: embedding models read their whole input in
one physical batch, and llama-server's default of 512 tokens makes long
chunks fail outright. In **managed** mode — the Embeddings tab of the
settings, point it at the binary and the GGUF — the app launches the
server with the right flags itself. External servers and the cloud
providers with an embeddings endpoint work too; a chat on Claude or
Grok simply pairs with a local embedder, since those clouds offer none.

## What it powers

**The knowledge base.** `/rag add <file-or-folder>` ingests txt,
Markdown, HTML, PDF and DOCX into a per-profile library; the model
searches it with `rag_search`. Files are cut into overlapping chunks of
roughly 800 characters — *characters*, not bytes, so Cyrillic text is
measured honestly — with Markdown split along its headings, each chunk
carrying its heading as a semantic anchor. At search time the overlap
is sewn back: adjacent hits from one document merge into a single
contiguous passage, so the model never sees the duplication the index
pays for.

**Large attachments.** `/file attach` puts a file into the current chat
in full — no embedder needed. But a file too big to inline switches to
by-reference: it is indexed in the background, and the model gets two
complementary tools. `attachment_search` finds the relevant passages;
`attachment_read` walks the file page by page — pages, not offsets,
because pages are enumerable, and the model can *know* it has read
everything. Retrieval is the shortcut; paging is the guarantee.

**Notes and the self-model.** Saved notes are embedded, so `note_recall`
finds them by meaning. The same vectors power the quieter machinery:
before saving a note or an observation, close existing ones are shown
back — revise, don't duplicate; and each turn, [the
self-model](/articles/self-model/) surfaces the observations relevant
to what you just said, not merely the newest ones.

**Web search**, finally, reranks results by how close each page's
content sits to your query.

One thing deliberately *not* on this list: cross-chat and history
search run on a plain full-text index. An embedding server is optional
equipment, and exact identifiers — the first thing a summary loses —
are precisely what substring search is better at.

## Swap the model, and the app notices

The uncomfortable property of embeddings is that vectors from different
models are mutually meaningless — and two models can share a dimension,
so no size check catches a swap. mindfork detects it *behaviourally*:
it stores the vector of a fixed canary phrase and re-embeds it on the
next run. The same model scores 1.0 against its stored vector; a
different one lands far away, whatever its dimension.

On a detected change nothing is deleted. Every stored vector is stamped
with a generation number; one counter bump retires them all while the
text stays put. Notes quietly re-embed themselves on the next search.
The knowledge base — too large to heal on a read path — refuses to
search and asks for **`/reindex`**, one command that re-embeds
everything, every profile, from the text already stored, resumable if
interrupted. Refusing beats warning: results against a foreign index
would be noise dressed up as answers.

The similarity thresholds move with the model, too. A cut-off tuned on
bge-m3 sits *below* the unrelated-text baseline of some other models,
so the app measures each new embedder against a small built-in probe
corpus and rescales its thresholds — and any failure in that
calibration degrades to changing nothing.

## If there is no embedder

Everything degrades, nothing breaks. Notes fall back to substring
search, attachments to page reading, web results keep the provider's
order, and the knowledge base says plainly that it needs an embedding
server. The status bar shows a small `emb` chip with the truth. You can
run mindfork for months without an embedder and never hit an error —
you are just choosing to search by letters instead of meaning.

And as with everything else in mindfork, the vectors live in the app's
own SQLite database (via sqlite-vec), per profile, next to the binary.
Your library is indexed on your disk, by a model on your machine.

The decision records — ADR 0002 on the dedicated embedding server, the
RAG journal with every measurement — live in the repository:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).
