# Terminal background detection — making `Theme::Auto` adapt

Track design plan. Option 1 of the fork in
[docs/research/auto-theme-detection.md](research/auto-theme-detection.md),
chosen by the user on 2026-08-26. §5 lists the forks that still need a decision
**before** implementation.

## 1. What and why

`Theme::Auto` is the default theme and is documented as "Follow the system
setting". It follows nothing: `Palette::auto()` (`src/shared/theme.rs`)
hard-codes `dark: true` and absolute dark RGB for the keycap roles. Most of the
palette genuinely adapts — named ANSI colours plus `text: Color::Reset` — but
those two do not, and they are enough to make the default theme look wrong on a
light terminal:

- `dark` is read by `build_code_theme` (`src/shared/markdown/code.rs`), so every
  code block in the feed gets dark-tuned greys.
- `keycap_bg` is not confined to the hotkey line despite its doc comment. It is
  the **selection backdrop** in the chat list, search, self-model, settings, the
  emoji picker, the help dialog and chat popups — so on a light terminal every
  selected row renders as a dark bar on white.

This was reported from a JupyterLab terminal while building the containerised
test environment, and worked around there by seeding `interface.theme = "light"`
(`docker/lab/settings.seed.json`, knob `LAB_THEME`), which fixes one container
and nobody else.

The goal of this track: `Auto` asks the terminal what its background is and
picks accordingly, so the label stops lying and the default stops being wrong on
light terminals.

## 2. What was measured

Full matrix and method in the research doc §4–§5. The load-bearing results:

| Host | Answers? | Terminator | Background | Cold | Warm |
|---|---|---|---|---|---|
| Windows Terminal | yes | ST | `#0c0c0c` | 16 ms | 15–16 ms |
| VS Code 1.134.0 | yes | ST | `#191a1b` | 31 ms | 16 ms |
| JupyterLab (xterm.js 4.6.3) | yes | ST | `#ffffff` | 382 ms | 1–34 ms |
| SSH → container, from WT | yes | ST | `#0c0c0c` | 23 ms | 32 ms |
| Windows conhost | **no** | — | — | timeout | timeout |
| tmux 3.4 (unwrapped) | yes | **BEL** | `#0c0c0c` | 0.3 ms | 0.1 ms |

Four facts from that table drive the whole design:

1. **The reply's terminator has to be read as either.** Every host but tmux
   answers with ST though asked with BEL; tmux answers with BEL. Accepting
   only one of the two loses either tmux or everything else.
2. **Local hosts answer in 15–31 ms; JupyterLab needs 382 ms cold.** The reply
   round-trips over a websocket to the browser. (Warm repeats were 1–34 ms, and
   the cold figure came from a browser that had just started, so it is a
   pessimistic bound rather than a typical one — but it is the bound.)
3. **conhost is a clean negative.** The probe accepted
   `ENABLE_VIRTUAL_TERMINAL_INPUT` there and still got nothing, so there is no
   console-mode trick that would make it answer.
4. **The fallback is safe where it fires.** The only silent host has a dark
   background by default, so "assume dark when nothing answers" is the *correct*
   answer for the case that reaches it, not merely a default.

## 3. Constraints

**crossterm does not parse OSC replies.** crossterm 0.29 has no event for them,
so the bytes must be read directly, before crossterm's input machinery is
running. On Windows crossterm reads through the Console API, not VT bytes, which
makes the ordering stricter still (§4.3).

**A silent terminal and an absent terminal are indistinguishable.** The research
doc §4.1 records a false negative caused exactly by this. Whatever the app does
on no-answer has to be safe on its own terms, which fact 4 above says it is.

**The query is written before the UI exists.** Emitting it while the terminal is
in cooked mode would echo the reply into the user's scrollback, so raw mode has
to be on for the whole window between the query and the read.

**Reading may consume type-ahead.** Anything the user typed before the UI came
up sits in the same buffer as the reply. The project already takes the position
that this input has no legitimate consumer and discards it
(`features/terminal_input.rs::discard_type_ahead`, with its rationale); the same
reasoning applies here, and the window is shorter.

**tmux passthrough is one-way.** `ESC P tmux; …` carries the query out to the
outer terminal but the reply returns on tmux's own input and is consumed there.
So the app sends the query **unwrapped** and lets tmux answer for itself — which
it does, in 0.1–0.3 ms (§7).

## 4. Design

### 4.1 A new `shared/osc11.rs`, shaped like `shared/osc52.rs`

`osc52.rs` sets the precedent worth copying: everything pure but for two
environment reads, the sequence built and handed back, the caller doing the IO —
"that is what lets the tests assert the exact bytes". Same split here:

