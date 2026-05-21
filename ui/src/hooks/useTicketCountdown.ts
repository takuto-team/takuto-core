// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useTicketCountdown` — exposes a countdown timer for the "Improve with AI"
 * overlay in `TicketDetailModal`. Extracted alongside `formatCountdown` so
 * the modal shell no longer owns interval refs (CODING_STANDARDS §3 — "All
 * `useRef` lives inside the hook that needs it").
 *
 * The countdown is started with `start(timeoutSecs)` and cleared with
 * `stop()`. The hook also auto-cleans on unmount.
 */

import { useEffect, useRef, useState } from "react";

export function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")} remaining until timeout`;
}

interface UseTicketCountdownResult {
  countdown: number;
  start: (timeoutSecs: number) => void;
  stop: () => void;
}

export function useTicketCountdown(
  defaultTimeoutSecs: number,
): UseTicketCountdownResult {
  const [countdown, setCountdown] = useState(defaultTimeoutSecs);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const startRef = useRef<number | null>(null);

  const stop = () => {
    if (intervalRef.current) {
      clearInterval(intervalRef.current);
      intervalRef.current = null;
    }
  };

  const start = (timeoutSecs: number) => {
    startRef.current = Date.now();
    setCountdown(timeoutSecs);
    stop();
    intervalRef.current = setInterval(() => {
      const elapsed = (Date.now() - (startRef.current ?? Date.now())) / 1000;
      setCountdown(Math.max(0, Math.round(timeoutSecs - elapsed)));
    }, 500);
  };

  useEffect(() => {
    return () => {
      stop();
    };
  }, []);

  return { countdown, start, stop };
}
