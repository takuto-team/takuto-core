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

## Status (2026-05-21)

The 2026-05-21 clean-code audit (`lore/audits/2026-05-21-clean-code.md`) executed Phases 1, 3, and 5 of the plan in `lore/audits/2026-05-21-plan.md` — module splits (`config/`, `routes/workflows/`), Docker image hardening, and React UI component splits respectively. See `git log --since=2026-05-21` for the commit trail.

**Phases 2 (typed `TakutoError`) and 4 (async hygiene) were deferred to this backlog per the option-2 wrap decision** — captured below as `P1-D-1` and `P1-D-2`. The audit context (worst offender rankings, systemic smells, prioritised fix order) is in the audit file; the verbatim scope / ACs / risks / verifier blocks are reproduced here so a future picker can work without flipping documents. Several P0/P1 entries below were superseded by earlier work — see the "Overlap" section of `lore/audits/2026-05-21-plan.md` for a per-item status.

---

## P0 — Top-priority fixes (audit §8)

### P0-1 · Split `crates/takuto-core/src/container.rs` into a `container/` module

**Scope**

- `crates/takuto-core/src/container.rs` (2,699 prod LOC, 52 module items, 3,816 LOC including tests).
- New tree:

  ```
  crates/takuto-core/src/container/
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

- `wc -l crates/takuto-core/src/container/*.rs` shows every file ≤ ~400 LOC of non-test code.
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

### P0-3 · Split `crates/takuto-core/src/workflow/engine/driver.rs` into focused engine sub-modules

**Scope**

- `crates/takuto-core/src/workflow/engine/driver.rs` (2,162 LOC, 22 fns, 0 inline tests).
- Keep `driver.rs` itself for `drive_workflow_def` plus the small glue it owns (~400 LOC).
- New siblings under `crates/takuto-core/src/workflow/engine/`:

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
- `crates/takuto-core/src/workflow/engine/driver.rs` keeps `drive_workflow_def` as its single entry-point.
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

### P1-1 · Split `crates/takuto-web/src/routes/workflows.rs` into an 8-file module

**Scope**

- `crates/takuto-web/src/routes/workflows.rs` (2,313 LOC, 23 handlers).
- New tree under `crates/takuto-web/src/routes/workflows/`:
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
- `cargo test -p takuto-web` and the existing integration tests pass.
- No public type renamed or moved across crate boundaries.

**Owner:** backend

---

### P1-2 · Split `crates/takuto-core/src/config.rs` into a `config/` module

**Scope**

- `crates/takuto-core/src/config.rs` (1,512 prod LOC, 22 pub structs, 3 `impl Config` blocks).
- New tree:
  - `config/mod.rs` — facade, re-exports.
  - `config/schema.rs` — the structs + `Default` impls.
  - `config/interpolation.rs` — `interpolate_*`, `validate_extra_args`, `validate_cors_origin`, `cursor_model_for_cli`.
  - `config/load.rs` — `Config::load*`, `validate`, `provisioning_sha`, `resolve_config_relative_path`, `detect_legacy_command_keys`.

**Rationale**

Three orthogonal concerns (data shape, value interpolation, file I/O + validation) currently share one file. Splitting them makes `provisioning_sha` and validation rules unit-testable in isolation.

**Acceptance criteria**

- `wc -l crates/takuto-core/src/config/*.rs` ≤ ~600 LOC each (schema may be the largest).
- Public type paths preserved (`takuto_core::Config`, `takuto_core::config::Config`, etc.) — confirm with `git grep -nE "use takuto_core::(config::)?Config"`.
- `cargo test -p takuto-core` passes.
- `AGENTS.md` § Configuration paths reviewed; update file references if any are named.

**Owner:** backend

---

### P1-3 · Promote `TakutoError` variants to typed `#[from]` wiring

**Scope**

- `crates/takuto-core/src/error.rs` — today 10 / 13 variants carry `String` payloads.
- Add `#[from] rusqlite::Error`, `#[from] serde_json::Error`, `#[from] chrono::ParseError` (drop the lossy `.to_string()` conversions).
- Promote `Jira`, `Git`, `Claude`, `AiAgent`, `Auth`, `Database`, `Config` variants to wrap `#[source] Box<dyn std::error::Error + Send + Sync>` (the **public crate-boundary** rule in CODING_STANDARDS §2 forbids `Box<dyn Error>` in a public API — these wrappers are internal to the `TakutoError` enum, not the public return type, so they comply; we only widen the source storage).
- Sweep the 41 `.map_err(|e| TakutoError::*(e.to_string()))` call sites and replace with `?` where the new `#[from]` covers it, or with explicit `TakutoError::Variant { source: Box::new(e) }` otherwise.

**Rationale**

`String` payloads break `std::error::Error::source()` — log output shows only the top-level message, never the root cause. CODING_STANDARDS §2 "Define errors with thiserror" is honoured in name but defeated in payload shape.

**Acceptance criteria**

- `grep -nE "TakutoError::\\w+\\(.*\\.to_string\\(\\)\\)" crates/ | wc -l` reports **zero** (down from 41).
- `cargo test --workspace` green; new unit test in `error.rs` confirms `TakutoError::Database(...).source().is_some()` for a wrapped `rusqlite::Error`.
- No public function signature change (callers still return `TakutoError`).
- No `Box<dyn Error>` in any **return type**; confined to the `TakutoError` variant's internal `#[source]` field.
- `cargo build --workspace` zero warnings.

**Owner:** backend

---

### P1-4 · Encapsulate `Workflow` and `AppState` field visibility

**Scope**

- `crates/takuto-core/src/workflow/engine/types.rs:131` — `Workflow` has 31 `pub` fields.
  - Keep `pub`: `id`, `ticket_key`, `state`, `started_at`.
  - Convert dashboard-projection / driver fields to `pub(crate)`: `current_step_label`, `terminal_lines`, `driver_started`, plus any other field mutated by the engine internals.
  - Expose **accessor methods** that enforce state-machine invariants for any field external callers actually need to write today (audit `engine/driver.rs:298` and similar — `wf.state = …` direct writes must go through a `try_transition()` method).
- `crates/takuto-web/src/state.rs` — `AppState` has 28 `pub` fields. Convert to `pub(crate)` minimum. Anything truly needed at the crate boundary stays `pub`; everything else drops a level.

**Rationale**

CODING_STANDARDS §1 (Open/Closed) — direct field mutation bypasses the state machine; future invariants can't be enforced. §2 "`pub(crate)` by default for internal items; `pub` only at true crate boundaries".

**Acceptance criteria**

- `grep -nE "wf\\.state = " crates/ | wc -l` → 0 (all writes go through an accessor).
- `git grep -n "pub " crates/takuto-core/src/workflow/engine/types.rs` shows only the 4 retained public fields plus the new accessor methods.
- `cargo build --workspace` zero warnings; no caller outside the crate broke (we didn't remove any externally-needed field, just narrowed its visibility).
- New `///` doc on each accessor method.

**Owner:** backend

---

### P1-5 · Gate `crates/takuto-web/src/test_helpers.rs` so it doesn't ship in prod

**Scope**

- `crates/takuto-web/src/lib.rs:22` — change `pub mod test_helpers;` to one of:
  - `#[cfg(any(test, feature = "test-utils"))] pub mod test_helpers;` with a corresponding `test-utils = []` feature in `crates/takuto-web/Cargo.toml`, **or**
  - Relocate the module to a sister `takuto-web-testing` crate listed as a `[dev-dependencies]` of `takuto-web`.

**Rationale**

CODING_STANDARDS §2 "Tests go in `#[cfg(test)] mod tests`". Test helpers shipped to the public surface bloat the binary and let production callers depend on test fakes by accident.

**Acceptance criteria**

- `cargo build -p takuto-web --release` succeeds and `cargo expand -p takuto-web --release | grep test_helpers` (or equivalent) shows the module compiled out.
- `cargo test -p takuto-web --features test-utils` (or via the new crate) still compiles every existing test.
- No callsite in production code (`crates/takuto-web/src/{routes,middleware,state,server}.rs`) references `test_helpers::*`.
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

### P2-1 · Split `crates/takuto-core/src/workflow/helpers.rs` and rename by responsibility

**Scope**

- `crates/takuto-core/src/workflow/helpers.rs` — three unrelated families.
- Replace with three named modules:
  - `workflow/ticket_context.rs` — `build_ticket_context`, `extract_acceptance_criteria`, `format_acceptance_criteria_block`.
  - `workflow/step_inspect.rs` — `step_already_succeeded`, `check_cancelled`, `parse_gh_issue_number`.
  - `workflow/text.rs` — `truncate_utf8_by_bytes`, `build_skill_search_paths`.

**Rationale**

"helpers" is a non-name. CODING_STANDARDS §1 "Name it after what it **does**".

**Acceptance criteria**

- File `crates/takuto-core/src/workflow/helpers.rs` no longer exists.
- `git grep -n "workflow::helpers" crates/` → zero hits.
- All call sites updated; `cargo build --workspace` zero warnings; `cargo test --workspace` green.

**Owner:** backend

---

### P2-2 · Remove `WorkflowEvent::default()`

**Scope**

- `crates/takuto-core/src/workflow/engine/types.rs:78` — the `Default` impl returns an empty/invalid `WorkflowEvent`.

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
  - `crates/takuto-core/src/process.rs:193–194, 254–255`
  - `crates/takuto-web/src/server.rs:445, 458, 463`
  - `crates/takuto-web/src/routes/sessions.rs:139, 160, 240, 384, 624`
  - `crates/takuto-web/src/routes/credentials.rs:395, 598`
  - `crates/takuto-web/src/routes/workflows.rs:1184`
  - `crates/takuto-core/src/auth/bundle.rs:135`
  - `crates/takuto-core/src/auth/master_key.rs:291`
  - `crates/takuto-core/src/db/credentials.rs:77, 89`
- For each: replace with `?`, an explicit `match`, or — if the invariant is truly compile-time-knowable — an `// SAFETY: …` comment and an `.expect("static invariant: …")` with a descriptive message (rare; the bar is high).
- The 14 unwraps in `crates/takuto-web/src/test_helpers.rs` are addressed by P1-5's `cfg`-gating, not by rewrite.

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
- Add a preamble comment block stating: "Takuto intentionally bakes Rust toolchain + 4 AI CLIs + openvscode-server + Playwright deps. Trade-off: larger image vs. zero first-run install latency for advertised features. See lore/code-quality-principles.md and AGENTS.md § Tool layout."
- Add build args (with sensible defaults that match today's image):
  - `ARG WITH_CODEX=true`
  - `ARG WITH_OPENCODE=true`
  - `ARG WITH_CURSOR=true`
- Gate the respective install blocks behind `RUN if [ "$WITH_X" = "true" ]; then …; fi` (or equivalent multi-stage selection).
- Do **not** remove anything from the default image. The change is opt-out-only.

**Rationale**

We accept the kitchen-sink trade-off (see `lore/code-quality-principles.md`) but admins building custom images via `FROM takuto:latest` should be able to skip CLIs they don't use. AGENTS.md § "Don't move a baked agent CLI to provisioning" is preserved — these are build-args, not provisioning entries.

**Acceptance criteria**

- `docker build .` with **no args** produces an image whose installed binaries match HEAD's image (`docker run … which codex opencode cursor-agent claude` all succeed).
- `docker build --build-arg WITH_CODEX=false --build-arg WITH_OPENCODE=false .` produces an image where `codex` and `opencode` are absent (`which codex` fails) but `claude` and `cursor-agent` are present.
- The preamble comment block is present at the top of the runtime stage.
- `AGENTS.md` § Tool layout updated with a one-paragraph note pointing to the new build args.

**Owner:** cross-cutting

---

## P1 — Deferred from 2026-05-21 audit (option-2 wrap)

These two entries reproduce Phase 2 and Phase 4 of `lore/audits/2026-05-21-plan.md` verbatim (Scope, Acceptance criteria, Commit shape, Risks, Verifier). They were deferred when the 2026-05-21 sprint scoped down to Phases 1, 3, 5 only. **Read these alongside the audit (`lore/audits/2026-05-21-clean-code.md`, §4 systemic smells, §6 cut plans) and the plan (`lore/audits/2026-05-21-plan.md`, cross-phase invariants)** before picking either item up — the audit captures the *why* (blast-radius ranking, baseline metric counts) and the plan captures the *order constraint* (Phase 4 originally depended on Phase 2 + Phase 1; Phase 1 has shipped, so the remaining dependency is `P1-D-1 → P1-D-2`).

---

### P1-D-1 · Phase 2 — Restructure `TakutoError` with typed payloads

**Owner:** backend

**Source documents:** `lore/audits/2026-05-21-clean-code.md` §3 worst offender #8 (`error.rs:13`), §4 "Stringly-typed `TakutoError`"; `lore/audits/2026-05-21-plan.md` §"Phase 2".

**Audit drivers:** §4 "Stringly-typed `TakutoError`" (11/16 variants wrap `String`; 33 `Result<_, String>` signatures); §3 worst offender #8 (`error.rs:13`).

#### Scope

- `crates/takuto-core/src/error.rs` — replace `String`-wrapped variants with typed payloads:
  - `Jira { ticket: String, action: String, source: Box<dyn std::error::Error + Send + Sync> }`
  - `Git { op: String, path: PathBuf, stderr: String }`
  - `GitHubApp { source: Box<dyn std::error::Error + Send + Sync> }`
  - `Claude { source: Box<dyn std::error::Error + Send + Sync> }`
  - `AiAgent { provider: String, source: Box<dyn std::error::Error + Send + Sync> }`
  - `Database(#[from] rusqlite::Error)` — replace the manual `impl From` block with `#[from]` on the variant.
  - `Auth { kind: AuthKind, source: Box<dyn std::error::Error + Send + Sync> }` where `AuthKind` is a new enum (`Pin`, `Token`, `MasterKey`, …).
  - `Config { section: String, reason: String }`
  - Add `#[from] reqwest::Error`, `#[from] serde_json::Error`, `#[from] chrono::ParseError` where they currently lose their cause through `.to_string()`.
- Sweep every `Result<_, String>` signature (33 per audit) and replace with `Result<_, TakutoError>`. Touched crates:
  - `crates/takuto-core/src/**`
  - `crates/takuto-web/src/routes/**` (post-Phase-1 split)
  - `crates/takuto-cli/src/main.rs` — including `run_server`'s `Box<dyn Error>` return type → `Result<(), TakutoError>`.
- Sweep every `TakutoError::*(e.to_string())` call site (audit lists 41 such); replace with `?` where `#[from]` covers it, or `TakutoError::Variant { source: Box::new(e), … }` otherwise.

#### Out of scope

- **No** new file moves. Phase 1 already reshaped the layout.
- **No** error-handling logic changes beyond payload typing — same fallback behaviour, same retry policy, same user-visible message text (the `#[error("…")]` strings are preserved verbatim where they exist).
- **No** changes to `tokio::spawn` patterns, lock-across-await sites, or `.clone()` cleanup — those are `P1-D-2`.
- **No** touching error sites in `test_helpers.rs` or `#[cfg(test)]` blocks — they already use `unwrap()` and that is acceptable per CODING_STANDARDS §2.

#### Acceptance criteria

- `grep -rnE "Result<[^,]+, ?String>" crates/*/src --include="*.rs" | wc -l` returns **0**.
- `grep -rnE "TakutoError::\w+\([^)]*\.to_string\(\)" crates/ --include="*.rs" | wc -l` returns **0** (down from ~41).
- `grep -nE "Box<dyn .*Error>" crates/takuto-cli/src/main.rs` returns **0**.
- `grep -cE "^\s+(Jira|Git|GitHubApp|Claude|AiAgent|Database|Auth|Config)\s*\(String\)" crates/takuto-core/src/error.rs` returns **0**.
- `grep -cE "#\[from\]" crates/takuto-core/src/error.rs` returns **≥ 5** (Io + TomlParse already present, plus rusqlite, reqwest, serde_json, chrono).
- The manual `impl From<rusqlite::Error> for TakutoError` block at the bottom of `error.rs` is gone (replaced by `#[from]` on the variant).
- A new unit test in `error.rs` asserts `TakutoError::from(rusqlite_err).source().is_some()` — i.e. the wrapped cause is recoverable, not stringified.
- `cargo build --workspace` exits 0 with **zero warnings**.
- `cargo test --workspace` exits 0.
- The public signature of `run_server` is `pub async fn run_server(...) -> Result<(), TakutoError>`; verify with `grep -nE "fn run_server" crates/takuto-cli/src/main.rs`.

