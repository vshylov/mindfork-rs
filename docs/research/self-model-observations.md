# The self-model's observations — their room, a relevance floor, their header

> Status: **forks decided** (2026-10-18; §6): B0 + B (d), B′ (b1), F (b), H (c).
> Built in three pull requests (§6): #784, #785 and #786. Three defects
> that [prompt-caching.md](prompt-caching.md) §2.2 recorded for the self-model
> track ("Adjacent, for the self-model track") are measured here and laid out as
> forks B, F and H (§6). Two constraints frame every option. The pick stays
> stable from turn to turn: since #782 it is anchored on the chat's opening
> (`request::observation_query`; prompt-caching.md, F1 (e1)), because a pick
> that moved with each message re-read the whole prompt on most turns. And the
> block stays in `system` (the owner's decision of 2026-08-08, prompt-caching.md
> §5 F1).
> Adjacent: spec §17.4–§17.5,
> [self-model-injection-budget.md](../history/self-model-injection-budget.md)
> (the per-section budgets, decisions D1–D5),
> [docs/journal/self-model.md](../journal/self-model.md),
> `shared/embed_calibration.rs` (the similarity scale a threshold is read in).
> Measured 2026-10-18 on a copy of the dev data, with a live embedder.

## 1. The section, as it is

Three functions make it.

