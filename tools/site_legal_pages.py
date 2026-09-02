#!/usr/bin/env python3
"""Generate the website's legal pages from the documents in the repository.

Today that is one page: `PRIVACY.md` -> `site/content/privacy.md`. The policy
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
* **Leaves everything else alone**, including the section numbering the text
  cross-references.

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

#: Front matter for the generated page. `legal.html` is `page.html` without the
#: reading-time line: a policy is not an article, and "12 min read" over a legal
#: text reads as a warning rather than a service.
FRONT_MATTER = """+++
# Generated from PRIVACY.md by tools/site_legal_pages.py - do not edit.
title = "Privacy policy"
description = "What mindfork keeps on your machine, what leaves it and only on which setting of yours, and what reaches its author — which is nothing."
template = "legal.html"
+++

"""

#: A Markdown link whose target is neither absolute nor an anchor.
RELATIVE_LINK = re.compile(r"\]\((?!https?://|#|/)([^)]+)\)")


def render(markdown: str) -> str:
    """The repository document -> the page Zola builds."""
    body = markdown.lstrip()
    if body.startswith("# "):
        body = body.split("\n", 1)[1].lstrip("\n")
    body = RELATIVE_LINK.sub(lambda m: f"]({BLOB}{m.group(1)})", body)
    return FRONT_MATTER + body


def main() -> int:
    check = "--check" in sys.argv
    src, out = ROOT / "PRIVACY.md", ROOT / "site" / "content" / "privacy.md"
    wanted = render(src.read_text(encoding="utf-8"))
    current = out.read_text(encoding="utf-8") if out.exists() else None

    if check:
        if current == wanted:
            print(f"OK: {out.relative_to(ROOT).as_posix()} matches {src.name}")
            return 0
        print(
            f"stale: {out.relative_to(ROOT).as_posix()} does not match {src.name}\n"
            "regenerate: python tools/site_legal_pages.py",
            file=sys.stderr,
        )
        return 1

    if current == wanted:
        print(f"unchanged: {out.relative_to(ROOT).as_posix()}")
        return 0
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(wanted, encoding="utf-8")
    print(f"written: {out.relative_to(ROOT).as_posix()} ({len(wanted)} chars from {src.name})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
