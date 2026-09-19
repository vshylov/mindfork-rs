#!/usr/bin/env python3
"""Generate the website's legal pages from the documents in the repository.

Three pages: `PRIVACY.md` -> `/privacy/`, `DISCLAIMER.md` -> `/disclaimer/` and
`LICENSE` -> `/license/`. The policy
has to exist in three places at once — the repository (where it is edited and
reviewed), the Windows installer (a wizard page, via `tools/wizard_rtf.py`) and
the website (a reviewer of a code-signing application follows a link from the
home page, docs/research/code-signing.md §7) — and only the first of those is
a place to edit it. So the other two are generated, and drift is a failing
check rather than a discovery.

**Committed, not gitignored**, unlike the brand assets `tools/site_sync_assets.py`
mirrors: this is prose that lands on a public page, so it belongs in a diff a
human reads, exactly like the installer's `.rtf` files. `--check` in CI's lint
job is what keeps the copy honest.

What the transform does, and why each part is needed:

* **Drops the source's `#` heading.** The page template renders `page.title` as
  the `<h1>`; keeping the document's own would print it twice.
* **Rewrites repository-relative links to absolute GitHub URLs.** `PRIVACY.md`
  links to `SECURITY.md`, `docs/install.md`, `infra/website.cfn.yaml` and
  others — targets that resolve on GitHub and in a checkout, and resolve to
  nothing on a static site. This is the reason the page is generated rather
  than copied by hand: a copy would rot link by link.
* **Carries over the document's own effective date**, where it states one, into
  the page's `updated` field - which is what reaches the sitemap's `lastmod` and
  the page's structured data. A source that states no date gets none.
* **Leaves everything else alone**, including the section numbering the text
  cross-references.

`LICENSE` is the one source that is not Markdown: the MIT text is plain prose
with hard-wrapped lines, so it is fenced rather than rendered, which keeps it
byte-identical to the file a court would read.

Usage:
    python tools/site_legal_pages.py            # (re)generate
    python tools/site_legal_pages.py --check    # verify it matches (CI)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where a repository-relative link points once the text is on the website.
BLOB = "https://github.com/vshylov/mindfork-rs/blob/main/"

#: One generated page: the source document, the page it becomes, its title and
#: description. `doc.html` is `page.html` without the reading-time line — a
#: policy is not an article, and "12 min read" over a legal text reads as a
#: warning rather than a service.
PAGES = [
    {
        "src": "PRIVACY.md",
        "out": "privacy.md",
        "title": "Privacy policy",
        "description": (
            "What mindfork keeps on your machine, what leaves it and only on "
            "which setting of yours, and what reaches its author — which is nothing."
        ),
    },
    {
        "src": "DISCLAIMER.md",
        "out": "disclaimer.md",
        "title": "Disclaimer",
        "description": (
            "The app ships no model: what that means for the output you get, for "
            "the tools a model may invoke on your machine, and for warranty and "
            "liability."
        ),
    },
    {
        "src": "LICENSE",
        "out": "license.md",
        "title": "License",
        "description": "mindfork is MIT-licensed — the standard text, unmodified.",
        # Not Markdown: fenced verbatim, so the page carries the licence exactly
        # as the file does.
        "verbatim": True,
    },
]

FRONT_MATTER = """+++
# Generated from {src} by tools/site_legal_pages.py - do not edit.
title = "{title}"
description = "{description}"
{updated}template = "doc.html"
+++

"""

#: A document that carries its own effective date states it in the opening
#: lines, as `Effective **2026-09-17**`. That date becomes the page's `updated`,
#: which is what the sitemap publishes as `lastmod` and what the structured data
#: reports as `dateModified`. It is deliberately not the file's mtime and not
#: the last commit to touch the file: the author moves this line when the policy
#: changes, and a typo fix must not tell a crawler the policy did. A source
#: without such a line gets no `updated` at all - inventing one would be a claim
#: about when the text last changed.
EFFECTIVE = re.compile(r"^Effective \*\*(\d{4}-\d{2}-\d{2})\*\*", re.M)

#: A Markdown link whose target is neither absolute nor an anchor.
RELATIVE_LINK = re.compile(r"\]\((?!https?://|#|/)([^)]+)\)")


def render(page: dict, text: str) -> str:
    """The repository document -> the page Zola builds."""
    effective = EFFECTIVE.search(text)
    front = FRONT_MATTER.format(
        src=page["src"],
        title=page["title"],
        description=page["description"],
        updated=f'updated = "{effective.group(1)}"\n' if effective else "",
    )
    if page.get("verbatim"):
        return front + "```\n" + text.rstrip("\n") + "\n```\n"
    body = text.lstrip()
    if body.startswith("# "):
        body = body.split("\n", 1)[1].lstrip("\n")
    body = RELATIVE_LINK.sub(lambda m: f"]({BLOB}{m.group(1)})", body)
    return front + body


def main() -> int:
    check = "--check" in sys.argv
    stale = 0
    for page in PAGES:
        src = ROOT / page["src"]
        out = ROOT / "site" / "content" / page["out"]
        wanted = render(page, src.read_text(encoding="utf-8"))
        current = out.read_text(encoding="utf-8") if out.exists() else None
        name = out.relative_to(ROOT).as_posix()

        if check:
            if current == wanted:
                print(f"OK: {name} matches {page['src']}")
            else:
                stale += 1
                print(
                    f"stale: {name} does not match {page['src']}\n"
                    "regenerate: python tools/site_legal_pages.py",
                    file=sys.stderr,
                )
            continue

        if current == wanted:
            print(f"unchanged: {name}")
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(wanted, encoding="utf-8")
        print(f"written: {name} ({len(wanted)} chars from {page['src']})")
    return 1 if stale else 0


if __name__ == "__main__":
    raise SystemExit(main())
