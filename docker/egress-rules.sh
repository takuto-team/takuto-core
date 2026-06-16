#!/bin/bash
# egress-rules.sh — Apply network egress allowlist
# Requires: --cap-add=NET_ADMIN on the Docker container
#
# This script restricts outbound traffic to only the services Takuto needs.
# Domain names are resolved to IPs at apply time. For dynamic resolution,
# consider using a forward proxy instead.

set -euo pipefail

echo "Applying egress allowlist rules..."

# CDN-backed GitHub hosts (api.github.com, github.com, codeload, …) sit behind
# Fastly and resolve to a large, frequently-rotating IPv4 set. Pinning a single
# boot-time A record (see allow_host below) is therefore unreliable: `gh` and
# `git` routinely open a connection to an address that was never whitelisted,
# the packet is DROPped, and the call fails at the transport layer
# (gh_transport_error on PAT save; clone/push hangs then errors).
#
# GitHub publishes its current ranges at https://api.github.com/meta. Fetch them
# HERE — before the default-DROP policy below is in effect, while egress is
# still fully open — and stash the IPv4 CIDRs for the GitHub section to allow.
# A failure (offline, schema change, no jq) leaves the variable empty and we
# fall back to the boot-time allow_host resolution, so this never makes egress
# stricter than before.
GH_META_CIDRS=$(curl -fsS --max-time 10 https://api.github.com/meta 2>/dev/null \
    | jq -r '(.api // []) + (.git // []) + (.web // []) + (.hooks // []) | .[]' 2>/dev/null \
    | grep -v ':' | sort -u || true)
if [ -n "$GH_META_CIDRS" ]; then
    echo "Fetched $(echo "$GH_META_CIDRS" | wc -l | tr -d ' ') GitHub IPv4 CIDR ranges from api.github.com/meta"
else
    echo "WARNING: could not fetch GitHub IP ranges from api.github.com/meta — falling back to boot-time DNS resolution for GitHub hosts" >&2
fi

# Default policy: drop all outbound
iptables -P OUTPUT DROP

# Allow loopback
iptables -A OUTPUT -o lo -j ACCEPT

# Allow established/related connections (responses to allowed requests)
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

# Allow DNS — auto-detect resolver from /etc/resolv.conf.
# Works with both Docker (127.0.0.11) and Podman (e.g. 10.89.x.1).
for dns_ip in $(awk '/^nameserver/ { print $2 }' /etc/resolv.conf); do
    iptables -A OUTPUT -d "$dns_ip" -p udp --dport 53 -j ACCEPT
    iptables -A OUTPUT -d "$dns_ip" -p tcp --dport 53 -j ACCEPT
done

# Allow Docker-in-Docker sidecar when DOCKER_HOST is set (e.g. tcp://dind:2375).
# Extract host and port from DOCKER_HOST and resolve to IP.
if [ -n "${DOCKER_HOST:-}" ]; then
    dind_host=$(echo "$DOCKER_HOST" | sed -n 's|^tcp://\([^:]*\):\([0-9]*\)$|\1|p')
    dind_port=$(echo "$DOCKER_HOST" | sed -n 's|^tcp://\([^:]*\):\([0-9]*\)$|\2|p')
    if [ -n "$dind_host" ] && [ -n "$dind_port" ]; then
        echo "Allowing DinD sidecar: $dind_host:$dind_port"
        for ip in $(getent ahostsv4 "$dind_host" 2>/dev/null | awk '{ print $1 }' | sort -u); do
            [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -p tcp --dport "$dind_port" -j ACCEPT
        done
    fi
fi
# Helper: resolve a domain and allow all its IPs.
# Uses getent (always available in glibc) instead of dig/host.
allow_host() {
    local host="$1"
    # Use ahostsv4 — iptables is IPv4 only; IPv6 addresses cause a fatal error.
    for ip in $(getent ahostsv4 "$host" 2>/dev/null | awk '{ print $1 }' | sort -u); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
}

# Helper: allow a single IPv4 CIDR block (e.g. 140.82.112.0/20). Used for the
# GitHub published ranges fetched above — these cover the whole rotating CDN
# pool, not just one boot-time A record. IPv6 CIDRs are skipped (iptables here
# is IPv4 only); the caller already filters them, this is belt-and-braces.
allow_cidr() {
    local cidr="$1"
    case "$cidr" in
        *:*) return 0 ;;
    esac
    [ -n "$cidr" ] && iptables -A OUTPUT -d "$cidr" -j ACCEPT
}

