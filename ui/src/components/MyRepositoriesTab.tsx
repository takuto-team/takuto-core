// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * My Repositories tab.
 *
 * Two sections:
 *   1. My repositories       — repos the caller has added; Remove / Force purge.
 *   2. Available repositories — repos the deployment's GitHub App / PAT can see
 *      that aren't on the caller's dashboard yet, with a search input. Click
 *      "Add" → clones the repo (if not yet on disk) and associates it with
 *      the caller. The set of addable repos is dictated by the GitHub
 *      installation/PAT scope — there is no free-form URL paste.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addRepository,
  listGitHubAccessibleRepos,
  listMyRepositories,
  removeRepository,
  type RepositoryRow,
} from "../api/client";
import type { GitHubRepo } from "../api/types";
import { ConfirmModal } from "./modals/ConfirmModal";

interface Props {
  isAdmin?: boolean;
}

export function MyRepositoriesTab({ isAdmin }: Props) {
  const [mine, setMine] = useState<RepositoryRow[]>([]);
  const [available, setAvailable] = useState<GitHubRepo[]>([]);
  const [loadingMine, setLoadingMine] = useState(true);
  const [loadingAvailable, setLoadingAvailable] = useState(true);
  const [availableError, setAvailableError] = useState<string>("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [removeTarget, setRemoveTarget] = useState<{
    repo: RepositoryRow;
    forcePurge: boolean;
  } | null>(null);

  // Debounce the search input — 300 ms — so we don't slam the GitHub API on
  // every keystroke.
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (debounceTimer.current) clearTimeout(debounceTimer.current);
    debounceTimer.current = setTimeout(() => setDebouncedSearch(search), 300);
    return () => {
      if (debounceTimer.current) clearTimeout(debounceTimer.current);
    };
  }, [search]);

  const refreshMine = useCallback(() => {
    setLoadingMine(true);
    listMyRepositories()
      .then(setMine)
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoadingMine(false));
  }, []);

  const refreshAvailable = useCallback((q: string) => {
    setLoadingAvailable(true);
    setAvailableError("");
    listGitHubAccessibleRepos(q)
      .then(setAvailable)
      .catch((e) => setAvailableError(String((e as Error).message || e)))
      .finally(() => setLoadingAvailable(false));
  }, []);

  useEffect(() => {
    refreshMine();
  }, [refreshMine]);

  useEffect(() => {
    refreshAvailable(debouncedSearch);
  }, [refreshAvailable, debouncedSearch]);

  // Fast-lookup set of repo URLs the user has already added, so we can hide
  // those rows from the "Available" list.
  const mineUrls = useMemo(() => {
    const s = new Set<string>();
    for (const r of mine) {
      if (r.repo_url) s.add(r.repo_url.replace(/\.git$/, ""));
    }
    return s;
  }, [mine]);

  const filteredAvailable = useMemo(() => {
    return available.filter((r) => !mineUrls.has(r.html_url.replace(/\.git$/, "")));
  }, [available, mineUrls]);

  const handleAddFromGitHub = async (repo: GitHubRepo) => {
    setError("");
    setSuccess("");
    setBusy(`add:${repo.full_name}`);
    try {
      const row = await addRepository({ repo_url: repo.html_url });
      setSuccess(`Added "${row.name}" to your dashboard.`);
      refreshMine();
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setBusy(null);
    }
  };

  const handleRemove = async () => {
    if (!removeTarget) return;
    const { repo, forcePurge } = removeTarget;
    setRemoveTarget(null);
    setError("");
    setSuccess("");
    setBusy(`remove:${repo.id}`);
    try {
      await removeRepository(repo.id, forcePurge ? { force_purge: true } : undefined);
      setSuccess(`Removed "${repo.name}" from your dashboard.`);
      refreshMine();
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setBusy(null);
    }
  };

  const removeConfirmMessage = removeTarget
    ? (() => {
        const { repo, forcePurge } = removeTarget;
        const co = repo.co_users_count ?? 0;
        if (forcePurge) {
          return `Force-purge "${repo.name}": all ${co + 1} associated user(s) will lose access, and the on-disk clone at ${repo.local_path} will be deleted. This cannot be undone.`;
        }
        if (co === 0) {
          return `You are the last user associated with "${repo.name}". Removing it will also delete the on-disk clone at ${repo.local_path}. The repository can be re-added later (it will be re-cloned from the remote).`;
        }
        return `Remove "${repo.name}" from your dashboard. ${co} other user(s) still have it added — the on-disk clone will be kept.`;
      })()
    : "";

  return (
    <div className="space-y-6">
      <header>
        <h2 className="text-base font-semibold text-gray-300 mb-1">My Repositories</h2>
        <p className="text-sm text-gray-500">
          Repositories you've added show up on your dashboard. Available repositories
          come from the deployment's GitHub App installation (or fallback PAT). The on-disk
          clone is shared between every user that adds the same repo.
        </p>
      </header>

      {error && <p className="text-sm text-red-400">{error}</p>}
      {success && <p className="text-sm text-green-400">{success}</p>}

      {/* Section 1: my repositories */}
      <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
        <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
          My repositories
        </div>
        {loadingMine ? (
          <p className="text-sm text-gray-500 p-3">Loading…</p>
        ) : mine.length === 0 ? (
          <p className="text-sm text-gray-500 p-3 italic">
            You haven't added any repositories yet. Pick one from the list below.
          </p>
        ) : (
          <ul className="divide-y divide-gray-800">
            {mine.map((repo) => {
              return (
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
                      onClick={() => setRemoveTarget({ repo, forcePurge: false })}
                      disabled={busy !== null}
                      className="text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                    >
                      {busy === `remove:${repo.id}` ? "Removing…" : "Remove"}
                    </button>
                    {isAdmin && (
                      <button
                        type="button"
                        onClick={() => setRemoveTarget({ repo, forcePurge: true })}
                        disabled={busy !== null}
                        title="Admin: drop the repository for every user and purge the on-disk clone."
                        className="text-xs px-3 py-1.5 rounded-lg bg-red-900/40 text-red-300 border border-red-800 hover:bg-red-900/70 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                      >
                        Force purge
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {/* Section 2: available (GitHub-accessible) repositories */}
      <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
        <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500 flex items-center justify-between gap-3">
          <span>Available repositories</span>
          <input
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search…"
            className="flex-1 max-w-xs bg-gray-900 border border-gray-700 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-500 normal-case tracking-normal"
          />
        </div>
        {availableError ? (
          <p className="text-sm text-red-400 p-3">{availableError}</p>
        ) : loadingAvailable ? (
          <p className="text-sm text-gray-500 p-3">Loading from GitHub…</p>
        ) : filteredAvailable.length === 0 ? (
          <p className="text-sm text-gray-500 p-3 italic">
            {search.trim().length > 0
              ? `No repositories matching "${search.trim()}" available to add.`
              : "No additional repositories accessible via the configured GitHub credentials."}
          </p>
        ) : (
          <ul className="divide-y divide-gray-800">
            {filteredAvailable.map((repo) => (
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
                  onClick={() => handleAddFromGitHub(repo)}
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

      {removeTarget && (
        <ConfirmModal
          title={removeTarget.forcePurge ? "Force-purge repository" : "Remove repository"}
          message={removeConfirmMessage}
          onConfirm={handleRemove}
          onCancel={() => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}
