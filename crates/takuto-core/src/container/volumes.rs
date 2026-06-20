// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Volume mounts shared between takuto and every worker container.
//!
//! Holds the fixed [`WORKER_VOLUMES`] list and the [`build_volume_args`]
//! helper that swaps the broad `/workspace` mount for targeted per-issue
//! mounts when isolation is requested.

use std::path::Path;

use tracing::warn;

/// Volume mounts shared between the orchestrator and every worker container.
pub(crate) const WORKER_VOLUMES: &[&str] = &[
    "/workspace:/workspace",
    "/shared-auth/claude:/home/takuto/.claude",
    "/shared-auth/cursor:/home/takuto/.cursor",
    // npx skills add -g stores actual files in .agents/skills/; .claude/skills/ and
    // .cursor/skills/ contain symlinks pointing there, so this must be shared.
    "/shared-auth/agents:/home/takuto/.agents",
    "/shared-auth/gh:/home/takuto/.config/gh",
    "/shared-auth/acli:/home/takuto/.config/acli",
    "/shared-auth/fcli:/home/takuto/.config/fcli",
    "/shared-auth/npm:/home/takuto/.npm",
    "/shared-auth/mise-data:/home/takuto/.local/share/mise",
    "/shared-auth/mise-cache:/home/takuto/.cache/mise",
    "/shared-auth/aws:/home/takuto/.aws",
    // Playwright browser cache — must align with the repo's package.json, not a baked image path
    "/shared-auth/playwright-cache:/home/takuto/.cache/ms-playwright",
    // openvscode-server data (extensions, settings, state)
    "/shared-auth/vscode:/home/takuto/.openvscode-server",
    // Config + env for egress rules (extra_egress_hosts, .npmrc registry hosts, allow_all_https)
    "/etc/takuto:/etc/takuto:ro",
];

