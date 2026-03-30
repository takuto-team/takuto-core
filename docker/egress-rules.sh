#!/bin/bash
# egress-rules.sh — Apply network egress allowlist
# Requires: --cap-add=NET_ADMIN on the Docker container
#
# This script restricts outbound traffic to only the services Maestro needs.
# Domain names are resolved to IPs at apply time. For dynamic resolution,
# consider using a forward proxy instead.

set -euo pipefail

echo "Applying egress allowlist rules..."

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

# Helper: resolve a domain and allow all its IPs.
# Uses getent (always available in glibc) instead of dig/host.
allow_host() {
    local host="$1"
    for ip in $(getent ahosts "$host" 2>/dev/null | awk '{ print $1 }' | sort -u); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
}

# ---------------------------------------------------------------------------
# Core services (required for Maestro to function)
# ---------------------------------------------------------------------------

# Jira / Atlassian Cloud
allow_host api.atlassian.com
# Allow the specific Jira Cloud instance used by acli.
# Read site from config.toml if available; fall back to a sensible default.
MAESTRO_CONFIG="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"
jira_site=$(sed -n 's/^[[:space:]]*site[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$MAESTRO_CONFIG" 2>/dev/null || true)
if [ -n "$jira_site" ]; then
    allow_host "$jira_site"
fi
# See: https://support.atlassian.com/organization-administration/docs/ip-addresses-and-domains-for-atlassian-cloud-products/

# GitHub
allow_host github.com
allow_host api.github.com
allow_host raw.githubusercontent.com
allow_host objects.githubusercontent.com

# Anthropic (Claude API + auth)
# Claude Code headless uses api.claude.ai for API calls, not api.anthropic.com
allow_host api.anthropic.com
allow_host api.claude.ai
allow_host claude.ai
allow_host console.anthropic.com
allow_host cdn.anthropic.com
allow_host statsig.anthropic.com
allow_host statsig.claude.ai

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
MAESTRO_CONFIG="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"
if [ -f "$MAESTRO_CONFIG" ]; then
    # Parse TOML array: extra_egress_hosts = ["host1", "host2"]
    extra_hosts=$(sed -n 's/^[[:space:]]*extra_egress_hosts[[:space:]]*=[[:space:]]*\[//p' "$MAESTRO_CONFIG" 2>/dev/null \
        | tr -d '[]"' | tr ',' '\n' | sed 's/^[[:space:]]*//' | sed 's/[[:space:]]*$//' | grep -v '^$')
    if [ -n "$extra_hosts" ]; then
        echo "Adding custom egress hosts from config..."
        echo "$extra_hosts" | while read -r host; do
            echo "  Allowing: $host"
            allow_host "$host"
        done
    fi
fi

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
# Also check worktree .npmrc files
for npmrc in /workspace/worktrees/*/.npmrc 2>/dev/null; do
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

echo "Egress rules applied. Only allowed hosts are reachable."
