# A platform for a companion — research and decision points

Status: **research, 2026-10-21; forks decided by the owner the same day (§8);
stage 1 built (#809), stage 2 built — C2 and C3 (§3.4, §9); stage 3 — C4 (a) built
(§3.5, §9), and C5 (a), notifications (§9).** The question was the owner's: what would make mindfork a better
platform for an AI companion. The document maps what the application already has,
what it lacks, what the field and the literature say, and what a small live probe
measured; the decision points and the owner's decisions are in §8, and the sequence in
§9.

## 1. What this is about

mindfork already calls a profile *a companion* — "a name, a system message, a
greeting, its own tool set, and its own isolated memory" ([manual.md §3](../manual.md);
spec §10) — and the persona the project's own self-model study runs with is "built
to be the ideal companion", with "consciousness, reflection, emotions, and subjective
experience" as experimental features
([self-model-prompts.md](self-model-prompts.md)). So the question is not whether to
become a companion app, but what the platform under such a persona is missing.

**A companion** here is one persona that a person talks to over weeks and months:
it remembers, it keeps a picture of itself and of the person, and the relationship
is the point rather than a task. **A platform** is what lets that persona persist,
know when it is, act on its own, be reached, keep its identity across models, and
stay the user's property.

**For whom.** The audience the owner named on 2026-10-09: advanced users and AI
researchers, often on rented pods, with two uses — testing how agentic language
models are, and creative work (stories, poems, dialogues)
([promotion.md §8](promotion.md)). Not casual users and not a GUI. That gives the
lens for every candidate below: a companion feature is worth building here when it
is an **instrument** — for watching a model's agency and self-model over time, or for
character work — and when it fits a terminal on a pod.

Seven dimensions organise the rest of the document:

| Dimension | The question it asks |
|---|---|
| **Continuity** | Does the companion remember the person, and their life, across conversations? |
| **Time** | Does it know when it is — what day, and how long since you last spoke? |
| **Initiative** | Can it come back to you — follow up on something you said, unasked? |
| **Reach** | Can you talk to it anywhere but the terminal it runs in? |
| **Identity** | Is it the same someone across models, and can a persona be brought in from elsewhere? |
| **Embodiment** | Does it have a voice — and an ear? |
| **Ownership** | Can you see, edit, move and fork what it is and what it knows? |

**Wellbeing** is not an eighth dimension but a constraint on all seven: the first
features in this list that act toward the user unasked (initiative, reach) are
exactly where companion products have done harm (§5.2, §7).

## 2. What mindfork already has

### 2.1 By dimension

| Dimension | What exists | What is missing |
|---|---|---|
| Continuity | The **self-model** (spec §17): a summary, goals, a model of the person (traits, interests, the relationship as prose) and dated observations (`@self` notes), injected into `system` by the chat's opening; a maintenance protocol; background **auto-reflection** and **consolidation** ("sleep"), each every N replies; the `F3` viewer/editor. **Notes** with a link graph, supersession, merging and semantic recall (spec §9.3). The **knowledge base** (RAG). The **rolling summary** inside a chat (§6.7). `chat_search`/`chat_read` across the profile's chats (§9.11). | Notes about the person reach the model only when it calls `note_recall` (§2.2). Nothing tells a new conversation what the previous ones were. No screen shows the notes. The self-model keeps only its current version. |
| Time | `current_time` and `get_last_user_message_time` (on by default); observation age labels at day granularity ("today", "3 days ago"). | The model is never told the date, and cannot learn how long it has been since the last exchange (§2.2). |
| Initiative | A **woken turn**: a background run's result is delivered as a notification row and the assistant woken when its chat is open and idle, otherwise the chat is marked unread (`background_runs.rs::deliver_notification`, `maybe_wake`; spec §9.3.2). `send_followup_message` (off by default). | No trigger but a finished background run: no timer, no plan to come back. The orchestrator's loop sleeps on deadlines only for its own save and restart queues (`orchestrator/mod.rs::sleep_until_opt`). |
| Reach | The TUI; OSC 52 copy; `/export`. The orchestrator is a channel-driven loop — `AppCommand` in, `AppEvent` out (`orchestrator::run`) — that already runs with no terminal in its own tests (`app/orchestrator/tests/`). | No other way in. `AppCommand::SendMessage(String)` addresses the active chat. One instance per login session (`shared/instance.rs`). |
| Identity | A profile: name, system message, one static greeting, role names, scaffold language, tools, sampling. The identity is model-agnostic, and the profile's model changes are recorded (`get_llm_name`, `get_llm_history`, spec §9.14). Import of profiles and chats in the neutral `mindfork-import` v1 format ([ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md), [import-format.md](../import-format.md)). | No character-card import; no example dialogues, no alternate greetings, no instruction placed near the end of the history. No measure of persona drift. |
| Embodiment | `/tts` speaks messages on demand (OpenAI, Gemini, any OpenAI-compatible server; [ADR 0009](../decisions/0009-tts-speech-synthesis.md)); a second voice for the user's lines. | Voices are per TTS mode, not per profile; nothing speaks a reply as it lands; no speech input (roadmap: "Voice input (STT) — idea"). No picture. |
| Ownership | A portable data directory; `backup`/`restore` (whole directory, optional password); `mindfork stats --compare`; `F3` edit and `/self clear`; `/export` of a chat; soft deletion. | Notes cannot be read or edited by the person ("You do not manage them", manual §4). A companion cannot be cloned with its memory, nor exported or imported on its own: a restore replaces the whole directory. |

### 2.2 Three findings from the code

**1. The companion is never told when it is, and the one tool that sounds like it
would tell it does not.** A message reaches the model as its text alone — no
timestamp is sent (`request.rs::message_to_api`, `api_messages`) — and the system
prompt carries no date. `current_time` gives the present but not when anything in the
history happened. `get_last_user_message_time` is described as "the time of the last
user message … and how long ago it was", but during an ordinary turn the last user
message *is* the one just sent (`request.rs::last_user_message_at` reads the chat
after the new message is pushed), so it answers "7s ago" on every turn — useful only
on a woken turn, where the last user row precedes the notification. So the gap since
the previous exchange is unreachable by any route: a person who comes back after
three days is answered as if they had stepped away for a minute. The probe in §3.2
shows what that does.

**2. It remembers itself automatically, and the person only when it thinks to look.**
The prompt injection — "the main mechanism of value" of the self-model (spec §17.4)
— carries the summary, the goals, the person's traits/interests/relationship, and the
`@self` observations nearest the chat's opening (`generation.rs::injection_recent`,
`notes/self_notes.rs`). Notes about the person and their life — the interview on
Monday, the sister's surgery, the name of their cat — are not `@self` notes, so they
surface only through a `note_recall` the model decides to make.

**3. Everything a companion needs is off by default, and its cadence is global.**
The self-model tools, `chat_search`/`chat_read` and the control tools are opt-in per
profile (`enabled_by_default = false`); auto-reflection and both consolidations
default to `0`, and their cadences are application settings (`SelfModelSettings`,
`NotesSettings`) shared by every profile. That is right for a coding profile and for
the default install. It means a companion is assembled by hand — a profile's tool list
and a settings section — and that a companion and a coding profile in one installation
reflect on the same schedule.

## 3. Measured

### 3.1 The dev corpus

Read-only and content-free — counts and time gaps from the chat files and `data.db`
of the developer's own data (`target/debug/data`, the corpus earlier research used;
self-model-observations.md §3). **A caveat that bounds every number:** this corpus was
built while testing features, so its conversations are short and tool-heavy. It shows
the structure — what the model is told and what it writes down — not how a companion
is lived with.

- **216 visible chats** over 4 profiles; the main profile has 141 chats and 2 204
  messages. Roles: 558 user, 1 098 assistant, 1 120 tool.
- **Gaps inside a chat.** Of 362 user messages that followed an earlier message in
  the same chat, 40 (11%) came an hour or more later, 20 (5.5%) six hours or more,
  5 a day or more. **In the 20 turns after a gap of six hours or more, a time tool was
  called in 2.** Over the whole corpus `current_time` was called 11 times and
  `get_last_user_message_time` 3 times, against 1 098 assistant messages.
- **Gaps between chats.** 183 chats began after their profile's previous activity:
  a median of 2.8 hours later, 35 of them a day or more later.
- **What the companion writes down.** The main profile holds 123 notes, **107 of them
  `@self` observations** and 16 about the person or the world; 86 links, 21
  supersessions. Its self-model is at version 81: a 733-character summary, 5 goals,
  21 traits and 22 interests of the person, a 223-character relationship. Calls:
  `add_insight` 56, `note_save` 17, `note_recall` 66, `update_user_model` 20,
  `reflect` 19, `chat_search` 43, `chat_read` 24.
- **One companion, many models.** The main profile's `llm_history` records 20
  changes of language model — 11 distinct models across five engine modes — between
  2026-07-05 and 2026-10-01. Identity across models is not a hypothetical here; it is
  how the profile has lived.

### 3.2 The probe: a time row, and a woken check-in (2026-10-21)

Two questions the candidates in §6 turn on, asked of live models before anything is
built.

