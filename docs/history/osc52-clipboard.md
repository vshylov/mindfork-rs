# OSC 52 — copying to the *client's* clipboard

Status: **research — forks pending the user's decision.**
Date: 2026-08-15.

## 1. What and why

Copying (`Ctrl+C`/`Ctrl+X` on a selection, `F5`/`/copy` on a whole conversation)
writes through `arboard` — the clipboard of the machine the **process** runs on.
When the terminal is somewhere else, that is the wrong machine, or no machine at
all:

- **SSH into a headless box.** There is no X11/Wayland there, so `arboard`'s
  constructor fails outright and the copy reports an error. This is the case the
  code already names in a comment (`clipboard.rs::write_clipboard`) and the one
  users hit most.
- **VS Code Remote-SSH / devcontainers.** The pty is remote, so a copy lands on
  a clipboard the user cannot paste from.

OSC 52 is the terminal escape that hands text to the **client's** clipboard —
the machine with the keyboard. It travels the same pipe the drawing does, so it
crosses SSH, containers and web sockets without any of them knowing.

Raised as a roadmap item while designing the typed routes
([command-only-control.md](../history/command-only-control.md) §2), deliberately
left out of that track: it is a transport question, not a keyboard one.

## 2. The premise the roadmap item got wrong

That item was written with **JupyterLab** as the motivating case. Measured, the
motivation does not hold there, and the correction reshapes the feature.

**JupyterLab does not support OSC 52.** Its terminal embeds xterm.js, and
xterm.js *ignores* OSC 52 unless the host application registers a handler — that
is what `@xterm/addon-clipboard` is for. JupyterLab's terminal package
(`packages/terminal/package.json`, `main` as of 2026-08-15) depends on
`addon-fit`, `addon-search`, `addon-web-links` and `addon-webgl`; there is no
clipboard addon, and `widget.ts` registers OSC 8 (hyperlinks) and nothing else.
So the pixels arrive, the escape is dropped.

**VS Code does support it** — `xtermTerminal.ts` loads `@xterm/addon-clipboard`
unconditionally and wires both directions to its own clipboard service (v1.93+
per the compatibility matrix). But VS Code's terminal usually runs the process
**locally**, where `arboard` already writes the right clipboard; it is the
*remote* modes where this matters.

So the beneficiary list inverts: the host that most needed it cannot use it, and
the host that can use it mostly does not need it. What remains is still worth
having — plain SSH is the common case, and there `arboard` does not merely write
the wrong clipboard, it **fails**.

