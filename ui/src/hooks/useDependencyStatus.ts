// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Polls `GET /api/system/dependencies` for the runtime agent-install progress.
 * Polls every ~1.5s while `phase === "installing"` (so the overlay's current
 * step updates as each CLI installs), then stops once it reaches a terminal
 * phase. Returns `null` until the first response.
 */

import { useEffect, useState } from "react";
import { getDependencyStatus, type DependencyInstallStatus } from "../api/system";

const POLL_MS = 1500;

export function useDependencyStatus(): DependencyInstallStatus | null {
  const [status, setStatus] = useState<DependencyInstallStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const tick = async () => {
      try {
        const s = await getDependencyStatus();
        if (cancelled) return;
        setStatus(s);
        if (s.phase === "installing") {
          timer = setTimeout(tick, POLL_MS);
        }
      } catch {
        if (!cancelled) timer = setTimeout(tick, POLL_MS * 2);
      }
    };
    tick();

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  return status;
}
