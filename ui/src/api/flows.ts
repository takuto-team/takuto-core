// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-workspace work-item flows.
 *
 * User-scoped `/api/me/flows*` endpoints — each authenticated user owns their
 * own ordered flow list for the active workspace; admins have no special
 * access. The full list is read and written atomically (the UI replaces the
 * whole row on every save), mirroring the `user_worktree_commands` precedent.
 */

import { api, apiJson } from "./http";

/** A single skill invocation attached to a step. */
export interface SkillRef {
  name: string;
  args: string[];
}

/** One step within a flow — a single agent prompt plus optional skills. */
export interface UserFlowStep {
  name: string;
  prompt: string;
  skills: SkillRef[];
}

/** A named, ordered list of steps a user triggers on a work-item card. */
export interface UserFlow {
  name: string;
  depends_on: string[];
  steps: UserFlowStep[];
}

/** Response shape shared by GET / PUT / reseed — the list plus the workspace it scopes to. */
export interface UserFlowsResponse {
  flows: UserFlow[];
  workspace: string;
}

/** Hard cap enforced server-side and mirrored client-side for instant feedback. */
export const MAX_FLOWS = 20;

/** Maximum length of a flow's kebab-case slug (mirrors the backend constant). */
const MAX_SLUG_LEN = 64;

/**
 * Lower-case, kebab-case, length-capped slug for a flow name. This is a
 * verbatim port of the backend `slugify` so the editor can detect slug
 * collisions client-side with the same result the server produces (the slug
 * is the `workflow_def_runs` key, so two flows must never share one).
 */
export function slugify(name: string): string {
  let out = "";
  let prevDash = false;
  for (const ch of name) {
    if (/[A-Za-z0-9]/.test(ch)) {
      out += ch.toLowerCase();
      prevDash = false;
    } else if (!prevDash) {
      out += "-";
      prevDash = true;
    }
  }
  const trimmed = out.replace(/^-+/, "").replace(/-+$/, "");
  let slug = Array.from(trimmed).slice(0, MAX_SLUG_LEN).join("");
  slug = slug.replace(/-+$/, "");
  return slug;
}

/**
 * Structured validation failure surfaced by PUT / reseed. `kind` is one of the
 * backend's typed reasons: `too_many_flows`, `empty_flow_name`,
 * `duplicate_flow_name`, `duplicate_slug`, `empty_slug`, `no_steps`,
 * `empty_step_name`, `empty_step_prompt`, `empty_skill_name`,
 * `unknown_dependency`, `dependency_cycle`, and `nul_byte`.
 */
export class UserFlowsError extends Error {
  readonly kind: string;
  constructor(message: string, kind: string) {
    super(message);
    this.name = "UserFlowsError";
    this.kind = kind;
  }
}

async function throwStructured(res: Response): Promise<never> {
  const body = (await res.json().catch(() => null)) as { error?: string; kind?: string } | null;
  throw new UserFlowsError(body?.error ?? `HTTP ${res.status}`, body?.kind ?? "unknown");
}

/** GET /api/me/flows — current user's flow list for the active workspace. Lazy-seeds if absent. */
export async function getMyFlows(): Promise<UserFlowsResponse> {
  return apiJson<UserFlowsResponse>("/api/me/flows");
}

/** PUT /api/me/flows — replace the full list. An empty list is a valid state. */
export async function putMyFlows(flows: UserFlow[]): Promise<UserFlowsResponse> {
  const res = await api("/api/me/flows", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ flows }),
  });
  if (!res.ok) {
    return throwStructured(res);
  }
  return res.json();
}

/** POST /api/me/flows/reseed — destructively overwrite with the TOML defaults. */
export async function reseedMyFlows(): Promise<UserFlowsResponse> {
  const res = await api("/api/me/flows/reseed", { method: "POST" });
  if (!res.ok) {
    return throwStructured(res);
  }
  return res.json();
}
