#!/bin/bash
# egress-rules.sh — Apply (and optionally keep refreshing) a default-DROP
# network egress allowlist for a Takuto container.
#
# Requires: --cap-add=NET_ADMIN on the container.
#
# Design:
#   - Base rules (DROP policy, loopback, conntrack, DNS, DinD/DB sidecars,
#     allow_all_https) live directly in OUTPUT and are applied idempotently
#     (check-before-add), so this script is safe to run repeatedly — at boot
#     via the entrypoint AND on demand via `docker exec`.
#   - Every *resolved-host* ACCEPT lives in a dedicated chain. ALL allowlisted
#     hosts (not just GitHub) are resolved to IPs each pass; GitHub additionally
#     contributes its published CIDR ranges.
#   - When TAKUTO_EGRESS_REFRESH=1, a detached background loop re-resolves every
#     host every TAKUTO_EGRESS_REFRESH_SECS (default 300) and atomically swaps
#     the live chain, so DNS/IP rotation is handled at runtime. The swap appends
#     the new chain's jump before removing the old one, so there is no
#     new-connection DROP window (a sub-millisecond over-allow window instead).
#   - IPv6 is default-DROP too (the allowlist is IPv4; without this a working v6
#     route would bypass the whole policy).
set -euo pipefail

TAKUTO_CONFIG="${TAKUTO_CONFIG:-/etc/takuto/config.toml}"
TAKUTO_BIN="${TAKUTO_BIN:-/usr/local/bin/takuto}"
EGRESS_REFRESH="${TAKUTO_EGRESS_REFRESH:-0}"
PIDFILE=/run/takuto-egress-refresh.pid
LOGFILE=/run/takuto-egress-refresh.log
# Two chains we flip between for atomic refresh.
CHAIN_A=TAKUTO_EGRESS_A
CHAIN_B=TAKUTO_EGRESS_B

# Refresh interval — validated numeric, floor 30s.
REFRESH_SECS="${TAKUTO_EGRESS_REFRESH_SECS:-300}"
case "$REFRESH_SECS" in
    ''|*[!0-9]*) REFRESH_SECS=300 ;;
esac
[ "$REFRESH_SECS" -lt 30 ] 2>/dev/null && REFRESH_SECS=30

log() { echo "[egress] $*"; }

# ── Resolution helpers ─────────────────────────────────────────────────────
# Emit (on stdout) one ip/cidr per line for a hostname. IPv4 only — iptables
# here is v4 and an IPv6 address is a fatal error.
resolve_host() {
    getent ahostsv4 "$1" 2>/dev/null | awk '{ print $1 }' | sort -u
}

