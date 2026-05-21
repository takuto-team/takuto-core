# Maestro — Phase Verification Log (2026-05-21 audit)

Tester regression coverage for the audit follow-up. Tracks the PO Verifier checks
from `lore/audits/2026-05-21-plan.md` against the landed phases. Read-only
verification — no code changes.

Working tree at final verification: rust-dev-1 has shipped all 18 Phase 1
commits plus the two `config/` cleanup commits (`4a5b167` mod.rs split,
`5dbb3f8` agent_legacy extraction). Only working-tree noise is the
in-progress `refactor-backlog.md` rewrite and a 3-blank-line whitespace
trim on `routes/workflows/mod.rs` (uncommitted, irrelevant to ACs).

---

## Phase 1 ✓ — module splits (config.rs + workflows.rs)

### Phase 1 split 1 — `config.rs` → `config/`

#### LOC against caps (PO production ≤ 600, `mod.rs` ≤ 200, tests ≤ 1000)

```
     110 crates/maestro-core/src/config/agent_legacy.rs    ✓
     518 crates/maestro-core/src/config/agent.rs           ✓ (≤600)
     289 crates/maestro-core/src/config/general.rs         ✓
     146 crates/maestro-core/src/config/git.rs             ✓
      75 crates/maestro-core/src/config/jira.rs            ✓
     278 crates/maestro-core/src/config/load.rs            ✓
      99 crates/maestro-core/src/config/mod.rs             ✓ (≤200)
      76 crates/maestro-core/src/config/patches.rs         ✓
      84 crates/maestro-core/src/config/runtime.rs         ✓
     121 crates/maestro-core/src/config/template.rs        ✓
     781 crates/maestro-core/src/config/tests.rs           ✓ (≤1000 relaxed AC)
     331 crates/maestro-core/src/config/web.rs             ✓
```

All twelve files within the post-cleanup caps. The two follow-up commits
(`4a5b167` mod.rs → mod/load/patches; `5dbb3f8` agent_legacy extraction)
landed and resolved the earlier overages.

#### Verifier checks

| Check | Result |
| ----- | ------ |
| `crates/maestro-core/src/config.rs` removed | **PASS** |
| Every file within caps (above) | **PASS** |
| `cargo build --workspace` zero warnings | **PASS** — `grep -cE "warning\|error"` = 0 |
| `cargo test -p maestro-core --lib` | **PASS** — 659 passed, 0 failed, 1 ignored |
| Consumer imports unchanged (`git grep -nE "use maestro_core::config" crates/`) | **PASS** — 26 hits resolve cleanly |

### Phase 1 split 2 — `routes/workflows.rs` → `routes/workflows/`

#### LOC against caps (PO production ≤ 500, `mod.rs` ≤ 150)

```
      65 crates/maestro-web/src/routes/workflows/definitions.rs    ✓
     281 crates/maestro-web/src/routes/workflows/dto.rs            ✓
     434 crates/maestro-web/src/routes/workflows/editor.rs         ✓
     198 crates/maestro-web/src/routes/workflows/lifecycle.rs      ✓
     443 crates/maestro-web/src/routes/workflows/list.rs           ✓
     218 crates/maestro-web/src/routes/workflows/manual.rs         ✓
     151 crates/maestro-web/src/routes/workflows/mod.rs            ✓*
     414 crates/maestro-web/src/routes/workflows/port_tracking.rs  ✓
     330 crates/maestro-web/src/routes/workflows/run_commands.rs   ✓
```

\* `mod.rs` is 1 LOC over the ≤150 cap. **Rounding-range exception accepted by
team-lead per intent-of-cap reasoning** — the file is genuinely facade
(license + `mod` + `pub use` + one shared `require_workflow_access` helper, no
fat methods). Eight production sub-modules all within the ≤500 cap.

#### Verifier checks

| Check | Result |
| ----- | ------ |
| `crates/maestro-web/src/routes/workflows.rs` removed | **PASS** |
| Every sub-module within caps (above) | **PASS** |
| No handler bodies in `mod.rs` (`grep -nE "^(async fn\|fn\|pub async fn\|pub fn) [a-z_]+.*Handler"`) | **PASS** — 0 hits |
| Consumer imports unchanged (`git grep -nE "use crate::routes::workflows::" crates/`) | **PASS** — 1 hit resolves cleanly |

### Workspace-level (both splits)

