// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useConfig` — fetches `/api/config` once on mount and exposes the
 * `ConfigResponse | null` result. Extracted alongside the rest of the
 * Dashboard data hooks (Phase 5 step 3 of Part B) so the shell shows
 * exactly one `useEffect` call site (the WS-reconnect refetch).
 */

import { useEffect, useState } from "react";
import { apiJson } from "../api/client";
import type { ConfigResponse } from "../api/types";

export function useConfig(): ConfigResponse | null {
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  useEffect(() => {
    apiJson<ConfigResponse>("/api/config").then(setConfig).catch(() => {});
  }, []);
  return config;
}
