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
