# Refactor spec — typed `GitHubAppError` sub-enum (phase 3)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes phase 3 / A.1 row 4: carve `TakutoError::GitHubApp(String)` into `GitHubAppError`.

## 1. Module layout — flat `github_app.rs` + sibling `github_app/error.rs`

`github_app.rs` is 726 LOC (§1 violation, tracked in `refactor-backlog.md` for future body-splitting — NON-GOAL here). Use **Rust 2018+ file-plus-sibling-dir**: keep `github_app.rs` as-is, create a sibling directory `crates/takuto-core/src/github_app/` containing **`error.rs` only**, declared from `github_app.rs` via `pub mod error; pub use error::GitHubAppError;`. Matches the arch-spec target path without `mod.rs` churn.

## 2. `GitHubAppError` definition — 15 variants, **zero** foreign `#[from]`

13 call sites → 15 variants: the L302 API-error path discriminates four typed sub-cases; L419/L433 collapse onto one variant via `setting: &'static str`. Lands at `crates/takuto-core/src/github_app/error.rs`.

```rust
use std::path::PathBuf;
#[derive(Debug, thiserror::Error)]
pub enum GitHubAppError {
    #[error("invalid RSA private key in [github] config: {source}")]
    InvalidPrivateKey { #[source] source: jsonwebtoken::errors::Error },
    #[error("set either [github] app_private_key or app_private_key_path, not both")]
    PrivateKeyConfigConflict,
    #[error("cannot read [github] app_private_key_path {path}")]
    PrivateKeyRead { path: PathBuf, #[source] source: std::io::Error },
    #[error("GitHub App private key not configured — set [github] app_private_key or app_private_key_path")]
    PrivateKeyMissing,
    #[error("failed to generate GitHub App JWT")]
    JwtSigning { #[source] source: jsonwebtoken::errors::Error },
    #[error("curl request to GitHub API failed (exit {exit_code}): {stderr}")]
    HttpRequestFailed { exit_code: i32, stderr: String },
    #[error("failed to parse token expiry {raw}")]
    ExpiresAtParse { raw: String, #[source] source: chrono::format::ParseError },
    #[error("GitHub App installation not found (installation_id = {installation_id}) — verify [github] app_installation_id is correct and the App is installed on your org/repo")]
    ApiInstallationNotFound { installation_id: u64, documentation_url: String },
    #[error("GitHub App JWT authentication failed (app_id = {app_id}): {message}")]
    ApiJwtRejected { app_id: u64, message: String, documentation_url: String },
    #[error("GitHub App lacks required permissions: {message} — needs contents (write), pull_requests (write), metadata (read)")]
    ApiPermissionDenied { message: String, documentation_url: String },
    #[error("GitHub API error: {message}")]
    ApiOther { message: String, documentation_url: String },
    #[error("unexpected GitHub API response: {body}")]
    UnexpectedApiResponse { body: String },
    #[error("git config {setting} failed: {stderr}")]
    GitConfigFailed { setting: &'static str, stderr: String },
    #[error("failed to write token file {path}")]
    TokenFileWrite { path: PathBuf, #[source] source: std::io::Error },
    #[error("failed to rename token file {from} → {to}")]
    TokenFileRename { from: PathBuf, to: PathBuf, #[source] source: std::io::Error },
}
```

**Foreign `#[from]`: none.** `jsonwebtoken::errors::Error` and `chrono::format::ParseError` appear on multiple-or-context-bearing variants (only one `#[from]` per source type is legal; both sites already use `map_err`). `std::io::Error` collides with the envelope-level `TakutoError::Io(#[from] std::io::Error)` and every github_app IO failure needs path context. `serde_json::Error` is intentionally swallowed by both `if let Ok(...)` branches. Every variant uses `#[source]` + explicit `.map_err(...)`. Diverges from `DbError::Sqlite(#[from] rusqlite::Error)` (no bulk-default analogue here).

`TakutoError` gains `#[error(transparent)] GitHubApp(#[from] GitHubAppError)`; old `GitHubApp(String)` renames to `GitHubAppStr(String)` `#[deprecated]` per A.4.

## 3. Call-site inventory — 13 sites, all in `github_app.rs`

