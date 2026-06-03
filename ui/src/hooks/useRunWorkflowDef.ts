// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Shared run/retry action for a work-item flow button. Both the inline
 * `WorkflowDefButtons` and the `StartFlowModal` use this so the endpoint and
 * error handling stay identical across the two surfaces.
 */

import { useCallback, useState } from "react";
import { apiPost } from "../api/client";
import type { WorkflowDefinition } from "../api/types";
import { useToast } from "./useToast";

export function useRunWorkflowDef(ticketKey: string, onRefresh: () => void) {
  const { showToast } = useToast();
  const [loadingDef, setLoadingDef] = useState<string | null>(null);

  const run = useCallback(
    async (def: WorkflowDefinition, state: string) => {
      const endpoint = state === "error" ? "retry-definition" : "run-definition";
      setLoadingDef(def.filename);
      try {
        const res = await apiPost(
          `/api/work-items/${encodeURIComponent(ticketKey)}/${endpoint}/${encodeURIComponent(def.filename)}`,
        );
        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || `Failed to ${endpoint}`);
        }
        onRefresh();
      } catch (e) {
        showToast(e instanceof Error ? e.message : "Action failed");
      } finally {
        setLoadingDef(null);
      }
    },
    [ticketKey, onRefresh, showToast],
  );

  return { run, loadingDef };
}