# Echo every allowlisted host (one per line), resolved from config + sources.
# Pure stdout — no iptables side effects — so it can run before a flush.
collect_host_ips() {
    {
        # Jira / Atlassian
        resolve_host api.atlassian.com
        resolve_host developer.atlassian.com
        # acli is ALWAYS installed (ticketing can switch at runtime); its binary
        # is fetched from acli.atlassian.com, so the boot-time install needs it
        # allowlisted or the install hangs on a dropped connection.
        resolve_host acli.atlassian.com
        jira_site=$(sed -n 's/^[[:space:]]*site[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$TAKUTO_CONFIG" 2>/dev/null || true)
        [ -n "$jira_site" ] && resolve_host "$jira_site"

        # GitHub hostnames (kept every pass as a fallback for when the meta
        # fetch fails — so one failed fetch can never strand the loop).
        resolve_host github.com
        resolve_host api.github.com
        resolve_host raw.githubusercontent.com
        resolve_host objects.githubusercontent.com
        resolve_host codeload.github.com
        resolve_host docs.github.com

        # AI provider API endpoints (active provider + every available_provider
        # + any self-hosted base_url), resolved by the takuto binary.
        if [ -x "$TAKUTO_BIN" ]; then
            while IFS= read -r h; do
                [ -n "$h" ] && resolve_host "$h"
            done < <("$TAKUTO_BIN" --config "$TAKUTO_CONFIG" egress-hosts 2>/dev/null || true)
        fi

        # Sentry
        resolve_host sentry.io
        resolve_host o0.ingest.sentry.io
        resolve_host o1.ingest.sentry.io
        resolve_host o2.ingest.sentry.io

        # Custom hosts (config [network] extra_egress_hosts)
        if [ -f "$TAKUTO_CONFIG" ]; then
            sed -n 's/^[[:space:]]*extra_egress_hosts[[:space:]]*=[[:space:]]*\[//p' "$TAKUTO_CONFIG" 2>/dev/null \
                | tr -d '[]"' | tr ',' '\n' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | grep -v '^$' \
                | while read -r h; do resolve_host "$h"; done
        fi

        # AWS (CodeArtifact / STS / SSO)
        resolve_host sts.amazonaws.com
        resolve_host sts.ap-northeast-1.amazonaws.com
        resolve_host portal.sso.ap-northeast-1.amazonaws.com

        # Package registries
        resolve_host registry.npmjs.org
        resolve_host static.rust-lang.org
        resolve_host crates.io
        # npm registries declared in .npmrc files (workspace + worktrees)
        for npmrc in /workspace/.npmrc $(find /workspace/worktrees -maxdepth 24 -name .npmrc 2>/dev/null); do
            [ -f "$npmrc" ] || continue
            for rhost in $(grep -oP 'https?://\K[^/]+' "$npmrc" 2>/dev/null | sort -u); do
                resolve_host "$rhost"
            done
        done

        # Documentation
        resolve_host docs.rs
        resolve_host doc.rust-lang.org
        resolve_host developer.mozilla.org
        resolve_host nodejs.org
        resolve_host stackoverflow.com

        # Figma (optional)
        resolve_host api.figma.com
    } | sort -u | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'
}

# GitHub published IPv4 CIDR ranges (covers the rotating Fastly CDN pool).
# Fetched while egress is still open enough to reach github (the previous pass's
# rules, or the fully-open boot state). Empty on failure — the getent fallback
# above still covers github.
collect_github_cidrs() {
    curl -fsS --max-time "${1:-5}" https://api.github.com/meta 2>/dev/null \
        | jq -r '(.api // []) + (.git // []) + (.web // []) + (.hooks // []) | .[]' 2>/dev/null \
        | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$' | sort -u || true
}

# ── Chain population + atomic swap ──────────────────────────────────────────
# Build the *inactive* chain from freshly-resolved targets, then flip the
# OUTPUT jump to it (append-new-before-remove-old → no DROP window), and flush
# the now-old chain. $ACTIVE_CHAIN tracks which chain OUTPUT currently jumps to.
ACTIVE_CHAIN=""
populate_and_swap() {
    local curl_timeout="${1:-5}"
    local inactive
    if [ "$ACTIVE_CHAIN" = "$CHAIN_A" ]; then inactive="$CHAIN_B"; else inactive="$CHAIN_A"; fi

    # Resolve EVERYTHING first (no iptables mutation yet) so the github meta
    # fetch runs while the current rules still allow github.
    local cidrs ips
    cidrs="$(collect_github_cidrs "$curl_timeout")"
    ips="$(collect_host_ips)"

    iptables -F "$inactive"
    local t
    while IFS= read -r t; do
        [ -n "$t" ] && iptables -A "$inactive" -d "$t" -j ACCEPT
    done < <(printf '%s\n%s\n' "$cidrs" "$ips" | grep -v '^$' | sort -u)

    # Flip: add the new jump, then remove the old. Brief over-allow, never a gap.
    iptables -C OUTPUT -j "$inactive" 2>/dev/null || iptables -A OUTPUT -j "$inactive"
    if [ -n "$ACTIVE_CHAIN" ] && [ "$ACTIVE_CHAIN" != "$inactive" ]; then
        iptables -D OUTPUT -j "$ACTIVE_CHAIN" 2>/dev/null || true
        iptables -F "$ACTIVE_CHAIN" 2>/dev/null || true
    fi
    ACTIVE_CHAIN="$inactive"
}

# ── Base rules (idempotent) ─────────────────────────────────────────────────
apply_base_rules() {
    # Create both chains (ignore "already exists").
    iptables -N "$CHAIN_A" 2>/dev/null || true
    iptables -N "$CHAIN_B" 2>/dev/null || true

    iptables -P OUTPUT DROP
    iptables -C OUTPUT -o lo -j ACCEPT 2>/dev/null || iptables -A OUTPUT -o lo -j ACCEPT
    iptables -C OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT 2>/dev/null \
        || iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

    # DNS (auto-detected resolvers) — must stay in OUTPUT so resolution keeps
    # working across chain flushes.
    for dns_ip in $(awk '/^nameserver/ { print $2 }' /etc/resolv.conf 2>/dev/null); do
        iptables -C OUTPUT -d "$dns_ip" -p udp --dport 53 -j ACCEPT 2>/dev/null \
            || iptables -A OUTPUT -d "$dns_ip" -p udp --dport 53 -j ACCEPT
        iptables -C OUTPUT -d "$dns_ip" -p tcp --dport 53 -j ACCEPT 2>/dev/null \
            || iptables -A OUTPUT -d "$dns_ip" -p tcp --dport 53 -j ACCEPT
    done

    # DinD sidecar (DOCKER_HOST tcp://host:port).
    if [ -n "${DOCKER_HOST:-}" ]; then
        dind_host=$(echo "$DOCKER_HOST" | sed -n 's|^tcp://\([^:]*\):\([0-9]*\)$|\1|p')
        dind_port=$(echo "$DOCKER_HOST" | sed -n 's|^tcp://\([^:]*\):\([0-9]*\)$|\2|p')
        if [ -n "$dind_host" ] && [ -n "$dind_port" ]; then
            for ip in $(resolve_host "$dind_host"); do
                iptables -C OUTPUT -d "$ip" -p tcp --dport "$dind_port" -j ACCEPT 2>/dev/null \
                    || iptables -A OUTPUT -d "$ip" -p tcp --dport "$dind_port" -j ACCEPT
            done
        fi
    fi

    # External DB sidecar (postgres/mysql) from env or config.
    local db_url="${TAKUTO_DATABASE_CONNECTION:-}"
    [ -z "$db_url" ] && db_url=$(sed -n 's/^[[:space:]]*connection[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$TAKUTO_CONFIG" 2>/dev/null | head -1 || true)
    case "$db_url" in
        postgres://*|postgresql://*|mysql://*)
            local hp host port
            hp=$(echo "$db_url" | sed -e 's|^[a-z]*://||' -e 's|^[^@]*@||' -e 's|[/?].*$||')
            host="${hp%%:*}"; port="${hp##*:}"
            if [ "$host" = "$port" ]; then
                case "$db_url" in postgres*) port=5432 ;; mysql*) port=3306 ;; esac
            fi
            if [ -n "$host" ] && [ -n "$port" ]; then
                for ip in $(resolve_host "$host"); do
                    iptables -C OUTPUT -d "$ip" -p tcp --dport "$port" -j ACCEPT 2>/dev/null \
                        || iptables -A OUTPUT -d "$ip" -p tcp --dport "$port" -j ACCEPT
                done
            fi
            ;;
    esac

    # Escape hatch: allow all outbound HTTPS.
    local allow_all
    allow_all=$(sed -n 's/^[[:space:]]*allow_all_https[[:space:]]*=[[:space:]]*\(.*\)/\1/p' "$TAKUTO_CONFIG" 2>/dev/null | tr -d ' "' || true)
    if [ "$allow_all" = "true" ]; then
        log "WARNING: allow_all_https is enabled — all outbound HTTPS (port 443) is permitted"
        iptables -C OUTPUT -p tcp --dport 443 -j ACCEPT 2>/dev/null \
            || iptables -A OUTPUT -p tcp --dport 443 -j ACCEPT
    fi

    # IPv6: default-DROP (allowlist is IPv4 only). Best-effort; ip6tables may be
    # absent. Without this a working v6 route bypasses the entire policy.
    if command -v ip6tables >/dev/null 2>&1; then
        ip6tables -P OUTPUT DROP 2>/dev/null || true
        ip6tables -C OUTPUT -o lo -j ACCEPT 2>/dev/null || ip6tables -A OUTPUT -o lo -j ACCEPT 2>/dev/null || true
        ip6tables -C OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT 2>/dev/null \
            || ip6tables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT 2>/dev/null || true
        for dns_ip6 in $(awk '/^nameserver/ { print $2 }' /etc/resolv.conf 2>/dev/null | grep ':' || true); do
            ip6tables -C OUTPUT -d "$dns_ip6" -p udp --dport 53 -j ACCEPT 2>/dev/null \
                || ip6tables -A OUTPUT -d "$dns_ip6" -p udp --dport 53 -j ACCEPT 2>/dev/null || true
        done
    fi
}

