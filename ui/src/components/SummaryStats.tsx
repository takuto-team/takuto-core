// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowCounts } from "../api/types";
import type { StatusFilterKey } from "./statusFilter";

interface Props {
  counts: WorkflowCounts;
  /** The currently-applied status filter, or `null` when showing all. */
  activeFilter?: StatusFilterKey | null;
  /** Toggle a status filter. Clicking the active card clears it (passes null). */
  onSelectFilter?: (key: StatusFilterKey | null) => void;
}

const STATS: { key: StatusFilterKey; label: string; color: string; ring: string }[] = [
  { key: "running", label: "Running", color: "text-blue-400", ring: "border-blue-500 ring-1 ring-blue-500/60" },
  { key: "completed", label: "Completed", color: "text-green-400", ring: "border-green-500 ring-1 ring-green-500/60" },
  { key: "errors", label: "Errors", color: "text-red-400", ring: "border-red-500 ring-1 ring-red-500/60" },
  { key: "paused", label: "Paused", color: "text-yellow-400", ring: "border-yellow-500 ring-1 ring-yellow-500/60" },
];

export function SummaryStats({ counts, activeFilter = null, onSelectFilter }: Props) {
  const valueFor = (key: StatusFilterKey): number =>
    key === "running"
      ? counts.running
      : key === "completed"
        ? counts.completed
        : key === "errors"
          ? counts.errors
          : counts.paused;

  return (
    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
      {STATS.map((s) => {
        const active = activeFilter === s.key;
        return (
          <button
            key={s.key}
            type="button"
            aria-pressed={active}
            onClick={() => onSelectFilter?.(active ? null : s.key)}
            className={`rounded-lg px-4 py-3 text-center transition-colors cursor-pointer border ${
              active
                ? `bg-gray-800/80 ${s.ring}`
                : "bg-gray-900/60 border-gray-800 hover:bg-gray-800/60 hover:border-gray-700"
            }`}
          >
            <div className="text-xs text-gray-500 mb-1">{s.label}</div>
            <div className={`text-2xl font-bold tabular-nums ${s.color}`}>{valueFor(s.key)}</div>
          </button>
        );
      })}
    </div>
  );
}
