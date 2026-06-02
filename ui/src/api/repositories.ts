// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user repository associations.
 *
 * Replaces the legacy workspace switcher concept. Every user opts-in to repos
 * they care about; the on-disk clone is shared across users that have added
 * the same repo. Cloning is open to any authenticated user.
 */

import { api, apiJson, apiPost } from "./http";
import type { GitHubRepo } from "./types";

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
): Promise<GitHubRepo[]> {
  const qs = q && q.trim().length > 0 ? `?q=${encodeURIComponent(q.trim())}` : "";
  return apiJson<GitHubRepo[]>(`/api/github/repos${qs}`);
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
