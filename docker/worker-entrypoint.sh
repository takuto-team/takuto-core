#!/bin/bash
set -euo pipefail
if iptables -L -n >/dev/null 2>&1; then
    /usr/local/bin/egress-rules.sh
fi
# Ensure shared volumes are writable by takuto (fresh volumes start root-owned)
for d in /home/takuto/.npm /home/takuto/.npm-global /home/takuto/.cache/mise /home/takuto/.local/share/mise; do
    [ -d "$d" ] && chown -R takuto:takuto "$d" 2>/dev/null || true
done
[ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a

# ────────────────────────────────────────────────────────────────────────
# Phase 2b.3 (04_architecture.md §6): per-workflow secrets bundle.
#
# When the Takuto orchestrator attached a `WorkerSecretsBundle` to this
# container, it bind-mounts the secrets at /run/takuto-secrets:ro and
# sets TAKUTO_AUTH_BUNDLE=1. We source each present file into the right
# env var, then `rm` the on-disk copy to shrink the blast radius if the
# container is later compromised.
#
# The token bytes were NEVER passed via `docker run -e KEY=value`, so
# `docker inspect <ctr>` does not leak them.
# ────────────────────────────────────────────────────────────────────────
if [ "${TAKUTO_AUTH_BUNDLE:-0}" = "1" ] && [ -d /run/takuto-secrets ]; then
    # Task #42: observability breadcrumb. When the bundle's discriminator
    # env var is set but /run/takuto-secrets/ is empty, the bundle's
    # host-side TempDir has dropped out from under us. Emit a single
    # grep-friendly stderr line so future regressions are visible in the
    # workflow terminal instead of degrading silently into the
    # deployment-default path.
    __bundle_present=$(ls -A /run/takuto-secrets 2>/dev/null | wc -l)
    if [ "${__bundle_present:-0}" = "0" ]; then
        echo "[takuto-bundle] TAKUTO_AUTH_BUNDLE=1 but /run/takuto-secrets/ is empty -- secret files vanished (host TempDir dropped). Check WorkerSecretsBundle lifetime in AppState." >&2
    fi
    # AI-provider tokens. Each file maps to one env var the provider CLI
    # picks up natively.
    if [ -f /run/takuto-secrets/claude ]; then
        CLAUDE_CODE_OAUTH_TOKEN="$(cat /run/takuto-secrets/claude)"
        export CLAUDE_CODE_OAUTH_TOKEN
        rm -f /run/takuto-secrets/claude || true
    fi
    if [ -f /run/takuto-secrets/cursor ]; then
        CURSOR_API_KEY="$(cat /run/takuto-secrets/cursor)"
        export CURSOR_API_KEY
        rm -f /run/takuto-secrets/cursor || true
    fi
    if [ -f /run/takuto-secrets/codex ]; then
        OPENAI_API_KEY="$(cat /run/takuto-secrets/codex)"
        export OPENAI_API_KEY
        rm -f /run/takuto-secrets/codex || true
    fi
    # OpenCode self-hosted spec (lore/audits/2026-05-27-opencode-self-hosted-spec.md):
    # the bundle does NOT write a per-provider secret file for OpenCode.
    # OpenCode's CLI ignores env-var tokens; it reads provider config from
    # ~/.config/opencode/opencode.json, which the bundle materialises and
    # bind-mounts read-only at /home/takuto/.config/opencode. The
    # user's bearer is baked into options.apiKey there. The legacy
    # ANTHROPIC_API_KEY mapping was a wrong-tool footgun (use the Claude
    # provider for Anthropic) and is intentionally absent.
    # Task #41 (was #39): Claude session-state (`~/.claude.json`). When the
    # user is on a team / Pro plan, the API key alone isn't enough — Claude
    # Code requires a populated `oauthAccount` block in $HOME/.claude.json
    # before it considers the session "logged in". The bundle ships only
    # the keys the user pasted (typically just `oauthAccount`), so a naive
    # `cp` would wipe other state (hasCompletedOnboarding, userID, accumulated
    # tipsHistory, …) that Claude Code also reads. Use jq's `*` shallow-merge
    # to overlay the bundle keys onto the existing file. Fall back to `cp`
    # when there's no existing .claude.json OR jq is unexpectedly missing.
    if [ -f /run/takuto-secrets/claude_session.json ]; then
        if [ -f "$HOME/.claude.json" ] && command -v jq >/dev/null 2>&1; then
            __mtmp=$(mktemp)
            if jq -s '.[0] * .[1]' "$HOME/.claude.json" /run/takuto-secrets/claude_session.json > "$__mtmp" 2>/dev/null; then
                mv "$__mtmp" "$HOME/.claude.json"
            else
                rm -f "$__mtmp"
                cp /run/takuto-secrets/claude_session.json "$HOME/.claude.json" || true
            fi
        else
            cp /run/takuto-secrets/claude_session.json "$HOME/.claude.json" || true
        fi
        rm -f /run/takuto-secrets/claude_session.json || true
    fi
    # GitHub token. Used by `gh`, `git push`, and the inline credential
    # helper that other parts of the stack install on the fly.
    if [ -f /run/takuto-secrets/gh ]; then
        GH_TOKEN="$(cat /run/takuto-secrets/gh)"
        export GH_TOKEN
        rm -f /run/takuto-secrets/gh || true
    fi
else
    # Legacy path: source the App-installation token written by Takuto's
    # background service. This is the pre-Phase-2b.3 default and stays
    # active for workflows with `user_id = None` (single-tenant + poller).
    GH_APP_TOKEN_FILE="/home/takuto/.config/gh/gh-app-token"
    [ -f "$GH_APP_TOKEN_FILE" ] && export GH_TOKEN="$(cat "$GH_APP_TOKEN_FILE")"
fi

exec runuser -u takuto -- "$@"
