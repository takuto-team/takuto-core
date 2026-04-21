// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

export function usePolling() {
  const [paused, setPaused] = useState(false);
  const [toggling, setToggling] = useState(false);

  useEffect(() => {
    api("/api/polling")
      .then((r) => r.json())
      .then((data) => setPaused(data.paused))
      .catch(() => {});
  }, []);

  const toggle = useCallback(async () => {
    setToggling(true);
    try {
      const endpoint = paused ? "/api/polling/resume" : "/api/polling/pause";
      const res = await api(endpoint, { method: "POST" });
      if (res.ok) setPaused(!paused);
    } finally {
      setToggling(false);
    }
  }, [paused]);

  return { paused, toggling, toggle };
}