#### Commit shape

- **One PR, ~6–8 commits.**
  1. Restructure `error.rs` variants (compiler errors expected across the workspace from now on; do not fix yet).
  2. Add `#[from]` impls and the new test.
  3–N. Sweep call sites, one crate at a time (`takuto-core`, then `takuto-web`, then `takuto-cli`). Each commit leaves `cargo build` green.
  - Final commit: delete any remaining `Result<_, String>` aliases / re-exports.

#### Risks and mitigations

- **Risk:** Promoting `Database(String)` to `Database(#[from] rusqlite::Error)` changes the variant's tuple arity; any `match TakutoError::Database(s)` site breaks. **Mitigation:** the compiler flags every site; fix them mechanically in the same commit. The audit listed the hot sites — `crates/takuto-core/src/db/credentials.rs`, `db/auth.rs`, etc.
- **Risk:** `Box<dyn Error>` in the variant payload looks like it violates CODING_STANDARDS §2 ("no `Box<dyn Error>` in public API"). **Mitigation:** the §2 rule applies to **return types**, not internal `#[source]` fields. `TakutoError` itself remains the public return type; the boxed source is an implementation detail of one variant. Add an inline comment in `error.rs` documenting this distinction.
- **Risk:** sweeping `Result<_, String>` may surface latent bugs where a handler relied on `.map_err(|e| e.to_string())` to flatten an `anyhow`-style chain. **Mitigation:** keep the test suite green between every commit; if a test starts failing on the typed error, the failure mode was already wrong and the test is the source of truth.

