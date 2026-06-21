// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Data layer for the "My Repositories" tab: the caller's repository set, the
 * GitHub-accessible "available" set (debounced search), and the add / remove
 * mutations. Keeping all I/O here lets `MyRepositoriesTab` stay presentational
 * (CODING_STANDARDS §3). The "mine" list shares the `repositories` query key
 * with the dashboard repo picker, so adding/removing here refreshes both.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addRepository,
  listGitHubAccessibleRepos,
  listMyRepositories,
  removeRepository,
  type RepositoryRow,
} from "../api/client";
import { queryKeys } from "../api/queryClient";
import type { GitHubRepo } from "../api/types";

const SEARCH_DEBOUNCE_MS = 300;

export interface UseRepositoryAdminResult {
  mine: RepositoryRow[];
  loadingMine: boolean;
  filteredAvailable: GitHubRepo[];
  loadingAvailable: boolean;
  availableError: string;
  search: string;
  setSearch: (s: string) => void;
  busy: string | null;
  error: string;
  success: string;
  addFromGitHub: (repo: GitHubRepo) => void;
  remove: (repo: RepositoryRow, forcePurge: boolean) => void;
}

function messageOf(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function useRepositoryAdmin(): UseRepositoryAdminResult {
  const { t } = useTranslation("config");
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [opError, setOpError] = useState("");
  const [success, setSuccess] = useState("");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");

  // Debounce the search input so we don't slam the GitHub API on every keystroke.
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(search), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [search]);

  const mineQuery = useQuery({
    queryKey: queryKeys.repositories,
    queryFn: listMyRepositories,
  });
  const availableQuery = useQuery({
    queryKey: ["github-accessible-repos", debouncedSearch],
    queryFn: () => listGitHubAccessibleRepos(debouncedSearch),
  });

  const mine = useMemo(() => mineQuery.data ?? [], [mineQuery.data]);
  const available = useMemo(() => availableQuery.data ?? [], [availableQuery.data]);

  // Fast-lookup set of repo URLs the user already added, to hide those rows
  // from the "Available" list.
  const mineUrls = useMemo(() => {
    const s = new Set<string>();
    for (const r of mine) {
      if (r.repo_url) s.add(r.repo_url.replace(/\.git$/, ""));
    }
    return s;
  }, [mine]);

  const filteredAvailable = useMemo(
    () => available.filter((r) => !mineUrls.has(r.html_url.replace(/\.git$/, ""))),
    [available, mineUrls]
  );

  const addMutation = useMutation({
    mutationFn: (repo: GitHubRepo) => addRepository({ repo_url: repo.html_url }),
    onMutate: (repo) => {
      setOpError("");
      setSuccess("");
      setBusy(`add:${repo.full_name}`);
    },
    onSuccess: (row) => {
      setSuccess(t("repositories.added", { name: row.name }));
      queryClient.invalidateQueries({ queryKey: queryKeys.repositories });
    },
    onError: (e) => setOpError(messageOf(e)),
    onSettled: () => setBusy(null),
  });

  const removeMutation = useMutation({
    mutationFn: ({ repo, forcePurge }: { repo: RepositoryRow; forcePurge: boolean }) =>
      removeRepository(repo.id, forcePurge ? { force_purge: true } : undefined),
    onMutate: ({ repo }) => {
      setOpError("");
      setSuccess("");
      setBusy(`remove:${repo.id}`);
    },
    onSuccess: (_data, { repo }) => {
      setSuccess(t("repositories.removed", { name: repo.name }));
      queryClient.invalidateQueries({ queryKey: queryKeys.repositories });
    },
    onError: (e) => setOpError(messageOf(e)),
    onSettled: () => setBusy(null),
  });

  const addFromGitHub = useCallback(
    (repo: GitHubRepo) => addMutation.mutate(repo),
    [addMutation]
  );
  const remove = useCallback(
    (repo: RepositoryRow, forcePurge: boolean) => removeMutation.mutate({ repo, forcePurge }),
    [removeMutation]
  );

  const mineLoadError = mineQuery.isError ? messageOf(mineQuery.error) : "";

  return {
    mine,
    loadingMine: mineQuery.isPending,
    filteredAvailable,
    loadingAvailable: availableQuery.isFetching,
    availableError: availableQuery.isError ? messageOf(availableQuery.error) : "",
    search,
    setSearch,
    busy,
    error: opError || mineLoadError,
    success,
    addFromGitHub,
    remove,
  };
}
