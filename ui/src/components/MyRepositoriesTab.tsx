// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Plan-10: My Repositories tab.
 *
 * Three sections:
 *   1. My repositories       — repos the caller has added. Remove button per row.
 *   2. Available repositories — repos registered but not yet added. Add button per row.
 *   3. Add new repository    — a free-form GitHub URL. Submits a clone-if-needed flow.
 *
 * No admin gate — every authenticated user manages their own list. Admins can
 * pass `force_purge: true` when removing a repo to drop the row for everyone.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  addRepository,
  listAvailableRepositories,
  listMyRepositories,
  removeRepository,
  type RepositoryRow,
} from "../api/client";
import { ConfirmModal } from "./modals/ConfirmModal";

const REPO_URL_RE = /^https:\/\/github\.com\/[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/;

interface Props {
  isAdmin?: boolean;
}

export function MyRepositoriesTab({ isAdmin }: Props) {
  const [mine, setMine] = useState<RepositoryRow[]>([]);
  const [available, setAvailable] = useState<RepositoryRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [removeTarget, setRemoveTarget] = useState<{
    repo: RepositoryRow;
    forcePurge: boolean;
  } | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    Promise.all([listMyRepositories(), listAvailableRepositories()])
      .then(([m, a]) => {
        setMine(m);
        setAvailable(a);
      })
      .catch((e) => setError(String((e as Error).message || e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const repoUrlValidationError = useMemo(() => {
    const trimmed = repoUrl.trim();
    if (trimmed.length === 0) return null;
    if (trimmed.length > 2000) return "URL is too long.";
    if (!REPO_URL_RE.test(trimmed)) {
      return "Must be a GitHub HTTPS URL: https://github.com/owner/repo";
    }
    if (trimmed.includes("@") || trimmed.includes("?") || trimmed.includes("#") || trimmed.includes("..")) {
      return "URL must not contain credentials, query strings, fragments, or `..` segments.";
    }
    return null;
  }, [repoUrl]);

  const handleAddExisting = async (repo: RepositoryRow) => {
    setError("");
    setSuccess("");
    setBusy(`add:${repo.id}`);
    try {
      await addRepository({ repository_id: repo.id });
      setSuccess(`Added "${repo.name}" to your dashboard.`);
      refresh();
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setBusy(null);
    }
  };

  const handleAddNew = async () => {
    if (repoUrlValidationError) {
      setError(repoUrlValidationError);
      return;
    }
    const url = repoUrl.trim();
    if (!url) return;
    setError("");
    setSuccess("");
    setBusy("clone");
    try {
      const row = await addRepository({ repo_url: url });
      setSuccess(`Repository "${row.name}" added to your dashboard.`);
      setRepoUrl("");
      refresh();
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
      refresh();
    } catch (e) {
      setError(String((e as Error).message || e));
    } finally {
      setBusy(null);
    }
  };

  const removeConfirmMessage = removeTarget ? (() => {
    const { repo, forcePurge } = removeTarget;
    const co = repo.co_users_count ?? 0;
    if (forcePurge) {
      return `Force-purge "${repo.name}": all ${co + 1} associated user(s) will lose access, and the on-disk clone at ${repo.local_path} will be deleted. This cannot be undone.`;
    }
    if (co === 0) {
      return `You are the last user associated with "${repo.name}". Removing it will also delete the on-disk clone at ${repo.local_path}. The repository can be re-added later (it will be re-cloned from the remote).`;
    }
    return `Remove "${repo.name}" from your dashboard. ${co} other user(s) still have it added — the on-disk clone will be kept.`;
  })() : "";

  return (
    <div className="space-y-6">
      <header>
        <h2 className="text-base font-semibold text-gray-300 mb-1">My Repositories</h2>
        <p className="text-sm text-gray-500">
          Repositories you've added show up on your dashboard. Add a registered repo or
          paste a GitHub URL to clone a new one. The on-disk clone is shared between every
          user that adds the same repo.
        </p>
      </header>

      {error && <p className="text-sm text-red-400">{error}</p>}
      {success && <p className="text-sm text-green-400">{success}</p>}

      {/* Section 1: my repositories */}
      <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
        <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
          My repositories
        </div>
        {loading ? (
          <p className="text-sm text-gray-500 p-3">Loading…</p>
        ) : mine.length === 0 ? (
          <p className="text-sm text-gray-500 p-3 italic">
            You haven't added any repositories yet. Add an existing one below or paste a URL.
          </p>
        ) : (
          <ul className="divide-y divide-gray-800">
            {mine.map((repo) => {
              const co = repo.co_users_count ?? 0;
              const isLast = co === 0;
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
                      {isLast && (
                        <span
                          className="text-[11px] px-1.5 py-0.5 rounded bg-amber-900/40 text-amber-300 border border-amber-800/50 shrink-0"
                          title="You are the last user associated with this repository; removing it will purge the on-disk clone."
                        >
                          last user
                        </span>
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

      {/* Section 2: available repositories */}
      <section className="border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
        <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
          Available repositories
        </div>
        {loading ? (
          <p className="text-sm text-gray-500 p-3">Loading…</p>
        ) : available.length === 0 ? (
          <p className="text-sm text-gray-500 p-3 italic">
            All registered repositories are already on your dashboard.
          </p>
        ) : (
          <ul className="divide-y divide-gray-800">
            {available.map((repo) => (
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
                <button
                  type="button"
                  onClick={() => handleAddExisting(repo)}
                  disabled={busy !== null}
                  className="text-xs px-3 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer shrink-0"
                >
                  {busy === `add:${repo.id}` ? "Adding…" : "Add"}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* Section 3: add new repository */}
      <section className="border border-gray-800 rounded-lg bg-gray-950 p-4 space-y-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-1">Add new repository</h3>
          <p className="text-xs text-gray-500">
            Paste a GitHub HTTPS URL. If the repository isn't already cloned, it will be
            cloned now. This can take a while for large repositories.
          </p>
        </div>
        <div className="flex gap-2 items-start">
          <input
            type="url"
            value={repoUrl}
            onChange={(e) => setRepoUrl(e.target.value)}
            placeholder="https://github.com/owner/repo"
            disabled={busy === "clone"}
            className="flex-1 bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 font-mono disabled:opacity-50"
          />
          <button
            type="button"
            onClick={handleAddNew}
            disabled={busy !== null || !repoUrl.trim() || !!repoUrlValidationError}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer shrink-0"
          >
            {busy === "clone" ? "Cloning…" : "Clone & Add"}
          </button>
        </div>
        {repoUrlValidationError && repoUrl.trim().length > 0 && (
          <p className="text-xs text-red-400">{repoUrlValidationError}</p>
        )}
        {busy === "clone" && (
          <p className="text-xs text-gray-400 italic">
            Cloning repository&hellip; this may take a few minutes for large repos.
          </p>
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
