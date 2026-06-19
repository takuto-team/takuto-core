// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Worktree commands (per-user-per-workspace init + run commands).
 *
 * User-scoped `/api/worktree-commands/*` endpoints — each authenticated
 * user manages their own data only; admins have no special access. A
 * single row stores BOTH the init commands (Vec<string>) and the run
 * commands (Vec<{ name, command }>), so a single PUT updates both
 * atomically.
 */

import { api, apiJson } from "./http";

export interface RunCommand {
  name: string;
  command: string;
}

/** A single row in `user_worktree_commands` for the caller's user. */
export interface WorktreeCommandsRow {
  workspace_name: string;
  init_commands: string[];
  run_commands: RunCommand[];
  /** Per-workspace toggle: generate a per-flow report on workflow runs. */
  generate_report: boolean;
  updated_at: number;
}

export interface WorktreeCommandsWorkspaceEntry {
  name: string;
  html_url?: string | null;
  active: boolean;
  /** True if the caller has a `user_worktree_commands` row for this workspace. */
  has_my_commands: boolean;
}

/** GET /api/worktree-commands — list every row the caller owns. */
export async function listMyWorktreeCommands(): Promise<WorktreeCommandsRow[]> {
  return apiJson<WorktreeCommandsRow[]>("/api/worktree-commands");
}

/** GET /api/worktree-commands/{workspace} — returns null on 404. */
export async function getMyWorktreeCommands(
  workspace: string,
): Promise<WorktreeCommandsRow | null> {
  const res = await api(`/api/worktree-commands/${encodeURIComponent(workspace)}`);
  if (res.status === 404) {
    return null;
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/**
 * PUT /api/worktree-commands/{workspace} — upsert both kinds in one round-trip.
 *
 * The server validates: ≤50 commands per kind, ≤2000 char per command, ≤100
 * char per run-command name, non-empty after trim, no NUL bytes, unique
 * run-command names within the list.
 */
export async function putMyWorktreeCommands(
  workspace: string,
  initCommands: string[],
  runCommands: RunCommand[],
  generateReport: boolean,
): Promise<WorktreeCommandsRow> {
  const res = await api(`/api/worktree-commands/${encodeURIComponent(workspace)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      init_commands: initCommands,
      run_commands: runCommands,
      generate_report: generateReport,
    }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/** DELETE /api/worktree-commands/{workspace} — remove the caller's row (204) or 404. */
export async function deleteMyWorktreeCommands(workspace: string): Promise<void> {
  const res = await api(`/api/worktree-commands/${encodeURIComponent(workspace)}`, {
    method: "DELETE",
  });
  if (res.status === 204) return;
  if (res.status === 404) {
    throw new Error("No commands set for this workspace");
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
}

/**
 * GET /api/worktree-commands/_workspaces — workspaces from the filesystem scan
 * augmented with a `has_my_commands` boolean for the caller's user.
 */
export async function listWorktreeCommandsWorkspaces(): Promise<WorktreeCommandsWorkspaceEntry[]> {
  return apiJson<WorktreeCommandsWorkspaceEntry[]>("/api/worktree-commands/_workspaces");
}