| Check | Result |
| ----- | ------ |
| `cargo build --workspace` zero warnings | **PASS** — `grep -cE "warning\|error"` = 0 |
| `cargo test --workspace` | **PASS** — 1019 tests passed, 1 ignored, 0 failed across all crates + integration suites + doctests |
| Public REST/WS surface preserved (cross-phase invariant #5) | **PASS by inference** — full workspace + 176 maestro-web integration tests green; no router-path changes flagged |
| No new `Cargo.toml` dependencies | **PASS** for Phase 1 |

Phase 1 ✓ **complete**.

---

## Phase 3 — Docker image hardening

### Verifier checks

| Check | Result |
| ----- | ------ |
| All `FROM` lines `@sha256:`-pinned | **PASS** — 3 external bases pinned; the 4th `FROM` is `runtime-base` (local multi-stage reference, no digest needed) |
| `@latest` outside comments | **PASS** — 0 hits |
| `curl … \| bash` outside comments | **PASS** — 0 hits; Cursor install replaced by pinned tarball + sha256 verify |
| Runtime-stage `RUN` count ≤ 15 | **PASS** — 11 `RUN` lines (down from 26) |
| `USER maestro` precedes `ENTRYPOINT` | **PASS (with caveat)** — see below |
| `docker build --target runtime-base` succeeds | **PASS** — built locally to `maestro:slim-test` |
| Runtime preamble comment present | **PASS** — line 41 `# Maestro runtime image — kitchen-sink bake…` |

### Caveat — `USER maestro` immediately followed by `USER root`

```
453:USER maestro
454:USER root
456:ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
```

The PO AC literally requires `USER maestro` to precede `ENTRYPOINT`, which it
does. However, line 454 re-elevates to root for the entrypoint because
`entrypoint.sh` still needs iptables + chown + provisioning writes. Comment
block 441–452 documents the reason and points to a future entrypoint refactor.
The net effective UID at container start is **root**, not `maestro` — the
hardening goal is sidestepped by design until the entrypoint is rewritten
(explicitly out of Phase 3 scope per the PO plan).

Optional checks skipped:
- `docker build --target runtime-build-tools` (slim build sufficed for AC; full
  build would re-download the Rust toolchain — skipped to save time).
- `docker compose up && curl /healthz` (no compose smoke run).

---

## Phase 5 — UI component splits

### Verifier checks

| Check | Result |
| ----- | ------ |
| `TicketDetailModal.tsx` ≤ 150 | **PASS** — 145 |
| `Dashboard.tsx` ≤ 250 | **PASS** — 214 |
| Each split sub-component `.tsx` ≤ 150 | **PASS** — Header 62, View 39, Editor 148, ImproveWithAI 146, StartWorkflowFooter 91, StartWorkflowRepoBanner 75, DashboardModals 86 |
| Each PO-named Dashboard hook + `DashboardModals.tsx` ≤ 150 | **PASS** — useOnboardingStatus 59, useMyRepositories 88, useWorkflowDefinitions 57, useDashboardModals 90 |
| `npm run build` | **PASS** — built in 478ms, no TS errors |
| `npx vitest run --project=unit` | **PASS** — 138/138 passing across 13 files |
| `as unknown as` / `@ts-ignore` / `: any` count | **PASS** — 0 hits |
| Snapshot tests | **PASS** — 0 hits |
| `useState` in `Dashboard.tsx` ≤ 4 | **PASS** — 0 |
| `useEffect` in `Dashboard.tsx` ≤ 2 | **PASS** — 2 |
| `useState` in `TicketDetailModal.tsx` ≤ 3 | **PASS** — 2 (`pendingImprovement` + state inside `useState` import) |

### Soft caveat — `useTicketEditor.ts` at 152 LOC

```
     152 ui/src/hooks/useTicketEditor.ts                # 2 over soft 150 cap
     143 ui/src/hooks/useTicketImproveWithAI.ts
      67 ui/src/hooks/useTicketCountdown.ts
      54 ui/src/hooks/useTicketDetail.ts
      53 ui/src/hooks/useStartWorkflow.ts
```

`useTicketEditor.ts` is 2 lines over the soft 150 LOC AC ("≤ 150 LOC per
non-shell .tsx"). The PO's explicit hook list (`useOnboardingStatus`,
`useMyRepositories`, `useWorkflowDefinitions`, `useDashboardModals`) does not
include it, so this is not a hard fail of any named AC — just over the soft
threshold. Flagging for the record; not blocking.

### Behavioural spot-checks (read-only inspection)

- `TicketDetailModal.tsx` — render tree branches on `loading`,
  `pendingImprovement`, `e.editMode` in the same way as the original
  monolithic component. The four states (**view / edit / improve-with-AI /
  start-workflow**) are all reachable; the original 12 `useState` slots were
  moved into the five hooks (`useTicketDetail`, `useTicketCountdown`,
  `useStartWorkflow`, `useTicketEditor`, `useTicketImproveWithAI`), not
  dropped.
- `useTicketEditor.ts` — 400 ms debounce wired at line 71
  (`setTimeout(() => setDebouncedText(editText), 400)`); the
  `requestAnimationFrame` post-save sequencing is preserved verbatim at lines
  97–101. Confirmed.
- `useDashboardModals.ts` — discriminated union (line 43–49) covers
  `none | picker | paste | nojira | detail | report`. `close()` writes
  `sessionStorage["noJiraAlertDismissed"]` only when transitioning out of
  `nojira` (line 73–78). Confirmed.

---

## Cross-phase invariants (final)

| Invariant | Status |
| --------- | ------ |
| #1 `cargo build --workspace` zero-warning | **PASS** — grep count = 0 |
| #2 `cargo test --workspace` green | **PASS** — 1019 passed, 0 failed, 1 ignored |
| #4 `npm --prefix ui run build` | **PASS** |
| #5 Public REST/WS contract preserved | **PASS by inference** — 176 maestro-web integration tests green, 0 router-path drift |
| #6 No new `Cargo.toml` dependency | **PASS** for landed phases |
| #8 `AGENTS.md` updates in same commit | not audited line-by-line; deferred to team-lead spot-check |

---

## Final status

- **Phase 1 ✓ complete** (config/ + routes/workflows/ both within caps after
  rust-dev-1's 2-commit cleanup).
- **Phase 3 ✓ complete** (Docker hardening).
- **Phase 5 ✓ complete** (UI splits).
- **Phases 2 and 4 deferred to `lore/refactor-backlog.md` per option-2 wrap.**
