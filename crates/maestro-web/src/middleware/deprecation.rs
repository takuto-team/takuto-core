// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Deprecation header for renamed REST paths.
//!
//! `/api/workflows/*` is renamed to `/api/work-items/*`. Both paths mount
//! the same handlers for one minor release; this middleware tags responses
//! on the legacy `/api/workflows/*` path with an `X-Maestro-Deprecation`
//! header so external callers (curl scripts, integration bots) know to
//! switch before the legacy paths are removed in the next release.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Uri};
use axum::middleware::Next;
use axum::response::Response;

const DEPRECATION_HEADER: HeaderName = HeaderName::from_static("x-maestro-deprecation");
// Note: workflow-definitions stays unchanged (the TOML *definitions* are
// still called "workflows" — only the work-item REST surface renames).
const LEGACY_PREFIX: &str = "/api/workflows";

/// Tag responses on `/api/workflows/*` (excluding `/api/workflow-definitions`)
/// with `X-Maestro-Deprecation`.
pub async fn deprecation_header_middleware(req: Request, next: Next) -> Response {
    let path_matches = is_legacy_workflows_path(req.uri());
    let mut resp = next.run(req).await;
    if path_matches
        && let Ok(value) = HeaderValue::from_str(
            "path renamed; use /api/work-items/* (removal targeted for the next minor release)",
        )
    {
        resp.headers_mut().insert(DEPRECATION_HEADER, value);
    }
    resp
}

fn is_legacy_workflows_path(uri: &Uri) -> bool {
    // `/api/workflows` itself, or any `/api/workflows/...` — but NOT
    // `/api/workflow-definitions` (different resource, not renamed).
    let path = uri.path();
    path == LEGACY_PREFIX || path.starts_with("/api/workflows/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_legacy_root() {
        assert!(is_legacy_workflows_path(&Uri::from_static(
            "/api/workflows"
        )));
    }

    #[test]
    fn matches_legacy_subpaths() {
        for p in [
            "/api/workflows/counts",
            "/api/workflows/abc",
            "/api/workflows/abc/pause",
            "/api/workflows/abc/run-commands/0/start",
        ] {
            assert!(is_legacy_workflows_path(&Uri::from_str_panic(p)), "{p}");
        }
    }

    #[test]
    fn does_not_match_new_paths() {
        for p in [
            "/api/work-items",
            "/api/work-items/abc",
            "/api/work-items/abc/pause",
        ] {
            assert!(!is_legacy_workflows_path(&Uri::from_str_panic(p)), "{p}");
        }
    }

    #[test]
    fn does_not_match_workflow_definitions() {
        // `workflow_definitions` refers to TOML pipeline definitions
        // (which stay named "workflows"). Only the work-item REST surface
        // renames.
        assert!(!is_legacy_workflows_path(&Uri::from_static(
            "/api/workflow-definitions"
        )));
    }

    #[test]
    fn does_not_match_unrelated_paths() {
        for p in ["/api/auth/status", "/api/users", "/api/config"] {
            assert!(!is_legacy_workflows_path(&Uri::from_str_panic(p)), "{p}");
        }
    }

    /// Helper because `Uri::from_static` requires a `'static` arg.
    trait UriExt {
        fn from_str_panic(s: &str) -> Uri;
    }
    impl UriExt for Uri {
        fn from_str_panic(s: &str) -> Uri {
            s.parse().unwrap()
        }
    }
}
