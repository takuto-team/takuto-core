// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Test-only helper: wraps a hook/component under test in a fresh
 * `QueryClientProvider`. A new `QueryClient` per call keeps each test's
 * cache isolated, and `retry: false` makes failed-fetch assertions
 * deterministic (no background retry storms).
 */

import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

export function createQueryWrapper(): {
  wrapper: ({ children }: { children: ReactNode }) => ReactNode;
  queryClient: QueryClient;
} {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
    },
  });
  function wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  }
  return { wrapper, queryClient };
}
