// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useEffect, useRef, useCallback } from "react";
import { apiJson } from "../../api/client";
import type { GitHubRepo, Workspace } from "../../api/types";

interface Props {
  onSelect: (fullName: string) => void;
  onClose: () => void;
}

export function RepoPickerModal({ onSelect, onClose }: Props) {
  const [repos, setRepos] = useState<GitHubRepo[]>([]);
  const [checkedOut, setCheckedOut] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [search, setSearch] = useState("");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const requestIdRef = useRef(0);

  const fetchRepos = useCallback((query: string) => {
    setLoading(true);
    setError("");
    const id = ++requestIdRef.current;
    const endpoint = query
      ? `/api/github/repos?q=${encodeURIComponent(query)}`
      : "/api/github/repos";
    apiJson<GitHubRepo[]>(endpoint)
      .then((data) => { if (requestIdRef.current === id) setRepos(data); })
      .catch((e) => { if (requestIdRef.current === id) setError(e.message); })
      .finally(() => { if (requestIdRef.current === id) setLoading(false); });
  }, []);

  // Fetch already-checked-out workspace names on mount.
  useEffect(() => {
    apiJson<Workspace[]>("/api/workspaces")
      .then((ws) => setCheckedOut(new Set(ws.map((w) => w.name))))
      .catch(() => {});
  }, []);

  // Debounced search (also fires immediately on mount with initial empty search)
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      fetchRepos(search);
    }, search === "" ? 0 : 400);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [search, fetchRepos]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-3xl w-full mx-4 max-h-[80vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">Setup a New Project</h3>
          <button
            onClick={onClose}
            className="text-gray-500 hover:text-gray-300 cursor-pointer"
          >
            &times;
          </button>
        </div>

        <div className="p-4 border-b border-gray-800">
          <input
            type="text"
            placeholder="Search repositories..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
            autoFocus
          />
        </div>

        <div className="overflow-y-auto flex-1 p-4">
          {loading && <p className="text-gray-500 text-sm">Loading repositories...</p>}
          {error && <p className="text-red-400 text-sm">{error}</p>}
          {!loading && repos.length === 0 && !error && (
            <p className="text-gray-500 text-sm">No repositories found.</p>
          )}
          {repos.map((r) => {
            const repoName = r.full_name.split("/")[1] ?? r.full_name;
            const alreadyCloned = checkedOut.has(repoName);
            return (
              <div
                key={r.full_name}
                className="flex items-center justify-between px-4 py-3 rounded-lg hover:bg-gray-800 transition-colors"
              >
                <div className="flex-1 min-w-0 mr-3">
                  <div className="flex items-center gap-2">
                    <span className={`text-sm font-medium truncate ${alreadyCloned ? "text-gray-500" : "text-gray-200"}`}>
                      {r.full_name}
                    </span>
                    {r.private && (
                      <span className="text-xs bg-gray-700 text-gray-400 px-1.5 py-0.5 rounded flex-shrink-0">
                        Private
                      </span>
                    )}
                    {alreadyCloned && (
                      <span className="text-xs bg-gray-800 text-gray-500 px-1.5 py-0.5 rounded flex-shrink-0">
                        Cloned
                      </span>
                    )}
                  </div>
                  {r.description && (
                    <p className="text-xs text-gray-500 truncate mt-0.5">
                      {r.description}
                    </p>
                  )}
                </div>
                <button
                  onClick={() => onSelect(r.full_name)}
                  disabled={alreadyCloned}
                  className="text-sm px-4 py-1.5 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer flex-shrink-0"
                >
                  Select
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
