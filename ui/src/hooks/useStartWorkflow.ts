// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * `useStartWorkflow` — owns the repository selector state for
 * `TicketDetailModal` when it is rendered with `showStartButton=true`.
 *
 * The repo dropdown shows at the top of the modal
 * (`StartWorkflowRepoBanner`) and the "Add to Dashboard" button at the
 * bottom (`StartWorkflowFooter`) both read the selected `repositoryId` —
 * so the state lives in this hook called once from the modal shell.
 *
 * Auto-selects the first repository when at least one exists (preserves
 * the modal's pre-extraction behaviour).
 */

import { useEffect, useState } from "react";
import { listMyRepositories, type RepositoryRow } from "../api/client";

export interface UseStartWorkflowResult {
  repos: RepositoryRow[];
  repositoryId: string;
  setRepositoryId: (id: string) => void;
  loadingRepos: boolean;
  /** True when the repo is fixed to a source repo (GitHub issue) — no selector. */
  repoLocked: boolean;
}

/**
 * `lockedRepoName` — when set (a GitHub issue, whose repo is its source), the
 * repository is pinned to the matching repo and the selector is suppressed.
 * Otherwise (Jira / manual, which aren't repo-bound) the user picks a repo.
 */
export function useStartWorkflow(
  showStartButton: boolean,
  lockedRepoName?: string | null,
): UseStartWorkflowResult {
  const [repos, setRepos] = useState<RepositoryRow[]>([]);
  const [repositoryId, setRepositoryId] = useState("");
  const [loadingRepos, setLoadingRepos] = useState(showStartButton);

  useEffect(() => {
    if (!showStartButton) return;
    setLoadingRepos(true);
    listMyRepositories()
      .then((rs) => {
        setRepos(rs);
        if (rs.length === 0) return;
        // GitHub issue → pin to its source repo; else default to the first repo.
        const locked = lockedRepoName
          ? rs.find((r) => r.name === lockedRepoName)
          : undefined;
        setRepositoryId((locked ?? rs[0]).id);
      })
      .catch(() => setRepos([]))
      .finally(() => setLoadingRepos(false));
  }, [showStartButton, lockedRepoName]);

  const repoLocked = Boolean(lockedRepoName) && repos.some((r) => r.name === lockedRepoName);
  return { repos, repositoryId, setRepositoryId, loadingRepos, repoLocked };
}
