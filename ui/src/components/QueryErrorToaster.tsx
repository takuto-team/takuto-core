// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Bridges server fetch errors (emitted on the fetch-error bus by the shared
 * QueryClient's `QueryCache.onError`) to the toast UI, so a failed config /
 * work-item / repository read shows a visible signal instead of leaving the
 * page silently stale. Renders nothing; it only subscribes. Identical
 * messages within a short window are coalesced so a reconnect refetch storm
 * doesn't stack duplicate toasts.
 */

import { useEffect, useRef } from "react";
import { onFetchError } from "../api/fetchErrorBus";
import { useToast } from "../hooks/useToast";

const DEDUPE_WINDOW_MS = 5000;

export function QueryErrorToaster() {
  const { showToast } = useToast();
  const last = useRef<{ message: string; at: number }>({ message: "", at: 0 });

  useEffect(() => {
    return onFetchError((message) => {
      const now = Date.now();
      if (last.current.message === message && now - last.current.at < DEDUPE_WINDOW_MS) return;
      last.current = { message, at: now };
      showToast(`Couldn't reach the server: ${message}`, "error");
    });
  }, [showToast]);

  return null;
}
