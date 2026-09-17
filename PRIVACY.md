# Privacy policy

Effective **2026-09-17**. It covers three things that are easy to confuse: the
**mindfork application**, the website **mindfork.io**, and the project's
presence on **GitHub**. They have very different answers, so they are kept
apart below.

**The short version.** mindfork is a local program. It has no telemetry, no
analytics, no crash reporting, no update check and no account. Nothing it does
reports back to the author, and there is no server of this project's for it to
report to. Everything it stores stays in its data folder on your computer (§2),
apart from the short-lived working files listed in §8. It talks to the network
for the model you configured, for the tools that are switched on and for the
setup commands you run — and where it does, your data goes to *that* provider,
under *their* policy, never through anything of ours.

The rest of this document is the detailed version, written from the code rather
than from a template, including the parts that are on by default. Its
companions: [SECURITY.md](SECURITY.md) (what the app promises, and how to report
a break), [DISCLAIMER.md](DISCLAIMER.md) §5 (what it means, legally, that a
request leaves your machine), and [docs/install.md](docs/install.md) (where the
data directory is on each platform).

## 1. Who is responsible for what

The software is published by **Vladimir Shylov**, its author and copyright
holder, as free open-source software under the MIT License.

For the data the application handles on your computer, **you** are the only
party with access to it. The author operates no service that receives it and
cannot obtain it. When you point the app at a provider, that provider becomes
responsible for what you send them, on the terms you accepted with them; the
author is not a party to that relationship and has no visibility into it.

For the website and the GitHub repository the author does make the choices —
see §9 and §10, where both are deliberately kept as thin as they can be.

## 2. What stays on your machine

The app is **portable by design**: its data lives in a `data/` directory next to
the executable, not in a hidden per-user location, unless a `defaults.json` next
to the binary selects a system location or another path
([docs/install.md](docs/install.md)). In a development build that is
`target/debug/data/`. So everything the app knows is in one place you can open,
copy, back up and delete.

| What | Where | Holds your conversations? |
|---|---|---|
| Settings, and the encrypted secrets map — each stored key labelled with the name of the computer it was entered on | `settings.json` | no |
| Profiles, system prompts, per-profile tool lists | `profiles.json` | your prompts |
| Chats | `chats/<uuid>.json` — messages, model thoughts, tool calls and their results, sub-agent transcripts, attached images inline, attachment text | **yes** |
| Notes, the RAG knowledge base and their embedding vectors, the self-model, the record of which model ran when | `data.db` (SQLite) | **yes** |
| The full-text search index | `cache.db` (SQLite) | **yes** — derived; delete it and it rebuilds |
| Pre-images of every project file the assistant edited, for the `F4` diff and revert | `workspace/<chat-id>/` | your source code |
| Files kept with a chat — what the Python sandbox saved (charts, tables, workbooks), and files you attached with `/file attach` whose original is kept: binaries, and the PDF, DOCX or HTML beside its extracted text | `files/<chat-id>/` | **yes** — your files, and what the code made of them |
| Backups | `backups/`, or the location you choose | yes — see §7 |
| Diagnostics | `logs/` | no — see §6 |
| The Python sandbox runtime | `sandbox/` | no |
| llama.cpp builds downloaded by `mindfork llama setup` | `llama/` | no |
| Spellcheck dictionaries, locales, the words you added yourself | `dictionaries/`, `locales/`, `personal_dictionary.txt` | the personal dictionary holds words you added |

None of this is uploaded anywhere by the app. Copying the folder to another
machine carries your chats with it — and deliberately does *not* carry usable
secrets (§7).

## 3. What leaves your machine

Every outbound connection is to an endpoint **you** configured, to a tool
endpoint that is part of a feature that is switched on, or to the download host
of a setup command you ran. There is no destination the app contacts on its own
initiative, and no request is ever made about you, your machine or your usage.
What follows is the complete list.

### 3.1 The model

Where your conversation goes depends on the engine mode you chose:

