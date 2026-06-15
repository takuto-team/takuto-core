# Refactor spec — typed `JiraError` sub-enum (phase 4)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes A.1 row 2: carve `TakutoError::Jira(String)` into `JiraError`.

## 1. Module layout — `jira/error.rs` + `mod.rs` re-export

`jira/` is already a directory (`adf_markdown.rs`, `browse_url.rs`, `client.rs`, `mod.rs`, `poller.rs`). Add `jira/error.rs`; declare via `pub mod error; pub use error::JiraError;` in `mod.rs`. `adf_markdown.rs` / `browse_url.rs` are infallible value-shapers (no `Result<_>`); `poller.rs` only `?`-propagates — every construction lives in `client.rs`.

## 2. `JiraError` definition — 13 variants, **zero** foreign `#[from]`

Lands at `crates/takuto-core/src/jira/error.rs`. Granularity mirrors `GitHubAppError` (one variant per operation): callers can match `transition-failed` (workflow misconfig) vs `assign-failed` (permission) vs `get-details-failed` (ticket missing) without substring-sniffing.

```rust
#[derive(Debug, thiserror::Error)]
pub enum JiraError {
    /// `jira/client.rs:198` — `get_ticket_description_preview` invariant: the key's project prefix is not in `[jira] project_keys`.
    #[error("ticket {key} is not in configured [jira] project_keys")]
    TicketNotInConfiguredProjects { key: String },

    // ── acli subprocess failures (output.success() == false) ────────────────
    /// `jira/client.rs:178` — `list_todo_tickets_by_rank`.
    #[error("acli list To Do tickets failed: {stderr}")]
    ListTodoFailed { stderr: String },
    /// `jira/client.rs:216` — `get_ticket_description_preview`.
    #[error("acli load ticket {key} failed: {stderr}")]
    GetDescriptionPreviewFailed { key: String, stderr: String },
    /// `jira/client.rs:269` + `actions/real.rs:179` + `actions/dry_run.rs:83`.
    #[error("acli get ticket details for {key} failed: {stderr}")]
    GetDetailsFailed { key: String, stderr: String },
    /// `jira/client.rs:313` + `actions/real.rs:100`.
    #[error("acli assign ticket {key} failed: {stderr}")]
    AssignFailed { key: String, stderr: String },
    /// `jira/client.rs:336` + `actions/real.rs:153`.
    #[error("acli unassign ticket {key} failed: {stderr}")]
    UnassignFailed { key: String, stderr: String },
    /// `jira/client.rs:360` + `actions/real.rs:127`.
    #[error("acli transition ticket {key} to {status} failed: {stderr}")]
    TransitionFailed { key: String, status: String, stderr: String },
    /// `jira/client.rs:384`.
    #[error("acli update description for {key} failed: {stderr}")]
    UpdateDescriptionFailed { key: String, stderr: String },
    /// `jira/client.rs:406`.
    #[error("acli get linked item {key} failed: {stderr}")]
    GetLinkedItemFailed { key: String, stderr: String },

    // ── serde_json parsing of acli `--json` output ──────────────────────────
    /// `jira/client.rs:223` — `get_ticket_description_preview`. Carries `key` because the parse site already has it bound.
    #[error("failed to parse acli ticket JSON for {key}")]
    ParseTicketJson { key: String, #[source] source: serde_json::Error },
    /// `jira/client.rs:421` — `parse_ticket_list` (no key in scope).
    #[error("failed to parse acli ticket list JSON")]
    ParseTicketListJson { #[source] source: serde_json::Error },
    /// `jira/client.rs:467` — `parse_ticket_detail` (no key bound at parse).
    #[error("failed to parse acli ticket detail JSON")]
    ParseTicketDetailJson { #[source] source: serde_json::Error },
    /// `jira/client.rs:589` — `parse_linked_item` (no key bound at parse).
    #[error("failed to parse acli linked item JSON")]
    ParseLinkedItemJson { #[source] source: serde_json::Error },
}
```

