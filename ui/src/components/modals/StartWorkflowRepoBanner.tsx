// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Repo-selector banner shown at the top of `TicketDetailModal` when the
 * modal is opened to start a workflow (`showStartButton=true`). Three
 * rendering modes: zero repos → amber CTA link; one repo → read-only
 * label; many repos → dropdown. Pairs with `StartWorkflowFooter` and
 * `useStartWorkflow`.
 */

import { Link } from "react-router-dom";
import type { RepositoryRow } from "../../api/client";

interface Props {
  showStartButton: boolean;
  repos: RepositoryRow[];
  repositoryId: string;
  setRepositoryId: (id: string) => void;
  loadingRepos: boolean;
  onClose: () => void;
}

export function StartWorkflowRepoBanner({
  showStartButton,
  repos,
  repositoryId,
  setRepositoryId,
  loadingRepos,
  onClose,
}: Props) {
  if (!showStartButton) return null;
  return (
    <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-3">
      <label className="text-xs text-gray-400 shrink-0">Repository:</label>
      {loadingRepos ? (
        <span className="text-xs text-gray-500">Loading…</span>
      ) : repos.length === 0 ? (
        <span className="text-xs text-amber-300">
          No repositories on your dashboard.{" "}
          <Link
            to="/config.html?tab=repositories"
            className="underline hover:text-amber-100"
            onClick={onClose}
          >
            Add one
          </Link>{" "}
          before starting a work item.
        </span>
      ) : repos.length === 1 ? (
        <span className="text-xs text-gray-300 font-mono">{repos[0].name}</span>
      ) : (
        <select
          value={repositoryId}
          onChange={(e) => setRepositoryId(e.target.value)}
          className="bg-gray-950 border border-gray-700 rounded-lg px-2 py-1 text-xs text-gray-200 font-mono"
        >
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}
