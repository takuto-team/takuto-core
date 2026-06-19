// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Left-pane repository picker, shared by Repository Settings and Workflows.
 *
 * Each row shows the repo name; the optional green-dot "set" / gray-dot "none"
 * badge is rendered only when an item supplies `hasCommands` (Repository
 * Settings uses it to flag which repos have init/run commands; Workflows omits
 * it). Presentational — the parent owns the repo list, selection, and loading.
 */

export interface RepoSidebarItem {
  name: string;
  /** When defined, render a "set"/"none" badge; when undefined, no badge. */
  hasCommands?: boolean;
}

interface Props {
  repos: RepoSidebarItem[];
  loading: boolean;
  selected: string | null;
  onSelect: (name: string) => void;
}

export function RepoSidebar({ repos, loading, selected, onSelect }: Props) {
  return (
    <aside className="md:w-1/3 md:max-w-xs border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
      <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
        Repositories
      </div>
      {loading ? (
        <p className="text-sm text-gray-500 p-3">Loading…</p>
      ) : repos.length === 0 ? (
        <p className="text-sm text-gray-500 p-3">No repositories found.</p>
      ) : (
        <ul className="divide-y divide-gray-800">
          {repos.map((r) => {
            const isSelected = selected === r.name;
            return (
              <li key={r.name}>
                <button
                  type="button"
                  onClick={() => onSelect(r.name)}
                  className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-left text-sm cursor-pointer transition-colors ${
                    isSelected
                      ? "bg-blue-950/40 text-blue-200"
                      : "text-gray-300 hover:bg-gray-900"
                  }`}
                >
                  <span className="truncate font-medium">{r.name}</span>
                  {r.hasCommands !== undefined && (
                    <span className="flex items-center gap-1.5 shrink-0">
                      {r.hasCommands ? (
                        <span
                          className="inline-flex items-center gap-1 text-[11px] text-emerald-300"
                          title="You have commands set for this repository"
                        >
                          <span className="w-2 h-2 rounded-full bg-emerald-400" />
                          set
                        </span>
                      ) : (
                        <span
                          className="inline-flex items-center gap-1 text-[11px] text-gray-500"
                          title="No commands set for this repository"
                        >
                          <span className="w-2 h-2 rounded-full bg-gray-600" />
                          none
                        </span>
                      )}
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
