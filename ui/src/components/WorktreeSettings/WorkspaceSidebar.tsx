// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorktreeCommandsWorkspaceEntry } from "../../api/client";

interface Props {
  workspaces: WorktreeCommandsWorkspaceEntry[];
  loading: boolean;
  selected: string | null;
  onSelect: (name: string) => void;
}

/**
 * Left-pane workspace picker. Each row shows the workspace name and a
 * green-dot "set" / gray-dot "none" badge derived from `has_my_commands`.
 */
export function WorkspaceSidebar({ workspaces, loading, selected, onSelect }: Props) {
  return (
    <aside className="md:w-1/3 md:max-w-xs border border-gray-800 rounded-lg bg-gray-950 overflow-hidden">
      <div className="px-3 py-2 border-b border-gray-800 text-xs uppercase tracking-wide text-gray-500">
        Workspaces
      </div>
      {loading ? (
        <p className="text-sm text-gray-500 p-3">Loading…</p>
      ) : workspaces.length === 0 ? (
        <p className="text-sm text-gray-500 p-3">No workspaces found.</p>
      ) : (
        <ul className="divide-y divide-gray-800">
          {workspaces.map((w) => {
            const isSelected = selected === w.name;
            return (
              <li key={w.name}>
                <button
                  type="button"
                  onClick={() => onSelect(w.name)}
                  className={`w-full flex items-center justify-between gap-2 px-3 py-2 text-left text-sm cursor-pointer transition-colors ${
                    isSelected
                      ? "bg-blue-950/40 text-blue-200"
                      : "text-gray-300 hover:bg-gray-900"
                  }`}
                >
                  <span className="truncate font-medium">{w.name}</span>
                  <span className="flex items-center gap-1.5 shrink-0">
                    {w.has_my_commands ? (
                      <span
                        className="inline-flex items-center gap-1 text-[11px] text-emerald-300"
                        title="You have commands set for this workspace"
                      >
                        <span className="w-2 h-2 rounded-full bg-emerald-400" />
                        set
                      </span>
                    ) : (
                      <span
                        className="inline-flex items-center gap-1 text-[11px] text-gray-500"
                        title="No commands set for this workspace"
                      >
                        <span className="w-2 h-2 rounded-full bg-gray-600" />
                        none
                      </span>
                    )}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
