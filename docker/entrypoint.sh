#!/bin/bash
# entrypoint.sh — Container entrypoint for Maestro
#
# Modes:
#   setup  — required: GitHub + Atlassian auth; optional: Claude, Cursor, repo clone
#   (default) — egress rules, preflight, compose_up_commands, start Maestro
#
# Root performs privileged work, then re-execs as maestro (see id -u check).

set -euo pipefail

# ─── Root preamble (runs only as root) ──────────────────────────────────────
if [ "$(id -u)" = "0" ]; then
    if [ -f /etc/maestro/env ]; then
        set -a
        source /etc/maestro/env
        set +a
    fi

    # Named volumes often arrive root-owned; maestro must own auth trees so runtime (and optional
    # [docker] compose_up_commands) can write there.
    chown_maestro_tree() {
        local dir=$1
        [ -e "$dir" ] || return 0
        if ! chown -R maestro:maestro "$dir"; then
            echo "[maestro] WARNING: chown maestro:maestro $dir failed (hooks writing under this path may see Permission denied)" >&2
        fi
    }
    chown_maestro_tree /home/maestro/.claude
    chown_maestro_tree /home/maestro/.cursor
    chown_maestro_tree /home/maestro/.config
    chown_maestro_tree /home/maestro/.local
    chown_maestro_tree /home/maestro/.npm
    chown_maestro_tree /home/maestro/.aws
    chown_maestro_tree /workspace

    # Optional ./skills: baked in image + host bind at /opt/maestro/project-skills-host (see docker-compose).
    if ! /usr/local/bin/merge-project-skills.sh; then
        echo "[maestro] WARNING: merge-project-skills.sh failed" >&2
    fi

    if [ "${1:-}" != "setup" ]; then
        if iptables -L -n >/dev/null 2>&1; then
            echo "NET_ADMIN capability detected, applying egress rules..."
            /usr/local/bin/egress-rules.sh
        else
            echo "WARNING: NET_ADMIN capability not available. Egress rules NOT applied."
            echo "         Run container with --cap-add=NET_ADMIN to enable network restrictions."
        fi
    fi

    # Use runuser (not `su -`): login shells can block without a TTY under Podman/Docker, and `su -`
    # strips the environment (e.g. MAESTRO_CONFIG, FIGMA_API_TOKEN from compose).
    echo "[maestro] Starting as maestro user (preflight, hooks, server)..."
    exec runuser -u maestro -- /bin/bash -c "cd /workspace && exec /usr/local/bin/entrypoint.sh $(printf '%q ' "$@")"
fi

# ─── Everything below runs as the maestro user ───────────────────────────────

export HOME="${HOME:-/home/maestro}"
export MAESTRO_HOME="${MAESTRO_HOME:-/home/maestro}"
# Match docker-compose cursor-auth volume so `agent login` (setup) and `agent` (runtime) use the same store.
export CURSOR_CONFIG_DIR="${CURSOR_CONFIG_DIR:-$HOME/.cursor}"
export MISE_DATA_DIR="/home/maestro/.local/share/mise"
export MISE_CACHE_DIR="/home/maestro/.cache/mise"
export MISE_CONFIG_DIR="/home/maestro/.config/mise"
export MISE_TRUST_ALL_CONFIGS=1
export MISE_YES=1
mkdir -p "$MISE_DATA_DIR/shims" "$MISE_CACHE_DIR" "$MISE_CONFIG_DIR"
export PATH="$MISE_DATA_DIR/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

CONFIG_FILE="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"

