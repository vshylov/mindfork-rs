# Generated screenshots

Every image here is rendered **from code**, never captured from a live
session — so no private data can leak into them, and they cannot silently rot
as the app evolves. The pipeline is described in
[docs/demo-screenshots.md](../../docs/demo-screenshots.md):

1. `src/features/demo.rs` — a fixed showcase conversation, chat list, config
   and self-model (fixed ids and timestamps: captures are byte-stable);
2. `cargo test dump_demo_frames -- --ignored` — renders the screens headlessly
   and (re)writes the JSON frame dumps under `dumps/`;
3. `python tools/screenshots.py` — rasters the dumps into the PNGs here
   (JetBrains Mono with symbol-font fallbacks; `pip install pillow fonttools`).

A drift gate (`app::demo_shots::tests::committed_dumps_match_the_code`) fails
the ordinary test suite when the committed dumps stop matching what the
current code renders. When it goes red, run the two commands above and commit
the regenerated dumps **and** images together with the change that moved
them.

The set (per the design plan's confirmed forks): the chat hero shot, the chat
list, settings twice — Model/server and Tools — and the self-model screen,
each in Dark and Light themes, English UI.
