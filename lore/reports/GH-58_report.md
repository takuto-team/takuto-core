# GH-58: Dynamic YAML Workflow Definitions

## Summary

Refactored the Takuto workflow system to support fully user-customizable secondary workflows via YAML files. Workflows are discovered dynamically from a `workflows/` directory, support inter-workflow dependencies, and are launchable from the dashboard UI with dependency-aware button states. Existing TOML workflow templates were migrated to the new YAML format.

## Architecture

### New Layer Design
The implementation adds YAML workflow definitions as a **new layer alongside the existing system**, preserving full backward compatibility. The main pipeline (assign -> retrieve -> worktree -> agent steps -> finalize) remains unchanged. YAML-defined workflows are triggered as secondary workflows via buttons on the workflow card, executing within an existing workflow's worktree context.

### Core Components

| Component | Location | Purpose |
|-----------|----------|---------|
| `definitions.rs` | `takuto-core/src/workflow/` | YAML parsing, discovery, validation, dependency resolution |
| Engine extensions | `takuto-core/src/workflow/engine.rs` | Runtime execution, state management, file watching |
| REST endpoints | `takuto-web/src/routes/workflows.rs` | API for listing definitions, starting/retrying runs |
| `WorkflowDefButtons.tsx` | `ui/src/components/` | Dependency-aware UI buttons |

## Changes

### Backend (Rust)

**New files:**
- `crates/takuto-core/src/workflow/definitions.rs` (944 lines) — Core module for YAML workflow definitions including:
  - YAML schema types (`WorkflowYaml`, `WorkflowStepYaml`, `DiscoveredWorkflow`)
  - `discover_workflows()` — scans directory, parses files, validates dependencies
  - `detect_cycles()` — DFS cycle detection for circular dependencies
  - `are_dependencies_met()` — checks upstream completions before enabling
  - `topological_order()` — Kahn's algorithm for execution ordering
  - Short form (`run: "cmd"`) and full form step support
  - Automatic skip of `*.example.yml` files
  - 15 unit tests covering parsing, validation, cycles, dependencies

**Modified files:**
- `crates/takuto-core/src/workflow/engine.rs` (+556 lines):
  - `start_workflow_def()` / `retry_workflow_def()` — entry points for running definitions
  - `drive_workflow_def()` / `run_workflow_def_steps()` — async drivers using existing `run_agent_step_sequence()`
  - `start_definitions_watcher()` — background task polling every 5s, broadcasting WebSocket events on changes
  - `workflow_def_runs: HashMap<String, WorkflowDefRunState>` on `Workflow` struct
- `crates/takuto-core/src/config.rs` — `workflow_definitions_dir` field on `GeneralConfig` (default `"workflows"`)
- `crates/takuto-core/src/workflow/snapshot.rs` — `workflow_def_runs` persistence in snapshots
- `crates/takuto-core/src/workflow/mod.rs` — registered `definitions` module
- `crates/takuto-core/src/workflow/dashboard_progress.rs` — test helper compatibility
- `crates/takuto-core/Cargo.toml` / `Cargo.toml` — `serde_yaml` workspace dependency
- `crates/takuto-web/src/routes/workflows.rs` — 3 new handlers:
  - `GET /api/workflow-definitions` — lists discovered definitions
  - `POST /api/workflows/{id}/run-workflow/{def}` — starts a definition
  - `POST /api/workflows/{id}/retry-workflow/{def}` — retries a failed definition
- `crates/takuto-web/src/server.rs` — route registration
- `crates/takuto-cli/src/main.rs` — resolves `workflows_dir`, passes to engine, starts watcher

### Frontend (React/TypeScript)

**New files:**
- `ui/src/components/WorkflowDefButtons.tsx` (216 lines) — Component with:
  - Topological sorting of definitions by `depends_on`
  - Five visual states: enabled, disabled (with lock icon + tooltip), running (spinner), completed (check), error (X, click to retry)
  - Dependency checking against `runStates`
  - API integration for start/retry actions

**Modified files:**
- `ui/src/api/types.ts` — `WorkflowDefinition` interface, `workflow_def_runs` on `WorkflowSummary`, `workflow_def_name` on `WorkflowEvent`
- `ui/src/components/WorkflowCard.tsx` — Renders `WorkflowDefButtons` in terminal and active state sections
- `ui/src/components/WorkflowGrid.tsx` — Passes `workflowDefs` prop through to cards
- `ui/src/pages/Dashboard.tsx` — Fetches definitions on mount, re-fetches on WebSocket events (debounced 500ms)

### Migration (TOML -> YAML)

Five YAML workflow files created from existing TOML templates:

