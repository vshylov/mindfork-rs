# A stress mark is part of the word, not a word boundary

<!-- cyrillic-ok:start — this doc is *about* checking Russian spelling: every
     Cyrillic run below is a word put through the dictionary, a segmenter's
     output, or a measurement's input. Transliterating them would describe a
     different experiment. The prose itself is English. -->

> **Status:** proposal (2026-09-10) — the forks in §5 await the user's
> decision. Reported from the input box: `И́стинно так` is typed, and
> `стинно` is underlined while `И` is not. The cause is one predicate:
> the segmenter's `is_alphabetic`, which a combining mark is not — so
> `U+0301` ends the word it belongs to. The second half of the question,
> `е`/`ё`, was measured and needs nothing; §2.3 says why, with the count.

## 1. Why, precisely

**The mark ends the word.** `segment::words`
([segment.rs:132](../../src/features/spellcheck/segment.rs)) grows a word
while `chars[i].is_alphabetic()`. `U+0301 COMBINING ACUTE ACCENT` — the
sign every Russian source uses for stress, and what `Alt+0769`, Wikipedia,
Wiktionary and any grammar reference put in the text — is category `Mn`,
and `'\u{301}'.is_alphabetic()` is **`false`** (measured). So a stressed
word is cut in two at the mark, and each half is looked up on its own.

Both failure directions follow, and both were measured against the
shipped `ru_RU`/`en_US` pair (`Checker` = the app's rule, correct if any
dictionary accepts it):

| Typed | Words today | Underlined today | Underlined after the fix |
|---|---|---|---|
| `И́стинно так` | `И` `стинно` `так` | `стинно` | — |
| `мо́локо` | `мо` `локо` | `мо`, `локо` (two) | — |
| `за́мок и замо́к` | `за` `мок` `и` `замо` `к` | `замо` | — |
| `по-мо́ему, хорошо́` | `по-мо` `ему` `хорошо` | `по-мо` | — |
| `чуде́сный ве́чер` | `чуде` `сный` `ве` `чер` | `сный`, `ве`, `чер` | — |
| `О́зеро Байка́л` | `О` `зеро` `Байка` `л` | `л` | — |
| `харашо́ напи́сано` | `харашо` `напи` `сано` | `харашо`, `напи`, `сано` | `харашо́` |
| `the café was naïve` (decomposed) | `the` `cafe` `was` `nai` `ve` | `nai`, `ve` | — |

- **Over-flagging** is the visible half: correct words underlined, up to
  three underlines on a two-word phrase, and a one-letter underline under
  `л` in `Байка́л` that names nothing a user could act on.
- **Under-flagging** is the quieter half: when both halves happen to be
  dictionary words the error disappears — `за́мок` passes as `за` + `мок`,
  `хорошо́` as `хорошо` + nothing. The check is not merely noisy on
  stressed text, it is **off** for it, and the last row shows the cost:
  the one genuine typo in `харашо́ напи́сано` is underlined among two
  false ones, so the signal is there but unreadable.

**The suggestions popup inherits it.** `Ctrl+G` on the word under the
cursor goes through the same `segment::words`
([check.rs:66](../../src/features/spellcheck/check.rs)), so it offers
corrections for `стинно` — a fragment the user never wrote — and
"Add to dictionary" would store that fragment.

**Where the marks come from.** Pasted reference text (Wikipedia,
Wiktionary, dictionaries, teaching material), and typed directly by
anyone writing about pronunciation, verse or a name's stress. It is not
an exotic input: it is the ordinary way Russian marks stress in plain
text, and the app's own input box is where such a note gets written.

**Requirements.**

- **R1. A mark does not end a word.** `И́стинно` is one word whose
  character range covers the mark, for the underline and for `Ctrl+G`
  alike.
- **R2. A mark does not fail a word.** A word correct without its stress
  marks is correct with them.
- **R3. A real typo still fails.** `харашо́` is underlined — as the whole
  word, once.
- **R4. Nothing already accepted becomes rejected.** In particular the
  precomposed diacritics `en_GB` carries (`café`, `naïve`, `résumé`) keep
  being answered by their own dictionary entries.

## 2. What exists (inventory), and what was measured

Everything below was measured with `spellbook` 0.4.2 against the three
dictionaries in `dictionaries/` as committed.

### 2.1 The code

