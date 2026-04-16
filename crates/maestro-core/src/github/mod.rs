pub mod poller;

/// Parse `owner/repo` from a GitHub URL or bare `owner/repo` string.
///
/// Handles:
/// - `https://github.com/owner/repo`
/// - `https://github.com/owner/repo.git`
/// - `git@github.com:owner/repo`
/// - `owner/repo` (bare)
pub fn parse_github_repo(repo_url: &str) -> Option<String> {
    let url = repo_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if let Some(rest) = url.strip_prefix("https://github.com/")
        && rest.contains('/')
    {
        return Some(rest.to_string());
    }
    if let Some(rest) = url.strip_prefix("git@github.com:")
        && rest.contains('/')
    {
        return Some(rest.to_string());
    }
    // bare "owner/repo"
    if url.contains('/') && !url.contains("://") {
        return Some(url.to_string());
    }
    None
}
