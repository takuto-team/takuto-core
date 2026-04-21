# syntax=docker/dockerfile:1.6
# Stage 1a: Build React dashboard
FROM node:23-bookworm-slim AS ui-builder

WORKDIR /ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --legacy-peer-deps
COPY ui/ ./
# VERSION file is read by vite.config.ts at build time (resolve("../VERSION"))
COPY VERSION /VERSION
RUN npm run build

# Stage 1b: Build Rust binary
FROM rust:1.88-bookworm AS builder

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
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    mkdir -p /out \
    && echo "=== cargo build --release (first build often takes 10–20+ minutes; rebuilds reuse BuildKit cache) ===" \
    && cargo build --release \
    && cp /app/target/release/maestro /out/maestro

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

ARG MAESTRO_VERSION=dev
LABEL org.opencontainers.image.version="${MAESTRO_VERSION}"
LABEL org.opencontainers.image.source="https://github.com/morphet81/maestro"
LABEL org.opencontainers.image.title="Maestro"
LABEL org.opencontainers.image.description="Automated workflow orchestration for AI coding agents"

# Match host UID for rootless Podman API sockets (often mode 0600 — only the user matters). Set in compose `.env` and rebuild.
# Do not force the primary GID to match the host: macOS "staff" is often GID 20, which is `dialout` on Debian and already exists.
ARG MAESTRO_UID=999