# ---------------------------------------------------------------------------
# Core services (required for Takuto to function)
# ---------------------------------------------------------------------------

# Jira / Atlassian Cloud
allow_host api.atlassian.com
# Allow the specific Jira Cloud instance used by acli.
# Read site from config.toml if available; fall back to a sensible default.
TAKUTO_CONFIG="${TAKUTO_CONFIG:-/etc/takuto/config.toml}"
jira_site=$(sed -n 's/^[[:space:]]*site[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$TAKUTO_CONFIG" 2>/dev/null || true)
if [ -n "$jira_site" ]; then
    allow_host "$jira_site"
fi
# See: https://support.atlassian.com/organization-administration/docs/ip-addresses-and-domains-for-atlassian-cloud-products/

# GitHub — allow the published IP ranges first (covers the full rotating CDN
# pool so gh/git connections don't land on an un-whitelisted Fastly edge), then
# keep the boot-time DNS resolution as a fallback for when the meta fetch failed
# and for hosts not covered by the meta ranges.
if [ -n "$GH_META_CIDRS" ]; then
    while IFS= read -r gh_cidr; do
        [ -n "$gh_cidr" ] && allow_cidr "$gh_cidr"
    done <<< "$GH_META_CIDRS"
fi
allow_host github.com
allow_host api.github.com
allow_host raw.githubusercontent.com
allow_host objects.githubusercontent.com

# AI provider API endpoints — resolved from config by `takuto egress-hosts`,
# which returns the hosts for the active provider (`[agent].provider`) plus
# every provider in `[agent].available_providers`, and any self-hosted
# `base_url`. This replaces the old hard-coded Anthropic-only block so Codex
# (OpenAI) and Cursor work out of the box too. With the default config (all
# four providers available) every supported vendor's hosts are allowed; trim
# `available_providers` to narrow the firewall to the providers you use.
TAKUTO_BIN="${TAKUTO_BIN:-/usr/local/bin/takuto}"
if [ -x "$TAKUTO_BIN" ]; then
    while IFS= read -r provider_host; do
        [ -n "$provider_host" ] || continue
        echo "Allowing AI provider host: $provider_host"
        allow_host "$provider_host"
    done < <("$TAKUTO_BIN" --config "$TAKUTO_CONFIG" egress-hosts 2>/dev/null || true)
else
    echo "WARNING: takuto binary not found at $TAKUTO_BIN — AI provider egress hosts not applied" >&2
fi

# Sentry error reporting — Claude Code uses subdomain-based ingest endpoints
allow_host sentry.io
allow_host o0.ingest.sentry.io
allow_host o1.ingest.sentry.io
allow_host o2.ingest.sentry.io

# ---------------------------------------------------------------------------
# Package registries (dependency installs)
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Custom egress hosts (from config.toml [network] extra_egress_hosts)
# ---------------------------------------------------------------------------
TAKUTO_CONFIG="${TAKUTO_CONFIG:-/etc/takuto/config.toml}"
if [ -f "$TAKUTO_CONFIG" ]; then
    # Parse TOML array: extra_egress_hosts = ["host1", "host2"]
    extra_hosts=$(sed -n 's/^[[:space:]]*extra_egress_hosts[[:space:]]*=[[:space:]]*\[//p' "$TAKUTO_CONFIG" 2>/dev/null \
        | tr -d '[]"' | tr ',' '\n' | sed 's/^[[:space:]]*//' | sed 's/[[:space:]]*$//' | grep -v '^$' || true)
    if [ -n "$extra_hosts" ]; then
        echo "Adding custom egress hosts from config..."
        echo "$extra_hosts" | while read -r host; do
            echo "  Allowing: $host"
            allow_host "$host"
        done
    fi
fi

# AWS (for CodeArtifact, STS, SSO)
allow_host sts.amazonaws.com
allow_host sts.ap-northeast-1.amazonaws.com
allow_host portal.sso.ap-northeast-1.amazonaws.com
# Allow all CodeArtifact endpoints (resolved dynamically from config)
# The specific CodeArtifact domain is added via extra_egress_hosts in config.toml

# npm registry
allow_host registry.npmjs.org

# AWS CodeArtifact (private npm registries)
# Read from .npmrc in the workspace if available
NPMRC="/workspace/.npmrc"
if [ -f "$NPMRC" ]; then
    for registry_host in $(grep -oP 'https?://\K[^/]+' "$NPMRC" 2>/dev/null | sort -u); do
        echo "Allowing npm registry host from .npmrc: $registry_host"
        allow_host "$registry_host"
    done
fi
# Also check worktree .npmrc files (bounded depth — unbounded find can stall on huge trees)
for npmrc in $(find /workspace/worktrees -maxdepth 24 -name .npmrc 2>/dev/null); do
    if [ -f "$npmrc" ]; then
        for registry_host in $(grep -oP 'https?://\K[^/]+' "$npmrc" 2>/dev/null | sort -u); do
            echo "Allowing npm registry host from $npmrc: $registry_host"
            allow_host "$registry_host"
        done
    fi
done

# Rust / Cargo
allow_host static.rust-lang.org
allow_host crates.io

# ---------------------------------------------------------------------------
# Documentation (read-only, helps the agent resolve issues)
# ---------------------------------------------------------------------------

allow_host docs.rs
allow_host doc.rust-lang.org
allow_host developer.mozilla.org
allow_host nodejs.org
allow_host docs.github.com
allow_host developer.atlassian.com
allow_host stackoverflow.com

# ---------------------------------------------------------------------------
# Optional — Figma (only needed if Figma integration is active)
# ---------------------------------------------------------------------------

allow_host api.figma.com

# Fallback: if any resolved host had 0 IPs (cloud providers with rotating IPs),
# allow all HTTPS as a safety net. This is a temporary measure until a
# DNS-based proxy (squid) is implemented.
ALLOW_ALL_HTTPS=$(sed -n 's/^[[:space:]]*allow_all_https[[:space:]]*=[[:space:]]*\(.*\)/\1/p' "$TAKUTO_CONFIG" 2>/dev/null | tr -d ' "' || true)
if [ "$ALLOW_ALL_HTTPS" = "true" ]; then
    echo "WARNING: allow_all_https is enabled — all outbound HTTPS (port 443) is permitted"
    iptables -A OUTPUT -p tcp --dport 443 -j ACCEPT
fi

# Plan-11: when Takuto is configured to talk to an external database
# (postgres / mysql / mariadb sidecar), allow that host:port pair.
# Otherwise the default DROP policy silently times out the sqlx pool
# at 30 s — surfacing as "Database backend unreachable" at startup.
#
# `TAKUTO_DATABASE_CONNECTION` env var wins (matches the compose
# overlays); fall back to `[database].connection` in config.toml so
# operators who set the URL there get the same allowance.
DB_URL="${TAKUTO_DATABASE_CONNECTION:-}"
if [ -z "$DB_URL" ]; then
    DB_URL=$(sed -n 's/^[[:space:]]*connection[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$TAKUTO_CONFIG" 2>/dev/null | head -1 || true)
fi
case "$DB_URL" in
    postgres://*|postgresql://*|mysql://*)
        # Strip scheme + userinfo: leaves `host[:port]/dbname?…`.
        HOST_PORT=$(echo "$DB_URL" | sed -e 's|^[a-z]*://||' -e 's|^[^@]*@||' -e 's|[/?].*$||')
        DB_HOST="${HOST_PORT%%:*}"
        DB_PORT="${HOST_PORT##*:}"
        # No explicit port → infer from scheme.
        if [ "$DB_HOST" = "$DB_PORT" ]; then
            case "$DB_URL" in
                postgres*) DB_PORT=5432 ;;
                mysql*)    DB_PORT=3306 ;;
            esac
        fi
        if [ -n "$DB_HOST" ] && [ -n "$DB_PORT" ]; then
            echo "Allowing database sidecar: ${DB_HOST}:${DB_PORT}"
            for ip in $(getent ahostsv4 "$DB_HOST" 2>/dev/null | awk '{ print $1 }' | sort -u); do
                [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -p tcp --dport "$DB_PORT" -j ACCEPT
            done
        fi
        ;;
esac

echo "Egress rules applied. Only allowed hosts are reachable."
