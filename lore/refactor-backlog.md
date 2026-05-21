# Refactor Backlog — Clean-Code Audit (2026-05)

Source: clean-code audit by the `clean-code-refactor` team.
Rules referenced: `CODING_STANDARDS.md` (§1 SOLID, §2 Rust, §3 React/TS, §5 General).
Companion lore: `lore/code-quality-principles.md`.

This file is the authoritative work list. Each item carries:

- **Scope** — files / directories touched.
- **Rationale** — one sentence on *why*.
- **Acceptance criteria** — verifiable, mechanical checks. A reviewer must be able to run each one and get a yes/no answer.
- **Owner** — recommended team role (backend / frontend / cross-cutting).

All items must obey `CODING_STANDARDS.md` §5: minimum viable change, one logical change per commit, `cargo test --workspace` and `npm run build` green after every commit, `AGENTS.md` updated in the same task when documented behaviour or crate layout changes.

---

## P0 — Top-priority fixes (audit §8)

### P0-1 · Split `crates/maestro-core/src/container.rs` into a `container/` module

**Scope**

- `crates/maestro-core/src/container.rs` (2,699 prod LOC, 52 module items, 3,816 LOC including tests).
- New tree:

  ```
  crates/maestro-core/src/container/
    mod.rs            # thin facade — re-exports + types shared across submodules
    runner.rs         # current 73–757 (struct + ctor + spawn helpers + impl block 427–757)
    editor.rs         # current 783–1520 (openvscode-server / per-user editor session lifecycle)
    terminal.rs       # current 1550–1726 (interactive terminal session)
    reap.rs           # current 1842–1925 (zombie / orphan cleanup)
    port_scanner.rs   # current 1928–2226 (port-tracking poller)
    run_command.rs    # current 2226–2699 (one-shot run-command spawner)
  ```
  Tests at 2700–3816 move into a `#[cfg(test)] mod tests` block at the bottom of the submodule they exercise (CODING_STANDARDS §2 "Tests go in `#[cfg(test)] mod tests` at the bottom of the same file they test").

**Rationale**

Violates CODING_STANDARDS §1 "one file = one reason to change" and the 300-LOC Rust threshold by ~9×. The file mixes editor, terminal, port-tracking, reaping, and run-command — five unrelated reasons to change.

**Acceptance criteria**

- `wc -l crates/maestro-core/src/container/*.rs` shows every file ≤ ~400 LOC of non-test code.
- `mod.rs` contains **only** `pub mod`/`pub(crate) use` lines plus shared type definitions used by ≥ 2 submodules (no `fn` bodies).
- `cargo build --workspace` produces **zero warnings** (§2 quality bar).
- `cargo test --workspace` passes; every test moved survives the move.
- `git grep -nE "container::|use crate::container" crates/` confirms no caller had to change its import path (re-exports preserve the public surface) — i.e. **no public API change**.
- `git log -p` shows tests live alongside the code they test.

**Owner:** backend

---

### P0-2 · Enable `strict: true` in the UI TypeScript configs

**Scope**

- `ui/tsconfig.app.json` (add `"strict": true`).
- `ui/tsconfig.node.json` (add `"strict": true`).
- `ui/src/components/AiProviderSettingsSection.tsx` lines ~99–106 — the 10 × `as AgentClaudeConfig` casts must be replaced with discriminated-union narrowing (the same `AgentConfig` type already has a `type` tag).
- Any other site that fails strict mode after the flag flip.

**Rationale**

CODING_STANDARDS §3 mandates `strict: true`. Today both UI tsconfigs silently lack it — every `any`, `as unknown as X`, and implicit-any parameter ships unchallenged.

**Acceptance criteria**

