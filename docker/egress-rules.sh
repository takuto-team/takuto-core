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

# Allow DNS (needed for domain resolution)
iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT

# Jira / Atlassian Cloud
# Note: iptables does not support wildcards in domain names.
# Resolve the specific Atlassian endpoints your instance uses.
for host in api.atlassian.com; do
    for ip in $(dig +short "$host" 2>/dev/null || true); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
done
# Allow Atlassian IP ranges (CIDR blocks for *.atlassian.net)
# These should be customized per deployment based on your Jira Cloud instance
# See: https://support.atlassian.com/organization-administration/docs/ip-addresses-and-domains-for-atlassian-cloud-products/

# GitHub
for host in github.com api.github.com; do
    for ip in $(dig +short "$host" 2>/dev/null || true); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
done

# Anthropic (Claude API)
for host in api.anthropic.com; do
    for ip in $(dig +short "$host" 2>/dev/null || true); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
done

# Figma API
for host in api.figma.com; do
    for ip in $(dig +short "$host" 2>/dev/null || true); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
done

# npm registry (needed if Playwright or tools need runtime installs)
for host in registry.npmjs.org; do
    for ip in $(dig +short "$host" 2>/dev/null || true); do
        [ -n "$ip" ] && iptables -A OUTPUT -d "$ip" -j ACCEPT
    done
done

echo "Egress rules applied. Only allowed hosts are reachable."
