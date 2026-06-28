// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Provider-aware egress allowlist.
//!
//! `docker/egress-rules.sh` applies a default-DROP iptables allowlist in the
//! main container and every worker container. Rather than hard-code one
//! vendor's API hosts, it calls `takuto egress-hosts`, which returns the hosts
//! the **configured** AI providers need: the active provider
//! (`[agent].provider`) plus every provider an admin lets users authenticate
//! against (`[agent].available_providers`), and each provider's self-hosted /
//! proxied `base_url`. OpenCode is self-hosted only, so it contributes nothing
//! beyond its `base_url`.

use std::collections::BTreeSet;

use super::Config;

// Cloud API + auth endpoints per provider. These are the documented surfaces
// the respective CLIs reach; verify against vendor docs (or a temporary
// `allow_all_https` capture) before trimming. Non-provider infrastructure
// (GitHub, Jira, package registries, Sentry) stays in egress-rules.sh.
const CLAUDE_HOSTS: &[&str] = &[
    "api.anthropic.com",
    "api.claude.ai",
    "claude.ai",
    "console.anthropic.com",
    "cdn.anthropic.com",
    "statsig.anthropic.com",
    "statsig.claude.ai",
];

const CODEX_HOSTS: &[&str] = &[
    "api.openai.com",
    "auth.openai.com",
    "chatgpt.com",
    "platform.openai.com",
];

const CURSOR_HOSTS: &[&str] = &[
    "api.cursor.com",
    "api2.cursor.sh",
    "cursor.sh",
    "repo42.cursor.sh",
    // Agent-CLI install sources: the unpinned version is parsed from
    // `cursor.com/install` and the versioned tarball is fetched from
    // `downloads.cursor.com`. Without these the boot-time agent install hangs
    // on a dropped connection once egress is enforced on the install host (the
    // npm CLIs are unaffected — they use the already-allowlisted npm registry).
    "cursor.com",
    "downloads.cursor.com",
];

impl Config {
    /// Outbound API hosts the configured AI provider(s) require, sorted and
    /// de-duplicated. Includes the active provider plus every provider in
    /// `available_providers` (users may authenticate/switch among those without
    /// an egress edit), and each of those providers' configured `base_url`
    /// host (self-hosted / proxied endpoints).
    pub fn provider_egress_hosts(&self) -> Vec<String> {
        let mut hosts: BTreeSet<String> = BTreeSet::new();

        // Active provider is always included even if an admin trimmed
        // `available_providers` to a set that excludes it.
        let mut providers: BTreeSet<String> =
            self.agent.available_providers.iter().cloned().collect();
        providers.insert(self.agent.provider.as_str().to_string());

        for name in &providers {
            match name.as_str() {
                "claude" => hosts.extend(CLAUDE_HOSTS.iter().map(|s| (*s).to_string())),
                "codex" => hosts.extend(CODEX_HOSTS.iter().map(|s| (*s).to_string())),
                "cursor" => hosts.extend(CURSOR_HOSTS.iter().map(|s| (*s).to_string())),
                // opencode: self-hosted only — no cloud endpoint.
                _ => {}
            }
            if let Some(host) = self.provider_base_url_host(name) {
                hosts.insert(host);
            }
        }

        hosts.into_iter().collect()
    }

    /// Host extracted from the named provider's `base_url`, if set.
    fn provider_base_url_host(&self, name: &str) -> Option<String> {
        let url = match name {
            "claude" => &self.agent.providers.claude.base_url,
            "codex" => &self.agent.providers.codex.base_url,
            "cursor" => &self.agent.providers.cursor.base_url,
            "opencode" => &self.agent.providers.opencode.base_url,
            _ => return None,
        };
        host_from_url(url)
    }
}

/// Extract the bare host from a `base_url` such as
/// `https://user@host.example:8080/v1` → `host.example`. Accepts scheme-less
/// values (`host:1234/v1`) too. Returns `None` for empty/host-less input.
fn host_from_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Drop scheme (`https://`), then userinfo (`user:pass@`), then keep the
    // host up to the first `/`, `:` (port) or `?`.
    let after_scheme = match s.find("://") {
        Some(i) => &s[i + 3..],
        None => s,
    };
    let after_userinfo = after_scheme.rsplit('@').next().unwrap_or(after_scheme);
    let host = after_userinfo
        .split(['/', ':', '?'])
        .next()
        .unwrap_or("")
        .trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiAgentProvider;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn default_config_includes_all_provider_clouds() {
        // Default `available_providers` is all four, so every cloud set shows up.
        let hosts = cfg().provider_egress_hosts();
        assert!(hosts.contains(&"api.anthropic.com".to_string()));
        assert!(hosts.contains(&"api.openai.com".to_string()));
        assert!(hosts.contains(&"api.cursor.com".to_string()));
    }

    #[test]
    fn codex_only_excludes_other_vendors() {
        let mut c = cfg();
        c.agent.provider = AiAgentProvider::Codex;
        c.agent.available_providers = vec!["codex".to_string()];
        let hosts = c.provider_egress_hosts();
        assert!(hosts.contains(&"api.openai.com".to_string()));
        assert!(
            !hosts
                .iter()
                .any(|h| h.contains("anthropic") || h.contains("claude"))
        );
        assert!(!hosts.iter().any(|h| h.contains("cursor")));
    }

    #[test]
    fn active_provider_included_even_if_not_in_available() {
        let mut c = cfg();
        c.agent.provider = AiAgentProvider::Cursor;
        c.agent.available_providers = vec!["claude".to_string()];
        let hosts = c.provider_egress_hosts();
        assert!(hosts.contains(&"api.cursor.com".to_string()));
        assert!(hosts.contains(&"api.anthropic.com".to_string()));
    }

    #[test]
    fn self_hosted_base_url_is_whitelisted() {
        let mut c = cfg();
        c.agent.provider = AiAgentProvider::OpenCode;
        c.agent.available_providers = vec!["opencode".to_string()];
        c.agent.providers.opencode.base_url = "http://lm-studio:1234/v1".to_string();
        let hosts = c.provider_egress_hosts();
        assert!(hosts.contains(&"lm-studio".to_string()));
    }

    #[test]
    fn host_from_url_variants() {
        assert_eq!(
            host_from_url("https://api.openai.com/v1"),
            Some("api.openai.com".into())
        );
        assert_eq!(
            host_from_url("http://lm-studio:1234/v1"),
            Some("lm-studio".into())
        );
        assert_eq!(
            host_from_url("https://user@gw.example:8443/x"),
            Some("gw.example".into())
        );
        assert_eq!(host_from_url("gw.example:1234"), Some("gw.example".into()));
        assert_eq!(host_from_url("  "), None);
        assert_eq!(host_from_url(""), None);
    }
}