L120 → `InvalidPrivateKey`. L161 → `PrivateKeyConfigConflict`. L173 → `PrivateKeyRead`. L180 → `PrivateKeyMissing`. L201 → `JwtSigning`. L272 → `HttpRequestFailed`. L283 → `ExpiresAtParse`. L302 → fans to `ApiInstallationNotFound` / `ApiJwtRejected` / `ApiPermissionDenied` / `ApiOther` via rewritten `format_api_error(&self, err) -> GitHubAppError`. L305 → `UnexpectedApiResponse`. L419 → `GitConfigFailed { setting: "user.name", … }`. L433 → `GitConfigFailed { setting: "user.email", … }`. L510 → `TokenFileWrite`. L516 → `TokenFileRename`.

## 4. Migration plan — 3 commits

1. **C1 — land `GitHubAppError` + envelope.** Create `github_app/error.rs`; wire `pub mod error;` + re-export in `github_app.rs`; add `#[error(transparent)] GitHubApp(#[from] GitHubAppError)` on `TakutoError`; rename `GitHubApp(String) → GitHubAppStr(String)` `#[deprecated]`. Mechanical sed of the 13 sites `::GitHubApp(` → `::GitHubAppStr(` so the commit compiles with **zero behaviour change**. Tests baseline.
2. **C2 — migrate `github_app.rs`** (atomic, 13 sites). Rewrite `format_api_error` to return `GitHubAppError`. Replace each `::GitHubAppStr(...)` with the typed variant via `.into()`. Switch `error = %e` to `?e` at L479/L490/L549 to walk the source chain. After this commit `GitHubAppStr` has zero callers.
3. **C3 — lock-in.** Add Display + `From → TakutoError::GitHubApp` tests in `github_app/error.rs` (mirror `claude/error.rs:42-95`), covering all 15 variants. Pin the four `format_api_error` Display substrings (`"installation not found"`, `"JWT authentication failed"`, `"lacks required permissions"`, `"GitHub API error"`) so the existing in-file `format_api_error_*` tests keep passing after C2.

## 5. `#[deprecated]` shim consumers outside `github_app.rs`

**None.** `grep -rn 'TakutoError::GitHubApp(' crates/` returns 13 hits, all in `github_app.rs`. `GitHubAppStr(String)` lands with zero callers — same dead-on-arrival pattern as `ClaudeStr`, first candidate for the post-phase-8 cleanup PR.

## 6. Acceptance criteria

- [ ] `cargo build --workspace` zero new warnings beyond the dead `#[deprecated] GitHubAppStr` declaration.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green; `cargo test --workspace` matches baseline.
- [ ] Zero `TakutoError::GitHubApp(` constructors under `crates/takuto-core/src/github_app{.rs,/}` after C2.
- [ ] The four `format_api_error_*` unit tests (`github_app.rs:656-702`) still pass — Display substrings preserved verbatim.
- [ ] HTTP responses unchanged at status-code + envelope level. No new `.unwrap()` / `.expect()` / `Box<dyn Error>`.

## 7. Risks

1. **API error Display delta.** `format_api_error` condenses multi-sentence operator hints into one `#[error("…")]` line per variant. The four assertion substrings are preserved verbatim; exact-body assertions would break. Sweep `grep -rn '"installation not found\|"JWT authentication\|"lacks required\|"GitHub App"' crates/` before C2 — verified empty outside `github_app.rs`.
2. **Source-chain logging.** `JwtSigning`, `PrivateKeyRead`, `TokenFileWrite`, `TokenFileRename`, `ExpiresAtParse` all Display without inlining `{source}`; the inner cause is reachable only via `Error::source()`. C2 switches the three `error = %e` lines (L479/L490/L549) to `?e` per code-quality-principles §3, otherwise operators see only the variant Display.
3. **HTTP envelope mapping.** No `match` arm on `TakutoError::GitHubApp` anywhere outside `github_app.rs` (verified). Both old and new shapes fall through the same default → identical response shape.

## 8. Non-goals (explicit)

- **NO** migration of `Jira` / `Git` / `AiAgent` / `Auth` / `Config` sub-enums; **NO** removal of any `TakutoError::*Str` shim (including `GitHubAppStr`).
- **NO** conversion of `github_app.rs` (726 LOC §1 violation) to `mod.rs`-style — tracked separately in `lore/refactor-backlog.md`.
- **NO** edits outside `crates/takuto-core/src/github_app{.rs,/}` + `crates/takuto-core/src/error.rs`.
- **NO** behaviour change in `GitHubAppTokenManager` methods other than the error variant produced (JWT shape, curl invocation, caching, atomic-rename semantics identical); **NO** new `pub` accessors on `GitHubAppError` beyond `thiserror` derives.
