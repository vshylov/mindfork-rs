+++
# Generated from DISCLAIMER.md by tools/site_legal_pages.py - do not edit.
title = "Disclaimer"
description = "The app ships no model: what that means for the output you get, for the tools a model may invoke on your machine, and for warranty and liability."
template = "doc.html"
+++

This notice **supplements and does not modify** the MIT License in the
[accompanying file](https://github.com/vshylov/mindfork-rs/blob/main/LICENSE). The MIT License is reproduced there verbatim and
governs your rights in the software; nothing in this notice takes any of them
away. What follows only spells out, for the avoidance of doubt, what the
software's "AS IS" and no-liability terms mean for a program whose entire
visible output is written by a language model.

**mindfork is a client, not a model.** It ships no model weights and no content
of its own. Every word it displays was produced by a language model that *you*
chose, obtained and configured — a GGUF file you downloaded and run locally
through `llama.cpp`, or a remote provider (OpenAI, Google Gemini, Anthropic, or
any other OpenAI-compatible server) that you signed up with. The author of this
software does not train, audit, host, review or moderate those models, and has
no control over, or advance knowledge of, anything they generate.

## 1. Generated output comes with no warranty of any kind

A language model produces statistically plausible text, not verified fact. Its
output may be **factually wrong, fabricated, internally inconsistent, biased,
offensive, obscene, hateful, harmful, dangerous, illegal in your jurisdiction,
or infringing on someone's rights** — including output that looks confident,
cites sources that do not exist, or explicitly claims to be safe or verified.

The author makes no representation or warranty, express or implied, about the
accuracy, reliability, safety, legality, fitness for any purpose, or
non-infringement of anything a model generates through this software. **You are
solely responsible for evaluating every output before acting on it, relying on
it, storing it, or passing it on to anyone else.**

## 2. You choose the model, and you accept its terms

The software imposes no restriction on which model you connect. That is a
deliberate design choice: it will run an "uncensored" or otherwise unaligned
fine-tune exactly as readily as a vendor's aligned release, and it applies no
content filtering, moderation, refusal layer, or output classification of its
own. It does not attempt to detect harmful requests or harmful responses, and
it will not stop a model from saying anything.

Consequently, **you** are responsible for:

- choosing a model appropriate to your purpose, your jurisdiction and the
  people who will see its output, and for keeping the software out of the hands
  of anyone for whom that output would be inappropriate or unsafe;
- complying with the license, terms of service and acceptable-use policy of
  every model, model host and API provider you connect to, including any
  restriction on how the model's output may be used or redistributed;
- any consequence of running a model that has had its safety training removed.

The author is not a party to your relationship with any model provider and
assumes no obligation arising out of it.

## 3. Not professional advice, and not for safety-critical use

Output from this software is **not** medical, psychological, psychiatric,
legal, financial, investment, tax, engineering or safety advice, and no
professional relationship of any kind is created by using it. A model has no
license to practice, no duty of care to you, and no way to know your situation
beyond what is in its context window.

Do not use this software, or any model connected through it, as the basis for
decisions where an error could cause injury, death, financial loss, legal
jeopardy, or damage to property or the environment. **In an emergency, or for a
medical, mental-health, legal or financial decision that matters, contact the
appropriate emergency services or a qualified professional — not a language
model.**

## 4. Autonomous behavior, tools and automated actions

This software is deliberately built to let a model act, not merely to chat. Its
features include a Python sandbox, URL fetching and web access, third-party MCP
tool servers you configure, a local knowledge base and note store, short-lived
sub-agents, and a self-model through which the model can rewrite its own system
message and its own sampling parameters.

These mechanisms execute **on your machine, under your user account, at your
direction and at your risk**. Confirmation prompts and the sandbox reduce the
blast radius; they are not a security boundary you should rely on against a
model, a prompt-injection payload embedded in a fetched page or an attached
document, or a malicious MCP server. Review what a tool is about to do before
approving it, and only connect MCP servers and model providers you trust.

## 5. Your data

The software stores your chats, notes, profiles and settings locally. When you
configure a remote provider, the content you send — including message history,
notes, attachments and retrieved documents assembled into the request — leaves
your machine and is processed by that provider under *its* privacy policy and
terms, over which the author has no control. Choose what you share accordingly;
if that matters to you, run a local model, which is what this project was built
for in the first place.

Which files hold what, which destinations the software can reach at all, and
which setting of yours has to be on before it reaches any of them, is set out in
[PRIVACY.md](https://github.com/vshylov/mindfork-rs/blob/main/PRIVACY.md) — installed beside this file.

## 6. Limitation of liability

To the maximum extent permitted by applicable law, and in addition to the
warranty disclaimer and liability limitation already stated in the MIT License,
**the author and copyright holder shall not be liable for any claim, damage,
loss or other liability arising out of or in connection with**:

- the content of anything generated by a model used with this software, or any
  action taken or not taken in reliance on it;
- any model, model host, API provider, MCP server, dictionary, or other
  third-party component you obtain, connect or configure;
- any action performed by the software's tools, sandbox, sub-agents or
  self-model mechanisms, whether or not you approved it;
- any loss, corruption, or disclosure of data handled by, or sent from, this
  software.

This applies whether the claim sounds in contract, tort (including negligence)
or otherwise, and whether or not the author was advised of the possibility of
such damage.

## 7. Mandatory law

Some jurisdictions do not allow the exclusion of certain warranties or the
limitation of certain liabilities. In those jurisdictions the exclusions and
limitations above apply only to the maximum extent permitted by applicable law,
and nothing in this notice or in the MIT License limits liability that cannot
lawfully be limited. Any provision held unenforceable is severed, and the
remainder stays in force.

---

If you do not accept the terms of this notice and of the
[MIT License](https://github.com/vshylov/mindfork-rs/blob/main/LICENSE), do not use this software.
