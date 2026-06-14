// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Admin-only item-polling config patch — the `[polling]` section plus the
 * top-level `item_types` that patches `config.jira.item_types`.
 *
 * Mirrors `agentConfig.ts`: a single `PUT /api/config/polling` call that
 * returns the fresh redacted `ConfigResponse` (with `persisted` /
 * `persist_warning`) on success, and throws a structured error on non-2xx.
 */

import { api } from "./http";
import type { ConfigResponse, ItemPollingConfigPatch } from "./types";

/**
 * Error thrown by `putItemPollingConfig` on a non-2xx response. Carries the
 * structured `code` from the server when available so callers can branch on a
 * stable identifier instead of free-form text.
 */
export class ItemPollingConfigError extends Error {
  public readonly code: string;
  public readonly status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "ItemPollingConfigError";
    this.code = code;
    this.status = status;
  }
}

/**
 * PUT /api/config/polling — atomic patch of the `[polling]` section. Returns
 * the fresh redacted `ConfigResponse` on success. On non-2xx, throws an
 * `ItemPollingConfigError` with the structured `code` from the server (or
 * `http_<status>` when the server didn't return JSON).
 */
export async function putItemPollingConfig(
  patch: ItemPollingConfigPatch,
): Promise<ConfigResponse> {
  const res = await api("/api/config/polling", {
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
    throw new ItemPollingConfigError(message, code, res.status);
  }
  return res.json();
}
