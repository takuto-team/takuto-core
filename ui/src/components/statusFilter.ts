// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowSummary } from "../api/types";
import { getStatusInfo } from "./StatusBadge";

/** The four summary-counter buckets the dashboard can filter by. */
export type StatusFilterKey = "running" | "completed" | "errors" | "paused";

/**
 * Whether a workflow falls in a counter bucket. Mirrors the server's
 * `workflow_counts` categorization (`routes/workflows/list.rs`): `Stopped` is
 * counted under **errors**, and any non-terminal driver state is **running**.
 * Uses the same `getStatusInfo` mapping the cards display, so a step-name
 * `state` (set by streaming events) still classifies as running.
 */
export function workflowMatchesStatus(w: WorkflowSummary, key: StatusFilterKey): boolean {
  const label = getStatusInfo(w.state, w.can_start).label;
  switch (key) {
    case "running":
      return label === "Running";
    case "completed":
      return label === "Completed";
    case "errors":
      return label === "Error" || label === "Stopped";
    case "paused":
      return label === "Paused";
  }
}
