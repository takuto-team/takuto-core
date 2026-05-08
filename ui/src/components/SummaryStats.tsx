// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { WorkflowSummary } from "../api/types";

interface Props {
  workflows: WorkflowSummary[];
  repoName?: string | null;
  repoHtmlUrl?: string | null;
}

function getStatusLabel(state: string): string {
  const s = state.toLowerCase();
  if (s === "done") return "Completed";
  if (s.startsWith("error")) return "Error";
  if (s === "stopped") return "Stopped";
  if (s.startsWith("paused")) return "Paused";
  return "Running";
}

export function SummaryStats({ workflows, repoName, repoHtmlUrl }: Props) {
  let running = 0, completed = 0, errors = 0, paused = 0;
  for (const w of workflows) {
    const label = getStatusLabel(w.state);
    if (label === "Running") running++;
    else if (label === "Completed") completed++;
    else if (label === "Error" || label === "Stopped") errors++;
    else if (label === "Paused") paused++;
  }

  const stats = [
    { label: "Running", value: running, color: "text-blue-400" },
    { label: "Completed", value: completed, color: "text-green-400" },
    { label: "Errors", value: errors, color: "text-red-400" },
    { label: "Paused", value: paused, color: "text-yellow-400" },
  ];

  return (
    <div className="grid grid-cols-2 sm:grid-cols-5 gap-3">
      {repoName && (
        <div className="px-4 py-3 flex flex-col justify-center">
          <div className="text-xs text-gray-500 mb-1">Repository</div>
          {repoHtmlUrl ? (
            <a
              href={repoHtmlUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="text-sm font-semibold text-blue-400 hover:text-blue-300 transition-colors break-all"
            >
              {repoName}
            </a>
          ) : (
            <div className="text-sm font-semibold text-gray-300 break-all">{repoName}</div>
          )}
        </div>
      )}
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
