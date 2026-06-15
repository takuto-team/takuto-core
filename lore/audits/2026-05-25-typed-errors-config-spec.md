# Refactor spec — typed `ConfigError` sub-enum (phase 8, final)

Source: 2026-05-24 typed-errors architecture spec (Part A is binding — see `lore/audits/2026-05-24-typed-errors-spec.md`). Executes A.1 row 8: carve `TakutoError::Config(String)` into `ConfigError`. This is the largest and final per-subsystem migration (111 sites across 20 files); the post-§8 #2 cleanup PR removes all 7 `*Str(String)` shims once this lands.

## 1. Module layout — `config/error.rs` + `mod.rs` re-export

`config/` is already a directory (`agent.rs`, `general.rs`, `git.rs`, `jira.rs`, `load.rs`, `patches.rs`, `runtime.rs`, `template.rs`, `web.rs`, plus `agent_legacy.rs`). Add `pub mod error; pub use error::ConfigError;` to `mod.rs`. The 111 sites span 20 files across `config/`, `auth/{bundle,master_key,seal}.rs`, `workflow/engine/*`, `workflow/snapshot.rs`, `git/remote.rs`, `github/`, `docker_hooks/`, `db/provider_credentials.rs`, and `config_writer.rs` — because `TakutoError::Config(String)` had become a true catch-all bag, not a "config file errors" variant.

## 2. `ConfigError` definition — 26 variants

Lands at `crates/takuto-core/src/config/error.rs`. Operation clusters:
- **Config file validation** (2) — `Validation { section: &'static str, field: &'static str, detail: String }`, `SerializeToml { #[source] toml::ser::Error }`.
- **Workflow state machine** (10) — `WorkflowNotFound { ticket_key }`, `InvalidWorkflowState { op, current_state, ticket_key }`, `DefinitionNotFound { def_name, dir }`, `DefinitionInvalid { def_name, reason }`, `DefinitionAlreadyRunning { def_name, ticket_key }`, `DefinitionDependenciesNotMet { def_name }`, `DefinitionRetryWrongState { def_name, current_state }`, `DefinitionNoRunState { def_name, ticket_key }`, `DockerUnavailable` (unit), `Snapshot { op, detail }`.
- **Master-key bootstrap** (5) — `MasterKeyHex { #[source] hex::FromHexError }`, `MasterKeyLength` (unit), `MasterKeyFile { op: &'static str, path: PathBuf, detail }`, `MasterKeyUnavailable` (unit), `Csprng { op, #[source] getrandom::Error }`.
- **AEAD seal/open** (3) — `AeadEncrypt { op, detail }`, `AeadDecrypt { op, detail }`, `SealMalformed { detail }`.
- **Worker secrets bundle** (5) — `BundleTempdir { #[source] std::io::Error }`, `BundleSecretFile { op, path, detail }`, `BundleDbLookup { op, detail }`, `BundleProviderInvalid { detail }`, `BundleClaudeState { op, detail }`.
- **Catch-all** (1) — `Operational { op: &'static str, detail: String }` for sites that don't fit a structured variant yet. Candidates for future split when their patterns stabilise.

## 3. Documented deviation — `detail: String` payloads

The architecture rule "no `String` payload on a sub-enum variant" is **deliberately relaxed** for this phase. The original `TakutoError::Config(String)` had absorbed 111 sites across 6 unrelated subsystems (config file parsing, workflow state guards, AEAD primitives, master-key file I/O, bundle construction, miscellaneous operational error wrapping); fully structuring every payload would have produced ~50+ variants and dragged the migration into a multi-week effort. Instead, ~10 of the 26 variants carry a `detail: String` field that captures the operator-visible context. Variants where `detail` is a sentence rather than typed identifier:

