// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useTranslation } from "react-i18next";
import type { WorkflowCounts } from "../api/types";
import type { StatusFilterKey } from "./statusFilter";

interface Props {
  counts: WorkflowCounts;
  /** The currently-applied status filter, or `null` when showing all. */
  activeFilter?: StatusFilterKey | null;
  /** Toggle a status filter. Clicking the active card clears it (passes null). */
  onSelectFilter?: (key: StatusFilterKey | null) => void;
}

const STATS: { key: StatusFilterKey; labelKey: string; color: string; ring: string }[] = [
  { key: "pending", labelKey: "stats.pending", color: "text-gray-400", ring: "border-gray-500 ring-1 ring-gray-500/60" },
  { key: "running", labelKey: "stats.running", color: "text-blue-400", ring: "border-blue-500 ring-1 ring-blue-500/60" },
  { key: "completed", labelKey: "stats.completed", color: "text-green-400", ring: "border-green-500 ring-1 ring-green-500/60" },
  { key: "errors", labelKey: "stats.errors", color: "text-red-400", ring: "border-red-500 ring-1 ring-red-500/60" },
  { key: "paused", labelKey: "stats.paused", color: "text-yellow-400", ring: "border-yellow-500 ring-1 ring-yellow-500/60" },
];

export function SummaryStats({ counts, activeFilter = null, onSelectFilter }: Props) {
  const { t } = useTranslation("dashboard");
  const valueFor = (key: StatusFilterKey): number => counts[key];

  return (
    <div className="grid grid-cols-2 sm:grid-cols-5 gap-3">
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
            <div className="text-xs text-gray-500 mb-1">{t(s.labelKey)}</div>
            <div className={`text-2xl font-bold tabular-nums ${s.color}`}>{valueFor(s.key)}</div>
          </button>
        );
      })}
    </div>
  );
}
