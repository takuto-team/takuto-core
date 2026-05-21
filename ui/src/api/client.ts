// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Barrel re-export of the per-domain API modules. The implementation lives in:
 *   - `./http.ts`              — `api`, `apiJson`, `apiPost`, `apiPostJson`
 *   - `./credentials.ts`       — per-user credentials + `UserCredentialsError`
 *   - `./agentConfig.ts`       — `putAgentConfig` + `AgentConfigError`
 *   - `./onboarding.ts`        — `fetchOnboardingStatus`
 *   - `./worktreeCommands.ts`  — per-user worktree commands
 *   - `./repositories.ts`      — per-user repository associations
 *
 * Kept as a barrel so every existing `from "../api/client"` import keeps
 * working without a sweeping churn commit (CODING_STANDARDS §5 minimum viable
 * change). New code should import from the per-domain module directly.
 */

export { api, apiJson, apiPost, apiPostJson } from "./http";
export {
  UserCredentialsError,
  deleteGithubPat,
  deleteProviderCredential,
  fetchUserCredentials,
  patchGithubSettings,
  setClaudeSession,
  setGithubPat,
  setProviderCredential,
} from "./credentials";
export { AgentConfigError, putAgentConfig } from "./agentConfig";
export { fetchOnboardingStatus } from "./onboarding";
export {
  type RunCommand,
  type WorktreeCommandsRow,
  type WorktreeCommandsWorkspaceEntry,
  deleteMyWorktreeCommands,
  getMyWorktreeCommands,
  listMyWorktreeCommands,
  listWorktreeCommandsWorkspaces,
  putMyWorktreeCommands,
} from "./worktreeCommands";
export {
  type RepositoryRow,
  addRepository,
  listAvailableRepositories,
  listGitHubAccessibleRepos,
  listMyRepositories,
  removeRepository,
} from "./repositories";
