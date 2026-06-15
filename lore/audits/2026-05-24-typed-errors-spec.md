# Refactor spec — typed error sub-enums (architecture) + db migration (phase 1)

Source: 2026-05-21 clean-code audit §8 #2 ("Carve `TakutoError`'s 8 String-wrapped variants into typed sub-enums"). Companion: `lore/code-quality-principles.md` §3. **Part A** binds the architecture for all 8 future subsystem migrations; **Part B** scopes the executable db work. db is first because it has the fewest construction sites (10 across 4 files).

## Part A — typed-error architecture (binding for all future phases)

### A.1 Sub-enum inventory & target module paths

Workspace-wide `TakutoError::X(format!(...))` construction counts per variant, with the target sub-enum + module path each will eventually move into:

| # | Sub-enum | Replaces | Construct sites | Target module path |
|---|----------|----------|-----------------|--------------------|
| 1 | `DbError` (this phase) | `TakutoError::Database(String)` | **18 wkspc / 10 in `db/`** | `crates/takuto-core/src/db/error.rs` |
| 2 | `JiraError` | `TakutoError::Jira(String)` | 18 | `crates/takuto-core/src/jira/error.rs` |
| 3 | `GitError` | `TakutoError::Git(String)` | 21 | `crates/takuto-core/src/git/error.rs` |
| 4 | `GitHubAppError` | `TakutoError::GitHubApp(String)` | 13 | `crates/takuto-core/src/github_app/error.rs` (today a flat file — split with the §1 LOC rules at migration time) |
| 5 | `AgentError` | `TakutoError::AiAgent(String)` | 18 | `crates/takuto-core/src/actions/error.rs` (covers cursor / codex / opencode sessions; the `actions` boundary owns AI-runtime failures) |
| 6 | `ClaudeError` | `TakutoError::Claude(String)` | 4 | `crates/takuto-core/src/claude/error.rs` (kept distinct from `AgentError` because `Claude` is a separate variant today and the Claude session module has its own surface) |
| 7 | `AuthError` | `TakutoError::Auth(String)` | 33 | `crates/takuto-core/src/auth/error.rs` |
| 8 | `ConfigError` | `TakutoError::Config(String)` + `TakutoError::ConfigNotFound(PathBuf)` | 111 + 1 | `crates/takuto-core/src/config/error.rs` |

`TakutoError::Command { … }` (10 sites), `TakutoError::Timeout(u64)` (10), `TakutoError::Cancelled` (12), `TakutoError::Io(#[from] std::io::Error)`, `TakutoError::TomlParse(#[from] toml::de::Error)` already carry structured fields or wrap typed sources — **they stay native on `TakutoError`** and are not migrated.

### A.2 TakutoError envelope shape (final)

**Decision: transparent `#[from]` envelope** — `TakutoError` becomes a thin sum of typed sub-enums plus the 5 already-structured native variants. The String variants stay as **deprecated shims** during the per-subsystem migration, then are removed in a final cleanup phase (out of scope for this spec).

```rust
pub enum TakutoError {
    #[error(transparent)] Db(#[from] DbError),
    #[error(transparent)] Jira(#[from] JiraError),
    #[error(transparent)] Git(#[from] GitError),
    #[error(transparent)] GitHubApp(#[from] GitHubAppError),
    #[error(transparent)] Agent(#[from] AgentError),
    #[error(transparent)] Claude(#[from] ClaudeError),
    #[error(transparent)] Auth(#[from] AuthError),
    #[error(transparent)] Config(#[from] ConfigError),
    // Native:
    #[error("Command failed: {cmd} (exit code {code})\n{stderr}")] Command { cmd: String, code: i32, stderr: String },
    #[error("Timeout after {0}s")] Timeout(u64),
    #[error("Workflow cancelled")] Cancelled,
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error(transparent)] TomlParse(#[from] toml::de::Error),
    // Deprecated shims (removed after the per-subsystem migrations complete):
    #[deprecated] #[error("Jira error: {0}")] JiraStr(String),
    // … one per old String variant, renamed to avoid name collision with the typed peer.
}
```

