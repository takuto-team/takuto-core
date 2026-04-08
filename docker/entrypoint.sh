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
    chown_maestro_tree /home/maestro/.agents
    chown_maestro_tree /home/maestro/.config
    chown_maestro_tree /home/maestro/.local
    chown_maestro_tree /home/maestro/.npm
    chown_maestro_tree /home/maestro/.aws
    chown_maestro_tree /workspace

    # Optional ./skills: baked in image + host bind at /opt/maestro/project-skills-host (see docker-compose).
    if ! /usr/local/bin/merge-project-skills.sh; then
        echo "[maestro] WARNING: merge-project-skills.sh failed" >&2
    fi

    if [ "${1:-}" != "setup" ] && [ "${1:-}" != "test-workflow" ]; then
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

# Restore .claude.json from backup if missing (volume can lose it on unclean shutdown)
if [ ! -f "$HOME/.claude.json" ]; then
    backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1)
    if [ -n "$backup" ]; then
        cp "$backup" "$HOME/.claude.json"
        echo "[maestro] Restored .claude.json from backup"
    fi
fi

CONFIG_FILE="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"

# Optional host engine socket — warn early when the mount exists but this user cannot use it.
if [ -e /var/run/docker.sock ]; then
    if [ ! -S /var/run/docker.sock ]; then
        echo "[maestro] WARNING: /var/run/docker.sock exists but is not a Unix socket (wrong host bind path?)" >&2
    elif [ ! -r /var/run/docker.sock ] || [ ! -w /var/run/docker.sock ]; then
        echo "[maestro] WARNING: /var/run/docker.sock is not readable/writable as uid=$(id -u) gid=$(id -g)." >&2
        echo "[maestro]          If you use rootless Podman, its socket is often 0600 — rebuild with MAESTRO_UID = host id -u" >&2
        echo "[maestro]          (see README \"Host container socket\"; host id -g is not used — it often conflicts with Debian)." >&2
        echo "[maestro]          Consider using the DinD sidecar instead (docker-compose.dind.yml) — see README." >&2
    fi
fi

# --- Test workflow mode ---
if [ "${1:-}" = "test-workflow" ]; then
    exec /usr/local/bin/test-workflow.sh
fi

