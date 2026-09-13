+++
title = "Where the trust boundaries are"
description = "An assistant that can browse, run code and edit your project needs edges, not promises: what leaves the machine, where the model may not point, what the code can touch, when the app asks — and what the author receives, which is nothing."
weight = 7
+++

*The seventh in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/),
[the self-model](/articles/self-model/),
[vector search](/articles/vector-search/),
[local speed](/articles/local-speed/) and
[the Python sandbox](/articles/python-sandbox/).*

## Edges, not promises

A model that can act can be talked into acting. The links it follows come
from a page it just read, and such a page can ask it to open something on
your side of the router; a file name it chose can be made to display as
another; a script's output can pose as the tool's own report. None of
that is hypothetical — each is a case the app has met and closed. So the
question is never whether the model will always judge well. It is where
the edges are that hold when it does not: which addresses a tool may
reach, which files a run may touch, when a person is asked first, and
what the program itself sends anywhere. Those are decided in code, per
tool, and the default answer is *no* until you say otherwise.

## What leaves the machine

Every outbound connection is either to an endpoint **you** configured or
part of a tool you switched on. There is no destination the app contacts
on its own initiative, and no request is ever made about you, your
machine or your usage.

- **The model.** In managed mode the app starts `llama-server` on your own
  machine and talks to `127.0.0.1` only. In external mode the conversation
  goes to the URL you typed; with a cloud provider, to that vendor, under
  their terms. The impersonation engine follows the same rule.
- **Embeddings.** A separate setting, which may well be a different vendor
  from the chat's. What is embedded, and therefore sent there: note text,
  the knowledge base's chunks, chunks of attachments — and, less obviously,
  the queries and up to 800 characters of each result page that
  `web_search` embeds to rank its results.
- **The web tools** — `web_search`, `fetch_url`, `youtube_watch` — are
  **off in a fresh installation**. Switched on, a search goes to engines
  the app picks, with a query the model composed from your conversation; a
  keyed provider is preferred when a key is available, and a key is found
  only where you put it — in the settings, or in an environment variable
  you named there. Result pages are fetched to extract their text. YouTube
  sends the video's address, not its bytes, to Gemini, if you gave it a
  key for that.
- **Speech** is sent only when you invoke `/tts`. An image attached by URL
  is downloaded by the app itself and never handed to a provider to fetch.
- **Plugins.** MCP servers run as local subprocesses over the process's own
  pipes; what they send onward is theirs. They are a double opt-in — a
  master switch and a per-profile approval — and a server that changes its
  tool set after approval has to be approved again.
- **Setup.** `mindfork sandbox setup` and `mindfork llama setup` download a
  runtime, a Python distribution and packages, or an engine build; every
  file is checked against a pinned checksum, or the release's own, before
  it is used. Nothing about your data travels in either.

## Where the model may not point

An address the model chose — in `fetch_url`, in the pages `web_search`
reads — is resolved and checked against the routable public internet
before anything is sent. Your own machine, your LAN, and the address cloud
providers keep their credentials behind are refused, on the original
request and again on every redirect, at most five hops. If you *do* want
a dashboard on your network reachable there is a switch for it, off by
default. An address you typed yourself — `/image attach <url>`, the engine
URL in the settings — is your decision, and is not subject to the guard.

## What the code can touch

The Python sandbox starts from nothing. The guest is granted a scratch
directory holding the script and the files the call named, copied in —
up to twenty of the chat's files, 100 MB in all — plus a package image it
cannot alter, so a file one call writes cannot run inside the next.
Sockets exist only with the network toggle on; a timeout kills the
process; one script runs at a time; on Windows an optional hard memory cap
holds it at the OS level. Only what the code saves into its output folder
comes back — ten files and 50 MB a call, kept with the chat, a workbook or
document among them carrying the mark of the web so that Office opens it
in Protected View. Running Python on your own machine instead is one
setting away and means exactly that: your interpreter, your permissions,
your network, with the same file exchange and, on Windows, an optional
memory limit of its own.

A code project attached to a chat gives the assistant that directory and
nothing outside it, `.gitignore` honoured. An edit keeps a pre-image of the
file before it is first touched, so `F4` shows each change as a diff and
puts one file back on request. Build, run and test are the command lines
you typed, executed exactly as written — the assistant can never compose a
command, add a flag or chain a pipeline, and a slot you left empty is a
tool that does not exist. A command that outruns its limit is stopped
together with everything it started.

The general file tools have a jail root of their own — and it is empty
until you set it, so enabling them without a root leaves them
unrestricted. `/file open` hands only document types to the desktop; a
script, a shortcut or an HTML page the assistant wrote opens the folder it
sits in instead.

## When it asks

One switch, *Confirm dangerous tool calls*, is a second layer over the
master switches: with it on, every call that changes something outside the
app — `python_exec`, a file write, a project edit or command, and every
plugin tool, since a third party's effects are unknown — is shown to you
first, formatted the way the feed will show it afterwards, code as code,
the files going into a sandbox run named with their sizes. `Enter` allows
it, `Esc` declines without derailing the answer. A server's own
"read-only" annotations are deliberately not consulted: a server can claim
anything, so they could only ever relax the decision. A call inside a
subagent's run asks exactly as one outside would, one popup at a time.

A run in the background never asks — there may be nobody at the keyboard
— so its permissions are the profile's tool set: switch off, for that
profile, whatever you would not let run unattended. Reads are never asked
about, and neither are writes to the app's own storage — notes, the
self-model, the knowledge base — which are visible, profile-scoped and
reversible.

## Secrets

Keys you enter — cloud providers, an external server, MCP tokens, a search
key, the backup password — are stored inside `settings.json` **encrypted
and bound to this machine**: DPAPI on Windows; on Linux a key derived from
the host's machine id and your user name, with ChaCha20-Poly1305. A
settings file carried to another computer holds no usable secret. The
scheme defends against exactly that — a file carried off — and not
against code running as you on your own machine; ADR 0008 in the
repository says so in as many words. If you would rather the app never
held a value at all, store the *name* of an environment variable instead.
A key is sent only to the service it belongs to, and never displayed back.

## What the author receives

Nothing. No telemetry, analytics, usage statistics, crash or error
reporting, update check, version ping, license check, account, or server
operated by the project. The addresses in the About dialog are text on a
screen. Logs stay in `logs/` on your machine and carry no message text,
prompt text or key — identifiers, counts, timings, model names, endpoint
URLs and error strings — and they are excluded from backups. A backup
holds the settings, the profiles, your personal dictionary, the database
and every chat; it can be AES-256 encrypted, and even then its manifest,
entry names and sizes remain readable. Deleting everything is deleting one
folder.

## What this does not claim

The sandbox is a process boundary, not a virtual machine, and the local
Python mode has no boundary at all — it is your machine, chosen in the
settings. A plugin runs with your permissions and its own network. Copying
over SSH sends the text through whatever sits between you and the host;
that is the feature. The Windows binaries are not signed yet; the
[code signing policy](/code-signing-policy/) describes the arrangement
applied for, and says so first. And a model given a tool will use it,
inside the edge — the edges are what you can rely on, not its judgement,
which is why they are drawn where they are.

The complete inventory — every endpoint, every default, what each file in
the data folder holds — is the [privacy policy](/privacy/), written from
the code rather than from a template; the promises the app makes, and how
to report a break, are in `SECURITY.md` in the
[repository](https://github.com/vshylov/mindfork-rs).
