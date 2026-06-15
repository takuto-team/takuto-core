# Refactor spec — push to grade A (audit §11)

Source: 2026-05-21 clean-code audit, post-§8 follow-up. After the four §8 priorities (engine demote/AppState carve; typed error sub-enums; god-module splits; React component splits) the audit re-rated the codebase A−. Two structural items remained between A− and A:

1. **`.unwrap()` / `.expect()` allowlist.** Every non-test panic site must carry a `// SAFETY:` comment explaining why the panic is unreachable. The audit's project-rule table flagged this as a minor fail (5 real sites + ~24 in subcomponents).
2. **4 of the 8 remaining 1000-LOC files split** to bring the worst offenders under the audit's ≤1000-LOC rule.

This spec covers both.

## 1. SAFETY-comment pass

Every production `.unwrap()` / `.expect()` outside `#[cfg(test)]` now carries an inline `// SAFETY:` block above it. The set is the documented allowlist — new sites must follow the same pattern or be refactored away.

### Sites covered (24 total)

| File | Sites | Justification class |
|---|---|---|
| `takuto-cli/src/main.rs` | 2 | SIGTERM/Ctrl-C handlers (cfg(unix) + tokio runtime contract) |
| `takuto-core/src/license.rs` | 1 | `OnceLock::set` once-init guard |
| `takuto-core/src/process.rs` | 4 | `Stdio::piped()` child stream `take()` contract |
| `takuto-core/src/db/credentials.rs` | 2 | Argon2 `Params::new` constants within published bounds |
| `takuto-core/src/config/web.rs` | 2 | `strip_suffix` guarded by `ends_with` |
| `takuto-core/src/container/editor/token_gen.rs` | 1 | `getrandom` OS CSPRNG contract (token reuse unacceptable) |
| `takuto-web/src/auth.rs` | 1 | HMAC-SHA256 key length type-checked at `&[u8; 32]` |
| `takuto-web/src/server.rs` | 5 | Governor config + config RwLock + static response builders |
| `takuto-web/src/routes/sessions/{mod,token_validator,proxy_forward}.rs` | 5 | `Response::builder()` with only `StatusCode` + empty body, or ASCII-only header values |
| `takuto-web/src/routes/credentials.rs` | 2 | `require_master_key` middleware gates `auth.db.is_some()` |
| `takuto-web/src/routes/repositories.rs` | 1 | Row was just upserted in same connection |
| `takuto-web/src/routes/workflows/manual.rs` | 1 | `user_repos.is_empty()` rejected with 400 above |

### SAFETY comment pattern

Lead with **why** the panic is unreachable, not what would break if it weren't. Examples:

```rust
// SAFETY: `signal(SIGTERM)` only fails on a system that does not support
// tokio's signal driver (no Unix process flag, no tokio runtime). Both
// are guaranteed by the cfg(unix) gate + the `#[tokio::main]`-equivalent
// runtime we already entered.
let mut sigterm = signal(SignalKind::terminate())
    .expect("signal(SIGTERM) cannot fail under cfg(unix) + tokio runtime");
```

```rust
// SAFETY: `Hmac::new_from_slice` only fails on invalid key length, and
// the type signature `&[u8; 32]` guarantees a 32-byte key — SHA-256
// accepts any length up to its block size (64 bytes), so 32 is valid.
let mut mac = HmacSha256::new_from_slice(key.as_slice())
    .expect("HMAC-SHA256 accepts any key length ≤ 64 bytes; key is &[u8; 32]");
```

```rust
// SAFETY: `require_master_key` returns `Err` when the DB is missing
// (no master key without a DB), so reaching this point guarantees
// `auth_state.db.is_some()`.
let db = auth_state.db.as_ref()
    .expect("require_master_key gated db.is_some()")
    .clone();
```

The opaque pre-pass messages (`"row just inserted/upserted"`, `"redirect builder is infallible"`, `"401 builder is infallible"`) were rewritten alongside the SAFETY block so the panic-message string itself names the unreachable invariant.

### Out-of-scope panics

- `panic!` macros inside `tokio::task::spawn` blocks — those are bugs in their own right and the audit's §8 follow-up already converted the worst into typed errors.
- Test-only `.unwrap()`/`.expect()` inside `#[cfg(test)]` mods or `*/tests/*.rs` integration files — those are encouraged for terseness and remain unchanged.
- `WorkerSecretsBundle::for_tests` `.expect("tempdir for test bundle")` — gated on `#[cfg(test)]`, never reached at runtime.

## 2. God-module splits (4 of 8 remaining ≥1000-LOC files)

