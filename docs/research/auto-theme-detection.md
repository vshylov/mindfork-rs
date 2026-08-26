# `Theme::Auto` — does it detect anything, and could it?

Pre-decision research. The fork at the end is **not** settled; §6 records what is
still missing before it can be.

## 1. The complaint

In a JupyterLab terminal (xterm.js inheriting the light lab theme, so a white
background) the default theme renders dark "keycap" pills and dark-tuned syntax
highlighting on white. Reported as "Auto looks bad in JupyterLab" while building
the containerised test environment
([docker-jupyter-env.md](docker-jupyter-env.md)). That environment works around
it by seeding `interface.theme = "light"` (`docker/lab/settings.seed.json`, knob
`LAB_THEME`), which fixes the container and nobody else.

## 2. What `Auto` actually does

`Theme::Auto` (`src/shared/config.rs`) is documented as "Follow the system
setting (default)". `Palette::auto()` (`src/shared/theme.rs`) detects nothing.

It genuinely adapts for most roles — they are named ANSI colours, whose shades
the terminal chooses, plus `text: Color::Reset`. Two things are not:

- **`dark: true`**, a literal. The doc comment on the field argues the
  assumption openly ("`Auto` counts as dark (a typical terminal is dark; that's
  how it was before themes)").
- **`keycap_fg` / `keycap_bg` / `keycap_danger`**, absolute dark RGB, with an
  in-code comment acknowledging "Auto is already counted as a dark theme".

### 2.1 Where `dark` is read

- `build_code_theme` (`src/shared/markdown/code.rs`) picks the base16 greys for
  default text and comments from it, so **every code block in the feed** is
  dark-tuned under `Auto`.
- `src/shared/shot.rs` branches on it for the screenshot canvas — moot, `Auto`
  is never captured, and its own comment says so.

So the `dark` flag has exactly **one** live consumer.

### 2.2 Where the keycap colours are read — wider than the name suggests

`keycap_bg` is documented as "keycap background in the hotkey line", but it is
the **selection backdrop across the app**:

| Surface | Site |
|---|---|
| chat list rows | `src/widgets/chat_list.rs` |
| search results | `src/screens/search.rs` |
| self-model screen | `src/screens/self_model.rs` |
| settings rows and fields | `src/screens/settings/render.rs`, `helpers.rs` |
| emoji picker cells | `src/widgets/emoji_picker.rs` |
| help dialog tabs and leaders | `src/widgets/help_dialog.rs` |
| chat popups | `src/screens/chat/popups.rs` |
| markdown keycap spans | `src/shared/markdown/mod.rs` |

On a light terminal that is not "dark pills in the hint line" — it is **every
selected row rendering as a dark bar on white**. This is the loudest half of the
complaint, and it is independent of `dark`.

### 2.3 It is a drift from `Auto`'s own stated design

The TUI-redesign entry (`docs/journal/ui-feed.md`, "Post-M9: TUI redesign")
describes the theme as: "**Auto** (default) — named ANSI (adapts to the
terminal) + **neutral grays** for structure". `Color::Rgb(36, 39, 45)` is not a
neutral grey. Whatever is decided about detection, the keycap values are a
regression against what `Auto` was specified to be.

## 3. Could `Auto` detect? — the protocol

