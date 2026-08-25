# syntax=docker/dockerfile:1.7
#
# JupyterLab 4 in the browser + the `mindfork` binary built from this working tree.
# Design: docs/research/docker-jupyter-env.md. Build context is the repository root
# (see docker/compose.yaml and /.dockerignore).

# The toolchain is pinned in rust-toolchain.toml, and this must not drift from it:
# a floating `rust:latest` would break `clippy -D warnings` the moment stable moves.
ARG RUST_VERSION=1.96.0
# Ubuntu 24.04 / Python 3.13 / JupyterLab 4.6.3. Older glibc in the build stage
# (bookworm, 2.36) running on newer here (2.39) is the direction that works.
ARG NB_IMAGE=quay.io/jupyter/minimal-notebook:lab-4.6.3

# ── Stage 1: the binary ───────────────────────────────────────────────────────
FROM rust:${RUST_VERSION}-bookworm AS build

# The only system build dependency this crate has. `rodio`/`cpal` link against
# libasound for message speech; everything else is vendored (rusqlite `bundled`,
# reqwest on rustls, an x11rb clipboard).
RUN apt-get update \
    && apt-get install -y --no-install-recommends libasound2-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# Cache mounts, not layers: a cache mount is not part of the image, so the binary
# has to be copied out inside the same RUN. This is what makes the second build
# after a code change incremental instead of a full recompile.
RUN --mount=type=cache,id=mindfork-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=mindfork-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=mindfork-target,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && mkdir -p /out \
    && cp target/release/mindfork /out/mindfork

# ── Stage 2: JupyterLab ───────────────────────────────────────────────────────
FROM ${NB_IMAGE}

USER root

# Runtime ALSA. It must be `libasound2t64`, not `libasound2`: after the time_t64
# transition the old name is only virtual on Ubuntu 24.04 and apt satisfies it
# with an OSS shim that lacks `snd_device_name_get_hint` — the binary then dies at
# startup. The project already paid for this once (packaging/nfpm.yaml).
RUN apt-get update \
    && apt-get install -y --no-install-recommends libasound2t64 \
    && rm -rf /var/lib/apt/lists/*

# `shared::secrets` derives its Linux encryption key from /etc/machine-id, and the
# base image ships that file empty — without this an API key typed into the
# settings screen cannot be saved at all. Generated per build rather than baked as
# a constant, so the key material is not published in this repository; the cost is
# that rebuilding the image invalidates keys already stored in the data volume.
# The durable route for a cloud key here is the env variable (docs/install.md §3.1),
# which is what docker/compose.yaml passes through.
RUN python3 -c "import uuid; print(uuid.uuid4().hex)" > /etc/machine-id

# The deb layout (packaging/nfpm.yaml): the real binary in a private directory next
# to `defaults.json` and `data/dictionaries/`, a symlink from PATH. `current_exe()`
# resolves the symlink through /proc/self/exe, so the app finds its neighbours —
# which is how spellcheck keeps working under a non-portable data root (the P1
# fallback, docs/install.md §2.1).
RUN mkdir -p /opt/mindfork/data/dictionaries
COPY --from=build /out/mindfork /opt/mindfork/mindfork
COPY dictionaries/ /opt/mindfork/data/dictionaries/
RUN ln -sf /opt/mindfork/mindfork /usr/local/bin/mindfork \
    && chmod 0755 /opt/mindfork/mindfork \
    # Owned by the notebook user so the start-up hook can write `defaults.json`
    # next to the binary from the environment, without a root container.
    && chown -R ${NB_UID}:${NB_GID} /opt/mindfork

# Run by the base image's `start.sh` before the server launches, as uid 1000.
COPY docker/lab/before-notebook.d/10-mindfork /usr/local/bin/before-notebook.d/
COPY docker/lab/settings.seed.json /opt/mindfork/settings.seed.json
COPY docker/lab/jupyter_server_config.py /etc/jupyter/jupyter_server_config.py
# Suppresses the "get notified about official Jupyter news?" prompt that
# otherwise greets every first load — a question about a public website, asked
# by a throwaway container that exists to test something else.
COPY docker/lab/overrides.json ${CONDA_DIR}/share/jupyter/lab/settings/overrides.json
COPY docker/lab/profile.d/mindfork.sh /etc/profile.d/mindfork.sh
RUN chmod 0755 /usr/local/bin/before-notebook.d/10-mindfork \
    && chmod 0644 /etc/profile.d/mindfork.sh /opt/mindfork/settings.seed.json \
    # The COPY above created this directory root-owned, and the base image's
    # `fix-permissions` later runs as the notebook user — it cannot chmod what it
    # does not own, and fails the build rather than warning.
    && chown -R ${NB_UID}:${NB_GID} ${CONDA_DIR}/share/jupyter/lab/settings

# Both mount points have to exist *and be owned by the notebook user* before the
# volumes land on them: docker seeds a fresh named volume from the image's
# directory, and a mount point that only appears at run time is created root-owned
# — after which uid 1000 cannot write its own data root.
RUN mkdir -p /home/${NB_USER}/mindfork/data /home/${NB_USER}/work \
    && chown -R ${NB_UID}:${NB_GID} /home/${NB_USER}/mindfork /home/${NB_USER}/work

USER ${NB_UID}

# The RAM/CPU readout in the status bar. `jupyterlab-system-monitor` — the package
# the JupyterLab 3 era used for this — pins `jupyterlab ~=3.0` and cannot be
# installed here; `jupyter-resource-usage` is the JupyterLab 4 equivalent.
RUN pip install --no-cache-dir jupyter-resource-usage==1.3.0 \
    && fix-permissions "${CONDA_DIR}" \
    && fix-permissions "/home/${NB_USER}"

WORKDIR /home/${NB_USER}
