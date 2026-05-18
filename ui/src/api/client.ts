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
// Agent config patch (Phase 1 — auth-overhaul).
//
// Source of truth: tmp/multi-agents/04_architecture.md §2.3. The server
// accepts a partial patch and persists atomically; errors carry a structured
// `error` field (e.g. `denied_extra_arg`, `unknown_provider`,
// `provider_subtable_missing`) plus a human-readable `message`.
// ---------------------------------------------------------------------------

/**
 * Error thrown by `putAgentConfig` on a non-2xx response. Carries the
 * structured `code` from the server when available so callers can branch on
 * a stable identifier instead of free-form text.
 */
export class AgentConfigError extends Error {
  public readonly code: string;
  public readonly status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "AgentConfigError";
    this.code = code;
    this.status = status;
  }
}

/**
 * PUT /api/config/agent — atomic patch of the [agent] table. Returns the
 * fresh redacted `ConfigResponse` on success. On non-2xx, throws an
 * `AgentConfigError` with the structured `code` from the server (or
 * `http_<status>` when the server didn't return JSON).
 */
export async function putAgentConfig(
  patch: import("./types").AgentConfigPatch,
): Promise<import("./types").ConfigResponse> {
  const res = await api("/api/config/agent", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  });
  if (!res.ok) {
    // Try to parse `{ error, message }` JSON first; fall back to plain text.
    let code = `http_${res.status}`;
    let message = `HTTP ${res.status}`;
    const text = await res.text();
    if (text) {
      try {
        const body = JSON.parse(text) as { error?: string; message?: string };
        if (typeof body.error === "string") code = body.error;
        if (typeof body.message === "string" && body.message.length > 0) {
          message = body.message;
        } else if (typeof body.error === "string") {
          message = body.error;
        } else {
          message = text;
        }
      } catch {
        message = text;
      }
    }
    throw new AgentConfigError(message, code, res.status);
  }
  return res.json();
}

// ---------------------------------------------------------------------------
// Onboarding status (Phase 0 — auth-overhaul).
//
// Source of truth: tmp/multi-agents/04_architecture.md §1. Returns a
// structured SystemStatus the dashboard renders into a banner. The endpoint
// is new in Phase 0 — older servers respond 404, in which case the caller
// falls back to ConfigResponse.preflight_error for one release.
// ---------------------------------------------------------------------------

/**
 * GET /api/onboarding/status — returns `SystemStatus` or `null` when the
 * server hasn't shipped the endpoint yet (404). All other non-2xx responses
 * raise so the caller can decide between retry and silent fallback.
 */
export async function fetchOnboardingStatus(): Promise<
  import("./types").SystemStatus | null
> {
  const res = await api("/api/onboarding/status");
  if (res.status === 404) {
    return null;
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

// ---------------------------------------------------------------------------
// Worktree commands (per-user-per-workspace init + run commands).
//
// Plan-09: drops the admin-only `/api/admin/worktree-commands/*` endpoints in
// favour of user-scoped `/api/worktree-commands/*` — each authenticated user
// manages their own data only; admins have no special access. A single row
// stores BOTH the init commands (Vec<string>) and the run commands
// (Vec<{ name, command }>), so a single PUT updates both atomically.
// ---------------------------------------------------------------------------

export interface RunCommand {
  name: string;
  command: string;
}

/** A single row in `user_worktree_commands` for the caller's user. */
export interface WorktreeCommandsRow {
  workspace_name: string;
  init_commands: string[];
  run_commands: RunCommand[];
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
): Promise<WorktreeCommandsRow> {
  const res = await api(`/api/worktree-commands/${encodeURIComponent(workspace)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      init_commands: initCommands,
      run_commands: runCommands,
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

// ---------------------------------------------------------------------------
// Plan-10: per-user repository associations.
//
// Replaces the legacy workspace switcher concept. Every user opts-in to repos
// they care about; the on-disk clone is shared across users that have added
// the same repo. Cloning is open to any authenticated user.
// ---------------------------------------------------------------------------

export interface RepositoryRow {
  id: string;
  name: string;
  repo_url: string | null;
  local_path: string;
  default_branch: string;
  /** Present only on `/api/repositories` (my list). */
  added_at?: number;
  /** Number of OTHER users (excluding the caller) associated with this
   *  repository. Used by the UI to decide whether deletion will purge the
   *  on-disk clone. Only meaningful on the "my repositories" list. */
  co_users_count?: number;
}

/** GET /api/repositories — list repos the calling user has added. */
export async function listMyRepositories(): Promise<RepositoryRow[]> {
  return apiJson<RepositoryRow[]>("/api/repositories");
}

/** GET /api/repositories/_available — registered repos not yet added by me. */
export async function listAvailableRepositories(): Promise<RepositoryRow[]> {
  return apiJson<RepositoryRow[]>("/api/repositories/_available");
}

/**
 * GET /api/github/repos — list GitHub repositories the deployment's GitHub App
 * installation (or PAT, fallback) can see. Pass `q` for server-side filtering
 * (uses GitHub's search API when set; lists installation repositories
 * otherwise). Returns up to ~50 results per call.
 */
export async function listGitHubAccessibleRepos(
  q?: string,
): Promise<import("./types").GitHubRepo[]> {
  const qs = q && q.trim().length > 0 ? `?q=${encodeURIComponent(q.trim())}` : "";
  return apiJson<import("./types").GitHubRepo[]>(`/api/github/repos${qs}`);
}

/**
 * POST /api/repositories — clone-if-needed + associate.
 *
 * Body: `{ repository_id }` to add an existing repo to MY dashboard, OR
 *       `{ repo_url }` to clone a new repo and add it. Idempotent when the
 * repo is already in `repositories` (returns the existing row with 200).
 */
export async function addRepository(
  body: { repository_id?: string; repo_url?: string },
): Promise<RepositoryRow> {
  const res = await apiPost("/api/repositories", body);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/**
 * DELETE /api/repositories/{id} — remove from MY dashboard.
 *
 * Always-purge semantics: if I'm the last associated user, the row and the
 * on-disk clone are removed. `force_purge` (admin only) drops the row for
 * everyone. Returns 204 on success.
 */
export async function removeRepository(
  id: string,
  opts?: { force_purge?: boolean },
): Promise<void> {
  const url = `/api/repositories/${encodeURIComponent(id)}`;
  const res = await api(url, {
    method: "DELETE",
    headers: opts?.force_purge ? { "Content-Type": "application/json" } : undefined,
    body: opts?.force_purge ? JSON.stringify({ force_purge: true }) : undefined,
  });
  if (res.status === 204) return;
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
}