**Foreign `#[from]`: none.** `serde_json::Error` appears on 4 variants (only one `#[from]` per source type is legal under `thiserror`, and each parse site carries distinct context — key vs. structural-kind), so all four use `#[source]` + explicit `.map_err(...)`. Same rationale as `GitHubAppError`'s `jsonwebtoken::errors::Error` / `chrono::format::ParseError`. No `std::io::Error` (acli failures `?`-propagate through `TakutoError::Io` / `Command`). No HTTP-client type (acli is shelled out). **ADF "parsing" is infallible** — `extract_description_text` (`client.rs:500`) + `jira_description_to_markdown` (`adf_markdown.rs`) walk JSON Value defensively with `unwrap_or` fallbacks; no ADF variant needed.

`TakutoError` gains `#[error(transparent)] Jira(#[from] JiraError)`; old `Jira(String)` renames to `JiraStr(String)` `#[deprecated]` per A.4.

## 3. Call-site inventory — 18 sites across 3 files

`jira/client.rs` (13): L178 → `ListTodoFailed`; L198 → `TicketNotInConfiguredProjects`; L216 → `GetDescriptionPreviewFailed`; L223 → `ParseTicketJson { key }`; L269 → `GetDetailsFailed`; L313 → `AssignFailed`; L336 → `UnassignFailed`; L360 → `TransitionFailed { status }`; L384 → `UpdateDescriptionFailed`; L406 → `GetLinkedItemFailed`; L421 → `ParseTicketListJson`; L467 → `ParseTicketDetailJson`; L589 → `ParseLinkedItemJson`.

`actions/real.rs` (4): L100 → `AssignFailed`; L127 → `TransitionFailed { status }`; L153 → `UnassignFailed`; L179 → `GetDetailsFailed`. `actions/dry_run.rs` (1): L83 → `GetDetailsFailed`.

The 5 `actions/{real,dry_run}.rs` sites re-implement the same acli ops as `client.rs` against a per-call `repo_path` (the action-trait surface, not a `JiraClient` instance). They produce errors semantically owned by the Jira subsystem, so they migrate to typed `JiraError` variants in the **same** C2 commit; they are not deferred to the `AgentError` (`actions/error.rs`) phase. After C2, `JiraStr` has zero callers — same dead-on-arrival shape as `ClaudeStr` / `GitHubAppStr`.

## 4. Migration plan — 3 commits

1. **C1 — land `JiraError` + envelope.** Create `jira/error.rs`; wire `pub mod error;` + re-export in `jira/mod.rs`; add `#[error(transparent)] Jira(#[from] JiraError)` on `TakutoError`; rename `Jira(String) → JiraStr(String)` `#[deprecated]`. Mechanical sed of all 18 sites `::Jira(` → `::JiraStr(` (jira/client.rs + actions/real.rs + actions/dry_run.rs) so the commit compiles with **zero behaviour change**. Tests baseline.
2. **C2 — migrate all 18 sites in one atomic commit.** Replace each `::JiraStr(...)` with the typed variant via `.into()`. Switch the four `error = %e` lines that observe a Jira error to `?e` so the `serde_json::Error` source is walked — `workflow/engine/lifecycle.rs:199` (`Failed to assign ticket at add-to-dashboard`), `lifecycle.rs:205` (`Failed to transition`), `lifecycle.rs:338` (`Failed to unassign on delete`), `lifecycle.rs:344` (`Failed to transition back to To Do on delete`); also `transitions.rs:345/351` and `bootstrap.rs:213/227`. After this commit `JiraStr` has zero callers.
3. **C3 — lock-in.** Add Display + `From → TakutoError::Jira` tests in `jira/error.rs` (mirror `github_app/error.rs:162-441`), covering all 13 variants. Add structural test in `crates/takuto-core/tests/` asserting `grep -rn 'TakutoError::Jira(\|TakutoError::JiraStr(' crates/takuto-core/src/{jira,actions}/` returns empty.

