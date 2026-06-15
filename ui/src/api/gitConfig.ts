// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only git config patch — the operator-tunable portion of the `[git]`
 * section (base branch + remote).
 *
 * Mirrors `jiraConfig.ts`: a single `PUT /api/config/git` call that returns the
 * fresh redacted `ConfigResponse` (with `persisted` / `persist_warning`) on
 * success, and throws a structured error on non-2xx (e.g. 403 for non-admins).
 */

import { api } from "./http";
import type { ConfigResponse, GitConfigPatch } from "./types";

/**
 * Error thrown by `putGitConfig` on a non-2xx response. Carries the structured
 * `code` from the server when available so callers can branch on a stable
 * identifier (e.g. distinguish the admin-gated 403) instead of free-form text.
 */
export class GitConfigError extends Error {
  public readonly code: string;
  public readonly status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "GitConfigError";
    this.code = code;
    this.status = status;
  }
}

/**
 * PUT /api/config/git — atomic patch of the git base branch / remote. Returns
 * the fresh redacted `ConfigResponse` on success. On non-2xx, throws a
 * `GitConfigError` with the structured `code` from the server (or
 * `http_<status>` when the server didn't return JSON).
 */
export async function putGitConfig(
  patch: GitConfigPatch,
): Promise<ConfigResponse> {
  const res = await api("/api/config/git", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  });
  if (!res.ok) {
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
    throw new GitConfigError(message, code, res.status);
  }
  return res.json();
}
