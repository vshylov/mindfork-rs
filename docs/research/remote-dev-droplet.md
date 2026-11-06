# A long-lived development machine: Claude Code Remote Control on a DigitalOcean droplet

How to give a Claude Code agent working on this repository a machine of its own
— more memory and disk than a cloud session's container, no network allowlist,
the cloud providers' keys for the live smokes — that runs for weeks and is driven
from the Claude app on a phone or from claude.ai/code. What was looked at, what
was measured, what was chosen, the recipe, and every trap the first setup hit.

User's decision (2026-11-05): **Remote Control on a CPU-only DigitalOcean
droplet**, on a Claude **Max** plan (§2, §4). The recipe in §5 is the one that
produced a working machine, with the defects of the first attempt (§6) fixed.

Related: [CONTRIBUTING.md](../../CONTRIBUTING.md) (the build with nothing
installed), [docker-jupyter-env.md](docker-jupyter-env.md) (a disposable CPU
stack on your own host), [docs/history/remote-e2e-hf.md](../history/remote-e2e-hf.md)
(the rented GPU live gate this does **not** replace — a CPU droplet runs the
cloud-provider smokes, not the local-model ones).

## 1. The need

- An agent session that can run for **weeks**: long tasks, background builds,
  state that survives between conversations.
- **Resources**: the full test build peaks at ~6 GB of memory and leaves a
  multi-gigabyte `target/` per checkout (§3).
- **Network**: no allowlist to extend host by host.
- **Keys**: `MINDFORK_OPENAI_KEY`, `MINDFORK_GEMINI_KEY`, `MINDFORK_ANTHROPIC_KEY`,
  `MINDFORK_OPENROUTER_KEY` and the rest, so the cloud-provider `#[ignore]`
  smokes run (CLAUDE.md §Commands).
- Driven from the phone or another computer; nothing typed on the machine itself
  after the setup.

## 2. What Claude Code offers (as documented on 2026-11-05)

