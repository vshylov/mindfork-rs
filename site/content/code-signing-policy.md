+++
title = "Code signing policy"
description = "Who builds and approves a mindfork release, how the Windows binaries are signed, and what the program does with your data."
updated = "2026-09-18"
template = "doc.html"
+++

**Status, first, because it is the honest order.** mindfork's Windows binaries
are **not signed yet**. This page describes the arrangement the project is
applying for, and exists because a code signing policy has to be published
before anyone can review one. When the first signed release ships, this
paragraph goes away and the release notes will say so.

## Signing

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

The certificate is issued to SignPath Foundation, not to this project — so
Windows will name **SignPath Foundation** as the verified publisher, and the
program identifies itself through the `mindfork` product name in the file's own
metadata.

Only two artifacts are signed, both produced by the release workflow on
GitHub-hosted runners from a tag of the public repository:

- `mindfork.exe`, the program itself;
- `mindfork-rs-vX.Y.Z-x86_64-setup.exe`, the Windows installer, and the
  uninstaller it writes.

Every signing request is approved by hand. Nothing is signed from a developer's
machine, and nothing that was not built by that workflow is signed at all.

The Linux packages and the portable archives are **not** signed; verify them
against the `sha256sums.txt` published with each release.

## Team roles

The project has one maintainer, who therefore holds all three roles:

- **Authors** — [Vladimir Shylov](https://github.com/vshylov)
- **Reviewers** — [Vladimir Shylov](https://github.com/vshylov)
- **Approvers** — [Vladimir Shylov](https://github.com/vshylov)

Changes reach `main` through pull requests, and releases are cut from tags on
`main`. Source, issues and every release artifact:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).

## Privacy

This program will not transfer any information to other networked systems
unless specifically requested by the user or the person installing or operating
it.

That sentence is exact rather than boilerplate: mindfork has no telemetry, no
analytics, no crash reporting and no update check, and every address it contacts
is one you configured or a tool you switched on. Which files it keeps, which
destinations it can reach at all, and which setting of yours has to be on first
are set out in the **[privacy policy](/privacy/)** — including the policies of
the third-party services you may point it at, since those are the only places
your conversations can go.
