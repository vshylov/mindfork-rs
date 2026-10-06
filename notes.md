**Ollama and LM Studio, found and offered.** Start mindfork with no model set up
and it looks for a local Ollama or LM Studio, lists what they serve, and
connects the one you pick with `Enter` — its embedding model too, when none is
set up. LM Studio joins Ollama as a server mindfork knows: its window is read,
and its silent cut of a long conversation is told.

### Added

- **LM Studio is a server mindfork knows.** It reads the window LM Studio runs
  your model with, so a long conversation is folded into a summary before LM
  Studio cuts it — and LM Studio does cut in silence: a conversation over its
  window loses its whole middle, everything between the first message and the
  last. A conversation it cut anyway is told, with the model's Context Length
  and the `lms` line that raises it.
- **A local Ollama or LM Studio is offered when nothing is connected.** Start
  mindfork with no model set up and it looks for both on your computer: what
  answers is listed, and `Enter` connects it — with that server's embedding
  model too, when none is set up, so notes and the knowledge base search by
  meaning from the first message. `/local` looks again whenever you type it.

### Changed

- **The notes about a server's window speak of that server.** Ollama's name
  `OLLAMA_CONTEXT_LENGTH` and its app's slider, LM Studio's its Context Length,
  and those of any other server name no product, where every one named Ollama's.

---

[mindfork.io](https://mindfork.io) · [Install guide](https://github.com/vshylov/mindfork-rs/blob/v0.17.0/docs/install.md)
