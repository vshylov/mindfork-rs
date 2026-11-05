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
packages: [build-essential, pkg-config, libasound2-dev, git, python3, tmux, unattended-upgrades]
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
Environment=PATH=/home/dev/.cargo/bin:/home/dev/.local/bin:/usr/local/bin:/usr/bin:/bin
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
it the agent finds no `cargo`. `Restart=always` covers the server's own exit
after a network outage (§2).

### 5.8 Verify from the phone

In the `do-mindfork` session ask the agent to run:

- `env | grep -c MINDFORK_` — the number of keys it sees, values not printed;
- `cargo test` — 4802 passed is the figure of §3.

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
- **Exposure.** Inbound SSH only (`ufw`, or a DigitalOcean cloud firewall);
  Remote Control needs no inbound port.

## 8. Not verified

- How long a login lasts before the warning — not documented, not yet observed.
- Behaviour across a droplet reboot and a long network outage — expected from
  `Restart=always`, not yet exercised.
- `cargo test` on the droplet itself — §5.8's figure is from the cloud
  container; the shared vCPUs may make it slower.