```rust
pub const QUERY_BG: &str;                       // ESC ] 11 ; ? BEL
pub fn parse_reply(raw: &[u8]) -> Option<Rgb>;  // rgb:, 1–4 digits/component, and #rrggbb
pub fn relative_luminance(rgb: Rgb) -> f32;     // WCAG, sRGB linearised
pub fn is_dark(rgb: Rgb) -> bool;               // luminance < DARK_THRESHOLD
```

The parsing and luminance halves are then unit-testable with no terminal at all,
against the replies actually recorded in §2 plus the component widths the
specification allows. `tools/osc11_probe.py` already carries that corpus and its
`--self-test`; the Rust tests take the same cases so both agree.

The IO half is a thin two-phase pair (§4.2), platform-split (§4.3).

### 4.2 Emit early, harvest late

A synchronous "query, wait, read" pays the full timeout on conhost at every
launch. Splitting it costs nothing and hides the wait behind work that has to
happen anyway:

| Phase | Where | What |
|---|---|---|
| emit | `real_main`, after `logging::init` and before `Storage::open` (`src/main.rs`) | put the terminal in raw mode, write the query, return a guard |
| — | | `Storage::open`, migrations, chat index, orchestrator spawn |
| harvest | `app::runtime::run`, after `ratatui::init()` and before the first `Palette` | read the reply if it has arrived, up to the remaining budget |

Storage open plus orchestrator spawn is the heaviest part of startup, so on the
hosts that answer the reply is already waiting by the time the harvest runs, and
on conhost the remaining budget is what is left of it — not the whole of it.

Budget: **500 ms** from emit, chosen to clear JupyterLab's 382 ms cold with
margin. What is actually paid is `500 ms − (time spent opening storage)`, and
zero on every host that answers.

### 4.3 Platform split for the raw read

**unix.** `termios` cbreak with echo off for the window, restored by the guard's
`Drop`. `ratatui::init()` enabling raw mode in between is idempotent.

**Windows.** crossterm reads input through the Console API, so VT bytes only
reach us with `ENABLE_VIRTUAL_TERMINAL_INPUT` set — a mode crossterm does not
expect to find and does not set. Leaving it on across `ratatui::init()` risks
changing how crossterm sees input for the whole session. So on Windows the
exchange is **synchronous and self-contained**, before `ratatui::init()`: set the
mode, query, read, restore. That is safe to keep short because the slow host does
not exist on Windows — measured 16–31 ms, so a **150 ms** budget carries a 5×
margin, and conhost pays 150 ms once at launch.

`windows-sys` is already a dependency; this needs the `Win32_System_Console`
feature added to its existing feature list. No new crate.

### 4.4 Threading the result into the palette

`Palette::for_theme(theme)` is called from many places, including
`screens/settings/render.rs` **per frame**. Rather than thread a parameter
through all of them, the detected polarity lands in a `OnceLock` in
`shared/theme.rs`, set once at startup and read by `Palette::auto()`:

```rust
static DETECTED: OnceLock<Option<Background>> = OnceLock::new();
pub fn set_detected(bg: Option<Background>);   // called once, from startup
```

Every existing call site stays as it is. `Palette` is `Hash` (it keys the
syntect theme cache in `shared/markdown`) — the value is fixed for the life of
the process, so the cache key stays stable within a run and nothing needs
invalidating.

`Palette::auto()` then differs from today in exactly two places:

- `dark` = the detected polarity, defaulting to `true` when nothing was detected;
- `keycap_fg` / `keycap_bg` / `keycap_danger` = **the values `dark()` and
  `light()` already use**, picked by polarity. Not new colours: the point is
  that `Auto` stops inventing an absolute dark constant and reuses whichever
  tuned pair matches the terminal.

That closes both §1 defects with one mechanism, and on a light terminal the
redesign's intended look (a soft backdrop plus a rail, not reverse video) is
preserved rather than compromised.

### 4.5 Honesty in the label and the docs

Option 2's content is still owed even though option 1 was chosen, because
`Auto` follows the **terminal**, not "the system", and because it falls back:

- `Theme::Auto`'s doc comment in `src/shared/config.rs` — what it queries, and
  what it does when nothing answers.
- `ui.settings.choice.theme_auto` in `locales/en.json` and `locales/ru.json`.
- spec §11.6 (the *Appearance* group), and the README's theme mention.
- The `dark` field's doc comment in `theme.rs`, which currently argues for the
  hard-coded assumption.
- `keycap_bg`'s doc comment, which says "hotkey line" and means "every selection
  backdrop" — a live trap independent of this track.

## 5. Forks — settled

**User's decisions, 2026-08-26: F1 (a) minimal, F2 the platform split, F3 the
environment variable only.** All three as recommended below; the reasoning
stands as written and is what the implementation follows.

**F1. On a detected *light* background, how much of the palette changes?**

