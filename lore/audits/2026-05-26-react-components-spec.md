# Refactor spec — react god components (audit §8 #4)

Source: 2026-05-21 clean-code audit §8 #4. Splits the three frontend god components flagged by the audit's "worst offenders" table into a shell + per-concern extractions (hooks, sub-components, pure functions).

## 1. Targets

| File | Before | After | Reduction |
|---|---|---|---|
| `ui/src/components/IssueCard.tsx` | 485 | 376 | −109 |
| `ui/src/pages/Onboarding.tsx` | 475 | 158 | −317 |
| `ui/src/components/WorktreeSettingsTab.tsx` | 367 | 241 | −126 |

## 2. Extractions

### IssueCard.tsx — 3 extractions

- `ui/src/components/IssueCard/EditorTerminalMenu.tsx` (99 LOC). Unifies the editor + terminal icon-with-dropdown pattern (`kind: "editor" | "terminal"`); the parent owns the `openMenu` state so the three card menus are mutually exclusive.
- `ui/src/components/IssueCard/PortMappingsMenu.tsx` (58 LOC). Right-bottom port-mapping dropdown.
- `ui/src/hooks/useIssueCardActions.ts` (75 LOC). `useIssueCardActions(ticketKey)` returns `{ doAction, openEditor, openTerminal, closeEditor }` — per-workflow endpoint URLs and the editor / terminal cold-start flow. `withLoading` + toast surfacing stay in the parent shell so the action thunks compose with the overlay state machine.

### Onboarding.tsx — 7 extractions

- `ui/src/pages/Onboarding/Stepper.tsx` (61 LOC) — exports `ONBOARDING_STEPS` constant + presentational `<Stepper current={...} />`.
- `ui/src/pages/Onboarding/{TicketingStep,ProviderStep,GitHubStep,CredentialsStep}.tsx` (24 / 102 / 23 / 33 LOC) — one per wizard step, each owns the JSX + per-step props it actually consumes.
- `ui/src/hooks/useOnboardingFlow.ts` (75 LOC). Wizard navigation state machine: `step`, `goNext`, `goSkip`, `goBack`, and the `POST /api/onboarding/complete` finalization. Pre-flight hook `onBeforeNext(step)` lets the page gate "Continue" — returning `false` blocks advance.
- `ui/src/hooks/useProviderForm.ts` (119 LOC). Step-2 provider form's local state + `/api/config` fetch + save. Returns the cached `ticketingSystem` / `githubAppConfigured` so steps 1 and 3 do not round-trip independently.

### WorktreeSettingsTab.tsx — 4 extractions

- `ui/src/hooks/useDiffForm.ts` (49 LOC). Generic diff-aware form state: tracks `value` + `original` under a pluggable equality (default JSON.stringify). Exposes `setValue`, `replaceOriginal` (after save), `reset` (revert). The two diff-form pairs in the original (init / run) collapse to two `useDiffForm` calls.
- `ui/src/hooks/useWorktreeWorkspaces.ts` (49 LOC). Owns the workspace list + initial load + `refresh` + a `setHasMyCommands(name, has)` patcher so Save / Delete flip the green-dot badge in place.
- `ui/src/components/WorktreeSettings/WorkspaceSidebar.tsx` (76 LOC). Presentational left-pane picker.
- `ui/src/components/WorktreeSettings/validateCommands.ts` (73 LOC). Pure pre-flight validator mirroring `db::user_worktree_commands::upsert`'s shape / NUL-byte / length-cap checks.

## 3. Acceptance criteria

- [x] `npm run build` produces zero TypeScript errors across all three splits.
- [x] `npx vitest run` matches baseline (241 of 243 passed; 2 unhandled errors from a pre-existing `Onboarding.stories.tsx` storybook iframe flake unrelated to this work).
- [x] No behaviour change visible to users: the editor/terminal/port menus still open mutually-exclusively, the wizard still gates Continue on step-2 save, the worktree settings tab still validates before saving.
- [x] Every extracted hook has a single, focused responsibility (no "kitchen-sink" hooks).
- [x] Every extracted sub-component takes only the props it consumes (no whole-state pass-through).

## 4. Risks & non-goals

1. **WorktreeSettingsTab residual size.** The shell lands at 241 LOC, above the audit's ≤180 target — the residual is JSX for the right-pane editor (save / delete buttons, validation messages, "no commands set" hint). Further extraction would be cosmetic; the state primitives are out.
2. **Storybook flake retained.** The 2 `Onboarding.stories.tsx` iframe errors visible in vitest output pre-date this work — they're a known issue with the `@storybook/addon-vitest` test helper under React Router (the auth-flow iframe load races). Not introduced or worsened here.
3. **No new tests.** The audit flagged the components for size, not for missing coverage. The 18 storybook stories that already exercise IssueCard / Onboarding / WorktreeSettingsTab continue to pass; adding a dedicated unit test per extracted hook would be value-positive but is out of scope for the structural split.
4. **No design-system extraction.** The icon button + menu patterns repeated across IssueCard / WorkflowDefButtons / DashboardModals are candidates for a shared primitive (see also the audit's "Systemic smells" §4 on inline JSX), but that's a cross-cutting refactor outside §8 #4.