# docker.io — Debian provides the `docker` CLI (bookworm has no `docker-cli` package in default repos).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    docker.io \
    git \
    jq \
    iptables \
    iproute2 \
    openssh-client \
    python3 \
    socat \
    # Playwright Chromium system dependencies
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
    && rm -rf /var/lib/apt/lists/*

# ttyd — lightweight web-based terminal (used by the dashboard "Open terminal" button)
RUN ARCH=$(dpkg --print-architecture) \
    && case "$ARCH" in amd64) TTYD_ARCH=x86_64 ;; arm64) TTYD_ARCH=aarch64 ;; *) echo "Unsupported arch: $ARCH" && exit 1 ;; esac \
    && curl -fsSL "https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.${TTYD_ARCH}" -o /usr/local/bin/ttyd \
    && chmod +x /usr/local/bin/ttyd


# mise — version manager for Node, Python, Ruby, etc. (project `.mise.toml` / `.tool-versions`)
RUN install -dm 755 /etc/apt/keyrings \
    && curl -fsSL https://mise.jdx.dev/gpg-key.pub -o /etc/apt/keyrings/mise-archive-keyring.asc \
    && echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.asc arch=$(dpkg --print-architecture)] https://mise.jdx.dev/deb stable main" \
       | tee /etc/apt/sources.list.d/mise.list > /dev/null \
    && apt-get update && apt-get install -y --no-install-recommends mise \
    && rm -rf /var/lib/apt/lists/* \
    && mise --version

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
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --default-toolchain stable --profile minimal \
    && /usr/local/cargo/bin/rustup component add rustfmt clippy \
    && chmod -R a+r /usr/local/rustup /usr/local/cargo \
    && find /usr/local/cargo/bin -type f -exec chmod a+x {} \; \
    && /usr/local/cargo/bin/cargo --version \
    && /usr/local/cargo/bin/rustc --version

# figma-cli (`fcli`) — optional Rust CLI for Figma integration.
# amd64: prebuilt release tarball. Other architectures: skipped (Figma features unavailable).
# Non-fatal: the build continues without fcli if the download fails.
ARG FCLI_VERSION=v0.2.0
RUN ARCH=$(dpkg --print-architecture); \
    if [ "$ARCH" = "amd64" ]; then \
      TARBALL="fcli-${FCLI_VERSION}-x86_64-unknown-linux-gnu.tar.gz"; \
      curl -fsSL "https://github.com/morphet81/figma-cli/releases/download/${FCLI_VERSION}/${TARBALL}" -o /tmp/fcli.tar.gz \
      && tar -xzf /tmp/fcli.tar.gz -C /tmp \
      && install -m 0755 /tmp/fcli /usr/local/bin/fcli \
      && rm -rf /tmp/fcli /tmp/fcli.tar.gz \
      && fcli --version \
      || echo "WARN: fcli install failed on $ARCH — Figma features unavailable"; \
    else \
      echo "WARN: fcli prebuilt binary not available for $ARCH — skipping"; \
    fi

# lokalise2 — Lokalise CLI v2 (Go). Prebuilt tarballs for both Linux arches
# published to GitHub releases. Binary lands at /usr/local/bin/lokalise2.
RUN set -eux; \
    ARCH=$(dpkg --print-architecture); \
    case "$ARCH" in \
      amd64) LOKA_ARCH=x86_64 ;; \
      arm64) LOKA_ARCH=arm64 ;; \
      *) echo "Unsupported arch: $ARCH"; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/lokalise/lokalise-cli-2-go/releases/latest/download/lokalise2_linux_${LOKA_ARCH}.tar.gz" -o /tmp/lokalise2.tar.gz; \
    tar -xzf /tmp/lokalise2.tar.gz -C /tmp; \
    install -m 0755 /tmp/lokalise2 /usr/local/bin/lokalise2; \
    rm -rf /tmp/lokalise2 /tmp/lokalise2.tar.gz; \
    lokalise2 --version

# Node.js 23+ (official tarball). Cursor Agent runs `node --use-system-ca`, which exists only on Node >= 23.9
# on Linux; NodeSource 20.x rejects that flag with "bad option: --use-system-ca".
ARG NODE_VERSION=23.11.0
RUN set -eux; \
    ARCH="$(dpkg --print-architecture)"; \
    case "$ARCH" in \
      amd64) NODE_ARCH=x64 ;; \
      arm64) NODE_ARCH=arm64 ;; \
      *) echo "unsupported architecture: $ARCH"; exit 1 ;; \
    esac; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${NODE_ARCH}.tar.gz" \
      | tar -xz -C /usr/local --strip-components=1; \
    node --version; npm --version

# Install gh CLI (official apt repository)
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
    && apt-get update && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI (npm global)
RUN npm install -g @anthropic-ai/claude-code@2.1.110

# Cursor Agent CLI (for [agent] provider = "cursor"). The launcher resolves paths with realpath("$0");
# copying only the script to /usr/local/bin breaks it (looks for index.js next to the copy). Install the
# full package under /usr/local and symlink agent into PATH.
RUN curl -fsSL https://cursor.com/install | bash \
    && AGENT_REAL="$(readlink -f /root/.local/bin/agent)" \
    && cp -a /root/.local/share/cursor-agent /usr/local/share/cursor-agent \
    && ln -sf "/usr/local/share/cursor-agent${AGENT_REAL#/root/.local/share/cursor-agent}" /usr/local/bin/agent \
    && chmod -R a+rX /usr/local/share/cursor-agent \
    && test -f "$(dirname "$(readlink -f /usr/local/bin/agent)")/index.js"

# Playwright is not baked into this image: isolated workflow workers use the project's @playwright/test
# version and download Chromium into ~/.cache/ms-playwright (persisted via docker-compose.dind.yml
# playwright-cache → /shared-auth/playwright-cache). Forcing a mismatched browser revision caused subtle
# visual snapshot drift vs local/CI.

# Install acli (Atlassian CLI) via official apt repo
RUN apt-get update && apt-get install -y --no-install-recommends wget gnupg2 \
    && mkdir -p -m 755 /etc/apt/keyrings \
    && wget -nv -O- https://acli.atlassian.com/gpg/public-key.asc | gpg --dearmor -o /etc/apt/keyrings/acli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/acli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/acli-archive-keyring.gpg] https://acli.atlassian.com/linux/deb stable main" \
       | tee /etc/apt/sources.list.d/acli.list > /dev/null \
    && apt-get update && apt-get install -y --no-install-recommends acli \
    && rm -rf /var/lib/apt/lists/*

# openvscode-server — browser-based VS Code for manual worktree editing via dashboard
ARG OPENVSCODE_VERSION=1.109.5
RUN set -eux; \
    ARCH="$(dpkg --print-architecture)"; \
    curl -fsSL "https://github.com/gitpod-io/openvscode-server/releases/download/openvscode-server-v${OPENVSCODE_VERSION}/openvscode-server-v${OPENVSCODE_VERSION}-linux-${ARCH}.tar.gz" \
      | tar -xz -C /opt \
    && ln -s "/opt/openvscode-server-v${OPENVSCODE_VERSION}-linux-${ARCH}/bin/openvscode-server" /usr/local/bin/openvscode-server

# Install AWS CLI (optional, for CodeArtifact npm registry auth)
RUN apt-get update && apt-get install -y --no-install-recommends unzip \
    && curl -sL "https://awscli.amazonaws.com/awscli-exe-linux-$(uname -m).zip" -o /tmp/awscliv2.zip \
    && unzip -q /tmp/awscliv2.zip -d /tmp \
    && /tmp/aws/install \
    && rm -rf /tmp/awscliv2.zip /tmp/aws /var/lib/apt/lists/*

# Install figma-cli (npm global)
RUN npm install -g figma-cli || echo "WARN: figma-cli install failed, Figma features will be unavailable"

# Copy egress rules script
COPY docker/egress-rules.sh /usr/local/bin/egress-rules.sh
RUN chmod +x /usr/local/bin/egress-rules.sh


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
RUN mkdir -p /etc/maestro/examples/workflows
COPY config.toml.example /etc/maestro/examples/config.toml.example
COPY maestro.env.example /etc/maestro/examples/maestro.env.example
COPY workflows/*.example.toml /etc/maestro/examples/workflows/

# Create non-root user (Claude Code refuses --dangerously-skip-permissions as root).
# Default UID 999; override MAESTRO_UID via compose for host engine sockets. Group `maestro` gets the next free GID.
RUN groupadd maestro \
    && useradd -u "${MAESTRO_UID}" -g maestro -m -s /bin/bash maestro

# Startup hooks run as maestro; config may use `sudo /usr/bin/bash` for root-owned volume paths.
# (Use bash explicitly — `sudo env bash` would match /usr/bin/env and fail the sudoers rule.)
RUN apt-get update && apt-get install -y --no-install-recommends sudo \
    && printf '%s\n' \
       'maestro ALL=(root) NOPASSWD: /usr/bin/bash, /bin/bash, /usr/bin/bash *, /bin/bash *' \
       > /etc/sudoers.d/maestro-hook-bash \
    && chmod 0440 /etc/sudoers.d/maestro-hook-bash \
    && visudo -cf /etc/sudoers.d/maestro-hook-bash \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /home/maestro/.local/share/mise/shims \
    /home/maestro/.cache/mise \
    /home/maestro/.config/mise \
    && chown -R maestro:maestro /home/maestro/.local /home/maestro/.cache /home/maestro/.config

ENV MISE_DATA_DIR=/home/maestro/.local/share/mise
ENV MISE_CACHE_DIR=/home/maestro/.cache/mise
ENV MISE_CONFIG_DIR=/home/maestro/.config/mise
ENV MISE_TRUST_ALL_CONFIGS=1
ENV MISE_YES=1
ENV PATH="/home/maestro/.local/share/mise/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

RUN printf '%s\n' \
    'export MISE_DATA_DIR=/home/maestro/.local/share/mise' \
    'export MISE_CACHE_DIR=/home/maestro/.cache/mise' \
    'export MISE_CONFIG_DIR=/home/maestro/.config/mise' \
    'export MISE_TRUST_ALL_CONFIGS=1' \
    'export MISE_YES=1' \
    'export PATH="$MISE_DATA_DIR/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' \
    > /etc/profile.d/zz-maestro-mise.sh \
    && chmod 644 /etc/profile.d/zz-maestro-mise.sh

# Source custom env file on any shell login
RUN echo '[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a' >> /etc/profile.d/maestro-env.sh \
    && echo '[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a' >> /home/maestro/.bashrc

# Create workspace and log directories with correct ownership
RUN mkdir -p /workspace /workspace/logs \
    && chown -R maestro:maestro /workspace

WORKDIR /workspace

EXPOSE 8080

# Entrypoint: apply egress rules (if NET_ADMIN capability is available), then start Maestro
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

COPY docker/worker-entrypoint.sh /usr/local/bin/worker-entrypoint.sh
RUN chmod +x /usr/local/bin/worker-entrypoint.sh

COPY docker/test-workflow.sh /usr/local/bin/test-workflow.sh
RUN chmod +x /usr/local/bin/test-workflow.sh

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--config", "/etc/maestro/config.toml"]