#### Verifier

The tester runs:
1. `cargo test --workspace`
2. The three `grep` ACs above (`Result<_, String>` count, `to_string()` call sites, `Box<dyn .*Error>` count).
3. Adds a new test that pattern-matches on a typed `TakutoError::Database { .. }` or `TakutoError::Jira { ticket, .. }` variant from a real failure path and confirms the payload fields carry structured data.

---

### P1-D-2 · Phase 4 — Async hygiene (`tokio::spawn` JoinSet, `await_holding_lock`, `eprintln!` sweep)

**Owner:** backend

**Source documents:** `lore/audits/2026-05-21-clean-code.md` §3 worst offender #9 (`docker_hooks.rs`), §4 "Fire-and-forget `tokio::spawn`", §4 "Lock-across-`.await`", §4 "`eprintln!` in production paths"; `lore/audits/2026-05-21-plan.md` §"Phase 4".

**Audit drivers:** §4 "Fire-and-forget `tokio::spawn`" (21 of 34 spawns drop their handle); §4 "Lock-across-`.await`" (heuristic 328 sites); §4 "`eprintln!` in production paths" (42, of which 19 in `docker_hooks.rs`); §3 worst offender #9.

**Depends on `P1-D-1`** (typed `TakutoError`). Originally also depended on Phase 1 module splits, which have shipped.

