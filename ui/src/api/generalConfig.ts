// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Runtime dashboard config patch — the `[general]` fields editable from the
 * dashboard via `PUT /api/config`. Today that covers `ticketing_system` and
 * the concurrency caps. Mirrors `agentConfig.ts` / `jiraConfig.ts`: a single
 * PUT that returns the fresh redacted `ConfigResponse` on success and throws a
 * structured error on non-2xx.
 */

import { api } from "./http";
import type { ConfigResponse, RuntimeConfigPatch } from "./types";

/**
 * Error thrown by `putRuntimeConfig` on a non-2xx response. Carries the
 * structured `code` from the server when available so callers can branch on a
 * stable identifier instead of free-form text.
 */
export class RuntimeConfigError extends Error {
  public readonly code: string;
  public readonly status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "RuntimeConfigError";
    this.code = code;
    this.status = status;
  }
}

/**
 * PUT /api/config — atomic patch of the runtime-editable `[general]` fields.
 * Returns the fresh redacted `ConfigResponse` on success. On non-2xx, throws a
 * `RuntimeConfigError` with the structured `code` from the server (or
 * `http_<status>` when the server didn't return JSON).
 */
export async function putRuntimeConfig(
  patch: RuntimeConfigPatch,
): Promise<ConfigResponse> {
  const res = await api("/api/config", {
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
    throw new RuntimeConfigError(message, code, res.status);
  }
  return res.json();
}
