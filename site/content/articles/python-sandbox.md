+++
title = "Why the Python sandbox is WebAssembly, not Docker"
description = "python_exec runs model-written code in a CPython compiled to WebAssembly — numpy, pandas and requests included — inside a box that ships with the app and, by default, cannot see your machine."
weight = 6
+++

*The sixth in a short series on how mindfork is put together. Earlier:
the [overview](/articles/mindfork-at-a-glance/),
[the engine](/articles/engine-as-a-server/),
[the self-model](/articles/self-model/),
[vector search](/articles/vector-search/) and
[local speed](/articles/local-speed/).*

## The sharpest tool in the box

Among the tools the assistant can call, `python_exec` is the bluntly
powerful one: the model writes a program, the app runs it, the output
returns to the conversation. Real computation mid-chat — statistics
over a CSV, a date calculation done right, a page fetched and parsed —
instead of a language model doing arithmetic in its head.

It is also the sharpest edge in the toolbox, because the question is
never whether the code works; it is where it runs. The first version
ran it on the system Python — your interpreter, your filesystem, your
network, your processes — and for exactly that reason shipped disabled.
The tool was honest about being dangerous. The task was to make it
honest about being safe.

## Three roads

**The system interpreter** is the road already tried: zero isolation,
plus a dependency the app cannot vendor — whatever Python the machine
happens to have, or none at all.

**Docker** is the reflex answer, and a fine one for a server product.
For a portable terminal app it is a strange fit: a daemon that must be
installed and kept running, images to pull, WSL2 or Hyper-V underneath
it on Windows, root on Linux. mindfork runs out of a folder you can
copy; "first, install a container platform" is not a sentence that
belongs in its install guide.

**WebAssembly** is the third road: compile CPython itself to wasm and
run it in a sandboxed runtime inside the app's own footprint. No
daemon, no images, no kernel features to arrange — the isolation is the
instruction set. That is the road taken.

## What it took to make Python real there

Plain WASI — the standard system interface for wasm — was not enough.
The official CPython-on-WASI build has no sockets, no threads and no
dynamic linking, which rules out the two packages that make the tool
worth calling: no `requests`, and no numpy, whose fast paths are native
modules the interpreter must load at runtime. Pyodide lives in browsers
and Node; the alternative Python implementations lack the C API that
native wheels need.

The combination that survived is **Wasmer + WASIX** — WASI extended
with exactly the missing pieces. Inside it runs a real CPython 3.13;
numpy's native modules load through dynamic linking; `requests`
completes an HTTPS call through sandboxed sockets. All of it was proven
live before the design was adopted, not assumed from the feature lists.

## A separate process, for the third time

This is the third time mindfork chose a separate process behind a small
contract over a linked library: [the engine](/articles/engine-as-a-server/)
is a server, [the embedder](/articles/vector-search/) is a second one,
and the sandbox is a **sidecar** — a bundled `wasmer` binary the app
spawns per call.

Embedding the runtime instead would have dragged a C++ toolchain and a
static JavaScript-engine build into every compile of a Rust TUI. The
sidecar keeps the main binary clean, and buys properties that matter
more than elegance: interrupting a runaway script is a process kill —
measured at about a third of a second, with no cooperative cancellation
to trust; a crash or out-of-memory dies in the sidecar without taking
the chat down; updating the runtime is replacing one file.

## Deny by default

A container walls off a full userland with kernel namespaces. The wasm
sandbox starts from the other end: the guest begins with *nothing* and
is granted a scratch directory holding the script, plus a read-only
package library. Your documents are not forbidden — they are absent.
There is no path to them, so there is nothing to escape to.

Network works the same way. It has its own toggle, and with it off the
sandbox is started without sockets — nothing to firewall, no rule to
get wrong. A timeout kills the process; one script runs at a time; on
Windows an optional hard memory cap can be set at the OS level. And the
tool itself stays **off by default** — enabling it is a deliberate act,
taken after the sandbox is actually installed.

## Batteries, pinned

Provisioning is one command: `mindfork sandbox setup`. It downloads the
runtime, the CPython package and the starter library — numpy and pandas
as native WASIX wheels, `requests` and beautifulsoup4 with their small
dependencies — from a lock list with exact URLs and checksums. Nothing
rides "latest"; what was tested is what installs. Setup is idempotent,
and it warms the compilation cache at install time, so the first real
call in a chat does not pay the seconds of compiling an interpreter.

## One EINVAL from working

The war story that justifies the "proven live" habit: WASIX does not
implement the `TCP_NODELAY` socket option, and Python's HTTP stack sets
it unconditionally — so sockets existed, and yet every network call
died on an error the feature lists could never have predicted. A
three-line shim taught the socket to shrug that option off, and
`requests` worked. The gap between "sockets exist" and "requests works"
is exactly the gap between reading about a runtime and running one.

## One tool, whatever the box

The model sees a single `python_exec` tool with a single schema; which
sandbox answers is the user's setting, not the model's concern. The
system-interpreter mode still exists, one setting away, for those who
want their own Python and accept what that means. The box is the
default.

The full decision record — ADR 0005 and the sandbox research log, with
every alternative and measurement — lives in the repository:
[github.com/vshylov/mindfork-rs](https://github.com/vshylov/mindfork-rs).