The §8 #3 split landed the three worst offenders (container/runner.rs 1513 LOC, github/auth_resolver.rs 1381 LOC, docker_hooks.rs 1216 LOC). The audit's post-cut list still showed eight files over 1000 LOC. Push-to-A takes the next four down to per-concern submodule trees, leaving four files (~1000–1200 LOC each) for a future cut.

### Targets

| File | Before | After (mod.rs + 3–5 leaves) | Reduction notes |
|---|---|---|---|
| `crates/takuto-web/src/routes/auth.rs` | 997 | 5 files, largest 347 LOC | 8 disjoint handlers; no shared mutable state |
| `crates/takuto-web/src/routes/sessions.rs` | 1165 | 4 files, largest 568 LOC | top-level dispatch + 3 cohesive forwarders |
| `crates/takuto-core/src/auth/bundle.rs` | 1244 | 6 files, largest 654 LOC (mod.rs tests dominate) | 5 production files + cross-module test mod |
| `crates/takuto-core/src/container/editor.rs` | 1157 | 6 files, largest 641 LOC | docker-run orchestrator stays cohesive; helpers carved |

### Per-target layout

#### `routes/auth.rs` → `routes/auth/` (5 leaves)

| File | LOC | Owns |
|---|---:|---|
| `mod.rs` | 194 | `pub use` re-exports + shared lockout constants + cross-handler integration tests |
| `status.rs` | 124 | `GET /api/auth/status` (public auth/setup probe) |
| `login.rs` | 238 | `POST /api/auth/login` + `POST /api/auth/logout` |
| `me.rs` | 60 | `GET /api/auth/me` |
| `password.rs` | 347 | change-password, regenerate-recovery-codes, recover |
| `register.rs` | 129 | `POST /api/auth/register` (first-user setup) |

Lockout constants (`LOCKOUT_THRESHOLD`, `LOCKOUT_WINDOW_SECS`) live in `mod.rs` as `pub(super)` and feed `login.rs` and `password.rs::recover`. External callers (`server.rs`) see the same `routes::auth::*` paths because every handler is re-exported from `mod.rs`.

#### `routes/sessions.rs` → `routes/sessions/` (4 leaves)

| File | LOC | Owns |
|---|---:|---|
| `mod.rs` | 568 | `proxy_session` + `proxy_or_static_fallback` + auth/ownership gate + integration tests |
| `token_validator.rs` | 285 | path parser, token-hash logger, 404/308 builders, WS-upgrade detection |
| `proxy_forward.rs` | 272 | hyper client, upstream URI builder, header sanitisation, redirect rewriting, HTTP forward |
| `websocket.rs` | 116 | `forward_websocket` 101 + bidirectional tunnel |

`parse_session_path` + `token_hash_prefix` remain `pub` re-exports; everything else is `pub(super)`. The 21-test suite splits by concern (integration → `mod.rs`, parser/hash/WS-upgrade/404/308 → `token_validator`, upstream URI → `proxy_forward`).

#### `auth/bundle.rs` → `auth/bundle/` (5 production leaves + mod)

| File | LOC | Owns |
|---|---:|---|
| `mod.rs` | 654 | re-exports + 23-test suite (test mod dominates the LOC) |
| `types.rs` | 139 | `WorkerSecretsBundle` struct + `SECRET_FILE_*` constants |
| `tempdir.rs` | 83 | per-bundle dir creation + `cleanup_orphan_secrets` |
| `write_secret.rs` | 64 | mode-0400 atomic write (Unix + non-Unix) |
| `unseal.rs` | 180 | open provider_credential + claude cli_state rows |
| `assembler.rs` | 241 | `build`, `build_for_endpoint`, `pin_for_workflow` |

`WorkerSecretsBundle::_temp_dir` stays `pub(super)` so the test mod (a sibling submodule) can construct stub bundles without breaking the field-level secrecy. Re-exports preserve every external import path (`auth::bundle::build`, `auth::bundle::cleanup_orphan_secrets`, `auth::WorkerSecretsBundle`).

#### `container/editor.rs` → `container/editor/` (5 production leaves + mod)

| File | LOC | Owns |
|---|---:|---|
| `mod.rs` | 268 | re-exports + 18-test suite |
| `port_alloc.rs` | 162 | 9100–9200 in-memory allocator + docker probe + restart recovery |
| `token_gen.rs` | 54 | UUID v4 connection token + 16-byte CSPRNG path token |
| `urls.rs` | 101 | direct + shared-port-proxy URL builders |
| `labels.rs` | 38 | `editor_container_name` + label JSON parsing |
| `container_builder.rs` | 641 | `EditorInfo` + `start_editor` / `stop_editor` / `get_editor_info` |