- **(a) Minimal — recommended.** Only `dark` and the keycap trio. The role
  colours stay named ANSI, so a terminal with a light theme supplies its own
  legible shades. Keeps `Auto`'s contract ("the terminal chooses the colours")
  and keeps `Auto` distinct from `Light`.
- (b) Full — `Auto` on a light background becomes the `Light` palette outright.
  Better tuned for white (the `light()` comment notes bright yellows are
  unreadable there), but it discards the adaptivity that is the point of `Auto`,
  and makes `Auto` and `Light` identical on such a terminal.

Recommendation (a): if a terminal's own ANSI yellow is unreadable on its own
background, that is the terminal's palette, and picking `Light` explicitly is
already the answer.

**F2. Budget and shape.** §4.2/§4.3 propose two-phase with 500 ms on unix and
synchronous 150 ms on Windows. The alternative is one shape everywhere —
simpler to describe, but either conhost pays 500 ms at launch or JupyterLab
loses its detection. Recommendation: the split as written; the asymmetry is
forced by where the slow host lives and by how crossterm reads input.

**F3. Escape hatch.** Recommendation: **an environment variable only** —
`MINDFORK_TERMINAL_BG=dark|light|off` — which forces the outcome or skips the
query. It gives tests and the container a deterministic override and gives a
user with a misbehaving terminal a way out, without adding a settings row for a
mechanism that should be invisible. No new config field; picking `Dark` or
`Light` in settings is already the user-facing opt-out.

## 6. Test plan

**Unit** (no terminal, alongside the code):

- `parse_reply` over the replies recorded in §2 (4-digit `rgb:`, ST-terminated),
  plus 1-, 2- and 3-digit components and `#rrggbb`; and over malformed bodies,
  which must yield `None`.
- `relative_luminance` / `is_dark` on the measured values: `#0c0c0c` → dark,
  `#191a1b` → dark, `#ffffff` → light.
- `Palette::auto()` with detection set to dark, to light, and unset: the `dark`
  flag and the keycap trio follow, and the unset case is byte-identical to
  today's palette. This is the regression guard for the fallback.
- The existing assertion at `theme.rs` (`Palette::for_theme(Theme::Auto).dark`)
  becomes "dark **when nothing was detected**".
- `build_code_theme` picks light greys under a detected-light `Auto`.

**Live** — this is a UI track, so there are no `#[ignore]` engine smokes, but it
does need a real terminal of each kind, which is the whole reason the matrix
exists. Run the app and check the feed's code blocks and a selected list row in:
Windows Terminal, conhost, VS Code, JupyterLab (light **and** dark lab theme),
and over SSH. The lab container from the spike has `mindfork` on `PATH` and an
sshd for exactly this. Outcome recorded in the journal entry.

## 7. Scope, risks and open items

**One PR.** Detection, the palette wiring, the keycap polarity fix and the
label/doc honesty are one change: they share a mechanism, and splitting them
would land a detector that nothing reads. The user's decision on 2026-08-26 was
explicitly that the keycap fix rides with the implementation rather than
becoming its own PR.

**Folded in rather than deferred:** `docker/lab` seeded `interface.theme =
"light"` to work around this bug, and documented it at length in
`docker/.env.example` and `docker/README.md` §5 as "light on purpose, and not a
workaround" — reasoning that detection makes false. Docs that describe behaviour
have to change with the behaviour, or `main` briefly contradicts itself, so the
seed default is now `auto` and the prose says what `auto` does. `LAB_THEME`
stays as the way to pin a polarity. The seed's guard test moves with it, and
deliberately so: the start-up hook substitutes the theme by matching its value
**literally**, so that assertion is what stops the default drifting away from the
`sed` pattern in silence.

**Closed: tmux answers.** Measured after the plan was written: the unwrapped
query gets a reply in **0.1–0.3 ms**, carrying the background of the terminal
the SSH session was opened from. Wrapped queries stay silent, exactly as §3
predicted — passthrough is output-only. So the design's decision to never wrap
is what makes tmux work rather than a limitation it tolerates, and the one
case where the dark fallback would have been *wrong* (a tmux session inside a
light terminal) does not arise.

It also corrected the plan: tmux replies with **BEL**, where every other host
replies with ST. Reading "either terminator" is therefore load-bearing in both
directions, not defensive coding.

**Risk: raw mode across an error path.** Between emit and harvest the terminal
is in raw mode; anything that prints and exits in that window (a `Storage::open`
failure) would render with unreturned carriages. The guard must restore on
`Drop`, and the error paths in `real_main` between the two phases need checking.

**Risk: the cold-latency figure.** 382 ms came from a browser that had just
started. If it is pessimistic, the unix budget is generous and costs nothing on
answering hosts. If some host is slower still, it loses detection and falls back
— visibly wrong only on a light terminal, which is the case the budget was sized
for.