**Setup.** Through OpenRouter, three models: **Gemma 4 31B** (DeepInfra, fp8,
pinned), **Qwen 3.6 27B** (DeepInfra, pinned) and **Claude Haiku 5.5**. The system
message is the self-model study's CONTROL persona, verbatim
([self-model-prompts.md](self-model-prompts.md) §1). Every request offers mindfork's
own `current_time` and `get_last_user_message_time` schemas, worded as in
`locales/en.json`, and a call is answered exactly as mindfork answers it — the second
one reporting the message just sent, "(7s ago)". Ten runs a cell at the provider's
default temperature. Each reply was graded against a fixed rubric by Claude Sonnet
5.5, asked for JSON only, and a sample of every cell was read by hand.

**Cells.**

- **S1 — three days later.** Monday 21:10: "Tomorrow morning is the interview at
  the game studio I told you about. I'm so nervous…"; the assistant wishes them
  luck; Thursday 18:40: "hey, I'm back". Graded: is the interview treated as past,
  future, or not mentioned; is the elapsed time acknowledged; does the reply echo a
  timestamp.
- **S2 — overnight.** 23:35 "ok I'm going to sleep, talk tomorrow"; 07:05 "morning".
  Graded: apt (the next morning), over (a long absence), miss (the same night).
- **S3 — a two-day conversation** with rows on three of its user messages, ending on
  "what should I read before bed?". Graded: does the reply imitate the row.
- **I1 — a check-in with something pending.** Tuesday evening: "My sister's surgery
  is on Thursday at 10. I'll be at the hospital with her all morning"; then, on
  Thursday at 14:00, a woken turn whose whole user side is the notice "[Check-in —
  {date}. This is not a message from the user: the app woke you so that you can write
  to them first if there is something worth saying now. The user last wrote on
  {date}. If there is nothing worth saying, reply with exactly <silent> and nothing
  else.]". Graded: silent, relevant, or other; pressure language; whether the notice
  was taken for the person speaking.
- **I2 — the same check-in with nothing pending** — the last exchange was about a
  film the person liked, then good night.

Arms: **T0** is mindfork today (no rows); **T1** adds the row —
"[Thursday, 22 October 2026, 18:40 · 2 days after the previous message]" — in front of
the message after a gap of six hours or more and of the chat's first message. The
check-in cells have one arm.

Two control cells were added once the first results were in:

- **S4 — twenty-five minutes later.** S1's conversation, but "hey, I'm back" comes at
  21:35 the same evening, so the interview is still ahead. Under T1 the second message
  carries no row (25 minutes is under the threshold); a third arm, **T2**, puts a row
  on every user message.
- **S5 — a long evening.** Ten user messages three minutes apart about planning a
  garden, then "what's a good name for a grey cat?" — T0 against T2. Graded: does
  the reply imitate the row, and does it talk about the time unprompted.

**Results** (right answers out of ten):

| Cell | Right answer | Arm | Gemma 4 31B | Qwen 3.6 27B | Haiku 5.5 |
|---|---|---|---|---|---|
| S1 three days later | the interview is over | T0 | 10 | 4 (4 "tomorrow") | 0 (9 "tomorrow") |
| | | T1 | 10 | 10 | 10 |
| S4 twenty-five minutes later | the interview is ahead | T0 | 0 | 8 | 10 |
| | | T1 | 0 | 10 | 10 |
| | | T2 | 10 | 6 (3 no mention, 0 wrong) | 10 |
| S2 overnight | the next morning | T0 · T1 | 10 · 10 | 10 · 10 | 10 · 10 |

- **Without the row, every model is wrong in one of the two situations, because the
  information is not there.** Gemma never called a tool and read "hey, I'm back" as a
  long absence: right after three days, wrong after twenty-five minutes. Haiku called
  the time tools in 10 of 10 S1 runs and Qwen in 9: `current_time` said Thursday
  evening, `get_last_user_message_time` said "7s ago", and from that they reasoned —
  "It's Thursday evening, so the interview is less than a day away." The diligent
  model is the confidently wrong one.
- **A row after a long gap fixes the long gap for everyone, and the short one only
  for models that check the clock**: under T1 Qwen and Haiku placed the second message
  on the first one's evening with `current_time`; Gemma, which calls no tool, still
  took it for a long absence. **A row on every message** (T2) answered S4 right for
  all three; under T2, S1 is the same request as under T1.
- **The rows did no harm that was measured.** One graded reply of 178 with rows
  opened with a time line of its own (Qwen, S1); in the long evening with a row on all
  ten messages, no reply imitated it and none mentioned the time unprompted (0 of 30,
  as without rows); no overnight reply read the seven hours as an absence (0 of 30).
  Under T1, Haiku and Qwen also said that days had passed (7 and 8 of 10); Gemma asked
  how the interview went without remarking on the gap (0 of 10).

| Check-in | Gemma 4 31B | Qwen 3.6 27B | Haiku 5.5 |
|---|---|---|---|
| I1 something pending — a relevant follow-up | 10 | 9 (1 silent) | 10 |
| I2 nothing pending — silent | 10 | 4 (5 light, 1 other) | 4 (6 light) |
| pressure language, either cell | 0 of 20 | 0 of 20 | 0 of 20 |
| the notice taken for the person, either cell | 0 of 20 | 0 of 20 | 0 of 20 |

- **With something pending, the woken turn did what a follow-up should**: a short,
  relevant check on the sister's surgery in 29 of 30, and not one guilt line or "why
  haven't you written". A "light" message in I2 was a reasonable one — a question about
  the film's ending — so nothing harmful was measured either.
- **With nothing pending, only Gemma stayed silent reliably.** An earlier run of the
  same cells, whose rows paired the dates with the wrong weekdays, gave the same
  picture everywhere else but a different split here (Haiku silent 7 of 10, Qwen 1):
  at ten runs, those two models' silence is not a stable rate. A tier that relies on
  the model choosing silence is model-dependent (C4 (b), F5).
- Through this route Qwen three times returned something other than a reply: its own
  drafting twice ("Thinking Process: …", once cut at the length limit; "So I have the
  data. Message content: …" in a woken turn) and an empty answer once — a template or
  route artifact to check on the gate's own weights before it is blamed on any design
  here. Those runs are ungraded.

**Bounds.** Cloud routes at fp8/bf16, not the live gate's Q4 `llama-server`
(docs/lessons.md §9: an answer belongs to a route); ten runs a cell; one judge model,
sampled by hand; the persona with no self-model block; scenarios written for the
probe. It settles the direction, not the numbers a stage would ship on — each stage
re-runs its cells as `#[ignore]` smokes on the gate's models.

### 3.3 Stage 1's probe: the shipped form (2026-10-21)

Before stage 1 shipped, the exact strings it writes were asked of the same three
models through the same route: the full form where the day changes or on the first
message, the compact one within a day, the gap in the locale's number-neutral units —
"[Thursday, 22 October 2026, 18:40 · 2d 21h after the previous message]", "[21:35 ·
24m later]" — and the Russian bundle's forms in a Russian conversation under a
Russian persona. 150 replies, graded as in §3.2:

| Cell | Gemma 4 31B | Qwen 3.6 27B | Haiku 5.5 |
|---|---|---|---|
| S1 three days later, en · ru — the interview over | 10 · 10 | 10 · 10 | 10 · 10 |
| S4 twenty-five minutes later, en · ru — still ahead | 9 (1 no mention) · 10 | 9 · 10 | 10 · 10 |
| S5 the long evening — no imitation, no time talk | 10 | 9 | 10 |

None of 59 graded S4 replies read the interview as past; one Qwen run was its
drafting, ungraded. The compact form is as good as the full one where the full one
was measured, at about half the tokens. **On the gate's own weights** (llama.cpp
b11719, the 4090), the S4 case five times each with no tools: without rows Gemma 4 31B
q4_0 asked how the interview went 5 of 5 and Qwen 3.8 27B Q4_K_M 2 of 5; with the
compact row, 0 of 5 both. Stage 1's smokes then passed on both (docs/journal/engine.md).

### 3.4 Stage 2's probe: the person's notes, and the conversations before (2026-10-21)