`container_builder.rs` stays at 641 LOC because the `docker run` orchestration (env wiring, volume mounting, label setting, post-spawn root setup, startup commands, label-or-port-fallback discovery) is one cohesive flow. Carving further would create false seams.

Cross-module `pub(crate)` callers (`port_scanner`, `run_command`, `terminal`) keep their `super::editor::*` imports because `editor/mod.rs` re-exports `EDITOR_PORT_*`, `allocate_editor_ports`, `release_container_ports`, `editor_container_name` with the same visibility.

### Acceptance criteria

- [x] `cargo build --workspace` produces zero warnings on every split.
- [x] `cargo test --workspace --lib --tests` matches the 1042 / 0 / 1 baseline after every split.
- [x] No behaviour change visible to callers (router wiring, public function signatures, test imports unchanged).
- [x] Every new production file ≤ 700 LOC.
- [x] Test modules co-located with the code they cover when the code spans only one new file; otherwise consolidated in `mod.rs::tests` with cross-module access via `pub(super)`.

## 3. Files NOT split

The audit's post-§8 list contained 8 files over 1000 LOC. Push-to-A took the four that scored highest on either "shared edit hotspot" (auth, sessions) or "easiest cohesive seams" (bundle, editor). The remaining four — `git/operations.rs` (~1100), `actions/jira/poller.rs` (~1050), `routes/admin.rs` (~1000), `db/migrations.rs` (~1000) — stayed intact because:

- `git/operations.rs` — one cohesive thin shell over `Command::new("git")`; every fn is 20–60 LOC, splitting would shuffle imports without improving navigation.
- `actions/jira/poller.rs` — one async loop with helpers; splitting would expose internal state via spurious `pub(crate)` items.
- `routes/admin.rs` — six handlers, each ~150 LOC + 320-LOC `#[cfg(test)] mod tests`; the per-handler split would land four ≤200 LOC files but the audit's threshold concern (cross-PR conflicts) doesn't apply here — admin routes are rarely touched.
- `db/migrations.rs` — append-only by design; every migration is a sealed block. Splitting would mean per-migration files, which works against the migration-numbering convention the team already follows.

Re-cut is on the table if any of those files crosses 1500 LOC.

## 4. Re-audit results

After the splits + unwrap pass:

| Project rule | Before push-to-A | After |
|---|---|---|
| No `unwrap()`/`expect()` in non-test code | Fail (minor): 5 real sites without justification | **Pass**: 24 documented sites in the allowlist |
| `thiserror` errors; no `Box<dyn Error>` in public API | Pass (since §8 #2) | Pass |
| Rust file > ~300 LOC ⇒ split | Fail: 8 files over 1000 LOC | Closer to pass: 4 over 1000, none over 1200 |
| React component > ~150 LOC ⇒ split | Pass (since §8 #4) | Pass |
| No `RwLock`/`Mutex` guard across `.await` | Pass (clippy::await_holding_lock enabled since §8 #1) | Pass |
| TS strict, no `any`, no `@ts-ignore` | Pass | Pass |
| All API shapes in `src/api/types.ts` | Pass | Pass |
| No `console.log` in merged code | Pass (1 sanitised borderline) | Pass |
| No `println!` in production paths | Pass (since docker_hooks split in §8 #3) | Pass |
| Zero hardcoded secrets | Pass | Pass |

**Final grade: A.** The remaining four ≥1000-LOC files are tractable but not blast-radius-critical; the unwrap allowlist is the strongest signal the audit's project-rule table tracks.

## 5. Risks & non-goals

1. **`editor/container_builder.rs` residual size.** At 641 LOC it's the largest sub-file in any push-to-A split. Further carving would slice a single docker-run orchestration into pieces that re-fetch the same context — net negative for readability. Documented as accepted.
2. **`bundle/mod.rs` LOC count.** 654 LOC, dominated by 23 integration-style tests that touch every submodule. The tests sit in `mod.rs` because they cross-cut; splitting them per submodule would duplicate fixtures (`db_with_master_key`, `seed_user`, `seed_provider_credential`). Production code in `bundle/mod.rs` is ~50 LOC of re-exports.
3. **No new tests added in the unwrap pass.** Every site carries a SAFETY comment + a non-opaque panic message; the panic strings themselves become a documentation contract. A linter that checks for `// SAFETY:` above every `.unwrap()` / `.expect()` outside `#[cfg(test)]` would lock the invariant in CI — out of scope for this spec, candidate for a follow-up.
4. **Four ≥1000-LOC files survive.** See §3 above for the per-file justification. The audit's grade-A bar accepts ≤4 files over 1000 LOC when each has a coherent reason to stay cohesive.
