// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useWorkflowDefinitions` — fetches the `/api/workflow-definitions`
 * collection and exposes:
 *   * `refresh()` — immediate fetch
 *   * `scheduleRefresh()` — debounced fetch (500 ms), used by the
 *     dashboard WS handler when `workflow_updated` /
 *     `workflow_definitions_changed` / `step_completed` events fire.
 *
 * Behaviour preserved verbatim from pre-extraction: the 500 ms debounce
 * timer has no unmount cleanup (designer flagged the lack of cleanup as
 * intentional — matches the original Dashboard code).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { apiJson } from "../api/client";
import type { WorkflowDefinition } from "../api/types";

export interface UseWorkflowDefinitionsResult {
  workflowDefs: WorkflowDefinition[];
  refresh: () => void;
  scheduleRefresh: () => void;
}

const DEBOUNCE_MS = 500;

export function useWorkflowDefinitions(): UseWorkflowDefinitionsResult {
  const [workflowDefs, setWorkflowDefs] = useState<WorkflowDefinition[]>([]);
  const defsFetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(() => {
    apiJson<WorkflowDefinition[]>("/api/workflow-definitions")
      .then(setWorkflowDefs)
      .catch(() => {});
  }, []);

  const scheduleRefresh = useCallback(() => {
    if (defsFetchTimer.current) clearTimeout(defsFetchTimer.current);
    defsFetchTimer.current = setTimeout(refresh, DEBOUNCE_MS);
  }, [refresh]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { workflowDefs, refresh, scheduleRefresh };
}
