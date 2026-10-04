# homebrew-tap

[![Bump the formula](https://github.com/vshylov/homebrew-tap/actions/workflows/bump.yml/badge.svg)](https://github.com/vshylov/homebrew-tap/actions/workflows/bump.yml)

A [Homebrew](https://brew.sh) tap for **[mindfork](https://mindfork.io)** — a
terminal AI chat written in Rust: local models via llama.cpp, or OpenAI,
Anthropic, Gemini, Grok and OpenRouter in the cloud, with persistent memory,
notes, RAG and tools.

## Install

On a Mac with Apple Silicon:

```sh
brew install vshylov/tap/mindfork
```

Then run `mindfork` in any terminal — or `mindfork demo` to look around with
sample chats and a scripted engine, no model and no key needed.

macOS support is a **preview**: what is known to differ on a Mac, and what is
still being checked, is in
[docs/research/macos.md](https://github.com/vshylov/mindfork-rs/blob/main/docs/research/macos.md).

## What it installs

- The release's macOS build (`mindfork-rs-vX.Y.Z-aarch64-macos.tar.gz`), its
  digest taken from the release's own `sha256sums.txt`. A formula's download is
  not quarantined, so macOS starts it without asking.
- A `defaults.json` beside the binary that says `{ "mode": "system" }`: your
  chats, notes and settings live in `~/Library/Application Support/mindfork-rs`,
  so an upgrade or a `brew uninstall` leaves them where they are. What the file
  can say instead is in
  [install.md §2.1](https://github.com/vshylov/mindfork-rs/blob/main/docs/install.md#21-installation-defaults-defaultsjson).
- `mindfork` on your `PATH`.

Intel Macs have no build: neither the Python sandbox nor llama.cpp's Metal
exists for them. `cargo install --locked mindfork` builds one from source.

## Updates

[bump.yml](.github/workflows/bump.yml) looks for a new release every six hours,
renders the formula for it from [formula.rb.in](formula.rb.in), installs and
tests it, and only then commits it; `brew upgrade mindfork` installs it.

## Where to report what

A problem with the app — [mindfork-rs issues](https://github.com/vshylov/mindfork-rs/issues).
A problem with installing it through this tap —
[this repository's issues](https://github.com/vshylov/homebrew-tap/issues).
