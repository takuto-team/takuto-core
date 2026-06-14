// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/** Presentational list of GitHub-accessible repositories the caller can add, with a search box. */

import type { GitHubRepo } from "../../api/types";

interface Props {
  repos: GitHubRepo[];
  loading: boolean;
  error: string;
  search: string;
  busy: string | null;
  onSearchChange: (value: string) => void;
  onAdd: (repo: GitHubRepo) => void;
}

export function AvailableRepositoriesList({
  repos,
  loading,
  error,
  search,
  busy,
  onSearchChange,
  onAdd,
}: Props) {
  const trimmed = search.trim();
  return (
    <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
      <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500 flex items-center justify-between gap-3">
        <span>Available repositories</span>
        <input
          type="search"
          value={search}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder="Search…"
          className="flex-1 max-w-xs bg-gray-900 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-500 normal-case tracking-normal"
        />
      </div>
      {error ? (
        <p className="text-sm text-red-400 p-3">{error}</p>
      ) : loading ? (
        <p className="text-sm text-gray-500 p-3">Loading from GitHub…</p>
      ) : repos.length === 0 ? (
        <p className="text-sm text-gray-500 p-3 italic">
          {trimmed.length > 0
            ? `No repositories matching "${trimmed}" available to add.`
            : "No additional repositories accessible via the configured GitHub credentials."}
        </p>
      ) : (
        <ul className="divide-y divide-gray-800">
          {repos.map((repo) => (
            <li key={repo.full_name} className="px-4 py-3 flex items-center gap-3">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 min-w-0">
                  <a
                    href={repo.html_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-sm font-medium text-blue-400 hover:text-blue-300 transition-colors truncate"
                  >
                    {repo.full_name}
                  </a>
                  {repo.private && (
                    <span
                      className="text-[11px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-400 border border-gray-700 shrink-0"
                      title="Private repository"
                    >
                      private
                    </span>
                  )}
                </div>
                {repo.description && (
                  <div className="text-xs text-gray-500 truncate">{repo.description}</div>
                )}
              </div>
              <button
                type="button"
                onClick={() => onAdd(repo)}
                disabled={busy !== null}
                className="text-xs px-3 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer shrink-0"
              >
                {busy === `add:${repo.full_name}` ? "Cloning…" : "Add"}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
