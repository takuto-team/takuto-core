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
import { listMyRepositories, type RepositoryRow } from "../api/client";

const ACTIVE_REPO_KEY = "maestro.activeRepoName";

export interface UseMyRepositoriesResult {
  myRepos: RepositoryRow[] | null;
  hasAnyRepo: boolean | null;
  activeRepoName: string | null;
  setActiveRepoName: (name: string | null) => void;
  refresh: () => void;
}

export function useMyRepositories(): UseMyRepositoriesResult {
  const [myRepos, setMyRepos] = useState<RepositoryRow[] | null>(null);

  const refresh = useCallback(() => {
    listMyRepositories()
      .then(setMyRepos)
      .catch(() => setMyRepos([]));
  }, []);

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

  useEffect(() => {
    refresh();
  }, [refresh]);

  const hasAnyRepo = myRepos === null ? null : myRepos.length > 0;
  return { myRepos, hasAnyRepo, activeRepoName, setActiveRepoName, refresh };
}
