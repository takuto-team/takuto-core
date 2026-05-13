// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Fetch wrapper that includes session cookie credentials.
 * On 401, redirects to the login page.
 */
export async function api(input: string, init: RequestInit = {}): Promise<Response> {
  const res = await fetch(input, { ...init, credentials: "same-origin" });
  if (res.status === 401) {
    const ret = encodeURIComponent(window.location.pathname + window.location.search);
    window.location.href = `/login.html?return=${ret}`;
  }
  return res;
}

export async function apiJson<T>(input: string, init: RequestInit = {}): Promise<T> {
  const res = await api(input, init);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

export async function apiPost(input: string, body?: unknown): Promise<Response> {
  return api(input, {
    method: "POST",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
}

export async function apiPostJson<T>(input: string, body?: unknown): Promise<T> {
  const res = await apiPost(input, body);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

// ---------------------------------------------------------------------------
// Worktree commands (per-workspace init command overrides)
// ---------------------------------------------------------------------------

export interface WorktreeCommandsOverride {
  workspace_name: string;
  commands: string[];
  updated_at: number;
  updated_by?: string | null;
}

export interface WorktreeCommandsListResponse {
  default: string[];
  overrides: WorktreeCommandsOverride[];
}

export interface WorktreeCommandsWorkspaceEntry {
  name: string;
  html_url?: string | null;
  active: boolean;
  has_override: boolean;
}

/** GET /api/admin/worktree-commands — global default + all per-workspace overrides. */
export async function getWorktreeCommands(): Promise<WorktreeCommandsListResponse> {
  return apiJson<WorktreeCommandsListResponse>("/api/admin/worktree-commands");
}

/** GET /api/admin/worktree-commands/{workspace} — returns null on 404. */
export async function getWorktreeCommandsOverride(
  workspace: string,
): Promise<WorktreeCommandsOverride | null> {
  const res = await api(`/api/admin/worktree-commands/${encodeURIComponent(workspace)}`);
  if (res.status === 404) {
    return null;
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/** PUT /api/admin/worktree-commands/{workspace} — upsert the override. */
export async function putWorktreeCommandsOverride(
  workspace: string,
  commands: string[],
): Promise<WorktreeCommandsOverride> {
  const res = await api(`/api/admin/worktree-commands/${encodeURIComponent(workspace)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ commands }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/** DELETE /api/admin/worktree-commands/{workspace} — drops the override. */
export async function deleteWorktreeCommandsOverride(workspace: string): Promise<void> {
  const res = await api(`/api/admin/worktree-commands/${encodeURIComponent(workspace)}`, {
    method: "DELETE",
  });
  if (res.status === 204) return;
  if (res.status === 404) {
    throw new Error("Override not found");
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
}

/** GET /api/admin/worktree-commands/_workspaces — workspaces with `has_override` flag. */
export async function listWorktreeCommandsWorkspaces(): Promise<WorktreeCommandsWorkspaceEntry[]> {
  return apiJson<WorktreeCommandsWorkspaceEntry[]>("/api/admin/worktree-commands/_workspaces");
}