**Rejected: hybrid (keep `Database(String)` as a peer of `Db(DbError)` forever)** — leaves two ways to express the same failure, defeats the source-chain win, contradicts code-quality-principles §3. The shim is transitional, not permanent.

### A.3 Sub-enum field convention (pinned)

1. **Wrapped foreign error → `#[from] source`** (e.g. `DbError::Sqlite(#[from] rusqlite::Error)`).
2. **Operation-specific context → named fields** (`path: PathBuf`, `version: u32`, `user_id: String`). Identifiers and paths, not free-form sentences.
3. **No `format!` inside `#[error("…")]`** — reference fields by name (`{name}`). One line per variant, no terminal punctuation.
4. **No `String` payload on a sub-enum variant.** If a variant feels like it needs free-form text, split it or push the context into named fields.
5. **No public accessors** beyond `thiserror` derives. Callers match on variants.

### A.4 TakutoError variant deprecation path

Per migrated subsystem: the new typed variant lands first via `#[from] SubError`; existing `TakutoError::X(String)` becomes `#[deprecated]` and renames to `XStr` so the typed peer claims the canonical name. In-subsystem call sites migrate; cross-subsystem callers `?`-propagate unchanged. A final cleanup PR (out of scope) removes the 8 deprecated String variants once every caller is off them.

## Part B — db migration scope (this team's executable work)

### B.1 `DbError` definition

