+++
title = "Fabula: the story engine that lost to one writer"
description = "mindfork's story tools were first built as a simulation: characters who know only what they witnessed, a World Master ruling on every deed, an Author keeping one of three versions of each scene. Then 108 stories were judged blind, and one writer revising its own draft won 33 pairs of 36. What was built, how it was measured, and what stayed."
weight = 11

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# article must not arrive there as news. `page.html` reads it for the
# byline and for schema.org `datePublished`.
published = "2026-11-09"
+++

*The story tools in mindfork were designed as a simulation of characters, and
built that way. An experiment whose rule was written before the code compared
the simulation with one model revising its own draft, and the simulation lost.
This is what was built, how the comparison was run, and what is left.*

Since [0.24.0](/blog/mindfork-0-24-0/) mindfork can write a story with you. You
attach your sketches, and the assistant composes the story's canon from them:
the world's rules, the places and the ways between them, the cast with their
goals and secrets, the chapters. Once you approve it, each chapter is written in
the background and lands in the chat for you to accept or send back. The writer
behind a chapter is about the simplest design there is. One model drafts the
chapter, reads it as an editor would, and writes it again, up to 24 times. The
feature is experimental and off by default.

That writer was built as the control group. The feature was designed around
something far more elaborate, a track called Fabula. Every character was an
agent of its own that knew only what it had witnessed. A World Master ruled on
each deed and kept the record of what happened. An Author had every scene played
three times, kept one version and wrote the chapter from it, and reviewers read
the result. The design carried a condition from its first page: beat one writer
working alone, at equal cost, in a blind comparison, or stop. It was built, it
worked as designed, and its stories lost 33 pairs of 36. The simulation, some
25,000 lines, was removed the next day.

## The bar was one writer

