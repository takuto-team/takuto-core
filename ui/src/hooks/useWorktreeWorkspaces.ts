// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useCallback, useEffect, useState } from "react";
import {
  listWorktreeCommandsWorkspaces,
  type WorktreeCommandsWorkspaceEntry,
} from "../api/client";

/**
 * Workspace-list data loader for the Worktree Settings tab.
 *
 * Fetches the list once on mount, exposes a `refresh` callback for
 * after-save re-fetches, and a `setHasMyCommands(name, has)` patcher so
 * the Save / Delete flows can flip the green "set" / gray "none" badge
 * without round-tripping the whole list.
 */
export function useWorktreeWorkspaces() {
  const [workspaces, setWorkspaces] = useState<WorktreeCommandsWorkspaceEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const refresh = useCallback(() => {
    setLoading(true);
    setError("");
    listWorktreeCommandsWorkspaces()
      .then(setWorkspaces)
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const setHasMyCommands = useCallback((name: string, has: boolean) => {
    setWorkspaces((prev) =>
      prev.map((w) => (w.name === name ? { ...w, has_my_commands: has } : w)),
    );
  }, []);

  return { workspaces, loading, error, refresh, setHasMyCommands };
}
