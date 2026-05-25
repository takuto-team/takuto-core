# Refactor spec — typed `GitError` sub-enum (phase 6)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes A.1 row 6: carve `MaestroError::Git(String)` into `GitError`.

## 1. Module layout — `git/error.rs` + `mod.rs` re-export

`git/` is already a directory (`pr.rs`, `remote.rs`, `worktree.rs`, `worktree_remove.rs`). Add `git/error.rs`; declare via `pub mod error; pub use error::GitError;` in `mod.rs`. The 21 `MaestroError::Git(format!(...))` call sites span 5 files — `actions/{real,dry_run,gh_github}.rs` and `git/worktree_remove.rs` and `workflow/engine/bootstrap.rs` — so the canonical home is `git/error.rs` because the variants are "things that fail when invoking git or `gh` against a worktree", which is the `git/` subsystem's domain plus the side-effecting `actions/` impls.

## 2. `GitError` definition — 14 variants

Lands at `crates/maestro-core/src/git/error.rs`. Operation clusters:
- **Worktree create / fetch / delete** (3) — `FetchBaseBranchFailed { base, stderr }`, `WorktreeCreateFailed { stderr }`, `DeleteBranchFailed { branch, stderr }`. Same shape for `RealActions` and `DryRunActions`.
- **Worktree remove** (3) — `WorktreePathInvalidUtf8 { path: PathBuf }`, `WorktreeRemoveFailed { stderr }` (both `git worktree remove --force` paths share this), `WorktreeDirRemoveFailed { path: PathBuf, #[source] io: std::io::Error, git_err: String }` (the `fs::remove_dir_all` fallback when git reports the tree unregistered).
- **`gh` CLI** (5) — `GhApiUserFailed { stderr }`, `GhApiUserParseJson(#[from] serde_json::Error)`, `GhApiUserEmptyLogin`, `EmptyPrUrl`, `GhPrAddReviewerFailed { stderr }`.
- **`git config`** (1, collapsed) — `GitConfigFailed { setting: &'static str, stderr }` covers both `user.name` and `user.email` sites with `setting: "name" | "email"` pinned at the call site. Matches the GitHubApp / Jira collapsed pattern.
- **Workflow-engine bootstrap steps** (2) — `MiseInstallFailed { exit_code, stderr_tail }`, `WorktreeInitCommandFailed { step_name, exit_code, stderr_tail, stdout_tail }`.

## 3. Foreign `#[from]` decisions

- `serde_json::Error` → `#[from]` on `GhApiUserParseJson`. Single-site (only `gh_github.rs:54`), no collision with any other variant — the architecture rule "Foreign error wrapped → `#[from] source` on a single variant per source type" applies cleanly. Diverges from JiraError's all-`#[source]` rationale because JiraError has 4 parse sites for the same `serde_json::Error` type; GitError has 1.
- `std::io::Error` → `#[source]` on `WorktreeDirRemoveFailed`. Avoids collision with `MaestroError::Io(#[from] std::io::Error)` envelope (same rationale as GitHubAppError).

## 4. Migration plan — 2 commits + lock-in (in C1)

- **C1** `refactor(git): C1 — land GitError + envelope + rename Git → GitStr` — define `GitError`, add `MaestroError::Git(#[from] GitError)`, rename `Git(String)` → `GitStr(String)` with `#[deprecated]`, mechanically sed all 21 sites to `::GitStr(`, gated under function-level `#[allow(deprecated)]` on the 10 enclosing fns (`create_worktree`×2, `delete_local_branch`×2, `fetch_gh_user`, `apply_git_identity_from_gh`, `gh_request_self_pr_reviewer`, `remove_git_worktree`, `clear_worktree_path_for_recreate`, `bootstrap_new_workflow`). Add 2 lock-in tests (Display + From impl, both with `cases.len() == 14` drift assertion).
- **C2** `refactor(git): C2 — migrate git/, actions/, bootstrap to GitError` — 19 sites become typed variants, 2 collapse to direct propagation (`bootstrap.rs:553,658` — both wrap an inner `MaestroError` with a zero-info `"<step> error: {e}"` prefix; `step_log.fail()` keeps the formatted message inline, the Err return drops the prefix), attrs from C1 removed.

The lock-in tests landed in C1 alongside the type definition (same precedent as the agent phase — small surface, single test commit would have been redundant).

## 5. Acceptance criteria

- [x] `cargo build --workspace` produces no NEW warnings beyond the 5 phase-1 `DatabaseStr` carryover.
- [x] `cargo test --workspace --lib --tests` matches baseline (1036) + 2 lock-in tests = 1038.
- [x] Zero `MaestroError::Git(` / `MaestroError::GitStr(` constructions remain under `crates/maestro-core/src/` (one matches!() pattern in the test assertion remains).
- [x] `MaestroError::GitStr(String)` retained as `#[deprecated]` shim — removed by the post-§8 #2 cleanup PR.
- [x] No HTTP API response shape changes. Only logic change is the 2 collapsed bootstrap sites — `step_log.fail()` now captures the formatted prefix, the Err propagation drops it.

## 6. Risks

1. **Display drift on the 2 collapsed bootstrap sites.** Previously `"mise install error: <inner>"` / `"<step_name> error: <inner>"`; now the inner `MaestroError`'s Display surfaces directly when the propagated error reaches a handler / log call. Operator-visible message in `step_log.fail()` is preserved (still gets the prefixed form). Verified by grep — no workspace tests string-match the old `"mise install error:"` / `"<step> error:"` patterns.
2. **`serde_json::Error #[from]` divergence from JiraError.** JiraError chose `#[source]` across 4 parse variants because `#[from]` would collide; GitError has only 1 parse variant so `#[from]` is legal. Both decisions are correct under the architecture spec — the convention is "single-site → `#[from]`", and the rationale is documented inline.
3. **Shim `GitStr(String)` lands dead-on-arrival.** No callers anywhere in the workspace; first candidate for the post-§8 cleanup PR alongside `ClaudeStr` / `JiraStr` / `GitHubAppStr` / `AiAgentStr`.

## 7. Non-goals

- **NO** other sub-enum migrations (Auth and Config remain).
- **NO** removal of `MaestroError::*Str(String)` shims (final cleanup PR).
- **NO** changes to `ExternalActions` trait. The `RealActions` / `DryRunActions` impls of `create_worktree` / `delete_local_branch` only change the error-construction sites — signatures unchanged.
- **NO** changes to `process::run_command` / `run_shell_command*` / `ProcessHandle` — these still return `MaestroError` with `Io` / `Command` / `Timeout` envelopes.
- **NO** changes to the worktree-remove fallback logic — only the error wrapping is restructured.