| TOML Source | YAML Output | Workflow Name |
|-------------|-------------|---------------|
| `ticket.example.toml` | `ticket.example.yml` | "Implement Ticket" |
| `review.example.toml` | `review.example.yml` | "Address PR Comments" |
| `merge_base.example.toml` | `merge_base.example.yml` | "Merge Base Branch" |
| `review.toml` | `review.yml` | "Address PR Comments" |
| `merge_base.toml` | `merge_base.yml` | "Merge Base Branch" |

Existing TOML files left untouched for backward compatibility.

### Documentation
- `AGENTS.md` updated with:
  - Dynamic workflow definitions section (YAML schema, discovery, dependencies, state management)
  - New REST API endpoints in the route table
  - WebSocket `workflow_definitions_changed` event
  - `workflow_def_runs` field on `WorkflowSummary`
  - `WorkflowDefButtons` component description

## Verification

| Check | Result |
|-------|--------|
| `cargo check --workspace` | Pass |
| `cargo test --workspace` | 201 tests pass (183 core + 18 web) |
| `tsc --noEmit` (frontend) | Pass (zero errors) |
| `vite build` (frontend) | Pass |
| New unit tests in `definitions.rs` | 15 tests, all passing |

## Stats

- **Files modified:** 16
- **New files:** 2 (+ 5 YAML migrations)
- **Lines changed:** +787 / -11
- **New test count:** 15 unit tests for definitions module

## YAML Schema Reference

```yaml
name: "Workflow Display Name"     # required
depends_on:                       # optional
  - upstream_filename             # without .yml extension
steps:                            # required, at least one
  # Short form (command step):
  - run: "npm test"

  # Full form (agent step):
  - name: "Step name"
    prompt: |
      Multi-line prompt with {placeholders}
    skills:
      - name: "skill-name"
        args: ["arg1"]
    repeat: 1
    when: "always"                # always | ticketing | no_ticketing

  # Full form (command step):
  - name: "Build"
    commands:
      - "npm run build"
      - "npm run lint"
    repeat: 1
```

## Dependency State Machine

```
Idle ──[start]──> Running ──[success]──> Completed
                     │
                     └──[failure]──> Error ──[retry]──> Idle
```

Downstream workflows remain disabled (locked) until all `depends_on` entries reach `Completed` state. Failed workflows show error state with retry capability. Circular dependencies are detected at load time.

## Step: Code Review

- **Key findings**
  - Two bugs found and fixed in `drive_workflow_def` and `start_workflow_def` in `crates/takuto-core/src/workflow/engine.rs`:
    1. **Missing Stopped-state guard in `drive_workflow_def`**: When a user explicitly stops a workflow that has a running definition, the definition driver received a `Cancelled` error but fell through to the generic error handler, writing `WorkflowDefRunState::Error { "Cancelled" }`. This caused the UI to show a red error button with "Click to retry" for something that was intentionally stopped. Fixed by adding a state-snapshot check (matching the existing pattern in `drive_pr_review_workflow` and `drive_merge_base_workflow`) that early-returns when the parent workflow is `Stopped` or removed.
    2. **Reused parent cancel token in `start_workflow_def`**: Unlike `start_pr_review_workflow` and `start_merge_base_workflow` which create a fresh `CancellationToken`, the definition starter reused the parent workflow's token. Since `CancellationToken` never un-cancels, starting a definition on a previously-stopped workflow would immediately fail at the first `check_cancelled` call. Fixed by creating a fresh token under the write lock, matching the pattern of the existing secondary drivers.
  - Five additional low-severity suggestions identified but intentionally not fixed: `scan_definitions_dir` not skipping `.example.` files (spurious but harmless change events), TOCTOU race between locks (follows existing codebase patterns), `serde_yaml` 0.9 deprecated (functional), `discover_workflows` re-scanned on every call (negligible overhead), O(n²) sort in `topological_order` (negligible for typical workflow counts).
  - A challenger agent independently validated all findings; no false positives.

- **Issues encountered**
  - None. Both fixes compiled cleanly and all 183 existing tests continued to pass.

- **Decisions taken**
  - Fixed both 🟡 Warning-level bugs since they affect user-reachable scenarios (stopping a workflow with running definitions, starting definitions on stopped workflows).
  - Did not fix the five 🔵 Suggestion items because they are either consistent with existing codebase patterns, have negligible real-world impact, or are cosmetic/optimization concerns better suited for a follow-up.
  - Chose to create the fresh `CancellationToken` in the existing write-lock block (where `Running` state is set) rather than adding a separate write-lock acquisition, keeping the lock count minimal.

## Step: Code Review (Pass 2)

