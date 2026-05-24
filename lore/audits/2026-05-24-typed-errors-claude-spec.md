# Refactor spec — typed `ClaudeError` sub-enum (phase 2)

Source: 2026-05-21 clean-code audit §8 #2 / 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). This spec executes phase 2: carve `MaestroError::Claude(String)` into `ClaudeError` per the architecture's A.1 row 6.

## 1. Subsystem selection — Claude wins on every axis

Workspace-wide constructor counts (`grep -rn 'MaestroError::<X>(' crates/`):

| Subsystem | Sites | Module shape |
|-----------|------:|--------------|
| **Claude** | **4** | **`claude/{mod.rs, session.rs}` — single producer file** |
| GitHubApp | 13 | flat `github_app/*.rs`, multi-file |
| Jira | 18 | `jira/` spans 6+ files |
| AiAgent | 18 | `actions/` (cursor / codex / opencode) |
| Git | 21 | `git/` spans 10+ files |
| Auth | 33 | `auth/` spans 8+ files |
| Config | 111 | `config/` spans 7+ files |

Claude is 3.25× smaller than the next candidate **and** has the cleanest module boundary (single producer file, no cross-file fan-out).

## 2. `ClaudeError` definition — 2 variants

Lands at `crates/maestro-core/src/claude/error.rs`, re-exported via `claude/mod.rs` (`pub mod error; pub use error::ClaudeError;`).

```rust
#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    /// `claude/session.rs:208` — process exited non-zero. `detail` is the
    /// parsed stream-json result OR a 5-line stderr snippet OR "(no output)".
    /// Free-form String matches `MaestroError::Command { stderr: String }`
    /// (already on the envelope) — operator diagnostic, not a sentence.
    #[error("Claude Code exited with code {exit_code}: {detail}")]
    NonZeroExit { exit_code: i32, detail: String },

    /// `claude/session.rs:215` — process succeeded but stdout was empty,
    /// implying Claude is unauthenticated inside the container.
    #[error("Claude Code session produced no output — check that Claude is authenticated in the container")]
    EmptyOutput,
}
```

The two **wrap-a-MaestroError** sites (lines 179, 239) do **not** become typed variants — they collapse to bare `?`/direct return. They wrap an inner `MaestroError` (from `ProcessHandle::spawn` / `wait_with_*`) in a string prefix that adds zero information; per arch-spec §A.3 rule 4 ("if a variant feels like it needs free-form text, the design is wrong") the right shape is direct propagation. `MaestroError` gains `#[error(transparent)] Claude(#[from] ClaudeError)`; old `Claude(String)` renames to `ClaudeStr(String)` with `#[deprecated]` per A.4.

## 3. Call-site inventory (Claude subsystem only)

| File:line | Current | New |
|-----------|---------|-----|
| `claude/session.rs:179` | `MaestroError::Claude(format!("Failed to spawn Claude Code: {e}"))` (e is `MaestroError`) | bare `?` — propagate `e` |
| `claude/session.rs:208` | `MaestroError::Claude(format!("Claude Code exited with code {}: {}", output.exit_code, detail))` | `ClaudeError::NonZeroExit { exit_code: output.exit_code, detail }.into()` |
| `claude/session.rs:215` | `MaestroError::Claude("…produced no output…".to_string())` | `ClaudeError::EmptyOutput.into()` |
| `claude/session.rs:239` | `Err(e) => Err(MaestroError::Claude(format!("Claude Code session error: {e}")))` | `Err(e) => Err(e)` (collapse the wrap; `MaestroError::Timeout` already handled on line 235) |

Total: **4 sites**, all in one file.

## 4. Migration plan (3 commits)

1. **C1 — land `ClaudeError` + envelope.** Add `claude/error.rs`, wire `pub mod error;` + re-export in `claude/mod.rs`, add `#[error(transparent)] Claude(#[from] ClaudeError)` on `MaestroError`, rename `Claude(String) → ClaudeStr(String)` with `#[deprecated]`. Mechanically sed the four call sites in `claude/session.rs` from `::Claude(` → `::ClaudeStr(` so the commit compiles with **zero behaviour change**. Tests baseline.
2. **C2 — migrate `claude/session.rs`** (atomic, 4 sites, one file). Lines 208 + 215 become typed variant constructors via `.into()`; lines 179 + 239 collapse the MaestroError wrap to direct propagation. After this commit `ClaudeStr` has zero callers.
3. **C3 — lock-in.** Add `ClaudeError` Display + `From → MaestroError::Claude` tests in `claude/error.rs` (mirror `db/error.rs:71-199`). Add structural test in `crates/maestro-core/tests/` asserting `grep -rn 'MaestroError::Claude(\|MaestroError::ClaudeStr(' crates/maestro-core/src/claude/` returns empty.

## 5. `#[deprecated]` shim consumers outside `claude/`

**None.** `grep -rn 'MaestroError::Claude(' crates/` returns 4 hits, all in `claude/session.rs`. After C2 the renamed `ClaudeStr(String)` has zero callers — a dead-on-arrival shim kept only to honour arch-spec A.4, and a candidate for **first** deletion by the final cleanup PR. No transitive caller surprises.

## 6. Acceptance criteria

- [ ] `cargo build --workspace` zero new warnings beyond the dead `#[deprecated] ClaudeStr` declaration (zero usage-site warnings since the shim has no callers).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green; `cargo test --workspace` matches baseline (1028/0/1).
- [ ] Zero `MaestroError::Claude(` constructors under `crates/maestro-core/src/claude/` after C2 (C3 structural test enforces).
- [ ] HTTP responses unchanged at status-code + envelope level (Display strings may differ on the collapse paths — see Risks §1).
- [ ] No new `.unwrap()` / `.expect()`; no new `Box<dyn Error>` in public signatures.

## 7. Risks

1. **Display delta on lines 179 + 239 collapse.** A `Cancelled`/`Io` propagating out of `wait_with_*` or `ProcessHandle::spawn` previously rendered as `"Claude session error: …"` / `"Failed to spawn Claude Code: …"`; after the collapse it renders as the inner error's Display. Both prefixes added zero signal. Sweep before C2: `grep -rn '"Claude Code session error\|"Claude session error\|"Failed to spawn Claude' crates/` — verified empty at spec time.
2. **HTTP envelope.** `maestro-web` has no `MaestroError::Claude`-specific status mapping (verified: no `match` on the variant anywhere outside the four construction sites). Old `Claude(String)` and new `Claude(ClaudeError)` fall through the same fallback path → identical response shape.
3. **`tracing` interpolation.** `error = %e` would flatten via `#[error(transparent)]`. Per code-quality-principles §3, lines wanting the full chain use `error = ?e`. `claude/session.rs` already uses `?e` everywhere (verified); no migration churn.
4. **dev_mock path.** `dev_mock::run_claude_mock` constructs only `MaestroError::Cancelled`, never `MaestroError::Claude`. Out of scope; verified.

## 8. Non-goals (explicit)

- **NO** migration of `Jira` / `Git` / `GitHubApp` / `AiAgent` / `Auth` / `Config` sub-enums (next 6 specs).
- **NO** removal of any `MaestroError::*Str` shim — including `ClaudeStr` (deferred to the post-phase-8 cleanup PR).
- **NO** edits outside `crates/maestro-core/src/claude/` + `error.rs`, except the C3 structural test under `crates/maestro-core/tests/`.
- **NO** behaviour change in `ClaudeSession::run_prompt` other than the error variant produced (inputs, args, container wrap, timeout, session-id extraction identical).
- **NO** new `pub` accessors on `ClaudeError` beyond `thiserror` derives.