# Which chain is OUTPUT currently jumping to (so a `--refresh-once` re-invocation
# is stateless — it reads live iptables rather than carrying shell state).
detect_active_chain() {
    if iptables -C OUTPUT -j "$CHAIN_A" 2>/dev/null; then
        echo "$CHAIN_A"
    elif iptables -C OUTPUT -j "$CHAIN_B" 2>/dev/null; then
        echo "$CHAIN_B"
    else
        echo ""
    fi
}

# ── Background refresh loop ─────────────────────────────────────────────────
# The loop just re-invokes THIS script in `--refresh-once` mode every interval,
# which avoids embedding function definitions into a nested shell string.
start_refresh_loop() {
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; then
        log "refresh loop already running (pid $(cat "$PIDFILE"))"
        return 0
    fi
    log "starting egress refresh loop (every ${REFRESH_SECS}s)"
    # `setsid` + redirected stdio detaches the loop so it survives the
    # `docker exec` session / entrypoint `exec` and is reparented to PID 1.
    setsid bash -c "
        echo \$\$ > '$PIDFILE'
        trap 'rm -f \"$PIDFILE\"' EXIT
        while sleep '$REFRESH_SECS'; do
            : > '$LOGFILE'
            '$0' --refresh-once >> '$LOGFILE' 2>&1 || echo '[egress] refresh pass failed' >> '$LOGFILE'
        done
    " </dev/null >>"$LOGFILE" 2>&1 &
    disown 2>/dev/null || true
}

# ── Main ────────────────────────────────────────────────────────────────────
# `--refresh-once`: re-resolve + swap against the already-applied base rules,
# then exit. Used by the background loop.
if [ "${1:-}" = "--refresh-once" ]; then
    ACTIVE_CHAIN="$(detect_active_chain)"
    populate_and_swap 5
    exit 0
fi

log "applying egress allowlist..."
apply_base_rules
# First pass uses a longer curl budget (cold) but stays bounded.
populate_and_swap 10
log "egress allowlist applied (active chain: $ACTIVE_CHAIN)"

if [ "$EGRESS_REFRESH" = "1" ]; then
    start_refresh_loop
fi