- `grep -n '"strict": true' ui/tsconfig.app.json ui/tsconfig.node.json` matches both files.
- `npm run build` from `ui/` passes with zero TypeScript errors.
- `grep -n "as AgentClaudeConfig" ui/src/components/AiProviderSettingsSection.tsx` returns nothing.
- No new `@ts-ignore` / `@ts-expect-error` / `any` / `as unknown as` introduced (`git diff` review).
- `git diff --stat` shows only the strict-mode flip, the narrowing in `AiProviderSettingsSection.tsx`, and any other sites the compiler flagged — no opportunistic refactors.

**Owner:** frontend

---

### P0-3 · Split `crates/maestro-core/src/workflow/engine/driver.rs` into focused engine sub-modules

**Scope**

- `crates/maestro-core/src/workflow/engine/driver.rs` (2,162 LOC, 22 fns, 0 inline tests).
- Keep `driver.rs` itself for `drive_workflow_def` plus the small glue it owns (~400 LOC).
- New siblings under `crates/maestro-core/src/workflow/engine/`:

  | File | Current lines | Functions moved |
  |---|---|---|
  | `resolve.rs` | 59, 87, 135, 175 | `resolve_workspace_name`, `resolve_repo_for_ticket`, `resolve_worktree_init_commands`, `scan_definitions_dir` |
  | `bootstrap.rs` | 430, 651–1168 | `prepare_worktree_for_ticket`, `bootstrap_new_workflow` |
  | `auth_pin.rs` | 531, 584 | `ensure_workflow_auth_pin`, `try_attach_secrets_bundle` |
  | `step_runner.rs` | 1169, 1365, 1794, 1866, 2113 | `run_workflow_def_steps`, `run_agent_step_sequence`, `acquire_agent_slot`, `broadcast_step_*`, `spawn_output_relay`, `close_github_issue` |

- `workflow/engine/mod.rs` (existing facade) declares the new sub-modules and re-exports anything callers outside `engine` already use.

**Rationale**

CODING_STANDARDS §1 + §2 "follow the engine/ pattern: large modules become directories; mod.rs is a thin facade". The driver currently violates the rule it should be the canonical example of.

**Acceptance criteria**

- Every new file is ≤ ~400 LOC non-test.
- `crates/maestro-core/src/workflow/engine/driver.rs` keeps `drive_workflow_def` as its single entry-point.
- `git grep -nE "workflow::engine::driver::" crates/` confirms no external caller had to change imports (re-exports preserved) — **no public API change**.
- `cargo build --workspace` → zero warnings; `cargo test --workspace` → green.
- `AGENTS.md` "Engine and storage" section reviewed; the bootstrap step-numbering bullets must still be accurate after the move. Update in the same commit if any file path is named.
- A short inline `///` doc on `bootstrap.rs::bootstrap_new_workflow` and `step_runner.rs::run_agent_step_sequence` documenting their entry contract (CODING_STANDARDS §2 "All public non-trivial items get a `///` doc comment").

**Owner:** backend

---

### P0-4 · Add a `[profile.release]` block + CI clippy / test gates

**Scope**

- Workspace `Cargo.toml` (root): add
  ```toml
  [profile.release]
  strip = "symbols"
  lto = "thin"
  codegen-units = 1
  ```
- `.github/workflows/` — add (or extend) a workflow that runs, on every PR:
  - `cargo build --workspace --all-targets`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `npm --prefix ui ci && npm --prefix ui run build`
- If `npm run lint` / `npm run typecheck` exist, add them too.

**Rationale**

Today nothing prevents a warning-bearing commit from landing, and release binaries ship with full symbols + default codegen settings. CODING_STANDARDS §2 "cargo build must produce zero warnings before any commit" is unenforceable without CI; the release profile fix is one-time leverage.

**Acceptance criteria**

- `grep -A4 "\\[profile.release\\]" Cargo.toml` shows the three lines above.
- A PR with `let _ = 1;` introduced fails the new clippy job.
- A PR with a deliberately failing test fails the new test job.
- `cargo build --release --workspace` succeeds locally with the new profile.
- The workflow runs on `pull_request` (not only `push`).
- README / CONTRIBUTING references the gate if appropriate (optional; minimum viable change is the workflow file itself).