Lands at `crates/takuto-core/src/db/error.rs`, re-exported via `db/mod.rs`. Every variant cites the `TakutoError::Database(...)` site it replaces.

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Every `?`-propagated `rusqlite::Error` + users.rs:50 fallthrough.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    /// schema.rs:375 — version mismatch after run_migrations.
    #[error("schema migration failed: expected version {expected}, got {actual}")]
    Migrations { expected: i32, actual: i32 },

    /// mod.rs:89 — `create_dir_all` failed when opening the database.
    #[error("failed to create data directory {path}")]
    DataDir { path: PathBuf, #[source] source: std::io::Error },

    /// user_worktree_commands.rs:125/131/138 — application-layer NUL-byte guardrail.
    /// `field` ∈ {"user_id_or_workspace_name", "init_command", "run_command_name_or_command"}.
    #[error("{field} contains a NUL byte")]
    NulByte { field: &'static str },

    /// user_worktree_commands.rs:145/147 — `serde_json::to_string` failed.
    /// `column` ∈ {"init_commands_json", "run_commands_json"}.
    #[error("encoding {column} failed")]
    CommandsJsonEncode { column: &'static str, #[source] source: serde_json::Error },

    /// user_worktree_commands.rs:258/263 — `serde_json::from_str` failed.
    #[error("decoding {column} for ({user_id},{workspace_name}) failed")]
    CommandsJsonDecode {
        column: &'static str,
        user_id: String,
        workspace_name: String,
        #[source] source: serde_json::Error,
    },
}
```

`TakutoError` gains `#[error(transparent)] Db(#[from] DbError)`. The existing `impl From<rusqlite::Error> for TakutoError` is rewritten to `TakutoError::Db(DbError::Sqlite(e))` — preserves the source chain while keeping every `?` propagation through `Result<T, TakutoError>` byte-compatible.

### B.2 Call-site count (db subsystem only)

| File | Constructors |
|------|-------------:|
| `db/mod.rs` | 1 (`Database::open` → `DbError::DataDir`) |
| `db/schema.rs` | 1 (`run_migrations` → `DbError::Migrations`) |
| `db/users.rs` | 1 (`create_user` fallthrough → `DbError::Sqlite`) |
| `db/user_worktree_commands.rs` | 7 (3× `NulByte`, 2× `CommandsJsonEncode`, 2× `CommandsJsonDecode`) |
| **Total** | **10** |

Plus 1 implicit conversion at `error.rs:63` rewritten through `DbError::Sqlite`. Sites at `takuto-web/src/routes/admin.rs` (4) + `worktree_commands.rs` (1), and `db/credentials.rs` (Auth/Config constructs — auth-domain) are **out of scope** — picked up by the AuthError / ConfigError specs.

### B.3 Migration strategy (6 commits)

1. **Land `DbError` + envelope.** Add `db/error.rs` with the enum above, `pub mod error;` + `pub use error::DbError;` in `db/mod.rs`, `#[error(transparent)] Db(#[from] DbError)` on `TakutoError`, rewrite `impl From<rusqlite::Error> for TakutoError` through `DbError::Sqlite`. Zero call-site edits. `cargo test --workspace` matches baseline (1026/0/1).
2. **Migrate `db/mod.rs`** — `Database::open` returns `DbError::DataDir { path, source }`.
3. **Migrate `db/schema.rs`** — `run_migrations` returns `DbError::Migrations { expected, actual }`.
4. **Migrate `db/users.rs`** — `create_user` fallthrough constructs `DbError::Sqlite(e).into()`. The `TakutoError::Auth(...)` UNIQUE arm is **unchanged** (auth-domain, migrated by AuthError spec).
5. **Migrate `db/user_worktree_commands.rs`** — 7 sites in one commit. Update doc-comments at line 60 + `credential_audit.rs:70` referencing `TakutoError::Database`.
6. **Lock in.** Add a structural test in `crates/takuto-core/tests/` (mirroring the engine-demote precedent) asserting zero `TakutoError::Database(` constructors under `db/`. `cargo build --workspace` warning-free; clippy green.

### B.4 Acceptance criteria

- [ ] `cargo build --workspace` produces **zero warnings**; `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo test --workspace` matches baseline (1026 pass / 0 fail / 1 ignored). No count delta.
- [ ] Zero `TakutoError::Database(` constructors remain under `crates/takuto-core/src/db/` (verifiable: `grep -rn 'TakutoError::Database(' crates/takuto-core/src/db/ | grep -v '///'` returns empty).
- [ ] `TakutoError::Database(String)` still exists on `TakutoError` (deprecated shim retained for non-db callers — picked up by later phases).
- [ ] No new `.unwrap()` / `.expect()` in production code; no new `Box<dyn Error>` in any public signature.
- [ ] HTTP API responses unchanged at the status-code + envelope level. Error message strings may differ (e.g. omit the `: <rusqlite text>` tail because source is now chained, not formatted inline) — test fixtures that assert on full body text need a mechanical update.

### B.5 Risks

1. **Test assertions on full error-message text.** Tests that `.contains("Database error: …")` or match the exact rusqlite-formatted body break: the rusqlite text moves from the format string into `#[source]`, so `e.to_string()` no longer includes it. Sweep `grep -rn '"Database error' crates/` before commit 5.
2. **HTTP error envelope.** `takuto-web`'s `IntoResponse` impl must map `Db(DbError)` to the same status code as `Database(String)` did (500). Pin in commit 1; confirm `cargo test -p takuto-web` green.
3. **`tracing` interpolation.** `error = %e` flattens to the source's Display via `#[error(transparent)]` — switch lines needing the full chain to `error = ?e` per code-quality-principles §3.
4. **Transitive `?` callers outside `db/`.** Routes and `workflow/engine/*` `?`-propagate db errors; after the migration those flow `DbError → TakutoError` via the new envelope — no caller-side edits needed. Verified: every outside caller returns `Result<_, TakutoError>` already.
5. **`match TakutoError::Database(s)` sites.** Any caller matching on the old String variant needs to add a `TakutoError::Db(DbError::Sqlite(_))` arm. Sweep `grep -rn 'TakutoError::Database' crates/` before commit 1; expect only the B.2 sites.

### B.6 Non-goals (explicit)

- **NO** migration of `jira` / `git` / `github_app` / `agent` / `claude` / `auth` / `config` sub-enums (next 7 specs).
- **NO** removal of `TakutoError::Database(String)` — deprecated shim retained so `takuto-web/src/routes/admin.rs`:574/582/600/605 and `worktree_commands.rs`:450 compile.
- **NO** HTTP error-response shape changes (status codes + envelope keys unchanged).
- **NO** edits outside `crates/takuto-core/src/db/` and `crates/takuto-core/src/error.rs`, except the structural test landing in `crates/takuto-core/tests/`.
- **NO** behaviour change in db fns other than the error variant produced on failure (inputs accepted, rows written, side effects identical).
- **NO** new `pub` accessors on `DbError` beyond `thiserror` derives.
