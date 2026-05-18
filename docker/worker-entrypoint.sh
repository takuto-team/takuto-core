#!/bin/bash
set -euo pipefail
if iptables -L -n >/dev/null 2>&1; then
    /usr/local/bin/egress-rules.sh
fi
# Ensure shared volumes are writable by maestro (fresh volumes start root-owned)
for d in /home/maestro/.npm /home/maestro/.npm-global /home/maestro/.cache/mise /home/maestro/.local/share/mise; do
    [ -d "$d" ] && chown -R maestro:maestro "$d" 2>/dev/null || true
done
[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a

# ────────────────────────────────────────────────────────────────────────
# Phase 2b.3 (04_architecture.md §6): per-workflow secrets bundle.
#
# When the Maestro orchestrator attached a `WorkerSecretsBundle` to this
# container, it bind-mounts the secrets at /run/maestro-secrets:ro and
# sets MAESTRO_AUTH_BUNDLE=1. We source each present file into the right
# env var, then `rm` the on-disk copy to shrink the blast radius if the
# container is later compromised.
#
# The token bytes were NEVER passed via `docker run -e KEY=value`, so
# `docker inspect <ctr>` does not leak them.
# ────────────────────────────────────────────────────────────────────────
if [ "${MAESTRO_AUTH_BUNDLE:-0}" = "1" ] && [ -d /run/maestro-secrets ]; then
    # AI-provider tokens. Each file maps to one env var the provider CLI
    # picks up natively.
    if [ -f /run/maestro-secrets/claude ]; then
        CLAUDE_CODE_OAUTH_TOKEN="$(cat /run/maestro-secrets/claude)"
        export CLAUDE_CODE_OAUTH_TOKEN
        rm -f /run/maestro-secrets/claude || true
    fi
    if [ -f /run/maestro-secrets/cursor ]; then
        CURSOR_API_KEY="$(cat /run/maestro-secrets/cursor)"
        export CURSOR_API_KEY
        rm -f /run/maestro-secrets/cursor || true
    fi
    if [ -f /run/maestro-secrets/codex ]; then
        OPENAI_API_KEY="$(cat /run/maestro-secrets/codex)"
        export OPENAI_API_KEY
        rm -f /run/maestro-secrets/codex || true
    fi
    if [ -f /run/maestro-secrets/opencode ]; then
        # OpenCode picks up an anthropic / openai / openrouter / etc. key
        # via the same env vars the underlying provider uses. We default
        # to ANTHROPIC_API_KEY; admins can override via the OpenCode
        # provider sub-table's base_url, exported by the bundle as
        # OPENCODE_PROVIDER_BASE_URL.
        ANTHROPIC_API_KEY="$(cat /run/maestro-secrets/opencode)"
        export ANTHROPIC_API_KEY
        rm -f /run/maestro-secrets/opencode || true
    fi
    # GitHub token. Used by `gh`, `git push`, and the inline credential
    # helper that other parts of the stack install on the fly.
    if [ -f /run/maestro-secrets/gh ]; then
        GH_TOKEN="$(cat /run/maestro-secrets/gh)"
        export GH_TOKEN
        rm -f /run/maestro-secrets/gh || true
    fi
else
    # Legacy path: source the App-installation token written by Maestro's
    # background service. This is the pre-Phase-2b.3 default and stays
    # active for workflows with `user_id = None` (single-tenant + poller).
    GH_APP_TOKEN_FILE="/home/maestro/.config/gh/gh-app-token"
    [ -f "$GH_APP_TOKEN_FILE" ] && export GH_TOKEN="$(cat "$GH_APP_TOKEN_FILE")"
fi

exec runuser -u maestro -- "$@"