**Owner:** cross-cutting

---

## P1 — High-value follow-ups

### P1-1 · Split `crates/maestro-web/src/routes/workflows.rs` into an 8-file module

**Scope**

- `crates/maestro-web/src/routes/workflows.rs` (2,313 LOC, 23 handlers).
- New tree under `crates/maestro-web/src/routes/workflows/`:
  - `mod.rs` — facade: declares sub-modules, exposes the `Router` builder.
  - `queries.rs` — list/get/count handlers.
  - `lifecycle.rs` — start, stop, retry, delete, mark-as-done.
  - `manual.rs` — manual-start + paste-description flows.
  - `editor.rs` — `/editor` session handlers.
  - `terminal.rs` — terminal session handlers.
  - `run_commands.rs` — run-command endpoints.
  - `port_tracking.rs` — port-scanner endpoints.

**Rationale**

CODING_STANDARDS §1 — one file = one reason to change. A single 2.3k-LOC route file is a merge-conflict magnet.

**Acceptance criteria**

- Every new file ≤ ~400 LOC.
- `mod.rs` contains only the router wiring + `pub(crate) use` re-exports; no handler bodies.
- The `Router` returned to `build_router` exposes **the same path → handler mapping** (verify with `git diff` on the `.route(...)` list).
- `cargo test -p maestro-web` and the existing integration tests pass.
- No public type renamed or moved across crate boundaries.

**Owner:** backend

---

### P1-2 · Split `crates/maestro-core/src/config.rs` into a `config/` module

**Scope**

- `crates/maestro-core/src/config.rs` (1,512 prod LOC, 22 pub structs, 3 `impl Config` blocks).
- New tree:
  - `config/mod.rs` — facade, re-exports.
  - `config/schema.rs` — the structs + `Default` impls.
  - `config/interpolation.rs` — `interpolate_*`, `validate_extra_args`, `validate_cors_origin`, `cursor_model_for_cli`.
  - `config/load.rs` — `Config::load*`, `validate`, `provisioning_sha`, `resolve_config_relative_path`, `detect_legacy_command_keys`.

**Rationale**

Three orthogonal concerns (data shape, value interpolation, file I/O + validation) currently share one file. Splitting them makes `provisioning_sha` and validation rules unit-testable in isolation.

**Acceptance criteria**

- `wc -l crates/maestro-core/src/config/*.rs` ≤ ~600 LOC each (schema may be the largest).
- Public type paths preserved (`maestro_core::Config`, `maestro_core::config::Config`, etc.) — confirm with `git grep -nE "use maestro_core::(config::)?Config"`.
- `cargo test -p maestro-core` passes.
- `AGENTS.md` § Configuration paths reviewed; update file references if any are named.

**Owner:** backend

---

### P1-3 · Promote `MaestroError` variants to typed `#[from]` wiring

**Scope**

- `crates/maestro-core/src/error.rs` — today 10 / 13 variants carry `String` payloads.
- Add `#[from] rusqlite::Error`, `#[from] serde_json::Error`, `#[from] chrono::ParseError` (drop the lossy `.to_string()` conversions).
- Promote `Jira`, `Git`, `Claude`, `AiAgent`, `Auth`, `Database`, `Config` variants to wrap `#[source] Box<dyn std::error::Error + Send + Sync>` (the **public crate-boundary** rule in CODING_STANDARDS §2 forbids `Box<dyn Error>` in a public API — these wrappers are internal to the `MaestroError` enum, not the public return type, so they comply; we only widen the source storage).
- Sweep the 41 `.map_err(|e| MaestroError::*(e.to_string()))` call sites and replace with `?` where the new `#[from]` covers it, or with explicit `MaestroError::Variant { source: Box::new(e) }` otherwise.

**Rationale**

`String` payloads break `std::error::Error::source()` — log output shows only the top-level message, never the root cause. CODING_STANDARDS §2 "Define errors with thiserror" is honoured in name but defeated in payload shape.

