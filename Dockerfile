# Stage 1: Build
FROM rust:1.85-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    jq \
    iptables \
    && rm -rf /var/lib/apt/lists/*

# Install Node.js 20.x (for Claude Code and Playwright)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install gh CLI (official apt repository)
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
    && apt-get update && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI (npm global)
RUN npm install -g @anthropic-ai/claude-code

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

# Copy Maestro binary from builder
COPY --from=builder /app/target/release/maestro /usr/local/bin/maestro

# Copy default config
COPY config.toml.example /etc/maestro/config.toml

# Create non-root user (Claude Code refuses --dangerously-skip-permissions as root)
RUN groupadd -r maestro && useradd -r -g maestro -m -s /bin/bash maestro


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
