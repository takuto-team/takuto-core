# Refactor spec — typed `AgentError` sub-enum (phase 5)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes A.1 row 5: carve `MaestroError::AiAgent(String)` into `AgentError`.

## 1. Module layout — `actions/error.rs` + `mod.rs` re-export

`actions/` is already a directory (`dry_run.rs`, `gh_github.rs`, `mod.rs`, `real.rs`, `traits.rs`). Add `actions/error.rs`; declare via `pub mod error; pub use error::AgentError;` in `mod.rs`. The 18 `MaestroError::AiAgent(format!(...))` call sites live in `cursor/session.rs` (5), `codex/session.rs` (6), `opencode/session.rs` (5), and `workflow/engine/step_runner.rs` (2) — `actions/error.rs` is the canonical home because `AgentError` is the typed envelope for "AI agent CLI invocations", which is the `ExternalActions::*` trait's domain.

## 2. `AgentError` definition — 6 variants

Lands at `crates/maestro-core/src/actions/error.rs`. Smaller variant count than `JiraError` / `GitHubAppError` because the 4 agent CLIs (Cursor, Codex, OpenCode + Claude indirectly) share the same operation shape — `NonZeroExit` / `EmptyOutput` / `StreamFailed` collapse into one generic variant each with a `provider: AiAgentProvider` discriminator.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// `cursor/session.rs:97`, `codex/session.rs:107`. OpenCode passes the worktree
    /// as `&Path` and has no equivalent site.
    #[error("{provider} worktree path is not valid UTF-8")]
    WorktreePathInvalidUtf8 { provider: AiAgentProvider },

    #[error("{provider} exited with code {exit_code}: {stderr_tail}")]
    NonZeroExit { provider: AiAgentProvider, exit_code: i32, stderr_tail: String },

    /// `hint` is a pinned per-provider `&'static str` selected at the call
    /// site — not free-form text. Three values total across cursor/codex/opencode.
    #[error("{provider} produced no output — {hint}")]
    EmptyOutput { provider: AiAgentProvider, hint: &'static str },

    /// Codex `turn.failed` event or OpenCode `error` event in the stream-JSON channel.
    #[error("{provider} stream reported error: {message}")]
    StreamFailed { provider: AiAgentProvider, message: String },

    /// `step_runner.rs:494` — `[[steps]]` shell command step exit ≠ 0.
    #[error("Command step failed")]
    CommandStepFailed,

    /// `step_runner.rs:682` — orchestrator abort after AI session failure.
    /// `hint` is a pinned `&'static str` per provider (4 values).
    #[error("Agent step failed — {hint}")]
    AgentStepAborted { hint: &'static str },
}
```

## 3. Foreign `#[from]` decisions — none

No `#[from]` impls. The 18 sites either (a) carry no foreign error (the constructors above are built from already-parsed `output.exit_code` / `output.stdout`) or (b) are the spawn/wait paths that already return `MaestroError` and should be propagated with bare `?` rather than re-wrapped. Six sites collapse to direct propagation under that rule:

- `cursor/session.rs` L144 spawn-failure (was `format!("Failed to spawn Cursor Agent: {e}")`)
- `cursor/session.rs` L183 generic-session-error wrap (was `format!("Cursor Agent error: {e}")`)
- `codex/session.rs` L128 spawn-failure
- `codex/session.rs` L174 generic-session-error wrap
- `opencode/session.rs` L126 spawn-failure
- `opencode/session.rs` L172 generic-session-error wrap

All six previously prefixed an inner `MaestroError` with zero-info text. Direct `?` / `Err(e) => Err(e)` preserves the typed inner error and source chain. Display drift is acknowledged; no test in the workspace string-matches the old prefixes (verified by grep for `"Cursor Agent error|Codex CLI error|OpenCode error|Failed to spawn (Cursor|Codex|OpenCode)"`).

## 4. `AiAgentProvider` Display impl

The `#[error("{provider} …")]` templates require `AiAgentProvider` to implement `Display`. Adds `pub fn display_name(self) -> &'static str` + `impl fmt::Display` returning the legacy prefixes verbatim ("Cursor Agent", "Codex CLI", "OpenCode", "Claude Code"). Additive trait impl on an internally-owned type — no behaviour change for the existing `as_str()` lowercase identifier path used by serde.

## 5. Migration plan — 2 commits + lock-in (already landed alongside C1)

- **C1** `refactor(agent): C1 — land AgentError + envelope + rename AiAgent → AiAgentStr` — define `AgentError`, add `MaestroError::Agent(#[from] AgentError)`, rename `AiAgent(String)` → `AiAgentStr(String)` with `#[deprecated]`, mechanically sed all 18 sites to `::AiAgentStr(`, gated under function-level `#[allow(deprecated)]` (three session runners) + statement-level (two step-runner sites). Add `impl Display for AiAgentProvider`. Add 2 lock-in tests (Display + From impl, both with `cases.len() == 6` drift assertion).
- **C2** `refactor(agent): C2 — migrate cursor/codex/opencode/step_runner to AgentError` — 12 sites become typed variants, 6 collapse to direct propagation, attrs from C1 removed.

The lock-in tests landed in C1 alongside the type definition (single test commit would have been redundant given the small surface).

## 6. Acceptance criteria

- [x] `cargo build --workspace` produces no NEW warnings beyond the 5 phase-1 `DatabaseStr` carryover.
- [x] `cargo test --workspace --lib --tests` matches baseline (1034) + 2 lock-in tests = 1036.
- [x] `cargo clippy --workspace -- -D warnings` is clean.
- [x] Zero `MaestroError::AiAgent(` / `MaestroError::AiAgentStr(` constructions remain under `crates/maestro-core/src/`.
- [x] `MaestroError::AiAgentStr(String)` retained as `#[deprecated]` shim — removed by the post-§8 #2 cleanup PR.
- [x] No HTTP API response shape changes. No logic changes to handler bodies. No changes to `MaestroError` variants outside the new `Agent` + `AiAgentStr` pair.

## 7. Risks

1. **Display drift on the six collapsed sites.** Previously `"Cursor Agent error: <inner>"` etc.; now the inner `MaestroError` Display surfaces directly. Verified no workspace tests string-match the old prefixes. Acceptable per arch-spec — the typed propagation is a strict information gain.
2. **`AiAgentProvider` becomes `Display`-able workspace-wide.** Any code that previously relied on `format!("{provider:?}")` Debug behaviour now sees a different label. Verified by grep: no production code formats `AiAgentProvider` via `{:?}` — only `as_str()` for the lowercase serde identifier.
3. **Shim `AiAgentStr(String)` lands dead-on-arrival.** No callers — confirmed by grep. The shim exists only to honour the architecture spec's A.4 deprecation path; first candidate for the post-§8 cleanup PR.

## 8. Non-goals

- **NO** other sub-enum migrations (Git, Auth, Config remain).
- **NO** removal of `MaestroError::*Str(String)` shims (final cleanup PR).
- **NO** changes to the `ExternalActions` trait or any `actions/{real,dry_run}.rs` site (those carry Jira / Git / GitHub call-throughs, not agent errors).
- **NO** changes to `ProcessHandle::spawn` or its timeout / cancellation contract.
- **NO** changes to the stream-JSON parsers — `find_codex_turn_failure` / `first_opencode_error` still return `Option<String>`.
