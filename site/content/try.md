+++
title = "Try it in your browser"
description = "The mindfork interface running in a browser tab: the program itself inside an emulated PC, with a scripted engine in place of a model. Nothing to install, nothing leaves the tab."
template = "try.html"
+++

The same program you would install, built for 32-bit x86 and running inside a PC
emulated in WebAssembly, in this tab. Nothing is installed, nothing you type
leaves your browser, and closing the tab discards it. The engine that answers is
a script; the machine has no network, so a real model cannot be connected here.

It is slower here than on your computer, and the terminal is the browser's rather
than yours: fonts, colours and a few keys differ. To use it for real,
[install it](@/install.md) and run `mindfork demo` — or connect a model.
