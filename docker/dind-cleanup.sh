#!/bin/bash
#
# Periodic Docker cleanup in DinD.
# Runs in background, removing dangling images, volumes, etc. every hour.
# Preserves maestro:latest to avoid requiring manual reload.

set -e

CLEANUP_INTERVAL_SECS=${CLEANUP_INTERVAL_SECS:-3600}  # 1 hour default

cleanup_dind() {
    local now=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[${now}] Running DinD cleanup..."

    # Remove dangling images (layers with no tags or dependent images)
    if docker images --filter "dangling=true" --quiet | grep -q .; then
        echo "[${now}] Removing dangling images..."
        docker rmi $(docker images --filter "dangling=true" --quiet) 2>/dev/null || true
    fi

    # Remove unused volumes (not in use by any container)
    if docker volume ls --filter "dangling=true" --quiet | grep -q .; then
        echo "[${now}] Removing dangling volumes..."
        docker volume rm $(docker volume ls --filter "dangling=true" --quiet) 2>/dev/null || true
    fi

    # Remove stopped containers older than 7 days
    echo "[${now}] Removing old stopped containers..."
    docker container prune -f --filter "until=168h" 2>/dev/null || true

    # Report space freed
    df -h /var/lib/docker | tail -1
}

# Run cleanup on schedule in background
while true; do
    sleep "$CLEANUP_INTERVAL_SECS"
    cleanup_dind
done &

# Print cleanup pid and continue
echo "[$(date '+%Y-%m-%d %H:%M:%S')] DinD cleanup daemon started (PID: $!), running every ${CLEANUP_INTERVAL_SECS}s"

# Keep this process running (docker daemon runs here)
exec "$@"
