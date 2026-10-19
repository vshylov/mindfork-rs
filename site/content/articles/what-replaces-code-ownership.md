+++
title = "What replaces code ownership when AI writes the code"
description = "257K lines of Rust, 754 PRs, zero read diffs, and four months of green metrics. How externalized memory, mutation testing, and CI gates replaced reading the code."
weight = 10

[extra]
# First published. Deliberately not a top-level `date`: that is what
# Zola puts into the Atom feed, which carries release news, and an
# article must not arrive there as news. `page.html` reads it for the
# byline and for schema.org `datePublished`.
published = "2026-10-19"
# The card a link to this page renders as on a social site, in place of
# the site-wide one (base.html, page.html).
og_image = "images/what-replaces-code-ownership.png"
og_image_alt = "Nobody owned this code. It didn't fall apart. And the metrics stayed green. CLAUDE.md grew to 994 KB on Aug 8 and was 12 KB the next day."
+++

*257K lines of Rust, 754 PRs, zero read diffs, and four months of green metrics. How externalized memory, mutation testing, and CI gates replaced reading the code.*

<img class="cover" src="/images/what-replaces-code-ownership.png" width="2000" height="1000" alt="Nobody owned this code. It didn't fall apart. And the metrics stayed green. Beside it, CLAUDE.md, the file every session loads: 994 KB on Aug 8, 12 KB the next day, under a 30 KB cap since. 124 days, 2,308 commits, 754 PRs, 4,291 tests, 0 Sonar issues.">

As AI writes a growing share of the world's code, the fear in engineering organisations has a name: loss of code ownership. Developers stop knowing what the code does, and when something breaks, nobody can reason about it. There have already been cases where that loss cost a product dearly. This is a story about a product where code ownership never existed in the first place. The product did not fall apart. It ships, it grows, and its quality metrics have stayed green for four months. I want to explain why, with numbers rather than impressions.

In June I started a project I had no business starting. I wanted a terminal AI chat with a memory: notes about you, a knowledge base over your files, and a model of itself that it revisits. I wanted it in Rust, because it had to run on Windows, Linux and a rented GPU pod as one native binary. Behind me were fifteen years of enterprise development on .NET, and no Rust at all: I had read the Rust book once and never written anything complicated in it.

Four months later the thing exists. It is called mindfork, it is open source, it ships installers for three platforms, and it has had 28 releases. Almost every line was written by an AI coding agent. I know the language well enough to read it, but early on I stopped reviewing the changes the models make, and I have not read most of the code since. By the usual definition, nobody owns this code.

The question people ask is whether it works. The question that interests me is why it did not rot. Anyone who has used a coding agent for longer than a weekend knows the failure mode. The code works, then it works a little less, then every new feature breaks two old ones, and the test suite turns into a pile of green lies. That did not happen here. New features do not break old ones. The static analyzer has reported zero issues for weeks, and keeping it that way costs nothing.

I went looking for the reason, with the agent's help, by measuring the repository instead of remembering it. The short answer surprised me. The quality did not come from the model. It came from a process in which every claim the agent makes has to be backed by something I can actually read: a number, a run on a real model, a test count, a dated decision. I was not going to verify the code line by line, so I made the code's quality legible without that.

I used no spec framework and no downloaded AGENTS.md. Every rule below grew out of something that went wrong in this repository. That turns out to matter.

## The short version

Six mechanisms, in the order the article takes them. Each gets its own section below, with the numbers from the repository rather than the principle alone.

- **The owner decides and verifies; the agent owns the code.** Every non-obvious choice comes to me as a list of options with a recommendation, and I pick. Every change to the engine, memory or tools runs against a real model before it merges.
- **Memory with a budget.** A 30 KB router file that CI keeps small, a journal of 644 entries split by subsystem, a lessons file that counts how often each trap recurred.
- **Measure, and let the plan lose.** 265 journal entries measure something; 107 record a rejection, whole stages among them.
- **Tests that must be able to fail.** Half the source is tests, checked by hand-run mutation testing; every CI gate is tested against a planted violation.
- **Two gates with different blind spots.** Four thousand unit tests that need no model, and 260 live smokes that need a real one.
- **Incident, lesson, rule, gate.** Every guardrail in the repository carries the incident that produced it, and a rule broken twice becomes a CI check.

## What the product actually is