#### Scope

- **Enable `clippy::await_holding_lock` workspace-wide.** Add to `crates/takuto-core/src/lib.rs` and `crates/takuto-web/src/lib.rs`:
  ```rust
  #![deny(clippy::await_holding_lock)]
  ```
  Or, preferred, add to root `Cargo.toml`:
  ```toml
  [workspace.lints.clippy]
  await_holding_lock = "deny"
  ```
  Fix every flagged site.
- **Route every fire-and-forget `tokio::spawn` through a `JoinSet`** owned by the relevant lifetime holder:
  - `WorkflowEngine` gains a `tasks: tokio::task::JoinSet<()>` field (or `Arc<Mutex<JoinSet<()>>>` if cross-task abort is needed).
  - `AppState` gains a `background_tasks: tokio::task::JoinSet<()>` field.
  - Every spawn site uses `engine.tasks.spawn(...)` / `state.background_tasks.spawn(...)`.
  - Graceful shutdown drains the `JoinSet` on engine/app teardown (this is already partially wired via `CancellationToken`; integrate the `JoinSet` with the existing token).
- **Replace 42 `eprintln!`/`println!` in `crates/*/src/**`** with `tracing::info!`/`tracing::warn!`/`tracing::error!` per the call's existing severity. `docker_hooks.rs` (19 sites) is the hot spot.
- **Replace the 5 prod-path `unwrap()`/`expect()`** flagged in the audit:
  - `crates/takuto-cli/src/main.rs:1204` (`SIGTERM` handler install) → `.expect("static invariant: signal handler installation cannot fail on supported platforms")` with an inline `// SAFETY:` comment, or propagate via `?` if practical.
  - `crates/takuto-cli/src/main.rs:1216` (Ctrl+C handler) → same.
  - Any remaining hits in `crates/takuto-core/src/{server,routes}/**` outside test files.
