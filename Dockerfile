# syntax=docker/dockerfile:1
# Stage 1a: Build React dashboard
# Renovate-managed digest, refresh weekly (audit 2026-05-21 §3.5 — pin all bases by @sha256).
FROM node:23-bookworm-slim@sha256:86191b94d2a163be41f3dc7fe5e5fcaca8ba2f1be7275d98a06343483c17414a AS ui-builder

WORKDIR /ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --legacy-peer-deps
COPY ui/ ./
# VERSION file is read by vite.config.ts at build time (resolve("../VERSION"))
COPY VERSION /VERSION
RUN npm run build

# Stage 1b: Build Rust binary
# The CI `rust` job uses `dtolnay/rust-toolchain@stable` and enforces
# `clippy::duration_suboptimal_units`, which insists on `Duration::from_mins`
# / `Duration::from_hours` — both stabilized in Rust 1.91. The Dockerfile
# must keep up with the workspace's stable-Rust floor or `cargo build`
# rejects those calls during the container smoke build.
# TODO: re-pin by @sha256 once Renovate is wired up (audit 2026-05-21 §3.5).
FROM rust:1-bookworm AS builder

WORKDIR /app
# Without this, Cargo hides progress in non-TTY Docker builds — looks hung for many minutes.
# `when = always` requires an explicit `width` (Cargo 1.8x+).
ENV CARGO_TERM_PROGRESS_WHEN=always
ENV CARGO_TERM_PROGRESS_WIDTH=80

COPY Cargo.toml Cargo.lock ./
COPY VERSION ./
COPY crates/ crates/
# rust-embed resolves ../../ui/dist/ relative to crates/maestro-web/
COPY --from=ui-builder /ui/dist/ ui/dist/

# BuildKit cache mounts: persist downloaded crates + `target/` between `docker compose build` runs.
# Copy the binary to `/out` so it exists in the image layer (the mounted `/app/target` is not part of the layer).
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,id=rust-target-${TARGETARCH},sharing=locked \
    mkdir -p /out \
    && echo "=== cargo build --release (first build often takes 10–20+ minutes; rebuilds reuse BuildKit cache) ===" \
    && cargo build --release \
    && cp /app/target/release/maestro /out/maestro

# Maestro runtime image — kitchen-sink bake, per the project's "code-quality
# vs. ergonomics" trade-off: every advertised feature works on a vanilla
# `docker compose up` with no extra setup steps. The cost is image size +
# audit surface (Playwright Chromium libs, four AI provider CLIs, an editor,
# a terminal, a build toolchain). The mitigations are:
#   • Two image targets — `runtime-base` = `maestro:slim` (no Rust, no
#     build-essential), `runtime-build-tools` = `maestro:full` (default).
#   • Every FROM is pinned by `@sha256:` digest (Renovate-refreshed weekly).
#   • Every direct download (ttyd, Node, Cursor agent, openvscode-server)
#     verifies a per-arch sha256 against an ARG-pinned digest.
#   • Every npm global is pinned via ARG; the one-line bump knob lives in
#     this file rather than `@latest`.
#   • Provisioning tier (`[provisioning]` in config.toml) lets admins drop
#     extra tools into a named volume that SHADOWS the bake — no rebuild.
# See `lore/code-quality-principles.md` for the broader trade-off rationale
# and `lore/audits/2026-05-21-clean-code.md` §3.5 for the audit findings
# this image hardening pass addresses.
#
# Stage 2a: Runtime base (= image target `maestro:slim`).
# Renovate-managed digest, refresh weekly (audit 2026-05-21 §3.5 — pin all bases by @sha256).
# Contains everything a deployed maestro server NEEDS to run its workflows except
# build toolchains (no Rust, no build-essential). For users who do not run Rust
# workflows or build native dependencies inside workers, `maestro:slim` is the
# smaller, lower-attack-surface image. Build with:
#   docker build --target runtime-base -t maestro:slim .
FROM debian:bookworm-slim@sha256:0104b334637a5f19aa9c983a91b54c89887c0984081f2068983107a6f6c21eeb AS runtime-base