The obvious way to have models write fiction is a pipeline of roles: a writer,
then an editor, then a proofreader. Pipelines like that have a record of losing
to one model that iterates on its own draft in a single session, and the reasons
are documented. Context is lost at each hand-off, so an editor sees the text and
not the intent behind it
([Cognition](https://cognition.com/blog/multi-agents-working)). Agents that read
each other's drafts converge on the same ideas
([LLM Review](https://arxiv.org/html/2601.08003)). And a taxonomy of multi-agent
failures sorts them into flaws of system design, misalignment between agents and
missing verification, which a better prompt does not remove
([MAST](https://arxiv.org/abs/2503.13657)).

Fabula's bet was that fiction offers a split that avoids those failures, because
it does not split the writing. Narratology separates the *fabula*, what happened
in the story's world in the order it happened, from the *syuzhet*, the way the
text tells it. A simulation can produce the first, and one author can keep the
second whole. The reason to simulate at all is that a single model moving every
character knows what all of them know, while the people in a story act on
unequal knowledge ([Zhou et al.](https://arxiv.org/html/2403.05020v2) measure
that gap for simulated conversations). With one mind behind the whole cast,
secrets leak from one character to another and everyone turns agreeable. Agents
with separate contexts can keep a real secret. Two published systems,
[BookWorld](https://arxiv.org/html/2504.14538v1) and
[StoryBox](https://arxiv.org/html/2510.11618), simulate stories this way, and
Fabula took its mechanisms from them: a world agent that answers each action by
the world's rules, event records that say who was present, a chance that a
character breaks its routine. From Cognition's essay it took one more rule: many
agents may contribute, but every store has exactly one writer.

The condition for success was written with the design. Stories would be compared
in pairs from the same premise, read blind. Of 36 pairs the simulation had to be
preferred in at least 24, which is a one-sided binomial test at p ≈ 0.03, and it
had to do so twice: against one writer alone, and against its own pipeline with
the simulation switched off. Failing the first would stop the work. Failing only
the second would remove the simulation and keep the pipeline around it.

## What was built

There were four kinds of agent, each with a different view of the story.

- **Characters**, one agent per cast member. A character is shown its own sheet
  (goals, voice, secrets), the public descriptions of the others, the observable
  part of the events it witnessed, and its own private reflections. It returns a
  proposal, a thought and a deed, and commits nothing.
- **The World Master** sees the rules, the map, the whole cast with their
  secrets, and the scene so far. Of each proposal it sees the deed and never the
  thought. It decides which deeds happen and with what effect, and it is the
  only writer of the event log.
- **The Author** sees everything and is the only writer of the text. It sets
  each scene's circumstances (who is present, where, what has just happened,
  what pressure applies) and never what a character will do.
- **Reviewers** write notes and nothing else. A Reader sees only the published
  text, and a Continuity checker reads the text against the log and the canon.

A scene is played three times from the same snapshot of the world, in branches
that never see each other. In each round every character present proposes at
once. The World Master picks one feasible deed as the round's initiative, then
rules on the reactions of those who witnessed it. With probability 0.3 a
character is told to break its routine, and every third round each character
writes a private reflection. A scene ends after eight rounds, after two rounds
in which nobody acts, or when the World Master judges its pressure resolved. The
Author then reads all three records in full and keeps one, scoring them on
tension, surprise, truth to character and consequences. It plans the next scene
from where the kept one ended. After the last scene it writes the chapter from
the kept branches, the reviewers return their notes, and the Author decides on
each note and revises, at most twice.

The world itself is held by code. The World Master proposes an event and the
engine commits it, after checking it against the state folded from the log: the
characters named are at that place, an item given is in the giver's hands, a
route exists, time does not run back. The witnesses of an event are computed and
never asked for: everyone present for a public deed, the addressees of a
whisper, the actor alone for a hidden one. A character's memory is a view over
the log filtered by those witness lists, so a secret has no path to someone who
was not there to hear it. To measure that, every character sheet carried a
planted token, and an audit read every model call back from the call log,
looking for a token shown to a character, or written by one, before its secret
had reached them.

Every id is derived from position (chapter 1, scene 2, branch 1, event 3), and
every model call is recorded under such an address. A recorded scene therefore
replays identically, and a run killed at any point resumes at the first step
that did not finish.

## What held

The machinery did what it was designed to do.

- **No secret leaked.** The first live test played 20 scenes in three branches
  each on Claude Haiku 5.5, Qwen 3.8 27B and Gemma 4 31B, then a smaller set on
  a local llama.cpp. No token appeared out of place in any run, while the World
  Master revealed between 9 and 38 secrets in each, and every one reached the
  characters it was revealed to and nobody else. In the final experiment the
  count stayed at zero.
- **The replies kept their shape.** Every role answers in JSON constrained by a
  schema. Of 6,418 first attempts in that test, three missed (two replies that
  looped to the token cap and one upstream error), and the retry took each.
- **Runs survived being killed.** A chapter stopped at random ten times resumed
  each time. Later a power cut ended a pilot with five stories in flight, and
  the batch picked every one up from its stores.

Two things built for the simulation are now the app's own. A request can carry
an output schema on every provider's wire, and building that found that
`gemini-2.5-flash` ignores Gemini's `responseFormat` in silence and answers in
prose, while `responseJsonSchema` holds. And the characters, who were to run
with thinking off, showed that "off" had never reached Claude: its 5.5 models
think when a request does not mention thinking at all. The fix serves every chat
turn with thinking off.

## The first story, and what its readers found

The first complete story was made the way a user would make one, in the chat,
from sketches: a northern island, a lighthouse keeper who did not light the lamp
the night her brother drowned, an inspector sent to close the light, a
shopkeeper who hid the logbook. On Claude Sonnet 5.5 it came to two chapters and
2,696 words of Russian, after 875 model calls, 14 million prompt tokens and
about $31 at list price.

The project's owner read it and judged the stage passed, then gave it to two
other models to review, and the two reviews named the same strengths and the
same faults. The prose was spare and concrete. The continuity of things and numbers
held: a sign blown down in one scene is picked up in the next, and three days
until the ferry are two by the second evening. The three secrets were kept and
spent on time, with the reader shown what the inspector misses.

What failed was the story's deeper logic. Clues were planted and never paid off:
footsteps on the path end the first chapter and never turn out to be anyone's. A
father "lost a few years ago" lies under an old, mossed cross. The second
chapter opens all three secrets in one evening and a morning, and everyone
forgives everyone. Characters state their plans and boundaries aloud, as if
reporting to the engine.

So the layer the simulation guarantees, the state of the world and who knows
what, held, and the faults were in the bookkeeping above it. That bookkeeping
was built before the experiment. A register of open threads and of stated facts
of time, age and kinship is written from each chapter's text alone, and later
chapters are checked against it. A staging pass reads who is where and whose
pronoun is whose. New rules of craft say that a scene is a collision which costs
someone something, and that a line of dialogue is a deed. Run again from the
same sketches, the story paid off all six threads its first chapter opened, and
no reply opened on thanks where the first version had five. The rules of the
prose and the reviewers' questions were shared with the systems the simulation
would be compared to, so they gained from this too.

## The comparison

Three systems wrote stories from the same premises, on the same model.

- **The simulation**: everything above.
- **Author and reviewers**: the same loop with the simulation switched off. The
  Author plans the same scenes and invents what happens in them, and the
  reviewers and the two revisions are unchanged. This system isolates what the
  simulation adds.
- **One writer**: one session. A draft, then turns of critique and of revision,
  each seeing the whole session, until it has spent what the simulation spent on
  the same premise.

The protocol was fixed in advance, and most of its details exist to close a way
of fooling oneself.

- **Twelve premises, frozen.** Three each of a mystery, a clash of goals, a
  villain, and a quiet story of a relationship. Each is a complete canon with
  three characters and five connected places, committed to the repository with
  its SHA-256. Three seeds per premise make the 36 pairs. The stories are in
  Russian, one chapter of 1,200 to 2,500 words.
- **Equal cost is money.** Tokens are a poor unit across these designs, because
  one long session and hundreds of short calls pay different prices for the same
  count. On a caching provider the session read 86–93% of its prompt from cache,
  where the simulation's calls read 12–23%. So the one writer stops at the
  simulation's dollars, at list price.
- **A model strong enough to tell the systems apart.** A pilot on Qwen 3.8 27B,
  the cheapest candidate, produced stories that were mostly weak under every
  system, and stories too weak to tell apart are judged by chance. A loss there
  would have been noise. Claude Sonnet 5.5 wrote well under every system and
  would have cost over $800. Claude Haiku 5.5 read far ahead of Qwen at about
  $50 for the run, and wrote all 108 stories.
- **Three judges who must agree with themselves.** The plan had people judge a
  calibration set first. The owner replaced them with three flagship models of
  three families, Claude Fable 5.1, GPT-6 Astra and Gemini 4 Argon, each at its
  highest reasoning effort, since none of the readers at hand was a literary
  professional. Every pair is read in both orders, and a judge's vote counts
  only when both readings choose the same story. A pair goes to the side with
  more votes, and a tie is not a win for the challenger. As a control, each
  judge was shown a story against its own paragraphs shuffled and had to pick
  the original. All three did, 24 times of 24.

## Signs before the verdict

A pilot, read blind by the owner, did not put the simulation first anywhere.
Before the full run, each failure it showed was traced to the calls that made it
and repaired.

- **Polish was not the missing piece.** The simulation's story on Sonnet was
  handed to the one writer's loop of critique and revision as its draft. Eight
  revisions later it had gone from 1,655 words to 1,041, the first revision had
  kept little of the original's sentences, and it read no better than the one
  writer's own story. A loop of critique and revision rewrites a story it did
  not invent into one of its own, and here it erased what the simulation had
  contributed.
- **The Author was retelling.** The call that drafts a chapter carried a prompt
  of 94,247 characters: the record of four scenes, some 14,000 words in English,
  round by round, with every character's thoughts. From it the Author was to
  write 1,200 to 2,500 words of Russian, told to "tell what the record holds".
  It did. Conversations came out as "he asked whether, she answered that". The
  narrator gave a secret away before its keeper confessed, because the thoughts
  are in the record. A plan that led nowhere kept its scene. The instruction
  became "the record is your material, not a text to retell", with rules for
  what to cut, and the draft replayed on the same record told its secrets on
  time.
- **The characters played for time.** In five of the simulation's trial stories,
  one chapter reached the goal its canon set. Ten of twenty scenes had run to
  the eight-round cap. In a story of one chapter, the Author's picks put the
  outcome off "to the next chapter" four times in ten, since nothing told it
  this chapter was the last. The world kept no clock: one World Master struck
  nine in the first scene, then a quarter past and half past, and the director
  due at nine never came. Where the Author had narrowed a goal to one
  character's decision, the simulated character refused it in all three runs.
  The fixes named the chapter's place in the arc, added the goals to the pick's
  score, put the hour in every scene's opening, and allowed one short closing
  scene at the deadline itself. Measured on six stories a side, three chapters
  reached their goals where two had.
- **Nobody read the last revision.** Three of four slips in one set of stories,
  a changed patronymic among them, came from the final revision, which the loop
  accepted unreviewed. It is now reviewed like every version before it.

The simulation went into the experiment in the best shape its own failures
could suggest.

## The verdict

The full run wrote 108 stories on a small cloud machine in 3 hours 34 minutes
and 22,884 model calls. Five of the simulation's stories did not finish. Four
ran out of their chapter's token budget, and one stopped on refusals: 30 times,
and nowhere else in the run, Claude Haiku declined to write the private
reflection of one character, a station doctor weighing her patient's heart
against the single place on a plane. Rather than let plumbing decide five pairs,
those stories were completed, with a refused reflection now left out and
counted, and the one writer's sessions on the same premises were carried on
until the spending matched again. The 36 stories of each system cost $28.32 for
the simulation, $30.57 for the one writer, and $1.64 for the author and
reviewers.

<figure class="chart" aria-labelledby="fabula-verdict-title">
<p class="chart-title" id="fabula-verdict-title">A challenger needed 24 pairs of 36, and the best result was 5</p>
<p class="chart-sub">Pairs won by the first-named system in blind judging by three models, of 36 per comparison</p>
<svg viewBox="0 0 720 266" role="img" aria-labelledby="fabula-verdict-desc">
<desc id="fabula-verdict-desc">Of 36 pairs in each comparison, the simulation won 3 against one writer and 3 against the author and reviewers, and the author and reviewers won 5 against one writer. Each needed 24 to pass.</desc>
<line class="grid" x1="702.8" x2="702.8" y1="32" y2="232"/>
<line class="cap" x1="471.2" x2="471.2" y1="32" y2="232"/>
<text class="note" x="471.2" y="20" text-anchor="middle">24 pairs to pass</text>
<text class="label" x="8" y="54">Simulation vs one writer</text>
<path class="bar" d="M8 66 h53.9 a4 4 0 0 1 4 4 v8 a4 4 0 0 1 -4 4 h-53.9 z"><title>Simulation vs one writer: 3 pairs won, 33 lost, none tied</title></path>
<text x="75.9" y="74" dominant-baseline="central">3 won · 33 lost</text>
<text class="label" x="8" y="124">Simulation vs author and reviewers</text>
<path class="bar" d="M8 136 h53.9 a4 4 0 0 1 4 4 v8 a4 4 0 0 1 -4 4 h-53.9 z"><title>Simulation vs author and reviewers: 3 pairs won, 30 lost, 3 tied</title></path>
<text x="75.9" y="144" dominant-baseline="central">3 won · 30 lost · 3 tied</text>
<text class="label" x="8" y="194">Author and reviewers vs one writer</text>
<path class="bar" d="M8 206 h92.5 a4 4 0 0 1 4 4 v8 a4 4 0 0 1 -4 4 h-92.5 z"><title>Author and reviewers vs one writer: 5 pairs won, 29 lost, 2 tied</title></path>
<text x="114.5" y="214" dominant-baseline="central">5 won · 29 lost · 2 tied</text>
<text class="tick" x="8" y="254" text-anchor="start">0</text>
<text class="tick" x="239.6" y="254" text-anchor="middle">12</text>
<text class="tick" x="471.2" y="254" text-anchor="middle">24</text>
<text class="tick" x="702.8" y="254" text-anchor="middle">36</text>
</svg>
<figcaption>The 36 pairs are 12 premises by 3 seeds, each read in both orders by each judge. A judge votes only when both of its readings choose the same story, and a tie is not a win.</figcaption>
</figure>
<details class="chart-data">
<summary>The counts, and each judge alone</summary>
<table><thead><tr><th>Comparison</th><th class="num">Won</th><th class="num">Lost</th><th class="num">Tied</th><th class="num">Fable</th><th class="num">Astra</th><th class="num">Argon</th></tr></thead><tbody>
<tr><td>Simulation vs one writer</td><td class="num">3</td><td class="num">33</td><td class="num">0</td><td class="num">0</td><td class="num">3</td><td class="num">2</td></tr>
<tr><td>Simulation vs author and reviewers</td><td class="num">3</td><td class="num">30</td><td class="num">3</td><td class="num">1</td><td class="num">5</td><td class="num">1</td></tr>
<tr><td>Author and reviewers vs one writer</td><td class="num">5</td><td class="num">29</td><td class="num">2</td><td class="num">2</td><td class="num">6</td><td class="num">6</td></tr>
</tbody></table>
<p>The last three columns are the pairs, of 36, that each judge's votes alone would have given the first-named system.</p>
</details>

The judges read the two comparisons the rule named. When the simulation had lost
both, they read the third, which the rule needed in order to choose what would
replace it. None of the three results is close.

The judges also rated every story from 1 to 10 on plot, character, surprise and
prose. Across those four measures the simulation's stories averaged 4.2 to 5.1,
with surprise the lowest, and the one writer's 6.5 to 7.3. Surprise was what
three branches and broken routines were there to produce.

The checks on the judging came back clean. Length did not decide it: the
simulation's story was the longer one in 20 of its 36 pairs against the one
writer, and was chosen in 16 of those pairs' 120 readings. Where two judges both
had a vote on a pair they almost always agreed, on 58 pairs of 61, 54 of 55 and
58 of 58. The both-orders rule mattered most on the closest comparison, where
Gemini 4 Argon chose whichever story was shown first in 56 of its 72 readings.
The rule discarded its vote on 20 of the 36 pairs instead of counting it. The
ratings, for their part, moved with the company a story kept: the one writer's
stories scored 6.5 to 7.3 beside the simulation's and 6.2 to 7.0 beside the
closer rival's.

The judges' written reasons were counted by keyword over the 432 readings of the
first two comparisons. The fault named most often against a losing story,
whichever system wrote it, is a fact that does not hold, such as a stone sold at
the bazaar that lies in the hero's palm. It appears in 175 of the 382 readings
the simulation lost. One fault is the simulation's own: English words spelled in
Cyrillic in the Russian prose, named in 74 of those readings, most likely the
English record showing through. And in 22 readings the judges note that the
scene the story turns on is missing, having fallen between two scenes that were
played.

Reading cost more than writing. The three judges made more than 700 readings for
$259.74, a little over four times the $60.53 the stories cost. The writer was a
small model, and the readers were the largest available, reasoning as long as
they could.

## Why it lost, as far as the records show

The experiment says that the simulation lost. It does not say which part is to
blame. The ablations that would apportion it (one branch instead of three, no
reviewers, characters who see everything) were built, and were to be judged only
after a win. This is what the records support.

- **A consistent world did not make a consistent story.** The log was checked
  event by event, and the stories were still faulted for facts that do not hold.
  The reader meets the prose, and the prose was a six- to tenfold compression of
  the record, made in one pass.
- **Simulated characters protect what a story needs them to spend.** Given goals
  and secrets, they stall: in the trial stories half the scenes ran out of
  rounds. An author has a character sign the paper because the story needs it
  signed. The simulated character would not.
- **The money went to events, and little was left for sentences.** At equal cost
  the simulation made several hundred calls to produce a record and had two
  revisions for the text. The one writer spent everything on the text, 25
  versions on average, each written with its own earlier intentions in view.
  That is the hand-off argument this article began with, and it held again.
- **Inside the pipeline, the simulation subtracted.** The same Author and
  reviewers without it won 30 pairs of 36 against it, for 6% of the money.

That pipeline then lost to the one writer, 29 pairs to 5, but at about 5% of
the one writer's cost. So that result says less about architecture than about
how far 25 revisions go.

## What the result does not show

- **One model wrote everything.** Haiku 5.5 was chosen for its price. A stronger
  Author might make better use of a record. In the pilot Sonnet's simulation
  story was good, though still not the best of its three.
- **The stories are one chapter long.** Keeping track of who knows what and
  where things are should matter more across a book than across 1,200 words. The
  long form was the next stage and was never built, so this is untested, not
  refuted.
- **The prose is Russian, and the canon, the prompts and the record are
  English.** Some of the simulation's loss is translation. A comparison in
  English, or under Russian prompts, was planned and not run.
- **The judges are models.** They agreed with each other and with themselves,
  and the owner's blind readings in the pilot pointed the same way, but no panel
  of people read the 108 stories.

## What stayed

The rule left one more decision, whether to keep the story tools at all once
neither challenger had passed, and the owner kept them, running the one writer.
A chapter is now a session of its own: the draft and up to 24 revisions, each
after a critique, with the chapters you accepted before it in front of the
writer whole, bounded by a token budget and by the model's context window. It
was checked end to end on a local Gemma 4 26B with a window of 65,536 tokens: a
story of two chapters and 2,264 words in 35 minutes, with a change asked for at
the first chapter's checkpoint written in one turn.

Most of what surrounds the writer was built for the simulation and survived it.

- **The canon and its checks.** One of them asks that every place on the map can
  be reached from every other. It was added after a model composed a world with
  no roads and a scene was refused 51 minutes into a run.
- **Your answer in your own words.** An answer the story waits for is read from
  the messages you typed, never from the assistant's account of them. In live
  trials no model tried to approve a canon on the person's behalf, and all 13
  approval turns called the tool.
- **The background run.** A chapter runs in the same seat as
  [a subagent's](/articles/background-runs/) run, says where it stopped, and
  resumes there when you ask.
- **The story's data.** Its database is in the app's backups.

The runner and the judging stay in the binary too, as a hidden command, for the
next comparison someone wants to make, and the twelve premises are still frozen
beside them. The last commit that holds the simulation is tagged
[`fabula/f-final`](https://github.com/vshylov/mindfork-rs/tree/fabula/f-final).
The pull request after it removed 83 files and 171 tests.

The whole track, from the design on October 31 to the removal on November 8,
took nine days, and every step of it is written down in the repository:
[the design](https://github.com/vshylov/mindfork-rs/blob/main/docs/research/fabula-engine.md),
[the plan](https://github.com/vshylov/mindfork-rs/blob/main/docs/history/fabula.md)
with its 28 decisions and what each stage measured,
[the decision record](https://github.com/vshylov/mindfork-rs/blob/main/docs/decisions/0014-fabula-story-engine.md),
and [the journal](https://github.com/vshylov/mindfork-rs/blob/main/docs/journal/stories.md),
which has the tables this article rounds. The essay on
[how this project is built](/articles/what-replaces-code-ownership/) has a
section called "Measure, and let the plan lose". This track is that section at
the size of a whole feature.
