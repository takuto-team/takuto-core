// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only Jira-context config patch — the deployment-global Jira-context
 * *processing* fields of the `[jira]` section (linked-issue inclusion, byte
 * caps, done status). The per-repo poll filters live in `pollingSettings.ts`.
 *
 * A single `PUT /api/config/jira` call returns the fresh redacted
 * `ConfigResponse` (with `persisted` / `persist_warning`) on success, and
 * throws a structured error on non-2xx.
 */

import { api } from "./http";
import type { ConfigResponse, JiraConfigPatch } from "./types";

/**
 * Error thrown by `putJiraConfig` on a non-2xx response. Carries the
 * structured `code` from the server when available so callers can branch on a
 * stable identifier instead of free-form text.
 */
export class JiraConfigError extends Error {
  public readonly code: string;
  public readonly status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "JiraConfigError";
    this.code = code;
    this.status = status;
  }
}

/**
 * PUT /api/config/jira — atomic patch of the Jira-context fields. Returns the
 * fresh redacted `ConfigResponse` on success. On non-2xx, throws a
 * `JiraConfigError` with the structured `code` from the server (or
 * `http_<status>` when the server didn't return JSON).
 */
export async function putJiraConfig(
  patch: JiraConfigPatch,
): Promise<ConfigResponse> {
  const res = await api("/api/config/jira", {
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
    throw new JiraConfigError(message, code, res.status);
  }
  return res.json();
}
