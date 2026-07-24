# Research: universal layout-independent hotkeys (beyond the JCUKEN table)

**Status:** decision points R1–R5 (§8) **accepted by the user, following the
recommendations, 2026-07-24**. **Stage 1 (Windows resolver) — done**
(`feat/universal-hotkeys-win`): the `hotkey_char` entry point with the tier
ladder, resolution through the active/installed layouts, call-site sweep.
Stage 2 (static deltas) is deliberately **not** done — the Windows tier removed
the need, and the unix gap is closed by stage 3 rather than by more tables.
Stage 3 (crossterm upstream, tier C) — a separate track, recorded in
[docs/roadmap.md](../roadmap.md).

**Implementation deviations from this document** (deliberate, recorded here):
the static table is not merely a "fallback tier" but is **ordered ahead of the
installed-layout probe** — see §5; a live run showed the resolver also covers
Russian letters on punctuation keys, which the table never had (§5, "found in
the live run").

## 1. Task

`Ctrl+<letter>` shortcuts must work regardless of the active keyboard layout.
Today this is solved by `shared/keys.rs::physical_char` — a hardcoded lookup
table for **one** layout (the standard Russian JCUKEN): a Cyrillic character is
mapped to the Latin letter on the same physical key. Every other non-Latin
layout (Greek, Hebrew, Arabic, Georgian, Armenian, Thai, Bulgarian BDS, …) is
not covered: `physical_char` passes the character through unchanged, the match
against `'q'`/`'l'`/… fails, and the hotkey is dead.

The question researched here: **is there a universal mechanism that covers most
languages without maintaining a table per layout?** Short answer: yes — but it
is two different mechanisms, one per platform, because the data available to a
terminal application differs fundamentally between Windows and unix.

## 2. Current state (inventory)

- **`shared/keys.rs`**: `physical_char(c) -> char` (lowercase + JCUKEN table,
  ~33 entries) and `is_slash_key(c)` (accepts `.` because the physical `/?` key
  under the Russian layout produces `.`). Spec §11.7 documents the mechanism.
- **Call sites** (~10): `screens/chat/input.rs`, `screens/chat/popups.rs`,
  `screens/settings/apply.rs`, `screens/self_model.rs`, `widgets/chat_list.rs`,
  `widgets/input_box.rs` — all of the form
  `match keys::physical_char(c) { 'q' => …, … }` under a Ctrl check. The
  signature is `char -> char`; no call site sees the raw `KeyEvent` beyond the
  char and modifiers.
- **Unix**: `app/runtime/mod.rs` pushes the kitty keyboard protocol at the
  `DISAMBIGUATE_ESCAPE_CODES` level when the terminal supports it (audit
  item 11, for `Shift+Enter`).
- **Windows**: input arrives via the Console API (crossterm reads
  `KEY_EVENT_RECORD`); no terminal protocol is involved.

## 3. How the key event actually reaches us (facts, verified in source)

### 3.1 Windows (crossterm 0.29, `event/sys/windows/parse.rs`)

For `Ctrl+<letter key>` the console delivers `u_char` in the control-code range
(`0x00..=0x1f`). crossterm then calls its `get_char_for_key`: `ToUnicodeEx`
with the key's **virtual key code**, an *empty* modifier state, and a
best-effort "active" layout — so the reported character is the **unmodified
character of that key in the active layout**. Consequences:

- Under a Russian layout, physical `L` + Ctrl is reported as
  `Ctrl+Char('д')` — this is what `physical_char` translates today. <!-- cyrillic-ok -->
- Under a Greek layout the same key is reported as `Ctrl+Char('λ')`, under
  Hebrew as the Hebrew letter, etc. — **the breakage is universal on Windows**,
  only Russian is patched.
- The `KEY_EVENT_RECORD` *contains* `virtual_key_code` and `virtual_scan_code`
  (the physical key!), but crossterm **does not expose them** in `KeyEvent`.
- The "active layout" resolution (`GetForegroundWindow` →
  `GetWindowThreadProcessId` → `GetKeyboardLayout`) works under Windows
  Terminal but fails under conhost, where crossterm falls back to the thread's
  inherited layout (crossterm's own source comment).

Key insight: since the char we receive is the output of
`ToUnicodeEx(vk, layout)`, it can be **inverted** with the mirror API —
`VkKeyScanExW(ch, layout)` returns the virtual key (low byte) + shift state
(high byte), or −1 when the char is not in that layout ([MSDN]). VK codes for
letter keys are the ASCII codes `'A'..='Z'`; `MapVirtualKeyExW(vk,
MAPVK_VK_TO_VSC, layout)` further yields the hardware **scan code**, which is
layout-independent by definition.

### 3.2 Unix, legacy encoding

`Ctrl+<Latin letter>` arrives as a C0 control code (`0x01..=0x1A`), which
crossterm decodes to `Ctrl+Char('a'..'z')`. For a **non-Latin** layout the
behavior is *unspecified by any standard* and terminal-specific:

- **VTE family** (GNOME Terminal, Ptyxis, Xfce Terminal, Tilix): the terminal
  itself falls back to the Latin group and sends the C0 code of the physical
  key (long-standing behavior; historically even reported as a bug "gnome-
  terminal ignores current keyboard layout for ctrl+key"). **Hotkeys work for
  any language with zero effort on our side.** VTE has *not* shipped the kitty
  protocol yet (GNOME/vte#2601 — patches under review as of late 2025), so
  this legacy path is what VTE users actually run.
- **kitty** resolved the same class of problem on its side
  (kovidgoyal/kitty#3082) — but once an application *opts into* the kitty
  protocol (we do), the terminal reports faithful layout codepoints instead,
  and the fallback responsibility moves to the client (§3.3).
- Terminals with neither behavior deliver the bare layout character **without
  the Ctrl modifier** (or nothing) — indistinguishable from typing. **No
  client-side mechanism can fix this case**; only terminal-side fallback or
  the protocol helps.

### 3.3 Unix, kitty keyboard protocol

- With `DISAMBIGUATE_ESCAPE_CODES` (what we push today), the terminal encodes
  `Ctrl+<key>` as `CSI <codepoint>;<mods>u` where `<codepoint>` is the key's
  unshifted character **in the active layout** — i.e. on kitty/foot/wezterm/
  ghostty/alacritty(0.13+)/konsole(22.12+) a Russian-layout `Ctrl+L` reaches us
  as `Ctrl+Char('д')` (JCUKEN table saves it), and a Greek/Hebrew/… one as the <!-- cyrillic-ok -->
  native letter (dead hotkey). Note the irony: on such terminals our
  `DISAMBIGUATE` push *replaces* the terminal-side legacy fallback with
  faithful reporting, so for non-Russian scripts the protocol opt-in is what
  breaks the hotkeys.
- The protocol has the exact cure: progressive enhancement **0b100 "Report
  alternate keys"** extends the encoding to
  `CSI unicode-key-code:shifted-key:base-layout-key;…u`, where the **base
  layout key** is defined as "the key corresponding to the physical key in the
  standard PC-101 key layout" — precisely the value `physical_char` tries to
  reconstruct. The spec's own example is the Cyrillic Ctrl+C case.
- **crossterm 0.29 parses only the shifted key** (and only when SHIFT is held)
  and **ignores the base-layout key** — a known open gap,
  [crossterm-rs/crossterm#968] ("Incomplete kitty progressive enhancement
  support and missing base-layout-key support"), with a proposed but
  unimplemented API (`KeyEvent { shifted_key, base_layout_code, … }`). No PR
  exists. So today, pushing `REPORT_ALTERNATE_KEYS` gains nothing.

### 3.4 Resulting coverage matrix (today)

| Platform / terminal | What we receive for Ctrl+letter, non-Latin layout | Russian | Other scripts |
|---|---|---|---|
| Windows (WT + conhost) | layout char + CONTROL | works (JCUKEN) | **broken** |
| unix, VTE family (legacy) | C0 of physical key (terminal fallback) | works | works |
| unix, kitty-protocol terminals | layout char + CONTROL (we push DISAMBIGUATE) | works (JCUKEN) | **broken** |
| unix, no fallback, no protocol | bare char, no CONTROL | broken | broken (unfixable client-side) |

## 4. Solution space

### A. Per-language static tables (extend the JCUKEN approach)

Add tables for Ukrainian, Belarusian, Greek, Hebrew, … Pros: no OS calls, works
identically everywhere the char+CONTROL arrives. Cons:

- **Variant ambiguity is fundamental**: one script ≠ one layout. Bulgarian has
  BDS *and* Phonetic (completely different geometries); Serbian, Macedonian,
  Armenian (Eastern/Western phonetic), Arabic (101/102/AZERTY variants) all
  differ. A char-keyed static table cannot know which national variant the
  user has — the OS/terminal does.
- Unbounded maintenance; every table is a new source of subtle bugs.
- Misses Latin-script-but-non-ASCII cases entirely (Turkish dotless `ı` on the
  I key: not in any Cyrillic-style table, not ASCII either).

Verdict: acceptable only as a **fallback tier**, not as the mechanism.
(Notable free win: the existing JCUKEN table already covers Kazakh, Kyrgyz,
Tatar, Bashkir… — their letter zone matches Russian; the extra national letters
sit on the digit row, which hotkeys don't use.)

### B. Windows: invert the layout with `VkKeyScanExW` (universal by construction)

Pipeline for a non-ASCII `c` reported with CONTROL:

```text
c ──VkKeyScanExW(c, hkl)──> VK (low byte; −1 = not in this layout)
  ──MapVirtualKeyExW(VK, MAPVK_VK_TO_VSC, hkl)──> scan code (physical key)
  ──static Set-1 scan-code → QWERTY table (~47 entries, hardware-standard)──> char
```

- **Candidate layouts** (`hkl`): try the foreground-window layout first (the
  same resolution crossterm itself used to *produce* the char — consistent
  round trip under Windows Terminal), then iterate `GetKeyboardLayoutList`
  (all installed layouts). Iterating installed layouts makes the lookup
  independent of knowing the true active layout — which also repairs the
  conhost case where the active layout is undeterminable: the char itself
  tells us which installed layout it belongs to. Cross-layout ambiguity (same
  char on different keys in two *installed* layouts, e.g. Russian + Bulgarian
  BDS both installed) is resolved by the active-first ordering and is the only
  residual imprecision.
- **ASCII passes through untouched** (as today). This preserves the label
  semantics for Latin layouts: on AZERTY/QWERTZ/Dvorak/Colemak, `Ctrl+Z` is
  the key *labeled* Z wherever it sits — matching user expectation and current
  behavior. Non-Latin layouts have no Latin labels; their keycaps carry QWERTY
  sublabels by position, which is exactly what the scan-code stage returns.
- **Why the scan-code stage** (not just `VK as char`): for letters, non-Latin
  layouts conventionally keep the US VK grid (`VkKeyScanExW('д', ru)` → <!-- cyrillic-ok -->
  `VK_L`), so VK alone would usually suffice — but the VK assignment is up to
  the layout author, while the scan code is hardware truth. The static
  VSC→QWERTY table is a *single fixed table for all languages forever* — the
  entire point of the exercise. It also handles punctuation keys (`VK_OEM_*`)
  uniformly, which VK-to-ASCII cannot.
- Edge cases: char not in any installed layout / produced by dead-key
  composition / ligature / non-BMP → `VkKeyScanExW` returns −1 → fall back to
  the static table (tier A). Numpad translations are ignored by the API by
  design (main-keyboard section only) — fine for hotkeys.
- Cost: 2 user32 calls per Ctrl keypress (pure table lookups, µs); caching
  optional and probably unnecessary.
- Dependencies: `windows-sys` is already a direct dependency
  (sandbox Job Object, DPAPI); add features `Win32_UI_Input_KeyboardAndMouse`
  (VkKeyScanExW, MapVirtualKeyExW, GetKeyboardLayout, GetKeyboardLayoutList)
  and `Win32_UI_WindowsAndMessaging` (GetForegroundWindow,
  GetWindowThreadProcessId). FSD: stays in `shared/keys.rs` behind
  `#[cfg(windows)]` — precedent `shared/sandbox.rs`/`shared/secrets.rs`.

This covers **every layout installed on the machine, including ones that don't
exist yet**, with zero per-language data. It is the same trick crossterm's own
Windows backend uses, run in reverse.

### C. Unix: kitty protocol `REPORT_ALTERNATE_KEYS` + upstream crossterm work

The protocol already ships the exact answer (base-layout key, §3.3); the gap is
purely in crossterm's parser/API ([crossterm-rs/crossterm#968]):

1. Contribute to crossterm: parse the second alternate
   (`unicode-key-code:shifted:base-layout`) and expose it — the issue's own
   proposal is `KeyEvent.base_layout_code: Option<KeyCode>` (no change to
   existing `code` semantics). Project precedent for upstream contributions:
   mermaid-text #29/#30 → 0.56.1, feature request #32 → 0.57.0.
2. After the release lands: push `DISAMBIGUATE_ESCAPE_CODES |
   REPORT_ALTERNATE_KEYS` in `app/runtime/mod.rs` and prefer
   `base_layout_code` when present.
3. Until then, the static tier (A) remains the unix fallback on
   kitty-protocol terminals; VTE-family terminals need nothing (§3.2).

Side effect to assess when enabling the flag: crossterm's *existing* handling
substitutes the shifted char and strips SHIFT when SHIFT is held (e.g.
`Shift+2` → `Char('@')`, no SHIFT). Audit of our consumers says this is
harmless: text input inserts the char as-is; `Shift+Enter`/`Shift`+arrows are
functional keys (no alternates sent); the input batcher ignores SHIFT. Must be
re-verified at upgrade time.

Note: crossterm on Windows reads the Console API, not escape sequences — so
kitty-protocol support in Windows Terminal (whenever it ships) changes nothing
for us there; B is the Windows mechanism, C is the unix mechanism. They meet in
the same `shared/keys.rs` entry point.

### D. Rejected approaches

- **xkbcommon reverse lookup on unix**: a PTY application cannot reliably know
  the active keymap/group — X11 requires a display connection (breaks over
  SSH/Wayland-native), Wayland gives the keymap to the *compositor's* client
  (the terminal), not to us. Heavy dependency, fragile env guessing.
- **Reading `/dev/input`**: privileged, bypasses the terminal, absurd for a
  TUI.
- **Forking/vendoring crossterm**: the project deliberately avoids forks;
  upstream-first worked before (mermaid-text).
- **`REPORT_ALL_KEYS_AS_ESCAPE_CODES`**: heavier protocol level, still doesn't
  expose base-layout through crossterm today, and would change text-input
  encoding; not needed — alternates ride on level 0b100.

## 5. Proposed target design

`shared/keys.rs` keeps its role as the single entry point:

```text
hotkey_char(&KeyEvent) -> Option<char>   // the one call sites use; None = not a Char key
  0. ASCII short-circuit (label semantics on Latin layouts, no OS calls)
  1. (windows) VkKeyScanExW pipeline against the ACTIVE layout       — tier B
  2. (everywhere) static JCUKEN table                                — tier A
  3. (windows) VkKeyScanExW pipeline against INSTALLED layouts       — tier B
  4. pass-through lowercase (current behavior)
```

**Why the table sits between the two Windows tiers** (a deviation from the
first sketch, which put all of tier B ahead of tier A): the active layout is
the *exact* inverse of the transform that produced the character, so it is
authoritative — it beats the table and handles non-standard geometries such as
Russian Typewriter. But when the active layout is undeterminable (conhost,
§3.1), probing installed layouts is a *guess*: on a machine with both Russian
and Serbian installed, the same Cyrillic letter sits on different keys, and the
guess could pick the wrong one — regressing today's Russian users, which is the
population we actually have. Putting the known-good table between the two makes
the change strictly non-regressive: Windows gains every language, and the
existing behavior is preserved wherever the table already had an answer.

**Found in the live run:** the resolver also fixes characters the table never
covered *within* Russian — `ю`/`ж`/`э`/`б`/`х` sit on punctuation keys <!-- cyrillic-ok -->
(`.`/`;`/`'`/`,`/`[`), outside the 26 letter positions the table encodes. Those
`Ctrl` combos were dead on Windows before and now resolve.

The `KeyEvent`-shaped signature (rather than `char -> char`) is what makes the
future unix upgrade a one-file change: tier C's base-layout key arrives as a
field on the event, so call sites never see it.

Introducing `hotkey_char(&KeyEvent)` in stage 1 makes the later unix upgrade a
one-file change instead of a second sweep over ~10 call sites. All tiers are
behavior-compatible: ASCII letters/digits are never remapped (label semantics
for Latin layouts preserved), so no existing test moves.

**`is_slash_key`** (search opener, a *bare* keypress — no Ctrl): stays as-is
(`'/' | '.'`). The bare-key case cannot use tier B blindly: the Russian
physical `/?` key produces ASCII `'.'`, and ASCII must not be positionally
remapped (a German QWERTZ `'-'` sits on the US `/?` position and must NOT open
search — the label rule). Resolving `.`-vs-slash properly needs to know whether
the *active* layout is non-Latin — doable on Windows (foreground layout), free
on unix under tier C (base-layout of the `.` key is `/` only when it really is
the slash key). Optional refinement, not part of the core.

**Testing**: the VSC→QWERTY table and tier-A/4 logic are pure (unit tests as
today). The Windows OS-call path cannot assume specific layouts are installed
on CI runners — cover it with a thin seam: the pure part
(`vk/vsc → char` tables, candidate ordering) unit-tested; the two user32 calls
exercised by `#[cfg(windows)]` tests that only assert on layouts actually
present (skip otherwise), plus a manual live check under a real Russian/other
layout (the only true verification, as with all terminal-behavior work).

## 6. Language coverage after B + C

| Layout family | Windows (tier B) | unix protocol terminals (tier C) | unix VTE legacy |
|---|---|---|---|
| Russian + Kazakh/Kyrgyz/Tatar/… (JCUKEN zone) | ✓ (already ✓ via A) | ✓ (already ✓ via A) | ✓ (terminal) |
| Ukrainian, Belarusian | ✓ | ✓ | ✓ |
| Bulgarian (BDS *and* Phonetic), Serbian, Macedonian | ✓ | ✓ | ✓ |
| Greek | ✓ | ✓ | ✓ |
| Hebrew, Arabic (all variants), Persian | ✓ | ✓ | ✓ |
| Georgian, Armenian (both phonetic variants) | ✓ | ✓ | ✓ |
| Thai, Devanagari (InScript), … | ✓ | ✓ | ✓ |
| Turkish dotless `ı`, other Latin-non-ASCII oddities | ✓ | ✓ | ✓ |
| CJK under IME | n/a — IME intercepts; Ctrl+key reaches the console as Latin already | same | same |
| AZERTY/QWERTZ/Dvorak/Colemak (Latin) | unchanged label semantics (pass-through) | unchanged | unchanged |

The only populations left uncovered: unix terminals with neither the protocol
nor terminal-side fallback (§3.4 last row — unfixable client-side), and
kitty-protocol unix users of non-Russian scripts **until** the crossterm
contribution lands (interim: tier A deltas per R3).

## 7. Staging

1. **Stage 1 — Windows resolver** (`feat/universal-hotkeys-win`) — **done**:
   tier B in `shared/keys.rs` (+`hotkey_char` entry point, call-site sweep),
   windows-sys features, tests. Removes the Windows dependency on per-language
   tables entirely; JCUKEN stays as fallback.
2. **Stage 2 — static deltas (optional, per R3)** — **dropped**: on Windows
   tier B covers Ukrainian/Belarusian already, and on unix the deltas would
   only serve kitty-protocol terminals until stage 3 lands — not worth new
   hand-maintained data with its variant-ambiguity risk (§4A).
3. **Stage 3 — crossterm upstream** (`spike` + PR): parse + expose
   base-layout key per #968; after release — bump, push
   `REPORT_ALTERNATE_KEYS`, prefer `base_layout_code`. Timing depends on
   upstream cadence; tracked in [docs/roadmap.md](../roadmap.md).

## 8. Decision points

**All accepted per the recommendations (user's decision, 2026-07-24.)**

- **R1 — adopt tier B (Windows `VkKeyScanExW` pipeline)?** Recommended: yes.
  Universal by construction, no new dependencies, small surface (~100 lines +
  one static table), the highest-impact platform (Windows is the primary dev/
  user platform and currently Russian-only).
- **R2 — pursue the crossterm contribution (tier C)?** Recommended: yes, as a
  separate track after stage 1; upstream-first has precedent and fixes the
  whole Rust TUI ecosystem, not just us. Until it lands, unix non-Russian
  scripts on kitty-protocol terminals stay on tier A.
- **R3 — extend static tables meanwhile?** Recommended: only
  Ukrainian/Belarusian (a handful of letter deltas on the same geometry,
  near-zero risk); skip Greek/Hebrew/… (variant ambiguity, and tier B already
  covers them where most users are).
- **R4 — universalize `is_slash_key`?** Recommended: defer (keep the `'.'`
  hack); revisit for free under tier C, optionally on Windows via
  active-layout detection.
- **R5 — caching of the Windows lookup?** Recommended: no (2 µs-scale calls
  per Ctrl press; a cache adds invalidation questions for zero felt gain).

## 9. Sources

- crossterm 0.29.0 sources: `src/event/sys/windows/parse.rs`
  (`get_char_for_key`, `ToUnicodeEx`, foreground-layout comment),
  `src/event/sys/unix/parse.rs` (`parse_csi_u_encoded_key_code` — shifted-only
  alternates), `src/event.rs` (`KeyboardEnhancementFlags`).
- [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
  — alternate keys (`0b100`), "base layout key … standard PC-101", Cyrillic
  Ctrl+C example.
- [crossterm-rs/crossterm#968](https://github.com/crossterm-rs/crossterm/issues/968)
  — missing base-layout-key support, proposed `KeyEvent` fields, open, no PR.
- [kovidgoyal/kitty#3082](https://github.com/kovidgoyal/kitty/issues/3082) —
  Ctrl combos in non-Latin layouts (terminal-side history);
  [kitty mapping docs](https://sw.kovidgoyal.net/kitty/mapping/) —
  `--allow-fallback` to the US-layout key for kitty's own shortcuts.
- [GNOME/vte#2601](https://gitlab.gnome.org/GNOME/vte/-/issues/2601) — kitty
  protocol not shipped in VTE (patches under review);
  [launchpad bug 204202](https://bugs.launchpad.net/bugs/204202) —
  "gnome-terminal ignores current keyboard layout for ctrl+key" (i.e. VTE
  sends the physical key's C0 — the terminal-side fallback we rely on).
- [MSDN: VkKeyScanExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-vkkeyscanexw)
  — return format (VK low byte / shift-state high byte, −1 when absent),
  main-keyboard-only remark;
  [MapVirtualKeyExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-mapvirtualkeyexw),
  [GetKeyboardLayoutList](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardlayoutlist).
- [wezterm/wezterm#3479](https://github.com/wezterm/wezterm/issues/3479) —
  alternate-key reporting quirks in wezterm (relevant at tier-C rollout).
