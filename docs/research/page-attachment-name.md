# A fetched page's attachment is named after the page

<!-- cyrillic-ok:start — the corpus below includes Russian and Ukrainian pages, and
     their names are the measurement: transliterating them would describe a
     different page. The prose itself is English. -->

> **Status:** implemented (2026-09-11) — the user's decisions of 2026-09-11: every fork as recommended —
> F1c the agreement rule, F2a everything before the title's last separator, F3a the
> name cleaned. The follow-up the
> page-charset fix ([page-charset.md](page-charset.md)) surfaced: with
> `sector.biz.ua`'s article finally readable, its attachment turned out to be named
> after the archive the article sits in, not the article.

## 1. What names a page today

`fetch_url` attaches a page over the attachment budget (spec §9.3.1) and names the
attachment in `features/tools/fetch.rs`: `page_name` takes the text of the first
`<h1>`, else of `<title>`, whitespace collapsed and clipped to 60 characters
(`NAME_TITLE_CHARS`); `attached_result` passes it through `unique_name`, which
appends the URL's last path segment when a *different* source already holds that
name.

`<h1>` comes first because of `docs.vlang.io`, where every page's `<title>` is
"V Documentation" and the `<h1>` is the page
([fetch-url-fidelity.md](../history/fetch-url-fidelity.md), S8).

The reported page has the opposite shape. `sector.biz.ua` puts the archive's banner
in the `<h1>` of every article, and the article in front of its `<title>`:

