// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Onboarding status (Phase 0 — auth-overhaul).
 *
 * Source of truth: tmp/multi-agents/04_architecture.md §1. Returns a
 * structured SystemStatus the dashboard renders into a banner. The endpoint
 * is new in Phase 0 — older servers respond 404, in which case the caller
 * falls back to ConfigResponse.preflight_error for one release.
 */

import { api } from "./http";
import type { SystemStatus } from "./types";

/**
 * GET /api/onboarding/status — returns `SystemStatus` or `null` when the
 * server hasn't shipped the endpoint yet (404). All other non-2xx responses
 * raise so the caller can decide between retry and silent fallback.
 */
export async function fetchOnboardingStatus(): Promise<SystemStatus | null> {
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

/**
 * First-run state derived from `GET /api/onboarding/status`. Used to auto-route
 * a signed-in user into the wizard when `config.toml` does not yet exist and
 * the user has not already completed onboarding.
 *
 * - `configTomlOk`: `false` only when the server explicitly reports a missing
 *   config file. Anything else (older server, network error, key absent) is
 *   treated as "ok" so we never bounce an existing deployment into the wizard.
 * - `completed`: `true` once the caller's onboarding row carries a
 *   `completed_at` timestamp — guards against a redirect loop after the wizard
 *   has been finished but before the process restarts.
 */
export interface OnboardingFirstRunState {
  configTomlOk: boolean;
  completed: boolean;
}

export async function fetchOnboardingFirstRunState(): Promise<OnboardingFirstRunState | null> {
  const res = await api("/api/onboarding/status");
  if (!res.ok) {
    return null;
  }
  const body = (await res.json().catch(() => null)) as
    | {
        config_toml_ok?: boolean;
        user_onboarding?: { completed_at?: string | null } | null;
      }
    | null;
  if (!body) {
    return null;
  }
  return {
    configTomlOk: body.config_toml_ok !== false,
    completed: Boolean(body.user_onboarding?.completed_at),
  };
}