## 5. `#[deprecated]` shim consumers outside `jira/`

Two files: `crates/takuto-core/src/actions/real.rs` (4 sites) + `crates/takuto-core/src/actions/dry_run.rs` (1 site). Both migrated in C2 (same variants — see §3). Workspace-wide grep `TakutoError::Jira(` returns exactly the 18 sites above — no other consumers (`takuto-cli`, `takuto-web/src/routes/{jira,tickets}.rs`, `workflow/engine/*`) construct the variant; they only `?`-propagate.

## 6. Acceptance criteria

- [ ] `cargo build --workspace` zero new warnings beyond the dead `#[deprecated] JiraStr` declaration.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green; `cargo test --workspace` matches baseline.
- [ ] Zero `TakutoError::Jira(` constructors under `crates/takuto-core/src/{jira,actions}/` after C2 (C3 structural test enforces).
- [ ] HTTP responses unchanged at status-code + envelope level. No new `.unwrap()` / `.expect()` / `Box<dyn Error>` in public signatures.
- [ ] All 13 variants reachable via `TakutoError::from(JiraError::…)` (C3 lock-in test exercises each).

## 7. Risks

1. **JSON-parse Display loses inner detail.** Old Display rendered `"Failed to parse ticket JSON for {key}: {serde_err}"`; new Display is `"failed to parse acli ticket JSON for {key}"` with `serde_json::Error` as `#[source]`. Any `error = %e` on a Jira-parse path loses the parser message — C2 switches the seven `error = %e` lines under `workflow/engine/{lifecycle,transitions,bootstrap}.rs` to `?e` per code-quality-principles §3. (The 9 acli-failure variants interpolate `{stderr}` directly into Display, so no delta there.)
2. **Test fixtures string-matching old prefixes.** Sweep `grep -rn '"Failed to assign\|"Failed to transition\|"Failed to unassign\|"Failed to list To Do\|"Failed to load ticket\|"Failed to get ticket\|"Failed to get linked\|"Failed to parse ticket\|"Failed to parse linked\|"Failed to update description\|"is not in configured project_keys' crates/` before C2 — verified empty outside the construction sites at spec time.
3. **HTTP envelope mapping.** No `match` arm on `TakutoError::Jira` anywhere outside the 18 construction sites (verified workspace-wide). Old `Jira(String)` and new `Jira(JiraError)` fall through the same default → identical response shape; `takuto-web/src/routes/{jira,tickets}.rs` only `?`-propagate.
4. **Variant churn if `actions/` is refactored to delegate to `JiraClient`.** The 5 actions-side sites are deliberate near-duplicates with different `repo_path` plumbing. A future §1/§3 consolidation may collapse them — the 8 acli-op variants are designed so the consolidation needs zero variant churn (`AssignFailed { key, stderr }` shape holds regardless of which module raises it).

## 8. Non-goals (explicit)

- **NO** migration of `Git` / `AiAgent` / `Auth` / `Config` sub-enums (next 4 specs); **NO** removal of any `TakutoError::*Str` shim (including `JiraStr`).
- **NO** consolidation of the `actions/{real,dry_run}.rs` ↔ `jira/client.rs` near-duplication (separate §1/§3 cleanup; tracked in refactor-backlog).
- **NO** behaviour change in `JiraClient` / `RealActions` / `DryRunActions` methods other than the error variant produced (acli args, JQL, JSON shape, dedupe / linked-item walk identical).
- **NO** new `pub` accessors on `JiraError` beyond `thiserror` derives.
- **NO** edits outside `crates/takuto-core/src/{jira,actions,error.rs}` and `workflow/engine/{lifecycle,transitions,bootstrap}.rs` (the `%e → ?e` log fixes), except the C3 structural test under `crates/takuto-core/tests/`.
- **NO** HTTP API response shape changes (status codes + envelope keys unchanged).
