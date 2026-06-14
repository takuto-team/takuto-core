// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useWorkflowDefinitions` — reads the `/api/workflow-definitions`
 * collection through TanStack Query and exposes:
 *   * `refresh()` — invalidate + refetch immediately
 *   * `scheduleRefresh()` — debounced invalidate (500 ms), used by the
 *     dashboard WS handler when `work_item_updated` /
 *     `workflow_definitions_changed` / `step_completed` events fire.
 *
 * The debounce coalesces bursts of WS events into a single refetch and is
 * not a server-state cache concern, so it stays. As before, the timer has
 * no unmount cleanup (matches the original Dashboard behaviour).
 */

import { useCallback, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "../api/client";
import { queryKeys } from "../api/queryClient";
import type { WorkflowDefinition } from "../api/types";

export interface UseWorkflowDefinitionsResult {
  workflowDefs: WorkflowDefinition[];
  refresh: () => void;
  scheduleRefresh: () => void;
}

const DEBOUNCE_MS = 500;

export function useWorkflowDefinitions(): UseWorkflowDefinitionsResult {
  const queryClient = useQueryClient();
  const defsFetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const { data } = useQuery({
    queryKey: queryKeys.workflowDefinitions,
    queryFn: () => apiJson<WorkflowDefinition[]>("/api/workflow-definitions"),
  });

  const refresh = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: queryKeys.workflowDefinitions });
  }, [queryClient]);

  const scheduleRefresh = useCallback(() => {
    if (defsFetchTimer.current) clearTimeout(defsFetchTimer.current);
    defsFetchTimer.current = setTimeout(refresh, DEBOUNCE_MS);
  }, [refresh]);

  return { workflowDefs: data ?? [], refresh, scheduleRefresh };
}