OSC 11 asks the terminal for its background: write `ESC ] 11 ; ? BEL`, read back
`ESC ] 11 ; rgb:RRRR/GGGG/BBBB` terminated by BEL or ST. A multiplexer needs the
query wrapped to reach the terminal it is itself inside (tmux `ESC P tmux; …`,
screen `ESC P … ESC \`).

The project has been here before. OSC 52 was specified, widely implemented, and
**dropped entirely by JupyterLab**
([osc52-clipboard.md](../history/osc52-clipboard.md)) — which is why this
document measures instead of citing support tables.

`tools/osc11_probe.py` is the measuring instrument: it writes the query, reads
the reply under a deadline, parses every component width the specification
allows, and reports luminance and a verdict. It refuses to run without a real
terminal on both ends, and it consumes **nothing** that is not plainly an OSC
reply — a terminal that stays silent leaves the user's type-ahead untouched
(covered by `--self-test`, which needs no terminal).

## 4. Measurement — JupyterLab

The host the complaint came from, measured in the project's own lab image
(`mindfork-lab:local`, JupyterLab 4.6.3 / Notebook 7.6.2, `LAB_THEME=light`), a
real browser attached to a real pty.

**JupyterLab answers, and answers correctly.**

```
raw reply : ESC]11;rgb:ffff/ffff/ffffESC\
parsed    : #ffffff   relative luminance 1.0000   verdict LIGHT
OSC 10    : ESC]10;rgb:0000/0000/0000ESC\  (foreground black — consistent)
```

Latency, three consecutive queries in one session:

| Attempt | Elapsed |
|---|---|
| 1 (cold) | **382.4 ms** |
| 2 | 34.0 ms |
| 3 | 1.1 ms |

The reply is **ST-terminated**, not BEL-terminated, even though the query used
BEL — a parser that only accepts BEL would read this host as silent.

**The cold-start number is the design constraint.** The reply travels to the
browser over a websocket and back; 382 ms on the first query means a naive
"query at startup, wait 100 ms" would time out in exactly the host that
motivated the work, then fall back to dark, and look like nothing had changed.

Corroboration from the shipped bundle (`/opt/conda/share/jupyter/lab/static/`),
so the behaviour is not an accident of this run — xterm.js registers the handler
and reports on `?`:

```js
this._parser.registerOscHandler(11, new OscHandler(e => this.setOrReportBgColor(e)))
_setOrReportSpecialColor(e, t) { … if ("?" === i[e]) this._onColor.fire([{type: 0, …}]) … }
// and the reply builder:
case 257: e = "background", i = "11" …
coreService.triggerDataEvent(`${ESC}]${i};${toRgbString(s)}${C1_ESCAPED.ST}`)
```

`toRgbString` defaults to 16 bits per component — the `rgb:ffff/ffff/ffff` form
observed. The colour is taken from xterm.js's **theme**, which in JupyterLab
follows the lab theme, so the reported background is the one actually on screen.

### 4.1 A false negative worth recording

The first run of this measurement reported `no-answer` on all four queries. It
was wrong: no browser was attached to the pty, so there was no emulator to
answer. The server log (no `terminals/websocket/N` connection) is what caught
it.

The lesson generalises past this spike: **a silent terminal and an absent
terminal are indistinguishable to the probe**, and a detection feature will hit
the same ambiguity whenever the app is started somewhere the reply cannot come
back from. Any fallback has to be safe, not merely defaulted.

## 5. Measurement — the host matrix

`python tools/osc11_probe.py --foreground --repeat 3`, each host a real terminal
with a human at it.

| Host | Answers? | Terminator | Reported background | Cold latency | Warm |
|---|---|---|---|---|---|
| Windows Terminal | **yes** | ST | `#0c0c0c` (dark) | 16 ms | 15–16 ms |
| VS Code integrated (1.134.0) | **yes** | ST | `#191a1b` (dark) | 31 ms | 16 ms |
| JupyterLab (xterm.js 4.6.3) | **yes** | ST | `#ffffff` (light) | **382 ms** | 1–34 ms |
| Windows conhost (legacy) | **no** | — | — | timeout | timeout |
| plain SSH into Linux | ? | | | | |
| tmux over SSH | ? | | | | |

**Three of the four hosts answer, and every one of them terminates with ST**
even though the query used BEL. A BEL-only parser would read the entire matrix
as silent.

**conhost is a clean negative, not an artefact.** The probe did not emit its
"no VT input" warning there, so `ENABLE_VIRTUAL_TERMINAL_INPUT` *was* accepted
and conhost simply does not implement the report. The distinction matters: there
is no console-mode trick that would make it answer.

### 5.1 What the numbers imply for a timeout

Local hosts answer in 15–31 ms; JupyterLab needs **382 ms** on the first query
because the reply round-trips through a websocket to the browser. So a single
timeout has to be ~400 ms to cover the host that motivated the work — and
conhost then pays that ~400 ms as dead startup latency on every launch, having
never intended to answer.

The way out is not a shorter timeout but **not blocking on it**: emit the query
as the very first thing the process does, get on with loading config, storage
and profiles, and harvest the reply just before the first palette is built.
Startup already does more than 400 ms of work, so the wait overlaps with it and
costs approximately nothing — on conhost included. This is a design constraint
worth carrying into the design doc, because the naive "query, sleep, read"
shape fails exactly where it is needed.

### 5.2 The fallback is safe where it fires

The one host that stays silent is a host whose background is dark
(conhost's default is `#0c0c0c`/black). So "assume dark when nothing answers"
is not merely a default — on the measured evidence it is the *right* answer for
the case where it fires. Detection succeeds on the light terminal (JupyterLab)
and fails only where the existing assumption already held.

## 6. The fork — open

**Option 1 — make `Auto` detect.** Query OSC 11 at startup, set `dark` from the
reply's luminance, and pick the keycap/selection colours per polarity. Then
`Auto` means what its label claims, the redesign's look is preserved in **both**
polarities, and §2.2 and §2.1 are fixed by the same mechanism.

Costs and open questions: the query must run before `ratatui::init()` and read
raw bytes itself (crossterm 0.29 has no OSC-response event); the timeout has to
clear 382 ms cold on the measured host, which is startup latency paid by
everyone; the fallback when nothing answers is still "assume dark", so §2.2
stays broken wherever detection fails; and it touches the theme system and
spec §11.6, so it needs a design doc of its own.

**Option 2 — rename and re-document.** Call `Auto` what it is — terminal-palette
colours, dark-tuned — in `ui.settings.choice.theme_auto` (`locales/*.json`), the
enum doc comment, spec §11.6 and the README. Cheap and honest, and it stops the
label lying. It does not stop the default looking wrong on a light terminal.

**Independent of the fork:** §2.2's keycap/selection values should stop being
absolute dark regardless — they are a regression against §2.3. Note that option 2
forces an unpleasant sub-choice here (a genuinely neutral selection backdrop has
no named-ANSI equivalent, so it would mean reverse video, which the redesign
deliberately moved away from), whereas option 1 dissolves it: detection picks the
existing dark or light values per polarity.

Recommendation, on the evidence so far: **option 1**, gated on §5 coming back
positive for at least Windows Terminal and one Linux host. If the Windows rows
come back silent, most users get the fallback and option 1 buys little — take
option 2 and fix the keycaps neutrally.
