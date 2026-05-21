// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Agent config patch (Phase 1 — auth-overhaul).
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §2.3. The server
 * accepts a partial patch and persists atomically; errors carry a structured
 * `error` field (e.g. `denied_extra_arg`, `unknown_provider`,
 * `provider_subtable_missing`) plus a human-readable `message`.
 */

import { api } from "./http";
import type { AgentConfigPatch, ConfigResponse } from "./types";

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
  patch: AgentConfigPatch,
): Promise<ConfigResponse> {
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
