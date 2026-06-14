// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useMyRepositories` — fetches and persists the caller's repository set
 * for the Dashboard page.
 *
 * Behaviour preserved from the pre-extracted Dashboard:
 *   * `myRepos` starts at `null` (loading sentinel) until the first
 *     fetch lands.
 *   * `activeRepoName` is `null` ("All repositories") or a name string,
 *     persisted in `localStorage` under `maestro.activeRepoName`.
 *     Lazy initializer + write-through both wrapped in `try`/`catch`
 *     to tolerate quota / disabled storage.
 *   * Sync effect drops a stale `activeRepoName` (no longer in
 *     `myRepos`) and auto-selects the lone remaining repo when the
 *     list shrinks to exactly one.
 */

import { useCallback, useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listMyRepositories, type RepositoryRow } from "../api/client";
import { queryKeys } from "../api/queryClient";

const ACTIVE_REPO_KEY = "maestro.activeRepoName";

export interface UseMyRepositoriesResult {
  myRepos: RepositoryRow[] | null;
  hasAnyRepo: boolean | null;
  activeRepoName: string | null;
  setActiveRepoName: (name: string | null) => void;
  refresh: () => void;
}

export function useMyRepositories(): UseMyRepositoriesResult {
  const queryClient = useQueryClient();
  const { data, isError } = useQuery({
    queryKey: queryKeys.repositories,
    queryFn: listMyRepositories,
  });
  // `null` is the loading sentinel (no data yet); a fetch error resolves to
  // an empty list so the empty-state CTA renders instead of a permanent
  // spinner — matching the pre-Query `.catch(() => setMyRepos([]))`.
  const myRepos: RepositoryRow[] | null = data ?? (isError ? [] : null);

  const refresh = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: queryKeys.repositories });
  }, [queryClient]);

  const [activeRepoName, setActiveRepoNameState] = useState<string | null>(() => {
    try {
      return localStorage.getItem(ACTIVE_REPO_KEY);
    } catch {
      return null;
    }
  });

  const setActiveRepoName = useCallback((name: string | null) => {
    setActiveRepoNameState(name);
    try {
      if (name === null) localStorage.removeItem(ACTIVE_REPO_KEY);
      else localStorage.setItem(ACTIVE_REPO_KEY, name);
    } catch {
      /* ignore quota / disabled storage */
    }
  }, []);

  // Sync activeRepoName with myRepos:
  //   * drop a stale active repo if it's no longer in the user's list
  //   * auto-select the lone repo when the list shrinks to one
  useEffect(() => {
    if (myRepos === null) return;
    if (activeRepoName !== null && !myRepos.some((r) => r.name === activeRepoName)) {
      setActiveRepoName(null);
      return;
    }
    if (myRepos.length === 1 && activeRepoName === null) {
      setActiveRepoName(myRepos[0].name);
    }
  }, [myRepos, activeRepoName, setActiveRepoName]);

  const hasAnyRepo = myRepos === null ? null : myRepos.length > 0;
  return { myRepos, hasAnyRepo, activeRepoName, setActiveRepoName, refresh };
}