| Route | Plans | Where commands run | Fit |
|---|---|---|---|
| Cloud session (the default environment) | all with Claude Code on the web | Anthropic's container | pauses after a few minutes idle, can be reclaimed; background processes do not survive; disk is a fixed allowance |
| [Self-hosted environments](https://code.claude.com/docs/en/self-hosted-environments.md) | **Team / Enterprise**, public beta, an Owner enables it | your host, via `claude self-hosted-runner` | exactly "a custom environment in place of the container" — not available on Pro/Max |
| [Remote Control](https://code.claude.com/docs/en/remote-control.md) | Pro, Max, Ultra, Team, Enterprise | your host, via `claude remote-control` | the sessions appear in the Claude app and on claude.ai/code beside the cloud ones |

**Every route that runs commands on your machine needs the `claude` binary on
it** — it is the process that reads the files and runs the commands; the model
itself always runs at Anthropic. Remote Control only makes outbound HTTPS
connections, so the machine needs no inbound port.

Facts that shape the setup (from the Remote Control and
[authentication](https://code.claude.com/docs/en/authentication.md) pages):

- It needs a **full subscription login** (`claude auth login` / `/login`). Neither
  an API key nor the token of `claude setup-token` can open Remote Control
  sessions. A set `ANTHROPIC_API_KEY` wins over the subscription — keep it unset.
- On a headless machine the login prints a URL (press `c` to copy it); open it
  anywhere and paste the code back.
- The first run asks two questions (enable Remote Control, trust the folder);
  without a TTY it exits with "Workspace not trusted". Run it once over `ssh -t`.
- The server **exits after about ten minutes without network**. Sessions stopped
  with Ctrl+C can be resumed for about four hours.
- A login warns three days before it expires; an expired one stops unattended
  sessions silently. The lifetime is not documented.
- Running it under systemd is **not documented** — the unit in §5.7 is ours, and
  it works.

The cloud container is not short of resources for this project (§3): what it
cannot give is persistence and an open network. If only the network matters, an
unrestricted network on the cloud environment is the cheaper answer.

## 3. What the project needs (measured)

Measured in a Claude Code cloud container (4 cores, 15 GB, no swap), commit
`ec7298b`, toolchain 1.96.0, memory sampled every 3 s:

| Step | Wall time | Peak memory used | Disk |
|---|---|---|---|
| `cargo test --no-run` from clean | 238 s | ~5.9 GB | `target/` 3.8 GB, `~/.cargo/registry` 0.4 GB |
| `cargo test` | 112 s | ~3.4 GB | — |

`cargo test`: **4802 passed, 322 ignored**. Clippy, a release build and every
extra worktree add their own `target/` trees on top.

One system package is required beyond a compiler: **`libasound2-dev`** —
without it `alsa-sys` fails its build script (`The system library 'alsa'
required by crate 'alsa-sys' was not found`). CI installs the same package.

So: **4 vCPU / 8 GB plus swap** is the floor, 16 GB is comfortable for parallel
sessions, and disk should be counted in tens of gigabytes.

**Python.** The gates and most of `tools/` are standard library only. Three
packages are needed by the media tools — `tools/screenshots.py`,
`tools/demo_reel.py`, `tools/hero.py`, `tools/og_card.py` and
`assets/build-wordmarks.py` — and they are pinned with hashes in
[`tools/media-requirements.txt`](../../tools/media-requirements.txt): **Pillow,
fontTools, Brotli** (Brotli reads the site's woff2 faces). Pillow 13 needs
Python 3.11 or newer; Ubuntu 24.04's 3.12 installs the file as is (checked, as
was 3.13). `requests` is needed only by two llama-server probes
(`tools/cache_ram_probe.py`, `tools/kv_pool_probe.py`) and is not pinned.
Ubuntu 24.04 refuses `pip install` into the system Python (PEP 668), so the
packages go into a virtual environment (§5.4), which also provides the
`python` command the documentation's examples use — Ubuntu itself has only
`python3`.

**Fonts** for the screenshots and the reel: `fonts-jetbrains-mono` (the brand
face `tools/screenshots.py` probes for under `/usr/share/fonts`) plus
`fonts-dejavu-core` and `fonts-wqy-microhei` for the glyphs it lacks — the same
fallbacks the release job installs. With them, `tools/screenshots.py --only
chat-dark-en` renders on Ubuntu: the SVG came out byte-identical to the
committed one, the PNG differed in pixels — regenerate and commit only the
images a change is meant to touch. The full set did not: the two `chat-list`
dumps hold `⌕` (U+2315, the list's search glyph), which neither JetBrains Mono
nor `DejaVuSans.ttf` has, and the script stopped with `no font covered: ⌕`
(§6, item 7).

## 4. The machine chosen

- Droplet `s5-4vcpu-8gb-50gb`: shared CPU, 4 vCPU, 8 GB, 50 GB boot disk,
  Ubuntu 24.04 LTS.
- A **200 GB block volume** for the checkouts and their `target/` trees. It
  outlives the droplet: recreate the machine, attach the volume, keep the work.
- Price at creation: **$0.132/hour, about $96/month** (compute $0.0956/h, boot
  disk $0.0069/h, volume $0.030/h). A volume can grow but never shrink.

## 5. Recipe

### 5.1 Create the droplet

- Ubuntu 24.04 LTS, **SSH key** authentication.
- **Monitoring** on (free; alerts on disk and CPU).
- The volume with **Automatically format & mount**.
- Backups are optional, and they do **not** cover the volume — snapshot the
  volume separately.
- **User data** — §5.2, pasted as is.

### 5.2 User data (ASCII only)

```yaml
#cloud-config
# ASCII only: a single non-ASCII byte makes cloud-init skip this whole file.
package_update: true
package_upgrade: true
packages: [build-essential, pkg-config, libasound2-dev, git, python3, python3-venv, fonts-jetbrains-mono, fonts-dejavu-core, fonts-wqy-microhei, tmux, unattended-upgrades]
users:
  - default
  - name: dev
    shell: /bin/bash
    ssh_authorized_keys:
      - ssh-ed25519 AAAA... you@laptop
runcmd:
  - fallocate -l 8G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  - echo '/swapfile none swap sw 0 0' >> /etc/fstab
  - ufw allow OpenSSH && ufw --force enable
  - su - dev -c 'curl --proto =https -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none'
  - su - dev -c 'curl -fsSL https://claude.ai/install.sh | bash'
```

Validated with `cloud-init schema` 26.2 (the version Ubuntu 24.04 ships).
Before pasting, `LC_ALL=C grep -n '[^ -~]' user-data.yaml` must print nothing.

- `dev` has **no sudo** on purpose: the agent runs as `dev`; the machine is
  administered as `root`.
- rustup installs no toolchain; the first `cargo` in the checkout installs the
  pinned one from `rust-toolchain.toml`.
- `package_upgrade` makes the first boot take several minutes; until
  `cloud-init status` says `done`, the swap and the user may not exist yet.

Check after boot (as root): `cloud-init status --long` must list no warnings,
`swapon --show` the swap file, `id dev` the user. If the user data never
applied, every `runcmd` line can be run by hand as root, plus the user:
`adduser --disabled-password --gecos "" dev`, then
`install -d -m 700 -o dev -g dev /home/dev/.ssh` and
`install -m 600 -o dev -g dev /root/.ssh/authorized_keys /home/dev/.ssh/`.

### 5.3 The work directory on the volume (root)

DigitalOcean mounts the volume under `/mnt/`, at a name derived from the
volume's (`/mnt/mindfork_dev_storage` on this machine), so the path is looked
up, not guessed, and the script stops on anything unexpected:

```bash
bash -eu <<'EOF'
V=$(findmnt -rn -o TARGET | grep '^/mnt/' || true)
[ -n "$V" ] && [ "$(printf '%s\n' "$V" | wc -l)" -eq 1 ] || { echo "expected one volume under /mnt, got: '$V'"; exit 1; }
[ ! -e /home/dev/work ] && [ ! -L /home/dev/work ] || { echo "/home/dev/work already exists"; exit 1; }
mkdir -p "$V/work" && chown dev:dev "$V/work"
ln -s "$V/work" /home/dev/work && chown -h dev:dev /home/dev/work
readlink -f /home/dev/work
EOF
```

### 5.4 GitHub access, the clone, the first build (`dev`)

A **deploy key** reaches this one repository and nothing else in the account:

```bash
ssh-keygen -t ed25519 -C mindfork-dev -N "" -f ~/.ssh/id_ed25519
cat ~/.ssh/id_ed25519.pub     # → repository Settings → Deploy keys → Add, "Allow write access"
git config --global user.name "..." && git config --global user.email "..."
cd ~/work && git clone git@github.com:vshylov/mindfork-rs.git
cd mindfork-rs && cargo test --no-run
python3 -m venv ~/.venv
~/.venv/bin/pip install --require-hashes -r tools/media-requirements.txt
~/.venv/bin/pip install requests      # only for the two llama-server probes
```

### 5.5 Claude login and the first Remote Control run (`dev`, over `ssh -t`)

```bash
unset ANTHROPIC_API_KEY
claude auth login                           # press c, open the URL elsewhere, paste the code
cd ~/work/mindfork-rs && claude remote-control --name do-mindfork
```

Answer `y` to both questions, see `do-mindfork` appear in the Claude app, then
Ctrl+C.

### 5.6 The providers' keys (`dev`)

```bash
mkdir -p ~/.config/mindfork && install -m 600 /dev/null ~/.config/mindfork/keys.env
# lines like MINDFORK_OPENAI_KEY=...; never ANTHROPIC_API_KEY (it would replace the subscription)
```

The agent can read these keys: give it keys of their own, with a spending limit.

### 5.7 The service (root)

`/etc/systemd/system/claude-rc.service`:

```ini
[Unit]
Description=Claude Code Remote Control (mindfork-rs)
After=network-online.target
Wants=network-online.target

[Service]
User=dev
WorkingDirectory=/home/dev/work/mindfork-rs
EnvironmentFile=/home/dev/.config/mindfork/keys.env
Environment=PATH=/home/dev/.venv/bin:/home/dev/.cargo/bin:/home/dev/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/home/dev/.local/bin/claude remote-control --name do-mindfork
Restart=always
RestartSec=30

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload && systemctl enable --now claude-rc
journalctl -u claude-rc -f
```

`PATH` is spelled out because a systemd service reads no `~/.profile`: without
it the agent finds no `cargo`. The virtual environment comes first, so
`python` and `python3` are the one with Pillow and fontTools. `Restart=always` covers the server's own exit
after a network outage (§2).

### 5.8 Verify from the phone

In the `do-mindfork` session ask the agent to run:

- `env | grep -c MINDFORK_` — the number of keys it sees, values not printed;
- `cargo test` — 4802 passed is the figure of §3.

### 5.9 llama.cpp and local models (optional)

The agent sometimes needs a local `llama-server`: often the **embedder**
(`bge-m3-q8_0.gguf`, for the RAG and attachment-search smokes), rarely a **chat
model**. On this machine a CPU build of llama.cpp lives in `~/work/llama.cpp`
and the GGUF files in `~/work/GGUF`, both owned by `dev` and on the volume.
Check where the build put the binary — `ls ~/work/llama.cpp/build/bin/llama-server`
— and adjust the paths below if it differs.

**The embedder runs as a service**, so the agent never has to start it.
`/etc/systemd/system/llama-embed.service` (root), with the arguments of the
project's CPU stack (`docker/compose.yaml`): an embedding server with the
default `-ub` silently rejects any chunk larger than 512 tokens.

```ini
[Unit]
Description=llama-server embeddings (bge-m3)
After=network.target

[Service]
User=dev
ExecStart=/home/dev/work/llama.cpp/build/bin/llama-server -m /home/dev/work/GGUF/bge-m3-q8_0.gguf --embeddings --host 127.0.0.1 --port 8001 -c 8192 -ub 8192 -b 8192
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Two lines join `keys.env` (§5.6):

```
MINDFORK_EMBED_URL=http://127.0.0.1:8001/v1
MINDFORK_LLAMA_BIN=/home/dev/work/llama.cpp/build/bin/llama-server
```

```bash
systemctl enable --now llama-embed && systemctl restart claude-rc
curl -s 127.0.0.1:8001/health        # {"status":"ok"}
```

Both variables are safe in every session: `cargo test` with them set (an
unreachable URL and a missing binary) still gave 4802 passed, 322 ignored. Do
**not** put `MINDFORK_MODEL` there — naming a model switches the managed mode
on, even over settings that name a cloud (install.md, "Quick start via
environment variables").

**Claude Code's own boundary.** The session's directory is
`~/work/mindfork-rs`; reading files outside it can raise a permission prompt,
forwarded to the phone. `~/.claude/settings.json` (`dev`; merge into an existing
file):

```json
{ "permissions": { "additionalDirectories": ["/home/dev/work/llama.cpp", "/home/dev/work/GGUF"] } }
```

**A note for the agent about the machine** — `~/.claude/CLAUDE.md` (`dev`), read
by every session on this machine and never committed:

```markdown
# This machine: mindfork-dev droplet
- CPU only: 4 vCPU, 8 GB RAM + 8 GB swap.
- llama-server: ~/work/llama.cpp/build/bin/llama-server
- Models: ~/work/GGUF/ - bge-m3-q8_0.gguf, google_gemma-4-E2B-it-Q8_0.gguf (+ its mmproj)
- Embedder is always on: http://127.0.0.1:8001/v1 (service llama-embed); MINDFORK_EMBED_URL is set.
- Always start llama-server with --host 127.0.0.1.
- A chat model only when needed: start llama-server on port 8000 with -c 8192, set
  MINDFORK_ENGINE_URL for that command only, stop the server afterwards.
  Never alongside a cargo build: Gemma 4 E2B Q8 + mmproj takes ~6 GB, a test build peaks at ~6 GB.
```

**Why the chat model is not a service.** Gemma 4 E2B at Q8_0 is 4.7 GB plus a
0.96 GB projector; beside a test build's ~6 GB peak (§3) that is twice the
machine's memory — swap at best, the OOM killer at worst.

**Exposure.** `--host 127.0.0.1` keeps a server off the network altogether,
and `ufw` drops inbound traffic to anything but SSH even when a server binds
`0.0.0.0` — as the example in CLAUDE.md §Commands does, hence the note's line.
Check: `ss -tlnp | grep 8001` shows `127.0.0.1:8001`, and
`curl -m 5 http://<droplet IP>:8001/health` from outside times out. One
exception: ports published by **Docker** bypass `ufw` — should `docker compose`
from `docker/` ever run here, its `8000`/`8001`/`8888` are open to anyone; a
DigitalOcean cloud firewall (inbound 22/TCP only) sits in front of the droplet
and is not bypassed.

## 6. What the first setup hit (2026-11-05)

1. **Non-ASCII in the user data skipped all of it.** A Cyrillic comment next to
   the SSH key reached cloud-init decoded as Latin-1: the letter sha (U+0448,
   UTF-8 `D1 88`) became the control character U+0088, the YAML did not load
   (`unacceptable character #x0088 ... position 270`) and `cloud-init status`
   still said `done`, only `degraded`. Nothing ran: no packages, no user, no
   swap. Fix: ASCII only, checked before pasting (§5.2).
2. **The volume is not at `/mnt/volume_*`.** A guessed glob matched nothing, the
   variable came out empty, and `ln -s "$V/work" ~/work` made `~/work` a link to
   `/work` **on the boot disk** — the clone and its build landed there. Fix: the
   path looked up, the script stopping on anything unexpected (§5.3).
3. **`ln -s` into an existing directory, or a link to one, creates the link
   inside it** — the repair attempt produced `~/work/work`, and `rmdir` refused
   `~/work` because it was a link, not a directory. The repair that worked:
   stop the service, `mv /work/mindfork-rs` onto the volume, `rm /home/dev/work`
   (no trailing slash — the link only), `rmdir /work`, link again, start. The
   path `/home/dev/work/mindfork-rs` never changed, so the unit, the folder
   trust and the session history stayed valid.
4. **Midnight Commander's panel footer shows free space, not used**
   (`31G / 47G (66%)` is 31 GB free). `df -h` is the reading to trust.
5. **A service has no login `PATH`** (§5.7).
6. **The Python packages were forgotten.** The first recipe installed none of
   them (caught by review, before a media tool was run; without them each one
   stops at its import, `Pillow is required`). Fix: the venv of §5.4 and its
   place at the head of the service's `PATH`.
7. **A glyph with no font on Linux.** `tools/screenshots.py`'s fallbacks had
   Segoe UI Symbol for `⌕` on Windows and nothing that holds it on Linux. Found
   by the agent on the droplet and fixed in a pull request of its own:
   `DejaVuSansMono.ttf` (package `fonts-dejavu-mono`, a dependency of
   `fonts-dejavu-core`) joined the fallbacks, and all twenty dumps render.

## 7. Keeping it up for weeks

- **The login.** When the three-day warning comes: `claude auth login` as `dev`
  over SSH, then `systemctl restart claude-rc`. The only routine manual step.
- **Disk.** `df -h /mnt/<volume>` weekly; `cargo clean` in a checkout frees its
  `target/`. `claude remote-control --spawn worktree` gives every session a
  worktree, and each builds a `target/` of its own (4–10 GB).
- **Data.** Snapshot the volume once the machine is set up, and again at
  milestones; the droplet's backups do not include it.
- **Updates.** `unattended-upgrades` with automatic reboot off; after a manual
  reboot the service starts by itself.
- **Exposure.** Inbound SSH only (`ufw`, and a DigitalOcean cloud firewall as a
  second layer that Docker cannot bypass); Remote Control needs no inbound
  port; local servers bind `127.0.0.1` (§5.9).

## 8. Verified, and not yet

Verified on the droplet (2026-11-05 and -06): the `do-mindfork` session appears in the
Claude app and sees the providers' keys; the media tools run from the venv; the
embedder service answers `/health` with `ok`, and its port times out from
outside; the agent finds llama.cpp and the models from its note (§5.9).

Not yet:

- How long a login lasts before the warning — not documented, not yet observed.
- Behaviour across a droplet reboot and a long network outage — expected from
  `Restart=always`, not yet exercised.
- `cargo test` on the droplet itself — §5.8's figure is from the cloud
  container; the shared vCPUs may make it slower.