- **Managed** — the app starts `llama-server` on your own machine and talks to
  `127.0.0.1` only. Nothing leaves the computer, whatever the server was told to
  bind to.
- **External** — the OpenAI-compatible URL you entered, and nowhere else.
- **Cloud** — the provider you selected: `api.openai.com`,
  `generativelanguage.googleapis.com` (Google Gemini), `api.anthropic.com`,
  `api.x.ai` (Grok), or a different base URL if you set one. Each processes your
  data under its own policy —
  [OpenAI](https://openai.com/policies/privacy-policy),
  [Google](https://policies.google.com/privacy),
  [Anthropic](https://www.anthropic.com/legal/privacy),
  [xAI](https://x.ai/legal/privacy-policy) — and their retention and
  training-on-your-data rules are theirs, not ours. Read them.

**Impersonation** — the model drafting your next message — has an engine setting
of its own. By default it shares the chat engine; pointed elsewhere, it sends the
conversation to that external server or cloud provider instead.

The `MINDFORK_ENGINE_URL`, `MINDFORK_LLAMA_BIN`, `MINDFORK_EMBED_URL` and
`MINDFORK_EMBED_BIN` environment variables, when set, replace the chat or
embedding engine in the settings for that launch
([docs/install.md](docs/install.md)).

**What a request contains:** the conversation so far (a chat model is stateless,
so the history is re-sent every turn), the system prompt — including the
self-model text if those tools are enabled, and the text of chat attachments,
which are re-injected each turn — images encoded inline, the schemas of the
tools that are on, and the results of tool calls. Anything a tool retrieved
therefore becomes part of the history: a note recalled by `note_recall` or a
passage found by `rag_search` travels with every subsequent turn of that
conversation.

**Requests you did not type.** Some features ask the model on your behalf, and
they are worth knowing about because they are ordinary requests to the same
provider: naming a new chat (on by default, after the first substantive reply),
compacting a long history into a rolling summary, summarizing a page that
`fetch_url` retrieved, sub-agents and `run_dialogue`, and — only when you turn
them on — reflection and the notes/self-model consolidation passes.

**Probes.** In managed and external mode the app asks the server `/health` and
`/props`, and `GET /models` for what it is running — the model's name, context
window, image support and accepted parameters. The Grok cloud is asked the same
`/props` and `/models`, with your key, when the engine is set up and when you
attach an image. The OpenAI, Gemini and Anthropic clouds are not probed.

### 3.2 Embeddings (notes, RAG, search reranking)

The embedding endpoint is a **separate setting** from the chat one: it can be a
local server, an external URL, or a cloud provider — which may well be a
*different* vendor from the one running your chat.

What gets embedded, and therefore sent there: note text, self-model traits,
chunks of documents you added to the knowledge base (including text extracted
from PDF and DOCX), chunks of chat attachments, the queries of `rag_search`,
`note_recall` and `attachment_search` — and, less obviously, **`web_search`
queries together with up to 800 characters of each result page**, which the tool
embeds to rank results. If your embedder is a cloud provider, that is search
content reaching a second vendor.

To notice that the embedding model has changed, the app also embeds one fixed
sentence (`mindfork embedding canary v1`) when it first uses the embedder in a
session, and a fixed set of calibration sentences once for each new model.
Neither contains anything of yours.

### 3.3 The web tools — off until you turn them on

`tools.web_enabled` is **off in a fresh installation**, and with it
`web_search`, `fetch_url` and `youtube_watch`. Everything in this section
happens only after you switch it on in Settings → Tools. (It shipped *on* before
0.9.9; an installation that already has a `settings.json` keeps whatever that
file says, so if you had it on, it stays on.)

- **`web_search`** queries, in order, `lite.duckduckgo.com`,
  `html.duckduckgo.com`, `www.mojeek.com` and `www.ecosia.org`. What is sent is
  the search string the model composed — which is derived from your
  conversation — plus a desktop browser User-Agent and an `Accept-Language`
  header. These engines are chosen by the app, not by you.
- **A keyed provider** is preferred over that chain when a key is available:
  today that is [Tavily](https://tavily.com/privacy) (`api.tavily.com`). A key
  is only ever found where you put it — entered in settings, or in an
  environment variable **you named** in settings. No variable name is assumed,
  here or anywhere else in the app: before 0.9.9 the app looked for
  `TAVILY_API_KEY` by default, which meant a key
  exported for some unrelated tool could route your searches, and spend its
  credits, without a decision made anywhere in this app.
- **Result pages are fetched** by default (`tools.web_fetch_content`) to extract
  readable text, so the search engines' hits are visited too.
- **`fetch_url`** retrieves the address the model chose. A page that fits the
  attachment budget reaches your chat model as a summary — up to 12 000
  characters of it are sent to be summarized — or as its text, when the model
  asks for no summary or summarizing fails. A larger page (up to 400 000
  characters) is attached to the chat and kept with it, and its passages travel
  like any attachment's (§3.1).
- **`youtube_watch`** fetches the video's watch page — or, failing that, its
  oEmbed record — from `www.youtube.com` for the title and length, and,
  if a Gemini key is available — the one saved for the Gemini provider, or the
  variable you named in the video settings — hands the URL to Gemini, which
  watches the video on its own servers. No video bytes leave your machine; the
  URL and the prompt do.
- **Where they may not go**: model-chosen addresses are resolved and checked
  against the routable public internet — your LAN, loopback and other
  non-routable addresses are refused, on the original request and on every
  redirect, unless you set `tools.web_allow_private`.

### 3.4 Speech

Nothing is ever spoken automatically: text is sent only when you invoke `/tts`.
When you do, the **default provider is OpenAI's cloud** (`api.openai.com`), with
Gemini and any OpenAI-compatible server as alternatives. What is sent is the
message text, flattened out of markdown and split into chunks.

### 3.5 Images you attach by URL

`/image attach <url>` downloads the image with its own client and re-encodes it
locally; the address is never handed to a model provider to fetch. Only `http`
and `https` addresses are accepted. Because you typed the address yourself, it is
not subject to the public-address guard in §3.3 — an intranet or loopback address
is your decision, not a model's.

### 3.6 MCP servers

MCP servers run as **local subprocesses** — the app speaks to them over the
process's own pipes, not over the network. What leaves the app is whatever the
model puts in a tool call's arguments; what the server then does with it, and
where it sends it, is that third party's business and is covered by their terms,
not this policy. Tokens you store for a server are passed to it as environment
variables — and the server process also inherits the rest of the environment
mindfork was started with, including any API keys exported in that shell. MCP is
a double opt-in: a master switch plus per-profile approval,
both off by default, and a server that changes its tool set after you approved it
has to be approved again.

### 3.7 The Python sandbox's one-time setup

`mindfork sandbox setup` is an explicit command, run once. It downloads the
`wasmer` runtime from its GitHub release page and 34 Python wheels from
`files.pythonhosted.org` and `pythonindex.wasix.org`; each of those is
**verified against a sha256 lock list** before use. It then runs the downloaded
`wasmer` binary to fetch the Python distribution (`python.webc`, the largest
piece) from the Wasmer registry. That one is fetched by `wasmer` rather than by
this app, so there is no URL for the lock list to pin — instead **the finished
file is checked against a pinned digest**, and one that does not match is
replaced rather than used. What that third-party binary contacts beyond this is
outside our control. Nothing about your data is sent in any of it; it is a
package download.

If a `wasmer` runtime is present but the setup never finished, a Python run can
make `wasmer` fetch the plain Python package from the Wasmer registry itself.

Once Python execution is enabled, the sandbox's own network access is on by
default (`tools.python_net_enabled`), which means code the model runs can reach
the internet from inside the sandbox. In the **local** Python mode the code runs
with your own interpreter and has whatever network access your computer has; that
switch does not apply to it.

### 3.8 The llama.cpp download

`mindfork llama backends` and `mindfork llama setup` are explicit commands. They
ask `api.github.com` for the release list of the `ggml-org/llama.cpp` repository,
and `setup` downloads the build you picked — and, for a CUDA build, its runtime
archive — from that release on GitHub, checking each file against the sha256
GitHub publishes for it. The requests carry the User-Agent `mindfork-llama-setup`
and no credentials: a `GITHUB_TOKEN` in your environment is not read.

### 3.9 The clipboard over SSH

When copying uses OSC 52 — automatically over a remote session or when the local
clipboard cannot be reached, or always if you set it — the copied text is handed
to the terminal, and over a remote session it travels the SSH connection to the
machine you are sitting at. That is the purpose of the feature, and it is worth knowing that the
text passes through whatever is between you and the host.

## 4. What is off until you turn it on

**Off until you say otherwise:**

- the **web tools** — search, page fetching, YouTube (§3.3);
- **Python execution** (`tools.python_enabled`);
- the **file tools** (`tools.fs_enabled`) — and they work only inside the folder
  you set as their root (`fs_root`): until you set one they refuse, and the app's
  own data and program folders stay out of their reach either way;
- **MCP plugins** — the master switch and the per-profile one, both;
- **cross-chat search** for the assistant (`chat_search`, `chat_read`);
- the **self-model tools** — and with them the injection of the self-model into
  the system prompt;
- **reflection** and the consolidation passes;
- reaching **private or loopback addresses** from the web tools;
- the **confirmation prompt** before a dangerous tool call
  (`tools.confirm_dangerous`) — it is opt-in, so tools you have enabled run
  without asking.

**On unless you turn them off:**

- fetching the **content of search results**, once the web tools are on;
- preferring a **keyed search provider**, once you have said where its key lives;
- **naming a new chat** automatically, which is one extra model request;
- **compacting a long history** into a rolling summary once it nears the model's
  context window, which is one extra model request each time;
- **showing the model the images a tool produced** — charts from Python, images
  from MCP tools — once those tools are on;
- **network access inside the Python sandbox**, once Python is enabled;
- the cloud as the **default speech provider** — though nothing is spoken until
  you invoke `/tts`;
- the ordinary local tools: notes, the knowledge base, the time, and so on.

## 5. What the author receives

Nothing. There is no telemetry, no analytics, no usage statistics, no crash or
error reporting, no update check, no version ping, no license check, no account
and no server operated by this project. The addresses of the website, the
repository and the crates.io page that appear in the "About" dialog are text on
a screen; nothing fetches them.

## 6. Logs

The app writes diagnostics to `logs/`, one file a day
(`mindfork.log.YYYY-MM-DD`), on your machine only.

**No message text, no prompt text and no API key is written to them.** What is
recorded is identifiers, counts, statuses, timings, model names, endpoint URLs,
error strings, and the paths and names of files you added to the knowledge base
or attached to a chat. Two things can carry third-party text into a log: the
error body a provider returned when a request failed, and lines an MCP server you
connected printed outside its protocol (its error output as well, if you raise
the level). At the default level, the URLs the web tools fetched are not logged;
they appear if you raise the level with `MINDFORK_LOG`.

Log files are **not pruned**: they accumulate until you delete them. They are
excluded from backups.

## 7. Secrets, and backups

**Secrets.** Keys you enter in settings — cloud provider keys, external-server
keys, MCP tokens, a search API key, the backup password — are stored inside
`settings.json` **encrypted and bound to the machine**: DPAPI on Windows, and on
Linux a key derived from the host's machine id and your user name, with
ChaCha20-Poly1305. A settings file copied to another computer therefore carries
no usable secret. The design and, importantly, its limits are in
[ADR 0008](docs/decisions/0008-api-key-storage.md): it defends against a
settings file carried off to another machine, not against code running as you on
your own. As an alternative you can store the
*name* of an environment variable instead of a key, in which case the app never
holds the value at all. Keys are sent only to the service they belong to.

**Backups** contain settings, profiles, your personal dictionary, a copy of
`data.db`, the whole `chats/` directory — that is, your conversations — the
files kept with chats (`files/`), the copies of project files the assistant
edited (`workspace/`), your dictionaries and locale overrides, the `.bak` copies
of all of these, and the file tools' folder when it lies inside the data folder.
They exclude logs and previous backups. A backup can be encrypted with a password
(AES-256); note that even then the archive's manifest, entry names and sizes
remain readable without it. Where you put a backup, and who can read it, is your
decision — an unencrypted backup is a copy of everything.

## 8. Files the app can write outside its own folder

For completeness, since "everything lives in `data/`" is true of the app's own
storage but not of everything it can do at your request:

- a Python run, in either mode, gets a working folder under the system temporary
  directory (`mindfork-sbx-<id>`) holding the model's code and copies of the
  chat files the call named; it is removed after the run, and one left behind by
  a crash is removed 24 hours later. The interpreter is given the script's path,
  not the code;
- `/export` writes a conversation to the path you give, or to a generated file
  name in the current directory;
- the file tools and the code-workspace tools write where the model asks, inside
  `fs_root` or the attached project directory — never into the app's own folders
  or a project's `.git/`;
- `mindfork demo` provisions a throwaway data directory under the system
  temporary folder and removes it on exit;
- copying puts text on the operating system clipboard, which other applications
  can read.

## 9. The website, mindfork.io

The site is **static** — pages generated ahead of time, served from Amazon S3
through CloudFront. It has **no analytics** of any kind, **no cookies** set by us
and therefore no cookie banner, **no forms**, no sign-up, no comments, and **no
third-party embeds**: fonts, styles and images come from the site's own origin.

The infrastructure that serves it
([infra/website.cfn.yaml](infra/website.cfn.yaml)) configures **no access
logging** — neither the bucket nor the distribution is set up to deliver or
retain request logs. Amazon Web Services is the hosting provider and processes
requests in order to serve them, under its own terms. While the site is
restricted to an IP allowlist during maintenance, a small function compares the
requesting address against that list at request time; it stores nothing.

## 10. GitHub

The source code, the issue tracker and the release downloads are hosted by
**GitHub**, so reading a page, opening an issue or downloading a release is
subject to
[GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-privacy-statement),
not this one. The author sees what any repository owner sees: public activity on
the repository and the contents of what you post there. Please do not put
personal data into an issue — a bug report needs a version, an operating system
and a reproduction, never your chat history. Security reports have their own
private channel ([SECURITY.md](SECURITY.md)).

## 11. Donations

If and when the project accepts donations, the donation platform handles the
payment and your payment details; the author never sees a card number. What
reaches the author is what such platforms show a recipient — a name or handle,
the amount, the date, and any message you attached. It is used only to
acknowledge the donation and to keep the project's accounts, is not combined
with anything else, is never sold or shared, and is not used to contact you
about anything you did not ask for.

Donating is never required to use, obtain or update the software, and no feature
is withheld from anyone who does not.

## 12. Deleting everything

Because the app is portable, deletion is a file operation and not a request to
anybody: remove the `data/` directory next to the executable, and any backup or
export you made elsewhere, and nothing of yours remains. Removing `cache.db`
alone just drops the derived search index. There is no account to close and
nothing of yours on any server of this project's.

Data you sent to a model or search provider is held by them, and only they can
delete it, under their own retention rules. The same is true of anything you
posted on GitHub.

## 13. Changes to this policy

The policy is versioned with the source: it lives in the repository, changes
through pull requests, and its history is public — so you can diff it. The date
at the top marks the current version. A change that widens what the software
sends anywhere is a user-visible change and would also appear in
[CHANGELOG.md](CHANGELOG.md).

## 14. Contact

Questions about this policy, or about what the software does with anything:
open an issue on the
[repository](https://github.com/vshylov/mindfork-rs/issues), or write to the
project's maintainer address, `vladimir.shylov@outlook.com` — the same address
that identifies the maintainer of the Linux packages. For anything that is
actually a security problem, use the private channel described in
[SECURITY.md](SECURITY.md) rather than a public issue.
