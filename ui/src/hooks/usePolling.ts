// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import { queryKeys } from "../api/queryClient";
import type { PollingStatus } from "../api/types";

async function fetchPollingStatus(): Promise<PollingStatus> {
  const res = await api("/api/polling");
  return res.json() as Promise<PollingStatus>;
}

export function usePolling() {
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: queryKeys.polling,
    queryFn: fetchPollingStatus,
  });
  const paused = data?.paused ?? false;

  const mutation = useMutation({
    mutationFn: async (currentlyPaused: boolean): Promise<boolean> => {
      const endpoint = currentlyPaused ? "/api/polling/resume" : "/api/polling/pause";
      const res = await api(endpoint, { method: "POST" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return !currentlyPaused;
    },
    onSuccess: (nextPaused) => {
      queryClient.setQueryData<PollingStatus>(queryKeys.polling, { paused: nextPaused });
    },
  });

  const toggle = useCallback(async () => {
    // Swallow failures (server unreachable): the toggle simply has no
    // effect, matching the pre-Query `if (res.ok)` guard.
    await mutation.mutateAsync(paused).catch(() => {});
  }, [mutation, paused]);

  return { paused, toggling: mutation.isPending, toggle };
}
