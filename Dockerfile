# syntax=docker/dockerfile:1.6
# Stage 1: Build
FROM rust:1.85-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

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
    && rm -rf /var/lib/apt/lists/*

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
RUN npm install -g @anthropic-ai/claude-code

# Cursor Agent CLI (for [agent] provider = "cursor"). The launcher resolves paths with realpath("$0");
# copying only the script to /usr/local/bin breaks it (looks for index.js next to the copy). Install the
# full package under /usr/local and symlink agent into PATH.
RUN curl -fsSL https://cursor.com/install | bash \
    && AGENT_REAL="$(readlink -f /root/.local/bin/agent)" \
    && cp -a /root/.local/share/cursor-agent /usr/local/share/cursor-agent \
    && ln -sf "/usr/local/share/cursor-agent${AGENT_REAL#/root/.local/share/cursor-agent}" /usr/local/bin/agent \
    && chmod -R a+rX /usr/local/share/cursor-agent \
    && test -f "$(dirname "$(readlink -f /usr/local/bin/agent)")/index.js"

# Install Playwright CLI with Chromium
RUN npx playwright install --with-deps chromium

# Install acli (Atlassian CLI) via official apt repo
RUN apt-get update && apt-get install -y --no-install-recommends wget gnupg2 \
    && mkdir -p -m 755 /etc/apt/keyrings \
    && wget -nv -O- https://acli.atlassian.com/gpg/public-key.asc | gpg --dearmor -o /etc/apt/keyrings/acli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/acli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/acli-archive-keyring.gpg] https://acli.atlassian.com/linux/deb stable main" \
       | tee /etc/apt/sources.list.d/acli.list > /dev/null \
    && apt-get update && apt-get install -y --no-install-recommends acli \
    && rm -rf /var/lib/apt/lists/*

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

# Merge optional ./skills from build context into image (empty if missing or empty dir).
RUN mkdir -p /opt/maestro/project-skills-baked /opt/maestro/project-skills-host
RUN --mount=type=bind,source=.,target=/ctx \
    if [ -d /ctx/skills ] && [ -n "$(ls -A /ctx/skills 2>/dev/null)" ]; then \
      cp -a /ctx/skills/. /opt/maestro/project-skills-baked/; \
    fi

COPY docker/merge-project-skills.sh /usr/local/bin/merge-project-skills.sh
RUN chmod 0755 /usr/local/bin/merge-project-skills.sh

# Copy Maestro binary from builder
COPY --from=builder /app/target/release/maestro /usr/local/bin/maestro

# Optional: TOML file in build context used only for [docker] build_commands (default: example with empty hooks)
ARG MAESTRO_BUILD_CONFIG=config.toml.example
COPY ${MAESTRO_BUILD_CONFIG} /tmp/maestro-build-config.toml
RUN mkdir -p /workspace \
    && maestro --config /tmp/maestro-build-config.toml docker-hooks build

# Copy default runtime config
COPY config.toml.example /etc/maestro/config.toml

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
ENV PATH="/home/maestro/.local/share/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

RUN printf '%s\n' \
    'export MISE_DATA_DIR=/home/maestro/.local/share/mise' \
    'export MISE_CACHE_DIR=/home/maestro/.cache/mise' \
    'export MISE_CONFIG_DIR=/home/maestro/.config/mise' \
    'export MISE_TRUST_ALL_CONFIGS=1' \
    'export MISE_YES=1' \
    'export PATH="$MISE_DATA_DIR/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' \
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

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--config", "/etc/maestro/config.toml"]