- **`segment::words`** ([segment.rs:132](../../src/features/spellcheck/segment.rs)) —
  the letter-run scanner. A word starts at an alphabetic character and
  grows while the next one is alphabetic, or is a connector (`'`, `’`,
  `-`, `‑`) with a letter behind it. Indices are character indices into
  the line. URLs and addresses are skipped whole, ahead of the scan.
- **`SpellChecker::check_word`** ([check.rs:44](../../src/features/spellcheck/check.rs)) —
  the personal set, then `Dictionary::check` on each active dictionary;
  correct if any accepts. `misspellings` maps the rejected words'
  ranges; `misspelled_word_at` finds the one under the cursor; `suggest`
  unions the dictionaries' suggestions; `add_to_personal` inserts and
  appends the word as given.
- **The consumers.** `InputBox::set_misspelled` takes ranges by row and
  `styled_line` ([input_box.rs:1714](../../src/widgets/input_box.rs))
  composes the underline **per character**, merging equal-styled
  neighbours into spans; the popup replaces `[start, end)` wholesale
  ([popups.rs:149](../../src/screens/chat/popups.rs)). Neither needs to
  change: a range that grew by a zero-width character still underlines
  and still replaces the right text.
- **The cursor already agrees.** `wrap::prev_boundary`/`next_boundary`
  move by grapheme cluster (UAX #29) and `width_at` gives a combining
  mark 0 columns — so `←`/`Backspace` already treat `И́` as one unit and
  the cursor already cannot sit between a letter and its mark. The
  segmenter is the only layer that disagrees with the rest of the input
  box.

### 2.2 The dictionaries

- **Not one combining mark** in any of the three `.dic` files: `ru_RU`
  0, `en_US` 0, `en_GB` 0. This is the fact that makes the fix safe in
  one direction: removing marks before a lookup can turn a rejection
  into an acceptance and can never do the reverse, because no entry has
  a mark to lose. `en_GB`'s accented words are **precomposed** (`café` is
  `U+00E9`, not `e` + `U+0301`) and a mark-strip does not touch them
  (R4).
- Stripping answers every stressed word correctly: `И́стинно`, `мо́локо`,
  `за́мок`, `замо́к`, `хорошо́` — all rejected as typed, all accepted
  unmarked. And it rescues no typo: `харашо́` → `харашо` → still
  rejected, `heló` → `helo` → still rejected (R3).
- `spellbook`'s own suggester already answers the marked form
  (`suggest("И́стинно")` → `["Истинно"]`) — an ngram accident, not a
  contract, and moot once the word is accepted before it is suggested
  for.

### 2.3 `е`/`ё` — measured, and it needs nothing

The `ru_RU` list carries **7347** stems spelled with `ё`. For each, the
same stem with `ё` → `е` was put through the dictionary: **0 of 7347**
were rejected. Writing `елка`, `ежик`, `зеленый`, `еще`, `Черемушки` is
therefore always accepted, and so is every other `е`-for-`ё` spelling in
the list — exactly as the user guessed.

The interesting half is that the reverse is **not** ignored: a `ё` where
the word has none is still flagged.

| Word | Verdict | Word | Verdict |
|---|---|---|---|
| `афера` | accepted | `афёра` | **flagged** |
| `опека` | accepted | `опёка` | **flagged** |
| `гренадер` | accepted | `гренадёр` | **flagged** |
| `свёкла` / `свекла` | both accepted | `шофёр` / `шофер` | both accepted |

So the dictionary is already `ё`-aware in the one direction that helps
(never punish the `е` spelling) while still catching the common
hypercorrections (`афёра`, `опёка`). There is nothing to build, and
anything built here — a `ё` → `е` fallback of our own — would only make
`афёра` stop being flagged. Recommendation: leave it alone (fork F5).

One `ё` case *is* broken today, and this track fixes it as a side
effect: `ё` typed as **`е` + `U+0308`** (decomposed, as pasted from a
macOS-authored source) is cut in two exactly like a stress mark. Under
the fix it becomes one word, the strip yields the `е` spelling, and by
the count above the `е` spelling is always accepted.

## 3. Design

Two predicates and one fallback; no new dependency, no new setting.

### 3.1 A mark is in-word (R1)

In `segment.rs`, beside `is_connector`:

```rust
/// A combining mark a letter carries: the Russian stress sign (`U+0301`,
/// `U+0300` for the secondary one), the decomposed halves of `ё`/`й`
/// (`U+0308`/`U+0306`) and the Latin diacritics of a decomposed `café`.
/// Unicode calls these `Mn`, and `char::is_alphabetic` is false for
/// every one of them — which is why a stressed word used to end at its
/// stress. See spec §11.5.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}')
}
```

