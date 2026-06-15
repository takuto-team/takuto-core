# Refactor spec — typed `AuthError` sub-enum (phase 7)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes A.1 row 7: carve `TakutoError::Auth(String)` into `AuthError`.

## 1. Module layout — `auth/error.rs` + `mod.rs` re-export

`auth/` is already a directory (`bundle.rs`, `gh_client.rs`, `master_key.rs`, `pat_validation.rs`, `seal.rs`). Add `auth/error.rs`; declare via `pub mod error; pub use error::AuthError;` in `mod.rs`. The 33 `TakutoError::Auth(format!(...))` call sites span 5 files across both crates: `crates/takuto-core/src/db/credentials.rs` (14), `db/users.rs` (10), `crates/takuto-web/src/routes/auth.rs` (6), `routes/admin.rs` (2), `auth.rs` middleware (1). The variants are "auth subsystem failures" — user CRUD invariants + argon2 cryptography + session / registration / recovery flows — so `auth/` is the canonical home even though the bulk of the sites live in `db/`.

## 2. `AuthError` definition — 17 variants

Lands at `crates/takuto-core/src/auth/error.rs`. Operation clusters:
- **User CRUD** (5) — `EmptyUsername`, `UsernameAlreadyExists { username }`, `UserNotFound { id }`, `LastAdminLockout { op: &'static str }` (collapses 3 sites with `op: "demote" | "suspend" | "delete"`), `UserDisappearedAfterUpdate` (race-condition guard).
- **Argon2 hashing** (6) — `SaltGeneration { #[source] getrandom::Error }`, `SaltEncoding { #[source] password_hash::Error }`, `HashFailed { kind: &'static str, #[source] password_hash::Error }` (collapses 4 password/recovery-code hash sites), `StoredHashEncoding { #[source] Utf8Error }`, `PasswordHashFormat { #[source] password_hash::Error }`, `ArgonParams { #[source] password_hash::Error }`.
- **Web routes** (5) — `CurrentPasswordIncorrect`, `InvalidSession`, `InvalidRecoveryCode`, `RegistrationClosed`, `PasswordTooShort`. All unit variants.
- **Web auth middleware** (1) — `SessionSerialize { #[source] serde_json::Error }`.

## 3. Foreign `#[from]` decisions — none

Zero `#[from]` impls. `password_hash::Error` appears on 4 variants (`SaltEncoding`, `HashFailed`, `PasswordHashFormat`, `ArgonParams`) — only one `#[from]` per source type is legal under `thiserror`, so all four use `#[source]` + explicit `.map_err`. Same rationale as JiraError's `serde_json::Error` and GitHubAppError's `jsonwebtoken::Error`. The single-site `Utf8Error` / `serde_json::Error` / `getrandom::Error` variants also use `#[source]` for consistency — none of them needed a typed `From` shortcut because the call sites already use `.map_err` to attach operation context (`SessionSerialize` needs no extra context, but the alternative `#[from]` would mask the intent at the call site).

## 4. Cargo dep change — enable `argon2/std`

`password_hash::Error` only implements `std::error::Error` when its parent crate's `std` feature is enabled. `argon2` 0.5 forwards `std` through `password-hash`. Enabling it (`argon2 = { version = "0.5", features = ["std"] }`) is required for `thiserror`'s `#[source]` derive to compile. Single-line change in the root `Cargo.toml`; the workspace doesn't run on no-std anywhere, so this only affects code that already used `password_hash::Error::to_string()` and now gets `error::source()` chain walking for free.

## 5. Migration plan — 2 commits + lock-in (in C1)

- **C1** `refactor(auth): C1 — land AuthError + envelope + rename Auth → AuthStr` — define `AuthError`, add `TakutoError::Auth(#[from] AuthError)`, rename `Auth(String)` → `AuthStr(String)` `#[deprecated]`, sed all 33 sites, gate under function-level `#[allow(deprecated)]` on the 17 enclosing fns, enable `argon2/std`, clean up 2 unused `TakutoError` imports left over from the git phase. Add 2 lock-in tests in `auth/error.rs` (`cases.len() == 17` drift assertion).
- **C2** `refactor(auth): C2 — migrate db/, web routes/auth+admin, auth middleware to AuthError` — 33 sites become typed variants, attrs from C1 removed. `spawn_blocking` closures gain explicit `Ok::<_, TakutoError>(...)` tail annotations because the typed `Err(AuthError::*.into())` paths inside the closure body otherwise can't infer their target type (multiple typed Err paths + one Ok path = type-inference ambiguity).

Lock-in tests land in C1 alongside the type (same precedent as agent / git phases).

## 6. Acceptance criteria

- [x] `cargo build --workspace` produces no NEW warnings beyond the 5 phase-1 `DatabaseStr` carryover.
- [x] `cargo test --workspace --lib --tests` matches baseline (1038) + 2 lock-in tests = 1040.
- [x] Zero `TakutoError::Auth(` / `TakutoError::AuthStr(` constructions remain under `crates/`.
- [x] `TakutoError::AuthStr(String)` retained as `#[deprecated]` shim — removed by the post-§8 #2 cleanup PR.
- [x] No HTTP API response shape changes. The `register` handler still routes `RegistrationClosed` to 409 because the existing `e.to_string().contains("already exist") || e.to_string().contains("Registration is closed")` substring check passes against the new `RegistrationClosed` Display string (`"Registration is closed: users already exist. Use admin API to create new users."`).

## 7. Risks

1. **Display drift on `UserNotFound`.** The old `format!("User not found: {id}")` produced `"User not found: u-123"`; the new variant `UserNotFound { id: String }` Display matches verbatim. Verified by lock-in test.
2. **`LastAdminLockout` op collapse changes the message-substring shape.** Old messages were three distinct sentences ("Cannot demote: …", "Cannot suspend: …", "Cannot delete: …"); new variant produces the same three with `op: &'static str` pinned at the call site. Substring searches like `msg.contains("Cannot demote")` still match because the entire sentence is preserved.
3. **`argon2/std` feature enable.** Adds the `std::error::Error` impl on `password_hash::Error`. No code outside the new variant uses that impl, so no behaviour change beyond what `thiserror`'s `#[source]` derives now produce.
4. **`spawn_blocking` closure annotations.** Three closures now carry `Ok::<_, takuto_core::error::TakutoError>(...)` on their tail. This is purely a type-inference hint — runtime behaviour is identical.

## 8. Non-goals

- **NO** other sub-enum migrations (ConfigError remains).
- **NO** removal of `TakutoError::*Str(String)` shims (final cleanup PR after Config).
- **NO** changes to `validate_db_session` / `authenticate_db_user` / `create_db_session` signatures — only their internal error construction.
- **NO** changes to argon2 parameter constants (`CURRENT_M_COST` / `CURRENT_T_COST_*`) or password-length minimum.
- **NO** changes to the recovery-code generation / verification crypto.
- **NO** changes to login lockout (`record_attempt` / `clear_failed_attempts`) — those return `Result<(), TakutoError>` and propagate via `?`, not typed Auth construction.