- `Validation.detail`: third-party validator message (cors / agent provider parse / extra-args denied — these have stable substrings that operator tooling matches against; see §6 risk #2).
- `AeadEncrypt.detail` / `AeadDecrypt.detail` / `SealMalformed.detail`: AEAD operator-readable diagnostics from XChaCha20-Poly1305.
- `Snapshot.detail`: serde_json error formatted at the call site (no `#[from]` because `serde_json::Error` shows up via 4 different operations all going through the same Snapshot variant).
- `BundleSecretFile.detail` / `BundleDbLookup.detail` / `BundleProviderInvalid.detail` / `BundleClaudeState.detail` / `MasterKeyFile.detail`: foreign-error Display strings that the call site `.to_string()`'s.
- `Operational.{op,detail}`: the explicit catch-all.

`detail: String` here is **not** free-form sentence text in the architecture-rule sense — it's the third-party error or operator-context that the call site formats once. The variant + `op` discriminator carries the structure; `detail` carries the human-readable trail. Operational variants in particular are tagged with `op: &'static str` (a pinned set of labels) and are candidates for splitting into structured variants when their site-patterns stabilise.

## 4. Foreign `#[from]` decisions — none

Zero `#[from]` impls. `password_hash::Error` is owned by AuthError. `std::io::Error` would collide with `TakutoError::Io(#[from])` envelope. `toml::ser::Error` (`SerializeToml`) and `hex::FromHexError` (`MasterKeyHex`) and `getrandom::Error` (`Csprng`) and `std::io::Error` (`BundleTempdir`) each appear on a single variant but use `#[source]` for consistency with the multi-site `password_hash::Error` pattern from AuthError — the architecture spec accepts `#[source]` everywhere as a valid choice when the alternative is mixed `#[from]`/`#[source]` policies inside one enum.

## 5. Migration plan — 2 commits + lock-in (in C1)

- **C1** `refactor(config): C1 — land ConfigError + envelope + rename Config → ConfigStr` — define `ConfigError`, add `TakutoError::Config(#[from] ConfigError)`, rename `Config(String)` → `ConfigStr(String)` `#[deprecated]`, sed all 111 sites. Gates use **file-level `#![allow(deprecated)]`** at the top of each of the 20 affected files (justified by scale — 17+ enclosing-fn-attrs per the prior phases' pattern would have been ~40-50 attrs spread across the same 20 files for ambiguous gain). Add 2 lock-in tests (display sample, From-impl exhaustive with `cases.len() == 26`).
- **C2** `refactor(config): C2 — migrate all 111 sites to typed ConfigError variants` — replace every site with a typed variant per the variant-cluster mapping documented in the commit message. Drop the file-level `#![allow(deprecated)]` attrs. One Display contract preserved: `extra_args_denied:` substring kept inside `Validation.detail` because three tests (config/agent.rs, config/tests.rs, web/tests/config_agent.rs) match against it as the operator-facing stable error code.

## 6. Acceptance criteria

- [x] `cargo build --workspace` produces no NEW warnings beyond the 5 phase-1 `DatabaseStr` carryover.
- [x] `cargo test --workspace --lib --tests` matches baseline (1040) + 2 lock-in tests = 1042.
- [x] Zero `TakutoError::Config(` / `TakutoError::ConfigStr(` constructions remain under `crates/` (1 hit is the test assertion pattern in `config/error.rs`).
- [x] `TakutoError::ConfigStr(String)` retained as `#[deprecated]` shim — removed by the post-§8 #2 cleanup PR.
- [x] No HTTP API response shape changes. The `extra_args_denied` test contract preserved.

## 7. Risks

1. **Scale.** 111 sites is 3.4× the next-largest phase (AuthError, 33). The C1+C2 split here was essential — C1 lands the type + shim renaming and stays compilable; C2 then walks each file. Without C1's file-level allows, the intermediate state would have been 100+ deprecation warnings.
2. **Display drift on `extra_args_denied`.** The architecture spec accepts Display drift but the stable error code is part of the operator contract (UI shows it; tests match it). Preserved verbatim by including `extra_args_denied:` in the `Validation.detail` field.
3. **`Operational` catch-all.** Of the 26 variants, `Operational` is the only true escape hatch. Sites landing on it (gh API misc, hook runner exit, github poller parse, etc.) are flagged in the commit message as candidates for future structural split when their per-subsystem patterns stabilise.
4. **`detail: String` precedent.** This is the first phase to formally accept `detail: String` payloads on multiple variants. Documented as a scoped deviation (§3) — the architecture rule still applies to future sub-enums; ConfigError's relaxation is justified by the catch-all nature of the original `TakutoError::Config(String)`.

## 8. Non-goals

- **NO** removal of `TakutoError::*Str(String)` shims (handled by the final post-§8 #2 cleanup PR).
- **NO** changes to operator-facing HTTP error shapes (preserved via `extra_args_denied` substring; `register` 409 contract via prior AuthError phase).
- **NO** further structural split of `Operational` — flagged but deferred.
- **NO** changes to AEAD crypto, master-key bootstrap logic, snapshot serde shape, or workflow state machine semantics — only the error wrapping is restructured.
- **NO** further refactor of the bundle / master_key / seal modules — the structural splits of these belong to a separate audit item (§8 #3 was about source-file LOC, which these are under).