/// Build the list of volume mount strings for a Docker container.
///
/// When `isolate_workspace` is `true`, the broad `/workspace:/workspace` mount is
/// replaced with three targeted mounts so the container sees only:
///   - its own worktree directory (read-write)
///   - the shared `.git` internals (needed for git operations)
///   - the shared `.takuto` directory (read-only; contains `.npmrc`, etc.)
///
/// All other mounts from [`WORKER_VOLUMES`] (auth volumes, `/etc/takuto`) are preserved.
///
/// The repo root is derived as the grandparent of `worktree_path`
/// (e.g. `/workspace/worktrees/slug` → `/workspace`).
pub fn build_volume_args(worktree_path: &Path, isolate_workspace: bool) -> Vec<String> {
    let mut mounts = Vec::new();
    for v in WORKER_VOLUMES {
        if isolate_workspace && *v == "/workspace:/workspace" {
            continue;
        }
        mounts.push((*v).to_string());
    }
    if isolate_workspace {
        if let Some(repo_root) = worktree_path.parent().and_then(|p| p.parent()) {
            let wt = worktree_path.to_string_lossy();
            let root = repo_root.to_string_lossy();
            mounts.push(format!("{wt}:{wt}"));
            mounts.push(format!("{root}/.git:{root}/.git"));
            mounts.push(format!("{root}/.takuto:{root}/.takuto:ro"));
        } else {
            warn!(
                path = %worktree_path.display(),
                "Cannot derive repo root from worktree path (need grandparent); \
                 falling back to full /workspace mount"
            );
            mounts.push("/workspace:/workspace".to_string());
        }
    }
    // Mount the `takuto-tools` named volume read-only into every spawned
    // worker / editor / run-command. The takuto container
    // populates this volume at startup via the `[provisioning]` install
    // commands (see `docs/extending-takuto.md`). The volume is a Docker
    // NAMED volume — no host-path translation is needed even in DinD
    // mode because the DinD daemon and takuto share the same volume by
    // name (the takuto service mounts it RW; DinD inherits the same
    // volume via `docker-compose.dind.yml`).
    //
    // `:ro` so workers can't pollute the volume; only the takuto boot
    // pass writes to it. The `ENV PATH` in the Dockerfile prepends
    // `/opt/takuto-tools/bin` so anything dropped here shadows the
    // baked-in tools (admin's lever for pinning a tool to a specific
    // version).
    //
    // Mounted at the PARENT `/opt/takuto-tools` (not `/bin`): the runtime
    // agent install lays down `bin/` symlinks plus their targets under
    // `lib/` (npm) and `share/` (cursor), so the whole tree must be in the
    // volume or the symlinks dangle in workers.
    mounts.push("takuto-tools:/opt/takuto-tools:ro".to_string());
    mounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_volume_args_legacy_includes_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, false);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Legacy build_volume_args must include /workspace:/workspace"
        );
    }

    #[test]
    fn build_volume_args_isolated_replaces_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            !pairs.contains(&"/workspace:/workspace"),
            "Isolated build_volume_args must NOT include /workspace:/workspace"
        );
        assert!(
            pairs.contains(&"/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"),
            "Isolated build_volume_args must include worktree mount"
        );
        assert!(
            pairs.contains(&"/workspace/.git:/workspace/.git"),
            "Isolated build_volume_args must include .git mount"
        );
        assert!(
            pairs.contains(&"/workspace/.takuto:/workspace/.takuto:ro"),
            "Isolated build_volume_args must include .takuto:ro mount"
        );
    }

    #[test]
    fn build_volume_args_isolated_no_duplicate_mounts() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        // Check for duplicate entries
        let mut seen = std::collections::HashSet::new();
        for mount in &args {
            assert!(
                seen.insert(mount.as_str()),
                "Duplicate volume mount: {mount}"
            );
        }
    }

    /// Lock-in test for `build_volume_args` exact output. Pins:
    ///   1. The complete legacy mount list (isolated=false) byte-for-byte.
    ///   2. The exact isolated-mode mount list — `/workspace:/workspace`
    ///      replaced by the worktree + `.git` + `.takuto:ro` trio,
    ///      remaining mounts preserved in order, and the
    ///      `takuto-tools:/opt/takuto-tools:ro` mount appended last.
    ///
    /// Any drift in mount strings, ordering, or the isolation splice
    /// fails this test.
    #[test]
    fn lock_in_build_volume_args_legacy_and_isolated_exact_output() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");

        // Legacy: WORKER_VOLUMES in order, then takuto-tools tail.
        let legacy_expected: Vec<String> = vec![
            "/workspace:/workspace".to_string(),
            "/shared-auth/claude:/home/takuto/.claude".to_string(),
            "/shared-auth/cursor:/home/takuto/.cursor".to_string(),
            "/shared-auth/agents:/home/takuto/.agents".to_string(),
            "/shared-auth/gh:/home/takuto/.config/gh".to_string(),
            "/shared-auth/acli:/home/takuto/.config/acli".to_string(),
            "/shared-auth/fcli:/home/takuto/.config/fcli".to_string(),
            "/shared-auth/npm:/home/takuto/.npm".to_string(),
            "/shared-auth/mise-data:/home/takuto/.local/share/mise".to_string(),
            "/shared-auth/mise-cache:/home/takuto/.cache/mise".to_string(),
            "/shared-auth/aws:/home/takuto/.aws".to_string(),
            "/shared-auth/playwright-cache:/home/takuto/.cache/ms-playwright".to_string(),
            "/shared-auth/vscode:/home/takuto/.openvscode-server".to_string(),
            "/etc/takuto:/etc/takuto:ro".to_string(),
            "takuto-tools:/opt/takuto-tools:ro".to_string(),
        ];
        assert_eq!(
            build_volume_args(&wt, false),
            legacy_expected,
            "build_volume_args legacy (isolate=false) wire-format drifted"
        );

        // Isolated: drop /workspace:/workspace, keep order of remaining
        // WORKER_VOLUMES, then append worktree+.git+.takuto trio, then
        // the takuto-tools tail.
        let isolated_expected: Vec<String> = vec![
            "/shared-auth/claude:/home/takuto/.claude".to_string(),
            "/shared-auth/cursor:/home/takuto/.cursor".to_string(),
            "/shared-auth/agents:/home/takuto/.agents".to_string(),
            "/shared-auth/gh:/home/takuto/.config/gh".to_string(),
            "/shared-auth/acli:/home/takuto/.config/acli".to_string(),
            "/shared-auth/fcli:/home/takuto/.config/fcli".to_string(),
            "/shared-auth/npm:/home/takuto/.npm".to_string(),
            "/shared-auth/mise-data:/home/takuto/.local/share/mise".to_string(),
            "/shared-auth/mise-cache:/home/takuto/.cache/mise".to_string(),
            "/shared-auth/aws:/home/takuto/.aws".to_string(),
            "/shared-auth/playwright-cache:/home/takuto/.cache/ms-playwright".to_string(),
            "/shared-auth/vscode:/home/takuto/.openvscode-server".to_string(),
            "/etc/takuto:/etc/takuto:ro".to_string(),
            "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42".to_string(),
            "/workspace/.git:/workspace/.git".to_string(),
            "/workspace/.takuto:/workspace/.takuto:ro".to_string(),
            "takuto-tools:/opt/takuto-tools:ro".to_string(),
        ];
        assert_eq!(
            build_volume_args(&wt, true),
            isolated_expected,
            "build_volume_args isolated (isolate=true) wire-format drifted"
        );
    }

    #[test]
    fn build_volume_args_isolated_shallow_path_falls_back() {
        // A shallow path like `/tmp` has no grandparent, so isolation cannot
        // derive the repo root. The function should fall back to the full
        // `/workspace:/workspace` mount instead of leaving the container
        // without any workspace volume.
        let wt = PathBuf::from("/tmp");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Shallow worktree path must fall back to /workspace:/workspace"
        );
    }
}