# --- Setup mode ---
if [ "${1:-}" = "setup" ]; then
    echo "=== Maestro Setup ==="
    echo "Required: GitHub CLI + Atlassian CLI. Optional: Claude Code, Cursor Agent, repository clone."
    echo "Optional: add a gitignored ./skills folder at the Maestro repo root (merged on start); other tools via [docker] build_commands / compose_up_commands in config.toml."
    echo ""

    # Step 1: GitHub (required)
    echo "--- Step 1/5: GitHub CLI (required) ---"
    if gh auth status >/dev/null 2>&1; then
        echo "GitHub CLI: already authenticated."
        read -p "Re-authenticate? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            gh auth login
        fi
    else
        echo "GitHub CLI: authentication is required."
        gh auth login
    fi
    if ! gh auth status >/dev/null 2>&1; then
        echo "ERROR: GitHub CLI authentication failed or was not completed."
        exit 1
    fi
    echo ""

    # Step 2: Atlassian (required)
    echo "--- Step 2/5: Atlassian CLI (required) ---"
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
            echo "$api_token" | acli jira auth login --site "$jira_site" --email "$jira_email" --token
        else
            acli jira auth login --web
        fi
    }

    if acli jira auth status >/dev/null 2>&1; then
        echo "Atlassian CLI: already authenticated."
        read -p "Re-authenticate? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            acli_auth
        fi
    else
        echo "Atlassian CLI: authentication is required."
        acli_auth
    fi
    if ! acli jira auth status >/dev/null 2>&1; then
        echo "ERROR: Atlassian CLI authentication failed or was not completed."
        exit 1
    fi
    echo ""

    # Step 3: Claude Code (optional)
    echo "--- Step 3/5: Claude Code (optional) ---"
    read -p "Configure Claude Code (claude auth login)? [Y/s=skip] " -r
    echo
    if [[ $REPLY =~ ^[sS]$ ]]; then
        echo "Skipped Claude setup."
    else
        if command -v claude >/dev/null 2>&1; then
            if claude auth status >/dev/null 2>&1; then
                echo "Claude Code: already authenticated."
                read -p "Re-authenticate? [y/N] " -n 1 -r
                echo
                if [[ $REPLY =~ ^[Yy]$ ]]; then
                    claude auth login
                fi
            else
                claude auth login
            fi
        else
            echo "WARN: claude CLI not found on PATH."
        fi
    fi
    echo ""

    # Step 4: Cursor Agent (optional)
    echo "--- Step 4/5: Cursor Agent (optional) ---"
    read -p "Configure Cursor Agent (agent login)? [Y/s=skip] " -r
    echo
    if [[ $REPLY =~ ^[sS]$ ]]; then
        echo "Skipped Cursor Agent setup."
    else
        if command -v agent >/dev/null 2>&1; then
            agent login
        else
            echo "WARN: agent CLI not found on PATH. Install Cursor CLI or set CURSOR_API_KEY in maestro.env."
        fi
    fi
    echo ""

    # Step 5: Repository (optional)
    echo "--- Step 5/5: Repository (optional) ---"
    read -p "Clone or refresh repository from config? [Y/s=skip] " -r
    echo
    if [[ $REPLY =~ ^[sS]$ ]]; then
        echo "Skipped repository clone."
    else
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
                gh repo clone "$repo_url" /workspace
            fi
        else
            if [ "$(ls -A /workspace 2>/dev/null)" ]; then
                echo "Workspace is not empty but has no git repo. Cleaning..."
                rm -rf /workspace/*  /workspace/.[!.]* 2>/dev/null || true
            fi
            echo "Cloning $repo_url into /workspace..."
            gh repo clone "$repo_url" /workspace
        fi
        git config --global --add safe.directory /workspace
    fi
    echo ""

    echo "=== Setup complete ==="
    echo "Auth and workspace data are persisted in Docker volumes where configured."
    echo "Start Maestro with: docker compose up"
    exit 0
fi

# --- Normal mode ---

echo "[maestro] Running auth preflight..."
if ! /usr/local/bin/maestro --config "$CONFIG_FILE" preflight; then
    exit 1
fi

echo "[maestro] Running docker startup hooks (compose_up_commands)..."
if ! /usr/local/bin/maestro --config "$CONFIG_FILE" docker-hooks startup; then
    exit 1
fi

if [ ! -d "/workspace/.git" ]; then
    echo "ERROR: No repository found at /workspace."
    echo "       Run: docker compose run --rm -it maestro setup"
    exit 1
fi

export HOME="/home/maestro"
export USER="maestro"

git config --global --add safe.directory /workspace
gh auth setup-git 2>/dev/null || true
echo "[maestro] Starting Maestro server..."
exec /usr/local/bin/maestro "$@"
