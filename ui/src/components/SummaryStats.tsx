// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowCounts } from "../api/types";

interface Props {
  counts: WorkflowCounts;
}

export function SummaryStats({ counts }: Props) {
  const stats = [
    { label: "Running", value: counts.running, color: "text-blue-400" },
    { label: "Completed", value: counts.completed, color: "text-green-400" },
    { label: "Errors", value: counts.errors, color: "text-red-400" },
    { label: "Paused", value: counts.paused, color: "text-yellow-400" },
  ];

  return (
    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
      {stats.map((s) => (
        <div
          key={s.label}
          className="bg-gray-900/60 border border-gray-800 rounded-lg px-4 py-3 text-center"
        >
          <div className="text-xs text-gray-500 mb-1">{s.label}</div>
          <div className={`text-2xl font-bold tabular-nums ${s.color}`}>{s.value}</div>
        </div>
      ))}
    </div>
  );
}
