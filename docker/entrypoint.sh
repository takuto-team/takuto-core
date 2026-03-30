#!/bin/bash
# entrypoint.sh — Container entrypoint for Maestro
#
# Supports two modes:
#   setup  — interactive auth for Claude Code, GitHub CLI, Atlassian CLI, then clone repo
#   (default) — apply egress rules and start Maestro

set -euo pipefail

# Source custom environment file if mounted
if [ -f /etc/maestro/env ]; then
    set -a
    source /etc/maestro/env
    set +a
fi

# --- Setup mode: interactive authentication + repo clone ---
if [ "${1:-}" = "setup" ]; then
    echo "=== Maestro Setup ==="
    echo ""

    # Ensure volumes are owned by maestro
    chown -R maestro:maestro /home/maestro/.claude 2>/dev/null || true
    chown -R maestro:maestro /home/maestro/.config 2>/dev/null || true

    # Step 1: Claude Code auth (run as maestro user)
    echo "--- Step 1/5: Claude Code authentication ---"
    if su maestro -c "claude auth status" >/dev/null 2>&1; then
        echo "Claude Code: already authenticated."
        read -p "Re-authenticate? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            su maestro -c "claude auth login"
        fi
    else
        echo "Claude Code: not authenticated."
        su maestro -c "claude auth login"
    fi
    echo ""

    # Step 2: GitHub CLI auth (run as maestro user)
    echo "--- Step 2/5: GitHub CLI authentication ---"
    if su maestro -c "gh auth status" >/dev/null 2>&1; then
        echo "GitHub CLI: already authenticated."
        read -p "Re-authenticate? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            su maestro -c "gh auth login"
        fi
    else
        echo "GitHub CLI: not authenticated."
        su maestro -c "gh auth login"
    fi
    echo ""

    # Step 3: Atlassian CLI auth
    echo "--- Step 3/5: Atlassian CLI authentication ---"
    CONFIG_FILE="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"
    jira_site=$(grep -E '^\s*site\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' || true)
    jira_email=$(grep -E '^\s*email\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' || true)

    acli_auth() {
        echo "Choose authentication method:"
        echo "  1) OAuth (browser-based)"
        echo "  2) API token"
        read -p "Choice [1/2]: " -n 1 -r
        echo
        if [[ $REPLY == "2" ]]; then
            if [ -z "$jira_site" ] || [ -z "$jira_email" ]; then
                echo "ERROR: Token auth requires 'site' and 'email' in [jira] config."
                echo "       Add them to config.toml and re-run setup."
                return 1
            fi
            read -sp "Paste your Atlassian API token: " api_token
            echo
            echo "$api_token" | su maestro -c "acli jira auth login --site \"$jira_site\" --email \"$jira_email\" --token"
        else
            su maestro -c "acli jira auth login --web"
        fi
    }

    if su maestro -c "acli jira auth status" >/dev/null 2>&1; then
        echo "Atlassian CLI: already authenticated."
        read -p "Re-authenticate? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            acli_auth
        fi
    else
        echo "Atlassian CLI: not authenticated."
        acli_auth
    fi
    echo ""

    # Step 4: Clone repository (read repo_url from config.toml)
    echo "--- Step 4/5: Repository setup ---"
    CONFIG_FILE="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"
    repo_url=$(grep -E '^\s*repo_url\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' || true)

    if [ -z "$repo_url" ]; then
        echo "WARNING: No repo_url found in $CONFIG_FILE. Skipping clone."
        echo "         Add repo_url under [git] in your config.toml."
    elif [ -d "/workspace/.git" ]; then
        echo "Repository already cloned at /workspace."
        read -p "Re-clone from $repo_url? This will delete the existing workspace. [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            rm -rf /workspace/*  /workspace/.[!.]* 2>/dev/null || true
            su maestro -c "gh repo clone '$repo_url' /workspace"
        fi
    else
        echo "Cloning $repo_url into /workspace..."
        gh repo clone "$repo_url" /workspace
    fi
    git config --global --add safe.directory /workspace
    echo ""

    # Step 5: Install Claude Code skills (as maestro user since that's who runs Claude)
    echo "--- Step 5/5: Installing Claude Code skills ---"
    if [ -d "/home/maestro/.claude/skills" ] && [ "$(ls -A /home/maestro/.claude/skills 2>/dev/null)" ]; then
        echo "Skills already installed."
        read -p "Re-install? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            su maestro -c 'curl -sL https://raw.githubusercontent.com/morphet81/cheat-sheets/main/install-skills.sh -o /tmp/install-skills.sh && bash /tmp/install-skills.sh && rm /tmp/install-skills.sh'
        fi
    else
        su maestro -c 'curl -sL https://raw.githubusercontent.com/morphet81/cheat-sheets/main/install-skills.sh -o /tmp/install-skills.sh && bash /tmp/install-skills.sh && rm /tmp/install-skills.sh'
    fi
    echo ""

    echo "=== Setup complete ==="
    echo "Auth and workspace are persisted in Docker volumes."
    echo "Start Maestro with: docker compose up"
    exit 0
fi

# --- Normal mode: start Maestro ---

# Check auth before starting
auth_ok=true

if ! su maestro -c "claude auth status" >/dev/null 2>&1; then
    echo "ERROR: Claude Code is not authenticated."
    echo "       Run: docker compose run maestro setup"
    auth_ok=false
fi

if ! su maestro -c "gh auth status" >/dev/null 2>&1; then
    echo "ERROR: GitHub CLI is not authenticated."
    echo "       Run: docker compose run maestro setup"
    auth_ok=false
fi

if ! su maestro -c "acli jira auth status" >/dev/null 2>&1; then
    echo "ERROR: Atlassian CLI is not authenticated."
    echo "       Run: docker compose run maestro setup"
    auth_ok=false
fi

if [ ! -d "/workspace/.git" ]; then
    echo "ERROR: No repository found at /workspace."
    echo "       Run: docker compose run --rm maestro setup"
    auth_ok=false
fi

if [ "$auth_ok" = false ]; then
    exit 1
fi

# Try to apply egress rules as root (requires NET_ADMIN capability)
if iptables -L -n >/dev/null 2>&1; then
    echo "NET_ADMIN capability detected, applying egress rules..."
    /usr/local/bin/egress-rules.sh
else
    echo "WARNING: NET_ADMIN capability not available. Egress rules NOT applied."
    echo "         Run container with --cap-add=NET_ADMIN to enable network restrictions."
fi

# Ensure workspace is owned by maestro user
chown -R maestro:maestro /workspace

# Ensure volumes mounted at maestro's home are owned correctly
chown -R maestro:maestro /home/maestro/.claude 2>/dev/null || true
chown -R maestro:maestro /home/maestro/.config 2>/dev/null || true
chown -R maestro:maestro /home/maestro/.npm 2>/dev/null || true

# Switch to non-root user and start Maestro
# (Claude Code refuses --dangerously-skip-permissions as root)
exec su maestro -c "
    git config --global --add safe.directory /workspace
    gh auth setup-git 2>/dev/null || true
    source /etc/profile.d/maestro-env.sh 2>/dev/null || true
    exec /usr/local/bin/maestro $*
"