mindfork is a terminal chat client for language models with a memory of its own. It runs local Gemma and Qwen models through llama.cpp, or any OpenAI-compatible server, and talks to OpenAI, Anthropic, Gemini, xAI, DeepSeek and OpenRouter in the cloud. One native binary for Windows, Linux and macOS. The code, the engineering journal and every number in this article are public in the [GitHub repository](https://github.com/vshylov/mindfork-rs).

What makes it a demanding thing to build is not the chat. It is what the assistant is allowed to do with tools:

- **Memory and knowledge.** It saves notes about you and the work, links them into a graph, merges, revises and supersedes them, and consolidates them over time. A [knowledge base over your files](/articles/vector-search/) is searched when you ask.
- **A model of itself, and of you.** The assistant keeps [a summary of who it is, its goals and its traits](/articles/self-model/), adds observations as it goes, and runs reflection passes that revise that model. It keeps a model of the person it is talking to as well.
- **Introspection.** It can read and change its own system message and its own sampling parameters in the middle of a conversation, and it knows what time it is.
- **Conversation management.** It can search and read other chats, read the compacted history of a long one, rewrite its own answer, or send a follow-up message on its own.
- **Doing things.** [Python in a sandbox](/articles/python-sandbox/), web search and page fetching, [a project workspace](/articles/code-workspace/) attached to a chat, subagents for a second opinion, directed dialogues between two models, MCP plugins, speech.

Every one of these sits behind a switch the user sets. And every one of them is a place where an agentic feature can quietly break another: a tool that edits the system message changes what every later tool sees. That is the codebase that had to stay coherent.

## The numbers, so you know what we are talking about

Everything below was measured on the main branch on 2026-10-16.

- **Time:** 124 days, 2026-06-14 to 2026-10-16.
- **Commits and pull requests:** 2,308 commits, 754 merged PRs, about six a day, every day.
- **Code:** 341 Rust files, 257K lines under src, about 54% of them test code.
- **Tests:** 4,291 test functions, 260 of them live smokes against real models.
- **SonarCloud:** 199K lines analysed, 0 bugs, 0 vulnerabilities, 0 code smells, 89% coverage, 0.6% duplication.
- **Documentation:** 7.8 MB of Markdown beside 10.5 MB of source.
- **Commits that touch only documentation:** 39%.
- **Models named in commit trailers:** eight, from Opus 4.8 in June to Fable 5.1 and Opus 5.5 in October.

Two of those lines explain most of what follows. More than a third of the commits never touched code. And the documentation is three quarters the size of the codebase it describes, a ratio that has held since the third week. That is not a byproduct of the work. That is the mechanism.

## What I did instead of reviewing the code

I did four things, and the repository shows them better than my memory does.

**I decided the forks.** The agent is required to lay out every non-obvious decision as a list of options with a recommendation, and to put the list to me as a question before it writes code. I pick. The engineering journal records 146 such decisions with a date, and later work cites them like case law: "Keep it opt-in, F2(b) of 2026-07-30 stands."

Most of the time the recommendation is the best option and I take it. Sometimes it is not. When the agent proposed to refresh a settings hint that named specific models, I said no: a hint that names a model goes stale, so ask the provider for its catalogue instead. That became a feature. The value of the ritual is not that I argue with the agent. It is that the agent has to enumerate the alternatives before it commits to one, and I get to see the whole design space without reading a line of the diff.

**I ran the live stand.** A unit test cannot tell you whether Gemma 4 actually calls your tool, or that llama.cpp sends the usage chunk after the finish reason. So every change that touches the engine, the memory or the tools has to be run against a real model, and the result has to be written down in the journal: "Smoke, GO (2026-08-12, gemma-4-31B + bge-m3, and all four clouds with real keys)." The GPU is mine. The Docker containers with Ollama and LM Studio are mine. The rented pods are mine. A full run of the rented gate takes about half an hour of wall clock.

**I merged every pull request by its artefacts, not its diff.** The [PR template](https://github.com/vshylov/mindfork-rs/blob/main/.github/pull_request_template.md) demands a test count, the list of live smokes and the stack they ran on, a documentation checklist, and a "Models" section naming which model wrote what. I read those. I read the journal entry. I do not read the diff.

**I set policy.** Who the product is for, what a message may and may not say to the user, which risks stay off the table. Those are product decisions, and product is the thing I actually know.

So not reviewing the code turned into a division of labour. The agent owns the code. I own the behaviour, the decisions and the facts.

## Memory with a budget

An agent has no memory between sessions. Whatever it learned yesterday about this codebase, its conventions and its traps, is gone this morning unless it is written down somewhere it will read. The obvious fix is to write everything into the one file the agent loads at the start of every session. I did exactly that, and here is what it looked like by August.

<figure class="chart" aria-labelledby="claude-md-size-title">
<p class="chart-title" id="claude-md-size-title">The file every session loads grew to 1&nbsp;MB, then became a 30&nbsp;KB router</p>
<p class="chart-sub">CLAUDE.md size in KB, one point per week, June to October 2026</p>
<svg viewBox="0 0 720 360" role="img" aria-labelledby="claude-md-size-desc">
<desc id="claude-md-size-desc">CLAUDE.md grew from 21 KB on June 14 to 995 KB on August 8, fell to 12 KB the next day, and has stayed under the 30 KB cap that CI enforces ever since; it was 29 KB on October 16.</desc>
<line class="grid" x1="72" x2="696" y1="253" y2="253"/>
<line class="grid" x1="72" x2="696" y1="182" y2="182"/>
<line class="grid" x1="72" x2="696" y1="111" y2="111"/>
<line class="grid" x1="72" x2="696" y1="40" y2="40"/>
<text class="tick" x="62" y="328.5" text-anchor="end">0</text>
<text class="tick" x="62" y="257.5" text-anchor="end">250</text>
<text class="tick" x="62" y="186.5" text-anchor="end">500</text>
<text class="tick" x="62" y="115.5" text-anchor="end">750</text>
<text class="tick" x="62" y="44.5" text-anchor="end">1,000</text>
<line class="base" x1="72" x2="696" y1="324" y2="324"/>
<text class="tick" x="72" y="352" text-anchor="start">Jun 14</text>
<text class="tick minor" x="177.7" y="352" text-anchor="middle">Jul 5</text>
<text class="tick" x="283.4" y="352" text-anchor="middle">Jul 26</text>
<text class="tick minor" x="389" y="352" text-anchor="middle">Aug 16</text>
<text class="tick" x="494.7" y="352" text-anchor="middle">Sep 6</text>
<text class="tick minor" x="600.4" y="352" text-anchor="middle">Sep 27</text>
<text class="tick" x="696" y="352" text-anchor="end">Oct 16</text>
<line class="cap" x1="72" x2="696" y1="315.5" y2="315.5"/>
<text class="note" x="696" y="307.5" text-anchor="end">30 KB cap in CI</text>
<path class="series" d="M72 318 L107.2 295.9 L142.5 264.2 L177.7 226.8 L212.9 168.6 L248.1 116.4 L283.4 149.4 L318.6 80.7 L348.8 41.5 L353.8 320.6 L389 317.3 L424.3 318.6 L459.5 316.3 L494.7 315.6 L529.9 315.6 L565.2 315.8 L600.4 315.8 L635.6 315.7 L670.8 315.7 L696 315.7"/>
<g class="pt"><circle class="dot" cx="72" cy="318" r="3"/><circle class="hit" cx="72" cy="318" r="12"><title>Jun 14: 21,092 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="107.2" cy="295.9" r="3"/><circle class="hit" cx="107.2" cy="295.9" r="12"><title>Jun 21: 99,007 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="142.5" cy="264.2" r="3"/><circle class="hit" cx="142.5" cy="264.2" r="12"><title>Jun 28: 210,589 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="177.7" cy="226.8" r="3"/><circle class="hit" cx="177.7" cy="226.8" r="12"><title>Jul 5: 342,286 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="212.9" cy="168.6" r="3"/><circle class="hit" cx="212.9" cy="168.6" r="12"><title>Jul 12: 547,214 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="248.1" cy="116.4" r="3"/><circle class="hit" cx="248.1" cy="116.4" r="12"><title>Jul 19: 730,819 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="283.4" cy="149.4" r="3"/><circle class="hit" cx="283.4" cy="149.4" r="12"><title>Jul 26: 614,684 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="318.6" cy="80.7" r="3"/><circle class="hit" cx="318.6" cy="80.7" r="12"><title>Aug 2: 856,661 bytes</title></circle></g>
<g class="pt"><circle class="key" cx="348.8" cy="41.5" r="5.5"/><circle class="hit" cx="348.8" cy="41.5" r="12"><title>Aug 8: 994,562 bytes</title></circle></g>
<g class="pt"><circle class="key" cx="353.8" cy="320.6" r="5.5"/><circle class="hit" cx="353.8" cy="320.6" r="12"><title>Aug 9: 11,969 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="389" cy="317.3" r="3"/><circle class="hit" cx="389" cy="317.3" r="12"><title>Aug 16: 23,758 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="424.3" cy="318.6" r="3"/><circle class="hit" cx="424.3" cy="318.6" r="12"><title>Aug 23: 19,123 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="459.5" cy="316.3" r="3"/><circle class="hit" cx="459.5" cy="316.3" r="12"><title>Aug 30: 27,169 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="494.7" cy="315.6" r="3"/><circle class="hit" cx="494.7" cy="315.6" r="12"><title>Sep 6: 29,530 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="529.9" cy="315.6" r="3"/><circle class="hit" cx="529.9" cy="315.6" r="12"><title>Sep 13: 29,435 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="565.2" cy="315.8" r="3"/><circle class="hit" cx="565.2" cy="315.8" r="12"><title>Sep 20: 28,947 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="600.4" cy="315.8" r="3"/><circle class="hit" cx="600.4" cy="315.8" r="12"><title>Sep 27: 28,750 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="635.6" cy="315.7" r="3"/><circle class="hit" cx="635.6" cy="315.7" r="12"><title>Oct 4: 29,338 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="670.8" cy="315.7" r="3"/><circle class="hit" cx="670.8" cy="315.7" r="12"><title>Oct 11: 29,263 bytes</title></circle></g>
<g class="pt"><circle class="dot" cx="696" cy="315.7" r="3"/><circle class="hit" cx="696" cy="315.7" r="12"><title>Oct 16: 29,363 bytes</title></circle></g>
<text class="label" x="336.8" y="46.5" text-anchor="end">995 KB on Aug 8</text>
<text class="label lift" x="365.8" y="291.5">12 KB the next day</text>
</svg>
<figcaption>Read from the repository's git history: the size of CLAUDE.md at the last commit of each week, plus the day it peaked and the day after. Hover a point, or open the table below, for the exact bytes.</figcaption>
</figure>
<details class="chart-data">
<summary>The numbers behind the chart</summary>
<table><thead><tr><th>Date</th><th class="num">Bytes</th><th>Commit</th></tr></thead><tbody>
<tr><td>2026-06-14</td><td class="num">21,092</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/ab48c4b1b2c0dbbae65fd91ba36826fb68e810ee"><code>ab48c4b1</code></a></td></tr>
<tr><td>2026-06-21</td><td class="num">99,007</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/7fc0c227ceb90cdc2b3f147605c868bd508aa056"><code>7fc0c227</code></a></td></tr>
<tr><td>2026-06-28</td><td class="num">210,589</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/22e915c787e1456aea85fe2fef7efe3e661bf9d0"><code>22e915c7</code></a></td></tr>
<tr><td>2026-07-05</td><td class="num">342,286</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/677de7b61f837ad6ea49dd80dc9fda24fbfe7ee7"><code>677de7b6</code></a></td></tr>
<tr><td>2026-07-12</td><td class="num">547,214</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/5916aca9dee74cb85fa5d359e28090d80e01322d"><code>5916aca9</code></a></td></tr>
<tr><td>2026-07-19</td><td class="num">730,819</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/7bff6bd3a54036311380c92b05f8566b9c01b4c2"><code>7bff6bd3</code></a></td></tr>
<tr><td>2026-07-26</td><td class="num">614,684</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/893f24249ba8e28885a3c7bcb8fc3797f98f5ddf"><code>893f2424</code></a></td></tr>
<tr><td>2026-08-02</td><td class="num">856,661</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/c220356056785edfc9a8495fc3508888d6044d74"><code>c2203560</code></a></td></tr>
<tr><td>2026-08-08</td><td class="num">994,562</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/15a903d1eaadfe17b604ef3a984da3d9c3dc7030"><code>15a903d1</code></a></td></tr>
<tr><td>2026-08-09</td><td class="num">11,969</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/dbef0b24fc7a3e7e260e29e013008c540dc49a00"><code>dbef0b24</code></a></td></tr>
<tr><td>2026-08-16</td><td class="num">23,758</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/0369d1acd2d12af971a03dd655d99b8f60f3dd53"><code>0369d1ac</code></a></td></tr>
<tr><td>2026-08-23</td><td class="num">19,123</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/b7060a18e50e0e4912fa47ccbb96fa86d18196e3"><code>b7060a18</code></a></td></tr>
<tr><td>2026-08-30</td><td class="num">27,169</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/c5cabccebe9d49afa3e8f361719828c56a25963f"><code>c5cabcce</code></a></td></tr>
<tr><td>2026-09-06</td><td class="num">29,530</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/ca21845c9f40381db49353b05994f8bad3fc1f57"><code>ca21845c</code></a></td></tr>
<tr><td>2026-09-13</td><td class="num">29,435</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/16a1b2c5dee33b182563e0fd96040ecb3f4a7fc1"><code>16a1b2c5</code></a></td></tr>
<tr><td>2026-09-20</td><td class="num">28,947</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/9e245c9e77f8999aa37513a0dd15848c6bc10271"><code>9e245c9e</code></a></td></tr>
<tr><td>2026-09-27</td><td class="num">28,750</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/d52c8e91879d122afeb99adf3bcfc12e7a258ef5"><code>d52c8e91</code></a></td></tr>
<tr><td>2026-10-04</td><td class="num">29,338</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/9ec31dfb87fee8b6546029c6bec7b1fa97b28ac7"><code>9ec31dfb</code></a></td></tr>
<tr><td>2026-10-11</td><td class="num">29,263</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/bc0de0c4d627800e3068cd597ade6358713319ab"><code>bc0de0c4</code></a></td></tr>
<tr><td>2026-10-16</td><td class="num">29,363</td><td><a href="https://github.com/vshylov/mindfork-rs/commit/475dfc175467fbf1795f614af25aaf9d4cb95475"><code>475dfc17</code></a></td></tr>
</tbody></table>
</details>

[CLAUDE.md](https://github.com/vshylov/mindfork-rs/blob/main/CLAUDE.md) started as an 8 KB guide. By 2026-08-08 it was 994,562 bytes: a short guide followed by a 243-entry engineering journal, loaded in full before any work began. [The gate that now guards it](https://github.com/vshylov/mindfork-rs/blob/main/tools/doc_index_check.py) records the cost in its own docstring: roughly 237K tokens spent per session before the first line of work.

The fix was not to write less. It was to give the memory a shape and a budget.

- CLAUDE.md became a router: orientation, the standing decisions, and a map that says "if you are working on the engine, read architecture §5 and §6, spec §3 and §6 to §8, and the engine journal." It is capped at 30,000 bytes, and CI fails if it grows past that. Today it sits at 29,363.
- The journal moved into 16 files split by subsystem, 644 entries today. Each one records what was done, what was measured, what was rejected and why, the test count, and the model that gave the GO.
- A [lessons file](https://github.com/vshylov/mindfork-rs/blob/main/docs/lessons.md) holds the traps that would bite on an unrelated task: 264 rules, each with the incident that produced it and a count of how many times it recurred.
- Research documents hold the reasoning before a decision, with claims tagged [code], [measured] or [unverified]. A finished plan moves to a history folder. The outcome becomes an architecture decision record that gets amended, never rewritten.
- The spec and the architecture document use stable section numbers, and the code cites them: 3,282 references from 305 of the 341 source files. An agent that lands in a file is told which two kilobytes of the spec to read.

Three separate documents repeat the same sentence: "Read the section, not the file." This is the whole trick. A stateless agent needs an externalised memory, and an externalised memory needs a context budget, or it eats the session it was meant to help.

## Measure, and let the plan lose

The lessons file has a rule I would put on the wall of any team working with agents: "Assume the measurement will overturn the plan, because it repeatedly has."

A language model's defining weakness is confident, plausible assertion. The plan sounds right. The fix sounds right. The explanation of why the test passes sounds right. The project's answer was to refuse every assertion until it had been measured. The journal has 265 entries that contain the word "measured", 30 that describe a control arm, and 107 that record something rejected.

The first time this happened was day two. The spec had been written around an inference engine called xinfer. A live run of Gemma 4 through it produced incoherent output, and the engine was [replaced with llama.cpp](/articles/engine-as-a-server/) the same day. The spec still carries the historical note.

Later the stakes got bigger. The code-workspace track had a fifth stage, [a semantic index over the project](/articles/code-workspace/), built as a probe and measured: 48 turns per arm on a 31B model, control against treatment. The effect changed sign depending on how you defined success, which means it was smaller than the instrument. The stage did not ship. The probe is frozen as [a git tag](https://github.com/vshylov/mindfork-rs/tree/probe/code-search-stage5), and the journal entry says: "The rejection is the deliverable."

<blockquote class="pull"><p>The rejection is the deliverable.</p></blockquote>

Smaller examples are everywhere. A roadmap item claimed that per-model prefixes improved embedding search; re-measured on 40 documents and 14 queries, the prefixes changed no ranking at all. A rendering change for raw HTML blocks was measured against a 186,805-word corpus, found to affect 0.047% of it, and rejected, while the census turned up a different bug.

Slop is what you get when nothing is ever measured and nothing is ever rejected. This repository rejects things constantly, and writes each rejection down so the next agent does not propose it again.

## Tests that must be able to fail

Half the source tree is test code, but that is the least interesting fact about it. The interesting one is that the project learned not to trust green.

Agents are very good at writing tests that pass. The lessons file is full of the ways a passing test can lie, each learned the hard way:

- "Beware the vacuous assertion: a value equal to the type's default proves nothing." Three such assertions in one change, "all three found by mutation, not by passing."
- "Tests that assert an absence can pass for the wrong reason." An always-empty store satisfies them either way.
- "On a path built to degrade gracefully, is_ok() can never be the assertion."
- "A test that waits on an event must bound the wait, or a regression hangs instead of failing." Recorded five times before it became a shared helper that every stub uses.
- "A gate that passes is indistinguishable from a gate that does nothing." So every CI gate is tested in both directions: a planted violation must turn it red, and a clean tree must keep it green.

The countermeasure is mutation testing, done by hand and done routinely; 117 journal entries describe it. The recipe is written down: every mutant is written into the source at once, each behind a switch read from an environment variable at run time; the project is compiled once, and the test binary is run once per mutant with that variable set. A Rust build of this size takes minutes, so compiling once per mutant would make the check too slow to be routine. A surviving mutant is read twice, because it indicts the fixture as often as the test.

You can see the habit in the test names. There are 155 tests with "refuse" in the name, 112 with "does_not", 71 with "says", 57 with "never". Twenty-five tests carry a doc comment naming the control arm and exercise the same code with the defect switched off.

These are tests written by someone who expects to be lied to.

## Two gates with different blind spots

The unit suite runs without a terminal and without a model. A scripted mock engine plays the agentic loop. A tiny TCP server serves canned server-sent events, so the HTTP clients are tested end to end rather than through a serializer, after a serializer-only test let a swallowed error event survive.

Every screen is drawn into an in-memory terminal buffer and read back as text. One test draws every screen and every popup from a window of 0 by 0 upwards, in both languages: 21 states by 493 sizes by 2 locales, and every frame must be either the placeholder or a complete screen with the cursor inside it. Ten committed screenshot dumps must match what the code renders today, byte for byte, or the build goes red. Every storage-format change ships with a golden fixture from the previous version.

The live suite is the other half: 260 tests marked ignore that run only against a real model and a real server, mandatory for anything touching the engine, the memory or the tools. The whole set also runs on rented GPUs as [one dispatch](https://github.com/vshylov/mindfork-rs/actions/runs/38955333678): on 2026-10-16, 253 smokes, mostly on Gemma 4 31B, in four parallel jobs, 65 minutes of model time inside 31 minutes of wall clock, zero failures. The journal records live runs as "Smoke, GO" with the date and the stack, 175 entries so far, or as an explicit "no live run needed", 122 entries.

The live runs keep finding what mocks structurally cannot. The client never emitted the usage chunk on llama.cpp, because the server sends it after the finish reason and the stream had stopped reading. A launch with no model file started llama.cpp's router mode, whose health endpoint says OK while every message is a 400. The Linux install script passed 25 local scenarios and five CI images, then failed on the first real GPU pod, because root without CAP_CHOWN cannot restore an archive's owner. One tar flag, found only by a pod.

Two gates, two blind spots, and a rule that you do not get to skip the second one because the first one is green.

## The process grew itself

I used no framework for any of this. Looking at the git history, the process has two layers with a clear seam.

The structural layer was there on day one. The first commit contains the brief, a 97 KB spec with sixteen numbered chapters, and a plan for milestones M0 to M9 with tasks, tests and a definition of done for each. It fixes Feature-Sliced Design with dependencies pointing strictly downward, the engine behind a trait, a branch per milestone, and even the rule that every commit names the model that wrote it. None of that was learned here. It came from a first attempt at the same product that I had shelved.

The guardrail layer is entirely reactive. Every file in the tools directory, every byte limit, every CI check appeared after something broke, and the commit messages say so in the same voice each time: "held only by discipline, now closed by the gate."

- **Day 1, 2026-06-15.** The first live gate. Gemma 4 through xinfer was incoherent; the engine was replaced.
- **Day 14, 2026-06-28.** The first MVP probe with a go/no-go, on the self-model track.
- **Day 28, 2026-07-12.** [AGENTS.md](https://github.com/vshylov/mindfork-rs/blob/main/AGENTS.md) and the PR template, written to codify practice that already existed.
- **Day 31, 2026-07-15.** CI, the release pipeline, a changelog, a pinned toolchain. "Before this the gates rested only on the agent's discipline."
- **Day 45, 2026-07-29.** A link checker, after 28 broken links when plans moved to the history folder.
- **Day 52, 2026-08-05.** SonarCloud, blocking the same day, while preparing for an open-source release.
- **Day 55, 2026-08-08.** The 30 KB router and the journal split, after a 994 KB CLAUDE.md.
- **Day 71, 2026-08-24.** A gate for list scroll state, after one scroll bug was copied into eight lists.
- **Day 95, 2026-09-17.** A release guard, pinned actions, a static C runtime, after a missing VCRUNTIME140.dll on a clean Windows machine.
- **Day 117, 2026-10-09.** A layer gate. See the last section.

The lessons file counts recurrences: seven rules recorded twice, one three times, three four times, one five times. One entry admits that a recurrence was "written by an agent with this very entry in its context." That admission is why the trajectory always runs from convention to gate. A rule in the agent's context is a suggestion. A rule in CI is a fact.

<blockquote class="pull"><p>A rule in the agent's context is a suggestion. A rule in CI is a fact.</p></blockquote>

This is also why an imported AGENTS.md would not have worked. Every rule in this repository carries its own incident. A rule copied from someone else's repository carries theirs.

## Why the analyzer stays green without trying

People look at the SonarCloud badge and assume there is a campaign behind it. There is not. The green gate is a symptom of the process, not a goal anyone pursues.

- Stricter gates run upstream. Every pull request runs rustfmt, clippy with warnings as errors on a pinned toolchain, and the full test suite on Linux, Windows and macOS before Sonar sees it. Sonar's Rust rules overlap heavily with clippy, so there is little left to find.
- The gate judges new code only, and the backlog was paid down once. On 2026-08-06 the agent triaged the initial 114 findings and I decided the classes: 49 wildcard imports accepted because they are the documented convention of the file-splitting playbook, seven complexity findings accepted with a written reason, everything else fixed in four pull requests by area, each verified by extraction rather than rewrite.
- The remainder is swept weekly. Twenty-four of the 34 entries in the quality journal are a "SonarQube follow-up": a handful of findings read off the main branch, fixed by extracting functions along natural seams, never by clicking Accept, and then mutation-tested.
- Coverage is high because tests are a definition-of-done item, not a separate effort. The project is honest about the accounting: inline test modules count toward the measured lines, the tools directory is excluded, and live-smoke files are named so the coverage tool leaves them out.
- The job waits for the server's verdict. The first run ever uploaded a report the server later rejected while CI stayed green. Now the job fails if the gate fails.

[The Sonar configuration file](https://github.com/vshylov/mindfork-rs/blob/main/sonar-project.properties) is worth reading on its own. It is written as an essay: every exclusion has a date, a reason, and a paragraph on what it costs. "What it costs, stated so it is not forgotten." A future agent cannot undo a decision without reading why it was made.

## What is not so clean

An article like this is only worth reading if it says where the method is weak.

Start with the obvious objection: is the code actually good, or are the metrics merely green? The claim here is narrower than "good code". The goal was never that a human could read every line; it was that the quality of the code is legible through artefacts: numbers, live runs, decisions, behaviour. A newcomer who opens this repository meets eight megabytes of documentation and a wall of gates, not Rust that a colleague can explain over coffee. That is a boundary of the method, not a footnote.

The method also scales to exactly one owner. It works as long as there is a person who owns the behaviour and the decisions, if not the lines, and has the time to look at the artefacts every day. For a team that is a different and more expensive arrangement, and I have not tried it.

The evidence is self-graded. The agent that wrote the code also writes the GO, decides whether a failure was a flake, and runs the mutation checks. None of that is a CI gate. The loop is closed by me reading, and by the live stand.

The documents drift. The router says the spec is 496 KB; it is 561. It counts 618 journal entries; the indexes sum to 644. When the coverage condition was switched back on in the Sonar server, nothing in the repository recorded it for two months. No gate checks that a cited section number still exists.

The lessons are not always read. "A fifth time, with this entry unread," says one of them. At 200 KB the lessons file is becoming what the router was built to prevent.

The early journal asserts without evidence. Twelve percent of the entries carry no test count, no GO and no measurement. They are the early ones. The discipline was acquired, not original.

Gates rot when nobody runs them. For several weeks the live smokes ran only on my own GPU and the rented gate sat idle. When I started it again, it did not come back clean. Putting it right took a week: the suite was reworked and split into four parallel jobs, and the nine real defects it surfaced went out as a patch release. The project's own lessons file had already said it: a gate nobody has seen fail is a gate nobody knows works.

And it is expensive in discipline and attention. Nearly eight megabytes of documentation for ten and a half of code. Thirty-nine percent of commits spent on documents. A GPU, rented pods, and a person who merges six pull requests a day and runs the live stand. In money, though, it is cheap: $100 a month for the Claude subscription, and the rented GPU runs came to $5.23 in July, $8.94 in August, $0.99 in September and $98.96 in October, the month the gate was reworked and run many times over, some of them on gpt-oss-120b. Across four months the rented GPUs came to $114, $99 of it in October alone. Three months of a live gate for less than the price of one developer hour is the strongest argument I know for measuring on a real model instead of guessing. The method is reproducible. What it costs is not money but an owner who knows the product cold and has the time to verify behaviour every day.

## The day the process ran on itself

While preparing this article I asked the agent to measure the repository, and one of the findings was embarrassing. The architecture document calls one rule "the key FSD invariant in the code": screens never import from the application layer. The survey found eleven event types imported from exactly there, the first on 2026-08-23, every later one written by an agent that had the rule in its context, copying the neighbour that had it. The other half of the same sentence, the arrow between the domain types and the shared layer, had been wrong in the other direction since the second milestone.

The same day, in a separate session, the types moved down a layer in a pure refactoring branch, the document was corrected to say what the code actually does, [a new gate](https://github.com/vshylov/mindfork-rs/blob/main/tools/layer_check.py) went into CI with a self-test that plants a leak in each direction and expects red, and a new lesson went into the file: "A documented never with no gate has drifted by the time anyone reads it."

That is the whole method in one afternoon. Measure. Find the gap between what the documents claim and what the code does. Fix the code mechanically, in its own pull request. Fix the document. Turn the rule into a gate. Test the gate. Write down what happened so it is not repeated.

## What to copy

If you are starting a project with an agent and you do not intend to review every change it makes:

1. Write the spec before the code, number its sections, and never renumber them. The code will cite them for months.
2. Give the agent's memory a shape and a budget: one small router file that CI keeps small, a journal split by subsystem, and a lessons file that is read before implementing.
3. Make the agent enumerate the alternatives and put them to you as a question. Decide, with a date. Write the decision where the next agent will find it.
4. Require a run on the real thing for anything the mocks cannot see, and require the result in writing: model, stack, date, GO or NO-GO.
5. Mutation-test the tests. A test that cannot fail is worse than no test, because it reports confidence it does not have.
6. When a rule is broken twice, stop writing rules and write a gate. Then test the gate against a planted violation.
7. Keep mechanical refactors and behaviour changes in separate pull requests, and use the test count as the invariant of a pure move.

None of this requires reading the diffs. All of it requires reading numbers, decisions and behaviour, and being willing to let a measurement throw your plan away. That is what stood in for code ownership here.

Do not take my word for any of it. The repository is public at [github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs). Start with [docs/journal](https://github.com/vshylov/mindfork-rs/tree/main/docs/journal) for what was done and measured, [docs/lessons.md](https://github.com/vshylov/mindfork-rs/blob/main/docs/lessons.md) for what went wrong more than once, and [tools](https://github.com/vshylov/mindfork-rs/tree/main/tools) for the gates, including [the script that produced every number in this article](https://github.com/vshylov/mindfork-rs/blob/main/tools/repo_metrics.py). If you find a figure here that the repository does not back, open an issue: that would be the process running on itself one more time. The product itself is on [the home page](/), and [the install guide](/install/) has every way to get it.
