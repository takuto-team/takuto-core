// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Shared TanStack Query client and the canonical query-key registry.
 *
 * Every server-state hook in `ui/src/hooks/` reads through this single
 * client so WebSocket events can target cache entries by key
 * (`queryClient.invalidateQueries`) instead of each hook re-running its own
 * imperative fetch.
 *
 * Defaults mirror the pre-Query behaviour: a failed fetch surfaces once and
 * is swallowed by the consumer (no silent retry storms), and the window is
 * not refetched on focus (the WebSocket connection is the live-update
 * channel, not focus polling).
 */

import { QueryCache, QueryClient } from "@tanstack/react-query";
import { surfaceError } from "../utils/surfaceError";

export const queryClient = new QueryClient({
  // Centralised error surface: a failed query read is no longer silent — it is
  // routed through `surfaceError` onto the fetch-error bus so
  // `QueryErrorToaster` can show a toast.
  queryCache: new QueryCache({
    onError: (error) => surfaceError(error),
  }),
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

/**
 * Canonical query keys. List and counts use deliberately non-overlapping
 * top-level keys so invalidating the work-item list does not also invalidate
 * the global counts (the two are fetched from independent endpoints).
 */
export const queryKeys = {
  config: ["config"] as const,
  auth: ["auth"] as const,
  polling: ["polling"] as const,
  pollingSettings: (workspace: string) => ["polling-settings", workspace] as const,
  repositories: ["repositories"] as const,
  workflowDefinitions: ["workflow-definitions"] as const,
  workItems: ["work-items"] as const,
  workItemCounts: ["work-item-counts"] as const,
};