**Acceptance criteria**

- `grep -nE "MaestroError::\\w+\\(.*\\.to_string\\(\\)\\)" crates/ | wc -l` reports **zero** (down from 41).
- `cargo test --workspace` green; new unit test in `error.rs` confirms `MaestroError::Database(...).source().is_some()` for a wrapped `rusqlite::Error`.
- No public function signature change (callers still return `MaestroError`).
- No `Box<dyn Error>` in any **return type**; confined to the `MaestroError` variant's internal `#[source]` field.
- `cargo build --workspace` zero warnings.

**Owner:** backend

---

### P1-4 · Encapsulate `Workflow` and `AppState` field visibility

**Scope**

- `crates/maestro-core/src/workflow/engine/types.rs:131` — `Workflow` has 31 `pub` fields.
  - Keep `pub`: `id`, `ticket_key`, `state`, `started_at`.
  - Convert dashboard-projection / driver fields to `pub(crate)`: `current_step_label`, `terminal_lines`, `driver_started`, plus any other field mutated by the engine internals.
  - Expose **accessor methods** that enforce state-machine invariants for any field external callers actually need to write today (audit `engine/driver.rs:298` and similar — `wf.state = …` direct writes must go through a `try_transition()` method).
- `crates/maestro-web/src/state.rs` — `AppState` has 28 `pub` fields. Convert to `pub(crate)` minimum. Anything truly needed at the crate boundary stays `pub`; everything else drops a level.

**Rationale**

CODING_STANDARDS §1 (Open/Closed) — direct field mutation bypasses the state machine; future invariants can't be enforced. §2 "`pub(crate)` by default for internal items; `pub` only at true crate boundaries".

**Acceptance criteria**

