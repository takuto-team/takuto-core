// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowSummary } from "../api/types";
import { getStatusInfo } from "./StatusBadge";

/** The summary-counter buckets the dashboard can filter by. */
export type StatusFilterKey = "pending" | "running" | "completed" | "errors" | "paused";

/**
 * Whether a workflow falls in a counter bucket. Mirrors the server's
 * `workflow_counts` categorization (`routes/workflows/list.rs`): `Stopped` is
 * counted under **errors**, and any non-terminal driver state is **running**.
 * Uses the same `getStatusInfo` mapping the cards display, so a step-name
 * `state` (set by streaming events) still classifies as running.
 */
export function workflowMatchesStatus(w: WorkflowSummary, key: StatusFilterKey): boolean {
  const status = getStatusInfo(w.state, w.can_start).status;
  switch (key) {
    case "pending":
      return status === "pending";
    case "running":
      return status === "running";
    case "completed":
      return status === "completed";
    case "errors":
      return status === "error" || status === "stopped";
    case "paused":
      return status === "paused";
  }
}
