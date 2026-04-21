// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Build Jira **browse** URLs from **`[jira] site`** and a ticket key (dashboard links).

/// Returns `https://…/browse/TICKET` using the same rules as operators expect from **`[jira] site`**
/// (host-only, `https://` prefix, optional context path before `/browse/…`).
pub fn ticket_browse_url(site: &str, ticket_key: &str) -> String {
    let mut s = site.trim();
    if s.is_empty() {
        return format!("https://jira.atlassian.net/browse/{ticket_key}");
    }
    if let Some(rest) = s.strip_prefix("https://") {
        s = rest;
    } else if let Some(rest) = s.strip_prefix("http://") {
        s = rest;
    }
    let s = s.trim().trim_end_matches('/');
    if s.is_empty() {
        return format!("https://jira.atlassian.net/browse/{ticket_key}");
    }
    format!("https://{s}/browse/{ticket_key}")
}

#[cfg(test)]
mod tests {
    use super::ticket_browse_url;

    #[test]
    fn empty_site_uses_legacy_atlassian_host() {
        assert_eq!(
            ticket_browse_url("", "PROJ-1"),
            "https://jira.atlassian.net/browse/PROJ-1"
        );
        assert_eq!(
            ticket_browse_url("   ", "PROJ-1"),
            "https://jira.atlassian.net/browse/PROJ-1"
        );
    }

    #[test]
    fn host_only_site() {
        assert_eq!(
            ticket_browse_url("acme.atlassian.net", "CORE-42"),
            "https://acme.atlassian.net/browse/CORE-42"
        );
    }

    #[test]
    fn site_with_https_prefix_and_trailing_slash() {
        assert_eq!(
            ticket_browse_url("https://acme.atlassian.net/", "X-9"),
            "https://acme.atlassian.net/browse/X-9"
        );
    }

    #[test]
    fn site_with_context_path() {
        assert_eq!(
            ticket_browse_url("https://jira.corp.example.com/jira", "BUG-1"),
            "https://jira.corp.example.com/jira/browse/BUG-1"
        );
    }
}
