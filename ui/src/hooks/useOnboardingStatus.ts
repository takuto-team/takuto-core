// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useOnboardingStatus` — fetches and exposes the Phase 0 system /
 * onboarding status banner state.
 *
 * Tri-state semantics preserved (OnboardingBanner depends on it):
 *   * `undefined` — fetch in flight
 *   * `null`      — endpoint 404'd (older server, fall back to
 *                   ConfigResponse.preflight_error in the consumer)
 *   * SystemStatus — loaded payload
 *
 * Side-effects: mount fetch + a window `focus` listener that re-runs
 * the fetch when the tab regains focus (intentional — not
 * `visibilitychange` — preserved verbatim from pre-extraction).
 */

import { useCallback, useEffect, useState } from "react";
import { fetchOnboardingStatus } from "../api/client";
import type { SystemStatus } from "../api/types";

export interface UseOnboardingStatusResult {
  systemStatus: SystemStatus | null | undefined;
  refresh: () => void;
}

export function useOnboardingStatus(): UseOnboardingStatusResult {
  const [systemStatus, setSystemStatus] = useState<SystemStatus | null | undefined>(
    undefined,
  );

  const refresh = useCallback(() => {
    fetchOnboardingStatus()
      .then(setSystemStatus)
      .catch(() => {
        // Network or 5xx — treat as "endpoint not available" so the legacy
        // preflight_error string is rendered instead of a blank banner.
        setSystemStatus(null);
      });
  }, []);

  useEffect(() => {
    refresh();
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  return { systemStatus, refresh };
}