- Production-code count: `cargo test --workspace` files like `github_app.rs`, `skill_resolve.rs`, `config_watcher.rs` use `.unwrap()` inside `#[cfg(test)]` blocks — those are **out of scope** (test code is allowed per CODING_STANDARDS §2).

#### Out of scope

- **No** file moves (Phase 1 has shipped).
- **No** error-type changes (`P1-D-1`'s job — do that first).
- **No** `.clone()` reduction — the audit flagged 793 clones; that is a separate item beyond this phase. This phase only touches lines that have a `tokio::spawn`, a lock-across-await, or a `println!`/`eprintln!`/`unwrap()` on them.
- **No** UI changes.
- **No** new `tracing` subscribers — assume the existing `tracing_subscriber` setup; only the call sites change.

#### Acceptance criteria

- `cargo clippy --workspace --all-targets -- -D warnings` exits 0. (The `await_holding_lock` lint is enabled and every flagged site is fixed.)
- `grep -rnE "tokio::spawn\(" crates/*/src --include="*.rs" | grep -vE "(joinset|tasks|background_tasks)\.spawn\(" | wc -l` returns **0**. Every spawn goes through a `JoinSet`.
- `grep -rnE "^\s+(eprintln|println)!" crates/*/src --include="*.rs" | grep -v "^.*tests\.rs:" | grep -v "#\[cfg(test)\]" | wc -l` returns **0**.
- `grep -rnE "\.(unwrap|expect)\(" crates/*/src --include="*.rs" | grep -vE "(tests?\.rs|test_helpers|#\[cfg\(test\)\])" | wc -l` returns **≤ 0 sites without an adjacent `// SAFETY:` comment**. (Any remaining `.expect("static invariant: …")` must be preceded by a `// SAFETY:` line — verify with `awk` script: every match in production code has a `// SAFETY:` on the previous non-blank line.)
- `WorkflowEngine` struct definition contains a `JoinSet` field; `grep -nE "JoinSet" crates/takuto-core/src/workflow/engine/mod.rs` returns ≥ 1.
- `AppState` contains a `JoinSet` field; `grep -nE "JoinSet" crates/takuto-web/src/state.rs` returns ≥ 1.
- `cargo build --workspace` exits 0 with **zero warnings**.
- `cargo test --workspace` exits 0.
- A new test in `engine/mod.rs` (or a new `engine/shutdown.rs`) asserts: spawn a background task that loops on a `tokio::time::sleep`; call `engine.shutdown()`; assert the task's `JoinHandle` aborts within 100 ms.

#### Commit shape

- **One PR, ~6 commits:**
  1. Enable `clippy::await_holding_lock` and fix every flagged site (one commit per crate if it's noisy).
  2. Add `JoinSet` field to `WorkflowEngine`; migrate `WorkflowEngine`-owned spawns.
  3. Add `JoinSet` field to `AppState`; migrate `AppState`-owned spawns.
  4. Wire `JoinSet` drain into the existing `CancellationToken` shutdown path; add the shutdown test.
  5. Replace `println!`/`eprintln!` → `tracing::*` across `docker_hooks.rs` and the other 23 sites.
  6. Replace the 5 prod-path `unwrap()`/`expect()` per the list above.

#### Risks and mitigations

- **Risk:** enabling `await_holding_lock` produces a large lint wall (heuristic 260+ matches). **Mitigation:** prioritise by lock type — `std::sync::Mutex` held across `.await` is a real bug; `parking_lot::RwLock::read()` released before `.await` is benign and clippy will not flag it. The lint only catches the former. Expect ~10–30 real flags, not 260.
- **Risk:** retrofitting `JoinSet` ownership on `WorkflowEngine` may require an `Arc<Mutex<JoinSet>>` if spawns happen from cloned handles. **Mitigation:** keep the `JoinSet` behind a `tokio::sync::Mutex` only if cross-task spawn is genuinely needed; otherwise own it directly and route all spawns through engine-owned methods.
- **Risk:** swapping `eprintln!` → `tracing::warn!` in `docker_hooks.rs` may change output destination during test runs that capture stderr. **Mitigation:** the test suite uses `tracing_test` or equivalent; verify a sample test's expected output still matches.
- **Risk:** the SIGTERM/Ctrl+C `expect` calls actually can't fail on Linux/macOS; rewriting them with `?` adds noise. **Mitigation:** keep `.expect(...)` with a precise message and an inline `// SAFETY: signal handler installation is infallible on tokio-supported platforms` comment — this is the rare case CODING_STANDARDS §2 permits.

#### Verifier

The tester runs:
1. `cargo clippy --workspace --all-targets -- -D warnings`
2. `cargo test --workspace` including the new shutdown test
3. The four `grep` ACs above (`tokio::spawn`, `println!`, `unwrap()`, `Box<dyn Error>`)
4. A local smoke run: start a workflow, send `SIGTERM` to the server, confirm the workflow's background tasks are aborted (no orphaned `tokio` threads in `ps`).

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
- **P1 (deferred from 2026-05-21 audit):** 2 (`P1-D-1`, `P1-D-2`)
- **P2:** 5
- **Total:** 21

See the "Status (2026-05-21)" section near the top of this file for which P0/P1 entries above were shipped or superseded; the per-item overlap matrix is in `lore/audits/2026-05-21-plan.md` §"Overlap with `lore/refactor-backlog.md`".