- **Key findings**
  - One convention violation found and fixed in `ui/src/components/WorkflowDefButtons.tsx`:
    1. **Inline styles instead of `wf-btn-success` class**: The completed-state button (lines 155–170) used hardcoded inline styles (`rgba(34,197,94,0.1)`, `#4ade80`, etc.) instead of the existing `.wf-btn-success` CSS class from `ui/src/styles/index.css`. All other button states in the same component use `wf-btn-*` CSS classes, and `wf-btn-success` is already used elsewhere in the codebase (`WorkflowCard.tsx:664`). The inline values were Tailwind v3-era RGB colors, inconsistent with the Tailwind v4 OKLCH palette the `wf-btn-success` class renders through. Fixed by replacing the `style` block and redundant utility classes with `className="action-btn wf-btn-success cursor-default"`.
  - A challenger agent initially rejected the finding, arguing that Tailwind v4's OKLCH-based `emerald-*` colors differ from the inline RGB values. This was re-evaluated: the color mismatch actually strengthened the case for using `wf-btn-success`, since the inline styles rendered a different shade of green from every other success-state button in the application.
  - No additional bugs, security issues, or correctness problems found across backend or frontend changes. The two bugs fixed in Pass 1 remain correct.

- **Issues encountered**
  - None. The fix compiled cleanly (`tsc --noEmit`, `vite build`, `cargo test --workspace` all pass, 201 tests).

- **Decisions taken**
  - Fixed the convention violation since it's new code that should follow the established `wf-btn-*` pattern. Using the existing class also ensures visual consistency if the project's Tailwind color palette changes in the future.
  - The `cursor-default` Tailwind utility was added to replace the inline `cursor: "default"` since `.action-btn` sets `cursor-pointer` by default and the completed state should not appear clickable.

## Step: Organize Commits

- **Key findings**
  - All 16 modified files and 2 new files (+ 1 untracked report) were directly related to the GH-58 ticket objective (Dynamic YAML Workflow Definitions). No out-of-scope changes were found.
  - Changes were organized into 5 logical commits by architectural layer, each with a conventional commit message:
    1. `feat(core): add YAML workflow definition parsing and validation` — serde_yaml dependency, `definitions.rs` module, `mod.rs` registration, `config.rs` additions (6 files)
    2. `feat(core): add engine support for running workflow definitions` — engine methods, snapshot persistence, dashboard_progress test compat, CLI wiring (4 files)
    3. `feat(web): add workflow definition API endpoints` — REST handlers and route registration (2 files)
    4. `feat(ui): add workflow definition buttons and dashboard integration` — types, new component, card/grid/dashboard integration (5 files)
    5. `docs: update AGENTS.md for dynamic workflow definitions` — architecture documentation (1 file)
  - The `lore/reports/GH-58_report.md` file was left uncommitted as a process artifact (generated by the automated workflow, not feature code).

- **Issues encountered**
  - None. Commit boundaries were chosen to keep each commit independently compilable: the definitions module is self-contained, engine changes include CLI wiring (required due to `WorkflowEngine::new()` signature change), web endpoints depend only on already-committed engine methods, and UI changes are independent of Rust compilation.

- **Decisions taken**
  - Grouped engine struct changes, engine methods, snapshot persistence, and CLI wiring into a single commit rather than splitting them, because `WorkflowEngine::new()` signature change required both the engine and CLI to be updated together for compilation.
  - Placed AGENTS.md documentation in a separate final commit following the existing project convention (e.g., prior commit `e33e769 docs: update AGENTS.md for add-to-dashboard two-step flow`).
  - Did not discard any changes — all modifications were verified to be in-scope for the dynamic YAML workflow definitions feature.

## Step: Fix lint warnings, format, and failing tests

- **Key findings**
  - `cargo clippy -- -D warnings` reported 4 lint errors across 2 files changed in this ticket:
    - `definitions.rs`: derivable `Default` impl (clippy::derivable_impls) — replaced manual impl with `#[derive(Default)]` + `#[default]` on the `Idle` variant.
    - `definitions.rs`: `&mut Vec<_>` parameter where `&mut [_]` suffices (clippy::ptr_arg) — changed `validate_dependencies` signature to accept `&mut [DiscoveredWorkflow]`.
    - `engine.rs`: two collapsible `if` blocks (clippy::collapsible_if) — merged nested conditions using let-chains.
  - `cargo test` passed all 201 tests (183 in takuto-core, 18 in takuto-web) on the first run — no test failures.
  - `cargo fmt --check` reported formatting differences across 3 files (`main.rs`, `definitions.rs`, `engine.rs`); `cargo fmt` applied them.
  - After all fixes, all three checks (`clippy`, `test`, `fmt --check`) pass cleanly with zero warnings/errors.

- **Issues encountered**
  - The `/usr/local/cargo` directory was owned by root, causing `cargo clippy` to fail on first run with "Permission denied" when downloading crates. Resolved by setting `CARGO_HOME="$HOME/.cargo"` to use a user-writable registry cache.

- **Decisions taken**
  - All 4 clippy lints were genuine improvements (derive instead of manual impl, slice instead of `&mut Vec`, collapsed conditionals using let-chains). No `#[allow(...)]` attributes were added.
  - Only the 3 files modified by this ticket were touched — no pre-existing issues in other files were addressed.
