// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState, useRef, useEffect } from "react";
import { Link } from "react-router-dom";
import type { RepositoryRow } from "../api/client";

interface Props {
  connected: boolean;
  authEnabled: boolean;
  githubAppConfigured: boolean;
  githubAppInstallationId?: number;
  githubAppName?: string | null;
  onLogout: () => void;
  /**
   * Plan-10: the user's added repositories. When the list is non-empty the
   * header shows a small picker (active repo + chevron) instead of the bare
   * "My Repositories" link.
   */
  repos?: RepositoryRow[];
  /**
   * Name of the currently-filtered repo (matches `RepositoryRow.name` and
   * `WorkflowSummary.workspace_name`). `null` = "All repositories".
   */
  activeRepoName?: string | null;
  /** Called when the user picks a different repo from the dropdown. */
  onSelectRepo?: (name: string | null) => void;
}

/**
 * Plan-10: per-user repo picker in the header.
 *
 * - When the caller has zero repos added → shows the legacy CTA link to the
 *   "My Repositories" tab so they can add one.
 * - When the caller has ≥1 repo added → shows a compact dropdown listing
 *   those repos with an "All repositories" option and a "Manage…" link
 *   back to the Config tab.
 *
 * The dropdown only filters the dashboard view; it does NOT mutate any
 * global state. The selection lives in the parent component (Dashboard) and
 * is persisted to localStorage by the caller.
 */
export function Header({
  connected,
  authEnabled,
  githubAppConfigured,
  githubAppInstallationId,
  githubAppName,
  onLogout,
  repos,
  activeRepoName,
  onSelectRepo,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const pickerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    function handleClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [menuOpen]);

  useEffect(() => {
    if (!pickerOpen) return;
    function handleClick(e: MouseEvent) {
      if (pickerRef.current && !pickerRef.current.contains(e.target as Node)) {
        setPickerOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [pickerOpen]);

  const hasRepos = (repos?.length ?? 0) > 0;
  const activeRepo = activeRepoName
    ? (repos ?? []).find((r) => r.name === activeRepoName) ?? null
    : null;
  const pickerLabel = activeRepo ? activeRepo.name : "All repositories";

  return (
    <header className="border-b border-gray-800 bg-gray-950/80 backdrop-blur-sm sticky top-0 z-40">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
        <div className="flex items-center justify-between h-14">
          <div className="flex items-center gap-3">
            <span className="text-lg font-bold tracking-tight text-white">Maestro</span>
            <span className="text-gray-700">|</span>
            {hasRepos && onSelectRepo ? (
              <div className="relative" ref={pickerRef}>
                <button
                  type="button"
                  onClick={() => setPickerOpen((v) => !v)}
                  className="inline-flex items-center gap-1.5 text-xs text-gray-300 hover:text-white transition-colors px-2 py-1 rounded hover:bg-gray-800 cursor-pointer"
                  title="Switch active repository (filters the dashboard)"
                >
                  <span className="font-medium">{pickerLabel}</span>
                  <svg className="w-3 h-3 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
                  </svg>
                </button>
                {pickerOpen && (
                  <div className="absolute left-0 mt-1 w-60 bg-gray-900 border border-gray-700 rounded-lg shadow-lg py-1 z-50 max-h-80 overflow-y-auto">
                    <button
                      type="button"
                      onClick={() => { setPickerOpen(false); onSelectRepo(null); }}
                      className={`w-full text-left px-3 py-1.5 text-sm transition-colors cursor-pointer ${
                        activeRepoName === null
                          ? "bg-blue-950/60 text-blue-300"
                          : "text-gray-300 hover:bg-gray-800 hover:text-white"
                      }`}
                    >
                      All repositories
                    </button>
                    <div className="border-t border-gray-800 my-1" />
                    {(repos ?? []).map((r) => (
                      <button
                        key={r.id}
                        type="button"
                        onClick={() => { setPickerOpen(false); onSelectRepo(r.name); }}
                        className={`w-full text-left px-3 py-1.5 text-sm transition-colors cursor-pointer truncate ${
                          activeRepoName === r.name
                            ? "bg-blue-950/60 text-blue-300"
                            : "text-gray-300 hover:bg-gray-800 hover:text-white"
                        }`}
                        title={r.local_path}
                      >
                        {r.name}
                      </button>
                    ))}
                    <div className="border-t border-gray-800 my-1" />
                    <Link
                      to="/config.html?tab=repositories"
                      onClick={() => setPickerOpen(false)}
                      className="block px-3 py-1.5 text-xs text-gray-400 hover:bg-gray-800 hover:text-white transition-colors"
                    >
                      Manage repositories…
                    </Link>
                  </div>
                )}
              </div>
            ) : (
              <Link
                to="/config.html?tab=repositories"
                className="text-xs text-gray-500 hover:text-gray-300 transition-colors cursor-pointer"
              >
                My Repositories
              </Link>
            )}
          </div>

          <div className="flex items-center gap-4">
            <span className="flex items-center gap-1.5 text-xs text-gray-400">
              <span
                className={`inline-block w-2 h-2 rounded-full ${
                  connected ? "bg-green-500 animate-pulse" : "bg-gray-600"
                }`}
              />
              {connected ? "Connected" : "Disconnected"}
            </span>

            {githubAppConfigured && (
              <>
                <span className="text-gray-700">|</span>
                <span
                  className="inline-flex items-center gap-1.5 text-xs bg-violet-950/60 border border-violet-700/50 text-violet-300 px-2 py-0.5 rounded-full"
                  title={githubAppName ? `${githubAppName} connected` : "GitHub App connected"}
                >
                  {githubAppInstallationId ? (
                    <img
                      src={`https://avatars.githubusercontent.com/in/${githubAppInstallationId}?s=28`}
                      alt=""
                      className="w-3.5 h-3.5 rounded-full"
                    />
                  ) : (
                    <GitHubIcon />
                  )}
                  {githubAppName ? `${githubAppName} connected` : "App Connected"}
                </span>
              </>
            )}

            {/* Hamburger menu */}
            <div className="relative" ref={menuRef}>
              <button
                onClick={() => setMenuOpen((v) => !v)}
                className="p-1.5 rounded text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition-colors cursor-pointer"
                aria-label="Menu"
              >
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M4 6h16M4 12h16M4 18h16" />
                </svg>
              </button>
              {menuOpen && (
                <div className="absolute right-0 mt-1 w-44 bg-gray-900 border border-gray-700 rounded-lg shadow-lg py-1 z-50">
                  <Link
                    to="/config.html"
                    className="block px-4 py-2 text-sm text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
                    onClick={() => setMenuOpen(false)}
                  >
                    Configuration
                  </Link>
                  <Link
                    to="/config.html?tab=repositories"
                    className="block px-4 py-2 text-sm text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
                    onClick={() => setMenuOpen(false)}
                  >
                    My Repositories
                  </Link>
                  {/* Phase 2 — per-user credential surface (every signed-in
                      user can manage their own keys). */}
                  <Link
                    to="/me/credentials"
                    className="block px-4 py-2 text-sm text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
                    onClick={() => setMenuOpen(false)}
                  >
                    My credentials
                  </Link>
                  {authEnabled && (
                    <button
                      onClick={() => { setMenuOpen(false); onLogout(); }}
                      className="w-full text-left px-4 py-2 text-sm text-gray-300 hover:bg-gray-800 hover:text-white transition-colors cursor-pointer"
                    >
                      Log out
                    </button>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </header>
  );
}

function GitHubIcon() {
  return (
    <svg className="w-3.5 h-3.5 flex-shrink-0" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z" />
    </svg>
  );
}