Support elsewhere (from the `can-i-use-terminal` matrix, cross-checked against
the projects' own issues): **yes** — alacritty 0.2.1+, kitty 0.36.2+, konsole
24.12+, mintty 2.6.1+, Windows Terminal 1.2+, wezterm, foot, iTerm2, contour,
hterm, tmux 2.5+ (`set-clipboard on`), zellij 0.31.2+; **opt-in, off by
default** — xterm, st, far2l; **no** — GNOME Terminal, Terminal.app, URxvt,
Termux, MobaXterm, QTerminal. GNOME Terminal is the notable absence: it is the
default on the distributions most likely to be SSH *clients*.

## 3. Constraints the protocol imposes

1. **No acknowledgement.** The sequence is written and that is all; nothing
   comes back. The app cannot know whether the terminal took it, ignored it, or
   is one of the ones that never will. Whatever the feed says afterwards has to
   be true of "we sent it", not of "it is on your clipboard".
2. **A size ceiling, and a low one in practice.** The sequence is capped at
   100 000 bytes in the common implementations, which after the `\e]52;c;`
   header and base64 leaves **74 994 bytes** of text. Real terminals are worse:
   kitty rejected over 6 138 bytes (#3031), tabby fails over ~1 KB (#10925), st
   truncated at 382 bytes before its 2019 fix. A whole-conversation copy passes
   that ceiling easily — the corpus this project measures against holds single
   messages of 38 KB.
3. **tmux and screen intercept it.** tmux forwards it only with
   `set-clipboard on`, and a nested session needs DCS passthrough
   (`\ePtmux;\e…\e\\`). `screen` needs a different wrapper *and* 768-byte
   chunking.
4. **Reading is a different animal.** OSC 52 can also *query* the clipboard, and
   that is how terminals leak clipboards to remote programs — which is why most
   disable it, and why VS Code's own read path goes through its clipboard
   service rather than the wire. We need nothing from it: paste already works
   because the terminal injects text (spec §11.5).

## 4. Design

**One seam.** Both copy paths already funnel through
`app::runtime::clipboard::write_clipboard` — the whole-conversation copy via
`deliver_clipboard`, the selection copy via `handle_key_event`. OSC 52 goes
there and both get it, with no call-site changes.

**Emitting.** A raw escape to `stdout`, between frames, exactly as the app
already writes `SetTitle` at startup (`runtime/mod.rs`). Payload is base64 of
the UTF-8 text; target `c` (the clipboard, not the `p` primary selection).
Written with the same `execute!` the terminal side effects use, so it is
serialized against the drawing rather than racing it.

**Deciding to emit** — the substance of fork F1. The signals available are:
`arboard` failing (a certainty, but only known after trying), and the session
looking remote (`SSH_TTY`/`SSH_CONNECTION` set — VS Code Remote-SSH sets these
too). Terminal support itself is **not** detectable: there is no query that does
not involve the read path we are refusing to use.

**Saying what happened.** Today's note claims `ui.chat.copied` on `arboard`'s
`Ok`. With OSC 52 in play there are three outcomes, and they are not the same
sentence: the local clipboard took it; the local clipboard refused but the
sequence went to the terminal; the text was too large for the sequence and only
the local clipboard has it. Wording follows the project's rule that a message
must close the door (docs/lessons.md §4) — and must not claim a delivery the
protocol cannot confirm.

## 5. Forks

- **F1. When to emit.**
  (a) **Auto: always try `arboard`; additionally emit OSC 52 when the session
  looks remote (`SSH_TTY`/`SSH_CONNECTION`) *or* `arboard` failed.** Locally
  nothing changes and no escape is written; remotely both clipboards get the
  text, which is what a user on a laptop with a remote pty wants. *(recommended)*
  (b) Always emit alongside `arboard`. Simplest rule, no environment sniffing —
  but it writes an escape on every local copy for no gain, and on a terminal
  that renders unknown OSC as text it would litter the TUI.
  (c) Only when `arboard` fails. Cheapest, and covers the headless-SSH case that
  motivated this — but misses VS Code Remote and any remote box that *does* have
  a (useless) X11 clipboard, which is where the wrong-machine copy is silent.
  User's decision: —
- **F2. A setting.** (a) **`interface.clipboard_osc52`: `auto` (F1a) / `always`
  / `off`** — three words, one place, and `off` is the escape hatch for a
  terminal that mis-renders the sequence *(recommended)*; (b) no setting, F1's
  rule is the whole behaviour; (c) a plain boolean (loses "always").
  User's decision: —
- **F3. Over the ceiling.** (a) **Do not emit, and say so** — the note names the
  size, says the local clipboard has it (when it does), and points at the export
  route *(recommended: a silently truncated conversation looks complete, which is
  the worse failure)*; (b) truncate and say so; (c) emit anyway and let the
  terminal decide (it will fail silently, or print the tail as text).
  User's decision: —
- **F4. The note.** (a) **Three wordings for the three outcomes** (local
  clipboard / sent to the terminal / too large) *(recommended)*; (b) keep one
  "copied" for all of them — shorter, but claims a delivery we cannot confirm.
  User's decision: —
- **F5. Multiplexers.** (a) **Wrap in DCS passthrough when `TMUX` is set, and do
  nothing special for `screen`** — tmux is common in exactly the SSH case this
  serves, the wrapper is a few lines; `screen`'s 768-byte chunking is a rabbit
  hole for a shrinking audience, and it degrades to "no clipboard", which is
  today's behaviour *(recommended)*; (b) support both; (c) support neither and
  let `set-clipboard on` be the user's business.
  User's decision: —
- **F6. Which copies.** Both, and for free — the two paths share
  `write_clipboard` *(recommended; the alternative is an artificial split)*.
  User's decision: —
- **Considered and rejected here:** OSC 52 **read** (the leak vector terminals
  disable, and paste already works by injection); querying support before
  sending (the query *is* the read path); an image over OSC 52 (no terminal
  agrees on a format, and `/image paste` already covers the direction that
  matters).

## 6. Test plan

- **Unit**: the sequence's exact bytes for ASCII, multi-byte UTF-8 and an empty
  string; the `TMUX` wrapper applied and not applied; the ceiling honoured at
  the boundary (74 994 in, one byte more out); the decision function over the
  environment matrix (local, `SSH_TTY`, `SSH_CONNECTION`, setting `always`/`off`)
  — a pure function, so it is table-driven; the three note wordings per bundled
  locale, each naming a route.
- **Gate**: no escape is written when the decision says no — asserted on the
  writer's own sink rather than on stdout, which means the emitter takes a
  `Write` (that is also what makes the byte assertions possible).
- **Live**: this is a claim about *terminals*, so the acceptance pass is manual
  and needs a second machine: SSH from a laptop into a headless box, run there,
  `/copy`, and paste locally. Worth repeating under tmux, and once in VS Code
  Remote-SSH. Nothing here touches the engine, memory or tool paths, so no
  live-model smoke is required.

## 7. Scope and documentation

One stage. On completion: spec §11.3/§11.7 (what copying does now), README (the
copy rows), CHANGELOG, `docs/journal/ui-input.md`, `docs/install.md` if the
setting needs a word, and the roadmap item closes.