# --- Setup mode ---
if [ "${1:-}" = "setup" ]; then
    # Read [agent] provider from config (default: claude).
    agent_provider=$(grep -E '^\s*provider\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | tr -d ' ' || true)
    agent_provider="${agent_provider:-claude}"

    echo "=== Maestro Setup ==="
    echo "Required: GitHub CLI + Atlassian CLI + agent provider ($agent_provider). Optional: repository clone."
    echo "Optional: add a gitignored ./skills folder at the Maestro repo root (merged on start); other tools via [docker] build_commands / compose_up_commands in config.toml."
    echo ""

    # Step 1: GitHub (required — uses browser OAuth via --network=host)
    echo "--- Step 1/4: GitHub CLI (required) ---"
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

    # Step 2: Atlassian (required — manual API token, no port needed)
    echo "--- Step 2/4: Atlassian CLI (required) ---"
    jira_site=$(grep -E '^\s*site\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' || true)
    jira_email=$(grep -E '^\s*email\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' || true)

    acli_auth() {
        if [ -z "$jira_site" ] || [ -z "$jira_email" ]; then
            echo "ERROR: 'site' and 'email' must be set in [jira] config."
            echo "       Add them to config.toml and re-run setup."
            return 1
        fi
        echo "Authenticate with an Atlassian API token."
        echo "  Site:  $jira_site"
        echo "  Email: $jira_email"
        echo ""
        echo "Generate a token at: https://id.atlassian.com/manage-profile/security/api-tokens"
        echo ""
        read -sp "Paste your Atlassian API token: " api_token
        echo
        echo "$api_token" | acli jira auth login --site "$jira_site" --email "$jira_email" --token
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
    # Sync Jira credentials to global auth so `acli auth status` (without
    # product qualifier) also reports authenticated.  Many skills check the
    # global status; without this copy the check fails even though Jira auth
    # is perfectly valid.
    acli_cfg_dir="${HOME}/.config/acli"
    if [ -f "${acli_cfg_dir}/jira_config.yaml" ]; then
        cp "${acli_cfg_dir}/jira_config.yaml" "${acli_cfg_dir}/global_auth_config.yaml"
    fi
    echo ""

    # Step 3: Agent provider auth (required — determined by [agent] provider in config)
    echo "--- Step 3/4: Agent provider — $agent_provider (required) ---"
    if [ "$agent_provider" = "claude" ]; then
        if ! command -v claude >/dev/null 2>&1; then
            echo "ERROR: claude CLI not found on PATH."
            exit 1
        fi
        if [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then
            echo "Claude Code: CLAUDE_CODE_OAUTH_TOKEN is set in environment, skipping interactive login."
        elif claude auth status >/dev/null 2>&1; then
            echo "Claude Code: already authenticated."
            read -p "Re-authenticate? [y/N] " -n 1 -r
            echo
            if [[ $REPLY =~ ^[Yy]$ ]]; then
                claude auth login
            fi
        else
            echo "Claude Code: authentication is required (browser OAuth via --network=host)."
            claude auth login
        fi
        if [ -z "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] && ! claude auth status >/dev/null 2>&1; then
            echo "ERROR: Claude Code authentication failed or was not completed."
            exit 1
        fi
    elif [ "$agent_provider" = "cursor" ]; then
        if ! command -v agent >/dev/null 2>&1; then
            echo "ERROR: Cursor Agent CLI (agent) not found on PATH."
            echo "       Install Cursor CLI or set CURSOR_API_KEY in maestro.env."
            exit 1
        fi
        if [ -n "${CURSOR_API_KEY:-}" ]; then
            echo "Cursor Agent: CURSOR_API_KEY is set in environment, skipping interactive login."
        else
            echo "Cursor Agent: authentication is required (browser OAuth via --network=host)."
            agent login
        fi
    else
        echo "WARNING: Unknown agent provider '$agent_provider'. Skipping agent auth."
    fi
    echo ""

    # Step 4: Repository (optional)
    echo "--- Step 4/4: Repository (optional) ---"
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

# When using a DinD sidecar (DOCKER_HOST=tcp://...), wait for the daemon.
# Compose depends_on + healthcheck handles most of this, but a brief poll
# avoids a race if compose_up_commands fire before the health check passes.
if [ -n "${DOCKER_HOST:-}" ] && [[ "$DOCKER_HOST" == tcp://* ]]; then
    echo "[maestro] Waiting for Docker daemon at $DOCKER_HOST..."
    for i in $(seq 1 30); do
        if docker info >/dev/null 2>&1; then
            echo "[maestro] Docker daemon is ready."
            break
        fi
        if [ "$i" = 30 ]; then
            echo "[maestro] WARNING: Docker daemon at $DOCKER_HOST not reachable after 30s." >&2
            echo "[maestro]          compose_up_commands that need docker may fail." >&2
        fi
        sleep 1
    done

    # After daemon is ready, check worker image
    WORKER_IMAGE="${MAESTRO_WORKER_IMAGE:-maestro:latest}"
    if ! docker image inspect "$WORKER_IMAGE" >/dev/null 2>&1; then
        echo "[maestro] WARNING: Worker image '$WORKER_IMAGE' not found on DinD." >&2
        echo "[maestro]          Workflow isolation requires the worker image. Run: make load-worker" >&2
        echo "[maestro]          Falling back to local execution." >&2
    fi
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
# Rewrite SSH GitHub URLs to HTTPS so the gh credential helper handles auth.
# Without this, git-over-SSH fails because no SSH keys are persisted across restarts.
git config --global url."https://github.com/".insteadOf "git@github.com:" 2>/dev/null || true
echo "[maestro] Starting Maestro server..."
exec /usr/local/bin/maestro "$@"