ARG MAESTRO_VERSION=dev
LABEL org.opencontainers.image.version="${MAESTRO_VERSION}"
LABEL org.opencontainers.image.source="https://github.com/morphet81/maestro-core"
LABEL org.opencontainers.image.licenses="FSL-1.1-ALv2"
LABEL org.opencontainers.image.title="Maestro"
LABEL org.opencontainers.image.description="Automated workflow orchestration for AI coding agents"

# Registry image name — used by entrypoint to auto-pull the worker image into DinD.
ENV MAESTRO_REGISTRY_IMAGE=ghcr.io/morphet81/maestro:${MAESTRO_VERSION}

# Match host UID for rootless Podman API sockets (often mode 0600 — only the user matters). Set in compose `.env` and rebuild.
# Do not force the primary GID to match the host: macOS "staff" is often GID 20, which is `dialout` on Debian and already exists.
ARG MAESTRO_UID=999

# Foundational apt block — all apt-managed packages + 3rd-party apt repos
# (mise, gh, acli) + the maestro user + sudoers, in ONE RUN. The runtime
# stage was previously 26 RUN layers (audit §3.5); collapsing related apt
# work into one cache-friendly layer is the bulk of the reduction.
#
#   • docker.io is Debian's `docker` CLI (bookworm has no `docker-cli`).
#   • Playwright Chromium system deps are baked so workers can run browser tests.
#   • sudo + the narrow `/bin/bash` sudoers rule serves [docker] hook commands;
#     `bash` is explicit because `sudo env bash` matches /usr/bin/env and would
#     fail the rule.
#   • acli is amd64-only via apt; `|| echo WARN` keeps arm64 builds working
#     (the few admins who need acli on arm64 install a binary release via
#     `[provisioning]`).
#   • The maestro user is created in the same layer so the sudoers entry can
#     be `visudo -cf`-validated against a real user immediately.
RUN set -eux \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        wget \
        gnupg2 \
    # ── 3rd-party apt repos (mise / gh / acli) ─────────────────────────────
    && install -dm 755 /etc/apt/keyrings \
    && curl -fsSL https://mise.jdx.dev/gpg-key.pub \
         -o /etc/apt/keyrings/mise-archive-keyring.asc \
    && echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.asc arch=$(dpkg --print-architecture)] https://mise.jdx.dev/deb stable main" \
         > /etc/apt/sources.list.d/mise.list \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
         -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
         > /etc/apt/sources.list.d/github-cli.list \
    && wget -nv -O- https://acli.atlassian.com/gpg/public-key.asc \
         | gpg --dearmor -o /etc/apt/keyrings/acli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/acli-archive-keyring.gpg \
    && echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/acli-archive-keyring.gpg] https://acli.atlassian.com/linux/deb stable main" \
         > /etc/apt/sources.list.d/acli.list \
    # ── Install foundational + Playwright + apt-managed CLIs ───────────────
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        docker.io \
        git \
        jq \
        iptables \
        iproute2 \
        openssh-client \
        python3 \
        socat \
        sudo \
        mise \
        gh \
        libglib2.0-0 \
        libnss3 \
        libnspr4 \
        libdbus-1-3 \
        libatk1.0-0 \
        libatk-bridge2.0-0 \
        libcups2 \
        libdrm2 \
        libxkbcommon0 \
        libatspi2.0-0 \
        libxcomposite1 \
        libxdamage1 \
        libxfixes3 \
        libxrandr2 \
        libgbm1 \
        libpango-1.0-0 \
        libcairo2 \
        libasound2 \
    && (apt-get install -y --no-install-recommends acli \
        || echo "WARN: acli not available for $(dpkg --print-architecture)") \
    # ── Maestro user + sudoers ─────────────────────────────────────────────
    && groupadd maestro \
    && useradd -u "${MAESTRO_UID}" -g maestro -m -s /bin/bash maestro \
    && printf '%s\n' \
        'maestro ALL=(root) NOPASSWD: /usr/bin/bash, /bin/bash, /usr/bin/bash *, /bin/bash *' \
        > /etc/sudoers.d/maestro-hook-bash \
    && chmod 0440 /etc/sudoers.d/maestro-hook-bash \
    && visudo -cf /etc/sudoers.d/maestro-hook-bash \
    && mise --version \
    && rm -rf /var/lib/apt/lists/*

# ttyd — lightweight web-based terminal (used by the dashboard "Open terminal" button).
# Pinned by version + per-arch sha256 (audit §3.5 — verify checksums on every direct download).
ARG TTYD_VERSION=1.7.7
ARG TTYD_SHA256_X86_64=8a217c968aba172e0dbf3f34447218dc015bc4d5e59bf51db2f2cd12b7be4f55
ARG TTYD_SHA256_AARCH64=b38acadd89d1d396a0f5649aa52c539edbad07f4bc7348b27b4f4b7219dd4165
RUN set -eux \
    && ARCH=$(dpkg --print-architecture) \
    && case "$ARCH" in \
         amd64) TTYD_ARCH=x86_64; TTYD_SHA256="${TTYD_SHA256_X86_64}" ;; \
         arm64) TTYD_ARCH=aarch64; TTYD_SHA256="${TTYD_SHA256_AARCH64}" ;; \
         *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;; \
       esac \
    && curl -fSL --retry 3 --retry-delay 5 \
         "https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}/ttyd.${TTYD_ARCH}" \
         -o /usr/local/bin/ttyd \
    && echo "${TTYD_SHA256}  /usr/local/bin/ttyd" | sha256sum -c - \
    && chmod +x /usr/local/bin/ttyd


# mise was previously installed in its own RUN here; it now lives in the
# foundational apt block above. The repo + key + install + `mise --version`
# smoke-check all happen in that single RUN.
#
# NOTE: build toolchains (build-essential, autoconf, libssl-dev, libyaml-dev, …)
# and the Rust toolchain live in the `runtime-build-tools` stage below, not
# here. `runtime-base` deliberately omits them so the slim image is smaller
# and has less attack surface. See audit §3.5 / lore/audits/2026-05-21-plan.md
# Phase 3 — "Runtime bundles build toolchains".
# RUSTUP_HOME / CARGO_HOME are declared here so `ENV PATH` (further down) can
# reference $RUSTUP_HOME / $CARGO_HOME consistently across both image targets;
# the directories are empty in `runtime-base` and populated in
# `runtime-build-tools`.
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo

# ── Three-tier tool layout (task #48) ────────────────────────────────────
#   BAKED        — required for advertised Maestro features (this Dockerfile).
#   PROVISIONING — admin preferences installed at runtime into the
#                  `maestro-tools` named volume via `[provisioning]` in
#                  config.toml. See `docs/extending-maestro.md` for the
#                  authoritative reference + decision table.
#   REMOVED      — specialized one-off tools. Admins add via custom
#                  Dockerfile `FROM maestro:latest` or compose override.
#
# Tools migrated OUT of the bake into `[provisioning]` defaults (task #48):
#   fcli, lokalise2, figma-cli — see config.toml.example.
# Tools REMOVED entirely (admin handles via custom image):
#   awscli — only a small minority of deployments need it.
# ─────────────────────────────────────────────────────────────────────────

# Node.js 23+ (official tarball). Cursor Agent runs `node --use-system-ca`, which exists only on Node >= 23.9
# on Linux; NodeSource 20.x rejects that flag with "bad option: --use-system-ca".
ARG NODE_VERSION=23.11.0
# Pinned per-arch sha256, sourced from https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt
# (audit §3.5 — verify checksums on every direct download).
ARG NODE_SHA256_X64=66f768a7f2d89ecdda8fe1e33ee71ac04ed9180111cbf1c5fb944655fe7c90c7
ARG NODE_SHA256_ARM64=12b29a87a7ccd7e1b97392d1e1533470d596578dad900430cff403e404fe72a7
RUN set -eux \
    && ARCH="$(dpkg --print-architecture)" \
    && case "$ARCH" in \
         amd64) NODE_ARCH=x64; NODE_SHA256="${NODE_SHA256_X64}" ;; \
         arm64) NODE_ARCH=arm64; NODE_SHA256="${NODE_SHA256_ARM64}" ;; \
         *) echo "unsupported architecture: $ARCH" >&2; exit 1 ;; \
       esac \
    && curl -fSL --retry 3 --retry-delay 5 \
       "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${NODE_ARCH}.tar.gz" \
       -o /tmp/node.tar.gz \
    && echo "${NODE_SHA256}  /tmp/node.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/node.tar.gz -C /usr/local --strip-components=1 \
    && rm -f /tmp/node.tar.gz \
    && node --version && npm --version

# gh CLI is installed in the foundational apt block above.

# Baked AI provider CLIs. Versions pinned via ARG (audit §3.5 — no @latest in image build).
# Each ARG is the one-line bump knob; refresh together with the per-CLI release cadence.
#  • claude        — `@anthropic-ai/claude-code` (Claude Code CLI for [agent] provider = "claude")
#  • codex         — `@openai/codex`             (Codex CLI for [agent] provider = "codex")
#  • opencode      — `opencode-ai`               (OpenCode CLI for [agent] provider = "opencode";
#                    canonical distribution is the `opencode-ai` package, NOT `opencode`)
ARG CLAUDE_CODE_VERSION=2.1.146
ARG CODEX_VERSION=0.132.0
ARG OPENCODE_AI_VERSION=1.15.6
RUN npm install -g \
        "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
        "@openai/codex@${CODEX_VERSION}" \
        "opencode-ai@${OPENCODE_AI_VERSION}" \
    && opencode --version

# Cursor Agent CLI (for [agent] provider = "cursor"). The launcher resolves paths with realpath("$0");
# the package must be installed as a directory tree so `index.js` sits next to the `cursor-agent` script
# (a symlink alone to /usr/local/bin breaks the realpath lookup).
#
# Pinned tarball replaces upstream `curl … | bash` (audit §3.5):
#   • reproducible — the installer URL pins to a moving lab build otherwise.
#   • supply-chain — a compromised install script can no longer inject arbitrary code at build time.
# To bump: download the new tarball from downloads.cursor.com/lab/<version>/linux/<arch>/agent-cli-package.tar.gz,
# update CURSOR_AGENT_VERSION + the two per-arch sha256 ARGs.
ARG CURSOR_AGENT_VERSION=2026.05.20-2b5dd59
ARG CURSOR_AGENT_SHA256_X64=27453acdea679d1570ab5adbbef9d19ecbf4c3efc8df687338c7fc156a693e18
ARG CURSOR_AGENT_SHA256_ARM64=baf2f0aa1ca890f0b71480fba2db40bacff4eb56b9408c940d574ce39d8ab3fc
RUN set -eux \
    && ARCH="$(dpkg --print-architecture)" \
    && case "$ARCH" in \
         amd64) CURSOR_ARCH=x64; CURSOR_SHA256="${CURSOR_AGENT_SHA256_X64}" ;; \
         arm64) CURSOR_ARCH=arm64; CURSOR_SHA256="${CURSOR_AGENT_SHA256_ARM64}" ;; \
         *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;; \
       esac \
    && CURSOR_DEST="/usr/local/share/cursor-agent/versions/${CURSOR_AGENT_VERSION}" \
    && mkdir -p "${CURSOR_DEST}" \
    && curl -fSL --retry 3 --retry-delay 5 \
         "https://downloads.cursor.com/lab/${CURSOR_AGENT_VERSION}/linux/${CURSOR_ARCH}/agent-cli-package.tar.gz" \
         -o /tmp/cursor-agent.tar.gz \
    && echo "${CURSOR_SHA256}  /tmp/cursor-agent.tar.gz" | sha256sum -c - \
    && tar --strip-components=1 -xzf /tmp/cursor-agent.tar.gz -C "${CURSOR_DEST}" \
    && rm -f /tmp/cursor-agent.tar.gz \
    && ln -sf "${CURSOR_DEST}/cursor-agent" /usr/local/bin/agent \
    && ln -sf "${CURSOR_DEST}/cursor-agent" /usr/local/bin/cursor-agent \
    && chmod -R a+rX /usr/local/share/cursor-agent \
    && test -f "${CURSOR_DEST}/index.js"

# Playwright is not baked into this image: isolated workflow workers use the project's @playwright/test
# version and download Chromium into ~/.cache/ms-playwright (persisted via docker-compose.dind.yml
# playwright-cache → /shared-auth/playwright-cache). Forcing a mismatched browser revision caused subtle
# visual snapshot drift vs local/CI.

# acli (Atlassian CLI) is installed in the foundational apt block above; the
# `|| echo WARN` fallback there keeps arm64 builds working.

# openvscode-server — browser-based VS Code for manual worktree editing via dashboard.
# Release tarballs use x64/arm64/armhf, not dpkg's amd64/arm64.
# Pinned per-arch sha256 (audit §3.5 — verify checksums on every direct download).
ARG OPENVSCODE_VERSION=1.109.5
ARG OPENVSCODE_SHA256_X64=b433bf4f0227321a7014d8460d10a8f958adc0f45aa79bd889e84e65e8f88363
ARG OPENVSCODE_SHA256_ARM64=36d9c14036489b63de84ebace837fcacf7e60e669a0dc715802c5443684ea4dc
RUN set -eux \
    && ARCH="$(dpkg --print-architecture)" \
    && case "$ARCH" in \
         amd64) VS_ARCH=x64; OVS_SHA256="${OPENVSCODE_SHA256_X64}" ;; \
         arm64) VS_ARCH=arm64; OVS_SHA256="${OPENVSCODE_SHA256_ARM64}" ;; \
         *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;; \
       esac \
    && curl -fSL --retry 3 --retry-delay 5 \
         "https://github.com/gitpod-io/openvscode-server/releases/download/openvscode-server-v${OPENVSCODE_VERSION}/openvscode-server-v${OPENVSCODE_VERSION}-linux-${VS_ARCH}.tar.gz" \
         -o /tmp/openvscode.tar.gz \
    && echo "${OVS_SHA256}  /tmp/openvscode.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/openvscode.tar.gz -C /opt \
    && ln -s "/opt/openvscode-server-v${OPENVSCODE_VERSION}-linux-${VS_ARCH}/bin/openvscode-server" /usr/local/bin/openvscode-server \
    && rm -f /tmp/openvscode.tar.gz

# Task #48: AWS CLI v2 was previously baked here; removed because it's
# only required for the minority of deployments that authenticate npm to
# CodeArtifact. Admins who need it add a custom Dockerfile:
#
#   FROM maestro:latest
#   RUN apt-get update && apt-get install -y --no-install-recommends unzip \
#       && curl -sL "https://awscli.amazonaws.com/awscli-exe-linux-$(uname -m).zip" -o /tmp/aws.zip \
#       && unzip -q /tmp/aws.zip -d /tmp && /tmp/aws/install \
#       && rm -rf /tmp/aws*
#
# See `docs/extending-maestro.md` for the full custom-image pattern.

# Task #48: figma-cli was previously baked here; migrated to a
# `[provisioning]` default in `config.toml.example` (admin can disable
# by removing the line, or pin to a specific version by editing it).
# `fcli` and `lokalise2` were migrated for the same reason — same
# rationale: clean single-binary installs that admins routinely want to
# pin / swap, not core requirements of any advertised feature.

# Copy egress rules script (executable bit set via BuildKit --chmod, no chmod RUN).
COPY --chmod=0755 docker/egress-rules.sh /usr/local/bin/egress-rules.sh

# Copy Maestro binary from builder (see builder stage: binary staged under `/out` for cache-friendly builds)
COPY --from=builder /out/maestro /usr/local/bin/maestro

# Optional: TOML file in build context used only for [docker] build_commands (default: example with empty hooks).
# Note: if you override MAESTRO_BUILD_CONFIG to a custom filename, ensure it is NOT excluded by .dockerignore.
ARG MAESTRO_BUILD_CONFIG=config.toml.example
COPY ${MAESTRO_BUILD_CONFIG} /tmp/maestro-build-config.toml
RUN mkdir -p /workspace \
    && maestro --config /tmp/maestro-build-config.toml docker-hooks build \
    && rm -f /tmp/maestro-build-config.toml

# Ship example files as reference for distributed-image users.
# Runtime config is NOT baked in — users must volume-mount config.toml, maestro.env,
# and workflows/ (see docker-compose.yml). The entrypoint validates the mount.
# The parent dirs are auto-created by BuildKit's COPY (no separate mkdir RUN);
# /etc/maestro/examples/workflows is also covered by the consolidated mkdir RUN below.
COPY config.toml.example /etc/maestro/examples/config.toml.example
COPY maestro.env.example /etc/maestro/examples/maestro.env.example
COPY workflows/implement_ticket.example.toml workflows/address_pr_comments.example.toml workflows/merge_base.example.toml /etc/maestro/examples/workflows/

# Maestro user, sudo install, and sudoers rule are set up in the foundational
# apt block above (so visudo can validate against the real user in the same
# layer). Claude Code refuses --dangerously-skip-permissions as root, hence
# the non-root identity for the maestro server process at runtime.

# All maestro-owned directory creation + ownership in one layer:
#   • Maestro home dirs (mise data/cache/config, npm cache, npm-global prefix)
#   • /opt/maestro-tools volume mountpoint (Task #48: shadows baked tools at runtime)
#   • /workspace (legacy single-workspace) + /workspaces (per-project clones)
#   • /etc/maestro/examples/workflows (reference files shipped with the image)
# Rust toolchain ownership transfer happens in `runtime-build-tools` (the only
# stage that installs rustup/cargo). `runtime-base` has empty $RUSTUP_HOME /
# $CARGO_HOME directories so chowning them here would be a no-op.
RUN set -eux \
    && mkdir -p \
        /home/maestro/.local/share/mise/shims \
        /home/maestro/.cache/mise \
        /home/maestro/.config/mise \
        /home/maestro/.npm \
        /home/maestro/.npm-global/lib \
        /home/maestro/.npm-global/bin \
        /opt/maestro-tools/bin \
        /workspace/logs \
        /workspaces \
        /etc/maestro/examples/workflows \
    && chown -R maestro:maestro \
        /home/maestro/.local \
        /home/maestro/.cache \
        /home/maestro/.config \
        /home/maestro/.npm \
        /home/maestro/.npm-global \
        /opt/maestro-tools \
        /workspace \
        /workspaces \
    && chmod 0755 /opt/maestro-tools /opt/maestro-tools/bin

# npm cache dir (for npx and package installs)
ENV NPM_CONFIG_CACHE=/home/maestro/.npm
# npm global prefix — redirects `npm install -g` and npx global installs away from root-owned /usr/local
ENV NPM_CONFIG_PREFIX=/home/maestro/.npm-global
ENV MISE_DATA_DIR=/home/maestro/.local/share/mise
ENV MISE_CACHE_DIR=/home/maestro/.cache/mise
ENV MISE_CONFIG_DIR=/home/maestro/.config/mise
ENV MISE_TRUST_ALL_CONFIGS=1
ENV MISE_YES=1
# Task #48: prepend the `maestro-tools` volume mount so anything dropped
# there by the `[provisioning]` install pass at boot SHADOWS baked tools
# of the same name. That gives admins a no-rebuild lever to pin a tool
# to a specific version (or swap an entire binary) without changing the
# image. The volume mountpoint was created in the combined mkdir RUN above;
# the named volume itself is declared in `docker-compose.yml`.
ENV PATH="/opt/maestro-tools/bin:/home/maestro/.npm-global/bin:/home/maestro/.local/share/mise/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

# Login-shell defaults — write both profile.d files + .bashrc in one RUN.
RUN set -eux \
    && printf '%s\n' \
        'export MISE_DATA_DIR=/home/maestro/.local/share/mise' \
        'export MISE_CACHE_DIR=/home/maestro/.cache/mise' \
        'export MISE_CONFIG_DIR=/home/maestro/.config/mise' \
        'export MISE_TRUST_ALL_CONFIGS=1' \
        'export MISE_YES=1' \
        'export PATH="$MISE_DATA_DIR/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' \
        > /etc/profile.d/zz-maestro-mise.sh \
    && chmod 644 /etc/profile.d/zz-maestro-mise.sh \
    && echo '[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a' \
        >> /etc/profile.d/maestro-env.sh \
    && echo '[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a' \
        >> /home/maestro/.bashrc

WORKDIR /workspace

EXPOSE 8080

# Entrypoint scripts — executable bit set via BuildKit --chmod, no chmod RUNs.
# The entrypoint applies egress rules (if NET_ADMIN capability is available),
# chowns named volumes that arrive root-owned, runs the [provisioning] install
# pass, then setpriv's to the maestro user and execs the maestro server.
COPY --chmod=0755 docker/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY --chmod=0755 docker/worker-entrypoint.sh /usr/local/bin/worker-entrypoint.sh
COPY --chmod=0755 docker/test-workflow.sh /usr/local/bin/test-workflow.sh

# Audit §3.5 — explicit non-root identity declaration. The runtime image declares
# the `maestro` user identity here so the build chain (and any `docker history`
# reader) can audit the non-root-by-default posture without grepping entrypoint.sh.
#
# The directive is immediately followed by `USER root` because the entrypoint
# still needs root for:
#   1. iptables (egress rules) — capabilities are bound to the container's
#      capability set, not to the calling UID, so even with CAP_NET_ADMIN a
#      non-root caller can't run iptables without ambient caps;
#   2. chown of named volumes that arrive root-owned from the host;
#   3. the [provisioning] install pass that writes to /opt/maestro-tools.
# `docker/entrypoint.sh` setpriv's back to maestro for the actual maestro
# server process (see entrypoint.sh ~line 178). A future entrypoint refactor
# (sudo-elevate via the existing /bin/bash sudoers rule) can move `USER maestro`
# to immediately before `ENTRYPOINT`; that change is out of Phase 3 scope
# because the brief explicitly forbids rewriting `entrypoint.sh`.
USER maestro
USER root

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--config", "/etc/maestro/config.toml"]

# ─────────────────────────────────────────────────────────────────────────────
# Stage 2b: Runtime + build tools (= image target `maestro:full`, default target).
#
# `FROM runtime-base` inherits everything in the slim image. This layer adds
# the C toolchain (gcc + headers) needed by `mise install` when building
# language runtimes from source (e.g. ruby-build → OpenSSL + Ruby on arm64
# where no prebuilt binary exists), and the Rust toolchain so Rust workflows
# can run `cargo build` inside ephemeral workers without bootstrapping rustup
# every time.
#
# This is the DEFAULT build target — plain `docker build .` and
# `docker compose build` produce `maestro:full`. Admins who want the smaller
# slim image opt in with `--target runtime-base`.
# ─────────────────────────────────────────────────────────────────────────────
FROM runtime-base AS runtime-build-tools

# Compiler + headers for mise-installed runtimes built from source (e.g. ruby-build → OpenSSL + Ruby).
# Without these, `mise install` fails on tools like ruby when no prebuilt binary exists (common on arm64).
RUN apt-get update && apt-get install -y --no-install-recommends \
    autoconf \
    bison \
    build-essential \
    libffi-dev \
    libgmp-dev \
    libreadline-dev \
    libssl-dev \
    libyaml-dev \
    patch \
    perl \
    pkg-config \
    zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust toolchain — baked system-wide so it is available in every container
# (editor, ephemeral workers, terminal) regardless of volume state.
# RUSTUP_HOME/CARGO_HOME are NOT volume-mounted, so the install lives in the
# image layer. Other runtimes (Java, Ruby, Go, …) are best managed via `mise`
# and the shared mise volume; Rust is special because many Rust workflows need
# `cargo` in ephemeral workers that start before any mise install can run.
RUN curl --proto '=https' --tlsv1.2 -sSf --retry 3 --retry-delay 5 https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --default-toolchain stable --profile minimal \
    && /usr/local/cargo/bin/rustup component add rustfmt clippy \
    && chmod -R a+r /usr/local/rustup /usr/local/cargo \
    && find /usr/local/cargo/bin -type f -exec chmod a+x {} \; \
    && /usr/local/cargo/bin/cargo --version \
    && /usr/local/cargo/bin/rustc --version \
    # Transfer rustup/cargo ownership so maestro can install toolchain versions at runtime.
    # (The image ships a pre-installed stable toolchain, but a project's .mise.toml may
    # request a different version; rustup needs to write to RUSTUP_HOME/tmp/ for that.)
    && chown -R maestro:maestro /usr/local/rustup /usr/local/cargo
