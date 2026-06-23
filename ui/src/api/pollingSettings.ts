// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Per-user-per-repository item-polling settings.
 *
 * User-scoped `/api/me/polling-settings/*` endpoints — each authenticated user
 * manages their own per-repository polling configuration; admins have no
 * special access. The selected repository's name IS the workspace key (same
 * convention as `/api/worktree-commands/{workspace}`). The deployment-global
 * "general limits" are NOT here — those ride the admin-only
 * `PUT /api/config/polling` (see `itemPollingConfig.ts`).
 *
 * Read types are the ts-rs generated `RepoPollingSettings` / `RepoPollingSettingsRow`
 * (re-exported from `./types`). The PUT accepts a partial object (omitted fields
 * take their server defaults), so the form sends only the per-repo fields it
 * shows — the poll interval and per-user cap are deployment-global, not here.
 */

import { api, apiJson } from "./http";
import type { RepoPollingSettings, RepoPollingSettingsRow } from "./types";

export type { RepoPollingSettings, RepoPollingSettingsRow };

/**
 * Write payload for PUT: a partial `RepoPollingSettings`. The server fills
 * omitted fields with defaults.
 */
export type RepoPollingSettingsInput = Partial<RepoPollingSettings>;

/** GET /api/me/polling-settings — list every row the caller owns. */
export async function listMyPollingSettings(): Promise<RepoPollingSettingsRow[]> {
  return apiJson<RepoPollingSettingsRow[]>("/api/me/polling-settings");
}

/** GET /api/me/polling-settings/{workspace} — returns null on 404. */
export async function getMyPollingSettings(
  workspace: string,
): Promise<RepoPollingSettingsRow | null> {
  const res = await api(`/api/me/polling-settings/${encodeURIComponent(workspace)}`);
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
 * PUT /api/me/polling-settings/{workspace} — upsert this repository's polling
 * settings. The body is the (partial) settings object. The server validates and
 * returns 400 on bad input (e.g. non-alphanumeric project keys, or
 * poll_interval_secs < 10 while auto_polling is on).
 */
export async function putMyPollingSettings(
  workspace: string,
  settings: RepoPollingSettingsInput,
): Promise<RepoPollingSettingsRow> {
  const res = await api(`/api/me/polling-settings/${encodeURIComponent(workspace)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(settings),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

/** DELETE /api/me/polling-settings/{workspace} — remove the caller's row (204) or 404. */
export async function deleteMyPollingSettings(workspace: string): Promise<void> {
  const res = await api(`/api/me/polling-settings/${encodeURIComponent(workspace)}`, {
    method: "DELETE",
  });
  if (res.status === 204) return;
  if (res.status === 404) {
    throw new Error("No polling settings set for this repository");
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
}
