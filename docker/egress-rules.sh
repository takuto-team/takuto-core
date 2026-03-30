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

# Anthropic (Claude API)
allow_host api.anthropic.com

# ---------------------------------------------------------------------------
# Package registries (dependency installs)
# ---------------------------------------------------------------------------

# npm registry
allow_host registry.npmjs.org

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