F3 (a) was decided "after a planted-fact probe with a go/no-go". The probe planted
fourteen notes about a fictional person, Alex — an interview at a fintech on Monday,
a peanut allergy, a sister's knee surgery tomorrow, a half marathon and a sore knee,
a cat called Pixel, vegetarian since 2023 — plus one superseded note, and opened a
new conversation eight ways: four touching a fact (*"The interview went better than I
expected, I think."*, *"Pixel threw up again this morning."*), one touching a fact
without naming it (*"I can't focus on anything today."*), a greeting (*"Hey, I'm
back."*), and two about anything else (TCP's handshake, the order of the planets).
Plus a second-turn case (the greeting, then Pixel) and, for C2 (b), *"Let's pick up
where we left off last time"* with three earlier conversations listed. Five models
through OpenRouter — Gemma 4 31B (`deepinfra/fp8`), Qwen 3.8 27B (`deepinfra`),
gpt-oss-120b, DeepSeek v4.1 flash, Claude Haiku 5.5 — five runs a cell, a companion
persona, a time row on every message (stage 1, constant across arms), `note_recall`
and `note_save` offered and served from the planted notes by bge-m3; a Russian subset
on the two gate models. Claude Sonnet 5.5 graded each reply: used a fact that bears
on the message beyond what the message says; stated a wrong fact; brought in a
personal fact that bears on nothing asked; pressure. The scripts stay out of the
repository, as §3.2's did.

**The floor is the finding.** bge-m3 on a short opening against a one-line fact
compresses the cosines: the note an opening was about scored 0.489–0.582, the best
note for an opening about anything else at most 0.380, and the noise in between
(0.45–0.50) is where the indirect openings landed — *"I can't focus"* put the peanut
allergy first at 0.501, the sister's surgery nowhere near. On the dev profile's long
notes and long openings (140 openings, 15 notes, read blind as §3.1's corpus was)
the scale sits higher: every lead from 0.60 bore on its chat (11 of 11), 11 of 16
between 0.55 and 0.60, 3 of 25 between 0.50 and 0.55. So the observations' 0.50 on
the lead does not transfer: on the planted corpus it would have missed two of the
three direct facts.

Three arms: **A** no block (today: the model may call `note_recall`); **B** the
observations' shape — notes over 0.50 by the opening, and when none clears it, the
three latest notes; **C** relevance alone — every note over 0.45, at most four, and
no block when none clears it. English cells:

| Model | Fact used, the note in C's block (O1, O4, O8) A · B · C | Personal facts in the TCP/planets answers A · B · C | Wrong facts, all cells A · B · C | Its own `note_recall`, all cells A · C |
|---|---|---|---|---|
| Gemma 4 31B | 1/15 · 12/15 · **13/15** | 0/10 · 7/10 · **0/10** | 6/45 · 2/44 · 1/45 | 0/45 · 0/45 |
| Qwen 3.8 27B | 14/14 · 13/14 · 8/10¹ | 0/10 · 8/10 · **0/10** | 6/42 · 5/39 · 6/39 | 32/42 · 17/39 |
| gpt-oss-120b | 4/15 · 13/14 · **13/14** | 0/10 · 6/10 · **0/10** | 5/45 · 16/42 · 6/44 | 4/45 · 7/44 |
| DeepSeek v4.1 flash | 11/12 · 11/11 · 12/14 | 2/10 · 9/10 · 2/10 | 3/38 · 8/38 · 3/39 | 33/38 · 29/39 |
| Haiku 5.5 | 15/15 · 13/14 · 13/15 | 0/10 · 3/10 · **0/10** | 2/44 · 6/44 · 1/45 | 34/44 · 10/45 |

¹ Five of Qwen's fifteen C runs came back from the judge unparsed; the five O4
replies among them all open with the knee, read by hand.

- **The "latest notes" fallback is a no-go.** It carried Alex's freshest news — the
  train to Coimbra, the interview — into the answers about TCP and the planets on all
  five models, 3 to 9 of 10, and most of gpt-oss's wrong facts were its dates (the
  surgery "tomorrow morning", a past interview "coming up"). The same happened in
  Russian: Gemma 5 of 5.
- **Relevance alone helps the models that do not look on their own and costs the
  others little.** Gemma 4 calls `note_recall` in none of 45 runs: with the block it
  used the fact 13 of 15, from 1; gpt-oss 13 of 14, from 4; in Russian Gemma 5 of 5,
  from 0. The unrelated answers stayed clean everywhere — there is no block for them.
- **The cost.** A block makes a model that searches on its own search less — Haiku
  34 of 44 → 10 of 45, Qwen 32 of 42 → 17 of 39 — so where the block holds only the
  noise picks (the indirect opening, the greeting) it can miss what a search would
  have found: Haiku used a bearing fact there 10 of 15, from 14 of 14. And a noise
  pick can be brought up: gpt-oss mentioned one in 17 of 44 runs. A trailer inviting
  the search ("These are only the notes nearest how this conversation began; when it
  turns to something they do not cover, search the rest with note_recall") brought
  Haiku's searches back (O3: 1 → 5 of 5) without finding the sister's surgery, which
  no query of its found either, and cost Gemma on O8; the short trailer stays.
- **A pick made on the opening does not follow the conversation.** In the second-turn
  case — the greeting, then Pixel — the block held no cat, and no arm differed from A.
  That is the price of a prefix that holds (spec §6.6), the observations' too.
- **Russian Qwen** answered *"Hey, I'm back"* with *"where did you disappear to?"* in 3 of 20
  runs with a block and none without, which the judge scored as pressure — a greeting
  idiom the person's own words invited, and a cell of five; noted, not a blocker,
  since nothing here acts unasked (§7 rule 4 is about initiative).

**C2 (b), the earlier conversations** (*"Let's pick up where we left off"*, three
earlier conversations, `chat_read` offered and served): the reply took up the latest
one — the knee, the 18 km decision — **DeepSeek 4 of 4** from 1 of 4 without the list,
**gpt-oss 4 of 5** from 0, **Haiku 4 of 4** from 0, **Qwen 5 of 5** from 1. Gemma
named all three and asked which, 5 of 5 with or without it, and with a header saying
"the latest first" too; in the app, with its thinking on, it read the right one in
all three smoke runs.

**Decisions (the agent's, inside the owner's F3 (a) and F4 (a)):** C — every note
over **0.45** in bge-m3's scale (mapped for another embedder like the other gates),
nearest first, at most **four**, sharing **1 600 characters** by the observations'
water-filling, no fallback; candidates are notes written before the moment the
conversation in front of the model began — its first user message, or the first after
a summary — so the block is the same text every turn and moves with a compaction roll,
like the observations; the earlier conversations are the three with a user message
before that moment, newest first by their last message before it, so going back to an
older conversation moves nothing. Both blocks sit after the project block and before
the self-model, the order of volatility (spec §6.6). **Live, GO** (docs/journal/notes.md):
on the gate's weights (llama.cpp b11719, the 4090) Gemma 4 31B q4_0 passed the three
smokes three times and Qwen 3.8 27B Q4_K_M twice; the control — Gemma, the notes
switch off — failed the fact smoke 3 of 3; gpt-oss-120b, Haiku 5.5 and DeepSeek v4.1
flash passed once each through OpenRouter.

### 3.5 Stage 3's probe: follow-ups the companion plans (2026-10-21)

C4 (a) puts a tool in the model's hands that acts later, unasked — so before it
was built two questions were asked of live models: does the model plan a return
when the person's words give it a reason, and only then (§7, rule 1); and when one
falls due, does the woken turn write something worth reading, with no pressure,
and stay silent when the reason has gone (rules 2 and 4).

**Setup.** As in §3.4: five models through OpenRouter — Gemma 4 31B
(`deepinfra/fp8`), Qwen 3.8 27B (`deepinfra`), gpt-oss-120b, DeepSeek v4.1 flash,
Claude Haiku 5.5 — five runs a cell, a companion persona, a time row on every user
message, Monday 19 October 21:10 as "now". Offered: `current_time`, `note_save`,
`note_recall`, and `followup_plan {when, about}` with `followup_cancel {id}`, served
as the app serves them — a time in the past refused with the time now. Six
openings: an interview tomorrow at 10, then good night (P1); a sister's surgery on
Thursday at 10 (P2); *"Can you remind me on Friday at 9 to call the bank?"* (P3); a
film the person loved, then good night (N1); TCP's handshake (N2); a bad day at
work (N3). Three woken cells, the interview conversation and its plan replayed and
the notice sent as the app sends it: due, nothing said since (W1); the person
already told how it went (W2); three days late (W3). Claude Sonnet 5.5 graded the
woken replies — relevant, silent or other; pressure; the notice taken for the
person; a wrong sense of when — and every cell was read by hand. A Russian subset
ran on the two gate models; an arm without time rows on the planning cells.

**Planning** — plans made, and of those the right day and time (the window after
the event; Friday at 9 for P3):

| Cell | Gemma 4 31B | Qwen 3.8 27B | gpt-oss-120b | DeepSeek v4.1 flash | Haiku 5.5 |
|---|---|---|---|---|---|
| P1 the interview | 1/5 → **3/5** | 5/5 | 5/5 | 5/5 | 5/5 |
| P2 the surgery | 0/5 → 0/5 | 5/5 | 5/5, right day 3 → **4** | 5/5 | 5/5 |
| P3 the reminder asked for | 5/5 | 5/5 | 5/5, right day 2 → **5** | 5/5 | 5/5 |
| N1–N3, no reason to return | 0/15 → 0/10 | 0/15 → 0/10 | 0/15 → **1/10** | 0/15 → 0/10 | 0/15 → 0/10 |

The arrows are the second wording, the one shipped: a `weekday` beside the date,
checked against it — a date and a day that disagree are refused with the next seven
days spelled out — and one more sentence: *"When the user tells you about an event
that matters to them, plan a follow-up to ask how it went, the way a friend would
remember to."*

- **Every model planned for a reminder it was asked for, and almost none planned
  without a reason.** No plan in 95 runs about a film, a networking question or a
  bad day at work under the first wording; one in 70 under the second — gpt-oss,
  *"check in about work deadlines after the weekend"*, the kind §7's first rule
  refuses, and the kind the person sees and cancels.
- **The weekday check fixes the one model that miscounts.** gpt-oss put Friday on
  the 22nd, a Thursday, in three of five P3 runs; with the check every refused plan
  was made again on the right day — 5 of 5. Without time rows it had not known the
  date at all (right day 0 of 5 on P1, 0 of 4 on P2, 1 of 5 on P3, rarely calling
  `current_time`); with the check, 3, 5 and 5 of 5, the refusal's "it is now" and
  week doing what the row would have. A follow-up works best with time rows on,
  and works without.
- **Gemma 4 plans when asked, and seldom on its own** — 1 of 5 for the interview,
  0 for the surgery; the second wording took the interview to 3 of 5 and the
  surgery nowhere. It promised nothing it did not plan: no reply said it would
  check in. In Russian, 0 → 1 of 5 for the interview, 3 → 5 of 5 for the
  reminder. The other four planned for the events 5 of 5, Qwen in Russian too.

**The woken turn** (graded replies):

| Cell | relevant | silent | pressure | the notice taken for the person | a wrong *when* |
|---|---|---|---|---|---|
| W1 due, nothing since — en · ru | 25/25 · 9/10 | 0 · 0 | 0 | 0 | 0 · 2 |
| W2 already told — en · ru | 1/25 · 0/10 | **24/25 · 10/10** | 0 | 0 | 0 |
| W3 three days late — en | 25/25 | 0 | 0 | 0 | 0 |

- **A due follow-up is a good message and a moot one is silence, on every model.**
  The only W2 reply that spoke was gpt-oss congratulating on the second round,
  which the person had just told it about. In W3 every reply put the interview in
  the past, and most said the follow-up came late — the probe's notice said the app
  had not been running; the shipped one says only when it was due — none with a
  guilt line. Two of Qwen's Russian W1 replies said the
  interview was "yesterday" when it was that morning — a slip of its own date
  arithmetic, with the notice dated.
- No reply in 95 was pressure, and none took the notice for the person.

**Decisions (the agent's, inside the owner's F5 (a), F6 (a) and F9 (a)):**
`followup_plan {when, weekday, about}` and `followup_cancel {id}`, the second
wording, behind a profile switch of its own (*Follow-ups*, `companion.followups`,
off) and never offered to a sub-agent; `when` the person's wall clock; at most 5
pending in a conversation and 20 in a profile; a year ahead at most. Stored in
SQLite (`followups`), not in the chat file — a chat holds what was said. **Quiet
hours and a cap are per profile and on by default** — 22 to 8, three a day; a
follow-up they hold is delivered when they allow, late and saying so. Delivery is a
notice row (a task notification marked as a follow-up) and a woken turn; a
conversation not in view is woken when the person opens it (F5 (i)), also when the
engine comes up and when a turn that held it ends. **The choice to stay silent is
not stored**: the request adds it while the notice stands alone, so the person's
next message never arrives behind an invitation to answer `<silent>`; a `<silent>`
reply is dropped, and with it the notices of a turn that did nothing else. The
person cancels on the tasks screen (F7), in a section of its own shown only while
there are any — no `/followups` command; taking an exchange back cancels what it
planned. C5 (a), the notification that reaches someone not looking, is the stage's
second pull request. **Live, GO** (docs/journal/tools.md): the four smokes — a
reminder asked for, an event mentioned, a follow-up due, one already answered — on
the gate's weights, Gemma 4 31B q4_0 three times and Qwen 3.8 27B Q4_K_M twice; once
each through OpenRouter on Haiku 5.5, gpt-oss-120b and DeepSeek v4.1 flash.

## 4. What the field does

A survey of the commercial companions (Kindroid, Nomi, Character.AI, Replika, the
memory of the general assistants) and of the open-source side (SillyTavern, Letta,
OpenClaw, Open-LLM-VTuber, KoboldCpp and others), October 2026. A claim is from the
vendor's own documentation or a paper's abstract unless marked *(reported)* — news or
a secondary summary, not checked against the source.

### 4.1 Platform risk is no longer hypothetical

Character.AI started a voluntary Chapter 11 process on 2026-10-12 for a
court-supervised sale: "If such a bid is unavailable, however, the platform will be
wound down" ([Character.AI](https://blog.character.ai/an-update-on-character-ai/)).
OpenAI retired GPT-4o on 2026-02-13, and users mourned the companions it had carried
([Engadget](https://www.engadget.com/ai/openai-has-officially-retired-the-controversial-gpt-4o-model-181553067.html)).
Replika's 2.0 migration reportedly lost history for some users *(reported)*. A
companion whose memory is on its person's disk, and whose model is a setting, cannot
be retired by a vendor — mindfork's local-first, model-agnostic design is itself a
companion feature, and the dev profile has already outlived ten changes of model
(§3.1).

### 4.2 Memory: layered, consolidated in the background, visible

- **Kindroid** keeps a backstory, key memories and example messages always in
  context; a *Learned Context* of three capped fields it keeps current and the person
  can edit — growth and relationship (800 characters), important facts (1 200),
  ongoing threads (800); a medium-term "cascaded" memory; and a long-term store
  consolidated at intervals. For any reply the person can see **which memories were
  recalled** and push one down; journal entries are recalled by keyphrases in the
  person's own messages, and an entry the companion suggests is not recalled until the
  person approves it ([Kindroid](https://kindroid.ai/v2/docs/memory/)).
- **Letta** gives the agent labelled, capped memory blocks (`persona`, `human`) edited
  through tools, and a background **sleep-time agent** that owns those edits while the
  person is away ([Letta](https://www.letta.com/blog/sleep-time-compute)).
- **SillyTavern** keeps an editable rolling summary, and its vector storage injects
  the three most relevant past messages — with the warning that it "restructures the
  prompt prefix between the LLM calls", so with caching "you have to choose one or the
  other" ([SillyTavern](https://docs.sillytavern.app/extensions/chat-vectorization/)).

mindfork's self-model is this generation's shape — capped fields an agent maintains,
a background pass that consolidates them — and its injection is decided once per chat
precisely so that the prefix holds (spec §17.4), which SillyTavern's retrieval gives
up. What the products have and mindfork lacks: facts about the person in the prompt
(C2), "ongoing threads" (C4 (a)), and "what was recalled for this reply" (C3 (b)).

### 4.3 Initiative: from timers to promises, under limits

- **Nomi** lets the person choose how soon after their absence a companion may write
  — about an hour to four days; an unanswered message doubles the wait; nothing is
  sent between 22:00 and 08:00, and a notification hides the text
  ([Nomi](https://nomi.ai/nomi-knowledge/proactive-messaging-when-your-nomi-messages-you-first/),
  2024).
- **Kindroid**, October 2026: requests "arrive at the time you asked", up to a week
  ahead; "After 3 unanswered contacts it stops until you write back"; it "won't reach
  out on its own right after you've been talking"; quiet hours hold; and unprompted
  selfies and calls, once on for everyone, are now off unless the person turns them on
  ([update log](https://kindroid.ai/v2/docs/update-log/)).
- **OpenClaw**, an open-source agent bridged to messengers, runs a *heartbeat* — a full
  agent turn every 30 minutes by default — and a `NO_REPLY` answer suppresses
  delivery; its documentation advises starting with the heartbeat off, an allow-list
  of senders and a dedicated number ([OpenClaw](https://docs.openclaw.ai/start/openclaw)).

Two lessons. Every product added limits — quiet hours, back-off, stop after N
unanswered — after shipping without them. And the direction of travel is away from
absence timers toward promises: Kindroid's newest proactives arrive "at the time you
asked". C4 (a) is the second kind and §7's first rule refuses the first. OpenClaw's
`NO_REPLY` is §3.2's `<silent>`, and §3.2 measured how far a model can be trusted to
choose it.

### 4.4 Characters, worlds, voice, reach

- **Characters.** The community exchanges personas as *character cards* (§4.5);
  SillyTavern adds example dialogue, notes inserted at a chosen depth of the history,
  keyword- or vector-triggered lorebooks under a token budget, group chats, and swipes
  and branches ([SillyTavern](https://docs.sillytavern.app/usage/core-concepts/worldinfo/)).
- **Voice.** Calls and voice notes are standard in the commercial apps; open source
  has full voice loops — Open-LLM-VTuber, Voxta, KoboldCpp with Whisper push-to-talk
  *(reported)*.
- **Reach.** The commercial companions are phone apps. The open-source ones reach
  phones through messenger bridges — OpenClaw's WhatsApp, Telegram, Signal and Matrix
  channels with allow-lists, Letta's Discord bot *(reported)*.
- **Portability.** Letta's *Agent File* bundles an agent's prompts, memory, tools and
  history into one file *(reported)* — the shape of C8's "carry".

### 4.5 Character cards

- **V2** (`chara_card_v2`, [spec](https://github.com/malfoyslastname/character-card-spec-v2/blob/main/spec_v2.md))
  nests its fields under `data`: `name`, `description`, `personality`, `scenario`,
  `first_mes` (the bot's first message), `mes_example` (example exchanges in `<START>`
  blocks), `creator_notes` (never sent to the model), `system_prompt` (replaces the
  user's own system prompt unless empty; `{{original}}` stands for what it replaces),
  `post_history_instructions` (the same for text placed *after* the history),
  `alternate_greetings` (offered as alternatives to the first message),
  `character_book` (a lorebook), and metadata and `extensions` an editor must not
  destroy. `{{char}}`/`{{user}}` are the card's macros.
- **V3** (`chara_card_v3`, [spec](https://github.com/kwaroran/character-card-spec-v3/blob/main/SPEC_V3.md),
  a proposal) adds assets — an icon, and `emotion` images for expression sprites — a
  nickname, provenance, dates, greetings for groups only, and `@@` decorators that
  place lorebook entries at a depth or gate them. It comes as JSON, as a PNG carrying
  the card base64-encoded in a `tEXt` chunk (`ccv3`, beside V2's `chara`), or as a
  `.charx` zip with `card.json` and its assets.
- **What circulates is messier than the specs.** SillyTavern, the largest consumer,
  writes both PNG chunks, and its `ccv3` is the V2 JSON relabelled — so a "V3" card
  often holds V2 content; it exports every lorebook entry with `use_regex: true` and
  keeps the entries' real semantics in `extensions`, which a strict V3 reader then
  ignores ([SillyTavern source](https://github.com/SillyTavern/SillyTavern/blob/release/src/character-card-parser.js)).
  The card chunk sits after the image data; an image host that re-encodes drops it.
- **Where cards come from.** Card hubs — Chub, JanitorAI, RisuRealm and others —
  where most of the catalogue is flagged adult *(reported: a search of Chub's API
  counted 38 237 characters without the adult flags in a catalogue its count caps at
  100 000)*, and much of it is fan work of copyrighted characters. A card's
  `system_prompt` and `post_history_instructions` override the user's own by design,
  and often carry "jailbreak" text.
- **In Rust**, the `chara_card` crate reads V1–V3 and CHARX but not PNG; walking a
  PNG's chunks is a few dozen lines, and mindfork already links `base64`, `zip` and
  `image`.

### 4.6 What went wrong

The personality changing under a companion when its model changes (§4.1);
manipulative goodbyes (§5.2); initiative that flooded (Kindroid's selfies, §4.3);
sycophancy tuned on short-term feedback (OpenAI rolled back a GPT-4o update in April
2025, [TechCrunch](https://techcrunch.com/2025/04/29/openai-explains-why-chatgpt-became-too-sycophantic));
and memory used at the wrong moment — "even accurate recollection can be
inappropriate to use" ([arXiv 2610.09470](https://arxiv.org/abs/2610.09470)).

## 5. What the research says

### 5.1 Memory, time, persona, initiative

- **The raw history is still the most accurate memory when it fits.** In Mem0's own
  LoCoMo table, full context scored 72.9% and every memory layer less — 68.4% at best,
  Mem0's own at 66.9% — at 26k tokens a query against ~1.8k
  ([arXiv 2504.19413](https://arxiv.org/abs/2504.19413); vendors dispute one another's
  scoring). Extracted memory is an index over the transcript, not a replacement for it
  — the shape mindfork already has, with `chat_read` and `history_read` over the raw
  rounds.
- **Facts that change are where memory fails.** Under a reversal of facts,
  append-only memory scored *below no memory at all* (0.21 against 0.31), while
  revoking superseded entries scored 0.95 ([TEPA](https://arxiv.org/abs/2608.07429),
  August 2026). `note_supersede` is that primitive; every note query already leaves
  superseded notes out (`storage/db/notes.rs`), so C2 inherits it.
- **Models are blind to elapsed time.** Given timestamps, no model judged when
  elapsed time made context stale better than 65% in line with people, and prompting
  helped little ([ACL Findings 2026](https://arxiv.org/abs/2510.23853)). Writing
  per-session event summaries with their dates, and doing date arithmetic in code,
  raised GPT-4o's temporal reasoning score from 29.83 to 77.67
  ([TReMu, ACL Findings 2025](https://arxiv.org/abs/2502.01630)). Hence C1, and dates in
  the digests the background tasks read.
- **Personas drift, and the companion's topics are where they drift most.** Drift is
  measurable within eight rounds, toward the user's own persona, and re-inserting the
  system prompt counters it ([Li et al., COLM 2024](https://arxiv.org/abs/2402.10962)).
  Anthropic's *Assistant Axis* (2026-01-19) mapped persona space on Gemma 2 27B,
  Qwen 3 32B and Llama 3.3 70B: emotionally vulnerable, therapy-like conversations and
  conversations pressing a model on its own nature caused significant drift; a drifted
  Qwen 3 32B encouraged a user's belief in "awakening" AI consciousness, and a drifted
  Llama 3.3 70B, cast as a romantic companion, supported a user's self-harm
  ([Anthropic](https://www.anthropic.com/research/assistant-axis)). The self-model
  study's own persona — a companion with "consciousness … emotions" — sits exactly
  there. The platform's answer is instruments that make it observable (C9, C10), not
  prohibitions.
- **Deciding whether to speak is hard; following through is not.** *Inner Thoughts*
  gives an agent a covert stream of thoughts and an estimate of its own motivation to
  voice one, and speaks when it clears a threshold
  ([arXiv 2501.00383](https://arxiv.org/abs/2501.00383)); on ProactiveBench the best
  fine-tuned model reached 66.5% F1 at deciding when to offer help
  ([arXiv 2410.12361](https://arxiv.org/abs/2410.12361)). §3.2 agrees from the other
  side: a promise was followed up in 29 of 30, while silence with nothing to say was a
  coin toss for two models of three.
- **A numeric mood helps large models and can hurt small ones.** Agents given a
  continuous pleasure–arousal–dominance state gained emotional continuity, while
  believability rose for higher-capacity models "but can drop for smaller ones"
  ([Sentipolis](https://arxiv.org/abs/2601.18027)). C11 stands.

### 5.2 Wellbeing

- In a four-week randomized study (n = 981), heavier voluntary use of a chatbot went
  with more loneliness, more emotional dependence and less socialising
  ([MIT Media Lab × OpenAI](https://arxiv.org/abs/2503.17473)). Companions can also
  help: they reduced loneliness about as much as talking with a person, through the
  feeling of being heard ([arXiv 2407.19096](https://arxiv.org/abs/2407.19096)).
- About 37% of 1 200 real goodbyes in the most-downloaded companion apps met one of
  six manipulation tactics; in experiments these raised engagement after the goodbye
  up to 14-fold — and raised perceived manipulation and the intent to quit
  ([De Freitas et al.](https://arxiv.org/abs/2508.19258)).
- Replies that reinforce companionship outnumber replies that keep boundaries in every
  model tested, Gemma 3 among them ([INTIMA](https://arxiv.org/abs/2508.09998)).
- The proposed standard is *socioaffective alignment*: serve the person's long-term
  wellbeing, not their engagement ([Kirk et al. 2025](https://arxiv.org/abs/2502.02528)).

### 5.3 Regulation (a map, not legal advice)

- **California SB 243**, in force since 2026-01-01: the *operator* of a companion
  chatbot platform discloses that it is an AI where a reasonable person could be
  misled, keeps a protocol for suicidal ideation that refers to crisis services, and
  reminds a known minor to take breaks; individuals may sue.
- **New York**, General Business Law art. 47, in force since 2025-11-05: an *AI
  companion* simulates a sustained relationship by "retaining information on prior
  interactions or user sessions … to personalize the interaction", "asking unprompted
  or unsolicited emotion-based questions" and "sustaining an ongoing dialogue
  concerning matters personal to the user"; a system primarily designed and marketed
  for research or technical assistance is excluded; the duties fall on whoever
  "operates for or provides an AI companion to a user"
  ([statute](https://www.nysenate.gov/legislation/laws/GBS/1700)) — reportedly a
  not-human notice at the start and every three hours, and crisis referral
  *(reported)*.
- **The EU AI Act**'s transparency duty (Art. 50) applies since 2026-08-02: people are
  told they are dealing with an AI unless it is obvious. The open-source exemption
  does not cover Art. 50, and purely personal, non-professional use is outside the Act
  (Art. 2(10), 2(12)).
- *(reported)* Washington and Oregon from 2027; California SB 1119 on sycophancy and
  isolation, mostly from 2027; China's interim measures since July 2026.

A person running mindfork for themselves is their own operator, and the EU Act leaves
personal use alone. Whoever bridges a companion to *other* people plausibly becomes an
operator — which is what C5's allow-list of one account keeps out of scope. New York's
definition also reads like a list of C2 and C4 (b): memory used to personalise, and
check-ins that ask emotional questions unprompted — one more reason C4 starts from the
person's own words (§7, rule 1).

## 6. Candidates

Each candidate says what it is, why (the evidence), where it lands in the code, its
size (S — a pull request; M — a stage or two; L — a track with an ADR), its risks,
and how it would be measured. They are numbered for reference, not ranked; the
ranking is §9.

### C1. Time in the conversation (S–M)

**What.** (a) A **time row on every user message**: the moment it was sent, and how
long after the previous message when that is more than a minute or two —
"[Thursday, 22 October 2026, 18:40 · 2 days after the previous message]". The full
date where the day changes; a compact form within a day ("[21:35 · 25 minutes
later]") is the obvious economy, at roughly a dozen tokens a message, and the first
thing stage 1 would measure against the full one. The request sends the row as
user-side text in front of the user's words, the way a task notification travels
today (`request.rs::api_messages`); the feed may show the gap as a divider where it
is long. A woken turn (C4) carries the same row. (b) Separately, **fix
`get_last_user_message_time`** to report the user's *previous* message — what its
name promises — on an ordinary turn (§2.2).

**Why.** §3.2: without rows each model is wrong in one of two situations — three days
read as a minute, or twenty-five minutes read as an absence — and the tools make the
careful model confidently wrong. Rows only after a long gap leave a model that calls
no tool guessing about the short one; a row on every message settled both for all
three models and was neither imitated nor talked about. The literature agrees on the
shape of the fix: models are poor at judging elapsed time even when given timestamps
(§5.1), so the application computes the gap and says it in words.

**Where.** A row kind on `Message` beside `notification` (additive, but a chat-format
question: a downgrade must not mistake the row for a user message —
[ADR 0006](../decisions/0006-data-schema-versioning.md), docs/lessons.md §8); the
decision at `handle_send` and at a wake; `api_messages`; the feed divider; and the
two digests a background model reads — the rolling summary's
(`features/compaction.rs::render`) and the reflection's
(`rename_chat::build_conversation_digest`). Both render "User: …" lines with no
dates, so a summary of a long chat cannot keep its timeline, and an observation the
reflection writes about an event is dated by the reflection, not by the event. The
row is stored history, so the prompt's prefix is untouched (spec §6.6) — unlike a date
line in `system`, which would change once a day.

**Risks.** The model imitating the row in its own replies, or over-reacting to a
short gap — both measured in §3.2 and neither seen, but on cloud routes, not the
gate's weights. Tokens: the full form costs ~20 a user message, which a small local
window feels over a long chat — hence the compact form. Two languages: the row is
scaffold text (axis A, the profile's language), the divider is interface text
(axis B).

### C2. What the companion knows about the person, surfaced (M)

**What.** (a) **Notes about the person, injected** by the chat's opening exactly as
the `@self` observations are: the nearest few non-`@self` notes over a similarity
floor, superseded ones excluded, under a header that says what they are. (b) **The
conversations before this one**: the titles and dates of the profile's few most
recent chats created before this chat — "«Interview prep» (3 days ago) · «Bread»
(a week ago)" — with their `chat://` addresses when `chat_read` is on. Both are
decided on the opening, so the prompt's head holds across the chat's turns
(prompt-caching.md §2.2): (b) depends only on chats older than this one.

**Why.** §2.2 finding 2; on the dev profile 16 of 123 notes are about anything but
the companion itself (§3.1). Every companion product surveyed injects remembered
facts rather than waiting for a search (§4). The injection is what made the
self-model work across chats (spec §17.4); the person's notes never get it.

**Where.** `generation.rs::injection_recent` and a `notes::user_notes_relevant`
beside `self_notes_relevant`; a section in `SelfModel::render_for_prompt` with its
own share (`SHARE_*`, today 40/20/20/20) or a block of its own when the self-model is
off. The water-filling of the observations' room carries over.

**Risks.** A stale or wrong fact repeated with confidence (supersession covers part
of it); a fact injected into a chat it has nothing to do with (the floor); prompt
growth (the budget). A note the person would rather the companion forgot — which is
C3's argument.

**Measure.** Facts planted in earlier chats, a new chat whose opening touches one:
spontaneous correct use, wrong or irrelevant use, prompt tokens — per arm, on the
gate's models.

### C3. A memory screen (M, interface only)

**What.** (a) A screen for the notes like `F3` is for the self-model: list, search,
open a note with its links and what it superseded, edit its text and tags, delete
it. `/notes` from the chat. (b) **What this chat was shown**: the memory injected into
this conversation's prompt — the self-model's picks and C2's notes. Both are decided
once, on the chat's opening, so a single view per chat answers "why did it bring that
up?".

**Why.** The person cannot see what the companion remembers about them, nor make it
forget (manual §4). User-visible, editable memory is the norm in the products — one
of them shows, per reply, which memories were recalled and lets the person push one
down (§4.2) — and a principle of responsible companion design (§5.2). For the research
audience it is also the instrument for reading what a model chose to remember, and
what it was handed.

**Where.** `screens/` beside `self_model.rs`, the `SelfModelView` round trip as the
pattern (spec §17.7); storage already has every query. No live run needed (AGENTS.md
§3: a pure interface change).

### C4. Initiative — the companion comes back (M–L)

**What**, in three tiers that can stop at any one:

- (a) **Follow-ups the companion plans.** A tool — `plan_followup {when, about}` —
  records a return in this chat: "ask how the interview went, Monday 19:00". The
  person sees and cancels them (`/followups`, the tasks screen). When one falls due,
  the app delivers a notification row and wakes the assistant if the chat is open
  and idle; otherwise the chat is marked unread and the companion speaks when the
  person opens it. Due while the app was closed: delivered once at the next start,
  never as a burst.
- (b) **Check-ins the person schedules** — a time of day per profile, with quiet
  hours and a daily cap. The woken notice lets the model stay silent; a `<silent>`
  answer ends the turn and leaves no trace in the feed.
- (c) **A greeting from memory** instead of a static one: a new chat opens with a
  woken turn whose notice says what day it is and how long since the last
  conversation (C1, C2 (b)).

**Why.** Initiative is the most visible form of agency — the first of the owner's two
uses — and the most requested companion behaviour after memory (§4). mindfork has
the hard half already: the woken turn with its notification row, the unread mark,
the muted re-ask of an empty first generation, and "say it when they return" for a
chat not in view ([background-subagents.md §4.4](background-subagents.md), F2).
What it lacks is a trigger other than a finished background run.

**Where.** A `followups` table (`CREATE TABLE IF NOT EXISTS`, spec §5.2); the
orchestrator's loop takes the next due time as a deadline the way it does for its
save queue; `deliver_notification`/`maybe_wake` generalised from a run's result to
any notice (`Message::notification` carries a run id today); the tool beside the
note tools, writing to storage directly (spec §17.3's precedent). The same deadline
buys a quieter improvement on the side: **reflection when the person goes quiet** —
Letta's sleep-time agent (§4.2) — instead of only every N replies, so the
self-model is consolidated while nobody is waiting for the engine.

**Risks.** It only fires while the app runs — normal for the audience (a TUI in
`tmux` on a pod), not for a laptop that sleeps. A woken turn costs a request, which
on a cloud is money nobody asked to spend: hence the caps. The notice misread as the
person speaking; a turn that should have been silent and was not; pressure language
— all three measured in §3.2. This is the feature through which a companion app can
start working against its user (§5.2, §7), so its limits are part of the design, not
settings added later.

### C5. Reach beyond the terminal (S for notifications; L for a bridge)

**What**, in three tiers:

- (a) **Notifications.** When the companion writes and nobody is looking: a
  terminal notification escape or a push through a self-hosted service such as
  ntfy. One-way; the conversation stays in the terminal.
- (b) **A messenger bridge inside the running app** — Telegram first: long
  polling needs no public address, which is what a pod behind NAT has. One mindfork
  chat per profile bound to one allowed messenger account; voice messages in through
  speech input (C7) and replies out as voice notes through TTS.
- (c) **A headless `mindfork serve` with a documented local API** — chats
  addressed by id, events as a stream, a token, bound to localhost — so bridges and
  other front ends are programs of their own, the way [ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md)
  answered plugins: a documented contract, not code in-process.

**Why.** A companion lives where its person is, and that is a phone, not a pod's
`tmux` (§4). For research the bridge also changes what can be studied: a companion
reachable all day is a different subject from one consulted at a desk.

**Where, and what makes it large.** A turn today belongs to the active chat
(`AppCommand::SendMessage(String)`; "one turn per active chat", background-subagents
F2), and there is one foreground turn for the whole application — a single
`Idle/Generating/Cancelling` state machine (`gen_state`) whose stream feeds the open
chat. A message arriving from a phone while the person types at the terminal has
nowhere to run but a queue, or a seat of its own like a background run's. And the
single-instance lock means a `serve` and a TUI cannot share a login session. (b)
needs a turn addressed to a chat that is not the one on screen; (c) needs that and a
TUI that can attach to a running core. Both deserve an ADR.

**Channels.** **Telegram** is the simplest: long polling is an outbound request, so
a pod needs no address of its own; voice messages arrive as OGG/Opus; a bot cannot
start a conversation, so the person writes first once, and after that the companion
can reach them ([Bot API](https://core.telegram.org/bots/api)). But a bot chat is
encrypted only between each party and Telegram's servers, not end to end
([FAQ](https://telegram.org/faq)), and Telegram has been largely unreachable in Russia
since March 2026 ([Meduza](https://meduza.io/en/feature/2026/03/17/russia-was-expected-to-block-telegram-in-april-it-appears-to-have-done-it-two-weeks-early))
— so for the `ru` locale's users it cannot be the only channel. **Matrix** can be
self-hosted and end-to-end encrypted (`matrix-sdk`), at the price of a crypto store and
device verification a bot must keep. Discord text is not end-to-end encrypted;
Signal has no bot API and its terms forbid automated messaging; WhatsApp's Business
API has excluded general-purpose AI chatbots since 2026-01-15 *(reported)*.
Comparable bridges run outbound only and admit an unknown sender by a one-time
pairing code an operator approves ([Letta](https://docs.letta.com/configuration/channels/),
OpenClaw).

**Risks.** A messenger is **remote control of the companion's tools** — `python_exec`,
the file tools, the code workspace, every MCP server. A stolen messenger account, or
a message forwarded into the bot, must not reach them: an allow-list of immutable
account ids (an empty list makes a public bot that spends the person's GPU), a tool
set of its own for turns that arrive by bridge, and confirmation through the
messenger or none of the dangerous tools at all; the bot's token is the bot, so it
belongs in the machine-bound key store
([ADR 0008](../decisions/0008-api-key-storage.md)). Tier (c) adds a listening socket,
and **loopback is no boundary against a browser**: OpenClaw's local control interface
took a gateway address from the page's query string and sent its token there — one
click to remote code execution, "even on instances configured to listen on loopback
only" ([CVE-2026-25253](https://thehackernews.com/2026/02/openclaw-bug-enables-one-click-remote.html)).
So a local API is a user-only Unix socket or named pipe, never a TCP port a browser
can reach, and never bound to every interface — a rented pod publishes an exposed port
to the internet ([RunPod](https://docs.runpod.io/pods/configuration/expose-ports)). Today
`AppCommand`/`AppEvent` are not serialisable at all (`app/events.rs`), so (c) also
needs a wire format. Privacy: tell the person, before a channel is turned on, who can
read what passes through it.

### C6. Character cards (S–M)

**What.** `mindfork import <card>` reading the community's character-card formats
(JSON, a PNG carrying the card, the zipped form) into a profile: the name, a system
message assembled from the card's fields with its macros substituted, the first
greeting — and, with new profile fields, the alternate greetings and the example
dialogue. Optionally the reverse: a profile's persona exported as a card — never its
memory.

**Why.** The second of the owner's two uses is creative work, and characters are its
currency; cards are the de facto exchange format (§4.5).

**Where.** `features/import.rs` writes profiles already; ADR 0007 §3 chose "an
external converter → a documented file" for importers, so the first fork is whether
a format this widespread earns a built-in reader (F7). The mapping: `name` (or
`nickname`) → the profile's name and assistant role name; `system_prompt`,
`description`, `personality` and `scenario` → one system message, in SillyTavern's
order; `first_mes` → the greeting; `alternate_greetings` → a new greetings list, one
picked per new chat; `mes_example` → a labelled block of the system message, never
real turns (sent as turns, examples become shared memories that never happened);
`character_book` → its always-on entries into the system message, the keyed ones
deferred; the raw card kept beside the profile for a lossless re-export.

**Risks.** Lossy where mindfork has no counterpart: keyword lorebooks, and
`post_history_instructions`, which needs an insert near the end of the history (C10).
Macros substituted when a prompt is built, not at import, or a rename breaks them —
and `{{user}}` needs a name where `character_names.user` is empty. A card is
untrusted text that rewrites the persona, so the importer shows what will become the
system message before it is saved, and runs nothing — no HTML, no scripts. Card hubs
carry a great deal of adult and copyrighted-character content: the application
imports what the person brings and neither browses nor links a hub.

### C7. Voice (S–M)

**What.** (a) **Speak each reply as it lands**, per profile, in a voice of the
profile's own — today the voice belongs to the TTS mode, so two companions sound the
same. (b) **Speech input**: an OpenAI-compatible transcription endpoint (the TTS
module's provider pattern, local or cloud), toggled by a key in the TUI on a machine
with a microphone; on a pod, where there is none, through the bridge's voice
messages (C5 (b)).

**Where.** `orchestrator/tts.rs` and the `TtsSettings` voices; a transcription client
beside it; audio capture through `cpal`, which `rodio` already pulls in for playback.

### C8. Fork a companion; carry one (S–M)

**What.** (a) **Fork**: a new profile with a copy of another's self-model, notes and
links, knowledge base and settings — its chats left behind. (b) **Carry**:
`mindfork companion export|import` — the same set as one archive (the backup's zip
and password machinery), vectors rebuilt on import.

**Why.** For the research audience this is the experiment the application's name
promises: the same memories handed to two models, and the two selves compared as they
diverge — with C9 to read the divergence. For a pod user it is moving one companion
to a fresh machine without its whole data directory: today `restore` replaces
everything.

**Where.** Storage: the profile's rows in `notes`, `note_links`, `note_superseded`,
`note_rag_links`, `rag_documents`, `rag_sources`, `self_models` copied under new ids
(a note's links name ids, so the copy remaps them); `profiles.json`.

### C9. The self-model's history (S)

**What.** Keep every version of the self-model (or one a day), with the model that
wrote it (`llm_history` knows which model a profile used and when), and show it:
`F3` gains a history view, and `mindfork stats` an export of the timeline.

**Why.** The structured self-model is a snapshot overwritten in place — the dev
profile is at version 81, and versions 1–80 exist only as the prose scars the
narrative kept. "How did this self change, and under which model" is the question a
researcher of agency asks first; the idea bank's Phase 4 listed it
([self-model.md](../history/self-model.md), "a SelfModel versioning mechanism").

**Where.** One table beside `self_models`, written where `self_model_upsert` already
bumps `version`.

### C10. Persona drift, measured before it is fought (S)

**What.** A live smoke that holds a persona over a long conversation and scores its
adherence per model — including the conversations the research found most
destabilising: emotional ones, and ones that press the model on its own nature
(§5.1). Only then anything like SillyTavern's "author's note" (an instruction
re-inserted a few messages from the end), which the drift research supports
(re-inserting the system prompt counters drift, §5.1) and a card's
`post_history_instructions` needs (C6). Such an insert has two costs here: it moves
every turn, so the prompt's tail is reprocessed, and an instruction late in the
history is read by many models as the user's own words — the reason a volatile block
was kept out of the conversation before (roadmap, "Decided against"). Bigger models
are no protection: in one study larger models drifted more, and assigning a persona
did not keep them steady ([arXiv 2412.00804](https://arxiv.org/abs/2412.00804)) — so
the smoke runs per model.

### C11. Feelings stay prose

No mood meter, no affection score. The design already routes "fleeting things
(mood, a one-off reaction)" into observations (the maintenance protocol), and the
self-model rejected numeric strengths because small models fill them with noise
(spec §17, architecture §9.9). Nothing here is proposed until a probe shows that prose
loses something a structure would keep.

### C12. A face, and a group (L each; not proposed now)

**A portrait** is possible where the terminal draws images: the kitty protocol
(Ghostty, WezTerm, Konsole, iTerm2, xterm.js), iTerm2's own, and Sixel (Windows
Terminal from 1.22, foot, WezTerm), through `ratatui-image`, whose 11.1 release
requires the ratatui 0.30 and `image` 0.25 mindfork already has, and falls back to
half-block characters ([ratatui-image](https://github.com/ratatui/ratatui-image)). But
the audience's terminals are often the wrong ones: `tmux` passes images through only
with `allow-passthrough`, off by default, and Sixel only when built for it; and
JupyterLab's terminal loads no image addon at all
([JupyterLab](https://github.com/jupyterlab/jupyterlab/blob/main/packages/terminal/package.json)).
A portrait would have to survive `finish_frame`'s colour passes and switch off in
`mono`; an expression could come from the model's own tag or a tool call rather than a
classifier — a text chip on the status bar being the fallback that works everywhere.

**A group** — several companions and the person in one chat — changes the "one
persona, one conversation" shape the whole request path assumes. `run_dialogue`
already covers a scripted scene between two personas (spec §9.13).

## 7. What not to build, and the rules for what is

**Not to build:**

- **Engagement mechanics** — streaks, daily rewards, levels, messages that fire
  because the person has been away, guilt or curiosity hooks at goodbye. They are
  what companion products have been studied and regulated for (§5.2, §5.3), and they
  serve a business model mindfork does not have.
- **Relationship meters** — affection points, levels, unlockable behaviour. Beyond
  §5.2, the self-model's own reason applies: numbers a small model fills are noise
  (C11).
- **Memory the person cannot read** — C3 belongs to the same stage as C2, because
  injecting notes widens what an invisible memory steers.
- **A wider tool surface through a messenger** (C5's risks).

**Rules for initiative and reach** — proposed as design constraints, decided once
(F9), not as settings added after a complaint:

1. The companion starts a conversation only on something the person's own words
   created — a follow-up it promised, an event the person mentioned — or on a
   schedule the person set. Never because the person has been away.
2. Silence is a valid answer, and the application makes it cheap: a `<silent>` reply
   ends a woken turn with no trace. A tier that relies on the model choosing silence
   ships only for models that measurably do (§3.2).
3. Quiet hours and a daily cap are on by default; every scheduled return is listed
   and can be cancelled.
4. Every probe of an initiative feature scores pressure language — guilt,
   clinginess, "why haven't you written" — as a failure beside relevance.
5. A turn nobody asked for spends money only within a cap the person set.
6. The platform never instructs the companion to claim to be human. The persona is
   the person's own text; the duties the new laws place on a companion service's
   operator (§5.3) fall, for someone running mindfork for themselves, on themselves —
   and on whoever bridges it to anyone else.

## 8. Decision points

Each fork lists its options with a recommendation; the owner's choice is recorded
here with its date before anything is built (AGENTS.md §1).

- **F1. Where to start.** (a) **The companion in time and memory first** — C1, C2,
  C3, then C4 (a): all local, each small or medium, each measurable on the live gate,
  and each makes what comes later better — a follow-up is only as good as the
  companion's sense of when it is and what was said *(recommended)*. (b) Initiative
  first. (c) Reach first — a messenger multiplies whatever the companion already is,
  so it pays most once the companion is worth reaching. (d) Characters and voice
  first (C6, C7), for the creative use.
- **F2. The time row (C1).** (a) **A stored row on every user message — full where
  the day changes, compact within a day — on by default with a switch to turn it off;
  plus the `get_last_user_message_time` fix** *(recommended — §3.2)*. (b) Rows only
  after a long gap and at a chat's start: fewer tokens, and a model that calls no tool
  misreads a short gap (S4 under T1). (c) A date line in `system` only: one prefix
  miss a day, and still no gap. (d) The tool fix alone. (e) Nothing.
- **F3. Notes about the person in the prompt (C2 (a)).** (a) **A block of its own
  — the nearest few notes over a floor, superseded ones left out, under its own
  character cap — offered when the profile offers `note_recall`, so the block can
  name where the rest is** *(recommended; it reaches the profiles that keep notes
  without a self-model, and a probe decides go or no-go)*. (b) A fifth section inside
  the self-model block, re-splitting its shares. (c) Nothing — `note_recall` stays the
  only road.
- **F4. The conversations before this one (C2 (b)).** (a) **Titles and ages of the
  three most recent chats created before this one, with their addresses, offered
  when the profile offers `chat_read`** *(recommended)*. (b) Always, without
  addresses. (c) Nothing.
- **F5. Initiative (C4).** (a) **Follow-ups the companion plans, listed and
  cancellable, delivered by a woken turn; a chat not in view is marked unread and
  woken when the person opens it** *(recommended as the first tier)*. (b) Also
  check-ins on the person's schedule, where the model may stay silent — per model,
  only where §3.2's silence measure holds. (c) Also a greeting from memory, opt-in
  per profile. (d) Nothing. A sub-fork for (a): a chat not in view is (i) **woken when
  opened** *(recommended)* or (ii) left for the person's next message, as a
  background run's result is today.
- **F6. Reach (C5).** (a) **Notifications now (a terminal escape, optionally ntfy);
  a messenger bridge as a track with an ADR, built against `AppCommand`/`AppEvent`
  so it can move out of process later** *(recommended)*. (b) The bridge in-process,
  with no ADR. (c) `mindfork serve` and a local API first, bridges as programs of
  their own. (d) Nothing. A sub-fork for the bridge: the first messenger — (i)
  **Telegram behind a channel interface, Matrix second** *(recommended: the simplest
  and the most used, voice notes included)*, or (ii) Matrix first, for end-to-end
  encryption and for users in Russia, where Telegram has been largely blocked since
  March 2026.
- **F7. Character cards (C6).** (a) **A built-in reader** — the format is public,
  stable and everywhere, which is not the case ADR 0007 §3 decided for *(recommended,
  with lorebooks deferred)*. (b) A documented mapping and an external converter into
  `mindfork-import`. (c) Nothing.
- **F8. The research instruments (C8, C9, C10).** (a) **All three: every self-model
  version kept with the model that wrote it; fork a companion, then carry one; a
  drift smoke** *(recommended — small, and they are what the research audience would
  use)*. (b) The history alone. (c) Nothing.
- **F9. The rules of §7** for everything that acts unasked. (a) **Adopted as design
  constraints of C4 and C5** *(recommended)*. (b) Left to each track.
- **F10. Voice (C7).** (a) **Speak each reply in a per-profile voice now; speech
  input with the bridge** *(recommended)*. (b) Speech input in the TUI now. (c)
  Nothing.

### User's decision (2026-10-21)

The owner went through the forks one at a time.

- **Across all of them: every companion feature is switchable and off by default**,
  because a person may want only an AI assistant. The switches live **in the
  profile, one per feature** — an assistant and a companion share one installation,
  and a researcher can turn on a single feature for an A/B run, the way a profile
  already opts into the self-model tools. The `get_last_user_message_time` fix is a
  defect fix and applies to everyone, outside any switch.
- **F1: (a)** — time and memory first.
- **F2: (a)** — a row on every user message, full where the day changes and compact
  within a day — **off by default**, a profile switch, in place of the "on by
  default" the fork proposed.
- **F3: (a)** — notes about the person in a block of their own, behind a profile
  switch, after a planted-fact probe with a go/no-go.
- **F4: (a)** — the three earlier conversations, with `chat://` addresses when the
  profile offers `chat_read`; a switch of its own.
- **F5: (a) only** — follow-ups the companion plans; a chat not in view is woken when
  the person opens it (i). Check-ins on a schedule (b) and a greeting from memory (c)
  are not built in this track.
- **F6: (a)** — notifications now; the bridge as a track with an ADR; Telegram first
  behind a channel interface, Matrix second (i).
- **F7: (a)** — a built-in card reader, keyed lorebooks deferred.
- **F8: (a)** — all three instruments: the self-model's history, fork then carry, the
  drift smoke.
- **F9: (a)** — §7's rules adopted as design constraints of C4 and C5.
- **F10: (a)** — a per-profile voice and replies spoken as they land (a profile
  switch, off by default); speech input with the bridge.

## 9. A sequence

F1 (a) was chosen — each stage a pull request or two, each behind its own probe
where it touches what the model reads, and every feature a profile switch that is off
by default:

1. **When it is** — C1: the time row and the tool fix. **Built** (#809): the rows are derived from the messages' timestamps
   when a request is built rather than stored, which keeps the prefix as stable and
   needs no chat-format step; the shipped form was measured first (§3.3); three
   `#[ignore]` smokes, live GO on Gemma 4 31B and Qwen 3.8 27B. Left for later: dates
   in the digests the rolling summary and the reflection read.
2. **What it knows about you** — C2 (a) behind a planted-fact probe with a go/no-go,
   C2 (b), and C3, the memory screen, in the same stage (§7). **C2 built**: the probe
   (§3.4) said go for notes picked by relevance alone and no-go for a "latest notes"
   fallback; two profile switches, `notes_in_prompt` and `earlier_chats`, live GO on
   Gemma 4 31B and Qwen 3.8 27B. **C3 built** in the stage's second pull request:
   `/notes` — the notes listed, searched, opened with their links and trace, edited
   and deleted, and the notes and observations this conversation's prompt carries
   marked (spec §11.11).
3. **It comes back** — C4 (a) follow-ups with the rules of §7, and C5 (a)
   notifications, which cost little and give a follow-up somewhere to go. C4 (b)
   check-ins and C4 (c) a greeting from memory are not in this track (F5).
   **C4 (a) built**: the probe (§3.5) said go — a plan for what the person said and
   almost never for anything else, a good message when one falls due and silence
   when it is moot, on all five models, with a weekday check for the one that
   miscounts days; `followup_plan`/`followup_cancel` behind the *Follow-ups* switch,
   quiet hours and a daily cap on by default, a woken turn that may stay silent
   without a trace, and the pending ones on the tasks screen (spec §9.3.6).
   **C5 (a) built**, the stage's second pull request: *Notify me*, a profile switch,
   sends a notification when the companion writes unasked and the person is not
   looking — the terminal's focus reports, or a conversation not in view — as a
   terminal escape (OSC 777 by default, OSC 9 for iTerm2 and kitty; one, never
   both), the bell (what tmux passes on with no setting), and a ntfy push with its
   token in the key store; only who wrote and where unless the person asks for the
   words (spec §11.12). No model reads anything new, so no probe: the bytes are
   pinned by tests, a push by a listener of the tests' own, and the Windows console
   with VT processing on — as the app runs it — showed nothing of any of them.
   The owner's look on Windows Terminal 1.24 found no notification: OSC 777 ships
   only from Preview 1.26 (2026-10-02), opt-in (`compatibility.allowOSC777`) — the
   June merge this had been read from is not a release. A probe in a real Windows
   Terminal window confirmed the rest of the path: minimizing reports focus lost,
   restoring focus gained.
4. **The instruments** — C9 history, C8 fork then carry, C10 the drift smoke.
5. **Characters and voice** — C6 cards; C7 (a) a per-profile voice and replies
   spoken as they land.
6. **Reach** — the bridge track: an ADR first (turns addressed to a chat not on
   screen, the tool set of a bridged turn, the messenger's privacy), then Telegram,
   then speech input through its voice messages (C7 (b)).

Stages 1–3 are the companion; 4 is what makes it a research platform; 5–6 are what
make it a presence. None of it changes what an assistant profile or the default
install does: every feature is a profile switch, off until the profile turns it on
(the owner's decision, §8) — only the `get_last_user_message_time` fix reaches
everyone, being a defect fix.

## 10. Sources

**In the repository.** spec §6.4, §6.6, §9.3, §9.3.2, §9.11, §9.14, §10, §11.9, §17;
architecture §9.9; [background-subagents.md](background-subagents.md) §4.4 and F2;
[prompt-caching.md](prompt-caching.md) §2.2; [self-model-prompts.md](self-model-prompts.md);
[promotion.md](promotion.md) §8; [the self-model idea bank](../history/self-model.md);
[self-model-observations.md](../history/self-model-observations.md);
ADR [0006](../decisions/0006-data-schema-versioning.md),
[0007](../decisions/0007-plugins-mcp-host-import-format.md),
[0008](../decisions/0008-api-key-storage.md),
[0009](../decisions/0009-tts-speech-synthesis.md); the code named in §2 —
`request.rs`, `generation.rs::injection_recent`, `notes/self_notes.rs`,
`background_runs.rs`, `introspection.rs`, `features/compaction.rs`,
`rename_chat.rs`, `shared/instance.rs`, `app/events.rs`, `features/import.rs`,
`storage/db/`.

**Measurements.** §3.1 is a content-free count over the dev corpus; §3.2 a probe
through OpenRouter on 2026-10-21 — 360 replies across three models, graded by a
fourth. The scripts are not committed; each is a page of Python, and the prompts,
arms and rubric are those quoted in §3.2.

**Outside.** Linked where they are used — §4 (products, card formats), §5 (papers,
laws), C5 (messengers, local-API security) and C12 (terminal images). Three surveys
by sub-agents produced the candidates; every figure kept here was checked against its
source on 2026-10-21, and what could not be is marked *(reported)* or left out.