| Page | `<h1>` | `<title>` |
|---|---|---|
| `mid203/aid5.html` | АРХИВ СТАТЕЙ ЖУРНАЛА «МОЙ КОМПЬЮТЕР» ЗА 2002 ГОД | Попьем чайку? Петр 'roxton' Семилетов \| Архив журнала «Мой компьютер» №32/203 \`2002 |
| `mid199/aid2.html` | АРХИВ СТАТЕЙ ЖУРНАЛА «МОЙ КОМПЬЮТЕР» ЗА 2002 ГОД | ВодВАRить на место. Геннадий Осипенко \| Архив журнала «Мой компьютер» №28/199 \`2002 |

So the first article attached is called "АРХИВ СТАТЕЙ ЖУРНАЛА «МОЙ КОМПЬЮТЕР» ЗА 2002
ГОД", and a second one from the site "АРХИВ СТАТЕЙ ЖУРНАЛА «МОЙ КОМПЬЮТЕР» ЗА 2002 ГОД —
aid2.html": two names that differ, and neither says what the page is.

**A correction to the premise.** `attachment_read` no longer resolves a name to its
first match: the same fidelity track made it report an ambiguous name with each
candidate's source (S10, `AttachmentRead::invoke`). The comments on `page_name` and
on its test still say "first match" and are stale. A collision is therefore loud to
the model, not silent. `/file remove <name>` does still act on the first match
(`file_command::resolve_target`), which is one reason a distinct name is still worth
keeping. What this track fixes is what the name *means*: an attachment called after
its site tells neither the model nor the user which page it holds, and a second page
from that site is told apart by a file name.

## 2. The corpus

Measured on 2026-09-11: **43 sites, 87 pages** — documentation (13), reference (2:
the English and Russian Wikipedias), code (GitHub), news (9), archives (3), blogs
(7), forums and Q&A (8); 76 pages in UTF-8, 9 in windows-1251, 2 in KOI8-R.

- **How.** Each page was fetched with the tool's own headers (`USER_AGENT`,
  `ACCEPT_HTML`, `ACCEPT_LANGUAGE`) — two or three per site, named pages or the first
  article links on the site's own index — and its body saved. A probe script read
  every `<h1>`, the `<title>`, `og:title` and `og:site_name`; then `page_name` and the
  candidate rule of §4 were run again **inside the app's test binary** on the saved
  bytes, through `text_decode::decode` and `scraper` — the app's own path.
- **What the app sees is what counts.** The probe and the app disagreed on two pages:
  Discourse (`users.rust-lang.org`) puts its `<h1>` inside `<noscript>`, which
  `html5ever` — scripting enabled, as `scraper` runs it — parses as text. The app sees
  no `<h1>` there, and names the page by its whole `<title>` today.
- **Not in the corpus, because the tool cannot fetch them from here either:** Stack
  Overflow, LWN, LinuxQuestions, the Raspberry Pi forums and phpbb.com answered 403
  or Cloudflare's "Just a moment..." to the tool's headers; lenta.ru, LiveJournal and
  cyberforum.ru do not resolve from this network.

**Four shapes**, by what the fields name:

| Shape | Sites | Today's name |
|---|---|---|
| `<title>` constant, `<h1>` is the page | docs.vlang.io | the page — right |
| **first `<h1>` constant, `<title>` is the page** | sector.biz.ua; the Rust Book (mdBook's `<h1 class="menu-title">`); jvns.ca and astralcodexten.com (a banner `<h1>` before the post's); simonwillison.net; bbs.archlinux.org (FluxBB) | **the site, on every page** |
| both name the page | 26: docs.python.org, MDN, docs.rs, Microsoft Learn, kubernetes.io, vite.dev, Read the Docs, go.dev, docusaurus.io, cppreference, both Wikipedias, GitHub, Habr, Українська правда, Ars Technica, The Guardian, BBC, 3DNews, iXBT, ITC.ua, DEV, Project Zero (Blogspot), the JetBrains blog, linux.org.ru, DOU | the page, with Sphinx's `¶` ("json — JSON encoder and decoder¶"), VitePress's zero-width space, or rustdoc's button text ("Crate serde Copy item path") |
| no `<h1>` | 10: the PostgreSQL docs, blog.rust-lang.org, Hacker News, opennet.ru, users.rust-lang.org, the iXBT and Ru.Board forums, forum.sources.ru, az.lib.ru, citforum.ru | the whole `<title>`, site suffix included |

And the fields themselves:

- **`og:title`**: on 55 of 87 pages and 27 of 43 sites — **constant on none**. It is
  absent on the documentation generators (mdBook, Sphinx on Read the Docs, docs.vlang.io,
  rustdoc, cppreference, go.dev), on MDN and Hacker News, and on every old site
  (sector.biz.ua, opennet, az.lib.ru, citforum, and the forums on old engines: iXBT,
  Ru.Board, forum.sources.ru and the FluxBB at bbs.archlinux.org).
- **A constant first `<h1>`**: 6 sites. **A constant `<title>`**: 1 site.
- **`<title>` segments**: 66 of 87 titles carry a spaced separator (` | `, ` — `,
  ` - `, ` / `, ` :: `, ` -> `). Where one segment is the site's, it is the **last** —
  on every such title but GitHub's repository page, which has the site at both ends
  ("GitHub - rust-lang/rust: … · GitHub").

## 3. Candidate rules

A **collision** is two different pages of one site under one name after the
60-character clip. Today's rule and the rule of §4 are counted in the app; the other
three in the probe (whose extra `<noscript>` `<h1>` changes no row).

| Rule | Sites with a collision |
|---|---|
| today: `<h1>` → `<title>` | **6**: sector.biz.ua, the Rust Book, jvns.ca, simonwillison.net, astralcodexten.com, bbs.archlinux.org |
| `og:title` → `<h1>` → `<title>` | 3: sector.biz.ua, the Rust Book, bbs.archlinux.org — none of which sets `og:title` |
| the `<title>`'s first segment → `<h1>` → `<title>` | 3: GitHub (site-first), jvns.ca and simonwillison.net (a bare post title, so the banner `<h1>` wins) |
| an `<h1>` found anywhere in the `<title>` → its first segment → … | 3: the Rust Book (its banner **is** the title's suffix), simonwillison.net, bbs.archlinux.org |
| the first `<h1>` outside `header`/`nav`/banner-like classes → `<title>` | 3: sector.biz.ua, simonwillison.net, bbs.archlinux.org — and the class test also flags the real headings of Habr, Ars Technica and Wikipedia |
| **agreement (§4)** | **0** |

## 4. The rule: a name a second field confirms

A page states its name in up to three places — `og:title`, an `<h1>`, the `<title>` —
and its site's name in some of the same places. What a second field confirms is the
page; the site's part of a `<title>` is its end. First match wins:

1. **`og:title`** — set per page wherever it is set at all — unless it is the site's
   own name (equal to `og:site_name` or to the `<title>`'s last segment). A trailing
   segment it shares with the `<title>` is dropped: "Moon - Wikipedia" → "Moon".
2. **an `<h1>` the `<title>` begins with**, any `<h1>` rather than the first (jvns.ca's
   second one is the post), spelled as the `<title>` spells it — which drops Sphinx's
   `¶` and VitePress's zero-width space.
3. **the `<title>` without its last segment** — everything before the last spaced
   separator.
4. **the first `<h1>`** — docs.vlang.io, whose `<title>` is the site's name alone.
5. **the `<title>`.**

On the corpus the steps name 55, 8, 16, 2 and 6 pages. In the app, the rule gives the
probe's name on all 87 pages, and **no site has two pages under one name**.

| Site | Today | The rule |
|---|---|---|
| sector.biz.ua | АРХИВ СТАТЕЙ ЖУРНАЛА «МОЙ КОМПЬЮТЕР» ЗА 2002 ГОД | Попьем чайку? Петр 'roxton' Семилетов |
| doc.rust-lang.org/book | The Rust Programming Language | What is Ownership? |
| bbs.archlinux.org | Arch Linux | The Official Hello Everyone Thread / Newbie Corner |
| simonwillison.net | Simon Willison’s Weblog | Any Nix package, live in your browser |
| docs.vlang.io | Memory management | Memory management |
| docs.python.org | json — JSON encoder and decoder¶ | json — JSON encoder and decoder |
| docs.rs | Crate serde Copy item path | serde |
| postgresql.org | PostgreSQL: Documentation: 18: SELECT | SELECT |
| users.rust-lang.org | Welcome to the Rust programming language users forum - meta | Welcome to the Rust programming language users forum |
| en.wikipedia.org | Moon | Moon |

## 5. Where it is still wrong

- **A bare `<title>` that is the page, a banner `<h1>`, and no `og:title`** —
  simonwillison.net without its `og:title` — gets the banner at step 4, as today, and
  the second such page gets `unique_name`'s URL segment, as today. No page in the
  corpus has this shape: every blog with a banner `<h1>` sets `og:title`.
- **A site-first `<title>`** ("Gentoo Forums :: View topic - …", phpBB 2's default)
  with no `og:title` gets the site's name at step 3 — even when an `<h1>` repeats the
  page at the title's end, because accepting that would name the Rust Book after
  itself: its banner is exactly the title's end. None was reachable for the corpus
  (the Gentoo forums' index has moved, phpbb.com refuses the tool); `unique_name`
  separates the second page, as today.
- **A site that sets `og:title` to its own name on every page** is caught only when
  that name is also its `og:site_name` or its `<title>`'s last segment. Not seen.
- **Breadcrumb titles keep their middle segments** ("…Thread / Newbie Corner"), and the
  clip can end on a separator. Cosmetic: the name still tells the pages apart.

## 6. Forks

**F1 — the rule.**

- (a) Keep `<h1>` first. Six of 43 sites name every page after themselves.
- (b) `og:title` first, then today's order. The smallest change: it fixes the three
  blogs, and none of the reported site, the Rust Book or the Arch forums, which do not
  set `og:title`.
- (c) **The agreement rule of §4 — recommended.** No collision on the corpus; the
  reported page and docs.vlang.io are both named by their page.
- (d) Learn a site's constant fields from the chat's other attachments from the same
  host. Rejected: the first page from a site is still named by its banner; a name
  would depend on what else is attached, so re-fetching a page could rename it —
  the property `unique_name` was made deterministic to keep; and renaming an
  attachment already made changes a name the model has been told.

**F2 — which part of a multi-segment `<title>` is the page (step 3).**

- (a) **Everything before the last separator — recommended.** A page's own name
  carries a spaced separator often enough to matter — 5 of the 55 `og:title`s ("json —
  JSON encoder and decoder", "The Qualcomm DSP Driver - Unexpectedly Excavating an
  Exploit") — and cutting there drops the words that tell two pages apart. The cost is
  a forum's breadcrumbs, bounded by the clip.
- (b) The first segment. Clean on breadcrumb forums; cuts one page name in eleven at
  its first dash.

**F3 — the name's spelling.**

- (a) **Cleaned — recommended**: `og:title` without the `<title>`'s site suffix, an
  agreeing `<h1>` as the `<title>` spells it. "Moon" rather than "Moon - Wikipedia",
  "Quickstart" rather than "Quickstart¶".
- (b) Verbatim: each field as the page has it.

## 7. What this does not do

- `/file remove <name>` still acts on the first match — a different entry point; a
  fetched page's unique name keeps it apart.
- `youtube_watch` keeps the video's title from its API; `web_search` attaches nothing.
- The 60-character clip is unchanged.

## 8. Tests and the live run

- **Unit, through `body_to_text`**, one fixture per shape of §2: the archive's banner
  `<h1>` over an article-first `<title>` (the reported page); mdBook's banner as the
  title's suffix; a second `<h1>` that is the post; a banner under `og:title`;
  `og:title` with the site's suffix, equal to the title's site segment, and equal to
  `og:site_name`; an agreeing `<h1>` in another case and with VitePress's zero-width
  anchor, read as the title spells it; an `<h1>` that is only a word's start ("Go" in
  "Google"); rustdoc's title; a page name that holds a separator; Sphinx's `¶` under a
  bare site `<title>` (docs.vlang.io's shape, the old test kept); no `<h1>` at all. And
  the title's split, one row per separator, unspaced ones left alone.
- **Through the tool**: two archive-shaped pages served locally attach under their
  articles' names, the second without a URL segment.
- **Mutation-tested**: each step dropped or moved fails its own fixture.
- **Live** (network only — the name is not the model's):
  `live_windows_1251_page_is_readable` names the attachment "Попьем чайку? Петр
  'roxton' Семилетов"; docs.vlang.io's memory-management page is still "Memory
  management"; and the corpus run again on the implemented function, with no
  collision.

<!-- cyrillic-ok:end -->