and the run loop grows on `chars[i].is_alphabetic() || is_mark(chars[i])`.
A word still **starts** only at a letter (the outer guard is unchanged),
so an orphaned mark after a space begins nothing; a trailing mark
(`хорошо́`) is inside the word's range, which is what a grapheme cluster
says it is.

### 3.2 The check falls back to the unmarked word (R2, R3, R4)

```rust
/// The word without its combining marks — `None` when it has none, so the
/// common word allocates nothing.
pub fn strip_marks(word: &str) -> Option<String>
```

and `check_word` becomes: the personal set and the dictionaries as
typed; on failure, the same two on `strip_marks(word)`. **As typed
first** — so a precomposed `café` is answered by `en_GB`'s own entry
rather than by `en_US`'s `cafe` (R4), and so the fallback is reached
only by words that already failed.

### 3.3 Suggestions and the personal dictionary

- `suggest` asks the dictionaries about the stripped form when there is
  one: no entry carries a mark, so the marked spelling has nothing to
  match against. The replacement lands unmarked — a user picking a
  correction is not asking to keep the stress of a word they misspelled
  (fork F4).
- `add_to_personal` stores the stripped form, and the personal lookup in
  `check_word` reaches it through the same fallback. Adding `И́стинно`
  therefore covers `Исти́нно` and every other placement, and the file
  stays plain text a human can edit (fork F3).

### 3.4 What the user sees

`И́стинно так` — nothing underlined. `харашо́ напи́сано` — one underline,
under `харашо́` whole, and `Ctrl+G` on it offers corrections instead of
corrections for `харашо`'s left half. Text without marks behaves exactly
as before: `is_mark` is false for every character in it and `strip_marks`
returns `None` before allocating.

## 4. Difficult spots

- **The range must not split a cluster.** The word now ends *after* its
  trailing mark, so a misspelling range never starts or ends between a
  letter and its mark, and `styled_line` never emits an orphaned mark as
  its own span (a terminal draws one of those on a dotted circle). The
  selection background can in principle cut anywhere, but its endpoints
  come from cursor positions, which already snap to cluster boundaries.
- **Precomposed is not decomposed.** `strip_marks` sees `U+00E9` as one
  character and leaves it alone — deliberately. A Latin acute *inside* a
  Cyrillic word (`мóлоко`, `U+00F3`) stays rejected: that is a homoglyph
  problem, not a mark problem, and a Latin→Cyrillic map is not something
  a spellchecker should guess at (§7).