- **The pick** (`injection_recent`, `app/orchestrator/generation.rs`).
  `self_notes_relevant` embeds the query (the chat's opening), takes the 20
  notes nearest it among **all** of the profile's notes, keeps the `@self` ones
  and returns the first `narrative_in_prompt` (3). There is no threshold.
  `blend_self_notes` then guarantees the freshest observation (`updated_at`
  first) a place, pushing the third relevant one out if it must. With no
  relevant pick (no query, no embedder) the section falls back to recency.
- **The room** (`SelfModel::render_for_prompt`, `entities/self_model.rs`). The
  four sections share `prompt_cap` 40/20/20/20, and what one leaves flows to the
  next. Observations come last. `fit_lines` takes whole items while they fit,
  holding 12 characters for the "… N more" marker. If none fits whole, the first
  is cut at a word boundary. Then the whole block is cut at `prompt_cap` by
  character, a safety net: in the code's words, "the budgets above count
  content, not the fixed labels around it".
- **The header**: `selfmodel.render.observations_recent`, "Recent observations:".

## 2. How it was measured

- **The data**: a copy of `target/debug/data`, its database last written
  2026-10-12. The profile is the dev one, the only one with the self-model on:
  86 live observations (106 written, 20 superseded), `prompt_cap` 4000,
  `narrative_in_prompt` 3, `max_narrative` 100, the block rendered in Russian.
  The original was not written to.
- **The renderer and the pick, ported.** `render_for_prompt`, `injection_recent`,
  `self_notes_relevant` and `blend_self_notes` were ported line for line to
  Python. The port was checked against the 20 chat requests that prompt-caching.md §2.2's runs captured
  from the app (the released binary and the (e1) build): it reproduces their
  self-model block **byte for byte, 20 of 20**. The options in §3.5 are edits of
  the port.
- **The queries**: every visible chat of the profile, 139, each opening embedded
  alone and whole, as the app does it. The embedder is the profile's own, bge-m3
  Q8_0 on `llama-server` b11715 (`-ub 8192`). One opening of 34,988 characters
  (12,483 tokens) is refused by the server; the app then falls back to recency,
  and so does the replay. No chat is compacted, so the query is the opening in
  all of them.
- **Probes**: 14 queries written for this, ten off the profile's subjects and
  four on them.
- **Profile shapes.** There is no second real profile with the self-model on, so
  three shapes are cut from the dev one. *dev* is the profile as it is: a
  733-character summary, 5 active goals (855 characters), 20 traits, 22
  interests, a 223-character dynamic. *at target* has the summary lengthened to
  the default target, 1000. *young* has a 400-character summary, one goal, three
  traits and three interests. *fresh* has observations only.
- **Relevance, read blind.** For each of the 138 embedded openings, the three
  relevant picks were listed with the scores hidden. Each was judged **R**
  (bears on the chat's subject: the reply would use it), **M** (same area: it
  colours the reply, not its substance) or **U** (unrelated). There was one
  reader, the agent that wrote this, so the verdicts are one judgement and not a
  panel. On the twelve chats prompt-caching.md's probe judged, the lead's
  verdict agrees in all twelve (its "weak" read as U).

The scripts are not committed; each is a page of Python around the port.

## 3. The room

### 3.1 The observations

The 86 live observations run from 142 to 2116 characters: p10 307, **median
526**, p90 948. Rendered as an item (`- (age) text`), the median is 536 and
13% run past 940. (prompt-caching.md §2.2's 482 counted another set; these are the live ones.)
Their first sentence has a median of 186 characters and a p90 of 311.

They were written in bursts: 31 in June, 33 in July, 28 in August, then 7 in
September and 7 in October, the last on 2026-10-05. The median observation is
105 days old, and 78 of 86 are older than 30 days.

### 3.2 What the section shows today

Over the 139 chats, with three observations offered to each:

| profile | `prompt_cap` | section's room | shows 1 / 2 / 3 | ≥ 1 whole | first pick cut | block's last cut | freshest shown | text shown |
|---|---:|---:|---|---:|---:|---:|---:|---:|
| dev | 1200 | 241 | 139 / 0 / 0 | 0 | 135 | 139 | 1 | 7% |
| dev | 2000 | 406 | 139 / 0 / 0 | 8 | 111 | 131 | 1 | 17% |
| dev | **4000** | 1056 | **92 / 45 / 2** | 107 | 22 | **56** | **4** | **45%** |
| dev | 6000 | 1783 | 34 / 18 / 87 | 134 | 5 | 19 | 88 | 82% |
| young | 1200 | 390 | 139 / 0 / 0 | 8 | 114 | 131 | 1 | 17% |
| young | 4000 | 3049 | 3 / 4 / 132 | 139 | 0 | 1 | 132 | 98% |
| fresh | 1200 | 1200 | 71 / 58 / 10 | 119 | 20 | 31 | 13 | 55% |
| fresh | 4000 | 4000 | 0 / 2 / 137 | 139 | 0 | 0 | 137 | 100% |

*Text shown* is the share of the three picks' characters that reach the
prompt, averaged over the chats. The *at target* shape reads like *dev*: at 4000
its room is 975, no chat shows three, and 41% of the text is shown.

So on a mature profile at the default cap, the section shows one observation in
two chats of three, and less than half of what was picked. A young profile at
the same cap shows all three, whole.

### 3.3 Three things the totals hide

1. **The block's last cut lands in the observations, by character.** The section
   budgets count content. The labels around it are counted by none of them: the
   block's header, "About yourself:", "Active goals:", the interlocutor's three
   captions and the separators. That is 91 characters in Russian and 111 in
   English on this profile, plus the 21 of the observations' own header. The
   block is then cut at `prompt_cap` by character (`truncate_chars`, not the
   word-boundary cut everything else uses). The observations come last, so they
   pay. At 4000, 56 of 139 chats end with the observations cut by character;
   at 1200, every chat does. Ten of the twenty requests prompt-caching.md §2.2 captured end that
   way, on the released binary and on the (e1) build alike: the section stops
   at character 943 of the 1055 it was given, and its "… N more" marker is cut
   off with it.
2. **The first pick is cut when it is longer than the section**: in 22 of 139
   chats at 4000. This is `fit_parts`' deliberate fallback (an absent section
   would read as "no observations"). On this profile five observations are
   longer than the section's whole room.
3. **The freshest observation's guaranteed place is nominal.**
   `blend_self_notes` appends it last, and with one or two shown it reaches the
   prompt in 4 of 139 chats. Spec §17.4 says it "is still guaranteed a place".

### 3.4 Installations at 1200

1200 was the default `prompt_cap` through 0.9.4. 0.9.5 (2026-08-09) raised it to
4000 for new installations, and an existing `settings.json` keeps its stored
value: decision D5 of the budgets plan, spec §17.5. So an installation from
0.9.0–0.9.4 that never touched the setting is still at 1200.

There, a mature profile's block holds 1050 characters before the observations
begin. The section's room is 241 by its share and about 130 once the labels are
paid. It shows a fragment of one observation, 128 characters at the median, or
7% of the pick. The interlocutor model shows one trait and one interest. None of
the options below makes 1200 hold a mature profile: the best, a larger share,
shows 21%.

### 3.5 The options, measured

On the mature *dev* profile at 4000. The *at target* shape, with less room left
by its longer summary, shows 4–5 points less text under (a) and (d); under (c)
three 300-character picks no longer fit its 863 characters, and it shows 38%.
The last two columns are where a fixed design costs something.

| option | shows 3 | text shown | freshest shown | block's last cut | *young* 4000, text | *fresh* 1200, text | what it costs |
|---|---:|---:|---:|---:|---:|---:|---|
| (a) today | 2 | 45% | 4 | 56 | 98% | 55% | — |
| (b) shares 30/20/20/30 | 43 | 66% | 46 | 51 | 98% | 55% | traits shown 6 → 5, interests 6 → 4 |
| (b) shares 30/15/15/40 | 71 | 78% | 72 | 27 | 98% | 55% | traits shown 6 → 3, interests 6 → 3 |
| (c) B0, each pick cut at 300 | 139 | 55% | 139 | 0 | 55% | 55% | the room a young profile has goes unused |
| (d) B0, the room shared, a short pick keeps its length | **139** | **56%** | **139** | **0** | **99%** | **64%** | in 122 chats all three end in "…" |
| (d′) B0, the room shared equally | 139 | 55% | 139 | 0 | 94% | 63% | as (d), with room wasted on a short pick |

**B0** counts the labels in the budget, so that the block's last cut is never
reached. In the rows above the observations pay for them, as they do today by
being cut, and every other section renders exactly as now. Charging the labels
to every section by its share instead gives the observations 96 more
characters at 4000 ((d) then shows 62%), and costs the interlocutor one
interest (6 → 5); at 1200 the description loses 54 characters and the
dynamic 25. (b) changes the shares and nothing else. (c) cuts each pick at 300
characters at a word boundary. (d) is `narrative_in_prompt` honoured: the
section's room is shared among the picks by water-filling. A pick shorter than
its share keeps its length, and what it leaves goes to the others. Each of the
rest is cut to its share at a word boundary and ends in "…". When a share would
fall under 160 characters (the median first sentence is 186), fewer picks are
shown, and the "… N more" marker counts the rest, as today. (d′) is the same
with equal shares.

At 1200 nothing helps the mature profile: (d) shows one pick cut at a word,
121 characters at the median (6%, against the 128 that today's cut by
character leaves), and the 30/15/15/40 shares show 21%.

Two notes on (d):

- It shows **more of the text** than the whole-item rule (56% against 45%),
  although it cuts more. The whole-item rule leaves room unused whenever the
  next pick does not fit whole.
- It reverses decision D3, "lists lose whole items", for observations only. D3's
  reason is that a trait cut in half reads as a different trait. An
  observation's first 300 characters, cut at a word and ending in "…", read as
  its beginning, and the whole text is one `get_self_model` away, as spec
  §17.4 already says of the lists. Today observations are cut anyway, but by
  accident: the first pick in 22 chats, and the block's last cut, by character,
  in 56.

## 4. A relevance floor

### 4.1 How near the pick is

The lead's cosine over the 138 openings runs from 0.354 to 0.776: p10 0.432,
**median 0.520**, p90 0.621. The candidate pool held 12 to 20 `@self` notes in
its top 20 (median 17), so no opening fell back for lack of candidates.

The probes:

| probe | lead | | probe | lead |
|---|---:|---|---|---:|
| off: a borscht recipe | 0.320 | | off: configuring nginx | 0.445 |
| off: tomorrow's weather | 0.366 | | off: a bare "Hi!" (Russian) | 0.456 |
| off: a Python sort | 0.402 | | off: the KV cache | 0.457 |
| off: a translation | 0.407 | | off: organising project notes | 0.538 |
| off: 17 × 23 | 0.419 | | on: "what do you remember about me?" | 0.509 |
| off: a bare "Hi!" (English) | 0.432 | | on: how the assistant relates to us | 0.509 |
| | | | on: its memory and how it changes | 0.551 |
| | | | on: consciousness | 0.570 |

The off-topic probes lead at 0.32–0.46, except "organising project notes", which
this profile has observations about. The on-topic ones lead at 0.51–0.57. For
scale, bge-m3 averages 0.4128 on unrelated pairs and 0.8176 on paraphrases
(`embed_calibration.rs`). Two leads in three sit in the first third of that
span.

### 4.2 Relevance, read

Of the 138 leads, 34 were judged R, 31 M and 73 U: 47% relevant or moderately
so. By the lead's score:

| lead cosine | chats | R | M | U |
|---|---:|---:|---:|---:|
| under 0.45 | 20 | 0 | 3 | 17 |
| 0.45–0.50 | 31 | 0 | 3 | 28 |
| 0.50–0.55 | 40 | 14 | 8 | 18 |
| 0.55–0.60 | 25 | 8 | 11 | 6 |
| 0.60 and over | 22 | 12 | 6 | 4 |

**No lead under 0.50 was judged relevant: 0 of 51.** A floor on the lead, under
which the chat takes the recency pick instead:

| floor | relevance kept | its leads R / M / U | sent to recency | their leads R / M / U | kept leads relevant or moderate | R and M leads kept |
|---:|---:|---|---:|---|---:|---:|
| none | 138 | 34 / 31 / 73 | 0 | — | 47% | 100% |
| 0.46 | 116 | 34 / 27 / 55 | 22 | 0 / 4 / 18 | 53% | 94% |
| 0.48 | 103 | 34 / 26 / 43 | 35 | 0 / 5 / 30 | 58% | 92% |
| **0.50** | **87** | 34 / 25 / 28 | **51** | **0 / 6 / 45** | **68%** | **91%** |
| 0.52 | 69 | 29 / 22 / 18 | 69 | 5 / 9 / 55 | 74% | 78% |
| 0.54 | 52 | 22 / 19 / 11 | 86 | 12 / 12 / 62 | 79% | 63% |

At 0.50 the floor sends 51 of 138 chats (37%) to recency, gives up 6 moderate
leads and no relevant one, and the leads it keeps are relevant or moderate in
68% of chats, from 47%. Above 0.50 it starts dropping relevant ones: 5 at 0.52,
12 at 0.54.

Two more readings of the same verdicts:

- **The absolute score is as good a signal as any tried.** As a classifier of a
  pick's relevance (R or M against U, all 414 picks), the cosine scores an AUC
  of 0.763. The lead's margin over the median `@self` score scores 0.763 as well,
  and a z-score among the `@self` notes 0.667.
- **Three picks find more than one.** A relevant or moderate observation is the
  lead in 65 chats and among the three in 79. That is an argument for showing
  all three (§3.5 (d)).

The caveat is the method's. The floor is read off the same 138 openings it is
scored on, by one reader, on one profile whose observations are mostly about the
relationship and the self-model, with one embedder. The boundary is sharp here
(no R among the 51 below 0.50, 14 among the 40 just above it), which is why the
number is a round 0.50 and not a finer one.

### 4.3 What the chats below the floor get instead

The three latest observations, all written 2026-10-05: where `run_dialogue` came
from, what agentic development leaves of an experienced developer's edge, and a
SonarCloud card of this repository. Read against the 51 chats below the floor,
they are moderate for about three of them (architectures for AI-assisted
development, a three-year forecast for AI) and unrelated to the rest. That is
about what the relevance pick gave those chats (6 moderate).

So **the floor buys no relevance below it.** What it buys is a label that is
true (§5): every chat with nothing nearer gets the same "what I noticed lately",
rather than a different arbitrary observation in each chat, presented as if it
fitted. Whether that changes what the model does, such as fewer forced links to
an unrelated observation, is not measured here; §6 proposes the probe.

### 4.4 The floor and the prefix

- **The decision is the chat's, made once.** It is taken on the opening's
  vector. The app embeds that text alone, and bge-m3 and OpenAI's
  `text-embedding-3-small` return it bit-identical (prompt-caching.md, "The
  embedder repeats itself"). So the arm holds every turn. It moves only when the
  observations change, at a compaction roll (the query becomes the summary), or
  when the embedder changes. Each of those changes `system` anyway.
- **Near the line**: 4 of 138 openings lead within 0.005 of 0.50. The same text
  cannot cross it. A batch of another shape moves a cosine by up to 0.0045, and
  the app does not embed the query in one.
- **Another embedder.** 0.50 is a position in bge-m3's scale and would be read
  through `similarity_scale().map(…)`, as the three similarity gates are
  (`CONSOLIDATE_SIMILARITY` and the rest). The scale is recorded once per model;
  bge-m3 maps to itself.
- **The recency arm moves with every observation written or revised**, anywhere
  in the profile: one missed prefix per chat in that arm, at its next turn. In
  September and October (to the copy's date) that meant 15 writes on 8 days. Option (d) of §3.5 does
  the same to the relevance arm, because it puts the freshest observation in the
  prompt every time (today it shows in 4 of 139 chats). That is the price of the
  guarantee spec §17.4 already states. Under the whole-item rule it is mostly not
  paid, because the guarantee mostly does not reach the prompt.

### 4.5 Variants that do not survive

- **A floor per pick, with recency filling the freed places.** At 0.50 the
  per-pick precision is 54%. A list that mixes the two orders cannot be headed
  truthfully (§5).
- **A relative floor** (the lead's margin over the median `@self` score). Its AUC
  is the same as the plain score's, so a second parameter buys nothing.
- **Re-anchoring on the first message that clears the floor.** Of the 17
  sub-floor chats with more than one user message, a later one (the 2nd to 4th)
  clears 0.50 in 14. Its picks read mostly unrelated: a follow-up about prefix
  caching picks the observation about the "boarded-up window", and one about
  environment variables picks the model carrier. Follow-ups are longer and more
  personal, so they sit nearer this profile's observations without being about
  them. This is prompt-caching.md's finding again: an opening states the
  subject. It would also move the pick once in each such chat.

## 5. The header

The section says "Recent observations". Since narrative-as-notes Tier 2 it has
held a relevance pick plus the freshest observation, and since #782, a pick by
the chat's opening. On this profile 127 of 138 leads are older than 30 days, and
the median lead is 104 days old. Its own age label says so ("3mo"), right under a
header that calls it recent. No observation on the profile is younger than 12
days.

The same claim stands in two more places:

- the settings' description of `narrative_in_prompt`: "How many of the freshest
  insights to blend into the system prompt (newest first)";
- the first paragraph of spec §17.4: "a handful of recent insights".

The maintenance protocol already says an observation "surfaces by relevance to
the topic", which is what the pick does. A new header changes `system` once, on
upgrade, like any release that touches the block.

## 6. Forks

### B. The observations' room

- **B0 — count the labels in the budget**, so that the block's last cut never
  falls inside the observations (56 of 139 chats at 4000 today, all at 1200).
  This is a defect fix, and it goes with every option below. The labels are
  charged to the observations, the section that pays for them today (§3.5).
- **(a)** Leave the whole-item rule.
- **(b)** A larger share for the observations (30/20/20/30, or 30/15/15/40),
  taken from the description and the interlocutor.
- **(c)** A fixed cap per pick (300 characters).
- **(d)** `narrative_in_prompt` honoured: the section's room shared among the
  picks by water-filling, each cut at a word boundary with "…", with fewer picks
  when a share would fall under 160 characters.

**Recommendation: B0 + (d).** At the default cap, all three picks show in
139 of 139 chats (from 2), 56% of their text reaches the prompt (from 45%), the
freshest observation's guarantee becomes real (139 from 4), and no other section
loses room. A young profile keeps 99%. Against (b): it takes the interlocutor
model's room (traits shown 6 → 3 at 30/15/15/40), which the per-section budgets
exist to protect. Against (c): a fixed cap wastes the room a young profile has
(55% against 99%).

### B′. Installations still at 1200

- **(a)** Leave them (D5).
- **(b)** A schema step that raises a stored 1200 to 4000. Either **(b1)** exactly
  1200, the old default, as `settings_to_v3` rewrote the old reply budget and
  left a typed one alone; or **(b2)** every value under 4000, or a higher
  minimum in `SelfModelParams::from_settings`.

**Recommendation: (a).** §3.4 gives the correctness argument D5 lacked, since a
mature profile at 1200 shows 7% of its pick and one trait. But a stored 1200
cannot be told from a chosen one. The installations it would reach are those of
0.9.0–0.9.4 (July 15–26), which predate the project's first steps towards an
audience (promotion.md).

### F. A floor under the relevance pick

- **(a)** None, as today.
- **(b)** A floor on the lead, 0.50 in bge-m3's scale and mapped for other
  models. An opening whose best `@self` pick is under it takes the recency pick
  (prompt-caching.md's (e2)) for the whole chat.
- **(c)** A floor per pick, with recency filling the freed places.
- **(d)** A relative floor (the lead's margin over the median).

**Recommendation: (b), together with H (c).** It sends 37% of this profile's
chats to recency, gives up no relevant lead, and makes the leads it keeps
relevant or moderate in 68% from 47% (§4.2). It is stable by construction
(§4.4). Below the floor it buys no relevance (§4.3), so its worth is the honest
header. Without H (c) it is worth little, and (a) with H (b) is the cheaper
truthful pair. Against (c) and (d), see §4.5.

### H. The header

- **(a)** Keep "Recent observations".
- **(b)** A neutral "Observations:".
- **(c)** Say how the pick was made. The relevance arm would read
  "Observations nearest this conversation, then the latest:" and the recency arm
  "Latest observations:", with Russian mirrored. The settings' description of
  `narrative_in_prompt` and spec §17.4's first paragraph change with it.

**Recommendation: (c) with F (b); (b) if F stays (a).** Under (c) each arm's
header is true of its content, including the freshest observation that the
relevance arm carries last. Because F (b) decides per chat, the header is as
stable as the pick.

### The build

Three pull requests. The first is independent of the other two; the third
builds on the second, since both change `render_for_prompt`.

1. **B′ (b1)**, a storage change: `SETTINGS_SCHEMA` 5 → 6 and a step
   `settings_to_v6` that rewrites a stored `self_model.prompt_cap` of exactly
   1200 to 4000, both numbers frozen in the step as `settings_to_v3`'s are. A
   golden fixture of v5, a test of the parsed outcome, an entry under the
   CHANGELOG's **Data**, and the release checklist's run of the previous
   binary on the new file, which must refuse it as a newer version's data.
2. **B0 + B (d)** (PR #785) in `render_for_prompt`, a pure rendering function in
   `entities`. Tests: three picks shown on a shape like the measured one at
   4000; the block's last cut never reached across caps; a short pick keeps its
   length; fewer picks under 160 characters, with the marker; the existing
   observation tests moved off the whole-item rule. By the per-section budgets'
   precedent, no live model run is required; the self-model smokes are run as a
   check.
3. **F (b) + H (c)** (PR #786), with the candidate-pool change from §7.
   `self_notes_relevant` returns its scores, `injection_recent` decides the arm,
   and `render_for_prompt` takes the arm's header. Tests: a sub-floor opening
   gets recency and its header; the floor is read through the similarity scale;
   the arm holds across turns (an extension of #782's two-turn test). Live:
   prompt-caching.md §2.2's two conversations again, keeping `system`
   identical on every follow-up as (e1) did on 8 of 8. Optionally, a probe for §4.3's open
   question: on a set of sub-floor openings, how often the reply drags in an
   injected observation, today's pick against recency. Go if recency is not
   worse.

**User's decision (2026-10-18)**, fork by fork, after going through them one at
a time:

- **B: B0 + (d)**, the labels charged to the observations (§3.5). The
  reversal of D3 for observations is accepted: three beginnings over one or
  two whole observations.
- **B′: (b), as (b1)**, against this document's recommendation of (a). The
  owner's reason: for an application with this much to say about itself, 1200
  is too little. Exactly the old default is rewritten, and any other stored
  value stays, because a small cap is a real choice on a small local window
  (Ollama's default is 4096 tokens). A 1200 that was chosen on purpose becomes
  4000 too, the price `settings_to_v3` already paid for 2048.
- **F: (b)**, the floor 0.50 **on the lead only**. Picks 2 and 3 of the
  relevance arm need not clear it. In the 87 chats of that arm, the second and
  third picks above the floor read R 25 / M 34 / U 71 and those below it
  R 4 / M 9 / U 31, so a floor on them would trade 4 relevant and 9 moderate
  picks for 31 unrelated ones.
- **H: (c)**, with the texts above. In Russian the relevance arm says the
  observations nearest the conversation's subject, and the latest; the recency
  arm, the latest observations. The settings' description of
  `narrative_in_prompt` says the same: the observations nearest the chat's
  subject and the latest one, sharing the section's room, so that more of them
  means each is shorter.

## 7. Adjacent, not forked

- **The candidate pool.** `self_notes_relevant` ranks the 20 notes nearest the
  query among all of the profile's notes and keeps the `@self` ones. On this
  profile 12 to 20 of the 20 are `@self`. On a profile with many notes about the
  user, a technical opening could fill all twenty with those notes, and the
  section would fall back to recency in silence. `note_search_semantic` scores
  every note anyway (a full scan in Rust), so ranking the `@self` ones alone
  costs nothing. It belongs in stage 2.
- **The block's last cut is by character** where every other cut in the block is
  at a word boundary. B0 makes it unreachable rather than changing it, and it
  stays as a backstop.