- `grep -nE "wf\\.state = " crates/ | wc -l` → 0 (all writes go through an accessor).
- `git grep -n "pub " crates/maestro-core/src/workflow/engine/types.rs` shows only the 4 retained public fields plus the new accessor methods.
- `cargo build --workspace` zero warnings; no caller outside the crate broke (we didn't remove any externally-needed field, just narrowed its visibility).
- New `///` doc on each accessor method.

**Owner:** backend

---

### P1-5 · Gate `crates/maestro-web/src/test_helpers.rs` so it doesn't ship in prod

**Scope**

- `crates/maestro-web/src/lib.rs:22` — change `pub mod test_helpers;` to one of:
  - `#[cfg(any(test, feature = "test-utils"))] pub mod test_helpers;` with a corresponding `test-utils = []` feature in `crates/maestro-web/Cargo.toml`, **or**
  - Relocate the module to a sister `maestro-web-testing` crate listed as a `[dev-dependencies]` of `maestro-web`.

**Rationale**

CODING_STANDARDS §2 "Tests go in `#[cfg(test)] mod tests`". Test helpers shipped to the public surface bloat the binary and let production callers depend on test fakes by accident.

**Acceptance criteria**

- `cargo build -p maestro-web --release` succeeds and `cargo expand -p maestro-web --release | grep test_helpers` (or equivalent) shows the module compiled out.
- `cargo test -p maestro-web --features test-utils` (or via the new crate) still compiles every existing test.
- No callsite in production code (`crates/maestro-web/src/{routes,middleware,state,server}.rs`) references `test_helpers::*`.
- The 14 `.unwrap()` calls inside `test_helpers.rs` are unchanged — they are test scaffolding (this gate is the fix; they no longer count as production unwraps).

**Owner:** cross-cutting

---

### P1-6 · Split `ui/src/components/MyCredentialsSection.tsx`

**Scope**

- `ui/src/components/MyCredentialsSection.tsx` (763 LOC, 14 `useState`, 5 components in one file).
- New tree under `ui/src/components/credentials/`:
  - `MyCredentialsSection.tsx` — shell + data fetching.
  - `AiCredentialPanel.tsx`
  - `ClaudeSessionField.tsx`
  - `GitHubCredentialPanel.tsx`
  - `helpers.ts` — pure functions / formatting.

**Rationale**

CODING_STANDARDS §1 "React: extract a sub-component when a component exceeds ~150 lines or mixes two unrelated concerns" + §3 "No component that both fetches data and renders UI".

**Acceptance criteria**

- Every new `.tsx` ≤ ~150 LOC.
- One component per file; filename = component name (PascalCase).
- All `useState`/`useEffect` colocated with the consumer that needs them.
- `npm run build` passes; the credentials page renders identically (visual diff via Storybook or e2e snapshot if available).
- No prop named `props` passed wholesale into a child (CODING_STANDARDS §3).

**Owner:** frontend

---

### P1-7 · Split `ui/src/components/modals/TicketDetailModal.tsx`

**Scope**

- `ui/src/components/modals/TicketDetailModal.tsx` (539 LOC, 17 `useState`, 5 `useEffect`, 4 `useRef`).
- Decomposition:
  - `TicketDetailModal.tsx` — shell + layout only.
  - `hooks/useTicketDetail.ts` — fetch / cache / mutate.
  - `hooks/useTicketCountdown.ts` — the timer + refs.
  - `TicketDetailAiPanel.tsx` — AI-related sub-block.

**Rationale**

CODING_STANDARDS §3 — extract reusable stateful logic into `use*` hooks; no `useEffect` for derived values; `useEffect` only for genuine side-effects.

**Acceptance criteria**

- Shell component ≤ ~150 LOC; each hook ≤ ~80 LOC.
- All `useRef` lives inside the hook that needs it, not the shell.
- `useEffect` count in the shell ≤ 1 (the subscription effect, if any).
- `npm run build` passes; modal behaviour unchanged (manual smoke + existing e2e if present).

**Owner:** frontend

---

### P1-8 · Split `ui/src/components/AiProviderSettingsSection.tsx`

**Scope**

- `ui/src/components/AiProviderSettingsSection.tsx` (606 LOC, 3 components colocated).
- One file per component (PascalCase filenames). Helpers go in `aiProviderSettings.helpers.ts` or similar.

**Rationale**

CODING_STANDARDS §3 "One component per file".

**Acceptance criteria**

- Each new file ≤ ~150 LOC.
- The 10 × `as AgentClaudeConfig` casts (already addressed by P0-2) remain absent.
- `npm run build` passes with `strict: true`.

**Owner:** frontend

---

### P1-9 · Split `ui/src/pages/Dashboard.tsx`

**Scope**

- `ui/src/pages/Dashboard.tsx` (440 LOC, 11 `useState`, 8 `useEffect`).
- Extract hooks:
  - `useActiveRepo()` — current repo + switch.
  - `useWorkflowDefinitions()` — workflow-def discovery + caching.
  - `useOnboardingStatus()` — first-user / setup gating.

**Rationale**

CODING_STANDARDS §3 — colocate state; extract reusable stateful logic; no useEffect for derived values.

**Acceptance criteria**

- `Dashboard.tsx` ≤ ~250 LOC.
- `useState` count in `Dashboard.tsx` ≤ 4 (UI-only state); the rest moved into the hooks.
- `useEffect` count ≤ 2 in the shell.
- `npm run build` passes.

**Owner:** frontend

---

### P1-10 · Split `ui/src/api/client.ts` into per-domain modules

**Scope**

- `ui/src/api/client.ts` (534 LOC, 23 exports).
- New files under `ui/src/api/`:
  - `http.ts` — fetch wrapper, error normalisation.
  - `credentials.ts`
  - `agentConfig.ts`
  - `onboarding.ts`
  - `worktreeCommands.ts`
  - `repositories.ts`

**Rationale**

CODING_STANDARDS §1 — one reason to change per file; today every API category cohabits.

**Acceptance criteria**

- Every new file ≤ ~150 LOC.
- A barrel `ui/src/api/index.ts` (or re-exports from `client.ts`) preserves existing import paths so consumers don't change — verify with `git grep -nE "from '.*api/client'" ui/src` showing zero broken imports (or all updated in this same commit if barrel is rejected).
- `npm run build` passes.
- All API shapes still live in `src/api/types.ts` (CODING_STANDARDS §3).

**Owner:** frontend

---

## P2 — Cleanups and policy fixes

### P2-1 · Split `crates/maestro-core/src/workflow/helpers.rs` and rename by responsibility

**Scope**

- `crates/maestro-core/src/workflow/helpers.rs` — three unrelated families.
- Replace with three named modules:
  - `workflow/ticket_context.rs` — `build_ticket_context`, `extract_acceptance_criteria`, `format_acceptance_criteria_block`.
  - `workflow/step_inspect.rs` — `step_already_succeeded`, `check_cancelled`, `parse_gh_issue_number`.
  - `workflow/text.rs` — `truncate_utf8_by_bytes`, `build_skill_search_paths`.

**Rationale**

"helpers" is a non-name. CODING_STANDARDS §1 "Name it after what it **does**".

**Acceptance criteria**

- File `crates/maestro-core/src/workflow/helpers.rs` no longer exists.
- `git grep -n "workflow::helpers" crates/` → zero hits.
- All call sites updated; `cargo build --workspace` zero warnings; `cargo test --workspace` green.

**Owner:** backend

---

### P2-2 · Remove `WorkflowEvent::default()`

**Scope**

- `crates/maestro-core/src/workflow/engine/types.rs:78` — the `Default` impl returns an empty/invalid `WorkflowEvent`.

**Rationale**

CODING_STANDARDS §1 (Liskov) — `Default::default()` must be a valid substitute for a constructed `WorkflowEvent`. An invalid one violates the trait contract. CODING_STANDARDS §2 — no dead code.

**Acceptance criteria**

- The `impl Default for WorkflowEvent` block is removed.
- `cargo build --workspace` shows the compiler flagging every remaining caller (if any). Either re-construct the event explicitly at each site, or delete the call.
- No new `_ = WorkflowEvent { … ..Default::default() }` patterns introduced.

**Owner:** backend

---

### P2-3 · Audit and fix the 37 production `.unwrap()` / `.expect()` sites

**Scope**

- 37 sites in production code (exclude `tests_phase2a_master_key.rs`, which is `#[cfg(test)]` at parent).
- Hot spots called out by the audit:
  - `crates/maestro-core/src/process.rs:193–194, 254–255`
  - `crates/maestro-web/src/server.rs:445, 458, 463`
  - `crates/maestro-web/src/routes/sessions.rs:139, 160, 240, 384, 624`
  - `crates/maestro-web/src/routes/credentials.rs:395, 598`
  - `crates/maestro-web/src/routes/workflows.rs:1184`
  - `crates/maestro-core/src/auth/bundle.rs:135`
  - `crates/maestro-core/src/auth/master_key.rs:291`
  - `crates/maestro-core/src/db/credentials.rs:77, 89`
- For each: replace with `?`, an explicit `match`, or — if the invariant is truly compile-time-knowable — an `// SAFETY: …` comment and an `.expect("static invariant: …")` with a descriptive message (rare; the bar is high).
- The 14 unwraps in `crates/maestro-web/src/test_helpers.rs` are addressed by P1-5's `cfg`-gating, not by rewrite.

**Rationale**

CODING_STANDARDS §2 — "No `.unwrap()` or `.expect()` in non-test code."

**Acceptance criteria**

- `rg -nE "\\.(unwrap|expect)\\(" crates/ --type rust | grep -vE "/(tests|test_helpers)\\.rs|#\\[cfg\\(test\\)\\]"` produces **0 hits** that aren't covered by an inline `// SAFETY:` comment.
- A grep for `expect("` in production sources, when present, is co-located with a `// SAFETY:` line.
- `cargo build --workspace` zero warnings; `cargo test --workspace` green.

**Owner:** backend (sweep can be split across the team; recommended single-PR audit then per-file fix PRs)

---

### P2-4 · Replace module-scope mutable counters in the UI

**Scope**

- `ui/src/hooks/useToast.tsx:28` — `let toastId = 0`.
- `ui/src/hooks/useWorkflows.ts:34` — `let errorIdCounter = 0`.
- `ui/src/components/MarkdownPreview.tsx:19, 35`.
- `ui/src/api/mocks.ts:43, 78, 98`.
- Replace with React 19 `useId()` where the value is used inside a component, `crypto.randomUUID()` for ad-hoc unique IDs, or a hook-local `useRef(0)` if a monotonically-increasing integer is truly needed.
- **Not** in scope: the `mermaid` singleton — keep as-is (acceptable singleton).

**Rationale**

Module-scope `let` is hidden mutable state — breaks SSR, breaks hot-reload, breaks tests that expect determinism. CODING_STANDARDS §3 "colocate state as close to its consumer as possible".

**Acceptance criteria**

- `rg -nE "^let \\w+( *:|.*= ?[0-9])" ui/src/` returns zero hits in component/hook files (mocks may legitimately keep counters scoped inside a factory function).
- `npm run build` passes.
- Toast IDs and workflow-error IDs are still unique under rapid-fire generation (manual smoke check or a small unit test using fake timers).

**Owner:** frontend

---

### P2-5 · Document the bundled-runtime-image trade-off and add Docker feature flags

**Scope**

- `Dockerfile` (runtime stage, lines ~56–293).
- Add a preamble comment block stating: "Maestro intentionally bakes Rust toolchain + 4 AI CLIs + openvscode-server + Playwright deps. Trade-off: larger image vs. zero first-run install latency for advertised features. See lore/code-quality-principles.md and AGENTS.md § Tool layout."
- Add build args (with sensible defaults that match today's image):
  - `ARG WITH_CODEX=true`
  - `ARG WITH_OPENCODE=true`
  - `ARG WITH_CURSOR=true`
- Gate the respective install blocks behind `RUN if [ "$WITH_X" = "true" ]; then …; fi` (or equivalent multi-stage selection).
- Do **not** remove anything from the default image. The change is opt-out-only.

**Rationale**

We accept the kitchen-sink trade-off (see `lore/code-quality-principles.md`) but admins building custom images via `FROM maestro:latest` should be able to skip CLIs they don't use. AGENTS.md § "Don't move a baked agent CLI to provisioning" is preserved — these are build-args, not provisioning entries.

**Acceptance criteria**

- `docker build .` with **no args** produces an image whose installed binaries match HEAD's image (`docker run … which codex opencode cursor-agent claude` all succeed).
- `docker build --build-arg WITH_CODEX=false --build-arg WITH_OPENCODE=false .` produces an image where `codex` and `opencode` are absent (`which codex` fails) but `claude` and `cursor-agent` are present.
- The preamble comment block is present at the top of the runtime stage.
- `AGENTS.md` § Tool layout updated with a one-paragraph note pointing to the new build args.

**Owner:** cross-cutting

---

## Cross-cutting acceptance gates (apply to every item)

These apply on top of each item's own criteria — never weaken or replace them:

1. `cargo build --workspace` produces **zero warnings**.
2. `cargo test --workspace` is **green**.
3. `npm run build` in `ui/` is **green** with `strict: true` (after P0-2 lands).
4. **No public API change** unless the item explicitly calls one out; verify by diffing every exported symbol in changed crates.
5. **One logical change per commit** (CODING_STANDARDS §5).
6. If documented behaviour changes — REST paths, WebSocket frames, config keys, crate layout, workflow sequencing — **`AGENTS.md` updated in the same commit**.
7. No new `.unwrap()` / `.expect()` introduced (production code).
8. No new `as`/`as unknown as` cast in TS without an inline `// SAFETY:` comment justifying it.

---

## Item count

- **P0:** 4
- **P1:** 10
- **P2:** 5
- **Total:** 19