- **Flattening versus composing.** Stripping turns `е` + `U+0308` into
  `е` rather than into `ё`. For the shipped dictionaries the two land on
  the same verdict (§2.3: every `ё` stem's `е` spelling is accepted), so
  composing buys nothing today — see fork F2 for what it would buy a
  user's own German or French dictionary, and what it would cost.
- **`Mn` without a table.** Rust's `char` exposes no general category, so
  the block range is the whole test. It is the Combining Diacritical
  Marks block and nothing else — one comparison on the hot path (every
  word of every line, behind the 300 ms debounce).
- **The two halves that used to pass.** Removing the split removes a few
  accidental acceptances (`за` + `мок`); this is the point, and it can
  surface a word that had been quietly passing as two.

## 5. Forks

- **F1. What counts as a mark.** (a) **The Combining Diacritical Marks
  block `U+0300–U+036F`** *(recommended — one range covers the stress
  signs, the decomposed `ё`/`й`, and every Latin diacritic)*. (b) Only
  `U+0300`/`U+0301` and their tone-mark twins `U+0340`/`U+0341` — the
  same code, less covered. (c) (a) plus the Cyrillic combining block
  `U+0483–U+0489` (Church Slavonic titlo): nothing in the shipped lists
  is Church Slavonic, so it changes only *which* rejected word is
  underlined.
- **F2. The check's fallback.** (a) **As typed, then with the marks
  removed** *(recommended — additive by the measurement in §2.2, and
  precomposed entries keep answering for themselves)*. (b) Always check
  the stripped form only — identical today (no bundled entry has a
  mark), but it would silently unaccent a user's dictionary that does.
  (c) Compose to NFC first, then strip — a new `unicode-normalization`
  dependency (Unicode tables) for the case of a decomposed `ü` in a
  German dictionary a user added; buys **nothing** for the three shipped
  dictionaries, and Windows/Linux keyboards emit precomposed characters
  anyway.
- **F3. The personal dictionary.** (a) **Store the stripped form; the
  lookup reaches it through the same fallback** *(recommended — one add
  covers every stress placement)*. (b) Store as typed — then only that
  exact placement is ever accepted.
- **F4. Suggestions.** (a) **Suggest on the stripped form; the
  replacement drops the mark** *(recommended)*. (b) Re-apply the mark at
  the same letter index in the replacement — guesswork on a word whose
  letters just changed.
- **F5. `е`/`ё`.** (a) **Nothing** *(recommended — §2.3: 0 of 7347 `ё`
  stems lack an accepted `е` spelling, and a fallback of our own would
  only stop `афёра` and `опёка` being flagged)*. (b) A `ё` → `е`
  fallback beside the mark strip.
- **F6. Staging.** (a) **One PR** *(recommended — two predicates, one
  fallback, tests and docs)*. (b) Split segmentation from the check.

## 6. Tests and the live run

- **`segment.rs`**: `И́стинно так` is two words and the first range covers
  the mark; `по-мо́ему` stays one token; a trailing mark is inside the
  range (`хорошо́`); an orphaned mark after a space starts no word; a
  decomposed `ё` (`е` + `U+0308`) and a decomposed `café` each stay one
  word; text with no marks segments exactly as before (the existing
  cases stand unchanged).
- **`check.rs`**: `И́стинно` accepted, `харашо́` flagged as the whole
  word once; `misspellings` on the reported line returns nothing;
  `misspelled_word_at` inside a stressed word finds the whole word;
  `suggest` on a marked typo returns the same list as on its unmarked
  form; `add_to_personal("И́стинно")` makes `Исти́нно` correct too, and
  the file holds `Истинно`.
- **`dict.rs`**, extending `the_bundled_dictionaries_load_and_answer`
  (line 308) — the guard that reads the **shipped** files rather than a
  fixture: the `ru_RU` row gains a stress-marked word that must be
  accepted and a stress-marked non-word that must not, so a dictionary
  swapped for a different upstream cannot quietly take the behaviour
  with it.
- **`input_box.rs`**: a misspelling range containing a zero-width mark
  renders as one underlined span (no orphaned mark, no panic).
- **Live: not required** — spellcheck touches no engine, no memory and no
  tool; this is a pure input-box/feature change (AGENTS.md §3). The
  screenshot in the report is the acceptance check: the same line typed
  into a real terminal, with no underline.

## 7. Not in this track

- **The spacing acute as a stress sign.** Measured: `мо´локо`
  (`U+00B4`) splits into two words today, and `моˊлоко` (`U+02CA`, which
  Unicode calls a letter) stays one word and is rejected. Both are rare
  conventions — every Russian source uses `U+0301` — and `´` is a quote
  in other contexts, so treating it as in-word would cost more than it
  buys.
- **Precomposed Cyrillic grave**: `всѐ` (`U+0450`), `ѝли` (`U+045D`) are
  rejected and stay so; they are single characters, not letter + mark, so
  the strip does not reach them. A two-entry map would, if anyone asks.
- **A Latin vowel inside a Cyrillic word** (`мóлоко`) — a homoglyph
  question, not a spelling one.
- **Validating where the stress falls.** The app accepts a mark wherever
  it is put; `мол́око` is as correct as `мо́локо`. Checking stress
  placement needs an accentuated dictionary, which is a different data
  set and a different feature.
- **`ё`-restoration** (offering `всё` where `все` was meant) — a
  semantic question the dictionary cannot answer.

## 8. Documentation touch list (AGENTS.md §4)

- **spec §11.5** — the segmentation bullet: a combining mark belongs to
  the letter before it, and a word that fails as typed is re-checked
  without its marks; one line on `е`/`ё` needing nothing, with the count.
- **architecture §10** (the Spellcheck bullet, line 3079) — "our own
  segmenter" gains the mark clause.
- **[docs/journal/ui-input.md](../journal/ui-input.md)** — a
  `### Post-M9: a stress mark is part of the word (done)` entry plus its
  index line.
- **[docs/lessons.md](../lessons.md)** — beside the existing grapheme
  lesson (line 1007): `char::is_alphabetic` is false for a combining
  mark, so any letter-run scanner cuts a marked word in two.
- **CHANGELOG** `[Unreleased]` → Fixed: stressed words are no longer
  underlined.
- **CLAUDE.md** — the status line's test count and date.
- Not affected: locales (no new string), README (no new key), the
  settings screen, the demo dumps and screenshots.

<!-- cyrillic-ok:end -->

