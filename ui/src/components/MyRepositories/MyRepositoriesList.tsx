// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Presentational list of the caller's added repositories with Remove / (admin) Force-purge actions. */

import type { RepositoryRow } from "../../api/client";

interface Props {
  repos: RepositoryRow[];
  loading: boolean;
  busy: string | null;
  onRemove: (repo: RepositoryRow, forcePurge: boolean) => void;
  isAdmin?: boolean;
}

export function MyRepositoriesList({ repos, loading, busy, onRemove, isAdmin }: Props) {
  return (
    <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
      <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
        My repositories
      </div>
      {loading ? (
        <p className="text-sm text-gray-500 p-3">Loading…</p>
      ) : repos.length === 0 ? (
        <p className="text-sm text-gray-500 p-3 italic">
          You haven't added any repositories yet. Pick one from the list below.
        </p>
      ) : (
        <ul className="divide-y divide-gray-800">
          {repos.map((repo) => (
            <li key={repo.id} className="px-4 py-3 flex items-center gap-3">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                  {repo.repo_url ? (
                    <a
                      href={repo.repo_url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-sm font-medium text-blue-400 hover:text-blue-300 transition-colors truncate"
                    >
                      {repo.name}
                    </a>
                  ) : (
                    <span className="text-sm font-medium text-gray-200 truncate">{repo.name}</span>
                  )}
                </div>
                <div className="text-xs text-gray-500 truncate font-mono">{repo.local_path}</div>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <button
                  type="button"
                  onClick={() => onRemove(repo, false)}
                  disabled={busy !== null}
                  className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {busy === `remove:${repo.id}` ? "Removing…" : "Remove"}
                </button>
                {isAdmin && (
                  <button
                    type="button"
                    onClick={() => onRemove(repo, true)}
                    disabled={busy !== null}
                    title="Admin: drop the repository for every user and purge the on-disk clone."
                    className="text-xs px-3 py-1.5 rounded-lg bg-red-900/40 text-red-300 border border-red-800 hover:bg-red-900/70 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                  >
                    Force purge
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
