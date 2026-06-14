// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useConfig` — reads `/api/config` through TanStack Query and exposes the
 * `ConfigResponse | null` result. The value is fetched once and cached; a
 * fetch error resolves to `null` (the consumer treats config as optional).
 */

import { useQuery } from "@tanstack/react-query";
import { apiJson } from "../api/client";
import { queryKeys } from "../api/queryClient";
import type { ConfigResponse } from "../api/types";

export function useConfig(): ConfigResponse | null {
  const { data } = useQuery({
    queryKey: queryKeys.config,
    queryFn: () => apiJson<ConfigResponse>("/api/config"),
  });
  return data ?? null;
}
